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
    /// "Yield the cookie so the next queued attacker can swing."
    ///
    /// Despite the name, this fires on the CURRENT attacker — not the
    /// next one.  When this is true the fight FSM's S_ATTACK state
    /// runs `A_RELEASE_COOKIE` and transitions to S_PREPARE_NEXT_ATTACKER,
    /// which then waits for `E_ATTACK_FINISHED` before returning to
    /// S_STARTING_FIGHT for another loop.  Without this firing, an
    /// attacker that grabbed the cookie holds it forever and the
    /// other queued attackers never get offered the cookie — combat
    /// degenerates to one fighter swinging in a tight loop.
    ///
    /// Legacy formula (rb/src/aifight/fighter.cpp:2797):
    /// ```cpp
    /// bool aiFighter::PrepareNextAttacker() const {
    ///     return !IsAttacking() || AttackStateMachine->IsDelayingCookie();
    /// }
    /// ```
    /// I.e. yield when EITHER (a) our swing animation has ended, OR
    /// (b) the inner attack-FSM voluntarily set its `FLAG_DELAYED_COOKIE`
    /// bit (a script-level "let someone else attack" hint emitted by
    /// `.atk` action steps that want to chain via a peer instead of
    /// re-grabbing the cookie themselves).
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
    /// Drives `E_PREPARE_NEXT_ATTACKER` — see the variant doc on
    /// `FightEvent::PrepareNextAttacker` for the full picture.
    ///
    /// **Currently always false** because no host system writes it.
    /// The fight FSM still functions: in S_ATTACK the fall-through
    /// rule fires `A_ATK_ATTACK` every tick, and the inner
    /// `AttackRuntime` ends the swing when its anim completes.  The
    /// downside is that the cookie is held until something external
    /// forces a release, so multi-attacker rotation around a single
    /// target doesn't time-share the way the legacy did.
    ///
    /// To wire this properly, populate from `fight_runtime_update_system`
    /// with the legacy formula `!is_attacking || is_delaying_cookie`:
    ///   - `!is_attacking` ≈ `ctx.attack_finished` (we already compute
    ///     this from `Oni2AnimState::current_time` reaching the end).
    ///   - `is_delaying_cookie` needs a new bool on `AttackCtx`
    ///     (`delaying_cookie: bool`) flipped on by an `.atk` action
    ///     step like `delay_cookie` / `yield_cookie` (legacy
    ///     `aiAtkAction` subclasses set `FLAG_DELAYED_COOKIE` on the
    ///     attack state machine).  No such action is parsed yet, so
    ///     until that lands the formula collapses to `attack_finished`,
    ///     which is a reasonable first cut: yields the cookie on swing
    ///     end so the next queued attacker can grab it.
    ///
    /// One additional legacy nuance the formula above doesn't capture:
    /// `aiAttackStateMachine::Update` (rb/src/aifight/
    /// attackstatemachine.cpp:399) ALSO force-yields the cookie when
    /// a fighter has held it longer than `FIGHTMGR.CookieHogTime`
    /// without swinging.  That's a watchdog timer separate from this
    /// flag — port it as a duration check on `FightResources` rather
    /// than threading it through here.
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
            FightEvent::HasPosition => {
                bevy::log::trace!(
                    "FightDriver eval_event: E_HAS_POSITION -> {}",
                    ctx.has_position
                );
                ctx.has_position
            }
            FightEvent::CanAttack => ctx.can_attack,
            FightEvent::TargetKilled => ctx.target_killed,
            FightEvent::Timer(t) => runtime.elapsed - runtime.timer_start >= *t,
            FightEvent::PositionOffered => ctx.position_offered,
            FightEvent::CookieOffered => ctx.cookie_offered,
            FightEvent::AttackFinished => ctx.attack_finished,
            FightEvent::PrepareNextAttacker => ctx.prepare_next_attacker,
            FightEvent::Attacked => ctx.attacked,
            FightEvent::Mode(m) => {
                let ctx_lower = ctx.mode.to_ascii_lowercase();
                let result = m
                    .split(',')
                    .map(|s| s.trim().to_ascii_lowercase())
                    .any(|m_lower| {
                        if (m_lower == "fight" || m_lower == "attack")
                            && (ctx_lower == "fight" || ctx_lower == "attack")
                        {
                            true
                        } else {
                            ctx_lower == m_lower
                        }
                    });
                bevy::log::trace!(
                    "FightDriver eval_event: E_MODE({}) against ctx.mode='{}' -> {}",
                    m,
                    ctx.mode,
                    result
                );
                result
            }
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
            FightAction::Display(msg) => bevy::log::trace!("fight FSM: {}", msg),
            other => {
                bevy::log::trace!("fight FSM requested: {:?}", other);
                output.requested_actions.push(other.clone());
            }
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
        "E_HAS_TARGET" => FightEvent::HasTarget,
        "E_REACTING" => FightEvent::Reacting,
        "E_HAS_POSITION" => FightEvent::HasPosition,
        "E_CAN_ATTACK" => FightEvent::CanAttack,
        "E_TARGET_KILLED" => FightEvent::TargetKilled,
        "E_TIMER" => FightEvent::Timer(args.parse::<f32>().unwrap_or(1.0)),
        "E_POSITION_OFFERED" => FightEvent::PositionOffered,
        "E_COOKIE_OFFERED" => FightEvent::CookieOffered,
        "E_ATTACK_FINISHED" => FightEvent::AttackFinished,
        "E_PREPARE_NEXT_ATTACKER" => FightEvent::PrepareNextAttacker,
        "E_ATTACKED" => FightEvent::Attacked,
        "E_MODE" => FightEvent::Mode(args.trim_matches('"').to_string()),
        "E_ALWAYS" | "" => FightEvent::Always,
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
        "A_IDLE" => FightAction::Idle,
        "A_ATK_IDLE" => FightAction::AtkIdle,
        "A_ATK_ATTACK" => FightAction::AtkAttack,
        "A_MOVE_TO_POSITION" => FightAction::MoveToPosition,
        "A_REQUEST_POSITION" => FightAction::RequestPosition,
        "A_GRAB_POSITION" => FightAction::GrabPosition,
        "A_UPGRADE_POSITION" => FightAction::UpgradePosition,
        "A_UPGRADE_POSITION_IN_FRONT" => FightAction::UpgradePositionInFront,
        "A_UPGRADE_POSITION_BEHIND" => FightAction::UpgradePositionBehind,
        "A_UPGRADE_POSITION_LEFT" => FightAction::UpgradePositionLeft,
        "A_UPGRADE_POSITION_RIGHT" => FightAction::UpgradePositionRight,
        "A_RELEASE_POSITION" => FightAction::ReleasePosition,
        "A_REQUEST_COOKIE" => FightAction::RequestCookie,
        "A_GRAB_COOKIE" => FightAction::GrabCookie,
        "A_RELEASE_COOKIE" => FightAction::ReleaseCookie,
        "A_ATTACK" => FightAction::Attack,
        "A_PARRY" => FightAction::Parry,
        "A_JOIN_FORMATION" => FightAction::JoinFormation,
        "A_LEAVE_FORMATION" => FightAction::LeaveFormation,
        "A_RESET_TIMER" => FightAction::ResetTimer,
        "Display" => FightAction::Display(arg.to_string()), // Retaining Display as an internal engine debug tool
        "Check" => match state_index.get(arg) {
            Some(&idx) => FightAction::Check(idx),
            None => return Ok(None),
        },
        _ => return Ok(None),
    }))
}

pub const FIGHT_EVENT_PARSER: EventParser<FightDriver> = parse_fight_event;
pub const FIGHT_ACTION_PARSER: ActionParser<FightDriver> = parse_fight_action;
