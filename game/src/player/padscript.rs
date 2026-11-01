/*
 * player/padscript.rs — scripted pad input for unattended locomotion captures.
 *
 * `--padscript <file>` replays a timed left-stick (+ buttons) script into the
 * player's input, the same file format rb.exe's `-padscript` reads
 * (tools/loco_harness/moves.pad):
 *
 *     start dur lx ly [buttons_hex]     # '#' starts a comment
 *
 * Times are seconds since the player first exists.  The stick uses the AGE
 * convention (+lx = right, +ly = down/back, camera-relative); buttons use the
 * ioPad bit layout (R1 = 0x0008 = fight mode / lock-on).
 */
use bevy::prelude::*;

/// ioPad::R1 — PADCMD_WEAPON_FIGHT_MODE / PADCMD_WEAPON_LOCKON.
pub const PAD_R1: u32 = 0x0008;

#[derive(Clone, Copy, Debug)]
struct PadStep {
    t0: f32,
    t1: f32,
    lx: f32,
    ly: f32,
    buttons: u32,
}

/// Loaded script plus this frame's resolved values.
#[derive(Resource, Default)]
pub struct PadScript {
    steps: Vec<PadStep>,
    start: Option<f64>,
    /// Seconds since the script clock started (the player's first frame).
    pub now: f32,
    /// True while a step covers `now` — the script owns the stick this frame.
    pub active: bool,
    /// AGE-convention stick (+x right, +y back).
    pub lx: f32,
    pub ly: f32,
    pub buttons: u32,
    pub prev_buttons: u32,
}

impl PadScript {
    pub fn enabled(&self) -> bool {
        !self.steps.is_empty()
    }
    pub fn held(&self, bit: u32) -> bool {
        self.buttons & bit != 0
    }
    pub fn just_pressed(&self, bit: u32) -> bool {
        self.buttons & bit != 0 && self.prev_buttons & bit == 0
    }
    /// The stick in the Rust `InputState.movement` convention
    /// (W/forward = -y, A/left = +x).
    pub fn movement(&self) -> Vec2 {
        Vec2::new(-self.lx, self.ly)
    }

    pub fn load_from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let Some(path) = args
            .windows(2)
            .find(|w| w[0] == "--padscript")
            .map(|w| w[1].clone())
        else {
            return Self::default();
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                error!("[padscript] can't read {}: {}", path, e);
                return Self::default();
            }
        };
        let mut steps = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("");
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 4 {
                continue;
            }
            let num = |s: &str| s.parse::<f32>().ok();
            let (Some(t0), Some(dur), Some(lx), Some(ly)) = (num(f[0]), num(f[1]), num(f[2]), num(f[3]))
            else {
                continue;
            };
            let buttons = f
                .get(4)
                .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0);
            steps.push(PadStep {
                t0,
                t1: t0 + dur,
                lx,
                ly,
                buttons,
            });
        }
        info!("[padscript] {} steps from {}", steps.len(), path);
        Self {
            steps,
            ..Default::default()
        }
    }
}

/// Resolves the script for this frame.  Runs first in the player input chain.
pub fn pad_script_tick_system(
    time: Res<Time>,
    mut script: ResMut<PadScript>,
    players: Query<(), With<super::components::Player>>,
) {
    if !script.enabled() || players.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    let start = *script.start.get_or_insert(now);
    let t = (now - start) as f32;
    script.now = t;
    script.prev_buttons = script.buttons;
    let step = script.steps.iter().rev().find(|s| t >= s.t0 && t < s.t1).copied();
    match step {
        Some(s) => {
            script.active = true;
            script.lx = s.lx;
            script.ly = s.ly;
            script.buttons = s.buttons;
        }
        None => {
            script.active = false;
            script.lx = 0.0;
            script.ly = 0.0;
            script.buttons = 0;
        }
    }
}
