/*
 * statemachine/drivers/animator.rs — AnimatorDriver: top-level character mode
 * orchestrator.
 *
 * Maps the legacy action-player flag-bitmasks and substate
 * hierarchy onto a single-cursor FSM.  The cursor
 * IS the character's high-level mode (Idle / Jump / Fall / Land / LedgeHang /
 * Crouch / Slide / Zipline / React / FightStance / DrawWeapon / Die / Pickup
 * / CustomAnim).  The 18 primary modes collapse to a similar set here —
 * overlapping flags in the legacy engine (crouched AND weapon-drawn, etc.) live on
 * separate components, not in the FSM state.
 *
 * Underlying systems do the real work.  The jump system isn't an FSM — it's
 * a velocity/gravity/animation pipeline that kicks off when this FSM enters
 * `JUMP`.  Fight is mostly an FSM (see FightDriver).  Zipline is a path-
 * follower.  Etc.  Each system watches this driver's `current_state` cursor
 * (or listens for `Broadcast` actions) and does its thing.
 *
 * Assets are shipped inside the .dat so there's no on-disk FSM file — the
 * initial machine definition is embedded as a string constant at the bottom
 * of this file.  Call `load_embedded()` once at startup to parse it into a
 * shareable `SmData<AnimatorDriver>`.
 */
use std::collections::HashMap;
use std::sync::Arc;

use super::super::core::{SmAdvance, SmData, SmDriver, SmRuntime};
use super::parse::{ActionParser, EventParser};
use super::parse_input_fsm::parse_input_fsm;

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

/// Guard predicates for the top-level character-mode FSM.  Designed to be
/// boolean over the per-tick `AnimatorCtx` — no stateful queries live here.
#[derive(Clone, Debug)]
pub enum AnimatorEvent {
    // --- Input-layer (button / command state this tick) --------------------
    /// Held — true for every tick the named pad command is currently active
    /// (value > 0.0 in the PadMapper).  Use for CONDITIONS that should
    /// persist while the button is down, e.g. `!PadCmd "PADCMD_CROUCH"`
    /// as an exit guard in #CROUCH.
    PadCmd(String),
    /// Edge — true only on the tick the named pad command just transitioned
    /// from inactive to active.  Use for ONE-SHOT actions like entering
    /// JUMP / EVADE / SLIDE / PICKUP — otherwise a held button repeatedly
    /// re-fires the rule and stomps the in-progress action's state.
    PadPressed(String),
    JumpsAvailable,
    /// True while the actor is airborne in the JUMP_MAIN sub_state_1 — i.e.
    /// past the COMPRESS phase and not yet in LAND.  Used to gate the
    /// double-jump re-entry: a second `PADCMD_JUMP` press is only valid
    /// after the first jump has fully launched (mirrors legacy
    /// `if (SubState1 != ACT_S1_JUMP_MAIN) return ACT_DENIED;` in
    /// the legacy `ActionStartJump` gate).  Without this gate, spamming jump
    /// during COMPRESS re-fires the takeoff schedule.
    JumpInAir,
    /// AI / script asked to start a CustomAnim this tick.
    CustomAnimRequested,

    // --- Physics / world contact ------------------------------------------
    GroundLost,
    GroundRegained,
    WallGrabAvailable,
    ZiplineAvailable,
    HighVelocityLanding,

    // --- Animation / Action lifecycle -------------------------------------
    /// The currently-running mode action reached completion.
    ModeDone,

    // --- Damage / death ---------------------------------------------------
    Damaged,
    HealthZero,

    // --- Query ------------------------------------------------------------
    /// Convenience predicate: true iff the ctx reports we're in the given
    /// mode (matched against the state name literally).  Lets states share
    /// transitions (e.g. "if InMode('FALL') ...").
    InMode(String),

    // --- Composables ------------------------------------------------------
    And(Vec<AnimatorEvent>),
    Or(Vec<AnimatorEvent>),
    Always,
}

/// Side-effects an `apply_action` branch can request.  The FSM state cursor
/// itself is the mode signal — `Broadcast` is for edge-triggered events
/// (e.g. "StartJumpCompress" fires the jump-system kickoff).
#[derive(Clone, Debug)]
pub enum AnimatorAction {
    /// Fire a named host event.  The host system routes these strings to
    /// whatever subsystem should react (jump impulse, slide-starter, etc).
    Broadcast(String),
    /// Request the host to despawn this entity.  Mirrors the legacy animator's
    /// `GetParent().Destroy()` at death-anim completion — currently unused
    /// in the embedded FSM because the health component handles actor
    /// cleanup via its own timeout, but the vocabulary supports it so
    /// future cases can express "anim-done → despawn" declaratively.
    Destroy,
    /// Inline-evaluate another state's rules without moving the cursor.
    Check(usize),
    /// Diagnostic.
    Display(String),
}

// ---------------------------------------------------------------------------
// Context + output
// ---------------------------------------------------------------------------

/// Per-tick context.  The host rebuilds this each frame from the player's
/// input state, the physics snapshot, and any incoming damage this tick.
///
/// Single-tick edges (e.g. `ground_lost_this_tick`) must be computed by the
/// host — the FSM only reads, never derives.
#[derive(Default, Clone, Debug)]
pub struct AnimatorCtx {
    /// Pad commands currently held (value > 0.0 this tick).  Matches the
    /// semantic of `PadCmd(...)` events — held-state checks.
    pub active_pad_cmds: std::collections::HashSet<String>,
    /// Pad commands that TRANSITIONED from inactive to active this tick —
    /// i.e. edge-triggered "just pressed" signals.  Computed by the host
    /// from the delta between this tick's `active_pad_cmds` and last
    /// tick's.  Matches the semantic of `PadPressed(...)` events.
    pub pressed_pad_cmds: std::collections::HashSet<String>,
    pub custom_anim_requested: bool,
    pub jumps_available: bool,
    pub jump_in_air: bool,

    // Physics
    pub ground_lost_this_tick: bool,
    pub ground_regained_this_tick: bool,
    pub wall_grab_available: bool,
    pub zipline_available: bool,
    pub high_velocity_landing: bool,

    // Action Pipeline / Animation
    pub mode_done: bool,

    // Damage
    pub damage_this_tick: bool,
    pub health_zero: bool,

    /// Name of the state we're currently in — filled in by the host from
    /// `runtime.sm.data.state_name(runtime.sm.current_state)` so `InMode`
    /// predicates work without the driver having to peek at the runtime.
    pub current_mode: String,
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct AnimatorOutput {
    /// Ordered list of edge-triggered event names the host should route this
    /// tick.  Typical values: "StartJumpCompress", "ApplyJumpImpulse",
    /// "StartSlide", "EndReact", etc.
    pub broadcasts: Vec<String>,
    /// True if the FSM fired a `Destroy` action this tick — the host should
    /// despawn the entity.  Separate from `broadcasts` so the host doesn't
    /// have to string-match, and so forgetting to wire a broadcast doesn't
    /// silently drop a despawn request.
    pub should_destroy: bool,
}

// ---------------------------------------------------------------------------
// Driver impl
// ---------------------------------------------------------------------------

pub struct AnimatorDriver;

impl SmDriver for AnimatorDriver {
    type Event = AnimatorEvent;
    type Action = AnimatorAction;
    type Context = AnimatorCtx;
    type Output = AnimatorOutput;

    fn eval_event(ctx: &Self::Context, event: &Self::Event, runtime: &SmRuntime<Self>) -> bool {
        match event {
            AnimatorEvent::PadCmd(name) => ctx.active_pad_cmds.contains(name),
            AnimatorEvent::PadPressed(name) => ctx.pressed_pad_cmds.contains(name),
            AnimatorEvent::JumpsAvailable => ctx.jumps_available,
            AnimatorEvent::JumpInAir => ctx.jump_in_air,
            AnimatorEvent::CustomAnimRequested => ctx.custom_anim_requested,
            AnimatorEvent::GroundLost => ctx.ground_lost_this_tick,
            AnimatorEvent::GroundRegained => ctx.ground_regained_this_tick,
            AnimatorEvent::WallGrabAvailable => ctx.wall_grab_available,
            AnimatorEvent::ZiplineAvailable => ctx.zipline_available,
            AnimatorEvent::HighVelocityLanding => ctx.high_velocity_landing,
            AnimatorEvent::ModeDone => ctx.mode_done,
            AnimatorEvent::Damaged => ctx.damage_this_tick,
            AnimatorEvent::HealthZero => ctx.health_zero,
            AnimatorEvent::InMode(name) => ctx.current_mode.eq_ignore_ascii_case(name),
            AnimatorEvent::And(events) => events.iter().all(|e| Self::eval_event(ctx, e, runtime)),
            AnimatorEvent::Or(events) => events.iter().any(|e| Self::eval_event(ctx, e, runtime)),
            AnimatorEvent::Always => true,
        }
    }

    fn apply_action(
        _ctx: &mut Self::Context,
        action: &Self::Action,
        output: &mut Self::Output,
        adv: &mut SmAdvance<Self>,
    ) {
        bevy::log::info!("animator: apply_action {:?}", action);
        match action {
            AnimatorAction::Broadcast(name) => output.broadcasts.push(name.clone()),
            AnimatorAction::Destroy => output.should_destroy = true,
            AnimatorAction::Check(idx) => adv.check(*idx),
            AnimatorAction::Display(msg) => bevy::log::info!("animator: {}", msg),
        }
    }
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

pub fn parse_animator_event(
    text: &str,
    state_idx: &HashMap<String, usize>,
) -> Result<AnimatorEvent, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("empty animator event".to_string());
    }

    if text.contains("||") {
        let mut parts = Vec::new();
        for piece in text.split("||") {
            parts.push(parse_animator_event(piece, state_idx)?);
        }
        return Ok(AnimatorEvent::Or(parts));
    }

    if text.contains("&&") {
        let mut parts = Vec::new();
        for piece in text.split("&&") {
            parts.push(parse_animator_event(piece, state_idx)?);
        }
        return Ok(AnimatorEvent::And(parts));
    }

    // Accept two call-syntax forms for the same rule:
    //   Name(arg)       — call-style with parens (handled by split_call)
    //   Name arg        — space-separated (common in the embedded FSM)
    // When split_call finds no parens, fall back to whitespace tokenizing.
    let (name, args) = {
        let (n, a) = crate::statemachine::drivers::parse::split_call(text);
        if a.is_empty() && n.contains(char::is_whitespace) {
            let mut it = n.splitn(2, char::is_whitespace);
            (
                it.next().unwrap_or("").trim(),
                it.next().unwrap_or("").trim(),
            )
        } else {
            (n, a)
        }
    };

    Ok(match name {
        "PadCmd" => AnimatorEvent::PadCmd(args.trim_matches('"').to_string()),
        "PadPressed" => AnimatorEvent::PadPressed(args.trim_matches('"').to_string()),
        "JumpsAvailable" => AnimatorEvent::JumpsAvailable,
        "JumpInAir" => AnimatorEvent::JumpInAir,
        "CustomAnimRequested" => AnimatorEvent::CustomAnimRequested,
        "GroundLost" => AnimatorEvent::GroundLost,
        "GroundRegained" => AnimatorEvent::GroundRegained,
        "WallGrabAvailable" => AnimatorEvent::WallGrabAvailable,
        "ZiplineAvailable" => AnimatorEvent::ZiplineAvailable,
        "HighVelocityLanding" => AnimatorEvent::HighVelocityLanding,
        "ModeDone" | "ModeAnimDone" | "Timeout" => AnimatorEvent::ModeDone,
        "Damaged" => AnimatorEvent::Damaged,
        "HealthZero" => AnimatorEvent::HealthZero,
        "InMode" => AnimatorEvent::InMode(args.trim_matches('"').to_string()),
        "Always" | "True" => AnimatorEvent::Always,
        other => return Err(format!("animator: unknown event '{}'", other)),
    })
}

pub fn parse_animator_action(
    line: &str,
    state_index: &HashMap<String, usize>,
) -> Result<Option<AnimatorAction>, String> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or("").trim();
    let arg = parts.next().unwrap_or("").trim();

    Ok(Some(match verb {
        "Broadcast" => {
            if arg.is_empty() {
                return Ok(None);
            }
            AnimatorAction::Broadcast(arg.trim_matches('"').to_string())
        }
        "Destroy" => AnimatorAction::Destroy,
        "Check" => match state_index.get(arg) {
            Some(&idx) => AnimatorAction::Check(idx),
            None => return Ok(None),
        },
        "Display" => AnimatorAction::Display(arg.to_string()),
        _ => return Ok(None),
    }))
}

pub const ANIMATOR_EVENT_PARSER: EventParser<AnimatorDriver> = parse_animator_event;
pub const ANIMATOR_ACTION_PARSER: ActionParser<AnimatorDriver> = parse_animator_action;

// ---------------------------------------------------------------------------
// Embedded FSM + loader
// ---------------------------------------------------------------------------

/// Parse the embedded FSM text into a reusable `SmData<AnimatorDriver>`.
/// Call once at startup and store the result in a Bevy resource (Arc-shared
/// across every character entity).
pub fn load_embedded() -> Result<SmData<AnimatorDriver>, String> {
    parse_input_fsm::<AnimatorDriver>(ANIMATOR_FSM, ANIMATOR_EVENT_PARSER, ANIMATOR_ACTION_PARSER)
}

/// Convenience: wrap `load_embedded` into an `Arc` for sharing via a resource.
pub fn load_embedded_arc() -> Result<Arc<SmData<AnimatorDriver>>, String> {
    load_embedded().map(Arc::new)
}

/// High-level character-mode orchestration FSM.
///
/// The *state cursor itself is the mode* — no flags, no overlapping.  When
/// transitioning, `Broadcast` fires edge-triggered side effects (e.g.
/// "StartJumpCompress") that the host routes to whatever subsystem owns that
/// mode's implementation.
///
/// States deliberately omit weapon-related overlays (DrawWeapon / holster)
/// and fight-stance transitions — those are orthogonal and live on sibling
/// components.  The FSM only tracks locomotion / damage / action mode.
pub const ANIMATOR_FSM: &str = r#"
; Animator top-level orchestration FSM.
; States = high-level modes.  Transitions on per-tick input / physics events.
; Each transition fires a Broadcast naming the mode's kickoff event so the
; host can route it to whichever subsystem owns that mode's implementation
; (jump pipeline, fall gravity, ledge IK, reaction anim, etc.).  Exit
; broadcasts also fire where the outgoing mode has teardown work (EndReact,
; EndZipline, EndCrouch).  `Timeout` is the canonical ONI2 anim-done alias
; accepted in addition to `ModeAnimDone`.

; One-shot enters use `PadPressed` (edge) so a held button doesn't re-fire
; the rule and stomp the in-progress schedule.  Held-state queries use
; `PadCmd` (e.g. !PadCmd PADCMD_CROUCH as a stay-in-stance guard).

#IDLE
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if GroundLost                 { Broadcast StartFall;          goto FALL }
if PadPressed("PADCMD_JUMP")  { Broadcast StartJumpCompress;  goto JUMP }
if PadPressed("PADCMD_EVADE") { Broadcast StartEvade;         goto EVADE }
if PadPressed("PADCMD_SLIDE") { Broadcast StartSlide;         goto SLIDE }
if PadPressed("PADCMD_CROUCH"){ Broadcast StartCrouch;        goto CROUCH }
if WallGrabAvailable          { Broadcast StartLedgeGrab;     goto LEDGE_HANG }
if ZiplineAvailable           { Broadcast StartZiplineGrab;   goto ZIPLINE }
if PadPressed("PADCMD_PICKUP"){ Broadcast StartPickup;        goto PICKUP }
if CustomAnimRequested        { Broadcast StartCustomAnim;    goto CUSTOM_ANIM }

#JUMP
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if GroundRegained             { Broadcast StartLand;          goto LAND }
if WallGrabAvailable          { Broadcast StartLedgeGrab;     goto LEDGE_HANG }
; Double-jump gate: requires `JumpInAir` so the second press is rejected
; while still in COMPRESS.  Mirrors the legacy substate guard.
if PadPressed("PADCMD_JUMP") && JumpsAvailable && JumpInAir { Broadcast StartJumpCompress;  goto JUMP }

#FALL
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if HighVelocityLanding        { Broadcast StartHardLand;      goto LAND }
if GroundRegained             { Broadcast StartLand;          goto LAND }
if WallGrabAvailable          { Broadcast StartLedgeGrab;     goto LEDGE_HANG }
if ZiplineAvailable           { Broadcast StartZiplineGrab;   goto ZIPLINE }
if PadPressed("PADCMD_JUMP") && JumpsAvailable { Broadcast StartJumpCompress;  goto JUMP }

#LAND
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if Timeout                    { goto IDLE }
if PadPressed("PADCMD_JUMP")  { Broadcast StartJumpCompress;  goto JUMP }

#SLIDE
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if GroundLost                 { Broadcast StartFall;          goto FALL }
if Timeout                    { goto IDLE }
if PadPressed("PADCMD_JUMP")  { Broadcast StartJumpCompress;  goto JUMP }

#EVADE
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if Timeout                    { goto IDLE }

#CROUCH
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if !PadCmd "PADCMD_CROUCH"    { Broadcast EndCrouch;          goto IDLE }
if PadPressed "PADCMD_JUMP"   { Broadcast StartJumpCompress;  goto JUMP }

#LEDGE_HANG
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if PadPressed "PADCMD_JUMP"   { Broadcast StartLedgeClamber;  goto LEDGE_CLAMBER }
if !WallGrabAvailable         { Broadcast StartFall;          goto FALL }

#LEDGE_CLAMBER
if Timeout                    { goto IDLE }

#ZIPLINE
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast EndZipline;         Broadcast StartReact; goto REACT }
if !ZiplineAvailable          { Broadcast EndZipline;         goto FALL }
if JumpPressed                { Broadcast EndZipline;         goto FALL }
if Timeout                    { Broadcast EndZipline;         goto IDLE }

#PICKUP
if Timeout                    { goto IDLE }

#CUSTOM_ANIM
if Damaged                    { Broadcast StartReact;         goto REACT }
if Timeout                    { goto IDLE }

#REACT
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Timeout                    { Broadcast EndReact;           goto IDLE }

#DIE
; Terminal mode.  Health component handles actor destruction on its own
; timeout; the animator FSM just holds here so no further modes can fire.
; The `Destroy` action is available (see AnimatorAction::Destroy) and could
; be wired in via `if Timeout { Destroy }` for cases where anim-done should
; trigger despawn directly — mirrors the commented-out legacy engine
; behaviour of despawning at death-anim completion.
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fsm_parses() {
        let data = load_embedded().expect("animator FSM parses");
        // Spot-check a handful of expected states.
        for name in &["IDLE", "JUMP", "FALL", "LAND", "LEDGE_HANG", "REACT", "DIE"] {
            assert!(
                data.state_index.contains_key(*name),
                "missing state {}",
                name
            );
        }
        assert!(data.states.len() >= 13, "expected ≥13 states");
    }

    #[test]
    fn idle_transitions_on_jump() {
        let data = Arc::new(load_embedded().expect("parses"));
        let idle = data.index_of_or_zero("IDLE");
        let jump = data.index_of_or_zero("JUMP");
        let mut rt = SmRuntime::<AnimatorDriver>::new(data, idle);

        // IDLE's jump rule now uses `PadPressed` (edge) not `PadCmd` (held)
        // so a held button doesn't re-fire each tick and stomp the jump
        // schedule.  Populate `pressed_pad_cmds` to simulate the "button
        // just transitioned from up to down" tick.
        let mut pressed_pad_cmds = std::collections::HashSet::new();
        pressed_pad_cmds.insert("PADCMD_JUMP".into());
        let mut active_pad_cmds = std::collections::HashSet::new();
        active_pad_cmds.insert("PADCMD_JUMP".into());
        let mut ctx = AnimatorCtx {
            active_pad_cmds,
            pressed_pad_cmds,
            current_mode: "IDLE".into(),
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, jump);
        assert!(out.broadcasts.iter().any(|b| b == "StartJumpCompress"));
    }

    #[test]
    fn held_pad_jump_does_not_re_fire_in_jump_state() {
        // Regression: while the button is HELD and we're already in JUMP,
        // the rule `PadPressed("PADCMD_JUMP") && JumpsAvailable` must not
        // re-fire.  Before the edge split, `PadCmd("PADCMD_JUMP")` was
        // true every held tick and stomped the in-progress jump schedule
        // with a second JUMP_SOMERSAULT schedule on tick 1.
        let data = Arc::new(load_embedded().expect("parses"));
        let jump = data.index_of_or_zero("JUMP");
        let mut rt = SmRuntime::<AnimatorDriver>::new(data, jump);

        let mut active_pad_cmds = std::collections::HashSet::new();
        active_pad_cmds.insert("PADCMD_JUMP".into());
        // Held but NOT pressed this tick (edge is absent).
        let mut ctx = AnimatorCtx {
            active_pad_cmds,
            pressed_pad_cmds: Default::default(),
            jumps_available: true,
            current_mode: "JUMP".into(),
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, jump, "should stay in JUMP while held");
        assert!(
            !out.broadcasts.iter().any(|b| b == "StartJumpCompress"),
            "held button must not re-fire StartJumpCompress, got: {:?}",
            out.broadcasts
        );
    }

    #[test]
    fn pressing_jump_during_compress_phase_does_not_re_fire() {
        // Regression: while in #JUMP and in the COMPRESS phase (not yet
        // airborne in JUMP_MAIN), a fresh `PADCMD_JUMP` press must NOT
        // trigger a second StartJumpCompress.  Mirrors the legacy
        // `if(SubState1 != ACT_S1_JUMP_MAIN) return ACT_DENIED;` gate.
        let data = Arc::new(load_embedded().expect("parses"));
        let jump = data.index_of_or_zero("JUMP");
        let mut rt = SmRuntime::<AnimatorDriver>::new(data, jump);

        let mut active_pad_cmds = std::collections::HashSet::new();
        active_pad_cmds.insert("PADCMD_JUMP".into());
        let mut pressed_pad_cmds = std::collections::HashSet::new();
        pressed_pad_cmds.insert("PADCMD_JUMP".into());
        let mut ctx = AnimatorCtx {
            active_pad_cmds,
            pressed_pad_cmds,
            jumps_available: true,
            jump_in_air: false, // still in COMPRESS — NOT in MAIN yet
            current_mode: "JUMP".into(),
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, jump);
        assert!(
            !out.broadcasts.iter().any(|b| b == "StartJumpCompress"),
            "press during COMPRESS must not re-fire StartJumpCompress, got: {:?}",
            out.broadcasts
        );
    }

    #[test]
    fn pressing_jump_in_main_phase_fires_somersault() {
        // Companion to the above: once SubState1 is JUMP_MAIN and the
        // button is freshly pressed, the rule fires the second jump.
        let data = Arc::new(load_embedded().expect("parses"));
        let jump = data.index_of_or_zero("JUMP");
        let mut rt = SmRuntime::<AnimatorDriver>::new(data, jump);

        let mut active_pad_cmds = std::collections::HashSet::new();
        active_pad_cmds.insert("PADCMD_JUMP".into());
        let mut pressed_pad_cmds = std::collections::HashSet::new();
        pressed_pad_cmds.insert("PADCMD_JUMP".into());
        let mut ctx = AnimatorCtx {
            active_pad_cmds,
            pressed_pad_cmds,
            jumps_available: true,
            jump_in_air: true,
            current_mode: "JUMP".into(),
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert!(
            out.broadcasts.iter().any(|b| b == "StartJumpCompress"),
            "press during JUMP_MAIN must fire StartJumpCompress, got: {:?}",
            out.broadcasts
        );
    }

    #[test]
    fn fall_lands_on_ground() {
        let data = Arc::new(load_embedded().expect("parses"));
        let fall = data.index_of_or_zero("FALL");
        let land = data.index_of_or_zero("LAND");
        let mut rt = SmRuntime::<AnimatorDriver>::new(data, fall);

        let mut ctx = AnimatorCtx {
            ground_regained_this_tick: true,
            current_mode: "FALL".into(),
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, land);
        assert!(out.broadcasts.iter().any(|b| b == "StartLand"));
    }

    #[test]
    fn death_is_terminal() {
        let data = Arc::new(load_embedded().expect("parses"));
        let die = data.index_of_or_zero("DIE");
        let mut rt = SmRuntime::<AnimatorDriver>::new(data, die);

        let mut active_pad_cmds = std::collections::HashSet::new();
        active_pad_cmds.insert("PADCMD_JUMP".into());
        let mut ctx = AnimatorCtx {
            active_pad_cmds,
            mode_done: true,
            damage_this_tick: true,
            current_mode: "DIE".into(),
            ..Default::default()
        };
        rt.tick(&mut ctx);
        assert_eq!(rt.current_state, die, "DIE should have no transitions out");
    }

    #[test]
    fn entering_die_broadcasts_start_death() {
        let data = Arc::new(load_embedded().expect("parses"));
        let idle = data.index_of_or_zero("IDLE");
        let die = data.index_of_or_zero("DIE");
        let mut rt = SmRuntime::<AnimatorDriver>::new(data, idle);

        let mut ctx = AnimatorCtx {
            health_zero: true,
            current_mode: "IDLE".into(),
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, die);
        assert!(
            out.broadcasts.iter().any(|b| b == "StartDeath"),
            "expected StartDeath broadcast on entry to DIE, got {:?}",
            out.broadcasts
        );
    }

    #[test]
    fn react_exit_broadcasts_end_react() {
        let data = Arc::new(load_embedded().expect("parses"));
        let react = data.index_of_or_zero("REACT");
        let idle = data.index_of_or_zero("IDLE");
        let mut rt = SmRuntime::<AnimatorDriver>::new(data, react);

        let mut ctx = AnimatorCtx {
            mode_done: true,
            current_mode: "REACT".into(),
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, idle);
        assert!(
            out.broadcasts.iter().any(|b| b == "EndReact"),
            "expected EndReact broadcast on leaving REACT, got {:?}",
            out.broadcasts
        );
    }

    #[test]
    fn zipline_damage_fires_both_end_and_start() {
        // Damage in #ZIPLINE fires two broadcasts in order — EndZipline to
        // tear down the zipline attachment, then StartReact to kick off the
        // reaction system.
        let data = Arc::new(load_embedded().expect("parses"));
        let zipline = data.index_of_or_zero("ZIPLINE");
        let react = data.index_of_or_zero("REACT");
        let mut rt = SmRuntime::<AnimatorDriver>::new(data, zipline);

        let mut ctx = AnimatorCtx {
            damage_this_tick: true,
            zipline_available: true,
            current_mode: "ZIPLINE".into(),
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, react);
        let end_pos = out.broadcasts.iter().position(|b| b == "EndZipline");
        let start_pos = out.broadcasts.iter().position(|b| b == "StartReact");
        assert!(end_pos.is_some() && start_pos.is_some());
        assert!(end_pos < start_pos, "teardown must precede kickoff");
    }

    #[test]
    fn destroy_action_expressible() {
        // The `Destroy` action isn't fired by the embedded FSM (health
        // component handles actor cleanup), but the vocabulary supports it.
        // Hand-author a tiny FSM that uses it to prove the pipeline works.
        let source = r#"
#START
if Always { Destroy; goto END }

#END
"#;
        let data = super::super::parse_input_fsm::parse_input_fsm::<AnimatorDriver>(
            source,
            ANIMATOR_EVENT_PARSER,
            ANIMATOR_ACTION_PARSER,
        )
        .expect("parses");
        let data = Arc::new(data);
        let start = data.index_of_or_zero("START");
        let end = data.index_of_or_zero("END");
        let mut rt = SmRuntime::<AnimatorDriver>::new(data, start);

        let mut ctx = AnimatorCtx::default();
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, end);
        assert!(
            out.should_destroy,
            "Destroy action should set should_destroy"
        );
    }
}
