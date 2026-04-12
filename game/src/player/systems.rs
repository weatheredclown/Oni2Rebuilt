use avian3d::prelude::*;
use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;

use crate::combat::components::{BlockState, Fighter};

use super::components::*;

const MOVE_SPEED: f32 = 6.0;
const MOUSE_SENSITIVITY: f32 = 0.003;
const JUMP_IMPULSE: f32 = 8.0;
const DOUBLE_JUMP_IMPULSE: f32 = 7.0;

// ---------------------------------------------------------------------------
// Keyboard / mouse input backend
// ---------------------------------------------------------------------------

/// Reads keyboard + mouse and writes to InputState.
/// Space and left-mouse both trigger the primary attack.
/// Q triggers jump (Space is now attack).
pub fn keyboard_input_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut query: Query<(&mut InputState, &mut BlockState), With<Player>>,
) {
    for (mut input, mut block) in &mut query {
        // Movement: W=forward(−y), S=back(+y), A=left(+x), D=right(−x)
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
        input.movement = if movement.length_squared() > 0.0 {
            movement.normalize()
        } else {
            Vec2::ZERO
        };

        // Attack inputs
        input.attack = keyboard.just_pressed(KeyCode::Space) || mouse.just_pressed(MouseButton::Left);
        input.attack_two = mouse.just_pressed(MouseButton::Right);

        // Other actions
        input.blocking = keyboard.pressed(KeyCode::ShiftLeft);
        input.grab = keyboard.just_pressed(KeyCode::KeyE);
        input.jump = keyboard.just_pressed(KeyCode::KeyQ);
        input.evade = keyboard.just_pressed(KeyCode::KeyF);

        block.is_blocking = input.blocking;
    }
}

// ---------------------------------------------------------------------------
// Gamepad input backend  (Xbox-style layout — stub, wired but no-op until
// a gamepad is actually connected; extend as needed)
// ---------------------------------------------------------------------------

/// Reads any connected gamepad and merges into InputState.
/// This runs *after* keyboard_input_system so gamepad can override/supplement.
pub fn gamepad_input_system(
    gamepads: Query<(Entity, &Gamepad)>,
    mut query: Query<(&mut InputState, &mut BlockState), With<Player>>,
) {
    // Find the first connected gamepad
    let Some((_, gamepad)) = gamepads.iter().next() else {
        return;
    };

    for (mut input, mut block) in &mut query {
        // Left stick → movement
        let lx = gamepad.get(GamepadAxis::LeftStickX).unwrap_or(0.0);
        let ly = gamepad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        let stick = Vec2::new(-lx, ly); // x-flip matches WASD convention
        const DEADZONE: f32 = 0.15;
        if stick.length() > DEADZONE {
            input.movement = stick.normalize();
        }

        // Face buttons (Xbox layout)
        if gamepad.just_pressed(GamepadButton::South) {
            // A button — jump
            input.jump = true;
        }
        if gamepad.just_pressed(GamepadButton::West) {
            // X button — primary attack
            input.attack = true;
        }
        if gamepad.just_pressed(GamepadButton::North) {
            // Y button — secondary attack
            input.attack_two = true;
        }
        if gamepad.just_pressed(GamepadButton::East) {
            // B button — evade
            input.evade = true;
        }

        // Shoulder buttons
        let lb = gamepad.pressed(GamepadButton::LeftTrigger);
        input.blocking = lb;
        block.is_blocking = lb;

        if gamepad.just_pressed(GamepadButton::RightTrigger) {
            input.grab = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Mouse look
// ---------------------------------------------------------------------------

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
            transform.rotate_y(yaw);
            fighter.facing = transform.forward().as_vec3();
        } else {
            if let Some(mut channel) = camera_query.iter_mut().next() {
                channel.desired_azimuth += yaw;
                channel.current_azimuth += yaw;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Physics movement
// ---------------------------------------------------------------------------

/// Moves the player based on InputState using physics velocity (runs in FixedUpdate).
pub fn player_movement_system(
    mut query: Query<
        (
            &InputState,
            &mut Transform,
            &mut LinearVelocity,
            &mut Fighter,
            Option<&crate::statemachine::runtime::FsmRuntime>,
        ),
        With<Player>,
    >,
    camera_query: Query<
        &Transform,
        (With<crate::camera::components::CameraController>, Without<Player>),
    >,
) {
    let camera_tf_opt = camera_query.iter().next();

    for (input, mut transform, mut velocity, mut fighter, fsm_opt) in &mut query {
        if let Some(fsm) = fsm_opt {
            // Lock movement and steering while an attack/block animation is actively playing
            if fsm.active_anim.is_some() && !fsm.timed_out {
                // Provide quick deceleration so running attacks don't slide wildly,
                // but leave vertical velocity alone for gravity.
                velocity.x *= 0.8;
                velocity.z *= 0.8;
                continue;
            }
        }

        if fighter.is_grounded {
            fighter.jumps_remaining = fighter.max_jumps;
        }

        if input.jump && fighter.jumps_remaining > 0 {
            let impulse = if fighter.jumps_remaining == fighter.max_jumps {
                JUMP_IMPULSE
            } else {
                DOUBLE_JUMP_IMPULSE
            };
            velocity.y = impulse;
            fighter.jumps_remaining -= 1;
        }

        if input.movement.length_squared() < 0.001 {
            velocity.x = 0.0;
            velocity.z = 0.0;
            continue;
        }

        let mut move_dir = Vec3::ZERO;

        if input.blocking {
            let forward = -transform.forward().as_vec3();
            let right = -transform.right().as_vec3();
            move_dir = (forward * input.movement.y + right * input.movement.x)
                .normalize_or_zero();
        } else if let Some(cam_tf) = camera_tf_opt {
            let mov_x = input.movement.x;
            let mov_y = input.movement.y;
            let cam_fwd = cam_tf.forward().as_vec3();
            let cam_right = cam_tf.right().as_vec3();
            let xz_fwd = Vec3::new(cam_fwd.x, 0.0, cam_fwd.z).normalize_or_zero();
            let xz_right = Vec3::new(cam_right.x, 0.0, cam_right.z).normalize_or_zero();

            if mov_y < -0.1 {
                move_dir -= xz_fwd;
            }
            if mov_y > 0.1 {
                move_dir += xz_fwd;
            }
            if mov_x > 0.1 {
                move_dir += xz_right;
            }
            if mov_x < -0.1 {
                move_dir -= xz_right;
            }

            move_dir = move_dir.normalize_or_zero();

            if move_dir.length_squared() > 0.001 {
                let target_rot =
                    Transform::default().looking_to(move_dir, Vec3::Y).rotation;
                transform.rotation = transform.rotation.slerp(target_rot, 0.25);
                fighter.facing = transform.forward().as_vec3();
            }
        } else {
            let forward = transform.forward().as_vec3();
            let right = transform.right().as_vec3();
            move_dir = (forward * input.movement.y + right * input.movement.x)
                .normalize_or_zero();
        }

        let desired = move_dir * -MOVE_SPEED;
        velocity.x = desired.x;
        velocity.z = desired.z;
    }
}
