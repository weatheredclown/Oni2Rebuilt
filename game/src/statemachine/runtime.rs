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
use crate::combat::components::Fighter;
use crate::control_map::PadMapper;
use crate::fight_vector::{FightVectorTrigger, facing_within, find_fight_trigger_vector};
use crate::oni2_loader::{Oni2AnimLibrary, Oni2AnimState};
use crate::player::components::{InputState, Player};

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
    fighter: &Fighter,
    mapper: &PadMapper,
    fighter_state: Option<&crate::fight::components::FighterState>,
    inventory: Option<&crate::inventory::components::Inventory>,
) -> FsmPacket {
    let mut pad: u64 = 0;
    let mut ctrl: u32 = 0;

    // ── Body / controller state flags ─────────────────────────────────────

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

    if pad
        & (pad_flags::PADCMD_ATTACK_HIGH
            | pad_flags::ACK_FORWARD_LEFT
            | pad_flags::ACK_FORWARD_RIGHT)
        != 0
        && mapper.get("PADCMD_CHR_FWD") > 0.0
        && fighter.is_grounded
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
    if let Some(inv) = inventory {
        if let Some(slot) = inv.current_weapon_slot() {
            has_weapon = Some(slot.ty.base.name.to_uppercase());
        }
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
        }
        if gated.attack_two {
            runtime.ctx.queued_attack_two = false;
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
        runtime.ctx.queued_attack_two = false;
        return (input.clone(), false);
    };

    if anim_state.anim.num_frames <= 1 {
        return (input.clone(), false);
    }

    let phase = anim_state.current_time / (anim_state.anim.num_frames as f32 - 1.0).max(1.0);

    if phase < strike.opp2_q_start {
        runtime.ctx.queued_attack = false;
        runtime.ctx.queued_attack_two = false;
        let mut gated = input.clone();
        gated.attack = false;
        gated.attack_two = false;
        return (gated, false);
    }

    if phase < strike.opp2_do_start {
        if strike.queue_next_attack {
            if input.attack {
                runtime.ctx.queued_attack = true;
            }
            if input.attack_two {
                runtime.ctx.queued_attack_two = true;
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
        if gated.attack {
            runtime.ctx.queued_attack = false;
        }
        if gated.attack_two {
            runtime.ctx.queued_attack_two = false;
        }
        return (gated, true);
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
    mut query: Query<
        (
            &mut FsmRuntime,
            &InputState,
            &Fighter,
            &mut Oni2AnimState,
            &Oni2AnimLibrary,
            &GlobalTransform,
            &mut Transform,
            Option<&crate::fight::components::FighterState>,
            Option<&crate::inventory::components::Inventory>,
        ),
        With<Player>,
    >,
    triggers: Query<(&GlobalTransform, &FightVectorTrigger)>,
) {
    for (
        mut runtime,
        input,
        fighter,
        mut anim_state,
        anim_lib,
        gtf,
        mut transform,
        fighter_state_opt,
        inventory_opt,
    ) in &mut query
    {
        let dt = time.delta_secs();
        runtime.sm.advance_clock(dt);

        if !runtime.ctx.timed_out && runtime.ctx.is_anim_done() {
            runtime.ctx.timed_out = true;
        }

        // Drop any queued combo input the moment a new animation kicks in
        // (stale input from the previous attack mustn't persist).
        if anim_state.current_anim_id != anim_state.previous_anim_id
            && anim_state.previous_anim_id.is_some()
        {
            runtime.ctx.queued_attack = false;
            runtime.ctx.queued_attack_two = false;
        }

        let (gated_input, is_critical_frame) =
            apply_timing_windows(&mut runtime, input, &anim_state);

        let mut packet = build_fsm_packet(&gated_input, fighter, &pad_mapper, fighter_state_opt, inventory_opt);
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

        // Refresh the per-tick context fields, then tick.
        runtime.ctx.packet = packet.clone();
        runtime.ctx.anim_num_frames = anim_state.anim.num_frames;
        runtime.ctx.anim_current_time = anim_state.current_time;
        runtime.ctx.anim_looping = anim_state.looping;
        runtime.ctx.fight_vector_anim = fight_vector_anim;

        let runtime = runtime.into_inner();
        let output = runtime.sm.tick(&mut runtime.ctx);

        if let Some((anim_name, rotation_notches)) = &output.attack_anim {
            info!(
                "FSM: DoAttack → '{}', pad_flags: {:#x}, ctrl_flags: {:#x}",
                anim_name, packet.pad_flags, packet.ctrl_flags
            );
            if !anim_lib.play(anim_name, &mut anim_state) {
                warn!(
                    "FSM: attack anim NOT in library: '{}' (lib has {} anims)",
                    anim_name,
                    anim_lib.anims.len()
                );
            }
            if *rotation_notches != 0 {
                use super::types::NOTCH_RADIANS;
                let rads = *rotation_notches as f32 * NOTCH_RADIANS;
                transform.rotation = Quat::from_rotation_y(rads) * transform.rotation;
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
