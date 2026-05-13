/*
 * laser.rs — projectile laser trail renderer.
 *
 * Ports the legacy `fxLaser`: a ring buffer of recent projectile positions
 * drawn as a camera-aligned ribbon with an additive-blended tail texture,
 * plus a billboarded head sprite at the leading edge and a pulsing point
 * light.  Replaces the solid cylinder fx_system.rs used to spawn — the
 * cylinder looks tube-y at oblique angles; the ribbon looks like a beam
 * because its width axis is `cross(view_dir, beam_dir)`, so it always
 * faces the camera.
 *
 * Lifecycle (mirrors FLAG_CONTROLLED / FLAG_ACTIVE in the C++):
 *   • Controlled: the source projectile still drives the laser — new
 *     positions are pushed at the head each frame.
 *   • Uncontrolled: source is gone or IntendedFxState(false).  Tail
 *     advances one slot per frame until it meets head, then the laser
 *     despawns.  This gives the tail a "retracting" fade.
 */
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::fx_system::IntendedFxState;
use crate::oni2_loader::parsers::effect::LaserFxDef;

pub struct LaserPlugin;

impl Plugin for LaserPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                laser_trail_update,
                laser_ribbon_rebuild.after(laser_trail_update),
                laser_head_billboard.after(laser_ribbon_rebuild),
                laser_light_animate.after(laser_trail_update),
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Ring buffer of recent source positions.  `path[tail..=head]` (wrapping)
/// is the live segment drawn each frame.
#[derive(Component)]
pub struct LaserTrail {
    pub path: Vec<Vec3>,
    pub head: usize,
    pub tail: usize,
    /// Cached previous state — used to detect the "first tick after
    /// becoming controlled" so the initial position seeds the buffer
    /// without producing a zero-length ribbon.
    pub starting: bool,
    /// True while the source is still driving new positions.  Cleared
    /// when the source despawns or IntendedFxState goes false; after
    /// that the tail chases the head and the laser self-despawns when
    /// they meet.
    pub controlled: bool,
    pub width: f32,
    pub head_scale: f32,
    pub light_max: f32,
    /// Entity the laser tracks for position.  Checked for existence each
    /// frame so we can enter fade-out when the projectile despawns.
    pub source: Option<Entity>,
    /// Accumulator so we push exactly one sample per 1/30s regardless
    /// of render framerate.  Legacy ran at 30Hz so `Length=16` in the
    /// .fx meant ~0.53s of trail; at 60fps that same buffer-size would
    /// only span ~0.27s, giving visibly short beams.  Ticking the push
    /// at a fixed 30Hz cadence makes the beam's TIME span match legacy
    /// at any render framerate.
    pub push_accum: f32,
}

/// Samples per second fed into the laser ring buffer.  Mirrors the
/// PS2-era ONI2 framerate so `.fx` `Length` values give the same trail
/// duration at any render framerate.
const TRAIL_PUSH_HZ: f32 = 30.0;
const TRAIL_PUSH_INTERVAL: f32 = 1.0 / TRAIL_PUSH_HZ;

impl LaserTrail {
    pub fn count(&self) -> usize {
        if self.head >= self.tail {
            self.head - self.tail
        } else {
            self.head + (self.path.len() - self.tail)
        }
    }
}

/// Marker for the child entity that draws the billboarded head sprite.
/// Separate from the trail entity because it has its own mesh + material
/// and needs per-frame rotation into the camera plane.
#[derive(Component)]
pub struct LaserHead;

/// Marker for the ribbon entity (the tail-texture tri-strip).  Carries
/// the Mesh3d whose vertex buffer is rebuilt each frame.
#[derive(Component)]
pub struct LaserRibbon;

/// Points from the trail entity to its child light entity so the light
/// ramp system can find it without scanning all children.
#[derive(Component)]
pub struct LaserLightRef(pub Entity);

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

/// Spawn a laser.  Called from fx_system's SpawnFx handler when the
/// resolved EffectDef is Laser.  `source` is the entity whose position
/// the trail follows.  The laser is NOT parented to the source — it has
/// its own lifecycle so it can fade out after the source despawns.
pub fn spawn_laser(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    ld: &LaserFxDef,
    source: Entity,
    at: Vec3,
    name_hint: &str,
) -> Entity {
    // Ribbon: empty mesh now, rebuilt each frame.  Start with a bright
    // opaque emissive material (no texture, no alpha blend) so we can
    // see whether the GEOMETRY is getting to the screen.  Tail texture +
    // additive blend can be layered back on once we confirm visibility.
    let ribbon_mesh = meshes.add(empty_ribbon_mesh());
    let ribbon_mat = materials.add(StandardMaterial {
        base_color: Color::from(ld.light_color.to_linear() * 8.0),
        base_color_texture: ld.tail_texture_handle.clone(),
        // Emissive is ignored when unlit: true, so we bake the HDR multiplier into base_color
        emissive: (ld.light_color.to_linear() * 8.0).into(),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        cull_mode: None,
        double_sided: true,
        ..default()
    });

    // Head sprite: unit quad, scaled via Transform to 0.5*Width*HeadScale.
    let head_quad = meshes.add(Rectangle::new(1.0, 1.0));
    let head_mat = materials.add(StandardMaterial {
        base_color: Color::from(ld.light_color.to_linear() * 3.0),
        base_color_texture: ld.head_texture_handle.clone(),
        emissive: (ld.light_color.to_linear() * 3.0).into(),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        cull_mode: None,
        double_sided: true,
        ..default()
    });

    let trail_entity = commands
        .spawn((
            Name::new(format!("Laser: {}", name_hint)),
            LaserTrail {
                path: vec![Vec3::ZERO; ld.length.max(2)],
                head: 0,
                tail: 0,
                starting: true,
                controlled: true,
                width: ld.width,
                head_scale: ld.head_scale,
                light_max: ld.light_max,
                source: Some(source),
                push_accum: 0.0,
            },
            LaserRibbon,
            Mesh3d(ribbon_mesh),
            MeshMaterial3d(ribbon_mat),
            Transform::from_translation(at),
            Visibility::Visible,
            IntendedFxState(true),
            crate::menu::InGameEntity,
            // Ribbon Aabb is stale the moment the vertex buffer changes
            // (frustum culler reads the Aabb that was valid at mesh-asset
            // creation, not the one matching the current frame's verts).
            // Dynamic trail meshes like this should always opt out of
            // culling — otherwise the beam vanishes the moment the Aabb
            // falls off-screen relative to where the current verts live.
            bevy::camera::visibility::NoFrustumCulling,
            // Beams are light-emitting sprites, not solid geometry — they
            // should never throw a shadow across the scene.
            bevy::light::NotShadowCaster,
        ))
        .id();

    let head_entity = commands
        .spawn((
            Name::new("LaserHead"),
            LaserHead,
            Mesh3d(head_quad),
            MeshMaterial3d(head_mat),
            Transform::from_scale(Vec3::splat(ld.width * ld.head_scale)),
            Visibility::default(),
            ChildOf(trail_entity),
            // Head glow sprite — a light-emitting billboarded quad,
            // not a physical object.  Without this it shadow-projects
            // a rectangle onto nearby surfaces (user-visible as "the
            // projectile head casts a shadow").
            bevy::light::NotShadowCaster,
        ))
        .id();
    let _ = head_entity;

    let light_entity = commands
        .spawn((
            Name::new("LaserLight"),
            PointLight {
                color: ld.light_color,
                intensity: 0.0,
                range: ld.width * 30.0,
                shadows_enabled: false,
                ..default()
            },
            Transform::default(),
            ChildOf(trail_entity),
        ))
        .id();
    commands.entity(trail_entity).insert(LaserLightRef(light_entity));

    trail_entity
}

fn empty_ribbon_mesh() -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, default());
    // Seed with one degenerate triangle so Bevy generates an Aabb.
    // Without an Aabb, the frustum culler drops the mesh entirely even with NoFrustumCulling.
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0, 0.0, 0.0]; 3]);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 3]);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; 3]);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0, 0.0, 0.0, 0.0]; 3]);
    m.insert_indices(Indices::U32(vec![0, 1, 2]));
    m
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Advance the ring buffer each frame.  Mirrors the legacy
/// `fxLaser::UpdateAll` — push Head while controlled, advance Tail while not, drop
/// the laser when Tail == Head with no source left.
fn laser_trail_update(
    time: Res<Time>,
    mut commands: Commands,
    mut q: Query<(Entity, &mut LaserTrail, &mut Transform, &IntendedFxState)>,
    transforms: Query<&GlobalTransform>,
) {
    let dt = time.delta_secs();
    for (ent, mut trail, mut trail_tf, state) in &mut q {
        // Check source liveness — the projectile may have despawned.
        let source_alive = trail
            .source
            .and_then(|s| transforms.get(s).ok())
            .map(|gt| gt.translation());

        let was_controlled = trail.controlled;
        trail.controlled = state.0 && source_alive.is_some();

        if trail.controlled {
            let pos = source_alive.unwrap();
            if trail.starting {
                let n = trail.path.len();
                let t = trail.tail;
                trail.path[t] = pos;
                trail.head = (t + 1) % n;
                let h = trail.head;
                trail.path[h] = pos;
                trail.starting = false;
                trail.push_accum = 0.0;
            } else {
                // Accumulate render-frame time; push one sample per
                // TRAIL_PUSH_INTERVAL.  At 60fps this pushes every
                // other frame; at 30fps every frame; at 120fps every
                // fourth frame.  Net: Length samples always span
                // Length × (1/30) seconds of world time.
                trail.push_accum += dt;
                let mut pushes = 0;
                while trail.push_accum >= TRAIL_PUSH_INTERVAL && pushes < trail.path.len() {
                    trail.push_accum -= TRAIL_PUSH_INTERVAL;
                    let n = trail.path.len();
                    trail.head = (trail.head + 1) % n;
                    if trail.head == trail.tail {
                        trail.tail = (trail.tail + 1) % n;
                    }
                    pushes += 1;
                }
                // Always write the most-recent pos into the current
                // head slot — the interpolated "front of the beam"
                // tracks the projectile every render frame even
                // between discrete pushes.
                let h = trail.head;
                trail.path[h] = pos;
            }
            // Move the trail entity's root transform to the source — keeps
            // the child LaserHead near the action (and gives the frustum
            // culler a sensible position).  Ribbon vertices are in local
            // space relative to this origin, so moving it is free.
            trail_tf.translation = pos;
        } else {
            // Retracting: eat one tail position at the same 30Hz cadence
            // we pushed at, so the fade-out takes ~Length/30 seconds
            // regardless of render framerate.
            trail.push_accum += dt;
            while trail.push_accum >= TRAIL_PUSH_INTERVAL && trail.head != trail.tail {
                trail.push_accum -= TRAIL_PUSH_INTERVAL;
                let n = trail.path.len();
                trail.tail = (trail.tail + 1) % n;
            }
            if trail.head == trail.tail {
                if was_controlled {
                    // Just became uncontrolled this frame with an empty
                    // buffer — nothing left to fade, drop it now.
                    commands.entity(ent).despawn();
                } else {
                    commands.entity(ent).despawn();
                }
            }
        }
    }
}

/// Rebuild the ribbon mesh from the current path buffer.  Vertices are
/// in WORLD space — the material is unlit so transform doesn't matter
/// for shading, and keeping them world-space avoids a per-frame
/// transform update on a moving projectile.
fn laser_ribbon_rebuild(
    q: Query<(&LaserTrail, &Transform, &Mesh3d, &mut Visibility), With<LaserRibbon>>,
    camera: Query<&GlobalTransform, (With<crate::camera::components::CameraController>, Without<LaserRibbon>)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Some(cam_tf) = camera.iter().next() else {
        return;
    };
    let cam_pos = cam_tf.translation();

    for (trail, trail_tf, mesh_h, mut vis) in q {
        // The trail entity's root transform sits at the current head
        // world position (see laser_trail_update).  Mesh vertices live
        // in the entity's LOCAL space so Bevy's transform doesn't
        // double-apply — each path sample is `path_world - trail_tf`.
        let origin = trail_tf.translation;
        let n = trail.path.len();
        let count = trail.count();

        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut normals: Vec<[f32; 3]> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut colors: Vec<[f32; 4]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();

        if count >= 1 {
            // Beam direction (tail→head in world space).  If the projectile
            // hasn't moved yet, fall back to world up so we draw something
            // rather than collapsing into a zero-width strip.
            let beam = trail.path[trail.head] - trail.path[trail.tail];
            let beam_dir = if beam.length_squared() > 1e-8 {
                beam.normalize()
            } else {
                Vec3::Y
            };
            let half_w = 0.5 * trail.width;

            // Width axis computed ONCE per beam from the head's view
            // vector — matches the legacy:
            //   matrix.b = camera - path[Head];  // head→camera
            //   matrix.a = cross(matrix.b, matrix.c);  // matrix.c = beam_dir
            // (the C++ uses `cross(b, c)`; `beam_dir.cross(head_to_cam)`
            // would be the opposite sign, swapping left/right — harmless
            // with cull_mode:None, but matching the C++ keeps UVs on the
            // conventional side.)  Fallback to an arbitrary perpendicular
            // when the camera is exactly behind the beam (cross collapses
            // to zero) so the ribbon doesn't vanish at edge-on views.
            let head_pos = trail.path[trail.head];
            let head_to_cam = (cam_pos - head_pos).normalize_or_zero();
            let width_axis_raw = head_to_cam.cross(beam_dir);
            let width_axis_unit = if width_axis_raw.length_squared() > 1e-8 {
                width_axis_raw.normalize()
            } else {
                // Edge-on view: pick any axis perpendicular to beam_dir.
                // Using world up or world right depending on which isn't
                // colinear with the beam.
                if beam_dir.dot(Vec3::Y).abs() < 0.9 {
                    beam_dir.cross(Vec3::Y).normalize()
                } else {
                    beam_dir.cross(Vec3::X).normalize()
                }
            };
            let width_axis = width_axis_unit * half_w;

            // Per-sample vertex build: two verts offset ±half_w along
            // the (beam-uniform) width axis.
            let inv_count = 1.0 / count as f32;
            let mut k: usize = 0;
            let mut j = trail.tail;
            loop {
                let p_world = trail.path[j];
                let t = k as f32 * inv_count;
                // Tail vertex gets alpha=0 to mask the hard edge
                // (legacy comment: "to help fix hard edge at tail").
                let alpha = if j == trail.tail { 0.0 } else { 1.0 };

                let p_local = p_world - origin;
                let left = p_local - width_axis;
                let right = p_local + width_axis;
                positions.push(left.to_array());
                positions.push(right.to_array());
                normals.push([0.0, 1.0, 0.0]);
                normals.push([0.0, 1.0, 0.0]);
                uvs.push([0.0, t]);
                uvs.push([1.0, t]);
                colors.push([2.0, 2.0, 2.0, alpha]);
                colors.push([2.0, 2.0, 2.0, alpha]);

                if k > 0 {
                    // Stitch to previous pair with two triangles.
                    let base = (k as u32 - 1) * 2;
                    indices.extend_from_slice(&[
                        base,
                        base + 1,
                        base + 2,
                        base + 1,
                        base + 3,
                        base + 2,
                    ]);
                }

                if j == trail.head {
                    break;
                }
                j = (j + 1) % n;
                k += 1;
                // Safety: shouldn't trigger, but bound the loop in case
                // head/tail invariants were violated.
                if k > n + 1 {
                    break;
                }
            }
        }

        if let Some(mesh) = meshes.get_mut(&mesh_h.0) {
            static mut TICK: u32 = 0;
            // SAFETY: single-threaded debug counter; race is harmless
            // since we only use the value to throttle logs.
            let tick = unsafe {
                TICK = TICK.wrapping_add(1);
                TICK
            };
            if tick % 60 == 0 {
                info!(
                    "laser_ribbon_rebuild: count={} verts={} indices={}",
                    count,
                    positions.len(),
                    indices.len()
                );
            }
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
            mesh.insert_indices(Indices::U32(indices));
        }

        // Hide the ribbon entity while fully retracted so we don't render
        // an empty mesh every frame.
        *vis = if count >= 1 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// Billboard the head sprite to face the camera and place it at the
/// most-recent path position.  Hidden while uncontrolled
/// (legacy: "don't draw head after impact").
fn laser_head_billboard(
    parent_q: Query<(&LaserTrail, &Children)>,
    mut head_q: Query<(&mut Transform, &mut Visibility), With<LaserHead>>,
    camera: Query<&GlobalTransform, (With<crate::camera::components::CameraController>, Without<LaserHead>)>,
) {
    let Some(cam_tf) = camera.iter().next() else {
        return;
    };
    let cam_pos = cam_tf.translation();

    for (trail, children) in &parent_q {
        let show_head = trail.controlled && trail.count() >= 1;
        for child in children.iter() {
            let Ok((mut tf, mut vis)) = head_q.get_mut(child) else {
                continue;
            };
            if !show_head {
                *vis = Visibility::Hidden;
                continue;
            }
            *vis = Visibility::Visible;
            // Parent entity's transform already sits at the head world
            // position (see laser_trail_update), so the head sprite
            // stays at local origin.
            tf.translation = Vec3::ZERO;
            // Billboard the unit quad (lying in XY, normal +Z) toward
            // the camera. A spherical billboard just copies the camera's rotation
            // so its local +Z faces exactly opposite the camera's look direction (-Z).
            let (_, cam_rot, _) = cam_tf.to_scale_rotation_translation();
            tf.rotation = cam_rot;
            tf.scale = Vec3::splat(trail.width * trail.head_scale);

            // --- Debug print requested by user ---
            let cam_euler = cam_rot.to_euler(EulerRot::YXZ);
            let head_euler = tf.rotation.to_euler(EulerRot::YXZ);
            
            // Just use a simple static to rate limit or only print on change
            static mut LAST_PRINT: (f32, f32) = (0.0, 0.0);
            unsafe {
                let diff_cam = (cam_euler.0 - LAST_PRINT.0).abs();
                let diff_head = (head_euler.0 - LAST_PRINT.1).abs();
                if diff_cam > 0.1 || diff_head > 0.1 {
                    println!(
                        "LASER DEBUG | Cam Yaw: {:.1}°, Head Yaw: {:.1}°",
                        cam_euler.0.to_degrees(),
                        head_euler.0.to_degrees()
                    );
                    LAST_PRINT = (cam_euler.0, head_euler.0);
                }
            }
            // -------------------------------------
        }
    }
}

/// Ramp the point light intensity up while active, down while fading.
/// Mirrors the legacy laser light ramp — multiply-by-2 up to LightMax,
/// halve per frame when goal is zero.
fn laser_light_animate(
    parent_q: Query<(&LaserTrail, &LaserLightRef)>,
    mut light_q: Query<(&mut PointLight, &mut Transform)>,
) {
    for (trail, light_ref) in &parent_q {
        let Ok((mut light, mut tf)) = light_q.get_mut(light_ref.0) else {
            continue;
        };
        if trail.count() >= 1 {
            // Place light at head path position, in world space.  The
            // light is a child of the trail entity (whose root sits at
            // head), so local translation is zero.
            tf.translation = Vec3::ZERO;
        }
        if trail.controlled {
            // Geometric ramp-up: I *= 2 each frame, clamped to LightMax.
            // The floor of 1.0 ensures we ramp from 0 on the first tick.
            let cur = light.intensity.max(1.0);
            light.intensity = (cur * 2.0).min(trail.light_max * 1000.0);
        } else {
            light.intensity *= 0.5;
            if light.intensity < 0.1 {
                light.intensity = 0.0;
            }
        }
    }
}
