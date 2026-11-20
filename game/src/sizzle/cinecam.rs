/*
 * sizzle/cinecam.rs — `--cinecam <file>`: rb's keyframed cinematic camera.
 *
 * Mirrors rb/src/camera_new/manager.cpp (sCineLoad / sCineAnchor /
 * sCineOrbitEye / sCineResolve / sCineEvaluate / sCineApply, the PC-port
 * `-cinecam` added 2026-11-07, spline runs 2026-11-08), so a sizzle-reel
 * shot's `tools/sizzle/shots/<shot>.cine` films the same path in both
 * engines.  One key per line ('#' comments):
 *
 *   <t> <anchor> look  <ex> <ey> <ez>  <tx> <ty> <tz>  [lin|ease|spline|cut]
 *   <t> <anchor> orbit <radius> <height> <az_deg> <look_height>  [interp]
 *   <t> game                         hand back to the game camera from <t>
 *   <t> shake <dur> <amp_deg> [hz]   rumble (default 9 hz)
 *   <t> gameshake <scale>            scale the game's own shakes from <t>
 *
 * <anchor> is world, player, or an actor name; a trailing '@' turns the
 * offsets with the anchor's heading.  The interpolation token shapes the
 * move to the NEXT key (default ease).  Time is game seconds since the
 * first frame the game camera updates (rb: the camera manager's first
 * Update, `sCineEpoch`).
 *
 * All the key math runs in rb (Oni2) world space, exactly as rb does it;
 * only the resolved eye/target are converted to Bevy space.  The two spaces
 * differ by a 180° yaw (`space::to_bevy_space_pos`), a proper rotation, so
 * AGE's camera basis (a right, b up, c = boom, looking down -c) and its
 * RotateLocalX/Y/Z signs carry over to Bevy's camera unchanged.
 *
 * Like rb, the cinematic view replaces only what is rendered: the game
 * camera keeps running underneath (rb keeps `m_GameViewMatrix`, which
 * `PADDATA.CameraMatrix` reads, so the pad stays relative to the game
 * camera).  Here the game camera's pose is stashed after
 * `polar::apply_camera_transform` and put back before the fixed main loop (after the
 * prepass has taken the rendered view as motion blur's previous view), so every
 * gameplay system (movement, EATME, `--padaim`) and the game camera's own
 * smoothing see the game camera, and only render extract sees the cine view.
 */
use bevy::prelude::*;

use crate::camera::channel::CameraChannel;
use crate::camera::components::CameraController;
use crate::oni2_loader::utils::space;

/// rb's array caps (sCineKeys[128], sCineShakes[32], sCineGameShakes[16]).
const MAX_KEYS: usize = 128;
const MAX_SHAKES: usize = 32;
const MAX_GAME_SHAKES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CineMode {
    Look,
    Orbit,
    /// `<t> game` — the game camera has the view from `t`.
    Game,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CineInterp {
    Lin,
    Ease,
    Cut,
    Spline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CineKey {
    pub t: f32,
    pub anchor: String,
    /// Trailing '@': offsets turn with the anchor's heading.
    pub local: bool,
    pub mode: CineMode,
    pub v: [f32; 6],
    pub interp: CineInterp,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CineShake {
    pub t: f32,
    pub dur: f32,
    pub amp: f32,
    pub hz: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CineGameShake {
    pub t: f32,
    pub scale: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CineScript {
    pub keys: Vec<CineKey>,
    pub shakes: Vec<CineShake>,
    pub game_shakes: Vec<CineGameShake>,
}

/// Leading floats of `toks`, stopping at the first that doesn't parse —
/// sscanf's `%f %f ...` behaviour.
fn leading_floats(toks: &[&str], max: usize) -> Vec<f32> {
    toks.iter()
        .take(max)
        .map_while(|s| s.parse::<f32>().ok())
        .collect()
}

/// Parses a `.cine` file the way `sCineLoad` does.
pub fn parse_cine(text: &str) -> CineScript {
    let mut out = CineScript::default();
    for raw in text.lines() {
        if out.keys.len() >= MAX_KEYS {
            break;
        }
        let line = raw.split('#').next().unwrap_or("");
        let toks: Vec<&str> = line.split_whitespace().collect();
        // n < 2 (time + anchor) → skip
        let Some(t) = toks.first().and_then(|s| s.parse::<f32>().ok()) else {
            continue;
        };
        let Some(&anchor_tok) = toks.get(1) else {
            continue;
        };
        // %47s
        let mut anchor: String = anchor_tok.chars().take(47).collect();
        if anchor.eq_ignore_ascii_case("gameshake") {
            // sscanf("%*f %*s %f") == 1
            if let Some(sc) = toks.get(2).and_then(|s| s.parse::<f32>().ok())
                && out.game_shakes.len() < MAX_GAME_SHAKES
            {
                out.game_shakes.push(CineGameShake { t, scale: sc });
            }
            continue;
        }
        if anchor.eq_ignore_ascii_case("shake") {
            // sscanf("%*f %*s %f %f %f") >= 2, hz defaults to 9
            let f = leading_floats(&toks[2..], 3);
            if f.len() >= 2 && out.shakes.len() < MAX_SHAKES {
                out.shakes.push(CineShake {
                    t,
                    dur: f[0],
                    amp: f[1],
                    hz: f.get(2).copied().unwrap_or(9.0),
                });
            }
            continue;
        }
        if anchor.eq_ignore_ascii_case("game") {
            out.keys.push(CineKey {
                t,
                anchor: "game".into(),
                local: false,
                mode: CineMode::Game,
                v: [0.0; 6],
                interp: CineInterp::Ease,
            });
            continue;
        }
        let local = anchor.ends_with('@');
        if local {
            anchor.pop();
        }
        let mode_tok = toks.get(2).copied().unwrap_or("");
        let floats = if toks.len() > 3 {
            leading_floats(&toks[3..], 6)
        } else {
            Vec::new()
        };
        let mut v = [0.0f32; 6];
        v[..floats.len()].copy_from_slice(&floats);
        // The 10th sscanf field (interp) is only reached when all 6 floats
        // scanned.
        let mut interp_tok: Option<&str> = if floats.len() == 6 {
            toks.get(9).copied()
        } else {
            None
        };
        let mode = if mode_tok.eq_ignore_ascii_case("orbit") {
            // orbit has 4 numbers; the 5th token may be the interp word
            // (`if (n >= 7) sscanf("%*f %*s %*s %*f %*f %*f %*f %15s")`).
            if floats.len() >= 4
                && let Some(tok) = toks.get(7)
            {
                interp_tok = Some(tok);
            }
            CineMode::Orbit
        } else if mode_tok.eq_ignore_ascii_case("look") {
            CineMode::Look
        } else {
            warn!("[cinecam] bad mode '{}'", mode_tok);
            continue;
        };
        let interp = match interp_tok {
            Some(s) if s.eq_ignore_ascii_case("lin") => CineInterp::Lin,
            Some(s) if s.eq_ignore_ascii_case("cut") => CineInterp::Cut,
            Some(s) if s.eq_ignore_ascii_case("spline") => CineInterp::Spline,
            _ => CineInterp::Ease,
        };
        out.keys.push(CineKey {
            t,
            anchor,
            local,
            mode,
            v,
            interp,
        });
    }
    out
}

/// An anchor's frame in rb space: position, plus its heading when the key is
/// local (`sCineAnchor`: `m.RotateY(atan2(c.x, c.z))`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CineFrame {
    pub pos: Vec3,
    pub yaw: f32,
}

impl CineFrame {
    pub const WORLD: Self = Self {
        pos: Vec3::ZERO,
        yaw: 0.0,
    };

    /// `Matrix34::Transform` of an identity matrix after `RotateY(yaw)`
    /// (age/src/vector/vector3.h RotateY: x' = cx + sz, z' = -sx + cz).
    pub fn transform(&self, v: Vec3) -> Vec3 {
        let (s, c) = self.yaw.sin_cos();
        Vec3::new(v.x * c + v.z * s, v.y, -v.x * s + v.z * c) + self.pos
    }
}

fn orbit_eye(fr: &CineFrame, r: f32, h: f32, az_deg: f32, look_h: f32) -> (Vec3, Vec3) {
    let az = az_deg.to_radians();
    let eye = fr.transform(Vec3::new(r * az.sin(), h, r * az.cos()));
    let tgt = fr.transform(Vec3::new(0.0, look_h, 0.0));
    (eye, tgt)
}

/// Resolves an anchor name (+ local flag) to its frame, or None when the
/// anchor isn't a live actor (rb then keeps the game camera).
pub trait AnchorResolver {
    fn frame(&self, anchor: &str, local: bool) -> Option<CineFrame>;
}

impl<F: Fn(&str, bool) -> Option<CineFrame>> AnchorResolver for F {
    fn frame(&self, anchor: &str, local: bool) -> Option<CineFrame> {
        self(anchor, local)
    }
}

fn anchor_frame(k: &CineKey, res: &dyn AnchorResolver) -> Option<CineFrame> {
    if k.anchor.eq_ignore_ascii_case("world") {
        return Some(CineFrame::WORLD);
    }
    res.frame(&k.anchor, k.local)
}

fn resolve(k: &CineKey, res: &dyn AnchorResolver) -> Option<(Vec3, Vec3)> {
    let fr = anchor_frame(k, res)?;
    Some(match k.mode {
        CineMode::Orbit => orbit_eye(&fr, k.v[0], k.v[1], k.v[2], k.v[3]),
        _ => (
            fr.transform(Vec3::new(k.v[0], k.v[1], k.v[2])),
            fr.transform(Vec3::new(k.v[3], k.v[4], k.v[5])),
        ),
    })
}

/// `sSameRun`: keys of one mode (look / orbit) on one anchor.
fn same_run(a: &CineKey, b: &CineKey) -> bool {
    a.mode == b.mode
        && a.mode != CineMode::Game
        && a.anchor.eq_ignore_ascii_case(&b.anchor)
        && a.local == b.local
}

/// `sCineParams`: orbit → its 4 parameters; look → resolved world eye+target.
fn params(k: &CineKey, res: &dyn AnchorResolver) -> ([f32; 6], usize) {
    if k.mode == CineMode::Orbit {
        let mut p = [0.0; 6];
        p[..4].copy_from_slice(&k.v[..4]);
        return (p, 4);
    }
    let (e, t) = resolve(k, res).unwrap_or((Vec3::ZERO, Vec3::ZERO));
    ([e.x, e.y, e.z, t.x, t.y, t.z], 6)
}

/// `sCineEvaluate`: the cinematic eye and target (rb space) at time `t`, or
/// None to keep the game camera.
pub fn evaluate(keys: &[CineKey], t: f32, res: &dyn AnchorResolver) -> Option<(Vec3, Vec3)> {
    if keys.is_empty() || t < keys[0].t {
        return None;
    }
    let n = keys.len();
    let mut i = 0;
    while i + 1 < n && keys[i + 1].t <= t {
        i += 1;
    }
    let a = &keys[i];
    if a.mode == CineMode::Game {
        return None;
    }
    let (mut eye, mut tgt) = resolve(a, res)?;
    if i + 1 < n && a.interp == CineInterp::Spline && same_run(a, &keys[i + 1]) {
        let b = &keys[i + 1];
        let pk = (i > 0 && keys[i - 1].interp == CineInterp::Spline && same_run(&keys[i - 1], a))
            .then(|| &keys[i - 1]);
        let nk = (i + 2 < n && b.interp == CineInterp::Spline && same_run(b, &keys[i + 2]))
            .then(|| &keys[i + 2]);
        let dt = (b.t - a.t).max(0.001);
        let u = ((t - a.t) / dt).clamp(0.0, 1.0);
        let (u2, u3) = (u * u, u * u * u);
        let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
        let h10 = u3 - 2.0 * u2 + u;
        let h01 = -2.0 * u3 + 3.0 * u2;
        let h11 = u3 - u2;
        let (pa, cnt) = params(a, res);
        let (pb, _) = params(b, res);
        let pp = pk.map(|k| params(k, res).0);
        let pn = nk.map(|k| params(k, res).0);
        let mut r = [0.0f32; 6];
        for j in 0..cnt {
            let ma = match (pk, pp) {
                (Some(k), Some(pp)) => (pb[j] - pp[j]) / (b.t - k.t).max(0.001),
                _ => 0.0,
            };
            let mb = match (nk, pn) {
                (Some(k), Some(pn)) => (pn[j] - pa[j]) / (k.t - a.t).max(0.001),
                _ => 0.0,
            };
            r[j] = h00 * pa[j] + h10 * ma * dt + h01 * pb[j] + h11 * mb * dt;
        }
        if a.mode == CineMode::Orbit {
            // rb ignores sCineAnchor's result here (resolve(a) succeeded above).
            let fr = anchor_frame(a, res).unwrap_or(CineFrame::WORLD);
            (eye, tgt) = orbit_eye(&fr, r[0], r[1], r[2], r[3]);
        } else {
            eye = Vec3::new(r[0], r[1], r[2]);
            tgt = Vec3::new(r[3], r[4], r[5]);
        }
    } else if i + 1 < n && a.interp != CineInterp::Cut && keys[i + 1].mode != CineMode::Game {
        let b = &keys[i + 1];
        let mut u = ((t - a.t) / (b.t - a.t).max(0.001)).clamp(0.0, 1.0);
        if a.interp == CineInterp::Ease {
            u = u * u * (3.0 - 2.0 * u);
        }
        if a.mode == CineMode::Orbit
            && b.mode == CineMode::Orbit
            && a.anchor.eq_ignore_ascii_case(&b.anchor)
            && a.local == b.local
        {
            let fr = anchor_frame(a, res).unwrap_or(CineFrame::WORLD);
            let p: Vec<f32> = (0..4).map(|j| a.v[j] + (b.v[j] - a.v[j]) * u).collect();
            (eye, tgt) = orbit_eye(&fr, p[0], p[1], p[2], p[3]);
        } else if let Some((eb, tb)) = resolve(b, res) {
            eye = eye.lerp(eb, u);
            tgt = tgt.lerp(tb, u);
        }
    }
    if eye.distance_squared(tgt) < 1e-6 {
        tgt.z += 1.0;
    }
    Some((eye, tgt))
}

/// The game-shake scale in force at `t` (`sCineGameShakeScale`; only applied
/// while the cine view is active).
pub fn game_shake_scale(script: &CineScript, t: f32) -> f32 {
    let mut s = 1.0;
    for g in &script.game_shakes {
        if t >= g.t {
            s = g.scale;
        }
    }
    s
}

/// Sum of the active rumbles at `t` as local (x, y, z) rotation angles in
/// radians, applied in that order (`RotateLocalX/Y/Z` in sCineApply).
pub fn rumble_angles(shakes: &[CineShake], t: f32) -> Vec<Vec3> {
    let mut out = Vec::new();
    for sh in shakes {
        let u = t - sh.t;
        if u < 0.0 || u > sh.dur {
            continue;
        }
        let mut env = (u / 0.08).min(1.0) * (1.0 - u / sh.dur);
        if env <= 0.0 {
            env = 0.0;
        }
        let w = 2.0 * std::f32::consts::PI * sh.hz * u;
        let a = sh.amp * env * std::f32::consts::PI / 180.0;
        let px = (w * 1.00 + 0.3).sin() * 0.6 + (w * 2.31 + 1.7).sin() * 0.4;
        let py = (w * 0.87 + 2.1).sin() * 0.6 + (w * 1.93 + 0.5).sin() * 0.4;
        let pz = (w * 1.13 + 4.0).sin() * 0.5;
        out.push(Vec3::new(a * px, a * py, a * pz * 0.5));
    }
    out
}

// ---------------------------------------------------------------------------
// Bevy side
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct CineCam {
    pub script: CineScript,
    /// Accumulated game seconds since the game camera's first update.
    clock: Option<f32>,
    /// Game camera pose (the frame's `m_GameViewMatrix`), restored before
    /// the fixed main loop.
    game_view: Option<(Entity, Transform)>,
    /// The game shake state polar.rs used this frame (it bakes the shake into
    /// the game pose; the cine view re-applies it scaled by `gameshake`).
    game_shake: Option<(f32, Vec3, f32)>,
}

impl CineCam {
    pub fn load(path: &str) -> Option<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let script = parse_cine(&text);
                info!(
                    "[cinecam] {} keys from {} ({} shakes, {} gameshakes)",
                    script.keys.len(),
                    path,
                    script.shakes.len(),
                    script.game_shakes.len()
                );
                Some(Self {
                    script,
                    clock: None,
                    game_view: None,
                    game_shake: None,
                })
            }
            Err(e) => {
                error!("[cinecam] can't open {}: {}", path, e);
                None
            }
        }
    }
}

/// Before the fixed main loop: hand the camera back its game pose so the frame's gameplay
/// systems and the game camera's smoothing never see the cine view.
pub fn cine_restore_game_view(
    mut cine: ResMut<CineCam>,
    mut cams: Query<(&mut Transform, &mut GlobalTransform), With<CameraController>>,
) {
    if let Some((e, tf)) = cine.game_view.take()
        && let Ok((mut t, mut g)) = cams.get_mut(e)
    {
        *t = tf;
        *g = GlobalTransform::from(tf);
    }
}

/// Before `apply_camera_transform`: remember the game shake it is about to
/// bake in.
pub fn cine_note_game_shake(
    mut cine: ResMut<CineCam>,
    cams: Query<&CameraChannel, With<CameraController>>,
) {
    cine.game_shake = cams.iter().next().and_then(|ch| {
        (ch.shake_time_remaining > 0.0).then_some((
            ch.shake_time_remaining,
            ch.shake_amplitude,
            ch.shake_time_to_damp,
        ))
    });
}

/// polar.rs `apply_camera_shake`'s perturbation for a given shake state.
fn game_shake_rot(rem: f32, amp: Vec3, to_damp: f32, scale: f32) -> Quat {
    let damp = if rem < to_damp { rem / to_damp } else { 1.0 };
    Quat::from_euler(
        EulerRot::XYZ,
        (rem * 97.3).sin() * amp.x * damp * scale,
        (rem * 53.7).sin() * amp.y * damp * scale,
        (rem * 31.1).sin() * amp.z * damp * scale,
    )
}

/// An actor's matrix `c` column (`atan2(c.x, c.z)` is rb's yaw) in rb
/// space.  Checked against rb's -cinelog on Sizzle_Brawl: the player's layout
/// pose logs yaw -90 in rb, and the Rust model's local -Z maps to that (its
/// visual front, local +Z, points the other way).
fn rb_c_axis(g: &GlobalTransform) -> Vec3 {
    space::to_oni2_space_pos(g.rotation() * Vec3::NEG_Z)
}

static CINE_LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// After `apply_camera_transform`: `sCineApply` — stash the game pose, then
/// put the cinematic view (plus rumbles) on the camera for rendering.
pub fn cine_apply(
    time: Res<Time>,
    mut cine: ResMut<CineCam>,
    mut cams: Query<(Entity, &mut Transform, &mut GlobalTransform), With<CameraController>>,
    actors: super::ActorLookup,
    mut warned: Local<Vec<String>>,
    mut log_sec: Local<i32>,
) {
    let Some((cam, mut tf, mut gtf)) = cams.iter_mut().next() else {
        return;
    };
    let t = match cine.clock {
        None => {
            info!("[cinecam] epoch: game camera's first update");
            cine.clock = Some(0.0);
            0.0
        }
        Some(c) => {
            let c = c + time.delta_secs();
            cine.clock = Some(c);
            c
        }
    };
    cine.game_view = Some((cam, *tf));

    let missing = std::cell::RefCell::new(Vec::<String>::new());
    let resolver = |name: &str, local: bool| -> Option<CineFrame> {
        let g = if name.eq_ignore_ascii_case("player") {
            actors.player()
        } else {
            actors.named(name)
        };
        let Some(g) = g else {
            missing.borrow_mut().push(name.to_string());
            return None;
        };
        let pos = space::to_oni2_space_pos(g.translation());
        let yaw = if local {
            let c = rb_c_axis(&g);
            c.x.atan2(c.z)
        } else {
            0.0
        };
        Some(CineFrame { pos, yaw })
    };
    let view = evaluate(&cine.script.keys, t, &resolver);
    // --cinelog: rb's -cinelog line once a second (player position and
    // heading in rb space, the game camera) plus the cine eye/target.
    if *CINE_LOG.get_or_init(|| super::has_flag("--cinelog"))
        && t as i32 != *log_sec
        && let Some(p) = actors.player()
    {
        *log_sec = t as i32;
        let pm = space::to_oni2_space_pos(p.translation());
        let c = rb_c_axis(&p);
        let cam = space::to_oni2_space_pos(tf.translation);
        info!(
            "[cinelog] t {} player {:.2} {:.2} {:.2} yaw {:.0}  cam {:.2} {:.2} {:.2}  cine {:?}",
            *log_sec,
            pm.x,
            pm.y,
            pm.z,
            c.x.atan2(c.z).to_degrees(),
            cam.x,
            cam.y,
            cam.z,
            view
        );
    }
    for name in missing.into_inner() {
        if !warned.contains(&name) {
            warn!(
                "[cinecam] anchor '{}' is not a live actor (actor names usually start with actor_); using the game camera",
                name
            );
            warned.push(name);
        }
    }

    if let Some((eye, tgt)) = view {
        let mut out = Transform::from_translation(space::to_bevy_space_pos(eye));
        out.look_at(space::to_bevy_space_pos(tgt), Vec3::Y);
        let scale = game_shake_scale(&cine.script, t);
        if let Some((rem, amp, damp)) = cine.game_shake
            && scale != 0.0
        {
            out.rotation *= game_shake_rot(rem, amp, damp, scale);
        }
        *tf = out;
    }
    for a in rumble_angles(&cine.script.shakes, t) {
        tf.rotation = tf.rotation
            * Quat::from_rotation_x(a.x)
            * Quat::from_rotation_y(a.y)
            * Quat::from_rotation_z(a.z);
    }
    *gtf = GlobalTransform::from(*tf);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_actors(_: &str, _: bool) -> Option<CineFrame> {
        None
    }

    #[test]
    fn parses_orbit_with_interp_word() {
        let s = parse_cine(
            "# melee_1\n0 player orbit 4.6 1.7 250 1.1 lin\n14 player orbit 4.6 1.7 340 1.1\n",
        );
        assert_eq!(s.keys.len(), 2);
        let k = &s.keys[0];
        assert_eq!(k.mode, CineMode::Orbit);
        assert_eq!(k.anchor, "player");
        assert!(!k.local);
        assert_eq!(&k.v[..4], &[4.6, 1.7, 250.0, 1.1]);
        assert_eq!(k.interp, CineInterp::Lin);
        assert_eq!(s.keys[1].interp, CineInterp::Ease);
        assert_eq!(s.keys[1].t, 14.0);
    }

    #[test]
    fn parses_look_local_game_and_shakes() {
        let s = parse_cine(
            "0   world look -5.5 12.0 1.2   -2.5 11.9 10 lin\n\
             2 actor_Mk2@ look 0 2 -6 0 1 4 cut\n\
             3 actor_WlyLead@ orbit 3 1.5 20 1.2 spline\n\
             5 look\n\
             6 world pan 1 2 3\n\
             9 game\n\
             0 gameshake 0\n\
             13.1 shake 0.75 1.9 13\n\
             14 shake 0.5 2\n\
             15 shake 1\n",
        );
        assert_eq!(s.keys.len(), 4, "{:?}", s.keys);
        assert_eq!(s.keys[0].v, [-5.5, 12.0, 1.2, -2.5, 11.9, 10.0]);
        assert_eq!(s.keys[0].interp, CineInterp::Lin);
        assert_eq!(s.keys[1].anchor, "actor_Mk2");
        assert!(s.keys[1].local);
        assert_eq!(s.keys[1].interp, CineInterp::Cut);
        assert_eq!(s.keys[2].interp, CineInterp::Spline);
        assert_eq!(s.keys[3].mode, CineMode::Game);
        assert_eq!(s.game_shakes, vec![CineGameShake { t: 0.0, scale: 0.0 }]);
        assert_eq!(s.shakes.len(), 2);
        assert_eq!(s.shakes[0].hz, 13.0);
        assert_eq!(s.shakes[1].hz, 9.0);
    }

    #[test]
    fn look_with_missing_floats_keeps_default_ease() {
        // sscanf stops at the first non-float: the interp field is never
        // reached, the unscanned floats stay 0.
        let s = parse_cine("1 world look 1 2 3 lin\n");
        assert_eq!(s.keys[0].v, [1.0, 2.0, 3.0, 0.0, 0.0, 0.0]);
        assert_eq!(s.keys[0].interp, CineInterp::Ease);
    }

    #[test]
    fn evaluates_lin_look_and_holds_after_last_key() {
        let s = parse_cine("0 world look 0 0 0  0 0 10 lin\n10 world look 10 0 0  10 0 10\n");
        assert!(evaluate(&s.keys, -0.1, &no_actors).is_none());
        let (e, t) = evaluate(&s.keys, 5.0, &no_actors).unwrap();
        assert!((e - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5);
        assert!((t - Vec3::new(5.0, 0.0, 10.0)).length() < 1e-5);
        let (e, _) = evaluate(&s.keys, 50.0, &no_actors).unwrap();
        assert!((e - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn ease_is_smoothstep_and_cut_holds() {
        let s = parse_cine(
            "0 world look 0 0 0 0 0 1\n4 world look 4 0 0 4 0 1 cut\n8 world look 8 0 0 8 0 1\n",
        );
        let (e, _) = evaluate(&s.keys, 1.0, &no_actors).unwrap();
        let u: f32 = 0.25;
        assert!((e.x - 4.0 * u * u * (3.0 - 2.0 * u)).abs() < 1e-5);
        let (e, _) = evaluate(&s.keys, 7.9, &no_actors).unwrap();
        assert!((e.x - 4.0).abs() < 1e-5);
    }

    #[test]
    fn orbit_arcs_around_the_anchor_and_game_key_releases() {
        let player = |n: &str, _: bool| {
            (n == "player").then_some(CineFrame {
                pos: Vec3::new(1.0, 0.0, 2.0),
                yaw: 0.0,
            })
        };
        let s = parse_cine("0 player orbit 2 1 0 1 lin\n10 player orbit 2 1 90 1\n20 game\n");
        let (e, t) = evaluate(&s.keys, 0.0, &player).unwrap();
        assert!((e - Vec3::new(1.0, 1.0, 4.0)).length() < 1e-5);
        assert!((t - Vec3::new(1.0, 1.0, 2.0)).length() < 1e-5);
        // halfway: azimuth 45 (orbit parameters interpolate, not the eye)
        let (e, _) = evaluate(&s.keys, 5.0, &player).unwrap();
        let r = 2.0 * std::f32::consts::FRAC_1_SQRT_2;
        assert!((e - Vec3::new(1.0 + r, 1.0, 2.0 + r)).length() < 1e-4);
        assert!(evaluate(&s.keys, 20.0, &player).is_none());
        // a dead anchor → game camera
        assert!(evaluate(&s.keys, 1.0, &no_actors).is_none());
    }

    #[test]
    fn local_frame_turns_offsets_with_heading() {
        // heading +X (yaw 90°): local z (ahead) → world +x, local x → world -z
        let fr = CineFrame {
            pos: Vec3::ZERO,
            yaw: std::f32::consts::FRAC_PI_2,
        };
        assert!((fr.transform(Vec3::Z) - Vec3::X).length() < 1e-5);
        assert!((fr.transform(Vec3::X) - Vec3::NEG_Z).length() < 1e-5);
    }

    #[test]
    fn spline_run_passes_through_keys_with_velocity() {
        let s = parse_cine(
            "0 world look 0 0 0 0 0 1 spline\n1 world look 1 0 0 1 0 1 spline\n2 world look 2 0 0 2 0 1 spline\n",
        );
        let (e, _) = evaluate(&s.keys, 1.0, &no_actors).unwrap();
        assert!((e.x - 1.0).abs() < 1e-5);
        // middle key carries velocity: just past it, it keeps moving at ~1/s
        let (a, _) = evaluate(&s.keys, 1.01, &no_actors).unwrap();
        assert!(a.x > 1.005, "{}", a.x);
        // run start is at rest: barely moved after 0.01 s
        let (b, _) = evaluate(&s.keys, 0.01, &no_actors).unwrap();
        assert!(b.x < 0.002, "{}", b.x);
    }

    #[test]
    fn game_shake_scale_and_rumble_window() {
        let s = parse_cine("0 gameshake 0\n5 gameshake 0.5\n1 shake 1 2\n");
        assert_eq!(game_shake_scale(&s, 1.0), 0.0);
        assert_eq!(game_shake_scale(&s, 6.0), 0.5);
        assert!(rumble_angles(&s.shakes, 0.5).is_empty());
        assert_eq!(rumble_angles(&s.shakes, 1.5).len(), 1);
        assert!(rumble_angles(&s.shakes, 2.5).is_empty());
    }
}
