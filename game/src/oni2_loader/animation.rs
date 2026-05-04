/*
 * oni2_loader/animation.rs — ONI2 skeletal animation playback.
 *
 * update_oni2_animation: advances Oni2AnimState frame counters, samples keyframe
 * channels (position, rotation, scale per bone), and writes SkinnedMesh joint
 * transforms.  Also drives curve_follower_system, creature_movement_anim_system,
 * ground_snap_system, and debug-draw systems for attack wedges / bounds / skeleton.
 */
use super::*;
use crate::oni2_loader::utils::space;

/// System: advance CurveFollower phase, evaluate NURBS position, update Transform.
pub fn curve_follower_system(
    mut query: Query<(&mut CurveFollower, &mut Transform)>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut follower, mut tf) in &mut query {
        if follower.speed == 0.0 && follower.reached_target {
            continue;
        }

        let prev_phase = follower.phase;

        let phase_advance = if follower.speed_is_physical {
            let current_pos = follower.curve.get_curve_point(follower.phase);
            let epsilon = 0.001;
            let delta_phase = if follower.speed > 0.0 {
                epsilon
            } else {
                -epsilon
            };

            let lookahead_phase = (follower.phase + delta_phase).clamp(0.0, 1.0);
            let lookahead_pos = follower.curve.get_curve_point(lookahead_phase);

            let dist = current_pos.distance(lookahead_pos);
            let phase_per_meter = if dist > 0.00001 {
                (lookahead_phase - follower.phase).abs() / dist
            } else {
                1.0
            };
            follower.speed * phase_per_meter
        } else {
            // Parametric speed from VM scripts
            follower.speed
        };

        follower.phase += phase_advance * dt;

        // Check if we've reached/passed the target
        if !follower.reached_target {
            let was_below = prev_phase < follower.target_phase;
            let now_above = follower.phase >= follower.target_phase;
            let was_above = prev_phase > follower.target_phase;
            let now_below = follower.phase <= follower.target_phase;
            if (was_below && now_above)
                || (was_above && now_below)
                || follower.phase == follower.target_phase
            {
                follower.phase = follower.target_phase;
                follower.reached_target = true;
            }
        }

        // Handle wrap-around / ping-pong
        let at_start = follower.phase <= 0.0;
        let at_end = follower.phase >= 1.0;
        if at_start || at_end {
            if follower.ping_pong {
                follower.speed = -follower.speed;
            } else if follower.wrap_around {
                while follower.phase >= 1.0 {
                    follower.phase -= 1.0;
                }
                while follower.phase <= 0.0 {
                    follower.phase += 1.0;
                }
            }
        }
        follower.phase = follower.phase.clamp(0.0, 1.0);

        // Evaluate position on curve
        let pos = follower.curve.get_curve_point(follower.phase);
        tf.translation = pos;

        // Orient along curve direction (look-ahead)
        if !follower.fixed_orientation {
            let look_t = (follower.phase + 0.005).min(0.999);
            let ahead = follower.curve.get_curve_point(look_t);
            let dir = ahead - pos;
            if dir.length_squared() > 0.001 {
                let forward = if follower.look_along_xz {
                    Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero()
                } else {
                    dir.normalize()
                };
                if forward.length_squared() > 0.001 {
                    let target = pos + forward;
                    tf.look_at(target, Vec3::Y);
                    // Oni2 models face +Z; look_at points -Z at target
                    tf.rotate_y(std::f32::consts::PI);
                }
            }
        }
    }
}

/// System: bridge ScrOni script VM to CurveFollower component.
/// Runs BEFORE scroni_tick_system so curve state is ready when the script checks it.
/// Runs AFTER scroni_tick_system for applying newly-issued commands.
/// We run it both before and after by just running it in Update alongside the others.
pub fn scroni_curve_bridge_system(
    mut query: Query<(
        &mut scroni::vm::ScrOniScript,
        Option<&mut CurveFollower>,
        Option<&Oni2AnimLibrary>,
        Option<&mut Oni2AnimState>,
        Option<&Name>,
    )>,
) {
    for (mut script, mut follower, anim_lib, mut anim_state, name_comp) in &mut query {
        let exec = &mut script.exec;
        let _entity_name = name_comp.map(|n| n.as_str()).unwrap_or("Unknown Entity");

        // We need to iterate over all active threads to see if any generated a curve variable or blocked on animation/curve
        for t in exec.all_threads_mut() {
            // 1. Apply non-blocking curve variable writes from script
            if let Some(ref mut follower) = follower {
                if let Some(v) = t.variables.remove("__curve_phase") {
                    follower.phase = v.as_float();
                    follower.reached_target = true;
                    follower.speed = 0.0;
                }
                if let Some(v) = t.variables.remove("__curve_speed") {
                    follower.speed = v.as_float();
                }
                if let Some(v) = t.variables.remove("__curve_pingpong") {
                    let pp = v.as_int() != 0;
                    follower.ping_pong = pp;
                    follower.wrap_around = !pp;
                }
            }

            // 2. Handle blocking actions
            if t.state == scroni::vm::ExecState::Blocked
                && let Some(ref action) = t.blocking.clone()
            {
                match action {
                    scroni::vm::BlockingAction::GotoCurvePhase { target, seconds } => {
                        if let Some(ref mut follower) = follower
                            && follower.reached_target
                        {
                            let target_val = *target;
                            let seconds_val = *seconds;
                            let dist = target_val - follower.phase;
                            follower.speed = if seconds_val > 0.0 {
                                dist / seconds_val
                            } else {
                                0.0
                            };
                            follower.speed_is_physical = false; // Script commands evaluate parametrically 
                            follower.target_phase = target_val;
                            follower.reached_target = false;
                            follower.wrap_around = false;
                            t.blocking = Some(scroni::vm::BlockingAction::WaitingForCurve);
                        }
                    }
                    scroni::vm::BlockingAction::WaitingForCurve => {
                        if let Some(ref follower) = follower
                            && follower.reached_target
                        {
                            t.blocking = None;
                            t.state = scroni::vm::ExecState::Running;
                        }
                    }
                    scroni::vm::BlockingAction::PlayAnimation {
                        name,
                        hold,
                        loop_anim,
                        rate,
                        ..
                    } => {
                        if let Some(lib) = anim_lib.as_ref() {
                            if let Some(ref mut state) = anim_state.as_deref_mut() {
                                let name_val = name.clone();
                                let hold_val = *hold;
                                let rate_val = *rate;
                                if lib.play(&name_val, state) {
                                    state.looping = *loop_anim;
                                    state.speed_multiplier = rate_val.unwrap_or(1.0);
                                    if hold_val {
                                        t.blocking =
                                            Some(scroni::vm::BlockingAction::WaitingForAnimation);
                                    } else {
                                        t.blocking = None;
                                        t.state = scroni::vm::ExecState::Running;
                                    }
                                } else {
                                    t.blocking = None;
                                    t.state = scroni::vm::ExecState::Running;
                                }
                            } else {
                                t.blocking = None;
                                t.state = scroni::vm::ExecState::Running;
                            }
                        } else {
                            t.blocking = None;
                            t.state = scroni::vm::ExecState::Running;
                        }
                    }
                    scroni::vm::BlockingAction::WaitingForAnimation => {
                        if let Some(state) = anim_state.as_deref() {
                            let num_frames = state.anim.frames.len() as f32;
                            if num_frames > 0.0
                                && state.current_time >= num_frames - 1.0
                                && !state.looping
                            {
                                t.blocking = None;
                                t.state = scroni::vm::ExecState::Running;
                            }
                        } else {
                            t.blocking = None;
                            t.state = scroni::vm::ExecState::Running;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Info about a spawned player creature from the layout.
#[derive(Component, Clone)]
pub struct Oni2DebugBounds {
    pub sub_bounds: Vec<Oni2DebugSubBound>,
}

#[derive(Clone)]
pub struct Oni2DebugSubBound {
    pub bone_idx: usize,
    pub vertices: Vec<Vec3>, // bound vertices in bone-local space
    pub edges: Vec<[u32; 2]>,
}

/// Debug component storing skeleton bone positions and parent links for gizmo rendering.
#[derive(Component, Clone)]
pub struct Oni2DebugSkeleton {
    pub positions: Vec<Vec3>, // bone world positions (Z-negated for Bevy)
    pub parent_indices: Vec<Option<usize>>,
    pub names: Vec<String>,
}

/// Animation state for cycling through frames.
#[derive(Component)]
pub struct Oni2AnimState {
    pub anim: Oni2Animation,
    pub skeleton: Oni2Skeleton,
    pub current_time: f32,
    pub fps: f32,
    pub paused: bool,
    pub looping: bool,
    pub speed_multiplier: f32,
    pub pending_step: i32, // +1 or -1 for single-frame step, 0 = none
    /// Last time that was rendered — skip joint update if unchanged
    pub last_rendered_time: f32,
    /// Joint entities for GPU skinning (one per bone, flat hierarchy)
    pub joint_entities: Vec<Entity>,
    /// Base rotation of the entity before any animation is applied.
    /// Used by single-channel property animations to compose on top of the original orientation.
    pub base_rotation: Quat,
    /// Blended current frame channel data
    pub current_frame: Vec<f32>,
    /// Currently playing animation ID for efficient comparison
    pub current_anim_id: Option<AnimId>,
    /// The anim ID that was playing JUST BEFORE the most recent `play_id`
    /// call — historical signal only.  Do NOT use `current != previous` to
    /// detect a transition: that comparison stays TRUE for the entire
    /// duration of the current anim (there's nothing to reset `previous`).
    /// Consumers wanting a one-shot "a new anim just started" edge should
    /// read `anim_just_started` instead.
    pub previous_anim_id: Option<AnimId>,
    /// One-tick edge flag: set to `true` by `play_id` when a new anim
    /// begins, cleared back to `false` by `reset_anim_transition_edges`
    /// which runs at the very end of each frame.  Consumers read this
    /// inside their normal Update/FixedUpdate systems and observe a
    /// clean single-tick pulse per anim transition.
    pub anim_just_started: bool,
    /// Physics/grounding state tracked for use by animation FX dispatch
    pub is_grounded: bool,
    pub material_stood_on: Option<String>,

    // --- Captured root motion ---------------------------------------------
    //
    // When the playback system strips root XZ translation from channel data
    // (gait Normalize=2, or the loop-heuristic fallback for looping anims),
    // the discarded translation represents the animation's intended root
    // motion — what the character's root bone WOULD have been displaced by
    // this frame if we let the anim drive movement directly.  We currently
    // drive character movement via Avian velocity instead, so stripping is
    // the right call visually; but discarding the value loses useful data
    // like "how far forward does this lunge animation want me to go".
    //
    // Instead of throwing it away, the playback system stashes it here so
    // gameplay code can fish it out when it wants to:
    //
    //   • `root_offset_this_frame` — this frame's raw channel-space root
    //     XZ offset relative to the skeleton rest pose (Y is always 0, we
    //     never strip vertical).
    //   • `root_offset_prev_frame` — last tick's value, so consumers can
    //     compute a per-frame delta without re-reading the channel buffer.
    //
    // Both are `Vec3::ZERO` when no stripping happened (or when an anim
    // lacks root translation channels entirely).
    /// Raw root XZ translation discarded from this frame's channel data.
    pub root_offset_this_frame: Vec3,
    /// Previous tick's `root_offset_this_frame` — subtract to get a delta.
    pub root_offset_prev_frame: Vec3,
}

impl Oni2AnimState {
    /// Per-frame delta of the stripped root XZ translation.  Useful as a
    /// "this animation wants to move me by this much this tick" signal —
    /// e.g. a running lunge attack could feed this into Avian velocity so
    /// the character travels with the animation.  Returns `Vec3::ZERO`
    /// when the anim doesn't strip root translation or just started.
    pub fn root_motion_delta(&self) -> Vec3 {
        self.root_offset_this_frame - self.root_offset_prev_frame
    }

    /// True iff a non-looping one-shot animation is currently mid-play
    /// (i.e. hasn't reached its final frame).  Used by both the AI
    /// behavior LockedMovement gate (rb/src/behavior/component.cpp:1176
    /// port) and the locomotion gait selector to suppress motion
    /// commands while an attack swing / block / evade / reaction
    /// animation has the actor committed.
    ///
    /// Locomotion gaits are always looping, so a non-looping anim that
    /// hasn't ended is by definition not a gait — it's a one-shot the
    /// actor must finish before any other system replaces it or steers
    /// the body.
    pub fn is_one_shot_in_progress(&self) -> bool {
        let num_frames = self.anim.num_frames as f32;
        if num_frames <= 1.0 {
            return false;
        }
        let last_frame = (num_frames - 1.0).max(0.0);
        !self.looping && self.current_time < last_frame
    }
}

/// Deterministic string hash used as animation identifier.
/// Use `AnimId::new("ANIMNAV_RUN_FORWARD")` in const context — compiles to a u64 literal.
/// Scripts produce the same hash at runtime via `AnimId::new(name)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AnimId(pub u64);

impl AnimId {
    /// FNV-1a 64-bit hash, usable in const context.
    pub const fn new(s: &str) -> Self {
        let bytes = s.as_bytes();
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u64;
            hash = hash.wrapping_mul(0x100000001b3);
            i += 1;
        }
        AnimId(hash)
    }
}

impl std::fmt::Display for AnimId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AnimId(0x{:016x})", self.0)
    }
}

/// Compile-time animation ID from a string literal.
/// Usage: `anim_id!("ANIMNAV_RUN_FORWARD")` compiles to a constant u64.
#[macro_export]
macro_rules! anim_id {
    ($s:literal) => {
        $crate::oni2_loader::AnimId::new($s)
    };
}

/// Animation library mapping AnimId hashes to loaded animations.
/// Built from .anims + .apkg package files.
#[derive(Component, Clone)]
pub struct Oni2AnimLibrary {
    pub anims: std::collections::HashMap<AnimId, Oni2Animation>,
    /// Reverse map for debug: hash -> original alias string
    pub debug_names: std::collections::HashMap<AnimId, String>,
}

impl Oni2AnimLibrary {
    /// Set the active animation by alias string (hashed at runtime).
    pub fn play(&self, alias: &str, state: &mut Oni2AnimState) -> bool {
        self.play_id(AnimId::new(alias), state)
    }

    /// Set the active animation by pre-computed AnimId (zero-cost lookup).
    ///
    /// Resets every transient playback field to defaults (current_time=0.0,
    /// speed_multiplier=1.0, etc.) — callers that want a non-default rate
    /// MUST assign `state.speed_multiplier` explicitly after this returns.
    /// This prevents leftover state (e.g. the jump schedule's rate-match on
    /// VERTICAL_3) from carrying over into a subsequent locomotion anim that
    /// doesn't set its own rate, making idle / walk / run play at the wrong
    /// speed after a jump completes.
    pub fn play_id(&self, id: AnimId, state: &mut Oni2AnimState) -> bool {
        if let Some(anim) = self.anims.get(&id) {
            let new_name = self
                .debug_names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("{:?}", id));
            let prev_name = state
                .current_anim_id
                .and_then(|pid| self.debug_names.get(&pid).cloned());
            println!(
                "PLAY_ANIM: '{}' (looping={}) replacing prev='{}'",
                new_name,
                anim.is_loop,
                prev_name.as_deref().unwrap_or("<none>")
            );
            state.anim = anim.clone();
            state.current_time = 0.0;
            state.looping = anim.is_loop;
            state.speed_multiplier = 1.0;
            state.last_rendered_time = -1.0; // force rebuild
            state.previous_anim_id = state.current_anim_id;
            state.current_anim_id = Some(id);
            // Set the one-shot transition edge so consumer systems see a
            // single-tick pulse this frame.  `reset_anim_transition_edges`
            // clears it at the end of the frame.
            state.anim_just_started = true;

            // Resize current_frame to match animation channel count
            let num_channels = anim.num_channels as usize;
            if state.current_frame.len() != num_channels {
                state.current_frame = vec![0.0; num_channels];
            }

            true
        } else {
            false
        }
    }

    /// Check if an alias exists.
    pub fn has(&self, alias: &str) -> bool {
        self.anims.contains_key(&AnimId::new(alias))
    }

    /// List all available alias names (debug only).
    pub fn aliases(&self) -> Vec<&str> {
        self.debug_names.values().map(|s| s.as_str()).collect()
    }
}

/// Load an animation library for a character entity.
/// Reads the .anims file, resolves package references to .apkg files,
/// loads all referenced .anim files that match the skeleton's channel count.
pub fn load_anim_library(
    entity_dir: &str,
    entity_name: &str,
    skeleton: &Oni2Skeleton,
) -> (
    Oni2AnimLibrary,
    Option<crate::oni2_loader::parsers::loco::LocomotionController>,
    Option<crate::oni2_loader::parsers::jump::JumpController>,
) {
    let mut expected_channels = skeleton.expected_anim_channels();
    if expected_channels == 0 {
        expected_channels = skeleton.positions.len() * 3 + 3;
    }

    let mut alias_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut loco_pkg: Option<String> = None;
    let mut jump_pkg: Option<String> = None;

    // Try entity.tune version first (more complete), then Entity version
    let tune_dir = "entity.tune".to_string();
    let tune_entity = format!("{}/{}", tune_dir, entity_name);
    let tune_anims = format!("{}/{}.anims", tune_entity, entity_name);
    let entity_anims = format!("{}/{}.anims", entity_dir, entity_name);

    info!(
        "Attempting to load anims from tune: {} and fallback entity: {}",
        tune_anims, entity_anims
    );

    let anims_path = if crate::vfs::exists("", &tune_anims) {
        tune_anims
    } else {
        entity_anims
    };

    if let Ok(content) = crate::vfs::read_to_string("", &anims_path) {
        crate::oni2_loader::parsers::anims::parse_anims_content(
            &content,
            &mut alias_map,
            &mut loco_pkg,
            &mut jump_pkg,
        );
    } else {
        info!("No .anims file found for {}", entity_name);
    }

    // Load attack animations from .attacks file (same Anims { } format as .apkg)
    let tune_attacks = format!("entity.tune/{}/{}.attacks", entity_name, entity_name);
    let entity_attacks = format!("{}/{}.attacks", entity_dir, entity_name);
    let attacks_path = if crate::vfs::exists("", &tune_attacks) {
        tune_attacks
    } else if crate::vfs::exists("", &entity_attacks) {
        entity_attacks
    } else {
        String::new()
    };
    if !attacks_path.is_empty() {
        if let Ok(content) = crate::vfs::read_to_string("", &attacks_path) {
            let before = alias_map.len();
            crate::oni2_loader::parsers::anims::parse_anims_content(
                &content,
                &mut alias_map,
                &mut loco_pkg,
                &mut jump_pkg,
            );
            info!(
                "FSM: loaded {} attack aliases from {}",
                alias_map.len() - before,
                attacks_path
            );
        }
    } else {
        info!("No .attacks file found for {}", entity_name);
    }

    // Load the actual .anim files
    let mut anims = std::collections::HashMap::new();
    let mut debug_names = std::collections::HashMap::new();
    let mut loaded = 0;
    let mut skipped_channels = 0;
    let mut skipped_missing = 0;
    for (alias, anim_name) in &alias_map {
        let anim_file = format!("{}/{}.anim", entity_dir, anim_name);

        let data = match crate::vfs::read("", &anim_file) {
            Ok(d) => d,
            Err(_) => {
                // Fallback: If anim starts with a prefix like "kno_" or "tim_", check that entity dir
                if let Some(prefix) = anim_name.split('_').next() {
                    let mut parts: Vec<&str> = entity_dir.split('/').collect();
                    if let Some(last) = parts.last_mut() {
                        *last = prefix;
                    }
                    let fallback_dir = parts.join("/");
                    let fallback_file = format!("{}/{}.anim", fallback_dir, anim_name);
                    if let Ok(d) = crate::vfs::read("", &fallback_file) {
                        d
                    } else {
                        skipped_missing += 1;
                        continue;
                    }
                } else {
                    skipped_missing += 1;
                    continue;
                }
            }
        };
        let mut anim = match parse_anim(&data) {
            Some(a) => a,
            None => {
                skipped_missing += 1;
                continue;
            }
        };

        // Sibling `.gait` file — tiny text file next to the `.anim` that
        // declares the root-motion conditioning mode.  Mirrors the load
        // path in rb/src/animator/animatortype.cpp:695.  Looked up with
        // the same fallback-to-prefix-dir strategy as the .anim above.
        let gait_file = format!("{}/{}.gait", entity_dir, anim_name);
        let mut gait_content = crate::vfs::read_to_string("", &gait_file).ok();
        if gait_content.is_none()
            && let Some(prefix) = anim_name.split('_').next()
        {
            let mut parts: Vec<&str> = entity_dir.split('/').collect();
            if let Some(last) = parts.last_mut() {
                *last = prefix;
            }
            let fallback_gait = format!("{}/{}.gait", parts.join("/"), anim_name);
            gait_content = crate::vfs::read_to_string("", &fallback_gait).ok();
        }
        if let Some(gc) = gait_content
            && let Some(gait) = crate::oni2_loader::parsers::gait::parse_gait(&gc, &gait_file)
        {
            anim.gait_normalize = Some(gait.normalize);
        }

        let tune_atdt = format!("entity.tune/{}/{}.atdt", entity_name, anim_name);
        let base_atdt = format!("{}/{}.atdt", entity_dir, anim_name);

        // Priority 1: entity.tune/kno/..
        // Priority 2: Entity/kno/..
        // Priority 3: fallback to prefix folder (e.g. if the mesh name differs from anim prefix)
        let mut atdt_content = crate::vfs::read_to_string("", &tune_atdt);
        if atdt_content.is_err() {
            atdt_content = crate::vfs::read_to_string("", &base_atdt);
        }
        if atdt_content.is_err()
            && let Some(prefix) = anim_name.split('_').next()
        {
            let fallback_tune = format!("entity.tune/{}/{}.atdt", prefix, anim_name);
            atdt_content = crate::vfs::read_to_string("", &fallback_tune);
            if atdt_content.is_err() {
                let fallback_base = format!("Entity/{}/{}.atdt", prefix, anim_name);
                atdt_content = crate::vfs::read_to_string("", &fallback_base);
            }
        }

        if let Ok(atdt_data) = atdt_content {
            anim.attack_data = Some(crate::oni2_loader::parsers::atdt::parse_atdt_content(
                &atdt_data,
            ));
        }

        if alias.starts_with("ANIMREACT_") {
            let rct_file = format!("{}/{}.rct", entity_dir, alias);
            if let Ok(rct_data) = crate::vfs::read_to_string("", &rct_file) {
                anim.react_data = Some(crate::oni2_loader::parsers::rct::parse_rct_content(
                    &rct_data,
                ));
            } else if let Some(prefix) = anim_name.split('_').next() {
                let mut parts: Vec<&str> = entity_dir.split('/').collect();
                if let Some(last) = parts.last_mut() {
                    *last = prefix;
                }
                let fallback_rct = format!("{}/{}.rct", parts.join("/"), alias);
                if let Ok(rct_data) = crate::vfs::read_to_string("", &fallback_rct) {
                    anim.react_data = Some(crate::oni2_loader::parsers::rct::parse_rct_content(
                        &rct_data,
                    ));
                }
            }
        }
        // Only skip if the anim has more than 1 channel but doesn't match skeleton.
        // Single-channel anims (e.g. simple rotation) are always accepted.
        // Also allow 6 channels for 0-joint skeletons (root pos + rot) like BCTrashTest.
        if anim.num_channels > 1 && anim.num_channels as usize != expected_channels {
            if expected_channels == 3 && anim.num_channels == 6 {
                // Accept 6 channels instead of 3 for rigid body / generic objects
            } else {
                skipped_channels += 1;
                continue;
            }
        }
        let id = AnimId::new(alias);
        anims.insert(id, anim);
        debug_names.insert(id, alias.clone());
        loaded += 1;
    }

    info!(
        "AnimLibrary for {}: {} aliases loaded, {} skipped (channel mismatch), {} missing",
        entity_name, loaded, skipped_channels, skipped_missing
    );

    let locomotion = if let Some(pkg) = loco_pkg {
        // e.g., "animpkg.nav.player" -> "entity.tune/animpkg/nav/player.loco"
        let parts: Vec<&str> = pkg.split('.').collect();
        if parts.len() >= 2 {
            let filename = parts.last().unwrap();
            let mut loco_dir = "entity.tune".to_string();
            for p in &parts[..parts.len() - 1] {
                loco_dir = format!("{}/{}", loco_dir, p);
            }
            let loco_path = format!("{}.loco", filename);
            if let Ok(loco_content) = crate::vfs::read_to_string(&loco_dir, &loco_path) {
                Some(crate::oni2_loader::parsers::loco::parse_loco_content(
                    &loco_content,
                ))
            } else {
                warn!(
                    "Could not read locomotion package: {}/{}",
                    loco_dir, loco_path
                );
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let jump_controller = if let Some(pkg) = jump_pkg {
        // e.g., "animpkg.nav.tim" -> "entity.tune/animpkg/nav/tim.jump"
        let parts: Vec<&str> = pkg.split('.').collect();
        if parts.len() >= 2 {
            let filename = parts.last().unwrap();
            let mut jump_dir = "entity.tune".to_string();
            // Drop the first part "animpkg" if needed, but normally we just path it out
            for p in &parts[..parts.len() - 1] {
                jump_dir = format!("{}/{}", jump_dir, p);
            }
            let jump_path = format!("{}.jump", filename);
            if let Ok(jump_content) = crate::vfs::read_to_string(&jump_dir, &jump_path) {
                Some(crate::oni2_loader::parsers::jump::parse_jump_content(
                    &jump_content,
                ))
            } else {
                warn!("Could not read jump package: {}/{}", jump_dir, jump_path);
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    (
        Oni2AnimLibrary { anims, debug_names },
        locomotion,
        jump_controller,
    )
}

/// Controls visibility of debug collision bounds wireframes.
#[derive(Resource)]
pub struct DebugBoundsVisible(pub bool);

/// Controls visibility of debug skeleton wireframes.
#[derive(Resource)]
pub struct DebugSkeletonVisible(pub bool);

/// Per-pass material handle.  Most passes are PBR diffuse; the `EnvReflect`
/// variant is the sphere-mapped reflection layer used by `texgen reflect`
/// shaders (e.g. the FX Cathedral statue).
#[derive(Clone)]
pub enum PassMaterial {
    Standard(Handle<StandardMaterial>),
    EnvReflect(Handle<crate::env_reflect_material::EnvReflectMaterial>),
}

/// Stores the parsed model for mesh rebuilding (point cloud toggle etc).
#[derive(Component)]
pub struct Oni2ModelData {
    pub model: Oni2Model,
    pub material_handles: Vec<Vec<PassMaterial>>,
    pub fallback_material: Handle<StandardMaterial>,
}

/// Toggle debug bounds with F3.
pub fn toggle_debug_bounds(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut visible: ResMut<DebugBoundsVisible>,
) {
    if keyboard.just_pressed(KeyCode::F3) {
        visible.0 = !visible.0;
    }
}

/// Draw wireframe bounds using the actual bound edge geometry.
pub fn debug_draw_bounds(
    query: Query<(
        &Transform,
        &Oni2DebugBounds,
        Option<&Oni2AnimState>,
        Option<&crate::oni2_loader::spawn::CreatureRenderOffset>,
    )>,
    joints: Query<&GlobalTransform>,
    mut gizmos: Gizmos,
    visible: Res<DebugBoundsVisible>,
) {
    if !visible.0 {
        return;
    }
    let color = Color::srgb(0.0, 1.0, 0.0);
    for (transform, bounds, anim_state, creature_offset) in &query {
        // Character physics centers are above ground by physics_center_height; their Oni bounds
        // are ground-relative, so subtract that to re-align. Level geometry has no correction.
        let y_correction = creature_offset
            .map(|o| o.physics_center_height)
            .unwrap_or(0.0);
        let base_translation = transform.translation - Vec3::new(0.0, y_correction, 0.0);

        for sub in &bounds.sub_bounds {
            let mut use_joint = false;
            let mut joint_tf = Transform::IDENTITY;

            if let Some(state) = anim_state
                && let Some(&j_ent) = state.joint_entities.get(sub.bone_idx)
                && let Ok(gtf) = joints.get(j_ent)
            {
                let (_, rot, trans) = gtf.to_scale_rotation_translation();
                joint_tf = Transform::from_translation(trans).with_rotation(rot);
                use_joint = true;
            }

            for edge in &sub.edges {
                if let (Some(&va), Some(&vb)) = (
                    sub.vertices.get(edge[0] as usize),
                    sub.vertices.get(edge[1] as usize),
                ) {
                    let (wa, wb) = if use_joint {
                        (joint_tf * va, joint_tf * vb)
                    } else {
                        (
                            base_translation + transform.rotation * va,
                            base_translation + transform.rotation * vb,
                        )
                    };
                    gizmos.line(wa, wb, color);
                }
            }
        }
    }
}

/// Draw wireframe capsules for all physics colliders when debug bounds are visible (F3).
pub fn debug_draw_capsules(
    query: Query<(&Transform, &Collider)>,
    mut gizmos: Gizmos,
    visible: Res<DebugBoundsVisible>,
) {
    if !visible.0 {
        return;
    }
    let color = Color::srgb(0.0, 1.0, 1.0); // cyan to distinguish from green bounds
    for (transform, _collider) in &query {
        // Draw capsule wireframe: two circles (top/bottom of cylinder) + vertical lines
        // Capsule(radius, height) where height is the cylinder portion
        // We approximate with the standard capsule dimensions used: radius=0.4, half_height=0.6
        let radius = 0.4_f32;
        let half_height = 0.6_f32;
        let pos = transform.translation;
        let segments = 16;

        // Top and bottom circle centers
        let top_center = pos + Vec3::Y * half_height;
        let bot_center = pos - Vec3::Y * half_height;

        // Draw circles at top and bottom of cylinder section
        for i in 0..segments {
            let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
            let dx0 = a0.cos() * radius;
            let dz0 = a0.sin() * radius;
            let dx1 = a1.cos() * radius;
            let dz1 = a1.sin() * radius;

            // Top circle
            gizmos.line(
                top_center + Vec3::new(dx0, 0.0, dz0),
                top_center + Vec3::new(dx1, 0.0, dz1),
                color,
            );
            // Bottom circle
            gizmos.line(
                bot_center + Vec3::new(dx0, 0.0, dz0),
                bot_center + Vec3::new(dx1, 0.0, dz1),
                color,
            );

            // Top hemisphere arc (vertical)
            let pa0 = (i as f32 / segments as f32) * std::f32::consts::PI;
            let pa1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::PI;
            // Front-back arc
            gizmos.line(
                top_center + Vec3::new(0.0, pa0.sin() * radius, pa0.cos() * radius),
                top_center + Vec3::new(0.0, pa1.sin() * radius, pa1.cos() * radius),
                color,
            );
            // Bottom hemisphere arc
            gizmos.line(
                bot_center + Vec3::new(0.0, -pa0.sin() * radius, pa0.cos() * radius),
                bot_center + Vec3::new(0.0, -pa1.sin() * radius, pa1.cos() * radius),
                color,
            );
        }

        // Vertical lines connecting top and bottom
        for i in [0, 4, 8, 12] {
            let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let dx = angle.cos() * radius;
            let dz = angle.sin() * radius;
            gizmos.line(
                top_center + Vec3::new(dx, 0.0, dz),
                bot_center + Vec3::new(dx, 0.0, dz),
                color,
            );
        }
    }
}

/// Draw wireframe layouts representing active tracking `CurveFollower` states,
/// as well as the overarching static `LayoutPaths` for unactivated curves.
pub fn debug_draw_curves(
    query: Query<&CurveFollower>,
    layout_paths: Option<Res<crate::oni2_loader::environment::LayoutPaths>>,
    mut gizmos: Gizmos,
    visible: Res<DebugBoundsVisible>,
) {
    if !visible.0 {
        return;
    }

    let active_color = Color::srgb(1.0, 0.0, 1.0); // magenta

    for follower in &query {
        let segments = 50;
        let mut prev_pos = follower.curve.get_curve_point(0.0);
        for i in 1..=segments {
            let t = i as f32 / segments as f32;
            let current_pos = follower.curve.get_curve_point(t);
            gizmos.line(prev_pos, current_pos, active_color);
            prev_pos = current_pos;
        }

        // Draw the current phase position as a sphere
        let current_pos = follower.curve.get_curve_point(follower.phase);
        gizmos.sphere(Isometry3d::from_translation(current_pos), 1.0, Color::WHITE);
    }

    // Draw all layout curves (unactivated or active in the base pool)
    let inactive_color = Color::srgb(1.0, 0.5, 0.0); // orange
    if let Some(paths) = layout_paths {
        for (_name, pts) in &paths.curves {
            if pts.len() < 2 {
                continue;
            }
            let mut prev_pos = pts[0];
            for i in 1..pts.len() {
                let current_pos = pts[i];
                gizmos.line(prev_pos, current_pos, inactive_color);
                prev_pos = current_pos;
            }
            gizmos.sphere(
                Isometry3d::from_translation(pts[0]),
                0.5,
                Color::srgb(1.0, 1.0, 0.0),
            );
        }
    }
}

/// Toggle debug skeleton with F4.
pub fn toggle_debug_skeleton(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut visible: ResMut<DebugSkeletonVisible>,
) {
    if keyboard.just_pressed(KeyCode::F4) {
        visible.0 = !visible.0;
    }
}

/// Draw skeleton bones as lines from child to parent, with spheres at joints.
pub fn debug_draw_skeleton(
    query: Query<(
        &Transform,
        &Oni2DebugSkeleton,
        Option<&CreatureRenderOffset>,
    )>,
    trigger_query: Query<&crate::scroni::vm::BroadcastTrigger>,
    mut gizmos: Gizmos,
    visible: Res<DebugSkeletonVisible>,
) {
    if !visible.0 {
        return;
    }
    let bone_color = Color::srgb(1.0, 1.0, 0.0);
    let joint_color = Color::srgb(1.0, 0.3, 0.0);

    for (transform, skel, offset) in &query {
        let base_offset = offset.map_or(Vec3::ZERO, |o| Vec3::new(0.0, o.y_offset, 0.0));

        // Draw lines from each bone to its parent
        for (i, parent_idx) in skel.parent_indices.iter().enumerate() {
            let pos = skel.positions[i] + base_offset;
            let world_pos = transform.transform_point(pos);

            // Draw joint sphere
            gizmos.sphere(Isometry3d::from_translation(world_pos), 0.008, joint_color);

            // Draw line to parent
            if let Some(pi) = parent_idx {
                let parent_pos = skel.positions[*pi] + base_offset;
                let world_parent = transform.transform_point(parent_pos);
                gizmos.line(world_pos, world_parent, bone_color);
            }
        }
    }

    let trigger_color = Color::srgba(1.0, 0.5, 0.0, 0.5); // Semi-transparent orange

    let _trigger_count = trigger_query.iter().count();

    for trigger in &trigger_query {
        let world_pos = trigger.world_center;
        gizmos.sphere(
            Isometry3d::from_translation(world_pos),
            trigger.radius,
            trigger_color,
        );
    }
}

/// Draw wireframe attack wedges (hit volumes) during the active frames of attacks.
pub fn debug_draw_attack_wedges(
    query: Query<(
        &Transform,
        &Oni2AnimState,
        &crate::combat::components::Fighter,
        Option<&crate::oni2_loader::spawn::CreatureRenderOffset>,
    )>,
    mut gizmos: Gizmos,
    visible: Res<DebugBoundsVisible>,
) {
    if !visible.0 {
        return;
    }

    let _color = Color::srgba(1.0, 0.0, 0.0, 0.8);

    for (transform, anim_state, fighter, _offset_opt) in &query {
        let Some(data) = &anim_state.anim.attack_data else {
            // ATDT not loaded for this animation — check VFS path if wedge is expected
            // debug!("debug_draw_attack_wedges: no attack_data for anim id={:?}", anim_state.current_anim_id);
            continue;
        };
        let Some(strike) = &data.strike else {
            continue;
        };

        let frame = anim_state.current_time;
        let num_frames = anim_state.anim.num_frames as f32;

        // Determine hit-active window. ATDT may omit framenum/frameduration (defaulting to 0);
        // fall back to minradiusframe/maxradiusframe (also raw frames), then the whole animation.
        let (win_start, win_end) = if strike.frameduration > 0.0 {
            (strike.framenum, strike.framenum + strike.frameduration)
        } else if strike.maxradiusframe > strike.minradiusframe {
            (strike.minradiusframe, strike.maxradiusframe)
        } else {
            (0.0, (num_frames - 1.0).max(0.0))
        };

        let in_window = frame >= win_start && frame <= win_end;
        // Red = active hit window, yellow = outside (whole-animation dim view)
        let draw_color = if in_window {
            Color::srgba(1.0, 0.0, 0.0, 0.9)
        } else {
            Color::srgba(1.0, 0.8, 0.0, 0.35)
        };

        // Character physics capsule centers are hardcoded to ground + 1.1m.
        let attacker_feet_y = transform.translation.y - 1.1;
        let attacker_base = Vec3::new(transform.translation.x, attacker_feet_y, transform.translation.z);

        let wedge = crate::combat::hitbox::EvaluatedWedge::evaluate(
            strike,
            attacker_base,
            fighter.facing,
            frame,
            num_frames,
        );

        if !wedge.is_active {
            continue;
        }

        // `wedge.center.y` comes from `attacker_base.y` (= feet).  The
        // actual hit VOLUME lives between `wedge.min_y` and `wedge.max_y`
        // (attacker feet + reactdiskheight ± tolerance).  Draw the arcs
        // at BOTH those heights + vertical risers so the gizmo reflects
        // the cylinder slice the hit-test actually evaluates — otherwise
        // the wedge renders flat on the floor and looks like it's missing.
        let radius = wedge.max_radius;
        let inner_r = wedge.inner_radius.max(0.0);
        let start_rad = wedge.start_rad;
        let end_rad = wedge.end_rad;

        let sweep = end_rad - start_rad;
        let full_circle = sweep.abs() < 1e-4 || sweep.abs() >= std::f32::consts::TAU * 0.99;

        // Anchor the debug drawing at the hit band's actual Y range, not
        // at the wedge's feet-anchored `center`.
        let center_xz = Vec3::new(wedge.center.x, 0.0, wedge.center.z);
        let bottom_center = center_xz + Vec3::Y * wedge.min_y;
        let top_center = center_xz + Vec3::Y * wedge.max_y;

        let get_point = |anchor: Vec3, angle: f32, r: f32| -> Vec3 {
            // Both slice_heading_xz and the angle limits are in Bevy space.
            let dir = Quat::from_rotation_y(angle) * wedge.slice_heading_xz;
            anchor + Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero() * r
        };

        let draw_wedge_arc = |gizmos: &mut Gizmos, anchor: Vec3, r: f32, color: Color| {
            if r <= 0.01 {
                return;
            }
            let segments = 24;
            if full_circle {
                let step = std::f32::consts::TAU / segments as f32;
                let mut prev_pt = anchor + Vec3::new(r, 0.0, 0.0);
                for i in 1..=segments {
                    let a = i as f32 * step;
                    let next_pt = anchor + Vec3::new(a.cos() * r, 0.0, a.sin() * r);
                    gizmos.line(prev_pt, next_pt, color);
                    prev_pt = next_pt;
                }
            } else {
                let angle_step = sweep / segments as f32;
                let pt_start = get_point(anchor, start_rad, r);
                let mut prev_pt = pt_start;
                for i in 1..=segments {
                    let angle = start_rad + angle_step * i as f32;
                    let next_pt = get_point(anchor, angle, r);
                    gizmos.line(prev_pt, next_pt, color);
                    prev_pt = next_pt;
                }
            }
        };

        // Draw outer arc at both heights.
        draw_wedge_arc(&mut gizmos, bottom_center, radius, draw_color);
        draw_wedge_arc(&mut gizmos, top_center, radius, draw_color);

        // Draw inner cut at both heights.
        if inner_r > 0.01 {
            let cyan = Color::srgba(0.0, 1.0, 1.0, 0.8);
            draw_wedge_arc(&mut gizmos, bottom_center, inner_r, cyan);
            draw_wedge_arc(&mut gizmos, top_center, inner_r, cyan);
        }

        // Side walls connecting the two height arcs (inner + outer edges
        // at start/end angles) make the volume readable.
        if !full_circle {
            let b_outer_start = get_point(bottom_center, start_rad, radius);
            let b_outer_end = get_point(bottom_center, end_rad, radius);
            let t_outer_start = get_point(top_center, start_rad, radius);
            let t_outer_end = get_point(top_center, end_rad, radius);
            let b_inner_start = if inner_r > 0.01 {
                get_point(bottom_center, start_rad, inner_r)
            } else {
                bottom_center
            };
            let b_inner_end = if inner_r > 0.01 {
                get_point(bottom_center, end_rad, inner_r)
            } else {
                bottom_center
            };
            let t_inner_start = if inner_r > 0.01 {
                get_point(top_center, start_rad, inner_r)
            } else {
                top_center
            };
            let t_inner_end = if inner_r > 0.01 {
                get_point(top_center, end_rad, inner_r)
            } else {
                top_center
            };

            // Start-angle radial lines (bottom & top).
            gizmos.line(b_inner_start, b_outer_start, draw_color);
            gizmos.line(t_inner_start, t_outer_start, draw_color);
            // End-angle radial lines (bottom & top).
            gizmos.line(b_inner_end, b_outer_end, draw_color);
            gizmos.line(t_inner_end, t_outer_end, draw_color);

            // Vertical risers at the four corners of the slice.
            gizmos.line(b_outer_start, t_outer_start, draw_color);
            gizmos.line(b_outer_end, t_outer_end, draw_color);
            gizmos.line(b_inner_start, t_inner_start, draw_color);
            gizmos.line(b_inner_end, t_inner_end, draw_color);
        } else {
            // Full-circle case: four vertical risers at cardinal directions
            // so the ring's vertical extent is still visible.
            for (dx, dz) in [(radius, 0.0), (-radius, 0.0), (0.0, radius), (0.0, -radius)] {
                let b = bottom_center + Vec3::new(dx, 0.0, dz);
                let t = top_center + Vec3::new(dx, 0.0, dz);
                gizmos.line(b, t, draw_color);
            }
        }
    }
}

/// Linearly interpolate between two animation frames, storing the result in `state.current_frame`.
pub fn frame_lerp(state: &mut Oni2AnimState, idx_a: usize, idx_b: usize, t: f32) {
    let Oni2AnimState {
        anim,
        current_frame,
        skeleton,
        ..
    } = state;

    let a = &anim.frames[idx_a];
    let b = &anim.frames[idx_b];
    let len = current_frame.len().min(a.len()).min(b.len());

    let rot_flags = &skeleton.channel_is_rot;

    for i in 0..len {
        let is_rot = if rot_flags.len() >= len {
            rot_flags[i]
        } else {
            // Legacy fallback if no explicit channel map exists
            if len == 1 {
                false
            } else {
                !(3..6).contains(&i)
            }
            //if len == 1 { false } else { i >= 3 }
        };

        if is_rot {
            // Shortest path interpolation for angles
            let mut diff = (b[i] - a[i]).rem_euclid(std::f32::consts::TAU);
            if diff > std::f32::consts::PI {
                diff -= std::f32::consts::TAU;
            }
            current_frame[i] = a[i] + diff * t;
        } else {
            // Linear interpolation for translations
            current_frame[i] = a[i] + (b[i] - a[i]) * t;
        }
    }
}

/// Update animation: advance frame, recompute bone transforms, rebuild meshes.
pub fn update_oni2_animation(
    time: Res<Time>,
    mut anim_query: Query<(
        Entity,
        &mut Oni2AnimState,
        Option<&mut Oni2DebugSkeleton>,
        Option<&CreatureRenderOffset>,
        Option<&crate::oni2_loader::components::ActorAsleep>,
    )>,
    mut transform_query: Query<(
        &mut Transform,
        Option<&mut avian3d::prelude::LinearVelocity>,
        Option<&mut avian3d::prelude::AngularVelocity>,
    )>,
) {
    for (entity, mut anim_state, debug_skel, render_offset, asleep_opt) in &mut anim_query {
        // Diagnostic: dump every anim transition's playback config exactly
        // once per transition (cleared at end of frame by
        // `reset_anim_transition_edges`).  Lets us see if a do_attack-issued
        // anim arrived with fps=0 / speed_multiplier=0 / paused=true / etc.,
        // which would explain why current_time never advances.
        if anim_state.anim_just_started {
            bevy::log::trace!(
                "anim_play[{:?}]: num_frames={} fps={} speed={} paused={} looping={} asleep={}",
                entity,
                anim_state.anim.num_frames,
                anim_state.fps,
                anim_state.speed_multiplier,
                anim_state.paused,
                anim_state.looping,
                asleep_opt.is_some(),
            );
        }

        if asleep_opt.is_some() {
            continue;
        }

        let num_frames = anim_state.anim.num_frames as usize;
        if num_frames <= 1 {
            continue;
        }

        if anim_state.paused {
            if anim_state.pending_step == 0 {
                continue;
            }
            // Single-frame step
            let cur = anim_state.current_time as i32 + anim_state.pending_step;
            anim_state.current_time = cur.rem_euclid(num_frames as i32) as f32;
            anim_state.pending_step = 0;
        } else {
            // Advance time
            anim_state.current_time +=
                time.delta_secs() * anim_state.fps * anim_state.speed_multiplier;
            if anim_state.looping {
                if anim_state.current_time >= num_frames as f32 {
                    anim_state.current_time %= num_frames as f32;
                }
            } else {
                anim_state.current_time = anim_state.current_time.min(num_frames as f32 - 1.0);
            }
        }
        let frame_idx = (anim_state.current_time as usize).min(anim_state.anim.frames.len() - 1);
        let mut next_idx = frame_idx + 1;
        if next_idx >= anim_state.anim.frames.len() {
            if anim_state.looping {
                next_idx = 0;
            } else {
                next_idx = frame_idx;
            }
        }
        let blend = anim_state.current_time - (anim_state.current_time as usize) as f32;

        // Skip joint update if time hasn't changed
        if anim_state.current_time == anim_state.last_rendered_time {
            continue;
        }
        anim_state.last_rendered_time = anim_state.current_time;

        // Ensure current_frame has correct capacity
        let expected_len = anim_state.anim.frames[frame_idx].len();
        if anim_state.current_frame.len() != expected_len {
            anim_state.current_frame = vec![0.0; expected_len];
        }

        frame_lerp(&mut anim_state, frame_idx, next_idx, blend);

        let frame = &anim_state.current_frame;

        // Single-channel animation: apply as Y-rotation composed on top of base orientation
        if anim_state.anim.num_channels == 1 {
            if let Some(y_rot) = frame.first()
                && let Ok((mut tf, mut opt_lvel, mut opt_avel)) = transform_query.get_mut(entity)
            {
                let new_rot = anim_state.base_rotation * Quat::from_rotation_y(*y_rot);

                let dt = time.delta_secs();
                if dt > 0.0001 {
                    if let Some(lv) = opt_lvel.as_deref_mut() {
                        lv.0 = Vec3::ZERO;
                    }
                    if let Some(av) = opt_avel.as_deref_mut() {
                        let diff = new_rot * tf.rotation.inverse();
                        let (axis, angle) = diff.to_axis_angle();
                        let mut angular_vel = axis * (angle / dt);
                        if angular_vel.is_nan() {
                            angular_vel = Vec3::ZERO;
                        }
                        av.0 = angular_vel;
                    }
                }

                tf.rotation = new_rot;
            }
            continue;
        }

        // Decide whether to strip root XZ translation from channel data this
        // tick.  Priority:
        //   1. If the anim has a `.gait` file, honour its Normalize value —
        //      `Root` (2) strips; every other value preserves.  Mirrors
        //      rb/src/animator/animatortype.cpp:722.
        //   2. Otherwise fall back to the legacy loop heuristic (strip while
        //      looping, preserve on one-shots) — a reasonable default for
        //      creatures where no .gait file ships.
        //
        // The stripped root translation used to be discarded outright.  We
        // now capture it into `anim_state.root_offset_this_frame` so
        // gameplay can read `anim_state.root_motion_delta()` each tick and
        // optionally apply that displacement (e.g. for a running lunge).
        // See `Oni2AnimState` doc for the field contract.  The C++
        // equivalent was `ComputeFrameDeltas()` at load time; we compute
        // lazily at playback instead.
        let strip_root = match anim_state.anim.gait_normalize {
            Some(mode) => mode.should_strip_root(),
            None => render_offset.is_some() && anim_state.looping,
        };
        let bone_result = crate::oni2_loader::utils::bone::compute_animated_bone_transforms(
            &anim_state.skeleton,
            frame,
            strip_root,
        );
        let bone_transforms = bone_result.bones;
        // Shift prev→this_frame, record the newly-stripped offset.  Zero
        // when the anim didn't strip (non-stripping anims produce no root
        // motion signal — gameplay should treat delta==0 as "no anim-driven
        // displacement this tick").
        anim_state.root_offset_prev_frame = anim_state.root_offset_this_frame;
        anim_state.root_offset_this_frame = bone_result.stripped_root_offset;

        // Creature render offset (capsule Y compensation + facing)
        let y_offset = render_offset.map(|o| o.y_offset).unwrap_or(0.0);
        let facing = render_offset.map(|o| o.facing).unwrap_or(Quat::IDENTITY);

        // Update joint entity transforms for GPU skinning
        for (i, (rot, pos)) in bone_transforms.iter().enumerate() {
            if let Some(&joint_entity) = anim_state.joint_entities.get(i)
                && let Ok((mut joint_tf, mut opt_lvel, mut opt_avel)) =
                    transform_query.get_mut(joint_entity)
            {
                // Convert from Oni2 coordinates to Bevy: negate X and Z
                let bevy_pos = space::to_bevy_space_pos(Vec3::new(pos.x, pos.y + y_offset, pos.z));
                // Conjugate rotation by 180° Y rotation: negate X and Z components
                let bevy_rot = Quat::from_xyzw(-rot.x, rot.y, -rot.z, rot.w);
                // Apply facing rotation (if model needs to be rotated)
                let final_rot = facing * bevy_rot;
                let final_pos = facing * bevy_pos;

                // Support native kinematic frictional bindings for moving objects
                let dt = time.delta_secs();
                if dt > 0.0001 {
                    if let Some(lv) = opt_lvel.as_deref_mut() {
                        lv.0 = (final_pos - joint_tf.translation) / dt;
                    }
                    if let Some(av) = opt_avel.as_deref_mut() {
                        let diff = final_rot * joint_tf.rotation.inverse();
                        let (axis, angle) = diff.to_axis_angle();
                        let mut angular_vel = axis * (angle / dt);
                        if angular_vel.is_nan() {
                            angular_vel = Vec3::ZERO;
                        }
                        av.0 = angular_vel;
                    }
                }

                *joint_tf = Transform::from_translation(final_pos).with_rotation(final_rot);
            }
        }

        // Update debug skeleton positions
        if let Some(mut ds) = debug_skel {
            ds.positions = bone_transforms
                .iter()
                .map(|(_, pos)| space::to_bevy_space_pos(pos))
                .collect();
        }
    }
}
