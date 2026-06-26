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
use crate::control_map::{PadMapper, PadTuneSettings, RawInputFrame};

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
    mut query: Query<(&mut InputState, Option<&crate::fight::components::FighterState>), With<Player>>,
) {
    for (mut input, fs_opt) in &mut query {
        if let Some(fs) = fs_opt {
            if fs.is_grappling() || fs.is_being_grappled() {
                *input = InputState::default();
                continue;
            }
        }

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
        
        // Targeting
        input.targeting_modifier = keyboard.pressed(KeyCode::ControlLeft);
        input.reticle_input = Vec2::ZERO;
        input.reticle_input_is_absolute = false;
    }
}

// ---------------------------------------------------------------------------
// Gamepad input backend  (Xbox-style layout — stub, wired but no-op until
// a gamepad is actually connected; extend as needed)
// ---------------------------------------------------------------------------

/// Reads any connected gamepad and merges into InputState.
/// This runs *after* keyboard_input_system so gamepad can override/supplement.
pub fn gamepad_input_system(
    pad_settings: Res<PadTuneSettings>,
    gamepads: Query<(Entity, &Gamepad)>,
    mut query: Query<(&mut InputState, Option<&crate::fight::components::FighterState>), With<Player>>,
) {
    // Find the first connected gamepad
    let Some((_, gamepad)) = gamepads.iter().next() else {
        return;
    };

    for (mut input, fs_opt) in &mut query {
        if let Some(fs) = fs_opt {
            if fs.is_grappling() || fs.is_being_grappled() {
                *input = InputState::default();
                continue;
            }
        }

        // Left stick → movement
        let lx = gamepad.get(GamepadAxis::LeftStickX).unwrap_or(0.0);
        let ly = gamepad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        let stick = Vec2::new(-lx, ly); // x-flip matches WASD convention
        let deadzone = pad_settings.threshold;
        if stick.length() > deadzone {
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

        // Targeting
        input.targeting_modifier = gamepad.pressed(GamepadButton::RightTrigger);
        let rx = gamepad.get(GamepadAxis::RightStickX).unwrap_or(0.0);
        let ry = gamepad.get(GamepadAxis::RightStickY).unwrap_or(0.0);
        let stick = Vec2::new(rx, ry);
        if stick.length() > pad_settings.threshold {
            input.reticle_input = stick;
            input.reticle_input_is_absolute = true;
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
    pad_settings: Res<PadTuneSettings>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<(Entity, &Gamepad)>,
    mut pad_mapper: ResMut<PadMapper>,
    mut cushion: ResMut<super::components::MenuTransitionInputCushion>,
    mut crouch_toggled: Local<bool>,
    mut block_toggled: Local<bool>,
    mut fight_mode_toggled: Local<bool>,
    mut lock_on_toggled: Local<bool>,
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

    let gamepad_opt = gamepads.iter().next().map(|(_, g)| g);

    // ── Unified Button Handling ──────────────────────────────────────────

    // Rup (Space / Gamepad North)
    let rup_pressed = keyboard.just_pressed(KeyCode::Space)
        || gamepad_opt
            .map(|g| g.just_pressed(GamepadButton::North))
            .unwrap_or(false);
    let rup_held = keyboard.pressed(KeyCode::Space)
        || gamepad_opt
            .map(|g| g.pressed(GamepadButton::North))
            .unwrap_or(false);
    if rup_pressed {
        frame.press("Rup");
    } else if rup_held {
        frame.hold("Rup");
    }

    // Rdown (KeyQ / Gamepad South)
    let rdown_pressed = keyboard.just_pressed(KeyCode::KeyQ)
        || gamepad_opt
            .map(|g| g.just_pressed(GamepadButton::South))
            .unwrap_or(false);
    let rdown_held = keyboard.pressed(KeyCode::KeyQ)
        || gamepad_opt
            .map(|g| g.pressed(GamepadButton::South))
            .unwrap_or(false);
    if rdown_pressed {
        frame.press("Rdown");
    } else if rdown_held {
        frame.hold("Rdown");
    }

    // Rleft (KeyE / LMB / Gamepad East)
    let rleft_pressed = keyboard.just_pressed(KeyCode::KeyE)
        || mouse.just_pressed(MouseButton::Left)
        || gamepad_opt
            .map(|g| g.just_pressed(GamepadButton::East))
            .unwrap_or(false);
    let rleft_held = keyboard.pressed(KeyCode::KeyE)
        || mouse.pressed(MouseButton::Left)
        || gamepad_opt
            .map(|g| g.pressed(GamepadButton::East))
            .unwrap_or(false);
    if rleft_pressed {
        frame.press("Rleft");
    } else if rleft_held {
        frame.hold("Rleft");
    }

    // Rright (RMB / Gamepad West)
    let rright_pressed = mouse.just_pressed(MouseButton::Right)
        || gamepad_opt
            .map(|g| g.just_pressed(GamepadButton::West))
            .unwrap_or(false);
    let rright_held = mouse.pressed(MouseButton::Right)
        || gamepad_opt
            .map(|g| g.pressed(GamepadButton::West))
            .unwrap_or(false);
    if rright_pressed {
        frame.press("Rright");
    } else if rright_held {
        frame.hold("Rright");
    }

    // L1 (Tab / Gamepad LeftTrigger2)
    let l1_pressed = keyboard.just_pressed(KeyCode::Tab)
        || gamepad_opt
            .map(|g| g.just_pressed(GamepadButton::LeftTrigger2))
            .unwrap_or(false);
    let l1_held = keyboard.pressed(KeyCode::Tab)
        || gamepad_opt
            .map(|g| g.pressed(GamepadButton::LeftTrigger2))
            .unwrap_or(false);
    if l1_pressed {
        frame.press("L1");
    } else if l1_held {
        frame.hold("L1");
    }

    // R3 (KeyV / KeyH / Gamepad RightThumb)
    let r3_pressed = keyboard.just_pressed(KeyCode::KeyV)
        || keyboard.just_pressed(KeyCode::KeyH)
        || gamepad_opt
            .map(|g| g.just_pressed(GamepadButton::RightThumb))
            .unwrap_or(false);
    let r3_held = keyboard.pressed(KeyCode::KeyV)
        || keyboard.pressed(KeyCode::KeyH)
        || gamepad_opt
            .map(|g| g.pressed(GamepadButton::RightThumb))
            .unwrap_or(false);
    if r3_pressed {
        frame.press("R3");
    } else if r3_held {
        frame.hold("R3");
    }

    // Crouch (R2)
    let crouch_raw_pressed = keyboard.pressed(KeyCode::KeyC)
        || gamepad_opt
            .map(|g| g.pressed(GamepadButton::RightTrigger2))
            .unwrap_or(false);
    let crouch_raw_just_pressed = keyboard.just_pressed(KeyCode::KeyC)
        || gamepad_opt
            .map(|g| g.just_pressed(GamepadButton::RightTrigger2))
            .unwrap_or(false);
    if pad_settings.crouch_button_is_toggle {
        if crouch_raw_just_pressed {
            *crouch_toggled = !*crouch_toggled;
        }
        if *crouch_toggled {
            if crouch_raw_just_pressed {
                frame.press("R2");
            } else {
                frame.hold("R2");
            }
        }
    } else {
        if crouch_raw_just_pressed {
            frame.press("R2");
        } else if crouch_raw_pressed {
            frame.hold("R2");
        }
    }

    // Block (L2)
    let block_raw_pressed = keyboard.pressed(KeyCode::ShiftLeft)
        || gamepad_opt
            .map(|g| g.pressed(GamepadButton::LeftTrigger))
            .unwrap_or(false);
    let block_raw_just_pressed = keyboard.just_pressed(KeyCode::ShiftLeft)
        || gamepad_opt
            .map(|g| g.just_pressed(GamepadButton::LeftTrigger))
            .unwrap_or(false);
    if pad_settings.block_button_is_hold {
        if block_raw_just_pressed {
            frame.press("L2");
        } else if block_raw_pressed {
            frame.hold("L2");
        }
    } else {
        if block_raw_just_pressed {
            *block_toggled = !*block_toggled;
        }
        if *block_toggled {
            if block_raw_just_pressed {
                frame.press("L2");
            } else {
                frame.hold("L2");
            }
        }
    }

    // Fight Mode / Lock-On (R1)
    let r1_raw_pressed = keyboard.pressed(KeyCode::ControlLeft)
        || gamepad_opt
            .map(|g| g.pressed(GamepadButton::RightTrigger))
            .unwrap_or(false);
    let r1_raw_just_pressed = keyboard.just_pressed(KeyCode::ControlLeft)
        || gamepad_opt
            .map(|g| g.just_pressed(GamepadButton::RightTrigger))
            .unwrap_or(false);

    let lock_on_active = if pad_settings.lock_on_button_is_toggle {
        if r1_raw_just_pressed {
            *lock_on_toggled = !*lock_on_toggled;
        }
        *lock_on_toggled
    } else if pad_settings.lock_on_button_hold_is_on {
        r1_raw_pressed
    } else {
        !r1_raw_pressed
    };

    let fight_mode_active = if pad_settings.fight_mode_button_is_toggle {
        if r1_raw_just_pressed {
            *fight_mode_toggled = !*fight_mode_toggled;
        }
        *fight_mode_toggled
    } else {
        r1_raw_pressed
    };

    let r1_active = lock_on_active || fight_mode_active;
    if r1_active {
        if r1_raw_just_pressed {
            frame.press("R1");
        } else {
            frame.hold("R1");
        }
    }

    // ── Keyboard Movement analogs ──────────────────────────────────────────

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

    frame.set_analog("Lup", fwd);
    frame.set_analog("Ldown", back);
    frame.set_analog("Lleft", left);
    frame.set_analog("Lright", right);

    let lx = right - left;
    let ly = fwd - back;
    let lmag = (lx * lx + ly * ly).sqrt().min(1.0);
    frame.set_analog("AnalogLmag", lmag);
    if lmag > 0.01 {
        let angle = ly.atan2(lx);
        frame.set_analog(
            "AnalogLdir",
            (angle + std::f32::consts::PI) / (2.0 * std::f32::consts::PI),
        );
    }

    // ── Gamepad stick analogs (merges on top, respects threshold) ─────────

    if let Some(gamepad) = gamepad_opt {
        let lx = gamepad.get(GamepadAxis::LeftStickX).unwrap_or(0.0);
        let ly = gamepad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        let rx = gamepad.get(GamepadAxis::RightStickX).unwrap_or(0.0);
        let ry = gamepad.get(GamepadAxis::RightStickY).unwrap_or(0.0);
        let thresh = pad_settings.threshold;

        let gfwd = if ly > thresh { ly } else { 0.0 };
        let gback = if ly < -thresh { -ly } else { 0.0 };
        let gleft = if lx < -thresh { -lx } else { 0.0 };
        let gright = if lx > thresh { lx } else { 0.0 };

        let gmag = (lx * lx + ly * ly).sqrt();
        if gmag > thresh {
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

        let rmag = (rx * rx + ry * ry).sqrt();
        if rmag > thresh {
            frame.set_analog("AnalogRmag", rmag.min(1.0));
            let angle = ry.atan2(rx);
            frame.set_analog(
                "AnalogRdir",
                (angle + std::f32::consts::PI) / (2.0 * std::f32::consts::PI),
            );
        }
    }

    pad_mapper.update(&frame);
}

// ---------------------------------------------------------------------------
// Mouse look
// ---------------------------------------------------------------------------

pub fn player_mouse_look_system(
    mut motion_reader: MessageReader<MouseMotion>,
    mut query: Query<(
        &mut Transform,
        &mut Fighter,
        &mut InputState,
        Option<&crate::fight::components::FighterState>,
    ), With<Player>>,
    mut camera_query: Query<&mut crate::camera::channel::CameraChannel>,
) {
    let mut total_delta = Vec2::ZERO;
    for motion in motion_reader.read() {
        total_delta += motion.delta;
    }

    if total_delta.length_squared() < 0.0001 {
        return;
    }

    for (_transform, mut fighter, mut input, fs_opt) in &mut query {
        if let Some(fs) = fs_opt {
            if fs.is_grappling() || fs.is_being_grappled() {
                input.yaw_delta = 0.0;
                input.reticle_input = Vec2::ZERO;
                continue;
            }
        }

        let yaw = -total_delta.x * MOUSE_SENSITIVITY;
        input.yaw_delta = yaw;

        if input.targeting_modifier {
            // Redirect mouse movement to reticle delta
            input.reticle_input = Vec2::new(total_delta.x, total_delta.y) * MOUSE_SENSITIVITY;
            input.reticle_input_is_absolute = false;
        } else if input.blocking {
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
    pad_settings: Res<PadTuneSettings>,
    mut players: Query<
        (&Transform, &mut InputState),
        (
            With<Player>,
            Without<crate::camera::components::CameraController>,
        ),
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
    if !pad_settings.enable_eatme {
        for (_, mut input) in &mut players {
            input.eatme_target = None;
            input.eatme_travel = None;
        }
        return;
    }

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

    let cull_range = pad_settings.enemy_auto_trac_cull_range;
    let cull_range_sq = cull_range * cull_range;
    let fudge_rad = pad_settings.enemy_auto_trac_fudge.to_radians();

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
                if dist_sq > cull_range_sq || dist_sq < 1e-4 {
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
            if dist_sq > cull_range_sq || dist_sq < 1e-4 {
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
            Option<&crate::fight::components::FighterState>,
            Option<&crate::player::components::PlayerZiplineState>,
            Option<&mut crate::mover::steering::LocomotionSteering>,
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
    time: Res<Time>,
    _start_action_writer: MessageWriter<crate::animator::StartActionMessage>,
    mut jump_writer: MessageWriter<crate::animator::JumpImpulseMessage>,
) {
    let camera_tf_opt = camera_query.iter().next();
    let dt = time.delta_secs();

    for (
        entity,
        input,
        transform,
        mut velocity,
        mut fighter,
        fsm_opt,
        anim_state_opt,
        fs_opt,
        zip_state_opt,
        mut steering_opt,
    ) in &mut query
    {
        if zip_state_opt.is_some() {
            // Locomotion is completely suspended while riding a zipline/hangline
            continue;
        }
        if let Some(fs) = fs_opt {
            if fs.is_grappling() || fs.is_being_grappled() {
                // Completely lock out movement and steering while grappling or being grappled
                velocity.x = 0.0;
                velocity.z = 0.0;
                continue;
            }
        }

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

            // Skip locomotion turning while committed to a one-shot
            // (attack/react/grapple) — those write `Fighter.facing` directly and
            // must stay instant.
            let locked = anim_state_opt.is_some_and(crate::combat::locked_movement);
            if !locked && travel.length_squared() > 0.001 {
                // The model faces local +Z, so the desired facing IS the travel
                // direction.  Rate-limit the turn toward it (frame-rate
                // independent, accel/decel + max turn rate) instead of the old
                // fixed-factor slerp, so fast camera swings don't snap-strobe.
                let desired = travel.normalize_or_zero();
                fighter.facing = match steering_opt.as_deref_mut() {
                    Some(st) => crate::mover::steering::steer_facing(fighter.facing, desired, st, dt),
                    None => desired,
                };
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

pub fn player_weapon_toggle_system(
    pad_mapper: Res<PadMapper>,
    mut start_writer: MessageWriter<crate::animator::StartActionMessage>,
    mut end_writer: MessageWriter<crate::animator::EndActionMessage>,
    query: Query<(
        Entity,
        &crate::animator::components::ActionPlayer,
        Option<&crate::inventory::components::Inventory>,
    ), With<Player>>,
) {
    if pad_mapper.get("PADCMD_WEAPON_DRAW") > 0.0 {
        for (entity, ap, inv_opt) in &query {
            // Check if player has a weapon equipped
            let has_weapon = inv_opt
                .map(|inv| inv.current_weapon.is_some())
                .unwrap_or(false);
            if has_weapon {
                if ap.is_weapon_drawn() {
                    end_writer.write(crate::animator::EndActionMessage {
                        entity,
                        action: crate::animator::MainAction::DrawWeapon,
                        subaction: crate::animator::components::sub_state_0::DRAWWEAPON_NO_TGT,
                    });
                    info!("PADCMD_WEAPON_DRAW: Requesting holster (EndAction DrawWeapon)");
                } else {
                    start_writer.write(crate::animator::StartActionMessage {
                        entity,
                        action: crate::animator::MainAction::DrawWeapon,
                        substate: crate::animator::components::sub_state_0::DRAWWEAPON_NO_TGT,
                    });
                    info!("PADCMD_WEAPON_DRAW: Requesting draw (StartAction DrawWeapon)");
                }
            }
        }
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

pub fn player_zipline_mount_system(
    mut commands: Commands,
    layout_paths: Option<Res<crate::oni2_loader::environment::LayoutPaths>>,
    mut query: Query<(
        Entity,
        &Transform,
        &InputState,
        &crate::animator::components::ActionPlayer,
        &crate::oni2_loader::animation::Oni2AnimState,
        Option<&crate::player::components::PlayerZiplineState>,
        Option<&crate::player::components::PendingZiplineMount>,
    ), With<Player>>,
    mut start_action_writer: MessageWriter<crate::animator::StartActionMessage>,
) {
    let Some(paths) = layout_paths else { return; };
    for (entity, tf, input, ap, anim_state, zip_state, pending) in &mut query {
        if zip_state.is_some() || pending.is_some() {
            continue;
        }
        // Must be jumping or in the air to mount a zipline/hangline
        let is_jumping = ap.check_flags(crate::animator::components::action_flags::JUMPING)
            || !anim_state.is_grounded
            || input.jump;

        if is_jumping {
            let player_pos = tf.translation;
            let mut closest_curve = None;
            let mut best_t = 0.0;
            let mut min_dist_xz = f32::MAX;

            for (name, pts) in &paths.curves {
                if pts.len() < 4 { continue; }
                let is_zip = name.to_lowercase().contains("zipline");
                let is_hang = name.to_lowercase().contains("hangline");
                if !is_zip && !is_hang {
                    continue;
                }
                let curve = crate::oni2_loader::curve::NurbsCurve::new(pts.clone());
                let (pt, t) = curve.nearest_point_xz(player_pos);
                let dist_xz = Vec2::new(pt.x, pt.z).distance(Vec2::new(player_pos.x, player_pos.z));
                let dist_y = (pt.y - player_pos.y).abs();

                // Proximity bounds: XZ < 1.2m, Y < 3.0m
                if dist_xz < 1.2 && dist_y < 3.0 {
                    if dist_xz < min_dist_xz {
                        min_dist_xz = dist_xz;
                        closest_curve = Some((name.clone(), is_hang));
                        best_t = t;
                    }
                }
            }

            if let Some((name, is_hang)) = closest_curve {
                info!("Grabbed zipline/hangline '{}' at phase {:.3}", name, best_t);
                commands.entity(entity).insert(crate::player::components::PendingZiplineMount {
                    curve_name: name,
                    t: best_t,
                    is_hangline: is_hang,
                });

                // Send start action message to trigger animator FSM transition to MainAction::ZipLine
                start_action_writer.write(crate::animator::StartActionMessage {
                    entity,
                    action: crate::animator::MainAction::ZipLine,
                    substate: if is_hang {
                        crate::animator::components::sub_state_0::ZIPLINE_NAV
                    } else {
                        crate::animator::components::sub_state_0::ZIPLINE_SLIDE
                    },
                });
            }
        }
    }
}

pub fn player_zipline_speed_system(
    mut query: Query<(
        &crate::player::components::PlayerZiplineState,
        &mut crate::oni2_loader::components::CurveFollower,
        &InputState,
    ), With<Player>>,
) {
    for (zip_state, mut follower, input) in &mut query {
        if zip_state.is_hangline {
            // controllable navigation along hangline
            follower.speed = -input.movement.y * 4.0;
        } else {
            // fast forward slide down zipline
            follower.speed = 12.0;
        }
    }
}

pub fn player_zipline_fsm_sync_system(
    mut query: Query<(
        &mut crate::statemachine::runtime::AnimatorRuntime,
        Option<&crate::player::components::PlayerZiplineState>,
        Option<&crate::player::components::PendingZiplineMount>,
        Option<&crate::oni2_loader::components::CurveFollower>,
    ), With<Player>>,
) {
    for (mut animator, zip_state, pending_opt, follower_opt) in &mut query {
        let mut available = false;
        if pending_opt.is_some() {
            available = true;
        } else if let Some(zip) = zip_state {
            if let Some(follower) = follower_opt {
                if zip.is_hangline {
                    if follower.phase > 0.001 && follower.phase < 0.999 {
                        available = true;
                    }
                } else {
                    if follower.phase < 0.999 {
                        available = true;
                    }
                }
            } else {
                available = true;
            }
        }
        animator.ctx.zipline_available = available;
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::fight::components::FighterState;
    use bevy::prelude::App;
    use std::collections::HashMap;
    use crate::animator::components::ActionPlayer;

    #[test]
    fn test_player_movement_lockout_during_grapple() {
        let mut app = App::new();

        app.add_message::<crate::animator::JumpImpulseMessage>();
        app.add_message::<crate::animator::StartActionMessage>();

        app.insert_resource(Time::<()>::default());

        app.add_systems(Update, player_movement_system);

        let mut fs = FighterState::default();
        fs.grapple_attacker = Some(Entity::PLACEHOLDER);

        let player = app
            .world_mut()
            .spawn((
                Player,
                InputState {
                    movement: Vec2::new(0.0, -1.0), // W key (forward)
                    ..Default::default()
                },
                Transform::from_xyz(0.0, 0.0, 0.0),
                LinearVelocity(Vec3::new(1.0, 2.0, 3.0)),
                Fighter::default(),
                fs,
            ))
            .id();

        app.update();

        // The linear velocity must be locked to 0.0 horizontally
        let vel = app.world().get::<LinearVelocity>(player).unwrap();
        assert_eq!(vel.x, 0.0);
        assert_eq!(vel.z, 0.0);
    }

    #[test]
    fn test_zipline_proximity_detection() {
        let mut app = App::new();

        app.add_message::<crate::animator::StartActionMessage>();

        let curves = vec![
            (
                "zipline1".to_string(),
                vec![
                    Vec3::new(0.0, 10.0, 0.0),
                    Vec3::new(0.0, 10.0, 5.0),
                    Vec3::new(0.0, 10.0, 10.0),
                    Vec3::new(0.0, 10.0, 15.0),
                ],
            ),
        ];
        app.insert_resource(crate::oni2_loader::environment::LayoutPaths {
            curves,
            ..Default::default()
        });

        app.add_systems(Update, player_zipline_mount_system);

        let player = app
            .world_mut()
            .spawn((
                Player,
                Transform::from_xyz(0.5, 9.0, 5.0), // XZ close (dist=0.5 < 1.2), Y close (diff=1.0 < 3.0)
                InputState {
                    jump: true,
                    ..Default::default()
                },
                ActionPlayer::default(),
                crate::oni2_loader::animation::Oni2AnimState {
                    anim: Default::default(),
                    skeleton: Default::default(),
                    current_time: 0.0,
                    fps: 30.0,
                    paused: false,
                    looping: false,
                    speed_multiplier: 1.0,
                    pending_step: 0,
                    last_rendered_time: 0.0,
                    joint_entities: vec![],
                    base_rotation: Quat::IDENTITY,
                    current_frame: vec![],
                    current_anim_id: None,
                    previous_anim_id: None,
                    anim_just_started: false,
                    root_motion_just_started: false,
                    is_grounded: false,
                    material_stood_on: None,
                    root_offset_this_frame: Vec3::ZERO,
                    root_offset_prev_frame: Vec3::ZERO,
                },
            ))
            .id();

        app.update();

        // Check that PendingZiplineMount was inserted
        let pending = app.world().get::<crate::player::components::PendingZiplineMount>(player);
        assert!(pending.is_some(), "PendingZiplineMount should have been inserted");
        let pending = pending.unwrap();
        assert_eq!(pending.curve_name, "zipline1");
        assert!(!pending.is_hangline);
    }

    #[test]
    fn test_zipline_dismount_teleport() {
        use avian3d::prelude::{LinearVelocity, RigidBody};
        let mut app = App::new();

        app.add_message::<crate::statemachine::runtime::AnimatorBroadcastEvent>();
        app.add_message::<crate::animator::StartActionMessage>();

        app.add_systems(Update, crate::animator::systems::animator_broadcast_handler);

        let initial_pos = Vec3::new(0.0, 10.0, 0.0);
        let player = app
            .world_mut()
            .spawn((
                Player,
                Transform::from_translation(initial_pos),
                ActionPlayer::default(),
                LinearVelocity(Vec3::new(5.0, 0.0, 5.0)),
                RigidBody::Kinematic,
            ))
            .id();

        // Send EndZipline event
        let mut writer = app
            .world_mut()
            .resource_mut::<Messages<crate::statemachine::runtime::AnimatorBroadcastEvent>>();
        writer.write(crate::statemachine::runtime::AnimatorBroadcastEvent {
            entity: player,
            payload: "EndZipline".to_string(),
        });

        app.update();

        // Verify entity state after update:
        // 1. RigidBody component should be restored to RigidBody::Dynamic.
        // 2. LinearVelocity should be reset to zero.
        // 3. Transform should be teleported downward by 2.0 meters.
        let rb = app.world().get::<RigidBody>(player).unwrap();
        assert_eq!(*rb, RigidBody::Dynamic);

        let vel = app.world().get::<LinearVelocity>(player).unwrap();
        assert_eq!(vel.0, Vec3::ZERO);

        let tf = app.world().get::<Transform>(player).unwrap();
        assert_eq!(tf.translation, initial_pos - Vec3::Y * 2.0);
    }
}

