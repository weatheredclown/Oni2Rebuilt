/*
 * animator/systems.rs — FixedUpdate systems for the animator subsystem.
 *
 * action_start_system       — dispatch StartActionMessage via reject/override lists
 * action_end_system         — dispatch EndActionMessage, clear flags, end animations
 * action_player_tick_system — per-frame: refresh sub_state_1_last_frame, consume
 *                             pending events when the matching substate fires, re-
 *                             enable collision after react, evaluate pending jumps
 * control_anim_system       — process ControlAnimMessage (stop / pause / restart
 *                             / loop / set rate / hold)
 * head_ik_mode_system       — apply HeadIkModeMessage → ActionPlayer.head_ik_mode
 * play_react_system         — short-cut: StartAction(React, react_enum) with the
 *                             alias resolved from ANIMREACT_NAMES
 * play_die_system           — short-cut: StartAction(Die, die_enum)
 */
use bevy::prelude::*;

use crate::oni2_loader::animation::{AnimId, Oni2AnimLibrary, Oni2AnimState};
use crate::oni2_loader::parsers::jump::JumpController;
use crate::oni2_loader::parsers::rct::ANIMREACT_NAMES;

use super::components::{
    ActionPlayer, ActionResult, HeadIkMode, MainAction, action_flags, pending_flags, sub_state_0,
    sub_state_1,
};
use super::events::{
    ActionEndedMessage, ActionStartedMessage, ControlAnimMessage, EndActionMessage,
    HeadIkModeMessage, JumpImpulseMessage, PlayDieMessage, PlayReactMessage, StartActionMessage,
    control_anim_bits,
};
use super::schedule::{AnimSchedule, AnimScheduleEntry};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Attempt to play `alias` and, on success, update the action player's
/// sub_state_1 tracking.  Returns `Succeeded` if the library had the alias,
/// `Failed` otherwise (mirrors ACT_FAILED in original Oni2).
fn play_and_record(
    alias: &str,
    new_substate_1: i32,
    ap: &mut ActionPlayer,
    lib: &Oni2AnimLibrary,
    state: &mut Oni2AnimState,
) -> ActionResult {
    if lib.play(alias, state) {
        ap.record_new_substate_1(new_substate_1);
        ActionResult::Succeeded
    } else {
        ActionResult::Failed
    }
}

// ---------------------------------------------------------------------------
// Jump helpers — mirror crJump physics math + crAnimList schedule construction
// ---------------------------------------------------------------------------

/// Compute the jump flight time from loaded `JumpState` data.  Mirrors
/// `crJump::Set`:  T = 2·√(2h / (gf·g))   (g = 9.81).
fn compute_jump_time(jump: &crate::oni2_loader::parsers::jump::JumpState) -> f32 {
    const G: f32 = 9.81;
    let h = jump.height.max(0.0001);
    let gf = jump.gravity_factor.max(0.0001);
    2.0 * ((2.0 * h) / (gf * G)).sqrt()
}

/// Which .jump index (matches the ordering encoded in kno.jump / jae.jump /
/// joe.jump) corresponds to a given ACT_S0_JUMP_* substate.
pub(crate) fn jump_index_for_substate(substate: i32) -> i32 {
    match substate {
        sub_state_0::JUMP_UP
        | sub_state_0::JUMP_FORWARD
        | sub_state_0::JUMP_LEFT
        | sub_state_0::JUMP_RIGHT
        | sub_state_0::JUMP_BACK => 0,
        sub_state_0::JUMP_SOMERSAULT => 1,
        sub_state_0::JUMP_WALL_SPRING => 2,
        sub_state_0::JUMP_LEFT_EVADE => 5,
        sub_state_0::JUMP_RIGHT_EVADE => 6,
        _ => 0,
    }
}

/// Build the ONI2-style multi-stage animation schedule for a given jump
/// substate. Enqueues compress → spring → main (rate-matched to
/// the jump trajectory) → land.
///
/// The rate-match on the main (airborne) anim comes from the legacy formula:
///   at = num_frames * (1/fps_native)      // native anim duration
///   rate = at / jump.Time                  // scale to match physics arc
/// Here we use the playback `state_fps` so the anim finishes in `jump.Time`
/// seconds regardless of our fps baseline.
pub(crate) fn build_jump_schedule(
    substate: i32,
    lib: &Oni2AnimLibrary,
    jump_controller: Option<&JumpController>,
    fight_stance: bool,
    state_fps: f32,
) -> Option<AnimSchedule> {
    let jump_time = jump_controller
        .and_then(|jc| jc.jumps.get(jump_index_for_substate(substate) as usize))
        .map(compute_jump_time)
        .unwrap_or(0.6);

    // Compute a rate multiplier that makes `alias` finish in `jump_time`.
    let main_rate = |alias: &str| -> f32 {
        let frames = lib
            .anims
            .get(&AnimId::new(alias))
            .map(|a| a.num_frames as f32)
            .unwrap_or(0.0);
        if frames <= 0.0 || state_fps <= 0.0 || jump_time <= 0.0 {
            1.0
        } else {
            (frames / (state_fps * jump_time)).max(0.1)
        }
    };

    // Every JUMP_MAIN / JUMP_SOMERSAULT entry is marked `with_hold()` so
    // that when the airborne anim reaches its natural end we LOOP it rather
    // than immediately advancing to LAND.  The schedule stays parked on
    // MAIN until `jump_land_on_ground_system` detects ground contact and
    // explicitly advances the cursor to LAND.  This mirrors the C++ behavior
    // of holding a looping FALL anim while airborne.
    let entries = match substate {
        sub_state_0::JUMP_UP => {
            let compress_alias = if fight_stance {
                "ANIMJUMP_VERTICAL_1_FIGHT"
            } else {
                "ANIMJUMP_VERTICAL_1_STAND"
            };
            // C++ layout from action.cpp (Parent->GetAnimation calls):
            //   VERTICAL_1_STAND → VERTICAL_2 → VERTICAL_3 (stretched to
            //   jump_time) → VERTICAL_3_EXT (looping airborne extension,
            //   plays if still airborne when VERTICAL_3 ends) → VERTICAL_4.
            // VERTICAL_3 plays ONCE at its rate-matched speed; if the
            // actual airtime is shorter, ground_regained advances past
            // EXT to VERTICAL_4.  If longer, VERTICAL_3 auto-advances to
            // EXT which loops until ground contact.
            vec![
                AnimScheduleEntry::new(compress_alias, sub_state_1::JUMP_COMPRESS),
                AnimScheduleEntry::new("ANIMJUMP_VERTICAL_2", sub_state_1::JUMP_COMPRESS),
                AnimScheduleEntry::new("ANIMJUMP_VERTICAL_3", sub_state_1::JUMP_MAIN)
                    .with_rate(main_rate("ANIMJUMP_VERTICAL_3")),
                AnimScheduleEntry::new("ANIMJUMP_VERTICAL_3_EXT", sub_state_1::JUMP_MAIN)
                    .with_hold(),
                AnimScheduleEntry::new("ANIMJUMP_VERTICAL_4", sub_state_1::JUMP_LAND),
            ]
        }
        // Directional jump variants all follow the same shape:
        //   SPRING (compress) → FLOAT (main arc, rate-stretched, NO hold)
        //     → VERTICAL_3_EXT (generic airborne fall-hold; loops via
        //       with_hold until ground) → LAND.
        // Matches the C++ pattern where VERTICAL_3_EXT is the shared
        // "extended airborne" hold (Parent->GetAnimation(ANIMCATEGORY_JUMP,
        // ANIMJUMP_VERTICAL_3_EXT)).  Without the EXT entry, MAIN with
        // with_hold() would loop visibly when the rate-stretched anim
        // ended before the actual airtime.
        sub_state_0::JUMP_FORWARD => vec![
            AnimScheduleEntry::new("ANIMJUMP_RUN_1", sub_state_1::JUMP_COMPRESS),
            AnimScheduleEntry::new("ANIMJUMP_RUN_2", sub_state_1::JUMP_MAIN)
                .with_rate(main_rate("ANIMJUMP_RUN_2")),
            AnimScheduleEntry::new("ANIMJUMP_VERTICAL_3_EXT", sub_state_1::JUMP_MAIN)
                .with_hold(),
            AnimScheduleEntry::new("ANIMJUMP_RUN_3", sub_state_1::JUMP_LAND),
        ],
        sub_state_0::JUMP_LEFT => vec![
            AnimScheduleEntry::new("ANIMJUMP_RUN_LEFT_SPRING", sub_state_1::JUMP_COMPRESS),
            AnimScheduleEntry::new("ANIMJUMP_RUN_LEFT_FLOAT", sub_state_1::JUMP_MAIN)
                .with_rate(main_rate("ANIMJUMP_RUN_LEFT_FLOAT")),
            AnimScheduleEntry::new("ANIMJUMP_VERTICAL_3_EXT", sub_state_1::JUMP_MAIN)
                .with_hold(),
            AnimScheduleEntry::new("ANIMJUMP_RUN_LEFT_LAND", sub_state_1::JUMP_LAND),
        ],
        sub_state_0::JUMP_LEFT_EVADE => vec![
            AnimScheduleEntry::new("ANIMJUMP_JOG_LEFT_SPRING", sub_state_1::JUMP_COMPRESS),
            AnimScheduleEntry::new("ANIMJUMP_JOG_LEFT_FLOAT", sub_state_1::JUMP_MAIN)
                .with_rate(main_rate("ANIMJUMP_JOG_LEFT_FLOAT")),
            AnimScheduleEntry::new("ANIMJUMP_VERTICAL_3_EXT", sub_state_1::JUMP_MAIN)
                .with_hold(),
            AnimScheduleEntry::new("ANIMJUMP_JOG_LEFT_LAND", sub_state_1::JUMP_LAND),
        ],
        sub_state_0::JUMP_RIGHT => vec![
            AnimScheduleEntry::new("ANIMJUMP_RUN_RIGHT_SPRING", sub_state_1::JUMP_COMPRESS),
            AnimScheduleEntry::new("ANIMJUMP_RUN_RIGHT_FLOAT", sub_state_1::JUMP_MAIN)
                .with_rate(main_rate("ANIMJUMP_RUN_RIGHT_FLOAT")),
            AnimScheduleEntry::new("ANIMJUMP_VERTICAL_3_EXT", sub_state_1::JUMP_MAIN)
                .with_hold(),
            AnimScheduleEntry::new("ANIMJUMP_RUN_RIGHT_LAND", sub_state_1::JUMP_LAND),
        ],
        sub_state_0::JUMP_RIGHT_EVADE => vec![
            AnimScheduleEntry::new("ANIMJUMP_JOG_RIGHT_SPRING", sub_state_1::JUMP_COMPRESS),
            AnimScheduleEntry::new("ANIMJUMP_JOG_RIGHT_FLOAT", sub_state_1::JUMP_MAIN)
                .with_rate(main_rate("ANIMJUMP_JOG_RIGHT_FLOAT")),
            AnimScheduleEntry::new("ANIMJUMP_VERTICAL_3_EXT", sub_state_1::JUMP_MAIN)
                .with_hold(),
            AnimScheduleEntry::new("ANIMJUMP_JOG_RIGHT_LAND", sub_state_1::JUMP_LAND),
        ],
        sub_state_0::JUMP_BACK => vec![
            AnimScheduleEntry::new("ANIMJUMP_RUN_BACK_SPRING", sub_state_1::JUMP_COMPRESS),
            AnimScheduleEntry::new("ANIMJUMP_RUN_BACK_FLOAT", sub_state_1::JUMP_MAIN)
                .with_rate(main_rate("ANIMJUMP_RUN_BACK_FLOAT")),
            AnimScheduleEntry::new("ANIMJUMP_VERTICAL_3_EXT", sub_state_1::JUMP_MAIN)
                .with_hold(),
            AnimScheduleEntry::new("ANIMJUMP_RUN_BACK_LAND", sub_state_1::JUMP_LAND),
        ],
        sub_state_0::JUMP_BACK_EVADE => vec![
            AnimScheduleEntry::new("ANIMJUMP_JOG_BACK_SPRING", sub_state_1::JUMP_COMPRESS),
            AnimScheduleEntry::new("ANIMJUMP_JOG_BACK_FLOAT", sub_state_1::JUMP_MAIN)
                .with_rate(main_rate("ANIMJUMP_JOG_BACK_FLOAT")),
            AnimScheduleEntry::new("ANIMJUMP_VERTICAL_3_EXT", sub_state_1::JUMP_MAIN)
                .with_hold(),
            AnimScheduleEntry::new("ANIMJUMP_JOG_BACK_LAND", sub_state_1::JUMP_LAND),
        ],
        sub_state_0::JUMP_SOMERSAULT => vec![
            // Mid-air flip — no compress phase.  The impulse fires as soon as
            // SubState1 becomes JUMP_SOMERSAULT (which is "fresh" on start).
            // `with_hold()` keeps the anim looping until ground contact.
            AnimScheduleEntry::new("ANIMJUMP_VERTICAL_2", sub_state_1::JUMP_SOMERSAULT)
                .with_rate(main_rate("ANIMJUMP_VERTICAL_2"))
                .with_hold(),
        ],
        sub_state_0::JUMP_WALL_COMPRESS => vec![AnimScheduleEntry::new(
            "ANIMJUMP_RUN_WALL_COMPRESS",
            sub_state_1::JUMP_WALL_COMPRESS,
        )],
        sub_state_0::JUMP_WALL_SPRING => vec![AnimScheduleEntry::new(
            "ANIMJUMP_RUN_WALL_JUMP",
            sub_state_1::JUMP_WALL_SPRING,
        )],
        _ => return None,
    };

    Some(AnimSchedule::new(entries))
}

// ---------------------------------------------------------------------------
// Per-action start handlers
// ---------------------------------------------------------------------------

/// Reject-list + override-list + anim dispatch for a StartAction.  Each arm
/// mirrors the legacy per-action jump handler:
///   ActionStartSlide, ActionStartJump, ActionStartCrouch, ActionStartLedgeHang,
///   ActionStartEvade, ActionStartDrawWeapon, ActionStartFightStance,
///   ActionStartFall, ActionStartReact, ActionStartDie, ActionStartCustomAnim,
///   ActionStartPickup, ActionStartDropItem, ActionStartZipLine,
///   ActionStartTransition, ActionStartStruggle, ActionStartLie
///
/// `schedule_out` is an out-parameter: if the action produces a multi-stage
/// animation queue (Jump does), the built schedule is returned through it so
/// the caller can attach it via Commands.
fn try_start_action(
    action: MainAction,
    substate: i32,
    ap: &mut ActionPlayer,
    lib: &Oni2AnimLibrary,
    state: &mut Oni2AnimState,
    jump_controller: Option<&JumpController>,
    schedule_out: &mut Option<AnimSchedule>,
) -> ActionResult {
    match action {
        MainAction::None => ActionResult::Succeeded,

        MainAction::Slide => {
            if ap.check_flags(action_flags::SLIDING) {
                return ActionResult::Denied;
            }
            ap.flags &= !action_flags::OVERRIDELIST_SLIDE;
            ap.flags |= action_flags::SLIDING;
            play_and_record("ANIMACTION_SLIDE", sub_state_1::IDLE, ap, lib, state)
        }

        MainAction::Jump => {
            if ap.find_at_least_one_flag(action_flags::REJECTLIST_JUMP) {
                return ActionResult::Denied;
            }
            if ap.is_ending_crouch() {
                return ActionResult::Denied;
            }
            ap.flags &= !action_flags::OVERRIDELIST_JUMP;
            ap.flags |= action_flags::JUMPING;

            // Set the JUMPING sub-flag first.
            match substate {
                sub_state_0::JUMP_UP => ap.flags |= action_flags::STANDING_JUMP,
                sub_state_0::JUMP_SOMERSAULT => ap.flags |= action_flags::DOUBLE_JUMPING,
                sub_state_0::JUMP_FORWARD
                | sub_state_0::JUMP_LEFT
                | sub_state_0::JUMP_RIGHT
                | sub_state_0::JUMP_BACK
                | sub_state_0::JUMP_LEFT_EVADE
                | sub_state_0::JUMP_RIGHT_EVADE
                | sub_state_0::JUMP_BACK_EVADE => ap.flags |= action_flags::RUNNING_JUMP,
                _ => {}
            }

            // Build the multi-stage crAnimList-style schedule.  Rate-matched
            // against the loaded JumpController so the airborne main anim
            // finishes in exactly jump.Time seconds.
            let fight_stance = ap.check_flags(action_flags::FIGHTSTANCE);
            let state_fps = state.fps.max(1.0);
            let Some(mut schedule) =
                build_jump_schedule(substate, lib, jump_controller, fight_stance, state_fps)
            else {
                return ActionResult::Failed;
            };

            // Kick off the first entry immediately so there is no one-tick gap
            // between StartAction and the compress anim.  The tick system will
            // pick it up from here (needs_play cleared) and auto-advance on
            // anim end.
            if let Some(first) = schedule.entries.first().cloned() {
                if !lib.play(&first.alias, state) {
                    warn!(
                        "Jump: first alias '{}' not in library — schedule will be no-op",
                        first.alias
                    );
                    return ActionResult::Failed;
                }
                state.speed_multiplier = first.rate;
                state.looping = first.hold_at_end;
                ap.record_new_substate_1(first.substate_1);
                schedule.mark_first_played();
            }

            // Record the selected jump index so downstream systems (impulse
            // emit) can look up the same JumpState.
            ap.jump_index = jump_index_for_substate(substate);

            *schedule_out = Some(schedule);
            ActionResult::Succeeded
        }

        MainAction::Crouch => {
            if ap.find_at_least_one_flag(action_flags::REJECTLIST_CROUCH) {
                return ActionResult::Denied;
            }
            ap.flags |= action_flags::CROUCHING;
            play_and_record(
                "ANIMNAV_START_CROUCH",
                sub_state_1::TRANSITION,
                ap,
                lib,
                state,
            )
        }

        MainAction::FightStance => {
            // Subaction picks enter vs. leave.  Defaults to enter when the
            // caller doesn't specify — matches the legacy default-START
            // behavior and keeps old call sites working.
            let leaving = substate == sub_state_0::TRANSITION_FIGHTSTANCE_END;

            if leaving {
                // Leaving fight stance: only valid if we're currently in it.
                // No reject list applies — a fight-stance exit should always
                // succeed when requested.
                ap.flags &= !action_flags::FIGHTSTANCE;
                play_and_record(
                    "ANIMFIGHTSTANCE_FIGHT_TO_STAND",
                    sub_state_1::TRANSITION,
                    ap,
                    lib,
                    state,
                )
            } else {
                // Entering fight stance: reject list + override list apply.
                if ap.find_at_least_one_flag(action_flags::REJECTLIST_FIGHTSTANCE) {
                    return ActionResult::Denied;
                }
                ap.flags &= !action_flags::OVERRIDELIST_FIGHTSTANCE;
                ap.flags |= action_flags::FIGHTSTANCE;
                play_and_record(
                    "ANIMFIGHTSTANCE_STAND_TO_FIGHT",
                    sub_state_1::TRANSITION,
                    ap,
                    lib,
                    state,
                )
            }
        }

        MainAction::DrawWeapon => {
            if ap.find_at_least_one_flag(action_flags::REJECTLIST_DRAWWEAPON) {
                return ActionResult::Denied;
            }
            ap.flags |= action_flags::WEAPON;
            ap.set_pending(pending_flags::DRAW | pending_flags::ARM_IK);
            play_and_record(
                "ANIMWEAPON_GET_FROM_HOLSTER",
                sub_state_1::WEAPON_GET_FROM_HOLSTER,
                ap,
                lib,
                state,
            )
        }

        MainAction::Fall => {
            // NOTE: this legacy arm is bypassed when the action registry
            // contains a FallAction — `action_start_system` skips
            // registered actions.  Kept here for entities that haven't
            // been migrated yet.
            if ap.find_at_least_one_flag(action_flags::REJECTLIST_FALL) {
                return ActionResult::Denied;
            }
            ap.flags &= !action_flags::OVERRIDELIST_FALL;
            // Fall uses the legacy "vertical-3 extended" airborne hold —
            // matches `action.cpp`'s `GetAnimation(ANIMCATEGORY_JUMP,
            // ANIMJUMP_VERTICAL_3_EXT)`.  Sub_state_1 stays at IDLE since
            // the airborne fall has no bespoke substate.
            play_and_record("ANIMJUMP_VERTICAL_3_EXT", sub_state_1::IDLE, ap, lib, state)
        }

        MainAction::LieDown => {
            if ap.find_at_least_one_flag(action_flags::REJECTLIST_LIE) {
                return ActionResult::Denied;
            }
            ap.flags |= action_flags::LYINGDOWN;
            play_and_record("ANIMNAV_START_LIE", sub_state_1::TRANSITION, ap, lib, state)
        }

        MainAction::React => {
            if ap.find_at_least_one_flag(action_flags::REJECTLIST_REACT) {
                return ActionResult::Denied;
            }
            // The concrete react animation is selected by ANIMREACT_NAMES[substate];
            // play_react_system resolves the alias before calling us.  If the caller
            // passed the substate directly (e.g. via StartActionMessage), fall back
            // to ANIMREACT_REGULAR.
            let idx = if substate >= 0 && (substate as usize) < ANIMREACT_NAMES.len() {
                substate as usize
            } else {
                0 // ANIMREACT_REGULAR
            };
            let alias = ANIMREACT_NAMES[idx];
            play_and_record(alias, sub_state_1::REACT, ap, lib, state)
        }

        MainAction::Die => {
            ap.flags |= action_flags::DEAD;
            let idx = if substate >= 0 && (substate as usize) < ANIMREACT_NAMES.len() {
                substate as usize
            } else {
                0
            };
            let alias = ANIMREACT_NAMES[idx];
            play_and_record(alias, sub_state_1::DIE, ap, lib, state)
        }

        MainAction::Evade => {
            if ap.find_at_least_one_flag(action_flags::REJECTLIST_EVADE) {
                return ActionResult::Denied;
            }
            let alias = match substate {
                sub_state_0::JUMP_LEFT_EVADE => "ANIMJUMP_LEFT_EVADE",
                sub_state_0::JUMP_RIGHT_EVADE => "ANIMJUMP_RIGHT_EVADE",
                sub_state_0::JUMP_BACK_EVADE => "ANIMJUMP_BACK_EVADE",
                _ => return ActionResult::Failed,
            };
            play_and_record(alias, sub_state_1::EVADE, ap, lib, state)
        }

        MainAction::LedgeHang => {
            let alias = match substate {
                sub_state_0::LEDGEHANG_GRAB => "ANIMLEDGE_GRAB",
                sub_state_0::LEDGEHANG_HOLD => "ANIMLEDGE_HOLD",
                sub_state_0::LEDGEHANG_LEFT => "ANIMLEDGE_LEFT",
                sub_state_0::LEDGEHANG_RIGHT => "ANIMLEDGE_RIGHT",
                sub_state_0::LEDGEHANG_CLAMBER => "ANIMLEDGE_CLAMBER",
                sub_state_0::LEDGEHANG_DROP => "ANIMLEDGE_DROP",
                _ => return ActionResult::Failed,
            };
            let new_s1 = match substate {
                sub_state_0::LEDGEHANG_GRAB => sub_state_1::LEDGE_GRAB,
                sub_state_0::LEDGEHANG_HOLD
                | sub_state_0::LEDGEHANG_LEFT
                | sub_state_0::LEDGEHANG_RIGHT => sub_state_1::LEDGE_HOLD,
                sub_state_0::LEDGEHANG_CLAMBER => sub_state_1::LEDGE_CLAMBER,
                sub_state_0::LEDGEHANG_DROP => sub_state_1::LEDGE_DROP,
                _ => sub_state_1::IDLE,
            };
            ap.flags |= action_flags::LEDGEHANGING;
            play_and_record(alias, new_s1, ap, lib, state)
        }

        MainAction::CraneStruggle => {
            ap.flags &= !action_flags::OVERRIDELIST_STRUGGLE;
            play_and_record(
                "ANIMNORMAL_CRANESTRUGGLE",
                sub_state_1::CRANE_STRUGGLE,
                ap,
                lib,
                state,
            )
        }

        MainAction::PlayCustomAnim => {
            // The caller is expected to have already loaded the custom anim into the
            // library under a known name.  For now we just mark the state; the actual
            // alias is supplied via ControlAnimMessage.
            ap.record_new_substate_1(sub_state_1::CUSTOM_ANIM);
            ActionResult::Succeeded
        }

        MainAction::Pickup => {
            if ap.find_at_least_one_flag(action_flags::REJECTLIST_PICKUP) {
                return ActionResult::Denied;
            }
            play_and_record("ANIMACTION_PICKUP", sub_state_1::PICKUP, ap, lib, state)
        }

        MainAction::DropItem => {
            play_and_record("ANIMACTION_DROP", sub_state_1::IDLE, ap, lib, state)
        }

        MainAction::ZipLine => {
            if ap.check_flags(action_flags::ZIPLINE) && substate != sub_state_0::ZIPLINE_TURN {
                return ActionResult::Denied;
            }
            ap.flags &= !action_flags::OVERRIDELIST_ZIPLINE;
            ap.flags |= action_flags::ZIPLINE;
            let alias = match substate {
                sub_state_0::ZIPLINE_NAV => "ANIMZIPLINE_GRAB",
                sub_state_0::ZIPLINE_SLIDE => "ANIMZIPLINE_MOVE_POSE",
                sub_state_0::ZIPLINE_TURN => "ANIMZIPLINE_TURN",
                _ => return ActionResult::Failed,
            };
            play_and_record(alias, sub_state_1::TRANSITION, ap, lib, state)
        }

        MainAction::Transition => {
            // Generic transition — the substate encodes which transition.  Animation
            // name resolution varies per transition; here we pick the common ones.
            let alias = match substate {
                sub_state_0::TRANSITION_CROUCH_START => "ANIMNAV_START_CROUCH",
                sub_state_0::TRANSITION_CROUCH_END => "ANIMNAV_STOP_CROUCH",
                sub_state_0::TRANSITION_FIGHTSTANCE_START => "ANIMFIGHTSTANCE_STAND_TO_FIGHT",
                sub_state_0::TRANSITION_FIGHTSTANCE_END => "ANIMFIGHTSTANCE_FIGHT_TO_STAND",
                _ => return ActionResult::Failed,
            };
            play_and_record(alias, sub_state_1::TRANSITION, ap, lib, state)
        }
    }
}

// ---------------------------------------------------------------------------
// Per-action end handlers
// ---------------------------------------------------------------------------

fn do_end_action(
    action: MainAction,
    _subaction: i32,
    ap: &mut ActionPlayer,
    _lib: &Oni2AnimLibrary,
    _state: &mut Oni2AnimState,
) {
    match action {
        MainAction::Slide => {
            ap.flags &= !action_flags::SLIDING;
        }
        MainAction::Jump => {
            ap.flags &= !action_flags::CLEARLIST_JUMP;
            ap.jump_index = -1;
        }
        MainAction::Crouch => {
            ap.flags &= !action_flags::CROUCHING;
        }
        MainAction::FightStance => {
            ap.flags &= !action_flags::FIGHTSTANCE;
        }
        MainAction::DrawWeapon => {
            ap.flags &= !action_flags::WEAPON;
            ap.set_pending(pending_flags::HOLSTER);
        }
        MainAction::LieDown => {
            ap.flags &= !action_flags::LYINGDOWN;
        }
        MainAction::CraneStruggle => {
            ap.record_new_substate_1(sub_state_1::IDLE);
        }
        MainAction::React => {
            ap.record_new_substate_1(sub_state_1::IDLE);
        }
        MainAction::ZipLine => {
            ap.flags &= !action_flags::ZIPLINE;
        }
        MainAction::LedgeHang => {
            ap.flags &= !action_flags::LEDGEHANGING;
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// action_start_system
// ---------------------------------------------------------------------------

pub fn action_start_system(
    mut commands: Commands,
    mut reader: MessageReader<StartActionMessage>,
    mut writer: MessageWriter<ActionStartedMessage>,
    registry: Res<super::actions::ActionRegistry>,
    mut query: Query<(
        &mut ActionPlayer,
        &Oni2AnimLibrary,
        &mut Oni2AnimState,
        Option<&JumpController>,
    )>,
) {
    for msg in reader.read() {
        // Migration: actions registered in the new pipeline are handled
        // by `action_start_dispatch_system`; skip them here so we don't
        // double-dispatch the same StartActionMessage through two
        // competing code paths.
        if registry.contains(msg.action) {
            continue;
        }
        let mut schedule_out: Option<AnimSchedule> = None;
        let result = if let Ok((mut ap, lib, mut state, jc)) = query.get_mut(msg.entity) {
            let r = try_start_action(
                msg.action,
                msg.substate,
                &mut ap,
                lib,
                &mut state,
                jc,
                &mut schedule_out,
            );
            if r == ActionResult::Succeeded {
                ap.last_action = msg.action;
                ap.set_sub_state_0(msg.action, msg.substate);
            }
            r
        } else {
            ActionResult::Failed
        };

        // Attach the multi-stage schedule (if built).  The
        // anim_schedule_tick_system picks it up next tick.
        if result == ActionResult::Succeeded {
            if let Some(schedule) = schedule_out {
                commands.entity(msg.entity).insert(schedule);
            }
        }

        writer.write(ActionStartedMessage {
            entity: msg.entity,
            action: msg.action,
            substate: msg.substate,
            result,
        });
    }
}

// ---------------------------------------------------------------------------
// action_end_system
// ---------------------------------------------------------------------------

pub fn action_end_system(
    mut reader: MessageReader<EndActionMessage>,
    mut writer: MessageWriter<ActionEndedMessage>,
    mut query: Query<(&mut ActionPlayer, &Oni2AnimLibrary, &mut Oni2AnimState)>,
) {
    for msg in reader.read() {
        // Special case from original Oni2: ending CROUCH while LYING_DOWN remaps to LIE/LIE_2STAND.
        let (action, subaction) = if msg.action == MainAction::Crouch {
            if let Ok((ap, _, _)) = query.get(msg.entity) {
                if ap.check_flags(action_flags::LYINGDOWN) {
                    (
                        MainAction::LieDown,
                        super::components::end_adverb::LIE_2STAND,
                    )
                } else {
                    (msg.action, msg.subaction)
                }
            } else {
                (msg.action, msg.subaction)
            }
        } else {
            (msg.action, msg.subaction)
        };

        if let Ok((mut ap, lib, mut state)) = query.get_mut(msg.entity) {
            do_end_action(action, subaction, &mut ap, lib, &mut state);
        }

        writer.write(ActionEndedMessage {
            entity: msg.entity,
            action,
            subaction,
        });
    }
}

// ---------------------------------------------------------------------------
// action_player_tick_system
// ---------------------------------------------------------------------------

/// Per-frame bookkeeping:
///   • Promote sub_state_1 → sub_state_1_last_frame at the top of the tick.
///   • Clear re-enable-collision flag once the react substate ends.
///   • Invoke pending-event handlers tied to substate transitions (draw, holster,
///     arm IK, crouch pickup) — mirrors the state-based section of
///     crAnimActionPlayer::Update().
///   • Detect end-of-jump via `sub_state_1_is_fresh` + landing conditions.
pub fn action_player_tick_system(mut query: Query<(&mut ActionPlayer, &Oni2AnimState)>) {
    for (mut ap, anim) in &mut query {
        // 1) Promote the previous sub_state_1 for `sub_state_1_is_fresh()`.
        ap.sub_state_1_last_frame = ap.sub_state_1;

        // 2) If the animation id just changed, sub_state_1 remains whatever an
        //    earlier system wrote (usually via record_new_substate_1 during
        //    StartAction).  Nothing to do here.

        // 3) Re-enable collision once the react substate clears.
        if ap.reenable_collision_after_react && ap.sub_state_1 != sub_state_1::REACT {
            ap.reenable_collision_after_react = false;
            // Actual collision toggle is owned by the mover; this flag is polled
            // by a downstream system (to be wired).
        }

        // 4) Crouch-pickup only makes sense during a transition.
        if !ap.is_transitioning() && ap.is_pending(pending_flags::CROUCH_PICKUP) {
            ap.clear_pending(pending_flags::CROUCH_PICKUP);
        }

        // 5) Pending weapon events tied to the current substate.
        match ap.sub_state_1 {
            sub_state_1::WEAPON_READY_TO_TARGET => {
                if ap.is_pending(pending_flags::DRAW) {
                    ap.weapon_state = super::components::WeaponState::Drawn;
                    ap.clear_pending(pending_flags::DRAW);
                }
                if ap.is_pending(pending_flags::ARM_IK) {
                    ap.allow_targeting_ik = true;
                    ap.clear_pending(pending_flags::ARM_IK);
                }
            }
            sub_state_1::WEAPON_GET_FROM_HOLSTER => {
                // Mid-anim arm-IK handoff: waits for >50% phase.
                let phase = if anim.anim.num_frames > 1 {
                    anim.current_time / (anim.anim.num_frames as f32 - 1.0).max(1.0)
                } else {
                    0.0
                };
                if ap.is_pending(pending_flags::DRAW) {
                    ap.weapon_state = super::components::WeaponState::Drawn;
                    ap.clear_pending(pending_flags::DRAW);
                }
                if ap.is_pending(pending_flags::ARM_IK) && phase > 0.5 {
                    ap.allow_targeting_ik = true;
                    ap.clear_pending(pending_flags::ARM_IK);
                }
            }
            sub_state_1::WEAPON_DROP_TO_HOLSTER => {
                if ap.is_pending(pending_flags::HOLSTER) {
                    ap.weapon_state = super::components::WeaponState::Holstered;
                    ap.clear_pending(pending_flags::HOLSTER);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// control_anim_system
// ---------------------------------------------------------------------------

pub fn control_anim_system(
    mut reader: MessageReader<ControlAnimMessage>,
    mut query: Query<(&Oni2AnimLibrary, &mut Oni2AnimState)>,
) {
    for msg in reader.read() {
        let Ok((lib, mut state)) = query.get_mut(msg.entity) else {
            continue;
        };

        if let Some(alias) = &msg.animation_alias {
            if !lib.play(alias, &mut state) {
                warn!("control_anim: alias '{}' not in library", alias);
                continue;
            }
        }

        if msg.control & control_anim_bits::RESTART != 0 {
            state.current_time = 0.0;
            state.last_rendered_time = -1.0;
            state.paused = false;
        }
        if msg.control & control_anim_bits::STOP != 0 {
            state.paused = true;
            state.current_time = 0.0;
            state.last_rendered_time = -1.0;
        }
        if msg.control & control_anim_bits::PAUSE != 0 {
            state.paused = msg.pause;
        }
        if msg.control & control_anim_bits::LOOP != 0 {
            state.looping = msg.loop_anim;
        }
        if msg.control & control_anim_bits::SET_RATE != 0 {
            state.speed_multiplier = msg.rate;
        }
        if msg.control & control_anim_bits::HOLD != 0 {
            // HOLD: pause on the final frame once reached.  We emulate by clamping
            // and disabling looping; the playback system already clamps non-looping
            // animations to the last frame.
            state.looping = false;
        }
        if msg.control & control_anim_bits::PAUSE_AT_END != 0 {
            state.looping = false;
            // Playback clamps at final frame; a separate pause_at_end watchdog
            // could set paused=true once we reach the last frame, but the base
            // playback already stops advancing.
        }
    }
}

// ---------------------------------------------------------------------------
// head_ik_mode_system
// ---------------------------------------------------------------------------

pub fn head_ik_mode_system(
    mut reader: MessageReader<HeadIkModeMessage>,
    mut query: Query<&mut ActionPlayer>,
) {
    for msg in reader.read() {
        if let Ok(mut ap) = query.get_mut(msg.entity) {
            ap.head_ik_mode = msg.mode.clone();
        }
    }
}

// ---------------------------------------------------------------------------
// head_ik_bridge_system — translate ActionPlayer.head_ik_mode into the
// existing oni2_loader head-IK runtime by inserting/updating ActiveHeadIK.
// Runs every tick but only re-inserts when the effective ControlHeadTask has
// actually changed so we don't spam Commands.
// ---------------------------------------------------------------------------

pub fn head_ik_bridge_system(
    mut commands: Commands,
    query: Query<
        (
            Entity,
            &ActionPlayer,
            Option<&crate::oni2_loader::components::ActiveHeadIK>,
        ),
        With<Oni2AnimState>,
    >,
) {
    use crate::oni2_loader::components::{ActiveHeadIK, ControlHeadTask};

    for (entity, ap, existing) in &query {
        // Map HeadIkMode → ControlHeadTask.
        //
        // Unit conventions (matching the existing head_ik runtime):
        //   • TrackPosition       — world-space Oni2 coords (the existing
        //                           head_ik_system converts via to_bevy_space_pos).
        //   • Set { azimuth, incline } — degrees.
        //   • Scan { range, period }   — degrees / seconds.
        let task = match &ap.head_ik_mode {
            HeadIkMode::Disabled => ControlHeadTask::Disable,
            HeadIkMode::TrackClosestActor => ControlHeadTask::TrackClosest,
            HeadIkMode::TrackActor { target } => ControlHeadTask::TrackActor(*target),
            HeadIkMode::TrackPosition { pos } => ControlHeadTask::TrackPos(*pos),
            HeadIkMode::Scan { range, period } => ControlHeadTask::Scan {
                range: *range,
                period: *period,
            },
            HeadIkMode::Set { azimuth, incline } => ControlHeadTask::Set {
                azimuth: *azimuth,
                incline: *incline,
            },
        };

        if let Some(active) = existing {
            if active.task == task {
                continue;
            }
        }

        commands.entity(entity).insert(ActiveHeadIK { task });
    }
}

// ---------------------------------------------------------------------------
// play_react_system / play_die_system — convenience adapters
// ---------------------------------------------------------------------------

pub fn play_react_system(
    mut reader: MessageReader<PlayReactMessage>,
    mut writer: MessageWriter<StartActionMessage>,
    mut query: Query<&mut ActionPlayer>,
) {
    for msg in reader.read() {
        if let Ok(mut ap) = query.get_mut(msg.entity) {
            ap.reenable_collision_after_react = msg.disable_creature_detect;
        }
        writer.write(StartActionMessage {
            entity: msg.entity,
            action: MainAction::React,
            substate: msg.react_enum,
        });
    }
}

pub fn play_die_system(
    mut reader: MessageReader<PlayDieMessage>,
    mut writer: MessageWriter<StartActionMessage>,
) {
    for msg in reader.read() {
        writer.write(StartActionMessage {
            entity: msg.entity,
            action: MainAction::Die,
            substate: msg.die_enum,
        });
    }
}

// ---------------------------------------------------------------------------
// jump_impulse_emit_system
// ---------------------------------------------------------------------------

/// Bridge from an animation-phase transition to a physics-level impulse.
///
/// Legacy jump execution flow:
///   1. User action sets crAnimActionPlayer::JumpIndex = N.
///   2. StartAction(ACT_JUMP, substate) queues compress/spring/main/fall on
///      the crAnimList.
///   3. crAnimActionPlayer::Update() sees SubState1IsFresh() && SubState1 is
///      JUMP_MAIN (or JUMP_SOMERSAULT), then calls mover.UsePacketJump(idx),
///      which loads the crJump from crJumpData and applies the impulse.
///
/// Rust: the AnimSchedule stamps SubState1 = JUMP_MAIN on the main entry; we
/// watch for that fresh transition here and look up the JumpState matching
/// ap.jump_index (set during try_start_action).  This reproduces the original Oni2
/// timing exactly — the impulse fires the frame the main anim begins, not on
/// button press.
pub fn jump_impulse_emit_system(
    mut writer: MessageWriter<JumpImpulseMessage>,
    query: Query<(
        Entity,
        &ActionPlayer,
        Option<&crate::oni2_loader::parsers::jump::JumpController>,
    )>,
) {
    for (entity, ap, controller_opt) in &query {
        // Fire only on the frame SubState1 transitions into JUMP_MAIN /
        // JUMP_SOMERSAULT.  `sub_state_1_is_fresh()` returns true on the first
        // tick the state changes.
        if !ap.sub_state_1_is_fresh() {
            continue;
        }
        if ap.sub_state_1 != sub_state_1::JUMP_MAIN
            && ap.sub_state_1 != sub_state_1::JUMP_SOMERSAULT
        {
            continue;
        }

        let Some(controller) = controller_opt else {
            continue;
        };
        let jump_idx = if ap.jump_index >= 0 {
            ap.jump_index as usize
        } else {
            0
        };
        let Some(jump) = controller.jumps.get(jump_idx) else {
            warn!(
                "jump_impulse_emit: entity {:?} requested jump_index={} but only {} entries loaded",
                entity,
                jump_idx,
                controller.jumps.len()
            );
            continue;
        };

        const G: f32 = 9.81;
        let h = jump.height.max(0.0);
        let gf = if jump.gravity_factor > 0.0 {
            jump.gravity_factor
        } else {
            1.0
        };

        let vertical = (2.0 * h * G * gf).sqrt();
        let time = compute_jump_time(jump);
        let along = if time > 0.0 { jump.length / time } else { 0.0 };

        // heading_adjust of ±90° encodes left/right evade — route the scalar
        // length onto the lateral axis instead of forward.
        let (forward, lateral) = if jump.heading_adjust.abs() >= 45.0 {
            let sign = jump.heading_adjust.signum();
            (0.0, along * sign)
        } else {
            (along, 0.0)
        };

        writer.write(JumpImpulseMessage {
            entity,
            vertical,
            forward,
            lateral,
            gravity_factor: gf,
            time,
            heading_adjust: jump.heading_adjust,
        });
    }
}

// ---------------------------------------------------------------------------
// jump_land_on_ground_system
// ---------------------------------------------------------------------------

/// Advances a jump schedule from JUMP_MAIN (or JUMP_SOMERSAULT) into JUMP_LAND
/// the moment the character reconnects with ground.
///
/// `build_jump_schedule` now marks every MAIN/SOMERSAULT entry as
/// `hold_at_end=true`, which writes `looping=true` into the anim state and
/// keeps the airborne FLOAT anim looping forever.  That's the right visual
/// for an in-flight character — the loop is effectively a fall animation —
/// but something has to break the loop when they touch down.  This system
/// is that hook: on a `false → true` edge in `is_grounded`, if the schedule
/// is parked on MAIN/SOMERSAULT, call `schedule.advance()` so LAND kicks in
/// on the next tick.
///
/// Matches the C++ animator.cpp loop: it holds a FALL anim while airborne
/// and on ground contact transitions the action to LAND.
pub fn jump_land_on_ground_system(
    mut query: Query<(
        Entity,
        &mut crate::animator::schedule::AnimSchedule,
        &Oni2AnimState,
    )>,
    mut was_grounded: Local<bevy::ecs::entity::EntityHashMap<bool>>,
) {
    for (entity, mut schedule, anim_state) in &mut query {
        // Default the previous reading to `true` for a fresh entity so the
        // first tick doesn't spuriously fire a "just landed" transition on
        // spawn (when we don't yet know the prior state).
        let prev = was_grounded.get(&entity).copied().unwrap_or(true);
        let curr = anim_state.is_grounded;
        was_grounded.insert(entity, curr);

        if !(!prev && curr) {
            continue;
        }

        // Only advance when the schedule is parked on an airborne entry.
        // Advancing out of COMPRESS (pre-impulse) or LAND (already past)
        // would skip or cut off legitimate frames.
        let Some(current) = schedule.current() else {
            continue;
        };
        let is_airborne_entry = current.substate_1 == sub_state_1::JUMP_MAIN
            || current.substate_1 == sub_state_1::JUMP_SOMERSAULT;
        if !is_airborne_entry {
            continue;
        }

        debug!(
            "jump_land_on_ground: entity {:?} landed while in substate_1={} — advancing schedule to LAND",
            entity, current.substate_1
        );
        schedule.advance();
    }
}

// ---------------------------------------------------------------------------
// schedule_finished_end_action_system
// ---------------------------------------------------------------------------

/// Emits `EndActionMessage` on the frame an entity's `AnimSchedule` just
/// finished.  This is the missing hook that lets `action_end_system` run
/// `do_end_action` and clear action flags (e.g. `CLEARLIST_JUMP` which
/// wipes JUMPING / DOUBLE_JUMPING / RUNNING_JUMP / STANDING_JUMP).
///
/// Matches the legacy animator pipeline: when the crAnimList empties
/// (e.g. the LAND anim finishes at the end of a jump), the animator calls
/// `EndAction` on the owning MainAction, clearing its flags.  Without
/// this, JUMPING stays set forever and re-entering JUMP in an FSM that
/// checks `check_flags(JUMPING)` would silently refuse.
pub fn schedule_finished_end_action_system(
    mut writer: MessageWriter<EndActionMessage>,
    query: Query<(
        Entity,
        &crate::animator::schedule::AnimSchedule,
        &ActionPlayer,
    )>,
    mut was_finished: Local<bevy::ecs::entity::EntityHashMap<bool>>,
) {
    for (entity, schedule, ap) in &query {
        // Default prior state to `true` so we don't spuriously fire
        // EndActionMessage for a freshly-spawned empty schedule.
        let prev = was_finished.get(&entity).copied().unwrap_or(true);
        let curr = schedule.finished;
        was_finished.insert(entity, curr);

        if !(!prev && curr) {
            continue;
        }
        if ap.last_action == MainAction::None {
            continue;
        }

        let subaction_idx = ap.last_action as usize;
        let subaction = ap.sub_state_0.get(subaction_idx).copied().unwrap_or(0);

        debug!(
            "schedule_finished_end_action: entity {:?} schedule finished — emitting EndActionMessage(action={:?}, subaction={})",
            entity, ap.last_action, subaction
        );
        writer.write(EndActionMessage {
            entity,
            action: ap.last_action,
            subaction,
        });
    }
}

// ---------------------------------------------------------------------------
// Jump impulse application
// ---------------------------------------------------------------------------

/// Consumes `JumpImpulseMessage` (emitted by the animator when SubState1
/// transitions into JUMP_MAIN / JUMP_SOMERSAULT) and applies the computed
/// velocity to the player's `LinearVelocity`.  It effectively sets both
/// vertical and planar horizontal impulses derived from the .jump data
/// (height, gravity_factor, length, heading_adjust).
///
/// Vertical component: overwrites `velocity.y` directly so the jump reaches
/// the scripted apex regardless of current downward velocity (falls reset).
///
/// Horizontal components: projected onto the entity's facing plane.
///   • `forward` → along `fighter.facing` (XZ).
///   • `lateral` → perpendicular to facing, +lateral = right (heading_adjust
///     = +90° in the legacy encoding).
///
/// When the message carries nonzero horizontal components, the existing
/// XZ velocity is replaced.  For vertical-only jumps (length=0 in kno.jump),
/// XZ is left alone so the jumper keeps any running start.
pub fn jump_impulse_apply_system(
    mut reader: MessageReader<crate::animator::JumpImpulseMessage>,
    mut commands: Commands,
    mut query: Query<(&mut avian3d::prelude::LinearVelocity, &crate::combat::components::Fighter)>,
) {
    for msg in reader.read() {
        let Ok((mut vel, fighter)) = query.get_mut(msg.entity) else {
            continue;
        };

        // Apply the jump's gravity scale so the airborne arc matches the
        // kno.jump data (height 1.56m, factor 3.2 → total airtime 0.63s).
        // Reset happens in `jump_gravity_reset_system` on next ground contact.
        if (msg.gravity_factor - 1.0).abs() > 0.01 {
            info!(
                "GRAVITY SET: entity {:?} → factor={:.2} (jump impulse: \
                 vertical={:.2}, forward={:.2}, lateral={:.2}, time={:.2}s)",
                msg.entity,
                msg.gravity_factor,
                msg.vertical,
                msg.forward,
                msg.lateral,
                msg.time,
            );
            commands
                .entity(msg.entity)
                .insert(avian3d::prelude::GravityScale(msg.gravity_factor));
        }

        vel.0.y = msg.vertical;

        if msg.forward.abs() > 0.001 || msg.lateral.abs() > 0.001 {
            let facing = {
                let f = Vec3::new(fighter.facing.x, 0.0, fighter.facing.z);
                f.normalize_or_zero()
            };
            if facing.length_squared() > 0.001 {
                let right = facing.cross(Vec3::Y).normalize_or_zero();
                let horiz = facing * msg.forward + right * msg.lateral;
                vel.0.x = horiz.x;
                vel.0.z = horiz.z;
            }
        }
    }
}

/// Restore normal gravity once the jumper lands.  A `GravityScale` left in
/// place after landing would make walking feel heavier than intended.  Runs
/// every tick and is a no-op when there's nothing to reset.
///
/// Gated on `is_grounded AND NOT ascending`: right after the jump impulse
/// fires, the ShapeCaster still reports ground contact for 1-2 FixedUpdate
/// ticks (velocity has been applied but position hasn't separated yet),
/// which would trigger a spurious reset and clobber the jump's gravity
/// factor — making the rest of the arc play under normal gravity.
/// Checking `linear_velocity.y <= 0` prevents that: we only reset when the
/// character is genuinely standing / landing, not in the process of
/// launching.
pub fn jump_gravity_reset_system(
    mut commands: Commands,
    mut query: Query<(
        Entity,
        Option<&crate::oni2_loader::animation::Oni2AnimState>,
        Option<&avian3d::prelude::LinearVelocity>,
        &avian3d::prelude::GravityScale,
        Option<&mut ActionPlayer>,
    )>,
) {
    const ASCENDING_VY_THRESHOLD: f32 = 0.5;

    for (entity, anim_state_opt, vel_opt, scale, ap_opt) in &mut query {
        let is_grounded = anim_state_opt.map_or(true, |s| s.is_grounded);
        if !is_grounded {
            continue;
        }

        // Suppress reset while the character is actively ascending — they
        // just fired a jump impulse and Avian hasn't moved them off the
        // ground collider yet.  Once velocity decays (apex or descent) we
        // resume the reset path on the next grounded tick.
        let vy = vel_opt.map_or(0.0, |v| v.0.y);
        if vy > ASCENDING_VY_THRESHOLD {
            continue;
        }

        if let Some(mut ap) = ap_opt {
            ap.jumps_remaining = ap.max_jumps;
        }
        if (scale.0 - 1.0).abs() > 0.01 {
            info!(
                "GRAVITY RESET: entity {:?} {:.2} → 1.0 (grounded, vy={:.2})",
                entity, scale.0, vy,
            );
            commands
                .entity(entity)
                .insert(avian3d::prelude::GravityScale(1.0));
        }
    }
}

// ---------------------------------------------------------------------------
// Animator Dispatch
// ---------------------------------------------------------------------------

/// Listens for top-level AnimatorBroadcastEvent messages generated from the
/// character FSM and dispatches the native movement routines (translating string logic down to internal StartActionMessages).
pub fn animator_broadcast_handler(
    mut events: MessageReader<crate::statemachine::runtime::AnimatorBroadcastEvent>,
    mut start_action_writer: MessageWriter<crate::animator::StartActionMessage>,
    mut player_query: Query<(
        &mut ActionPlayer,
        Option<&crate::player::components::InputState>,
    )>,
) {
    for ev in events.read() {
        match ev.payload.as_str() {
            "StartJumpCompress" => {
                if let Ok((mut ap, input_opt)) = player_query.get_mut(ev.entity) {
                    // Ensure jumps are replenished if we're technically grounded or starting our first jump
                    let is_first_jump = ap.jumps_remaining == ap.max_jumps;
                    if ap.jumps_remaining > 0 {
                        ap.jumps_remaining -= 1;
                    }

                    let substate = if !is_first_jump {
                        crate::animator::sub_state_0::JUMP_SOMERSAULT
                    } else if let Some(input) = input_opt {
                        if input.movement.length_squared() > 0.05 {
                            if input.movement.y < -0.1 {
                                crate::animator::sub_state_0::JUMP_FORWARD
                            } else if input.movement.y > 0.1 {
                                crate::animator::sub_state_0::JUMP_BACK
                            } else if input.movement.x > 0.1 {
                                crate::animator::sub_state_0::JUMP_LEFT
                            } else if input.movement.x < -0.1 {
                                crate::animator::sub_state_0::JUMP_RIGHT
                            } else {
                                crate::animator::sub_state_0::JUMP_UP
                            }
                        } else {
                            crate::animator::sub_state_0::JUMP_UP
                        }
                    } else {
                        // AI jump default for now (could parse velocity/facing intent instead later)
                        crate::animator::sub_state_0::JUMP_UP
                    };

                    start_action_writer.write(crate::animator::StartActionMessage {
                        entity: ev.entity,
                        action: crate::animator::MainAction::Jump,
                        substate,
                    });
                }
            }

            // FSM entered FALL from somewhere other than an internal
            // Jump→Fall handoff (e.g. IDLE→FALL when walking off a
            // ledge or on spawn).  Kick off a fresh FallAction so the
            // fall anim plays + the landing handoff is armed.
            "StartFall" => {
                start_action_writer.write(crate::animator::StartActionMessage {
                    entity: ev.entity,
                    action: crate::animator::MainAction::Fall,
                    substate: 0,
                });
            }

            // The FSM broadcasts `StartLand` / `StartHardLand` when
            // transitioning to #LAND, but LAND is an internal phase of
            // JumpAction / FallAction (no MainAction::Land exists — see
            // the action-pipeline design note in animator/actions).  The
            // Actions detect ground contact themselves via the physics
            // edge in `ActionCtx.ground_just_regained`, so these
            // broadcasts are informational — intentional no-ops here
            // kept for FX / audio hooks to subscribe to later.
            "StartLand" | "StartHardLand" => {}

            // StartDeath / StartReact / StartEvade / StartSlide /
            // StartCrouch / EndCrouch / StartLedgeGrab / StartLedgeClamber
            // / StartZiplineGrab / EndZipline / EndReact / StartPickup /
            // StartCustomAnim: not yet wired to Actions; silent no-ops
            // until those modes migrate to the action pipeline.
            "StartDeath" | "StartReact" | "EndReact" | "StartEvade" | "StartSlide"
            | "StartCrouch" | "EndCrouch" | "StartLedgeGrab" | "StartLedgeClamber"
            | "StartZiplineGrab" | "EndZipline" | "StartPickup" | "StartCustomAnim" => {}

            _ => {
                bevy::log::warn!(
                    "Unhandled AnimatorBroadcastEvent: '{}' for entity {:?}",
                    ev.payload,
                    ev.entity
                );
            }
        }
    }
}
