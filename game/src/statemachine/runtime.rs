/*
 * statemachine/runtime.rs — FsmRuntime component and fsm_update_system.
 *
 * `FsmRuntime` wraps the generic `SmRuntime<InputDriver>` plus the persistent
 * `InputCtx` state for one player entity.  `fsm_update_system` rebuilds the
 * per-tick parts of the context from `PadMapper`, `Fighter`, and the active
 * animation, applies ATDT timing-window gating, ticks the FSM, and applies
 * the resulting animation outputs.
 */
use bevy::prelude::*;
use std::sync::Arc;

use super::core::{SmData, SmRuntime};
use super::drivers::input::{InputCtx, InputDriver};
use super::types::*;
use super::types::{ctrl_flags, pad_flags};
use crate::animator::events::EndActionMessage;
use crate::combat::components::{Fighter, Health};
use crate::combat::events::DamageMessage;
use crate::control_map::PadMapper;
use crate::fight_vector::{FightVectorTrigger, facing_within, find_fight_trigger_vector};
use crate::oni2_loader::{Oni2AnimLibrary, Oni2AnimState};
use crate::player::components::{InputState, Player};
use crate::statemachine::drivers::animator::AnimatorCtx;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// FsmRuntime — per-entity state-machine runtime
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// FSM action helpers — shared DNA for animation-effect FSM actions
// ---------------------------------------------------------------------------
//
// Both the player's `fsm_update_system` (draining `FsmOutput.attack_anim`
// etc.) and the AI-side `ai_fsm_update_system` historically inlined the
// "play the anim, maybe rotate" code here.  `FightBehavior` (the
// behavior-layer combat shim) wants the same semantics.  Extracting the
// logic into named helpers keeps one implementation of each FSM action
// so a future tuning tweak (e.g. wiring in `rotation_notches` on the AI
// side, or adding a broadcast for `DoAttack` start) lives in one place.
//
// Downstream combat is entirely ATDT-driven off `Oni2AnimState.anim`:
// `attack_sync_system` sees the newly-played animation, populates
// `AttackState`, and `hit_detection_system` / `hit_reaction_system` pick
// up the ATDT strike data (damage, hit window, cylinder slice, reaction)
// without any further wiring on the caller's side.

/// Execute the `DoAttack` FSM action: play an attack animation and
/// optionally apply an FSM-authored facing rotation.  Return value
/// matches `Oni2AnimLibrary::play` — `false` when the alias was missing.
pub fn do_attack(
    anim_lib: &Oni2AnimLibrary,
    anim_state: &mut Oni2AnimState,
    fighter: &mut Fighter,
    anim_name: &str,
    rotation_notches: i32,
) -> bool {
    let ok = anim_lib.play(anim_name, anim_state);
    if ok {
        info!(
            "DoAttack: anim '{}' STARTED: num_frames={} looping={}",
            anim_name, anim_state.anim.num_frames, anim_state.looping
        );
    } else {
        warn!(
            "DoAttack: anim '{}' NOT in library ({} anims). Sample aliases: {:?}",
            anim_name,
            anim_lib.anims.len(),
            anim_lib.aliases().iter().take(8).collect::<Vec<_>>(),
        );
    }
    if rotation_notches != 0 {
        let rads = rotation_notches as f32 * NOTCH_RADIANS;
        fighter.facing = Quat::from_rotation_y(rads) * fighter.facing;
    }
    ok
}

/// Execute the `DoBlock` FSM action: play a block animation.
pub fn do_block(
    anim_lib: &Oni2AnimLibrary,
    anim_state: &mut Oni2AnimState,
    anim_name: &str,
) -> bool {
    let ok = anim_lib.play(anim_name, anim_state);
    if !ok {
        warn!("DoBlock: anim '{}' NOT in library", anim_name);
    }
    ok
}

/// Execute the `DoEvade` FSM action: play an evade animation.  `_mirror`
/// is accepted for future asymmetric-evade support (left-vs-right) but
/// currently unused on both player and AI paths.
pub fn do_evade(
    anim_lib: &Oni2AnimLibrary,
    anim_state: &mut Oni2AnimState,
    anim_name: &str,
    _mirror: bool,
) -> bool {
    let ok = anim_lib.play(anim_name, anim_state);
    if !ok {
        warn!("DoEvade: anim '{}' NOT in library", anim_name);
    }
    ok
}

/// Execute the `PlayCustomAnim` FSM action: play a script-requested
/// animation.
pub fn do_custom_anim(
    anim_lib: &Oni2AnimLibrary,
    anim_state: &mut Oni2AnimState,
    anim_name: &str,
) -> bool {
    let ok = anim_lib.play(anim_name, anim_state);
    if !ok {
        warn!("PlayCustomAnim: anim '{}' NOT in library", anim_name);
    }
    ok
}

// ---------------------------------------------------------------------------
// FsmRuntime — per-entity state-machine runtime
// ---------------------------------------------------------------------------

/// Per-entity FSM runtime.  Holds the generic state-machine cursor plus the
/// driver context whose persistent fields (`active_anim`, `timed_out`,
/// `queued_attack/two`) survive across ticks.
#[derive(Component)]
pub struct FsmRuntime {
    pub sm: SmRuntime<InputDriver>,
    pub ctx: InputCtx,
}

impl FsmRuntime {
    pub fn new(data: Arc<SmData<InputDriver>>) -> Self {
        let initial = data.index_of_or_zero("ATTACK_START");
        FsmRuntime {
            sm: SmRuntime::new(data, initial),
            ctx: InputCtx::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Packet builder
// ---------------------------------------------------------------------------

/// Build the FSM input packet from PadMapper values plus Fighter state.
///
/// PadMapper has already evaluated control.map this frame; we just read
/// command values and map them onto FsmPacket bit-fields.
pub fn build_fsm_packet(
    input: &InputState,
    is_grounded: bool,
    mapper: &PadMapper,
    fighter_state: Option<&crate::fight::components::FighterState>,
    inventory: Option<&crate::inventory::components::Inventory>,
) -> FsmPacket {
    let mut pad: u64 = 0;
    let mut ctrl: u32 = 0;

    // ── Body / controller state flags ─────────────────────────────────────

    if is_grounded {
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

    if mapper.get("PADCMD_BLOCK") > 0.0 {
        ctrl |= ctrl_flags::CAN_BLOCK;
    }
    if mapper.get("PADCMD_WEAPON_FIGHT_MODE") > 0.0 || mapper.get("PADCMD_WEAPON_LOCKON") > 0.0 {
        ctrl |= ctrl_flags::FIGHT_MODE;
    }
    if mapper.get("PADCMD_CROUCH") > 0.0 {
        ctrl |= ctrl_flags::CROUCHING;
    }

    // ── Pad command flags ─────────────────────────────────────────────────

    if mapper.get("PADCMD_BLOCK") > 0.0 {
        pad |= pad_flags::PADCMD_BLOCK;
    }
    if mapper.get("PADCMD_GRAPPLE") > 0.0 {
        pad |= pad_flags::PADCMD_GRAPPLE;
    }
    if mapper.get("PADCMD_GRAPPLE_ATK") > 0.0 {
        pad |= pad_flags::PADCMD_GRAPPLE_ATK;
    }
    if mapper.get("PADCMD_GRAPPLE_ATK_2") > 0.0 {
        pad |= pad_flags::PADCMD_GRAPPLE_ATK_2;
    }
    if mapper.get("PADCMD_END_GRAPPLE") > 0.0 {
        pad |= pad_flags::PADCMD_END_GRAPPLE;
    }

    if mapper.get("PADCMD_ATTACK_HIGH") > 0.0 {
        pad |= pad_flags::PADCMD_ATTACK_HIGH;
    }
    if mapper.get("PADCMD_ATTACK_TWO_HIGH") > 0.0 {
        pad |= pad_flags::PADCMD_ATTACK_TWO_HIGH;
    }

    if mapper.get("PADCMD_SWEEP_FORWARD") > 0.0 {
        pad |= pad_flags::PADCMD_SWEEP_FORWARD;
    }
    if mapper.get("PADCMD_LARIAT") > 0.0 {
        pad |= pad_flags::PADCMD_LARIAT;
    }

    if mapper.get("PADCMD_WEAPON_FIRE") > 0.0 {
        let fwd = mapper.get("PADCMD_CHR_FWD") > 0.0;
        let back = mapper.get("PADCMD_CHR_BAK") > 0.0;
        let left = mapper.get("PADCMD_CHR_LFT") > 0.0;
        let right = mapper.get("PADCMD_CHR_RGH") > 0.0;
        let dir_flag = match (fwd, back, left, right) {
            (true, _, true, _) => pad_flags::PADCMD_WEAPON_FIRE_FWD_LEFT,
            (true, _, _, true) => pad_flags::PADCMD_WEAPON_FIRE_FWD_RIGHT,
            (_, true, true, _) => pad_flags::PADCMD_WEAPON_FIRE_BACK_LEFT,
            (_, true, _, true) => pad_flags::PADCMD_WEAPON_FIRE_BACK_RIGHT,
            (_, true, _, _) => pad_flags::PADCMD_WEAPON_FIRE_BACK,
            (_, _, true, _) => pad_flags::PADCMD_WEAPON_FIRE_LEFT,
            (_, _, _, true) => pad_flags::PADCMD_WEAPON_FIRE_RIGHT,
            _ => pad_flags::PADCMD_WEAPON_FIRE_FORWARD,
        };
        pad |= dir_flag;
    }

    if mapper.get("PADCMD_WEAPON_LOCKON") > 0.0 {
        pad |= pad_flags::PADCMD_WEAPON_LOCKON;
    }

    if mapper.get("PADCMD_CHR_FWD") > 0.0 {
        pad |= pad_flags::PADCMD_CHR_FWD;
    }
    if mapper.get("PADCMD_CHR_BAK") > 0.0 {
        pad |= pad_flags::PADCMD_CHR_BAK;
    }
    if mapper.get("PADCMD_CHR_LFT") > 0.0 {
        pad |= pad_flags::PADCMD_CHR_LFT;
    }
    if mapper.get("PADCMD_CHR_RGH") > 0.0 {
        pad |= pad_flags::PADCMD_CHR_RGH;
    }

    if mapper.get("PADCMD_REDIRECT_FWD_BACK") > 0.0 {
        pad |= pad_flags::PADCMD_REDIRECT_FWD_BACK;
    }
    if mapper.get("PADCMD_REDIRECT_FWD_BACK_LEFT") > 0.0 {
        pad |= pad_flags::PADCMD_REDIRECT_FWD_BACK_LEFT;
    }
    if mapper.get("PADCMD_REDIRECT_FWD_BACK_RIGHT") > 0.0 {
        pad |= pad_flags::PADCMD_REDIRECT_FWD_BACK_RIGHT;
    }

    if mapper.get("ACK") > 0.0 {
        pad |= pad_flags::ACK;
    }
    if mapper.get("ACK_LEFT") > 0.0 {
        pad |= pad_flags::ACK_LEFT;
    }
    if mapper.get("ACK_RIGHT") > 0.0 {
        pad |= pad_flags::ACK_RIGHT;
    }
    if mapper.get("ACK_FORWARD_LEFT") > 0.0 {
        pad |= pad_flags::ACK_FORWARD_LEFT;
    }
    if mapper.get("ACK_FORWARD_RIGHT") > 0.0 {
        pad |= pad_flags::ACK_FORWARD_RIGHT;
    }
    if mapper.get("ACK_BACKWARD_LEFT") > 0.0 {
        pad |= pad_flags::ACK_BACKWARD_LEFT;
    }
    if mapper.get("ACK_BACKWARD_RIGHT") > 0.0 {
        pad |= pad_flags::ACK_BACKWARD_RIGHT;
    }

    if mapper.get("ACK_TWO") > 0.0 {
        pad |= pad_flags::ACK_TWO;
    }
    if mapper.get("ACK_TWO_LEFT") > 0.0 {
        pad |= pad_flags::ACK_TWO_LEFT;
    }
    if mapper.get("ACK_TWO_RIGHT") > 0.0 {
        pad |= pad_flags::ACK_TWO_RIGHT;
    }
    if mapper.get("ACK_TWO_FORWARD_LEFT") > 0.0 {
        pad |= pad_flags::ACK_TWO_FORWARD_LEFT;
    }
    if mapper.get("ACK_TWO_FORWARD_RIGHT") > 0.0 {
        pad |= pad_flags::ACK_TWO_FORWARD_RIGHT;
    }
    if mapper.get("ACK_TWO_BACKWARD_LEFT") > 0.0 {
        pad |= pad_flags::ACK_TWO_BACKWARD_LEFT;
    }
    if mapper.get("ACK_TWO_BACKWARD_RIGHT") > 0.0 {
        pad |= pad_flags::ACK_TWO_BACKWARD_RIGHT;
    }

    // Reconcile the raw mapper-derived attack pad bits with the gated
    // InputState produced by `apply_timing_windows`.  The mapper reads
    // keyboard/gamepad state directly; without this pass, a press during
    // a no-queue window would still set PADCMD_ATTACK_HIGH and the FSM
    // would fire the next attack regardless of timing windows.  Symmetric
    // case: a queued attack auto-firing in the do window has no raw
    // press, so we OR in PADCMD_ATTACK_HIGH so the FSM actually sees it.
    const ATTACK_PAD_BITS: u64 = pad_flags::PADCMD_ATTACK_HIGH
        | pad_flags::ACK
        | pad_flags::ACK_LEFT
        | pad_flags::ACK_RIGHT
        | pad_flags::ACK_FORWARD_LEFT
        | pad_flags::ACK_FORWARD_RIGHT
        | pad_flags::ACK_BACKWARD_LEFT
        | pad_flags::ACK_BACKWARD_RIGHT;
    const ATTACK_TWO_PAD_BITS: u64 = pad_flags::PADCMD_ATTACK_TWO_HIGH
        | pad_flags::ACK_TWO
        | pad_flags::ACK_TWO_LEFT
        | pad_flags::ACK_TWO_RIGHT
        | pad_flags::ACK_TWO_FORWARD_LEFT
        | pad_flags::ACK_TWO_FORWARD_RIGHT
        | pad_flags::ACK_TWO_BACKWARD_LEFT
        | pad_flags::ACK_TWO_BACKWARD_RIGHT;

    if !input.attack {
        pad &= !ATTACK_PAD_BITS;
    } else if (pad & ATTACK_PAD_BITS) == 0 {
        pad |= pad_flags::PADCMD_ATTACK_HIGH;
    }
    if !input.attack_two {
        pad &= !ATTACK_TWO_PAD_BITS;
    } else if (pad & ATTACK_TWO_PAD_BITS) == 0 {
        pad |= pad_flags::PADCMD_ATTACK_TWO_HIGH;
    }

    if pad
        & (pad_flags::PADCMD_ATTACK_HIGH
            | pad_flags::ACK_FORWARD_LEFT
            | pad_flags::ACK_FORWARD_RIGHT)
        != 0
        && mapper.get("PADCMD_CHR_FWD") > 0.0
        && is_grounded
    {
        ctrl |= ctrl_flags::RUNNING;
    }

    let mut me_flags: u32 = 0;
    if let Some(fs) = fighter_state {
        if fs.is_grappling() {
            me_flags |= entity_flags::GRAPPLING;
        }
        if fs.is_being_grappled() {
            me_flags |= entity_flags::GETTING_GRAPPLED;
        }
    }

    let mut has_weapon = None;
    if let Some(inv) = inventory
        && let Some(slot) = inv.current_weapon_slot()
    {
        has_weapon = Some(slot.ty.base.name.to_uppercase());
    }

    FsmPacket {
        pad_flags: pad,
        ctrl_flags: ctrl,
        class_hit: -1,
        me_flags,
        target_flags: 0,
        has_weapon,
    }
}

// ---------------------------------------------------------------------------
// ATDT timing windows
// ---------------------------------------------------------------------------

/// Apply ATDT combo-linking timing windows to produce a gated InputState and
/// the CRITICAL_FRAME ctrl flag.
///
/// Windows:
///   0.0 .. opp2_q_start   → no-queue: ignore attack input entirely
///   opp2_q_start .. opp2_do_start → queue: remember press, don't fire yet
///   opp2_do_start .. opp2_do_end  → branch/CRITICAL_FRAME: fire queued or new press
///   > opp2_do_end                 → window closed
pub fn apply_timing_windows(
    runtime: &mut FsmRuntime,
    input: &InputState,
    anim_state: &Oni2AnimState,
) -> (InputState, bool) {
    use crate::oni2_loader::parsers::atdt::AtdtStrike;

    if runtime.ctx.timed_out {
        let mut gated = input.clone();
        gated.attack = input.attack || runtime.ctx.queued_attack;
        gated.attack_two = input.attack_two || runtime.ctx.queued_attack_two;
        if gated.attack {
            runtime.ctx.queued_attack = false;
            runtime.ctx.queued_attack_critical = false;
        }
        if gated.attack_two {
            runtime.ctx.queued_attack_two = false;
            runtime.ctx.queued_attack_two_critical = false;
        }
        return (gated, false);
    }

    let strike: Option<&AtdtStrike> = anim_state
        .anim
        .attack_data
        .as_ref()
        .and_then(|a| a.strike.as_ref());

    let Some(strike) = strike else {
        runtime.ctx.queued_attack = false;
        runtime.ctx.queued_attack_critical = false;
        runtime.ctx.queued_attack_two = false;
        runtime.ctx.queued_attack_two_critical = false;
        return (input.clone(), false);
    };

    if anim_state.anim.num_frames <= 1 {
        return (input.clone(), false);
    }

    let phase = anim_state.current_time / (anim_state.anim.num_frames as f32 - 1.0).max(1.0);

    if phase < strike.opp2_q_start {
        runtime.ctx.queued_attack = false;
        runtime.ctx.queued_attack_critical = false;
        runtime.ctx.queued_attack_two = false;
        runtime.ctx.queued_attack_two_critical = false;
        let mut gated = input.clone();
        gated.attack = false;
        gated.attack_two = false;
        return (gated, false);
    }

    if phase < strike.opp2_do_start {
        if strike.queue_next_attack {
            if input.attack {
                runtime.ctx.queued_attack = true;
                if phase >= strike.opp2_crit_start {
                    runtime.ctx.queued_attack_critical = true;
                }
            }
            if input.attack_two {
                runtime.ctx.queued_attack_two = true;
                if phase >= strike.opp2_crit_start {
                    runtime.ctx.queued_attack_two_critical = true;
                }
            }
        }
        let mut gated = input.clone();
        gated.attack = false;
        gated.attack_two = false;
        return (gated, false);
    }

    if phase <= strike.opp2_do_end {
        let mut gated = input.clone();
        gated.attack = input.attack || runtime.ctx.queued_attack;
        gated.attack_two = input.attack_two || runtime.ctx.queued_attack_two;

        let mut is_critical = false;
        if gated.attack {
            is_critical |= (input.attack && phase >= strike.opp2_do_crit_start)
                || (runtime.ctx.queued_attack && runtime.ctx.queued_attack_critical);
            runtime.ctx.queued_attack = false;
            runtime.ctx.queued_attack_critical = false;
        }
        if gated.attack_two {
            is_critical |= (input.attack_two && phase >= strike.opp2_do_crit_start)
                || (runtime.ctx.queued_attack_two && runtime.ctx.queued_attack_two_critical);
            runtime.ctx.queued_attack_two = false;
            runtime.ctx.queued_attack_two_critical = false;
        }
        return (gated, is_critical);
    }

    let mut gated = input.clone();
    gated.attack = false;
    gated.attack_two = false;
    (gated, false)
}

// ---------------------------------------------------------------------------
// Update system
// ---------------------------------------------------------------------------

/// Tick every player's FsmRuntime and apply the resulting animation outputs.
pub fn fsm_update_system(
    time: Res<Time>,
    pad_mapper: Res<PadMapper>,
    mut anim_start_reader: MessageReader<crate::animator::AnimStartedMessage>,
    mut query: Query<
        (
            Entity,
            &mut FsmRuntime,
            &InputState,
            &mut Fighter,
            &mut Oni2AnimState,
            &Oni2AnimLibrary,
            &GlobalTransform,
            &mut Transform,
            Option<&crate::fight::components::FighterState>,
            Option<&crate::inventory::components::Inventory>,
            Option<&crate::animator::components::ActionPlayer>,
        ),
        With<Player>,
    >,
    triggers: Query<(&GlobalTransform, &FightVectorTrigger)>,
) {
    // Drain `AnimStartedMessage` events into a per-entity flag set for
    // this system's loop.  `true` iff the entity had a fresh anim this
    // tick (or since this system last ran, if FixedUpdate ran multiple
    // times between Update ticks).  The `previous.is_some()` guard
    // suppresses the very-first anim play on a freshly-spawned entity.
    let mut newly_started_anims: std::collections::HashMap<Entity, bool> =
        std::collections::HashMap::new();
    for msg in anim_start_reader.read() {
        if msg.previous.is_some() {
            newly_started_anims.insert(msg.entity, true);
        }
    }

    for (
        entity,
        mut runtime,
        input,
        mut fighter,
        mut anim_state,
        anim_lib,
        gtf,
        _transform,
        fighter_state_opt,
        inventory_opt,
        ap_opt,
    ) in &mut query
    {
        let dt = time.delta_secs();
        runtime.sm.advance_clock(dt);

        // Refresh the anim-state mirror BEFORE deriving anything that
        // reads it.  The order used to be: set `timed_out` → … → refresh
        // anim fields.  That meant the `timed_out` setter saw the
        // PREVIOUS tick's anim_current_time, so on the tick a non-looping
        // anim's current_time first crossed its last frame, `is_anim_done()`
        // still returned false and `timed_out` stayed false until the
        // next tick.  Downstream consumers (notably
        // `creature_movement_anim_system`'s `!fsm.ctx.timed_out` skip)
        // therefore lagged by one tick, holding off the next gait
        // dispatch (FIGHTSTANCE_FIGHT) until tick N+1 while
        // `apply_end_rotation_on_anim_end_system` ran in tick N — the
        // one-frame "old pose at new rotation" combo-boundary blink.
        runtime.ctx.anim_num_frames = anim_state.anim.num_frames;
        runtime.ctx.anim_current_time = anim_state.current_time;
        runtime.ctx.anim_looping = anim_state.looping;

        if !runtime.ctx.timed_out && runtime.ctx.is_anim_done() {
            runtime.ctx.timed_out = true;
        }

        // Drop any queued combo input the moment a new animation kicks
        // in (stale input from the previous attack mustn't persist).
        // Reads from the pre-collected `newly_started_anims` set rather
        // than a shared bool on the component — see the top of the
        // system for the event drain.  The `previous.is_some()` guard
        // suppresses the very first anim play (no prior anim yet).
        if newly_started_anims.get(&entity).copied().unwrap_or(false) {
            runtime.ctx.queued_attack = false;
            runtime.ctx.queued_attack_two = false;
        }

        let (gated_input, is_critical_frame) =
            apply_timing_windows(&mut runtime, input, &anim_state);

        let mut packet = build_fsm_packet(
            &gated_input,
            anim_state.is_grounded,
            &pad_mapper,
            fighter_state_opt,
            inventory_opt,
        );
        if is_critical_frame {
            packet.ctrl_flags |= ctrl_flags::CRITICAL_FRAME;
        }

        // Pre-resolve fight vector so DoTriggerAtk can succeed or fail inline.
        let fighter_pos = gtf.translation();
        let fight_vector_anim: Option<String> = find_fight_trigger_vector(fighter_pos, &triggers)
            .and_then(|fv| {
                if facing_within(fighter.facing, fv.direction, 30.0) {
                    Some(fv.attack_alias)
                } else {
                    None
                }
            });

        // Anim-state mirror was already refreshed above; just set the
        // per-tick packet/fight-vector fields here.
        runtime.ctx.packet = packet.clone();
        runtime.ctx.fight_vector_anim = fight_vector_anim;

        let runtime = runtime.into_inner();
        let output = runtime.sm.tick(&mut runtime.ctx);

        // While the actor is reacting (substate stays REACT for the
        // whole react+getup queue), swallow FSM-issued anim dispatches —
        // mirrors the legacy `CanStartAttack` / `CanStartBlock` gates
        // (both return false during `IsReacting()`). Without this, the
        // FSM's Timeout transition fires the moment the react reaches
        // its last frame and clobbers the queued getup with whatever
        // attack/block the next state wants.
        let is_reacting = ap_opt.is_some_and(|ap| ap.is_reacting());

        if is_reacting {
            // continue; — fall through so we still drop the per-tick
            // pulses below if any; but skip the anim-output dispatch.
        } else if let Some((anim_name, rotation_notches)) = &output.attack_anim {
            info!(
                "FSM: DoAttack → '{}', pad_flags: {:#x}, ctrl_flags: {:#x}",
                anim_name, packet.pad_flags, packet.ctrl_flags
            );
            do_attack(
                anim_lib,
                &mut anim_state,
                &mut fighter,
                anim_name,
                *rotation_notches,
            );
        } else if let Some(anim_name) = &output.block_anim {
            info!("FSM: DoBlock → '{}'", anim_name);
            do_block(anim_lib, &mut anim_state, anim_name);
        } else if let Some((anim_name, mirror)) = &output.evade_anim {
            do_evade(anim_lib, &mut anim_state, anim_name, *mirror);
        } else if let Some(anim_name) = &output.custom_anim {
            do_custom_anim(anim_lib, &mut anim_state, anim_name);
        }
    }
}

// ---------------------------------------------------------------------------
// AnimatorRuntime — parallel orchestration FSM loop
// ---------------------------------------------------------------------------

#[derive(Message, Debug, Clone)]
pub struct AnimatorBroadcastEvent {
    pub entity: Entity,
    pub payload: String,
}

#[derive(Component)]
pub struct AnimatorRuntime {
    pub sm: SmRuntime<crate::statemachine::drivers::animator::AnimatorDriver>,
    pub ctx: AnimatorCtx,
    pub previous_grounded: bool,
    /// Last tick's active pad commands.  Used to derive the edge-triggered
    /// `pressed_pad_cmds` set (commands that transitioned off→on this tick).
    pub previous_pad_cmds: std::collections::HashSet<String>,
}

impl AnimatorRuntime {
    pub fn new(data: Arc<SmData<crate::statemachine::drivers::animator::AnimatorDriver>>) -> Self {
        let initial = data.index_of_or_zero("IDLE");
        AnimatorRuntime {
            sm: SmRuntime::new(data, initial),
            ctx: AnimatorCtx::default(),
            previous_grounded: true,
            previous_pad_cmds: std::collections::HashSet::new(),
        }
    }
}

/// Tick the top-level orchestration machine (AnimatorDriver) for every player.
pub fn animator_update_system(
    time: Res<Time>,
    pad_mapper: Res<PadMapper>,
    mut damage_reader: MessageReader<DamageMessage>,
    mut query: Query<(
        Entity,
        &mut AnimatorRuntime,
        Option<&Oni2AnimState>,
        Option<&Health>,
        Option<&crate::animator::components::ActionPlayer>,
        Option<&crate::animator::ActiveAction>,
    )>,
    mut event_writer: MessageWriter<AnimatorBroadcastEvent>,
    mut end_action_reader: MessageReader<EndActionMessage>,
) {
    let dt = time.delta_secs();

    // Collect who was damaged this frame
    let mut damaged_entities = HashSet::new();
    for msg in damage_reader.read() {
        damaged_entities.insert(msg.target);
    }

    // Collect who explicitly ended a pipelined or scheduled action
    let mut finished_actions = HashSet::new();
    for msg in end_action_reader.read() {
        finished_actions.insert(msg.entity);
    }

    // Collect active PadCmds from the unified active PadMapper
    let active_pad_cmds: HashSet<String> = pad_mapper.active_commands().into_iter().collect();

    for (entity, mut runtime, anim_state_opt, health_opt, ap_opt, active_action_opt) in &mut query {
        runtime.sm.advance_clock(dt);

        let is_grounded = anim_state_opt.is_none_or(|s| s.is_grounded); // Fallback to grounded
        let ground_regained = is_grounded && !runtime.previous_grounded;
        let ground_lost = !is_grounded && runtime.previous_grounded;
        runtime.previous_grounded = is_grounded;

        // Edge-triggered: commands that were NOT active last tick but ARE
        // active this tick.  Drives the FSM's `PadPressed(...)` events so
        // a held button doesn't re-fire one-shot rules (jump / evade /
        // slide) each tick and stomp the in-progress schedule.
        let pressed_pad_cmds: std::collections::HashSet<String> = active_pad_cmds
            .difference(&runtime.previous_pad_cmds)
            .cloned()
            .collect();
        runtime.previous_pad_cmds = active_pad_cmds.clone();

        runtime.ctx.active_pad_cmds = active_pad_cmds.clone();
        runtime.ctx.pressed_pad_cmds = pressed_pad_cmds;
        runtime.ctx.jumps_available = ap_opt.is_some_and(|a| a.jumps_remaining > 0);
        // Window for the double-jump gate: only true once the takeoff has
        // promoted SubState1 to JUMP_MAIN.  Prevents the #JUMP rule from
        // re-firing StartJumpCompress while still in the COMPRESS phase.
        runtime.ctx.jump_in_air = ap_opt
            .is_some_and(|a| a.sub_state_1 == crate::animator::components::sub_state_1::JUMP_MAIN);
        runtime.ctx.ground_lost_this_tick = ground_lost;
        runtime.ctx.ground_regained_this_tick = ground_regained;

        runtime.ctx.damage_this_tick = damaged_entities.contains(&entity);
        runtime.ctx.health_zero = health_opt.map(|h| h.current <= 0.0).unwrap_or(false);

        // Evaluation of ModeDone (formerly Timeout / ModeAnimDone).
        // If the modern action pipeline is running, the mode is done when the pipeline
        // clears the ActiveAction. For legacy animations, we fall back to checking if
        // the non-looping animation reached its final frame.
        runtime.ctx.mode_done = if let Some(active) = active_action_opt {
            if active.0.is_some() {
                // If a pipelined action is actively running, it's not done.
                false
            } else {
                // Not actively running a new pipelined action.
                // Was it explicitly finished this tick? OR fall back to legacy animation check.
                finished_actions.contains(&entity)
                    || anim_state_opt.is_some_and(|s| {
                        !s.looping && s.current_time >= (s.anim.num_frames as f32 - 1.0).max(0.0)
                    })
            }
        } else {
            // No ActiveAction component. Fall back to legacy animation check.
            finished_actions.contains(&entity)
                || anim_state_opt.is_some_and(|s| {
                    !s.looping && s.current_time >= (s.anim.num_frames as f32 - 1.0).max(0.0)
                })
        };

        runtime.ctx.current_mode = runtime
            .sm
            .data
            .state_name(runtime.sm.current_state)
            .to_string();

        let runtime = runtime.into_inner();
        let prev_state_idx = runtime.sm.current_state;

        let output = runtime.sm.tick(&mut runtime.ctx);

        let new_state_idx = runtime.sm.current_state;
        if prev_state_idx != new_state_idx {
            bevy::log::info!(
                "Entity {:?} Animator GOTO: {} -> {}",
                entity,
                runtime.sm.data.state_name(prev_state_idx),
                runtime.sm.data.state_name(new_state_idx)
            );
        }

        for broadcast in output.broadcasts {
            event_writer.write(AnimatorBroadcastEvent {
                entity,
                payload: broadcast,
            });
        }
    }
}
