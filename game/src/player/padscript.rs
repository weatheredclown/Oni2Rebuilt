/*
 * player/padscript.rs — scripted pad input for unattended captures.
 *
 * `--padscript <file>` replays a timed left-stick (+ buttons) script into the
 * player's input as if from a controller — the file format rb.exe's
 * `-padscript` reads (age/src/input/pad.cpp, ioPad::PollHardware; used by
 * tools/loco_harness/moves.pad and the sizzle reel's shots/<shot>.pad):
 *
 *     start dur lx ly [buttons_hex]     # '#' starts a comment
 *
 * Every step covering the current time applies: the stick comes from the
 * last such step in file order, the buttons are OR'd together.  Times are
 * seconds since the player first updates (rb: g_ioPadScriptEpoch, set on the
 * player's first animator update, rb/src/animator/animator.cpp).  The stick
 * uses the AGE convention (+lx = right, +ly = down/back, camera-relative);
 * buttons use the ioPad bit layout (age/src/input/pad.h, `PAD_BUTTONS`).
 *
 * `--padaim <file>` (rb/src/behavior/pad.cpp, sApplyPadAim) points the stick
 * at a named actor, on the same clock:
 *
 *     start dur actor [mag]
 *
 * The first step (file order) whose actor is live wins.  rb tries 72 stick
 * angles through its pad mapper and keeps the one whose world direction
 * points most nearly at the actor; this does the same through the Rust
 * stick → world mapping (camera-relative, see `player_movement_system`),
 * using the game camera (rb: PADDATA.CameraMatrix = the game view, not the
 * `-cinecam` view).  Buttons stay with `--padscript`.
 */
use bevy::prelude::*;

/// ioPad::R1 — PADCMD_WEAPON_FIGHT_MODE / PADCMD_WEAPON_LOCKON.
pub const PAD_R1: u32 = 0x0008;

/// ioPad button bits (age/src/input/pad.h — the original AGE order, which
/// control.map names resolve through) and their control.map names.
pub const PAD_BUTTONS: [(u32, &str); 16] = [
    (0x0001, "L2"),
    (0x0002, "R2"),
    (0x0004, "L1"),
    (0x0008, "R1"),
    (0x0010, "Rup"),
    (0x0020, "Rright"),
    (0x0040, "Rdown"),
    (0x0080, "Rleft"),
    (0x0100, "Select"),
    (0x0200, "L3"),
    (0x0400, "R3"),
    (0x0800, "Start"),
    (0x1000, "Lup"),
    (0x2000, "Lright"),
    (0x4000, "Ldown"),
    (0x8000, "Lleft"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PadStep {
    pub t0: f32,
    pub t1: f32,
    pub lx: f32,
    pub ly: f32,
    pub buttons: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AimStep {
    pub t0: f32,
    pub t1: f32,
    pub actor: String,
    pub mag: f32,
}

/// Leading tokens parsed as floats until the first failure (sscanf `%f`).
fn leading_floats(toks: &[&str], max: usize) -> Vec<f32> {
    toks.iter()
        .take(max)
        .map_while(|s| s.parse::<f32>().ok())
        .collect()
}

/// sscanf("%x"): optional 0x prefix, hex digits up to the first non-digit.
fn scan_hex(s: &str) -> Option<u32> {
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    let end = s.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(s.len());
    u32::from_str_radix(&s[..end], 16).ok()
}

/// Parses a `.pad` file like rb's PollHardware (max 256 steps, n >= 4).
pub fn parse_pad(text: &str) -> Vec<PadStep> {
    let mut steps = Vec::new();
    for line in text.lines() {
        if steps.len() >= 256 {
            break;
        }
        let line = line.split('#').next().unwrap_or("");
        let toks: Vec<&str> = line.split_whitespace().collect();
        let f = leading_floats(&toks, 4);
        if f.len() < 4 {
            continue;
        }
        let buttons = toks.get(4).and_then(|s| scan_hex(s)).unwrap_or(0);
        steps.push(PadStep {
            t0: f[0],
            t1: f[0] + f[1],
            lx: f[2],
            ly: f[3],
            buttons,
        });
    }
    steps
}

/// Parses a `.aim` file like rb's sApplyPadAim (max 256 steps, n >= 3).
pub fn parse_aim(text: &str) -> Vec<AimStep> {
    let mut steps = Vec::new();
    for line in text.lines() {
        if steps.len() >= 256 {
            break;
        }
        let line = line.split('#').next().unwrap_or("");
        let toks: Vec<&str> = line.split_whitespace().collect();
        let f = leading_floats(&toks, 2);
        if f.len() < 2 {
            continue;
        }
        let Some(actor) = toks.get(2) else {
            continue;
        };
        let mag = toks.get(3).and_then(|s| s.parse().ok()).unwrap_or(1.0);
        steps.push(AimStep {
            t0: f[0],
            t1: f[0] + f[1],
            actor: actor.chars().take(47).collect(),
            mag,
        });
    }
    steps
}

/// The pad at script time `t`: (any step covers t, lx, ly, buttons).
pub fn resolve_pad(steps: &[PadStep], t: f32) -> (bool, f32, f32, u32) {
    let (mut on, mut lx, mut ly, mut buttons) = (false, 0.0, 0.0, 0);
    for s in steps {
        if t >= s.t0 && t < s.t1 {
            on = true;
            lx = s.lx;
            ly = s.ly;
            buttons |= s.buttons;
        }
    }
    (on, lx, ly, buttons)
}

/// rb's 72-angle search: the unit stick (AGE convention) whose world travel
/// direction, through the camera's ground-plane basis, points most nearly
/// along `to` (all ground-plane, Bevy space).
pub fn aim_stick(to: Vec3, cam_fwd_xz: Vec3, cam_right_xz: Vec3) -> (f32, f32, f32) {
    let mut best = (-2.0f32, 0.0f32, 0.0f32);
    for k in 0..72 {
        let a = k as f32 * (std::f32::consts::TAU / 72.0);
        let (lx, ly) = (a.sin(), a.cos());
        // player_movement_system: movement = (-lx, ly); travel = fwd*(-ly) + right*lx
        let w = cam_fwd_xz * -ly + cam_right_xz * lx;
        let d = w.x * to.x + w.z * to.z;
        if d > best.0 {
            best = (d, lx, ly);
        }
    }
    (best.1, best.2, best.0)
}

/// Loaded scripts plus this frame's resolved values.
#[derive(Resource, Default)]
pub struct PadScript {
    steps: Vec<PadStep>,
    aim: Vec<AimStep>,
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
        !self.steps.is_empty() || !self.aim.is_empty()
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

    fn read(flag: &str) -> Option<(String, String)> {
        let path = crate::sizzle::arg_value(flag)?;
        match std::fs::read_to_string(&path) {
            Ok(t) => Some((path, t)),
            Err(e) => {
                error!(
                    "[{}] can't open {}: {}",
                    flag.trim_start_matches('-'),
                    path,
                    e
                );
                None
            }
        }
    }

    pub fn load_from_args() -> Self {
        let mut s = Self::default();
        if let Some((path, text)) = Self::read("--padscript") {
            s.steps = parse_pad(&text);
            info!("[padscript] {} steps from {}", s.steps.len(), path);
        }
        if let Some((path, text)) = Self::read("--padaim") {
            s.aim = parse_aim(&text);
            info!("[padaim] {} steps from {}", s.aim.len(), path);
        }
        s
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
    let start = *script.start.get_or_insert_with(|| {
        info!("[padscript] epoch: player's first update");
        now
    });
    let t = (now - start) as f32;
    script.now = t;
    script.prev_buttons = script.buttons;
    let (on, lx, ly, buttons) = resolve_pad(&script.steps, t);
    script.active = on;
    script.lx = lx;
    script.ly = ly;
    script.buttons = buttons;
}

static AIM_LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// `--padaim`: overrides the stick toward this frame's aim actor
/// (sApplyPadAim runs after PollHardware, so it wins over `--padscript`).
pub fn pad_aim_system(
    mut script: ResMut<PadScript>,
    players: Query<&GlobalTransform, With<super::components::Player>>,
    cams: Query<&Transform, With<crate::camera::components::CameraController>>,
    actors: crate::sizzle::ActorLookup,
) {
    if script.aim.is_empty() || script.start.is_none() {
        return;
    }
    let (Some(player), Some(cam)) = (players.iter().next(), cams.iter().next()) else {
        return;
    };
    let fwd = cam.forward().as_vec3();
    let right = cam.right().as_vec3();
    let fwd = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
    let right = Vec3::new(right.x, 0.0, right.z).normalize_or_zero();
    let now = script.now;
    let mut hit = None;
    for st in &script.aim {
        if now < st.t0 || now >= st.t1 {
            continue;
        }
        let Some(g) = actors.named(&st.actor) else {
            continue;
        };
        let mut to = g.translation() - player.translation();
        to.y = 0.0;
        if to.length_squared() < 1e-4 {
            continue;
        }
        let (bx, by, dot) = aim_stick(to.normalize(), fwd, right);
        hit = Some((bx * st.mag, by * st.mag, dot, st.actor.clone()));
        break;
    }
    if let Some((lx, ly, dot, actor)) = hit {
        script.active = true;
        script.lx = lx;
        script.ly = ly;
        if *AIM_LOG.get_or_init(|| crate::sizzle::has_flag("--padaimlog")) {
            info!(
                "[padaim] t {:.2} -> {} stick {:.2} {:.2} (match {:.3})",
                now, actor, lx, ly, dot
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pad_lines_like_sscanf() {
        let s = parse_pad(
            "# comment\n2.00 0.10 0 0 10\n0.6 0.2 0 0 400   # draw\n1 2 0.5 -1\n3 1 0\nx 1 0 0 10\n",
        );
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].buttons, 0x10);
        assert!((s[0].t1 - 2.10).abs() < 1e-6);
        assert_eq!(s[1].buttons, 0x400);
        assert_eq!(s[2].buttons, 0);
        assert_eq!((s[2].lx, s[2].ly), (0.5, -1.0));
    }

    #[test]
    fn overlapping_steps_or_buttons_and_last_stick_wins() {
        let s = parse_pad("0 10 0 -1 8\n2 1 1 0 10\n");
        assert_eq!(resolve_pad(&s, 1.0), (true, 0.0, -1.0, 0x08));
        assert_eq!(resolve_pad(&s, 2.5), (true, 1.0, 0.0, 0x18));
        // end is exclusive
        assert_eq!(resolve_pad(&s, 10.0), (false, 0.0, 0.0, 0));
    }

    #[test]
    fn parses_aim_lines() {
        let a = parse_aim("1.88 0.62 actor_SzBrawler1\n2 1 actor_X 0.5\n3 1\n");
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].actor, "actor_SzBrawler1");
        assert_eq!(a[0].mag, 1.0);
        assert!((a[0].t1 - 2.5).abs() < 1e-6);
        assert_eq!(a[1].mag, 0.5);
    }

    #[test]
    fn aim_stick_points_through_the_camera_basis() {
        // Camera looks down -Z (Bevy default): forward -Z, right +X.
        let (fwd, right) = (Vec3::NEG_Z, Vec3::X);
        // Actor straight ahead of the camera → stick up (ly = -1).
        let (lx, ly, d) = aim_stick(Vec3::NEG_Z, fwd, right);
        assert!(lx.abs() < 1e-5 && (ly + 1.0).abs() < 1e-5 && d > 0.999);
        // Actor to the camera's right → stick right.
        let (lx, ly, _) = aim_stick(Vec3::X, fwd, right);
        assert!((lx - 1.0).abs() < 1e-5 && ly.abs() < 1e-5);
    }
}
