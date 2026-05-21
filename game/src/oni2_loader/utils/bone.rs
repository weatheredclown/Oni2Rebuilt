/*
 * oni2_loader/utils/bone.rs — bone-space vertex conversion utilities.
 *
 * convert_world_to_bone_local: transforms world-space vertex positions in an
 * Oni2Model to bone-local coordinates by subtracting each bone's bind-pose
 * world position.  Normalises win32 (world-space) models to match the PS2 /
 * ASCII bone-local format so both animate identically.
 * compute_inverse_bind_poses: builds SkinnedMeshInverseBindposes from skeleton.
 */
use crate::oni2_loader::parsers::types::{Oni2Model, Oni2Skeleton};
use bevy::prelude::*;

/// Convert world-space vertices to bone-local by subtracting each vertex's
/// bind-pose bone position. This normalizes win32 (world-space) models to match
/// the PS2/ASCII bone-local format so all models animate the same way.
pub fn convert_world_to_bone_local(model: &mut Oni2Model, skel: &Oni2Skeleton) {
    // Build a lookup: vertex_index → bone position (from skeleton bind pose).
    // A vertex may be referenced by multiple adjuncts in different packets with
    // different bones. We use the first bone assignment we find per vertex.
    let mut vert_bone_pos: Vec<Option<[f32; 3]>> = vec![None; model.vertices.len()];

    for packet in &model.packets {
        for adj in &packet.adjuncts {
            let vi = adj.vertex_idx as usize;
            if vi >= model.vertices.len() || vert_bone_pos[vi].is_some() {
                continue;
            }
            let global_bone = if !packet.bone_map.is_empty() {
                *packet.bone_map.get(adj.bone_idx as usize).unwrap_or(&0) as usize
            } else {
                adj.bone_idx as usize
            };
            if let Some(bp) = skel.positions.get(global_bone) {
                vert_bone_pos[vi] = Some(*bp);
            }
        }
    }

    // Subtract bone position from each vertex (bind-pose rotation is identity)
    for (vi, vert) in model.vertices.iter_mut().enumerate() {
        if let Some(bp) = vert_bone_pos[vi] {
            vert[0] -= bp[0];
            vert[1] -= bp[1];
            vert[2] -= bp[2];
        }
    }

    model.world_space_verts = false;
}

/// Compute per-bone global transforms from one animation frame.
/// Uses XZY euler convention and parent-chain accumulation per AGE engine.
/// Returns `BoneEvalResult` with per-bone transforms plus any root XZ
/// translation that was stripped — see the type doc.
pub struct BoneEvalResult {
    /// Per-bone (rotation, world_position).
    pub bones: Vec<(Quat, Vec3)>,
    /// Channel-space root XZ translation relative to the skeleton rest
    /// offset, captured BEFORE stripping.  `Vec3::ZERO` when stripping
    /// didn't happen (or when the root bone has no translation channels).
    /// Consumers that want to apply the anim's intended root motion to
    /// gameplay (e.g. a running lunge pushing the character forward) can
    /// read this and feed it into velocity/position.
    pub stripped_root_offset: Vec3,
}

pub fn compute_animated_bone_transforms(
    skel: &Oni2Skeleton,
    frame_channels: &[f32],
    strip_root_xz: bool,
) -> BoneEvalResult {
    let num_bones = skel.positions.len();
    let mut result = vec![(Quat::IDENTITY, Vec3::ZERO); num_bones];
    let mut stripped_root_offset = Vec3::ZERO;

    let mut ch_idx = 0;
    let has_flags = !skel.channel_is_rot.is_empty();

    for i in 0..num_bones {
        if !has_flags {
            // Legacy struct fallback — used when the .skel file omits
            // explicit `transX/Y/Z` / `rotX/Y/Z` channel declarations
            // (e.g. `edi.skel`).  When per-bone flags ARE present
            // (e.g. `kno.skel`) the dynamic path below handles channel
            // layout.
            //
            // Layout mirrors `crAnimFrame::Pose`
            // (legacy `crAnimFrame`):
            //
            //   • channels 0/1/2     = root TRANSLATION (overrides
            //                          bone 0's rest-pose position).
            //   • channels (i*3 + 3) = bone i's EULER (x, y, z) — for
            //                          ALL bones, including i==0.
            //
            // The original Rust port had bone 0's euler at channels
            // 0/1/2 and translation at 3/4/5 — the inverse of the C++
            // layout.  That worked silently for characters with
            // channel-flag-bearing skeletons (those go down the
            // dynamic-path branch and never touch this code), but for
            // characters like edi the per-frame Z-translation (stride
            // progression up to `stride_z` ~= 2.33 over a run cycle)
            // landed on bone 0's Z-rotation slot and produced a
            // visible barrel-roll across the loop.
            let ch_base = i * 3 + 3;
            let euler_x = *frame_channels.get(ch_base).unwrap_or(&0.0);
            let euler_y = *frame_channels.get(ch_base + 1).unwrap_or(&0.0);
            let euler_z = *frame_channels.get(ch_base + 2).unwrap_or(&0.0);
            let local_rot = Quat::from_euler(EulerRot::YZX, euler_y, euler_z, euler_x);

            if i == 0 {
                // Root translation — channels 0/1/2 are stored as
                // DELTAS from the rest-pose offset, not absolute local
                // positions.  Match what the dynamic-mapping path
                // does (`bone.rs` ~line 169): `final = channel + rest`.
                //
                // Originally this path used the channel as absolute,
                // which put edi's hip at world Y=0 instead of at the
                // rest height (`+1.037869` in edi.skel), sinking the
                // character ~1m into the ground.
                //
                // `unwrap_or(&0.0)` (not the rest offset) is correct
                // for the channel-too-short fallback: if the channel
                // is absent, the delta is 0, and the final position is
                // exactly the rest offset.
                let ch_dx = *frame_channels.first().unwrap_or(&0.0);
                let ch_dy = *frame_channels.get(1).unwrap_or(&0.0);
                let ch_dz = *frame_channels.get(2).unwrap_or(&0.0);
                let mut tx = ch_dx + skel.local_offsets[0][0];
                let ty = ch_dy + skel.local_offsets[0][1];
                let mut tz = ch_dz + skel.local_offsets[0][2];
                if strip_root_xz {
                    // The channel-space delta IS the gameplay-driven
                    // root motion — capture it for downstream
                    // consumption, then pin tx/tz to the rest offset
                    // so the visual stays anchored.  Y left as-is so
                    // hip bob still shows.
                    stripped_root_offset = Vec3::new(ch_dx, 0.0, ch_dz);
                    tx = skel.local_offsets[0][0];
                    tz = skel.local_offsets[0][2];
                }
                result[0] = (local_rot, Vec3::new(tx, ty, tz));
            } else {
                let local_offset = Vec3::from(skel.local_offsets[i]);
                let parent_idx = skel.parent_indices[i].unwrap_or(0);
                let (parent_rot, parent_pos) = result[parent_idx];

                let global_rot = parent_rot * local_rot;
                let global_pos = parent_rot.mul_vec3(local_offset) + parent_pos;

                result[i] = (global_rot, global_pos);
            }
        } else {
            // Evaluated dynamic mapping bounds off explicitly declared AST variables
            let ch = &skel.channels[i];

            let tx = if ch.has_trans_x {
                let v = *frame_channels.get(ch_idx).unwrap_or(&0.0);
                ch_idx += 1;
                v
            } else {
                0.0
            };
            let ty = if ch.has_trans_y {
                let v = *frame_channels.get(ch_idx).unwrap_or(&0.0);
                ch_idx += 1;
                v
            } else {
                0.0
            };
            let tz = if ch.has_trans_z {
                let v = *frame_channels.get(ch_idx).unwrap_or(&0.0);
                ch_idx += 1;
                v
            } else {
                0.0
            };

            let euler_x = if ch.has_rot_x {
                let v = *frame_channels.get(ch_idx).unwrap_or(&0.0);
                ch_idx += 1;
                v
            } else {
                0.0
            };
            let euler_y = if ch.has_rot_y {
                let v = *frame_channels.get(ch_idx).unwrap_or(&0.0);
                ch_idx += 1;
                v
            } else {
                0.0
            };
            let euler_z = if ch.has_rot_z {
                let v = *frame_channels.get(ch_idx).unwrap_or(&0.0);
                ch_idx += 1;
                v
            } else {
                0.0
            };

            let local_rot = Quat::from_euler(EulerRot::YZX, euler_y, euler_z, euler_x);

            let mut final_tx = tx + skel.local_offsets[i][0];
            let final_ty = ty + skel.local_offsets[i][1];
            let mut final_tz = tz + skel.local_offsets[i][2];
            if i == 0 && strip_root_xz {
                // Capture the channel-space translation relative to rest
                // before overriding.  Y left at 0 — vertical is preserved.
                stripped_root_offset = Vec3::new(tx, 0.0, tz);
                final_tx = skel.local_offsets[i][0];
                final_tz = skel.local_offsets[i][2];
            }
            let local_pos = Vec3::new(final_tx, final_ty, final_tz);

            if i == 0 {
                result[0] = (local_rot, local_pos);
            } else {
                let parent_idx = skel.parent_indices[i].unwrap_or(0);
                let (parent_rot, parent_pos) = result[parent_idx];

                let global_rot = parent_rot * local_rot;
                let global_pos = parent_rot.mul_vec3(local_pos) + parent_pos;

                result[i] = (global_rot, global_pos);
            }
        }
    }

    BoneEvalResult {
        bones: result,
        stripped_root_offset,
    }
}

/// Heuristic: does the `.mod` file's vertex array encode positions in
/// **entity-local space** (a.k.a. "model space") rather than the
/// bone-local space `build_skinned_meshes_by_material` and the non-skinned
/// bone-local branch of `build_meshes_by_material` assume?
///
/// **Why this matters.** Two different `.mod` authoring conventions exist:
///
/// 1. **Bone-local** (typical for character meshes): each vertex is stored
///    relative to its assigned bone's bind position. The mesh build adds
///    `bone_pos` to recover the entity-local bind position.
///
/// 2. **Model-local** (observed on animated props like the IAControlDoor
///    sliding door): each vertex is stored at its full entity-local
///    bind-pose position. Adding `bone_pos` would *double-offset* it,
///    which is exactly the symptom seen on `actor_Door1` — the mesh
///    rendered ~1m further along the slide axis than its collider.
///
/// The `.mod` file format itself doesn't disambiguate, so we infer from
/// the data: per bone that has both `.mod` adjuncts and a corresponding
/// `.bnd` sub-bound, compute two candidate centroids in entity-local
/// Bevy space — one assuming bone-local (add `bone_pos` then negate) and
/// one assuming model-local (just negate). Whichever is closer to the
/// nearest `.bnd` sub-bound centroid wins. Majority vote across bones
/// decides the file's convention.
///
/// **Two paths, two tests** — see the module's `tests` submodule:
///
/// - `world_space_seam_vertex_round_trips_through_both_bones` pins the
///   *non-skinned world-space* path, which intentionally bypasses
///   [`convert_world_to_bone_local`].
/// - `detects_model_local_door_mod` pins this heuristic returning true
///   on door-shaped synthetic data.
/// - `detects_bone_local_character_mod_stays_bone_local` pins it
///   returning false on character-shaped synthetic data, so this fix
///   can't silently break characters.
///
/// If a future change can't keep all three of those tests passing under
/// one implementation, that's the signal that the shared code (likely
/// [`convert_world_to_bone_local`] or the centroid-comparison itself)
/// needs to fork into per-format branches with the divergent invariants
/// each pinned by its own test.
pub fn is_model_local_heuristic(
    model: &Oni2Model,
    skel: &Oni2Skeleton,
    bound: &crate::oni2_loader::parsers::types::Oni2Bound,
) -> bool {
    use std::collections::HashMap;

    if skel.positions.is_empty() || bound.sub_bounds.is_empty() || model.packets.is_empty() {
        return false;
    }

    struct BoneCentroids {
        model_local_sum: Vec3,
        bone_local_sum: Vec3,
        n: usize,
    }
    let mut by_bone: HashMap<usize, BoneCentroids> = HashMap::new();

    for packet in &model.packets {
        for adj in &packet.adjuncts {
            let global_bone = if !packet.bone_map.is_empty() {
                *packet.bone_map.get(adj.bone_idx as usize).unwrap_or(&0) as usize
            } else {
                adj.bone_idx as usize
            };
            let raw = model
                .vertices
                .get(adj.vertex_idx as usize)
                .copied()
                .unwrap_or([0.0; 3]);
            let bone_pos = skel.positions.get(global_bone).copied().unwrap_or([0.0; 3]);

            // Model-local interpretation: raw is already entity-local Oni2;
            // entity-local Bevy is just X/Z negate.
            let ml = Vec3::new(-raw[0], raw[1], -raw[2]);
            // Bone-local interpretation: raw + bone_pos is entity-local Oni2
            // (matches what `build_skinned_meshes_by_material` produces).
            let bl = Vec3::new(
                -(raw[0] + bone_pos[0]),
                raw[1] + bone_pos[1],
                -(raw[2] + bone_pos[2]),
            );

            let entry = by_bone.entry(global_bone).or_insert(BoneCentroids {
                model_local_sum: Vec3::ZERO,
                bone_local_sum: Vec3::ZERO,
                n: 0,
            });
            entry.model_local_sum += ml;
            entry.bone_local_sum += bl;
            entry.n += 1;
        }
    }

    // Collect bnd centroids (already in entity-local Bevy per the bnd parser).
    let bnd_centroids: Vec<Vec3> = bound
        .sub_bounds
        .iter()
        .map(|sub| Vec3::new(sub.centroid[0], sub.centroid[1], sub.centroid[2]))
        .collect();

    let nearest_dist = |target: Vec3| -> f32 {
        bnd_centroids
            .iter()
            .map(|b| target.distance(*b))
            .fold(f32::INFINITY, f32::min)
    };

    let mut model_local_score = 0.0_f32;
    let mut bone_local_score = 0.0_f32;
    let mut votes = 0_usize;

    for (_, c) in by_bone {
        if c.n == 0 {
            continue;
        }
        let n = c.n as f32;
        let ml_centroid = c.model_local_sum / n;
        let bl_centroid = c.bone_local_sum / n;
        model_local_score += nearest_dist(ml_centroid);
        bone_local_score += nearest_dist(bl_centroid);
        votes += 1;
    }

    if votes == 0 {
        return false;
    }

    // Lower aggregate distance from candidate centroid to nearest bnd centroid wins.
    // Tie or near-tie defaults to bone-local (the current behavior).
    model_local_score + 1e-3 < bone_local_score
}

/// Compute inverse bind-pose matrices for GPU skinning.
/// Bind pose is translation-only (no rotation), so inverse is just negated translation.
/// Positions are in Oni2 coordinates; we apply X/Z negate for Bevy space.
pub fn compute_inverse_bind_poses(skel: &Oni2Skeleton) -> Vec<Mat4> {
    skel.positions
        .iter()
        .map(|pos| {
            // Bind-pose matrix: translation with X/Z negate for Bevy coordinate system
            let bind = Mat4::from_translation(Vec3::new(-pos[0], pos[1], -pos[2]));
            bind.inverse()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oni2_loader::parsers::mesh::build_meshes_by_material;
    use crate::oni2_loader::parsers::types::{
        Oni2Adjunct, Oni2Bound, Oni2Material, Oni2Model, Oni2Packet, Oni2Skeleton, Oni2SubBound,
    };
    use bevy::mesh::Mesh;

    /// Round-trip invariant for the *non-skinned* world-space → mesh-bake
    /// pipeline (sliding doors and similar prop entities): a vertex
    /// referenced by adjuncts in different bones (a "seam" vertex — the
    /// touching edges of two sliding door panels are the canonical case)
    /// must land at its original world position no matter which packet's
    /// adjunct produced the output mesh entry.
    ///
    /// The bug this pins: when `spawn.rs` ran `convert_world_to_bone_local`
    /// on entities that take the *non-skinned* mesh path, the conversion
    /// recorded only the *first-seen* bone for each shared vertex and
    /// subtracted that bone's rest position once.
    /// `build_meshes_by_material` then iterated every adjunct and added
    /// the *per-adjunct* bone's rest position back. When the two
    /// disagreed, the rebuilt vertex was offset by
    /// `(other_bone - first_seen_bone)` — visible in-game as door panels
    /// rendering slightly open even though the collision bound was at the
    /// closed position.
    ///
    /// The fix: skip `convert_world_to_bone_local` whenever the entity
    /// takes the non-skinned mesh path. This test models that pipeline
    /// faithfully (no convert call) and asserts every adjunct that
    /// references the same source vertex bakes to the same world position.
    ///
    /// What this test catches:
    ///   - Re-introducing the unconditional convert call on the
    ///     non-skinned path.
    ///   - A "fix" to `convert_world_to_bone_local` that doesn't preserve
    ///     world positions on shared verts but is still wired into the
    ///     door pipeline.
    ///   - A change to `build_meshes_by_material` that breaks the
    ///     `world_space_verts == true` branch.
    #[test]
    fn world_space_seam_vertex_round_trips_through_both_bones() {
        // Two bones spaced 2m apart along X. Think: left door panel
        // pivot at origin, right door panel pivot 2m to the right.
        let skel = Oni2Skeleton {
            positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            parent_indices: vec![None, Some(0)],
            names: vec!["left".into(), "right".into()],
            local_offsets: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            channels: vec![],
            channel_is_rot: vec![],
        };

        // Three vertices, all in world space. Vertex 0 sits on the seam
        // between the two panels at world X=1.5 — the bug case.
        let vertices: Vec<[f32; 3]> = vec![[1.5, 1.0, 0.0], [1.0, 1.0, 0.0], [2.0, 1.0, 0.0]];

        let normals = vec![[0.0, 1.0, 0.0]; 3];
        let colors = vec![[1.0, 1.0, 1.0, 1.0]; 3];
        let tex_coords = vec![[0.0, 0.0]; 3];

        let make_packet = |local_to_global: u32| Oni2Packet {
            adjuncts: (0..3)
                .map(|i| Oni2Adjunct {
                    vertex_idx: i,
                    normal_idx: i,
                    color_idx: i,
                    tex1_idx: i as i32,
                    bone_idx: 0, // local index — bone_map remaps
                })
                .collect(),
            strips: vec![vec![0, 1, 2]],
            strip_types: vec![1],
            material_index: 0,
            bone_map: vec![local_to_global],
        };

        // Packet 0: every adjunct → global bone 0 (left panel).
        // Packet 1: every adjunct → global bone 1 (right panel).
        // Vertex 0 is referenced from both, with different effective bones.
        let packets = vec![make_packet(0), make_packet(1)];

        let mut model = Oni2Model {
            vertices,
            normals,
            colors,
            tex_coords,
            materials: vec![Oni2Material {
                name: "test".into(),
                diffuse: [1.0, 1.0, 1.0],
                texture_name: None,
                primitive_count: 0,
                packet_count: 0,
                passes: vec![],
            }],
            packets,
            bone_world_positions: vec![],
            bone_rotations: vec![],
            world_space_verts: true,
        };

        // Mirror the spawn-time setup the non-skinned mesh path runs:
        // copy skeleton positions into bone_world_positions so mesh.rs's
        // bone-local branch can re-add them.
        model.bone_world_positions = skel.positions.clone();
        model.bone_rotations = vec![[0.0, 0.0, 0.0, 1.0]; skel.positions.len()];

        // Run the *fixed* non-skinned pipeline: no `convert_world_to_bone_local`
        // call (the fix gates it on `use_gpu_skinning`). `mesh.rs` then
        // takes the `world_space_verts == true` branch and uses vertices
        // as-is. If a future change re-runs the conversion on this path,
        // the assertions below catch it.
        let sub_meshes = build_meshes_by_material(&model);

        // Walk all produced submesh positions in adjunct order. With one
        // material and 6 total adjuncts (3 per packet × 2 packets), the
        // output is a single 6-vertex mesh. Indices [0..3] come from
        // packet 0; [3..6] from packet 1. Both packets reference the same
        // 3 source vertices, so the per-packet outputs must match.
        let mut all_positions: Vec<[f32; 3]> = Vec::new();
        for (_mat_idx, mesh) in sub_meshes {
            let attr = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .expect("position attribute");
            let positions = attr.as_float3().expect("position attribute is float3");
            all_positions.extend_from_slice(positions);
        }

        assert_eq!(
            all_positions.len(),
            6,
            "expected 6 output verts (3 per packet × 2 packets), got {}",
            all_positions.len()
        );

        for i in 0..3 {
            let p0 = all_positions[i]; // packet 0, vertex i
            let p1 = all_positions[i + 3]; // packet 1, vertex i (same source)
            for axis in 0..3 {
                assert!(
                    (p0[axis] - p1[axis]).abs() < 1e-4,
                    "vertex {} produced different baked positions across packets — \
                     packet 0: {:?}, packet 1: {:?}. \
                     Likely cause: convert_world_to_bone_local picked the first-seen \
                     bone (per packet 0's adjuncts) while build_meshes_by_material \
                     added the other bone's offset for packet 1's adjuncts. \
                     Net offset = (other_bone - first_seen_bone), \
                     visible in-game as door panels rendering offset from collision.",
                    i,
                    p0,
                    p1
                );
            }
        }
    }

    /// Build a synthetic `(Oni2Model, Oni2Skeleton, Oni2Bound)` shaped like
    /// the sliding door (`actor_Door1` / `IAControlDoor`):
    /// - 3 bones: root at origin, two panel pivots at z=±1 in Oni2.
    /// - `.mod` verts encoded in **entity-local Oni2 space** (model-local
    ///   convention) — span the full closed-door extent.
    /// - `.bnd` sub-bounds with centroids matching the panel surfaces in
    ///   entity-local Bevy.
    fn make_model_local_door_fixture() -> (Oni2Model, Oni2Skeleton, Oni2Bound) {
        // 3 bones (root + two panel pivots, sliding doors animate transZ).
        let skel = Oni2Skeleton {
            positions: vec![[0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 0.0, 1.0]],
            parent_indices: vec![None, Some(0), Some(0)],
            names: vec!["root".into(), "panel_left".into(), "panel_right".into()],
            local_offsets: vec![[0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 0.0, 1.0]],
            channels: vec![],
            channel_is_rot: vec![],
        };

        // Verts in entity-local Oni2: panel 1 spans z∈[-2, +1], panel 2
        // mirrors it at z∈[-1, +2]. Entity-local bevy after X/Z negate:
        // panel 1 z∈[-1, +2], panel 2 z∈[-2, +1] — the same range as
        // the .bnd sub-bounds below.
        let vertices: Vec<[f32; 3]> = vec![
            // Panel 1 (assigned to bone 1 at z=-1 Oni2 / z=+1 Bevy)
            [0.0, 0.0, 1.0],
            [0.0, 4.5, 1.0],
            [0.0, 0.0, -2.0],
            [0.0, 4.5, -2.0],
            // Panel 2 (assigned to bone 2 at z=+1 Oni2 / z=-1 Bevy)
            [0.0, 0.0, -1.0],
            [0.0, 4.5, -1.0],
            [0.0, 0.0, 2.0],
            [0.0, 4.5, 2.0],
        ];
        let normals = vec![[0.0, 1.0, 0.0]; 8];
        let colors = vec![[1.0; 4]; 8];
        let tex_coords = vec![[0.0; 2]; 8];

        let make_packet = |verts: &[u32], local_to_global: u32| Oni2Packet {
            adjuncts: verts
                .iter()
                .map(|&vi| Oni2Adjunct {
                    vertex_idx: vi,
                    normal_idx: vi,
                    color_idx: 0,
                    tex1_idx: 0,
                    bone_idx: 0,
                })
                .collect(),
            strips: vec![(0..verts.len() as u32).collect()],
            strip_types: vec![1],
            material_index: 0,
            bone_map: vec![local_to_global],
        };
        let packets = vec![
            make_packet(&[0, 1, 2, 3], 1), // panel 1 → global bone 1
            make_packet(&[4, 5, 6, 7], 2), // panel 2 → global bone 2
        ];

        let model = Oni2Model {
            vertices,
            normals,
            colors,
            tex_coords,
            materials: vec![Oni2Material {
                name: "test".into(),
                diffuse: [1.0; 3],
                texture_name: None,
                primitive_count: 0,
                packet_count: 0,
                passes: vec![],
            }],
            packets,
            bone_world_positions: vec![],
            bone_rotations: vec![],
            world_space_verts: false,
        };

        // Bnd sub-bounds with centroids matching the panel surfaces in
        // entity-local Bevy (after the bnd parser's X/Z negate). Panel 1
        // surface centroid in Oni2 = (0, 2.25, -0.5), in Bevy = (0, 2.25, 0.5).
        let bound = Oni2Bound {
            sub_bounds: vec![
                Oni2SubBound {
                    bound_type: None,
                    material_type: None,
                    vertices: vec![],
                    centroid: [0.0, 2.25, 0.5],
                    edges: vec![],
                    quads: vec![],
                    tris: vec![],
                },
                Oni2SubBound {
                    bound_type: None,
                    material_type: None,
                    vertices: vec![],
                    centroid: [0.0, 2.25, -0.5],
                    edges: vec![],
                    quads: vec![],
                    tris: vec![],
                },
            ],
            centroid: [0.0, 2.25, 0.0],
        };

        (model, skel, bound)
    }

    /// Pins: the heuristic detects the door's `.mod` as model-local. If
    /// this fails after a future change, the IAControlDoor render-vs-collider
    /// drift bug has regressed.
    #[test]
    fn detects_model_local_door_mod() {
        let (model, skel, bound) = make_model_local_door_fixture();
        assert!(
            is_model_local_heuristic(&model, &skel, &bound),
            "door-shaped fixture (verts at entity-local positions, panels at \
             ±1.5 bone-local-baked vs bnd at ±0.5) should be detected as \
             model-local"
        );
    }

    /// Pins: the heuristic returns false for a character-shaped fixture
    /// (verts authored bone-local, bnd centroid coincides with the bone +
    /// small offset). If this fails after a future change, characters will
    /// flip into the model-local code path and render incorrectly.
    #[test]
    fn detects_bone_local_character_mod_stays_bone_local() {
        // One bone at entity-local Bevy (0, 1.6, 0) — think "head bone".
        // Bnd centroid at (0.05, 1.7, 0.02) entity-local Bevy — the head
        // surface, slightly forward and above the bone pivot.
        let skel = Oni2Skeleton {
            positions: vec![[0.0, 1.6, 0.0]], // Oni2 (no negate needed; X=Z=0)
            parent_indices: vec![None],
            names: vec!["head".into()],
            local_offsets: vec![[0.0, 1.6, 0.0]],
            channels: vec![],
            channel_is_rot: vec![],
        };

        // Bone-local raw verts: small offsets around the bone (head surface).
        let vertices: Vec<[f32; 3]> = vec![
            [0.05, 0.10, 0.02],
            [-0.05, 0.10, 0.02],
            [0.05, 0.10, -0.02],
            [-0.05, 0.10, -0.02],
        ];
        let model = Oni2Model {
            vertices: vertices.clone(),
            normals: vec![[0.0, 1.0, 0.0]; vertices.len()],
            colors: vec![[1.0; 4]; vertices.len()],
            tex_coords: vec![[0.0; 2]; vertices.len()],
            materials: vec![Oni2Material {
                name: "test".into(),
                diffuse: [1.0; 3],
                texture_name: None,
                primitive_count: 0,
                packet_count: 0,
                passes: vec![],
            }],
            packets: vec![Oni2Packet {
                adjuncts: (0..vertices.len() as u32)
                    .map(|i| Oni2Adjunct {
                        vertex_idx: i,
                        normal_idx: i,
                        color_idx: 0,
                        tex1_idx: 0,
                        bone_idx: 0,
                    })
                    .collect(),
                strips: vec![(0..vertices.len() as u32).collect()],
                strip_types: vec![1],
                material_index: 0,
                bone_map: vec![0], // local 0 → global 0 (head)
            }],
            bone_world_positions: vec![],
            bone_rotations: vec![],
            world_space_verts: false,
        };

        // Bnd centroid: head surface in entity-local Bevy. Bone is at
        // (0, 1.6, 0) Oni2; verts cluster at +Y=0.10 above bone, so the
        // baked entity-local centroid is around (0, 1.7, 0). The bnd's
        // own bounding-box centroid is offset slightly.
        let bound = Oni2Bound {
            sub_bounds: vec![Oni2SubBound {
                bound_type: None,
                material_type: None,
                vertices: vec![],
                centroid: [0.0, 1.70, 0.0],
                edges: vec![],
                quads: vec![],
                tris: vec![],
            }],
            centroid: [0.0, 1.70, 0.0],
        };

        assert!(
            !is_model_local_heuristic(&model, &skel, &bound),
            "character-shaped fixture (bone-local verts) must NOT be detected \
             as model-local; flipping it would push every character render \
             into the wrong path"
        );
    }
}
