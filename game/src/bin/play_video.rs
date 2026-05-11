use anyhow::{bail, Context, Result};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use mpeg2_player::{DeinterlaceMode, Mpeg2Player, Mpeg2PlayerOptions, VideoBuffer};
use std::env;
use std::path::PathBuf;

#[derive(Clone)]
struct DecodedFrame {
    rgba: Vec<u8>,
}

#[derive(Resource)]
struct VideoPlayback {
    frames: Vec<DecodedFrame>,
    texture: Handle<Image>,
    frame_time: f32,
    accumulator: f32,
    current: usize,
}

#[derive(Resource)]
struct VideoInfo {
    path: PathBuf,
    width: u32,
    height: u32,
    fps: f32,
    frame_count: usize,
}

fn main() -> Result<()> {
    let config = Config::parse()?;
    let frames = decode_frames(&config.path, config.deinterlace)?;
    let first = frames
        .first()
        .context("video did not produce any decoded frames")?;

    let width = first.width as u32;
    let height = first.height as u32;
    let decoded_frames = frames
        .into_iter()
        .map(|frame| {
            let rgba = frame
                .rgba
                .context("RGBA conversion was requested but the decoder did not return RGBA data")?;
            if frame.width as u32 != width || frame.height as u32 != height {
                bail!(
                    "variable-size frame decoded (first frame {}x{}, later frame {}x{}); this test player expects fixed-size video",
                    width,
                    height,
                    frame.width,
                    frame.height
                );
            }
            Ok(DecodedFrame { rgba })
        })
        .collect::<Result<Vec<_>>>()?;

    let info = VideoInfo {
        path: config.path,
        width,
        height,
        fps: config.fps,
        frame_count: decoded_frames.len(),
    };

    App::new()
        .insert_resource(ClearColor(Color::BLACK))
        .insert_resource(info)
        .insert_resource(DecodedFrames(decoded_frames))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Oni2 MPEG-2 video player".to_string(),
                resolution: (width, height).into(),
                resizable: true,
                ..default()
            }),
            ..default()
        }))
        .add_systems(Startup, setup)
        .add_systems(Update, advance_video)
        .run();

    Ok(())
}

struct Config {
    path: PathBuf,
    fps: f32,
    deinterlace: DeinterlaceMode,
}

impl Config {
    fn parse() -> Result<Self> {
        let mut args = env::args().skip(1);
        let Some(raw_path) = args.next() else {
            eprintln!(
                "Usage: cargo run --release -p rb-game --bin play_video -- <input.m2v|.mpg> [--fps 30] [--deinterlace preserve|bob]"
            );
            std::process::exit(2);
        };

        let mut fps = 30.0;
        let mut deinterlace = DeinterlaceMode::Preserve;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--fps" => {
                    let value = args.next().context("--fps requires a numeric value")?;
                    fps = value
                        .parse::<f32>()
                        .with_context(|| format!("invalid --fps value '{value}'"))?;
                    if fps <= 0.0 {
                        bail!("--fps must be greater than zero");
                    }
                }
                "--deinterlace" => {
                    let value = args
                        .next()
                        .context("--deinterlace requires preserve, weave, or bob")?;
                    deinterlace = DeinterlaceMode::parse(&value)?;
                }
                other => bail!("unknown argument '{other}'"),
            }
        }

        Ok(Self {
            path: normalize_cli_path(raw_path),
            fps,
            deinterlace,
        })
    }
}

fn normalize_cli_path(raw: String) -> PathBuf {
    let path = PathBuf::from(&raw);
    if path.exists() || !raw.contains('\\') {
        return path;
    }

    let normalized = raw.replace('\\', std::path::MAIN_SEPARATOR_STR);
    PathBuf::from(normalized)
}

fn decode_frames(path: &PathBuf, deinterlace: DeinterlaceMode) -> Result<Vec<VideoBuffer>> {
    let options = Mpeg2PlayerOptions {
        deinterlace,
        convert_to_rgba: true,
    };
    Mpeg2Player::decode_file(path, options).with_context(|| {
        format!(
            "failed to decode {}; the bundled MPEG-2 path is still limited, so interlaced/field-coded Oni 2 movies may need a fuller decoder",
            path.display()
        )
    })
}

#[derive(Resource)]
struct DecodedFrames(Vec<DecodedFrame>);

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    decoded: Res<DecodedFrames>,
    info: Res<VideoInfo>,
) {
    let image = Image::new(
        Extent3d {
            width: info.width,
            height: info.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        decoded.0[0].rgba.clone(),
        TextureFormat::Rgba8UnormSrgb,
        default(),
    );
    let texture = images.add(image);

    commands.spawn(Camera2d);
    commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            position_type: PositionType::Relative,
            ..default()
        },
        BackgroundColor(Color::BLACK),
        children![
            (
                ImageNode::new(texture.clone()),
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    position_type: PositionType::Absolute,
                    ..default()
                },
            ),
            (
                Text::new(format!(
                    "{} — {}x{} @ {:.02} fps ({} frame buffers)",
                    info.path.display(),
                    info.width,
                    info.height,
                    info.fps,
                    info.frame_count
                )),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.75)),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(12.0),
                    bottom: Val::Px(12.0),
                    ..default()
                },
            ),
        ],
    ));

    commands.insert_resource(VideoPlayback {
        frames: decoded.0.clone(),
        texture,
        frame_time: 1.0 / info.fps,
        accumulator: 0.0,
        current: 0,
    });
}

fn advance_video(
    time: Res<Time>,
    mut playback: ResMut<VideoPlayback>,
    mut images: ResMut<Assets<Image>>,
) {
    if playback.frames.len() < 2 {
        return;
    }

    playback.accumulator += time.delta_secs();
    if playback.accumulator < playback.frame_time {
        return;
    }

    while playback.accumulator >= playback.frame_time {
        playback.accumulator -= playback.frame_time;
        playback.current = (playback.current + 1) % playback.frames.len();
    }

    let rgba = playback.frames[playback.current].rgba.clone();
    if let Some(image) = images.get_mut(&playback.texture) {
        image.data = Some(rgba);
    }
}
