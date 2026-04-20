/*
 * ai/systems.rs — AI behaviour systems.
 *
 * ai_target_system: picks the nearest Player as the combat target.
 * ai_decision_system: evaluates priorities (attack, pursue, circle) and updates
 * AiState + emits AttackMessage.
 * ai_movement_system: converts AiState into LinearVelocity toward / around target.
 */
use avian3d::prelude::*;
use bevy::prelude::*;
use rand::Rng;

use crate::combat::components::*;
use crate::player::components::Player;

use super::components::*;

const AWARENESS_RANGE: f32 = 20.0;
const ATTACK_RANGE: f32 = 3.0;
const MOVE_SPEED: f32 = 4.5;
const CIRCLE_STRAFE_SPEED: f32 = 3.0;
const CIRCLE_CLOSE_SPEED: f32 = 1.0;

/// Finds the nearest Player entity and sets it as the AI's target.
pub fn ai_target_system(
    mut ai_query: Query<(&mut AiFighter, &Transform)>,
    players: Query<(Entity, &Transform), With<Player>>,
) {
    for (mut ai, ai_tf) in &mut ai_query {
        if ai.manual_target {
            continue;
        }

        let mut best: Option<(Entity, f32)> = None;
        let awareness_range_sq = AWARENESS_RANGE * AWARENESS_RANGE;
        for (player_entity, player_tf) in &players {
            let dist_sq = ai_tf.translation.distance_squared(player_tf.translation);
            if dist_sq <= awareness_range_sq {
                if best.map_or(true, |(_, d)| dist_sq < d) {
                    best = Some((player_entity, dist_sq));
                }
            }
        }
        ai.target = best.map(|(e, _)| e);
    }
}

/// The AI brain. Runs state transitions, ticks timers, picks attacks and blocks.
pub fn ai_decision_system(
    mut ai_query: Query<(
        &mut AiFighter,
        &Transform,
        &mut AttackState,
        &HitReaction,
        Option<&crate::ai::navigation::ActorPathfollower>,
    )>,
    targets: Query<&Transform>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    let mut rng = rand::rng();

    for (mut ai, ai_tf, mut attack_state, reaction, follower_opt) in &mut ai_query {
        if follower_opt.is_some() {
            // Let the path following system handle its state.
            continue;
        }
        // Priority 1: If in a hit reaction, go to Recovering
        if reaction.active.is_some() {
            ai.state = AiState::Recovering;
            continue;
        }

        // If recovering and reaction just ended, return to Circling
        if ai.state == AiState::Recovering {
            ai.state = if ai.target.is_some() {
                AiState::Circling
            } else {
                AiState::Idle
            };
            ai.decision_timer = rng.random_range(0.3..0.8);
        }

        // No target -> Idle
        let Some(target_entity) = ai.target else {
            ai.state = AiState::Idle;
            continue;
        };

        let Ok(target_tf) = targets.get(target_entity) else {
            ai.target = None;
            ai.state = AiState::Idle;
            continue;
        };

        let to_target = target_tf.translation - ai_tf.translation;
        let distance = Vec3::new(to_target.x, 0.0, to_target.z).length();

        // attack range (range_extension is negative = further reach)
        let effective_attack_range = ATTACK_RANGE;

        // If attacking, wait for attack to complete
        if ai.state == AiState::Attacking {
            if attack_state.active_attack.is_none() {
                ai.state = AiState::Circling;
                ai.decision_timer = rng.random_range(0.5..1.2);
            }
            continue;
        }

        // State transitions based on distance
        match ai.state {
            AiState::Idle => {
                if distance <= AWARENESS_RANGE {
                    ai.state = AiState::Pursuing;
                }
            }
            AiState::Pursuing => {
                if distance <= effective_attack_range {
                    ai.state = AiState::Circling;
                    ai.decision_timer = rng.random_range(0.3..0.8);
                }
            }
            AiState::Circling => {
                // Tick circle switch timer
                ai.circle_switch_timer -= dt;
                if ai.circle_switch_timer <= 0.0 {
                    ai.circle_direction = -ai.circle_direction;
                    ai.circle_switch_timer = rng.random_range(1.5..3.5);
                }

                // If target moved out of attack range, pursue again
                if distance > effective_attack_range * 1.5 {
                    ai.state = AiState::Pursuing;
                    continue;
                }

                // Tick decision timer for attack
                ai.decision_timer -= dt;
                if ai.decision_timer <= 0.0 {
                    // Decide whether to attack
                    let attack_chance =
                        ai.aggression * (1.0 - (distance / effective_attack_range).min(1.0)) + 0.2;
                    if rng.random_range(0.0..1.0) < attack_chance
                        && distance <= effective_attack_range
                    {
                        // Can't attack if already attacking or reacting
                        if attack_state.active_attack.is_none() {
                            // Pick attack type: weighted toward punches (FSM proxy for now)
                            /*
                            let roll: f32 = rng.random_range(0.0..1.0);
                            let (class, strength, target) = if roll < 0.5 {
                                (AttackClass::Punch, AttackStrength::Low, AttackTarget::Body)
                            } else if roll < 0.75 {
                                (AttackClass::Punch, AttackStrength::High, AttackTarget::Head)
                            } else if roll < 0.9 {
                                (AttackClass::Kick, AttackStrength::Low, AttackTarget::Legs)
                            } else {
                                (AttackClass::Kick, AttackStrength::High, AttackTarget::Head)
                            };

                            let attack = ActiveAttack::new_with_modifiers(
                                class, strength, target, 1.0, // Damage mtplr
                                1.0, // Speed mtplr
                                0.0, // AI always attacks forward for now
                            );

                            attack_state.active_attack = Some(attack);
                            */
                            ai.state = AiState::Attacking;
                        }
                    }

                    // Reset decision timer regardless
                    let base_interval = 1.5 - ai.aggression;
                    ai.decision_timer = rng.random_range(base_interval * 0.5..base_interval * 1.5);
                }
            }
            _ => {}
        }
    }
}

/// Drives LinearVelocity and Fighter.facing based on AI state.
pub fn ai_movement_system(
    mut ai_query: Query<(
        &AiFighter,
        &mut Transform,
        &mut LinearVelocity,
        &mut Fighter,
        Option<&crate::ai::navigation::ActorPathfollower>,
    )>,
    targets: Query<&Transform, Without<AiFighter>>,
) {
    for (ai, mut ai_tf, mut velocity, mut fighter, follower_opt) in &mut ai_query {
        if follower_opt.is_some() {
            continue;
        }
        let Some(target_entity) = ai.target else {
            // No target: stop moving
            velocity.x = 0.0;
            velocity.z = 0.0;
            continue;
        };

        let Ok(target_tf) = targets.get(target_entity) else {
            velocity.x = 0.0;
            velocity.z = 0.0;
            continue;
        };

        let to_target = target_tf.translation - ai_tf.translation;
        let horizontal = Vec3::new(to_target.x, 0.0, to_target.z);
        let distance = horizontal.length();

        if distance < 1.0 {
            velocity.x = 0.0;
            velocity.z = 0.0;
            continue;
        }

        let dir_to_target = horizontal / distance;

        // Always face the target
        let look_target = Vec3::new(
            target_tf.translation.x,
            ai_tf.translation.y,
            target_tf.translation.z,
        );
        ai_tf.look_at(look_target, Vec3::Y);
        // Oni2 models face +Z in local space; look_at points -Z at target,
        // so rotate 180° Y to make the model visually face the target.
        ai_tf.rotate_y(std::f32::consts::PI);
        fighter.facing = dir_to_target;

        match ai.state {
            AiState::Pursuing => {
                let desired = dir_to_target * MOVE_SPEED;
                velocity.x = desired.x;
                velocity.z = desired.z;
            }
            AiState::Circling => {
                // Strafe perpendicular to target direction
                let strafe_dir = Vec3::new(
                    -dir_to_target.z * ai.circle_direction,
                    0.0,
                    dir_to_target.x * ai.circle_direction,
                );

                // Close/retreat to maintain preferred range
                let range_diff = distance - ai.preferred_range;
                let close_component =
                    dir_to_target * range_diff.clamp(-1.0, 1.0) * CIRCLE_CLOSE_SPEED;

                let desired = strafe_dir * CIRCLE_STRAFE_SPEED + close_component;
                velocity.x = desired.x;
                velocity.z = desired.z;
            }
            AiState::Attacking => {
                // Slight forward movement during attack to close gap
                let desired = dir_to_target * 1.5;
                velocity.x = desired.x;
                velocity.z = desired.z;
            }
            AiState::Idle | AiState::Recovering => {
                velocity.x = 0.0;
                velocity.z = 0.0;
            }
        }
    }
}
