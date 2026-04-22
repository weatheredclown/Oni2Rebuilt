/*
 * player/components.rs — player component types.
 *
 * Player marker, InputState (movement Vec2, attack/jump/evade/block bools,
 * yaw_delta), and any other per-player data that input backends write and
 * movement / combat systems read.
 */
use bevy::prelude::*;

/// Resource used to ignore spurious hardware input clicks (like Menu selection) immediately after loading a level.
#[derive(Resource, Default)]
pub struct MenuTransitionInputCushion(pub f32);

/// Marker component for the player entity.
#[derive(Component)]
pub struct Player;

/// Which input state machine file to attach when the player (or any pad-driven
/// actor) spawns.  Value is the bare FSM name — no `.fsm` suffix.  Mirrors the
/// legacy `bhPadTuningData::FSM` / `SUB_ATTRIBUTE(Pad, FSM)` pipeline.
#[derive(Component, Clone)]
pub struct PadFsmName(pub String);

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
