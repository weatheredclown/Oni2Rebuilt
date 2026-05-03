/*
 * env_reflect_material.rs — sphere-mapped environment reflection material.
 *
 * Implements the legacy `texgen reflect` shader directive used by the FX
 * Cathedral statue (Entity/Statue/statueBody_SG.shader pass 1, env tex
 * M4_StatueReflection.tex).  The original game layered an additive
 * environment-map pass on top of the diffuse pass — the reflected eye
 * vector indexes a 2D matcap-style "M4_StatueReflection" texture, which
 * gives the central statue its gold-metallic look without a real cube map.
 *
 * UV formula matches GL_SPHERE_MAP (the de-facto interpretation of fixed-
 * function `texgen=reflect` in the rb engine):
 *     r  = reflect(eye_dir_view, normal_view)
 *     m  = 2 * sqrt(rx² + ry² + (rz + 1)²)
 *     uv = (rx/m + 0.5, ry/m + 0.5)
 *
 * Blend mode is additive so the material composites on top of the diffuse
 * pass that's spawned as a sibling child entity by `oni2_loader::spawn`.
 */
use bevy::asset::uuid_handle;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, CompareFunction, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::{Shader, ShaderRef};

const ENV_REFLECT_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("0c1f3d52-7e9a-4f6b-9d38-5a6c1b2a4e10");

const ENV_REFLECT_SHADER_SRC: &str = r#"
#import bevy_pbr::{
    mesh_functions,
    mesh_view_bindings::view,
}

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
}

struct EnvVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) env_uv: vec2<f32>,
}

@vertex
fn vertex(in: Vertex) -> EnvVertexOutput {
    let world_from_local = mesh_functions::get_world_from_local(in.instance_index);
    let world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(in.position, 1.0),
    );
    let world_normal = mesh_functions::mesh_normal_local_to_world(in.normal, in.instance_index);

    // Eye-space vectors: in view space the camera sits at the origin, so the
    // direction from eye to surface point is just the view-space position.
    let view_pos = (view.view_from_world * world_position).xyz;
    let eye_dir = normalize(view_pos);
    var view_normal = normalize((view.view_from_world * vec4<f32>(world_normal, 0.0)).xyz);

    // The statue mesh is `cull none`, so we also render backfaces whose
    // normals point away from the camera.  Flip those toward the eye so the
    // reflection vector below stays in the camera-facing hemisphere — without
    // this, `r.z` collapses to ~-1 on backfaces and the denominator below
    // goes to zero, producing NaN UVs that sample garbage from the env tex
    // (the bright-orange static symptom).
    if (dot(view_normal, eye_dir) > 0.0) {
        view_normal = -view_normal;
    }

    // Classic GL_SPHERE_MAP — what fixed-function `texgen=reflect` produced
    // on PS2-era hardware.  The reflected vector is mapped onto the unit
    // sphere whose silhouette fills the [0,1]² UV square.  The `max()`
    // floor guards against the residual `r.z ≈ -1` case at extreme grazing
    // angles where the normal-flip above would still leave the denom near
    // zero.
    let r = reflect(eye_dir, view_normal);
    let denom = max(2.0 * sqrt(r.x * r.x + r.y * r.y + (r.z + 1.0) * (r.z + 1.0)), 0.001);
    let uv = vec2<f32>(r.x / denom + 0.5, -r.y / denom + 0.5);

    var out: EnvVertexOutput;
    out.clip_position = view.clip_from_world * world_position;
    out.env_uv = uv;
    return out;
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var env_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var env_sampler: sampler;

@fragment
fn fragment(in: EnvVertexOutput) -> @location(0) vec4<f32> {
    let c = textureSample(env_texture, env_sampler, in.env_uv);
    // Pre-multiplied: alpha = max channel keeps additive blending colored
    // by the env tex without darkening areas where the env is black.
    return vec4<f32>(c.rgb, 1.0);
}
"#;

/// Sphere-mapped environment reflection material.  Bound texture is the
/// matcap-style env tex (e.g. `M4_StatueReflection.tex`); the shader samples
/// it using the reflected eye vector and outputs additive RGB.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct EnvReflectMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub env_texture: Handle<Image>,
}

impl Material for EnvReflectMaterial {
    fn vertex_shader() -> ShaderRef {
        ENV_REFLECT_SHADER_HANDLE.into()
    }
    fn fragment_shader() -> ShaderRef {
        ENV_REFLECT_SHADER_HANDLE.into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        // `blendset add` in the original .shader — composites onto the
        // diffuse sibling pass.
        AlphaMode::Add
    }
    fn enable_prepass() -> bool {
        // The additive env layer is purely a color overlay — it must not
        // contribute to the depth/normal prepass.  Leaving it on causes the
        // prepass to write the same depth as the diffuse sibling pass, after
        // which the forward rendering of both materials does an
        // order-dependent equal-depth comparison and the env layer flickers
        // on/off frame-to-frame as floating-point rounding swaps the winner.
        false
    }
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // Statue meshes are open and authored single-sided in places.  Match
        // the legacy `cull none` directive in the .shader.
        descriptor.primitive.cull_mode = None;
        // Original .shader pass 1 is silent on `depthwrite` (legacy default
        // = off for non-pass-0 layers).  An additive overlay must not stomp
        // depth — otherwise the diffuse sibling and the env pass write
        // equal depths, and the z-fight flickers per frame.
        //
        // We also need `GreaterEqual` for the depth comparison.  Bevy uses
        // reverse-Z, so the transparency pipeline's default `Greater` would
        // reject any pixel at exactly the same depth as the diffuse sibling
        // — and the env mesh IS the diffuse mesh, so the depths are
        // bit-identical apart from FP rounding.  `GreaterEqual` makes the
        // "co-planar overlay" case render every frame instead of flickering
        // per-pixel.
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.depth_write_enabled = false;
            ds.depth_compare = CompareFunction::GreaterEqual;
        }
        Ok(())
    }
}

pub struct EnvReflectMaterialPlugin;

impl Plugin for EnvReflectMaterialPlugin {
    fn build(&self, app: &mut App) {
        let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
        let _ = shaders.insert(
            &ENV_REFLECT_SHADER_HANDLE,
            Shader::from_wgsl(ENV_REFLECT_SHADER_SRC, "env_reflect.wgsl"),
        );
        app.add_plugins(MaterialPlugin::<EnvReflectMaterial>::default());
    }
}
