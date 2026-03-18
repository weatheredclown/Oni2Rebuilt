use bevy::prelude::*;
use std::collections::HashMap;

use crate::oni2_loader::parsers::effect::{parse_effect, EffectDef};
use crate::oni2_loader::parsers::particle::{parse_ptx, ParticleSystemDef};
use crate::oni2_loader::parsers::projectile::{parse_projectile, ProjectileDef};
use crate::oni2_loader::parsers::settings::parse_settings;
use crate::oni2_loader::Oni2Skeleton;
use crate::oni2_loader::Oni2AnimLibrary;
use crate::oni2_loader::Oni2DebugBounds;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use crate::vfs;

#[derive(Resource, Default)]
pub struct EntityLibrary {
    pub entities: HashMap<String, Oni2EntityType>,
}

#[derive(Clone)]
pub struct Oni2EntityType {
    pub name: String,
    pub sub_meshes: Vec<(usize, Handle<Mesh>)>,
    pub materials: Vec<Handle<StandardMaterial>>,
    pub skeleton: Option<Oni2Skeleton>,
    pub inverse_bind_poses: Option<Handle<SkinnedMeshInverseBindposes>>,
    pub bounds: Oni2DebugBounds,
    pub bound_quads: Vec<[u32; 4]>,
    pub bound_tris: Vec<[u32; 3]>,
    pub anim_library: Option<Oni2AnimLibrary>,
    pub debug_skeleton: Option<crate::oni2_loader::Oni2DebugSkeleton>,
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

pub fn load_global_registries(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
) {
    let mut proj_lib = ProjLibrary::default();
    let mut fx_lib = FxLibrary::default();
    let mut ptx_lib = ParticleLibrary::default();

    // 1. Load rb.proj
    if let Ok(content) = vfs::read_to_string("Settings", "rb.proj") {
        let blocks = parse_settings(&content);
        for def in &blocks {
            if let Some(parsed) = parse_projectile(&def.def_type, &def.name, &def.block, &asset_server) {
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
            if let Some(parsed) = parse_effect(&def.def_type, &def.name, &def.block, &asset_server, &mut images) {
                fx_lib.effects.insert(def.name.to_lowercase(), parsed.clone());

                // Eagerly load .ptx files if this effect references one
                match &parsed {
                    EffectDef::Particle(p) => try_load_ptx(&p.system.system_name, &asset_server, &mut ptx_lib, &mut images),
                    EffectDef::DelayedParticle(d) => try_load_ptx(&d.system.system_name, &asset_server, &mut ptx_lib, &mut images),
                    EffectDef::HealthIndicator(h) => try_load_ptx(&h.system.system_name, &asset_server, &mut ptx_lib, &mut images),
                    _ => {}
                }
            }
        }
    } else {
        warn!("Could not find Settings/rb.fx in VFS.");
    }

    commands.insert_resource(proj_lib);
    commands.insert_resource(fx_lib);
    commands.insert_resource(ptx_lib);
    commands.insert_resource(EntityLibrary::default());
}

fn try_load_ptx(name: &str, asset_server: &AssetServer, ptx_lib: &mut ParticleLibrary, images: &mut Assets<Image>) {
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
            if !entry.is_dir && entry.path.to_lowercase().ends_with(&format!("/{}.ptx", lower_name)) {
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
    warn!("Expected to find {}.ptx for particle system but it was not found in Settings/.", name);
}
