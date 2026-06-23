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
    /// Fade timer.  Counts down each tick when non-zero.  Seeded with
    /// `start_fade` at spawn and used to drive the alpha ramp-in
    /// during the first `start_fade` seconds of life.  When the
    /// spline runs out, `stopping` flips true and `timer` is reset
    /// to `end_fade` to drive the ramp-out; the entity despawns when
    /// `timer` reaches 0 while `stopping`.
    pub timer: f32,
    pub stopping: bool,
    /// Per-instance `StandardMaterial` handles cloned from the
    /// shared library at spawn.  We modulate `base_color.alpha` on
    /// these each tick to drive the start-/end-fade ramps without
    /// bleeding into other blast instances or other FX that share
    /// the source material.
    pub instance_materials: Vec<Handle<StandardMaterial>>,
    /// Child entity carrying the legacy `fxLight` (`kPoint`, warm
    /// color, intensity modulated by `fade * dual_sine`).  `None`
    /// when the platform's lighting pipeline is unavailable or
    /// PointLight construction failed; the fade logic still runs.
    pub light_entity: Option<Entity>,
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
    materials: &mut Assets<StandardMaterial>,
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

    // Start-fade duration; legacy default = 1.0s (`fxBlastFireType`
    // ctor sets `StartFade=1.0f`).  Seed `timer` to this value so
    // `update_blast_fire_system` can ramp alpha 0 → 1 over the first
    // second.  Legacy `Start()` does `SetTimer(StartFade)`.
    let start_fade = 1.0;
    let end_fade = 2.0;

    // Pre-clone all StandardMaterial assets used by this blast.  We
    // build the full handle list here so the parent component is
    // populated in one shot (no placeholder insert-then-overwrite
    // dance).  Env-reflect passes share the source handle since
    // their shader has no tint uniform; the cloned standard pass
    // beneath fades the visible diffuse and the additive env layer
    // contributes proportionally less as the base goes dark.
    let mut collect_clones =
        |drawable: &LoadedDrawable, out: &mut Vec<(PassMaterial, Handle<StandardMaterial>)>| {
            for pass in &drawable.passes {
                if let PassMaterial::Standard(h) = &pass.material
                    && let Some(src) = materials.get(h)
                {
                    let cloned = materials.add(src.clone());
                    out.push((PassMaterial::Standard(cloned.clone()), cloned));
                }
            }
        };
    let mut cloned_per_pass: Vec<(PassMaterial, Handle<StandardMaterial>)> = Vec::new();
    collect_clones(fire, &mut cloned_per_pass);
    if let Some(residue) = &assets.residue {
        collect_clones(residue, &mut cloned_per_pass);
    }
    if let Some(edge) = &assets.edge {
        collect_clones(edge, &mut cloned_per_pass);
    }
    let instance_materials: Vec<Handle<StandardMaterial>> =
        cloned_per_pass.iter().map(|(_, h)| h.clone()).collect();

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
                start_fade,
                end_fade,
                timer: start_fade,
                stopping: false,
                instance_materials,
                light_entity: None, // filled below
            },
            // Cleanup on layout exit.
            crate::menu::InGameEntity,
        ))
        .id();

    // Spawn one renderable child per pass.  The cloned-material
    // queue is drained in the same drawable/pass order
    // `collect_clones` walked, so the indices match up — pop the
    // next clone whenever we hit a Standard pass.
    let mut clone_cursor = 0usize;
    let mut spawn_passes = |drawable: &LoadedDrawable| {
        for pass in &drawable.passes {
            let mesh = Mesh3d(pass.mesh.clone());
            let child = match &pass.material {
                PassMaterial::Standard(_) => {
                    let (_, h) = &cloned_per_pass[clone_cursor];
                    clone_cursor += 1;
                    commands
                        .spawn((mesh, MeshMaterial3d(h.clone()), Transform::IDENTITY))
                        .id()
                }
                PassMaterial::EnvReflect(h) => commands
                    .spawn((mesh, MeshMaterial3d(h.clone()), Transform::IDENTITY))
                    .id(),
            };
            commands.entity(parent_entity).add_child(child);
        }
    };
    spawn_passes(fire);
    if let Some(residue) = &assets.residue {
        spawn_passes(residue);
    }
    if let Some(edge) = &assets.edge {
        spawn_passes(edge);
    }

    // Spawn the point light as a child of the blast.  Color matches
    // legacy `fxBlastFireType::Init` (`SetColor(Vector4(1, 0.5, 0.2,
    // 1))`).  Intensity is driven per-tick by the dual-sine flicker
    // formula in `update_blast_fire_system`; seed at 0 so a fresh
    // light doesn't pop in at peak brightness for one frame.
    //
    // `range` is the radius beyond which the light contributes
    // nothing.  Legacy `fxLight::kPoint` has natural quadratic
    // falloff with no hard cutoff, but Bevy's PointLight needs a
    // finite range.  20 units roughly matches the spline extent of
    // the default `BlastFireAll` def — large enough to wash the
    // surrounding geometry without lighting the whole layout.
    let light_entity = commands
        .spawn((
            PointLight {
                color: Color::srgb(1.0, 0.5, 0.2),
                intensity: 0.0,
                range: 20.0,
                shadows_enabled: false,
                ..default()
            },
            Transform::IDENTITY,
        ))
        .id();
    commands.entity(parent_entity).add_child(light_entity);

    // Patch the light handle back onto the parent.  Re-inserting
    // the full `BlastFireFx` would require re-cloning the path and
    // material list; using a small queryless update via a one-shot
    // observer would be heavier than just splitting the component.
    // Cheapest: a marker resource keyed by entity is overkill here
    // — we just update via a follow-up `entity_mut` write.
    commands
        .entity(parent_entity)
        .entry::<BlastFireFx>()
        .and_modify(move |mut fx| {
            fx.light_entity = Some(light_entity);
        });

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
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut lights: Query<&mut PointLight>,
) {
    let dt = time.delta_secs();
    if dt == 0.0 {
        return;
    }
    let t = time.elapsed_secs();

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

        // Compute the current visual fade.  Legacy `DrawAll`:
        //   fade = STOPPING ? ClampRange(Timer, 0, EndFade)
        //                   : 1 - ClampRange(Timer, 0, StartFade)
        // The non-stopping branch ramps 0→1 as Timer counts down
        // from StartFade to 0; the stopping branch ramps 1→0 as
        // Timer counts down from EndFade to 0.
        let fade = if fx.stopping {
            if fx.end_fade > 0.0 {
                (fx.timer / fx.end_fade).clamp(0.0, 1.0)
            } else {
                0.0
            }
        } else if fx.timer > 0.0 && fx.start_fade > 0.0 {
            (1.0 - (fx.timer / fx.start_fade)).clamp(0.0, 1.0)
        } else {
            1.0
        };

        // Apply fade to every per-instance StandardMaterial alpha.
        // The shared env-reflect handles are deliberately not in
        // `instance_materials`; their additive shader has no tint
        // uniform and they fade indirectly via the underlying
        // diffuse pass going dark.
        for h in &fx.instance_materials {
            if let Some(mat) = materials.get_mut(h) {
                let mut col = mat.base_color.to_srgba();
                col.alpha = fade;
                mat.base_color = col.into();
            }
        }

        // Drive the point light: dual-sine flicker × fade ×
        // base intensity.  Legacy formula
        //   I = 100 + 900*(0.5 + 0.25*(sin(0.75·2π·t) + sin(1.5·2π·t)))
        // ranges in [100, 1000] AGE units; we keep the same shape
        // and apply Bevy-scale base intensity for visibility against
        // Bevy's physical lighting.  `fade` smoothly attenuates the
        // flicker across the start/end ramps so the light doesn't
        // pop on/off.
        const BLAST_LIGHT_BASE_INTENSITY: f32 = 80_000.0; // lumens
        if let Some(light_entity) = fx.light_entity
            && let Ok(mut light) = lights.get_mut(light_entity)
        {
            let two_pi = std::f32::consts::TAU;
            let sine_sum = (0.75 * two_pi * t).sin() + (1.5 * two_pi * t).sin();
            let modulator = (0.5 + 0.25 * sine_sum).clamp(0.1, 1.0);
            light.intensity = fade * modulator * BLAST_LIGHT_BASE_INTENSITY;
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

// ---------------------------------------------------------------------------
// Screen-wash "fade to white"
// ---------------------------------------------------------------------------
//
// Simplified port of the legacy `fxCopyToFront` screen wash that
// pulses the framebuffer during a blast.  The C++ engine does a full
// render-target ping-pong (`copytofront.cpp`) with a multiplicative
// blit + optional additive pass — keyed off per-frame brightness /
// attenuation ramps in `fxBlastFireType::DrawAll`.  Here we use a
// single fullscreen UI overlay whose alpha tracks one scalar
// intensity, fired bright on blast spawn and ramped back to zero.
// Coarser than the legacy effect (no warm-tint blit, no additive
// glow), but covers the cinematic "detonation flash" the user sees
// and avoids hooking a post-processing pass.

/// Per-frame intensity of the fullscreen white wash.  `1.0` = fully
/// opaque white, `0.0` = invisible.  Driven by
/// `trigger_blast_flash_on_spawn_system` and ramped down by
/// `update_blast_flash_system`.
#[derive(Resource, Default)]
pub struct BlastScreenFlash {
    pub intensity: f32,
    /// Linear decay rate (units / second).  Tuned so a peak-1.0 flash
    /// fades in roughly the duration of the blast's `StartFade`
    /// window (~0.5s).  Legacy `ColorFade.w` is set per-frame from
    /// the camera-distance brightness ramp; this is a coarser, time-
    /// based equivalent that matches the visible feel without
    /// porting the full ping-pong pipeline.
    pub fade_per_sec: f32,
}

#[derive(Component)]
pub struct BlastFlashOverlay;

/// Lazily spawn the fullscreen overlay node on the first frame that
/// the flash intensity goes non-zero.  Spawning at `Startup` runs
/// before the main camera/window are guaranteed to exist, which on
/// Bevy 0.18 can leave the render pipeline in a bad state (a full-
/// screen Node sized in `Val::Percent(100.0)` against a zero-sized
/// viewport tripped panics in `prepare_view_uniforms` and friends).
/// Deferring to the first triggered frame guarantees the camera is
/// up and the UI graph has a valid target.
pub fn ensure_blast_flash_overlay_system(
    mut commands: Commands,
    flash: Res<BlastScreenFlash>,
    existing: Query<(), With<BlastFlashOverlay>>,
) {
    if flash.intensity <= 0.0 {
        return;
    }
    if !existing.is_empty() {
        return;
    }
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            left: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.0)),
        // High but bounded — `i32::MAX` overflows some UI math paths.
        GlobalZIndex(10_000),
        BlastFlashOverlay,
        // Despawn with the layout so a flash that was mid-fade when
        // the user quits to menu doesn't bleed into the next layout
        // load.  `reset_blast_flash_on_exit` resets the resource on
        // the same edge so the overlay's re-spawn will start clean.
        crate::menu::InGameEntity,
    ));
}

/// Zero the flash resource when the user leaves InGame.  Without
/// this, an in-progress fade would resume immediately on the next
/// layout load, looking like the new layout already had a blast
/// going off.  The overlay entity itself is despawned by
/// `cleanup_game` via the `InGameEntity` tag above; this system
/// just clears the driver state.
pub fn reset_blast_flash_on_exit(mut flash: ResMut<BlastScreenFlash>) {
    flash.intensity = 0.0;
    flash.fade_per_sec = 0.0;
}

/// Decay the flash intensity each frame and mirror onto the
/// overlay's BackgroundColor alpha.  Mirrors the legacy per-frame
/// `ColorFade` write — but as a single linear ramp rather than the
/// brightness-keyed per-blast formula in DrawAll.
pub fn update_blast_flash_system(
    time: Res<Time>,
    mut flash: ResMut<BlastScreenFlash>,
    mut overlay: Query<&mut BackgroundColor, With<BlastFlashOverlay>>,
) {
    if flash.intensity > 0.0 {
        flash.intensity = (flash.intensity - flash.fade_per_sec * time.delta_secs()).max(0.0);
    }
    for mut bg in &mut overlay {
        bg.0 = Color::srgba(1.0, 1.0, 1.0, flash.intensity);
    }
}

/// Bump the screen flash to full whenever a new `BlastFireFx` enters
/// the world.  `Added<BlastFireFx>` is the spawn-edge filter — fires
/// exactly once per blast instance, regardless of who called
/// `spawn_blast_fire` or whether multiple blasts collide on the same
/// tick.  Multiple flashes the same frame just re-saturate
/// `intensity` (clamped at 1.0).
pub fn trigger_blast_flash_on_spawn_system(
    spawned: Query<(), Added<BlastFireFx>>,
    mut flash: ResMut<BlastScreenFlash>,
) {
    if spawned.iter().next().is_some() {
        flash.intensity = 1.0;
        // 0.5s linear fade matches the legacy `StartFade` default.
        flash.fade_per_sec = 2.0;
    }
}
