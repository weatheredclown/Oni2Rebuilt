/*
 * behavior/impls/fight.rs — FightBehavior: grant the attack cookie and face
 * the target.
 *
 * The behavior layer's job in fight mode is narrow: flip the actor's
 * `AttackRuntime.ctx.got_cookie` to true (so the .atk machine switches
 * from no_cookie "ambient" mood rows to cookie "attacking" rows) and
 * keep the actor facing the target so the cylinder-slice hit test can
 * land.  Attack selection itself lives in the `.atk` state machine —
 * `attack_runtime_update_system` ticks it and drains its `attack_anim`
 * output through `do_attack`.
 *
 * Mirrors the legacy split: the fight coordinator (`aiFightStateMachine`)
 * grants the cookie by calling `aiFighter::TryAttack` — we're just
 * standing in for that coordinator until its port lands.
 */
use super::super::{Behavior, BehaviorRunCtx, BehaviorUpdate};
use crate::statemachine::drivers::behavior::BehaviorKind;
use bevy::prelude::*;

#[derive(Default)]
pub struct FightBehavior;

impl Behavior for FightBehavior {
    fn kind(&self) -> BehaviorKind {
        BehaviorKind::Fight
    }

    fn on_enter(&mut self, ctx: &mut BehaviorRunCtx<'_>) {
        // Stamp the target from pending_params onto AiFighter so downstream
        // systems (hit detection, VM status queries, the fight coordinator
        // when it lands) see it on the expected component.
        if let (Some(target), Some(af)) = (ctx.params.target_entity, ctx.ai_fighter.as_deref_mut())
        {
            af.target = Some(target);
            af.manual_target = true;
        }
        // Grant the attack cookie.  This is a placeholder for the fight
        // coordinator's position/cookie queue — here we just assume
        // "fighting means I have the floor to attack" until the real
        // coordinator is online.
        if let Some(atk) = ctx.attack_runtime.as_deref_mut() {
            atk.ctx.got_cookie = true;
            bevy::log::info!(
                "FightBehavior::on_enter: granted cookie on {:?}",
                ctx.entity
            );
        }
        // Stop any residual steering from the previous behavior.
        ctx.velocity.x = 0.0;
        ctx.velocity.z = 0.0;
    }

    fn update(&mut self, ctx: &mut BehaviorRunCtx<'_>, dt: f32) -> BehaviorUpdate {
        // Hold position — approach / distance management will come from
        // the fight-coordinator port (via cookie-table move options like
        // `distance 2.0`).
        ctx.velocity.x = 0.0;
        ctx.velocity.z = 0.0;

        // Face the target so attack hitboxes aim at them.  Oni2 models
        // face +Z locally; look_at points -Z at target, so rotate 180°
        // Y afterward.
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

        BehaviorUpdate::Continue
    }

    fn on_exit(&mut self, ctx: &mut BehaviorRunCtx<'_>) {
        // Revoke the cookie — the actor stops being "the one attacking
        // right now".  The .atk machine will drop back to no_cookie rows
        // on its next pick.
        if let Some(atk) = ctx.attack_runtime.as_deref_mut() {
            atk.ctx.got_cookie = false;
            bevy::log::info!("FightBehavior::on_exit: revoked cookie on {:?}", ctx.entity);
        }
    }
}
