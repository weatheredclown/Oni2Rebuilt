/*
 * mover/ — Oni2 character mover A/B prototype.
 *
 * Phase 1 of the mover.cpp port. Today the codebase uses RigidBody::Dynamic
 * capsules driven by `LinearVelocity` writes from `player::systems` and
 * `ai::navigation`. This module adds a parallel path that routes the same
 * desired velocity through `bevy-tnua` (floating character controller) on top
 * of a Dynamic body, so the two backends can be compared without a recompile.
 *
 * Toggle: env var `RB_MOVER` ("dynamic" default, "tnua" enables Tnua path).
 *
 * Architecture mirrors `fx_system::FxPlugin` + `bevy_hanabi`:
 *   - `MoverBackend` resource picks the path (analog: `ParticleSystemDef`-time
 *     decision baked at spawn);
 *   - `mover::components` holds the legacy-shaped data (`MoverDef`, scheme
 *     typestate) and the spawn-time bundle constructor;
 *   - `mover::bridge` is the per-frame translator from legacy
 *     `LinearVelocity` writes (player/AI output) into Tnua's
 *     `TnuaBuiltinWalk` basis (analog: `get_or_create_ptx_asset`).
 *
 * Not yet ported (deferred): jump, ledgehang, walljump, zipline, grapple,
 * snowboard, slope-slide-band classification (slide_steepness vs
 * wall_steepness), Mover Gem backward-rejection, character-vs-character
 * separation. The dynamic body Tnua sits on still allows characters to push
 * each other — fixing that is a separate change (collision layers or custom
 * separation pass).
 */
use bevy::ecs::system::EntityCommands;
use bevy::prelude::*;
use bevy_tnua::prelude::*;
use bevy_tnua_avian3d::prelude::*;

pub mod bridge;
pub mod components;

pub use components::{Oni2Scheme, Oni2SchemeConfig, TnuaCreaturePhysicsBundle};

/// Shared `Oni2SchemeConfig` handle. Created at startup via
/// [`MoverPlugin`] and read by every creature spawn site that uses the Tnua
/// backend.
#[derive(Resource)]
pub struct SharedMoverConfig(pub Handle<Oni2SchemeConfig>);

/// Single chokepoint for creature physics spawning. Branches on
/// [`MoverBackend`] so the call sites in `main.rs` and `oni2_loader/spawn.rs`
/// stay one-liners. `config_handle` is only consulted for the Tnua path.
pub fn insert_creature_physics(
    entity_commands: &mut EntityCommands,
    capsule_radius: f32,
    capsule_length: f32,
    backend: MoverBackend,
    config_handle: Option<Handle<Oni2SchemeConfig>>,
) {
    match backend {
        MoverBackend::Dynamic => {
            entity_commands.insert(crate::combat::CreaturePhysicsBundle::new(
                capsule_radius,
                capsule_length,
            ));
        }
        MoverBackend::Tnua => {
            // Caller is expected to provide the shared config handle; if not,
            // build a fresh default asset (one-off; loses sharing).
            let handle = config_handle.unwrap_or_default();
            entity_commands.insert(TnuaCreaturePhysicsBundle::new(
                capsule_radius,
                capsule_length,
                handle,
            ));
        }
    }
}

/// Which character-mover backend to use at spawn time.
///
/// Resolved at startup from the `RB_MOVER` env var. The two spawn sites
/// (`main::spawn_fallback_player`, `oni2_loader::spawn::*`) read this resource
/// and pick which physics bundle to insert. Once a creature is spawned, its
/// backend is fixed for the entity's lifetime.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MoverBackend {
    /// Today's behavior: `RigidBody::Dynamic` capsule, gravity from Avian,
    /// `LinearVelocity` written directly by player/AI.
    #[default]
    Dynamic,
    /// `bevy-tnua` floating controller on top of `RigidBody::Dynamic`. Player
    /// /AI still write `LinearVelocity` (xz only); `bridge::tnua_basis_from_linvel`
    /// translates that into a `TnuaBuiltinWalk` basis each frame.
    Tnua,
}

impl MoverBackend {
    /// Read `RB_MOVER` env var; default to `Dynamic`. Accepts `tnua` or
    /// `dynamic` (case-insensitive).
    pub fn from_env() -> Self {
        match std::env::var("RB_MOVER")
            .ok()
            .as_deref()
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("tnua") => Self::Tnua,
            _ => Self::Dynamic,
        }
    }
}

/// `run_if` condition: true when the legacy Dynamic-mover path should run.
/// Used to gate systems that write `LinearVelocity` directly (which would
/// otherwise fight Tnua's float spring + action machinery).
pub fn backend_is_dynamic(backend: Res<MoverBackend>) -> bool {
    matches!(*backend, MoverBackend::Dynamic)
}

/// `run_if` condition: true when the Tnua bridge systems should run.
pub fn backend_is_tnua(backend: Res<MoverBackend>) -> bool {
    matches!(*backend, MoverBackend::Tnua)
}

pub struct MoverPlugin;

impl Plugin for MoverPlugin {
    fn build(&self, app: &mut App) {
        let backend = app
            .world()
            .get_resource::<MoverBackend>()
            .copied()
            .unwrap_or_default();

        // Tnua's plugins are always registered (no harm if Dynamic mode is
        // active — entities just won't have TnuaController components).
        // Schedule choice matches the v0.29 demo: Tnua runs in FixedUpdate,
        // alongside Avian's default FixedPostUpdate physics step.
        app.add_plugins(TnuaAvian3dPlugin::new(FixedUpdate));
        app.add_plugins(TnuaControllerPlugin::<Oni2Scheme>::new(FixedUpdate));

        // The config asset type Tnua reads per frame.
        app.init_asset::<Oni2SchemeConfig>();

        // Build and stash a single shared Oni2SchemeConfig handle so every
        // spawn site reuses the same tuning data.
        app.add_systems(PreStartup, init_shared_mover_config);

        if matches!(backend, MoverBackend::Tnua) {
            app.add_systems(
                FixedUpdate,
                (
                    bridge::tnua_basis_from_linvel,
                    bridge::tnua_jump_from_message.after(bridge::tnua_basis_from_linvel),
                )
                    .in_set(TnuaUserControlsSystems),
            );
        }
    }
}

fn init_shared_mover_config(
    mut commands: Commands,
    mut configs: ResMut<Assets<Oni2SchemeConfig>>,
) {
    let handle = configs.add(Oni2SchemeConfig::default());
    commands.insert_resource(SharedMoverConfig(handle));
}
