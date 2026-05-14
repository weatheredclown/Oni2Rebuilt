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
    BlockLibrary, BlockStatus, FighterState, FighterType, GrabAction, GrappleState, fighter_flags,
    grapple_flags,
};
use super::events::{
    ApplyRotationNotchesEvent, BlockFailedEvent, BlockSuccessEvent, GrappleEndEvent,
    GrappleEndReason, GrappleStartEvent, SuperMeterAddEvent,
};

/// Radians per notch (PI/4 = 45°).
pub const NOTCH_RADIANS: f32 = std::f32::consts::FRAC_PI_4;

/// Rate at which TurnLerper advances each second.  Legacy constant
/// `scale = 15.0f` — gives a ~67ms settle time from turn-start to
/// alignment.
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
        // Mirrors `NumPending = 0;` (called once per update before
        // strike probes run).  Keeping
        // this at the top of fighter_state_update ensures the list is
        // empty before any strike evaluation this frame populates it.
        fs.reset_targets_pending();

        // Disarmer-count rollover: snapshot last frame's writes into
        // the stable read-side counter, then clear the write-side so
        // this frame's `report_disarming()` calls accumulate fresh.
        // Mirrors `NumDisarmersLastFrame = NumDisarmers; NumDisarmers = 0;`.
        fs.num_disarmers_last_frame = fs.num_disarmers;
        fs.num_disarmers = 0;

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
// strike_debug_facing_watcher
// ---------------------------------------------------------------------------

/// Diagnostic system: logs `fighter.facing` changes between ticks for
/// every Fighter entity.  Intended for tracking down which system is
/// rotating a fighter during an attack.  Only logs when facing changes
/// by more than a tiny threshold so it doesn't spam unchanging frames.
///
/// Runs LAST in FixedUpdate so it captures the final post-tick value
/// (after all other systems have had a chance to mutate facing).
///
/// Remove or gate on a feature flag once the bug is found.
pub fn strike_debug_facing_watcher(
    time: Res<Time>,
    mut last_facing: Local<bevy::ecs::entity::EntityHashMap<Vec3>>,
    query: Query<(Entity, &Fighter, Option<&Oni2AnimState>)>,
) {
    let t = time.elapsed_secs();
    for (entity, fighter, anim_opt) in &query {
        let prev = last_facing.get(&entity).copied().unwrap_or(fighter.facing);
        let delta = fighter.facing.distance_squared(prev);
        if delta > 1e-4 {
            let anim_name = anim_opt
                .and_then(|a| a.current_anim_id.as_ref())
                .map(|id| id.to_string())
                .unwrap_or_else(|| "<none>".to_string());
            let anim_frame = anim_opt.map(|a| a.current_time).unwrap_or(0.0);
            info!(
                "STRIKE_DEBUG: facing-changed entity={:?} t={:.3} anim='{}' frame={:.2} prev=({:.3},{:.3}) cur=({:.3},{:.3}) delta_sq={:.6}",
                entity,
                t,
                anim_name,
                anim_frame,
                prev.x, prev.z,
                fighter.facing.x, fighter.facing.z,
                delta,
            );
        }
        last_facing.insert(entity, fighter.facing);
    }
}

// ---------------------------------------------------------------------------
// block_state_sync_system — connect Oni2AnimState transitions to FighterState
//   block bookkeeping, apply BlockDef.rate to anim playback, run the
//   hold-button mid-point loop. Mirrors the bits of crBlockData::StartBlock /
//   StopBlock that aren't already covered by the FSM's `do_block` helper.
// ---------------------------------------------------------------------------

/// Watches every fighter for transitions into and out of a block animation.
///
/// On a transition INTO a block alias (matched against `ANIMBLOCK_NAMES`):
///   * sets `FighterState.cur_block` to the BlockLibrary index,
///   * stamps `block_start_time` for the max-hold timer,
///   * resets per-session counters (`block_combo_count`, `block_counter_executed`),
///   * applies `BlockDef.rate` to the anim's `speed_multiplier`.
///
/// While IN a block, if `BlockDef.max_hold_button > 0` and the actor is still
/// "holding":
///   * AnimMidPoint == AnimMidPointEnd → freeze `current_time` at AnimMidPoint
///     (HOLD mode, crBlockData::StartBlock ANIMLIST_HOLD branch),
///   * AnimMidPoint != AnimMidPointEnd → wrap `current_time` back to
///     AnimMidPoint when it crosses AnimMidPointEnd (LOOP mode, ANIMLIST_LOOP).
///
/// "Holding" is `input.blocking` for the player and an implicit `true` for AI
/// (the FSM is responsible for transitioning AI out of a block by playing a
/// different anim — when the alias leaves the ANIMBLOCK_ set this system
/// notices on the next tick and clears state). The `max_hold_button` timer
/// also forces hold→released even while the button is still down, mirroring
/// the `StopBlocking()` call the legacy `crFighter::StartBlock` makes on
/// hold timeout.
///
/// On a transition OUT (current anim is not a block alias):
///   * clears `cur_block` to -1, `block_status` to NotBlocking, and the
///     hold/counter flags. The exit transition (AnimMidPointEnd → 1.0) is
///     left to the animator's normal forward play, matching the C++
///     `StopBlock` which queues a tail segment from AnimMidPointEnd to 1.0.
pub fn block_state_sync_system(
    time: Res<Time>,
    mut query: Query<(
        &mut Oni2AnimState,
        &mut FighterState,
        &BlockLibrary,
        &Oni2AnimLibrary,
        Option<&crate::player::components::InputState>,
    )>,
) {
    let now = time.elapsed_secs_f64();

    for (mut anim_state, mut fs, block_lib, anim_lib, input_opt) in &mut query {
        // Resolve the currently-playing anim's alias string. AnimId is just
        // an FNV hash; debug_names is the reverse map.
        let current_alias = anim_state
            .current_anim_id
            .and_then(|id| anim_lib.debug_names.get(&id))
            .cloned();
        let block_idx = current_alias
            .as_deref()
            .and_then(crate::oni2_loader::parsers::blk::block_index_for_alias);

        match block_idx {
            None => {
                if fs.cur_block >= 0 {
                    fs.cur_block = -1;
                    fs.block_status = BlockStatus::NotBlocking;
                    fs.block_held = false;
                    fs.block_counter_executed = false;
                }
            }
            Some(idx) => {
                let Some(block) = block_lib.blocks.get(idx as usize) else {
                    continue;
                };

                // Edge: transitioned into a different block (or any block
                // when previously not blocking) — seed the per-session state.
                if fs.cur_block != idx {
                    fs.cur_block = idx;
                    fs.block_status = BlockStatus::Untested;
                    fs.block_start_time = now;
                    fs.block_combo_count = 0;
                    fs.block_counter_executed = false;
                }

                // BlockDef.Rate — legacy engine applies this via
                // `animList.SetRate(Rate)` at the end of `crBlockData::StartBlock`.
                // Cheap idempotent assign — the threshold avoids triggering
                // any change-detection waste.
                if block.rate > 0.0
                    && (anim_state.speed_multiplier - block.rate).abs() > 1e-4
                {
                    anim_state.speed_multiplier = block.rate;
                }

                // Hold-button state. AI has no InputState so we treat them
                // as "always holding"; their FSM ends the block by playing
                // a non-block anim, at which point block_idx becomes None
                // above and we clear state.
                let still_held = input_opt.map(|i| i.blocking).unwrap_or(true);
                let elapsed = (now - fs.block_start_time) as f32;
                let timed_out =
                    block.max_hold_button > 0.0 && elapsed >= block.max_hold_button;
                let held = still_held && !timed_out;
                fs.block_held = held;

                // Hold-loop / hold-freeze: only when BlockDef opts in via
                // max_hold_button > 0 AND we're still in the hold window.
                if block.max_hold_button > 0.0 && held && anim_state.anim.num_frames > 1 {
                    let denom = (anim_state.anim.num_frames as f32 - 1.0).max(1.0);
                    let phase = anim_state.current_time / denom;
                    let mp = block.anim_mid_point;
                    let mpe = block.anim_mid_point_end;

                    if (mp - mpe).abs() < 1e-4 {
                        // ANIMLIST_HOLD: pin at AnimMidPoint.
                        if phase > mp {
                            anim_state.current_time = mp * denom;
                        }
                    } else if phase > mpe {
                        // ANIMLIST_LOOP: wrap mpe → mp.
                        anim_state.current_time = mp * denom;
                    }
                }
                // Once held becomes false (released or hold timer expired),
                // we leave current_time alone — the anim plays through its
                // post-AnimMidPointEnd exit portion normally, matching
                // crBlockData::StopBlock which queues a segment from
                // AnimMidPointEnd → 1.0.
            }
        }
    }
}

// ---------------------------------------------------------------------------
// block_success_system
// ---------------------------------------------------------------------------

/// Handles BlockSuccessEvents — the legacy equivalent is
/// `crStrike::GetBlocked`. Flow:
///   1. If combo pressure on the blocker exceeded `combo_count_before_react`,
///      play the attacker-side break reaction (`block_reaction_on_attacker`,
///      sourced from the attacker's ATDT).
///   2. Mark blocker `BlockStatus::Successful` and increment the
///      consecutive-block counter.
///   3. Play `successful_block_anim` on the blocker (if any) — the legacy
///      `StartSecondaryBlock` path inside `crStrike::GetBlocked`. This is
///      the defender's reaction to a clean block, NOT a counter-attack.
///   4. Only if the BlockDef has `auto_counter` set AND a `counter_atk`
///      is defined, fire the counter on the same tick (player-initiated
///      counters come from input, not this system).
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

            // Combo pressure: too many consecutive blocks → forced
            // break-react on the blocker. Mirrors the legacy
            // `GetCurComboIndex() > GetComboCountBeforeCausingReact()`
            // check inside `crStrike::GetBlocked`.
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
                continue;
            }

            // Pull animator + library once so we can play the
            // successful-block anim and (optionally) the counter without
            // borrowing twice.
            let Some(mut anim) = anim_state_opt else { continue };
            let Some(lib) = lib_opt else { continue };

            // Successful-block anim: the defender's clean-block reaction.
            // Plays on every successful block regardless of auto_counter
            // (mirrors the `StartSecondaryBlock` branch inside the legacy
            // `crStrike::GetBlocked`).
            if let Some(sba_name) = &ev.successful_block_anim
                && !lib.play(sba_name, &mut anim)
            {
                warn!(
                    "block_success: successful_block_anim '{}' not in library",
                    sba_name
                );
            }

            // Auto-counter: only fires when the BlockDef's `auto_counter`
            // flag is set. Without it, the counter_atk is informational
            // (player can still trigger it manually via input) and this
            // system does not play it. Matches the auto-counter branch in
            // the legacy `crFighter::StartBlock`.
            if ev.auto_counter
                && let Some(counter_name) = &ev.counter_atk
                && !lib.play(counter_name, &mut anim)
            {
                warn!(
                    "block_success: counter anim '{}' not in library",
                    counter_name
                );
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
/// `dt * TURN_LERP_SCALE` per frame.  Mirrors the legacy
/// matrix-interpolation block on `crFighter`.  Callers invoke
/// `FighterState::start_turn_to(dir)` to trigger a lerp; this system
/// drives it to completion.
pub fn fighter_turn_lerp_system(
    time: Res<Time>,
    mut query: Query<
        (Entity, &mut Fighter, &mut FighterState),
        Without<crate::oni2_loader::components::ActorAsleep>,
    >,
) {
    let dt = time.delta_secs();
    for (entity, mut fighter, mut fs) in &mut query {
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

/// Compute the slice-heading angle for the current frame of an active
/// strike.  Mirrors the midpoint logic in `EvaluatedWedge::evaluate`:
/// `slice_a = (slicestart + sliceend) / 2`, optionally sweep-lerped
/// toward `sliceheadingradiansb` while `SweepHeading != 0`.
///
/// Extracted from `update_fighter_strike_facing_system` so we can unit-test
/// the rotation math without ECS scaffolding.
pub fn current_slice_heading(
    strike: &crate::oni2_loader::parsers::atdt::AtdtStrike,
    current_time: f32,
) -> f32 {
    let slice_a = (strike.slicestartradians + strike.sliceendradians) * 0.5;
    if strike.sweepheading != 0 {
        let sweep_t = if strike.frameduration > 0.0 {
            ((current_time - strike.framenum) / strike.frameduration).clamp(0.0, 1.0)
        } else {
            1.0
        };
        slice_a + (strike.sliceheadingradiansb - slice_a) * sweep_t
    } else {
        slice_a
    }
}

/// Compute the world-space `fighter.facing` direction that orients an
/// attacker so the wedge for the current strike lands on `target_pos`.
///
/// The wedge math in `EvaluatedWedge::evaluate` builds:
///     wedge_world = Quat::from_rotation_y(slice_heading) * fighter.facing
/// Solving for `fighter.facing` given a desired
/// `wedge_world = (target - attacker).normalize()` yields the inverse
/// rotation applied to `dir`:
///     fighter.facing = Quat::from_rotation_y(-slice_heading) * dir
///
/// Equivalent to (and a port of) the legacy C++ snippet:
///     m.LookAt(target);
///     m.RotateY(slice_heading);
///     mv.SetWorldYRotation(GetRotationFromMatrix(m));
///
/// Returns `None` if `attacker_pos` and `target_pos` are at the same XZ
/// position (degenerate `dir`).
pub fn strike_facing_for_target(
    attacker_pos: Vec3,
    target_pos: Vec3,
    slice_heading: f32,
) -> Option<Vec3> {
    let mut dir = target_pos - attacker_pos;
    dir.y = 0.0;
    if dir.length_squared() < 1e-6 {
        return None;
    }
    let dir_norm = dir.normalize();
    Some(Quat::from_rotation_y(-slice_heading) * dir_norm)
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

        // Need the active anim to read the strike's slice heading and to
        // gate the lock on actually-attacking.
        let Some(anim) = anim_opt else {
            fs.strike_target = None;
            continue;
        };
        if !fs.is_attacking(anim) {
            fs.strike_target = None;
            continue;
        }

        // Release tracking once the strike's anim has passed its
        // `stop_track_frame` mark — this is what lets the
        // `end_rotation_notches` rotation at attack-end take effect
        // cleanly instead of being immediately overridden by the lock.
        // Mirrors `crStrike::Update`:
        //
        //     if (GetCurrentPhase() >= StopTrackPhase) {
        //         Fighter->ClearStrikeTarget();
        //     }
        //
        // The C++ gates this on `FLAG_CLEAR_STRIKETARGET` so an attack
        // that's already locked onto a target from a *previous* hit
        // keeps the lock through the whole anim.  We simplify: any
        // strike with `stop_track_frame > 0` releases at that phase.
        // Without this, the lock kept rotating the body each frame
        // through the recovery tail, and the post-attack
        // `end_rotation_notches` had nothing to act on — the next
        // tick's strike_facing immediately reset facing back to the
        // locked angle.
        if let Some(strike) = anim.anim.attack_data.as_ref().and_then(|d| d.strike.as_ref()) {
            if strike.stop_track_frame > 0.0 && anim.anim.num_frames > 1 {
                let stop_track_phase = strike.stop_track_frame / anim.anim.num_frames as f32;
                let cur_phase =
                    anim.current_time / (anim.anim.num_frames as f32 - 1.0).max(1.0);
                if cur_phase >= stop_track_phase {
                    fs.strike_target = None;
                    continue;
                }
            }
        }

        let Ok(target_tf) = transform_query.get(target_entity) else {
            fs.strike_target = None;
            continue;
        };
        let target_pos = target_tf.translation;

        if let Ok(attacker_tf) = transform_query.get_mut(entity) {
            let slice_heading = anim
                .anim
                .attack_data
                .as_ref()
                .and_then(|d| d.strike.as_ref())
                .map(|strike| current_slice_heading(strike, anim.current_time))
                .unwrap_or(0.0);

            if let Some(new_facing) =
                strike_facing_for_target(attacker_tf.translation, target_pos, slice_heading)
            {
                fighter.facing = new_facing;
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
    query: Query<(Entity, &FighterState, Option<&ActionPlayer>, Option<&Oni2AnimState>)>,
    mut was_ready: Local<bevy::ecs::entity::EntityHashMap<bool>>,
) {
    for (entity, fs, ap_opt, anim_opt) in &query {
        let Some(ap) = ap_opt else {
            continue;
        };
        // Only fire when the animator FLAG says we're in stance — the exit
        // anim is meaningless otherwise.  `in_fight_mode()` reads the synced
        // FighterState mirror which tracks the animator flag.
        if !ap.check_flags(ap_flags::FIGHTSTANCE) {
            was_ready.insert(entity, false);
            continue;
        }
        // A corpse can be in FIGHTSTANCE at the moment of death (died mid-fight
        // with an active leave timer).  Without this gate the timer expires on
        // the corpse and FIGHT_TO_STAND replaces the held death pose, popping
        // the body to standing.  Mirrors the DEAD check on the entry side.
        if ap.check_flags(ap_flags::DEAD) {
            was_ready.insert(entity, false);
            continue;
        }

        // Block the exit while the actor is committed to a one-shot
        // (react/getup/attack/evade). Without this, the leave timer
        // can elapse mid-react and `FIGHT_TO_STAND` clobbers the
        // queued getup animation. Mirrors the legacy
        // `ActionEndFightStance` `if (AnimList.IsPlaying()) return
        // ACT_DENIED` guard. The edge-trigger fires once the actor is
        // both timed-out AND no longer locked, so a react that runs
        // past the leave-timer deadline gracefully exits stance the
        // tick the react finishes.
        let locked = anim_opt
            .map(crate::combat::locked_movement)
            .unwrap_or(false);
        let ready = fs.leave_fight_stance_timer <= 0.0 && !locked;
        let prev = was_ready.get(&entity).copied().unwrap_or(false);
        was_ready.insert(entity, ready);

        // Edge-trigger: fire exactly once on the rising edge of `ready`.
        if ready && !prev {
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
// disarm_apply_system
// ---------------------------------------------------------------------------

/// Drop the victim's currently-held weapon when an attacker's grab-type
/// disarm attack crosses its `removes_weapon_phase`.  Mirrors the
/// `PassedPhase(GetAttackData()->RemovesWeaponPhase)` edge-trigger
/// in `crGrab` — when true, the C++ engine calls
/// `inv->DropCurrentWeapon()` on the victim.
///
/// One-shot per attack: `ActiveAttack.has_disarmed` latches at first
/// crossing and resets when a new attack starts (handled by
/// `attack_sync_system` blanking `active_attack`).  Without this guard
/// the message fires every frame the phase remains exceeded and the
/// victim would drop every weapon they own in one tick.
pub fn disarm_apply_system(
    mut attackers: Query<(
        Entity,
        &Oni2AnimState,
        &mut crate::combat::components::AttackState,
        &FighterState,
    )>,
    inventories: Query<&crate::inventory::components::Inventory>,
    mut drop_writer: MessageWriter<crate::inventory::events::DropWeaponMessage>,
) {
    for (_attacker, anim, mut attack_state, fs) in &mut attackers {
        // No active attack? Nothing to track.
        let Some(active) = attack_state.active_attack.as_mut() else {
            continue;
        };
        if active.has_disarmed {
            continue;
        }

        // Pull the Grab block off the currently-playing animation's
        // attack data.  Non-grab attacks (strikes / ranged) skip.
        let Some(attack_data) = anim.anim.attack_data.as_ref() else {
            continue;
        };
        let Some(grab) = attack_data.grab.as_ref() else {
            continue;
        };
        if !grab.removes_weapon {
            continue;
        }

        // Phase check — only trip once the anim has crossed
        // `removes_weapon_phase`.
        if anim.anim.num_frames <= 1 {
            continue;
        }
        let phase = anim.current_time / (anim.anim.num_frames as f32 - 1.0).max(1.0);
        if phase < grab.removes_weapon_phase {
            continue;
        }

        // Victim is the entity this fighter is grappling.  The legacy
        // path runs the disarm strictly inside an active grapple
        // (the function is on `crGrab` and operates on the held target).
        let Some(victim) = fs.grapple_target else {
            continue;
        };

        // Find the victim's currently-equipped weapon slot.  Skip
        // silently if they have no inventory or are already disarmed.
        let Ok(inv) = inventories.get(victim) else {
            active.has_disarmed = true; // don't keep retrying
            continue;
        };
        let Some(slot_index) = inv.current_weapon else {
            active.has_disarmed = true;
            continue;
        };

        drop_writer.write(crate::inventory::events::DropWeaponMessage {
            entity: victim,
            slot_index,
        });
        active.has_disarmed = true;
        bevy::log::info!(
            "disarm_apply: victim={:?} dropped weapon slot {} at phase={:.2}",
            victim,
            slot_index,
            phase,
        );
    }
}

// ---------------------------------------------------------------------------
// update_disarming_system
// ---------------------------------------------------------------------------

/// Per-AI disarm-decision pass mirroring
/// `aiFightAndShoot::UpdateDisarming`.
///
/// Each tick, every AI fighter that has a current target evaluates
/// whether to commit to disarming based on:
///   • Target's armed state (`Inventory.current_weapon.is_some()`).
///   • How long the target has been armed (`target_drew_weapon_time`).
///   • Distance to target (flat XZ).
///   • Whether the target is aiming at us — not wired yet, treated as
///     `false` until weapon aim state is exposed.  Documented gap.
///   • This fighter's `disarm_tuning` (0..100, higher = more committed).
///   • How many allies have already committed
///     (`target_fs.get_num_disarmers()`).
///   • Whether the target is mid-react (legacy `abh.IsReacting()`).
///
/// When committed, this fighter:
///   • Sets `is_disarming = true` on self.
///   • Calls `target.report_disarming()` so the next tick's count
///     reflects the commitment.
///
/// The legacy companion piece — attack-table swap to `DisarmTable` —
/// requires the full `aifight` coordination layer that is NOT YET
/// PORTED.  Until then the flag is set but no behavioural change
/// follows; downstream consumers can read `fs.is_disarming` and route
/// their attack selection.
///
/// Aiming-at-me detection (`bh->IsAimingAt(Actor->GetGuid())`)
/// needs the weapon-aim state machine + a per-target lock.
/// Filed for follow-up.
pub fn update_disarming_system(
    time: Res<Time>,
    inventories: Query<&crate::inventory::components::Inventory>,
    transforms: Query<&Transform>,
    hit_reactions: Query<&crate::combat::components::HitReaction>,
    ai_fighters: Query<(Entity, &crate::ai::components::AiFighter)>,
    mut fighter_states: Query<
        (&mut FighterState, Option<&FighterType>),
        Without<crate::oni2_loader::components::ActorAsleep>,
    >,
) {
    let now = time.elapsed_secs_f64();

    // ── Pass 1: read-only decision collection ────────────────────────
    // Read each AI's state + their target's state from immutable
    // queries.  Decisions are recorded into a Vec to apply in Pass 2,
    // so we never alias mutable + immutable access to FighterState
    // (Bevy B0001).
    struct Decision {
        entity: Entity,
        target: Entity,
        armed: bool,
        update_drew_time: Option<f64>,
        was_disarming: bool,
        will_disarm: bool,
    }
    let mut decisions: Vec<Decision> = Vec::new();
    let mut clear_to_idle: Vec<Entity> = Vec::new();

    for (entity, ai) in &ai_fighters {
        // Read this AI's own state.  `get` is immutable so we can also
        // read the target's state from the same query.
        let Ok((fs, ft_opt)) = fighter_states.get(entity) else {
            continue;
        };
        let was_disarming = fs.is_disarming;
        let disarm_tuning = ft_opt.map(|t| t.disarm_tuning).unwrap_or(0.0);
        let target_drew_time = fs.target_drew_weapon_time;

        let Some(target) = ai.target else {
            clear_to_idle.push(entity);
            continue;
        };

        // Target's armed state — `current_weapon.is_some()` is our
        // stand-in for `bh->IsWeaponCompletelyDrawn`.
        let armed = inventories
            .get(target)
            .ok()
            .and_then(|inv| inv.current_weapon)
            .is_some();
        let was_armed = target_drew_time >= 0.0;
        let update_drew_time = if armed && !was_armed {
            Some(now)
        } else if !armed {
            Some(-1.0)
        } else {
            None
        };

        // Effective "drew weapon at" — what the next pass will store.
        let effective_drew_time = update_drew_time.unwrap_or(target_drew_time);

        // Flat XZ distance to target.
        let attacker_pos = transforms.get(entity).map(|t| t.translation);
        let target_pos = transforms.get(target).map(|t| t.translation);
        let flat_dist_sq = match (attacker_pos, target_pos) {
            (Ok(a), Ok(b)) => {
                let dx = a.x - b.x;
                let dz = a.z - b.z;
                dx * dx + dz * dz
            }
            _ => f32::INFINITY,
        };

        // ── Disarm decision ──────────────────────────────────────────
        let d = disarm_tuning;
        let mut want_disarm = false;
        if armed && effective_drew_time >= 0.0 {
            let elapsed = (now - effective_drew_time) as f32;
            let q = elapsed - flat_dist_sq * 0.1; // MAGIC!
            let threshold = if d <= 0.0 {
                f32::MAX
            } else if d < 25.0 {
                lerp(d / 25.0, 40.0, 10.0)
            } else if d < 50.0 {
                lerp((d - 25.0) / 25.0, 10.0, 0.0)
            } else {
                lerp((d - 50.0) / 50.0, 0.0, -10.0)
            };
            want_disarm = q > threshold;
        }
        let mut will_disarm = was_disarming || want_disarm;
        if !armed {
            will_disarm = false;
        }

        // Cap by max-disarmers + target-reacting.
        if will_disarm {
            let target_disarmer_count = fighter_states
                .get(target)
                .map(|(tfs, _)| {
                    let mut n = tfs.get_num_disarmers();
                    if was_disarming {
                        n -= 1; // don't double-count our own prior commit
                    }
                    n
                })
                .unwrap_or(0);
            let max_disarmers = if d < 55.0 {
                1
            } else if d < 75.0 {
                2
            } else if d < 85.0 {
                3
            } else if d < 95.0 {
                4
            } else {
                10_000
            };
            let target_reacting = hit_reactions
                .get(target)
                .ok()
                .is_some_and(|hr| hr.active.is_some());
            if target_disarmer_count >= max_disarmers || target_reacting {
                will_disarm = false;
            }
        }

        decisions.push(Decision {
            entity,
            target,
            armed,
            update_drew_time,
            was_disarming,
            will_disarm,
        });
    }

    // ── Pass 2: apply ────────────────────────────────────────────────
    // Mutate each AI's own state, then nudge their target's
    // num_disarmers counter.  Self and target are different entities,
    // so sequential `get_mut` calls don't alias.
    for entity in clear_to_idle {
        if let Ok((mut fs, _)) = fighter_states.get_mut(entity) {
            fs.is_disarming = false;
            fs.target_drew_weapon_time = -1.0;
            fs.num_disarm_frames = 0;
        }
    }

    for d in decisions {
        if let Ok((mut fs, _)) = fighter_states.get_mut(d.entity) {
            if let Some(t) = d.update_drew_time {
                fs.target_drew_weapon_time = t;
            }
            fs.is_disarming = d.will_disarm;
            if d.will_disarm {
                fs.num_disarm_frames += 1;
            } else {
                fs.num_disarm_frames = 0;
            }
            // If target unarmed, also flush state defensively.
            if !d.armed {
                fs.is_disarming = false;
                fs.target_drew_weapon_time = -1.0;
            }
        }

        // Report to target.  Increment when newly committing this
        // frame; decrement when withdrawing a prior commitment so the
        // stable last-frame count doesn't carry a phantom disarmer.
        if d.will_disarm
            && let Ok((mut tfs, _)) = fighter_states.get_mut(d.target)
        {
            tfs.num_disarmers += 1;
        } else if d.was_disarming
            && !d.will_disarm
            && let Ok((mut tfs, _)) = fighter_states.get_mut(d.target)
        {
            tfs.num_disarmers_last_frame = (tfs.num_disarmers_last_frame - 1).max(0);
        }
    }
}

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + (b - a) * t
}

// ---------------------------------------------------------------------------
// react_distance_apply_system
// ---------------------------------------------------------------------------

/// Push the defender across the react animation by their stashed
/// `react_distance` displacement.  Mirrors the per-frame
/// `dist.Scale(ReactDistance,scale)` block on `crFighter` that the
/// legacy mover consumes as a `TRANSLATIONTYPE_REACT` translation.
///
/// We don't have a mover-translation channel; we approximate the same
/// total displacement by writing a constant XZ velocity each tick while
/// the react anim plays.  Velocity = `react_distance / react_duration`
/// so the integrated motion sums back to `react_distance` over the anim.
/// When the react anim is no longer current, `react_distance` is zeroed
/// (mirrors the legacy `ReactDistance.Zero()`).
pub fn react_distance_apply_system(
    mut query: Query<(
        &mut FighterState,
        &mut avian3d::prelude::LinearVelocity,
        Option<&Oni2AnimState>,
    )>,
) {
    for (mut fs, mut vel, anim_opt) in &mut query {
        if fs.react_distance.length_squared() < 1e-6 {
            continue;
        }

        let Some(anim) = anim_opt else {
            // No animator — drop the stashed push so we don't accumulate
            // it on a future hit.
            fs.react_distance = Vec3::ZERO;
            continue;
        };

        // Identify whether the currently-playing animation is the react
        // we set up for.  Use the same lookup pattern as
        // `fighter_state_update_system` for consistency.
        let react_anim_id = if fs.react_anim >= 0 {
            ANIMREACT_NAMES
                .get(fs.react_anim as usize)
                .map(|name| AnimId::new(name))
        } else {
            None
        };
        let playing_react = match react_anim_id {
            Some(rid) => anim.current_anim_id == Some(rid),
            None => false,
        };

        if !playing_react {
            fs.react_distance = Vec3::ZERO;
            continue;
        }

        // Total play time for the react anim at its current speed.
        let fps = anim.fps.max(1.0);
        let total_frames = (anim.anim.num_frames as f32 - 1.0).max(1.0);
        let duration = total_frames / fps;
        if duration < 1e-3 {
            continue;
        }

        let push = fs.react_distance / duration;
        vel.x = push.x;
        vel.z = push.z;
    }
}

// ---------------------------------------------------------------------------
// face_after_run_system
// ---------------------------------------------------------------------------

/// Port of the legacy "face after run" hook on `crFighter`.
///
/// Behaviour:
///   • While throttle is at full and no attack/react/block is playing,
///     accumulate `run_before_face_timer`.
///   • When throttle drops below 0.25 (the legacy threshold) and the
///     entity isn't falling, grappling, or reacting, and the timer has
///     reached `FighterType.run_before_face`, find the creature most
///     directly in front of the entity and start a turn-lerp toward it.
///   • Always reset the timer to 0 in the deceleration branch — matches
///     `RunBeforeFaceTimer=0.0f` in the legacy reset branch.
///
/// Uses the existing `FighterState::start_turn_to` to drive the lerp,
/// which `fighter_turn_lerp_system` then advances to completion.
pub fn face_after_run_system(
    time: Res<Time>,
    others: Query<
        (Entity, &Transform, &Health),
        (
            With<FighterState>,
            Without<crate::oni2_loader::components::ActorAsleep>,
        ),
    >,
    mut query: Query<
        (
            Entity,
            &Transform,
            &Fighter,
            &mut FighterState,
            &FighterType,
            Option<&Oni2AnimState>,
            Option<&avian3d::prelude::LinearVelocity>,
        ),
        Without<crate::oni2_loader::components::ActorAsleep>,
    >,
) {
    let dt = time.delta_secs();
    const FACE_RADIUS: f32 = 3.5; // MaxCloseRadius (legacy)

    for (entity, tf, fighter, mut fs, ft, anim_opt, vel_opt) in &mut query {
        let anim_playing = anim_opt
            .map(|a| a.anim.attack_data.is_some() && !a.paused)
            .unwrap_or(false);
        let reacting = fs.react_anim >= 0;
        let grappling = fs.is_grappling() || fs.is_being_grappled();
        let falling = vel_opt
            .map(|v| v.y < -0.1 && v.y > -50.0)
            .unwrap_or(false);

        // Full-throttle, nothing playing → accumulate timer
        // (legacy: `throttle==1.0f && !anythingplaying`).
        if fighter.throttle >= 0.99 && !anim_playing {
            fs.run_before_face_timer += dt;
            continue;
        }

        // Otherwise (legacy `else` branch).  Only the
        // slow-down sub-branch resets the timer and (maybe) triggers the
        // face-snap.
        if fighter.throttle.abs() < 0.25 && !falling {
            let can_face = !grappling && !reacting && ft.face_after_run;
            if can_face && fs.run_before_face_timer >= ft.run_before_face {
                // Find the creature most directly in front of us within
                // FACE_RADIUS.  Mirrors `GetMostFacedTowardCreature`
                // — pick the smallest |angle diff| between our facing
                // and the direction to each candidate.
                let my_pos = tf.translation;
                let my_facing_angle = fighter.facing.x.atan2(fighter.facing.z);
                let mut best: Option<(f32, Vec3)> = None;
                for (other_entity, other_tf, other_health) in &others {
                    if other_entity == entity {
                        continue;
                    }
                    if other_health.current <= 0.0 {
                        continue;
                    }
                    let mut to_other = other_tf.translation - my_pos;
                    to_other.y = 0.0;
                    let dist2 = to_other.length_squared();
                    if dist2 > FACE_RADIUS * FACE_RADIUS || dist2 < 1e-4 {
                        continue;
                    }
                    let dir = to_other.normalize();
                    let to_angle = dir.x.atan2(dir.z);
                    let mut diff = to_angle - my_facing_angle;
                    while diff > std::f32::consts::PI {
                        diff -= std::f32::consts::TAU;
                    }
                    while diff < -std::f32::consts::PI {
                        diff += std::f32::consts::TAU;
                    }
                    let abs_diff = diff.abs();
                    if best.map(|(d, _)| abs_diff < d).unwrap_or(true) {
                        best = Some((abs_diff, dir));
                    }
                }
                if let Some((_, dir)) = best {
                    fs.start_turn_to(dir);
                }
            }
            fs.run_before_face_timer = 0.0;
        }
        // (partial throttle, not slowing, anim playing, …) → timer
        // persists — matches the legacy "do nothing" path.
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

        // Note: the getup anim itself is wired by `hit_reaction_system`
        // which reads `ReactLibrary` directly to install a 2-entry
        // `AnimSchedule` for the react+getup pair. That read is
        // race-free (no cross-system data hop), so we don't stash the
        // getup name here on `FighterState`.

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

// ---------------------------------------------------------------------------
// Strike-facing math tests
// ---------------------------------------------------------------------------
//
// Unit-test the math that orients an attacker's body to keep their
// wedge on the target.  The four cardinal directions (forward, back,
// left, right) are the high-value cases — earlier regressions:
//   • +slice_heading instead of -slice_heading: forward/back looked
//     fine (rotations self-inverse at 0 and ±π) but side strikes
//     locked 180° wrong.
//   • wrap_angle_to_pi on the slice angles in the parser: split
//     back-kick slice across the ±π discontinuity, collapsing a 78°
//     back wedge into a 282° forward arc.
// Both bugs would be obvious in the test grid below.
//
// Geometry contract: when fighter.facing is fed back into
// `EvaluatedWedge::evaluate` together with the same slice_heading, the
// resulting wedge direction must point at the target.  Tests assert
// this round-trip property directly.

#[cfg(test)]
mod strike_facing_tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    /// Round-trip helper: given an attacker pos, target pos, and
    /// slice_heading, compute the locked fighter.facing and verify the
    /// resulting wedge (= Quat(slice_heading) * facing) actually points
    /// at the target.
    fn assert_wedge_lands_on_target(
        attacker_pos: Vec3,
        target_pos: Vec3,
        slice_heading: f32,
        label: &str,
    ) {
        let facing = strike_facing_for_target(attacker_pos, target_pos, slice_heading)
            .expect("non-degenerate dir");
        let wedge_dir = Quat::from_rotation_y(slice_heading) * facing;
        let mut expected = target_pos - attacker_pos;
        expected.y = 0.0;
        let expected_norm = expected.normalize();
        let dot = wedge_dir.dot(expected_norm);
        assert!(
            dot > 0.999,
            "{}: wedge {:?} should align with dir-to-target {:?} (dot={})",
            label,
            wedge_dir,
            expected_norm,
            dot,
        );
    }

    #[test]
    fn forward_attack_faces_target() {
        // Forward attack: slice_heading = 0, target in front (NEG_Z).
        // Player should rotate to face target directly.
        let facing = strike_facing_for_target(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -5.0), // target straight ahead
            0.0,
        )
        .unwrap();
        assert!(
            facing.dot(Vec3::NEG_Z) > 0.999,
            "forward attack should leave fighter facing NEG_Z (toward target); got {:?}",
            facing,
        );
        assert_wedge_lands_on_target(Vec3::ZERO, Vec3::new(0.0, 0.0, -5.0), 0.0, "forward");
    }

    #[test]
    fn back_attack_faces_away_from_target() {
        // Back kick: slice_heading = π in Bevy (after-parser midpoint
        // of a back-wedge ATDT; equivalent to -π since rotations are
        // mod 2π).  Target behind player (POS_Z).
        // Expected: player keeps facing forward (NEG_Z), so the
        // back-foot wedge stays glued to the target behind them.
        let facing = strike_facing_for_target(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 5.0), // target behind
            PI,
        )
        .unwrap();
        assert!(
            facing.dot(Vec3::NEG_Z) > 0.999,
            "back attack should leave fighter facing NEG_Z (forward); got {:?}",
            facing,
        );
        assert_wedge_lands_on_target(Vec3::ZERO, Vec3::new(0.0, 0.0, 5.0), PI, "back");
        // -π is the same rotation as +π — verify symmetry.
        assert_wedge_lands_on_target(Vec3::ZERO, Vec3::new(0.0, 0.0, 5.0), -PI, "back-neg");
    }

    #[test]
    fn right_attack_target_ends_on_right() {
        // Right-side attack: slice_heading = -π/2 in Bevy (C++ +π/2
        // CW → negated by oni2_to_bevy_yaw_rads).  Target on player's
        // right (POS_X).  Expected: player faces forward (NEG_Z),
        // target stays to their right.
        let facing = strike_facing_for_target(
            Vec3::ZERO,
            Vec3::new(5.0, 0.0, 0.0), // target on right
            -FRAC_PI_2,
        )
        .unwrap();
        assert!(
            facing.dot(Vec3::NEG_Z) > 0.999,
            "right-side attack should leave fighter facing NEG_Z (forward); got {:?} (this fails if the earlier +slice_heading bug returns)",
            facing,
        );
        assert_wedge_lands_on_target(
            Vec3::ZERO,
            Vec3::new(5.0, 0.0, 0.0),
            -FRAC_PI_2,
            "right",
        );
    }

    #[test]
    fn left_attack_target_ends_on_left() {
        // Left-side attack: slice_heading = +π/2 in Bevy (C++ -π/2 CW
        // → negated).  Target on player's left (NEG_X).
        // Expected: player faces forward (NEG_Z), target on their left.
        let facing = strike_facing_for_target(
            Vec3::ZERO,
            Vec3::new(-5.0, 0.0, 0.0), // target on left
            FRAC_PI_2,
        )
        .unwrap();
        assert!(
            facing.dot(Vec3::NEG_Z) > 0.999,
            "left-side attack should leave fighter facing NEG_Z (forward); got {:?}",
            facing,
        );
        assert_wedge_lands_on_target(
            Vec3::ZERO,
            Vec3::new(-5.0, 0.0, 0.0),
            FRAC_PI_2,
            "left",
        );
    }

    #[test]
    fn diagonal_attack_round_trip() {
        // Diagonal: slice_heading is the parsed midpoint of
        // kno_atk_comb_rht_p_elbowLH (slicestart 1.26 / sliceend 1.58
        // negate-and-sort → midpoint ≈ -1.42).  Target on player's
        // forward-right at +X+(-Z).  The lock should rotate the player
        // so this off-axis attack lands on the right diagonal target.
        let slice_heading = -1.42_f32; // Bevy-space midpoint
        let target = Vec3::new(3.5, 0.0, -2.0);
        let facing =
            strike_facing_for_target(Vec3::ZERO, target, slice_heading).unwrap();
        assert_wedge_lands_on_target(Vec3::ZERO, target, slice_heading, "right-diagonal");

        // The fighter should NOT end up facing the target directly
        // (that would be the slice_heading = 0 case).  It should be
        // rotated by +1.42 rad from the direction-to-target.  Verify
        // by checking the cross product is non-trivial.
        let to_target = target.normalize();
        let cross = facing.cross(to_target).y;
        assert!(
            cross.abs() > 0.1,
            "diagonal attack facing {:?} should NOT align with to_target {:?}; got cross.y={}",
            facing,
            to_target,
            cross,
        );
    }

    #[test]
    fn slice_heading_midpoint_back_kick_data() {
        // Regression: real back-kick ATDT data
        // (kno_atk_BCH1_lft_kick.atdt) carries angles outside
        // [-π, π] — slicestart=-3.829, sliceend=-2.467 in the file.
        // After `oni2_to_bevy_yaw_rads` (negate only — no wrap):
        // +3.829, +2.468; sort: +2.468, +3.829; midpoint ≈ +3.149 ≈ π.
        // `current_slice_heading` should NOT collapse this to 0.
        let strike = crate::oni2_loader::parsers::atdt::AtdtStrike {
            framenum: 0.0,
            frameduration: 1.0,
            slicestartradians: 2.468,
            sliceendradians: 3.829,
            sliceheadingradiansb: 0.0,
            sweepheading: 0,
            ..Default::default()
        };
        let h = current_slice_heading(&strike, 0.5);
        // Should resolve to ≈π regardless of sign — rotation-equivalent
        // to "directly behind."
        let heading_vec = Quat::from_rotation_y(h) * Vec3::NEG_Z;
        assert!(
            heading_vec.dot(Vec3::Z) > 0.95,
            "back-kick slice midpoint should produce a back-facing heading; got {:?}",
            heading_vec,
        );
    }

    #[test]
    fn slice_heading_midpoint_right_elbow_data() {
        // Regression: real right-elbow ATDT (kno_atk_comb_rht_p_elbowLH).
        // File values slicestart=1.26 / sliceend=1.58 are in the
        // principal range, so `oni2_to_bevy_yaw_rads` just negates
        // them → -1.26 / -1.58; sort → -1.58 / -1.26; midpoint ≈ -1.42.
        // Heading direction should be roughly right (POS_X) — that's
        // what a right-side wedge looks like in Bevy after the negate.
        let strike = crate::oni2_loader::parsers::atdt::AtdtStrike {
            framenum: 0.0,
            frameduration: 1.0,
            slicestartradians: -1.58,
            sliceendradians: -1.26,
            sliceheadingradiansb: 0.0,
            sweepheading: 0,
            ..Default::default()
        };
        let h = current_slice_heading(&strike, 0.5);
        let heading_vec = Quat::from_rotation_y(h) * Vec3::NEG_Z;
        assert!(
            heading_vec.dot(Vec3::X) > 0.9,
            "right-elbow slice should produce a right-facing heading; got {:?}",
            heading_vec,
        );
    }

    #[test]
    fn degenerate_dir_returns_none() {
        // If attacker and target are at the same XZ position, dir is
        // zero — `strike_facing_for_target` should return None so the
        // caller skips the facing update (avoiding NaN propagation).
        assert!(strike_facing_for_target(Vec3::ZERO, Vec3::new(0.0, 5.0, 0.0), 0.0).is_none());
        assert!(strike_facing_for_target(Vec3::ZERO, Vec3::ZERO, 0.0).is_none());
    }

    #[test]
    fn sweep_heading_lerps_across_attack() {
        // SweepHeading on means slice_heading lerps from slice_a (the
        // midpoint at attack start) toward sliceheadingradiansb across
        // the frameduration.  At t=framenum the lerp should be 0
        // (slice_a), at t=framenum+frameduration it should be 1
        // (sliceheadingradiansb).
        let strike = crate::oni2_loader::parsers::atdt::AtdtStrike {
            framenum: 10.0,
            frameduration: 4.0,
            slicestartradians: -0.1,
            sliceendradians: 0.1, // midpoint 0
            sliceheadingradiansb: PI, // sweep to behind
            sweepheading: 1,
            ..Default::default()
        };
        let h_start = current_slice_heading(&strike, 10.0); // sweep_t = 0
        assert!((h_start - 0.0).abs() < 1e-4, "start of sweep is slice_a");
        let h_end = current_slice_heading(&strike, 14.0); // sweep_t = 1
        assert!((h_end - PI).abs() < 1e-4, "end of sweep is sliceheadingradiansb");
        let h_mid = current_slice_heading(&strike, 12.0); // sweep_t = 0.5
        assert!(
            (h_mid - PI * 0.5).abs() < 1e-4,
            "midpoint of sweep is halfway, got {}",
            h_mid
        );
    }
}
