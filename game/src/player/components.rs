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

    // ── EATME (Enemy Auto-Track Mistake Eliminator) outputs ──
    /// Enemy currently picked by the EATME magnetism pass.  Set when an
    /// enemy is within the auto-track cull range and (if the stick is
    /// active) within fudge° of the pad-input direction.  Mirrors
    /// `bhPadMapper::EATMETarget` (rb/src/behavior/padmapper.h:338) —
    /// downstream consumers use it to preselect a strike target the
    /// instant attack is pressed, and to drive Z-lock acquisition.
    pub eatme_target: Option<Entity>,
    /// World-space XZ travel direction with EATME snap applied — when
    /// the stick is pushed roughly toward an enemy, this points
    /// straight at them instead of the raw stick direction.  `None`
    /// when EATME didn't engage (stick neutral, no enemies in range,
    /// or angle outside fudge°); `player_movement_system` falls back
    /// to the raw camera-relative computation in that case.  Mirrors
    /// `bhPadMapper::ChrRelativeDir` (padmapper.h:329).
    pub eatme_travel: Option<Vec3>,
}
