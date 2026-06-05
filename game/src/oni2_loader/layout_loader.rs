/*
 * oni2_loader/layout_loader.rs — ONI2 level layout loader.
 *
 * load_layout: reads layout/ directory (lights, geometry, entities, cameras,
 * paths, fog, scripts) and spawns the corresponding Bevy entities.  Returns
 * LayoutPlayerInfo if a Player="1" creature was found.
 * find_konoko_spawn: quick scan for the player spawn point without full load.
 * load_global_registries: Startup system that pre-loads entity, anim, fx, and
 * projectile registries from the VFS.
 */
use super::*;
use crate::oni2_loader::parsers::texture::decode_tex;
use crate::oni2_loader::parsers::texture::load_texture;
use crate::oni2_loader::utils::space;

// Light intensity / range scaling — used both at spawn time
// (`load_layout`) and by the runtime ScrOni `setLightIntensity` binding
// in `scroni::system_bindings`. Keeping the formula in one place means a
// .lights file Intensity of N produces the same brightness as the script
// later writing `intensity N` against the same light.
//
// The 3000× factor lifts the unitless legacy values (typical 30–300, see
// `lgtLight::ContributionTo`) into Bevy's
// candela range; tuned against Blast Chambers. Range follows from
// intensity because legacy doesn't author it directly: `1/r²` falloff
// means contribution drops to ~1% by `r ≈ 10·sqrt(intensity)`, and 3×
// intensity is the rough heuristic that keeps fill lights local while
// letting 300-intensity lamps reach across rooms. `MIN_RANGE` is the
// floor so an authored-zero / script-rampable light still has some
// reach the moment its intensity goes nonzero.
pub const POINT_INTENSITY_TO_CANDELA: f32 = 3000.0;
pub const SPOT_INTENSITY_TO_CANDELA: f32 = 3000.0;
pub const POINT_RANGE_FROM_INTENSITY: f32 = 3.0;
pub const POINT_MIN_RANGE: f32 = 25.0;

pub struct LayoutPlayerInfo {
    pub entity: Entity,
    pub position: Vec3,
    pub rotation: Quat,
    pub entity_type: String,
    pub animator_type: String,
    pub max_hitpoints: Option<f32>,
    pub faction: Option<String>,
    /// FSM name from the actor's `<Pad FSM="..."/>` attribute, with `.fsm`
    /// stripped.  None when the XML did not specify one — callers fall back
    /// to `"player"`.
    pub pad_fsm: Option<String>,
}

/// Who initiated a chunked layout load — determines cleanup scope and
/// post-finalize routing.  `InGame` is the default path: the loading
/// screen UI drives the load and on completion transitions to
/// `AppState::InGame` via `LoadedLayoutPlayer`.  `Frontend` is the
/// PAGE_3D backdrop path: the frontend `rebuild_current_page` kicks
/// the load while staying in `AppState::FrontEnd`, spawned entities
/// are additionally tagged `FrontendBackdrop` so `teardown_frontend`
/// sweeps them on page/state exit (and the per-page rebuild leaves
/// them alone), and the player-info transition is suppressed (the
/// backdrop isn't the player's level).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutLoadScope {
    #[default]
    InGame,
    Frontend,
}

/// Chunked-load state held across frames during a layout load.
/// Populated once by `begin_chunked_layout_load`, drained a handful of
/// actors at a time by `drive_chunked_actor_spawn`, and closed out by
/// `finalize_chunked_layout_load` (the post-actor phase: camera
/// packages, lights).  Exists as a Resource so caller systems can
/// watch `cursor` / `actor_names.len()` for progress.
///
/// The driver is no longer gated by `AppState::LoadingLayout` — it
/// runs whenever this resource exists, which lets the frontend drive
/// a layout load for the PAGE_3D backdrop without AppState churn.
#[derive(Resource)]
pub struct PendingLayoutLoad {
    pub actor_names: Vec<String>,
    pub cursor: usize,
    pub layout_dir: String,
    pub layout_ctx: LayoutContext,
    pub layout_paths: LayoutPaths,
    pub texture_collections: TextureCollections,
    pub player_info: Option<LayoutPlayerInfo>,
    pub spawned: u32,
    pub creatures: u32,
    pub skipped: u32,
    /// Set once `finalize_chunked_layout_load` has run so the driver
    /// system doesn't call it a second time while waiting for the
    /// state transition.
    pub post_done: bool,
    /// Which code path kicked the load — controls cleanup-marker
    /// tagging + the finalize-phase routing.
    pub scope: LayoutLoadScope,
    /// Entity-type prepass queue.  Walked before actor spawning starts,
    /// one per tick, so the heavy `load_oni2_entity_type` work (mesh
    /// build, material build, skeleton + anim library load) happens
    /// during the loading screen instead of on the first spawn that
    /// needs each type.  Each tuple is
    /// `(entity_dir, entity_type, animator_type_override)`; the loader
    /// is invoked with `name = animator.unwrap_or(entity_type)` so the
    /// cached `Oni2EntityType` matches what `spawn_oni2_creature` would
    /// have produced on first spawn.  Without this prepass, draining a
    /// chunk of 8 unseen actor types in one frame stacks 8 entity-type
    /// loads = the ~90 ms hitch we observed in trace
    /// `1777898793624427`.
    pub entity_prepass: Vec<(String, String, Option<String>)>,
    pub entity_prepass_cursor: usize,
}

/// Resource produced by the chunked loader once finalized, carrying
/// the player info out to `setup_scene` (OnEnter InGame) where the
/// player entity gets its combat / camera / FSM bundles.
#[derive(Resource)]
pub struct LoadedLayoutPlayer(pub LayoutPlayerInfo);

/// Pre-actor phase of a chunked load: parse `layout.et`, `layout.paths`,
/// `layout.graphs`, insert those as resources, then read
/// `layout.actors` into the returned state's `actor_names` queue.
/// Does NOT spawn any actors — those get drained one per tick by
/// `spawn_next_layout_actor`.
pub fn begin_chunked_layout_load(
    commands: &mut Commands,
    layout_dir: &str,
    entity_base_dir: &str,
) -> Option<PendingLayoutLoad> {
    crate::oni2_loader::parsers::actor_xml::clear_xml_cache();

    let layout_path = layout_dir;
    let entity_base = entity_base_dir;

    // Parse layout.et to find which types are BASICENTITY
    let mut basic_types = std::collections::HashSet::new();
    if let Ok(et_content) = crate::vfs::read_to_string(layout_path, "layout.et") {
        for line in et_content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("BASICENTITY") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    basic_types.insert(parts[1].to_string());
                }
            }
        }
    }
    info!("Layout: {} basic entity types", basic_types.len());

    let layout_paths = LayoutPaths {
        curves: parsers::layout::parse_layout_paths(layout_path),
    };
    if !layout_paths.curves.is_empty() {
        info!("Layout: loaded {} path curves", layout_paths.curves.len());
    }

    let nav_graphs = crate::oni2_loader::parsers::graph::parse_layout_graphs(layout_path);
    let nav_graph = crate::ai::navigation::NavGraph::new(nav_graphs);
    info!(
        "Layout: generated NavGraph with {} points",
        nav_graph.points.len()
    );
    let cover_mgr = crate::ai::cover::build_cover_points(&nav_graph);
    info!(
        "Layout: cover-point manager seeded with {} POINT_COVER nodes",
        cover_mgr.points.len()
    );
    commands.insert_resource(nav_graph);
    commands.insert_resource(cover_mgr);
    commands.insert_resource(layout_paths.clone());

    let actors_content = match crate::vfs::read_to_string(layout_path, "layout.actors") {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to read layout.actors: {}", e);
            return None;
        }
    };

    let layout_ctx = LayoutContext {
        layout_dir: layout_path.to_string(),
        entity_base: entity_base.to_string(),
        basic_types,
    };
    commands.insert_resource(layout_ctx.clone());

    // Extract actor names (skip the count line and blanks).
    let actor_names: Vec<String> = actors_content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.parse::<u32>().is_ok() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect();

    // Walk every actor XML once now to discover which entity types the
    // layout will need.  We dedupe by entity_dir so each unique type is
    // loaded exactly once during the prepass.  `parse_actor_xml` caches
    // its output, so the chunked spawner re-parsing these XMLs later
    // is a cache-hit, not extra IO.
    //
    // We capture the FIRST animator_type override we see for a given
    // entity_dir.  In ONI2 layouts each entity_type is normally bound to
    // one animator, so this matches what `spawn_oni2_creature` would
    // produce on first spawn (cache-key is `entity_dir` either way).
    let template_dir = "template".to_string();
    let mut seen_dirs = std::collections::HashSet::<String>::new();
    let mut entity_prepass: Vec<(String, String, Option<String>)> = Vec::new();
    for name in &actor_names {
        let Some(actor) = crate::oni2_loader::parsers::actor_xml::parse_actor_xml(
            layout_path,
            &format!("{}.xml", name),
            &template_dir,
        ) else {
            continue;
        };
        let entity_dir = format!("{}/{}", entity_base, actor.entity_type);
        if seen_dirs.insert(entity_dir.clone()) {
            entity_prepass.push((
                entity_dir,
                actor.entity_type.clone(),
                actor.animator_type.clone(),
            ));
        }
    }
    info!(
        "Layout: {} actor names, {} unique entity types to preload",
        actor_names.len(),
        entity_prepass.len()
    );

    Some(PendingLayoutLoad {
        actor_names,
        cursor: 0,
        layout_dir: layout_path.to_string(),
        layout_ctx,
        layout_paths,
        texture_collections: TextureCollections::default(),
        player_info: None,
        spawned: 0,
        creatures: 0,
        skipped: 0,
        post_done: false,
        scope: LayoutLoadScope::InGame,
        entity_prepass,
        entity_prepass_cursor: 0,
    })
}

/// Frontend-scope variant of `begin_chunked_layout_load`.  Identical
/// parse behavior; the only difference is the scope tag that drives
/// cleanup marker insertion + suppresses the LoadedLayoutPlayer →
/// InGame transition.  Used by `rebuild_current_page` when a PAGE_3D
/// with a `LAYOUT` directive is entered.
pub fn begin_frontend_layout_load(
    commands: &mut Commands,
    layout_dir: &str,
    entity_base_dir: &str,
) -> Option<PendingLayoutLoad> {
    let mut pending = begin_chunked_layout_load(commands, layout_dir, entity_base_dir)?;
    pending.scope = LayoutLoadScope::Frontend;
    Some(pending)
}

/// Resource marker: `finalize_chunked_layout_load` has completed for
/// a frontend-scope load.  The frontend input dispatcher watches for
/// this to un-gate navigation events after the backdrop is ready.
/// Carries the layout name so the frontend can tell which backdrop
/// is currently mounted (used to skip re-loading when the user
/// navigates away and back to the same PAGE_3D page).
#[derive(Resource, Clone, Debug)]
pub struct FrontendLayoutReady {
    pub layout_name: String,
}

/// Tagging system: while a frontend-scope chunked load is in flight,
/// every freshly-spawned `InGameEntity` picks up `FrontendBackdrop`
/// so the frontend teardown sweep catches it on page/state exit.
/// Also catches the camera/light entities the finalize phase spawns.
///
/// `FrontendBackdrop` (not `FrontendUiEntity`) is used deliberately —
/// `rebuild_current_page` despawns every `FrontendUiEntity` on each
/// rebuild, but the backdrop scene must persist across rebuilds (a
/// SET_VISIBLE handler triggered by arrow nav would otherwise wipe
/// the uitest scene). `teardown_frontend` sweeps both markers.
///
/// Runs every frame — cheap because the `Without<FrontendBackdrop>`
/// filter + `Without<ChildOf>` on the outer query narrows to new,
/// top-level layout spawns.  Nested children get swept by Bevy's
/// recursive despawn when the root goes.
pub fn tag_frontend_layout_spawns_system(
    mut commands: Commands,
    pending: Option<Res<PendingLayoutLoad>>,
    ready: Option<Res<FrontendLayoutReady>>,
    untagged: Query<
        Entity,
        (
            With<crate::menu::InGameEntity>,
            Without<crate::frontend::render::FrontendBackdrop>,
        ),
    >,
) {
    // Only tag while a frontend-scope load is in flight OR has just
    // finished (covers the frame where finalize spawned lights /
    // camera packages after the pending resource was dropped).
    let frontend_active = matches!(
        pending.as_deref(),
        Some(p) if p.scope == LayoutLoadScope::Frontend,
    );
    let frontend_ready = ready.is_some();
    if !frontend_active && !frontend_ready {
        return;
    }
    for entity in &untagged {
        commands
            .entity(entity)
            .insert(crate::frontend::render::FrontendBackdrop);
    }
}

/// Result record from one chunked spawn call — used to fold
/// bookkeeping into `PendingLayoutLoad` from the caller side (rather
/// than passing `&mut state` alongside SpawnAssets, which would
/// double-borrow `state.texture_collections`).
pub struct ChunkedSpawnResult {
    /// `Some` if an actor reference was consumed from the queue this
    /// call; `None` when the queue was already empty.
    pub spawned_entity: Option<Entity>,
    /// `Some` if the spawned actor was flagged Player="1" and no
    /// player info had been recorded yet.  Caller should store this
    /// in `state.player_info`.
    pub player_info: Option<LayoutPlayerInfo>,
    pub is_creature: bool,
    pub is_basic_entity: bool,
    pub failed: bool,
}

/// Spawn the actor at `index` in the queue.  Caller handles cursor
/// advancement and the counter/field updates on `PendingLayoutLoad`
/// — we can't take `&mut state` here because SpawnAssets already
/// borrows `state.texture_collections`.
pub fn spawn_queued_layout_actor(
    actor_name: &str,
    layout_ctx: &LayoutContext,
    layout_paths: &LayoutPaths,
    assets: &mut SpawnAssets,
) -> ChunkedSpawnResult {
    match spawn_layout_actor(
        assets,
        actor_name,
        layout_ctx,
        layout_paths,
        None,
        false,
        None,
    ) {
        Some((entity, actor)) => {
            let player_info = if actor.is_creature && actor.is_player {
                Some(LayoutPlayerInfo {
                    entity,
                    position: actor.position,
                    rotation: space::to_bevy_space_rot(actor.orientation_o2),
                    entity_type: actor.entity_type.clone(),
                    animator_type: actor.animator_type.clone().unwrap_or_default(),
                    max_hitpoints: actor.max_hitpoints,
                    faction: actor.faction.clone(),
                    pad_fsm: actor.pad_fsm.clone(),
                })
            } else {
                None
            };
            ChunkedSpawnResult {
                spawned_entity: Some(entity),
                player_info,
                is_creature: actor.is_creature,
                is_basic_entity: !actor.is_creature,
                failed: false,
            }
        }
        None => ChunkedSpawnResult {
            spawned_entity: None,
            player_info: None,
            is_creature: false,
            is_basic_entity: false,
            failed: true,
        },
    }
}

/// Standalone chunked actor-spawn driver.  Runs whenever
/// `PendingLayoutLoad` exists, drains up to `CHUNK_SIZE` actors per
/// tick, and calls `finalize_chunked_layout_load` once the queue
/// empties.  Replaces the former `drive_chunked_actor_spawn` in
/// `menu.rs` — this version is AppState-agnostic so the frontend
/// (PAGE_3D backdrop load) and the normal loading screen can both
/// drive the same machinery.
///
/// For the InGame scope, we move the finalized player info into a
/// `LoadedLayoutPlayer` resource so `setup_scene` consumes it
/// OnEnter(InGame).  For the Frontend scope, there's no player — the
/// backdrop load drops the info on the floor and instead inserts
/// `FrontendLayoutReady` so the frontend input dispatcher knows the
/// backdrop is mounted.
pub fn drive_chunked_actor_spawn_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut env_materials: ResMut<Assets<crate::env_reflect_material::EnvReflectMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut skinned_mesh_ibp: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut entity_lib: ResMut<crate::oni2_loader::registries::EntityLibrary>,
    mut anim_registry: ResMut<crate::oni2_loader::registries::AnimRegistry>,
    mut fight_fsm_cache: ResMut<crate::fightai::FightFsmCache>,
    mut attack_fsm_cache: ResMut<crate::fightai::AttackFsmCache>,
    mut pending: Option<ResMut<PendingLayoutLoad>>,
    mover_backend: Res<crate::mover::MoverBackend>,
    shared_mover_config: Option<Res<crate::mover::SharedMoverConfig>>,
) {
    const CHUNK_SIZE: usize = 8;
    // Entity-type prepass cap: how many `load_oni2_entity_type` calls we do
    // per tick before moving on to actor spawning.  Each call can take
    // 5-30 ms for a complex creature (skeleton + animations + materials),
    // so 1/tick keeps the loading-screen frames bounded by the single
    // heaviest type instead of stacking N of them in one frame.
    const PREPASS_CHUNK_SIZE: usize = 1;
    let Some(ref mut state) = pending else {
        return;
    };

    // Phase 1: prepay entity-type loads.  Until this drains, we don't
    // spawn any actors — every actor spawn will be a cache-hit on the
    // EntityLibrary so the per-actor cost drops to instantiation only.
    for _ in 0..PREPASS_CHUNK_SIZE {
        if state.entity_prepass_cursor >= state.entity_prepass.len() {
            break;
        }
        let idx = state.entity_prepass_cursor;
        state.entity_prepass_cursor += 1;
        let (entity_dir, entity_type, animator) = state.entity_prepass[idx].clone();
        if entity_lib.entities.contains_key(&entity_dir) {
            continue;
        }
        let anim_name = animator.as_deref().unwrap_or(entity_type.as_str());
        if let Some(loaded) = crate::oni2_loader::spawn::load_oni2_entity_type(
            &mut meshes,
            &mut materials,
            &mut env_materials,
            &mut images,
            &mut skinned_mesh_ibp,
            &mut anim_registry,
            &entity_dir,
            anim_name,
            Some(anim_name),
        ) {
            entity_lib.entities.insert(entity_dir, loaded);
        }
    }
    if state.entity_prepass_cursor < state.entity_prepass.len() {
        // Hold actor spawning until the prepass finishes — cheaper to
        // spend a few extra loading-screen frames here than to pay the
        // load cost during gameplay.
        return;
    }

    // Phase 2: drain actor names.  Every actor spawn from this point on
    // hits the EntityLibrary cache, so it's pure instantiation.
    for _ in 0..CHUNK_SIZE {
        if state.cursor >= state.actor_names.len() {
            break;
        }
        let actor_name = state.actor_names[state.cursor].clone();
        state.cursor += 1;
        let layout_ctx = state.layout_ctx.clone();
        let layout_paths = state.layout_paths.clone();
        let result = {
            let mut assets = crate::oni2_loader::environment::SpawnAssets {
                commands: &mut commands,
                meshes: &mut meshes,
                materials: &mut materials,
                env_materials: &mut env_materials,
                images: &mut images,
                skinned_mesh_ibp: &mut skinned_mesh_ibp,
                entity_lib: &mut entity_lib,
                anim_registry: &mut anim_registry,
                fight_fsm_cache: &mut fight_fsm_cache,
                attack_fsm_cache: &mut attack_fsm_cache,
                texture_collections: &mut state.texture_collections,
                mover_backend: *mover_backend,
                mover_config: shared_mover_config.as_ref().map(|c| c.0.clone()),
            };
            spawn_queued_layout_actor(&actor_name, &layout_ctx, &layout_paths, &mut assets)
        };
        if result.is_creature {
            state.creatures += 1;
            if state.player_info.is_none() && result.player_info.is_some() {
                state.player_info = result.player_info;
            }
        } else if result.is_basic_entity {
            state.spawned += 1;
        }
        if result.failed {
            state.skipped += 1;
        }
    }

    if state.cursor >= state.actor_names.len() && !state.post_done {
        finalize_chunked_layout_load(
            state,
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
        );
        match state.scope {
            LayoutLoadScope::InGame => {
                if let Some(player_info) = state.player_info.take() {
                    commands.insert_resource(LoadedLayoutPlayer(player_info));
                }
            }
            LayoutLoadScope::Frontend => {
                // Signal the frontend dispatcher that the backdrop is
                // mounted — it watches for this to un-gate input.  The
                // player info (if any) gets dropped: a backdrop isn't
                // a playable level.  Drop the pending resource too so
                // the tagging system can stop watching for new spawns;
                // `FrontendLayoutReady` carries the information that
                // "a backdrop exists" for teardown logic and the skip-
                // reload check in `rebuild_current_page`.
                commands.insert_resource(FrontendLayoutReady {
                    layout_name: state.layout_dir.clone(),
                });
                commands.remove_resource::<PendingLayoutLoad>();
            }
        }
    }
}

/// Post-actor phase: insert TextureCollections / camera packages /
/// camera parameter sets, and load lights.  Mirrors the tail end of
/// the legacy monolithic `load_layout`.  Consumes the `TextureCollections`
/// from the state.
pub fn finalize_chunked_layout_load(
    state: &mut PendingLayoutLoad,
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
) {
    if state.post_done {
        return;
    }
    info!(
        "Layout: spawned {} entities, {} creatures, skipped {}",
        state.spawned, state.creatures, state.skipped
    );
    if let Some(ref pi) = state.player_info {
        info!(
            "Layout: player creature found: type={} animator={}",
            pi.entity_type, pi.animator_type
        );
    }

    // Insert LayoutPaths again for legacy symmetry (the monolithic path
    // re-inserted them after the actor loop — harmless double-insert).
    if !state.layout_paths.curves.is_empty() {
        commands.insert_resource(state.layout_paths.clone());
    }
    // Transfer texture collections out of the pending state.
    commands.insert_resource(std::mem::take(&mut state.texture_collections));

    // Camera packages + parameters.
    let camera_packages = CameraPackages {
        packages: crate::oni2_loader::parsers::camera::parse_campacknew(&state.layout_dir),
    };
    let mut camera_sets = CameraParameterSets::default();
    let mut files_to_load = std::collections::HashSet::new();
    for pkg in camera_packages.packages.values() {
        if !pkg.navigation.is_empty() {
            files_to_load.insert(pkg.navigation.clone());
        }
        if !pkg.targeting.is_empty() {
            files_to_load.insert(pkg.targeting.clone());
        }
        if !pkg.fighting.is_empty() {
            files_to_load.insert(pkg.fighting.clone());
        }
    }
    for file_base in files_to_load {
        let xml_name = format!("{}.xml", file_base);
        if let Some(params) =
            crate::oni2_loader::parsers::camera::parse_camera_xml(&state.layout_dir, &xml_name)
        {
            camera_sets.sets.insert(file_base, params);
        } else {
            warn!("Failed to load camera xml: {}", xml_name);
        }
    }
    info!(
        "Layout: loaded {} camera packages, {} parameter sets",
        camera_packages.packages.len(),
        camera_sets.sets.len()
    );
    commands.insert_resource(camera_packages);
    commands.insert_resource(camera_sets);
    commands.insert_resource(ActiveCameraPackage::default());

    // Lights, fog, skyhat.
    load_layout_lights(commands, meshes, materials, images, &state.layout_dir);

    state.post_done = true;
}

/// Load an ONI2 layout directory, spawning all entities and creatures.
/// Returns info about the player creature if one was found (Player="1").
pub fn load_layout(
    commands: &mut Commands,
    _asset_server: &AssetServer,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    env_materials: &mut ResMut<Assets<crate::env_reflect_material::EnvReflectMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    skinned_mesh_ibp: &mut ResMut<Assets<SkinnedMeshInverseBindposes>>,
    entity_lib: &mut ResMut<crate::oni2_loader::registries::EntityLibrary>,
    anim_registry: &mut ResMut<crate::oni2_loader::registries::AnimRegistry>,
    fight_fsm_cache: &mut ResMut<crate::fightai::FightFsmCache>,
    attack_fsm_cache: &mut ResMut<crate::fightai::AttackFsmCache>,
    layout_dir: &str,
    entity_base_dir: &str,
) -> Option<LayoutPlayerInfo> {
    crate::oni2_loader::parsers::actor_xml::clear_xml_cache();

    let layout_path = layout_dir;
    let entity_base = entity_base_dir;

    // Parse layout.et to find which types are BASICENTITY
    let mut basic_types = std::collections::HashSet::new();
    if let Ok(et_content) = crate::vfs::read_to_string(layout_path, "layout.et") {
        for line in et_content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("BASICENTITY") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    basic_types.insert(parts[1].to_string());
                }
            }
        }
    }
    info!("Layout: {} basic entity types", basic_types.len());

    // Parse layout.paths early so we can look up curves during entity spawning
    let layout_paths = LayoutPaths {
        curves: parsers::layout::parse_layout_paths(layout_path),
    };
    if !layout_paths.curves.is_empty() {
        info!("Layout: loaded {} path curves", layout_paths.curves.len());
    }

    // Parse layout.graphs to construct the NavGraph
    let nav_graphs = crate::oni2_loader::parsers::graph::parse_layout_graphs(layout_path);
    let nav_graph = crate::ai::navigation::NavGraph::new(nav_graphs);
    info!(
        "Layout: generated NavGraph with {} points",
        nav_graph.points.len()
    );
    let cover_mgr = crate::ai::cover::build_cover_points(&nav_graph);
    info!(
        "Layout: cover-point manager seeded with {} POINT_COVER nodes",
        cover_mgr.points.len()
    );
    commands.insert_resource(nav_graph);
    commands.insert_resource(cover_mgr);

    // Insert LayoutPaths globally for dynamic spawned actors
    commands.insert_resource(layout_paths.clone());

    // Parse layout.actors to get actor list
    let actors_content = match crate::vfs::read_to_string(layout_path, "layout.actors") {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to read layout.actors: {}", e);
            return None;
        }
    };

    // Template directory for resolving base= references
    let mut parts: Vec<&str> = layout_path.split('/').collect();
    parts.pop();
    parts.pop();
    let assets_base = if parts.is_empty() {
        String::new()
    } else {
        parts.join("/")
    };
    let _template_dir = format!("{}/template", assets_base);

    let mut texture_collections = TextureCollections::default();

    // Insert LayoutContext for dynamic spawning by scripts
    let layout_ctx = LayoutContext {
        layout_dir: layout_path.to_string(),
        entity_base: entity_base.to_string(),
        basic_types,
    };
    commands.insert_resource(layout_ctx.clone());

    let mut spawned = 0;
    let mut creatures = 0;
    let mut skipped = 0;
    let mut player_info: Option<LayoutPlayerInfo> = None;
    for line in actors_content.lines() {
        let actor_name = line.trim();
        if actor_name.is_empty() || actor_name.parse::<u32>().is_ok() {
            continue; // skip count line and blank lines
        }

        let mut spawn_assets = SpawnAssets {
            commands,
            meshes,
            materials,
            env_materials,
            images,
            skinned_mesh_ibp: &mut *skinned_mesh_ibp,
            entity_lib: &mut *entity_lib,
            anim_registry: &mut *anim_registry,
            fight_fsm_cache: &mut *fight_fsm_cache,
            attack_fsm_cache: &mut *attack_fsm_cache,
            texture_collections: &mut texture_collections,
            // Legacy monolithic loader path (unused — chunked loader replaces
            // it). Mover backend toggle is wired through the chunked driver
            // only; if this path is revived, plumb a Res<MoverBackend> here.
            mover_backend: crate::mover::MoverBackend::Dynamic,
            mover_config: None,
        };

        if let Some((entity, actor)) = spawn_layout_actor(
            &mut spawn_assets,
            actor_name,
            &layout_ctx,
            &layout_paths,
            None,
            false,
            None,
        ) {
            if actor.is_creature {
                creatures += 1;
                if actor.is_player && player_info.is_none() {
                    player_info = Some(LayoutPlayerInfo {
                        entity,
                        position: actor.position,
                        rotation: space::to_bevy_space_rot(actor.orientation_o2),
                        entity_type: actor.entity_type.clone(),
                        animator_type: actor.animator_type.clone().unwrap_or_default(),
                        max_hitpoints: actor.max_hitpoints,
                        faction: actor.faction.clone(),
                        pad_fsm: actor.pad_fsm.clone(),
                    });
                }
            } else {
                spawned += 1;
            }
        } else {
            // Not spawned because it failed to parse or wasn't a basic type
            skipped += 1;
        }
    }
    info!(
        "Layout: spawned {} entities, {} creatures, skipped {}",
        spawned, creatures, skipped
    );
    if let Some(ref pi) = player_info {
        info!(
            "Layout: player creature found: type={} animator={}",
            pi.entity_type, pi.animator_type
        );
    }

    // Insert LayoutPaths resource for potential future use
    if !layout_paths.curves.is_empty() {
        commands.insert_resource(layout_paths);
    }

    // Insert TextureCollections resource for the texture_movie_system observer
    commands.insert_resource(texture_collections);

    // Load camera packages and parameters
    let camera_packages = CameraPackages {
        packages: crate::oni2_loader::parsers::camera::parse_campacknew(layout_dir),
    };
    let mut camera_sets = CameraParameterSets::default();

    // We only need to load the xml files referenced in the packages
    let mut files_to_load = std::collections::HashSet::new();
    for pkg in camera_packages.packages.values() {
        if !pkg.navigation.is_empty() {
            files_to_load.insert(pkg.navigation.clone());
        }
        if !pkg.targeting.is_empty() {
            files_to_load.insert(pkg.targeting.clone());
        }
        if !pkg.fighting.is_empty() {
            files_to_load.insert(pkg.fighting.clone());
        }
    }

    for file_base in files_to_load {
        let xml_name = format!("{}.xml", file_base);
        if let Some(params) =
            crate::oni2_loader::parsers::camera::parse_camera_xml(layout_dir, &xml_name)
        {
            camera_sets.sets.insert(file_base, params);
        } else {
            warn!("Failed to load camera xml: {}", xml_name);
        }
    }

    info!(
        "Layout: loaded {} camera packages, {} parameter sets",
        camera_packages.packages.len(),
        camera_sets.sets.len()
    );

    commands.insert_resource(camera_packages);
    commands.insert_resource(camera_sets);
    commands.insert_resource(ActiveCameraPackage::default());

    // Load lights, fog, skyhat
    load_layout_lights(commands, meshes, materials, images, layout_dir);

    player_info
}

/// Spawns a single actor by name, parsing its XML internally. Can override position.
pub fn spawn_layout_actor(
    assets: &mut SpawnAssets,
    xml_name: &str,
    layout_ctx: &LayoutContext,
    layout_paths: &LayoutPaths,
    pos_override: Option<Vec3>,
    force_spawn: bool,
    entity_name_override: Option<&str>,
) -> Option<(Entity, LayoutActor)> {
    let actor_name = entity_name_override.unwrap_or(xml_name);
    // Find template dir
    let template_dir = "template".to_string();

    // Parse the actor XML
    let actor = crate::oni2_loader::parsers::actor_xml::parse_actor_xml(
        &layout_ctx.layout_dir,
        &format!("{}.xml", xml_name),
        &template_dir,
    )?;

    if actor.spawn_later && !force_spawn {
        return None; // this should be loaded (with all sub-assets loaded) then spawning is just making it appear
    }

    // Find the entity directory
    let entity_dir = format!("{}/{}", layout_ctx.entity_base, actor.entity_type);

    let mut is_basic = layout_ctx.basic_types.contains(&actor.entity_type)
        || layout_ctx
            .basic_types
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&actor.entity_type));

    let is_trigger = actor.broadcast_radius.is_some()
        || actor.checkpoint_radius.is_some()
        || actor.fvt_radius.is_some();
    if !is_basic && !is_trigger && !actor.is_creature {
        is_basic = true;
    }

    let has_geometry = actor.is_creature || is_basic;

    // Try parsing .sha to find .tc (Texture Collection) and preload textures
    if has_geometry
        && !assets
            .texture_collections
            .collections
            .contains_key(&actor.entity_type)
    {
        let sha_filenames = vec![
            format!("{}.sha", actor.entity_type),
            format!("{}_LODs0.sha", actor.entity_type),
            format!("{}_lods0.sha", actor.entity_type),
        ];

        let mut frames = Vec::new();

        let mut sha_content = None;
        let mut sha_exists = false;

        for fname in &sha_filenames {
            if crate::vfs::exists(&entity_dir, fname) {
                sha_exists = true;
                if let Ok(content) = crate::vfs::read_to_string(&entity_dir, fname) {
                    sha_content = Some(content);
                    break;
                }
            }
        }

        if sha_exists && sha_content.is_none() {
            warn!(
                "Failed to read sha file for: {} (file exists but could not be read)",
                actor.entity_type
            );
        }

        if let Some(sha_content) = sha_content {
            let mut tc_name = None;
            for line in sha_content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("texcluster ") {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() >= 2 {
                        tc_name = Some(parts[1].to_string());
                        break;
                    }
                }
            }

            if let Some(mut tc) = tc_name {
                if !tc.to_lowercase().ends_with(".tc") {
                    tc.push_str(".tc");
                }
                if let Ok(tc_content) = crate::vfs::read_to_string(&entity_dir, &tc) {
                    for line in tc_content.lines() {
                        let trimmed = line.trim();
                        if trimmed.is_empty()
                            || trimmed.starts_with("version:")
                            || trimmed.starts_with("texCount:")
                        {
                            continue;
                        }

                        // Load the texture handle using the asset server
                        let tex_name = match trimmed.strip_suffix(".tex") {
                            Some(stripped) => stripped.to_string(),
                            None => trimmed.to_string(),
                        };

                        if let Some((tex_handle, _)) =
                            load_texture(&entity_dir, &tex_name, assets.images)
                        {
                            frames.push(tex_handle);
                        }
                    }
                    info!(
                        "Loaded Texture Collection for {}: {} frames",
                        actor.entity_type,
                        frames.len()
                    );
                }
            }
            assets
                .texture_collections
                .collections
                .insert(actor.entity_type.clone(), frames);
        }
    }

    let spawned_entity = if actor.is_creature {
        // Position already in Bevy coordinates (Z negated at parse time)
        let position = pos_override.unwrap_or(actor.position);
        let rotation = space::to_bevy_space_rot(actor.orientation_o2);

        if let Some(ref anim_type) = actor.animator_type {
            info!(
                "Creature {} type={} animator={} player={}",
                actor_name, actor.entity_type, anim_type, actor.is_player
            );
        }

        if let Some(entity) = spawn_oni2_creature(
            assets.commands,
            assets.meshes,
            assets.materials,
            assets.env_materials,
            assets.images,
            assets.skinned_mesh_ibp,
            assets.entity_lib,
            assets.anim_registry,
            &entity_dir,
            position,
            rotation,
            actor_name,
            &actor.entity_type,
            actor.animator_type.as_deref(),
            &layout_ctx.entity_base,
            assets.mover_backend,
            assets.mover_config.clone(),
        ) {
            if !actor.is_player {
                // Non-player creature: attach AI + combat components
                if actor.has_fight_ai {
                    if let Some(fsm_data) =
                        assets.fight_fsm_cache.get_or_load(&layout_ctx.entity_base)
                    {
                        let mut fsm = crate::statemachine::core::SmRuntime::new(fsm_data, 0);
                        fsm.entity = Some(entity);
                        assets.commands.entity(entity).insert(
                            crate::fightai::components::FightRuntime {
                                fsm,
                                ctx: crate::statemachine::drivers::fight::FightCtx::default(),
                                last_state_idx: usize::MAX,
                                last_mode: String::new(),
                            },
                        );
                    }

                    if let Some(table) = &actor.attack_table
                        && let Some(atk_data) = assets
                            .attack_fsm_cache
                            .get_or_load(table, &layout_ctx.entity_base)
                    {
                        let mut fsm = crate::statemachine::core::SmRuntime::new(atk_data, 0);
                        fsm.entity = Some(entity);
                        assets.commands.entity(entity).insert(
                            crate::fightai::components::AttackRuntime {
                                fsm,
                                ctx: crate::statemachine::drivers::attack::AttackCtx::default(),
                                last_state_idx: usize::MAX,
                                last_cookie: false,
                                tick_count: 0,
                            },
                        );
                    }

                    assets
                        .commands
                        .entity(entity)
                        .insert(crate::ai::components::AiFighter::default());
                    // Position/cookie coordinator state.  Both
                    // components live on every fighter — defender
                    // resources (slots/cookie) AND attacker state
                    // (held slot, offered queue, cookie held).
                    // Mirrors `aiFighter::Resources` +
                    // `CurrentPosition`/`ResourceTarget` fields in
                    // legacy `aiFighter`.
                    assets.commands.entity(entity).insert((
                        crate::fightai::position::FightResources::default(),
                        crate::fightai::position::FightSlotState::default(),
                    ));
                }
                assets.commands.entity(entity).insert((
                    crate::combat::components::Enemy,
                    crate::combat::faction::Faction(actor.faction.clone().unwrap_or_default()),
                    crate::combat::components::Fighter {
                        facing: rotation * Vec3::Z,
                        ..Default::default()
                    },
                    crate::combat::components::FighterId(uuid::Uuid::new_v4()),
                    crate::combat::components::Health::new(actor.max_hitpoints.unwrap_or(100.0)),
                ));
                if let Some(destroy_time) = actor.destroy_time {
                    assets
                        .commands
                        .entity(entity)
                        .insert(crate::combat::components::DestroyOnDeath(destroy_time));
                }
                // Full combat + fight loadout for AI creatures — mirrors
                // what setup_scene puts on the player so the fight
                // pipeline (react_data_apply, block_success/failed,
                // grapple, super meter, successive attacks, hit eta,
                // fight stance timer) engages for AI too.
                assets
                    .commands
                    .entity(entity)
                    .insert(crate::combat::FighterBundle {
                        fighter_type: crate::fight::components::FighterType {
                            name: actor
                                .fighter_type
                                .clone()
                                .or_else(|| actor.animator_type.clone())
                                .unwrap_or_else(|| actor.entity_type.clone()),
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                assets
                    .commands
                    .entity(entity)
                    .insert(crate::camera::components::PrototypeElement);
            }
            // Attach FXType component if present
            if actor.fx_type.is_some() || actor.ptx_name.is_some() {
                assets.commands.entity(entity).insert(
                    crate::oni2_loader::components::ActorFxType {
                        fx_name: actor.fx_type.clone(),
                        start_active: actor.fx_start_active,
                        ptx_name: actor.ptx_name.clone(),
                        ptx_birth_rate: actor.ptx_birth_rate,
                        ptx_num_particles: actor.ptx_num_particles,
                        ptx_offset: actor.ptx_offset,
                    },
                );
                info!("Attached FX component to creature {}", actor.entity_type);
            }
            if let Some(ref w) = actor.weapon_string {
                assets.commands.entity(entity).insert(
                    crate::inventory::components::PendingInventory {
                        weapon_string: w.clone(),
                        can_be_picked_up: actor.inventory_can_be_picked_up,
                        pickup_range: actor.inventory_pickup_range,
                        drop_items_on_death: actor.inventory_drop_items_on_death,
                        drop_range: actor.inventory_drop_range,
                    },
                );
                info!(
                    "Attached PendingInventory ({}) to creature {}",
                    w, actor.entity_type
                );
            }
            if let Some(sound_data) = actor.sound.as_ref() {
                assets
                    .commands
                    .entity(entity)
                    .insert(crate::actor_sound::ActorSound::new(sound_data.clone()));
            }

            Some(entity)
        } else {
            None
        }
    } else {
        // Static entity (BASICENTITY check) or trigger (has broadcast_radius)
        let position = pos_override.unwrap_or(actor.position);
        let rotation = space::to_bevy_space_rot(actor.orientation_o2);

        if is_basic {
            spawn_oni2_entity_with_rotation(
                assets.commands,
                assets.meshes,
                assets.materials,
                assets.env_materials,
                assets.images,
                assets.skinned_mesh_ibp,
                assets.entity_lib,
                assets.anim_registry,
                &entity_dir,
                position,
                rotation,
                actor_name,
                None,
                Some(&actor.entity_type),
            )
        } else {
            // It's just a trigger without a visual model, so spawn an empty entity
            Some(
                assets
                    .commands
                    .spawn((
                        Transform::from_translation(position).with_rotation(rotation),
                        GlobalTransform::default(),
                        Name::new(actor_name.to_string()),
                        crate::menu::InGameEntity,
                    ))
                    .id(),
            )
        }
    };

    if let Some(entity) = spawned_entity {
        // Attach Health if the actor declared <Health><MaxHitPoints/></Health>.
        // Without this, ScrOni's `health(guid("actor_Statue"))` returns 0 and
        // scripts like FXCathedral_Scenario_2/Statue.oni instantly trip their
        // `if health < 0.5` mission-failure branch.
        if let Some(max_hp) = actor.max_hitpoints {
            assets
                .commands
                .entity(entity)
                .insert(crate::combat::components::Health::new(max_hp));
            if let Some(destroy_time) = actor.destroy_time {
                assets
                    .commands
                    .entity(entity)
                    .insert(crate::combat::components::DestroyOnDeath(destroy_time));
            }
        }

        // Mark shootable if the actor declared a <Target> block.  Lets
        // `shoot <actor>` script ops resolve onto non-creature props
        // (e.g. the FX-Cathedral statue) the same way creatures already
        // resolve via their Fighter components.
        if actor.has_target {
            assets
                .commands
                .entity(entity)
                .insert(crate::combat::components::Targetable);
        }

        // <Sound> with a non-empty AudioPackage: attach the per-actor
        // sound driver.  drive_actor_sound_system picks it up,
        // resolves the package nuggets through the TD/HD/BD chain,
        // and spawns a child AudioPlayer with distance attenuation.
        if let Some(sound_data) = actor.sound.as_ref() {
            assets
                .commands
                .entity(entity)
                .insert(crate::actor_sound::ActorSound::new(sound_data.clone()));
        }

        // Attach CurveFollower if actor references a named curve
        if let Some(ref cname) = actor.curve_name {
            if let Some((_, pts)) = layout_paths
                .curves
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(cname))
            {
                if pts.len() >= 4 {
                    let curve = NurbsCurve::new(pts.clone());
                    let has_script = actor.script_filename.is_some();
                    let speed = if has_script {
                        0.0 // script will set speed via GotoCurvePhase
                    } else if actor.curve_speed > 0.0 {
                        actor.curve_speed
                    } else {
                        0.2 // 1.0 / 5.0 seconds
                    };
                    assets.commands.entity(entity).insert((
                        CurveFollower {
                            curve,
                            phase: 0.0,
                            speed,
                            speed_is_physical: true,
                            target_phase: if has_script { 0.0 } else { 1.0 },
                            wrap_around: if has_script {
                                false
                            } else {
                                !actor.curve_ping_pong
                            },
                            ping_pong: actor.curve_ping_pong,
                            look_along_xz: actor.curve_look_xz,
                            fixed_orientation: actor.curve_fixed_orientation,
                            reached_target: has_script,
                        },
                        avian3d::prelude::RigidBody::Kinematic,
                    ));
                    info!(
                        "Attached CurveFollower '{}' to {} ({} control points)",
                        cname,
                        actor.entity_type,
                        pts.len()
                    );
                } else {
                    warn!(
                        "Curve '{}' has {} points (need >= 4), skipping",
                        cname,
                        pts.len()
                    );
                }
            } else {
                warn!(
                    "Curve '{}' not found in layout.paths for {}",
                    cname, actor.entity_type
                );
            }
        }

        // Attach ScrOni script if actor has a <ScrOni> component
        if let Some(ref filename) = actor.script_filename {
            let default_main = std::path::Path::new(filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .trim_start_matches('$')
                .to_string();
            let main_script = actor
                .script_main
                .as_ref()
                .unwrap_or(&default_main)
                .trim_start_matches('$');

            let (script_dir, script_fname) = resolve_script_path(&layout_ctx.layout_dir, filename);
            match scroni::vm::load_script_file(&script_dir, &script_fname) {
                Ok(file) => {
                    if let Some(script_def) = file
                        .scripts
                        .iter()
                        .find(|s| s.name.eq_ignore_ascii_case(main_script))
                    {
                        let mut exec = scroni::vm::ScriptExec::new(script_def.clone(), entity);
                        for s in &file.scripts {
                            exec.available_scripts.insert(s.name.clone(), s.clone());
                        }
                        if let Some(ref update) = actor.updatestate
                            && update.eq_ignore_ascii_case("Asleep")
                        {
                            // Intentionally no-op.  Layout XML
                            // `updatestate="Asleep"` is common but we've
                            // opted to treat it as a hint, not a hard
                            // command: auto-inserting `ActorAsleep` here
                            // froze every such actor's animator, physics,
                            // and AI at spawn (so they stood in bind pose
                            // until something explicitly woke them),
                            // which didn't match intent — many of those
                            // actors need to at least animate.
                            //
                            // Dormancy is now entirely scroni-driven:
                            // scripts call `setupdatestate Asleep` or
                            // `sendaction deactivate`, which takes the
                            // `ActorAsleep` path through
                            // `scroni::system_bindings::SetUpdateState`
                            // and `scroni::vm` message delivery.  The
                            // `AsleepPlugin` (gravity layer + velocity
                            // pin + tick gates) handles those explicit
                            // deactivations correctly.
                            let _ = update;
                        }
                        assets
                            .commands
                            .entity(entity)
                            .insert(scroni::vm::ScrOniScript { exec });
                        info!(
                            "Attached ScrOni script '{}:{}' to {}",
                            filename, main_script, actor.entity_type
                        );
                    } else {
                        warn!(
                            "Script '{}' not found in {}/{} (available: {})",
                            main_script,
                            script_dir,
                            script_fname,
                            file.scripts
                                .iter()
                                .map(|s| s.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to compile script {}/{}: {}",
                        script_dir, script_fname, e
                    );
                }
            }
        }

        // Attach BroadcastTrigger if present
        if let Some(radius) = actor.broadcast_radius {
            assets
                .commands
                .entity(entity)
                .insert(crate::scroni::vm::BroadcastTrigger {
                    radius,
                    ..Default::default()
                });
            info!(
                "Attached BroadcastTrigger (radius {}) to {} at position {:?}",
                radius,
                actor.entity_type,
                pos_override.unwrap_or(actor.position)
            );
        }

        // Attach fx components if present
        if !actor.is_creature && (actor.fx_type.is_some() || actor.ptx_name.is_some()) {
            assets
                .commands
                .entity(entity)
                .insert(crate::oni2_loader::components::ActorFxType {
                    fx_name: actor.fx_type.clone(),
                    start_active: actor.fx_start_active,
                    ptx_name: actor.ptx_name.clone(),
                    ptx_birth_rate: actor.ptx_birth_rate,
                    ptx_num_particles: actor.ptx_num_particles,
                    ptx_offset: actor.ptx_offset,
                });
            info!("Attached FX component to {}", actor.entity_type);
        }

        // Attach BroadcastTrigger if present
        if let Some(radius) = actor.broadcast_radius {
            assets
                .commands
                .entity(entity)
                .insert(crate::scroni::vm::BroadcastTrigger {
                    radius,
                    ..Default::default()
                });
            info!(
                "Attached BroadcastTrigger (r={}) to {}",
                radius, actor.entity_type
            );
        }

        // Attach CheckpointTrigger if present
        if let Some(index) = actor.checkpoint_index {
            let radius = actor.checkpoint_radius.unwrap_or(2.0); // Fallback radius
            assets.commands.entity(entity).insert((
                crate::oni2_loader::components::CheckpointTrigger { index, radius },
                avian3d::prelude::Collider::sphere(radius),
                avian3d::prelude::Sensor, // Triggers don't physically block the player
            ));
            info!(
                "Attached CheckpointTrigger (index {}, radius {}) to {}",
                index, radius, actor.entity_type
            );
        }

        // Attach FightVectorTrigger if present
        if let Some(radius) = actor.fvt_radius {
            let attack_alias = actor.fvt_attack.clone().unwrap_or_default();
            assets
                .commands
                .entity(entity)
                .insert(crate::fight_vector::FightVectorTrigger {
                    radius,
                    directional: actor.fvt_directional.unwrap_or(true),
                    offset: actor.fvt_offset.unwrap_or(Vec3::ZERO),
                    attack_alias: attack_alias.clone(),
                    enabled: true,
                });
            info!(
                "Attached FightVectorTrigger (radius {}, attack {}) to {}",
                radius, attack_alias, actor.entity_type
            );
        }

        // Defer parent attachment if XML dictates
        if let Some(parent_actor) = &actor.parent_actor {
            assets
                .commands
                .entity(entity)
                .insert(crate::oni2_loader::components::PendingParent {
                    parent_name: parent_actor.clone(),
                    bone_name: actor.parent_bone.clone(),
                });
            info!("Attached PendingParent deferral onto {}", actor.entity_type);
        }

        return Some((entity, actor));
    }

    None
}

/// Parse layout.lights, default.environment, layout.fog, layout.paths, and skyhat.
/// Spawns Bevy light entities, fog resource, paths resource, and skyhat mesh.
/// If this light entry has the `fxLight` flag set, tag its spawned
/// entity with `PendingGlow` so `resolve_pending_glow_system` will
/// look up the named `LightGlowDef` from `FxLibrary` next frame and
/// attach the billboard corona child.  No-op when `fx_light` is false.
fn attach_pending_glow_if_requested(
    commands: &mut Commands,
    light_entity: Entity,
    light: &crate::oni2_loader::parsers::layout::LayoutLight,
) {
    if !light.fx_light {
        return;
    }
    let Some(glow_type_name) = light.light_glow_type.clone() else {
        // fxLight flag set without a glow type — file malformed; skip.
        return;
    };
    let light_color = Color::srgb(light.color[0], light.color[1], light.color[2]);
    commands
        .entity(light_entity)
        .insert(crate::oni2_loader::light_glow::PendingGlow {
            glow_type_name,
            glow_intensity_scale: light.glow_intensity_scale,
            light_dir: light.direction,
            light_color,
        });
}

fn load_layout_lights(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    layout_dir: &str,
) {
    let layout_path = layout_dir;

    // Parse default.environment for directional/ambient
    let env = parse_environment(layout_path);

    // Parse layout.fog for fog + lighting (used when default.environment is absent)
    let fog_data = parse_layout_fog(layout_path);

    // Parse layout.lights for point lights
    let lights = parse_lights_file(layout_path);

    // layout.paths already parsed in load_layout — no need to re-parse here

    // Load skyhat model if present (sky dome that follows camera)
    load_skyhat(commands, meshes, materials, images, layout_path);

    // Apply lighting from environment file or fog file
    if let Some(ref env) = env {
        commands.spawn((
            DirectionalLight {
                illuminance: 20_000.0,
                shadows_enabled: true,
                color: Color::srgb(env.light_color[0], env.light_color[1], env.light_color[2]),
                ..default()
            },
            Transform::from_xyz(
                env.light_direction.x,
                env.light_direction.y,
                env.light_direction.z,
            )
            .looking_at(Vec3::ZERO, Vec3::Y),
            InGameEntity,
        ));

        commands.spawn((
            AmbientLight {
                color: Color::srgb(
                    env.ambient_color[0],
                    env.ambient_color[1],
                    env.ambient_color[2],
                ),
                brightness: 800.0,
                ..default()
            },
            InGameEntity,
        ));

        // Apply fog from environment file
        if env.fog_end > env.fog_start {
            commands.insert_resource(LayoutFogSettings {
                color: Color::srgb(env.fog_color[0], env.fog_color[1], env.fog_color[2]),
                start: env.fog_start,
                end: env.fog_end,
            });
        }

        info!(
            "Layout: loaded environment (dir_light=({:.2},{:.2},{:.2}), fog_start={:.1}, fog_end={:.1})",
            env.light_direction.x,
            env.light_direction.y,
            env.light_direction.z,
            env.fog_start,
            env.fog_end
        );
    } else if let Some(ref fog) = fog_data {
        // No environment file — use layout.fog for lighting + fog
        for (i, light) in fog.lights.iter().enumerate() {
            if !light.enabled {
                continue;
            }
            let color = Color::srgb(light.color[0], light.color[1], light.color[2]);
            let dir = space::to_bevy_space_pos(Vec3::new(
                light.direction[0],
                light.direction[1],
                light.direction[2],
            ));
            if i < 2 {
                // First two lights are directional
                commands.spawn((
                    DirectionalLight {
                        illuminance: 20_000.0,
                        shadows_enabled: i == 0,
                        color,
                        ..default()
                    },
                    Transform::from_translation(dir * 100.0).looking_at(Vec3::ZERO, Vec3::Y),
                    InGameEntity,
                ));
            } else {
                // Third light is ambient fill
                commands.spawn((
                    AmbientLight {
                        color,
                        brightness: 800.0,
                        ..default()
                    },
                    InGameEntity,
                ));
            }
        }

        // Apply fog
        if fog.enabled && fog.end > fog.start {
            commands.insert_resource(LayoutFogSettings {
                color: Color::srgb(fog.color[0], fog.color[1], fog.color[2]),
                start: fog.start,
                end: fog.end,
            });
            info!(
                "Layout: loaded fog from layout.fog (start={:.1}, end={:.1})",
                fog.start, fog.end
            );
        }

        info!(
            "Layout: loaded lighting from layout.fog ({} lights)",
            fog.lights.len()
        );
    }

    // Spawn lights from layout.lights.  Mirrors the legacy
    // `lvlLightManager::AddLight`:
    // the light is always added to the renderer, and ALSO registered
    // with the shadow manager iff `CastShadow()` (i.e. `CastShadowRange > 0`).
    // We mirror that with `shadows_enabled = (cast_shadow_range > 0.0)`.
    //
    // Intensity scaling: legacy `lgtLight::ContributionTo`
    // uses a `1/r²` falloff with the file's
    // `Intensity` as the numerator — same shape Bevy's PBR PointLight
    // uses by default.  But the units differ:
    //   - Legacy intensity is unitless / scene-tuned, typical 30–300.
    //   - Bevy `PointLight.intensity` is in candela (luminous intensity).
    // Constants live at module scope (top of file) so the runtime
    // SetLightIntensity binding in scroni::system_bindings can apply the
    // same scaling — keeping authored vs scripted intensity in the same
    // units. See LIGHT_* below.

    let mut point_count = 0;
    let mut spot_count = 0;
    let mut ambient_count = 0;
    let mut shadow_count = 0;
    let mut fx_glow_count = 0;
    for light in &lights {
        let pos = light.position;
        let color = Color::srgb(light.color[0], light.color[1], light.color[2]);
        let casts_shadow = light.cast_shadow_range > 0.0;
        if casts_shadow {
            shadow_count += 1;
        }
        if light.fx_light {
            fx_glow_count += 1;
        }

        match light.light_type.as_str() {
            "point" => {
                // Lights with intensity 0 are spawned anyway: ScrOni
                // (`setLightParameter <name>`/`intensity`) can ramp them up
                // later, and skipping here leaves the lookup-by-Name unable
                // to find them. The trash-chute door light is the canonical
                // example — starts dark, the open/close ramps drive it.
                let range = (light.intensity * POINT_RANGE_FROM_INTENSITY).max(POINT_MIN_RANGE);
                let lumens = light.intensity * POINT_INTENSITY_TO_CANDELA;

                let mut ec = commands.spawn((
                    PointLight {
                        color,
                        intensity: lumens,
                        range,
                        // Shadows are gated by the shadow-LOD system at
                        // runtime — start off, the LOD picks the K closest
                        // to the player each tick.  See shadow_lod.rs.
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_translation(pos),
                    InGameEntity,
                    Name::new(light.name.clone()),
                ));
                if casts_shadow {
                    ec.insert(crate::shadow_lod::ShadowCandidate);
                }
                let entity = ec.id();
                attach_pending_glow_if_requested(commands, entity, light);
                point_count += 1;
            }
            "spot" => {
                let range = (light.intensity * POINT_RANGE_FROM_INTENSITY).max(POINT_MIN_RANGE);
                let lumens = light.intensity * SPOT_INTENSITY_TO_CANDELA;
                // Legacy `SpotAngle` is the half-angle of the cone in
                // degrees (lgtLight stores in degrees, ContributionTo
                // computes against it).  Bevy's SpotLight wants the
                // OUTER half-angle in radians; inner is the soft-edge
                // start, choose 90% of outer for a slight feathering.
                let outer = light
                    .spot_angle
                    .to_radians()
                    .clamp(0.0, std::f32::consts::PI);
                let inner = outer * 0.9;

                // Bevy's SpotLight points along its local -Z by default.
                // Build a transform that aligns -Z with the file's
                // Direction (already converted to Bevy space at parse time).
                let look = if light.direction.length_squared() > 1e-6 {
                    pos + light.direction.normalize()
                } else {
                    // Defensive fallback: aim down.
                    pos + Vec3::NEG_Y
                };
                let transform = Transform::from_translation(pos).looking_at(look, Vec3::Y);

                let mut ec = commands.spawn((
                    SpotLight {
                        color,
                        intensity: lumens,
                        range,
                        outer_angle: outer,
                        inner_angle: inner,
                        // Same LOD treatment as PointLight above.
                        shadows_enabled: false,
                        ..default()
                    },
                    transform,
                    InGameEntity,
                    Name::new(light.name.clone()),
                ));
                if casts_shadow {
                    ec.insert(crate::shadow_lod::ShadowCandidate);
                }
                let entity = ec.id();
                attach_pending_glow_if_requested(commands, entity, light);
                spot_count += 1;
            }
            "directional" => {
                // Legacy directional lights from layout.lights are
                // RARE — the level's main directional usually comes
                // from `default.environment` (handled above).  When
                // one IS present here, treat it as a supplementary
                // sun.  Direction already in Bevy space.
                let dir = if light.direction.length_squared() > 1e-6 {
                    light.direction.normalize()
                } else {
                    Vec3::NEG_Y
                };
                let transform = Transform::from_translation(pos).looking_to(dir, Vec3::Y);
                commands.spawn((
                    DirectionalLight {
                        color,
                        // Map the unitless intensity into illuminance
                        // (lux).  20 000 lux ≈ overcast daylight,
                        // matches the env-light fallback elsewhere.
                        illuminance: light.intensity * 200.0,
                        shadows_enabled: casts_shadow,
                        ..default()
                    },
                    transform,
                    InGameEntity,
                    Name::new(light.name.clone()),
                ));
            }
            "ambient" => {
                if env.is_none() && fog_data.is_none() {
                    let brightness = light.intensity * 2.0;
                    commands.spawn((
                        AmbientLight {
                            color,
                            brightness,
                            ..default()
                        },
                        InGameEntity,
                        Name::new(light.name.clone()),
                    ));
                }
                ambient_count += 1;
            }
            other => {
                warn!("layout light '{}': unknown type '{}'", light.name, other);
            }
        }
    }
    if point_count > 0 || spot_count > 0 || ambient_count > 0 {
        info!(
            "Layout: loaded {} point + {} spot + {} ambient lights ({} cast shadows, {} fxLight glows queued for rendering)",
            point_count, spot_count, ambient_count, shadow_count, fx_glow_count
        );
    }

    // Fallback: if no lighting data at all, add defaults
    if env.is_none() && fog_data.is_none() && ambient_count == 0 {
        commands.spawn((
            DirectionalLight {
                illuminance: 20_000.0,
                shadows_enabled: true,
                ..default()
            },
            Transform::from_xyz(50.0, 80.0, 50.0).looking_at(Vec3::ZERO, Vec3::Y),
            InGameEntity,
        ));
        commands.spawn((
            AmbientLight {
                color: Color::WHITE,
                brightness: 800.0,
                ..default()
            },
            InGameEntity,
        ));
        info!("Layout: no environment data, using placeholder lighting");
    }
}

/// Load skyhat.mod from layout directory and spawn as an unlit sky dome.
fn load_skyhat(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    layout_path: &str,
) {
    let skyhat_path = format!("{}/skyhat.mod", layout_path);
    if !crate::vfs::exists("", &skyhat_path) {
        return;
    }

    let model = match load_mod_file(&skyhat_path) {
        Some(m) => m,
        None => return,
    };

    // Look for sky texture in the layout directory
    let sky_texture = find_sky_texture(layout_path, images);

    let sub_meshes = build_meshes_by_material(&model);
    if sub_meshes.is_empty() {
        return;
    }

    // Spawn parent entity for skyhat
    let parent = commands
        .spawn((
            Transform::default(),
            Visibility::Visible,
            SkyHat,
            InGameEntity,
        ))
        .id();

    for (mat_idx, mesh) in sub_meshes {
        // Use unlit material — skyhat appears as illuminated sky
        let texture = if let Some(ref tex) = sky_texture {
            Some(tex.clone())
        } else {
            model.materials.get(mat_idx).and_then(|oni_mat| {
                oni_mat.texture_name.as_ref().and_then(|tex_name| {
                    load_texture(layout_path, tex_name, images).map(|(handle, _)| handle)
                })
            })
        };

        let mat = materials.add(StandardMaterial {
            base_color_texture: texture,
            unlit: true,
            cull_mode: None,
            ..default()
        });

        let mesh_handle = meshes.add(mesh);
        let child = commands
            .spawn((
                Mesh3d(mesh_handle),
                MeshMaterial3d(mat),
                Transform::default(),
            ))
            .id();
        commands.entity(parent).add_child(child);
    }

    info!("Layout: loaded skyhat model from {:?}", skyhat_path);
}

/// Find a sky texture (.tex or .tga) in the layout directory.
fn find_sky_texture(
    layout_path: &str,
    images: &mut ResMut<Assets<Image>>,
) -> Option<Handle<Image>> {
    // Search for any *sky*.tex or *sky*.tga file
    if let Ok(entries) = crate::vfs::read_dir(layout_path) {
        for entry in entries {
            let name = entry
                .path
                .split('/')
                .next_back()
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if name.contains("sky") {
                if name.ends_with(".tex") && !name.ends_with(".tex.tga") {
                    if let Ok(tex_bytes) = crate::vfs::read("", &entry.path)
                        && let Some((width, height, rgba, _)) = decode_tex(&tex_bytes)
                    {
                        info!("Loaded sky texture: {} ({}x{})", entry.path, width, height);
                        let mut image = Image::new(
                            bevy::render::render_resource::Extent3d {
                                width,
                                height,
                                depth_or_array_layers: 1,
                            },
                            bevy::render::render_resource::TextureDimension::D2,
                            rgba,
                            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                            default(),
                        );
                        image.sampler = bevy::image::ImageSampler::Descriptor(
                            bevy::image::ImageSamplerDescriptor {
                                address_mode_u: bevy::image::ImageAddressMode::Repeat,
                                address_mode_v: bevy::image::ImageAddressMode::Repeat,
                                ..default()
                            },
                        );
                        return Some(images.add(image));
                    }
                } else if (name.ends_with(".tex.tga") || name.ends_with(".tga"))
                    && let Some((handle, _)) = super::spawn::load_tga_file(&entry.path, images)
                {
                    info!("Loaded sky texture: {}", entry.path);
                    return Some(handle);
                }
            }
        }
    }
    None
}

/// Load a .mod file, auto-detecting text v1.10 vs binary v2.10 format.
pub(crate) fn load_mod_file(path: &str) -> Option<Oni2Model> {
    let data = crate::vfs::read("", path).ok()?;
    if data.len() < 14 {
        return None;
    }

    // Check for binary v2.10 header: "version: 2.10\0"
    let entity_dir = std::path::Path::new(path)
        .parent()
        .unwrap_or(std::path::Path::new(""))
        .to_str()
        .unwrap_or("");

    if data.starts_with(b"version: 2.10\0") {
        info!("Loading binary v2.10 model: {}", path);
        return parse_mod_binary(&data, entity_dir);
    }

    // Otherwise try text v1.10
    let text = String::from_utf8_lossy(&data);
    if text.starts_with("version: 1.10") {
        info!("Loading text v1.10 model: {}", path);
        return Some(parse_mod(&text, entity_dir));
    }

    warn!("Unknown .mod format: {}", path);
    None
}

/// Resolve a ScrOni script filename to a filesystem path.
/// `$name` means layout-local: `<layout_dir>/scripts/<name>.oni`
/// Otherwise the filename is a relative path from the assets root (layout_dir/../..).
fn resolve_script_path(layout_dir: &str, filename: &str) -> (String, String) {
    let add_ext = |name: &str| -> String {
        if name.to_ascii_lowercase().ends_with(".oni") {
            name.to_string()
        } else {
            format!("{}.oni", name)
        }
    };

    let normalized = filename.replace('\\', "/");

    if let Some(stripped) = normalized.strip_prefix('$') {
        // Layout-local script
        (format!("{}/scripts", layout_dir), add_ext(stripped))
    } else {
        // Relative path from assets root. layout_dir is like "layout/EndlessCity"
        let mut parts: Vec<&str> = layout_dir.split('/').collect();
        parts.pop();
        parts.pop();

        let path = if parts.is_empty() {
            normalized
        } else {
            format!("{}/{}", parts.join("/"), normalized)
        };

        let mut segments: Vec<&str> = path.split('/').collect();
        let fname = segments.pop().unwrap_or("");
        let dir = segments.join("/");

        (dir, add_ext(fname))
    }
}

/// Extract the base="..." attribute from an <actor> tag.
fn extract_xml_base_attr(content: &str) -> Option<String> {
    let idx = content.find("<actor ")?;
    let after = &content[idx..];
    let end = after.find('>')?;
    let tag = &after[..end];
    let base_start = tag.find("base=\"")? + 6;
    let base_end = tag[base_start..].find('"')? + base_start;
    Some(tag[base_start..base_end].to_string())
}

/// Extract value="..." from an XML attribute tag like <TagName value="..."/>
fn extract_xml_attr(content: &str, tag: &str) -> Option<String> {
    let pattern = format!("<{}", tag);
    let idx = content.find(&pattern)?;
    let after = &content[idx..];
    let val_start = after.find("value=\"")? + 7;
    let val_end = after[val_start..].find('"')? + val_start;
    Some(after[val_start..val_end].to_string())
}

/// Parse "x y z" string into Vec3.
fn parse_vec3(s: &str) -> Option<Vec3> {
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() >= 3 {
        Some(Vec3::new(parts[0], parts[1], parts[2]))
    } else {
        None
    }
}

/// Find the Konoko (player) spawn point from a layout's actor files.
/// Searches for an actor with base="template_konoko" and extracts its position.
pub fn find_konoko_spawn(layout_dir: &str) -> Option<Vec3> {
    let layout_path = layout_dir;
    let actors_path = format!("{}/layout.actors", layout_path);
    let actors_content = crate::vfs::read_to_string("", &actors_path).ok()?;

    for line in actors_content.lines() {
        let actor_name = line.trim();
        if actor_name.is_empty() || actor_name.parse::<u32>().is_ok() {
            continue;
        }

        let xml_path = format!("{}/{}.xml", layout_path, actor_name);
        let content = match crate::vfs::read_to_string("", &xml_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if !content.contains("template_konoko") {
            continue;
        }

        let position = extract_xml_attr(&content, "Position")
            .and_then(|s| parse_vec3(&s))
            .unwrap_or(Vec3::ZERO);

        // Convert from left-handed to right-handed at parse boundary
        let bevy_pos = space::to_bevy_space_pos(position);
        info!("Found Konoko spawn at {:?} → bevy {:?}", position, bevy_pos);
        return Some(bevy_pos);
    }

    None
}
