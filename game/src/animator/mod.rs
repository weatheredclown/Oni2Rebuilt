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
pub mod schedule;
pub mod systems;

pub use actions::{Action, ActionCtx, ActionRegistry, ActionUpdate, ActiveAction};
pub use anim_fx::{AnimFxEntry, AnimFxTracker};
pub use schedule::{AnimSchedule, AnimScheduleEntry};

use bevy::prelude::*;

use crate::menu::AppState;

pub use components::{
    ACT_NUM_ACTIONS, ActionPlayer, ActionResult, HeadIkMode, MainAction, WeaponState, action_flags,
    end_adverb, pending_flags, sub_state_0, sub_state_1,
};
pub use events::{
    ActionEndedMessage, ActionStartedMessage, ControlAnimMessage, DropMessage, EndActionMessage,
    HeadIkModeMessage, JumpImpulseMessage, PlayDieMessage, PlayReactMessage,
    SetPickupMatrixMessage, StartActionMessage, control_anim_bits,
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
                (
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
                    // Advance AnimSchedule cursors & play queued entries
                    // before the tick system promotes sub_state_1_last_frame.
                    schedule::anim_schedule_tick_system
                        .after(systems::action_start_system)
                        .before(systems::action_player_tick_system),
                    systems::action_player_tick_system
                        .after(systems::action_start_system)
                        .after(systems::action_end_system),
                    // Bridge: SubState1 transition into JUMP_MAIN / SOMERSAULT
                    // → JumpImpulseMessage (physics).  Must run AFTER the
                    // schedule tick (which stamps the new SubState1) and
                    // BEFORE action_player_tick_system promotes
                    // sub_state_1_last_frame — otherwise is_fresh() collapses.
                    systems::jump_impulse_emit_system
                        .after(schedule::anim_schedule_tick_system)
                        .before(systems::action_player_tick_system),
                    // `jump_land_on_ground_system` removed: JumpAction
                    // and FallAction now handle the ground-contact edge
                    // themselves through `ActionUpdate` transitions.
                    // Keeping the old system would double-advance the
                    // schedule on landing (MAIN → LAND → past-end)
                    // skipping the LAND entry entirely.
                    // Detect schedule-finished edge and fire EndActionMessage
                    // so do_end_action clears CLEARLIST_JUMP / etc.  Runs
                    // AFTER the schedule tick (so `finished` is up-to-date)
                    // and BEFORE action_end_system so the emitted event is
                    // processed the same tick.
                    systems::schedule_finished_end_action_system
                        .after(schedule::anim_schedule_tick_system)
                        .before(systems::action_end_system),
                    // Consume JumpImpulseMessage emitted by the animator once SubState1
                    // transitions into JUMP_MAIN — ordered strictly after the animator's
                    // emit so the message is readable in the same FixedUpdate tick.
                    systems::jump_impulse_apply_system
                        .after(systems::jump_impulse_emit_system),
                    // Restore GravityScale to 1.0 once the jumper lands.
                    // MUST run BEFORE `jump_impulse_apply_system`: on the
                    // tick a jump's MAIN starts, the character is still
                    // briefly grounded.  Without this ordering, reset can
                    // fire AFTER apply in the same FixedUpdate and clobber
                    // the jump's gravity factor — producing a floaty second
                    // jump when the first jump happened to sort correctly.
                    systems::jump_gravity_reset_system
                        .before(systems::jump_impulse_apply_system),
                    // Top-level FSM broadcast handler
                    systems::animator_broadcast_handler,
                    anim_fx::anim_fx_system,
                )
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
