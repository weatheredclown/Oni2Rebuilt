/*
 * oni2_loader/registries.rs — global asset registries.
 *
 * EntityLibrary: cached map of entity-dir → parsed EntityType + meshes.
 * AnimRegistry: cached map of anim-path → Oni2AnimData.
 * ProjLibrary, FxLibrary, ParticleLibrary: projectile, effect, and particle
 * definitions loaded at startup and looked up by name at runtime.
 * try_load_ptx: helper to lazily load a ParticleSystemDef on first reference.
 */
use bevy::prelude::*;
use std::collections::HashMap;

use crate::oni2_loader::Oni2AnimLibrary;
use crate::oni2_loader::Oni2DebugBounds;
use crate::oni2_loader::Oni2Skeleton;
use crate::oni2_loader::parsers::expl::parse_expl;
use crate::oni2_loader::parsers::effect::{EffectDef, parse_effect};
use crate::oni2_loader::parsers::particle::{ParticleSystemDef, parse_ptx};
use crate::oni2_loader::parsers::projectile::{ProjectileDef, parse_projectile};
use crate::oni2_loader::parsers::settings::parse_settings;
use crate::vfs;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

#[derive(Component, Clone, Default, Debug)]
pub struct TextureUVAnimator {
    pub slides_speed: f32, // U increment per second
    pub slidet_speed: f32, // V increment per second
    pub rotate_speed: f32, // Radians per second
    pub scalet_speed: f32, // Scalar
}

#[derive(Resource, Default)]
pub struct EntityLibrary {
    pub entities: HashMap<String, Oni2EntityType>,
}

#[derive(Resource, Default)]
pub struct AnimRegistry {
    pub libraries: HashMap<
        String,
        (
            Oni2AnimLibrary,
            Option<crate::oni2_loader::parsers::loco::LocomotionController>,
            Option<crate::oni2_loader::parsers::jump::JumpController>,
        ),
    >,
}

#[derive(Clone)]
pub struct Oni2EntityType {
    pub name: String,
    pub sub_meshes: Vec<(usize, Handle<Mesh>)>,
    pub materials: Vec<Vec<Handle<StandardMaterial>>>,
    pub material_animators: Vec<Vec<TextureUVAnimator>>,
    pub skeleton: Option<Oni2Skeleton>,
    pub inverse_bind_poses: Option<Handle<SkinnedMeshInverseBindposes>>,
    pub bounds: Oni2DebugBounds,
    pub bound_quads: Vec<[u32; 4]>,
    pub bound_tris: Vec<[u32; 3]>,
    pub anim_library: Option<Oni2AnimLibrary>,
    pub locomotion: Option<crate::oni2_loader::parsers::loco::LocomotionController>,
    pub debug_skeleton: Option<crate::oni2_loader::Oni2DebugSkeleton>,
    pub jump_controller: Option<crate::oni2_loader::parsers::jump::JumpController>,
}

#[derive(Resource, Default)]
pub struct ProjLibrary {
    pub projectiles: HashMap<String, ProjectileDef>,
}

#[derive(Resource, Default)]
pub struct FxLibrary {
    pub effects: HashMap<String, EffectDef>,
}

#[derive(Resource, Default)]
pub struct ParticleLibrary {
    pub systems: HashMap<String, ParticleSystemDef>,
}

#[derive(Resource, Default)]
pub struct ExplosionRegistry {
    pub explosions: HashMap<String, crate::oni2_loader::parsers::types::BasicExplosionDef>,
}

pub fn load_global_registries(
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut proj_lib: ResMut<ProjLibrary>,
    mut fx_lib: ResMut<FxLibrary>,
    mut ptx_lib: ResMut<ParticleLibrary>,
) {
    // 1. Load rb.proj
    if let Ok(content) = vfs::read_to_string("Settings", "rb.proj") {
        let blocks = parse_settings(&content);
        for def in &blocks {
            if let Some(parsed) =
                parse_projectile(&def.def_type, &def.name, &def.block, &asset_server)
            {
                proj_lib.projectiles.insert(def.name.to_lowercase(), parsed);
            }
        }
    } else {
        warn!("Could not find Settings/rb.proj in VFS.");
    }

    // 2. Load rb.fx
    if let Ok(content) = vfs::read_to_string("Settings", "rb.fx") {
        let blocks = parse_settings(&content);
        for def in &blocks {
            if let Some(parsed) = parse_effect(
                &def.def_type,
                &def.name,
                &def.block,
                &asset_server,
                &mut images,
            ) {
                fx_lib
                    .effects
                    .insert(def.name.to_lowercase(), parsed.clone());

                // Eagerly load .ptx files if this effect references one
                match &parsed {
                    EffectDef::Particle(p) => try_load_ptx(
                        &p.system.system_name,
                        &asset_server,
                        &mut ptx_lib,
                        &mut images,
                    ),
                    EffectDef::DelayedParticle(d) => try_load_ptx(
                        &d.system.system_name,
                        &asset_server,
                        &mut ptx_lib,
                        &mut images,
                    ),
                    EffectDef::HealthIndicator(h) => try_load_ptx(
                        &h.system.system_name,
                        &asset_server,
                        &mut ptx_lib,
                        &mut images,
                    ),
                    _ => {}
                }
            }
        }
    } else {
        warn!("Could not find Settings/rb.fx in VFS.");
    }
}

pub fn try_load_ptx(
    name: &str,
    asset_server: &AssetServer,
    ptx_lib: &mut ParticleLibrary,
    images: &mut Assets<Image>,
) {
    let lower_name = name.to_lowercase();
    if ptx_lib.systems.contains_key(&lower_name) {
        return; // Already loaded
    }

    let ptx_filename = format!("{}.ptx", name);
    if let Ok(content) = vfs::read_to_string("Settings", &ptx_filename) {
        if let Some(def) = parse_ptx(&content, name.to_string(), asset_server, images) {
            ptx_lib.systems.insert(lower_name, def);
            return;
        }
    }

    // Case-insensitive search inside Settings/ folder as fallback
    if let Ok(entries) = vfs::read_dir("Settings") {
        for entry in entries {
            if !entry.is_dir
                && entry
                    .path
                    .to_lowercase()
                    .ends_with(&format!("/{}.ptx", lower_name))
            {
                // vfs read_dir returns full paths, but read_to_string requires (dir, filename)
                // We'll extract the filename component safely.
                let fallback_filename = entry.path.split('/').last().unwrap_or("");
                if let Ok(content) = vfs::read_to_string("Settings", fallback_filename) {
                    if let Some(def) = parse_ptx(&content, name.to_string(), asset_server, images) {
                        ptx_lib.systems.insert(lower_name, def);
                        return;
                    }
                }
            }
        }
    }
    warn!(
        "Expected to find {}.ptx for particle system but it was not found in Settings/.",
        name
    );
}

pub fn load_global_explosions(mut cmd: Commands) {
    let mut reg = ExplosionRegistry::default();
    
    // We scan the exact path the user specified: `assets/Settings/rb.expl`
    // Note: vfs handles the actual path resolving.
    if let Ok(text) = vfs::read_to_string("Settings", "rb.expl") {
        let explosions = parse_expl(&text);
        for ex in explosions {
            reg.explosions.insert(ex.name.clone(), ex);
        }
    } else {
        warn!("Could not find Settings/rb.expl in VFS!");
        return; // Soft fail, missing file
    }
    
    info!("Loaded {} global basic explosions from Settings/rb.expl", reg.explosions.len());
    cmd.insert_resource(reg);
}
