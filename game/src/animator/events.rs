/*
 * animator/events.rs — Message types for the animator subsystem.
 *
 * StartActionMessage       — request a MainAction (jump, react, evade, …).
 * EndActionMessage         — end a running MainAction.
 * ControlAnimMessage       — stop/pause/loop/set-rate on a playing animation
 *                            (mirrors animMsgControlAnim).
 * HeadIkModeMessage        — update an ActionPlayer.head_ik_mode
 *                            (mirrors animMsgHeadIK).
 * PlayReactMessage         — play a specific animReactEnum (short-cut for the
 *                            common "play reaction animation" use-case).
 * ActionStartedMessage /
 * ActionEndedMessage       — outbound notifications from the animator — emitted
 *                            by action_start_system / action_end_system so other
 *                            subsystems (fight / AI / FSM) can observe.
 */
use bevy::prelude::*;

use super::components::{ActionResult, HeadIkMode, MainAction};

// ---------------------------------------------------------------------------
// Action dispatch
// ---------------------------------------------------------------------------

/// Request a MainAction.  Processed by `action_start_system`, which checks
/// reject / override lists and plays the appropriate animation.  An
/// `ActionStartedMessage` is emitted back with the ActionResult.
#[derive(Message, Clone)]
pub struct StartActionMessage {
    pub entity: Entity,
    pub action: MainAction,
    /// ACT_S0_* integer constant (see components::sub_state_0).  Use -1 for none.
    pub substate: i32,
}

/// End a running MainAction.  Processed by `action_end_system`.
#[derive(Message, Clone)]
pub struct EndActionMessage {
    pub entity: Entity,
    pub action: MainAction,
    /// Adverb for EndAction (e.g. end_adverb::LIE_2STAND).  Use -1 for none.
    pub subaction: i32,
}

/// Outbound: fired by `action_start_system` for every processed
/// `StartActionMessage`, regardless of result.
#[derive(Message, Clone)]
pub struct ActionStartedMessage {
    pub entity: Entity,
    pub action: MainAction,
    pub substate: i32,
    pub result: ActionResult,
}

/// Outbound: fired by `action_end_system`.
#[derive(Message, Clone)]
pub struct ActionEndedMessage {
    pub entity: Entity,
    pub action: MainAction,
    pub subaction: i32,
}

// ---------------------------------------------------------------------------
// Control-animation message (mirrors animMsgControlAnim)
// ---------------------------------------------------------------------------

/// Compose multiple bits to stop/pause/restart/loop/hold a playing animation.
/// The `animation_alias` field, if set, will re-play that animation before the
/// bits are applied (useful for scripts that want to restart with a new rate).
#[derive(Message, Clone)]
pub struct ControlAnimMessage {
    pub entity: Entity,
    /// Optional alias to play.  If None, the currently playing animation is used.
    pub animation_alias: Option<String>,
    pub control: u32,
    pub rate: f32,
    pub loop_anim: bool,
    pub hold: bool,
    pub pause: bool,
}

pub mod control_anim_bits {
    pub const NO_ACTION: u32 = 0x00;
    pub const RESTART: u32 = 0x01;
    pub const STOP: u32 = 0x02;
    pub const PAUSE: u32 = 0x04;
    pub const LOOP: u32 = 0x08;
    pub const SET_RATE: u32 = 0x10;
    pub const HOLD: u32 = 0x20;
    pub const PAUSE_AT_END: u32 = 0x40;
}

// ---------------------------------------------------------------------------
// Head-IK mode message (mirrors animMsgHeadIK)
// ---------------------------------------------------------------------------

/// Replace the `ActionPlayer.head_ik_mode` for an entity.
#[derive(Message, Clone)]
pub struct HeadIkModeMessage {
    pub entity: Entity,
    pub mode: HeadIkMode,
}

// ---------------------------------------------------------------------------
// Convenience messages
// ---------------------------------------------------------------------------

/// Short-cut for the common "play a react animation" use-case.  Equivalent to
/// StartActionMessage { action: React, substate: react_enum } but keeps the
/// animReactEnum out-of-band from the ACT_S0_* integer space.
#[derive(Message, Clone)]
pub struct PlayReactMessage {
    pub entity: Entity,
    /// animReactEnum integer (indexes into ANIMREACT_NAMES).
    pub react_enum: i32,
    pub disable_creature_detect: bool,
}

/// Short-cut for "play a die animation" — same relationship to StartAction(Die, …).
#[derive(Message, Clone)]
pub struct PlayDieMessage {
    pub entity: Entity,
    /// animReactEnum / animDieEnum integer.
    pub die_enum: i32,
}

// ---------------------------------------------------------------------------
// SetPickupMatrixMessage / DropMessage
// ---------------------------------------------------------------------------

/// Reparent an entity visually to a carrier (claw, crane, ledge grab arm).
/// The sender provides the carrier's current world-space matrix; the receiver
/// is expected to pose itself to match for the duration of the pickup.
///
/// Mirrors legacy `animMsgSetPickupMatrix` (animator/messages.h) — emitted by the
/// elbow-crane IK and consumed by `animElbowCraneHitchComponent::HandlePickupMatrix`
/// in the original game.  In the shipped ONI2 asset set, the only sender is
/// the boss-fight elbow-crane; consumers are whichever entity is being
/// grabbed.  Ports of mechanic-specific IK hitching should listen to this.
#[derive(Message, Clone)]
pub struct SetPickupMatrixMessage {
    /// Entity being carried / picked up.
    pub entity: Entity,
    /// Carrier's current world transform.  The receiver should set its own
    /// global transform to match (optionally with a bone offset).
    pub claw_translation: Vec3,
    /// Carrier's current world rotation.
    pub claw_rotation: Quat,
}

/// Release an entity from a pickup / hitch mode back to normal physics.
/// Typically paired with a previously-sent `SetPickupMatrixMessage`.
///
/// Mirrors legacy `animMsgDrop`.  Empty payload beyond the entity — the receiver
/// resets its IK / reparenting state.
#[derive(Message, Clone)]
pub struct DropMessage {
    pub entity: Entity,
}

// ---------------------------------------------------------------------------
// JumpImpulseMessage
// ---------------------------------------------------------------------------

/// Fired by `action_player_tick_system` when an entity's SubState1 transitions
/// into JUMP_MAIN or JUMP_SOMERSAULT — the moment the actual jump impulse
/// should be applied to the physics body (mirrors crMover::UsePacketJump).
///
/// The player / AI mover systems listen for this and apply the impulse to
/// their LinearVelocity.  Values are derived from the entity's JumpController
/// entry for the selected jump_index.
#[derive(Message, Clone)]
pub struct JumpImpulseMessage {
    pub entity: Entity,
    /// Vertical impulse in m/s (derived from jump height × effective gravity).
    pub vertical: f32,
    /// Horizontal forward impulse in m/s (derived from jump length / time).
    /// Positive = forward along the entity's facing.
    pub forward: f32,
    /// Horizontal lateral impulse in m/s (for strafe evades).
    /// Positive = to the entity's right.
    pub lateral: f32,
    /// Effective gravity multiplier for this jump (applied downstream by a
    /// custom-gravity system if present; otherwise informational).
    pub gravity_factor: f32,
    /// Duration of the jump trajectory in seconds — useful for rate-matching
    /// the animation playback to the physics arc.
    pub time: f32,
    /// Optional heading adjustment in degrees (evades apply ±90°).
    pub heading_adjust: f32,
}
