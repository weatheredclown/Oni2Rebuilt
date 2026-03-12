use avian3d::prelude::*;
use bevy::prelude::*;
use rb_shared::events::CombatEvent;

use crate::player::components::{InputState, Player};
use crate::telemetry::bridge::TelemetryChannel;

use super::components::*;
use super::events::*;

const HIT_RADIUS: f32 = 0.6;
const INVULN_DURATION: f64 = 0.2;
const FIST_REST: Vec3 = Vec3::new(0.3, 0.3, -0.5);
const FIST_EXTENDED: Vec3 = Vec3::new(0.3, 0.3, -2.0);
const GRAB_DAMAGE: f32 = 15.0;
const GRAB_HOLD_MAX: f32 = 2.0;

/// Reads player InputState and starts attacks on AttackState.
pub fn attack_input_system(
    mut query: Query<(&InputState, &mut AttackState, &HitReaction, &GrabState), With<Player>>,
    time: Res<Time>,
) {
    for (input, mut attack_state, reaction, grab) in &mut query {
        if reaction.active.is_some() || grab.phase.is_some() {
            continue;
        }
        if attack_state.active_attack.is_some() {
            continue;
        }
        if time.elapsed_secs_f64() < attack_state.cooldown_until {
            continue;
        }

        let attack = if input.light_attack {
            Some(ActiveAttack::new_with_modifiers(
                AttackClass::Punch,
                AttackStrength::Low,
                AttackTarget::Body,
                1.0, // dmg
                1.0, // spd
                input.attack_direction,
            ))
        } else if input.heavy_attack {
            Some(ActiveAttack::new_with_modifiers(
                AttackClass::Kick,
                AttackStrength::High,
                AttackTarget::Head,
                1.0, // dmg
                1.0, // spd
                input.attack_direction,
            ))
        } else {
            None
        };

        if let Some(attack) = attack {
            attack_state.active_attack = Some(attack);
        }
    }
}

/// Advances active attack elapsed time. Clears finished attacks.
pub fn attack_advance_system(mut query: Query<&mut AttackState>, time: Res<Time>) {
    let dt = time.delta_secs();
    for mut attack_state in &mut query {
        let done = if let Some(ref mut attack) = attack_state.active_attack {
            attack.elapsed += dt;
            attack.phase() == AttackPhase::Done
        } else {
            false
        };

        if done {
            attack_state.active_attack = None;
        }
    }
}

/// Sphere-overlap hit detection replacing cone test.
/// Computes fist world position during Active phase and checks distance to targets.
/// Emits AboutToBeHitMessage during Startup phase.
/// Supports directional attacks via direction_offset rotation.
pub fn hit_detection_system(
    mut attackers: Query<(Entity, &Transform, &Fighter, &mut AttackState)>,
    mut targets: Query<(Entity, &Transform, &mut Health, &mut BlockState, &Fighter)>,
    time: Res<Time>,
    mut damage_writer: MessageWriter<DamageMessage>,
    mut about_writer: MessageWriter<AboutToBeHitMessage>,
    mut reaction_writer: MessageWriter<HitReactionMessage>,
    mut block_writer: MessageWriter<BlockSuccessMessage>,
) {
    let now = time.elapsed_secs_f64();

    // Collect attacker data to avoid borrow conflicts
    let attacker_data: Vec<_> = attackers
        .iter()
        .map(|(e, tf, f, atk)| {
            let info = atk.active_attack.as_ref().map(|a| {
                (
                    a.phase(),
                    a.phase_f32(),
                    a.damage,
                    a.class,
                    a.strength,
                    a.hit_type,
                    a.hit_entities.clone(),
                    a.attack_start_phase,
                    a.damage_end_phase,
                    a.total_duration,
                    a.super_power_up,
                    a.direction_offset,
                )
            });
            (e, *tf, f.facing, info, 0, 0)
        })
        .collect();

    for (attacker_entity, attacker_tf, _facing, attack_info, _, _) in &attacker_data {
        let Some((
            phase,
            phase_f32,
            base_damage,
            class,
            strength,
            hit_type,
            hit_entities,
            attack_start_phase,
            _damage_end_phase,
            total_duration,
            _super_power_up,
            direction_offset,
        )) = attack_info
        else {
            continue;
        };

        let effective_hit_radius = HIT_RADIUS;

        // Compute fist local position based on phase, with range extension
        let fist_extended = FIST_EXTENDED + Vec3::new(0.0, 0.0, 0.0);

        let fist_local = match phase {
            AttackPhase::Startup => {
                let t = if *attack_start_phase > 0.0 {
                    phase_f32 / attack_start_phase
                } else {
                    1.0
                };
                FIST_REST.lerp(fist_extended, t)
            }
            AttackPhase::Active => fist_extended,
            AttackPhase::Recovery | AttackPhase::Done => continue,
        };

        // Apply direction_offset rotation around Y axis
        let rotated_fist = if *direction_offset != 0.0 {
            Quat::from_rotation_y(*direction_offset) * fist_local
        } else {
            fist_local
        };

        let fist_world = attacker_tf.translation + attacker_tf.rotation * rotated_fist;

        for (target_entity, target_tf, mut health, mut block_state, target_fighter) in &mut targets
        {
            if target_entity == *attacker_entity {
                continue;
            }

            let distance = fist_world.distance(target_tf.translation);

            // During Startup: emit about-to-be-hit warning for nearby targets
            if *phase == AttackPhase::Startup {
                if distance < effective_hit_radius + 2.5 {
                    let eta = (attack_start_phase - phase_f32) * total_duration;
                    about_writer.write(AboutToBeHitMessage {
                        target: target_entity,
                        eta,
                        hit_type: *hit_type,
                        from: attacker_tf.translation,
                        attacker: *attacker_entity,
                    });
                }
                continue;
            }

            // During Active: check sphere overlap
            if *phase != AttackPhase::Active {
                continue;
            }
            if hit_entities.contains(&target_entity) {
                continue;
            }
            if now < health.invulnerable_until {
                continue;
            }
            if distance > effective_hit_radius {
                continue;
            }

            // Enhanced block check
            let mut damage = *base_damage;
            let mut was_blocked = false;

            if block_state.is_blocking {
                let attack_dir = (attacker_tf.translation - target_tf.translation).normalize();
                let target_facing = target_fighter.facing.normalize();
                let dot = (-target_facing).dot(attack_dir);
                let angle = dot.clamp(-1.0, 1.0).acos();
                let within_arc = angle <= block_state.width_radians / 2.0;
                let can_block_type = block_state.can_block_hit_type(*hit_type);

                if within_arc && can_block_type {
                    damage *= block_state.damage_multiplier;
                    was_blocked = true;
                    block_state.hits_absorbed += 1;

                    if block_state.hits_absorbed >= block_state.combo_count_before_react {
                        reaction_writer.write(HitReactionMessage {
                            entity: target_entity,
                            kind: ReactionKind::GuardBreak,
                            direction: attack_dir,
                        });
                        block_state.hits_absorbed = 0;
                    }

                    if block_state.auto_counter {
                        block_writer.write(BlockSuccessMessage {
                            blocker: target_entity,
                            attacker: *attacker_entity,
                        });
                    }
                }
            }

            // Apply damage
            health.current = (health.current - damage).max(0.0);
            health.invulnerable_until = now + INVULN_DURATION;

            // Record hit on attacker's state
            if let Ok((_, _, _, mut atk_state)) = attackers.get_mut(*attacker_entity) {
                if let Some(ref mut attack) = atk_state.active_attack {
                    attack.hit_entities.push(target_entity);
                }
            }

            // Emit damage message
            damage_writer.write(DamageMessage {
                attacker: *attacker_entity,
                target: target_entity,
                damage,
                was_blocked,
                attack_class: *class,
                attack_strength: *strength,
            });

            // Emit hit reaction if not blocked
            if !was_blocked {
                let reaction_kind = match strength {
                    AttackStrength::Low => ReactionKind::Flinch,
                    AttackStrength::High => ReactionKind::Knockback,
                    AttackStrength::Super => ReactionKind::Knockdown,
                };
                let dir = (target_tf.translation - attacker_tf.translation).normalize();
                reaction_writer.write(HitReactionMessage {
                    entity: target_entity,
                    kind: reaction_kind,
                    direction: dir,
                });
            }
        }
    }
}

/// Fist visual system - updates fist mesh visibility, position, material, and scale pulse.
/// Supports directional attacks via direction_offset rotation and range extension.
pub fn fist_visual_system(
    fighters: Query<(&AttackState, &Children)>,
    mut fist_query: Query<
        (
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        With<FistVisual>,
    >,
    combat_materials: Res<CombatMaterials>,
) {
    for (attack_state, children) in &fighters {
        for child in children.iter() {
            let Ok((mut tf, mut vis, mut mat)) = fist_query.get_mut(child) else {
                continue;
            };

            let Some(ref attack) = attack_state.active_attack else {
                *vis = Visibility::Hidden;
                continue;
            };

            *vis = Visibility::Visible;
            let phase = attack.phase();
            let p = attack.phase_f32();

            // Weapon-adjusted extended position
            let fist_extended = FIST_EXTENDED + Vec3::new(0.0, 0.0, 0.0);

            let base_pos = match phase {
                AttackPhase::Startup => {
                    let t = if attack.attack_start_phase > 0.0 {
                        p / attack.attack_start_phase
                    } else {
                        1.0
                    };
                    tf.scale = Vec3::splat(1.0);
                    mat.0 = combat_materials.fist_startup.clone();
                    FIST_REST.lerp(fist_extended, t)
                }
                AttackPhase::Active => {
                    let active_range = attack.damage_end_phase - attack.attack_start_phase;
                    let active_progress = if active_range > 0.0 {
                        (p - attack.attack_start_phase) / active_range
                    } else {
                        0.0
                    };
                    let scale = if active_progress < 0.5 {
                        1.0 + 0.3 * (active_progress / 0.5)
                    } else {
                        1.3 - 0.3 * ((active_progress - 0.5) / 0.5)
                    };
                    tf.scale = Vec3::splat(scale);
                    mat.0 = combat_materials.fist_active.clone();
                    fist_extended
                }
                AttackPhase::Recovery => {
                    let recovery_range = 1.0 - attack.damage_end_phase;
                    let t = if recovery_range > 0.0 {
                        (p - attack.damage_end_phase) / recovery_range
                    } else {
                        1.0
                    };
                    tf.scale = Vec3::splat(1.0 - 0.2 * t);
                    mat.0 = combat_materials.fist_recovery.clone();
                    fist_extended.lerp(FIST_REST, t)
                }
                AttackPhase::Done => {
                    *vis = Visibility::Hidden;
                    continue;
                }
            };

            // Apply direction_offset rotation around Y
            if attack.direction_offset != 0.0 {
                tf.translation = Quat::from_rotation_y(attack.direction_offset) * base_pos;
            } else {
                tf.translation = base_pos;
            }
        }
    }
}

/// Shield visual system - shows/hides shield disc based on blocking state.
pub fn shield_visual_system(
    fighters: Query<(&BlockState, &Children)>,
    mut shield_query: Query<&mut Visibility, With<ShieldVisual>>,
) {
    for (block_state, children) in &fighters {
        for child in children.iter() {
            let Ok(mut vis) = shield_query.get_mut(child) else {
                continue;
            };
            *vis = if block_state.is_blocking {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        }
    }
}

/// About-to-be-hit system: reads messages and sets component on target.
/// Ticks down existing warnings and clears expired ones.
pub fn about_to_be_hit_system(
    mut reader: MessageReader<AboutToBeHitMessage>,
    mut query: Query<&mut AboutToBeHit>,
    time: Res<Time>,
) {
    // Tick down existing warnings
    for mut about in &mut query {
        if let Some(ref mut data) = about.active {
            data.eta -= time.delta_secs();
            if data.eta <= 0.0 {
                about.active = None;
            }
        }
    }

    // Apply new warnings
    for msg in reader.read() {
        if let Ok(mut about) = query.get_mut(msg.target) {
            about.active = Some(AboutToBeHitData {
                eta: msg.eta,
                hit_type: msg.hit_type,
                from: msg.from,
                attacker: msg.attacker,
            });
        }
    }
}

/// Grab input system: reads InputState grab flag and initiates grab.
pub fn grab_input_system(
    mut player_query: Query<
        (
            Entity,
            &Transform,
            &InputState,
            &AttackState,
            &mut GrabState,
            &HitReaction,
        ),
        With<Player>,
    >,
    targets: Query<(Entity, &Transform), (With<Fighter>, Without<Player>)>,
    mut grab_writer: MessageWriter<GrabMessage>,
) {
    for (player_entity, player_tf, input, attack_state, mut grab, reaction) in &mut player_query {
        if !input.grab {
            continue;
        }
        if attack_state.active_attack.is_some() || reaction.active.is_some() || grab.phase.is_some()
        {
            continue;
        }

        // Find closest target in range
        let mut closest: Option<(Entity, f32)> = None;
        for (target_entity, target_tf) in &targets {
            let dist = player_tf.translation.distance(target_tf.translation);
            if dist <= grab.grab_range {
                if closest.map_or(true, |(_, d)| dist < d) {
                    closest = Some((target_entity, dist));
                }
            }
        }

        if let Some((target, _)) = closest {
            grab.phase = Some(GrabPhase::Reaching);
            grab.target = Some(target);
            grab.hold_timer = 0.0;
            grab.shake_amount = 0.0;
            grab_writer.write(GrabMessage {
                attacker: player_entity,
                target,
            });
        }
    }
}

/// Grab system: manages grab lifecycle (Reaching -> Holding -> Throwing/Released).
pub fn grab_system(
    mut grabbers: Query<(Entity, &Transform, &mut GrabState)>,
    mut targets: Query<(&mut Transform, &mut Health), Without<GrabState>>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    mut damage_writer: MessageWriter<DamageMessage>,
    mut reaction_writer: MessageWriter<HitReactionMessage>,
) {
    let dt = time.delta_secs();

    for (grabber_entity, grabber_tf, mut grab) in &mut grabbers {
        let Some(phase) = grab.phase else {
            continue;
        };
        let Some(target_entity) = grab.target else {
            grab.phase = None;
            continue;
        };

        // Copy grabber translation to avoid borrow issues
        let grabber_pos = grabber_tf.translation;
        let grabber_forward = grabber_tf.forward().as_vec3();

        let Ok((mut target_tf, mut target_health)) = targets.get_mut(target_entity) else {
            grab.phase = None;
            grab.target = None;
            continue;
        };

        match phase {
            GrabPhase::Reaching => {
                let dist = grabber_pos.distance(target_tf.translation);
                if dist <= 1.2 {
                    grab.phase = Some(GrabPhase::Holding);
                    grab.hold_timer = 0.0;
                } else {
                    grab.hold_timer += dt;
                    if grab.hold_timer > 0.5 {
                        grab.phase = Some(GrabPhase::Released);
                    }
                }
            }
            GrabPhase::Holding => {
                // Lock target position near grabber
                let hold_pos = grabber_pos + grabber_forward * -1.0;
                target_tf.translation = target_tf.translation.lerp(hold_pos, 10.0 * dt);

                grab.hold_timer += dt;
                grab.shake_amount = (grab.hold_timer / GRAB_HOLD_MAX).clamp(0.0, 1.0);

                // Left click during hold -> throw
                if mouse.just_pressed(MouseButton::Left) {
                    grab.phase = Some(GrabPhase::Throwing);
                }
                // Timer expiry -> release
                if grab.hold_timer >= GRAB_HOLD_MAX {
                    grab.phase = Some(GrabPhase::Released);
                }
            }
            GrabPhase::Throwing => {
                let throw_dir = (target_tf.translation - grabber_pos).normalize();

                target_health.current = (target_health.current - GRAB_DAMAGE).max(0.0);

                damage_writer.write(DamageMessage {
                    attacker: grabber_entity,
                    target: target_entity,
                    damage: GRAB_DAMAGE,
                    was_blocked: false,
                    attack_class: AttackClass::Grab,
                    attack_strength: AttackStrength::High,
                });

                reaction_writer.write(HitReactionMessage {
                    entity: target_entity,
                    kind: ReactionKind::Knockback,
                    direction: throw_dir,
                });

                grab.phase = Some(GrabPhase::Released);
            }
            GrabPhase::Released => {
                grab.phase = None;
                grab.target = None;
                grab.hold_timer = 0.0;
                grab.shake_amount = 0.0;
            }
        }
    }
}

/// Hit reaction system: applies and ticks hit reactions using physics impulses for knockback.
pub fn hit_reaction_system(
    mut reader: MessageReader<HitReactionMessage>,
    mut query: Query<(&mut HitReaction, &mut LinearVelocity)>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();

    // Tick existing reactions
    for (mut reaction, _velocity) in &mut query {
        if let Some(ref mut active) = reaction.active {
            active.elapsed += dt;

            if active.elapsed >= active.duration {
                reaction.active = None;
            }
        }
    }

    // Apply new reactions via physics impulse
    for msg in reader.read() {
        if let Ok((mut reaction, mut velocity)) = query.get_mut(msg.entity) {
            reaction.active = Some(ActiveReaction::new(msg.kind, msg.direction));

            // Apply knockback as an immediate velocity change
            match msg.kind {
                ReactionKind::Knockback => {
                    let knockback_dir =
                        Vec3::new(msg.direction.x, 0.0, msg.direction.z).normalize_or_zero();
                    let impulse = knockback_dir * 8.0 + Vec3::Y * 2.0;
                    velocity.x += impulse.x;
                    velocity.y += impulse.y;
                    velocity.z += impulse.z;
                }
                ReactionKind::Knockdown => {
                    let knockback_dir =
                        Vec3::new(msg.direction.x, 0.0, msg.direction.z).normalize_or_zero();
                    let impulse = knockback_dir * 12.0 + Vec3::Y * 4.0;
                    velocity.x += impulse.x;
                    velocity.y += impulse.y;
                    velocity.z += impulse.z;
                }
                ReactionKind::Flinch => {
                    let knockback_dir =
                        Vec3::new(msg.direction.x, 0.0, msg.direction.z).normalize_or_zero();
                    let impulse = knockback_dir * 3.0;
                    velocity.x += impulse.x;
                    velocity.z += impulse.z;
                }
                ReactionKind::GuardBreak => {
                    let knockback_dir =
                        Vec3::new(msg.direction.x, 0.0, msg.direction.z).normalize_or_zero();
                    let impulse = knockback_dir * 5.0;
                    velocity.x += impulse.x;
                    velocity.z += impulse.z;
                }
            }
        }
    }
}

/// Super meter system: gains meter on hits and damage taken.
pub fn super_meter_system(
    mut reader: MessageReader<DamageMessage>,
    mut meters: Query<&mut SuperMeter>,
) {
    for msg in reader.read() {
        // Attacker gains meter on hit
        if let Ok(mut meter) = meters.get_mut(msg.attacker) {
            let gain = if msg.was_blocked { 2.5 } else { 5.0 };
            meter.current = (meter.current + gain).min(meter.max);
        }
        // Defender gains meter from taking damage
        if let Ok(mut meter) = meters.get_mut(msg.target) {
            let gain = msg.damage * 0.25;
            meter.current = (meter.current + gain).min(meter.max);
        }
    }
}

/// Increments combo counter on successive hits within the combo window.
pub fn combo_tracking_system(
    mut reader: MessageReader<DamageMessage>,
    mut combo_query: Query<&mut ComboTracker>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for msg in reader.read() {
        let Ok(mut combo) = combo_query.get_mut(msg.attacker) else {
            continue;
        };

        if now - combo.last_hit_time <= combo.combo_window {
            combo.hit_count += 1;
        } else {
            combo.hit_count = 1;
        }
        combo.last_hit_time = now;
    }
}

/// Checks for dead entities and emits DeathMessages.
pub fn death_system(
    query: Query<(Entity, &Health), Changed<Health>>,
    mut writer: MessageWriter<DeathMessage>,
) {
    for (entity, health) in &query {
        if health.current <= 0.0 {
            writer.write(DeathMessage {
                entity,
                killer: Entity::PLACEHOLDER,
            });
        }
    }
}

/// Sends combat events to the telemetry channel.
pub fn telemetry_combat_system(
    mut damage_reader: MessageReader<DamageMessage>,
    mut death_reader: MessageReader<DeathMessage>,
    fighter_ids: Query<(&FighterId, &Transform)>,
    combo_query: Query<&ComboTracker>,
    channel: Res<TelemetryChannel>,
) {
    for msg in damage_reader.read() {
        let attacker_id = fighter_ids
            .get(msg.attacker)
            .map(|(id, _)| id.0)
            .unwrap_or(uuid::Uuid::nil());
        let (target_id, pos) = fighter_ids
            .get(msg.target)
            .map(|(id, tf)| (id.0, tf.translation))
            .unwrap_or((uuid::Uuid::nil(), Vec3::ZERO));
        let combo_count = combo_query
            .get(msg.attacker)
            .map(|c| c.hit_count)
            .unwrap_or(0);

        let event = CombatEvent::damage(
            attacker_id,
            target_id,
            msg.damage,
            msg.was_blocked,
            combo_count,
            msg.attack_class.name(),
            [pos.x, pos.y, pos.z],
        );
        let _ = channel.sender.send(event);
    }

    for msg in death_reader.read() {
        let (target_id, pos) = fighter_ids
            .get(msg.entity)
            .map(|(id, tf)| (id.0, tf.translation))
            .unwrap_or((uuid::Uuid::nil(), Vec3::ZERO));
        let killer_id = fighter_ids
            .get(msg.killer)
            .map(|(id, _)| id.0)
            .unwrap_or(uuid::Uuid::nil());

        let event = CombatEvent::death(target_id, killer_id, [pos.x, pos.y, pos.z]);
        let _ = channel.sender.send(event);
    }
}

/// Updates Fighter.is_grounded based on ShapeCaster ground detection.
pub fn ground_detection_system(mut query: Query<(&mut Fighter, &ShapeHits)>) {
    for (mut fighter, hits) in &mut query {
        fighter.is_grounded = !hits.is_empty();
    }
}
