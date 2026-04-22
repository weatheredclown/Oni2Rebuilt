/*
 * statemachine/drivers/attack.rs — AttackDriver: per-attack action scheduler.
 *
 * Implements the per-attack action scheduler.
 * Each loaded `.atk` file becomes one `SmData<AttackDriver>` parsed via the
 * shared `parse_sm` skeleton.
 *
 * The defining feature of this driver — and the reason the base class needed
 * the `update_running` hook — is that ACTIONS ARE STATEFUL.  The legacy implementation uses
 * `aiAtkAction` polymorphism: each action has Start/Update/End/Finished
 * lifecycle methods.  We mirror that with the `AtkAction` trait and a pool
 * indexed by `AAction(idx)`.
 *
 * Lifecycle, per tick:
 *   1. update_running(ctx, output) ticks the currently-running action via
 *      its `update()` virtual.  When `update()` returns true, the action
 *      transitions to "finished" (and `end()` is called next tick or on
 *      replacement).
 *   2. Rule evaluation begins normally.  When apply_action fires:
 *        AAction(idx) → end the current action if any; start the new one.
 *                       If start() returns false, set adv.fail() so the rule
 *                       falls through to the next.
 *        AResume      → re-run the current action's start() without ending it.
 *        AFail        → adv.fail()
 *        AFinish      → end the current action and clear it.
 *
 * Almost every concrete `AtkAction` (turn, run, swing, projectile, vfx, sfx,
 * grapple) is unbuilt today.  This file ships the framework + a single
 * placeholder `LogAction` so .atk files referencing actions by name can still
 * load and step deterministically.
 */
use std::collections::HashMap;

use super::super::core::{SmAdvance, SmDriver, SmRuntime};
use super::parse::{split_call, ActionParser, EventParser};

// ---------------------------------------------------------------------------
// AtkAction trait — the polymorphic per-attack behavior unit
// ---------------------------------------------------------------------------

/// The `aiAtkAction` virtual interface.  Implementors live in the action pool
/// inside `AttackCtx::actions` and are referenced by index from `AAction(n)`.
///
/// Lifecycle contract (matches original Oni2):
///   • `start` is called when the action is selected.  Returning `false`
///     reports a soft failure — the state machine treats this as `AFail`.
///   • `update` is called each tick while the action is current.  Returning
///     `true` means "I'm done"; the runtime then calls `end` and looks for a
///     `Finished` rule on the next tick.
///   • `end` is the cleanup hook.  Always paired with a successful `start`.
///   • `is_event(name)` is queried by `EEvent("foo")` rules; the action can
///     emit named events (e.g. "swingstart", "release") at script-defined
///     phases of its execution.
pub trait AtkAction: Send + Sync {
    fn name(&self) -> &str;
    fn start(&mut self, _ctx: &mut AttackCtx) -> bool {
        true
    }
    fn update(&mut self, _ctx: &mut AttackCtx, _output: &mut AttackOutput, _dt: f32) -> bool {
        true // default: complete in one tick
    }
    fn end(&mut self, _ctx: &mut AttackCtx) {}
    fn can_be_aborted(&self) -> bool {
        true
    }
    fn is_event(&self, _name: &str) -> bool {
        false
    }
}

// Each action from legacy Oni is stubbed out.
macro_rules! atk_action_stub {
    ($struct_name:ident, $log_name:expr) => {
        pub struct $struct_name {
            pub args: String,
        }
        impl AtkAction for $struct_name {
            fn name(&self) -> &str {
                $log_name
            }
            fn start(&mut self, _ctx: &mut AttackCtx) -> bool {
                bevy::log::warn!("atk: {} is not implemented (args: '{}')", $log_name, self.args);
                true
            }
        }
    };
}

atk_action_stub!(ActionAttack, "attack");
atk_action_stub!(ActionCombo, "combo");
atk_action_stub!(ActionGrapple, "grapple");
atk_action_stub!(ActionCtrlGrapple, "ctrlgrapple");
atk_action_stub!(ActionGrapplePush, "grapplepush");
atk_action_stub!(ActionEvade, "evade");
atk_action_stub!(ActionJump, "jump");
atk_action_stub!(ActionDistance, "distance");
atk_action_stub!(ActionSideMove, "sidemove");
atk_action_stub!(ActionAnim, "anim");
atk_action_stub!(ActionWait, "wait");
atk_action_stub!(ActionDefend, "defend");
atk_action_stub!(ActionBlockSuccess, "blocksuccess");
atk_action_stub!(ActionBlockFail, "blockfail");
atk_action_stub!(ActionCloser, "closer");
atk_action_stub!(ActionFurther, "further");
atk_action_stub!(ActionIncomingAttack, "incomingattack");
atk_action_stub!(ActionHealth, "health");
atk_action_stub!(ActionTimeAndSpace, "timeandspace");
atk_action_stub!(ActionTargetJumping, "targetjumping");
atk_action_stub!(ActionTargetCrouching, "targetcrouching");
atk_action_stub!(ActionTargetBlocking, "targetblocking");
atk_action_stub!(ActionTargetAttacking, "targetattacking");
atk_action_stub!(ActionTargetKnockedDown, "targetknockeddown");
atk_action_stub!(ActionTargetKnockedDownCount, "targetknockeddowncount");
atk_action_stub!(ActionTargetGoingToBeHit, "targetgoingtobehit");
atk_action_stub!(ActionTargetHealth, "targethealth");

pub struct LogAction {
    pub name: String,
    pub args: String,
}
impl AtkAction for LogAction {
    fn name(&self) -> &str {
        &self.name
    }
    fn start(&mut self, _ctx: &mut AttackCtx) -> bool {
        bevy::log::warn!("atk: {} (Fallback) is not implemented (args: '{}')", self.name, self.args);
        true
    }
}

pub fn build_atk_action(name: &str, args: &str) -> Box<dyn AtkAction> {
    let args = args.to_string();
    match name {
        "attack" => Box::new(ActionAttack { args }),
        "combo" => Box::new(ActionCombo { args }),
        "grapple" => Box::new(ActionGrapple { args }),
        "ctrlgrapple" => Box::new(ActionCtrlGrapple { args }),
        "grapplepush" => Box::new(ActionGrapplePush { args }),
        "evade" => Box::new(ActionEvade { args }),
        "jump" => Box::new(ActionJump { args }),
        "distance" => Box::new(ActionDistance { args }),
        "sidemove" => Box::new(ActionSideMove { args }),
        "anim" => Box::new(ActionAnim { args }),
        "wait" => Box::new(ActionWait { args }),
        "defend" => Box::new(ActionDefend { args }),
        "blocksuccess" => Box::new(ActionBlockSuccess { args }),
        "blockfail" => Box::new(ActionBlockFail { args }),
        "closer" => Box::new(ActionCloser { args }),
        "further" => Box::new(ActionFurther { args }),
        "incomingattack" => Box::new(ActionIncomingAttack { args }),
        "health" => Box::new(ActionHealth { args }),
        "timeandspace" => Box::new(ActionTimeAndSpace { args }),
        "targetjumping" => Box::new(ActionTargetJumping { args }),
        "targetcrouching" => Box::new(ActionTargetCrouching { args }),
        "targetblocking" => Box::new(ActionTargetBlocking { args }),
        "targetattacking" => Box::new(ActionTargetAttacking { args }),
        "targetknockeddown" => Box::new(ActionTargetKnockedDown { args }),
        "targetknockeddowncount" => Box::new(ActionTargetKnockedDownCount { args }),
        "targetgoingtobehit" => Box::new(ActionTargetGoingToBeHit { args }),
        "targethealth" => Box::new(ActionTargetHealth { args }),
        _ => Box::new(LogAction { name: name.to_string(), args }), // Fallback
    }
}

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum AttackEvent {
    /// Always fires.
    Always,
    /// Fires with probability `p` (0..1).
    Probability(f32),
    /// Fires once a cookie was successfully grabbed this tick.
    GotCookie,
    /// Fires when the currently-running action emits the named event.
    Event(String),
    /// Fires once the currently-running action's `update()` returned true.
    Finished,
}

#[derive(Clone, Debug)]
pub enum AttackAction {
    /// End the current action and exit the .atk script with success.
    Finish,
    /// End the current action and exit with failure.
    Fail,
    /// End the current action, then instantiate and start the named action.
    Action(String, String),
    /// Re-call the current action's `start()` without ending it.
    Resume,
    /// Inline-evaluate another state's rules.
    Check(usize),
    /// Diagnostic.
    Display(String),
}

pub struct AttackCtx {
    /// The currently-running action, if any.
    pub current_action: Option<Box<dyn AtkAction>>,
    /// True once `current_action.update()` returned true this tick.
    pub current_finished: bool,
    /// Set once a successful cookie grab landed this tick.
    pub got_cookie: bool,
    /// Named events emitted by the running action this tick.
    pub action_events: Vec<String>,
    /// Per-tick delta seconds — the host sets this BEFORE calling `tick()`.
    pub dt: f32,
}

impl Default for AttackCtx {
    fn default() -> Self {
        Self {
            current_action: None,
            current_finished: false,
            got_cookie: false,
            action_events: Vec::new(),
            dt: 0.0,
        }
    }
}

/// Output produced by one tick.  `done` is the terminal flag — the host stops
/// ticking the attack runtime once this becomes Some.
#[derive(Default)]
pub struct AttackOutput {
    /// Set to Some(true) on `AFinish`, Some(false) on `AFail`.  None while
    /// the attack is still running.
    pub done: Option<bool>,
    /// Named events the action wants to surface to the host (vfx/sfx triggers
    /// keyed off action progression).  Currently unused — the action's own
    /// `update()` should write directly to game state via `_ctx`.
    pub events: Vec<String>,
}

// ---------------------------------------------------------------------------
// SmDriver impl
// ---------------------------------------------------------------------------

pub struct AttackDriver;

impl SmDriver for AttackDriver {
    type Event = AttackEvent;
    type Action = AttackAction;
    type Context = AttackCtx;
    type Output = AttackOutput;

    fn eval_event(
        ctx: &Self::Context,
        event: &Self::Event,
        runtime: &SmRuntime<Self>,
    ) -> bool {
        match event {
            AttackEvent::Always => true,
            AttackEvent::Probability(p) => runtime.random_pct < *p,
            AttackEvent::GotCookie => ctx.got_cookie,
            AttackEvent::Event(name) => {
                ctx.action_events.iter().any(|e| e == name)
                    || ctx
                        .current_action
                        .as_ref()
                        .map(|a| a.is_event(name))
                        .unwrap_or(false)
            }
            AttackEvent::Finished => ctx.current_finished,
        }
    }

    fn apply_action(
        ctx: &mut Self::Context,
        action: &Self::Action,
        output: &mut Self::Output,
        adv: &mut SmAdvance<Self>,
    ) {
        match action {
            AttackAction::Finish => {
                end_current(ctx);
                output.done = Some(true);
            }
            AttackAction::Fail => {
                end_current(ctx);
                output.done = Some(false);
                adv.fail();
            }
            AttackAction::Action(name, args) => {
                end_current(ctx);
                let mut new_action = build_atk_action(name, args);
                if call_action_start(ctx, &mut *new_action) {
                    ctx.current_action = Some(new_action);
                    ctx.current_finished = false;
                } else {
                    adv.fail();
                }
            }
            AttackAction::Resume => {
                if let Some(mut action) = ctx.current_action.take() {
                    let _ = call_action_start(ctx, &mut *action);
                    ctx.current_action = Some(action);
                }
            }
            AttackAction::Check(idx) => adv.check(*idx),
            AttackAction::Display(msg) => bevy::log::info!("atk: {}", msg),
        }
    }

    fn update_running(ctx: &mut Self::Context, output: &mut Self::Output) {
        // Fresh per-tick state; rule eval will re-read these.
        ctx.action_events.clear();
        ctx.got_cookie = false;
        ctx.current_finished = false;

        let Some(mut action) = ctx.current_action.take() else {
            return;
        };
        let dt = ctx.dt;

        let finished = action.update(ctx, output, dt);
        ctx.current_action = Some(action);

        ctx.current_finished = finished;
    }
}

/// Start an action, passing an exclusive lock on `ctx`.
fn call_action_start(ctx: &mut AttackCtx, action: &mut dyn AtkAction) -> bool {
    action.start(ctx)
}

/// End and clear the current action.
fn end_current(ctx: &mut AttackCtx) {
    if let Some(mut action) = ctx.current_action.take() {
        action.end(ctx);
    }
}

// ---------------------------------------------------------------------------
// Text parsers
// ---------------------------------------------------------------------------

pub fn parse_attack_event(
    text: &str,
    _state_index: &HashMap<String, usize>,
) -> Result<AttackEvent, String> {
    let text = text.trim();
    let (name, args) = split_call(text);

    Ok(match name {
        "EAlways" | "Always" | "True" => AttackEvent::Always,
        "EProbability" | "Probability" => {
            AttackEvent::Probability(args.parse::<f32>().unwrap_or(0.5))
        }
        "EGotCookie" | "GotCookie" => AttackEvent::GotCookie,
        "EEvent" | "Event" => AttackEvent::Event(args.trim_matches('"').to_string()),
        "EFinished" | "Finished" => AttackEvent::Finished,
        other => return Err(format!("atk: unknown event '{}'", other)),
    })
}

/// Parse one action line.  `AAction <name>` resolves the action name through
/// `state_index` here is NOT correct — the action pool is per-attack runtime
/// state; the parser instead emits `AAction(0)` and the host re-resolves the
/// index when binding the parsed `SmData` to a freshly-built action pool.
///
/// Until the per-attack action pool builder is in place, every named action
/// becomes `AAction(usize::MAX)` which the runtime will treat as a soft
/// failure (logged + adv.fail).
pub fn parse_attack_action(
    line: &str,
    state_index: &HashMap<String, usize>,
) -> Result<Option<AttackAction>, String> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("").trim();
    let arg = parts.next().unwrap_or("").trim();

    Ok(Some(match verb {
        "AFinish" | "Finish" => AttackAction::Finish,
        "AFail" | "Fail" => AttackAction::Fail,
        "AResume" | "Resume" => AttackAction::Resume,
        "AAction" | "Action" => {
            // Named action instantiation
            let mut inner_parts = arg.splitn(2, char::is_whitespace);
            let name = inner_parts.next().unwrap_or("").trim().to_string();
            let action_args = inner_parts.next().unwrap_or("").trim().to_string();
            AttackAction::Action(name, action_args)
        }
        "Check" => match state_index.get(arg) {
            Some(&idx) => AttackAction::Check(idx),
            None => return Ok(None),
        },
        "Display" => AttackAction::Display(arg.to_string()),
        _ => return Ok(None),
    }))
}

pub const ATTACK_EVENT_PARSER: EventParser<AttackDriver> = parse_attack_event;
pub const ATTACK_ACTION_PARSER: ActionParser<AttackDriver> = parse_attack_action;
