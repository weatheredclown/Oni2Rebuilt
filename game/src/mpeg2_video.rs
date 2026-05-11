/*
 * mpeg2_video.rs — Bevy plugin for in-engine MPEG-2 video playback.
 *
 * Decodes a `.m2v` (or PES-wrapped `.mpg`) on a background thread via the
 * pure-Rust `mpeg2_player_v2` crate, ring-buffers a few frames ahead, and
 * uploads each frame's Y/Cb/Cr planes as three R8Unorm textures.  A 2D
 * fullscreen quad samples them with a custom `Material2d` whose fragment
 * shader does the BT.601 YCbCr → RGB matrix multiply on the GPU.
 *
 * Architecture choices:
 *
 *   • Worker thread for decode.  The decoder pushes into a bounded
 *     channel (depth = `RING_DEPTH`) so backpressure handles the
 *     uncommon "GPU is slower than decode" case naturally — the worker
 *     blocks on send until the main thread consumes.  Decode does NOT
 *     run on the Bevy render thread, so a tough P/B frame won't drop
 *     the framerate of the surrounding scene.
 *
 *   • Shader-side YCbCr→RGB.  Three R8Unorm plane textures (Y at full
 *     res, Cb/Cr at half) get sampled and combined in the fragment
 *     shader, saving the CPU a ~7 MB/s YUV→RGBA conversion per frame
 *     and a 4× larger texture upload.  Chroma is sampled with
 *     `Linear` filtering which gives a passable bilinear upsample.
 *
 *   • Spec-rate playback.  Frames are pulled at the source frame rate
 *     (29.97 fps for NTSC) regardless of the engine's render rate.  If
 *     the engine renders at 240 Hz, we still emit one new frame every
 *     ~33 ms and re-sample the same texture between video frames.
 *
 * Use:
 *
 *   app.add_plugins(Mpeg2VideoPlugin);
 *   commands.trigger(PlayMpeg2Video { path: "rockstarlogo.m2v".into() });
 *
 * The plugin spawns a single `Camera2d` if none exists, places a quad
 * with the video material in front of it, and despawns the quad +
 * playback state when the stream ends.  Listen for
 * `Mpeg2VideoFinishedMessage` to chain into the next cinematic / game
 * state transition.
 */

use bevy::asset::{RenderAssetUsages, weak_handle};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dPlugin};
use crossbeam_channel::{Receiver, TryRecvError, bounded};
use mpeg2_player_v2::{FramePlanes, Mpeg2Player, VideoFrame};
use std::path::PathBuf;
use std::thread;

/// Number of decoded frames the worker may queue ahead of the display.
/// Larger smooths over decode-time variance; smaller saves a few MB of
/// memory.  3 is enough to hide one slow P/B frame's decode jitter.
const RING_DEPTH: usize = 3;

/// Weak handle for the YCbCr → RGB fragment shader.
const VIDEO_SHADER_HANDLE: Handle<Shader> = weak_handle!("bb84a701-9d2f-4c46-a4a2-9b54f29c1fce");

/// BT.601 (full-range) YCbCr → RGB.  Matches the conventions of
/// `mpeg2_player_v2::colorspace::yuv420p_to_rgba8` when called with
/// `ChromaUpsample::Bilinear`.  Cb/Cr are stored at half res in R8Unorm;
/// the shader sampler upsamples with bilinear interpolation when the
/// texture's `sampler_descriptor` requests `FilterMode::Linear`.
const VIDEO_SHADER_SRC: &str = r#"
#import bevy_sprite::mesh2d_vertex_output::VertexOutput

@group(2) @binding(0) var y_tex:  texture_2d<f32>;
@group(2) @binding(1) var y_samp: sampler;
@group(2) @binding(2) var cb_tex: texture_2d<f32>;
@group(2) @binding(3) var cb_samp: sampler;
@group(2) @binding(4) var cr_tex: texture_2d<f32>;
@group(2) @binding(5) var cr_samp: sampler;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let uv = mesh.uv;
    let y  = textureSample(y_tex,  y_samp,  uv).r;
    let cb = textureSample(cb_tex, cb_samp, uv).r - 0.5;
    let cr = textureSample(cr_tex, cr_samp, uv).r - 0.5;
    let r = y + 1.402   * cr;
    let g = y - 0.344136 * cb - 0.714136 * cr;
    let b = y + 1.772    * cb;
    return vec4<f32>(clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

/// Custom 2D material with three planar texture bindings + samplers.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct Mpeg2VideoMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub y_tex: Handle<Image>,
    #[texture(2)]
    #[sampler(3)]
    pub cb_tex: Handle<Image>,
    #[texture(4)]
    #[sampler(5)]
    pub cr_tex: Handle<Image>,
}

impl Material2d for Mpeg2VideoMaterial {
    fn fragment_shader() -> ShaderRef {
        VIDEO_SHADER_HANDLE.into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Opaque
    }
}

/// Trigger event: start playing the video at `path`.  Replaces any
/// already-playing video on the same `World`.
#[derive(Message, Debug, Clone)]
pub struct PlayMpeg2Video {
    pub path: PathBuf,
}

/// Emitted once on the tick after the stream ends.  Use to chain into
/// the next game state.
#[derive(Message, Debug, Clone, Copy)]
pub struct Mpeg2VideoFinishedMessage;

/// Per-active-video state.  Owns the receiver end of the decode channel,
/// the three plane textures, and a frame-pacing accumulator.
#[derive(Component)]
pub struct Mpeg2VideoPlayback {
    rx: Receiver<VideoFrame>,
    y_tex: Handle<Image>,
    cb_tex: Handle<Image>,
    cr_tex: Handle<Image>,
    /// Source frame interval in seconds (1.0 / fps).
    frame_period: f64,
    /// Time accumulated since the last frame was uploaded.  When this
    /// exceeds `frame_period` we pull the next frame.
    accum: f64,
    finished: bool,
}

pub struct Mpeg2VideoPlugin;

impl Plugin for Mpeg2VideoPlugin {
    fn build(&self, app: &mut App) {
        // Insert the shader into the asset store under our weak handle so
        // `Material2d::fragment_shader()` can resolve it without disk I/O.
        let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
        let _ = shaders.insert(
            &VIDEO_SHADER_HANDLE,
            Shader::from_wgsl(VIDEO_SHADER_SRC, "mpeg2_video.wgsl"),
        );

        app.add_plugins(Material2dPlugin::<Mpeg2VideoMaterial>::default())
            .add_message::<PlayMpeg2Video>()
            .add_message::<Mpeg2VideoFinishedMessage>()
            .add_systems(Update, (start_playback, advance_playback));
    }
}

fn start_playback(
    mut events: MessageReader<PlayMpeg2Video>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<Mpeg2VideoMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    existing_cameras: Query<(), With<Camera2d>>,
    existing_playback: Query<Entity, With<Mpeg2VideoPlayback>>,
) {
    for ev in events.read() {
        // Open + parse on the main thread so we know the dimensions and
        // frame rate up front; only the slice-by-slice decode loop runs
        // on the worker.
        let player = match Mpeg2Player::open(&ev.path) {
            Ok(p) => p,
            Err(err) => {
                error!("mpeg2_video: failed to open {:?}: {err}", ev.path);
                continue;
            }
        };
        let info = player.info().clone();
        let width = info.width;
        let height = info.height;
        let c_w = width.div_ceil(2);
        let c_h = height.div_ceil(2);
        let frame_period = if info.frame_rate_num > 0 {
            f64::from(info.frame_rate_den) / f64::from(info.frame_rate_num)
        } else {
            1.0 / 30.0
        };
        info!(
            "mpeg2_video: starting {}x{} @ {:.3} fps ({:?})",
            width,
            height,
            1.0 / frame_period,
            ev.path
        );

        // Despawn any prior playback so triggering twice replaces cleanly.
        for entity in &existing_playback {
            commands.entity(entity).despawn();
        }

        let y_tex = images.add(plane_image(width, height));
        let cb_tex = images.add(plane_image(c_w, c_h));
        let cr_tex = images.add(plane_image(c_w, c_h));

        let (tx, rx) = bounded::<VideoFrame>(RING_DEPTH);
        thread::Builder::new()
            .name("mpeg2_video_decode".into())
            .spawn(move || {
                let mut player = player;
                loop {
                    match player.next_frame() {
                        Ok(Some(frame)) => {
                            if tx.send(frame).is_err() {
                                // Display dropped — playback was cancelled.
                                return;
                            }
                        }
                        Ok(None) => return,
                        Err(err) => {
                            error!("mpeg2_video: decode error: {err}");
                            return;
                        }
                    }
                }
            })
            .expect("spawn mpeg2_video decode thread");

        let material = materials.add(Mpeg2VideoMaterial {
            y_tex: y_tex.clone(),
            cb_tex: cb_tex.clone(),
            cr_tex: cr_tex.clone(),
        });
        let quad = meshes.add(Rectangle::new(width as f32, height as f32));

        commands.spawn((
            Mesh2d(quad),
            MeshMaterial2d(material),
            Transform::default(),
            Mpeg2VideoPlayback {
                rx,
                y_tex,
                cb_tex,
                cr_tex,
                frame_period,
                accum: frame_period, // present the first frame immediately
                finished: false,
            },
            Name::new(format!("Mpeg2Video[{}]", ev.path.display())),
        ));

        if existing_cameras.is_empty() {
            commands.spawn((Camera2d, Name::new("Mpeg2VideoCamera")));
        }
    }
}

fn advance_playback(
    time: Res<Time>,
    mut images: ResMut<Assets<Image>>,
    mut query: Query<(Entity, &mut Mpeg2VideoPlayback)>,
    mut finished: MessageWriter<Mpeg2VideoFinishedMessage>,
    mut commands: Commands,
) {
    let dt = time.delta_secs_f64();
    for (entity, mut playback) in &mut query {
        if playback.finished {
            continue;
        }
        playback.accum += dt;
        // Try to upload at most one frame per system run.  If the channel
        // is empty we just leave the existing texture data on screen —
        // standard "hold last frame" behavior when the decoder is behind.
        if playback.accum < playback.frame_period {
            continue;
        }
        match playback.rx.try_recv() {
            Ok(frame) => {
                playback.accum -= playback.frame_period;
                upload_frame(&mut images, &playback, frame);
            }
            Err(TryRecvError::Empty) => {
                // Decoder is behind; cap the accumulator so we don't
                // burst-emit a backlog the moment frames arrive.
                playback.accum = playback.frame_period;
            }
            Err(TryRecvError::Disconnected) => {
                playback.finished = true;
                finished.write(Mpeg2VideoFinishedMessage);
                commands.entity(entity).despawn();
            }
        }
    }
}

fn upload_frame(images: &mut Assets<Image>, playback: &Mpeg2VideoPlayback, frame: VideoFrame) {
    let FramePlanes::Yuv420p { y, cb, cr } = frame.planes else {
        // RGBA-mode frames aren't expected from this decoder configuration.
        // The default Mpeg2Player output is YUV420P; if that changes we'd
        // need to either reformat here or extend the material to sample
        // an RGBA texture instead.
        warn!("mpeg2_video: ignoring non-YUV frame variant");
        return;
    };
    overwrite_plane(images, &playback.y_tex, y);
    overwrite_plane(images, &playback.cb_tex, cb);
    overwrite_plane(images, &playback.cr_tex, cr);
}

fn overwrite_plane(images: &mut Assets<Image>, handle: &Handle<Image>, bytes: Vec<u8>) {
    if let Some(img) = images.get_mut(handle) {
        img.data = Some(bytes);
    }
}

fn plane_image(width: u32, height: u32) -> Image {
    // R8Unorm matches the decoder's 8-bit Y/Cb/Cr samples.  The default
    // sampler (`linear` filter) gives bilinear chroma upsampling for free
    // when the half-res Cb/Cr textures are sampled by the full-res quad.
    Image::new_fill(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[128u8],
        TextureFormat::R8Unorm,
        RenderAssetUsages::default(),
    )
}
