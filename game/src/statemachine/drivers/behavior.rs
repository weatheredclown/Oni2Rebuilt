/*
 * statemachine/drivers/behavior.rs — BehaviorDriver: top-level AI behavior
 * orchestrator.
 *
 * The FSM cursor IS the current behavior: Idle / Fight / Follow / Goto /
 * Patrol / Retreat / Pad.  Matches the legacy `bhBehaviorComponent` shape
 * from rb/src/behavior/ but uses our FSM+hybrid-action pattern instead of
 * the C++ inheritance hierarchy: the FSM file owns the transition graph
 * (readable spec), a Rust-side `Behavior` trait impl owns the per-frame
 * work for whichever state is current.
 *
 * Transitions between behaviors are universal — any state accepts any
 * inbound behavior request — so the FSM collapses the big inbound switch
 * into one shared `BEHAVIOR_SWITCHES` subroutine invoked via `Check` from
 * every state.  Per-state behaviour only appears in `EBehaviorDone`
 * handling: most fall to IDLE when the current behavior finishes, PATROL
 * re-enters itself (looping), PAD has no done handler.
 *
 * The FSM is embedded at the bottom of this file and loaded once into a
 * shared `Arc<SmData<BehaviorDriver>>` by the behavior plugin.
 */
use std::collections::HashMap;
use std::sync::Arc;

use super::super::core::{SmAdvance, SmData, SmDriver, SmRuntime};
use super::parse::{ActionParser, EventParser, parse_sm};

// ---------------------------------------------------------------------------
// Behavior kind — the small closed set of behaviors we dispatch.
// ---------------------------------------------------------------------------

/// One of the top-level AI behaviors.  Parallel to the legacy `bhBehaviors`
/// enum (rb/src/behavior/messages.h BEHAVIORLIST).  Used as both the FSM
/// action payload (`StartBehavior(kind)`) and the key in `BehaviorRegistry`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BehaviorKind {
    Idle,
    Fight,
    Follow,
    Goto,
    Patrol,
    Retreat,
    Pad,
}

impl BehaviorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BehaviorKind::Idle => "Idle",
            BehaviorKind::Fight => "Fight",
            BehaviorKind::Follow => "Follow",
            BehaviorKind::Goto => "Goto",
            BehaviorKind::Patrol => "Patrol",
            BehaviorKind::Retreat => "Retreat",
            BehaviorKind::Pad => "Pad",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "Idle" => BehaviorKind::Idle,
            "Fight" => BehaviorKind::Fight,
            "Follow" => BehaviorKind::Follow,
            "Goto" => BehaviorKind::Goto,
            "Patrol" => BehaviorKind::Patrol,
            "Retreat" => BehaviorKind::Retreat,
            "Pad" => BehaviorKind::Pad,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

/// Guard predicates for the behavior-layer FSM.  The "E*" inbound events
/// correspond to commands scripts (ScrOni `fight`/`follow`/`goto`/…) or
/// other systems route into this actor's `BehaviorRuntime`.
#[derive(Clone, Debug)]
pub enum BehaviorEvent {
    /// Fight-mode requested this tick.
    Fight,
    /// Follow-mode requested this tick.
    Follow,
    /// Goto-point requested this tick.
    Goto,
    /// Patrol-a-path requested this tick.
    Patrol,
    /// Retreat requested this tick.
    Retreat,
    /// Idle requested this tick.
    Idle,
    /// Pad (player takeover) requested this tick.
    Pad,
    /// The currently-active behavior's `update` just returned `Finished`.
    BehaviorDone,
    /// Always true — used for unconditional rules that drop into `Check`.
    Always,
}

/// Side-effects a rule body can request.  `StartBehavior` is how the FSM
/// tells the host to swap in a new Rust-side `Behavior` impl; the host
/// reads `BehaviorOutput.started_behavior` post-tick and dispatches a
/// `StartBehaviorMessage`.
#[derive(Clone, Debug)]
pub enum BehaviorAction {
    /// Ask the host to enter the given behavior (spawn its Rust impl).
    StartBehavior(BehaviorKind),
    /// Inline-evaluate another state's rules without moving the cursor —
    /// our `BEHAVIOR_SWITCHES` subroutine mechanism.
    Check(usize),
    /// Diagnostic log line.
    Display(String),
}

// ---------------------------------------------------------------------------
// Context + output
// ---------------------------------------------------------------------------

/// Per-tick context.  Inbound "request" flags are pulse-semantics — the
/// host sets them for exactly one tick when a ScrOni command fires or
/// another system asks for a behavior swap.  `behavior_done` is a single-
/// tick edge fired by the dispatcher when the current Rust-side Behavior's
/// `update` returned `Finished`.
#[derive(Default, Clone, Debug)]
pub struct BehaviorCtx {
    pub requested_idle: bool,
    pub requested_fight: bool,
    pub requested_follow: bool,
    pub requested_goto: bool,
    pub requested_patrol: bool,
    pub requested_retreat: bool,
    pub requested_pad: bool,
    pub behavior_done: bool,
    /// Diagnostic — name of the state we're currently in.
    pub current_state: String,
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct BehaviorOutput {
    /// Set by `apply_action` when the FSM fires `StartBehavior(kind)`.
    /// Drained post-tick by the host and turned into a
    /// `StartBehaviorMessage`.  The last `StartBehavior` in the tick wins
    /// — the FSM graph is authored so only one ever fires per tick.
    pub started_behavior: Option<BehaviorKind>,
}

// ---------------------------------------------------------------------------
// Driver impl
// ---------------------------------------------------------------------------

pub struct BehaviorDriver;

impl SmDriver for BehaviorDriver {
    type Event = BehaviorEvent;
    type Action = BehaviorAction;
    type Context = BehaviorCtx;
    type Output = BehaviorOutput;

    fn eval_event(ctx: &Self::Context, event: &Self::Event, _runtime: &SmRuntime<Self>) -> bool {
        match event {
            BehaviorEvent::Fight => ctx.requested_fight,
            BehaviorEvent::Follow => ctx.requested_follow,
            BehaviorEvent::Goto => ctx.requested_goto,
            BehaviorEvent::Patrol => ctx.requested_patrol,
            BehaviorEvent::Retreat => ctx.requested_retreat,
            BehaviorEvent::Idle => ctx.requested_idle,
            BehaviorEvent::Pad => ctx.requested_pad,
            BehaviorEvent::BehaviorDone => ctx.behavior_done,
            BehaviorEvent::Always => true,
        }
    }

    fn apply_action(
        _ctx: &mut Self::Context,
        action: &Self::Action,
        output: &mut Self::Output,
        adv: &mut SmAdvance<Self>,
    ) {
        match action {
            BehaviorAction::StartBehavior(kind) => {
                output.started_behavior = Some(*kind);
            }
            BehaviorAction::Check(idx) => adv.check(*idx),
            BehaviorAction::Display(msg) => bevy::log::info!("behavior: {}", msg),
        }
    }
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

pub fn parse_behavior_event(
    text: &str,
    _state_idx: &HashMap<String, usize>,
) -> Result<BehaviorEvent, String> {
    let t = text.trim();
    Ok(match t {
        "EFight" | "Fight" => BehaviorEvent::Fight,
        "EFollow" | "Follow" => BehaviorEvent::Follow,
        "EGoto" | "Goto" => BehaviorEvent::Goto,
        "EPatrol" | "Patrol" => BehaviorEvent::Patrol,
        "ERetreat" | "Retreat" => BehaviorEvent::Retreat,
        "EIdle" | "Idle" => BehaviorEvent::Idle,
        "EPad" | "Pad" => BehaviorEvent::Pad,
        "EBehaviorDone" | "BehaviorDone" => BehaviorEvent::BehaviorDone,
        "EAlways" | "Always" | "True" => BehaviorEvent::Always,
        other => return Err(format!("behavior: unknown event '{}'", other)),
    })
}

pub fn parse_behavior_action(
    line: &str,
    state_index: &HashMap<String, usize>,
) -> Result<Option<BehaviorAction>, String> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("").trim();
    let arg = parts.next().unwrap_or("").trim();

    Ok(Some(match verb {
        "StartBehavior" => match BehaviorKind::parse(arg) {
            Some(k) => BehaviorAction::StartBehavior(k),
            None => {
                return Err(format!(
                    "behavior: StartBehavior with unknown kind '{}'",
                    arg
                ));
            }
        },
        "Check" => match state_index.get(arg) {
            Some(&idx) => BehaviorAction::Check(idx),
            None => return Ok(None),
        },
        "Display" => BehaviorAction::Display(arg.to_string()),
        _ => return Ok(None),
    }))
}

pub const BEHAVIOR_EVENT_PARSER: EventParser<BehaviorDriver> = parse_behavior_event;
pub const BEHAVIOR_ACTION_PARSER: ActionParser<BehaviorDriver> = parse_behavior_action;

// ---------------------------------------------------------------------------
// Embedded FSM + loader
// ---------------------------------------------------------------------------

/// Parse the embedded FSM text into a reusable `SmData<BehaviorDriver>`.
pub fn load_embedded() -> Result<SmData<BehaviorDriver>, String> {
    parse_sm::<BehaviorDriver>(BEHAVIOR_FSM, BEHAVIOR_EVENT_PARSER, BEHAVIOR_ACTION_PARSER)
}

/// Convenience: wrap `load_embedded` into an `Arc` for sharing via a resource.
pub fn load_embedded_arc() -> Result<Arc<SmData<BehaviorDriver>>, String> {
    load_embedded().map(Arc::new)
}

/// Top-level AI behavior orchestration FSM.
///
/// States = behaviors.  Each non-PAD state opens with `EBehaviorDone`
/// handling (where applicable) then falls into the shared
/// `BEHAVIOR_SWITCHES` subroutine that accepts any inbound behavior
/// request.  Transitions are universal from every state — a `fight`
/// command mid-patrol swaps Patrol → Fight cleanly.
pub const BEHAVIOR_FSM: &str = r#"
; Behavior-layer FSM.  State cursor = current behavior kind.  Per-state
; EBehaviorDone handling diverges (most → IDLE; PATROL loops; PAD has no
; done handler).  All inbound behavior-switch commands route through the
; shared BEHAVIOR_SWITCHES subroutine.

#IDLE_STATE
if EAlways       { Check BEHAVIOR_SWITCHES }

#FIGHT_STATE
if EBehaviorDone { StartBehavior Idle;    goto IDLE_STATE }
if EAlways       { Check BEHAVIOR_SWITCHES }

#FOLLOW_STATE
if EBehaviorDone { StartBehavior Idle;    goto IDLE_STATE }
if EAlways       { Check BEHAVIOR_SWITCHES }

#GOTO_STATE
if EBehaviorDone { StartBehavior Idle;    goto IDLE_STATE }
if EAlways       { Check BEHAVIOR_SWITCHES }

#PATROL_STATE
if EBehaviorDone { StartBehavior Patrol;  goto PATROL_STATE }
if EAlways       { Check BEHAVIOR_SWITCHES }

#RETREAT_STATE
if EBehaviorDone { StartBehavior Idle;    goto IDLE_STATE }
if EAlways       { Check BEHAVIOR_SWITCHES }

#PAD_STATE
; Player has the pad.  No EAlways/Check here — PAD is a dead-end until the
; ScrOni unpad path strips the BehaviorRuntime and hands the entity back.

#BEHAVIOR_SWITCHES
if EFight   { StartBehavior Fight;   goto FIGHT_STATE }
if EFollow  { StartBehavior Follow;  goto FOLLOW_STATE }
if EGoto    { StartBehavior Goto;    goto GOTO_STATE }
if EPatrol  { StartBehavior Patrol;  goto PATROL_STATE }
if ERetreat { StartBehavior Retreat; goto RETREAT_STATE }
if EIdle    { StartBehavior Idle;    goto IDLE_STATE }
if EPad     { StartBehavior Pad;     goto PAD_STATE }
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fsm_parses() {
        let data = load_embedded().expect("behavior FSM parses");
        for name in &[
            "IDLE_STATE",
            "FIGHT_STATE",
            "FOLLOW_STATE",
            "GOTO_STATE",
            "PATROL_STATE",
            "RETREAT_STATE",
            "PAD_STATE",
            "BEHAVIOR_SWITCHES",
        ] {
            assert!(
                data.state_index.contains_key(*name),
                "missing state {}",
                name
            );
        }
    }

    #[test]
    fn idle_to_goto_on_request() {
        let data = Arc::new(load_embedded().expect("parses"));
        let idle = data.index_of_or_zero("IDLE_STATE");
        let goto = data.index_of_or_zero("GOTO_STATE");
        let mut rt = SmRuntime::<BehaviorDriver>::new(data, idle);

        let mut ctx = BehaviorCtx {
            requested_goto: true,
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, goto);
        assert_eq!(out.started_behavior, Some(BehaviorKind::Goto));
    }

    #[test]
    fn goto_done_returns_to_idle() {
        let data = Arc::new(load_embedded().expect("parses"));
        let idle = data.index_of_or_zero("IDLE_STATE");
        let goto = data.index_of_or_zero("GOTO_STATE");
        let mut rt = SmRuntime::<BehaviorDriver>::new(data, goto);

        let mut ctx = BehaviorCtx {
            behavior_done: true,
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, idle);
        assert_eq!(out.started_behavior, Some(BehaviorKind::Idle));
    }

    #[test]
    fn patrol_done_loops() {
        let data = Arc::new(load_embedded().expect("parses"));
        let patrol = data.index_of_or_zero("PATROL_STATE");
        let mut rt = SmRuntime::<BehaviorDriver>::new(data, patrol);

        let mut ctx = BehaviorCtx {
            behavior_done: true,
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, patrol, "patrol loops on done");
        assert_eq!(out.started_behavior, Some(BehaviorKind::Patrol));
    }

    #[test]
    fn fight_interrupts_patrol() {
        let data = Arc::new(load_embedded().expect("parses"));
        let patrol = data.index_of_or_zero("PATROL_STATE");
        let fight = data.index_of_or_zero("FIGHT_STATE");
        let mut rt = SmRuntime::<BehaviorDriver>::new(data, patrol);

        let mut ctx = BehaviorCtx {
            requested_fight: true,
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, fight);
        assert_eq!(out.started_behavior, Some(BehaviorKind::Fight));
    }

    #[test]
    fn pad_is_sticky() {
        let data = Arc::new(load_embedded().expect("parses"));
        let pad = data.index_of_or_zero("PAD_STATE");
        let mut rt = SmRuntime::<BehaviorDriver>::new(data, pad);

        // Even with a fight request, PAD doesn't transition (no rules).
        let mut ctx = BehaviorCtx {
            requested_fight: true,
            requested_goto: true,
            behavior_done: true,
            ..Default::default()
        };
        rt.tick(&mut ctx);
        assert_eq!(rt.current_state, pad, "PAD_STATE has no transitions");
    }
}
