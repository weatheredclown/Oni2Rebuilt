use avian3d::prelude::*;
use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;

use crate::combat::components::{BlockState, Fighter};

use super::components::*;

const MOVE_SPEED: f32 = 6.0;
const MOUSE_SENSITIVITY: f32 = 0.003;
const JUMP_IMPULSE: f32 = 8.0;
const DOUBLE_JUMP_IMPULSE: f32 = 7.0;

/// Reads keyboard/mouse input and writes to InputState (runs in Update).
pub fn player_input_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut query: Query<(&mut InputState, &mut BlockState), With<Player>>,
) {
    for (mut input, mut block) in &mut query {
        let mut movement = Vec2::ZERO;
        if keyboard.pressed(KeyCode::KeyW) {
            movement.y -= 1.0;
        }
        if keyboard.pressed(KeyCode::KeyS) {
            movement.y += 1.0;
        }
        if keyboard.pressed(KeyCode::KeyA) {
            movement.x += 1.0;
        }
        if keyboard.pressed(KeyCode::KeyD) {
            movement.x -= 1.0;
        }
        if movement.length_squared() > 0.0 {
            movement = movement.normalize();
        }
        input.movement = movement;

        input.light_attack = mouse.just_pressed(MouseButton::Left);
        input.heavy_attack = mouse.just_pressed(MouseButton::Right);
        input.blocking = keyboard.pressed(KeyCode::ShiftLeft);
        input.grab = keyboard.just_pressed(KeyCode::KeyE);
        input.jump = keyboard.just_pressed(KeyCode::Space);
        // Directional attacks: Ctrl + A/D/S/W
        input.attack_direction =
            if keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight) {
                if keyboard.pressed(KeyCode::KeyA) {
                    -std::f32::consts::FRAC_PI_2 // left
                } else if keyboard.pressed(KeyCode::KeyD) {
                    std::f32::consts::FRAC_PI_2 // right
                } else if keyboard.pressed(KeyCode::KeyS) {
                    std::f32::consts::PI // behind
                } else {
                    0.0 // forward (default)
                }
            } else {
                0.0
            };

        block.is_blocking = input.blocking;
    }
}

pub fn player_mouse_look_system(
    mut motion_reader: MessageReader<MouseMotion>,
    mut query: Query<(&mut Transform, &mut Fighter, &mut InputState), With<Player>>,
    mut camera_query: Query<&mut crate::camera::channel::CameraChannel>,
) {
    let mut total_delta = Vec2::ZERO;
    for motion in motion_reader.read() {
        total_delta += motion.delta;
    }

    if total_delta.length_squared() < 0.0001 {
        return;
    }

    for (mut transform, mut fighter, mut input) in &mut query {
        let yaw = -total_delta.x * MOUSE_SENSITIVITY;
        input.yaw_delta = yaw;
        
        if input.blocking {
            // Strafing: always tied to mouse
            transform.rotate_y(yaw);
            fighter.facing = transform.forward().as_vec3();
        } else {
            // Without shift, pass yaw straight to the camera so we can still look around
            if let Some(mut channel) = camera_query.iter_mut().next() {
                channel.desired_azimuth += yaw;
                channel.current_azimuth += yaw;
            }
            
        }
    }
}

/// Moves the player based on InputState using physics velocity (runs in FixedUpdate).
pub fn player_movement_system(
    mut query: Query<(&InputState, &mut Transform, &mut LinearVelocity, &mut Fighter), With<Player>>,
    camera_query: Query<&Transform, (With<crate::camera::components::CameraController>, Without<Player>)>,
) {
    let camera_tf_opt = camera_query.iter().next();

    for (input, mut transform, mut velocity, mut fighter) in &mut query {
        // Reset jumps when grounded
        if fighter.is_grounded {
            fighter.jumps_remaining = fighter.max_jumps;
        }

        // Jump / double jump
        if input.jump && fighter.jumps_remaining > 0 {
            let impulse = if fighter.jumps_remaining == fighter.max_jumps {
                JUMP_IMPULSE
            } else {
                DOUBLE_JUMP_IMPULSE
            };
            // Reset vertical velocity before applying impulse (cleaner double jump)
            velocity.y = impulse;
            fighter.jumps_remaining -= 1;
        }

        // Horizontal movement
        if input.movement.length_squared() < 0.001 {
            velocity.x = 0.0;
            velocity.z = 0.0;
            continue;
        }

        let mut move_dir = Vec3::ZERO;

        if input.blocking {
            // Strafing relies purely on current orientation
            let forward = -transform.forward().as_vec3();
            let right = -transform.right().as_vec3();
            move_dir = (forward * input.movement.y + right * input.movement.x).normalize_or_zero();
        } else {
            // Navigation relies on WASD relative to the camera
            if let Some(cam_tf) = camera_tf_opt {
                let mov_x = input.movement.x;
                let mov_y = input.movement.y;
                let cam_fwd = cam_tf.forward().as_vec3();
                let cam_right = cam_tf.right().as_vec3();
                let xz_fwd = Vec3::new(cam_fwd.x, 0.0, cam_fwd.z).normalize_or_zero();
                let xz_right = Vec3::new(cam_right.x, 0.0, cam_right.z).normalize_or_zero();

                if mov_y < -0.1 { // W
                    move_dir -= xz_fwd;
                }
                if mov_y > 0.1 { // S
                    move_dir += xz_fwd;
                }
                if mov_x > 0.1 { // A
                    move_dir += xz_right;
                }
                if mov_x < -0.1 { // D
                    move_dir -= xz_right;
                }

                move_dir = move_dir.normalize_or_zero();

                if move_dir.length_squared() > 0.001 {
                    // Pivot character physically
                    let target_rot = Transform::default().looking_to(move_dir, Vec3::Y).rotation;
                    transform.rotation = transform.rotation.slerp(target_rot, 0.25);
                    fighter.facing = transform.forward().as_vec3();
                }
            } else {
                // Fallback without camera
                let forward = transform.forward().as_vec3();
                let right = transform.right().as_vec3();
                move_dir = (forward * input.movement.y + right * input.movement.x).normalize_or_zero();
            }
        }

        let desired = move_dir * -MOVE_SPEED;
        velocity.x = desired.x;
        velocity.z = desired.z;
    }
}
