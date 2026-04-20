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

/// Diagnostic-only fallback action — logs its name on start.  Used as a
/// placeholder for any `.atk` action verb whose concrete implementation is
/// not yet ported.
pub struct LogAction {
    pub name: String,
}

impl AtkAction for LogAction {
    fn name(&self) -> &str {
        &self.name
    }
    fn start(&mut self, _ctx: &mut AttackCtx) -> bool {
        bevy::log::info!("atk: LogAction.start '{}'", self.name);
        true
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
    /// End the current action, then start the action at the given pool index.
    Action(usize),
    /// Re-call the current action's `start()` without ending it.
    Resume,
    /// Inline-evaluate another state's rules.
    Check(usize),
    /// Diagnostic.
    Display(String),
}

/// Per-attack runtime context.  `actions` is a parallel `Vec` whose indices
/// match the `AttackAction::Action(idx)` references parsed from the `.atk`
/// file.  The host populates `actions` at load time by mapping each
/// action-name token in the file onto a concrete `Box<dyn AtkAction>`.
///
/// Most fields default to the values a brand-new attack would have.
pub struct AttackCtx {
    /// Pool of action implementations indexed by `AttackAction::Action(idx)`.
    pub actions: Vec<Box<dyn AtkAction>>,
    /// Index of the currently-running action, if any.
    pub current_action: Option<usize>,
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
            actions: Vec::new(),
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
                        .and_then(|i| ctx.actions.get(i))
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
            AttackAction::Action(idx) => {
                let new_idx = *idx;
                end_current(ctx);
                if new_idx >= ctx.actions.len() {
                    bevy::log::warn!(
                        "atk: AAction({}) but pool only has {} actions",
                        new_idx,
                        ctx.actions.len()
                    );
                    adv.fail();
                    return;
                }
                if call_action_start(ctx, new_idx) {
                    ctx.current_action = Some(new_idx);
                    ctx.current_finished = false;
                } else {
                    adv.fail();
                }
            }
            AttackAction::Resume => {
                if let Some(idx) = ctx.current_action {
                    let _ = call_action_start(ctx, idx);
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

        let Some(idx) = ctx.current_action else {
            return;
        };
        let dt = ctx.dt;

        // Re-borrow trick: temporarily detach the action from the pool so we
        // can pass `ctx` into update() without aliasing.  Replace it with a
        // sentinel `LogAction` so the slot stays valid; the swap puts it back.
        let mut placeholder: Box<dyn AtkAction> = Box::new(LogAction {
            name: String::new(),
        });
        std::mem::swap(&mut placeholder, &mut ctx.actions[idx]);
        let finished = placeholder.update(ctx, output, dt);
        std::mem::swap(&mut placeholder, &mut ctx.actions[idx]);

        ctx.current_finished = finished;
    }
}

/// Swap the action at `idx` out of the pool, call `start(ctx)`, swap back.
/// The temporary swap eliminates the alias conflict between the borrow on
/// `ctx.actions[idx]` and the `&mut ctx` argument the action needs.
fn call_action_start(ctx: &mut AttackCtx, idx: usize) -> bool {
    if idx >= ctx.actions.len() {
        return false;
    }
    let mut placeholder: Box<dyn AtkAction> = Box::new(LogAction {
        name: String::new(),
    });
    std::mem::swap(&mut placeholder, &mut ctx.actions[idx]);
    let started = placeholder.start(ctx);
    std::mem::swap(&mut placeholder, &mut ctx.actions[idx]);
    started
}

/// End and clear the current action.  Uses the same swap pattern so `end()`
/// receives an exclusive `&mut AttackCtx`.
fn end_current(ctx: &mut AttackCtx) {
    let Some(idx) = ctx.current_action.take() else {
        return;
    };
    if idx >= ctx.actions.len() {
        return;
    }
    let mut placeholder: Box<dyn AtkAction> = Box::new(LogAction {
        name: String::new(),
    });
    std::mem::swap(&mut placeholder, &mut ctx.actions[idx]);
    placeholder.end(ctx);
    std::mem::swap(&mut placeholder, &mut ctx.actions[idx]);
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
            // Numeric index? use as-is.
            if let Ok(idx) = arg.parse::<usize>() {
                AttackAction::Action(idx)
            } else {
                // Named action — needs late binding by host.  Emit MAX as a
                // tombstone the runtime fails on cleanly.
                bevy::log::warn!(
                    "atk: AAction '{}' needs late-binding (no action pool builder yet)",
                    arg
                );
                AttackAction::Action(usize::MAX)
            }
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
