/*
 * camera/systems.rs — shared camera systems.
 *
 * camera_mode_toggle_system: Tab cycles Navigation ↔ Fighting; F5 spawns /
 * despawns the debug free camera.
 * update_camera_channel: gathers player position, facing, input, scroll, and
 * active camera package parameters into CameraChannel each frame.
 * prototype_toggle_system: F2 shows/hides prototype overlay and HUD.
 * debug_render_camera_targets: draws gizmo spheres for focus/lens positions.
 */
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy::render::view::ColorGrading;

use super::channel::CameraChannel;
use super::components::{ActiveCameraMode, CameraController, PrototypeElement, PrototypeVisible};

/// Ticks down a pending mode transition started by ScrOni
/// `cameraMode <mode> time <secs>`.  When the timer expires, commits
/// `next_mode` as `active_mode`.  Legacy behavior effectively forces
/// transitionTime to 0 for CameraMode — we honor the
/// requested duration as a delayed-switch (the old mode keeps running
/// until the timer elapses) rather than a cross-blend, which matches
/// intent without needing a real mode-mixer.
pub fn camera_mode_transition_tick(time: Res<Time>, mut q: Query<&mut CameraController>) {
    let dt = time.delta_secs();
    for mut c in &mut q {
        if c.next_mode.is_some() && c.transition_time_remaining > 0.0 {
            c.transition_time_remaining -= dt;
            if c.transition_time_remaining <= 0.0 {
                if let Some(next) = c.next_mode.take() {
                    c.active_mode = next;
                }
                c.transition_time = 0.0;
                c.transition_time_remaining = 0.0;
            }
        }
    }
}

/// Toggle camera mode with Tab key or F5 (FreeCam).
pub fn camera_mode_toggle_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut main_cam_query: Query<(&mut Camera, &Transform, &mut CameraController)>,
    mut debug_cam_query: Query<
        (
            Entity,
            &mut Camera,
            &mut crate::camera::components::DebugFreeCamera,
        ),
        Without<CameraController>,
    >,
) {
    if keyboard.just_pressed(KeyCode::Tab) {
        for (_, _, mut controller) in &mut main_cam_query {
            controller.active_mode = match controller.active_mode {
                ActiveCameraMode::GameTargeting => ActiveCameraMode::GameNavigation,
                ActiveCameraMode::GameNavigation => ActiveCameraMode::GameFighting,
                ActiveCameraMode::GameFighting => ActiveCameraMode::GameNavigation,
                _ => controller.active_mode,
            };
        }
    }
    if keyboard.just_pressed(KeyCode::F5)
        && let Ok((mut main_cam, main_tf, _)) = main_cam_query.single_mut()
    {
        if let Ok((debug_ent, _, _)) = debug_cam_query.single_mut() {
            // Return to main camera
            commands.entity(debug_ent).despawn();
            main_cam.is_active = true;
        } else {
            // Switch to debug camera
            main_cam.is_active = false;
            let (yaw, pitch, _) = main_tf.rotation.to_euler(EulerRot::YXZ);
            commands.spawn((
                Camera3d::default(),
                bevy::post_process::motion_blur::MotionBlur {
                    shutter_angle: 1.0,
                    samples: 4,
                },
                Transform::from_translation(main_tf.translation).with_rotation(main_tf.rotation),
                crate::camera::components::DebugFreeCamera {
                    yaw,
                    pitch,
                    speed: 10.0,
                },
            ));
            main_cam.is_active = false;
        }
    }
}

/// Debug render the main camera's target and position using gizmos
pub fn debug_render_camera_targets(
    camera_query: Query<(&CameraChannel, &Transform), With<CameraController>>,
    debug_cam_query: Query<(), With<crate::camera::components::DebugFreeCamera>>,
    mut gizmos: Gizmos,
) {
    if debug_cam_query.is_empty() {
        return;
    }

    for (channel, transform) in &camera_query {
        // Render where the camera is looking (yellow sphere)
        gizmos.sphere(channel.current_focus_pos, 0.2, Color::srgb(1.0, 1.0, 0.0));

        // Render where the game thinks the camera's actual lens is (cyan sphere with axis)
        gizmos.sphere(transform.translation, 0.2, Color::srgb(0.0, 1.0, 1.0));
        gizmos.ray(
            transform.translation,
            transform.forward() * 1.0,
            Color::srgb(0.0, 1.0, 1.0),
        );
    }
}

/// Gathers input and target data to populate the `CameraChannel` blackboard
/// for the mode evaluators.  Mirrors `camnewCamera::UpdateChannel`
/// (camera_new/camera.cpp:290-337) plus the "current polar from the camera
/// matrix" step every rb game camera does first (fight.cpp:105-108,
/// follow.cpp:152-155): the current azimuth/incline/distance are re-read from
/// where the camera actually ended up last frame, so collision moves carry over.
#[allow(clippy::too_many_arguments)]
pub fn update_camera_channel(
    mut camera_query: Query<(&CameraController, &mut CameraChannel, &Transform)>,
    target_query: Query<
        (
            &Transform,
            &crate::combat::components::Fighter,
            Option<&crate::player::components::InputState>,
        ),
        With<crate::player::components::Player>,
    >,
    enemy_query: Query<
        (&Transform, Option<&crate::combat::components::Health>),
        (
            With<crate::fight::components::FighterState>,
            Without<crate::player::components::Player>,
            Without<CameraController>,
        ),
    >,
    time: Res<Time>,
    scroll: Res<AccumulatedMouseScroll>,
    keyboard: Res<ButtonInput<KeyCode>>,
    active_camera_package: Option<Res<crate::oni2_loader::environment::ActiveCameraPackage>>,
    camera_packages: Option<Res<crate::oni2_loader::environment::CameraPackages>>,
    camera_sets: Option<Res<crate::oni2_loader::environment::CameraParameterSets>>,
) {
    let dt = time.delta_secs();
    let scroll_y = scroll.delta.y;

    for (controller, mut channel, cam_tf) in &mut camera_query {
        let Ok((target_tf, fighter, input_opt)) = target_query.get(channel.focus_actor) else {
            continue;
        };
        let first_frame = !channel.has_snapped_to_target;

        // Populate Environmental Camera Packages
        if let (Some(active_pkg), Some(pkgs), Some(sets)) =
            (&active_camera_package, &camera_packages, &camera_sets)
            && let Some(pkg) = pkgs.packages.get(&active_pkg.name)
        {
            let is_combat = controller.active_mode == ActiveCameraMode::GameFighting;
            channel.package_fight_mode_radius = pkg.fight_mode_radius;
            channel.package_fight_mode_running_away_time = pkg.fight_mode_running_away_time;

            // camnewPackage::GetCamera(state): one camera per game state.
            let set_name = match controller.active_mode {
                ActiveCameraMode::GameFighting if !pkg.fighting.is_empty() => &pkg.fighting,
                ActiveCameraMode::GameTargeting if !pkg.targeting.is_empty() => &pkg.targeting,
                _ if !pkg.navigation.is_empty() => &pkg.navigation,
                _ => "",
            };

            if let Some(params) = sets.sets.get(set_name) {
                if channel.active_set_name != params.name {
                    info!("Camera selected parameter set: {}", params.name);
                    channel.active_set_name = params.name.clone();
                    channel.params = params.clone();
                }

                channel.package_fov = params.fov;
                channel.package_distance = params.distance;
                channel.package_incline_offset = params.incline_offset;
                channel.package_incline_offset_running = params.incline_offset_running;
                channel.package_focus_offset = Vec3::from_array(params.focus_offset);
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

        let target_pos = target_tf.translation;

        // rb rbGameState fighter array + local count (rbgame/gamestate.cpp:220-230).
        let radius = channel.package_fight_mode_radius;
        channel.fighters = enemy_query
            .iter()
            .filter(|(_, h)| h.is_none_or(|h| h.current > 0.0))
            .map(|(tf, _)| tf.translation)
            .collect();
        channel.num_local_fighters = channel
            .fighters
            .iter()
            .filter(|p| p.distance(target_pos) < radius)
            .count();

        // Camera stick (rb tgtReticleComponent::GetFrameInput) from the arrow
        // keys, dead-zoned by AnalogX/YStart (polar.cpp:224-225).
        let axis = |neg: KeyCode, pos: KeyCode| {
            (keyboard.pressed(pos) as i32 - keyboard.pressed(neg) as i32) as f32
        };
        let mut stick = Vec2::new(
            axis(KeyCode::ArrowLeft, KeyCode::ArrowRight),
            axis(KeyCode::ArrowUp, KeyCode::ArrowDown),
        );
        if stick.x.abs() < channel.params.analog_x_start {
            stick.x = 0.0;
        }
        if stick.y.abs() < channel.params.analog_y_start {
            stick.y = 0.0;
        }
        channel.cam_stick = stick;

        // Z-lock comes from the fight-mode button every frame (behavior/pad.cpp:978);
        // no Rust input drives it yet, so only a ForceZLock camera sets it.
        channel.is_zlock_targeting = false;

        channel.is_moving = input_opt.is_some_and(|i| i.movement.length_squared() > 0.01);
        if channel.is_moving {
            channel.time_running += dt;
        } else {
            channel.time_running = 0.0;
        }

        // Focus: offset lerps toward the collision offset while colliding
        // (rbpolar.cpp:340-352), the focus point toward root + offset.
        channel.previous_focus_pos = channel.current_focus_pos;
        channel.current_focus_pos = target_pos;
        channel.desired_focus_offset = Vec3::from_array(if channel.is_colliding {
            channel.params.collision_focus_offset
        } else {
            channel.params.focus_offset
        });
        // rb's Vector3::Lerp(dt * rate) is unclamped; at 60 Hz it never
        // exceeds 1, but Rust frames can be long, so clamp to stay a lerp.
        let t = (dt * channel.params.lerp_rate_focus_offset).min(1.0);
        channel.current_focus_offset = channel
            .current_focus_offset
            .lerp(channel.desired_focus_offset, t);

        channel.previous_focus_azimuth = channel.current_focus_azimuth;
        // fighter.facing points toward the model's visual front (+Z in Oni2 convention).
        // The camera must sit BEHIND that direction, so we negate before atan2.
        channel.current_focus_azimuth = (-fighter.facing.x).atan2(-fighter.facing.z);

        channel.previous_focus_point = channel.current_focus_point;
        channel.desired_focus_pos = target_pos + channel.current_focus_offset;
        if channel.is_jumping {
            channel.time_since_jumping = 0.0;
        } else {
            channel.time_since_jumping += dt;
        }
        let t = (dt * channel.params.lerp_rate_focus_point).min(1.0);
        channel.current_focus_point = channel
            .current_focus_point
            .lerp(channel.desired_focus_pos, t);

        if first_frame {
            // camnewCamera::Update first frame: Reset + teleport (camera.cpp:240-252).
            let focus_offset = Vec3::from_array(channel.params.focus_offset);
            channel.is_colliding = false;
            channel.time_since_jumping = 0.0;
            channel.desired_focus_offset = focus_offset;
            channel.current_focus_offset = focus_offset;
            channel.desired_focus_pos = target_pos + focus_offset;
            channel.current_focus_point = channel.desired_focus_pos;
            channel.previous_focus_point = channel.current_focus_point;
            channel.previous_focus_pos = target_pos;
            channel.previous_focus_azimuth = channel.current_focus_azimuth;
            channel.dead_zone_center = target_pos;
            channel.desired_azimuth = channel.current_focus_azimuth;
            channel.desired_distance = channel.params.distance;
            channel.desired_incline = channel.params.incline_offset;
            channel.desired_fov = channel.params.fov;
            channel.current_azimuth = channel.desired_azimuth;
            channel.current_distance = channel.desired_distance;
            channel.current_incline = channel.desired_incline;
            channel.current_fov = channel.desired_fov;
        } else if channel.script_override_transform.is_none() {
            let cam = cam_tf.translation;
            let back = cam_tf.back();
            let flat = channel.previous_focus_point.xz().distance(cam.xz());
            channel.current_distance = channel.current_focus_point.distance(cam);
            channel.current_azimuth = back.x.atan2(back.z) + channel.manual_yaw;
            channel.current_incline = (cam.y - channel.previous_focus_point.y).atan2(flat);
        }
        channel.manual_yaw = 0.0;

        // Mouse wheel zoom (PC only)
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
    if keyboard.just_pressed(KeyCode::F2) {
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
