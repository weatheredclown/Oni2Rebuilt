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
        let mut best: Option<(Entity, f32)> = None;
        for (player_entity, player_tf) in &players {
            let dist = ai_tf.translation.distance(player_tf.translation);
            if dist <= AWARENESS_RANGE {
                if best.map_or(true, |(_, d)| dist < d) {
                    best = Some((player_entity, dist));
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
        &mut BlockState,
        &HitReaction,
        &AboutToBeHit,
        &GrabState,
    )>,
    targets: Query<&Transform>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs_f64();
    let mut rng = rand::rng();

    for (mut ai, ai_tf, mut attack_state, mut block_state, reaction, about_to_be_hit, grab) in
        &mut ai_query
    {
        // Priority 1: If in a hit reaction, go to Recovering
        if reaction.active.is_some() {
            ai.state = AiState::Recovering;
            block_state.is_blocking = false;
            continue;
        }

        // Priority 2: If in a grab, don't interfere
        if grab.phase.is_some() {
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
            block_state.is_blocking = false;
            continue;
        };

        let Ok(target_tf) = targets.get(target_entity) else {
            ai.target = None;
            ai.state = AiState::Idle;
            block_state.is_blocking = false;
            continue;
        };

        let to_target = target_tf.translation - ai_tf.translation;
        let distance = Vec3::new(to_target.x, 0.0, to_target.z).length();

        // attack range (range_extension is negative = further reach)
        let effective_attack_range = ATTACK_RANGE;

        // Priority 3: React to incoming attack with block
        if about_to_be_hit.active.is_some() && ai.state != AiState::Attacking {
            if rng.random_range(0.0..1.0) < ai.block_probability {
                ai.state = AiState::Blocking;
                block_state.is_blocking = true;
                continue;
            }
        }

        // If blocking and threat cleared, stop blocking
        if ai.state == AiState::Blocking {
            if about_to_be_hit.active.is_none() {
                block_state.is_blocking = false;
                ai.state = AiState::Circling;
                ai.decision_timer = rng.random_range(0.3..0.6);
            }
            continue;
        }

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
                    let attack_chance = ai.aggression * (1.0 - (distance / effective_attack_range).min(1.0)) + 0.2;
                    if rng.random_range(0.0..1.0) < attack_chance && distance <= effective_attack_range {
                        // Can't attack if already attacking, in cooldown, or reacting
                        if attack_state.active_attack.is_none()
                            && now >= attack_state.cooldown_until
                        {
                            // Pick attack type: weighted toward punches
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

                            let attack = ActiveAttack::new_with_weapon(
                                class,
                                strength,
                                target,
                                1.0, // Damage mtplr
                                1.0, // Speed mtplr
                                0.0, // AI always attacks forward for now
                            );

                            attack_state.active_attack = Some(attack);
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
    )>,
    targets: Query<&Transform, Without<AiFighter>>,
) {
    for (ai, mut ai_tf, mut velocity, mut fighter) in &mut ai_query {
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

        if distance < 0.01 {
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
                let close_component = dir_to_target * range_diff.clamp(-1.0, 1.0) * CIRCLE_CLOSE_SPEED;

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
            AiState::Idle | AiState::Blocking | AiState::Recovering => {
                velocity.x = 0.0;
                velocity.z = 0.0;
            }
        }
    }
}
