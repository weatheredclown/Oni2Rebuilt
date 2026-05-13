/*
 * fight/events.rs — Message types for the high-fidelity fight system.
 *
 * GrappleStartEvent    — grapple initiation confirmed
 * GrappleEndEvent      — grapple ended (throw / break / die / timeout)
 * BlockSuccessEvent    — a defender's block deflected an attack
 * BlockFailedEvent     — a block attempt did not deflect the attack
 * ApplyRotationNotchesEvent — rotate entity by N × 45° on the Y axis
 * SuperMeterAddEvent   — add/subtract from an entity's super meter
 */
use bevy::prelude::*;

// ---------------------------------------------------------------------------
// Grapple
// ---------------------------------------------------------------------------

/// Fired when a grapple initiation animation succeeds and the holder has
/// grabbed the target (mirrors the point where crGrab::Start() is called).
#[derive(Message, Clone)]
pub struct GrappleStartEvent {
    /// The entity that initiated the grapple.
    pub attacker: Entity,
    /// The entity being grappled.
    pub target: Entity,
    /// World-space position of the target at grab time.
    pub target_pos: Vec3,
}

/// Fired when a grapple ends for any reason.
#[derive(Message, Clone)]
pub struct GrappleEndEvent {
    pub attacker: Entity,
    pub target: Option<Entity>,
    pub reason: GrappleEndReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrappleEndReason {
    /// Holder executed a throw attack.
    Throw,
    /// Target or holder broke out (shake / damage threshold).
    Break,
    /// One participant died during the grapple.
    Die,
    /// Grapple exceeded FighterType.grapple_break_time.
    Timeout,
    /// Manually ended (FSM End action).
    Manual,
}

// ---------------------------------------------------------------------------
// Block
// ---------------------------------------------------------------------------

/// Fired by hit_detection_system when a defender's block successfully
/// deflects an incoming strike.  The fight block system responds by playing
/// counter-attacks and block-break reactions.
#[derive(Message, Clone)]
pub struct BlockSuccessEvent {
    /// Entity whose attack was blocked.
    pub attacker: Entity,
    /// Entity doing the blocking.
    pub blocker: Entity,
    /// Index of the BlockDef that was active (into BlockLibrary.blocks).
    pub block_index: i32,
    /// Counter-attack animation alias to play on the blocker (only used
    /// when the defender's BlockDef has `auto_counter` set).
    pub counter_atk: Option<String>,
    /// `auto_counter` flag from the defender's BlockDef — gates whether
    /// `counter_atk` actually fires this tick.
    pub auto_counter: bool,
    /// Successful-block anim name from the defender's BlockDef — the
    /// secondary anim to play on the blocker when the block succeeds
    /// (mirrors `crBlockData::SuccessfulBlockAnim`).
    pub successful_block_anim: Option<String>,
    /// animBlockEnum to play as secondary block response on the blocker.
    /// Sourced from the **attacker's** ATDT (`AtdtData::enemy_block_anim`),
    /// not from the defender's BlockDef — mirrors
    /// `GetAttackData()->EnemyBlockAnim` in `crStrike::GetBlocked`.
    pub enemy_block_anim: i32,
    /// animReactEnum to force on the *attacker* (block-break reaction).
    /// Sourced from the **attacker's** ATDT (`AtdtData::block_reaction`)
    /// — mirrors `GetAttackData()->BlockReaction` in `crStrike::GetBlocked`.
    pub block_reaction_on_attacker: i32,
    /// Number of consecutive hits the blocker has taken this block session.
    pub block_combo_count: i32,
    /// Threshold before combo pressure forces a block-break react on blocker.
    pub combo_count_before_react: i32,
}

/// Fired when a block attempt fails — angle, phase, or hit type outside range.
#[derive(Message, Clone)]
pub struct BlockFailedEvent {
    /// Entity whose attack was not blocked.
    pub attacker: Entity,
    /// Entity that attempted to block.
    pub blocker: Entity,
    /// animReactEnum to play on the blocker for the failed block.
    pub failed_react: i32,
}

// ---------------------------------------------------------------------------
// Rotation
// ---------------------------------------------------------------------------

/// Request to apply N rotation notches to an entity's Y-axis.
/// 1 notch = PI/4 radians (45°).  Positive = clockwise from above.
/// Processed by rotation_notches_system.
#[derive(Message, Clone)]
pub struct ApplyRotationNotchesEvent {
    pub entity: Entity,
    /// Signed notch count (positive = CW, negative = CCW).
    pub notches: i32,
}

// ---------------------------------------------------------------------------
// Super meter
// ---------------------------------------------------------------------------

/// Add (positive) or remove (negative) from an entity's super meter.
/// Processed by super_meter_system.
#[derive(Message, Clone)]
pub struct SuperMeterAddEvent {
    pub entity: Entity,
    /// Amount to add; negative values drain the meter.
    pub amount: f32,
}
