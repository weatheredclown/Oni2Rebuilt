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
use super::parse::{ActionParser, EventParser, split_call};

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
    /// One-shot entry.  `output` is available so actions can write an
    /// `attack_anim`/`block_anim`/etc. pulse synchronously with start —
    /// matching how `DoAttack` fires in the player FSM (output value
    /// written at apply_action time, drained by the host this tick).
    fn start(&mut self, _ctx: &mut AttackCtx, _output: &mut AttackOutput) -> bool {
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
            fn start(&mut self, _ctx: &mut AttackCtx, _output: &mut AttackOutput) -> bool {
                bevy::log::warn!(
                    "atk: {} is not implemented (args: '{}')",
                    $log_name,
                    self.args
                );
                true
            }
        }
    };
}

// ActionAttack — real impl.  Mirrors the player FSM's `DoAttack` terminal
// DNA: write the anim alias into `AttackOutput.attack_anim` at start, then
// wait for the animation to finish before signaling action-complete.  The
// host (attack_runtime_update_system) drains `attack_anim` via `do_attack`
// on the same tick we set it, so the attack anim is playing by the time
// our next `update` sees `ctx.anim_*` reflecting the new anim.
pub struct ActionAttack {
    pub args: String,
    /// Filled from `self.args` on `start` — the anim alias handed off to
    /// `do_attack`.  Kept so `update` can check whether the alias it
    /// started matches what's currently on Oni2AnimState (detect stomp).
    anim_name: String,
    /// Set once `ctx.anim_current_time` has caught up to what we started
    /// (guards against the one-frame "stale prior anim" state).
    saw_our_anim: bool,
}
impl ActionAttack {
    fn new(args: String) -> Self {
        Self {
            args,
            anim_name: String::new(),
            saw_our_anim: false,
        }
    }
}
impl AtkAction for ActionAttack {
    fn name(&self) -> &str {
        "attack"
    }
    fn start(&mut self, _ctx: &mut AttackCtx, output: &mut AttackOutput) -> bool {
        let anim = parse_string_arg(&self.args);
        if anim.is_empty() {
            bevy::log::warn!("ActionAttack: missing anim alias in args '{}'", self.args);
            return false;
        }
        self.anim_name = anim.clone();
        self.saw_our_anim = false;
        output.attack_anim = Some(anim.clone());
        bevy::log::info!(
            "ActionAttack::start -> anim='{}' (output.attack_anim set)",
            anim
        );
        true
    }
    fn update(&mut self, ctx: &mut AttackCtx, _output: &mut AttackOutput, _dt: f32) -> bool {
        // Wait until we've actually entered our own anim and then until it
        // plays through.  `anim_num_frames > 1` gates against the initial
        // zero-state pre-play tick.  `saw_our_anim` flips once we've seen
        // a tick AFTER the host wired in our anim — without it we'd match
        // stale-anim end-of-play state left over from the previous action.
        if ctx.anim_num_frames <= 1 {
            bevy::log::trace!(
                "ActionAttack::update '{}' waiting: anim_num_frames={} (need >1)",
                self.anim_name,
                ctx.anim_num_frames
            );
            return false;
        }
        if !self.saw_our_anim {
            bevy::log::trace!(
                "ActionAttack::update '{}' waiting saw_our_anim: t={:.2} num={} looping={}",
                self.anim_name,
                ctx.anim_current_time,
                ctx.anim_num_frames,
                ctx.anim_looping
            );
            if ctx.anim_current_time < 0.5 {
                self.saw_our_anim = true;
            }
            return false;
        }
        let last_frame = (ctx.anim_num_frames as f32 - 1.0).max(0.0);
        let done = !ctx.anim_looping && ctx.anim_current_time >= last_frame;
        if done {
            bevy::log::info!(
                "ActionAttack::update '{}' DONE: t={:.2}/{:.2}",
                self.anim_name,
                ctx.anim_current_time,
                last_frame
            );
        }
        done
    }
}

/// Utility to extract the first string token from an action argument string.
pub fn parse_string_arg(args: &str) -> String {
    args.split_whitespace().next().unwrap_or("").to_string()
}

/// Utility to extract the first f32 from an action argument string.
pub fn parse_f32_arg(args: &str, default_val: f32) -> f32 {
    args.split_whitespace()
        .next()
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(default_val)
}

// ActionWait — elapsed-time wait with dt accumulation.
pub struct ActionWait {
    pub args: String,
    duration: f32,
    elapsed: f32,
}
impl ActionWait {
    fn new(args: String) -> Self {
        Self {
            args,
            duration: 0.0,
            elapsed: 0.0,
        }
    }
}
impl AtkAction for ActionWait {
    fn name(&self) -> &str {
        "wait"
    }
    fn start(&mut self, _ctx: &mut AttackCtx, _output: &mut AttackOutput) -> bool {
        self.duration = parse_f32_arg(&self.args, 0.0);
        self.elapsed = 0.0;
        true
    }
    fn update(&mut self, _ctx: &mut AttackCtx, _output: &mut AttackOutput, dt: f32) -> bool {
        self.elapsed += dt;
        self.elapsed >= self.duration
    }
}

pub struct ActionDistance {
    pub args: String,
    target_distance: f32,
}
impl ActionDistance {
    fn new(args: String) -> Self {
        Self {
            args,
            target_distance: 1.0,
        }
    }
}

impl AtkAction for ActionDistance {
    fn name(&self) -> &str {
        "distance"
    }
    fn start(&mut self, _ctx: &mut AttackCtx, output: &mut AttackOutput) -> bool {
        self.target_distance = parse_f32_arg(&self.args, 0.0);
        // Dispatch edge request that we should begin following the target
        output.start_following_distance = Some(self.target_distance);
        true
    }
    fn update(&mut self, ctx: &mut AttackCtx, _output: &mut AttackOutput, _dt: f32) -> bool {
        // Complete the state once we get within the requested distance
        if let Some(dist) = ctx.target_distance_current {
            if dist <= self.target_distance {
                return true;
            }
        }
        false
    }
}

atk_action_stub!(ActionCombo, "combo");
atk_action_stub!(ActionGrapple, "grapple");
atk_action_stub!(ActionCtrlGrapple, "ctrlgrapple");
atk_action_stub!(ActionGrapplePush, "grapplepush");
atk_action_stub!(ActionEvade, "evade");
atk_action_stub!(ActionJump, "jump");
atk_action_stub!(ActionSideMove, "sidemove");
atk_action_stub!(ActionAnim, "anim");
// ActionWait has its own real impl above.
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
    fn start(&mut self, _ctx: &mut AttackCtx, _output: &mut AttackOutput) -> bool {
        bevy::log::warn!(
            "atk: {} (Fallback) is not implemented (args: '{}')",
            self.name,
            self.args
        );
        true
    }
}

pub fn build_atk_action(name: &str, args: &str) -> Box<dyn AtkAction> {
    let args = args.to_string();
    match name {
        "attack" => Box::new(ActionAttack::new(args)),
        "combo" => Box::new(ActionCombo { args }),
        "grapple" => Box::new(ActionGrapple { args }),
        "ctrlgrapple" => Box::new(ActionCtrlGrapple { args }),
        "grapplepush" => Box::new(ActionGrapplePush { args }),
        "evade" => Box::new(ActionEvade { args }),
        "jump" => Box::new(ActionJump { args }),
        "distance" => Box::new(ActionDistance::new(args)),
        "sidemove" => Box::new(ActionSideMove { args }),
        "anim" => Box::new(ActionAnim { args }),
        "wait" => Box::new(ActionWait::new(args)),
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
        _ => Box::new(LogAction {
            name: name.to_string(),
            args,
        }), // Fallback
    }
}

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum AttackEvent {
    /// Always fires.
    Always,
    /// Weight-walk step: decrements `runtime.random_budget` by `p`; matches
    /// when the budget drops to zero or below.  Matches the legacy
    /// `aiAttackStateMachine::EProbability` evaluator which walks a
    /// row of weights until the
    /// accumulator reaches zero.  Budget is re-seeded each tick inside
    /// `SmRuntime::tick`.
    Probability(f32),
    /// Fires while the fighter has the attack cookie (attack-slot granted by
    /// the coordinator — see `aiFighter::FLAG_GOT_COOKIE`).  Persistent
    /// across ticks, not pulse-style.
    GotCookie,
    /// Fires when the currently-running action emits the named event.
    Event(String),
    /// Fires once the currently-running action's `update()` returned true.
    Finished,
    /// Fires while the machine has no action running — first tick after
    /// entering a state (before its option-action has been started) or the
    /// tick after `end_current` cleared the slot.  Drives each RUN state's
    /// "kickoff the option's verb" rule.
    NoCurrentAction,
    /// Logical conjunction; matches iff every inner event matches.
    /// Short-circuits left-to-right.  Inner probability events still
    /// consume their budget as they're reached, mirroring the legacy
    /// walk semantics.
    And(Vec<AttackEvent>),
    /// Logical disjunction; matches iff any inner event matches.
    Or(Vec<AttackEvent>),
    /// Logical negation of the inner event.
    Not(Box<AttackEvent>),
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
    /// Set while this fighter has the attack cookie.  **Persistent** —
    /// granted by the coordinator (or FightBehavior as a placeholder) and
    /// cleared when revoked, NOT reset each tick.
    pub got_cookie: bool,
    /// Named events emitted by the running action this tick.
    pub action_events: Vec<String>,
    /// Per-tick delta seconds — the host sets this BEFORE calling `tick()`.
    pub dt: f32,
    /// Mirrors of the attacker's current Oni2AnimState — actions that need
    /// to wait for the animation to finish (e.g. ActionAttack) poll these
    /// to decide when to signal `Finished`.  Host fills them each tick
    /// before calling `sm.tick`.
    pub anim_num_frames: i32,
    pub anim_current_time: f32,
    pub anim_looping: bool,
    /// Distance to the entity's fight target, updated by the host system before tick()
    pub target_distance_current: Option<f32>,
    /// The entity owning this FSM, for debug logging.
    pub entity: Option<bevy::prelude::Entity>,
}

impl Default for AttackCtx {
    fn default() -> Self {
        Self {
            current_action: None,
            current_finished: false,
            got_cookie: false,
            action_events: Vec::new(),
            dt: 0.0,
            anim_num_frames: 0,
            anim_current_time: 0.0,
            anim_looping: false,
            target_distance_current: None,
            entity: None,
        }
    }
}

/// Output produced by one tick.  `done` is the terminal flag — the host stops
/// ticking the attack runtime once this becomes Some.  `attack_anim`/
/// `block_anim`/`evade_anim`/`custom_anim` are drained by the host via the
/// shared `do_attack` / `do_block` / … helpers (`statemachine/runtime.rs`)
/// — the SAME helpers the player's `FsmOutput` drain uses.  That's the
/// shared DNA: whether `DoAttack ANIMATTACK_1` comes from player.fsm's
/// rules or from a `.atk` `attack ANIMATTACK_1` verb, it converges on
/// `Oni2AnimLibrary::play` via the same named helper.
#[derive(Default)]
pub struct AttackOutput {
    /// Set to Some(true) on `AFinish`, Some(false) on `AFail`.  None while
    /// the attack is still running.
    pub done: Option<bool>,
    /// Named events the action wants to surface to the host (vfx/sfx triggers
    /// keyed off action progression).
    pub events: Vec<String>,
    /// Attack animation alias requested this tick — `ActionAttack::start`
    /// writes here.  Host calls `do_attack` with this name.
    pub attack_anim: Option<String>,
    /// Block animation alias requested this tick.
    pub block_anim: Option<String>,
    /// Evade animation alias requested this tick.  Bool is the mirror flag
    /// (unused today; reserved for left/right-evade asymmetry).
    pub evade_anim: Option<(String, bool)>,
    /// Script-requested custom animation alias for the current tick.
    pub custom_anim: Option<String>,
    /// Script-requested intent for pathfinding/steering toward a target distance.
    pub start_following_distance: Option<f32>,
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

    fn eval_event(ctx: &Self::Context, event: &Self::Event, runtime: &SmRuntime<Self>) -> bool {
        match event {
            AttackEvent::Always => true,
            AttackEvent::Probability(p) => {
                // Walk-by-subtraction — decrement the per-tick budget,
                // match when it crosses zero.  Matches legacy
                // aiAttackStateMachine::EProbability (attackstatemachine.cpp:
                // 707).  `random_budget` is seeded at the top of
                // `SmRuntime::tick`; we use an atomic f32 (stored as u32
                // bits) because eval_event takes `&SmRuntime` and Bevy
                // requires components be Sync.
                use std::sync::atomic::Ordering;
                let current_bits = runtime.random_budget.load(Ordering::Relaxed);
                let new_budget = f32::from_bits(current_bits) - *p;
                runtime
                    .random_budget
                    .store(new_budget.to_bits(), Ordering::Relaxed);
                new_budget <= 0.0
            }
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
            AttackEvent::NoCurrentAction => ctx.current_action.is_none(),
            AttackEvent::And(events) => events.iter().all(|e| Self::eval_event(ctx, e, runtime)),
            AttackEvent::Or(events) => events.iter().any(|e| Self::eval_event(ctx, e, runtime)),
            AttackEvent::Not(inner) => !Self::eval_event(ctx, inner, runtime),
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
                let debug_str = ctx
                    .entity
                    .map(crate::debug::debug_name)
                    .unwrap_or_else(|| "<no_entity>".to_string());
                bevy::log::info!(
                    "atk {} [{}]: Starting action {} '{}'",
                    debug_str,
                    ctx.entity
                        .map(|e| format!("{:?}", e))
                        .unwrap_or("?".to_string()),
                    name,
                    args
                );
                let mut new_action = build_atk_action(name, args);
                if call_action_start(ctx, output, &mut *new_action) {
                    ctx.current_action = Some(new_action);
                    ctx.current_finished = false;
                } else {
                    adv.fail();
                }
            }
            AttackAction::Resume => {
                if let Some(mut action) = ctx.current_action.take() {
                    let _ = call_action_start(ctx, output, &mut *action);
                    ctx.current_action = Some(action);
                }
            }
            AttackAction::Check(idx) => adv.check(*idx),
            AttackAction::Display(msg) => bevy::log::info!("atk: {}", msg),
        }
    }

    fn update_running(ctx: &mut Self::Context, output: &mut Self::Output) {
        // Fresh per-tick ephemeral state.  Note: `got_cookie` is NOT reset
        // here — it's persistent across ticks, set/cleared externally by
        // the fight coordinator (or FightBehavior today as a placeholder).
        // Same for the anim mirror fields — host fills them each tick.
        ctx.action_events.clear();
        ctx.current_finished = false;

        // Drain the per-tick animation-play slots too — these are pulse
        // outputs (consumed by the host's do_attack/do_block/etc. each
        // tick), and should not leak across ticks.
        output.attack_anim = None;
        output.block_anim = None;
        output.evade_anim = None;
        output.custom_anim = None;
        output.start_following_distance = None;
        output.events.clear();

        let Some(mut action) = ctx.current_action.take() else {
            return;
        };
        let dt = ctx.dt;

        let finished = action.update(ctx, output, dt);
        ctx.current_action = Some(action);

        ctx.current_finished = finished;
    }
}

/// Start an action, passing an exclusive lock on `ctx` + access to the
/// tick's `output` (so `ActionAttack::start` can write `attack_anim`).
fn call_action_start(
    ctx: &mut AttackCtx,
    output: &mut AttackOutput,
    action: &mut dyn AtkAction,
) -> bool {
    action.start(ctx, output)
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
        "ENoCurrentAction" | "NoCurrentAction" => AttackEvent::NoCurrentAction,
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
