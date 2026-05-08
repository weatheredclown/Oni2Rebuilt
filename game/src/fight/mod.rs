/*
 * fight/mod.rs — FightPlugin: high-fidelity crFighter system.
 *
 * Registers all fight components, events, and FixedUpdate systems.
 * Systems are ordered relative to the CombatPlugin pipeline:
 *   react_data_apply  → before hit_reaction (combat)
 *   fighter_state_update → after all combat systems
 *   All other fight systems run in sequence after that.
 */
pub mod ai_attack;
pub mod components;
pub mod events;
pub mod fx_table;
pub mod systems;

use bevy::prelude::*;

use crate::combat::systems::{hit_reaction_system, injure_system};
use crate::menu::AppState;

pub use ai_attack::AiAttackTableCache;
pub use components::FighterType;
pub use events::{
    ApplyRotationNotchesEvent, BlockFailedEvent, BlockSuccessEvent, GrappleEndEvent,
    GrappleStartEvent, SuperMeterAddEvent,
};
pub use fx_table::AttackFxRegistry;

pub struct FightPlugin;

impl Plugin for FightPlugin {
    fn build(&self, app: &mut App) {
        // Seed the FX registry with a minimal default table so hits produce
        // something visible/audible until per-fighter-type FX are loaded.
        let mut fx_registry = AttackFxRegistry::default();
        fx_table::register_default(&mut fx_registry);
        app.insert_resource(fx_registry);

        // AI attack table cache — holds one derived roster per fighter-type
        // name, populated lazily by build_ai_attack_tables_system.
        app.init_resource::<AiAttackTableCache>();

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
                    // (knockdown_getup_system removed — getup is now
                    // queued as the second entry of the React action's
                    // AnimSchedule by action_start_system; the schedule
                    // tick auto-advances knockdown→getup atomically with
                    // the FSM's REACT state, mirroring legacy
                    // ActionStartReact's two-batch react+getup queue.)
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                FixedUpdate,
                (
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
                    // Track heading during attack for locked fighting targets
                    systems::update_fighter_strike_facing_system,
                    // Interpolate Fighter.facing toward
                    // FighterState.turn_final_target_dir each frame
                    // (crFighter::TurnLerper; fighter.cpp:1377-1387).
                    systems::fighter_turn_lerp_system,
                    // Fight stance entry/exit orchestration.  `sync` must
                    // run first so FighterState.FIGHT_MODE reflects the
                    // animator's FIGHTSTANCE flag before entry/exit read it.
                    systems::fight_stance_sync_system,
                    systems::fight_stance_entry_system.after(systems::fight_stance_sync_system),
                    systems::fight_stance_exit_system.after(systems::fight_stance_sync_system),
                    // Build AiAttackTable for any FighterType that doesn't
                    // have one yet.  One-shot per entity: the Without<...>
                    // filter means this is a no-op once every fighter has
                    // its table attached.
                    ai_attack::build_ai_attack_tables_system,
                )
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
