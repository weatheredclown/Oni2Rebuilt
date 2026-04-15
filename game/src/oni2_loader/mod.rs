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
pub mod components;
pub mod curve;
pub mod environment;
pub mod formation;
pub mod headik;
pub mod layout_loader;
pub mod parsers;
pub mod registries;
pub mod spawn;
pub mod testanim;
pub mod utils;

pub use animation::*;
pub use components::*;
pub use formation::{free_camera_system, setup_formation_scene};
pub use headik::{head_ik_setup_system, head_ik_system};
pub use environment::*;
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

        app.insert_resource(DebugBoundsVisible(false))
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
            .add_systems(Startup, (load_global_registries, load_global_explosions))
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
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                (
                    scroni::vm::update_broadcast_triggers
                        .before(scroni::vm::scroni_tick_system),
                    scroni::vm::checkpoint_trigger_system
                        .before(scroni::vm::scroni_tick_system),
                    scroni::vm::scroni_tick_system,
                    scroni::vm::cleanup_scroni_text,
                    scroni::vm::update_screen_fade_system,
                    scroni::vm::apply_shader_locals_system,
                    scroni_curve_bridge_system,
                    curve_follower_system,
                )
                    .run_if(in_state(AppState::InGame)),
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
            .add_systems(
                Update,
                (
                    debug_draw_bounds,
                    debug_draw_capsules,
                    debug_draw_curves,
                    debug_draw_attack_wedges,
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
    }
}
