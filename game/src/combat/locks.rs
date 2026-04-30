/*
 * combat/locks.rs — actor-state gating predicates.
 *
 * Centralizes the "actor is committed to something — don't drive it
 * with movement / locomotion selection / pad input" rules so every
 * consumer sees the same answer.  Mirrors legacy
 * `bhBehaviorComponent::LockedMovement` (rb/src/behavior/component.cpp:
 * 1176), which is consulted from many places (the packet→velocity path
 * at component.cpp:674, gotopoint.cpp:1122/2322/2475, pad.cpp:1214).
 */

use crate::oni2_loader::animation::Oni2AnimState;

/// Returns true iff movement / locomotion / pad-driven steering should
/// be suppressed for this actor this tick.
///
/// **Legacy reference:** `bhBehaviorComponent::LockedMovement`
/// (rb/src/behavior/component.cpp:1176).  Legacy ORs together
/// `IsReacting() / IsBlocking() / IsGrappling() / IsAttacking()` (with
/// a jump carve-out), then also defers to the animator's
/// `LockOutInput()`.  Each of those four predicates ultimately implied
/// the actor's animator was running a non-looping one-shot, so the
/// initial port collapses to that single signal:
/// `Oni2AnimState::is_one_shot_in_progress`.
///
/// **Why a named helper rather than inlining the predicate at each
/// call site:** as we port more of the legacy combat surface, this
/// function will grow new inputs (HitReaction component, jumping flag,
/// blocking sub-state, animator `LockOutInput` flag, ...).  Keeping the
/// callers funneling through `locked_movement` means we update one
/// place when the rule gets richer; the call sites stay stable.
///
/// Current callers:
///   - `behavior::behavior_update_dispatch_system` — zeroes
///     `LinearVelocity.x/z` after the active behavior writes them,
///     suppressing slide-during-attack.
///   - `oni2_loader::spawn::creature_movement_anim_system` — skips
///     locomotion gait selection so the AI's attack swing isn't
///     clobbered by `ANIMNAV_STAND` mid-swing.
pub fn locked_movement(anim_state: &Oni2AnimState) -> bool {
    anim_state.is_one_shot_in_progress()
}
