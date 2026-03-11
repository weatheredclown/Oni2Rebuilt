use bevy::prelude::*;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use super::types::{Oni2Model, Oni2Skeleton};

/// Build one Bevy Mesh per material from an Oni2Model.
/// Returns (material_index, Mesh) pairs so the caller can assign textures.
pub fn build_meshes_by_material(model: &Oni2Model) -> Vec<(usize, Mesh)> {
    // Group packets by material index
    let mat_count = model.materials.len().max(1);
    let mut per_mat: Vec<(Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 4]>, Vec<u32>)> =
        (0..mat_count).map(|_| (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new())).collect();

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
                let adj = &packet.adjuncts[adj_idx as usize];

                let raw_pos = model.vertices.get(adj.vertex_idx as usize)
                    .copied().unwrap_or([0.0; 3]);
                let raw_norm = model.normals.get(adj.normal_idx as usize)
                    .copied().unwrap_or([0.0, 1.0, 0.0]);

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
                    let bone_offset = model.bone_world_positions.get(global_bone)
                        .copied().unwrap_or([0.0; 3]);
                    let bone_rot = if !model.bone_rotations.is_empty() {
                        let r = model.bone_rotations.get(global_bone)
                            .copied().unwrap_or([0.0, 0.0, 0.0, 1.0]);
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

                // Left-handed → right-handed: negate X and Z (180° Y rotation, not a mirror)
                let pos = [-transformed[0], transformed[1], -transformed[2]];
                let norm = [-rotated_norm.x, rotated_norm.y, -rotated_norm.z];
                let raw_uv = if adj.tex1_idx >= 0 {
                    model.tex_coords.get(adj.tex1_idx as usize)
                        .copied().unwrap_or([0.0; 2])
                } else {
                    [0.0; 2]
                };
                let uv = [raw_uv[0], 1.0 - raw_uv[1]]; // DirectX V → OpenGL V
                let color = model.colors.get(adj.color_idx as usize)
                    .copied().unwrap_or([1.0, 1.0, 1.0, 1.0]);

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
    let mut per_mat: Vec<(Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 4]>, Vec<[u16; 4]>, Vec<[f32; 4]>, Vec<u32>)> =
        (0..mat_count).map(|_| (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new())).collect();

    for packet in &model.packets {
        let mat_idx = packet.material_index.min(mat_count - 1);
        let mat = model.materials.get(mat_idx);
        let mat_diffuse = mat.map(|m| m.diffuse).unwrap_or([0.8, 0.8, 0.8]);
        let (positions, normals, uvs, colors, joint_indices, joint_weights, indices) = &mut per_mat[mat_idx];

        for (strip_idx, strip) in packet.strips.iter().enumerate() {
            if strip.len() < 3 {
                continue;
            }
            let stype = packet.strip_types.get(strip_idx).copied().unwrap_or(1);

            let mut strip_verts: Vec<u32> = Vec::new();
            for &adj_idx in strip {
                let adj = &packet.adjuncts[adj_idx as usize];

                let raw_pos = model.vertices.get(adj.vertex_idx as usize)
                    .copied().unwrap_or([0.0; 3]);
                let raw_norm = model.normals.get(adj.normal_idx as usize)
                    .copied().unwrap_or([0.0, 1.0, 0.0]);

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
                    model.tex_coords.get(adj.tex1_idx as usize)
                        .copied().unwrap_or([0.0; 2])
                } else {
                    [0.0; 2]
                };
                let uv = [raw_uv[0], 1.0 - raw_uv[1]];
                let color = model.colors.get(adj.color_idx as usize)
                    .copied().unwrap_or([1.0, 1.0, 1.0, 1.0]);
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
    for (mat_idx, (positions, normals, uvs, colors, joint_indices, joint_weights, indices)) in per_mat.into_iter().enumerate() {
        if positions.is_empty() {
            continue;
        }

        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(joint_indices));
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, joint_weights);
        mesh.insert_indices(Indices::U32(indices));

        result.push((mat_idx, mesh));
    }

    result
}

/// Build point cloud meshes (one per material) — each vertex rendered as a tiny triangle "dot".
pub fn build_point_clouds_by_material(model: &Oni2Model) -> Vec<(usize, Mesh)> {
    let mat_count = model.materials.len().max(1);
    let mut per_mat: Vec<(Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 4]>, Vec<u32>)> =
        (0..mat_count).map(|_| (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new())).collect();

    let dot_size = 0.008; // radius of each dot triangle

    for packet in &model.packets {
        let mat_idx = packet.material_index.min(mat_count - 1);
        let mat = model.materials.get(mat_idx);
        let mat_diffuse = mat.map(|m| m.diffuse).unwrap_or([0.8, 0.8, 0.8]);
        let (positions, normals, uvs, colors, indices) = &mut per_mat[mat_idx];

        for adj in &packet.adjuncts {
            let raw_pos = model.vertices.get(adj.vertex_idx as usize)
                .copied().unwrap_or([0.0; 3]);
            let raw_norm = model.normals.get(adj.normal_idx as usize)
                .copied().unwrap_or([0.0, 1.0, 0.0]);

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
                let bone_offset = model.bone_world_positions.get(global_bone)
                    .copied().unwrap_or([0.0; 3]);
                let bone_rot = if !model.bone_rotations.is_empty() {
                    let r = model.bone_rotations.get(global_bone)
                        .copied().unwrap_or([0.0, 0.0, 0.0, 1.0]);
                    Quat::from_xyzw(r[0], r[1], r[2], r[3])
                } else {
                    Quat::IDENTITY
                };
                let rv = bone_rot.mul_vec3(Vec3::new(raw_pos[0], raw_pos[1], raw_pos[2]));
                transformed = [rv.x + bone_offset[0], rv.y + bone_offset[1], rv.z + bone_offset[2]];
                rotated_norm = bone_rot.mul_vec3(Vec3::new(raw_norm[0], raw_norm[1], raw_norm[2]));
            };

            let cx = -transformed[0]; // X+Z negate = 180° Y rotation
            let cy = transformed[1];
            let cz = -transformed[2];
            let norm = [-rotated_norm.x, rotated_norm.y, -rotated_norm.z];
            let uv = if adj.tex1_idx >= 0 {
                model.tex_coords.get(adj.tex1_idx as usize).copied().unwrap_or([0.0; 2])
            } else {
                [0.0; 2]
            };
            let color = model.colors.get(adj.color_idx as usize)
                .copied().unwrap_or([1.0, 1.0, 1.0, 1.0]);
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
