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
            // Legacy struct fallback mapping
            if i == 0 {
                let euler_x = *frame_channels.get(0).unwrap_or(&0.0);
                let euler_y = *frame_channels.get(1).unwrap_or(&0.0);
                let euler_z = *frame_channels.get(2).unwrap_or(&0.0);
                let mut tx = *frame_channels.get(3).unwrap_or(&skel.local_offsets[i][0]);
                let ty = *frame_channels.get(4).unwrap_or(&skel.local_offsets[i][1]);
                let mut tz = *frame_channels.get(5).unwrap_or(&skel.local_offsets[i][2]);
                if strip_root_xz {
                    // Capture the channel-space translation relative to the
                    // rest offset before we overwrite it.  `Y` left at 0 —
                    // vertical root motion is preserved.
                    stripped_root_offset = Vec3::new(
                        tx - skel.local_offsets[i][0],
                        0.0,
                        tz - skel.local_offsets[i][2],
                    );
                    tx = skel.local_offsets[i][0];
                    tz = skel.local_offsets[i][2];
                }
                let rot = Quat::from_euler(EulerRot::YZX, euler_y, euler_z, euler_x);
                result[0] = (rot, Vec3::new(tx, ty, tz));
            } else {
                let ch_base = i * 3 + 3;
                let euler_x = *frame_channels.get(ch_base).unwrap_or(&0.0);
                let euler_y = *frame_channels.get(ch_base + 1).unwrap_or(&0.0);
                let euler_z = *frame_channels.get(ch_base + 2).unwrap_or(&0.0);
                let local_rot = Quat::from_euler(EulerRot::YZX, euler_y, euler_z, euler_x);

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
            let mut final_ty = ty + skel.local_offsets[i][1];
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
