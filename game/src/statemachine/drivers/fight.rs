/*
 * statemachine/drivers/fight.rs — FightDriver: cooperative AI fight scheduler.
 *
 * Implements the cooperative AI fight scheduler. This
 * machine runs once per AI fighter and decides high-level behavior — when to
 * occupy a strike position, when to throw an attack, when to back off, when
 * to coordinate with formation peers via the position/cookie system.
 *
 * Vocabulary parity with legacy implementation:
 *   Events  — EHasTarget, EReacting, EHasPosition, ECanAttack, ETargetKilled,
 *             ETimer, EPositionOffered, ECookieOffered, EAttackFinished,
 *             EPrepareNextAttacker, EAttacked, EMode, EAlways
 *   Actions — AIdle, AAtkIdle, AAtkAttack, AMoveToPosition, ARequestPosition,
 *             AGrabPosition, AUpgradePosition (+InFront/Behind/Left/Right),
 *             AReleasePosition, ARequestCookie, AGrabCookie, AReleaseCookie,
 *             AAttack, AParry, AJoinFormation, ALeaveFormation, AResetTimer
 *
 * Most eval_event branches and apply_action branches are STUBBED with `false`
 * or no-op writes to `FightOutput.requested_actions` until the supporting
 * coordinator (`aifight::position`, `aifight::cookie`, `aifight::formation`)
 * is ported.  The shape is finalized so once the support code lands we only
 * need to fill in branches — no signature changes.
 */
use std::collections::HashMap;

use super::super::core::{SmAdvance, SmDriver, SmRuntime};
use super::parse::{ActionParser, EventParser, split_call};

// ---------------------------------------------------------------------------
// FightDriver vocabulary
// ---------------------------------------------------------------------------

/// One high-level fight FSM event.  Maps 1:1 onto the legacy aiFightStateMachine
/// `EventType` enum.
#[derive(Clone, Debug)]
pub enum FightEvent {
    /// A current strike-target exists.
    HasTarget,
    /// We are currently in a react animation.
    Reacting,
    /// We have an assigned attack position around the target.
    HasPosition,
    /// All upstream conditions for throwing an attack are satisfied.
    CanAttack,
    /// Our current target was killed this tick.
    TargetKilled,
    /// At least N seconds have elapsed since the last `AResetTimer`.
    Timer(f32),
    /// Another fighter has offered us their attack position (handoff).
    PositionOffered,
    /// Another fighter has offered us the attack-cookie (slot to swing).
    CookieOffered,
    /// The attack we were running just finished.
    AttackFinished,
    /// Coordinator asked us to step up as the next attacker.
    PrepareNextAttacker,
    /// We were just hit (attacked) by the target.
    Attacked,
    /// Enum-tagged behavior mode (string compared to AI mode field).
    Mode(String),
    /// Always true.
    Always,
}

/// Side-effect requested by an `apply_action` branch.  The host system reads
/// these out of `FightOutput.requested_actions` after each tick and dispatches
/// them — we don't mutate the world directly from inside the FSM.
#[derive(Clone, Debug)]
pub enum FightAction {
    Idle,
    AtkIdle,
    AtkAttack,
    MoveToPosition,
    RequestPosition,
    GrabPosition,
    UpgradePosition,
    UpgradePositionInFront,
    UpgradePositionBehind,
    UpgradePositionLeft,
    UpgradePositionRight,
    ReleasePosition,
    RequestCookie,
    GrabCookie,
    ReleaseCookie,
    Attack,
    Parry,
    JoinFormation,
    LeaveFormation,
    ResetTimer,
    /// Inline-evaluate another state's rules without changing the cursor.
    Check(usize),
    /// Diagnostic.
    Display(String),
}

/// Per-tick context passed into the FightDriver tick.  Until the supporting
/// systems are ported, most fields are placeholders the host builds from
/// whatever AI signal is already available (or leaves at default).
#[derive(Default)]
pub struct FightCtx {
    pub has_target: bool,
    pub is_reacting: bool,
    pub has_position: bool,
    pub can_attack: bool,
    pub target_killed: bool,
    pub position_offered: bool,
    pub cookie_offered: bool,
    pub attack_finished: bool,
    pub prepare_next_attacker: bool,
    pub attacked: bool,
    /// String mode tag used by `EMode("foo")` events.
    pub mode: String,
}

/// Output accumulator filled during a tick — the host drains
/// `requested_actions` and runs the corresponding aiFighter handlers.
#[derive(Default)]
pub struct FightOutput {
    /// Ordered list of actions the FSM asked the host to perform this tick.
    pub requested_actions: Vec<FightAction>,
}

// ---------------------------------------------------------------------------
// SmDriver impl
// ---------------------------------------------------------------------------

pub struct FightDriver;

impl SmDriver for FightDriver {
    type Event = FightEvent;
    type Action = FightAction;
    type Context = FightCtx;
    type Output = FightOutput;

    fn eval_event(ctx: &Self::Context, event: &Self::Event, runtime: &SmRuntime<Self>) -> bool {
        match event {
            FightEvent::HasTarget => ctx.has_target,
            FightEvent::Reacting => ctx.is_reacting,
            FightEvent::HasPosition => ctx.has_position,
            FightEvent::CanAttack => ctx.can_attack,
            FightEvent::TargetKilled => ctx.target_killed,
            FightEvent::Timer(t) => runtime.elapsed - runtime.timer_start >= *t,
            FightEvent::PositionOffered => ctx.position_offered,
            FightEvent::CookieOffered => ctx.cookie_offered,
            FightEvent::AttackFinished => ctx.attack_finished,
            FightEvent::PrepareNextAttacker => ctx.prepare_next_attacker,
            FightEvent::Attacked => ctx.attacked,
            FightEvent::Mode(m) => ctx.mode.eq_ignore_ascii_case(m),
            FightEvent::Always => true,
        }
    }

    fn apply_action(
        _ctx: &mut Self::Context,
        action: &Self::Action,
        output: &mut Self::Output,
        adv: &mut SmAdvance<Self>,
    ) {
        // Most actions just queue their request into the output buffer.  Once
        // the supporting aiFighter coordinator lands, the host system reading
        // `output.requested_actions` will dispatch them — no FSM-side change
        // is needed.
        match action {
            FightAction::Check(idx) => adv.check(*idx),
            FightAction::Display(msg) => bevy::log::info!("fight FSM: {}", msg),
            other => output.requested_actions.push(other.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Text parsers
// ---------------------------------------------------------------------------

pub fn parse_fight_event(
    text: &str,
    _state_index: &HashMap<String, usize>,
) -> Result<FightEvent, String> {
    let text = text.trim();
    let (name, args) = split_call(text);

    Ok(match name {
        "EHasTarget" | "HasTarget" => FightEvent::HasTarget,
        "EReacting" | "Reacting" => FightEvent::Reacting,
        "EHasPosition" | "HasPosition" => FightEvent::HasPosition,
        "ECanAttack" | "CanAttack" => FightEvent::CanAttack,
        "ETargetKilled" | "TargetKilled" => FightEvent::TargetKilled,
        "ETimer" | "Timer" => FightEvent::Timer(args.parse::<f32>().unwrap_or(1.0)),
        "EPositionOffered" | "PositionOffered" => FightEvent::PositionOffered,
        "ECookieOffered" | "CookieOffered" => FightEvent::CookieOffered,
        "EAttackFinished" | "AttackFinished" => FightEvent::AttackFinished,
        "EPrepareNextAttacker" | "PrepareNextAttacker" => FightEvent::PrepareNextAttacker,
        "EAttacked" | "Attacked" => FightEvent::Attacked,
        "EMode" | "Mode" => FightEvent::Mode(args.trim_matches('"').to_string()),
        "EAlways" | "Always" | "True" => FightEvent::Always,
        other => return Err(format!("fight: unknown event '{}'", other)),
    })
}

pub fn parse_fight_action(
    line: &str,
    state_index: &HashMap<String, usize>,
) -> Result<Option<FightAction>, String> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("").trim();
    let arg = parts.next().unwrap_or("").trim();

    Ok(Some(match verb {
        "AIdle" | "Idle" => FightAction::Idle,
        "AAtkIdle" | "AtkIdle" => FightAction::AtkIdle,
        "AAtkAttack" | "AtkAttack" => FightAction::AtkAttack,
        "AMoveToPosition" | "MoveToPosition" => FightAction::MoveToPosition,
        "ARequestPosition" | "RequestPosition" => FightAction::RequestPosition,
        "AGrabPosition" | "GrabPosition" => FightAction::GrabPosition,
        "AUpgradePosition" | "UpgradePosition" => FightAction::UpgradePosition,
        "AUpgradePositionInFront" | "UpgradePositionInFront" => FightAction::UpgradePositionInFront,
        "AUpgradePositionBehind" | "UpgradePositionBehind" => FightAction::UpgradePositionBehind,
        "AUpgradePositionLeft" | "UpgradePositionLeft" => FightAction::UpgradePositionLeft,
        "AUpgradePositionRight" | "UpgradePositionRight" => FightAction::UpgradePositionRight,
        "AReleasePosition" | "ReleasePosition" => FightAction::ReleasePosition,
        "ARequestCookie" | "RequestCookie" => FightAction::RequestCookie,
        "AGrabCookie" | "GrabCookie" => FightAction::GrabCookie,
        "AReleaseCookie" | "ReleaseCookie" => FightAction::ReleaseCookie,
        "AAttack" | "Attack" => FightAction::Attack,
        "AParry" | "Parry" => FightAction::Parry,
        "AJoinFormation" | "JoinFormation" => FightAction::JoinFormation,
        "ALeaveFormation" | "LeaveFormation" => FightAction::LeaveFormation,
        "AResetTimer" | "ResetTimer" => FightAction::ResetTimer,
        "Display" => FightAction::Display(arg.to_string()),
        "Check" => match state_index.get(arg) {
            Some(&idx) => FightAction::Check(idx),
            None => return Ok(None),
        },
        _ => return Ok(None),
    }))
}

pub const FIGHT_EVENT_PARSER: EventParser<FightDriver> = parse_fight_event;
pub const FIGHT_ACTION_PARSER: ActionParser<FightDriver> = parse_fight_action;
