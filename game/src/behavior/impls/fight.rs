/*
 * behavior/impls/fight.rs — FightBehavior: drive AI combat.
 *
 * Today's minimum viable path: on a decision interval, kick off an
 * attack animation directly via `anim_lib.play(name, anim_state)`.
 * This is the exact same helper the player's FSM `DoAttack` action
 * eventually resolves to (via `runtime.rs`'s `FsmOutput.attack_anim`
 * drain).  Both paths — `DoAttack ANIMATTACK_1` in an input FSM and
 * `attack ANIMATTACK_1` in a `.atk` file — share that terminal DNA.
 *
 * The `.atk` state machine (AttackRuntime) isn't driving attack
 * selection yet: its files use the Format-2 nested-brace grammar which
 * `parse_sm` doesn't cover, so today the loaded machine is effectively
 * empty.  FightBehavior hardcodes `ANIMATTACK_1` as a placeholder until
 * the Format-2 parser and the fight coordinator (`fight.fsm`) come
 * online.  Once they do, attack-name selection moves into the .atk
 * runtime and this behavior becomes the thin shim that advances it.
 */
use super::super::{Behavior, BehaviorRunCtx, BehaviorUpdate};
use crate::statemachine::drivers::behavior::BehaviorKind;
use bevy::prelude::*;

/// Seconds between attack kickoffs.  Long enough that the previous
/// attack's animation can complete on a typical 30fps anim.  Real
/// cadence will come from the fight-coordinator port.
const ATTACK_DECISION_INTERVAL: f32 = 2.0;

/// Placeholder attack-anim alias fired every decision tick until the
/// `.atk` runtime drives selection.  ANIMATTACK_1 is the canonical
/// first-attack slot used across atk files (see bra1.atk `A1 "Attack 1"
/// { attack ANIMATTACK_1 }`).
const DEFAULT_ATTACK_ANIM: &str = "ANIMATTACK_1";

#[derive(Default)]
pub struct FightBehavior {
    decision_timer: f32,
}

impl Behavior for FightBehavior {
    fn kind(&self) -> BehaviorKind {
        BehaviorKind::Fight
    }

    fn on_enter(&mut self, ctx: &mut BehaviorRunCtx<'_>) {
        // Copy target from pending_params onto AiFighter so downstream
        // systems (hit detection, VM status queries, coordinator when it
        // lands) see the combat target on the expected component.
        if let (Some(target), Some(af)) =
            (ctx.params.target_entity, ctx.ai_fighter.as_deref_mut())
        {
            af.target = Some(target);
            af.manual_target = true;
        }
        // Fire the first attack on the very next update tick rather than
        // waiting a full interval.
        self.decision_timer = 0.0;
        // Stop any residual steering from the previous behavior.
        ctx.velocity.x = 0.0;
        ctx.velocity.z = 0.0;
    }

    fn update(&mut self, ctx: &mut BehaviorRunCtx<'_>, dt: f32) -> BehaviorUpdate {
        // Hold position — approach / distance management will come from
        // the fight-coordinator port.
        ctx.velocity.x = 0.0;
        ctx.velocity.z = 0.0;

        // Face the target so the attack's hitbox aims at them.  Oni2
        // models face +Z locally; look_at points -Z at target, so rotate
        // 180° Y afterward.
        if let Some(target_pos) = ctx.target_position {
            let here = ctx.transform.translation;
            let mut delta = target_pos - here;
            delta.y = 0.0;
            let dist = delta.length();
            if dist > 0.01 {
                let dir = delta / dist;
                ctx.fighter.facing = dir;
                let look_target = here + dir;
                let mut rot_tf = *ctx.transform;
                rot_tf.look_at(look_target, Vec3::Y);
                rot_tf.rotate_y(std::f32::consts::PI);
                ctx.transform.rotation = ctx
                    .transform
                    .rotation
                    .slerp(rot_tf.rotation, (10.0 * dt).min(1.0));
            }
        }

        // Tick the attack cadence.  When the interval elapses, call the
        // shared `DoAttack` helper — same named entry point the player
        // FSM hits when draining `FsmOutput.attack_anim`.  Rotation
        // notches = 0 for now (placeholder until the .atk runtime picks
        // the attack and supplies ATDT-authored end-rotation).
        self.decision_timer -= dt;
        if self.decision_timer <= 0.0 {
            if let (Some(lib), Some(state)) = (ctx.anim_lib, ctx.anim_state.as_deref_mut()) {
                info!(
                    "FightBehavior: DoAttack → '{}' on {:?}",
                    DEFAULT_ATTACK_ANIM, ctx.entity
                );
                crate::statemachine::runtime::do_attack(
                    lib,
                    state,
                    ctx.fighter,
                    DEFAULT_ATTACK_ANIM,
                    0,
                );
            }
            self.decision_timer = ATTACK_DECISION_INTERVAL;
        }

        BehaviorUpdate::Continue
    }

    fn on_exit(&mut self, _ctx: &mut BehaviorRunCtx<'_>) {
        // No teardown needed — the current attack anim keeps playing to
        // its natural end on the animator side regardless of which
        // behavior installs next.  AiFighter.target is left intact so
        // follow-ups (scripts, status queries) still see who was fought.
    }
}
