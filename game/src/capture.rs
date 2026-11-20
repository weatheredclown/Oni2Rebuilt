//! Timed in-game screenshots for engine-vs-engine comparison runs
//! (`tools/sizzle_compare.sh`).
//!
//! `RB_CAPTURE="2,4.5,8"` saves the primary window at those seconds of
//! in-game time — counted from the first `AppState::InGame` frame, the same
//! base as rb's `-recordat` ("game seconds after the first 3D frame") — to
//! `$RB_CAPTURE_DIR/rs_<secs>.png` (default dir: the cwd).  After the last
//! shot the app exits, so a batch run needs no wall-clock kill.
//!
//! Unset `RB_CAPTURE` → the plugin does nothing.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use crate::menu::AppState;

pub struct CapturePlugin;

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        let Some(times) = parse_times(std::env::var("RB_CAPTURE").ok().as_deref()) else {
            return;
        };
        let dir = std::env::var("RB_CAPTURE_DIR").unwrap_or_else(|_| ".".into());
        // Capture runs are launched in the background, so the window never
        // has focus; main.rs's unfocused mode (reactive low power, 500 ms)
        // would pin them at 2 fps.  Render continuously regardless.
        app.insert_resource(bevy::winit::WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Continuous,
            unfocused_mode: bevy::winit::UpdateMode::Continuous,
        })
        .insert_resource(CaptureSchedule {
            pending: times,
            dir,
            in_game_secs: 0.0,
            exit_in_frames: None,
        })
        .add_systems(Update, timed_capture.run_if(in_state(AppState::InGame)));
    }
}

#[derive(Resource)]
struct CaptureSchedule {
    /// Remaining capture times (seconds of in-game time), ascending.
    pending: Vec<f32>,
    dir: String,
    in_game_secs: f32,
    /// Frames to wait after the last screenshot so its async readback lands
    /// on disk before exit.
    exit_in_frames: Option<u32>,
}

fn parse_times(spec: Option<&str>) -> Option<Vec<f32>> {
    let mut times: Vec<f32> = spec?
        .split(',')
        .filter_map(|t| t.trim().parse().ok())
        .collect();
    if times.is_empty() {
        return None;
    }
    times.sort_by(f32::total_cmp);
    Some(times)
}

fn timed_capture(
    mut commands: Commands,
    time: Res<Time>,
    mut sched: ResMut<CaptureSchedule>,
    mut exit: MessageWriter<AppExit>,
) {
    if let Some(n) = sched.exit_in_frames.as_mut() {
        if *n == 0 {
            exit.write(AppExit::Success);
        } else {
            *n -= 1;
        }
        return;
    }
    sched.in_game_secs += time.delta_secs();
    let now = sched.in_game_secs;
    while sched.pending.first().is_some_and(|&t| now >= t) {
        let t = sched.pending.remove(0);
        let path = format!("{}/rs_{:06.2}.png", sched.dir, t);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if sched.pending.is_empty() {
        sched.exit_in_frames = Some(30);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_times;

    #[test]
    fn parses_and_sorts_capture_times() {
        assert_eq!(parse_times(Some("8, 2,4.5,x")), Some(vec![2.0, 4.5, 8.0]));
        assert_eq!(parse_times(Some("")), None);
        assert_eq!(parse_times(None), None);
    }
}
