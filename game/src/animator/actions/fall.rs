/*
 * animator/actions/fall.rs — FallAction: looping airborne pose + landing.
 *
 * No kickoff impulse (gravity is already acting — fall is just what
 * happens after a jump's MAIN runs its course, or the character walks
 * off an edge).  Internal phases:
 *
 *   • FallLoop — ANIMJUMP_VERTICAL_3_EXT plays on a with_hold()'d single
 *                entry schedule, looping until ground contact.  Matches
 *                the legacy `action.cpp` call:
 *                  Parent->GetAnimation(ANIMCATEGORY_JUMP,
 *                                       ANIMJUMP_VERTICAL_3_EXT)
 *   • Landing  — one LAND anim (vertical fallback), ends with Finished.
 *
 * We handle Landing internally rather than handing off to a separate
 * action because there's no `MainAction::Land` — LAND was just the last
 * entry of Jump's animlist in the legacy engine.  Keeping it inside Fall
 * mirrors that structurally.
 */
use super::{Action, ActionCtx, ActionUpdate};
use crate::animator::components::{MainAction, action_flags, sub_state_1};
use crate::animator::schedule::{AnimSchedule, AnimScheduleEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FallPhase {
    FallLoop,
    Landing,
}

pub struct FallAction {
    phase: FallPhase,
}

impl Default for FallAction {
    fn default() -> Self {
        Self {
            phase: FallPhase::FallLoop,
        }
    }
}

impl Action for FallAction {
    fn main_action(&self) -> MainAction {
        MainAction::Fall
    }

    fn can_enter(&self, ctx: &ActionCtx<'_>, _subaction: i32) -> bool {
        !ctx.ap.find_at_least_one_flag(action_flags::REJECTLIST_FALL)
    }

    fn on_enter(&mut self, ctx: &mut ActionCtx<'_>, _subaction: i32) {
        self.phase = FallPhase::FallLoop;

        // Legacy quirk: Fall uses the JUMPING flag (not a dedicated
        // FALLING flag).  Clear the override list first, then set
        // JUMPING.  Mirrors the legacy MainAction::Fall arm.
        ctx.ap.flags &= !action_flags::OVERRIDELIST_FALL;
        ctx.ap.flags |= action_flags::JUMPING;

        // Legacy: `action.cpp` pulls this via
        //   Parent->GetAnimation(ANIMCATEGORY_JUMP, ANIMJUMP_VERTICAL_3_EXT)
        // — the "extended vertical-3" airborne hold variant.  That's the
        // real fall-loop alias, not the forward-float we were using.
        let entry =
            AnimScheduleEntry::new("ANIMJUMP_VERTICAL_3_EXT", sub_state_1::IDLE).with_hold();
        if ctx.lib.play(&entry.alias, ctx.anim_state) {
            ctx.anim_state.speed_multiplier = entry.rate;
            ctx.anim_state.looping = entry.hold_at_end;
            ctx.ap.record_new_substate_1(entry.substate_1);
            let mut schedule = AnimSchedule::single(entry);
            schedule.mark_first_played();
            *ctx.schedule = schedule;
        } else {
            // Missing alias — don't install a dead schedule.  The action
            // still runs (ground-contact detection handoff still works)
            // but locomotion stays free and the entity doesn't freeze.
            // Warn only once per character type would be nicer, but a
            // per-tick warning here would be spammy so we only emit it
            // on entry (which is a one-shot edge).
            bevy::log::warn!(
                "FallAction: fall alias '{}' not in library — running \
                 without anim (locomotion unfrozen)",
                entry.alias
            );
            ctx.schedule.clear();
        }
    }

    fn update(&mut self, ctx: &mut ActionCtx<'_>, _dt: f32) -> ActionUpdate {
        match self.phase {
            FallPhase::FallLoop => {
                let hit_ground = ctx.ground_just_regained || ctx.high_velocity_landing;
                if hit_ground {
                    if ctx.high_velocity_landing {
                        ctx.broadcasts.push("StartHardLand".into());
                    } else {
                        ctx.broadcasts.push("StartLand".into());
                    }
                    // Try to swap to a LAND entry.  If the alias is
                    // missing, skip straight to Finished so the action
                    // tears down and locomotion unfreezes.
                    let land_entry =
                        AnimScheduleEntry::new("ANIMJUMP_VERTICAL_4", sub_state_1::JUMP_LAND);
                    if ctx.lib.play(&land_entry.alias, ctx.anim_state) {
                        ctx.anim_state.speed_multiplier = land_entry.rate;
                        ctx.anim_state.looping = land_entry.hold_at_end;
                        ctx.ap.record_new_substate_1(land_entry.substate_1);
                        let mut schedule = AnimSchedule::single(land_entry);
                        schedule.mark_first_played();
                        *ctx.schedule = schedule;
                        self.phase = FallPhase::Landing;
                        return ActionUpdate::Continue;
                    } else {
                        bevy::log::warn!(
                            "FallAction: land alias '{}' not in library — \
                             finishing without land anim",
                            land_entry.alias
                        );
                        ctx.schedule.clear();
                        return ActionUpdate::Finished;
                    }
                }
                // Ledge / zipline handoffs will go here once those Actions
                // are ported.  For now, keep looping.
                ActionUpdate::Continue
            }
            FallPhase::Landing => {
                // Wait for the LAND anim to finish, then Finished.
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
        // Wipe the jump-family flags on clean exit (natural Finished after
        // Landing, or override).  Mirrors do_end_action's Jump branch
        // since Fall uses the JUMPING flag under the hood.
        ctx.ap.flags &= !action_flags::CLEARLIST_JUMP;
    }
}
