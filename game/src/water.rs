/*
 * water.rs — animated water surface (visual) + buoyancy/drag (physics).
 *
 * Ports the Angel Studios `fzxWaterSurface` / `fzxWaterWave` model
 * (rb/src/rbphys/watersurface.c, wave.c).  The original GPU rendering lived in
 * the AGE `gfxWater` layer which is absent from our source copy, but the wave
 * *math* and the buoyancy/drag physics survive and are reproduced here:
 *
 *   • Surface: a procedural subdivided plane displaced on the GPU by a sum of
 *     directional sine waves (`fzxWaterWave::GetHeight`:
 *     `h = A·sin(k·relPos − ω·t − φ)`), lit through Bevy's PBR via an
 *     `ExtendedMaterial<StandardMaterial, WaterExtension>` whose vertex shader
 *     also recomputes the analytic surface normal.  Approximates the flat,
 *     vertex-animated PS2-era look (no transparency / reflection probe).
 *   • Physics: bodies inside the water volume get buoyancy + drag, the
 *     consumable part of `fzxWaterSurface::ImpactLiquid` (buoyancy impulse +
 *     pushing/sliding drag).  Impact ripples are intentionally NOT ported.
 *
 * Wave defaults are `InitOcean()`'s three waves; `WaterColor` is the engine
 * default (0.5, 0.3, 0.7, 0.9).  No `.wave` asset files ship in our tree, so
 * the surface tuning is the hardcoded ocean preset (kept in one place so a
 * `.wave` loader can replace it later).
 */
use avian3d::prelude::{LinearVelocity, RigidBody};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::{Shader, ShaderRef};

/// Subdivisions per axis of the generated water plane.  Enough resolution for
/// the longest shipped wave (`InitOcean` λ ≈ 2π/0.75 ≈ 8u) to read as a curve
/// rather than facets across a 40u surface.
const GRID_SUBDIVISIONS: u32 = 96;

const WATER_SHADER_HANDLE: Handle<Shader> =
    bevy::asset::uuid_handle!("3f1c8a02-6b7d-4e91-a2c5-9d04f7e61b83");

// ---------------------------------------------------------------------------
// Wave model (port of fzxWaterWave / InitOcean)
// ---------------------------------------------------------------------------

/// One directional sine wave (`fzxWaterWave`).  Heights are summed in world
/// XZ; the lateral Gerstner pinch (`GetDisplacement`) is dropped — vertical
/// displacement alone matches the PS2-era look and keeps normals analytic.
#[derive(Clone, Copy, Debug)]
pub struct WaterWave {
    pub amplitude: f32,
    pub ang_frequency: f32,
    /// Wave-number vector `k` (its magnitude = 2π/wavelength, direction = the
    /// travel direction).  `.y` maps to world Z (matching the engine's 2D
    /// `Vector2(x, z)` convention).
    pub wave_number: Vec2,
    pub origin: Vec2,
    pub phase: f32,
}

/// `fzxWaterSurface::InitOcean()` — the three waves the engine seeds by
/// default.  Amplitudes/frequencies/directions verbatim from watersurface.c.
pub fn ocean_waves() -> Vec<WaterWave> {
    vec![
        WaterWave {
            amplitude: 0.6,
            ang_frequency: 12.0,
            wave_number: Vec2::new(-0.6, 0.45),
            origin: Vec2::ZERO,
            phase: 0.0,
        },
        WaterWave {
            amplitude: 0.4,
            ang_frequency: 16.0,
            wave_number: Vec2::new(-0.9, 0.1),
            origin: Vec2::ZERO,
            phase: 0.4,
        },
        WaterWave {
            amplitude: 0.4,
            ang_frequency: 16.0,
            wave_number: Vec2::new(-0.1, 0.9),
            origin: Vec2::ZERO,
            phase: 0.8,
        },
    ]
}

/// Surface tint.  The engine's literal `fzxWaterSurface` default is
/// `(0.5, 0.3, 0.7)` — a muted violet (high R+B, low G) that came across as
/// purple; the real harbor color lived in the absent AGE `gfxWater` layer.  We
/// use a blue-grey instead, which reads as harbor water.  Opaque (PS2-era look,
/// no transparency).
const WATER_COLOR: Color = Color::srgb(0.20, 0.30, 0.39);

// Physics tuning (watersurface.c defaults).
const PUSHING_DRAG_CONST: f32 = 1.0;
const SLIDING_DRAG_CONST: f32 = 2.0;
/// Upward acceleration applied per unit of submersion depth.  The original
/// scaled `−GRAVITY·GravityScale·Density·buoyancy.w`; `Density` lived in the
/// missing `phForceLiquid` base, so this is a tuned stand-in that lifts a
/// submerged body back to the surface and lets the drag terms settle it.
const BUOYANCY_ACCEL: f32 = 22.0;
/// Submersion depth (world units) at which buoyancy/drag reach full strength.
const SUBMERGE_FULL_DEPTH: f32 = 1.5;

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Marks a layout actor whose entity should become a water surface.  Inserted
/// at spawn (by entity-type name); consumed by `setup_water_surfaces`, which
/// swaps in the procedural plane + water material and replaces it with a
/// resolved [`WaterSurface`].
#[derive(Component, Debug, Clone, Copy)]
pub struct WaterTag;

/// A resolved water surface: the world-space volume used for buoyancy and the
/// waves used by both the shader and the height query.
#[derive(Component, Clone)]
pub struct WaterSurface {
    /// World Y of the still surface (`LevelY`).
    pub level_y: f32,
    /// World-space XZ center of the surface footprint.
    pub center_xz: Vec2,
    /// Half-extents of the footprint on X and Z.
    pub half_extents: Vec2,
    /// How deep the water volume extends below the surface (`Depth`, 12u).
    pub depth: f32,
    pub waves: Vec<WaterWave>,
}

impl WaterSurface {
    /// Still-surface + summed wave height at a world XZ position and time
    /// (`fzxWaterSurface::GetHeight` analog used for submersion tests).
    pub fn surface_height(&self, world_xz: Vec2, time: f32) -> f32 {
        let mut h = self.level_y;
        for w in &self.waves {
            let rel = world_xz - w.origin;
            h += w.amplitude * (w.wave_number.dot(rel) - w.ang_frequency * time - w.phase).sin();
        }
        h
    }

    /// True if `pos` is within the surface footprint in XZ.
    fn within_footprint(&self, pos: Vec3) -> bool {
        let d = (pos.xz() - self.center_xz).abs();
        d.x <= self.half_extents.x && d.y <= self.half_extents.y
    }
}

// ---------------------------------------------------------------------------
// Material
// ---------------------------------------------------------------------------

/// The water surface needs **no** material-stage bindings.  The wave set is
/// the fixed `InitOcean` preset, baked into the shader as constants at plugin
/// init (`build_water_shader_src`).  An earlier design uploaded the waves as a
/// `@group(2) @binding(100)` uniform read from the *vertex* stage, but that
/// tripped a wgpu validation error: `AsBindGroup`'s `visibility(vertex, …)` is
/// not honored through `ExtendedMaterial`'s combined bind-group layout, so the
/// binding never became vertex-visible.  Driving the surface entirely from
/// `globals.time` (a view-group binding, always vertex-visible) sidesteps the
/// whole issue — at the cost of per-surface runtime wave tuning, which we
/// don't need while no `.wave` assets ship.
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug, Default)]
pub struct WaterExtension {}

impl MaterialExtension for WaterExtension {
    fn vertex_shader() -> ShaderRef {
        WATER_SHADER_HANDLE.into()
    }
}

/// Water surface material: PBR base + GPU wave displacement.
pub type WaterMaterial = ExtendedMaterial<StandardMaterial, WaterExtension>;

/// Marker replaced by the generated `wave_field` body in the shader template.
const WAVE_FIELD_PLACEHOLDER: &str = "//__WAVE_TERMS__//";

/// Builds the surface WGSL with the `InitOcean` waves baked in as constant
/// terms, so the vertex shader needs only `globals.time` and no material
/// uniform.  One source of truth: the terms come from `ocean_waves()`.
fn build_water_shader_src() -> String {
    let mut terms = String::new();
    for w in ocean_waves() {
        // Each term contributes to height `h` and the XZ gradient (dx, dz).
        // Scoped block so the locals don't collide between terms.
        terms.push_str(&format!(
            "    {{\n        let k = vec2<f32>({:?}, {:?});\n        let o = vec2<f32>({:?}, {:?});\n        let th = dot(k, world_xz - o) - {:?} * t - {:?};\n        h = h + {:?} * sin(th);\n        let c = {:?} * cos(th);\n        dx = dx + c * k.x;\n        dz = dz + c * k.y;\n    }}\n",
            w.wave_number.x,
            w.wave_number.y,
            w.origin.x,
            w.origin.y,
            w.ang_frequency,
            w.phase,
            w.amplitude,
            w.amplitude,
        ));
    }
    WATER_SHADER_TEMPLATE.replace(WAVE_FIELD_PLACEHOLDER, &terms)
}

const WATER_SHADER_TEMPLATE: &str = r#"
#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::globals,
}

// Returns (height, dHeight/dx, dHeight/dz) summed over the baked-in waves at a
// world-space XZ position and time.  Mirrors fzxWaterWave::GetHeight; the
// derivatives give the analytic surface normal.
fn wave_field(world_xz: vec2<f32>, t: f32) -> vec3<f32> {
    var h = 0.0;
    var dx = 0.0;
    var dz = 0.0;
//__WAVE_TERMS__//
    return vec3<f32>(h, dx, dz);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;

    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );

    let field = wave_field(world_position.xz, globals.time);
    world_position.y = world_position.y + field.x;

    out.world_position = world_position;
    out.position = position_world_to_clip(world_position.xyz);
    // Analytic normal of the height field z = h(x, z): (-dh/dx, 1, -dh/dz).
    out.world_normal = normalize(vec3<f32>(-field.y, 1.0, -field.z));

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}
"#;

// ---------------------------------------------------------------------------
// Mesh generation
// ---------------------------------------------------------------------------

/// Build a flat subdivided plane in the XZ plane spanning `min..max` (world
/// units, relative to the entity origin) at local Y = `y`, with `subdivisions`
/// quads per axis.  Positions/normals/UVs only; the shader displaces it.
fn build_water_plane(min: Vec2, max: Vec2, y: f32, subdivisions: u32) -> Mesh {
    let n = subdivisions.max(1);
    let verts_per_axis = n + 1;
    let size = max - min;
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity((verts_per_axis * verts_per_axis) as usize);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(positions.capacity());
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(positions.capacity());

    for iz in 0..verts_per_axis {
        let fz = iz as f32 / n as f32;
        for ix in 0..verts_per_axis {
            let fx = ix as f32 / n as f32;
            let x = min.x + size.x * fx;
            let z = min.y + size.y * fz;
            positions.push([x, y, z]);
            normals.push([0.0, 1.0, 0.0]);
            uvs.push([fx, fz]);
        }
    }

    let mut indices: Vec<u32> = Vec::with_capacity((n * n * 6) as usize);
    for iz in 0..n {
        for ix in 0..n {
            let row = verts_per_axis;
            let i0 = iz * row + ix;
            let i1 = i0 + 1;
            let i2 = i0 + row;
            let i3 = i2 + 1;
            // CCW winding for an upward-facing surface.
            indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Converts freshly-spawned [`WaterTag`] entities into water surfaces: finds
/// the entity's rendered mesh child, replaces its geometry with a procedural
/// subdivided plane covering the same XZ footprint, swaps the material for the
/// wave-displacing [`WaterMaterial`], and attaches a resolved [`WaterSurface`]
/// for the buoyancy system.  Runs until the mesh asset is available, then
/// removes the tag.
#[allow(clippy::too_many_arguments)]
pub fn setup_water_surfaces(
    mut commands: Commands,
    tagged: Query<(Entity, &GlobalTransform), With<WaterTag>>,
    children_q: Query<&Children>,
    mesh_handles: Query<&Mesh3d>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut water_materials: ResMut<Assets<WaterMaterial>>,
) {
    for (entity, gt) in &tagged {
        // Find the first descendant carrying a Mesh3d (the rendered submesh).
        let Some((mesh_child, mesh3d)) = find_mesh_child(entity, &children_q, &mesh_handles) else {
            // Mesh children may not exist yet on the spawn frame; retry later.
            continue;
        };
        let Some(src) = meshes.get(&mesh3d.0) else {
            continue;
        };
        let Some((aabb_min, aabb_max)) = mesh_position_bounds(src) else {
            continue;
        };

        // Footprint from the source mesh's local AABB (XZ), centered on its
        // local center so the plane overlays the original water quad.
        let center = (aabb_min + aabb_max) * 0.5;
        let he = (aabb_max - aabb_min) * 0.5;
        let min = Vec2::new(center.x - he.x, center.z - he.z);
        let max = Vec2::new(center.x + he.x, center.z + he.z);
        let plane = build_water_plane(min, max, center.y, GRID_SUBDIVISIONS);
        let plane_handle = meshes.add(plane);

        let waves = ocean_waves();
        let material = water_materials.add(WaterMaterial {
            base: StandardMaterial {
                base_color: WATER_COLOR,
                perceptual_roughness: 0.2,
                metallic: 0.1,
                ..default()
            },
            extension: WaterExtension {},
        });

        // Swap geometry + material on the rendered child, dropping the old
        // StandardMaterial handle.
        commands
            .entity(mesh_child)
            .insert((Mesh3d(plane_handle), MeshMaterial3d(material)))
            .remove::<MeshMaterial3d<StandardMaterial>>();

        // Resolve the world-space volume.  The footprint is axis-aligned in
        // world XZ (water entities are only ever yaw-rotated and span symmetric
        // extents, so the rotated AABB matches the world AABB closely enough
        // for the submersion test).
        let world_center = gt.transform_point(center);
        commands.entity(entity).insert(WaterSurface {
            level_y: world_center.y,
            center_xz: world_center.xz(),
            half_extents: Vec2::new(he.x, he.z),
            depth: 12.0,
            waves,
        });
        commands.entity(entity).remove::<WaterTag>();
        info!(
            "Water: surface ready (level_y {:.2}, footprint {:.1}x{:.1})",
            world_center.y,
            he.x * 2.0,
            he.z * 2.0
        );
    }
}

/// Local-space min/max of a mesh's position attribute.  `None` if the mesh has
/// no float3 positions.
fn mesh_position_bounds(mesh: &Mesh) -> Option<(Vec3, Vec3)> {
    let positions = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?.as_float3()?;
    if positions.is_empty() {
        return None;
    }
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for p in positions {
        let v = Vec3::from(*p);
        min = min.min(v);
        max = max.max(v);
    }
    Some((min, max))
}

/// Depth-first search for the first descendant of `root` that has a `Mesh3d`.
fn find_mesh_child(
    root: Entity,
    children_q: &Query<&Children>,
    mesh_handles: &Query<&Mesh3d>,
) -> Option<(Entity, Mesh3d)> {
    if let Ok(m) = mesh_handles.get(root) {
        return Some((root, m.clone()));
    }
    let children = children_q.get(root).ok()?;
    for &child in children {
        if let Some(found) = find_mesh_child(child, children_q, mesh_handles) {
            return Some(found);
        }
    }
    None
}

/// Applies buoyancy + drag to dynamic bodies inside a water volume — the
/// consumable half of `fzxWaterSurface::ImpactLiquid` (buoyancy impulse +
/// pushing/sliding drag).  Runs in FixedUpdate *after* the Tnua bridge (which
/// owns/zeroes mover velocity each tick) so the writes survive, mirroring how
/// `apply_force_vector_triggers` is scheduled.
pub fn apply_water_buoyancy(
    time: Res<Time>,
    surfaces: Query<&WaterSurface>,
    mut bodies: Query<(&GlobalTransform, &mut LinearVelocity, &RigidBody)>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let t = time.elapsed_secs();

    for surface in &surfaces {
        for (gt, mut vel, body) in &mut bodies {
            // Only dynamic bodies are pushed by the liquid.
            if !matches!(body, RigidBody::Dynamic) {
                continue;
            }
            let pos = gt.translation();
            if !surface.within_footprint(pos) {
                continue;
            }
            // Submersion = how far the body sits below the wavy surface.
            let surf_y = surface.surface_height(pos.xz(), t);
            let depth = surf_y - pos.y;
            if depth <= 0.0 {
                continue; // above the surface
            }
            let frac = (depth / SUBMERGE_FULL_DEPTH).clamp(0.0, 1.0);

            // Buoyancy: lift toward the surface (the engine's upward impulse).
            vel.y += BUOYANCY_ACCEL * frac * dt;

            // Pushing drag: resist motion into the surface (downward), scaled
            // by the squared normal velocity like `PushingDragConst·v²`.
            if vel.y < 0.0 {
                let push = (PUSHING_DRAG_CONST * frac * vel.y * vel.y * dt).min(-vel.y);
                vel.y += push;
            }

            // Sliding drag: damp lateral motion (`SlidingDragConst`).
            let slide = (SLIDING_DRAG_CONST * frac * dt).min(1.0);
            vel.x -= vel.x * slide;
            vel.z -= vel.z * slide;
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        {
            let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
            let _ = shaders.insert(
                &WATER_SHADER_HANDLE,
                Shader::from_wgsl(build_water_shader_src(), "water.wgsl"),
            );
        }
        app.add_plugins(MaterialPlugin::<WaterMaterial>::default())
            .add_systems(Update, setup_water_surfaces)
            .add_systems(
                FixedUpdate,
                // After the Tnua bridge so the velocity writes aren't zeroed
                // (same scheduling rationale as `apply_force_vector_triggers`).
                apply_water_buoyancy.after(crate::mover::bridge::tnua_basis_from_linvel),
            );
    }
}

/// True for layout entity types that should render as animated water.
/// Matches `*water*` but excludes waterfalls (vertical scrolling surfaces,
/// out of scope here).
pub fn is_water_entity_type(entity_type: &str) -> bool {
    let lower = entity_type.to_lowercase();
    lower.contains("water") && !lower.contains("fall")
}
