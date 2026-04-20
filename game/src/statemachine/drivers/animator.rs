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
use super::parse::{parse_sm, split_call, ActionParser, EventParser};

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

/// Guard predicates for the top-level character-mode FSM.  Designed to be
/// boolean over the per-tick `AnimatorCtx` — no stateful queries live here.
#[derive(Clone, Debug)]
pub enum AnimatorEvent {
    // --- Input-layer (button / command state this tick) --------------------
    JumpPressed,
    CrouchPressed,
    SlidePressed,
    EvadePressed,
    WeaponToggled,
    PickupPressed,
    /// AI / script asked to start a CustomAnim this tick.
    CustomAnimRequested,

    // --- Physics / world contact ------------------------------------------
    GroundLost,
    GroundRegained,
    WallGrabAvailable,
    ZiplineAvailable,
    HighVelocityLanding,

    // --- Animation lifecycle ----------------------------------------------
    /// The currently-running mode animation reached its final frame.
    ModeAnimDone,

    // --- Damage / death ---------------------------------------------------
    Damaged,
    HealthZero,

    // --- Query ------------------------------------------------------------
    /// Convenience predicate: true iff the ctx reports we're in the given
    /// mode (matched against the state name literally).  Lets states share
    /// transition shortcuts without having to re-check which mode is active.
    InMode(String),

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
    // Input (held / pressed this tick)
    pub jump_pressed: bool,
    pub crouch_pressed: bool,
    pub slide_pressed: bool,
    pub evade_pressed: bool,
    pub weapon_toggled: bool,
    pub pickup_pressed: bool,
    pub custom_anim_requested: bool,

    // Physics
    pub ground_lost_this_tick: bool,
    pub ground_regained_this_tick: bool,
    pub wall_grab_available: bool,
    pub zipline_available: bool,
    pub high_velocity_landing: bool,

    // Animation
    pub mode_anim_done: bool,

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

    fn eval_event(
        ctx: &Self::Context,
        event: &Self::Event,
        _runtime: &SmRuntime<Self>,
    ) -> bool {
        match event {
            AnimatorEvent::JumpPressed => ctx.jump_pressed,
            AnimatorEvent::CrouchPressed => ctx.crouch_pressed,
            AnimatorEvent::SlidePressed => ctx.slide_pressed,
            AnimatorEvent::EvadePressed => ctx.evade_pressed,
            AnimatorEvent::WeaponToggled => ctx.weapon_toggled,
            AnimatorEvent::PickupPressed => ctx.pickup_pressed,
            AnimatorEvent::CustomAnimRequested => ctx.custom_anim_requested,
            AnimatorEvent::GroundLost => ctx.ground_lost_this_tick,
            AnimatorEvent::GroundRegained => ctx.ground_regained_this_tick,
            AnimatorEvent::WallGrabAvailable => ctx.wall_grab_available,
            AnimatorEvent::ZiplineAvailable => ctx.zipline_available,
            AnimatorEvent::HighVelocityLanding => ctx.high_velocity_landing,
            AnimatorEvent::ModeAnimDone => ctx.mode_anim_done,
            AnimatorEvent::Damaged => ctx.damage_this_tick,
            AnimatorEvent::HealthZero => ctx.health_zero,
            AnimatorEvent::InMode(name) => ctx.current_mode.eq_ignore_ascii_case(name),
            AnimatorEvent::Always => true,
        }
    }

    fn apply_action(
        _ctx: &mut Self::Context,
        action: &Self::Action,
        output: &mut Self::Output,
        adv: &mut SmAdvance<Self>,
    ) {
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
    _state_index: &HashMap<String, usize>,
) -> Result<AnimatorEvent, String> {
    let text = text.trim();
    let (name, args) = split_call(text);

    Ok(match name {
        "JumpPressed" => AnimatorEvent::JumpPressed,
        "CrouchPressed" => AnimatorEvent::CrouchPressed,
        "SlidePressed" => AnimatorEvent::SlidePressed,
        "EvadePressed" => AnimatorEvent::EvadePressed,
        "WeaponToggled" => AnimatorEvent::WeaponToggled,
        "PickupPressed" => AnimatorEvent::PickupPressed,
        "CustomAnimRequested" => AnimatorEvent::CustomAnimRequested,
        "GroundLost" => AnimatorEvent::GroundLost,
        "GroundRegained" => AnimatorEvent::GroundRegained,
        "WallGrabAvailable" => AnimatorEvent::WallGrabAvailable,
        "ZiplineAvailable" => AnimatorEvent::ZiplineAvailable,
        "HighVelocityLanding" => AnimatorEvent::HighVelocityLanding,
        "ModeAnimDone" | "Timeout" => AnimatorEvent::ModeAnimDone,
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
    parse_sm::<AnimatorDriver>(
        ANIMATOR_FSM,
        ANIMATOR_EVENT_PARSER,
        ANIMATOR_ACTION_PARSER,
    )
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

#IDLE
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if GroundLost                 { Broadcast StartFall;          goto FALL }
if JumpPressed                { Broadcast StartJumpCompress;  goto JUMP }
if EvadePressed               { Broadcast StartEvade;         goto EVADE }
if SlidePressed               { Broadcast StartSlide;         goto SLIDE }
if CrouchPressed              { Broadcast StartCrouch;        goto CROUCH }
if WallGrabAvailable          { Broadcast StartLedgeGrab;     goto LEDGE_HANG }
if ZiplineAvailable           { Broadcast StartZiplineGrab;   goto ZIPLINE }
if PickupPressed              { Broadcast StartPickup;        goto PICKUP }
if CustomAnimRequested        { Broadcast StartCustomAnim;    goto CUSTOM_ANIM }

#JUMP
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if GroundRegained             { Broadcast StartLand;          goto LAND }
if Timeout                    { Broadcast StartFall;          goto FALL }
if WallGrabAvailable          { Broadcast StartLedgeGrab;     goto LEDGE_HANG }

#FALL
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if HighVelocityLanding        { Broadcast StartHardLand;      goto LAND }
if GroundRegained             { Broadcast StartLand;          goto LAND }
if WallGrabAvailable          { Broadcast StartLedgeGrab;     goto LEDGE_HANG }
if ZiplineAvailable           { Broadcast StartZiplineGrab;   goto ZIPLINE }

#LAND
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if Timeout                    { goto IDLE }
if JumpPressed                { Broadcast StartJumpCompress;  goto JUMP }

#SLIDE
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if GroundLost                 { Broadcast StartFall;          goto FALL }
if Timeout                    { goto IDLE }
if JumpPressed                { Broadcast StartJumpCompress;  goto JUMP }

#EVADE
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if Timeout                    { goto IDLE }

#CROUCH
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if !CrouchPressed             { Broadcast EndCrouch;          goto IDLE }
if JumpPressed                { Broadcast StartJumpCompress;  goto JUMP }

#LEDGE_HANG
if HealthZero                 { Broadcast StartDeath;         goto DIE }
if Damaged                    { Broadcast StartReact;         goto REACT }
if JumpPressed                { Broadcast StartLedgeClamber;  goto LEDGE_CLAMBER }
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

        let mut ctx = AnimatorCtx {
            jump_pressed: true,
            current_mode: "IDLE".into(),
            ..Default::default()
        };
        let out = rt.tick(&mut ctx);
        assert_eq!(rt.current_state, jump);
        assert!(out.broadcasts.iter().any(|b| b == "StartJumpCompress"));
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

        let mut ctx = AnimatorCtx {
            jump_pressed: true,
            mode_anim_done: true,
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
            mode_anim_done: true,
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
        let data = super::parse_sm::<AnimatorDriver>(
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
        assert!(out.should_destroy, "Destroy action should set should_destroy");
    }
}
