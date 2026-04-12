use bevy::prelude::*;

/// Marker component for the player entity.
#[derive(Component)]
pub struct Player;

/// Logical per-player input state.
///
/// This is the *only* place game logic reads input — it is populated each frame
/// by whichever input backend is active (keyboard/mouse or gamepad).
/// Adding a new input source means writing a system that fills these fields;
/// nothing else needs to change.
#[derive(Component, Default, Clone)]
pub struct InputState {
    /// Normalised movement vector.  x: left (+) / right (−),  y: forward (−) / back (+).
    pub movement: Vec2,
    /// Camera yaw delta this frame (radians).  Positive = rotate right.
    pub yaw_delta: f32,

    // ── just-triggered actions (true for exactly one frame) ──
    /// Primary attack (Space or left mouse or gamepad face button).
    pub attack: bool,
    /// Secondary attack (right mouse or gamepad alternate face button).
    pub attack_two: bool,
    /// Grab / grapple initiation.
    pub grab: bool,
    /// Jump.
    pub jump: bool,
    /// Evade / dodge roll.
    pub evade: bool,

    // ── held actions ──
    /// Block / strafe modifier.
    pub blocking: bool,
}
