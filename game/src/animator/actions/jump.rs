/*
 * animator/actions/jump.rs — JumpAction: the full jump lifecycle.
 *
 * Owns the COMPRESS → MAIN (with_hold looping float) → LAND progression
 * as an internal phase machine.  Mirrors the legacy C++ where LAND was
 * the last entry of Jump's animlist and `MainAction::Land` didn't exist
 * as a separate action.
 *
 * Internal phases:
 *   • Compress  — wind-up anim (the first N entries of the schedule).
 *                 Auto-advances via the anim schedule tick system.
 *   • Main      — airborne with_hold loop.  On `ground_just_regained`
 *                 we advance the schedule into LAND and switch phase
 *                 to Landing.  On anim-cycle-once-still-airborne we
 *                 hand off to FallAction so the pose can move to a
 *                 dedicated looping fall anim.
 *   • Landing   — the LAND anim plays once.  Finished on anim end.
 *
 * The jump impulse itself is fired by `jump_impulse_emit_system` on the
 * fresh `sub_state_1::JUMP_MAIN` / `JUMP_SOMERSAULT` transition — that
 * system still owns the physics impulse; this action just owns the
 * schedule build + phase transitions.
 */
use super::{Action, ActionCtx, ActionUpdate};
use crate::animator::components::{MainAction, action_flags, sub_state_0, sub_state_1};
use crate::animator::systems::{build_jump_schedule, jump_index_for_substate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JumpPhase {
    Compress,
    Main,
    Landing,
}

pub struct JumpAction {
    substate: i32,
    phase: JumpPhase,
}

impl Default for JumpAction {
    fn default() -> Self {
        Self {
            substate: sub_state_0::JUMP_UP,
            phase: JumpPhase::Compress,
        }
    }
}

impl Action for JumpAction {
    fn main_action(&self) -> MainAction {
        MainAction::Jump
    }

    fn can_enter(&self, ctx: &ActionCtx<'_>, _subaction: i32) -> bool {
        if ctx.ap.find_at_least_one_flag(action_flags::REJECTLIST_JUMP) {
            return false;
        }
        if ctx.ap.is_ending_crouch() {
            return false;
        }
        true
    }

    fn on_enter(&mut self, ctx: &mut ActionCtx<'_>, subaction: i32) {
        self.substate = subaction;
        self.phase = JumpPhase::Compress;

        // Clear the override list and set JUMPING + variant-specific
        // sub-flag.  Mirrors the legacy try_start_action branch exactly.
        ctx.ap.flags &= !action_flags::OVERRIDELIST_JUMP;
        ctx.ap.flags |= action_flags::JUMPING;
        match subaction {
            sub_state_0::JUMP_UP => ctx.ap.flags |= action_flags::STANDING_JUMP,
            sub_state_0::JUMP_SOMERSAULT => ctx.ap.flags |= action_flags::DOUBLE_JUMPING,
            sub_state_0::JUMP_FORWARD
            | sub_state_0::JUMP_LEFT
            | sub_state_0::JUMP_RIGHT
            | sub_state_0::JUMP_BACK
            | sub_state_0::JUMP_LEFT_EVADE
            | sub_state_0::JUMP_RIGHT_EVADE
            | sub_state_0::JUMP_BACK_EVADE => ctx.ap.flags |= action_flags::RUNNING_JUMP,
            _ => {}
        }

        // Build the multi-stage schedule (COMPRESS → MAIN(with_hold) →
        // LAND) and play its first entry immediately.
        let fight_stance = ctx.ap.check_flags(action_flags::FIGHTSTANCE);
        let state_fps = ctx.anim_state.fps.max(1.0);
        let Some(mut schedule) = build_jump_schedule(
            subaction,
            ctx.lib,
            ctx.jump_controller,
            fight_stance,
            state_fps,
        ) else {
            bevy::log::warn!(
                "JumpAction: build_jump_schedule returned None for substate {}",
                subaction,
            );
            return;
        };

        // Try to play the first entry.  If the library is missing it
        // (e.g. the placeholder player has no ONI2 anims), we still want
        // the flag / jump_index bookkeeping to run — but we must NOT
        // install a dead schedule, or `creature_movement_anim_system`
        // will freeze locomotion on the entity forever.
        let first_played = if let Some(first) = schedule.entries.first().cloned() {
            if ctx.lib.play(&first.alias, ctx.anim_state) {
                ctx.anim_state.speed_multiplier = first.rate;
                ctx.anim_state.looping = first.hold_at_end;
                ctx.ap.record_new_substate_1(first.substate_1);
                schedule.mark_first_played();
                true
            } else {
                bevy::log::warn!(
                    "JumpAction: first alias '{}' not in library — \
                     running without anim (locomotion unfrozen, physics \
                     impulse still fires via the placeholder path)",
                    first.alias
                );
                false
            }
        } else {
            false
        };
        ctx.ap.jump_index = jump_index_for_substate(subaction);

        // Push the jump's gravity factor onto the modifier stack.  Remove
        // happens in on_exit (whether we Finish naturally via Landing or
        // get overridden by another action via HandoffTo).  Using a stack
        // instead of a mutable field means a cutscene freeze, stun, or
        // other sibling override can coexist without either side fighting
        // the other's "reset."
        let jump_idx = jump_index_for_substate(subaction);
        let factor = ctx
            .jump_controller
            .and_then(|jc| jc.jumps.get(jump_idx as usize))
            .map(|j| j.gravity_factor.max(0.01))
            .unwrap_or(1.0);
        if let Some(g) = ctx.gravity.as_deref_mut() {
            bevy::log::info!(
                "JumpAction on_enter: about to push 'jump'={:.2} on entity {:?} (prior layer_count={})",
                factor,
                ctx.entity,
                g.0.layer_count(),
            );
            g.push("jump", factor);
        } else {
            bevy::log::warn!(
                "JumpAction on_enter: ctx.gravity is None for entity {:?} — jump physics \
                 won't apply a gravity factor (entity missing GravityModifiers component?)",
                ctx.entity,
            );
        }

        if first_played {
            *ctx.schedule = schedule;
        } else {
            // Anim-less jump: clear the schedule so locomotion keeps
            // picking idle/run/walk each tick.
            ctx.schedule.clear();
        }
    }

    fn update(&mut self, ctx: &mut ActionCtx<'_>, _dt: f32) -> ActionUpdate {
        // Phase tracking — derive from sub_state_1 the schedule has
        // stamped onto the ActionPlayer.  We watch for transitions into
        // MAIN/SOMERSAULT (to promote Compress → Main) and into LAND
        // (to promote Main → Landing).
        let s1 = ctx.ap.sub_state_1;
        if self.phase == JumpPhase::Compress
            && (s1 == sub_state_1::JUMP_MAIN || s1 == sub_state_1::JUMP_SOMERSAULT)
        {
            self.phase = JumpPhase::Main;
        }
        if self.phase == JumpPhase::Main && s1 == sub_state_1::JUMP_LAND {
            // Either ground_just_regained or a with_hold override caused
            // the schedule to advance to LAND — enter Landing phase.
            self.phase = JumpPhase::Landing;
        }

        match self.phase {
            JumpPhase::Compress => {
                // Don't handoff yet — wait for schedule to reach MAIN.
                // Ground-contact in COMPRESS is pathological (we haven't
                // impulsed yet); if it happens, let COMPRESS finish so the
                // schedule moves through MAIN → LAND naturally.
                ActionUpdate::Continue
            }
            JumpPhase::Main => {
                // Two possible MAIN sub-entries: VERTICAL_3 (rate-stretched
                // primary arc) and VERTICAL_3_EXT (looping airborne hold).
                // Either can be current when ground contact hits.  Keep
                // advancing the cursor until we land on a JUMP_LAND entry
                // so both paths collapse cleanly into LAND regardless of
                // which one was playing.
                let hit_ground = ctx.ground_just_regained || ctx.high_velocity_landing;
                if hit_ground {
                    if ctx.high_velocity_landing {
                        ctx.broadcasts.push("StartHardLand".into());
                    } else {
                        ctx.broadcasts.push("StartLand".into());
                    }
                    // Advance until the current entry is a LAND substate
                    // (or the schedule finishes).  Max 4 hops is plenty
                    // for any jump variant's entry count.
                    for _ in 0..4 {
                        let is_land = ctx
                            .schedule
                            .current()
                            .map(|e| e.substate_1 == sub_state_1::JUMP_LAND)
                            .unwrap_or(false);
                        if is_land {
                            break;
                        }
                        ctx.schedule.advance();
                        if ctx.schedule.is_empty() {
                            break;
                        }
                    }
                    self.phase = JumpPhase::Landing;
                }
                ActionUpdate::Continue
            }
            JumpPhase::Landing => {
                // LAND plays once (not with_hold).  Finish when anim ends.
                if ctx.anim_state.anim.num_frames > 1 {
                    let last = (ctx.anim_state.anim.num_frames as f32 - 1.0).max(0.0);
                    if !ctx.anim_state.looping && ctx.anim_state.current_time >= last {
                        return ActionUpdate::Finished;
                    }
                }
                ActionUpdate::Continue
            }
        }
    }

    fn on_exit(&mut self, ctx: &mut ActionCtx<'_>) {
        // Clear the jump flag family.  Mirrors do_end_action's Jump branch:
        // CLEARLIST_JUMP wipes JUMPING | DOUBLE_JUMPING | RUNNING_JUMP |
        // STANDING_JUMP all at once.
        ctx.ap.flags &= !action_flags::CLEARLIST_JUMP;
        ctx.ap.jump_index = -1;
        // Remove our gravity layer.  `gravity_sync_system` will reflect
        // the resolved stack into Avian's `GravityScale` next tick.  If
        // another action pushed its own layer while this jump was
        // running (slow-time fx, cutscene), that layer stays active —
        // no fight over "who resets last."
        if let Some(g) = ctx.gravity.as_deref_mut() {
            bevy::log::info!(
                "JumpAction on_exit: about to remove 'jump' from entity {:?} (prior layer_count={})",
                ctx.entity,
                g.0.layer_count(),
            );
            g.remove("jump");
        } else {
            bevy::log::warn!(
                "JumpAction on_exit: ctx.gravity is None for entity {:?} — can't remove 'jump' layer",
                ctx.entity,
            );
        }
    }
}
