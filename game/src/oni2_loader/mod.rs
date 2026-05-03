/*
 * oni2_loader/mod.rs — Oni2LoaderPlugin: ONI2 asset loading and runtime systems.
 *
 * Plugin initialises all loader resources (registries, debug flags, checkpoint
 * index) and registers every Update system: ONI2 animation playback, head IK,
 * parent resolution, creature movement, fog, skyhat, scroni VM tick, curve
 * followers, and all debug-draw systems.  Also re-exports formation mode
 * helpers (setup_formation_scene, free_camera_system) for use in main.rs.
 */
pub mod animation;
pub mod asleep;
pub mod components;
pub mod curve;
pub mod environment;
pub mod formation;
pub mod headik;
pub mod layout_loader;
pub mod light_glow;
pub mod parsers;
pub mod physics;
pub mod registries;
pub mod spawn;
pub mod testanim;
pub mod utils;

pub use animation::*;
pub use components::*;
pub use environment::*;
pub use formation::{free_camera_system, setup_formation_scene};
pub use headik::{head_ik_setup_system, head_ik_system};
pub use layout_loader::*;
pub use registries::*;
pub use spawn::*;
pub use testanim::*;

use avian3d::prelude::*;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;

use crate::menu::InGameEntity;
use crate::oni2_loader::curve::NurbsCurve;
use crate::oni2_loader::parsers::actor_xml::*;
use crate::oni2_loader::parsers::animation::*;
// use crate::oni2_loader::parsers::anims::*;
use crate::oni2_loader::parsers::bound::*;
use crate::oni2_loader::parsers::entity_type::*;
use crate::oni2_loader::parsers::layout::*;
use crate::oni2_loader::parsers::mesh::*;
use crate::oni2_loader::parsers::model::*;
use crate::oni2_loader::parsers::skeleton::*;
use crate::oni2_loader::parsers::types::*;
// use crate::oni2_loader::parsers::texture::load_tga_file as texture_load_tga_file;
use crate::oni2_loader::utils::bone::*;
use crate::scroni;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct Oni2LoaderPlugin;

impl Plugin for Oni2LoaderPlugin {
    fn build(&self, app: &mut App) {
        use crate::menu::AppState;

        app.add_plugins(asleep::AsleepPlugin)
            .insert_resource(DebugBoundsVisible(false))
            .insert_resource(DebugSkeletonVisible(false))
            .init_resource::<DebugLightGridState>()
            .init_resource::<registries::EntityLibrary>()
            .init_resource::<registries::AnimRegistry>()
            .init_resource::<registries::ProjLibrary>()
            .init_resource::<registries::FxLibrary>()
            .init_resource::<registries::ParticleLibrary>()
            .init_resource::<registries::ExplosionRegistry>()
            .init_resource::<environment::TextureCollections>()
            .init_resource::<components::CurrentCheckpointIndex>()
            .init_resource::<light_glow::LightGlowMesh>()
            .add_systems(Startup, (load_global_registries, load_global_explosions))
            .add_systems(
                PhysicsSchedule,
                physics::octree_one_way_contact_system
                    .run_if(|q: Query<&crate::camera::components::CameraController>| {
                        q.iter().any(|c| {
                            c.active_mode == crate::camera::components::ActiveCameraMode::Script
                        })
                    })
                    .in_set(NarrowPhaseSystems::Last),
            )
            .add_systems(
                Update,
                (
                    toggle_debug_bounds,
                    toggle_debug_skeleton,
                    update_oni2_animation,
                    head_ik_setup_system,
                    head_ik_system.after(update_oni2_animation),
                    resolve_pending_parents_system,
                    creature_movement_anim_system,
                    ground_snap_system,
                    apply_fog_to_camera.run_if(resource_exists::<FogEnabled>),
                    update_skyhat,
                    // fxLightGlow corona pipeline.  resolve runs first
                    // so freshly-spawned PendingGlow tags get their
                    // billboard child this frame; the per-frame
                    // billboard/occlusion update runs after.
                    light_glow::resolve_pending_glow_system,
                    light_glow::update_light_glow_billboards
                        .after(light_glow::resolve_pending_glow_system),
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                (
                    scroni::vm::update_broadcast_triggers.before(scroni::vm::scroni_tick_system),
                    scroni::vm::checkpoint_trigger_system.before(scroni::vm::scroni_tick_system),
                    scroni::vm::scroni_tick_system,
                    scroni::vm::cleanup_scroni_text,
                    scroni::vm::update_screen_fade_system,
                    scroni::vm::apply_shader_locals_system,
                    scroni_curve_bridge_system,
                    curve_follower_system,
                )
                    // ScrOni also ticks in FrontEnd because the
                    // PAGE_3D backdrop (uitest layout) attaches
                    // `$newgame:CameraMotion` to actor_Tunnel1 — that
                    // script is what frames the menu camera.  Without
                    // this, the script gets attached at page-start
                    // but its `do forever` loop never runs and the
                    // camera stays on the static CAMERA_INIT pose
                    // (which lands the projector in the upper-left).
                    .run_if(
                        in_state(AppState::InGame).or(in_state(AppState::FrontEnd)),
                    ),
            )
            .add_systems(
                Update,
                (
                    toggle_debug_fog,
                    toggle_debug_light_grid,
                    update_debug_light_grid,
                )
                    .run_if(in_state(AppState::InGame)),
            )
            // Resource cleanup on layout exit — clears the
            // DebugLightGridState's stale Entity HashMap so the
            // next layout starts with a fresh grid.  The lights
            // themselves are despawned by `cleanup_game` via the
            // InGameEntity tag.
            .add_systems(
                OnExit(AppState::InGame),
                environment::reset_debug_light_grid_on_exit,
            )
            .add_systems(
                Update,
                (
                    debug_draw_bounds,
                    debug_draw_capsules,
                    debug_draw_curves,
                    debug_draw_attack_wedges,
                    environment::debug_draw_light_grid,
                )
                    .run_if(in_state(AppState::InGame))
                    .run_if(|v: Res<DebugBoundsVisible>| v.0),
            )
            .add_systems(
                Update,
                debug_draw_skeleton
                    .run_if(in_state(AppState::InGame))
                    .run_if(|v: Res<DebugSkeletonVisible>| v.0),
            )
            .add_systems(
                Update,
                (testanim_input_system, update_testanim_hud)
                    .run_if(in_state(AppState::InGame).and(resource_exists::<TestAnimMode>)),
            )
            .add_systems(
                Update,
                orbit_camera_system.run_if(in_state(AppState::InGame).and(
                    |t1: Option<Res<TestAnimMode>>, t2: Option<Res<TestEntityMode>>| {
                        t1.is_some() || t2.is_some()
                    },
                )),
            );
        // NOTE: the old `Last`-scheduled `reset_anim_transition_edges`
        // system was removed when `anim_just_started` migrated from a
        // shared bool consumed by multiple systems (which needed a late
        // reset to preserve the one-tick pulse) to an intention bit
        // drained into `AnimStartedMessage` events by
        // `anim_start_emit_system` (in `animator::systems`).  The emit
        // system clears the bit itself, so no separate reset is needed.
    }
}
