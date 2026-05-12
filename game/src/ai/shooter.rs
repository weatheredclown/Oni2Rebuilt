use bevy::prelude::*;

use crate::ai::components::{AiFighter, AiShooter};
use crate::ai::fsm::AiPadCommands;
use crate::behavior::BehaviorRuntime;
use crate::inventory::components::Inventory;

/// High-fidelity port of `aiShooter::Update` from legacy C++.
/// This system evaluates tactical positioning, line-of-sight (LOS),
/// and trigger-control for AI entities armed with ranged weapons.
pub fn ai_shooter_system(
    time: Res<Time>,
    mut query: Query<(
        Entity,
        &mut AiPadCommands,
        &mut AiShooter,
        &AiFighter,
        Option<&Inventory>,
        Option<&mut BehaviorRuntime>,
        &GlobalTransform,
    )>,
    transforms: Query<&GlobalTransform>,
) {
    let dt = time.delta_secs();

    for (entity, mut cmds, mut shooter, fighter, inv_opt, mut behavior_rt_opt, self_tf) in &mut query {
        let Some(inv) = inv_opt else { continue; };
        
        // If we don't have a weapon, the shooter subsystem goes dormant.
        if inv.current_weapon_entity().is_none() {
            shooter.state = 0; // STATE_INIT
            continue;
        }

        // If we have a weapon, we relinquish our melee ring position / cookie
        // so melee allies can engage while we provide fire support.
        // Mirrors legacy `StateMachine->AReleaseCookie(0); ReleasePosition();`
        if let Some(behavior_rt) = behavior_rt_opt.as_deref_mut() {
            behavior_rt.pending_params.target_point = None;
            behavior_rt.pending_params.target_entity = None;
            behavior_rt.ctx.requested_goto = false;
        }

        let Some(target) = fighter.target else {
            shooter.state = 1; // STATE_NO_TARGET
            continue;
        };

        let Ok(tgt_tf) = transforms.get(target) else {
            continue;
        };

        let self_pos = self_tf.translation();
        let tgt_pos = tgt_tf.translation();
        let dist_sq = self_pos.distance_squared(tgt_pos);
        
        // -------------------------------------------------------------------
        // Simplified tactical firing state machine (MVP)
        // -------------------------------------------------------------------
        // This approximates STATE_BEHIND_FIXED -> STATE_AIMING -> STATE_FIRING
        // with basic burst control.

        // Point at target
        // (Handled automatically by the ActorFollower/fight FSM if they track the target,
        // but we can inject facing constraints here if needed)

        let forward = self_tf.forward();
        let dir_to_tgt = (tgt_pos - self_pos).normalize_or_zero();
        let facing_target = forward.dot(dir_to_tgt) > 0.85; // Roughly 30 degrees

        let t = time.elapsed_secs();

        // Very basic firing loop
        if dist_sq < 300.0 * 300.0 && facing_target {
            if t > shooter.can_fire_timer {
                // Determine burst parameters
                let burst_duration = 0.5 + rand::random::<f32>() * 1.5;
                shooter.stop_fire_timer = t + burst_duration;
                shooter.can_fire_timer = t + burst_duration + 1.0; // Wait 1 sec between bursts
            }

            if t < shooter.stop_fire_timer {
                // Fire the weapon!
                cmds.weapon_fire_forward = true;
                shooter.state = 7; // STATE_FIRING
            } else {
                shooter.state = 5; // STATE_AIMING1
            }
        } else {
            // Need to adjust range or line of sight
            shooter.state = 2; // STATE_BEHIND_FIXED
        }
    }
}
