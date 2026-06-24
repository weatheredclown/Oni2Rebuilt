/*
 * animator/actions/mod.rs — polymorphic Action lifecycle.
 *
 * The top-level animator FSM (`AnimatorDriver`) broadcasts INTENT
 * ("StartJumpCompress", "StartLand", "StartFall", …).  Those broadcasts
 * translate to `StartActionMessage` events, which the action pipeline
 * turns into a concrete `Action` impl stored on the entity.  That impl
 * owns its own internal state machine and the per-tick work:
 *
 *   • `can_enter(ctx)` — pure predicate checked before a new action
 *     overrides the current one.
 *   • `on_enter(ctx, subaction)` — one-shot entry work: set flags,
 *     build the AnimSchedule, fire kickoff broadcasts.
 *   • `update(ctx, dt)` — per-tick work, returns `ActionUpdate` telling
 *     the dispatcher to Continue, to treat this action as Finished, or
 *     to Hand off to another action (e.g. Jump → Fall at apex).
 *   • `can_exit(ctx)` — gate for external override attempts.
 *   • `on_exit(ctx)` — teardown: clear flags, cancel the schedule.
 *
 * The legacy `try_start_action` match still handles actions that haven't
 * been migrated yet — the dispatcher peels off Jump/Fall/Land/etc as
 * they port over.  `ActionRegistry` holds factories keyed on MainAction.
 */
use bevy::prelude::*;
use std::collections::HashMap;

use crate::animator::components::{ActionPlayer, MainAction};
use crate::animator::schedule::AnimSchedule;
use crate::oni2_loader::animation::{Oni2AnimLibrary, Oni2AnimState};
use crate::oni2_loader::parsers::jump::JumpController;
use crate::inventory::components::Inventory;
use crate::weapons::components::{HoldType, Weapon};

pub mod draw_weapon;
pub mod fall;
pub mod jump;

// ---------------------------------------------------------------------------
// ActionCtx — bundled references the Action trait methods need
// ---------------------------------------------------------------------------

/// Per-call context handed to `Action` methods.  Built by the dispatcher
/// each tick from the entity's query row so Actions don't need to perform
/// ECS queries themselves.
pub struct ActionCtx<'a> {
    pub entity: Entity,
    pub ap: &'a mut ActionPlayer,
    pub schedule: &'a mut AnimSchedule,
    pub anim_state: &'a mut Oni2AnimState,
    pub lib: &'a Oni2AnimLibrary,
    pub jump_controller: Option<&'a JumpController>,
    /// Stack-of-overrides for gravity.  Actions that want to modify the
    /// character's gravity push a keyed layer on `on_enter` and remove
    /// it on `on_exit` — see `super::gravity::GravityModifiers`.  None
    /// when the entity has no gravity (a static prop, say).
    pub gravity: Option<&'a mut super::gravity::GravityModifiers>,

    // --- Physics / world contact (one-tick edges from the animator tick) ---
    pub is_grounded: bool,
    pub ground_just_regained: bool,
    pub ground_just_lost: bool,
    pub wall_grab_available: bool,
    pub zipline_available: bool,
    pub high_velocity_landing: bool,

    // --- Accumulator for Broadcast events the Action wants to emit ---
    /// Strings pushed here by an action are turned into
    /// `AnimatorBroadcastEvent`s by the dispatcher so the existing
    /// `animator_broadcast_handler` can route them.
    pub broadcasts: &'a mut Vec<String>,
    pub weapon_hold_type: crate::weapons::components::HoldType,
}

// ---------------------------------------------------------------------------
// ActionUpdate — return value from update()
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ActionUpdate {
    /// Action is still running — nothing to do.
    Continue,
    /// Action's lifecycle completed naturally.  Dispatcher clears the
    /// `ActiveAction` component and fires `EndActionMessage`.
    Finished,
    /// Action wants to hand off to another action in the same tick, as a
    /// natural internal transition (e.g. Jump → Fall when MAIN ends while
    /// airborne).  Dispatcher calls `on_exit` on current, instantiates
    /// the next action via `ActionRegistry`, calls `on_enter` on it.
    HandoffTo { action: MainAction, subaction: i32 },
}

// ---------------------------------------------------------------------------
// Action trait
// ---------------------------------------------------------------------------

/// One concrete animator action (Jump / Fall / Land / Slide / …).  Each
/// impl owns its internal state machine; methods are called by the
/// dispatcher with a `ActionCtx` bundling the shared refs.
pub trait Action: Send + Sync {
    /// Identity — used to look up the factory in `ActionRegistry`.  One
    /// `Action` impl per `MainAction` variant.
    fn main_action(&self) -> MainAction;

    /// Pre-flight check for a fresh start request.  Default implementation
    /// always allows entry; override to consult reject/override flag lists
    /// or incompatible-state conditions.  Pure — no side effects.
    fn can_enter(&self, _ctx: &ActionCtx<'_>, _subaction: i32) -> bool {
        true
    }

    /// One-shot entry work — set flags, build the AnimSchedule, push
    /// kickoff broadcasts.  Always paired with a later `on_exit`.  Called
    /// only after `can_enter` returned true.
    fn on_enter(&mut self, ctx: &mut ActionCtx<'_>, subaction: i32);

    /// Per-tick work.  Return `Continue` to keep running, `Finished` to
    /// end the action, or `HandoffTo(...)` to transition directly into a
    /// sibling action without round-tripping through the FSM.
    fn update(&mut self, ctx: &mut ActionCtx<'_>, dt: f32) -> ActionUpdate;

    /// Can this action be interrupted by an external override right now?
    /// Default true; override for actions that shouldn't be cancellable
    /// mid-swing (active attack, react, grapple break).
    fn can_exit(&self, _ctx: &ActionCtx<'_>) -> bool {
        true
    }

    /// Teardown — clear flags, cancel the schedule if still running.
    /// Called when the action reaches `Finished` OR is overridden via a
    /// fresh StartActionMessage whose `can_exit` check passed.
    fn on_exit(&mut self, ctx: &mut ActionCtx<'_>);
}

// ---------------------------------------------------------------------------
// ActiveAction — per-entity component holding the currently-running action
// ---------------------------------------------------------------------------

/// Holds the one Action that's currently driving this entity's top-level
/// animator mode.  `None` means no action is active (the entity is in
/// idle/nav locomotion).  The dispatcher swaps the inner Box on entry /
/// handoff / exit.
#[derive(Component, Default)]
pub struct ActiveAction(pub Option<Box<dyn Action>>);

// ---------------------------------------------------------------------------
// ActionRegistry — factory resource
// ---------------------------------------------------------------------------

/// Maps `MainAction` to a boxed factory that instantiates a fresh Action.
/// Registered at plugin build time by each action module.
#[derive(Resource, Default)]
pub struct ActionRegistry {
    factories: HashMap<MainAction, Box<dyn Fn() -> Box<dyn Action> + Send + Sync>>,
}

impl ActionRegistry {
    pub fn register<F>(&mut self, action: MainAction, factory: F)
    where
        F: Fn() -> Box<dyn Action> + Send + Sync + 'static,
    {
        self.factories.insert(action, Box::new(factory));
    }

    pub fn make(&self, action: MainAction) -> Option<Box<dyn Action>> {
        self.factories.get(&action).map(|f| f())
    }

    pub fn contains(&self, action: MainAction) -> bool {
        self.factories.contains_key(&action)
    }
}

/// Populate the registry with every Action we've ported.  Called from
/// `AnimatorPlugin::build`.
pub fn register_default_actions(registry: &mut ActionRegistry) {
    registry.register(MainAction::Jump, || Box::new(jump::JumpAction::default()));
    registry.register(MainAction::Fall, || Box::new(fall::FallAction::default()));
    registry.register(MainAction::DrawWeapon, || Box::new(draw_weapon::DrawWeaponAction::default()));
    // No separate Land action — LAND is the last phase of JumpAction and
    // FallAction (matches the C++ where LAND was the last entry of the
    // Jump animlist).
}

// ---------------------------------------------------------------------------
// Dispatch systems
// ---------------------------------------------------------------------------

use crate::animator::events::{EndActionMessage, StartActionMessage};
use crate::statemachine::runtime::{AnimatorBroadcastEvent, AnimatorRuntime};

/// Consumes `StartActionMessage` for registered actions, calls `on_enter`,
/// and installs the resulting Box<dyn Action> into `ActiveAction`.
/// Messages for UNregistered actions are left alone so the legacy
/// `action_start_system` can handle them — this is the migration-period
/// half-system.
pub fn action_start_dispatch_system(
    mut reader: MessageReader<StartActionMessage>,
    registry: Res<ActionRegistry>,
    mut query: Query<(
        &mut ActionPlayer,
        &mut ActiveAction,
        &mut AnimSchedule,
        &mut Oni2AnimState,
        &Oni2AnimLibrary,
        Option<&JumpController>,
        Option<&mut super::gravity::GravityModifiers>,
        Option<&Inventory>,
    )>,
    weapons: Query<&Weapon>,
    mut broadcast_writer: MessageWriter<AnimatorBroadcastEvent>,
    mut end_writer: MessageWriter<EndActionMessage>,
) {
    for msg in reader.read() {
        if !registry.contains(msg.action) {
            // Leave unported actions to action_start_system.
            continue;
        }
        let Ok((mut ap, mut active, mut schedule, mut anim_state, lib, jc, gravity_opt, inv_opt)) =
            query.get_mut(msg.entity)
        else {
            continue;
        };

        // Build a fresh Action instance via the registry factory.
        let Some(mut new_action) = registry.make(msg.action) else {
            continue;
        };

        // Collect-then-flush the broadcasts accumulator so this system
        // doesn't hold two borrows of the writer across the ctx build.
        let mut broadcasts: Vec<String> = Vec::new();

        // Capture `is_grounded` BEFORE the mutable borrow of anim_state
        // moves into ctx — the borrow checker rejects a simultaneous
        // shared + exclusive borrow on the same component.
        let is_grounded = anim_state.is_grounded;
        let gravity_mut = gravity_opt.map(|g| g.into_inner());
        let weapon_hold_type = inv_opt
            .and_then(|inv| inv.current_weapon_entity())
            .and_then(|weap_ent| weapons.get(weap_ent).ok())
            .map(|weap| weap.hold_type())
            .unwrap_or(HoldType::Invalid);
        let mut ctx = ActionCtx {
            entity: msg.entity,
            ap: &mut ap,
            schedule: &mut schedule,
            anim_state: &mut anim_state,
            lib,
            jump_controller: jc,
            gravity: gravity_mut,
            // Physics flags aren't needed at entry time (update-only);
            // default them here rather than querying.
            is_grounded,
            ground_just_regained: false,
            ground_just_lost: false,
            wall_grab_available: false,
            zipline_available: false,
            high_velocity_landing: false,
            broadcasts: &mut broadcasts,
            weapon_hold_type,
        };

        if !new_action.can_enter(&ctx, msg.substate) {
            bevy::log::debug!(
                "action_start_dispatch: can_enter returned false for {:?} on {:?}",
                msg.action,
                msg.entity,
            );
            continue;
        }

        // If an action is already running, tear it down first — only if
        // it permits being overridden.
        if let Some(prev) = active.0.as_mut() {
            if !prev.can_exit(&ctx) {
                bevy::log::debug!(
                    "action_start_dispatch: current {:?} vetoed exit on {:?}",
                    prev.main_action(),
                    msg.entity,
                );
                continue;
            }
            prev.on_exit(&mut ctx);
            // The legacy pipeline emits EndActionMessage when a schedule
            // finishes; we emit it here on override so do_end_action's
            // flag-clearing still runs for any unmigrated flag work.
            end_writer.write(EndActionMessage {
                entity: msg.entity,
                action: prev.main_action(),
                subaction: msg.substate,
            });
        }

        new_action.on_enter(&mut ctx, msg.substate);
        ap.last_action = msg.action;
        ap.set_sub_state_0(msg.action, msg.substate);
        active.0 = Some(new_action);

        for payload in broadcasts {
            broadcast_writer.write(AnimatorBroadcastEvent {
                entity: msg.entity,
                payload,
            });
        }
    }
}

/// Per-tick driver for `ActiveAction`.  Calls `update`, handles
/// Continue/Finished/HandoffTo, wires broadcasts and end-action messages.
///
/// Physics-edge detection is done locally (per-entity `was_grounded`
/// cache) rather than by reading `AnimatorRuntime.ctx`.  The animator
/// runtime lives in the Update schedule while this system runs in
/// FixedUpdate — the schedules tick at different cadences, so a
/// single-frame edge set by one can be missed by the other.  Computing
/// the edge here ourselves from `Oni2AnimState.is_grounded` keeps the
/// Action's ground-contact signal consistent with this system's cadence.
pub fn action_dispatch_system(
    time: Res<Time>,
    registry: Res<ActionRegistry>,
    mut query: Query<(
        Entity,
        &mut ActionPlayer,
        &mut ActiveAction,
        &mut AnimSchedule,
        &mut Oni2AnimState,
        &Oni2AnimLibrary,
        Option<&JumpController>,
        Option<&AnimatorRuntime>,
        Option<&mut super::gravity::GravityModifiers>,
        Option<&Inventory>,
    )>,
    weapons: Query<&Weapon>,
    mut broadcast_writer: MessageWriter<AnimatorBroadcastEvent>,
    mut end_writer: MessageWriter<EndActionMessage>,
    mut was_grounded: Local<bevy::ecs::entity::EntityHashMap<bool>>,
) {
    let dt = time.delta_secs();
    for (
        entity,
        mut ap,
        mut active,
        mut schedule,
        mut anim_state,
        lib,
        jc,
        animator_opt,
        gravity_opt,
        inv_opt,
    ) in &mut query
    {
        // Derive ground edges locally so we don't depend on
        // AnimatorRuntime's Update-scheduled ctx.  Default prev to `true`
        // so a fresh spawn doesn't spuriously fire "just regained".
        let prev_grounded = was_grounded.get(&entity).copied().unwrap_or(true);
        let curr_grounded = anim_state.is_grounded;
        was_grounded.insert(entity, curr_grounded);
        let ground_just_regained = !prev_grounded && curr_grounded;
        let ground_just_lost = prev_grounded && !curr_grounded;

        let Some(mut current) = active.0.take() else {
            continue;
        };

        // Still pull the "world-contact" flags (wall grab, zipline,
        // high-velocity landing) from AnimatorRuntime since those aren't
        // simple per-entity derivations.  Missing a frame on them is
        // less critical than the ground edge.
        let (wall_grab, zipline, high_vel_land) = animator_opt
            .map(|a| {
                (
                    a.ctx.wall_grab_available,
                    a.ctx.zipline_available,
                    a.ctx.high_velocity_landing,
                )
            })
            .unwrap_or((false, false, false));

        let mut broadcasts: Vec<String> = Vec::new();
        let is_grounded = anim_state.is_grounded;
        let gravity_mut = gravity_opt.map(|g| g.into_inner());
        let weapon_hold_type = inv_opt
            .and_then(|inv| inv.current_weapon_entity())
            .and_then(|weap_ent| weapons.get(weap_ent).ok())
            .map(|weap| weap.hold_type())
            .unwrap_or(HoldType::Invalid);
        let mut ctx = ActionCtx {
            entity,
            ap: &mut ap,
            schedule: &mut schedule,
            anim_state: &mut anim_state,
            lib,
            jump_controller: jc,
            gravity: gravity_mut,
            is_grounded,
            ground_just_regained,
            ground_just_lost,
            wall_grab_available: wall_grab,
            zipline_available: zipline,
            high_velocity_landing: high_vel_land,
            broadcasts: &mut broadcasts,
            weapon_hold_type,
        };

        let outcome = current.update(&mut ctx, dt);

        match outcome {
            ActionUpdate::Continue => {
                active.0 = Some(current);
            }
            ActionUpdate::Finished => {
                let which = current.main_action();
                current.on_exit(&mut ctx);
                // Clear any lingering schedule so
                // `creature_movement_anim_system` sees
                // `schedule.finished == true` on the next tick and
                // locomotion resumes.  Individual Action on_exits
                // shouldn't have to remember this invariant each time.
                ctx.schedule.clear();
                active.0 = None;
                end_writer.write(EndActionMessage {
                    entity,
                    action: which,
                    subaction: 0,
                });
            }
            ActionUpdate::HandoffTo {
                action: next,
                subaction,
            } => {
                // End the outgoing action.
                let outgoing = current.main_action();
                current.on_exit(&mut ctx);
                end_writer.write(EndActionMessage {
                    entity,
                    action: outgoing,
                    subaction,
                });

                // Start the new action.  If the registry has no factory
                // (action not yet ported), leave ActiveAction empty and
                // fire a StartActionMessage-style broadcast so the legacy
                // pipeline could pick it up — but we don't emit
                // StartActionMessage directly to avoid re-entering this
                // same dispatch system.  Instead, a caller can watch for
                // broadcasts and react.
                if let Some(mut next_action) = registry.make(next) {
                    if next_action.can_enter(&ctx, subaction) {
                        next_action.on_enter(&mut ctx, subaction);
                        active.0 = Some(next_action);
                    } else {
                        bevy::log::debug!(
                            "action handoff {:?}→{:?} rejected by can_enter on {:?}",
                            outgoing,
                            next,
                            entity
                        );
                    }
                } else {
                    bevy::log::debug!(
                        "action handoff {:?}→{:?} requested on {:?} but no factory registered",
                        outgoing,
                        next,
                        entity
                    );
                }
            }
        }

        for payload in broadcasts {
            broadcast_writer.write(AnimatorBroadcastEvent { entity, payload });
        }
    }
}

// ---------------------------------------------------------------------------
// Auto-attach
// ---------------------------------------------------------------------------

/// Ensure every entity with an `ActionPlayer` also has `ActiveAction`
/// AND `AnimSchedule`.  The new dispatch queries require BOTH; without
/// the schedule component pre-attached, a fresh entity's first
/// StartActionMessage would silently miss the query and never fire
/// `on_enter`.  Idempotent — filtered on `Without<...>`.
///
/// Safety net, not the canonical path.  `AnimatorBundle` in
/// `animator/mod.rs` is the one-stop spawn for every animator-driven
/// entity and already includes `ActiveAction` + `AnimSchedule`.  This
/// system catches stragglers that grow an `ActionPlayer` at runtime
/// (or old spawn paths that slipped through migration) so they still
/// get the dispatch components before the next tick — but prefer
/// adding `AnimatorBundle` at spawn time whenever you can, since
/// `commands.insert` here flushes at end-of-stage and the first
/// dispatch after the add still misses.
pub fn ensure_active_action_component(
    mut commands: Commands,
    missing_active: Query<Entity, (With<ActionPlayer>, Without<ActiveAction>)>,
    missing_schedule: Query<Entity, (With<ActionPlayer>, Without<AnimSchedule>)>,
) {
    for e in &missing_active {
        commands.entity(e).insert(ActiveAction::default());
    }
    for e in &missing_schedule {
        commands.entity(e).insert(AnimSchedule::default());
    }
}
