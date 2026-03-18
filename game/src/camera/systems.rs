use bevy::prelude::*;

use bevy::input::mouse::AccumulatedMouseScroll;

use super::components::{CameraMode, CameraRig, PrototypeElement, PrototypeVisible};

/// Toggle camera mode with Tab key (MouseLook/SmartFollow) or F5 (FreeCam).
pub fn camera_mode_toggle_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut camera_query: Query<(&mut CameraRig, &Transform)>,
) {
    if keyboard.just_pressed(KeyCode::Tab) {
        for (mut rig, _) in &mut camera_query {
            rig.mode = match rig.mode {
                CameraMode::MouseLook => CameraMode::SmartFollow,
                CameraMode::SmartFollow => CameraMode::MouseLook,
                CameraMode::FreeCam => CameraMode::FreeCam, // Tab does nothing in free cam
            };
        }
    }
    if keyboard.just_pressed(KeyCode::F5) {
        for (mut rig, cam_tf) in &mut camera_query {
            if rig.mode == CameraMode::FreeCam {
                rig.mode = rig.pre_free_mode.unwrap_or(CameraMode::MouseLook);
                rig.pre_free_mode = None;
            } else {
                // Capture current camera orientation so view doesn't jump
                let (yaw, pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);
                rig.free_yaw = yaw;
                rig.free_pitch = pitch;
                rig.pre_free_mode = Some(rig.mode);
                rig.mode = CameraMode::FreeCam;
            }
        }
    }
}

/// Camera follow system that dispatches to mouse-look, smart-follow, or free-cam based on mode.
pub fn camera_follow_system(
    mut camera_query: Query<
        (&mut CameraRig, &mut Transform),
        Without<crate::player::components::Player>,
    >,
    target_query: Query<
        (&Transform, &crate::combat::components::Fighter),
        With<crate::player::components::Player>,
    >,
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    accumulated_motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    active_camera_package: Option<Res<crate::oni2_loader::environment::ActiveCameraPackage>>,
    camera_packages: Option<Res<crate::oni2_loader::environment::CameraPackages>>,
    camera_sets: Option<Res<crate::oni2_loader::environment::CameraParameterSets>>,
    enemies_query: Query<(&Transform, &crate::combat::components::Fighter), (Without<crate::player::components::Player>, Without<CameraRig>)>,
) {
    let dt = time.delta_secs();

    // Mouse wheel zoom: adjust follow_distance and offset distance
    let scroll_y = scroll.delta.y;

    for (mut rig, mut cam_tf) in &mut camera_query {
        if scroll_y.abs() > 0.01 {
            let zoom_speed = 2.0;
            let delta = -scroll_y * zoom_speed;
            rig.follow_distance = (rig.follow_distance + delta).clamp(3.0, 30.0);
            rig.height = (rig.height + delta * 0.5).clamp(2.0, 20.0);
            // Also adjust mouse-look offset proportionally
            let ratio = rig.follow_distance / (rig.follow_distance - delta);
            rig.offset *= ratio;
            rig.offset.y = rig.offset.y.clamp(2.0, 20.0);
        }
        // Free cam doesn't need a target
        if rig.mode == CameraMode::FreeCam {
            // Mouse look (hold right mouse button)
            if mouse_button.pressed(MouseButton::Right) {
                let sensitivity = 0.003;
                let delta = accumulated_motion.delta;
                rig.free_yaw -= delta.x * sensitivity;
                rig.free_pitch = (rig.free_pitch - delta.y * sensitivity).clamp(-1.4, 1.4);
            }

            let speed = rig.free_speed;

            let forward = Vec3::new(rig.free_yaw.sin(), 0.0, rig.free_yaw.cos()).normalize();
            let right = Vec3::new(-rig.free_yaw.cos(), 0.0, rig.free_yaw.sin()).normalize();
            let mut velocity = Vec3::ZERO;

            if keyboard.pressed(KeyCode::KeyW) {
                velocity -= forward;
            }
            if keyboard.pressed(KeyCode::KeyS) {
                velocity += forward;
            }
            if keyboard.pressed(KeyCode::KeyA) {
                velocity += right;
            }
            if keyboard.pressed(KeyCode::KeyD) {
                velocity -= right;
            }
            if keyboard.pressed(KeyCode::ShiftLeft) {
                velocity += Vec3::Y;
            }
            if keyboard.pressed(KeyCode::ControlLeft) {
                velocity -= Vec3::Y;
            }

            if velocity.length_squared() > 0.0 {
                velocity = velocity.normalize() * speed * dt;
                cam_tf.translation += velocity;
            }

            cam_tf.rotation =
                Quat::from_rotation_y(rig.free_yaw) * Quat::from_rotation_x(rig.free_pitch);
            continue;
        }

        let Ok((target_tf, fighter)) = target_query.get(rig.target) else {
            continue;
        };

        let target_pos = target_tf.translation;

        match rig.mode {
            CameraMode::MouseLook => {
                // Original simple follow: rotate offset by target rotation, lerp to position
                let target_rotation = target_tf.rotation;
                let rotated_offset = target_rotation * rig.offset;
                let desired_pos = target_pos + rotated_offset;

                let t = (rig.mouse_lerp_speed * dt).clamp(0.0, 1.0);
                cam_tf.translation = cam_tf.translation.lerp(desired_pos, t);
                cam_tf.look_at(target_pos + Vec3::Y * 1.0, Vec3::Y);
            }
            CameraMode::SmartFollow => {
                // Check camera packages and parameters to determine state and params
                let mut fov = 50.0;
                let mut follow_distance = rig.follow_distance;
                let mut height_offset = rig.height;
                let mut inner_dz = rig.dead_zone_inner;
                let mut outer_dz = rig.dead_zone_outer;
                let mut spin_thresh = rig.spin_threshold;
                let mut z_lerp_rates = rig.zone_lerp_rates;

                if let (Some(active_pkg), Some(pkgs), Some(sets)) = (&active_camera_package, &camera_packages, &camera_sets) {
                    if let Some(pkg) = pkgs.packages.get(&active_pkg.name) {
                        // Check if we should be in Fight mode
                        let mut in_fight = false;
                        for (enemy_tf, _enemy_fighter) in &enemies_query {
                            if enemy_tf.translation.distance(target_pos) <= pkg.fight_mode_radius {
                                in_fight = true;
                                break;
                            }
                        }

                        let set_name = if in_fight && !pkg.fighting.is_empty() {
                            &pkg.fighting
                        } else if !pkg.navigation.is_empty() {
                            &pkg.navigation
                        } else {
                            ""
                        };

                        if let Some(params) = sets.sets.get(set_name) {
                            fov = params.fov;
                            follow_distance = params.distance;
                            inner_dz = if in_fight { params.inner_radius } else { params.dead_zone_inner_radius };
                            outer_dz = if in_fight { params.outer_radius } else { params.dead_zone_outer_radius };
                            spin_thresh = params.spin_threshold;
                            
                            if !in_fight {
                                z_lerp_rates = [
                                    params.lerp_rate_azimuth_zone1,
                                    params.lerp_rate_azimuth_zone2,
                                    params.lerp_rate_azimuth_zone3,
                                    params.lerp_rate_azimuth_zone4,
                                ];
                            }
                        }
                    }
                }

                // Manual arrow key overrides
                let mut manual_yaw = 0.0;
                let manual_turn_speed = 3.0;
                
                if keyboard.pressed(KeyCode::ArrowLeft) {
                    manual_yaw += manual_turn_speed * dt;
                }
                if keyboard.pressed(KeyCode::ArrowRight) {
                    manual_yaw -= manual_turn_speed * dt;
                }
                
                let manual_pitch_speed = 5.0;
                if keyboard.pressed(KeyCode::ArrowUp) {
                    rig.height -= manual_pitch_speed * dt;
                }
                if keyboard.pressed(KeyCode::ArrowDown) {
                    rig.height += manual_pitch_speed * dt;
                }
                rig.height = rig.height.clamp(1.0, 30.0);
                height_offset = rig.height;

                if manual_yaw != 0.0 {
                    rig.current_azimuth += manual_yaw;
                    rig.target_azimuth = rig.current_azimuth;
                } else {
                    // Zone-based auto-follow camera (from rb's camnewFollow)
                    let facing = fighter.facing;
                    let new_target_azimuth = facing.x.atan2(facing.z);

                    // Compute heading difference, normalized to [-PI, PI]
                    let mut heading_diff = new_target_azimuth - rig.current_azimuth;
                    while heading_diff > std::f32::consts::PI {
                        heading_diff -= 2.0 * std::f32::consts::PI;
                    }
                    while heading_diff < -std::f32::consts::PI {
                        heading_diff += 2.0 * std::f32::consts::PI;
                    }

                    let abs_diff = heading_diff.abs();

                    // Determine zone lerp rate
                    let lerp_rate = if abs_diff < rig.zone_thresholds[0] {
                        rig.zone_lerp_rates[0]
                    } else if abs_diff < rig.zone_thresholds[1] {
                        rig.zone_lerp_rates[1]
                    } else if abs_diff < rig.zone_thresholds[2] {
                        rig.zone_lerp_rates[2]
                    } else {
                        rig.zone_lerp_rates[3]
                    };

                    // Dead zone: compute horizontal distance from current camera XZ to target XZ
                    let cam_xz = Vec3::new(cam_tf.translation.x, 0.0, cam_tf.translation.z);
                    let target_xz = Vec3::new(target_pos.x, 0.0, target_pos.z);
                    let focus_dist = cam_xz.distance(target_xz);

                    // Only update azimuth if outside dead zone
                    if focus_dist > inner_dz {
                        rig.target_azimuth = new_target_azimuth;
                        let t = (lerp_rate * dt).clamp(0.0, 1.0);
                        rig.current_azimuth += heading_diff * t;

                        // Spin threshold: snap on sharp turn
                        if abs_diff > spin_thresh && spin_thresh > 0.0 {
                            rig.current_azimuth +=
                                heading_diff * (lerp_rate * 2.0 * dt).clamp(0.0, 1.0);
                        }
                    }
                }

                // Decay bump angle
                rig.bump_angle *= (1.0 - rig.bump_lerp_rate * dt).max(0.0);

                let total_azimuth = rig.current_azimuth + rig.bump_angle;

                // Compute final camera position from azimuth + follow_distance + height
                let behind_x = total_azimuth.sin() * follow_distance;
                let behind_z = total_azimuth.cos() * follow_distance;
                let desired_pos = Vec3::new(
                    target_pos.x + behind_x,
                    target_pos.y + height_offset,
                    target_pos.z + behind_z,
                );

                let cam_t = (3.0 * dt).clamp(0.0, 1.0);
                cam_tf.translation = cam_tf.translation.lerp(desired_pos, cam_t);
                cam_tf.look_at(target_pos + Vec3::Y * 1.0, Vec3::Y);
            }
            CameraMode::FreeCam => unreachable!(),
        }
    }
}

/// Toggle visibility of prototype elements (capsules, combat markers, HUD) with F6.
pub fn prototype_toggle_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut visible: ResMut<PrototypeVisible>,
    mut proto_query: Query<&mut Visibility, With<PrototypeElement>>,
    mut hud_query: Query<
        &mut Visibility,
        (
            With<Node>,
            With<crate::menu::InGameEntity>,
            Without<PrototypeElement>,
        ),
    >,
) {
    if keyboard.just_pressed(KeyCode::F6) {
        visible.0 = !visible.0;
        let vis = if visible.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        for mut v in &mut proto_query {
            *v = vis;
        }
        for mut v in &mut hud_query {
            *v = vis;
        }
    }
}
