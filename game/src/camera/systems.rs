use bevy::prelude::*;
use bevy::input::mouse::AccumulatedMouseScroll;

use super::components::{ActiveCameraMode, CameraController, PrototypeElement, PrototypeVisible};
use super::channel::{CameraChannel, CameraBumpDirection};

/// Toggle camera mode with Tab key or F5 (FreeCam).
pub fn camera_mode_toggle_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut camera_query: Query<(&mut CameraController, &Transform)>,
) {
    if keyboard.just_pressed(KeyCode::Tab) {
        for (mut controller, _) in &mut camera_query {
            controller.active_mode = match controller.active_mode {
                ActiveCameraMode::GameTargeting => ActiveCameraMode::GameNavigation,
                ActiveCameraMode::GameNavigation => ActiveCameraMode::GameFighting, 
                ActiveCameraMode::GameFighting => ActiveCameraMode::GameNavigation,
                _ => controller.active_mode,
            };
        }
    }
    if keyboard.just_pressed(KeyCode::F5) {
        for (mut controller, cam_tf) in &mut camera_query {
            if controller.active_mode == ActiveCameraMode::DebugFreeCam {
                controller.active_mode = controller.pre_free_mode.unwrap_or(ActiveCameraMode::GameNavigation);
                controller.pre_free_mode = None;
            } else {
                let (yaw, pitch, _) = cam_tf.rotation.to_euler(EulerRot::YXZ);
                controller.free_yaw = yaw;
                controller.free_pitch = pitch;
                controller.pre_free_mode = Some(controller.active_mode);
                controller.active_mode = ActiveCameraMode::DebugFreeCam;
            }
        }
    }
}

/// Gathers input and target data to populate the `CameraChannel` blackboard for specific modes to evaluate.
pub fn update_camera_channel(
    mut camera_query: Query<(&CameraController, &mut CameraChannel)>,
    target_query: Query<
        (
            &Transform,
            &crate::combat::components::Fighter,
            Option<&crate::player::components::InputState>,
        ),
        With<crate::player::components::Player>,
    >,
    time: Res<Time>,
    scroll: Res<AccumulatedMouseScroll>,
    active_camera_package: Option<Res<crate::oni2_loader::environment::ActiveCameraPackage>>,
    camera_packages: Option<Res<crate::oni2_loader::environment::CameraPackages>>,
    camera_sets: Option<Res<crate::oni2_loader::environment::CameraParameterSets>>,
) {
    let dt = time.delta_secs();
    let scroll_y = scroll.delta.y;

    for (controller, mut channel) in &mut camera_query {
        
        let Ok((target_tf, fighter, input_opt)) = target_query.get(channel.focus_actor) else {
            continue;
        };

        // Populate Environmental Camera Packages
        if let (Some(active_pkg), Some(pkgs), Some(sets)) =
            (&active_camera_package, &camera_packages, &camera_sets)
        {
            if let Some(pkg) = pkgs.packages.get(&active_pkg.name) {
                let is_combat = controller.active_mode == ActiveCameraMode::GameFighting;
                
                let set_name = if is_combat && !pkg.fighting.is_empty() {
                    &pkg.fighting
                } else if !pkg.navigation.is_empty() {
                    &pkg.navigation
                } else {
                    ""
                };

                if let Some(params) = sets.sets.get(set_name) {
                    if channel.active_set_name != params.name {
                        info!("Camera selected parameter set: {}", params.name);
                        channel.active_set_name = params.name.clone();
                    }
                    
                    channel.package_fov = params.fov;
                    channel.package_distance = params.distance;
                    channel.package_incline_offset = params.incline_offset;
                    channel.package_spin_threshold = params.spin_threshold;
                    channel.package_zone_lerp_rates = [
                        params.lerp_rate_azimuth_zone1,
                        params.lerp_rate_azimuth_zone2,
                        params.lerp_rate_azimuth_zone3,
                        params.lerp_rate_azimuth_zone4,
                    ];
                    
                    if is_combat {
                        channel.package_inner_radius = params.inner_radius;
                        channel.package_outer_radius = params.outer_radius;
                    } else {
                        channel.package_inner_radius = params.dead_zone_inner_radius;
                        channel.package_outer_radius = params.dead_zone_outer_radius;
                    }
                }
            }
        }

        let target_pos = target_tf.translation;
        
        channel.previous_focus_pos = channel.current_focus_pos;
        channel.current_focus_pos = target_pos;
        
        channel.previous_focus_azimuth = channel.current_focus_azimuth;
        channel.current_focus_azimuth = fighter.facing.x.atan2(fighter.facing.z);
        
        channel.is_moving = input_opt.map_or(false, |i| i.movement.length_squared() > 0.01);
        
        // Accumulate running time natively
        if channel.is_moving {
            channel.time_running += dt;
        } else {
            channel.time_running = 0.0;
        }

        // Mouse wheel zoom defaults mapping natively over all standard modes
        if scroll_y.abs() > 0.01 {
            let zoom_speed = 2.0;
            let delta = -scroll_y * zoom_speed;
            channel.desired_distance = (channel.desired_distance + delta).clamp(3.0, 30.0);
            channel.desired_incline = (channel.desired_incline + delta * 0.05).clamp(0.0, 1.5);
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
