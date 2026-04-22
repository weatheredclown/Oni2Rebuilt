/*
 * animator/mod.rs — AnimatorPlugin.
 *
 * Wires up the ActionPlayer subsystem (Rust equivalent of crAnimActionPlayer)
 * and its supporting messages.  Systems run every FixedUpdate while in AppState::InGame.
 *
 * System order:
 *   1. control_anim_system         — apply stop/pause/restart/rate before tick
 *   2. play_react_system /
 *      play_die_system             — expand convenience messages into StartAction
 *   3. action_start_system         — dispatch StartActionMessage
 *   4. action_end_system           — dispatch EndActionMessage
 *   5. head_ik_mode_system         — apply mode changes
 *   6. action_player_tick_system   — per-frame state bookkeeping (runs last so
 *                                    sub_state_1_is_fresh() sees this frame's
 *                                    starts)
 */
pub mod actions;
pub mod anim_fx;
pub mod components;
pub mod events;
pub mod gravity;
pub mod schedule;
pub mod systems;

pub use actions::ActiveAction;
pub use anim_fx::AnimFxEntry;
pub use gravity::GravityModifiers;
pub use schedule::AnimSchedule;

use bevy::prelude::*;

// ---------------------------------------------------------------------------
// AnimatorBundle — components every animator-driven entity needs at spawn
// ---------------------------------------------------------------------------

/// Bundle of components that must coexist for the animator pipeline to
/// function correctly on an entity.  Spawn sites insert this in one go
/// instead of enumerating every field — and when we add a new required
/// component (like `GravityModifiers` did), the only file that needs
/// editing is this module.
///
/// Why all of these at once and not lazy-attach via `ensure_*` systems:
/// `commands.insert` flushes at end-of-stage, so the FIRST
/// `StartActionMessage` / dispatch / sync tick after spawn would see
/// missing components and silently skip the entity.  Present-at-spawn
/// is the only race-free path.
///
/// Add a new field here when a new animator subsystem needs a per-entity
/// component at spawn time.
#[derive(Bundle, Default)]
pub struct AnimatorBundle {
    /// Top-level action-player state (flags, substates, jump index).
    pub action_player: components::ActionPlayer,
    /// Currently-running polymorphic `Action` (None when locomotion).
    pub active_action: ActiveAction,
    /// Multi-stage animation schedule (crAnimList equivalent).
    pub anim_schedule: AnimSchedule,
    /// Stack-of-overrides for gravity; JumpAction / cutscenes push
    /// layers here and `gravity_sync_system` reflects the resolution
    /// into Avian's `GravityScale`.
    pub gravity_modifiers: GravityModifiers,
}

use crate::menu::AppState;

pub use components::{MainAction, sub_state_0};
pub use events::{
    ActionEndedMessage, ActionStartedMessage, AnimStartedMessage, ControlAnimMessage, DropMessage,
    EndActionMessage, HeadIkModeMessage, JumpImpulseMessage, PlayDieMessage, PlayReactMessage,
    SetPickupMatrixMessage, StartActionMessage,
};

pub struct AnimatorPlugin;

impl Plugin for AnimatorPlugin {
    fn build(&self, app: &mut App) {
        // Action registry for the new polymorphic pipeline.  Populated
        // with Jump / Fall / Land at build time; other MainAction variants
        // continue through the legacy try_start_action match.
        let mut registry = actions::ActionRegistry::default();
        actions::register_default_actions(&mut registry);
        app.insert_resource(registry);

        app
            // --- Messages ---
            .add_message::<AnimStartedMessage>()
            .add_message::<StartActionMessage>()
            .add_message::<EndActionMessage>()
            .add_message::<ActionStartedMessage>()
            .add_message::<ActionEndedMessage>()
            .add_message::<ControlAnimMessage>()
            .add_message::<HeadIkModeMessage>()
            .add_message::<PlayReactMessage>()
            .add_message::<PlayDieMessage>()
            .add_message::<JumpImpulseMessage>()
            .add_message::<SetPickupMatrixMessage>()
            .add_message::<DropMessage>()
            // --- Systems ---
            .add_systems(
                FixedUpdate,
                // Two nested tuples to stay under Bevy's 20-system
                // add_systems arity limit.  Ordering is declared via
                // `.after` / `.before` on specific pairs — the grouping
                // itself doesn't imply any sequencing between the two
                // sub-tuples.
                (
                    (
                        // Drain `anim_just_started` intention bits into
                        // `AnimStartedMessage` events first so downstream
                        // systems reading the event see this tick's
                        // fresh starts without relying on component
                        // ordering.
                        systems::anim_start_emit_system,
                        systems::control_anim_system,
                        systems::play_react_system,
                        systems::play_die_system,
                        // New polymorphic-action pipeline.  Runs BEFORE
                        // action_start_system so registered actions are
                        // intercepted and handled via the Action trait;
                        // action_start_system skips registered ones by
                        // checking the registry.
                        actions::action_start_dispatch_system
                            .after(systems::play_react_system)
                            .after(systems::play_die_system),
                        actions::action_dispatch_system
                            .after(actions::action_start_dispatch_system),
                        actions::ensure_active_action_component,
                        systems::action_start_system
                            .after(actions::action_start_dispatch_system)
                            .after(systems::play_react_system)
                            .after(systems::play_die_system),
                        systems::action_end_system,
                        systems::head_ik_mode_system,
                        systems::head_ik_bridge_system.after(systems::head_ik_mode_system),
                        // Advance AnimSchedule cursors & play queued
                        // entries before the tick system promotes
                        // sub_state_1_last_frame.
                        schedule::anim_schedule_tick_system
                            .after(systems::action_start_system)
                            .before(systems::action_player_tick_system),
                        systems::action_player_tick_system
                            .after(systems::action_start_system)
                            .after(systems::action_end_system),
                    ),
                    (
                        // Bridge: SubState1 transition into JUMP_MAIN /
                        // SOMERSAULT → JumpImpulseMessage (physics).
                        // Must run AFTER the schedule tick (which stamps
                        // the new SubState1) and BEFORE
                        // action_player_tick_system promotes
                        // sub_state_1_last_frame — otherwise is_fresh()
                        // collapses.
                        systems::jump_impulse_emit_system
                            .after(schedule::anim_schedule_tick_system)
                            .before(systems::action_player_tick_system),
                        // Detect schedule-finished edge and fire
                        // EndActionMessage so do_end_action clears
                        // CLEARLIST_JUMP / etc.  Runs AFTER the schedule
                        // tick (so `finished` is up-to-date) and BEFORE
                        // action_end_system so the emitted event is
                        // processed the same tick.
                        systems::schedule_finished_end_action_system
                            .after(schedule::anim_schedule_tick_system)
                            .before(systems::action_end_system),
                        // Consume JumpImpulseMessage emitted by the
                        // animator once SubState1 transitions into
                        // JUMP_MAIN — ordered strictly after the
                        // animator's emit so the message is readable in
                        // the same FixedUpdate tick.  Gravity scaling
                        // is no longer driven from inside this system —
                        // JumpAction pushes a `"jump"` layer onto
                        // `GravityModifiers` at on_enter, and
                        // `gravity_sync_system` reflects the resolved
                        // stack into Avian's `GravityScale` each tick.
                        // This system only applies the velocity impulse.
                        systems::jump_impulse_apply_system.after(systems::jump_impulse_emit_system),
                        // Idempotently attach `GravityModifiers` to any
                        // entity with `GravityScale` that doesn't have
                        // one yet.  Runs first so downstream sync has
                        // something to resolve.
                        gravity::ensure_gravity_modifiers_component,
                        // Reflect the resolved `GravityModifiers.current()`
                        // into Avian's `GravityScale` component when it
                        // differs.  Replaces the old
                        // `jump_gravity_reset_system` — no more ordering
                        // fight with the impulse apply, no more
                        // velocity-gate hack.  JumpAction's on_exit
                        // removes the "jump" layer, which naturally
                        // restores base `1.0`.
                        gravity::gravity_sync_system
                            .after(gravity::ensure_gravity_modifiers_component),
                        // Replenish jump count on ground contact.  Split
                        // out from the old reset system which mixed
                        // gravity and jump-count concerns.
                        systems::replenish_jumps_on_ground_system,
                        // Top-level FSM broadcast handler
                        systems::animator_broadcast_handler,
                        anim_fx::anim_fx_system,
                    ),
                )
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
