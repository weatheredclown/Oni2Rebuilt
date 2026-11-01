/*
 * oni2_loader/gait_blender.rs — port of AGE's crGaitBlender
 * (age/src/crlocomotion/gaitblender.cpp) for locomotion blending.
 *
 * A .loco file lists gaits along one value axis (the player's: STAND 0..0,
 * BACK 0..-1, SNEAK, WLK, JOG, RUN up to 1) plus an optional nested locodata
 * that becomes a *sub-blender* on a second axis (strafe: LEFT +, RIGHT -).
 * Each frame:
 *   - every gait gets a scale from the value (linear in [min,max], or a
 *     0.25 s time ramp for gaits with an ideal_throttle) and a playback rate
 *     (stride-matched against its neighbours);
 *   - the highest non-independent gait with scale > 0 drives the phase, the
 *     other phase-locked gaits copy it, independent ones (STAND, the
 *     sub-blender) keep their own clock, and play_from_start gaits restart
 *     while unused;
 *   - the pose starts from the highest fully-weighted gait and lerps the
 *     higher ones in; the sub-blender then lerps its own gaits over that, so
 *     |strafe| >= 0.5 takes the pose over completely.
 * The value is the signed mover throttle × GaitBlendFactor (1.75) and the
 * sub value the side-step (crmover/packet.cpp:374, animator.cpp
 * UpdateSlewAnimation) — backward and right-strafe are negative.
 */
use super::animation::{AnimId, Oni2AnimLibrary, sample_animation_pose};
use super::parsers::types::{Oni2Animation, Oni2Skeleton};
use super::parsers::loco::LocoBlendGait;
use bevy::prelude::*;

#[derive(Clone, Debug, Default)]
struct GaitRec {
    scale: f32,
    /// This gait's own rate multiplier (Low/HiThrottlePct, 1 at ideal).
    rate: f32,
    /// The rate its anim list last ran at (as master, or its own if
    /// independent) — `crAnimList::SetRate`.
    list_rate: f32,
    /// Normalized phase 0..1.
    phase: f32,
    low_pct: f32,
    hi_pct: f32,
}

#[derive(Clone, Debug, Default)]
pub struct GaitBlenderState {
    recs: Vec<GaitRec>,
    current: i32,
}

/// Per-entity blender state: the main blender and its strafe sub-blender.
#[derive(Component, Clone, Debug, Default)]
pub struct GaitBlendRuntime {
    pub main: GaitBlenderState,
    pub sub: GaitBlenderState,
    initialized: bool,
}

/// What one record of a blender is.
enum RecKind<'a> {
    Gait(&'a LocoBlendGait),
    /// The nested locodata — fixed scale 1, independent phase.
    Sub,
}

fn kinds<'a>(gaits: &'a [LocoBlendGait], has_sub: bool) -> Vec<RecKind<'a>> {
    let mut v: Vec<RecKind> = gaits.iter().map(RecKind::Gait).collect();
    if has_sub {
        v.push(RecKind::Sub);
    }
    v
}

/// `crAnimation::GetAverageSpeed`: stride length per second at 30 fps.
fn average_speed(anim: &Oni2Animation) -> f32 {
    if anim.num_frames == 0 {
        return 0.0;
    }
    anim.stride_z.abs() * 30.0 / anim.num_frames as f32
}

fn init_blender(state: &mut GaitBlenderState, gaits: &[LocoBlendGait], has_sub: bool, lib: &Oni2AnimLibrary) {
    let ks = kinds(gaits, has_sub);
    state.recs = vec![GaitRec::default(); ks.len()];
    state.current = -1;
    for (i, k) in ks.iter().enumerate() {
        let r = &mut state.recs[i];
        r.rate = 1.0;
        r.list_rate = 1.0;
        r.hi_pct = 1.0;
        r.low_pct = 0.0;
        // AddGait: SetScale(!NumGaits) — only the first record starts visible.
        r.scale = if i == 0 { 1.0 } else { 0.0 };
        let _ = k;
    }
    // AddGait / FindLowerGait: the gait whose max equals my min is the one
    // below me; match our strides at the boundary.
    for i in 0..gaits.len() {
        let g = &gaits[i];
        let Some(lower) = (0..i).find(|&j| gaits[j].max_throttle == g.min_throttle) else {
            continue;
        };
        if gaits[lower].max_throttle == 0.0 {
            state.recs[i].low_pct = 0.0;
            state.recs[lower].hi_pct = 1.0;
        } else {
            let s1 = lib.anims.get(&gaits[lower].anim).map(average_speed).unwrap_or(0.0);
            let s2 = lib.anims.get(&g.anim).map(average_speed).unwrap_or(0.0);
            if s1 > 0.0 && s2 > 0.0 {
                let avg = (s1 + s2) * 0.5;
                state.recs[lower].hi_pct = avg / s1;
                state.recs[i].low_pct = avg / s2;
            }
        }
    }
}

fn is_fixed(k: &RecKind) -> bool {
    match k {
        RecKind::Sub => true,
        RecKind::Gait(g) => g.min_throttle == g.max_throttle,
    }
}
fn is_independent(k: &RecKind) -> bool {
    match k {
        RecKind::Sub => true,
        RecKind::Gait(g) => g.independent_phase,
    }
}

fn frames_of(lib: &Oni2AnimLibrary, anim: AnimId) -> f32 {
    lib.anims.get(&anim).map(|a| a.num_frames as f32).unwrap_or(1.0).max(1.0)
}

fn advance(phase: f32, rate: f32, frames: f32, fps: f32, dt: f32) -> f32 {
    (phase + dt * rate * fps / frames).rem_euclid(1.0)
}

/// `crGaitBlender::UpdatePhase`.
#[allow(clippy::too_many_arguments)]
fn update_phase(
    state: &mut GaitBlenderState,
    gaits: &[LocoBlendGait],
    has_sub: bool,
    value: f32,
    dt: f32,
    fps: f32,
    lib: &Oni2AnimLibrary,
    anim_for: &dyn Fn(usize) -> AnimId,
    sub: Option<(&mut GaitBlenderState, &[LocoBlendGait], f32, &dyn Fn(usize) -> AnimId)>,
) {
    let ks = kinds(gaits, has_sub);
    let n = ks.len();

    // Scale (and rate) per record.
    for i in 0..n {
        let r = &mut state.recs[i];
        let RecKind::Gait(g) = &ks[i] else {
            r.scale = 1.0;
            continue;
        };
        if is_fixed(&ks[i]) {
            r.scale = 1.0;
            continue;
        }
        if g.trans_blend_time > 0.0 {
            let trans = 1.0 / g.trans_blend_time;
            if value <= g.min_throttle {
                r.scale = (r.scale - dt * trans).max(0.0);
                r.rate = r.low_pct;
            } else if value > g.max_throttle {
                r.scale = (r.scale + dt * trans).min(1.0);
                r.rate = r.hi_pct;
            } else {
                r.scale = (r.scale + dt * trans).min(1.0);
                let ideal = g.ideal();
                if value == ideal {
                    r.rate = 1.0;
                } else if value < ideal {
                    let pct = (value - g.min_throttle) / (ideal - g.min_throttle);
                    if g.trans_blend_lo {
                        r.rate = r.low_pct + (1.0 - r.low_pct) * pct;
                    } else {
                        r.scale = pct.clamp(0.0, 1.0);
                    }
                } else {
                    let pct = (value - ideal) / (g.max_throttle - ideal);
                    r.rate = 1.0 + (r.hi_pct - 1.0) * pct;
                }
            }
        } else {
            r.scale = ((value - g.min_throttle) / (g.max_throttle - g.min_throttle)).clamp(0.0, 1.0);
        }
    }

    // Total anim rate: lerp through the phase-locked gaits by their scales.
    let mut anim_rate = state.recs[0].rate;
    for j in 1..n {
        if !is_independent(&ks[j]) {
            let r = &state.recs[j];
            anim_rate += (r.rate - anim_rate) * r.scale;
        }
    }

    state.current = -1;
    let mut sub = sub;
    let Some(driver) = (0..n)
        .rev()
        .find(|&i| state.recs[i].scale > 0.0 && (!is_independent(&ks[i]) || i == 0))
    else {
        return;
    };
    state.current = driver as i32;
    let dframes = frames_of(lib, anim_for(driver));
    {
        let r = &mut state.recs[driver];
        r.list_rate = anim_rate;
        r.phase = advance(r.phase, anim_rate, dframes, fps, dt);
    }
    let master_phase = state.recs[driver].phase;
    for j in 0..n {
        if j == driver {
            continue;
        }
        match &ks[j] {
            RecKind::Sub => {
                if let Some((sub_state, sub_gaits, sub_value, sub_anim_for)) = sub.as_mut() {
                    update_phase(sub_state, sub_gaits, false, *sub_value, dt, fps, lib, *sub_anim_for, None);
                }
            }
            RecKind::Gait(g) => {
                let r = &mut state.recs[j];
                if g.play_from_start && r.scale == 0.0 {
                    r.phase = 0.0; // Restart()
                } else if g.independent_phase {
                    r.phase = advance(r.phase, r.list_rate, frames_of(lib, anim_for(j)), fps, dt);
                } else {
                    r.phase = master_phase;
                }
            }
        }
    }
}

fn lerp_pose(frame: &mut [f32], pose: &[f32], scale: f32, skeleton: &Oni2Skeleton) {
    let len = frame.len().min(pose.len());
    let rot_flags = &skeleton.channel_is_rot;
    for i in 0..len {
        let is_rot = if rot_flags.len() >= frame.len() {
            rot_flags[i]
        } else {
            frame.len() != 1 && !(3..6).contains(&i)
        };
        if is_rot {
            let mut diff = (pose[i] - frame[i]).rem_euclid(std::f32::consts::TAU);
            if diff > std::f32::consts::PI {
                diff -= std::f32::consts::TAU;
            }
            frame[i] += diff * scale;
        } else {
            frame[i] += (pose[i] - frame[i]) * scale;
        }
    }
}

/// `crGaitBlender::UpdateFrame` — does NOT clear `frame` first, so a
/// sub-blender layers onto the parent's pose.
#[allow(clippy::too_many_arguments)]
fn update_frame(
    state: &GaitBlenderState,
    gaits: &[LocoBlendGait],
    has_sub: bool,
    lib: &Oni2AnimLibrary,
    skeleton: &Oni2Skeleton,
    frame: &mut Vec<f32>,
    anim_for: &dyn Fn(usize) -> AnimId,
    sub: Option<(&GaitBlenderState, &[LocoBlendGait], &dyn Fn(usize) -> AnimId)>,
) {
    let ks = kinds(gaits, has_sub);
    let n = ks.len();
    // Highest fully-weighted, non-sub record is the base; fall back to 0.
    let base = (1..n)
        .rev()
        .find(|&i| state.recs[i].scale == 1.0 && !matches!(ks[i], RecKind::Sub))
        .unwrap_or(0);
    for k in base..n {
        match &ks[k] {
            RecKind::Sub => {
                if let Some((sub_state, sub_gaits, sub_anim_for)) = sub {
                    update_frame(sub_state, sub_gaits, false, lib, skeleton, frame, sub_anim_for, None);
                }
            }
            RecKind::Gait(_) => {
                let r = &state.recs[k];
                if r.scale <= 0.0 {
                    continue;
                }
                let Some(anim) = lib.anims.get(&anim_for(k)) else {
                    continue;
                };
                let t = r.phase * anim.num_frames as f32;
                let mut pose = sample_animation_pose(anim, t, true, skeleton);
                if frame.is_empty() {
                    frame.resize(pose.len(), 0.0);
                }
                pose.resize(frame.len(), 0.0);
                lerp_pose(frame, &pose, r.scale, skeleton);
            }
        }
    }
}

/// One frame of locomotion blending.  `stand_override` replaces record 0's
/// anim (fight-stance / weapon-ready idle, the C++ channel-0 stack override).
/// Returns the blended pose.
#[allow(clippy::too_many_arguments)]
pub fn blend(
    rt: &mut GaitBlendRuntime,
    main_gaits: &[LocoBlendGait],
    sub_gaits: &[LocoBlendGait],
    lib: &Oni2AnimLibrary,
    skeleton: &Oni2Skeleton,
    value: f32,
    sub_value: f32,
    stand_override: Option<AnimId>,
    fps: f32,
    dt: f32,
    frame_len: usize,
) -> Vec<f32> {
    let has_sub = !sub_gaits.is_empty();
    if !rt.initialized || rt.main.recs.len() != main_gaits.len() + has_sub as usize {
        init_blender(&mut rt.main, main_gaits, has_sub, lib);
        init_blender(&mut rt.sub, sub_gaits, false, lib);
        rt.initialized = true;
    }
    let main_anim = |i: usize| -> AnimId {
        if i == 0
            && let Some(o) = stand_override
            && lib.anims.contains_key(&o)
        {
            return o;
        }
        main_gaits.get(i).map(|g| g.anim).unwrap_or_default()
    };
    let sub_anim = |i: usize| -> AnimId { sub_gaits.get(i).map(|g| g.anim).unwrap_or_default() };

    let GaitBlendRuntime { main, sub, .. } = rt;
    update_phase(
        main,
        main_gaits,
        has_sub,
        value,
        dt,
        fps,
        lib,
        &main_anim,
        Some((sub, sub_gaits, sub_value, &sub_anim)),
    );
    let mut frame = vec![0.0; frame_len];
    update_frame(main, main_gaits, has_sub, lib, skeleton, &mut frame, &main_anim, Some((sub, sub_gaits, &sub_anim)));
    frame
}

/// `-gaitlog` style description, matching crGaitBlender::DebugDescribe.
pub fn describe(
    rt: &GaitBlendRuntime,
    main_gaits: &[LocoBlendGait],
    sub_gaits: &[LocoBlendGait],
    lib: &Oni2AnimLibrary,
    value: f32,
    sub_value: f32,
) -> String {
    fn one(st: &GaitBlenderState, gaits: &[LocoBlendGait], has_sub: bool, lib: &Oni2AnimLibrary, value: f32, out: &mut String, sub: Option<(&GaitBlenderState, &[LocoBlendGait], f32)>) {
        out.push_str(&format!("v={:.3} g={}", value, st.current));
        for (i, r) in st.recs.iter().enumerate() {
            if has_sub && i == gaits.len() {
                if let Some((ss, sg, sv)) = sub {
                    out.push_str(&format!(" [{}:sub ", i));
                    one(ss, sg, false, lib, sv, out, None);
                    out.push(']');
                }
                continue;
            }
            if r.scale <= 0.0 {
                continue;
            }
            let name = gaits
                .get(i)
                .and_then(|g| lib.debug_names.get(&g.anim).cloned())
                .unwrap_or_else(|| "?".into());
            out.push_str(&format!(" [{}:{} s={:.2} p={:.3} r={:.2}]", i, name, r.scale, r.phase, r.rate));
        }
    }
    let mut out = String::new();
    one(&rt.main, main_gaits, !sub_gaits.is_empty(), lib, value, &mut out, Some((&rt.sub, sub_gaits, sub_value)));
    out
}
