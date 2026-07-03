/*
 * combat/targeting.rs — targeting / reticle systems.
 *
 * Implements the runtime targeting sweeps, manual aiming updates, line-of-sight checks,
 * magnetic snap target evaluation, weapon aim synchronization, and camera channel updates.
 */
use bevy::prelude::*;
use bevy::ecs::relationship::Relationship;
use crate::combat::components::{TargetComponent, ReticleComponent, Health};
use crate::player::components::{Player, InputState};
use crate::camera::components::{ActiveCameraMode, CameraController};
use crate::camera::channel::CameraChannel;
use crate::weapons::components::{Weapon, AimTarget};
use crate::inventory::components::Inventory;
use crate::fight::components::FighterState;

/// Automatically switches the camera's mode between Navigation, Fighting, and Targeting
/// based on wielder draw state and aiming inputs (replaces manual tab-toggle for targeting).
pub fn camera_gameplay_mode_system(
    player_query: Query<(Entity, &Transform, &crate::animator::components::ActionPlayer, &InputState), With<Player>>,
    mut camera_query: Query<(&mut CameraController, &mut CameraChannel)>,
    fighters_query: Query<(Entity, &Transform), (With<FighterState>, Without<Player>, Without<CameraController>)>,
) {
    let Ok((_player_entity, player_tf, action_player, input_state)) = player_query.single() else {
        return;
    };
    let weapon_drawn = action_player.is_weapon_drawn();
    let targeting_modifier = input_state.targeting_modifier;

    let mut nearby_fighters = 0;
    let fight_mode_radius = 20.0;
    for (_ent, tf) in &fighters_query {
        if tf.translation.distance(player_tf.translation) < fight_mode_radius {
            nearby_fighters += 1;
        }
    }

    for (mut controller, mut channel) in &mut camera_query {
        if controller.active_mode == ActiveCameraMode::Script {
            continue;
        }
        if targeting_modifier && weapon_drawn {
            if controller.active_mode != ActiveCameraMode::GameTargeting {
                controller.active_mode = ActiveCameraMode::GameTargeting;
                channel.has_snapped_to_target = true; // snap polar settings immediately
            }
        } else if nearby_fighters > 0 {
            if controller.active_mode != ActiveCameraMode::GameFighting {
                controller.active_mode = ActiveCameraMode::GameFighting;
            }
        } else {
            if controller.active_mode != ActiveCameraMode::GameNavigation {
                controller.active_mode = ActiveCameraMode::GameNavigation;
            }
        }
    }
}

/// Runs the proximity checks, line-of-sight raycasts, stick/mouse manual aiming,
/// magnetism calculations, weapon aim updates, and camera channel feed.
pub fn reticle_update_system(
    time: Res<Time>,
    mut wielder_query: Query<(
        Entity,
        &Transform,
        &crate::animator::components::ActionPlayer,
        &InputState,
        &Inventory,
        &mut ReticleComponent,
    )>,
    mut camera_query: Query<(&CameraController, &mut CameraChannel)>,
    mut weapons_query: Query<&mut Weapon>,
    targets_query: Query<(Entity, &Transform, &TargetComponent, &Health)>,
    global_transforms: Query<&GlobalTransform>,
    anim_states: Query<&crate::oni2_loader::animation::Oni2AnimState>,
    parents: Query<&ChildOf>,
    spatial: avian3d::prelude::SpatialQuery,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    let mut camera_info = None;
    for (controller, channel) in &camera_query {
        camera_info = Some((controller.active_mode, channel.current_focus_pos));
    }
    let Some((cam_mode, _cam_focus_pos)) = camera_info else {
        return;
    };

    for (wielder_entity, wielder_tf, action_player, input_state, inventory, mut reticle) in &mut wielder_query {
        let weapon_drawn = action_player.is_weapon_drawn();
        let targeting_active = cam_mode == ActiveCameraMode::GameTargeting && weapon_drawn;
        reticle.is_active = targeting_active;

        // Reset/init muzzle position
        let mut muzzle_pos = wielder_tf.translation + wielder_tf.rotation * Vec3::new(0.0, 1.2, -0.5);
        if let Some(weapon_entity) = inventory.current_weapon_entity() {
            if let Ok(weapon) = weapons_query.get(weapon_entity) {
                if let Some(firing_mode) = weapon.firing_mode() {
                    if let Some(proj) = firing_mode.first_state.projectiles.first() {
                        let (_, weapon_rot, weapon_world_pos) = global_transforms
                            .get(weapon_entity)
                            .map(|gt| gt.to_scale_rotation_translation())
                            .unwrap_or((Vec3::ONE, wielder_tf.rotation, muzzle_pos));
                        muzzle_pos = weapon_world_pos + weapon_rot * proj.muzzle_offset;
                    }
                }
            }
        }
        reticle.muzzle_position = muzzle_pos;

        if !reticle.is_active {
            // Reticle is inactive - reset targeting state
            reticle.target_point = reticle.muzzle_position + wielder_tf.rotation * Vec3::new(0.0, 0.0, -reticle.max_lock_on_distance);
            reticle.target_locked_on = None;
            
            // Clear weapon aim target
            if let Some(weapon_entity) = inventory.current_weapon_entity() {
                if let Ok(mut weapon) = weapons_query.get_mut(weapon_entity) {
                    weapon.aim = AimTarget::None;
                }
            }
            continue;
        }

        // --- 1. Manual Targeting input processing ---
        if reticle.manual_targeting_enabled {
            let max_radians = reticle.max_angular_velocity * dt;
            let inp = input_state.reticle_input;
            if input_state.reticle_input_is_absolute {
                // Gamepad stick
                reticle.current_azimuth -= inp.x * max_radians;
                reticle.current_incline = (reticle.current_incline - inp.y * max_radians)
                    .clamp(reticle.min_angle_x, reticle.max_angle_x);
            } else {
                // Mouse delta
                reticle.current_azimuth -= inp.x;
                reticle.current_incline = (reticle.current_incline - inp.y)
                    .clamp(reticle.min_angle_x, reticle.max_angle_x);
            }
            reticle.current_azimuth = wrap_angle_neg_pi_to_pi(reticle.current_azimuth);
        }

        // --- 2. Target list sweep and line-of-sight filter ---
        let mut candidates = Vec::new();
        for (target_entity, target_tf, target_comp, target_health) in &targets_query {
            if target_entity == wielder_entity || target_health.current <= 0.0 {
                continue;
            }

            // Get target world position
            let target_pos = get_target_world_position_local(
                target_entity,
                target_tf.translation,
                &global_transforms,
                target_comp,
                &anim_states,
            );

            let dist = muzzle_pos.distance(target_pos);
            if dist > reticle.max_lock_on_distance {
                continue;
            }

            // Line of sight probe test
            let dir = (target_pos - muzzle_pos).normalize_or_zero();
            let filter = avian3d::prelude::SpatialQueryFilter::from_excluded_entities([wielder_entity]);
            let mut passed_probe = true;
            if let Some(hit) = spatial.cast_ray(
                muzzle_pos,
                Dir3::new(dir).unwrap_or(Dir3::NEG_Z),
                dist,
                true,
                &filter,
            ) {
                if !is_same_or_child_of(hit.entity, target_entity, &parents) {
                    passed_probe = false;
                }
            }

            if passed_probe {
                candidates.push((target_entity, target_pos, target_comp));
            }
        }

        // --- 3. Determine Locked-On Target ---
        let mut best_target: Option<(Entity, Vec3)> = None;

        // Apply magnetic snapping
        let collision_point = get_default_target_point_3d_local(
            reticle.muzzle_position,
            reticle.current_azimuth,
            reticle.current_incline,
            reticle.max_lock_on_distance,
            wielder_entity,
            &spatial,
        );

        if reticle.lock_on_enabled {
            let mut max_mag_factor = 0.0;
            
            for &(target_entity, target_pos, target_comp) in &candidates {
                let line_delta = collision_point - muzzle_pos;
                let target_delta = target_pos - muzzle_pos;
                let line_len_sq = line_delta.length_squared();
                
                if line_len_sq > 1e-4 {
                    let pct = (line_delta.dot(target_delta) / line_len_sq).clamp(0.0, 1.0);
                    let closest_point = muzzle_pos.lerp(collision_point, pct);
                    let dist_to_line = closest_point.distance(target_pos);
                    
                    let mag_radius = target_comp.magnet_radius;
                    if dist_to_line <= mag_radius {
                        let mag_factor = ((mag_radius - dist_to_line) / mag_radius) * target_comp.magnet_strength;
                        if mag_factor > max_mag_factor {
                            max_mag_factor = mag_factor;
                            best_target = Some((target_entity, target_pos));
                        }
                    }
                }
            }
        }

        if let Some((target_entity, target_pos)) = best_target {
            reticle.target_point = target_pos;
            reticle.target_locked_on = Some(target_entity);
            if reticle.target_locked_on != reticle.target_locked_on_last {
                reticle.new_lock_on = true;
            } else {
                reticle.new_lock_on = false;
            }
            reticle.target_locked_on_last = Some(target_entity);
            
            // Recalculate azimuth/incline toward lock-on target
            let source_to_target = target_pos - muzzle_pos;
            let flat_dist = Vec2::new(source_to_target.x, source_to_target.z).length();
            reticle.current_azimuth = -source_to_target.x.atan2(-source_to_target.z);
            reticle.current_incline = (target_pos.y - muzzle_pos.y).atan2(flat_dist);
        } else {
            reticle.target_point = collision_point;
            reticle.target_locked_on = None;
            reticle.target_locked_on_last = None;
        }

        // --- 4. Update Weapon aim target ---
        if let Some(weapon_entity) = inventory.current_weapon_entity() {
            if let Ok(mut weapon) = weapons_query.get_mut(weapon_entity) {
                if let Some(target_entity) = reticle.target_locked_on {
                    weapon.aim = AimTarget::Actor {
                        target: target_entity,
                        target_velocity: Vec3::ZERO,
                        bone_offset: Vec3::ZERO,
                    };
                } else {
                    weapon.aim = AimTarget::Point(reticle.target_point);
                }
            }
        }

        // --- 5. Update CameraChannel target variables ---
        for (_, mut channel) in &mut camera_query {
            channel.reticle_x = reticle.current_azimuth;
            channel.reticle_y = reticle.current_incline;
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn wrap_angle_neg_pi_to_pi(mut angle: f32) -> f32 {
    while angle > std::f32::consts::PI {
        angle -= 2.0 * std::f32::consts::PI;
    }
    while angle < -std::f32::consts::PI {
        angle += 2.0 * std::f32::consts::PI;
    }
    angle
}

fn get_target_world_position_local(
    target_entity: Entity,
    fallback_pos: Vec3,
    global_transforms: &Query<&GlobalTransform>,
    target_comp: &TargetComponent,
    anim_states: &Query<&crate::oni2_loader::animation::Oni2AnimState>,
) -> Vec3 {
    let offset = target_comp.target_offset;
    if let Some(bone_idx) = target_comp.bone_index {
        if let Ok(anim_state) = anim_states.get(target_entity) {
            if let Some(&joint_entity) = anim_state.joint_entities.get(bone_idx) {
                if let Ok(joint_gt) = global_transforms.get(joint_entity) {
                    return joint_gt.transform_point(offset);
                }
            }
        }
    }
    if let Ok(actor_gt) = global_transforms.get(target_entity) {
        return actor_gt.transform_point(offset);
    }
    fallback_pos
}

fn get_default_target_point_3d_local(
    muzzle_pos: Vec3,
    azimuth: f32,
    incline: f32,
    max_dist: f32,
    wielder_entity: Entity,
    spatial: &avian3d::prelude::SpatialQuery,
) -> Vec3 {
    let rotation = Quat::from_rotation_y(azimuth) * Quat::from_rotation_x(incline);
    let dir = rotation * Vec3::NEG_Z;
    let target_point = muzzle_pos + dir * max_dist;
    
    let filter = avian3d::prelude::SpatialQueryFilter::from_excluded_entities([wielder_entity]);
    if let Some(hit) = spatial.cast_ray(
        muzzle_pos,
        Dir3::new(dir).unwrap_or(Dir3::NEG_Z),
        max_dist,
        true,
        &filter,
    ) {
        muzzle_pos + dir * (hit.distance - 0.3).max(0.0)
    } else {
        target_point
    }
}

fn is_same_or_child_of(entity: Entity, potential_parent: Entity, parents: &Query<&ChildOf>) -> bool {
    let mut curr = entity;
    loop {
        if curr == potential_parent {
            return true;
        }
        if let Ok(parent) = parents.get(curr) {
            curr = parent.get();
        } else {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::App;
    use crate::combat::components::TargetComponent;
    use crate::oni2_loader::animation::Oni2AnimState;
    use bevy::ecs::system::SystemState;

    #[test]
    fn test_target_position_fallback() {
        let mut app = App::new();
        let target_ent = app.world_mut().spawn((
            Transform::from_xyz(10.0, 2.0, -5.0),
            GlobalTransform::from_xyz(10.0, 2.0, -5.0),
            TargetComponent {
                magnet_radius: 1.0,
                magnet_strength: 0.5,
                target_offset: Vec3::new(0.0, 1.0, 0.0),
                is_bump_targetable: true,
                parent_bone: None,
                bone_index: None,
            },
        )).id();

        let mut system_state: SystemState<(Query<&GlobalTransform>, Query<&Oni2AnimState>)> = SystemState::new(app.world_mut());
        let (gt_query, anim_states) = system_state.get(app.world());
        let target_comp = app.world().get::<TargetComponent>(target_ent).unwrap();

        let pos = get_target_world_position_local(
            target_ent,
            Vec3::ZERO,
            &gt_query,
            target_comp,
            &anim_states,
        );

        assert_eq!(pos, Vec3::new(10.0, 3.0, -5.0));
    }

    #[test]
    fn test_target_position_bone() {
        let mut app = App::new();
        
        let joint_ent = app.world_mut().spawn((
            Transform::from_xyz(10.0, 5.0, -5.0),
            GlobalTransform::from_xyz(10.0, 5.0, -5.0),
        )).id();

        let anim_state = Oni2AnimState {
            joint_entities: vec![Entity::PLACEHOLDER, Entity::PLACEHOLDER, joint_ent],
            is_grounded: true,
            ..default()
        };

        let target_ent = app.world_mut().spawn((
            Transform::from_xyz(10.0, 2.0, -5.0),
            GlobalTransform::from_xyz(10.0, 2.0, -5.0),
            TargetComponent {
                magnet_radius: 1.0,
                magnet_strength: 0.5,
                target_offset: Vec3::new(0.0, 1.0, 0.0),
                is_bump_targetable: true,
                parent_bone: Some("Head".to_string()),
                bone_index: Some(2),
            },
            anim_state,
        )).id();

        let mut system_state: SystemState<(Query<&GlobalTransform>, Query<&Oni2AnimState>)> = SystemState::new(app.world_mut());
        let (gt_query, anim_states) = system_state.get(app.world());
        let target_comp = app.world().get::<TargetComponent>(target_ent).unwrap();

        let pos = get_target_world_position_local(
            target_ent,
            Vec3::ZERO,
            &gt_query,
            target_comp,
            &anim_states,
        );

        assert_eq!(pos, Vec3::new(10.0, 6.0, -5.0));
    }
}
