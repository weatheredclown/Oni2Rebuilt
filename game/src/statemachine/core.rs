/*
 * statemachine/core.rs — generic state-machine abstraction.
 *
 * Mirrors the underlying shape shared by the legacy aiStateMachine (aicore/) and its
 * three derived machines (behavior/istatemachine, aifight/statemachine,
 * aifight/attackstatemachine).  Each of those has the identical structure —
 * states + rules + events + actions + optional tuning — with different
 * vocabularies plugged into the same eval loop.
 *
 * Usage:
 *   1. Define a unit struct D and implement `SmDriver` for it.  Provide the
 *      associated types Event/Action/Context/Output, and the three hooks
 *      `eval_event`, `apply_action`, and (optionally) `update_running`.
 *   2. Construct `SmData<D>` by filling `states` and `state_index` — typically
 *      from a text-format parser shared by all drivers.
 *   3. Put an `SmRuntime<D>` on the entity (or wherever ticking is driven).
 *   4. Each tick, build the driver's `Context` (typically from per-frame input
 *      snapshots) and call `runtime.tick(ctx)` to get back an `Output`.
 *
 * Rule semantics match the legacy implementation exactly:
 *   • Rules are scanned top-to-bottom in the current state.
 *   • The first rule whose `event` evaluates true (XOR `negated`) fires.
 *   • Its `actions` are applied in order; `SmAdvance` collects any requested
 *     check-chain / goto / failure from the driver.
 *   • If an action marks itself failed (`adv.fail()`), the goto is suppressed
 *     and evaluation resumes at the NEXT rule (not the next state).
 *   • Otherwise, `adv.goto(n)` — or the rule's own `goto_state` — transitions
 *     to state n and eval returns.
 *   • If a rule fires and produces no state change, evaluation stops for this
 *     tick (the rule is "consumed").
 *   • `adv.check(n)` evaluates state n's rules inline WITHOUT moving the
 *     cursor — mirrors the legacy `Check()` action that lets one state delegate
 *     to a shared subroutine-state.  Depth-bounded.
 */
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Public trait — each concrete machine implements this.
// ---------------------------------------------------------------------------

/// A driver describes one specific state-machine dialect — its event
/// vocabulary, action vocabulary, context shape, and output shape.
///
/// Concrete implementors: `InputDriver` (player/AI pad-driven FSM),
/// `FightDriver` (AI cooperative-fighting coordination machine), and
/// `AttackDriver` (per-attack probabilistic action scheduler).
pub trait SmDriver: Sized + 'static {
    /// Guard predicate type referenced by each `SmRule.event`.  Typically an
    /// enum so exhaustive matching on it is a compile-time check.
    type Event: Clone + std::fmt::Debug;

    /// Action effect type referenced by `SmRule.actions`.  Also typically an
    /// enum; stateful actions (e.g. aiAtkAction lifecycle) use a variant that
    /// carries an index into the context's polymorphic action pool.
    type Action: Clone + std::fmt::Debug;

    /// Driver-private per-entity state mutated by `apply_action` and read by
    /// `eval_event`.  Holds the "packet" of inputs, lifecycle flags, queued
    /// work, etc.  Does NOT own the shared `SmData` — that's in `SmRuntime`.
    type Context;

    /// Accumulator filled during a single tick — the driver writes here in
    /// `apply_action` so the caller can consume the result post-tick without
    /// having to re-inspect the runtime.
    type Output: Default;

    /// Evaluate a rule's guard against the current context + runtime.  Pure
    /// predicate — must not mutate anything.  Returning `true` makes the
    /// rule "match"; `SmRule.negated` then flips the sense of the match.
    fn eval_event(ctx: &Self::Context, event: &Self::Event, runtime: &SmRuntime<Self>) -> bool;

    /// Apply one action's side effects.  Writes mutations to `ctx` and the
    /// tick `output`.  For state changes, request them via `adv.goto(n)` or
    /// `adv.check(n)` rather than mutating the runtime directly — the eval
    /// loop needs to see the request to handle recursion bounds correctly.
    fn apply_action(
        ctx: &mut Self::Context,
        action: &Self::Action,
        output: &mut Self::Output,
        adv: &mut SmAdvance<Self>,
    );

    /// Optional hook fired once per tick BEFORE rule evaluation.  Used by
    /// drivers whose Actions have a per-tick lifecycle beyond "fire once"
    /// (e.g. `AttackDriver`'s `aiAtkAction::Update`).  Default is a no-op.
    fn update_running(_ctx: &mut Self::Context, _output: &mut Self::Output) {}

    /// When a rule whose event matches THIS predicate causes a successful
    /// `goto`, the runtime immediately re-evaluates the destination state in
    /// the same tick (depth+1).  Used by `InputDriver` to make queued input
    /// fire on the same tick as a `Timeout` transition rather than waiting
    /// one frame.  Default is no cascade.
    fn cascade_after_event(_event: &Self::Event) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// SmData — loaded-from-file state table (shared via Arc across runtimes)
// ---------------------------------------------------------------------------

/// The parsed state machine — states + their rules + a name→index map.
/// Wrap in `Arc` for sharing across multiple `SmRuntime`s (many entities using
/// the same parsed table).
pub struct SmData<D: SmDriver> {
    pub states: Vec<SmState<D>>,
    pub state_index: HashMap<String, usize>,
}

impl<D: SmDriver> SmData<D> {
    /// Look up a state's numeric index by name, defaulting to 0 if the name
    /// isn't present.  Drivers that have a specific entry point should call
    /// this with their convention (e.g. "ATTACK_START" for input, "IDLE" for
    /// fight, "FIRST" for attack).
    pub fn index_of_or_zero(&self, name: &str) -> usize {
        self.state_index.get(name).copied().unwrap_or(0)
    }

    /// Display name for diagnostics.  Returns `"<invalid>"` for out-of-range
    /// indices so logs never panic.
    pub fn state_name(&self, idx: usize) -> &str {
        self.states
            .get(idx)
            .map(|s| s.name.as_str())
            .unwrap_or("<invalid>")
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

impl<D: SmDriver> Default for SmData<D> {
    fn default() -> Self {
        SmData {
            states: Vec::new(),
            state_index: HashMap::new(),
        }
    }
}

/// One state: a name plus an ordered list of rules evaluated top-to-bottom.
pub struct SmState<D: SmDriver> {
    pub name: String,
    pub rules: Vec<SmRule<D>>,
}

/// One guarded transition.  Semantics:
///   • Rule matches iff `eval_event(event) XOR negated` is true.
///   • On match: `actions` fire in order, then optional `goto_state`.
pub struct SmRule<D: SmDriver> {
    pub event: D::Event,
    pub negated: bool,
    pub actions: Vec<D::Action>,
    pub goto_state: Option<usize>,
}

// SmRule needs Clone for the eval loop to iterate over rules without holding
// a borrow into the data.  Since D::Event and D::Action are themselves Clone,
// manual impl avoids requiring `D: Clone` (which would be wrong — the driver
// itself isn't cloneable, only its data types are).
impl<D: SmDriver> Clone for SmRule<D> {
    fn clone(&self) -> Self {
        SmRule {
            event: self.event.clone(),
            negated: self.negated,
            actions: self.actions.clone(),
            goto_state: self.goto_state,
        }
    }
}

// ---------------------------------------------------------------------------
// SmAdvance — the view apply_action writes through to request state changes
// ---------------------------------------------------------------------------

/// Passed mutably to `apply_action` so the driver can request transitions
/// without needing direct access to the runtime (avoids re-entrancy hazards).
///
/// If multiple actions within a single rule call `goto`, the LAST one wins.
/// `check` requests are processed between actions, so repeated `check` calls
/// run each target in turn.
pub struct SmAdvance<D: SmDriver> {
    /// Inline-evaluate this state's rules without moving the cursor.
    /// Processed between successive actions in the same rule.
    check_request: Option<usize>,
    /// Move the cursor to this state at the end of the rule.
    goto_request: Option<usize>,
    /// If true, suppress `goto_state` and resume rule evaluation on the next
    /// rule.  Matches the legacy `ActionFailed` flag set by e.g. `DoTriggerAtk`
    /// when the trigger resolution failed.
    action_failed: bool,
    _phantom: std::marker::PhantomData<D>,
}

impl<D: SmDriver> SmAdvance<D> {
    fn new() -> Self {
        SmAdvance {
            check_request: None,
            goto_request: None,
            action_failed: false,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Inline-check state `state_idx` after the current action returns.  The
    /// eval loop processes this before the next action fires.
    pub fn check(&mut self, state_idx: usize) {
        self.check_request = Some(state_idx);
    }

    /// Request a state transition to `state_idx` at the end of this rule
    /// (provided no action called `fail()`).
    pub fn goto(&mut self, state_idx: usize) {
        self.goto_request = Some(state_idx);
    }

    /// Mark the current action failed.  Suppresses the rule's `goto_state`
    /// and resumes evaluation at the next rule in the state.
    pub fn fail(&mut self) {
        self.action_failed = true;
    }
}

// ---------------------------------------------------------------------------
// SmRuntime — per-entity machine state
// ---------------------------------------------------------------------------

/// Per-entity runtime holding the state cursor + timer + RNG.  Typically
/// attached to a Bevy entity as a component (or embedded in a wrapper such
/// as `FsmRuntime`).
pub struct SmRuntime<D: SmDriver> {
    pub data: Arc<SmData<D>>,
    pub current_state: usize,

    /// Elapsed time at the last `ResetTimer` action.  Used by `ETimer`.
    pub timer_start: f32,
    /// Total elapsed time since runtime creation.  Advanced each tick by
    /// the caller (we don't own a clock — the host system passes `dt`).
    pub elapsed: f32,
    /// Random value in [0, 1) — refreshed each tick from the RNG.  Used by
    /// threshold-style probability events (e.g. `FsmEvent::Random(p)` — fires
    /// iff `random_pct < p`).
    pub random_pct: f32,

    /// Walk-style budget — seeded each tick to a fresh [0,1) roll, but
    /// mutable-through-shared-reference via an atomic f32 (stored as
    /// u32 bits; Bevy components must be Send+Sync so `Cell` is out).
    /// `AttackEvent::Probability(p)` uses this: each rule in a pick-row
    /// subtracts `p`; the first rule whose subtraction drives the budget
    /// to `<= 0` fires.  Mirrors legacy `aiAttackStateMachine::
    /// EProbability` (rb/src/aifight/attackstatemachine.cpp:707).
    pub random_budget: AtomicU32,

    /// xorshift seed.  Pub so the host can synchronize a shadow runtime to a
    /// production one for parity testing.
    pub rng_seed: u64,
}

impl<D: SmDriver> SmRuntime<D> {
    pub fn new(data: Arc<SmData<D>>, initial_state: usize) -> Self {
        SmRuntime {
            data,
            current_state: initial_state,
            timer_start: 0.0,
            elapsed: 0.0,
            random_pct: 0.0,
            random_budget: AtomicU32::new(0_f32.to_bits()),
            rng_seed: 0xdeadbeef_cafebabe,
        }
    }

    /// Advance elapsed time by `dt`.  Call once per tick before `tick()`.
    pub fn advance_clock(&mut self, dt: f32) {
        self.elapsed += dt;
    }

    /// Mark the current tick's timer start (the action handler for
    /// ResetTimer should call this).
    pub fn reset_timer(&mut self) {
        self.timer_start = self.elapsed;
    }

    /// Run the eval loop for one tick: optional running-action update, then
    /// rule evaluation starting from `current_state`.  Returns the accumulated
    /// `Output`.
    pub fn tick(&mut self, ctx: &mut D::Context) -> D::Output {
        self.random_pct = self.advance_rng();
        // Seed the walk budget with a fresh roll.  Drivers that consume
        // it (AttackDriver::Probability) decrement and consider a match
        // when the value crosses zero.  Stored as f32 bits in the atomic.
        let walk_seed = self.advance_rng();
        self.random_budget
            .store(walk_seed.to_bits(), Ordering::Relaxed);

        let mut output = D::Output::default();
        D::update_running(ctx, &mut output);

        let start = self.current_state;
        self.evaluate_state(ctx, start, &mut output, 0);

        output
    }

    fn evaluate_state(
        &mut self,
        ctx: &mut D::Context,
        state_idx: usize,
        output: &mut D::Output,
        depth: u32,
    ) -> bool {
        const MAX_DEPTH: u32 = 16;
        if depth > MAX_DEPTH {
            bevy::log::warn!(
                "state machine check-recursion limit hit at state {} (depth {})",
                state_idx,
                depth
            );
            return false;
        }

        let rules = match self.data.states.get(state_idx) {
            Some(s) => s.rules.clone(),
            None => return false,
        };

        for rule in &rules {
            let raw = D::eval_event(ctx, &rule.event, self);
            let matches = raw ^ rule.negated;
            if !matches {
                continue;
            }

            let mut adv = SmAdvance::<D>::new();

            for action in &rule.actions {
                D::apply_action(ctx, action, output, &mut adv);

                // Process any inline `check` request before the next action
                // in this rule fires.  Check can RECURSIVELY change state,
                // which is surfaced via our own evaluate_state returning true.
                if let Some(check_target) = adv.check_request.take() {
                    self.evaluate_state(ctx, check_target, output, depth + 1);
                }

                if adv.action_failed {
                    break;
                }
            }

            if adv.action_failed {
                // This rule's actions bailed — try the next rule.
                continue;
            }

            // Action chain succeeded.  Apply the goto if requested.
            if let Some(target) = adv.goto_request.or(rule.goto_state) {
                if target != self.current_state {
                    bevy::log::info!(
                        "FSM Transition [{}]: {} -> {}",
                        std::any::type_name::<D>(),
                        self.data.state_name(self.current_state),
                        self.data.state_name(target)
                    );
                }
                self.current_state = target;
                // Cascade: some events (Timeout in InputDriver) want the
                // destination's rules evaluated immediately so queued input
                // doesn't pay a one-frame latency.  Bounded to depth==0 so
                // chains can't recurse forever.
                if depth == 0 && D::cascade_after_event(&rule.event) {
                    self.evaluate_state(ctx, target, output, depth + 1);
                }
                return true;
            }
            // Matched but no transition — keep evaluating subsequent rules so
            // inline Check / accumulator-action patterns don't stall the
            // engine.  Mirrors the legacy FsmRuntime semantics.
        }

        false
    }

    fn advance_rng(&mut self) -> f32 {
        self.rng_seed ^= self.rng_seed << 13;
        self.rng_seed ^= self.rng_seed >> 7;
        self.rng_seed ^= self.rng_seed << 17;
        (self.rng_seed >> 32) as f32 / u32::MAX as f32
    }
}
