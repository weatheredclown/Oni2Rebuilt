use bevy::prelude::*;
use bevy::input::mouse::AccumulatedMouseMotion;
use super::components::{ActiveCameraMode, CameraController};

/// Handles debug free-fly movement and rotation independently of the CameraChannel
/// because it overrides Transform natively directly via WASD.
pub fn freecam_system(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    accumulated_motion: Res<AccumulatedMouseMotion>,
    mut camera_query: Query<(&mut CameraController, &mut Transform)>,
) {
    let dt = time.delta_secs();

    for (mut controller, mut cam_tf) in &mut camera_query {
        if controller.active_mode != ActiveCameraMode::DebugFreeCam {
            continue;
        }

        // Mouse look (hold right mouse button)
        if mouse_button.pressed(MouseButton::Right) {
            let sensitivity = 0.003;
            let delta = accumulated_motion.delta;
            controller.free_yaw -= delta.x * sensitivity;
            controller.free_pitch = (controller.free_pitch - delta.y * sensitivity).clamp(-1.4, 1.4);
        }

        let speed = controller.free_speed;

        let forward = Vec3::new(controller.free_yaw.sin(), 0.0, controller.free_yaw.cos()).normalize();
        let right = Vec3::new(-controller.free_yaw.cos(), 0.0, controller.free_yaw.sin()).normalize();
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
            Quat::from_rotation_y(controller.free_yaw) * Quat::from_rotation_x(controller.free_pitch);
    }
}
