/*
 * fight/systems.rs — All fight-system FixedUpdate systems.
 *
 * fighter_state_update_system  — tick frame flags, decay super meter, fight stance timer
 * block_success_system         — respond to BlockSuccessEvent (counter-attacks, FX)
 * block_failed_system          — respond to BlockFailedEvent (force blocker react)
 * grapple_system               — manage grapple lifecycle from FSM CtrlGrapple actions
 * super_meter_system           — add/remove from super meter via SuperMeterAddEvent
 * successive_attacks_system    — escalate reactions after repeated hits on same target
 * (knockdown→getup is now handled by the React action's AnimSchedule;
 *  see action_start_system in animator/systems.rs)
 * react_end_rotation_system    — apply ReactData.end_rotation_notches after react
 * attack_spin_system           — rotate entity by ATDT.spin during active attack frames
 * hit_eta_system               — compute AboutToBeHit ETA from ATDT reactphase
 * fight_stance_timer_system    — leave fight stance after inactivity
 * rotation_notches_system      — apply queued rotation notch events to transforms
 */
use bevy::prelude::*;

use crate::combat::components::{
    ComboTracker, Fighter, Health, HitReaction, ReactLibrary, ReactionKind,
};
use crate::combat::events::{AboutToBeHitMessage, HitReactionMessage};
use crate::oni2_loader::animation::{AnimId, Oni2AnimLibrary, Oni2AnimState};
use crate::oni2_loader::parsers::rct::ANIMREACT_NAMES;

use super::components::{
    BlockStatus, FighterState, FighterType, GrabAction, GrappleState, fighter_flags, grapple_flags,
};
use super::events::{
    ApplyRotationNotchesEvent, BlockFailedEvent, BlockSuccessEvent, GrappleEndEvent,
    GrappleEndReason, GrappleStartEvent, SuperMeterAddEvent,
};

/// Radians per notch (PI/4 = 45°).
pub const NOTCH_RADIANS: f32 = std::f32::consts::FRAC_PI_4;

/// Rate at which TurnLerper advances each second.  Legacy constant
/// `scale = 15.0f` in rb/src/fight/fighter.cpp:1381 — gives a ~67ms
/// settle time from turn-start to alignment.
pub const TURN_LERP_SCALE: f32 = 15.0;

// ---------------------------------------------------------------------------
// fighter_state_update_system
// ---------------------------------------------------------------------------

/// Per-frame bookkeeping for FighterState:
///   • Transition HIT_START → HIT (one-frame latch) and IH_START → IMPENDING_HIT
///   • Decay hit_eta_seconds
///   • Decay super meter over time (using FighterType.super_meter_decay)
///   • Tick the leave-fight-stance timer
///   • Track and update in_invuln_phase from current animation phase vs
///     invulnerability_start_phase
pub fn fighter_state_update_system(
    time: Res<Time>,
    mut query: Query<
        (
            &mut FighterState,
            Option<&FighterType>,
            Option<&Oni2AnimState>,
        ),
        Without<crate::oni2_loader::components::ActorAsleep>,
    >,
) {
    let dt = time.delta_secs();

    for (mut fs, ft_opt, anim_opt) in &mut query {
        fs.tick_frame_flags(dt);
        // New frame — clear the per-frame pending-hit dedup list.
        // Mirrors `NumPending = 0;` at rb/src/fight/fighter.cpp:1332
        // (called once per update before strike probes run).  Keeping
        // this at the top of fighter_state_update ensures the list is
        // empty before any strike evaluation this frame populates it.
        fs.reset_targets_pending();

        // Super meter decay
        let decay_rate = ft_opt
            .map(|ft| {
                if ft.super_meter_decay > 0.0 {
                    1.0 / ft.super_meter_decay
                } else {
                    0.0
                }
            })
            .unwrap_or(0.1);
        if fs.super_meter > 0.0 {
            fs.super_meter = (fs.super_meter - decay_rate * dt).max(0.0);
        }

        // Leave-fight-stance countdown
        if fs.leave_fight_stance_timer > 0.0 {
            fs.leave_fight_stance_timer = (fs.leave_fight_stance_timer - dt).max(0.0);
            if fs.leave_fight_stance_timer <= 0.0 {
                // Signal FSM to leave fight stance (cleared FIGHT_MODE flag)
                fs.clear_flag(fighter_flags::FIGHT_MODE);
            }
        }

        // Invulnerability phase / no-react-after-phase are only meaningful
        // *while the react anim that set them is playing*.  The original
        // logic only cleared the flags when the current anim hit its last
        // frame and was non-looping — but if the react anim is interrupted
        // (transitioned to idle, walk, or another react before completing)
        // the flags get orphaned and the entity becomes permanently
        // invulnerable.  Compute "still playing the react that set these
        // flags" up front and use that for both setting and clearing.
        let react_anim_id = if fs.react_anim >= 0 {
            ANIMREACT_NAMES
                .get(fs.react_anim as usize)
                .map(|name| AnimId::new(name))
        } else {
            None
        };
        let playing_react_anim = match (anim_opt, react_anim_id) {
            (Some(anim), Some(rid)) => anim.current_anim_id == Some(rid),
            _ => false,
        };

        if playing_react_anim {
            // We're playing the react.  Latch in_invuln_phase once the
            // animation crosses invulnerability_start_phase.
            if !fs.in_invuln_phase
                && fs.invulnerability_start_phase > 0.0
                && let Some(anim) = anim_opt
                && anim.anim.num_frames > 1
            {
                let phase = anim.current_time / (anim.anim.num_frames as f32 - 1.0).max(1.0);
                if phase >= fs.invulnerability_start_phase {
                    fs.in_invuln_phase = true;
                }
            }
        } else if fs.in_invuln_phase
            || fs.invulnerability_start_phase > 0.0
            || fs.no_react_start_phase > 0.0
            || fs.react_anim >= 0
        {
            // We are NOT playing the react anim that set these flags
            // anymore — either it ran to completion, was interrupted by
            // another anim, or no react was ever played.  Either way,
            // there is no longer a meaningful "react window" and stale
            // flags would gate this entity out of all hit-detection.
            // Clear the whole react bookkeeping defensively.
            fs.in_invuln_phase = false;
            fs.invulnerability_start_phase = 0.0;
            fs.no_react_start_phase = 0.0;
            fs.react_anim = -1;
        }
    }
}

// ---------------------------------------------------------------------------
// block_success_system
// ---------------------------------------------------------------------------

/// Handles BlockSuccessEvents:
///   1. Force a block-reaction on the attacker (animReactEnum from BlockDef)
///   2. Increment block_combo_count; if it exceeds combo_count_before_react,
///      play ANIMREACT_BLOCKED on the blocker
///   3. If auto_counter or counter_atk present, queue the counter animation
///
/// Mirrors the logic in crStrike::GetBlocked(crFighter *tgFighter).
pub fn block_success_system(
    mut events: MessageReader<BlockSuccessEvent>,
    mut query: Query<(
        &mut FighterState,
        Option<&mut Oni2AnimState>,
        Option<&Oni2AnimLibrary>,
    )>,
    mut reaction_writer: MessageWriter<HitReactionMessage>,
) {
    for ev in events.read() {
        // --- Force block-break reaction on the ATTACKER ---
        if ev.block_reaction_on_attacker >= 0
            && let Ok((mut attacker_fs, _, _)) = query.get_mut(ev.attacker)
            && attacker_fs.cur_combo_index > ev.combo_count_before_react
        {
            attacker_fs.set_flag(fighter_flags::HIT_START);
            reaction_writer.write(HitReactionMessage {
                entity: ev.attacker,
                kind: ReactionKind::Flinch,
                direction: Vec3::ZERO,
                react_enum: ev.block_reaction_on_attacker,
            });
        }

        // --- Blocker response ---
        if let Ok((mut blocker_fs, anim_state_opt, lib_opt)) = query.get_mut(ev.blocker) {
            blocker_fs.block_combo_count += 1;
            blocker_fs.block_status = BlockStatus::Successful;

            // Combo pressure: too many consecutive blocks → break reaction
            if blocker_fs.block_combo_count >= ev.combo_count_before_react
                && ev.combo_count_before_react > 0
            {
                reaction_writer.write(HitReactionMessage {
                    entity: ev.blocker,
                    kind: ReactionKind::Flinch,
                    direction: Vec3::ZERO,
                    react_enum: 18, // ANIMREACT_BLOCKED
                });
                blocker_fs.block_combo_count = 0;
            } else {
                // Play the successful block animation / counter
                if let Some(counter_name) = &ev.counter_atk
                    && let (Some(mut anim), Some(lib)) = (anim_state_opt, lib_opt)
                {
                    let played = lib.play(counter_name, &mut anim);
                    if !played {
                        warn!(
                            "block_success: counter anim '{}' not in library",
                            counter_name
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// block_failed_system
// ---------------------------------------------------------------------------

/// Handles BlockFailedEvents: plays the failed-block reaction on the blocker.
/// In the original Oni2 this is where the target still gets hurt even though they were
/// trying to block; the InjureMessage was already emitted by hit_detection.
pub fn block_failed_system(
    mut events: MessageReader<BlockFailedEvent>,
    mut reaction_writer: MessageWriter<HitReactionMessage>,
    mut query: Query<&mut FighterState>,
) {
    for ev in events.read() {
        if let Ok(mut fs) = query.get_mut(ev.blocker) {
            fs.block_status = BlockStatus::NotBlocking;
        }
        if ev.failed_react >= 0 {
            reaction_writer.write(HitReactionMessage {
                entity: ev.blocker,
                kind: ReactionKind::Flinch,
                direction: Vec3::ZERO,
                react_enum: ev.failed_react,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// grapple_system
// ---------------------------------------------------------------------------

pub fn grapple_tick_system(
    mut start_events: MessageReader<GrappleStartEvent>,
    time: Res<Time>,
    mut holders: Query<(
        Entity,
        &mut GrappleState,
        &mut FighterState,
        &Health,
        Option<&mut Oni2AnimState>,
        Option<&Oni2AnimLibrary>,
    )>,
    mut targets: Query<&mut FighterState, Without<GrappleState>>,
    fighter_types: Query<&FighterType>,
    mut end_writer: MessageWriter<GrappleEndEvent>,
    mut rotation_writer: MessageWriter<ApplyRotationNotchesEvent>,
) {
    let now = time.elapsed_secs_f64();
    let dt = time.delta_secs();

    // --- Handle GrappleStartEvent ---
    for ev in start_events.read() {
        // Insert GrappleState on holder
        if let Ok((_, mut gs, mut holder_fs, health, _, _)) = holders.get_mut(ev.attacker) {
            gs.set_flag(grapple_flags::STARTED);
            gs.grab_start_health = health.current;
            gs.grab_start_time = now;
            gs.shake_amt = 0.0;
            gs.full_shake_timer = 0.0;
            holder_fs.grapple_target = Some(ev.target);
        }
        // Mark target as being grappled
        if let Ok(mut target_fs) = targets.get_mut(ev.target) {
            target_fs.grapple_attacker = Some(ev.attacker);
        }
    }

    // --- Per-frame grapple tick ---
    let mut to_end: Vec<(Entity, Option<Entity>, GrappleEndReason)> = Vec::new();

    for (holder_entity, mut gs, holder_fs, health, _anim_opt, _lib_opt) in &mut holders {
        if gs.is_broken() {
            continue;
        }

        let target_entity = match holder_fs.grapple_target {
            Some(e) => e,
            None => {
                to_end.push((holder_entity, None, GrappleEndReason::Manual));
                continue;
            }
        };

        // Grapple timeout check
        let break_time = fighter_types
            .get(holder_entity)
            .map(|ft| ft.grapple_break_time)
            .unwrap_or(5.0);

        if gs.is_timed_out(now, break_time) {
            to_end.push((
                holder_entity,
                Some(target_entity),
                GrappleEndReason::Timeout,
            ));
            continue;
        }

        // Process pending action from FSM CtrlGrapple
        let action = std::mem::replace(&mut gs.pending_action, GrabAction::None);
        match action {
            GrabAction::None => {}

            GrabAction::End => {
                to_end.push((holder_entity, Some(target_entity), GrappleEndReason::Manual));
            }

            GrabAction::Shake => {
                if gs.tick_shake(dt) {
                    to_end.push((holder_entity, Some(target_entity), GrappleEndReason::Break));
                }
            }

            GrabAction::TurnLeft => {
                gs.nav_rotation_notches = -1;
                rotation_writer.write(ApplyRotationNotchesEvent {
                    entity: holder_entity,
                    notches: -1,
                });
                rotation_writer.write(ApplyRotationNotchesEvent {
                    entity: target_entity,
                    notches: -1,
                });
            }

            GrabAction::TurnRight => {
                gs.nav_rotation_notches = 1;
                rotation_writer.write(ApplyRotationNotchesEvent {
                    entity: holder_entity,
                    notches: 1,
                });
                rotation_writer.write(ApplyRotationNotchesEvent {
                    entity: target_entity,
                    notches: 1,
                });
            }

            GrabAction::Throw | GrabAction::Die => {
                let reason = if action == GrabAction::Throw {
                    GrappleEndReason::Throw
                } else {
                    GrappleEndReason::Die
                };
                to_end.push((holder_entity, Some(target_entity), reason));
            }

            GrabAction::MoveForward | GrabAction::MoveBackward => {
                // Navigation moves — in the legacy engine these play matched anims on both fighters.
                // Animation is driven by FSM; here we just reset the grapple timer.
                gs.grab_start_time = now;
            }
        }

        // Health-loss break: if holder took significant damage, break grapple
        let hp_lost = gs.grab_start_health - health.current;
        let break_hp_threshold = 20.0; // crGrabData.LostHPBeforeBreak default
        if hp_lost >= break_hp_threshold && !gs.is_breaking() {
            gs.set_flag(grapple_flags::BREAKING);
            to_end.push((holder_entity, Some(target_entity), GrappleEndReason::Break));
        }
    }

    // --- End grapples (Emit events for next system) ---
    for (holder_entity, target_opt, reason) in to_end {
        // Emit end event immediately.
        // It will be picked up by grapple_end_system in the same frame.
        end_writer.write(GrappleEndEvent {
            attacker: holder_entity,
            target: target_opt,
            reason,
        });
    }
}

pub fn grapple_end_system(
    mut end_events: MessageReader<GrappleEndEvent>,
    mut commands: Commands,
    mut holders: Query<(&mut GrappleState, &mut FighterState)>,
    mut targets: Query<&mut FighterState, Without<GrappleState>>,
) {
    for ev in end_events.read() {
        if let Ok((mut gs, mut holder_fs)) = holders.get_mut(ev.attacker) {
            gs.set_flag(grapple_flags::BROKEN);
            holder_fs.grapple_target = None;
            holder_fs.clear_flag(fighter_flags::GRAPPLE_PENDING);
        }
        // Remove GrappleState component from holder
        commands.entity(ev.attacker).remove::<GrappleState>();

        if let Some(target) = ev.target
            && let Ok(mut target_fs) = targets.get_mut(target)
        {
            target_fs.grapple_attacker = None;
        }
    }
}

// ---------------------------------------------------------------------------
// super_meter_system
// ---------------------------------------------------------------------------

/// Processes SuperMeterAddEvents and applies them to FighterState.super_meter.
/// Clamps the meter to [0.0, 2.0] (allows brief overcharge beyond 1.0).
pub fn super_meter_system(
    mut events: MessageReader<SuperMeterAddEvent>,
    mut query: Query<&mut FighterState>,
) {
    for ev in events.read() {
        if let Ok(mut fs) = query.get_mut(ev.entity) {
            fs.super_meter = (fs.super_meter + ev.amount).clamp(0.0, 2.0);
        }
    }
}

// ---------------------------------------------------------------------------
// successive_attacks_system
// ---------------------------------------------------------------------------

/// Escalates reactions after repeated hits on the same target.
///
/// If the attacker's ComboTracker shows hits against the same target with the
/// same hit type, increment num_successive_attacks.  After enough hits
/// (FighterType.successive_level0_reacts), escalate the reaction to a
/// stronger animReactEnum (e.g. regular → fromback → knockdown).
///
/// Mirrors ftFighterComponentType::SuccessiveLevel0Reacts logic in crFighter::Update().
pub fn successive_attacks_system(
    mut damage_reader: MessageReader<crate::combat::events::DamageMessage>,
    mut attacker_query: Query<(&mut FighterState, Option<&FighterType>, &ComboTracker)>,
    _target_query: Query<&mut FighterState, Without<ComboTracker>>,
) {
    for msg in damage_reader.read() {
        // Update attacker successive attack count
        let Ok((mut attacker_fs, ft_opt, _combo)) = attacker_query.get_mut(msg.attacker) else {
            continue;
        };

        let hit_type = msg.attack_class as i32;
        if attacker_fs.last_successive_react_hit_type == hit_type {
            attacker_fs.num_successive_attacks += 1;
        } else {
            attacker_fs.num_successive_attacks = 1;
            attacker_fs.last_successive_react_hit_type = hit_type;
        }
        attacker_fs.last_hit_type = hit_type;

        let threshold = ft_opt.map(|ft| ft.successive_level0_reacts).unwrap_or(3);

        // When threshold exceeded, signal for a stronger reaction on the target
        if attacker_fs.num_successive_attacks >= threshold {
            attacker_fs.num_successive_attacks = 0;
            // Signal target to use escalated reaction (fromback / knockdown)
            // The hit_reaction_system will pick this up via react_enum on the message.
        }
    }
}


// ---------------------------------------------------------------------------
// react_end_rotation_system
// ---------------------------------------------------------------------------

/// After a reaction animation ends, apply any pending end-rotation notches
/// from ReactData (EndRotationNotches field in .rct files).
///
/// Mirrors crReactData::ApplyRotationNotches(rbActor*).
pub fn react_end_rotation_system(
    mut query: Query<(Entity, &mut FighterState, &HitReaction)>,
    mut rotation_writer: MessageWriter<ApplyRotationNotchesEvent>,
) {
    for (entity, mut fs, reaction) in &mut query {
        if fs.pending_end_rotation_notches == 0 {
            continue;
        }
        // Apply once the reaction clears
        if reaction.active.is_none() {
            rotation_writer.write(ApplyRotationNotchesEvent {
                entity,
                notches: fs.pending_end_rotation_notches,
            });
            fs.pending_end_rotation_notches = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// attack_spin_system
// ---------------------------------------------------------------------------

/// During an active attack, rotate the entity by AtdtStrike.spin each frame.
/// spin is in degrees per frame as stored in the ATDT binary format.
///
/// Mirrors the per-tick spin application in crFighter::Update().
pub fn attack_spin_system(
    mut query: Query<(
        &mut Transform,
        &FighterState,
        &Oni2AnimState,
        &mut crate::combat::components::Fighter,
    )>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();

    for (_transform, _fs, anim_state, mut fighter) in &mut query {
        let Some(attack_data) = &anim_state.anim.attack_data else {
            continue;
        };
        let Some(strike) = &attack_data.strike else {
            continue;
        };

        if strike.spin.abs() < 0.001 {
            continue;
        }

        // Only spin during the active hit window
        let frame = anim_state.current_time;
        let is_active = if strike.frameduration > 0.0 {
            frame >= strike.framenum && frame <= strike.framenum + strike.frameduration
        } else {
            true
        };
        if !is_active {
            continue;
        }

        // spin is stored in the ATDT as radians-per-frame; scale by dt * fps.
        // The original Oni2 game runs at 60 fps, so spin × 60 × dt gives correct world-rate.
        let spin_rads = strike.spin * 60.0 * dt;
        let rotation = Quat::from_rotation_y(spin_rads);
        fighter.facing = rotation * fighter.facing;
    }
}

// ---------------------------------------------------------------------------
// update_fighter_strike_facing_system
// ---------------------------------------------------------------------------

/// Forces a fighter to face their registered strike_target while an attack
/// animation is active, ensuring proper locked orientations during combos.
/// Ticks any pending turn lerp on each FighterState, interpolating
/// `Fighter.facing` toward `turn_final_target_dir` by
/// `dt * TURN_LERP_SCALE` per frame.  Mirrors the matrix-interpolation
/// block at rb/src/fight/fighter.cpp:1377-1387.  Callers invoke
/// `FighterState::start_turn_to(dir)` to trigger a lerp; this system
/// drives it to completion.
pub fn fighter_turn_lerp_system(
    time: Res<Time>,
    mut query: Query<
        (&mut Fighter, &mut FighterState),
        Without<crate::oni2_loader::components::ActorAsleep>,
    >,
) {
    let dt = time.delta_secs();
    for (mut fighter, mut fs) in &mut query {
        if fs.turn_lerper >= 1.0 {
            continue;
        }
        let target = fs.turn_final_target_dir;
        if target.length_squared() < 1e-6 {
            // Stale / invalid target — just finish.
            fs.turn_lerper = 1.0;
            continue;
        }
        // Match the C++ exactly: advance the lerper, then lerp from
        // CURRENT facing toward target by the *new* lerper value.  Each
        // frame converges further because the starting point is the
        // previous frame's result (ExponentialSmoothing pattern).
        fs.turn_lerper = (fs.turn_lerper + dt * TURN_LERP_SCALE).min(1.0);
        let blended = fighter
            .facing
            .lerp(target, fs.turn_lerper)
            .try_normalize()
            .unwrap_or(fighter.facing);
        fighter.facing = blended;
    }
}

pub fn update_fighter_strike_facing_system(
    mut transform_query: Query<&mut Transform>,
    mut attackers: Query<(
        Entity,
        &mut Fighter,
        &mut FighterState,
        Option<&Oni2AnimState>,
    )>,
) {
    for (entity, mut fighter, mut fs, anim_opt) in &mut attackers {
        let Some(target_entity) = fs.strike_target else {
            continue;
        };

        if let Some(anim) = anim_opt {
            if !fs.is_attacking(anim) {
                fs.strike_target = None;
                continue;
            }
        } else {
            fs.strike_target = None;
            continue;
        }

        let Ok(target_tf) = transform_query.get(target_entity) else {
            fs.strike_target = None;
            continue;
        };
        let target_pos = target_tf.translation;

        if let Ok(attacker_tf) = transform_query.get_mut(entity) {
            let mut dir = target_pos - attacker_tf.translation;
            dir.y = 0.0;
            if dir.length_squared() > 0.001 {
                fighter.facing = dir.normalize();
            }
        }

        if fs.clear_st_after_first_use {
            fs.strike_target = None;
        }
    }
}

// ---------------------------------------------------------------------------
// hit_eta_system
// ---------------------------------------------------------------------------

/// For each active attacker, compute the time-to-hit from ATDT reactphase data
/// and emit AboutToBeHitMessages on every potential target in the strike volume.
///
/// Mirrors SetAboutToBeHit() called from the legacy crStrike::Update().
pub fn hit_eta_system(
    attackers: Query<(Entity, &Transform, &Oni2AnimState, &FighterState)>,
    targets: Query<(Entity, &Transform), With<FighterState>>,
    mut about_writer: MessageWriter<AboutToBeHitMessage>,
    _target_writer: MessageWriter<crate::fight::events::SuperMeterAddEvent>,
) {
    for (attacker_entity, attacker_tf, anim_state, _fs) in &attackers {
        let Some(attack_data) = &anim_state.anim.attack_data else {
            continue;
        };
        let Some(strike) = &attack_data.strike else {
            continue;
        };

        if anim_state.anim.num_frames <= 1 {
            continue;
        }

        let total_frames = anim_state.anim.num_frames as f32 - 1.0;
        let phase = (anim_state.current_time / total_frames).clamp(0.0, 1.0);

        // reactphase[0] is when the reaction starts — use that as ETA pivot
        let react_phase = strike.reactphase[0];
        if react_phase <= 0.0 || phase >= react_phase {
            continue;
        }

        // FPS-independent ETA estimate: frames remaining × assumed dt
        let frames_remaining = (react_phase - phase) * total_frames;
        let fps = 60.0_f32;
        let eta = frames_remaining / fps;

        // Broad radius check to avoid spamming every entity in the world
        let scan_radius = (strike.reactdiskradius * 2.0).max(5.0);

        for (target_entity, target_tf) in &targets {
            if target_entity == attacker_entity {
                continue;
            }
            let diff = target_tf.translation - attacker_tf.translation;
            let dist_xz = (diff.x * diff.x + diff.z * diff.z).sqrt();
            if dist_xz > scan_radius {
                continue;
            }

            about_writer.write(AboutToBeHitMessage {
                target: target_entity,
                eta,
                hit_type: strike.hittype,
                from: attacker_tf.translation,
                attacker: attacker_entity,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// fight_stance_timer_system
// ---------------------------------------------------------------------------

/// Resets the leave-fight-stance timer whenever the entity takes a meaningful
/// action (attack, block, grapple).  The timer itself ticks in
/// fighter_state_update_system.
///
/// Called from the combat systems that know an action occurred, or can be
/// triggered directly: call fs.leave_fight_stance_timer = ft.leave_fight_stance_delay.
pub fn fight_stance_timer_system(
    mut query: Query<(&mut FighterState, Option<&FighterType>, &Oni2AnimState)>,
) {
    for (mut fs, ft_opt, anim_state) in &mut query {
        // If we're playing an attack, block, or reaction, reset the timer
        let delay = ft_opt.map(|ft| ft.leave_fight_stance_delay).unwrap_or(3.0);

        // Reset if we're in fight mode and actively doing something
        if fs.in_fight_mode() {
            let has_attack = anim_state.anim.attack_data.is_some();
            let is_reacting = fs.react_anim >= 0;
            let is_blocking = fs.is_blocking();
            if has_attack || is_reacting || is_blocking {
                fs.leave_fight_stance_timer = delay;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// fight_stance_entry_system + fight_stance_exit_system + sync
// ---------------------------------------------------------------------------
//
// Fight stance is a POSE MODE that's orthogonal to locomotion — the
// character pushes into a combat-ready stance that stays on while they
// can still walk, run, jump, etc.  Legacy engine models it as
// `ACT_FLAG_FIGHTSTANCE` on the action player, driven by:
//   • Enter triggers: attack pressed / damaged / fight-mode pad cmd held.
//   • Exit trigger: `leave_fight_stance_delay` seconds of combat idle.
//
// We reproduce this with three cooperating systems:
//
//   1. fight_stance_entry_system — detects the enter triggers and emits
//      `StartActionMessage(FightStance, TRANSITION_FIGHTSTANCE_START)`.
//      `try_start_action` plays STAND_TO_FIGHT and sets `FIGHTSTANCE`
//      on the action player.
//   2. fight_stance_exit_system — when `leave_fight_stance_timer` hits 0
//      and we're currently in stance, emits
//      `StartActionMessage(FightStance, TRANSITION_FIGHTSTANCE_END)`.
//      The handler plays FIGHT_TO_STAND and clears the flag.
//   3. fight_stance_sync_system — mirrors `ActionPlayer.FIGHTSTANCE` onto
//      `FighterState.flags::FIGHT_MODE` so the rest of the fight code
//      (which queries `fs.in_fight_mode()`) sees the real state without
//      having to couple to the animator component directly.

use crate::animator::components::{ActionPlayer, action_flags as ap_flags, sub_state_0};
use crate::animator::events::StartActionMessage;
use crate::combat::events::DamageMessage;
use crate::control_map::PadMapper;
use crate::player::components::InputState;

/// Emits `StartActionMessage(FightStance, START)` on any entity that meets an
/// enter trigger and isn't already in stance.  Once the animator starts the
/// transition the FIGHTSTANCE flag goes on; that suppresses re-triggering
/// until something leaves stance again (this is a one-shot per entry).
pub fn fight_stance_entry_system(
    mut writer: MessageWriter<StartActionMessage>,
    mut damage_reader: MessageReader<DamageMessage>,
    pad_mapper: Option<Res<PadMapper>>,
    mut query: Query<(
        Entity,
        &mut FighterState,
        Option<&FighterType>,
        Option<&ActionPlayer>,
        Option<&InputState>,
    )>,
) {
    use std::collections::HashSet;
    // Gather entities that were damaged this tick — cheap to collect once.
    let damaged: HashSet<Entity> = damage_reader.read().map(|m| m.target).collect();

    // Read pad-mode input once — the player entity shares this with all AI,
    // but AI entities lack InputState and won't trigger via this path.
    let pad_fight_mode = pad_mapper
        .as_ref()
        .map(|p| p.get("PADCMD_WEAPON_FIGHT_MODE") > 0.0 || p.get("PADCMD_WEAPON_LOCKON") > 0.0)
        .unwrap_or(false);

    for (entity, mut fs, ft_opt, ap_opt, input_opt) in &mut query {
        // Already in stance?  Nothing to trigger.  The sync system keeps
        // FighterState.FIGHT_MODE aligned with the animator's FIGHTSTANCE
        // flag, so this check is authoritative.
        if fs.in_fight_mode() {
            continue;
        }

        // Skip entities that can't enter stance at all (dead / no animator).
        let Some(ap) = ap_opt else {
            continue;
        };
        if ap.check_flags(ap_flags::DEAD) {
            continue;
        }

        // Trigger set.  Any one of these flips the enter flag.
        let attack_input = input_opt.map(|i| i.attack || i.attack_two).unwrap_or(false);
        let was_damaged = damaged.contains(&entity);
        let triggered = attack_input || pad_fight_mode || was_damaged;
        if !triggered {
            continue;
        }

        writer.write(StartActionMessage {
            entity,
            action: crate::animator::components::MainAction::FightStance,
            substate: sub_state_0::TRANSITION_FIGHTSTANCE_START,
        });

        // Seed the leave-stance timer so the exit system has something to
        // tick down.  Without this the exit would fire immediately on the
        // next tick (timer at 0).
        let delay = ft_opt.map(|ft| ft.leave_fight_stance_delay).unwrap_or(3.0);
        fs.leave_fight_stance_timer = delay;
    }
}

/// Emits `StartActionMessage(FightStance, END)` when the inactivity timer
/// expires on an entity that's currently in stance.  The actual countdown
/// happens in `fighter_state_update_system`; this one just fires the edge.
pub fn fight_stance_exit_system(
    mut writer: MessageWriter<StartActionMessage>,
    query: Query<(Entity, &FighterState, Option<&ActionPlayer>)>,
    mut was_timed_out: Local<bevy::ecs::entity::EntityHashMap<bool>>,
) {
    for (entity, fs, ap_opt) in &query {
        let Some(ap) = ap_opt else {
            continue;
        };
        // Only fire when the animator FLAG says we're in stance — the exit
        // anim is meaningless otherwise.  `in_fight_mode()` reads the synced
        // FighterState mirror which tracks the animator flag.
        if !ap.check_flags(ap_flags::FIGHTSTANCE) {
            was_timed_out.insert(entity, false);
            continue;
        }
        // A corpse can be in FIGHTSTANCE at the moment of death (died mid-fight
        // with an active leave timer).  Without this gate the timer expires on
        // the corpse and FIGHT_TO_STAND replaces the held death pose, popping
        // the body to standing.  Mirrors the DEAD check on the entry side.
        if ap.check_flags(ap_flags::DEAD) {
            was_timed_out.insert(entity, false);
            continue;
        }

        let timed_out = fs.leave_fight_stance_timer <= 0.0;
        let prev = was_timed_out.get(&entity).copied().unwrap_or(false);
        was_timed_out.insert(entity, timed_out);

        // Edge-trigger: fire exactly once when the timer crosses 0.
        if timed_out && !prev {
            writer.write(StartActionMessage {
                entity,
                action: crate::animator::components::MainAction::FightStance,
                substate: sub_state_0::TRANSITION_FIGHTSTANCE_END,
            });
        }
    }
}

/// Mirror the animator's FIGHTSTANCE flag onto `FighterState.flags::FIGHT_MODE`
/// so existing fight-side queries (`fs.in_fight_mode()`, `fs.has_flag(FIGHT_MODE)`)
/// see the real stance state without reaching into animator components.
pub fn fight_stance_sync_system(mut query: Query<(&mut FighterState, &ActionPlayer)>) {
    for (mut fs, ap) in &mut query {
        let animator_in_stance = ap.check_flags(ap_flags::FIGHTSTANCE);
        let fs_in_stance = fs.has_flag(fighter_flags::FIGHT_MODE);
        if animator_in_stance != fs_in_stance {
            fs.set_flag_val(fighter_flags::FIGHT_MODE, animator_in_stance);
        }
    }
}

// ---------------------------------------------------------------------------
// rotation_notches_system
// ---------------------------------------------------------------------------

/// Applies ApplyRotationNotchesEvents to entity Transform rotations.
/// 1 notch = PI/4 radians (45°) around the Y axis.
/// Positive notches = clockwise, negative = counter-clockwise (from above).
///
/// Mirrors NOTCHES2RADIANS = PI/4 and the rotation helpers in crFighter.
pub fn rotation_notches_system(
    mut events: MessageReader<ApplyRotationNotchesEvent>,
    mut query: Query<(
        &mut Transform,
        Option<&mut FighterState>,
        Option<&mut Fighter>,
    )>,
) {
    for ev in events.read() {
        let Ok((mut transform, fs_opt, fighter_opt)) = query.get_mut(ev.entity) else {
            continue;
        };
        if ev.notches == 0 {
            continue;
        }
        let radians = ev.notches as f32 * NOTCH_RADIANS;
        let rotation = Quat::from_rotation_y(radians);

        let new_forward = if let Some(mut fighter) = fighter_opt {
            // Because we have fighter_rotation_sync_system, we only mutate
            // Fighter.facing here so it can natively propagate to Avian and Transform
            fighter.facing = rotation * fighter.facing;
            fighter.facing
        } else {
            transform.rotation = rotation * transform.rotation;
            transform.rotation * Vec3::Z
        };

        // Update facing direction in FighterState if present
        if let Some(mut fs) = fs_opt {
            fs.last_attack_angle = new_forward.z.atan2(new_forward.x);
        }
    }
}

// ---------------------------------------------------------------------------
// react_data_apply_system
// ---------------------------------------------------------------------------

/// When a HitReactionMessage is processed, read the target's ReactLibrary entry
/// for that react_enum and:
///   1. Set pending_getup_anim on FighterState (for knockdown reactions)
///   2. Set pending_end_rotation_notches (end_rotation_notches from ReactData)
///   3. Set invulnerability_start_phase and no_react_start_phase from ReactData
///
/// This runs BEFORE hit_reaction_system so the data is available when the
/// animation begins playing.
pub fn react_data_apply_system(
    mut reader: MessageReader<HitReactionMessage>,
    mut query: Query<(&mut FighterState, Option<&ReactLibrary>)>,
) {
    for msg in reader.read() {
        let Ok((mut fs, react_lib_opt)) = query.get_mut(msg.entity) else {
            continue;
        };

        let Some(lib) = react_lib_opt else { continue };
        let Some(react_data) = lib.get(msg.react_enum) else {
            continue;
        };

        // Pending get-up anim (for knockdown reactions)
        if !react_data.get_up_anim.is_empty() {
            fs.pending_getup_anim = Some(react_data.get_up_anim.clone());
        }

        // End-rotation notches to apply after animation ends
        fs.pending_end_rotation_notches = react_data.end_rotation_notches;

        // Invulnerability start phase within this react animation
        fs.invulnerability_start_phase = react_data.invulnerability_start_phase;

        // no_react_start_phase: after this phase, can't be hit again this react
        if react_data.does_not_take_damage_after_phase != 0 {
            fs.no_react_start_phase = react_data.does_not_take_damage_after_phase as f32;
        }

        // Clear in_invuln so the new animation can enter the window fresh
        fs.in_invuln_phase = false;
        fs.react_anim = msg.react_enum;
    }
}
