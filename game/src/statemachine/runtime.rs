/*
 * statemachine/runtime.rs — FsmRuntime component and fsm_update_system.
 *
 * FsmRuntime holds per-entity FSM execution state: current state index, active
 * animation name, timed_out flag.  fsm_update_system evaluates pad flags from
 * InputState + FightVectorTrigger hits each frame, fires transitions, and posts
 * AttackMessage / updates Oni2AnimState when a new animation starts.
 */
use bevy::prelude::*;
use std::sync::Arc;

use crate::fight_vector::{FightVectorTrigger, facing_within, find_fight_trigger_vector};
use crate::oni2_loader::{Oni2AnimLibrary, Oni2AnimState};
use crate::player::components::{InputState, Player};
use crate::combat::components::Fighter;
use super::types::*;
use super::types::{ctrl_flags, pad_flags};

// ---------------------------------------------------------------------------
// FsmRuntime component
// ---------------------------------------------------------------------------

/// Per-entity FSM runtime.  Add this alongside FsmData to enable FSM-driven
/// animation for any entity (player or AI).
#[derive(Component)]
pub struct FsmRuntime {
    pub data: Arc<FsmData>,
    /// Current state index into data.states.
    pub current_state: usize,
    /// Name of the animation last started by a DoAttack/DoBlock action.
    pub active_anim: Option<String>,
    /// True once the active_anim has reached its final frame.
    pub timed_out: bool,
    /// Elapsed time when the last ResetTimer action fired.
    timer_start: f32,
    /// Running total of elapsed time (updated every tick).
    elapsed: f32,
    /// Per-tick random value [0, 1).
    random_pct: f32,
    /// Simple xorshift seed for cheap random.
    rng_seed: u64,
    /// Attack button pressed during the queue window — fires when branch window opens.
    pub queued_attack: bool,
    pub queued_attack_two: bool,
}

impl FsmRuntime {
    pub fn new(data: Arc<FsmData>) -> Self {
        let initial = data.initial_state_idx();
        FsmRuntime {
            data,
            current_state: initial,
            active_anim: None,
            timed_out: false,
            timer_start: 0.0,
            elapsed: 0.0,
            random_pct: 0.0,
            rng_seed: 0xdeadbeef_cafebabe,
            queued_attack: false,
            queued_attack_two: false,
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn advance_rng(&mut self) -> f32 {
        self.rng_seed ^= self.rng_seed << 13;
        self.rng_seed ^= self.rng_seed >> 7;
        self.rng_seed ^= self.rng_seed << 17;
        (self.rng_seed >> 32) as f32 / u32::MAX as f32
    }

    fn is_anim_done(&self, anim_state: &Oni2AnimState) -> bool {
        if self.active_anim.is_none() {
            return false;
        }
        let total = anim_state.anim.num_frames as f32;
        !anim_state.looping && anim_state.current_time >= (total - 1.0).max(0.0)
    }

    fn eval_event(&self, event: &FsmEvent, packet: &FsmPacket, anim_state: &Oni2AnimState) -> bool {
        match event {
            FsmEvent::Packet(cond) => {
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
                if let Some(req) = cond.class_hit {
                    if packet.class_hit != req {
                        return false;
                    }
                }
                true
            }
            FsmEvent::Timeout => self.timed_out || self.is_anim_done(anim_state),
            FsmEvent::Me(flags) => {
                *flags == 0 || (packet.me_flags & flags) == *flags
            }
            FsmEvent::Target(flags) => {
                *flags == 0 || (packet.target_flags & flags) == *flags
            }
            FsmEvent::True => true,
            FsmEvent::Random(p) => self.random_pct < *p,
            FsmEvent::Timer(t) => self.elapsed - self.timer_start >= *t,
            FsmEvent::CustomAnimDone => self.is_anim_done(anim_state),
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Tick the FSM: evaluate the current state's rules against `packet` and
    /// produce an `FsmOutput`.  May change `current_state`.
    ///
    /// `fight_vector_anim` is the pre-resolved fight-vector attack name (if the
    /// player is standing inside a trigger and facing within 30°).  It is passed
    /// to `DoTriggerAtk` so the action can succeed or fail inline — a failed action
    /// lets rule evaluation continue to the next rule rather than halting.
    pub fn update(&mut self, packet: &FsmPacket, anim_state: &Oni2AnimState, fight_vector_anim: Option<&str>) -> FsmOutput {
        let mut output = FsmOutput::default();
        self.evaluate_state(self.current_state, packet, anim_state, &mut output, 0, fight_vector_anim);
        output
    }

    /// Evaluate `state_idx`'s rules.  Returns true if a state transition occurred.
    /// `depth` guards against infinite Check chains.
    fn evaluate_state(
        &mut self,
        state_idx: usize,
        packet: &FsmPacket,
        anim_state: &Oni2AnimState,
        output: &mut FsmOutput,
        depth: u32,
        fight_vector_anim: Option<&str>,
    ) -> bool {
        if depth > 16 {
            warn!("FSM: Check recursion limit hit");
            return false;
        }

        // Clone the rule list so we can mutate `self` (timer, state, etc.) while iterating.
        let rules = match self.data.states.get(state_idx) {
            Some(s) => s.rules.clone(),
            None => return false,
        };

        for rule in &rules {
            let matches = self.eval_event(&rule.event, packet, anim_state);
            if matches == rule.negated {
                // negated XOR result: rule does NOT fire
                continue;
            }

            // Execute actions.
            // `action_failed`: when true the goto is suppressed and evaluation
            // continues to the next rule — the loop only stops when the state
            // actually changes.
            let mut action_failed = false;

            for action in &rule.actions {
                match action {
                    FsmAction::DoAttack { anim_name, rotation_notches } => {
                        output.attack_anim = Some((anim_name.clone(), *rotation_notches));
                        self.active_anim = Some(anim_name.clone());
                        self.timed_out = false;
                    }
                    FsmAction::DoBlock { anim_name } => {
                        output.block_anim = Some(anim_name.clone());
                        self.active_anim = Some(anim_name.clone());
                        self.timed_out = false;
                    }
                    FsmAction::DoCounter => {
                        output.counter = true;
                    }
                    FsmAction::DoTriggerAtk => {
                        // Only succeeds when a fight-vector trigger is in range
                        // and the character is facing within 30°.
                        // On failure, rule evaluation continues to the next rule.
                        if let Some(anim) = fight_vector_anim {
                            output.attack_anim = Some((anim.to_string(), 0));
                            self.active_anim = Some(anim.to_string());
                            self.timed_out = false;
                            info!("FSM: DoTriggerAtk (fight vector) → '{}'", anim);
                        } else {
                            action_failed = true;
                        }
                    }
                    FsmAction::DoEvade { anim_name, mirror } => {
                        output.evade_anim = Some((anim_name.clone(), *mirror));
                        self.active_anim = Some(anim_name.clone());
                        self.timed_out = false;
                    }
                    FsmAction::CtrlGrapple { action } => {
                        output.ctrl_grapple = Some(action.clone());
                    }
                    FsmAction::ChangeAttack { anim_name, rotation_notches } => {
                        output.attack_anim = Some((anim_name.clone(), *rotation_notches));
                        self.active_anim = Some(anim_name.clone());
                        self.timed_out = false;
                    }
                    FsmAction::CustomAnim { anim_name } => {
                        output.custom_anim = Some(anim_name.clone());
                        self.active_anim = Some(anim_name.clone());
                        self.timed_out = false;
                    }
                    FsmAction::ResetTimer => {
                        self.timer_start = self.elapsed;
                    }
                    FsmAction::SetDone | FsmAction::EnablePad | FsmAction::DisablePad => {}
                    FsmAction::Display(msg) => {
                        info!("FSM: {}", msg);
                    }
                    FsmAction::Check(check_idx) => {
                        let idx = *check_idx;
                        if self.evaluate_state(idx, packet, anim_state, output, depth + 1, fight_vector_anim) {
                            // The Check caused a state transition — stop evaluating this state
                            return true;
                        }
                    }
                }
            }

            // If the action failed, don't apply the goto — continue to the next
            // rule — only stops when the state actually changes.
            if action_failed {
                continue;
            }

            // Apply goto
            if let Some(next) = rule.goto_state {
                if next != self.current_state {
                    debug!(
                        "FSM: {} → {}",
                        self.data.state_name(self.current_state),
                        self.data.state_name(next)
                    );
                    self.current_state = next;
                    self.timed_out = false;
                }
                // After a Timeout goto, cascade-evaluate the destination state
                // immediately in the same tick so that queued input (e.g. an attack
                // press queued during the animation) fires in ATTACK_START without
                // waiting a frame.  Depth guard: only cascade at depth 0 to prevent
                // infinite Timeout chains when the new state also has a live Timeout.
                if matches!(&rule.event, FsmEvent::Timeout) && depth == 0 {
                    self.evaluate_state(next, packet, anim_state, output, depth + 1, fight_vector_anim);
                }
                return true;
            }

            // Rule fired, no goto — technically handled, but we MUST cascade and evaluate
            // downstream rules, allowing inline Check and action macros to not stall the engine!
            continue;
        }

        false
    }
}

// ---------------------------------------------------------------------------
// Bevy system: build FsmPacket and tick every FsmRuntime on player entities
// ---------------------------------------------------------------------------

/// Build the FSM input packet from the player's InputState and Fighter status.
pub fn build_fsm_packet(input: &InputState, fighter: &Fighter) -> FsmPacket {
    let mut pad: u64 = 0;
    let mut ctrl: u32 = 0;

    // --- control flags from fighter state ---
    if fighter.is_grounded {
        let speed = input.movement.length();
        if speed > 0.6 {
            ctrl |= ctrl_flags::RUNNING;
        } else if speed > 0.05 {
            ctrl |= ctrl_flags::WALKING;
        } else {
            ctrl |= ctrl_flags::STANDING;
        }
    } else {
        ctrl |= ctrl_flags::JUMPING;
    }

    if input.blocking {
        ctrl |= ctrl_flags::CAN_BLOCK;
    }

    // --- pad commands from logical input ---
    if input.blocking {
        pad |= pad_flags::PADCMD_BLOCK;
    }

    if input.grab {
        pad |= pad_flags::PADCMD_GRAPPLE;
    }

    // Attack buttons — directional variants derived from movement vector
    // movement: y < 0 = forward (W), y > 0 = back (S), x > 0 = left (A), x < 0 = right (D)
    let mx = input.movement.x;
    let my = input.movement.y;
    let moving = input.movement.length_squared() > 0.09; // ≈ 0.3 threshold

    if input.attack {
        let flag = if moving {
            directional_attack_flag(mx, my, false)
        } else {
            pad_flags::PADCMD_ATTACK_HIGH
        };
        pad |= flag;

        // Add RUNNING ctrl flag for forward attacks while moving
        if my < -0.3 && fighter.is_grounded {
            ctrl |= ctrl_flags::RUNNING;
        }
    }

    if input.attack_two {
        let flag = if moving {
            directional_attack_flag(mx, my, true)
        } else {
            pad_flags::PADCMD_ATTACK_TWO_HIGH
        };
        pad |= flag;
    }

    let me_flags = 0u32; // filled in when grapple is implemented
    let target_flags = 0u32;

    FsmPacket {
        pad_flags: pad,
        ctrl_flags: ctrl,
        class_hit: -1,
        me_flags,
        target_flags,
    }
}

/// Apply ATDT combo-linking timing windows to produce a gated InputState and
/// the CRITICAL_FRAME ctrl flag.
///
/// Windows:
///   0.0 .. opp2_q_start   → no-queue: ignore attack input entirely
///   opp2_q_start .. opp2_do_start → queue: remember press, don't fire yet
///   opp2_do_start .. opp2_do_end  → branch/CRITICAL_FRAME: fire queued or new press
///   > opp2_do_end                 → window closed (input ignored until next anim)
///
/// Returns (gated InputState, is_critical_frame).
pub fn apply_timing_windows(
    runtime: &mut FsmRuntime,
    input: &InputState,
    anim_state: &Oni2AnimState,
) -> (InputState, bool) {
    use crate::oni2_loader::parsers::atdt::AtdtStrike;

    // If the active attack animation has finished, release any queued input so it
    // fires immediately in the next ready state (ATTACK_START) via the Timeout cascade.
    if runtime.timed_out {
        let mut gated = input.clone();
        gated.attack     = input.attack     || runtime.queued_attack;
        gated.attack_two = input.attack_two || runtime.queued_attack_two;
        if gated.attack     { runtime.queued_attack     = false; }
        if gated.attack_two { runtime.queued_attack_two = false; }
        return (gated, false);
    }

    // Only gate input when actively playing an attack animation with ATDT data
    let strike: Option<&AtdtStrike> = anim_state
        .anim
        .attack_data
        .as_ref()
        .and_then(|a| a.strike.as_ref());

    let Some(strike) = strike else {
        // Not in an attack animation — pass input through, clear queue
        runtime.queued_attack = false;
        runtime.queued_attack_two = false;
        return (input.clone(), false);
    };

    if anim_state.anim.num_frames <= 1 {
        return (input.clone(), false);
    }

    let phase = anim_state.current_time / (anim_state.anim.num_frames as f32 - 1.0).max(1.0);

    // No-queue window: ignore input, clear any stale queue
    if phase < strike.opp2_q_start {
        runtime.queued_attack = false;
        runtime.queued_attack_two = false;
        let mut gated = input.clone();
        gated.attack = false;
        gated.attack_two = false;
        return (gated, false);
    }

    // Queue window: remember press, don't fire
    if phase < strike.opp2_do_start {
        if strike.queue_next_attack {
            if input.attack { runtime.queued_attack = true; }
            if input.attack_two { runtime.queued_attack_two = true; }
        }
        let mut gated = input.clone();
        gated.attack = false;
        gated.attack_two = false;
        return (gated, false);
    }

    // Branch window (CRITICAL_FRAME): fire queued or current press
    if phase <= strike.opp2_do_end {
        let mut gated = input.clone();
        gated.attack = input.attack || runtime.queued_attack;
        gated.attack_two = input.attack_two || runtime.queued_attack_two;
        // Consume the queue once it fires
        if gated.attack { runtime.queued_attack = false; }
        if gated.attack_two { runtime.queued_attack_two = false; }
        return (gated, true); // is_critical_frame = true
    }

    // Post-branch window: closed, ignore input
    let mut gated = input.clone();
    gated.attack = false;
    gated.attack_two = false;
    (gated, false)
}

/// Map a normalised movement vector + attack button index onto the correct ACK flag.
fn directional_attack_flag(mx: f32, my: f32, is_two: bool) -> u64 {
    // Classify direction: my < 0 = forward, my > 0 = back, mx > 0 = left, mx < 0 = right
    let fwd = my < -0.3;
    let back = my > 0.3;
    let left = mx > 0.3;
    let right = mx < -0.3;

    if is_two {
        match (fwd, back, left, right) {
            (true, _, true, _) => pad_flags::ACK_TWO_FORWARD_LEFT,
            (true, _, _, true) => pad_flags::ACK_TWO_FORWARD_RIGHT,
            (_, true, true, _) => pad_flags::ACK_TWO_BACKWARD_LEFT,
            (_, true, _, true) => pad_flags::ACK_TWO_BACKWARD_RIGHT,
            (_, true, _, _) => pad_flags::ACK_TWO,
            (_, _, true, _) => pad_flags::ACK_TWO_LEFT,
            (_, _, _, true) => pad_flags::ACK_TWO_RIGHT,
            _ => pad_flags::PADCMD_ATTACK_TWO_HIGH,
        }
    } else {
        match (fwd, back, left, right) {
            (true, _, true, _) => pad_flags::ACK_FORWARD_LEFT,
            (true, _, _, true) => pad_flags::ACK_FORWARD_RIGHT,
            (_, true, true, _) => pad_flags::ACK_BACKWARD_LEFT,
            (_, true, _, true) => pad_flags::ACK_BACKWARD_RIGHT,
            (_, true, _, _) => pad_flags::ACK,
            (_, _, true, _) => pad_flags::ACK_LEFT,
            (_, _, _, true) => pad_flags::ACK_RIGHT,
            _ => pad_flags::PADCMD_ATTACK_HIGH,
        }
    }
}

/// Update system: ticks every player's FsmRuntime and applies animation outputs.
pub fn fsm_update_system(
    time: Res<Time>,
    mut query: Query<
        (
            &mut FsmRuntime,
            &InputState,
            &Fighter,
            &mut Oni2AnimState,
            &Oni2AnimLibrary,
            &GlobalTransform,
        ),
        With<Player>,
    >,
    triggers: Query<(&GlobalTransform, &FightVectorTrigger)>,
) {
    for (mut runtime, input, fighter, mut anim_state, anim_lib, gtf) in &mut query {
        let dt = time.delta_secs();
        runtime.elapsed += dt;
        runtime.random_pct = runtime.advance_rng();

        // Check if the currently tracked animation just finished
        if !runtime.timed_out && runtime.is_anim_done(&anim_state) {
            runtime.timed_out = true;
        }

        // Clear queued input when a new animation starts (stale queue from prior anim)
        if anim_state.current_anim_id != anim_state.previous_anim_id
            && anim_state.previous_anim_id.is_some()
        {
            runtime.queued_attack = false;
            runtime.queued_attack_two = false;
        }

        // Apply ATDT combo-linking timing windows before building the FSM packet.
        // This gates attack input based on animation phase and sets CRITICAL_FRAME.
        let (gated_input, is_critical_frame) =
            apply_timing_windows(&mut runtime, input, &anim_state);

        let mut packet = build_fsm_packet(&gated_input, fighter);
        if is_critical_frame {
            packet.ctrl_flags |= ctrl_flags::CRITICAL_FRAME;
        }

        // Pre-resolve fight vector so DoTriggerAtk can succeed or fail inline.
        // Query level-placed triggers and angle-check facing for DoTriggerAtk.
        let fighter_pos = gtf.translation();
        let fight_vector_anim: Option<String> =
            find_fight_trigger_vector(fighter_pos, &triggers).and_then(|fv| {
                if facing_within(fighter.facing, fv.direction, 30.0) {
                    Some(fv.attack_alias)
                } else {
                    None
                }
            });

        // Log state + packet every time attack is pressed
        if packet.pad_flags != 0 {
            info!(
                "FSM tick: state='{}' pad=0x{:x} ctrl=0x{:x}{}",
                runtime.data.state_name(runtime.current_state),
                packet.pad_flags,
                packet.ctrl_flags,
                if is_critical_frame { " [CRITICAL]" } else { "" },
            );
        }

        let output = runtime.update(&packet, &anim_state, fight_vector_anim.as_deref());

        if let Some((anim_name, _rotation)) = &output.attack_anim {
            info!("FSM: DoAttack → '{}'", anim_name);
            if !anim_lib.play(anim_name, &mut anim_state) {
                warn!("FSM: attack anim NOT in library: '{}' (lib has {} anims)", anim_name, anim_lib.anims.len());
            }
        } else if let Some(anim_name) = &output.block_anim {
            info!("FSM: DoBlock → '{}'", anim_name);
            if !anim_lib.play(anim_name, &mut anim_state) {
                warn!("FSM: block anim NOT in library: '{}'", anim_name);
            }
        } else if let Some((anim_name, _mirror)) = &output.evade_anim {
            if !anim_lib.play(anim_name, &mut anim_state) {
                warn!("FSM: evade anim NOT in library: '{}'", anim_name);
            }
        } else if let Some(anim_name) = &output.custom_anim {
            if !anim_lib.play(anim_name, &mut anim_state) {
                warn!("FSM: custom anim NOT in library: '{}'", anim_name);
            }
        }
    }
}
