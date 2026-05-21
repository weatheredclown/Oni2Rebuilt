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

use crate::inventory::components::{ItemRegistry, WeaponItemRegistry};
use crate::oni2_loader::Oni2AnimLibrary;
use crate::oni2_loader::Oni2DebugBounds;
use crate::oni2_loader::Oni2Skeleton;
use crate::oni2_loader::parsers::ammo::parse_ammo_content;
use crate::oni2_loader::parsers::effect::{EffectDef, parse_effect};
use crate::oni2_loader::parsers::expl::parse_expl;
use crate::oni2_loader::parsers::inventory::parse_inventory_content;
use crate::oni2_loader::parsers::particle::{ParticleSystemDef, parse_ptx};
use crate::oni2_loader::parsers::projectile::{ProjectileDef, parse_projectile};
use crate::oni2_loader::parsers::settings::parse_settings;
use crate::oni2_loader::parsers::weap::parse_weap_content;
use crate::vfs;
use crate::weapons::components::{AmmoRegistry, WeaponRegistry};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use std::sync::Arc;

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

/// One drawable: a mesh handle plus the UV-animation directives that
/// `uv_animator_system` ticks every frame.  This is the unified registry
/// the legacy AGE engine implements as `rmcDrawable` — the per-frame
/// scrolling/scaling rotation work doesn't care whether the drawable
/// came from an entity load (.mod via `.type`) or an FX load (.mesh via
/// `.type`).  `name` is a debug label only; `key` is what dedupes
/// repeated mirror passes from entities.
#[derive(Debug, Clone)]
pub struct DrawableEntry {
    /// Stable unique key.  Conventions: `entity:<dir>#<mat_idx>` for
    /// entity sub-meshes, `fx:<fx_name>#<pass_idx>` for FX drawables.
    pub key: String,
    /// Human-readable label (entity name / FX name / shader name).
    pub name: String,
    pub mesh: Handle<Mesh>,
    pub animators: Vec<TextureUVAnimator>,
}

/// Unified per-drawable registry.  FX loaders push directly; entity
/// loads are mirrored in by `sync_drawables_from_entities_system`.
/// `uv_animator_system` walks this list (no longer the per-entity
/// sub_meshes loop on `EntityLibrary`) so adding new non-entity
/// drawables doesn't require teaching every animator system about them.
#[derive(Resource, Default)]
pub struct DrawableLibrary {
    pub entries: Vec<DrawableEntry>,
}

impl DrawableLibrary {
    /// Insert a drawable if its `key` isn't already in the registry.
    /// Returns `true` if a new entry was added.
    pub fn register(
        &mut self,
        key: String,
        name: String,
        mesh: Handle<Mesh>,
        animators: Vec<TextureUVAnimator>,
    ) -> bool {
        if self.entries.iter().any(|e| e.key == key) {
            return false;
        }
        self.entries.push(DrawableEntry {
            key,
            name,
            mesh,
            animators,
        });
        true
    }
}

/// Mirrors entity sub-meshes into `DrawableLibrary` whenever
/// `EntityLibrary` changes.  Uses `Local<HashSet<String>>` to dedupe
/// across frames, so an entity loaded mid-game gets registered once
/// and never re-walked.  Runs cheap when no entity load happened this
/// tick (the `is_changed()` early-return).
pub fn sync_drawables_from_entities_system(
    entity_lib: Res<EntityLibrary>,
    mut drawables: ResMut<DrawableLibrary>,
    mut seen: bevy::ecs::system::Local<HashMap<String, ()>>,
) {
    if !entity_lib.is_changed() && !seen.is_empty() {
        return;
    }
    for (entity_dir, ent_type) in &entity_lib.entities {
        for (mat_idx, mesh) in &ent_type.sub_meshes {
            let key = format!("entity:{}#{}", entity_dir, mat_idx);
            if seen.contains_key(&key) {
                continue;
            }
            let animators = ent_type
                .material_animators
                .get(*mat_idx)
                .cloned()
                .unwrap_or_default();
            drawables.register(key.clone(), ent_type.name.clone(), mesh.clone(), animators);
            seen.insert(key, ());
        }
    }
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
    pub materials: Vec<Vec<crate::oni2_loader::animation::PassMaterial>>,
    pub material_animators: Vec<Vec<TextureUVAnimator>>,
    pub skeleton: Option<Oni2Skeleton>,
    pub inverse_bind_poses: Option<Handle<SkinnedMeshInverseBindposes>>,
    /// Debug wireframe used by F3 gizmo rendering (merged, entity-local space).
    pub bounds: Oni2DebugBounds,
    /// Per-sub-bound collision geometry ready for spawning.
    /// Each entry is (bone_index, sub_bound_in_bone_local_space).
    /// bone_index == 0 means the entity root (or no skeleton).
    pub bone_colliders: Vec<(usize, crate::oni2_loader::parsers::types::Oni2SubBound)>,
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
    mut weap_lib: ResMut<WeaponRegistry>,
    mut ammo_lib: ResMut<AmmoRegistry>,
    mut item_lib: ResMut<ItemRegistry>,
    mut weapon_item_lib: ResMut<WeaponItemRegistry>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut env_materials: ResMut<Assets<crate::env_reflect_material::EnvReflectMaterial>>,
    mut skinned_mesh_ibp: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut entity_lib: ResMut<EntityLibrary>,
    mut anim_registry: ResMut<AnimRegistry>,
) {
    // 1. Load rb.proj
    if let Ok(content) = vfs::read_to_string("Settings", "rb.proj") {
        let blocks = parse_settings(&content);
        for def in &blocks {
            if let Some(parsed) =
                parse_projectile(&def.def_type, &def.name, &def.block, &asset_server)
            {
                if let ProjectileDef::Model(ref m) = parsed {
                    let entity_dir = format!("Entity/{}", m.model_name);
                    if !entity_lib.entities.contains_key(&entity_dir) {
                        if let Some(e) = crate::oni2_loader::spawn::load_oni2_entity_type(
                            &mut meshes,
                            &mut materials,
                            &mut env_materials,
                            &mut images,
                            &mut skinned_mesh_ibp,
                            &mut anim_registry,
                            &entity_dir,
                            &m.model_name,
                            None,
                        ) {
                            entity_lib.entities.insert(entity_dir.clone(), e);
                            info!("Pre-loaded entity type for projectile: {}", m.model_name);
                        } else {
                            warn!(
                                "Failed to load entity type for projectile: {}",
                                m.model_name
                            );
                        }
                    }
                }
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

    // 3. Load rb.ammo (must precede rb.weap so weapons can resolve ammo refs).
    if let Ok(content) = vfs::read_to_string("Settings", "rb.ammo") {
        let ammos = parse_ammo_content(&content);
        for (name, ammo) in ammos {
            ammo_lib.types.insert(name, ammo);
        }
        info!(
            "Loaded {} ammo types from Settings/rb.ammo",
            ammo_lib.types.len()
        );
    } else {
        warn!("Could not find Settings/rb.ammo in VFS.");
    }

    // 4. Load rb.weap
    if let Ok(content) = vfs::read_to_string("Settings", "rb.weap") {
        let weapons = parse_weap_content(&content);
        for (name, weapon) in weapons {
            weap_lib.types.insert(name.to_lowercase(), weapon);
        }
        info!(
            "Loaded {} weapons from Settings/rb.weap",
            weap_lib.types.len()
        );
    } else {
        warn!("Could not find Settings/rb.weap in VFS.");
    }

    // 5. Load rb.inventory
    if let Ok(content) = vfs::read_to_string("Settings", "rb.inventory") {
        let (items, weapons) = parse_inventory_content(&content);
        for (name, item) in items {
            item_lib.types.insert(name, Arc::new(item));
        }
        for (name, weapon) in weapons {
            weapon_item_lib.types.insert(name, Arc::new(weapon));
        }
        info!(
            "Loaded {} items and {} weapon items from Settings/rb.inventory",
            item_lib.types.len(),
            weapon_item_lib.types.len()
        );
    } else {
        warn!("Could not find Settings/rb.inventory in VFS.");
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
    if let Ok(content) = vfs::read_to_string("Settings", &ptx_filename)
        && let Some(def) = parse_ptx(&content, name.to_string(), asset_server, images)
    {
        ptx_lib.systems.insert(lower_name, def);
        return;
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
                let fallback_filename = entry.path.split('/').next_back().unwrap_or("");
                if let Ok(content) = vfs::read_to_string("Settings", fallback_filename)
                    && let Some(def) = parse_ptx(&content, name.to_string(), asset_server, images)
                {
                    ptx_lib.systems.insert(lower_name, def);
                    return;
                }
            }
        }
    }
    warn!(
        "Expected to find {}.ptx for particle system but it was not found in Settings/.",
        name
    );
}

pub fn load_global_explosions(mut reg: ResMut<ExplosionRegistry>) {
    // We scan the exact path the user specified: `assets/Settings/rb.expl`
    // Note: vfs handles the actual path resolving.
    if let Ok(text) = vfs::read_to_string("Settings", "rb.expl") {
        let explosions = parse_expl(&text);
        for ex in explosions {
            reg.explosions.insert(ex.name.clone(), ex);
        }
    } else {
        warn!("Could not find Settings/rb.expl in VFS!");
    }

    info!(
        "Loaded {} global basic explosions from Settings/rb.expl",
        reg.explosions.len()
    );
}
