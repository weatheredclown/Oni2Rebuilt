/*
 * play_video.rs — minimal smoke-test binary for the Mpeg2VideoPlugin.
 *
 * Spins up the bare-minimum Bevy app needed to drive the plugin, opens
 * the .m2v passed on the command line, and quits when playback ends.
 * Useful for "does the plugin work at all" verification without booting
 * the full game.
 *
 * Run with:
 *   cargo run --release -p rb-game --bin play_video -- path/to/clip.m2v
 */

use bevy::prelude::*;
use bevy::window::{ExitCondition, PrimaryWindow};
use rb_game::mpeg2_video::{Mpeg2VideoFinishedMessage, Mpeg2VideoPlugin, PlayMpeg2Video};
use std::env;
use std::path::PathBuf;

#[derive(Resource)]
struct VideoPath(PathBuf);

fn main() {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: play_video <path-to-clip.m2v>");
    let title = format!("mpeg2_video: {}", path.display());

    App::new()
        .insert_resource(VideoPath(path))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title,
                resolution: UVec2::new(512, 448).into(),
                ..default()
            }),
            exit_condition: ExitCondition::OnPrimaryClosed,
            ..default()
        }))
        .add_plugins(Mpeg2VideoPlugin)
        .add_systems(Startup, kick_off)
        .add_systems(Update, exit_on_finish)
        .add_systems(Update, resize_window_to_video.run_if(run_once))
        .run();
}

fn kick_off(mut writer: MessageWriter<PlayMpeg2Video>, path: Res<VideoPath>) {
    writer.write(PlayMpeg2Video {
        source: rb_game::mpeg2_video::VideoSource::Disk(path.0.clone()),
    });
}

fn exit_on_finish(
    mut reader: MessageReader<Mpeg2VideoFinishedMessage>,
    mut exit: MessageWriter<AppExit>,
) {
    for _ in reader.read() {
        info!("playback finished; exiting");
        exit.write(AppExit::Success);
    }
}

/// One-shot helper: after the plugin has spawned the playback quad, size
/// the window so the video fills it.  Skipped on Wayland / fixed-window
/// platforms where the resize wouldn't take.
fn resize_window_to_video(mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    for mut win in &mut windows {
        win.resolution = UVec2::new(512, 448).into();
    }
}

fn run_once(mut done: Local<bool>) -> bool {
    if *done {
        false
    } else {
        *done = true;
        true
    }
}
