/*
 * fx_visuals.rs — billboarded-quad visual fx ports.
 *
 * Houses spawners + per-frame tickers for the fx-effect-types that
 * render as textured billboard sprites:
 *   • Flash  — a popping additive sprite that scales up while fading
 *              out (legacy fxFlashType, rb/src/fx/flash.cpp).  Used
 *              for explosion blowouts, weapon-impact pops, and other
 *              one-shot bursts.
 *   • Sprite — a textured billboard that lingers for a duration
 *              (legacy fxSpriteEffect, rb/src/fx/sprite.cpp).  Used
 *              for muzzle flashes, charge-up halos, and other
 *              continuous-while-controlled overlays.
 *
 * Both are dispatched from fx_system::handle_spawn_fx via the SpawnFx
 * event pipeline.  Each spawn produces one entity with a unit-quad
 * mesh + per-instance StandardMaterial (additive blend, unlit) +
 * lifetime component.  The per-frame system updates Transform
 * (billboard + scale), material alpha, and despawns expired effects.
 *
 * Quad mesh + a default Flash texture (legacy hardcoded "daisyalpha")
 * are cached as resources so spawn-time is asset-free.
 */
use crate::oni2_loader::parsers::effect::{FlashDef, SpriteEffectDef, StrikeFxDef};
use bevy::prelude::*;

pub struct FxVisualsPlugin;

impl Plugin for FxVisualsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FxVisualMesh>()
            .init_resource::<FxFlashTexture>()
            .init_resource::<FxStrikeMeshCache>()
            .add_systems(
                Update,
                (
                    update_flash_fx,
                    update_sprite_fx,
                    update_strike_fx,
                )
                    .run_if(in_state(crate::menu::AppState::InGame)),
            );
    }
}

// --- Cached shared assets ----------------------------------------------------

/// Unit quad shared by every Flash/Sprite instance.  Per-instance
/// scale lives on `Transform`, per-instance color on the material.
#[derive(Resource, Default)]
pub struct FxVisualMesh(Option<Handle<Mesh>>);

fn get_or_create_quad(cache: &mut FxVisualMesh, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
    if let Some(h) = &cache.0 {
        return h.clone();
    }
    let h = meshes.add(Rectangle::new(1.0, 1.0));
    cache.0 = Some(h.clone());
    h
}

/// Lazily-loaded `daisyalpha.tga` — the legacy hard-coded Flash texture
/// (rb/src/fx/flash.cpp:136).  None until first Flash spawn.
#[derive(Resource, Default)]
pub struct FxFlashTexture(Option<Handle<Image>>);

fn get_or_load_flash_texture(
    cache: &mut FxFlashTexture,
    images: &mut Assets<Image>,
) -> Option<Handle<Image>> {
    if let Some(h) = &cache.0 {
        return Some(h.clone());
    }
    let (h, _) = crate::oni2_loader::parsers::texture::load_tga_file(
        "texture",
        "daisyalpha.tga",
        images,
    )?;
    cache.0 = Some(h.clone());
    Some(h)
}

#[derive(Resource, Default)]
pub struct FxStrikeMeshCache(pub std::collections::HashMap<String, Handle<Mesh>>);

fn clamp_range(val: f32, min: f32, max: f32) -> f32 {
    if val <= min { return 0.0; }
    if val >= max { return 1.0; }
    (val - min) / (max - min)
}

fn create_strike_mesh(def: &StrikeFxDef) -> Mesh {
    let res_x = 8;
    let res_y = 35;
    
    use rand::Rng;
    let mut rng = rand::rng();
    let mut waves = Vec::with_capacity(def.wave_count as usize);
    for i in 0..def.wave_count {
        waves.push((
            0.5f32,
            0.0f32,
            rng.random_range(0.0..std::f32::consts::TAU),
            (i + 1) as f32,
        ));
    }
    
    let mut positions = Vec::with_capacity(res_x * res_y);
    let mut uvs = Vec::with_capacity(res_x * res_y);
    let mut colors = Vec::with_capacity(res_x * res_y);
    let mut normals = Vec::with_capacity(res_x * res_y);
    
    for y in 0..res_y {
        let ty = clamp_range(y as f32, 0.0, (res_y - 1) as f32);
        
        let mut sum = 1.0;
        for (amp, offset, time, freq) in &waves {
            sum += (ty * freq + time).sin() * amp + offset;
        }
        
        for x in 0..res_x {
            let tx = clamp_range(x as f32, 0.0, (res_x - 1) as f32);
            
            let angle = (1.0 - ty) * std::f32::consts::TAU;
            let mut p = Vec3::new(tx * angle.cos(), tx * angle.sin(), 0.0);
            
            let radial = tx.powf(def.exponent);
            let mut scale = radial + (1.0 - radial) * sum;
            if y % 2 != 0 {
                scale *= 1.0 + def.spike_scale * radial;
            }
            p *= scale;
            
            positions.push([p.x, p.y, p.z]);
            
            let mut u = ty * def.tile.x;
            let mut v = tx * def.tile.y;
            // Interior-row UV jitter.  `random_range` panics on an empty
            // range, so guard against `jumble == 0` — common in shipped
            // .fxl entries (e.g. `Punches_Body_Hard` has `Jumble 0`).
            // Legacy `RangeRand(0,0)` just returned 0 in that case.
            if y > 0 && y < res_y - 1 && def.jumble > 0.0 {
                u += rng.random_range(-def.jumble..def.jumble);
                v += rng.random_range(-def.jumble..def.jumble);
            }
            uvs.push([u, v]);
            normals.push([0.0, 0.0, 1.0]);
            
            let alpha = 1.0 - clamp_range(tx, def.fade_start, 1.0);
            colors.push([1.0, 1.0, 1.0, alpha]);
        }
    }
    
    let mut indices = Vec::new();
    for y in 0..(res_y - 1) {
        for x in 0..(res_x - 1) {
            let i0 = (y * res_x + x) as u32;
            let i1 = (y * res_x + x + 1) as u32;
            let i2 = ((y + 1) * res_x + x) as u32;
            let i3 = ((y + 1) * res_x + x + 1) as u32;
            
            indices.push(i0);
            indices.push(i2);
            indices.push(i1);
            
            indices.push(i1);
            indices.push(i2);
            indices.push(i3);
        }
    }
    
    let mut mesh = Mesh::new(
        bevy::render::render_resource::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default()
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(bevy::mesh::Indices::U32(indices));
    
    mesh
}

// --- Flash --------------------------------------------------------------------

/// Per-instance Flash state.  Mirrors the `fxFlashInst` lifecycle from
/// `fxFlashType::UpdateAll` (rb/src/fx/flash.cpp:194):
///
///   • `current_scale` Approach()s `goal_scale` at `rate` per second.
///   • `alpha = clamp(fade * (goal_scale - current_scale), 0..1)`
///   • Effect dies when alpha hits 0 (current_scale reaches goal).
///
/// Bevy quad scale = `current_scale × 2` (the legacy quad spans
/// ±scale; our unit quad spans ±0.5).  Material alpha tracks `alpha`.
#[derive(Component)]
pub struct FlashFx {
    pub current_scale: f32,
    pub goal_scale: f32,
    pub rate: f32,
    pub fade: f32,
    /// Owned material handle so we can mutate alpha each frame
    /// independently of any other Flash sharing the same FlashDef.
    pub material: Handle<StandardMaterial>,
}

/// Spawn a Flash burst at `world_pos`, optionally parented to `parent`.
/// Picks `goal_scale` randomly in `[scale_min, scale_max]` per
/// legacy `RangeRand(ScaleMin, ScaleMax)` (flash.cpp:217).
pub fn spawn_flash(
    commands: &mut Commands,
    quad_cache: &mut FxVisualMesh,
    flash_tex_cache: &mut FxFlashTexture,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    def: &FlashDef,
    world_pos: Vec3,
    parent: Option<Entity>,
) {
    let Some(tex) = get_or_load_flash_texture(flash_tex_cache, images) else {
        // No texture → silent skip rather than rendering an untextured
        // magenta quad.  Reported once per missing-asset session via
        // load_tga_file's own warning path.
        return;
    };
    let mesh = get_or_create_quad(quad_cache, meshes);

    let goal = if def.scale_max > def.scale_min {
        use rand::Rng;
        let mut rng = rand::rng();
        rng.random_range(def.scale_min..=def.scale_max)
    } else {
        def.scale_min
    };
    // Random Z rotation, mirrors the per-instance orientation in
    // legacy (`flash.cpp:218 — frand()*2*PI` with the 2D `Rotation`
    // basis).  Keeps stacked flashes from looking identical.
    use rand::Rng;
    let theta = rand::rng().random_range(0.0..std::f32::consts::TAU);

    // Color1 is the primary tint; Color2/3 in the def are cycle
    // colors used by the legacy palette animator (flash.cpp:365+).
    // Static Color1 covers the visible behavior for now — a future
    // pass can interpolate between the three.
    let material = materials.add(StandardMaterial {
        base_color: def.color1.with_alpha(1.0),
        base_color_texture: Some(tex),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        ..default()
    });

    let mut transform = Transform::from_translation(world_pos)
        .with_rotation(Quat::from_rotation_z(theta))
        .with_scale(Vec3::splat(0.001));
    transform.scale = Vec3::splat(0.001); // start tiny, grow

    let mut ec = commands.spawn((
        Name::new(format!("FlashFx:{}", def.name)),
        Mesh3d(mesh),
        MeshMaterial3d(material.clone()),
        transform,
        Visibility::Visible,
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
        FlashFx {
            current_scale: 0.0,
            goal_scale: goal,
            rate: def.rate.max(0.001),
            fade: def.fade.max(0.001),
            material,
        },
        // Cleanup-on-layout-exit.  Even when parented (which only
        // happens when ev.parent is set), tag for safety — the
        // parent cascade catches it then, and parentless flashes
        // need this to not leak across layouts.
        crate::menu::InGameEntity,
    ));
    if let Some(p) = parent {
        ec.insert(ChildOf(p));
    }
}

/// Per-frame Flash tick.  Approach scale toward goal at `rate` per
/// second; alpha = `clamp(fade * (goal - current), 0..1)`.  Despawn
/// when alpha hits 0 (or goal is reached).  Camera-billboard each
/// surviving instance to face the active camera.
fn update_flash_fx(
    mut commands: Commands,
    time: Res<Time>,
    mut flashes: Query<(Entity, &mut Transform, &mut FlashFx)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
) {
    let dt = time.delta_secs();
    let cam_pos = camera.iter().next().map(|t| t.translation());
    for (entity, mut tf, mut flash) in &mut flashes {
        // Approach (legacy `Approach(scale, goal, rate, dt)`):
        // monotonic step toward the goal, clamped so we don't overshoot.
        let step = flash.rate * dt;
        let remaining = flash.goal_scale - flash.current_scale;
        if remaining.abs() <= step {
            flash.current_scale = flash.goal_scale;
        } else {
            flash.current_scale += step * remaining.signum();
        }

        let alpha = (flash.fade * (flash.goal_scale - flash.current_scale))
            .clamp(0.0, 1.0);
        if alpha <= 1e-3 {
            commands.entity(entity).despawn();
            continue;
        }

        // Material alpha + Transform scale (×2 because the unit quad
        // spans 1.0 not 2.0).
        if let Some(mat) = materials.get_mut(&flash.material) {
            mat.base_color = mat.base_color.with_alpha(alpha);
        }
        let world_size = flash.current_scale * 2.0;
        // Preserve the per-instance Z rotation set at spawn — only
        // re-orient if we have a camera to billboard against.
        let z_rot = tf.rotation.to_euler(EulerRot::ZYX).0;
        if let Some(cp) = cam_pos {
            let to_cam = cp - tf.translation;
            if to_cam.length_squared() > 1e-6 {
                tf.rotation = Quat::from_rotation_arc(Vec3::Z, to_cam.normalize())
                    * Quat::from_rotation_z(z_rot);
            }
        }
        tf.scale = Vec3::splat(world_size);
    }
}

// --- Sprite -------------------------------------------------------------------

/// Per-instance Sprite state.  Mirrors `fxSpriteEffect` lifecycle
/// (rb/src/fx/sprite.cpp:347): sits at its position for `duration`
/// seconds (or indefinitely if `duration == 0` and the spawner stays
/// controlled), then despawns.  Camera-billboarded each frame.
#[derive(Component)]
pub struct SpriteFx {
    /// Time remaining before despawn.  When `duration == 0` in the
    /// def, we use a small fallback (1 frame) so something visible
    /// happens — legacy "controlled forever" fx isn't expressible
    /// without the parent-tracking that the FxEffect controller used.
    pub time_remaining: f32,
    /// World-space size (the quad's side length when displayed).
    /// Equals `particle_size`; constant per legacy.
    pub size: f32,
    /// `1` = legacy "Y-axis screen-aligned" (cylindrical billboard);
    /// `0` (default) = full screen-aligned billboard via camera matrix.
    /// `2` = no rotation, oriented at spawn (rare).
    pub alignment: i32,
}

/// Spawn a Sprite billboard at `world_pos`, optionally parented.
pub fn spawn_sprite(
    commands: &mut Commands,
    quad_cache: &mut FxVisualMesh,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    def: &SpriteEffectDef,
    world_pos: Vec3,
    parent: Option<Entity>,
) {
    let mesh = get_or_create_quad(quad_cache, meshes);
    let size = if def.particle_size > 0.0 {
        def.particle_size
    } else {
        // Legacy default when ParticleSize is 0 — use line_width as a
        // fallback for line-style sprites, else 1.0.
        if def.line_width > 0.0 { def.line_width } else { 1.0 }
    };
    // Legacy `BlendSet 1` = `blendSet_SrcAlpha_One` (additive); other
    // values fall back to alpha blending.  Most muzzle/flare uses are
    // additive in practice, so the typical visible behaviour matches.
    let alpha_mode = if def.blend_set == 1 {
        AlphaMode::Add
    } else {
        AlphaMode::Blend
    };
    let material = materials.add(StandardMaterial {
        base_color: def.color,
        base_color_texture: Some(def.texture.clone()),
        alpha_mode,
        unlit: true,
        ..default()
    });
    // Duration of 0 in the legacy means "stay until controller stops" —
    // since we don't have the FxEffect controller layer, give it a
    // short visible window so the spawn is at least seen.
    let lifetime = if def.duration > 0.0 { def.duration } else { 0.1 };
    let mut ec = commands.spawn((
        Name::new(format!("SpriteFx:{}", def.name)),
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::from_translation(world_pos).with_scale(Vec3::splat(size)),
        Visibility::Visible,
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
        SpriteFx {
            time_remaining: lifetime,
            size,
            alignment: def.alignment,
        },
        crate::menu::InGameEntity,
    ));
    if let Some(p) = parent {
        ec.insert(ChildOf(p));
    }
}

/// Per-frame Sprite tick: countdown, billboard, despawn.
fn update_sprite_fx(
    mut commands: Commands,
    time: Res<Time>,
    mut sprites: Query<(Entity, &mut Transform, &mut SpriteFx)>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
) {
    let dt = time.delta_secs();
    let Some(cam_xf) = camera.iter().next() else {
        return;
    };
    let cam_pos = cam_xf.translation();
    for (entity, mut tf, mut sprite) in &mut sprites {
        sprite.time_remaining -= dt;
        if sprite.time_remaining <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }

        // Billboard.  Alignment 1 (Y-axis only) = cylindrical;
        // anything else (including the common 0) = full camera
        // rotation.  Skip orientation for alignment 2 if it ever
        // shows up in the wild (no .fx files use it currently).
        match sprite.alignment {
            2 => {} // no billboard
            1 => {
                // Cylindrical: face camera in XZ, world-up stays Y.
                let mut to_cam_flat = cam_pos - tf.translation;
                to_cam_flat.y = 0.0;
                if to_cam_flat.length_squared() > 1e-6 {
                    tf.rotation = Quat::from_rotation_arc(Vec3::Z, to_cam_flat.normalize());
                }
            }
            _ => {
                tf.rotation = cam_xf.compute_transform().rotation;
            }
        }
        tf.scale = Vec3::splat(sprite.size);
    }
}

// --- Strike -------------------------------------------------------------------

#[derive(Component)]
pub struct StrikeFx {
    pub timer: f32,
    pub fade_duration: f32,
    pub scale_rate: f32,
    pub current_scale: f32,
    pub slide_rate: Vec2,
    pub current_slide: Vec2,
    pub rotation: f32,
    pub material: Handle<StandardMaterial>,
    /// Health-resolved tint captured at spawn time.  Stored so the per-frame
    /// alpha pre-multiplication in `update_strike_fx` has a stable source
    /// (otherwise reading + re-writing `base_color` would compound-fade).
    pub base_tint: LinearRgba,
}

/// Mirrors `fxStrikeType::UpdatePalette(health)` from rb/src/fx/strike.cpp:363.
/// Legacy builds a full 256-entry palette where each entry is a piecewise
/// lerp through (color1, color2, color3); each of those is itself lerped
/// between the "a" and "b" color sets by `health²`.  We don't repalettize
/// the texture, so we collapse the ramp to the dominant mid color (`color2`)
/// — at full health that's `color2b`, dying it shifts toward `color2a`.
fn resolve_strike_tint(def: &StrikeFxDef, target_health: f32) -> LinearRgba {
    let h = target_health.clamp(0.0, 1.0);
    let h2 = h * h;
    let a = def.color2a.to_linear();
    let b = def.color2b.to_linear();
    LinearRgba::new(
        a.red + h2 * (b.red - a.red),
        a.green + h2 * (b.green - a.green),
        a.blue + h2 * (b.blue - a.blue),
        1.0,
    )
}

pub fn spawn_strike(
    commands: &mut Commands,
    mesh_cache: &mut FxStrikeMeshCache,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    def: &StrikeFxDef,
    world_pos: Vec3,
    target_health: f32,
) {
    info!(
        "spawn_strike: def={} world_pos={:?} health={:.2}",
        def.name, world_pos, target_health
    );

    let mesh_handle = if let Some(h) = mesh_cache.0.get(&def.name) {
        h.clone()
    } else {
        let mesh = create_strike_mesh(def);
        let h = meshes.add(mesh);
        mesh_cache.0.insert(def.name.clone(), h.clone());
        h
    };

    use rand::Rng;
    let mut rng = rand::rng();
    let theta = rng.random_range(0.0..std::f32::consts::TAU);

    let base_tint = resolve_strike_tint(def, target_health);

    let material = materials.add(StandardMaterial {
        base_color: Color::LinearRgba(base_tint),
        base_color_texture: def.texture_handle.clone(),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        cull_mode: None,
        double_sided: true,
        ..default()
    });

    // Stick at the impact point — no parenting (avoids double-applying the
    // parent's world transform on top of our world-space position).
    commands.spawn((
        Name::new(format!("StrikeFx:{}", def.name)),
        Mesh3d(mesh_handle),
        MeshMaterial3d(material.clone()),
        Transform::from_translation(world_pos).with_scale(Vec3::splat(0.001)),
        Visibility::Visible,
        bevy::camera::visibility::NoFrustumCulling,
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
        StrikeFx {
            timer: def.duration,
            fade_duration: def.fade_duration,
            scale_rate: def.scale_rate,
            current_scale: 0.0,
            slide_rate: def.slide_rate,
            current_slide: Vec2::ZERO,
            rotation: theta,
            material,
            base_tint,
        },
        crate::menu::InGameEntity,
    ));
}

fn update_strike_fx(
    mut commands: Commands,
    time: Res<Time>,
    mut strikes: Query<(Entity, &mut Transform, &mut StrikeFx)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
) {
    let dt = time.delta_secs();

    for (entity, mut tf, mut strike) in &mut strikes {
        strike.timer -= dt;
        if strike.timer <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }

        let scale_delta = strike.scale_rate * dt;
        strike.current_scale += scale_delta;
        
        let slide_delta = strike.slide_rate * dt;
        strike.current_slide += slide_delta;

        let alpha = if strike.timer < strike.fade_duration && strike.fade_duration > 0.0 {
            (strike.timer / strike.fade_duration).powi(2)
        } else {
            1.0
        };

        // Bevy's `AlphaMode::Add` doesn't modulate by `base_color.a`, unlike the
        // legacy `blendSet_SrcAlpha_One` (which did `src*src.alpha + dst`).  To
        // get the same effective fade, pre-multiply alpha into RGB each frame
        // from the *stable* `base_tint` (not from `mat.base_color`, which we
        // overwrite below — re-reading it would compound the fade).
        if let Some(mat) = materials.get_mut(&strike.material) {
            let t = strike.base_tint;
            mat.base_color = Color::LinearRgba(LinearRgba::new(
                t.red * alpha,
                t.green * alpha,
                t.blue * alpha,
                1.0,
            ));
        }

        // C++ folds a 0.5 into `Rotation.Set(sinθ*0.5, cosθ*0.5)`, so the
        // effective world stretch is `Scale * 0.5`.  Match that here so the
        // strike's apparent size lines up with the legacy game.
        let world_size = strike.current_scale * 0.5;

        let mut base_rot = Quat::IDENTITY;
        if let Some(cam_tf) = camera.iter().next() {
            base_rot = cam_tf.compute_transform().rotation;
        }

        tf.rotation = base_rot * Quat::from_rotation_z(strike.rotation);
        tf.scale = Vec3::splat(world_size.max(0.001));
    }
}
