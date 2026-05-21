/*
 * oni2_loader/parsers/mesh.rs — Bevy mesh builder from Oni2Model.
 *
 * build_meshes_by_material: groups Oni2Packet triangles by material index and
 * builds one Bevy Mesh per material with positions, normals, UVs, colors, and
 * skinning (joint indices + weights).  Returns (material_index, Mesh) pairs.
 */
use super::types::{Oni2Adjunct, Oni2Material, Oni2Model, Oni2Packet, Oni2Skeleton};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

/// Build one Bevy Mesh per material from an Oni2Model.
/// Returns (material_index, Mesh) pairs so the caller can assign textures.
pub fn build_meshes_by_material(model: &Oni2Model) -> Vec<(usize, Mesh)> {
    // Group packets by material index
    let mat_count = model.materials.len().max(1);
    let mut per_mat: Vec<(
        Vec<[f32; 3]>,
        Vec<[f32; 3]>,
        Vec<[f32; 2]>,
        Vec<[f32; 4]>,
        Vec<u32>,
    )> = (0..mat_count)
        .map(|_| (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()))
        .collect();

    for packet in &model.packets {
        let mat_idx = packet.material_index.min(mat_count - 1);
        let mat = model.materials.get(mat_idx);
        let mat_diffuse = mat.map(|m| m.diffuse).unwrap_or([0.8, 0.8, 0.8]);
        let (positions, normals, uvs, colors, indices) = &mut per_mat[mat_idx];

        for (strip_idx, strip) in packet.strips.iter().enumerate() {
            if strip.len() < 3 {
                continue;
            }
            let stype = packet.strip_types.get(strip_idx).copied().unwrap_or(1);

            let mut strip_verts: Vec<u32> = Vec::new();
            for &adj_idx in strip {
                let default_adj = crate::oni2_loader::parsers::types::Oni2Adjunct {
                    vertex_idx: 0,
                    normal_idx: 0,
                    color_idx: 0,
                    tex1_idx: -1,
                    bone_idx: 0,
                };
                let adj = packet
                    .adjuncts
                    .get(adj_idx as usize)
                    .unwrap_or(&default_adj);

                let raw_pos = model
                    .vertices
                    .get(adj.vertex_idx as usize)
                    .copied()
                    .unwrap_or([0.0; 3]);
                let raw_norm = model
                    .normals
                    .get(adj.normal_idx as usize)
                    .copied()
                    .unwrap_or([0.0, 1.0, 0.0]);

                // Apply bone transform: rotate vertex by bone rotation + offset by bone position
                // Skip if vertices are already in world space (win32 binary models)
                let (transformed, rotated_norm);
                if model.world_space_verts {
                    transformed = raw_pos;
                    rotated_norm = Vec3::new(raw_norm[0], raw_norm[1], raw_norm[2]);
                } else {
                    let global_bone = if !model.bone_world_positions.is_empty() {
                        if !packet.bone_map.is_empty() {
                            *packet.bone_map.get(adj.bone_idx as usize).unwrap_or(&0) as usize
                        } else {
                            adj.bone_idx as usize
                        }
                    } else {
                        0
                    };
                    let bone_offset = model
                        .bone_world_positions
                        .get(global_bone)
                        .copied()
                        .unwrap_or([0.0; 3]);
                    let bone_rot = if !model.bone_rotations.is_empty() {
                        let r = model
                            .bone_rotations
                            .get(global_bone)
                            .copied()
                            .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                        Quat::from_xyzw(r[0], r[1], r[2], r[3])
                    } else {
                        Quat::IDENTITY
                    };
                    let rv = bone_rot.mul_vec3(Vec3::new(raw_pos[0], raw_pos[1], raw_pos[2]));
                    transformed = [
                        rv.x + bone_offset[0],
                        rv.y + bone_offset[1],
                        rv.z + bone_offset[2],
                    ];
                    rotated_norm =
                        bone_rot.mul_vec3(Vec3::new(raw_norm[0], raw_norm[1], raw_norm[2]));
                };

                // Left-handed → right-handed: negate X and Z (180° Y rotation, not a mirror)
                let pos = [-transformed[0], transformed[1], -transformed[2]];
                let norm = [-rotated_norm.x, rotated_norm.y, -rotated_norm.z];
                let raw_uv = if adj.tex1_idx >= 0 {
                    model
                        .tex_coords
                        .get(adj.tex1_idx as usize)
                        .copied()
                        .unwrap_or([0.0; 2])
                } else {
                    [0.0; 2]
                };
                let uv = [raw_uv[0], 1.0 - raw_uv[1]]; // DirectX V → OpenGL V
                let color = model
                    .colors
                    .get(adj.color_idx as usize)
                    .copied()
                    .unwrap_or([1.0, 1.0, 1.0, 1.0]);

                let tinted = [
                    color[0] * mat_diffuse[0],
                    color[1] * mat_diffuse[1],
                    color[2] * mat_diffuse[2],
                    color[3],
                ];

                let vert_idx = positions.len() as u32;
                positions.push(pos);
                normals.push(norm);
                uvs.push(uv);
                colors.push(tinted);
                strip_verts.push(vert_idx);
            }

            // Triangle strip → triangle list with alternating winding.
            // X+Z negate is a 180° rotation (preserves winding), so use standard order.
            // stp (type 2) starts with swapped parity vs str (type 1).
            let parity_offset = if stype == 2 { 1usize } else { 0usize };
            for j in 0..strip_verts.len().saturating_sub(2) {
                if (j + parity_offset) % 2 == 0 {
                    indices.push(strip_verts[j]);
                    indices.push(strip_verts[j + 1]);
                    indices.push(strip_verts[j + 2]);
                } else {
                    indices.push(strip_verts[j + 2]);
                    indices.push(strip_verts[j + 1]);
                    indices.push(strip_verts[j]);
                }
            }
        }
    }

    let mut result = Vec::new();
    for (mat_idx, (positions, normals, uvs, colors, indices)) in per_mat.into_iter().enumerate() {
        if positions.is_empty() {
            continue;
        }

        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_indices(Indices::U32(indices));

        result.push((mat_idx, mesh));
    }

    result
}

/// Build skinned meshes for GPU skinning. Vertices are placed in bind-pose object space
/// with JOINT_INDEX and JOINT_WEIGHT attributes. Bevy's GPU skinning shader transforms them.
pub fn build_skinned_meshes_by_material(
    model: &Oni2Model,
    skel: &Oni2Skeleton,
) -> Vec<(usize, Mesh)> {
    let mat_count = model.materials.len().max(1);
    // positions, normals, uvs, colors, joint_indices, joint_weights, triangle indices
    let mut per_mat: Vec<(
        Vec<[f32; 3]>,
        Vec<[f32; 3]>,
        Vec<[f32; 2]>,
        Vec<[f32; 4]>,
        Vec<[u16; 4]>,
        Vec<[f32; 4]>,
        Vec<u32>,
    )> = (0..mat_count)
        .map(|_| {
            (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
        })
        .collect();

    for packet in &model.packets {
        let mat_idx = packet.material_index.min(mat_count - 1);
        let mat = model.materials.get(mat_idx);
        let mat_diffuse = mat.map(|m| m.diffuse).unwrap_or([0.8, 0.8, 0.8]);
        let (positions, normals, uvs, colors, joint_indices, joint_weights, indices) =
            &mut per_mat[mat_idx];

        for (strip_idx, strip) in packet.strips.iter().enumerate() {
            if strip.len() < 3 {
                continue;
            }
            let stype = packet.strip_types.get(strip_idx).copied().unwrap_or(1);

            let mut strip_verts: Vec<u32> = Vec::new();
            for &adj_idx in strip {
                let default_adj = crate::oni2_loader::parsers::types::Oni2Adjunct {
                    vertex_idx: 0,
                    normal_idx: 0,
                    color_idx: 0,
                    tex1_idx: -1,
                    bone_idx: 0,
                };
                let adj = packet
                    .adjuncts
                    .get(adj_idx as usize)
                    .unwrap_or(&default_adj);

                let raw_pos = model
                    .vertices
                    .get(adj.vertex_idx as usize)
                    .copied()
                    .unwrap_or([0.0; 3]);
                let raw_norm = model
                    .normals
                    .get(adj.normal_idx as usize)
                    .copied()
                    .unwrap_or([0.0, 1.0, 0.0]);

                // Resolve global bone index
                let global_bone = if !packet.bone_map.is_empty() {
                    *packet.bone_map.get(adj.bone_idx as usize).unwrap_or(&0) as usize
                } else {
                    adj.bone_idx as usize
                };

                // Compute bind-pose object-space position:
                // vertex is bone-local, bind pose has no rotation, so just add bone position
                let bone_pos = skel.positions.get(global_bone).copied().unwrap_or([0.0; 3]);
                let obj_pos = [
                    raw_pos[0] + bone_pos[0],
                    raw_pos[1] + bone_pos[1],
                    raw_pos[2] + bone_pos[2],
                ];

                // Left-handed → right-handed: negate X and Z (180° Y rotation)
                let pos = [-obj_pos[0], obj_pos[1], -obj_pos[2]];
                // Normals in bind pose have no rotation, just coordinate convert
                let norm = [-raw_norm[0], raw_norm[1], -raw_norm[2]];

                let raw_uv = if adj.tex1_idx >= 0 {
                    model
                        .tex_coords
                        .get(adj.tex1_idx as usize)
                        .copied()
                        .unwrap_or([0.0; 2])
                } else {
                    [0.0; 2]
                };
                let uv = [raw_uv[0], 1.0 - raw_uv[1]];
                let color = model
                    .colors
                    .get(adj.color_idx as usize)
                    .copied()
                    .unwrap_or([1.0, 1.0, 1.0, 1.0]);
                let tinted = [
                    color[0] * mat_diffuse[0],
                    color[1] * mat_diffuse[1],
                    color[2] * mat_diffuse[2],
                    color[3],
                ];

                let vert_idx = positions.len() as u32;
                positions.push(pos);
                normals.push(norm);
                uvs.push(uv);
                colors.push(tinted);
                joint_indices.push([global_bone as u16, 0, 0, 0]);
                joint_weights.push([1.0, 0.0, 0.0, 0.0]);
                strip_verts.push(vert_idx);
            }

            // Triangle strip → triangle list
            let parity_offset = if stype == 2 { 1usize } else { 0usize };
            for j in 0..strip_verts.len().saturating_sub(2) {
                if (j + parity_offset) % 2 == 0 {
                    indices.push(strip_verts[j]);
                    indices.push(strip_verts[j + 1]);
                    indices.push(strip_verts[j + 2]);
                } else {
                    indices.push(strip_verts[j + 2]);
                    indices.push(strip_verts[j + 1]);
                    indices.push(strip_verts[j]);
                }
            }
        }
    }

    let mut result = Vec::new();
    for (mat_idx, (positions, normals, uvs, colors, joint_indices, joint_weights, indices)) in
        per_mat.into_iter().enumerate()
    {
        if positions.is_empty() {
            continue;
        }

        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(joint_indices),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, joint_weights);
        mesh.insert_indices(Indices::U32(indices));

        result.push((mat_idx, mesh));
    }

    result
}

/// Build point cloud meshes (one per material) — each vertex rendered as a tiny triangle "dot".
pub fn build_point_clouds_by_material(model: &Oni2Model) -> Vec<(usize, Mesh)> {
    let mat_count = model.materials.len().max(1);
    let mut per_mat: Vec<(
        Vec<[f32; 3]>,
        Vec<[f32; 3]>,
        Vec<[f32; 2]>,
        Vec<[f32; 4]>,
        Vec<u32>,
    )> = (0..mat_count)
        .map(|_| (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()))
        .collect();

    let dot_size = 0.008; // radius of each dot triangle

    for packet in &model.packets {
        let mat_idx = packet.material_index.min(mat_count - 1);
        let mat = model.materials.get(mat_idx);
        let mat_diffuse = mat.map(|m| m.diffuse).unwrap_or([0.8, 0.8, 0.8]);
        let (positions, normals, uvs, colors, indices) = &mut per_mat[mat_idx];

        for adj in &packet.adjuncts {
            let raw_pos = model
                .vertices
                .get(adj.vertex_idx as usize)
                .copied()
                .unwrap_or([0.0; 3]);
            let raw_norm = model
                .normals
                .get(adj.normal_idx as usize)
                .copied()
                .unwrap_or([0.0, 1.0, 0.0]);

            let (transformed, rotated_norm);
            if model.world_space_verts {
                transformed = raw_pos;
                rotated_norm = Vec3::new(raw_norm[0], raw_norm[1], raw_norm[2]);
            } else {
                let global_bone = if !model.bone_world_positions.is_empty() {
                    if !packet.bone_map.is_empty() {
                        *packet.bone_map.get(adj.bone_idx as usize).unwrap_or(&0) as usize
                    } else {
                        adj.bone_idx as usize
                    }
                } else {
                    0
                };
                let bone_offset = model
                    .bone_world_positions
                    .get(global_bone)
                    .copied()
                    .unwrap_or([0.0; 3]);
                let bone_rot = if !model.bone_rotations.is_empty() {
                    let r = model
                        .bone_rotations
                        .get(global_bone)
                        .copied()
                        .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                    Quat::from_xyzw(r[0], r[1], r[2], r[3])
                } else {
                    Quat::IDENTITY
                };
                let rv = bone_rot.mul_vec3(Vec3::new(raw_pos[0], raw_pos[1], raw_pos[2]));
                transformed = [
                    rv.x + bone_offset[0],
                    rv.y + bone_offset[1],
                    rv.z + bone_offset[2],
                ];
                rotated_norm = bone_rot.mul_vec3(Vec3::new(raw_norm[0], raw_norm[1], raw_norm[2]));
            };

            let cx = -transformed[0]; // X+Z negate = 180° Y rotation
            let cy = transformed[1];
            let cz = -transformed[2];
            let norm = [-rotated_norm.x, rotated_norm.y, -rotated_norm.z];
            let uv = if adj.tex1_idx >= 0 {
                model
                    .tex_coords
                    .get(adj.tex1_idx as usize)
                    .copied()
                    .unwrap_or([0.0; 2])
            } else {
                [0.0; 2]
            };
            let color = model
                .colors
                .get(adj.color_idx as usize)
                .copied()
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            let tinted = [
                color[0] * mat_diffuse[0],
                color[1] * mat_diffuse[1],
                color[2] * mat_diffuse[2],
                color[3],
            ];

            // Emit 3 vertices forming a small triangle "dot" around (cx, cy, cz)
            let d = dot_size as f32;
            let base = positions.len() as u32;
            // Small equilateral triangle in XY plane
            positions.push([cx - d, cy - d * 0.577, cz]);
            positions.push([cx + d, cy - d * 0.577, cz]);
            positions.push([cx, cy + d * 1.155, cz]);
            for _ in 0..3 {
                normals.push(norm);
                uvs.push(uv);
                colors.push(tinted);
            }
            // Front face
            indices.push(base);
            indices.push(base + 1);
            indices.push(base + 2);
            // Back face (so dot is visible from both sides)
            indices.push(base + 2);
            indices.push(base + 1);
            indices.push(base);
        }
    }

    let mut result = Vec::new();
    for (mat_idx, (positions, normals, uvs, colors, indices)) in per_mat.into_iter().enumerate() {
        if positions.is_empty() {
            continue;
        }
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_indices(Indices::U32(indices));
        result.push((mat_idx, mesh));
    }
    result
}

// ---------------------------------------------------------------------------
// parse_mesh — standalone `.mesh` text-format parser.
// ---------------------------------------------------------------------------
//
// The `.mesh` text format is the AGE-engine standalone counterpart to `.mod`
// (which packages a model + skeleton-bone hierarchy + materials).  `.mesh`
// drops the bone hierarchy and the entity-shaped metadata, keeping just the
// per-vertex attribute arrays and a tri-strip primitive list — exactly what a
// standalone drawable needs.  FX assets (blast fire / residue / edge,
// projectile trails, etc.) ship as `.mesh`; entity skins ship as `.mod`.
//
// Format (braced block; sections may appear in any order — we scan):
//
//   {
//       Skinned 0            ; 0 = unskinned (only case in shipping FX)
//       PosSkin 0            ; only meaningful when Skinned=1
//       Pos  N { x y z … }   ; N positions (3 tab-separated floats per line)
//       Nrm  N { x y z … }   ; N normals
//       Cpv  N { r g b a … } ; N color-per-vertex (4 floats, premultiplied)
//       Tex0 N { u v … }     ; N UV0
//       Tex1 K               ; K UV1 (always 0 in current FX assets, no block)
//       Adj  N { P i / N i / C0 i / T0 i … } ; N adjuncts, 4 lines each
//       Mtl  M {             ; M materials
//           { Name "…"  Priority p  Prim P { (Type TRISTRIP, Idx K { idx… }) } }
//       }
//       Offset 0             ; coordinate-system origin offset (ignored)
//   }
//
// Mapping to `Oni2Model` matches `parse_mod`'s output so the same
// `build_meshes_by_material` downstream handles it.  Vertices stay in raw
// AGE space — `build_meshes_by_material` negates X/Z and flips V at the
// bone-transform step, and a single identity "bone 0" is seeded so that
// path runs cleanly (vs the world-space-verts branch, which would skip
// the conversion).
//
// Returns `Some(Oni2Model)` on a recognisable top-level `{`; `None` if the
// file doesn't start with one (caller decides whether to log/warn).

pub fn parse_mesh(content: &str, _entity_dir: &str) -> Option<Oni2Model> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut tex_coords: Vec<[f32; 2]> = Vec::new();
    let mut adjuncts_flat: Vec<Oni2Adjunct> = Vec::new();
    let mut materials: Vec<Oni2Material> = Vec::new();
    let mut packets: Vec<Oni2Packet> = Vec::new();

    while i < lines.len() {
        let line = lines[i].trim();

        // Section dispatch.  Every section's header is `<Name> <count>`
        // optionally followed by `{` on the same or next line.  We
        // tokenise to get the section name and count.
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() || toks[0] == "{" || toks[0] == "}" {
            i += 1;
            continue;
        }

        let section = toks[0];
        let count: usize = toks.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

        match section {
            "Pos" => {
                let (next, rows) = read_n_floats(&lines, i + 1, count, 3);
                i = next;
                for row in rows {
                    vertices.push([row[0], row[1], row[2]]);
                }
            }
            "Nrm" => {
                let (next, rows) = read_n_floats(&lines, i + 1, count, 3);
                i = next;
                for row in rows {
                    normals.push([row[0], row[1], row[2]]);
                }
            }
            "Cpv" => {
                let (next, rows) = read_n_floats(&lines, i + 1, count, 4);
                i = next;
                for row in rows {
                    colors.push([row[0], row[1], row[2], row[3]]);
                }
            }
            "Tex0" => {
                let (next, rows) = read_n_floats(&lines, i + 1, count, 2);
                i = next;
                for row in rows {
                    tex_coords.push([row[0], row[1]]);
                }
            }
            "Tex1" => {
                // Always 0 in shipping FX; if non-zero we'd need a
                // tex_coords_1 array, which `Oni2Model` doesn't carry.
                // Skip the block (or the header-only "Tex1 0" line).
                if count > 0 {
                    let (next, _) = read_n_floats(&lines, i + 1, count, 2);
                    i = next;
                } else {
                    i += 1;
                }
            }
            "Adj" => {
                // Each adjunct is 4 consecutive `<Letter> <index>` lines
                // in the order P (position), N (normal), C0 (color0),
                // T0 (tex0).  Unskinned mesh → bone_idx = 0.
                let (next, adjs) = read_adjuncts(&lines, i + 1, count);
                i = next;
                adjuncts_flat = adjs;
            }
            "Mtl" => {
                let (next, parsed_mats, parsed_packets) =
                    read_materials(&lines, i + 1, count, &adjuncts_flat);
                i = next;
                materials = parsed_mats;
                packets = parsed_packets;
            }
            // `Skinned`, `PosSkin`, `Offset` — single-line flags, ignored.
            _ => {
                i += 1;
            }
        }
    }

    Some(Oni2Model {
        vertices,
        normals,
        colors,
        tex_coords,
        materials,
        packets,
        // Seed identity bone 0 so the build_meshes_by_material bone path
        // becomes a no-op composition (rotate by IDENT, translate by 0).
        // We keep `world_space_verts = false` so the X/Z negation that
        // lives in the bone-transform branch runs.
        bone_world_positions: vec![[0.0, 0.0, 0.0]],
        bone_rotations: vec![[0.0, 0.0, 0.0, 1.0]],
        world_space_verts: false,
    })
}

/// Read `count` rows from `lines` starting at index `start`, each row
/// containing `fields` floats (tab- or space-separated).  Skips the
/// section's opening `{` and stops at the matching `}`.  Returns
/// `(line_index_after_close_brace, rows)`.
fn read_n_floats(
    lines: &[&str],
    start: usize,
    count: usize,
    fields: usize,
) -> (usize, Vec<Vec<f32>>) {
    let mut rows: Vec<Vec<f32>> = Vec::with_capacity(count);
    let mut i = start;

    // Skip a leading `{` if present (it may be on the header line or
    // on its own line — both layouts appear in the wild).
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    // NOTE: section headers always carry an inline `{` in the
    // observed `.mesh` format (e.g. `Pos 288 {`).  We do NOT consume a
    // standalone `{` here — that would eat the per-material brace in
    // `Mtl 1 { \n { ... } }` and skip the material body entirely.

    while i < lines.len() && rows.len() < count {
        let trimmed = lines[i].trim();
        if trimmed == "}" {
            i += 1;
            return (i, rows);
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= fields {
            let row: Vec<f32> = parts
                .iter()
                .take(fields)
                .map(|s| s.parse().unwrap_or(0.0))
                .collect();
            rows.push(row);
        }
        i += 1;
    }

    // Drain trailing `}` if we hit count first.
    while i < lines.len() {
        let t = lines[i].trim();
        i += 1;
        if t == "}" {
            break;
        }
    }
    (i, rows)
}

/// Read `count` adjuncts.  Each adjunct is the next 4 lines in the form
/// `P n` / `N n` / `C0 n` / `T0 n` (any order, but always 4 lines).
fn read_adjuncts(lines: &[&str], start: usize, count: usize) -> (usize, Vec<Oni2Adjunct>) {
    let mut adjs: Vec<Oni2Adjunct> = Vec::with_capacity(count);
    let mut i = start;

    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    // NOTE: section headers always carry an inline `{` in the
    // observed `.mesh` format (e.g. `Pos 288 {`).  We do NOT consume a
    // standalone `{` here — that would eat the per-material brace in
    // `Mtl 1 { \n { ... } }` and skip the material body entirely.

    while i < lines.len() && adjs.len() < count {
        // Gather up to 4 attribute lines for one adjunct.
        let mut p: u32 = 0;
        let mut n: u32 = 0;
        let mut c: u32 = 0;
        let mut t: i32 = -1;
        let mut filled = 0;
        while i < lines.len() && filled < 4 {
            let trimmed = lines[i].trim();
            if trimmed == "}" || trimmed.is_empty() {
                i += 1;
                continue;
            }
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() < 2 {
                i += 1;
                continue;
            }
            let v: i64 = parts[1].parse().unwrap_or(0);
            match parts[0] {
                "P" => p = v as u32,
                "N" => n = v as u32,
                "C0" => c = v as u32,
                "T0" => t = v as i32,
                _ => {}
            }
            filled += 1;
            i += 1;
        }
        adjs.push(Oni2Adjunct {
            vertex_idx: p,
            normal_idx: n,
            color_idx: c,
            tex1_idx: t,
            bone_idx: 0,
        });
    }

    while i < lines.len() {
        let t = lines[i].trim();
        i += 1;
        if t == "}" {
            break;
        }
    }
    (i, adjs)
}

/// Read `count` materials.  Each material is a brace-wrapped block with
/// `Name "<n>"`, `Priority p`, and `Prim P { … }` containing P primitive
/// sub-blocks (each with `Type TRISTRIP` and `Idx K { i0 i1 … }`).  Builds
/// one packet PER material: tri-strip indices index into the shared
/// adjunct array, and we mark `material_index = current material idx`.
fn read_materials(
    lines: &[&str],
    start: usize,
    count: usize,
    adjuncts_flat: &[Oni2Adjunct],
) -> (usize, Vec<Oni2Material>, Vec<Oni2Packet>) {
    let mut materials: Vec<Oni2Material> = Vec::with_capacity(count);
    let mut packets: Vec<Oni2Packet> = Vec::with_capacity(count);
    let mut i = start;

    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    // NOTE: section headers always carry an inline `{` in the
    // observed `.mesh` format (e.g. `Pos 288 {`).  We do NOT consume a
    // standalone `{` here — that would eat the per-material brace in
    // `Mtl 1 { \n { ... } }` and skip the material body entirely.

    while i < lines.len() && materials.len() < count {
        let trimmed = lines[i].trim();
        if trimmed == "}" {
            i += 1;
            break;
        }
        if trimmed == "{" {
            i += 1;
            let mat_idx = materials.len();
            let (next, mat, strips, strip_types) = read_material_body(lines, i, mat_idx);
            i = next;
            materials.push(mat);
            packets.push(Oni2Packet {
                adjuncts: adjuncts_flat.to_vec(),
                strips,
                strip_types,
                material_index: mat_idx,
                bone_map: Vec::new(),
            });
            continue;
        }
        i += 1;
    }

    (i, materials, packets)
}

/// Body of one material block (after the opening `{`).  Returns
/// `(line_after_close_brace, material, strips, strip_types)`.
fn read_material_body(
    lines: &[&str],
    start: usize,
    _mat_idx: usize,
) -> (usize, Oni2Material, Vec<Vec<u32>>, Vec<u32>) {
    let mut i = start;
    let mut name = String::new();
    let mut strips: Vec<Vec<u32>> = Vec::new();
    let mut strip_types: Vec<u32> = Vec::new();
    let mut prim_count: u32 = 0;
    let mut depth = 0i32; // brace depth WITHIN the material body (excluding the outer `{`)

    while i < lines.len() {
        let trimmed = lines[i].trim();
        let toks: Vec<&str> = trimmed.split_whitespace().collect();

        // Close-brace handling: depth tracks nested blocks (Prim, individual
        // primitives, Idx).  When depth returns to 0 and we see the next
        // `}`, that's the material body's terminator.
        if trimmed == "}" {
            if depth == 0 {
                i += 1;
                break;
            }
            depth -= 1;
            i += 1;
            continue;
        }
        if trimmed == "{" {
            depth += 1;
            i += 1;
            continue;
        }

        if toks.is_empty() {
            i += 1;
            continue;
        }

        match toks[0] {
            "Name" => {
                if let Some(raw) = toks.get(1) {
                    name = raw.trim_matches('"').to_string();
                }
                i += 1;
            }
            "Priority" => {
                // Material priority — ignored for now; render-order is
                // driven by AlphaMode + depth_bias.
                i += 1;
            }
            "Prim" => {
                prim_count = toks.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                // Opening `{` is either trailing on this line or on the next.
                if toks.last().copied() != Some("{") {
                    // Look ahead for `{`.
                    if let Some(next) = lines.get(i + 1)
                        && next.trim() == "{"
                    {
                        i += 1;
                    }
                }
                depth += 1; // entered Prim block
                i += 1;
            }
            "Type" => {
                // TRISTRIP / TRILIST / etc.  Only TRISTRIP appears in the
                // FX meshes we've seen; defaulting strip_type to 1
                // (normal winding) matches `parse_mod`'s convention.
                i += 1;
            }
            "Idx" => {
                // Format: `Idx K { i0 i1 i2 … }`.  The indices can be
                // on the same line as the opening `{` (common case in
                // .mesh) or split across lines.  We accept both.
                let idx_count: usize = toks.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                let mut accumulated: Vec<u32> = Vec::with_capacity(idx_count);
                // First, harvest any indices on this line after `{`.
                let mut started = false;
                for tok in toks.iter().skip(2) {
                    if *tok == "{" {
                        started = true;
                        continue;
                    }
                    if *tok == "}" {
                        break;
                    }
                    if started && let Ok(v) = tok.parse::<u32>() {
                        accumulated.push(v);
                    }
                }
                i += 1;
                // Continue across follow-on lines if we didn't collect enough.
                while i < lines.len() && accumulated.len() < idx_count {
                    let line_trim = lines[i].trim();
                    if line_trim == "}" {
                        break;
                    }
                    for tok in line_trim.split_whitespace() {
                        if tok == "{" || tok == "}" {
                            continue;
                        }
                        if let Ok(v) = tok.parse::<u32>() {
                            accumulated.push(v);
                        }
                    }
                    i += 1;
                }
                // Consume the closing `}` of the Idx block if we stopped
                // on it.
                if i < lines.len() && lines[i].trim() == "}" {
                    i += 1;
                }
                strips.push(accumulated);
                strip_types.push(1); // TRISTRIP normal winding
            }
            _ => {
                i += 1;
            }
        }
    }

    let mat = Oni2Material {
        name,
        diffuse: [1.0, 1.0, 1.0],
        texture_name: None,
        primitive_count: prim_count,
        packet_count: 1,
        passes: Vec::new(),
    };
    (i, mat, strips, strip_types)
}

#[cfg(test)]
mod mesh_parse_tests {
    use super::*;

    /// Smallest plausible `.mesh`: one position, one normal, one color,
    /// one uv, one adjunct, one material with one primitive (degenerate
    /// 3-index strip).  Exercises the section dispatch + adjunct +
    /// material reader without depending on filesystem assets.
    #[test]
    fn parses_minimal_mesh() {
        let text = "{
\tSkinned 0
\tPosSkin 0
\tPos 3 {
\t\t0.0\t0.0\t0.0
\t\t1.0\t0.0\t0.0
\t\t0.0\t1.0\t0.0
\t}
\tNrm 3 {
\t\t0.0\t1.0\t0.0
\t\t0.0\t1.0\t0.0
\t\t0.0\t1.0\t0.0
\t}
\tCpv 3 {
\t\t1.0\t1.0\t1.0\t1.0
\t\t1.0\t1.0\t1.0\t1.0
\t\t1.0\t1.0\t1.0\t1.0
\t}
\tTex0 3 {
\t\t0.0\t0.0
\t\t1.0\t0.0
\t\t0.0\t1.0
\t}
\tTex1 0
\tAdj 3 {
\t\tP 0
\t\tN 0
\t\tC0 0
\t\tT0 0
\t\tP 1
\t\tN 1
\t\tC0 1
\t\tT0 1
\t\tP 2
\t\tN 2
\t\tC0 2
\t\tT0 2
\t}
\tMtl 1 {
\t\t{
\t\t\tName \"test_mat\"
\t\t\tPriority 32
\t\t\tPrim 1 {
\t\t\t\t{
\t\t\t\t\tType TRISTRIP
\t\t\t\t\tPriority 32
\t\t\t\t\tIdx 3 { 0 1 2 }
\t\t\t\t}
\t\t\t}
\t\t}
\t}
\tOffset 0
}
";
        let m = parse_mesh(text, "").expect("parse_mesh accepts the top-level brace");
        assert_eq!(m.vertices.len(), 3);
        assert_eq!(m.normals.len(), 3);
        assert_eq!(m.colors.len(), 3);
        assert_eq!(m.tex_coords.len(), 3);
        assert_eq!(m.materials.len(), 1);
        assert_eq!(m.materials[0].name, "test_mat");
        assert_eq!(m.materials[0].primitive_count, 1);
        assert_eq!(m.packets.len(), 1);
        assert_eq!(m.packets[0].strips.len(), 1);
        assert_eq!(m.packets[0].strips[0], vec![0, 1, 2]);
        assert_eq!(m.packets[0].adjuncts.len(), 3);
        // First adjunct's index pointers
        let a = &m.packets[0].adjuncts[0];
        assert_eq!(a.vertex_idx, 0);
        assert_eq!(a.normal_idx, 0);
        assert_eq!(a.color_idx, 0);
        assert_eq!(a.tex1_idx, 0);
        // Identity bone for unskinned FX mesh
        assert_eq!(m.bone_world_positions, vec![[0.0, 0.0, 0.0]]);
        assert_eq!(m.bone_rotations, vec![[0.0, 0.0, 0.0, 1.0]]);
        assert!(!m.world_space_verts);
    }

    #[test]
    fn rejects_non_mesh_files() {
        // A `.mod` v1.10 header should not parse as a .mesh.
        assert!(parse_mesh("version: 1.10\nverts: 3\n", "").is_none());
    }

    /// Real-asset probe: load the shipped `blast_fire.mesh` from the
    /// adjacent `oni2/zips/assets` tree and verify the section counts
    /// the file declares in its own headers.  Skipped silently if the
    /// asset isn't reachable from the test's working directory (CI
    /// without the asset tree, etc.).
    #[test]
    fn parses_real_blast_fire_mesh() {
        let path = "../oni2/zips/assets/Entity/BlastFire/blast_fire.mesh";
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("skipping: {} not reachable from cwd", path);
            return;
        };
        let m = parse_mesh(&text, "").expect("blast_fire.mesh has top-level `{`");
        assert_eq!(m.vertices.len(), 288, "Pos count");
        assert_eq!(m.normals.len(), 288, "Nrm count");
        assert_eq!(m.colors.len(), 288, "Cpv count");
        assert_eq!(m.tex_coords.len(), 288, "Tex0 count");
        assert_eq!(m.materials.len(), 1, "Mtl count");
        assert_eq!(m.materials[0].name, "blast_fire", "Mtl Name");
        assert_eq!(m.materials[0].primitive_count, 23, "Prim count");
        assert_eq!(m.packets.len(), 1, "one packet per material");
        assert_eq!(m.packets[0].strips.len(), 23, "23 tri-strips");
        // Every strip has 24 indices per the file.
        for (idx, s) in m.packets[0].strips.iter().enumerate() {
            assert_eq!(s.len(), 24, "strip #{} should have 24 indices", idx);
        }
        // Adjuncts: 288 entries, all `bone_idx = 0` (unskinned mesh).
        assert_eq!(m.packets[0].adjuncts.len(), 288);
        assert!(m.packets[0].adjuncts.iter().all(|a| a.bone_idx == 0));
    }
}
