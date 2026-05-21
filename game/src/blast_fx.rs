/*
 * blast_fx.rs — Blast-Chamber BLASTFIRE port.
 *
 * Three hardcoded `.type` drawables (fire / residue / edge) loaded once
 * at startup via the standalone `load_drawable` (rmcDrawable-equivalent)
 * orchestrator.  When `handle_spawn_fx` resolves an `EffectDef::BlastFire`,
 * the FX dispatch path constructs a parent transform at the spawn
 * position and parents one renderable child per loaded pass under it.
 * The parent carries a `BlastFireFx` component that holds the spline
 * path / speed / fade timers from the parsed `BlastFireDef`; a
 * forthcoming `update_blast_fire_system` will advance the parent along
 * the spline and drive the point-light intensity (legacy `UpdateAll`).
 *
 * Asset filenames are HARDCODED — unlike most FX types, the blast
 * shipped with the C++ engine baking the three mesh names into
 * `fxBlastFireType::Init`.  The `BlastFireDef` config drives motion,
 * not geometry.  See [[rmcdrawable-gap]] for the architectural reason
 * the standalone `load_drawable` exists at all.
 */

use bevy::prelude::*;

use crate::env_reflect_material::EnvReflectMaterial;
use crate::oni2_loader::animation::PassMaterial;
use crate::oni2_loader::drawable::{LoadedDrawable, load_drawable};
use crate::oni2_loader::parsers::effect::BlastFireDef;
use crate::oni2_loader::registries::DrawableLibrary;

/// Cached, startup-loaded handles for the three blast-fire drawables.
/// Spawning is then a transform + child commands — no I/O on the hot
/// path.
#[derive(Resource, Default)]
pub struct BlastFxAssets {
    pub fire: Option<LoadedDrawable>,
    pub residue: Option<LoadedDrawable>,
    pub edge: Option<LoadedDrawable>,
}

/// Per-instance runtime state for one spawned blast.  Mirrors the
/// legacy `fxBlastFireType` runtime fields: timer ramps the light fade,
/// `path` is the world-local control polyline, `birth_fraction` walks
/// from 0 → `path.len()-1` at `speed` units per (path-arclength)
/// second.  `update_blast_fire_system` consumes these each frame.
///
/// Path points are stored in entity-local space (already X/Z-negated
/// from AGE → Bevy at parse-egress).  `spawn_origin` is the world-space
/// anchor; final translation each tick =
/// `spawn_origin + lerp(path[i], path[i+1], frac)`.
#[derive(Component, Clone)]
pub struct BlastFireFx {
    pub path: Vec<Vec3>,
    pub spawn_origin: Vec3,
    pub speed: f32,
    pub birth_fraction: f32,
    pub start_fade: f32,
    pub end_fade: f32,
    /// Fade timer.  Counts down each tick when non-zero.  Used at start
    /// (initialised to `start_fade`) for a brief stabilisation window
    /// and at end (set to `end_fade` when the spline runs out) for the
    /// despawn fade.  When `stopping == true` and `timer` reaches 0, the
    /// entity is despawned.
    pub timer: f32,
    pub stopping: bool,
}

/// Startup: load the three hardcoded drawables from disk.  Runs once
/// after the global registries are initialised.  Failures here are
/// logged but non-fatal — the BlastFire branch in `handle_spawn_fx`
/// short-circuits when an asset is missing so the rest of the game
/// keeps working.
pub fn load_blast_fx_assets(
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut env_materials: ResMut<Assets<EnvReflectMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut drawables: ResMut<DrawableLibrary>,
    mut blast_assets: ResMut<BlastFxAssets>,
) {
    let triples = [
        ("Entity/BlastFire/blast_fire.type", "fire"),
        ("Entity/BlastFire/blast_residue.type", "residue"),
        ("Entity/BlastFire/blast_edge.type", "edge"),
    ];
    for (path, label) in triples {
        match load_drawable(
            path,
            &mut meshes,
            &mut materials,
            &mut env_materials,
            &mut images,
            &mut drawables,
        ) {
            Some(d) => {
                info!(
                    "blast_fx: loaded {} ({} passes, radius {})",
                    label,
                    d.passes.len(),
                    d.radius
                );
                match label {
                    "fire" => blast_assets.fire = Some(d),
                    "residue" => blast_assets.residue = Some(d),
                    "edge" => blast_assets.edge = Some(d),
                    _ => {}
                }
            }
            None => {
                warn!("blast_fx: failed to load {}", path);
            }
        }
    }
}

/// Spawn one blast-fire instance at `position`.  Builds a parent
/// entity carrying the `BlastFireFx` runtime state plus child entities
/// for each loaded pass of fire/residue/edge.  All pass meshes share
/// the spawn transform — the legacy engine sweeps them along the
/// spline as a single rigid bundle.
///
/// `parent` (when `Some`) re-parents the whole bundle under another
/// entity so its transform follows.  Most makefx callers pass `None`
/// for world-fixed FX.
pub fn spawn_blast_fire(
    commands: &mut Commands,
    def: &BlastFireDef,
    position: Vec3,
    parent: Option<Entity>,
    assets: &BlastFxAssets,
) -> Option<Entity> {
    let fire = assets.fire.as_ref()?;
    // Convert legacy AGE rotation (Euler-Y radians, AGE handedness)
    // into a Bevy rotation.  Legacy yaw is around Y; X/Z negation on
    // the parent is already handled by the existing entity coord-
    // conversion convention, but blast spawns aren't entities — they
    // sit in raw Bevy space.  Apply yaw directly.
    let rotation = Quat::from_euler(
        EulerRot::YXZ,
        def.rotation.y,
        def.rotation.x,
        def.rotation.z,
    );
    let scale = def.scale;

    // Convert the AGE path points (X/Z negation for left→right handed)
    // at parse-egress time so downstream consumers only see Bevy space.
    // See [[feedback_convert_at_boundary]] — coord conversion belongs at
    // the boundary, not scattered through the runtime.
    let bevy_path: Vec<Vec3> = def
        .path
        .iter()
        .map(|p| Vec3::new(-p.x, p.y, -p.z))
        .collect();

    // Initial translation = spawn position + path[0] so the geometry
    // appears at the first spline control point right away rather than
    // popping after one frame of update.  `update_blast_fire_system`
    // re-derives translation each tick from `spawn_origin + lerp(...)`.
    let initial_offset = bevy_path.first().copied().unwrap_or(Vec3::ZERO);

    let parent_entity = commands
        .spawn((
            Transform {
                translation: position + initial_offset,
                rotation,
                scale,
            },
            GlobalTransform::default(),
            Visibility::Visible,
            BlastFireFx {
                path: bevy_path,
                spawn_origin: position,
                speed: def.speed,
                birth_fraction: 0.0,
                start_fade: 1.0,
                end_fade: 2.0,
                timer: 0.0,
                stopping: false,
            },
            // Cleanup on layout exit.
            crate::menu::InGameEntity,
        ))
        .id();

    // Spawn one renderable child per pass of each loaded drawable.
    spawn_drawable_children(commands, parent_entity, fire);
    if let Some(residue) = &assets.residue {
        spawn_drawable_children(commands, parent_entity, residue);
    }
    if let Some(edge) = &assets.edge {
        spawn_drawable_children(commands, parent_entity, edge);
    }

    if let Some(p) = parent {
        commands.entity(p).add_child(parent_entity);
    }

    Some(parent_entity)
}

/// Advance every active `BlastFireFx` along its spline and despawn the
/// ones that finish.  Mirrors the legacy `fxBlastFireType::UpdateAll`:
///
///   • `birth_fraction` walks 0 → N-1 (segment indices).  Per-tick
///     increment is `speed * dt / segment_length` so world velocity
///     stays roughly constant regardless of path-segment spacing.
///   • Integer part = current segment, fractional part = lerp factor
///     within that segment.
///   • When `birth_fraction >= N-1` (past last segment), flag `stopping`
///     and start a `end_fade`-second despawn timer.  Mesh stays at the
///     final path point during the fade so it doesn't pop away mid-
///     flight; once `timer <= 0`, despawn the parent (children clean
///     up via Bevy's hierarchy despawn).
///
/// Light fade + screen wash are NOT yet wired — they belong with the
/// PointLight + post-processing additions in Phases 3-4 of the port.
pub fn update_blast_fire_system(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut BlastFireFx, &mut Transform)>,
) {
    let dt = time.delta_secs();
    if dt == 0.0 {
        return;
    }

    for (entity, mut fx, mut transform) in &mut query {
        let n = fx.path.len();
        if n < 2 {
            // Degenerate path — nothing to traverse, despawn immediately.
            commands.entity(entity).despawn();
            continue;
        }

        // Tick the fade timer when active.  When `stopping` and the
        // timer hits zero, despawn the parent (and its children).
        if fx.timer > 0.0 {
            fx.timer = (fx.timer - dt).max(0.0);
            if fx.stopping && fx.timer <= 0.0 {
                commands.entity(entity).despawn();
                continue;
            }
        }

        if !fx.stopping {
            // Are we past the last segment?  If so, transition to
            // STOPPING and let the end-fade timer run out.
            let segment_idx = fx.birth_fraction.floor() as usize;
            if segment_idx >= n - 1 {
                fx.stopping = true;
                fx.timer = fx.end_fade;
                // Pin to last point so the geometry rests at the spline
                // tail during fade-out rather than continuing to drift.
                let tail = fx.path[n - 1];
                transform.translation = fx.spawn_origin + tail;
                continue;
            }

            let p0 = fx.path[segment_idx];
            let p1 = fx.path[segment_idx + 1];
            let segment_length = p0.distance(p1);
            // Guard against zero-length degenerate segments — advance
            // by a full unit so we step past them in one tick (matches
            // legacy's `if (invLength)` skip-when-zero behaviour).
            let advance = if segment_length > 1e-5 {
                fx.speed * dt / segment_length
            } else {
                1.0
            };
            fx.birth_fraction += advance;

            // Re-check after advance: parameter may have crossed N-1.
            let new_idx = fx.birth_fraction.floor() as usize;
            if new_idx >= n - 1 {
                fx.stopping = true;
                fx.timer = fx.end_fade;
                transform.translation = fx.spawn_origin + fx.path[n - 1];
                continue;
            }

            // Lerp position within the current segment.  Segment may
            // have changed (when advance > 1.0 over a short segment),
            // so refetch endpoints.
            let i = new_idx;
            let frac = fx.birth_fraction - i as f32;
            let local = fx.path[i].lerp(fx.path[i + 1], frac);
            transform.translation = fx.spawn_origin + local;
        }
    }
}

/// Spawn one Bevy entity per pass of a loaded drawable under
/// `parent_entity`.  Each pass renders the same mesh with a different
/// material — multi-pass shaders are layered via `depth_bias`, already
/// baked into the material by `build_pass_material`.
fn spawn_drawable_children(
    commands: &mut Commands,
    parent_entity: Entity,
    drawable: &LoadedDrawable,
) {
    for pass in &drawable.passes {
        let mesh = Mesh3d(pass.mesh.clone());
        let child = match &pass.material {
            PassMaterial::Standard(h) => commands
                .spawn((mesh, MeshMaterial3d(h.clone()), Transform::IDENTITY))
                .id(),
            PassMaterial::EnvReflect(h) => commands
                .spawn((mesh, MeshMaterial3d(h.clone()), Transform::IDENTITY))
                .id(),
        };
        commands.entity(parent_entity).add_child(child);
    }
}
