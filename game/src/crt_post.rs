/*
 * crt_post.rs — PS2-CRT-look post-process pass.
 *
 * Runs after tonemapping in the Core3d / Core2d render graphs.  Reads
 * the post-tonemap view target, applies black-level crush + gamma
 * curve + saturation lift, writes back into the same ping-pong target
 * before the upscaling node hands off to the swapchain.
 *
 * The intent is to compensate for the difference between a PS2 game
 * authored on a CRT (deeper blacks, ~2.4–2.5 effective gamma, narrower
 * but more saturated phosphor primaries) and a modern sRGB-displayed
 * panel (lifted blacks, 2.2 gamma, wider color gamut).  No scanlines /
 * mask / barrel — pure color/contrast adjustment per the user's
 * request.
 *
 * Tunables live on a `CrtSettings` component per camera.  Cameras
 * without the component are untouched.  Defaults are conservative;
 * tweak per-camera or change the constants in `CrtSettings::ps2_crt`.
 */
use bevy::asset::weak_handle;
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::core_2d::graph::{Core2d, Node2d};
use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::{
    Render, RenderApp, RenderSystems,
    extract_component::{
        ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
        UniformComponentPlugin,
    },
    render_graph::{
        NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
    },
    render_resource::{
        BindGroupEntries, BindGroupLayoutDescriptor, CachedRenderPipelineId, ColorTargetState,
        ColorWrites, DynamicBindGroupLayoutEntries, FragmentState, LoadOp, MultisampleState,
        Operations, PipelineCache, PrimitiveState, RenderPassColorAttachment, RenderPassDescriptor,
        RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
        ShaderType, StoreOp, TextureFormat, TextureSampleType,
        binding_types::{sampler, texture_2d, uniform_buffer},
    },
    renderer::{RenderContext, RenderDevice},
    view::ViewTarget,
};
use bevy::shader::Shader;

const CRT_SHADER_HANDLE: Handle<Shader> = weak_handle!("a4b6c8de-1f23-4567-89ab-cdef01234567");

const CRT_SHADER_SRC: &str = r#"
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct CrtSettings {
    /// Display gamma curve.  <1 BRIGHTENS midtones, >1 darkens.
    /// PS2-vivid-but-bright target: ~0.78–0.88.  Lower = brighter
    /// mids, but flattens contrast.
    gamma: f32,
    /// Bottom-of-range crush.  Values < this get pulled toward zero
    /// via a smoothstep, giving CRT-like deep blacks.  Independent
    /// of gamma — applies BEFORE the gamma lift so darks stay
    /// crushed even when gamma < 1 brightens everything else.
    /// Sensible range 0.0–0.08.
    black_crush: f32,
    /// Saturation multiplier.  1.0 = unchanged, >1 boosts.
    saturation: f32,
    /// Strength of the "high-mid bias" lift, blended on top of the
    /// gamma curve.  The bias curve is `1 - (1-x)^2`, which holds
    /// 0 → 0 and 1 → 1 but pushes mid values aggressively upward
    /// (e.g. 0.5 → 0.75, 0.7 → 0.91).  0.0 = pure gamma curve,
    /// 1.0 = full bias.  ~0.25–0.40 is the "punchy but not blown
    /// out" range.
    high_mid_bias: f32,
};

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
@group(0) @binding(2) var<uniform> settings: CrtSettings;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let raw = textureSample(screen_texture, screen_sampler, in.uv);

    // 1. Black crush — smoothstep maps [black_crush, 1.0] to [0, 1].
    //    Deep darks get pushed to true black; lifts the rest.
    let crushed = smoothstep(
        vec3<f32>(settings.black_crush),
        vec3<f32>(1.0),
        raw.rgb,
    );

    // 2. Gamma curve.  With gamma < 1, this brightens — strongest
    //    around x=0.4, tapering off near both endpoints.  Holds 0
    //    and 1 fixed, so true blacks (post-crush) stay black.
    let gamma_lifted = pow(max(crushed, vec3<f32>(0.0)), vec3<f32>(settings.gamma));

    // 3. High-mid bias.  `1 - (1-x)^2` is a parabolic curve that
    //    holds 0 and 1 fixed but pushes the mid range hard upward
    //    (0.5 → 0.75, 0.7 → 0.91).  Blending between the gamma curve
    //    and this gives us extra brightness in the high-mid band
    //    without blowing out highlights or lifting blacks.
    let one_minus = vec3<f32>(1.0) - gamma_lifted;
    let upper_lifted = vec3<f32>(1.0) - one_minus * one_minus;
    let blended = mix(gamma_lifted, upper_lifted, settings.high_mid_bias);

    // 4. Saturation around per-pixel luma (Rec.709 weights).
    let luma = dot(blended, vec3<f32>(0.2126, 0.7152, 0.0722));
    let saturated = mix(vec3<f32>(luma), blended, settings.saturation);

    return vec4<f32>(saturated, raw.a);
}
"#;

/// Per-camera knobs.  Add this component to any camera you want the
/// CRT pass to run on.  Cameras without this component are untouched.
#[derive(Component, Clone, Copy, ExtractComponent, ShaderType)]
pub struct CrtSettings {
    /// <1 brightens midtones, >1 darkens.
    pub gamma: f32,
    /// 0–0.08; deep blacks below this get pushed to true black.
    pub black_crush: f32,
    /// 1.0 = unchanged, >1 boosts color punch.
    pub saturation: f32,
    /// 0–1; strength of the "high-mid lift" parabolic curve blended
    /// on top of the gamma result.
    pub high_mid_bias: f32,
}

impl Default for CrtSettings {
    fn default() -> Self {
        Self::ps2_crt()
    }
}

impl CrtSettings {
    /// "Bright + vivid + deep blacks" — keeps the saturation and
    /// black crush from the original PS2-CRT preset, but flips the
    /// gamma to brighten midtones (was 1.10 = darken) and adds a
    /// high-mid bias curve so the upper midrange picks up extra
    /// punch without blowing out highlights or lifting blacks.
    /// Tweak from here.
    pub fn ps2_crt() -> Self {
        Self {
            // Lower gamma from 0.82 to 0.75. This lifts the shadows and midtones
            // significantly, making dark rooms like the blast chamber visible again.
            gamma: 0.75,
            // 0.04 was too aggressive with smoothstep, swallowing all low-light
            // detail into pure black. 0.015 keeps the deep black floor but preserves detail.
            black_crush: 0.015,
            saturation: 1.15,
            // Tone down the high-mid bias slightly from 0.30 so we don't blow out
            // the bright levels now that the base gamma is brighter.
            high_mid_bias: 0.20,
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct CrtPostLabel;

pub struct CrtPostPlugin;

impl Plugin for CrtPostPlugin {
    fn build(&self, app: &mut App) {
        // Register the embedded WGSL as a shader asset so the pipeline
        // can reference it by handle.
        app.world_mut().resource_mut::<Assets<Shader>>().insert(
            &CRT_SHADER_HANDLE,
            Shader::from_wgsl(CRT_SHADER_SRC, "crt_post.wgsl"),
        );

        app.add_plugins((
            ExtractComponentPlugin::<CrtSettings>::default(),
            UniformComponentPlugin::<CrtSettings>::default(),
        ));

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .add_systems(Render, init_crt_pipeline.in_set(RenderSystems::Prepare))
            // Core3d: Tonemapping → CrtPost → EndMainPassPostProcessing.
            .add_render_graph_node::<ViewNodeRunner<CrtNode>>(Core3d, CrtPostLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    Node3d::Tonemapping,
                    CrtPostLabel,
                    Node3d::EndMainPassPostProcessing,
                ),
            )
            // Core2d: same insertion point, mirrored from the 3D
            // graph so the menu's Camera2d gets the same treatment as
            // the in-game Camera3d.
            .add_render_graph_node::<ViewNodeRunner<CrtNode>>(Core2d, CrtPostLabel)
            .add_render_graph_edges(
                Core2d,
                (
                    Node2d::Tonemapping,
                    CrtPostLabel,
                    Node2d::EndMainPassPostProcessing,
                ),
            );
    }
}

#[derive(Resource)]
struct CrtPipeline {
    /// Descriptor handed to the pipeline-cache.  Resolved to a real
    /// `BindGroupLayout` via `pipeline_cache.get_bind_group_layout`
    /// inside the render node when we build the bind group, mirroring
    /// the pattern used by `TonemappingPipeline`.
    layout_desc: BindGroupLayoutDescriptor,
    sampler: Sampler,
    /// Pipeline targeting the HDR-format ping-pong buffer (cameras
    /// with `Camera { hdr: true, .. }`).  Format = `Rgba16Float`.
    pipeline_hdr: CachedRenderPipelineId,
    /// Pipeline targeting the LDR / sRGB-format ping-pong buffer
    /// (default `Camera3d` / `Camera2d` without HDR).  Format =
    /// `TextureFormat::bevy_default()` — typically `Bgra8UnormSrgb`.
    pipeline_ldr: CachedRenderPipelineId,
    /// Format the LDR pipeline was built for, captured here so the
    /// node can match `view_target.main_texture_format()` against it
    /// at run time and bail cleanly if Bevy's default ever changes.
    ldr_format: TextureFormat,
}

/// One-shot init system that builds the bind group layout descriptor,
/// sampler, and queues both render pipelines (HDR + LDR).  Idempotent
/// — only inserts on the first frame.
fn init_crt_pipeline(
    mut commands: Commands,
    pipeline: Option<Res<CrtPipeline>>,
    render_device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    fullscreen_shader: Res<FullscreenShader>,
) {
    if pipeline.is_some() {
        return;
    }

    let entries = DynamicBindGroupLayoutEntries::sequential(
        ShaderStages::FRAGMENT,
        (
            texture_2d(TextureSampleType::Float { filterable: true }),
            sampler(SamplerBindingType::Filtering),
            uniform_buffer::<CrtSettings>(true),
        ),
    );
    let layout_desc = BindGroupLayoutDescriptor::new("crt_post_bind_group_layout", &entries);

    let sampler = render_device.create_sampler(&SamplerDescriptor::default());

    let ldr_format = TextureFormat::bevy_default();
    let make_pipeline = |label: &'static str, target_format: TextureFormat| {
        pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout_desc.clone()],
            vertex: fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: CRT_SHADER_HANDLE.clone(),
                shader_defs: vec![],
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            push_constant_ranges: vec![],
            zero_initialize_workgroup_memory: false,
        })
    };
    let pipeline_hdr = make_pipeline("crt_post_pipeline_hdr", ViewTarget::TEXTURE_FORMAT_HDR);
    let pipeline_ldr = make_pipeline("crt_post_pipeline_ldr", ldr_format);
    drop(make_pipeline);

    commands.insert_resource(CrtPipeline {
        layout_desc,
        sampler,
        pipeline_hdr,
        pipeline_ldr,
        ldr_format,
    });
}

#[derive(Default)]
struct CrtNode;

impl ViewNode for CrtNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static DynamicUniformIndex<CrtSettings>,
    );

    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        (view_target, uniform_index): QueryItem<Self::ViewQuery>,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let Some(pipeline) = world.get_resource::<CrtPipeline>() else {
            return Ok(());
        };
        let pipeline_cache = world.resource::<PipelineCache>();

        // Pick the right pipeline by target format.  HDR cameras
        // (Camera { hdr: true, .. }) ping-pong via Rgba16Float; non-
        // HDR cameras use whatever Bevy's default LDR format is
        // (typically Bgra8UnormSrgb).  Format mismatch crashes wgpu
        // at command-encoder finish time, hence the explicit guard.
        let target_format = view_target.main_texture_format();
        let id = if target_format == ViewTarget::TEXTURE_FORMAT_HDR {
            pipeline.pipeline_hdr
        } else if target_format == pipeline.ldr_format {
            pipeline.pipeline_ldr
        } else {
            // Unknown / unsupported format — bail rather than crash.
            return Ok(());
        };
        let Some(render_pipeline) = pipeline_cache.get_render_pipeline(id) else {
            return Ok(());
        };

        let settings_uniforms = world.resource::<ComponentUniforms<CrtSettings>>();
        let Some(settings_binding) = settings_uniforms.uniforms().binding() else {
            return Ok(());
        };

        let post_process = view_target.post_process_write();

        let bind_group = render_context.render_device().create_bind_group(
            Some("crt_post_bind_group"),
            &pipeline_cache.get_bind_group_layout(&pipeline.layout_desc),
            &BindGroupEntries::sequential((
                post_process.source,
                &pipeline.sampler,
                settings_binding.clone(),
            )),
        );

        let mut render_pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("crt_post_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post_process.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        render_pass.set_render_pipeline(render_pipeline);
        render_pass.set_bind_group(0, &bind_group, &[uniform_index.index()]);
        render_pass.draw(0..3, 0..1);

        Ok(())
    }
}
