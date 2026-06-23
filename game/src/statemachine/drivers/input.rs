/*
 * statemachine/drivers/input.rs — InputDriver: pad-driven player FSM.
 *
 * Mirrors legacy aiInputStateMachine (behavior/istatemachine).  The text format
 * is the existing `.fsm` and the event/action vocabularies are the
 * already-defined `FsmEvent`/`FsmAction` enums in `statemachine::types`.
 *
 * This driver is the NEW path that re-uses those vocabularies through the
 * generic `SmDriver` machinery.  The legacy `FsmRuntime` in
 * `statemachine::runtime` continues to drive the player today; once parity is
 * proven this driver can replace it without changing the on-disk format or
 * the consumer API.
 *
 * Responsibilities:
 *   • `eval_event` — pure predicate evaluation against the per-tick `InputCtx`
 *   • `apply_action` — emit animation requests into `FsmOutput`, mutate flags
 *     on `InputCtx`, and request goto/check/fail through `SmAdvance`
 *   • `parse_input_event` / `parse_input_action` — text → enum (pluggable into
 *     the shared `parse_sm` skeleton)
 */
use std::collections::HashMap;

use super::super::core::{SmAdvance, SmDriver, SmRuntime};
use super::super::types::{
    FsmAction, FsmEvent, FsmOutput, FsmPacket, PacketCondition, ctrl_flags, entity_flags, pad_flags,
};
use super::parse::{ActionParser, EventParser, split_call};

// ---------------------------------------------------------------------------
// InputDriver
// ---------------------------------------------------------------------------

/// Marker driver type — implements `SmDriver` to bind the input FSM
/// vocabularies into the generic state-machine engine.
pub struct InputDriver;

/// Per-tick context passed into `tick(&mut ctx)`.  The host system rebuilds
/// this each frame from `PadMapper` / `Fighter` / `Oni2AnimState` / fight-vector
/// triggers and reads `output` afterwards to apply animation playback.
#[derive(Default)]
pub struct InputCtx {
    /// Input + body-state flags assembled by `build_fsm_packet`.
    pub packet: FsmPacket,

    /// Name of the animation last started by `DoAttack`/`DoBlock`/etc.  Used
    /// only for diagnostics and to gate the `Timeout` event when paired with
    /// the anim_* fields below.
    pub active_anim: Option<String>,

    /// Anim-playback snapshot for the currently active clip.  These three
    /// mirror the bits of `Oni2AnimState` the FSM cares about so the driver
    /// stays decoupled from the renderer's component shape.
    pub anim_num_frames: u32,
    pub anim_current_time: f32,
    pub anim_looping: bool,

    /// Set externally when the active animation has reached its final frame.
    pub timed_out: bool,

    /// Combo queue: button presses captured in the queue window that should
    /// fire the moment the branch window opens (see `apply_timing_windows`).
    pub queued_attack: bool,
    pub queued_attack_critical: bool,
    pub queued_attack_two: bool,
    pub queued_attack_two_critical: bool,

    /// Pre-resolved fight-vector attack (entity inside trigger AND facing
    /// within 30°).  Consumed by `DoTriggerAtk`; absence makes that action
    /// fail-and-continue.
    pub fight_vector_anim: Option<String>,

    /// Accumulates consecutive running time for running attacks.
    pub running_timer: f32,
}

impl InputCtx {
    pub fn is_anim_done(&self) -> bool {
        if self.active_anim.is_none() {
            return false;
        }
        let total = self.anim_num_frames as f32;
        !self.anim_looping && self.anim_current_time >= (total - 1.0).max(0.0)
    }
}

impl SmDriver for InputDriver {
    type Event = FsmEvent;
    type Action = FsmAction;
    type Context = InputCtx;
    type Output = FsmOutput;

    fn eval_event(ctx: &Self::Context, event: &Self::Event, runtime: &SmRuntime<Self>) -> bool {
        match event {
            FsmEvent::Packet(cond) => packet_matches(&ctx.packet, cond),
            FsmEvent::Timeout => ctx.timed_out || ctx.is_anim_done(),
            FsmEvent::Me(flags) => *flags == 0 || (ctx.packet.me_flags & flags) == *flags,
            FsmEvent::Target(flags) => *flags == 0 || (ctx.packet.target_flags & flags) == *flags,
            FsmEvent::True => true,
            FsmEvent::Random(p) => runtime.random_pct < *p,
            FsmEvent::Timer(t) => runtime.elapsed - runtime.timer_start >= *t,
            FsmEvent::CustomAnimDone => ctx.is_anim_done(),
        }
    }

    fn apply_action(
        ctx: &mut Self::Context,
        action: &Self::Action,
        output: &mut Self::Output,
        adv: &mut SmAdvance<Self>,
    ) {
        match action {
            FsmAction::DoAttack {
                anim_name,
                rotation_notches,
            } => {
                output.attack_anim = Some((anim_name.clone(), *rotation_notches));
                ctx.active_anim = Some(anim_name.clone());
                ctx.timed_out = false;
            }
            FsmAction::DoBlock { anim_name } => {
                output.block_anim = Some(anim_name.clone());
                ctx.active_anim = Some(anim_name.clone());
                ctx.timed_out = false;
            }
            FsmAction::DoCounter => {
                output.counter = true;
            }
            FsmAction::DoTriggerAtk => {
                if let Some(anim) = ctx.fight_vector_anim.clone() {
                    output.attack_anim = Some((anim.clone(), 0));
                    ctx.active_anim = Some(anim);
                    ctx.timed_out = false;
                } else {
                    adv.fail();
                }
            }
            FsmAction::DoEvade { anim_name, mirror } => {
                output.evade_anim = Some((anim_name.clone(), *mirror));
                ctx.active_anim = Some(anim_name.clone());
                ctx.timed_out = false;
            }
            FsmAction::CtrlGrapple { action } => {
                output.ctrl_grapple = Some(action.clone());
            }
            FsmAction::ChangeAttack {
                anim_name,
                rotation_notches,
            } => {
                output.attack_anim = Some((anim_name.clone(), *rotation_notches));
                ctx.active_anim = Some(anim_name.clone());
                ctx.timed_out = false;
            }
            FsmAction::CustomAnim { anim_name } => {
                output.custom_anim = Some(anim_name.clone());
                ctx.active_anim = Some(anim_name.clone());
                ctx.timed_out = false;
            }
            FsmAction::Check(state_idx) => {
                adv.check(*state_idx);
            }
            FsmAction::ResetTimer => adv.reset_timer(),
            FsmAction::SetDone | FsmAction::EnablePad | FsmAction::DisablePad => {
                // No-op in the current input pipeline (pads are always live).
            }
            FsmAction::Display(msg) => {
                bevy::log::info!("FSM: {}", msg);
            }
        }
    }

    fn cascade_after_event(event: &Self::Event) -> bool {
        // Mirrors legacy FsmRuntime: a Timeout-driven goto re-evaluates the
        // destination state in the same tick so queued input fires without
        // one frame of latency.
        matches!(event, FsmEvent::Timeout)
    }
}

// ---------------------------------------------------------------------------
// Packet condition helper
// ---------------------------------------------------------------------------

fn packet_matches(packet: &FsmPacket, cond: &PacketCondition) -> bool {
    // Guard against an all-empty `PacketCondition`, which would otherwise
    // match every packet (every guard below short-circuits on `!= 0`).
    // An empty condition is almost always a parser failure — the source
    // text said `if Packet(SOMETHING)` but every token in `SOMETHING`
    // was unrecognized, so all bits ended up zero.  Returning false here
    // makes the rule reliably never fire instead of always fire.
    //
    // Caught a real bug: enemy.fsm's `if Packet(PADCMD_ATTACK_LOW)` rule
    // was being treated as "match anything" because PADCMD_ATTACK_LOW was
    // missing from `build_pad_table`, causing every spawned AI fighter to
    // immediately dispatch ANIMATTACK_2 on tick 0.
    if cond.pad_flags == 0
        && cond.ctrl_flags == 0
        && cond.not_pad_flags == 0
        && cond.not_ctrl_flags == 0
        && cond.class_hit.is_none()
        && cond.has_weapon_name.is_none()
        && cond.not_has_weapon_name.is_none()
    {
        // Diagnostic for the empty-condition case.  If any legitimate
        // rule was relying on the old "match everything" behavior, we'll
        // see this warning and can decide whether to convert it to
        // `if Always` (the right way to express "fire every tick") or
        // relax the guard.  One-shot per process so a per-tick rule
        // doesn't flood the log.
        use std::sync::atomic::{AtomicBool, Ordering};
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            bevy::log::warn!(
                "packet_matches: encountered an empty PacketCondition — \
                 returning false (rule will never fire).  This is the \
                 anti-`every-AI-fires-ANIMATTACK_2-on-spawn` guard.  If \
                 expected, the source rule should use `if Always` instead \
                 of `if Packet()`.  This warning fires only once per process."
            );
        }
        return false;
    }
    if cond.pad_flags != 0 && (packet.pad_flags & cond.pad_flags) != cond.pad_flags {
        return false;
    }
    if cond.ctrl_flags != 0 && (packet.ctrl_flags & cond.ctrl_flags) != cond.ctrl_flags {
        return false;
    }
    if cond.not_pad_flags != 0 && (packet.pad_flags & cond.not_pad_flags) != 0 {
        return false;
    }
    if cond.not_ctrl_flags != 0 && (packet.ctrl_flags & cond.not_ctrl_flags) != 0 {
        return false;
    }
    if let Some(req) = cond.class_hit
        && packet.class_hit != req
    {
        return false;
    }
    if let Some(req_w) = &cond.has_weapon_name {
        if let Some(packet_w) = &packet.has_weapon {
            if !packet_w.contains(req_w) {
                return false;
            }
        } else {
            return false;
        }
    }
    if let Some(req_w) = &cond.not_has_weapon_name
        && let Some(packet_w) = &packet.has_weapon
        && packet_w.contains(req_w)
    {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Text → enum parsers (plug into parse::parse_sm)
// ---------------------------------------------------------------------------

/// Wire pad/ctrl/entity flag tables once — built lazily on first parser call.
struct InputFlagTables {
    pad: HashMap<&'static str, u64>,
    ctrl: HashMap<&'static str, u32>,
    ent: HashMap<&'static str, u32>,
}

fn flag_tables() -> &'static InputFlagTables {
    use std::sync::OnceLock;
    static TABLES: OnceLock<InputFlagTables> = OnceLock::new();
    TABLES.get_or_init(|| InputFlagTables {
        pad: build_pad_table(),
        ctrl: build_ctrl_table(),
        ent: build_entity_table(),
    })
}

pub fn parse_input_event(
    text: &str,
    _state_index: &HashMap<String, usize>,
) -> Result<FsmEvent, String> {
    let text = text.trim();
    let tables = flag_tables();

    // An empty event clause is legitimate authoring shorthand for
    // "always fire" — the legacy convention is `{ actions }` (no `if`)
    // means an unconditional rule.  Real example in shipped player.fsm
    // (around line 1673):
    //   ; Check Standard Forward Diagonal Attacks
    //   ;if True
    //   {
    //       Check FWD_ATTACK_TWO_DEFS
    //   }
    // Author commented out the `if True` keeping just the action block.
    // Treat empty exactly as `True` — same enum variant, no warning.
    if text.is_empty() {
        return Ok(FsmEvent::True);
    }

    if text.starts_with("Timeout") {
        return Ok(FsmEvent::Timeout);
    }
    if text.starts_with("True") {
        return Ok(FsmEvent::True);
    }
    if text.starts_with("CustomAnimDone") {
        return Ok(FsmEvent::CustomAnimDone);
    }

    let (name, args) = split_call(text);
    if !args.is_empty() || text.contains('(') {
        return match name {
            "Packet" => Ok(FsmEvent::Packet(parse_packet_args(args, tables)?)),
            "Me" => Ok(FsmEvent::Me(parse_entity_flags(args, &tables.ent))),
            "Target" => Ok(FsmEvent::Target(parse_entity_flags(args, &tables.ent))),
            "Random" => Ok(FsmEvent::Random(args.parse::<f32>().unwrap_or(0.5))),
            "Timer" => Ok(FsmEvent::Timer(args.parse::<f32>().unwrap_or(1.0))),
            _ => Err(format!("input: unknown event '{}'", name)),
        };
    }

    // Space-separated form: "Random 0.5"
    let mut parts = text.splitn(2, ' ');
    let n = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match n {
        "Random" => Ok(FsmEvent::Random(arg.parse::<f32>().unwrap_or(0.5))),
        "Timer" => Ok(FsmEvent::Timer(arg.parse::<f32>().unwrap_or(1.0))),
        _ => Err(format!("input: unknown event '{}'", n)),
    }
}

pub fn parse_input_action(
    line: &str,
    state_index: &HashMap<String, usize>,
) -> Result<Option<FsmAction>, String> {
    let mut parts = line.splitn(3, char::is_whitespace);
    let verb = parts.next().unwrap_or("").trim();
    let arg1 = parts.next().unwrap_or("").trim();
    let arg2 = parts.next().unwrap_or("").trim();

    let action = match verb {
        "DoAttack" => {
            if arg1.is_empty() {
                return Ok(None);
            }
            FsmAction::DoAttack {
                anim_name: arg1.to_string(),
                rotation_notches: arg2.parse::<i32>().unwrap_or(0),
            }
        }
        "DoBlock" => {
            if arg1.is_empty() {
                return Ok(None);
            }
            FsmAction::DoBlock {
                anim_name: arg1.to_string(),
            }
        }
        "DoCounter" => FsmAction::DoCounter,
        "DoTriggerAtk" | "TriggerAtk" => FsmAction::DoTriggerAtk,
        "DoEvade" => {
            if arg1.is_empty() {
                return Ok(None);
            }
            FsmAction::DoEvade {
                anim_name: arg1.to_string(),
                mirror: arg2 == "mirror" || arg2 == "true",
            }
        }
        "CtrlGrapple" => FsmAction::CtrlGrapple {
            action: arg1.to_string(),
        },
        "ChangeAttack" => {
            if arg1.is_empty() {
                return Ok(None);
            }
            FsmAction::ChangeAttack {
                anim_name: arg1.to_string(),
                rotation_notches: arg2.parse::<i32>().unwrap_or(0),
            }
        }
        "CustomAnim" => {
            if arg1.is_empty() {
                return Ok(None);
            }
            FsmAction::CustomAnim {
                anim_name: arg1.to_string(),
            }
        }
        "Check" => match state_index.get(arg1) {
            Some(&idx) => FsmAction::Check(idx),
            None => return Ok(None),
        },
        "ResetTimer" => FsmAction::ResetTimer,
        "SetDone" | "ASetDone" => FsmAction::SetDone,
        "EnablePad" | "AEnabledPad" => FsmAction::EnablePad,
        "DisablePad" | "ADisablePad" => FsmAction::DisablePad,
        "Display" => FsmAction::Display(arg1.to_string()),
        _ => return Ok(None),
    };
    Ok(Some(action))
}

/// Type-erased aliases for use with parse_sm.
pub const INPUT_EVENT_PARSER: EventParser<InputDriver> = parse_input_event;
pub const INPUT_ACTION_PARSER: ActionParser<InputDriver> = parse_input_action;

// ---------------------------------------------------------------------------
// Flag tables — same content as parser.rs for parity
// ---------------------------------------------------------------------------

fn build_pad_table() -> HashMap<&'static str, u64> {
    let mut m = HashMap::new();
    m.insert("PADCMD_ATTACK_HIGH", pad_flags::PADCMD_ATTACK_HIGH);
    m.insert("PADCMD_ATTACK_LOW", pad_flags::PADCMD_ATTACK_LOW);
    m.insert("PADCMD_ATTACK_TWO_HIGH", pad_flags::PADCMD_ATTACK_TWO_HIGH);
    m.insert("PADCMD_ACTION", pad_flags::PADCMD_ACTION);
    m.insert("PADCMD_SHAKE", pad_flags::PADCMD_SHAKE);
    m.insert("PADCMD_BLOCK", pad_flags::PADCMD_BLOCK);
    m.insert("PADCMD_SWEEP_FORWARD", pad_flags::PADCMD_SWEEP_FORWARD);
    m.insert("PADCMD_GRAPPLE", pad_flags::PADCMD_GRAPPLE);
    m.insert("PADCMD_GRAPPLE_ATK", pad_flags::PADCMD_GRAPPLE_ATK);
    m.insert("PADCMD_GRAPPLE_ATK_2", pad_flags::PADCMD_GRAPPLE_ATK_2);
    m.insert("PADCMD_END_GRAPPLE", pad_flags::PADCMD_END_GRAPPLE);
    m.insert("PADCMD_LARIAT", pad_flags::PADCMD_LARIAT);
    m.insert("PADCMD_WEAPON_LOCKON", pad_flags::PADCMD_WEAPON_LOCKON);
    m.insert(
        "PADCMD_WEAPON_FIRE_FORWARD",
        pad_flags::PADCMD_WEAPON_FIRE_FORWARD,
    );
    m.insert(
        "PADCMD_WEAPON_FIRE_LEFT",
        pad_flags::PADCMD_WEAPON_FIRE_LEFT,
    );
    m.insert(
        "PADCMD_WEAPON_FIRE_RIGHT",
        pad_flags::PADCMD_WEAPON_FIRE_RIGHT,
    );
    m.insert(
        "PADCMD_WEAPON_FIRE_BACK",
        pad_flags::PADCMD_WEAPON_FIRE_BACK,
    );
    m.insert(
        "PADCMD_WEAPON_FIRE_BACK_LEFT",
        pad_flags::PADCMD_WEAPON_FIRE_BACK_LEFT,
    );
    m.insert(
        "PADCMD_WEAPON_FIRE_BACK_RIGHT",
        pad_flags::PADCMD_WEAPON_FIRE_BACK_RIGHT,
    );
    m.insert(
        "PADCMD_WEAPON_FIRE_FWD_LEFT",
        pad_flags::PADCMD_WEAPON_FIRE_FWD_LEFT,
    );
    m.insert(
        "PADCMD_WEAPON_FIRE_FWD_RIGHT",
        pad_flags::PADCMD_WEAPON_FIRE_FWD_RIGHT,
    );
    m.insert(
        "PADCMD_WEAPON_FIRE_FRONT_LEFT",
        pad_flags::PADCMD_WEAPON_FIRE_FWD_LEFT,
    );
    m.insert(
        "PADCMD_WEAPON_FIRE_FRONT_RIGHT",
        pad_flags::PADCMD_WEAPON_FIRE_FWD_RIGHT,
    );
    m.insert("PADCMD_CHR_FWD", pad_flags::PADCMD_CHR_FWD);
    m.insert("PADCMD_CHR_BAK", pad_flags::PADCMD_CHR_BAK);
    m.insert("PADCMD_CHR_LFT", pad_flags::PADCMD_CHR_LFT);
    m.insert("PADCMD_CHR_RGH", pad_flags::PADCMD_CHR_RGH);
    m.insert(
        "PADCMD_REDIRECT_FWD_BACK",
        pad_flags::PADCMD_REDIRECT_FWD_BACK,
    );
    m.insert(
        "PADCMD_REDIRECT_FWD_BACK_LEFT",
        pad_flags::PADCMD_REDIRECT_FWD_BACK_LEFT,
    );
    m.insert(
        "PADCMD_REDIRECT_FWD_BACK_RIGHT",
        pad_flags::PADCMD_REDIRECT_FWD_BACK_RIGHT,
    );
    m.insert("ACK", pad_flags::ACK);
    m.insert("ACK_LEFT", pad_flags::ACK_LEFT);
    m.insert("ACK_RIGHT", pad_flags::ACK_RIGHT);
    m.insert("ACK_FORWARD_LEFT", pad_flags::ACK_FORWARD_LEFT);
    m.insert("ACK_FORWARD_RIGHT", pad_flags::ACK_FORWARD_RIGHT);
    m.insert("ACK_BACKWARD_LEFT", pad_flags::ACK_BACKWARD_LEFT);
    m.insert("ACK_BACKWARD_RIGHT", pad_flags::ACK_BACKWARD_RIGHT);
    m.insert("ACK_TWO", pad_flags::ACK_TWO);
    m.insert("ACK_TWO_LEFT", pad_flags::ACK_TWO_LEFT);
    m.insert("ACK_TWO_RIGHT", pad_flags::ACK_TWO_RIGHT);
    m.insert("ACK_TWO_FORWARD_LEFT", pad_flags::ACK_TWO_FORWARD_LEFT);
    m.insert("ACK_TWO_FORWARD_RIGHT", pad_flags::ACK_TWO_FORWARD_RIGHT);
    m.insert("ACK_TWO_BACKWARD_LEFT", pad_flags::ACK_TWO_BACKWARD_LEFT);
    m.insert("ACK_TWO_BACKWARD_RIGHT", pad_flags::ACK_TWO_BACKWARD_RIGHT);
    m
}

fn build_ctrl_table() -> HashMap<&'static str, u32> {
    let mut m = HashMap::new();
    m.insert("REACT", ctrl_flags::REACT);
    m.insert("STANDING", ctrl_flags::STANDING);
    m.insert("WALKING", ctrl_flags::WALKING);
    m.insert("RUNNING", ctrl_flags::RUNNING);
    m.insert("REDIRECT", ctrl_flags::REDIRECT);
    m.insert("POSTHIT", ctrl_flags::POSTHIT);
    m.insert("BLOCK_SUCCESS", ctrl_flags::BLOCK_SUCCESS);
    m.insert("CRITICAL_FRAME", ctrl_flags::CRITICAL_FRAME);
    m.insert("JUMPING", ctrl_flags::JUMPING);
    m.insert("CROUCHING", ctrl_flags::CROUCHING);
    m.insert("RUNNING_JUMP", ctrl_flags::RUNNING_JUMP);
    m.insert("STANDING_JUMP", ctrl_flags::STANDING_JUMP);
    m.insert("WEAPON_DRAWN", ctrl_flags::WEAPON_DRAWN);
    m.insert("CAN_BLOCK", ctrl_flags::CAN_BLOCK);
    m.insert("HITTARGET", ctrl_flags::HITTARGET);
    m.insert("FIGHT_MODE", ctrl_flags::FIGHT_MODE);
    m.insert("ENEMY_IN_SLICE", ctrl_flags::ENEMY_IN_SLICE);
    m.insert("STARTING_CROUCH", ctrl_flags::STARTING_CROUCH);
    m.insert("ENDING_CROUCH", ctrl_flags::ENDING_CROUCH);
    m
}

fn build_entity_table() -> HashMap<&'static str, u32> {
    let mut m = HashMap::new();
    m.insert("GRAPPLING", entity_flags::GRAPPLING);
    m.insert("GETTING_GRAPPLED", entity_flags::GETTING_GRAPPLED);
    m
}

fn parse_packet_args(args: &str, tables: &InputFlagTables) -> Result<PacketCondition, String> {
    let mut cond = PacketCondition::default();

    for token in args.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        let is_negated = token.starts_with('!');
        let key = if is_negated { token[1..].trim() } else { token };

        if let Some(rest) = key.strip_prefix("CLASSHIT:") {
            cond.class_hit = rest.trim().parse::<i32>().ok();
        } else if let Some(weapon) = key.strip_prefix("HAS_WEAPON:") {
            let weapon_name = weapon.trim().to_uppercase();
            if is_negated {
                cond.not_has_weapon_name = Some(weapon_name);
            } else {
                cond.has_weapon_name = Some(weapon_name);
            }
        } else if let Some(&bit) = tables.pad.get(key) {
            if is_negated {
                cond.not_pad_flags |= bit;
            } else {
                cond.pad_flags |= bit;
            }
        } else if let Some(&bit) = tables.ctrl.get(key) {
            if is_negated {
                cond.not_ctrl_flags |= bit;
            } else {
                cond.ctrl_flags |= bit;
            }
        } else {
            bevy::log::warn!("input: unknown Packet arg '{}'", token);
        }
    }

    Ok(cond)
}

fn parse_entity_flags(args: &str, ent: &HashMap<&str, u32>) -> u32 {
    let mut flags = 0u32;
    for token in args.split(',') {
        if let Some(&bit) = ent.get(token.trim()) {
            flags |= bit;
        }
    }
    flags
}
