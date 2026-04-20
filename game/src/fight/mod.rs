/*
 * fight/mod.rs — FightPlugin: high-fidelity crFighter system.
 *
 * Registers all fight components, events, and FixedUpdate systems.
 * Systems are ordered relative to the CombatPlugin pipeline:
 *   react_data_apply  → before hit_reaction (combat)
 *   fighter_state_update → after all combat systems
 *   All other fight systems run in sequence after that.
 */
pub mod components;
pub mod events;
pub mod systems;

use bevy::prelude::*;

use crate::combat::systems::{hit_reaction_system, injure_system};
use crate::menu::AppState;

pub use components::{
    AnimControlBlock, BlockDef, BlockLibrary, BlockStatus, FighterState, FighterType, GrabAction,
    GrappleState, fighter_flags, grapple_flags,
};
pub use events::{
    ApplyRotationNotchesEvent, BlockFailedEvent, BlockSuccessEvent, GrappleEndEvent,
    GrappleEndReason, GrappleStartEvent, SuperMeterAddEvent,
};

pub struct FightPlugin;

impl Plugin for FightPlugin {
    fn build(&self, app: &mut App) {
        app
            // --- Messages ---
            .add_message::<GrappleStartEvent>()
            .add_message::<GrappleEndEvent>()
            .add_message::<BlockSuccessEvent>()
            .add_message::<BlockFailedEvent>()
            .add_message::<ApplyRotationNotchesEvent>()
            .add_message::<SuperMeterAddEvent>()
            // --- FixedUpdate systems ---
            .add_systems(
                FixedUpdate,
                (
                    // react_data_apply runs BEFORE hit_reaction_system so FighterState
                    // fields are populated before the animation starts.
                    systems::react_data_apply_system.before(hit_reaction_system),
                    // Block success/failed responses run after hit_detection but before injure.
                    systems::block_success_system.after(injure_system),
                    systems::block_failed_system.after(injure_system),
                    // Grapple management runs each tick.
                    systems::grapple_tick_system,
                    systems::grapple_end_system,
                    // Rotation notches are applied after all state mutations are done.
                    systems::rotation_notches_system,
                    // React-data-driven post-reaction housekeeping.
                    systems::react_end_rotation_system,
                    systems::knockdown_getup_system,
                    // Per-frame timer bookkeeping (decay, flag transitions).
                    systems::fighter_state_update_system,
                    // Super meter.
                    systems::super_meter_system,
                    // Successive-attacks / combo escalation.
                    systems::successive_attacks_system,
                    // Attack spin during active frames.
                    systems::attack_spin_system,
                    // Hit ETA prediction (AboutToBeHit).
                    systems::hit_eta_system,
                    // Fight stance timer reset.
                    systems::fight_stance_timer_system,
                )
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
