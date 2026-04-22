/*
 * ai/fsm.rs — AiFsmRuntime, AiPadCommands, and the AI-side FSM tick.
 *
 * Every AI fighter uses the same `InputDriver` state machine the player
 * does — the only difference is the .fsm file (enemy.fsm instead of
 * player.fsm) and who fills in the PADCMD bits (AI decision systems
 * instead of hardware input).  This module owns the per-entity runtime
 * + the tick that packs synthesized pad commands into an `FsmPacket`
 * and drives the resulting attack animation through `anim_lib.play` —
 * the same `DoAttack` → `anim_lib.play` path the player's
 * `fsm_update_system` uses.
 *
 * Design notes:
 *   • `AiFsmRuntime` is a distinct component type from the player's
 *     `FsmRuntime` so the player's Player-filtered `fsm_update_system`
 *     can't accidentally pick up AI entities and vice versa.
 *     Structurally identical otherwise.
 *   • `AiPadCommands` is pulse-level: the decision system sets bits
 *     each tick they want to request, the update system packs them
 *     into a packet, and the pulse is cleared at the end of the tick.
 *     This matches the edge-triggered semantics of human button
 *     presses without needing a hardware-style "pressed/released"
 *     state machine per bit.
 */
use bevy::prelude::*;
use std::sync::Arc;

use crate::combat::components::Fighter;
use crate::oni2_loader::animation::{Oni2AnimLibrary, Oni2AnimState};
use crate::statemachine::core::{SmData, SmRuntime};
use crate::statemachine::drivers::input::{InputCtx, InputDriver};
use crate::statemachine::enemy_fsm::EnemyFsmCache;
use crate::statemachine::runtime::{do_attack, do_block, do_custom_anim, do_evade};
use crate::statemachine::types::{FsmPacket, ctrl_flags, pad_flags};

use super::components::AiFighter;

/// Synthesized per-AI pad commands.  Set each FixedUpdate by AI
/// decision systems; consumed once per tick by `ai_fsm_update_system`.
///
/// Pulse semantics: fields are cleared after every FSM tick, so a
/// decision system must re-assert a flag on every tick it wants the
/// FSM to see it.  This mirrors how the player's per-frame input
/// read is a fresh snapshot rather than persistent state.
#[derive(Component, Default)]
pub struct AiPadCommands {
    pub attack_high: bool,
    pub attack_two_high: bool,
    pub block: bool,
    pub chr_fwd: bool,
    pub chr_bak: bool,
    pub chr_lft: bool,
    pub chr_rgh: bool,
}

/// Per-AI FSM runtime.  Wraps the generic `SmRuntime<InputDriver>` +
/// `InputCtx` — the same shape the player uses — but a distinct
/// component type so the two update systems stay on their own lanes.
#[derive(Component)]
pub struct AiFsmRuntime {
    pub sm: SmRuntime<InputDriver>,
    pub ctx: InputCtx,
}

impl AiFsmRuntime {
    /// Build a runtime with the cursor parked at `IDLE_STATE` (the
    /// initial state in every shipped `.fsm` I inspected).  Falls
    /// back to state 0 if a future `.fsm` uses a different initial
    /// state name.
    pub fn new(data: Arc<SmData<InputDriver>>) -> Self {
        let initial = data.index_of_or_zero("IDLE_STATE");
        AiFsmRuntime {
            sm: SmRuntime::new(data, initial),
            ctx: InputCtx::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Attach on spawn
// ---------------------------------------------------------------------------

/// Lazy-load `enemy.fsm` and attach `AiFsmRuntime` + `AiPadCommands`
/// to every newly-spawned `AiFighter`.  Today every AI uses
/// `enemy.fsm`; when per-creature tuning (components.xml) carries an
/// FSM name, feed it in here instead of the hard-coded default.
pub fn ai_attach_fsm_system(
    mut commands: Commands,
    mut cache: ResMut<EnemyFsmCache>,
    query: Query<Entity, (Added<AiFighter>, Without<AiFsmRuntime>)>,
) {
    for entity in &query {
        let Some(data) = cache.get_or_load("enemy") else {
            warn!(
                "ai_attach_fsm_system: enemy.fsm unavailable — AI entity {:?} gets no FSM",
                entity
            );
            continue;
        };
        commands
            .entity(entity)
            .insert((AiFsmRuntime::new(data), AiPadCommands::default()));
    }
}

// ---------------------------------------------------------------------------
// Tick
// ---------------------------------------------------------------------------

/// Advance each AI entity's FSM, consuming `AiPadCommands` pulses and
/// driving any requested attack/block/evade animation through the
/// same `anim_lib.play` pathway the player uses.  Pulse bits are
/// cleared at the end of the tick so decision systems see a fresh
/// `AiPadCommands::default()` next frame.
pub fn ai_fsm_update_system(
    time: Res<Time>,
    mut query: Query<(
        Entity,
        &mut AiFsmRuntime,
        &mut AiPadCommands,
        &Oni2AnimLibrary,
        &mut Oni2AnimState,
        &mut Fighter,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut runtime, mut cmds, anim_lib, mut anim_state, mut fighter) in &mut query {
        runtime.sm.advance_clock(dt);

        // Keep the context's anim-state mirror current so `Timeout` /
        // `CustomAnimDone` / `is_anim_done()` rules fire at the right
        // frame — this is the same bookkeeping `fsm_update_system`
        // does for the player, just per-entity here.
        runtime.ctx.anim_num_frames = anim_state.anim.num_frames;
        runtime.ctx.anim_current_time = anim_state.current_time;
        runtime.ctx.anim_looping = anim_state.looping;
        if !runtime.ctx.timed_out && runtime.ctx.is_anim_done() {
            runtime.ctx.timed_out = true;
        }

        // Pack pulses into an FsmPacket.  No global PadMapper — each
        // AI carries its own synthesized pad on the entity.
        let mut pad: u64 = 0;
        let mut ctrl: u32 = 0;
        if cmds.attack_high {
            pad |= pad_flags::PADCMD_ATTACK_HIGH;
        }
        if cmds.attack_two_high {
            pad |= pad_flags::PADCMD_ATTACK_TWO_HIGH;
        }
        if cmds.block {
            pad |= pad_flags::PADCMD_BLOCK;
            ctrl |= ctrl_flags::CAN_BLOCK;
        }
        if cmds.chr_fwd {
            pad |= pad_flags::PADCMD_CHR_FWD;
        }
        if cmds.chr_bak {
            pad |= pad_flags::PADCMD_CHR_BAK;
        }
        if cmds.chr_lft {
            pad |= pad_flags::PADCMD_CHR_LFT;
        }
        if cmds.chr_rgh {
            pad |= pad_flags::PADCMD_CHR_RGH;
        }

        runtime.ctx.packet = FsmPacket {
            pad_flags: pad,
            ctrl_flags: ctrl,
            class_hit: -1,
            me_flags: 0,
            target_flags: 0,
            has_weapon: None,
        };

        let runtime_mut = runtime.into_inner();
        let output = runtime_mut.sm.tick(&mut runtime_mut.ctx);

        // Drain the anim-play outputs through the same `DoAttack`/
        // `DoBlock`/`DoEvade`/`PlayCustomAnim` helpers the player uses —
        // the underlying combat pipeline picks up the newly-played
        // animation via its ATDT on the next tick regardless of who
        // kicked it off.
        if let Some((anim_name, rotation_notches)) = &output.attack_anim {
            info!("AI FSM: DoAttack → '{}' (entity {:?})", anim_name, entity);
            do_attack(anim_lib, &mut anim_state, &mut fighter, anim_name, *rotation_notches);
        }
        if let Some(anim_name) = &output.block_anim {
            do_block(anim_lib, &mut anim_state, anim_name);
        }
        if let Some((anim_name, mirror)) = &output.evade_anim {
            do_evade(anim_lib, &mut anim_state, anim_name, *mirror);
        }
        if let Some(anim_name) = &output.custom_anim {
            do_custom_anim(anim_lib, &mut anim_state, anim_name);
        }

        // Clear pulses — decision systems must re-set each tick.
        *cmds = AiPadCommands::default();
    }
}
