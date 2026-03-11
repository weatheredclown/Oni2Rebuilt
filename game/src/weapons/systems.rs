use bevy::prelude::*;

use crate::combat::components::*;
use crate::combat::events::*;
use crate::player::components::{InputState, Player};

use super::components::*;

const PICKUP_RANGE: f32 = 2.0;
const PROJECTILE_HIT_RADIUS: f32 = 0.4;

/// F key picks up nearest WeaponPickup within range. Q key drops current weapon.
/// Swaps existing weapon if holding one.
pub fn weapon_pickup_system(
    mut fighters: Query<(
        Entity,
        &Transform,
        &InputState,
        &mut EquippedWeapon,
        &Children,
    ), With<Player>>,
    pickups: Query<(Entity, &Transform, &WeaponPickup)>,
    mut commands: Commands,
    weapon_materials: Res<WeaponMaterials>,
) {
    for (fighter_entity, fighter_tf, input, mut equipped, children) in &mut fighters {
        // Drop weapon on Q
        if input.drop_weapon && !equipped.is_fists() {
            let old_stats = equipped.stats.clone();
            // Spawn dropped weapon as pickup
            spawn_weapon_pickup(
                &mut commands,
                &weapon_materials,
                &old_stats,
                fighter_tf.translation + fighter_tf.forward().as_vec3() * -1.5,
            );
            // Reset to fists
            *equipped = EquippedWeapon::default();
            // Remove weapon visual child
            despawn_weapon_visual(&mut commands, children);
        }

        // Pickup weapon on F
        if input.pickup {
            let mut closest: Option<(Entity, f32, WeaponStats)> = None;
            for (pickup_entity, pickup_tf, pickup) in &pickups {
                let dist = fighter_tf.translation.distance(pickup_tf.translation);
                if dist <= PICKUP_RANGE {
                    if closest.as_ref().map_or(true, |(_, d, _)| dist < *d) {
                        closest = Some((pickup_entity, dist, pickup.stats.clone()));
                    }
                }
            }

            if let Some((pickup_entity, _, new_stats)) = closest {
                // If currently holding a weapon (not fists), drop it
                if !equipped.is_fists() {
                    let old_stats = equipped.stats.clone();
                    let pickup_tf = pickups.get(pickup_entity).unwrap().1;
                    spawn_weapon_pickup(
                        &mut commands,
                        &weapon_materials,
                        &old_stats,
                        pickup_tf.translation,
                    );
                }

                // Equip new weapon
                *equipped = EquippedWeapon::from_stats(new_stats.clone());

                // Despawn pickup entity
                commands.entity(pickup_entity).despawn();

                // Remove old weapon visual, spawn new one
                despawn_weapon_visual(&mut commands, children);
                spawn_weapon_visual(
                    &mut commands,
                    fighter_entity,
                    &weapon_materials,
                    &new_stats,
                );
            }
        }
    }
}

/// Ranged attack: when ranged weapon equipped and light_attack pressed,
/// check fire rate, decrement ammo, spawn projectile.
/// Heavy attack with ranged = melee bash (fist-level damage).
pub fn ranged_attack_system(
    mut fighters: Query<(
        Entity,
        &Transform,
        &Fighter,
        &InputState,
        &mut EquippedWeapon,
        &mut AttackState,
        &HitReaction,
        &GrabState,
    ), With<Player>>,
    time: Res<Time>,
    mut commands: Commands,
    weapon_materials: Res<WeaponMaterials>,
) {
    let now = time.elapsed_secs_f64();

    for (entity, tf, _fighter, input, mut weapon, attack_state, reaction, grab) in &mut fighters {
        if !weapon.is_ranged() {
            continue;
        }
        if reaction.active.is_some() || grab.phase.is_some() {
            continue;
        }

        // Auto-reload when ammo hits 0
        if weapon.ammo == 0 && weapon.reload_until <= 0.0 {
            weapon.reload_until = now + weapon.stats.reload_time as f64;
        }

        // Check if reloading
        if weapon.reload_until > 0.0 {
            if now >= weapon.reload_until {
                weapon.ammo = weapon.stats.max_ammo;
                weapon.reload_until = 0.0;
            } else {
                continue; // Still reloading
            }
        }

        if !input.light_attack {
            continue;
        }
        if attack_state.active_attack.is_some() {
            continue;
        }

        // Check fire rate
        let fire_interval = 1.0 / weapon.stats.fire_rate as f64;
        if now - weapon.last_fire_time < fire_interval {
            continue;
        }

        if weapon.ammo == 0 {
            continue;
        }

        // Fire projectile
        weapon.ammo -= 1;
        weapon.last_fire_time = now;

        let direction = tf.forward().as_vec3();
        let spawn_pos = tf.translation + direction * -0.8 + Vec3::Y * 0.3;

        commands.spawn((
            Mesh3d(weapon_materials.projectile_mesh.clone()),
            MeshMaterial3d(weapon_materials.projectile_mat.clone()),
            Transform::from_translation(spawn_pos).with_scale(Vec3::splat(0.5)),
            Projectile {
                owner: entity,
                damage: weapon.stats.projectile_damage,
                spawn_time: now,
                lifetime: weapon.stats.projectile_lifetime,
            },
            ProjectileVelocity(direction * weapon.stats.projectile_speed),
        ));
    }
}

/// Advances projectile positions manually. Hit detection against fighters (excluding owner).
/// Emits DamageMessage/HitReactionMessage on hit. Despawns on hit or lifetime expiry.
pub fn projectile_system(
    mut commands: Commands,
    mut projectiles: Query<(Entity, &mut Transform, &ProjectileVelocity, &Projectile)>,
    mut targets: Query<(Entity, &Transform, &mut Health, &mut BlockState, &Fighter), Without<Projectile>>,
    time: Res<Time>,
    mut damage_writer: MessageWriter<DamageMessage>,
    mut reaction_writer: MessageWriter<HitReactionMessage>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs_f64();

    for (proj_entity, mut proj_tf, proj_vel, projectile) in &mut projectiles {
        // Advance position
        proj_tf.translation += proj_vel.0 * dt;

        // Check lifetime
        if now - projectile.spawn_time > projectile.lifetime as f64 {
            commands.entity(proj_entity).despawn();
            continue;
        }

        // Hit detection against fighters
        let mut hit = false;
        for (target_entity, target_tf, mut health, mut block_state, target_fighter) in &mut targets {
            if target_entity == projectile.owner {
                continue;
            }
            if now < health.invulnerable_until {
                continue;
            }

            let distance = proj_tf.translation.distance(target_tf.translation);
            if distance > PROJECTILE_HIT_RADIUS + 0.6 {
                continue;
            }

            // Block check
            let mut damage = projectile.damage;
            let mut was_blocked = false;

            if block_state.is_blocking {
                let attack_dir = (proj_tf.translation - target_tf.translation).normalize();
                let target_facing = target_fighter.facing.normalize();
                let dot = (-target_facing).dot(attack_dir);
                let angle = dot.clamp(-1.0, 1.0).acos();
                let within_arc = angle <= block_state.width_radians / 2.0;
                // Ranged shot hit type = 9 (bit 9)
                let can_block_type = block_state.can_block_hit_type(9);

                if within_arc && can_block_type {
                    damage *= block_state.damage_multiplier;
                    was_blocked = true;
                    block_state.hits_absorbed += 1;
                }
            }

            health.current = (health.current - damage).max(0.0);
            health.invulnerable_until = now + 0.2;

            damage_writer.write(DamageMessage {
                attacker: projectile.owner,
                target: target_entity,
                damage,
                was_blocked,
                attack_class: AttackClass::RangedShot,
                attack_strength: AttackStrength::Low,
            });

            if !was_blocked {
                let dir = (target_tf.translation - proj_tf.translation).normalize();
                reaction_writer.write(HitReactionMessage {
                    entity: target_entity,
                    kind: ReactionKind::Flinch,
                    direction: dir,
                });
            }

            hit = true;
            break;
        }

        if hit {
            commands.entity(proj_entity).despawn();
        }
    }
}

/// Positions weapon mesh child: melee follows fist path, ranged at hip/aim position.
pub fn weapon_visual_system(
    fighters: Query<(&AttackState, &EquippedWeapon, &Children)>,
    mut weapon_query: Query<(&mut Transform, &mut Visibility), With<WeaponVisual>>,
) {
    let fist_rest = Vec3::new(0.3, 0.3, -0.5);
    let fist_extended = Vec3::new(0.3, 0.3, -2.0);
    let hip_pos = Vec3::new(0.4, -0.1, -0.3);

    for (attack_state, weapon, children) in &fighters {
        if weapon.is_fists() {
            // Hide weapon visual when using fists
            for child in children.iter() {
                if let Ok((_, mut vis)) = weapon_query.get_mut(child) {
                    *vis = Visibility::Hidden;
                }
            }
            continue;
        }

        for child in children.iter() {
            let Ok((mut tf, mut vis)) = weapon_query.get_mut(child) else {
                continue;
            };

            *vis = Visibility::Visible;

            if weapon.stats.kind == WeaponKind::Melee {
                // Melee: follow fist interpolation path
                if let Some(ref attack) = attack_state.active_attack {
                    let phase = attack.phase();
                    let p = attack.phase_f32();

                    match phase {
                        AttackPhase::Startup => {
                            let t = if attack.attack_start_phase > 0.0 {
                                p / attack.attack_start_phase
                            } else {
                                1.0
                            };
                            tf.translation = fist_rest.lerp(fist_extended, t);
                        }
                        AttackPhase::Active => {
                            tf.translation = fist_extended;
                        }
                        AttackPhase::Recovery => {
                            let recovery_range = 1.0 - attack.damage_end_phase;
                            let t = if recovery_range > 0.0 {
                                (p - attack.damage_end_phase) / recovery_range
                            } else {
                                1.0
                            };
                            tf.translation = fist_extended.lerp(fist_rest, t);
                        }
                        AttackPhase::Done => {
                            tf.translation = fist_rest;
                        }
                    }
                } else {
                    tf.translation = fist_rest;
                }
            } else {
                // Ranged: hip position, aim forward during attack
                if attack_state.active_attack.is_some() {
                    let aim_pos = Vec3::new(0.3, 0.3, -0.8);
                    tf.translation = aim_pos;
                } else {
                    tf.translation = hip_pos;
                }
            }
        }
    }
}

/// Sine-wave bob + slow Y rotation on world pickups.
pub fn weapon_pickup_bob_system(
    mut pickups: Query<(&mut Transform, &WeaponPickup)>,
    time: Res<Time>,
) {
    let t = time.elapsed_secs();
    for (mut tf, pickup) in &mut pickups {
        tf.translation.y = pickup.base_y + (t * 2.0).sin() * 0.15;
        tf.rotate_y(time.delta_secs() * 1.0);
    }
}

// === Helper Functions ===

fn despawn_weapon_visual(commands: &mut Commands, children: &Children) {
    // We can't query WeaponVisual here directly, so we'll use a command-based approach
    // The weapon_visual_cleanup_system handles this via marker
    for child in children.iter() {
        // We mark for despawn — the visual system will handle cleanup
        // Actually, let's just try despawning all WeaponVisual children
        commands.entity(child).try_despawn();
    }
}

fn spawn_weapon_visual(
    commands: &mut Commands,
    parent: Entity,
    materials: &WeaponMaterials,
    stats: &WeaponStats,
) {
    let (mesh, mat) = match stats.id {
        WeaponId::Pipe => (materials.pipe_mesh.clone(), materials.pipe_mat.clone()),
        WeaponId::Sword => (materials.sword_mesh.clone(), materials.sword_mat.clone()),
        WeaponId::Pistol => (materials.pistol_mesh.clone(), materials.pistol_mat.clone()),
        WeaponId::Rifle => (materials.rifle_mesh.clone(), materials.rifle_mat.clone()),
        WeaponId::Fists => return,
    };

    let child = commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(Vec3::new(0.3, 0.3, -0.5)),
        WeaponVisual,
    )).id();

    commands.entity(parent).add_child(child);
}

pub fn spawn_weapon_pickup(
    commands: &mut Commands,
    materials: &WeaponMaterials,
    stats: &WeaponStats,
    position: Vec3,
) {
    let (mesh, mat) = match stats.id {
        WeaponId::Pipe => (materials.pipe_mesh.clone(), materials.pipe_mat.clone()),
        WeaponId::Sword => (materials.sword_mesh.clone(), materials.sword_mat.clone()),
        WeaponId::Pistol => (materials.pistol_mesh.clone(), materials.pistol_mat.clone()),
        WeaponId::Rifle => (materials.rifle_mesh.clone(), materials.rifle_mat.clone()),
        WeaponId::Fists => return,
    };

    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_translation(position),
        WeaponPickup {
            stats: stats.clone(),
            base_y: position.y,
        },
    ));
}
