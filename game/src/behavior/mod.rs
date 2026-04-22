/*
 * behavior/mod.rs — BehaviorPlugin: top-level AI behavior layer.
 *
 * Parallels `animator/` one level up.  The animator orchestrates *how* a
 * character moves/animates; this module orchestrates *what the character
 * is doing* — Idle / Fight / Follow / Goto / Patrol / Retreat / Pad.
 *
 * Architecture mirrors `animator/actions/` (the jump hybrid pattern):
 *   - `BehaviorDriver` (in statemachine/drivers/behavior.rs) owns the
 *     FSM graph authored in text.  Its state cursor IS the current
 *     behavior kind.
 *   - `Behavior` trait + `ActiveBehavior` component carry the per-frame
 *     Rust-side work (velocity steering, timers, lifecycle).
 *   - `BehaviorRegistry` is a factory map from `BehaviorKind` → boxed
 *     `Behavior` impl.
 *   - Dispatch flow: FSM tick → `BehaviorOutput.started_behavior` →
 *     `StartBehaviorMessage` → dispatcher pulls factory, calls on_enter.
 *
 * ScrOni commands (`fight`/`follow`/`goto`/`patrol`/`retreat`/`idle`)
 * and any other system can push inbound requests by writing to the
 * entity's `BehaviorRuntime.ctx.requested_*` flag + filling
 * `pending_params` — the next FSM tick routes to the right state.
 */
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

use avian3d::prelude::LinearVelocity;

use crate::combat::components::Fighter;
use crate::menu::AppState;
use crate::statemachine::core::{SmData, SmRuntime};
use crate::statemachine::drivers::behavior::{self, BehaviorCtx, BehaviorDriver, BehaviorKind};

pub mod impls;

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Per-entity FSM runtime for the behavior layer.  Owns the SM cursor +
/// tick context + any pending params written by scripts/systems before
/// the next tick routes them into a `StartBehavior` action.
#[derive(Component)]
pub struct BehaviorRuntime {
    pub sm: SmRuntime<BehaviorDriver>,
    pub ctx: BehaviorCtx,
    /// Params for the *next* behavior start.  Scripts write here before
    /// flipping one of the `ctx.requested_*` flags; the dispatcher reads
    /// them in `on_enter` and clears them after.
    pub pending_params: BehaviorParams,
}

impl BehaviorRuntime {
    /// Build a runtime parked at `IDLE_STATE` (the canonical default).
    pub fn new(data: Arc<SmData<BehaviorDriver>>) -> Self {
        let initial = data.index_of_or_zero("IDLE_STATE");
        BehaviorRuntime {
            sm: SmRuntime::new(data, initial),
            ctx: BehaviorCtx::default(),
            pending_params: BehaviorParams::default(),
        }
    }
}

/// Holds the current Rust-side Behavior impl.  `None` means the entity is
/// between behaviors (the FSM cursor still names a state — IDLE by
/// default — but no impl is actively ticking).
#[derive(Component, Default)]
pub struct ActiveBehavior(pub Option<Box<dyn Behavior>>);

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

/// Variant-union of params any behavior might need on entry.  Fields are
/// optional; each `Behavior` impl reads the ones it cares about.  Keeps
/// the `StartBehaviorMessage` schema simple (no generics) while letting
/// every behavior declare its own required/optional inputs.
#[derive(Clone, Debug, Default)]
pub struct BehaviorParams {
    /// Follow-target / fight-target entity.
    pub target_entity: Option<Entity>,
    /// Goto destination in world space.  Used when `path` is empty.
    pub target_point: Option<Vec3>,
    /// Pre-computed waypoint chain (e.g. from NavGraph A*).  Preferred
    /// over `target_point` when non-empty — GotoBehavior walks the list.
    pub path: Vec<Vec3>,
    /// Arrival radius for Goto / Follow (against the last waypoint).
    pub within: Option<f32>,
    /// Speed throttle (0..1) applied to MOVE_SPEED for Goto.
    pub speed_throttle: Option<f32>,
    /// Named path for Patrol.
    pub patrol_path: Option<String>,
    /// Idle duration in seconds (0 or None = run until interrupted).
    pub idle_duration: Option<f32>,
}

// ---------------------------------------------------------------------------
// Behavior trait
// ---------------------------------------------------------------------------

/// Per-tick context handed to `Behavior` trait methods.  Built by the
/// dispatcher from the entity's query row.  Combat-oriented behaviors
/// that need to kick off an attack animation call `anim_lib.play(name,
/// anim_state)` directly — the same helper the player's `DoAttack`
/// ultimately resolves to (shared DNA: both paths converge on
/// `Oni2AnimLibrary::play`).
pub struct BehaviorRunCtx<'a> {
    pub entity: Entity,
    pub params: &'a BehaviorParams,
    pub transform: &'a mut Transform,
    pub velocity: &'a mut LinearVelocity,
    pub fighter: &'a mut Fighter,
    /// AiFighter data slot — `target`, `manual_target`.  `None` for
    /// non-AI entities (the player doesn't get a BehaviorRuntime so this
    /// is rare in practice, but the field is optional for symmetry).
    pub ai_fighter: Option<&'a mut crate::ai::components::AiFighter>,
    /// Optional AiPadCommands — reserved for future behaviors (Pad /
    /// Follow) that want to drive the AI input FSM through pad pulses.
    /// Combat doesn't use this path; it plays animations directly.
    pub pad_commands: Option<&'a mut crate::ai::fsm::AiPadCommands>,
    /// Animation library shared across rigs of the same Oni2 entity
    /// type.  Read-only — behaviors call `.play(alias, &mut anim_state)`
    /// to kick an anim, same as the player.fsm `DoAttack` pathway.
    pub anim_lib: Option<&'a crate::oni2_loader::animation::Oni2AnimLibrary>,
    /// Current animation playback state on this entity.  Mutated by
    /// `anim_lib.play(..)` to switch the active anim.
    pub anim_state: Option<&'a mut crate::oni2_loader::animation::Oni2AnimState>,
    /// The .atk-driven attack state machine.  FightBehavior toggles
    /// `ctx.got_cookie` here on enter / exit so the attack machine
    /// switches between no_cookie (ambient) and cookie (attacking) rows.
    pub attack_runtime: Option<&'a mut crate::fightai::components::AttackRuntime>,
    /// World position of the entity currently stored in
    /// `ai_fighter.target`, if any.  Pre-resolved by the dispatcher so
    /// behaviors don't need their own transforms query.
    pub target_position: Option<Vec3>,
    /// Accumulator — values pushed by a behavior are applied by the
    /// dispatcher after the call so the Behavior doesn't need a Commands
    /// handle of its own.  Empty for the current first cut (Idle/Goto/
    /// Fight don't need it); Follow/Pad will add variants.
    pub side_effects: &'a mut Vec<BehaviorSideEffect>,
}

/// Deferred work a behavior wants the dispatcher to apply.  Keeps the
/// Behavior trait purely data-in/data-out so the dispatcher holds the
/// only Commands handle.
#[derive(Debug)]
pub enum BehaviorSideEffect {
    // Reserved for future behavior impls — e.g. Fight might ask the
    // dispatcher to poke the entity's FightRuntime context.
}

#[derive(Debug, Clone)]
pub enum BehaviorUpdate {
    /// Behavior is still running.
    Continue,
    /// Behavior completed naturally — dispatcher clears ActiveBehavior
    /// and fires EBehaviorDone into the FSM so it can transition.
    Finished,
}

/// One concrete behavior impl (IdleBehavior, GotoBehavior, …).  Each impl
/// owns its per-tick state; the dispatcher swaps the Box<dyn Behavior>
/// stored in `ActiveBehavior` on entry/exit.
pub trait Behavior: Send + Sync {
    fn kind(&self) -> BehaviorKind;

    fn can_enter(&self, _ctx: &BehaviorRunCtx<'_>) -> bool {
        true
    }

    fn on_enter(&mut self, ctx: &mut BehaviorRunCtx<'_>);

    fn update(&mut self, ctx: &mut BehaviorRunCtx<'_>, dt: f32) -> BehaviorUpdate;

    fn can_exit(&self, _ctx: &BehaviorRunCtx<'_>) -> bool {
        true
    }

    fn on_exit(&mut self, ctx: &mut BehaviorRunCtx<'_>);
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Factory map: BehaviorKind → boxed impl.  Populated at plugin build via
/// `impls::register_default_behaviors`.
#[derive(Resource, Default)]
pub struct BehaviorRegistry {
    factories: HashMap<BehaviorKind, Box<dyn Fn() -> Box<dyn Behavior> + Send + Sync>>,
}

impl BehaviorRegistry {
    pub fn register<F>(&mut self, kind: BehaviorKind, factory: F)
    where
        F: Fn() -> Box<dyn Behavior> + Send + Sync + 'static,
    {
        self.factories.insert(kind, Box::new(factory));
    }

    pub fn make(&self, kind: BehaviorKind) -> Option<Box<dyn Behavior>> {
        self.factories.get(&kind).map(|f| f())
    }

    pub fn contains(&self, kind: BehaviorKind) -> bool {
        self.factories.contains_key(&kind)
    }
}

// ---------------------------------------------------------------------------
// Shared FSM data
// ---------------------------------------------------------------------------

/// Parsed singleton of the embedded behavior.fsm.  Shared Arc so every
/// actor's `BehaviorRuntime` can clone it cheaply.
#[derive(Resource)]
pub struct BehaviorFsmData(pub Arc<SmData<BehaviorDriver>>);

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Emitted by the FSM tick when a `StartBehavior(kind)` action fires.
/// The dispatcher consumes this to swap in a fresh Behavior impl.
#[derive(Message)]
pub struct StartBehaviorMessage {
    pub entity: Entity,
    pub kind: BehaviorKind,
}

/// Emitted by the dispatcher when the current Behavior's `update`
/// returns `Finished`.  Surfaces up to ScrOni for blocking-action
/// resolution.
#[derive(Message)]
pub struct EndBehaviorMessage {
    pub entity: Entity,
    pub kind: BehaviorKind,
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct BehaviorPlugin;

impl Plugin for BehaviorPlugin {
    fn build(&self, app: &mut App) {
        let mut registry = BehaviorRegistry::default();
        impls::register_default_behaviors(&mut registry);

        app.insert_resource(registry)
            .add_message::<StartBehaviorMessage>()
            .add_message::<EndBehaviorMessage>()
            .add_systems(Startup, load_behavior_fsm)
            .add_systems(
                Update,
                (behavior_attach_system, ensure_active_behavior_component),
            )
            .add_systems(
                FixedUpdate,
                (
                    behavior_fsm_tick_system,
                    behavior_start_dispatch_system,
                    behavior_update_dispatch_system,
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

// ---------------------------------------------------------------------------
// Startup — parse embedded FSM
// ---------------------------------------------------------------------------

fn load_behavior_fsm(mut commands: Commands) {
    match behavior::load_embedded_arc() {
        Ok(data) => {
            let n = data.states.len();
            commands.insert_resource(BehaviorFsmData(data));
            info!("behavior: embedded FSM loaded — {} states", n);
        }
        Err(e) => error!("behavior: failed to parse embedded FSM: {}", e),
    }
}

// ---------------------------------------------------------------------------
// Safety-net component attach
// ---------------------------------------------------------------------------

/// Every entity with a `BehaviorRuntime` also needs `ActiveBehavior`.
/// Spawn sites should insert both together via a bundle; this catches
/// stragglers that only got the runtime.  Idempotent.
pub fn ensure_active_behavior_component(
    mut commands: Commands,
    missing: Query<Entity, (With<BehaviorRuntime>, Without<ActiveBehavior>)>,
) {
    for e in &missing {
        commands.entity(e).insert(ActiveBehavior::default());
    }
}

/// Attach `BehaviorRuntime` + `ActiveBehavior` to any newly-spawned
/// `AiFighter` that doesn't have them yet.  Matches the pattern
/// `ai_attach_fsm_system` uses for the input-side FSM — lazy attach
/// from the resource, so the spawner doesn't need to thread
/// `BehaviorFsmData` through its asset bundles.
pub fn behavior_attach_system(
    mut commands: Commands,
    data: Option<Res<BehaviorFsmData>>,
    query: Query<
        Entity,
        (
            With<crate::ai::components::AiFighter>,
            Without<BehaviorRuntime>,
        ),
    >,
) {
    let Some(data) = data else { return };
    for entity in &query {
        commands.entity(entity).insert((
            BehaviorRuntime::new(Arc::clone(&data.0)),
            ActiveBehavior::default(),
        ));
    }
}

// ---------------------------------------------------------------------------
// FSM tick system
// ---------------------------------------------------------------------------

/// Advance each actor's BehaviorRuntime FSM once per FixedUpdate.  Drains
/// `BehaviorOutput.started_behavior` into a `StartBehaviorMessage`, then
/// clears the per-tick request flags (pulse semantics).
pub fn behavior_fsm_tick_system(
    mut query: Query<(Entity, &mut BehaviorRuntime)>,
    mut start_writer: MessageWriter<StartBehaviorMessage>,
) {
    for (entity, mut rt) in &mut query {
        let current_name = rt.sm.data.state_name(rt.sm.current_state).to_string();
        rt.ctx.current_state = current_name;

        // Split the two mutable borrows — `rt.sm.tick(&mut rt.ctx)` would
        // alias `rt` to itself.  Go through `as_mut` to get disjoint refs.
        let rt_mut = rt.as_mut();
        let output = rt_mut.sm.tick(&mut rt_mut.ctx);

        if let Some(kind) = output.started_behavior {
            start_writer.write(StartBehaviorMessage { entity, kind });
        }

        // Clear pulse flags — callers must re-set each tick they want the
        // request to fire.  `behavior_done` is also a single-tick edge
        // written by `behavior_update_dispatch_system`, so it gets
        // cleared here too.
        rt.ctx.requested_idle = false;
        rt.ctx.requested_fight = false;
        rt.ctx.requested_follow = false;
        rt.ctx.requested_goto = false;
        rt.ctx.requested_patrol = false;
        rt.ctx.requested_retreat = false;
        rt.ctx.requested_pad = false;
        rt.ctx.behavior_done = false;
    }
}

// ---------------------------------------------------------------------------
// Start dispatch
// ---------------------------------------------------------------------------

/// Consumes `StartBehaviorMessage`s, instantiates the requested Behavior
/// impl via `BehaviorRegistry`, tears down the outgoing impl (if any),
/// and installs the new one.  Mirrors `animator::actions::mod::
/// action_start_dispatch_system`.
pub fn behavior_start_dispatch_system(
    mut reader: MessageReader<StartBehaviorMessage>,
    registry: Res<BehaviorRegistry>,
    mut query: Query<(
        &mut BehaviorRuntime,
        &mut ActiveBehavior,
        &mut Transform,
        &mut LinearVelocity,
        &mut Fighter,
        Option<&mut crate::ai::components::AiFighter>,
        Option<&mut crate::ai::fsm::AiPadCommands>,
        Option<&crate::oni2_loader::animation::Oni2AnimLibrary>,
        Option<&mut crate::oni2_loader::animation::Oni2AnimState>,
        Option<&mut crate::fightai::components::AttackRuntime>,
    )>,
    target_transforms: Query<&GlobalTransform>,
    mut end_writer: MessageWriter<EndBehaviorMessage>,
) {
    for msg in reader.read() {
        let Ok((
            mut runtime,
            mut active,
            mut transform,
            mut velocity,
            mut fighter,
            ai_fighter_opt,
            pad_commands_opt,
            anim_lib_opt,
            anim_state_opt,
            attack_runtime_opt,
        )) = query.get_mut(msg.entity)
        else {
            continue;
        };

        // Re-entry guard: if the active behavior already matches the
        // requested kind, drop the message silently.  Prevents the
        // "strobe" where an upstream pulse (e.g. ScrOni's `fight` stmt
        // re-firing TriggerFight, or any FSM rule that unconditionally
        // keeps firing StartBehavior) would tear down and re-instantiate
        // the current behavior every tick — resetting its internal
        // timers (FightBehavior.decision_timer, Goto.cursor, etc.) and
        // causing visible flicker.  Behaviors persist across repeated
        // start requests of the same kind; only a kind change does the
        // full on_exit/on_enter swap.
        if let Some(prev) = active.0.as_ref()
            && prev.kind() == msg.kind
        {
            continue;
        }

        let Some(mut new_behavior) = registry.make(msg.kind) else {
            warn!(
                "behavior_start_dispatch: no factory for {:?} on {:?}",
                msg.kind, msg.entity
            );
            continue;
        };

        // Resolve target's world position BEFORE moving the AiFighter ref
        // into ctx — target_transforms is a disjoint read query.
        let target_position = ai_fighter_opt
            .as_ref()
            .and_then(|af| af.target)
            .and_then(|e| target_transforms.get(e).ok())
            .map(|gt| gt.translation());

        let mut side_effects: Vec<BehaviorSideEffect> = Vec::new();
        let runtime_mut = runtime.as_mut();
        let params_snapshot = runtime_mut.pending_params.clone();
        let ai_fighter_mut = ai_fighter_opt.map(|a| a.into_inner());
        let pad_commands_mut = pad_commands_opt.map(|p| p.into_inner());
        let anim_state_mut = anim_state_opt.map(|s| s.into_inner());
        let attack_runtime_mut = attack_runtime_opt.map(|r| r.into_inner());
        let mut ctx = BehaviorRunCtx {
            entity: msg.entity,
            params: &params_snapshot,
            transform: &mut transform,
            velocity: &mut velocity,
            fighter: &mut fighter,
            ai_fighter: ai_fighter_mut,
            pad_commands: pad_commands_mut,
            anim_lib: anim_lib_opt,
            anim_state: anim_state_mut,
            attack_runtime: attack_runtime_mut,
            target_position,
            side_effects: &mut side_effects,
        };

        if !new_behavior.can_enter(&ctx) {
            debug!(
                "behavior_start_dispatch: can_enter rejected {:?} on {:?}",
                msg.kind, msg.entity
            );
            continue;
        }

        if let Some(prev) = active.0.as_mut() {
            if !prev.can_exit(&ctx) {
                debug!(
                    "behavior_start_dispatch: current {:?} vetoed exit on {:?}",
                    prev.kind(),
                    msg.entity
                );
                continue;
            }
            let outgoing_kind = prev.kind();
            prev.on_exit(&mut ctx);
            end_writer.write(EndBehaviorMessage {
                entity: msg.entity,
                kind: outgoing_kind,
            });
        }

        new_behavior.on_enter(&mut ctx);
        active.0 = Some(new_behavior);

        // Side-effects are currently empty (no variants); the match is a
        // placeholder so adding variants later is a localized edit.
        for _effect in side_effects {
            // match _effect { /* ... */ }
        }

        // Params are consumed on entry — clear so the next start reads fresh.
        runtime_mut.pending_params = BehaviorParams::default();
    }
}

// ---------------------------------------------------------------------------
// Update dispatch
// ---------------------------------------------------------------------------

/// Per-tick call to `Behavior::update`.  On `Finished`, fire
/// `EndBehaviorMessage` AND set `runtime.ctx.behavior_done = true` so
/// the FSM's next tick sees `EBehaviorDone` and transitions per the
/// authored graph.
pub fn behavior_update_dispatch_system(
    time: Res<Time>,
    mut query: Query<(
        Entity,
        &mut BehaviorRuntime,
        &mut ActiveBehavior,
        &mut Transform,
        &mut LinearVelocity,
        &mut Fighter,
        Option<&mut crate::ai::components::AiFighter>,
        Option<&mut crate::ai::fsm::AiPadCommands>,
        Option<&crate::oni2_loader::animation::Oni2AnimLibrary>,
        Option<&mut crate::oni2_loader::animation::Oni2AnimState>,
        Option<&mut crate::fightai::components::AttackRuntime>,
    )>,
    target_transforms: Query<&GlobalTransform>,
    mut end_writer: MessageWriter<EndBehaviorMessage>,
) {
    let dt = time.delta_secs();

    for (
        entity,
        mut runtime,
        mut active,
        mut transform,
        mut velocity,
        mut fighter,
        ai_fighter_opt,
        pad_commands_opt,
        anim_lib_opt,
        anim_state_opt,
        attack_runtime_opt,
    ) in &mut query
    {
        let Some(mut current) = active.0.take() else {
            continue;
        };

        let target_position = ai_fighter_opt
            .as_ref()
            .and_then(|af| af.target)
            .and_then(|e| target_transforms.get(e).ok())
            .map(|gt| gt.translation());

        let mut side_effects: Vec<BehaviorSideEffect> = Vec::new();
        let params_snapshot = runtime.pending_params.clone();
        let ai_fighter_mut = ai_fighter_opt.map(|a| a.into_inner());
        let pad_commands_mut = pad_commands_opt.map(|p| p.into_inner());
        let anim_state_mut = anim_state_opt.map(|s| s.into_inner());
        let attack_runtime_mut = attack_runtime_opt.map(|r| r.into_inner());
        let mut ctx = BehaviorRunCtx {
            entity,
            params: &params_snapshot,
            transform: &mut transform,
            velocity: &mut velocity,
            fighter: &mut fighter,
            ai_fighter: ai_fighter_mut,
            pad_commands: pad_commands_mut,
            anim_lib: anim_lib_opt,
            anim_state: anim_state_mut,
            attack_runtime: attack_runtime_mut,
            target_position,
            side_effects: &mut side_effects,
        };

        let outcome = current.update(&mut ctx, dt);

        match outcome {
            BehaviorUpdate::Continue => {
                active.0 = Some(current);
            }
            BehaviorUpdate::Finished => {
                let kind = current.kind();
                current.on_exit(&mut ctx);
                active.0 = None;
                end_writer.write(EndBehaviorMessage { entity, kind });
                // Signal to the FSM tick on the next FixedUpdate pass.
                runtime.ctx.behavior_done = true;
            }
        }

        for _effect in side_effects {
            // match _effect { /* ... */ }
        }
    }
}
