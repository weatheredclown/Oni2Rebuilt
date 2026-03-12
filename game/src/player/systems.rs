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

/// Reads mouse motion and rotates the player (runs in Update).
pub fn player_mouse_look_system(
    mut motion_reader: MessageReader<MouseMotion>,
    mut query: Query<(&mut Transform, &mut Fighter, &mut InputState), With<Player>>,
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
        transform.rotate_y(yaw);
        fighter.facing = transform.forward().as_vec3();
    }
}

/// Moves the player based on InputState using physics velocity (runs in FixedUpdate).
pub fn player_movement_system(
    mut query: Query<(&InputState, &Transform, &mut LinearVelocity, &mut Fighter), With<Player>>,
) {
    for (input, transform, mut velocity, mut fighter) in &mut query {
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

        let forward = transform.forward().as_vec3();
        let right = transform.right().as_vec3();

        let move_dir = (forward * input.movement.y + right * input.movement.x).normalize();
        let desired = move_dir * MOVE_SPEED;

        velocity.x = desired.x;
        velocity.z = desired.z;
    }
}
