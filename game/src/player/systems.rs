/*
 * player/systems.rs — player input and movement systems.
 *
 * keyboard_input_system / gamepad_input_system: write InputState from hardware.
 * player_mouse_look_system: yaw camera or rotate player while blocking.
 * player_movement_system (FixedUpdate): converts InputState + camera forward
 * into Avian3d LinearVelocity; handles jump impulses and model rotation.
 */
use avian3d::prelude::*;
use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;

use crate::combat::components::Fighter;
use crate::control_map::{PadMapper, RawInputFrame};

use super::components::*;

const MOVE_SPEED: f32 = 6.0;
const MOUSE_SENSITIVITY: f32 = 0.003;
const JUMP_IMPULSE: f32 = 8.0;

// ---------------------------------------------------------------------------
// Keyboard / mouse input backend
// ---------------------------------------------------------------------------

/// Reads keyboard + mouse and writes to InputState.
/// Space and left-mouse both trigger the primary attack.
/// Q triggers jump (Space is now attack).
pub fn keyboard_input_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut query: Query<&mut InputState, With<Player>>,
) {
    for mut input in &mut query {
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
        input.attack =
            keyboard.just_pressed(KeyCode::Space) || mouse.just_pressed(MouseButton::Left);
        input.attack_two = mouse.just_pressed(MouseButton::Right);

        // Other actions
        input.blocking = keyboard.pressed(KeyCode::ShiftLeft);
        input.grab = keyboard.just_pressed(KeyCode::KeyE);
        input.jump = keyboard.just_pressed(KeyCode::KeyQ);
        input.evade = keyboard.just_pressed(KeyCode::KeyF);
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
    mut query: Query<&mut InputState, With<Player>>,
) {
    // Find the first connected gamepad
    let Some((_, gamepad)) = gamepads.iter().next() else {
        return;
    };

    for mut input in &mut query {
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
        input.blocking = gamepad.pressed(GamepadButton::LeftTrigger);

        if gamepad.just_pressed(GamepadButton::RightTrigger) {
            input.grab = true;
        }
    }
}

// ---------------------------------------------------------------------------
// PadMapper update — builds RawInputFrame from Bevy hardware state and ticks
// the data-driven control mapper so FSM logic can read PADCMD_* values.
// ---------------------------------------------------------------------------

/// Keyboard/mouse layout → control.map button / axis names.
///
/// Button name mapping (PlayStation-style from control.map):
///   Rup   = triangle  → Space (primary attack)
///   Rdown = cross/X   → Q (jump)
///   Rleft = circle    → E / LMB (grab, action, weapon fire)
///   Rright= square    → RMB (secondary attack)
///   L2    = L-trigger → Left Shift (block)
///   R1    = R-shoulder→ Left Ctrl (fight mode / lock-on)
///   R2    = R-trigger → C (crouch)
///   L1    = L-shoulder→ Tab (sweep forward)
///   R3    = R-stick Ø → V (weapon draw)
///
/// Analog axis mapping (character-relative, matching AnalogCharacter* in control.map):
///   AnalogCharacterForward   → W key (1.0 when pressed)
///   AnalogCharacterBackward  → S key
///   AnalogCharacterLeft      → A key
///   AnalogCharacterRight     → D key
///   AnalogLmag               → movement vector magnitude (0..1)
pub fn pad_mapper_update_system(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<(Entity, &Gamepad)>,
    mut pad_mapper: ResMut<PadMapper>,
    mut cushion: ResMut<super::components::MenuTransitionInputCushion>,
) {
    if cushion.0 > 0.0 {
        cushion.0 -= time.delta_secs();
        return;
    }

    let t = time.elapsed_secs();
    let mut frame = RawInputFrame {
        time: t,
        ..Default::default()
    };

    // ── Keyboard + mouse ──────────────────────────────────────────────────

    // Face buttons (right cluster)
    if keyboard.just_pressed(KeyCode::Space) {
        frame.press("Rup");
    } else if keyboard.pressed(KeyCode::Space) {
        frame.hold("Rup");
    }
    if keyboard.just_pressed(KeyCode::KeyQ) {
        frame.press("Rdown");
    } else if keyboard.pressed(KeyCode::KeyQ) {
        frame.hold("Rdown");
    }
    if keyboard.just_pressed(KeyCode::KeyE) || mouse.just_pressed(MouseButton::Left) {
        frame.press("Rleft");
    } else if keyboard.pressed(KeyCode::KeyE) || mouse.pressed(MouseButton::Left) {
        frame.hold("Rleft");
    }
    if mouse.just_pressed(MouseButton::Right) {
        frame.press("Rright");
    } else if mouse.pressed(MouseButton::Right) {
        frame.hold("Rright");
    }

    // Shoulder / trigger buttons
    if keyboard.pressed(KeyCode::ShiftLeft) {
        frame.hold("L2"); // block
    }
    if keyboard.just_pressed(KeyCode::ShiftLeft) {
        frame.press("L2");
    }
    if keyboard.pressed(KeyCode::ControlLeft) {
        frame.hold("R1"); // fight mode / lock-on
    }
    if keyboard.just_pressed(KeyCode::ControlLeft) {
        frame.press("R1");
    }
    if keyboard.pressed(KeyCode::KeyC) {
        frame.hold("R2"); // crouch
    }
    if keyboard.just_pressed(KeyCode::KeyC) {
        frame.press("R2");
    }
    if keyboard.just_pressed(KeyCode::Tab) {
        frame.press("L1"); // sweep forward
    } else if keyboard.pressed(KeyCode::Tab) {
        frame.hold("L1");
    }
    if keyboard.just_pressed(KeyCode::KeyV) {
        frame.press("R3"); // weapon draw
    } else if keyboard.pressed(KeyCode::KeyV) {
        frame.hold("R3");
    }

    // Movement / character analogs
    let fwd = if keyboard.pressed(KeyCode::KeyW) {
        1.0_f32
    } else {
        0.0
    };
    let back = if keyboard.pressed(KeyCode::KeyS) {
        1.0_f32
    } else {
        0.0
    };
    let left = if keyboard.pressed(KeyCode::KeyA) {
        1.0_f32
    } else {
        0.0
    };
    let right = if keyboard.pressed(KeyCode::KeyD) {
        1.0_f32
    } else {
        0.0
    };

    frame.set_analog("AnalogCharacterForward", fwd);
    frame.set_analog("AnalogCharacterBackward", back);
    frame.set_analog("AnalogCharacterLeft", left);
    frame.set_analog("AnalogCharacterRight", right);

    // D-pad / left stick cardinal analogs (same as WASD for keyboard)
    frame.set_analog("Lup", fwd);
    frame.set_analog("Ldown", back);
    frame.set_analog("Lleft", left);
    frame.set_analog("Lright", right);

    // Left stick magnitude and direction angle
    let lx = right - left; // +right, −left
    let ly = fwd - back; // +forward, −backward
    let lmag = (lx * lx + ly * ly).sqrt().min(1.0);
    frame.set_analog("AnalogLmag", lmag);
    if lmag > 0.01 {
        // Direction: 0 = forward, increasing clockwise (matching PS2 convention)
        let angle = ly.atan2(lx); // simple 2D angle
        // Normalise to [0, 1] range as control.map uses ANALOG_RANGE on AnalogLdir
        frame.set_analog(
            "AnalogLdir",
            (angle + std::f32::consts::PI) / (2.0 * std::f32::consts::PI),
        );
    }

    // ── Gamepad (merges on top of keyboard) ──────────────────────────────

    if let Some((_, gamepad)) = gamepads.iter().next() {
        let lx = gamepad.get(GamepadAxis::LeftStickX).unwrap_or(0.0);
        let ly = gamepad.get(GamepadAxis::LeftStickY).unwrap_or(0.0); // +Y = up
        let rx = gamepad.get(GamepadAxis::RightStickX).unwrap_or(0.0);
        let ry = gamepad.get(GamepadAxis::RightStickY).unwrap_or(0.0);
        const DEAD: f32 = 0.15;

        let gfwd = if ly > DEAD { ly } else { 0.0 };
        let gback = if ly < -DEAD { -ly } else { 0.0 };
        let gleft = if lx < -DEAD { -lx } else { 0.0 };
        let gright = if lx > DEAD { lx } else { 0.0 };

        // Override with gamepad values when stick is active
        let gmag = (lx * lx + ly * ly).sqrt();
        if gmag > DEAD {
            frame.set_analog("AnalogCharacterForward", gfwd);
            frame.set_analog("AnalogCharacterBackward", gback);
            frame.set_analog("AnalogCharacterLeft", gleft);
            frame.set_analog("AnalogCharacterRight", gright);
            frame.set_analog("Lup", gfwd);
            frame.set_analog("Ldown", gback);
            frame.set_analog("Lleft", gleft);
            frame.set_analog("Lright", gright);
            frame.set_analog("AnalogLmag", gmag.min(1.0));
            let angle = ly.atan2(lx);
            frame.set_analog(
                "AnalogLdir",
                (angle + std::f32::consts::PI) / (2.0 * std::f32::consts::PI),
            );
        }

        // Right stick aim
        let rmag = (rx * rx + ry * ry).sqrt();
        if rmag > DEAD {
            frame.set_analog("AnalogRmag", rmag.min(1.0));
            let angle = ry.atan2(rx);
            frame.set_analog(
                "AnalogRdir",
                (angle + std::f32::consts::PI) / (2.0 * std::f32::consts::PI),
            );
        }

        // Face buttons (South=cross, North=triangle, West=square, East=circle)
        if gamepad.just_pressed(GamepadButton::North) {
            frame.press("Rup");
        } else if gamepad.pressed(GamepadButton::North) {
            frame.hold("Rup");
        }
        if gamepad.just_pressed(GamepadButton::South) {
            frame.press("Rdown");
        } else if gamepad.pressed(GamepadButton::South) {
            frame.hold("Rdown");
        }
        if gamepad.just_pressed(GamepadButton::East) {
            frame.press("Rleft");
        } else if gamepad.pressed(GamepadButton::East) {
            frame.hold("Rleft");
        }
        if gamepad.just_pressed(GamepadButton::West) {
            frame.press("Rright");
        } else if gamepad.pressed(GamepadButton::West) {
            frame.hold("Rright");
        }

        // Shoulders / triggers
        if gamepad.pressed(GamepadButton::LeftTrigger) {
            frame.hold("L2");
        }
        if gamepad.just_pressed(GamepadButton::LeftTrigger) {
            frame.press("L2");
        }
        if gamepad.pressed(GamepadButton::RightTrigger2) {
            frame.hold("R2");
        }
        if gamepad.just_pressed(GamepadButton::RightTrigger2) {
            frame.press("R2");
        }
        if gamepad.pressed(GamepadButton::RightTrigger) {
            frame.hold("R1");
        }
        if gamepad.just_pressed(GamepadButton::RightTrigger) {
            frame.press("R1");
        }
        if gamepad.pressed(GamepadButton::LeftTrigger2) {
            frame.hold("L1");
        }
        if gamepad.just_pressed(GamepadButton::LeftTrigger2) {
            frame.press("L1");
        }
        if gamepad.just_pressed(GamepadButton::RightThumb) {
            frame.press("R3");
        }
    }

    pad_mapper.update(&frame);
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

    for (_transform, mut fighter, mut input) in &mut query {
        let yaw = -total_delta.x * MOUSE_SENSITIVITY;
        input.yaw_delta = yaw;

        if input.blocking {
            fighter.facing = Quat::from_rotation_y(yaw) * fighter.facing;
        } else if let Some(mut channel) = camera_query.iter_mut().next() {
            channel.desired_azimuth += yaw;
            channel.current_azimuth += yaw;
        }
    }
}

// ---------------------------------------------------------------------------
// EATME (Enemy Auto-Track Mistake Eliminator)
// ---------------------------------------------------------------------------
//
// Port of `bhPadMapper::CalcEATME`.
// Magnetises the player's pad input toward nearby enemies: when the
// stick is pushed roughly in an enemy's direction, the resolved travel
// vector snaps to point straight at that enemy.  Also exposes the
// snapped target so attack input can preselect a strike target instead
// of relying on the strike's wedge happening to hit something.
//
// Knobs (PADDATA.EnemyAutoTrac* in the C++):
//   • cull range — radius around the player that's eligible for the snap.
//     The legacy default is ~8 m; we mirror that.
//   • fudge      — half-angle of the snap cone.  The legacy comment at
//     legacy `bhPadMapper` says "45 degree lw" but the real value is a
//     PADDATA tuning slider; 45° gives a generous magnet without
//     feeling like the game is playing for you.
//
// Two modes per the C++ implementation:
//   • Stick has input (|stick|² > 0.04):  for each candidate enemy,
//     compute the angle difference between the camera-relative pad
//     direction and the world-space direction to the enemy; pick the
//     enemy with the smallest |diff|; if it's within fudge°, snap
//     `eatme_travel` to point at that enemy and stash the entity in
//     `eatme_target`.
//   • Stick neutral:                       just pick the enemy most
//     directly in front of the character heading.  Sets only
//     `eatme_target`; movement isn't biased.

const EATME_CULL_RANGE: f32 = 8.0;
const EATME_CULL_RANGE_SQ: f32 = EATME_CULL_RANGE * EATME_CULL_RANGE;
const EATME_FUDGE_DEG: f32 = 45.0;
const EATME_STICK_DEAD_SQ: f32 = 0.2 * 0.2;
/// Maximum |angle diff| (radians) for the "stick neutral" pick to be
/// considered "in front of" the character.  The C++ uses `minDiff =
/// 1.0f` (~57° in their atan2 coord); we mirror that loose tolerance.
const EATME_FRONT_TOLERANCE_RAD: f32 = 1.0;

use crate::combat::components::Health;
use crate::fight::components::FighterState;

/// Magnetises `InputState.movement` toward nearby enemies and surfaces
/// the locked-onto enemy via `InputState.eatme_target`.  Writes both
/// outputs each frame regardless of whether EATME engages — the
/// `Option` shape signals engagement.
pub fn eatme_system(
    mut players: Query<
        (&Transform, &mut InputState),
        (With<Player>, Without<crate::camera::components::CameraController>),
    >,
    camera_q: Query<
        &Transform,
        (
            With<crate::camera::components::CameraController>,
            Without<Player>,
        ),
    >,
    candidates: Query<
        (Entity, &Transform, &Health),
        (
            With<FighterState>,
            Without<Player>,
            Without<crate::camera::components::CameraController>,
        ),
    >,
) {
    let Some(cam_tf) = camera_q.iter().next() else {
        // No camera — clear and bail.
        for (_, mut input) in &mut players {
            input.eatme_target = None;
            input.eatme_travel = None;
        }
        return;
    };

    // Build camera basis once.  `cam_fwd_xz` is the camera's forward
    // projected onto the XZ plane (the input plane).  The same
    // transform the movement system uses — keeps the two views aligned.
    let cam_fwd = cam_tf.forward().as_vec3();
    let cam_right = cam_tf.right().as_vec3();
    let cam_fwd_xz = Vec3::new(cam_fwd.x, 0.0, cam_fwd.z).normalize_or_zero();
    let cam_right_xz = Vec3::new(cam_right.x, 0.0, cam_right.z).normalize_or_zero();
    if cam_fwd_xz.length_squared() < 1e-6 || cam_right_xz.length_squared() < 1e-6 {
        // Camera is looking straight up/down — bail.
        for (_, mut input) in &mut players {
            input.eatme_target = None;
            input.eatme_travel = None;
        }
        return;
    }

    let fudge_rad = EATME_FUDGE_DEG.to_radians();

    for (player_tf, mut input) in &mut players {
        // Start fresh each tick.
        input.eatme_target = None;
        input.eatme_travel = None;

        // Reconstruct the world-space travel direction from the raw
        // stick using the same mapping `player_movement_system` does
        // (W=−y forward, S=+y back, A=+x left, D=−x right).
        let stick = input.movement;
        let stick_mag_sq = stick.length_squared();
        let mut travel = Vec3::ZERO;
        if stick.y < -0.1 {
            travel += cam_fwd_xz;
        }
        if stick.y > 0.1 {
            travel -= cam_fwd_xz;
        }
        if stick.x > 0.1 {
            travel -= cam_right_xz;
        }
        if stick.x < -0.1 {
            travel += cam_right_xz;
        }
        let travel = travel.normalize_or_zero();

        let player_pos = player_tf.translation;

        // ── Stick-neutral branch ─────────────────────────────────────
        // Just pick the enemy most directly in front of the character
        // heading.  Don't bias movement.  Mirrors the legacy stick-neutral branch.
        if stick_mag_sq < EATME_STICK_DEAD_SQ {
            // Player's forward is +Z relative to their model; the
            // `Transform.back()` happens to be local +Z (model faces
            // away from camera).  Use that as the heading reference.
            let player_fwd = player_tf.back().as_vec3();
            let head_angle = player_fwd.x.atan2(player_fwd.z);

            let mut best: Option<(f32, Entity)> = None;
            for (ent, ent_tf, hp) in &candidates {
                if hp.current <= 0.0 {
                    continue;
                }
                let diff = ent_tf.translation - player_pos;
                let dist_sq = diff.x * diff.x + diff.z * diff.z;
                if dist_sq > EATME_CULL_RANGE_SQ || dist_sq < 1e-4 {
                    continue;
                }
                let to_enemy = Vec3::new(diff.x, 0.0, diff.z)
                    .try_normalize()
                    .unwrap_or(Vec3::Z);
                let enemy_angle = to_enemy.x.atan2(to_enemy.z);
                let h_diff = (head_angle - enemy_angle).abs();
                if h_diff >= EATME_FRONT_TOLERANCE_RAD {
                    continue;
                }
                if best.map(|(d, _)| h_diff < d).unwrap_or(true) {
                    best = Some((h_diff, ent));
                }
            }
            if let Some((_, ent)) = best {
                input.eatme_target = Some(ent);
            }
            continue;
        }

        // ── Stick-active branch ──────────────────────────────────────
        // Find the enemy with the smallest signed angle diff between
        // the desired travel and the direction to them.  If that diff
        // is within fudge°, snap travel to point at them and stash
        // them as the target.  Mirrors the legacy stick-active branch.
        let travel_angle = travel.x.atan2(travel.z);

        let mut best_diff = std::f32::consts::PI;
        let mut best_dir: Vec3 = travel;
        let mut best_ent: Option<Entity> = None;

        for (ent, ent_tf, hp) in &candidates {
            if hp.current <= 0.0 {
                continue;
            }
            let diff = ent_tf.translation - player_pos;
            let dist_sq = diff.x * diff.x + diff.z * diff.z;
            if dist_sq > EATME_CULL_RANGE_SQ || dist_sq < 1e-4 {
                continue;
            }
            let to_enemy = Vec3::new(diff.x, 0.0, diff.z)
                .try_normalize()
                .unwrap_or(Vec3::Z);
            let enemy_angle = to_enemy.x.atan2(to_enemy.z);
            let mut signed = enemy_angle - travel_angle;
            while signed > std::f32::consts::PI {
                signed -= std::f32::consts::TAU;
            }
            while signed < -std::f32::consts::PI {
                signed += std::f32::consts::TAU;
            }
            if signed.abs() < best_diff.abs() {
                best_diff = signed;
                best_dir = to_enemy;
                best_ent = Some(ent);
            }
        }

        if best_diff.abs() < fudge_rad
            && let Some(ent) = best_ent
        {
            input.eatme_travel = Some(best_dir);
            input.eatme_target = Some(ent);
        }
    }
}

// ---------------------------------------------------------------------------
// eatme_strike_target_seed_system
// ---------------------------------------------------------------------------

/// On attack press, pre-seed `FighterState.strike_target` with the
/// current EATME target so the attacker's body locks onto that enemy
/// from the first frame of the swing.  Without this, `strike_target`
/// only gets set REACTIVELY after a hit lands (combat/systems.rs:483),
/// which is too late for the per-frame body-rotation in
/// `update_fighter_strike_facing_system` to track during the windup.
///
/// Mirrors the legacy `PADDATA.AttackTrackTargetIsEATMETarget` branch
/// — when set, the strike's preselected
/// target is the EATME pick.
pub fn eatme_strike_target_seed_system(
    mut players: Query<(&InputState, &mut FighterState), With<Player>>,
) {
    for (input, mut fs) in &mut players {
        if !(input.attack || input.attack_two || input.grab) {
            continue;
        }
        let Some(target) = input.eatme_target else {
            continue;
        };
        // Don't clobber an existing strike target — the attack-resolution
        // path may have already set one this frame.
        if fs.strike_target.is_none() {
            fs.strike_target = Some(target);
            // Strike targets seeded from EATME aren't auto-cleared on
            // first use — keeping the body-lock active for the full
            // attack feels right with how the player aims.  Set the
            // flag to false explicitly so a prior attack's value
            // doesn't carry over.
            fs.clear_st_after_first_use = false;
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
            Entity,
            &InputState,
            &mut Transform,
            &mut LinearVelocity,
            &mut Fighter,
            Option<&crate::statemachine::runtime::FsmRuntime>,
            Option<&crate::oni2_loader::animation::Oni2AnimState>,
        ),
        With<Player>,
    >,
    camera_query: Query<
        &Transform,
        (
            With<crate::camera::components::CameraController>,
            Without<Player>,
        ),
    >,
    _start_action_writer: MessageWriter<crate::animator::StartActionMessage>,
    mut jump_writer: MessageWriter<crate::animator::JumpImpulseMessage>,
) {
    let camera_tf_opt = camera_query.iter().next();

    for (entity, input, transform, mut velocity, mut fighter, fsm_opt, anim_state_opt) in
        &mut query
    {
        if let Some(fsm) = fsm_opt {
            // Lock movement and steering while an attack/block animation is actively playing
            if fsm.ctx.active_anim.is_some() && !fsm.ctx.timed_out {
                // Provide quick deceleration so running attacks don't slide wildly,
                // but leave vertical velocity alone for gravity.
                velocity.x *= 0.8;
                velocity.z *= 0.8;
                continue;
            }
        }

        let is_grounded = anim_state_opt.is_none_or(|s| s.is_grounded);
        // Debug/Placeholder players have no Animator/Oni2AnimState — they can't
        // run the data-driven jump schedule, so they emit a JumpImpulseMessage
        // directly. Both backends subscribe: Dynamic via
        // `jump_impulse_apply_system` (writes velocity.y), Tnua via
        // `mover::bridge::tnua_jump_from_message` (fires TnuaBuiltinJump).
        let is_placeholder = anim_state_opt.is_none();
        if is_placeholder && input.jump && is_grounded {
            jump_writer.write(crate::animator::JumpImpulseMessage {
                entity,
                vertical: JUMP_IMPULSE,
                forward: 0.0,
                lateral: 0.0,
                gravity_factor: 1.0,
                time: 0.0,
                heading_adjust: 0.0,
            });
        }

        if input.movement.length_squared() < 0.001 {
            velocity.x = 0.0;
            velocity.z = 0.0;
            continue;
        }

        let mut travel = Vec3::ZERO;

        if input.blocking {
            // The 3D model inherently faces local +Z (which is transform.back())
            let visual_front = transform.back().as_vec3();
            let visual_right = -transform.right().as_vec3(); // +X is left relative to +Z front, so right is -X

            // W -> y=-1 -> go visual_front
            // S -> y=+1 -> go -visual_front
            // A -> x=+1 -> go -visual_right (left)
            // D -> x=-1 -> go +visual_right (right)
            travel = (visual_front * -input.movement.y + visual_right * -input.movement.x)
                .normalize_or_zero();
        } else if let Some(cam_tf) = camera_tf_opt {
            // Locomotion uses the RAW camera-relative stick direction —
            // EATME does NOT magnetise navigation.  The earlier port
            // routed `input.eatme_travel` into `travel` here, which
            // visibly pulled the player toward nearby enemies during
            // normal running.  EATME's only locomotion-side effect in
            // the legacy engine is the strike-target preselection
            // (`eatme_strike_target_seed_system`) — the player still
            // moves where the stick points.  `eatme_target`/
            // `eatme_travel` remain populated each tick for the
            // attack-time seed and for any future consumer that wants
            // a magnetised aim (e.g. Z-lock acquisition), but they're
            // not fed into world movement.
            let cam_fwd = cam_tf.forward().as_vec3();
            let cam_right = cam_tf.right().as_vec3();
            let xz_fwd = Vec3::new(cam_fwd.x, 0.0, cam_fwd.z).normalize_or_zero();
            let xz_right = Vec3::new(cam_right.x, 0.0, cam_right.z).normalize_or_zero();

            if input.movement.y < -0.1 {
                // W
                travel += xz_fwd;
            }
            if input.movement.y > 0.1 {
                // S
                travel -= xz_fwd;
            }
            if input.movement.x > 0.1 {
                // A
                travel -= xz_right;
            }
            if input.movement.x < -0.1 {
                // D
                travel += xz_right;
            }

            travel = travel.normalize_or_zero();

            if travel.length_squared() > 0.001 {
                // Since visually the model faces local +Z, we must point local -Z OPPOSITE to travel
                let target_rot = Transform::default().looking_to(-travel, Vec3::Y).rotation;

                let current_rot =
                    Quat::from_rotation_arc(Vec3::Z, fighter.facing.normalize_or_zero());
                let new_rot = current_rot.slerp(target_rot, 0.25);
                fighter.facing = new_rot * Vec3::Z;
            }
        } else {
            let visual_front = fighter.facing;
            let visual_right = Vec3::new(visual_front.z, 0.0, visual_front.x);
            travel = (visual_front * -input.movement.y + visual_right * -input.movement.x)
                .normalize_or_zero();
        }

        let desired = travel * MOVE_SPEED;
        velocity.x = desired.x;
        velocity.z = desired.z;
    }
}

pub fn clear_inputs_on_enter(
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut pad_mapper: ResMut<PadMapper>,
    mut cushion: ResMut<super::components::MenuTransitionInputCushion>,
) {
    mouse.clear();
    keys.clear();
    pad_mapper.clear();
    cushion.0 = 0.4;
}
