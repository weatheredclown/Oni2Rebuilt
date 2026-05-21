/*
 * oni2_loader/light_glow.rs — port of legacy `fxLightGlow`.
 *
 * Spawns a billboarded additive sprite at each fxLight's world position
 * and updates it per-frame for camera facing, optional occlusion fade,
 * and optional radial attenuation.  Mirrors `fxLightGlow::DrawLightGlow`
 * — the legacy renders one fullscreen-quad per
 * light with these conditional behaviors:
 *
 *   1. Build a quad transform of size `currentIntensity` centered at
 *      the light's position.
 *   2. Orient the quad: kGlowBillboard → cylindrical (face-camera
 *      around world-Y); else kGlowLookAt → camera-matrix orientation.
 *   3. If kOccludeGlow: ray-test from light to camera; on hit, ramp
 *      `currentIntensity` down by `intensityRateOfChange × intensity`,
 *      else ramp up.  Quad alpha = currentIntensity / intensity.
 *   4. If kRadialAttenuate: scale glowsize by
 *      `clamp(dot(-light_dir, lightToCam), 0..1)` — fades when camera
 *      moves out of the cone.
 *   5. Draw with additive (SrcAlpha, One) blend; Z-test off when
 *      occluding (so the fading glow renders over occluders).
 *
 * Bevy implementation:
 *   - The quad mesh is a unit `Rectangle::new(1.0, 1.0)` cached as a
 *     resource (one shared mesh; per-instance scale via Transform).
 *   - Material is `StandardMaterial { unlit: true, alpha_mode: Add,
 *     base_color_texture: glow.handle, base_color: glow.color }` —
 *     additive blend matches legacy SrcAlpha_One.
 *   - `LightGlow` component carries the per-instance state
 *     (intensity, current_intensity, flag bits, optional dir for
 *     radial attenuate).
 *   - One per-frame system (`update_light_glow_billboards`) reads
 *     the active player camera and applies all four behaviors above.
 */
use crate::oni2_loader::parsers::effect::{EffectDef, LightGlowDef};
use crate::oni2_loader::registries::FxLibrary;
use bevy::prelude::*;

/// Tag a freshly-spawned light with this to request that a glow
/// corona be attached to it.  A separate system
/// (`resolve_pending_glow_system`) consumes the tag, looks up the
/// `LightGlowDef` in `FxLibrary` by name, and spawns the corona as
/// a child of the light.  This decouples light-spawning (which
/// happens deep inside `load_layout_lights`) from the asset lookup
/// (which needs `Res<FxLibrary>` + `ResMut<Assets<Mesh>>` etc., not
/// available at that point in the call graph).
#[derive(Component)]
pub struct PendingGlow {
    /// Name from `LightGlowType` in `layout.lights` — case-folded
    /// when looking up against `FxLibrary` (FxLibrary keys are
    /// lowercased on insert).
    pub glow_type_name: String,
    /// `GlowIntensityScale` from `layout.lights`; multiplied with
    /// the def's own `glow_intensity` per the legacy chain
    /// chain.
    pub glow_intensity_scale: f32,
    /// Spot lights' forward direction in Bevy space, used by the
    /// `RadialAttenuate` flag.  Pass `Vec3::ZERO` for non-spot
    /// lights — radial attenuation falls back to fully lit.
    pub light_dir: Vec3,
    /// Parent light's authored Color from `layout.lights`.  Legacy
    /// `fxLight::DrawGlow` passes `m_Color` into the glow draw and
    /// it modulates the texture.  The def's
    /// own `GlowColor` is only used by standalone fxEffects (not
    /// fxLight-attached glows) — using it here would tint
    /// LightConeDown bright magenta because the def authors
    /// `GlowColor 1 0 1` as a placeholder default.
    pub light_color: Color,
}

/// Per-instance state for a spawned glow corona.  Lives on the same
/// entity as the billboard quad's `Mesh3d` + `MeshMaterial3d`.  Built
/// from a `LightGlowDef` + the parent fxLight's `intensity` scalar
/// and (for radial-attenuate spots) `direction`.
///
/// The per-frame max-intensity is recomputed from the **parent light's
/// live intensity** rather than captured at spawn (legacy
/// `fxLight::DrawGlow` passes
/// `m_Intensity * m_GlowIntensityScale` every frame, which is `glowsize`
/// directly — the def's `GlowIntensity` is *not* multiplied in for
/// fxLight glows; it's the standalone-effect default).  This is what
/// makes ScrOni `intensity (multiplier*52)` ramps drive shaft size —
/// without it, the shaft is locked to whatever the layout-authored
/// intensity was at level load.
#[derive(Component)]
pub struct LightGlow {
    /// Per-Light `GlowIntensityScale` (typically 0.1).  Constant after
    /// spawn — this is layout-authored, not script-driven.  Multiplied
    /// each frame with the parent's live intensity to give the live
    /// `max_intensity`, matching legacy `m_Intensity * m_GlowIntensityScale`
    /// each frame in the legacy draw path.
    pub intensity_scale: f32,
    /// `glow_intensity_rate_of_change` from the `LightGlowDef`.
    /// Multiplied each frame with the live `max_intensity` to give the
    /// occlusion ramp rate (matches legacy `intensityRateChange =
    /// intensityRateOfChange * lightIntensity`).
    pub rate_of_change_factor: f32,
    /// Current intensity, ramped toward live `max_intensity` (visible)
    /// or 0 (occluded) per frame.
    pub current_intensity: f32,
    /// World-space forward direction of the parent light (for
    /// `radial_attenuate`).  None when the parent light isn't a spot
    /// or the flag is off.
    pub light_dir: Option<Vec3>,
    /// Bit-packed flags from `LightGlowDef`.  We carry the whole set
    /// rather than four bools so it's a single component access in
    /// the per-frame system.
    pub flags: GlowFlags,
    /// Per-instance handle to the material we own — needed so the
    /// per-frame system can write `base_color.alpha` for the
    /// occlusion fade without affecting any other glow that happens
    /// to share the same `LightGlowDef`.
    pub material: Handle<StandardMaterial>,
}

/// Behavior flags from the legacy `fxLightGlow` Flags bitfield, kept
/// as discrete bools so we don't drag in the `bitflags` crate just
/// for five booleans.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlowFlags {
    pub look_at: bool,
    pub billboard: bool,
    pub occlude: bool,
    pub radial_attenuate: bool,
    pub screen_rotate: bool,
}

impl GlowFlags {
    pub fn from_def(def: &LightGlowDef) -> Self {
        Self {
            look_at: def.glow_look_at != 0,
            billboard: def.glow_billboard != 0,
            occlude: def.occlude_glow != 0,
            radial_attenuate: def.radial_attenuate != 0,
            screen_rotate: def.screen_rotate != 0,
        }
    }
}

/// Cached unit-quad mesh, shared by every spawned glow.  Built once
/// at first spawn and reused thereafter.
#[derive(Resource, Default)]
pub struct LightGlowMesh(pub Option<Handle<Mesh>>);

/// Build (or fetch) the shared unit-quad mesh.
fn get_or_create_quad_mesh(cached: &mut LightGlowMesh, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
    if let Some(h) = &cached.0 {
        return h.clone();
    }
    // 1×1 quad in XY plane, normal +Z — billboard system rotates
    // this entity each frame so the +Z faces the camera.
    let h = meshes.add(Rectangle::new(1.0, 1.0));
    cached.0 = Some(h.clone());
    h
}

/// Spawn a glow corona at `world_pos` for the given def, and parent
/// it to `parent_entity` (typically the layout-light entity itself).
/// `intensity_scale` is the per-Light `GlowIntensityScale` value
/// from `layout.lights`.  The size formula matches legacy
/// `fxLight::DrawGlow`: `glowsize = parent.intensity *
/// intensity_scale` per frame — the def's `GlowIntensity` is *not*
/// multiplied in for fxLight-attached glows (that field is the default
/// intensity for standalone fx-effect glows).  `light_color` is the
/// parent light's authored Color, used to modulate the texture
/// — *not* `def.glow_color`, which is a
/// placeholder default and is bright magenta for several defs.
pub fn spawn_light_glow(
    commands: &mut Commands,
    glow_mesh: &mut LightGlowMesh,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    parent_entity: Entity,
    world_pos: Vec3,
    light_dir: Vec3,
    intensity_scale: f32,
    light_color: Color,
    name: &str,
    def: &LightGlowDef,
) {
    let Some(tex_handle) = def.glow_texture_handle.clone() else {
        // Texture missing — skip silently rather than spawning an
        // untextured magenta quad (which would look broken).
        return;
    };
    // Note: we don't gate on intensity_scale > 0 here.  A script-driven
    // light (e.g. trash-chute door) authors the parent at intensity 0
    // and ramps it up at runtime; the glow needs to exist beforehand so
    // it's ready when the parent intensity rises.  The per-frame update
    // culls invisible glows (`glowsize < 0.1`).
    let mesh = get_or_create_quad_mesh(glow_mesh, meshes);
    let flags = GlowFlags::from_def(def);

    // Material: additive, unlit (the texture IS the light).  Each
    // instance gets its own material handle so the per-frame system
    // can write its alpha for the occlusion fade independently.
    // `cull_mode: None` so the billboard reads from both sides — even
    // with the cylindrical/look_at billboarding, momentary frame-edge
    // cases (camera passing through the quad plane) shouldn't blink the
    // shaft off.
    let material = materials.add(StandardMaterial {
        base_color: light_color.with_alpha(1.0),
        base_color_texture: Some(tex_handle),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        cull_mode: None,
        // The corona shouldn't write to depth or cast shadows — it's
        // a screen-space sprite, not geometry.
        depth_bias: 0.0,
        ..default()
    });

    commands.spawn((
        Name::new(format!("LightGlow:{}", name)),
        Mesh3d(mesh),
        MeshMaterial3d(material.clone()),
        // Spawn at zero scale; the per-frame system rescales from the
        // parent's live intensity on its first tick.
        Transform::from_translation(world_pos).with_scale(Vec3::ZERO),
        Visibility::Visible,
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
        ChildOf(parent_entity),
        LightGlow {
            intensity_scale,
            rate_of_change_factor: def.glow_intensity_rate_of_change.max(0.0),
            current_intensity: 0.0,
            light_dir: if flags.radial_attenuate {
                Some(light_dir.normalize_or_zero())
            } else {
                None
            },
            flags,
            material,
        },
    ));
}

/// Resolve `PendingGlow` tags into actual glow children.  Looks up
/// the `LightGlowDef` in `FxLibrary` by lowercased name, then defers
/// to `spawn_light_glow`.  Removes the `PendingGlow` tag after
/// processing (success OR failure — a missing def is a permanent
/// failure, not retryable).
pub fn resolve_pending_glow_system(
    mut commands: Commands,
    mut glow_mesh: ResMut<LightGlowMesh>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    fx_library: Res<FxLibrary>,
    pending: Query<(Entity, &PendingGlow, &GlobalTransform, Option<&Name>)>,
) {
    for (entity, glow, gtf, name_opt) in &pending {
        let name_str = name_opt.map(|n| n.as_str()).unwrap_or("unnamed");
        let key = glow.glow_type_name.to_lowercase();
        match fx_library.effects.get(&key) {
            Some(EffectDef::LightGlow(def)) => {
                spawn_light_glow(
                    &mut commands,
                    &mut glow_mesh,
                    &mut meshes,
                    &mut materials,
                    entity,
                    gtf.translation(),
                    glow.light_dir,
                    glow.glow_intensity_scale,
                    glow.light_color,
                    name_str,
                    def,
                );
            }
            Some(_other) => {
                warn!(
                    "fxLight glow '{}' (on light '{}'): rb.fx entry exists but isn't a LIGHTGLOW effect",
                    glow.glow_type_name, name_str
                );
            }
            None => {
                warn!(
                    "fxLight glow '{}' (on light '{}'): not found in rb.fx",
                    glow.glow_type_name, name_str
                );
            }
        }
        commands.entity(entity).remove::<PendingGlow>();
    }
}

/// Per-frame: orient each glow toward the camera, run occlusion ramp
/// + radial attenuation, scale the quad to its current intensity.
///
/// Mirrors the legacy chain `fxLight::DrawGlow` →
/// `fxLightGlow::DrawInstancedGlow` → `DrawLightGlow`
/// in the legacy draw path.  The crucial detail:
/// `lightIntensity` in `DrawLightGlow` is `m_Intensity * m_GlowIntensityScale`
/// where `m_Intensity` is the parent light's **live** intensity, not a
/// cached spawn-time value.  We replicate that by querying the parent
/// PointLight/SpotLight every frame via the `ChildOf` relation.
pub fn update_light_glow_billboards(
    time: Res<Time>,
    camera_q: Query<&GlobalTransform, With<bevy::camera::Camera>>,
    mut glows: Query<(&mut Transform, &GlobalTransform, &mut LightGlow, &ChildOf)>,
    parent_lights: Query<(Option<&PointLight>, Option<&SpotLight>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    spatial: avian3d::prelude::SpatialQuery,
) {
    // Use whichever active camera is present.  In-game we have a
    // single Camera3d; in the frontend menu we have FrontendMenuCamera
    // (a Camera3d).  Picking the first one is fine.
    let Some(cam_xf) = camera_q.iter().next() else {
        return;
    };
    let cam_pos = cam_xf.translation();
    let dt = time.delta_secs();

    for (mut tf, gtf, mut glow, child_of) in &mut glows {
        let glow_pos = gtf.translation();
        let to_cam = cam_pos - glow_pos;
        let to_cam_dist = to_cam.length();
        if to_cam_dist < 1e-4 {
            // Camera right on top of the glow — nothing meaningful to draw.
            continue;
        }

        // Recover the parent light's live, legacy-unitless intensity.
        // Spawn-time conversion was `lumens = legacy * POINT_INTENSITY_TO_CANDELA`,
        // so we invert that to get a comparable scalar back.  If the
        // parent has somehow been despawned or stripped of its light
        // component, treat as off (max_intensity = 0).
        let parent_legacy_intensity = parent_lights
            .get(child_of.parent())
            .ok()
            .and_then(|(pt, sp)| pt.map(|p| p.intensity).or_else(|| sp.map(|s| s.intensity)))
            .map(|cd| cd / crate::oni2_loader::layout_loader::POINT_INTENSITY_TO_CANDELA)
            .unwrap_or(0.0);
        // Legacy `fxLight::DrawGlow` passes
        // `m_Intensity * m_GlowIntensityScale` straight in as
        // `lightIntensity`, and `glowsize = lightIntensity` directly
        // directly.  The def's `GlowIntensity` field is only consumed
        // by the standalone fx-effect path (legacy `DrawAll`).
        // Keep them parallel here.
        let max_intensity = parent_legacy_intensity * glow.intensity_scale;
        let rate_per_sec = glow.rate_of_change_factor * max_intensity;

        // 1. Occlusion fade (kOccludeGlow).  Cast a ray glow → camera.
        //    If anything blocks, ramp current_intensity DOWN; else UP.
        if glow.flags.occlude {
            let dir = to_cam / to_cam_dist;
            let blocked = Dir3::new(dir)
                .ok()
                .and_then(|d| {
                    spatial.cast_ray(
                        glow_pos,
                        d,
                        to_cam_dist,
                        true,
                        &avian3d::prelude::SpatialQueryFilter::default(),
                    )
                })
                .is_some();
            let target = if blocked { 0.0 } else { max_intensity };
            let max_step = rate_per_sec * dt;
            if max_step > 0.0 {
                let delta = (target - glow.current_intensity).clamp(-max_step, max_step);
                glow.current_intensity = (glow.current_intensity + delta).clamp(0.0, max_intensity);
            } else {
                // Rate-of-change of zero → snap (matches legacy when
                // intensityRateChange == 0).
                glow.current_intensity = target;
            }
        } else {
            // No occlusion → track the live max each frame.  This is
            // what makes ScrOni intensity ramps drive shaft size — the
            // BCTrashChamberB08 trash-chute light is the canonical case.
            glow.current_intensity = max_intensity;
        }

        let mut glowsize = glow.current_intensity;

        // 2. Radial attenuation (kRadialAttenuate) — fades as the
        //    camera moves outside the spot's forward cone.  Uses
        //    `dot(-light_dir, lightToCam)` per legacy.
        if let Some(light_dir) = glow.light_dir {
            let lc = (cam_pos - glow_pos).normalize_or_zero();
            let cone_factor = (-light_dir).dot(lc).clamp(0.0, 1.0);
            glowsize *= cone_factor;
        }

        // Cull tiny glows (legacy: `glowsize < 0.1f` early-return).
        if glowsize < 0.1 {
            // Hide by zeroing scale rather than mutating Visibility —
            // saves a component churn on the next visible frame.
            tf.scale = Vec3::ZERO;
            continue;
        }

        // 3. Orient the quad.  kGlowBillboard takes priority — it's
        //    the "Y-axis billboard" used for ceiling cones; otherwise
        //    full kGlowLookAt camera-matrix billboard.  Default (no
        //    flags) keeps the quad's spawn rotation.
        if glow.flags.billboard {
            // Cylindrical: face camera in XZ plane, world-up stays Y.
            let mut to_cam_flat = cam_pos - glow_pos;
            to_cam_flat.y = 0.0;
            if to_cam_flat.length_squared() > 1e-6 {
                tf.rotation = Quat::from_rotation_arc(Vec3::Z, to_cam_flat.normalize());
            }
        } else if glow.flags.look_at {
            // Full screen-space billboard: copy camera rotation so
            // the quad's +Z aligns with the camera's view direction.
            tf.rotation = cam_xf.compute_transform().rotation;
        }

        // 4. Apply scale (quad spans ±glowsize → side length 2 ×
        //    glowsize).  Material alpha mirrors the fade fraction so
        //    occlusion ramps both size AND opacity.
        let alpha = if max_intensity > 0.0 {
            glow.current_intensity / max_intensity
        } else {
            0.0
        };
        tf.scale = Vec3::splat(glowsize * 2.0);
        if let Some(mat) = materials.get_mut(&glow.material) {
            mat.base_color = mat.base_color.with_alpha(alpha);
        }
    }
}
