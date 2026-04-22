/*
 * oni2_loader/headik.rs — head inverse-kinematics system.
 *
 * head_ik_setup_system: attaches HeadIkState to newly spawned Oni2 entities.
 * head_ik_system (runs after update_oni2_animation): rate-limited approach of
 * neck+head bones toward a world-space goal, with angle clamping (azimuth/incline
 * split equally between neck and head in spine-local space).
 */
/// Head IK system.
///
/// Algorithm:
///   1. Transform the world-space goal into spine-local space.
///   2. Extract azimuth  = atan2(-x, -z)  and  incline = asin(y/dist).
///   3. Proportionally clip both angles so their direction is preserved.
///   4. Rate-limited approach toward the clipped goal each frame.
///   5. Build a local rotation:  RotX(incline*0.5) then RotY(azimuth*0.5)
///      (split equally between neck and head bones — both share the same local
///       matrix but use their own bind-pose translation offsets).
///   6. Blend-in/out to interpolate with the existing animation pose.
use bevy::prelude::*;
use std::f32::consts::PI;

use crate::oni2_loader::animation::Oni2AnimState;
use crate::oni2_loader::components::{ActiveHeadIK, ControlHeadTask};
use crate::oni2_loader::utils::space;

// ---------------------------------------------------------------------------
// Tuning constants (degrees → radians where applicable)
// ---------------------------------------------------------------------------
const AZM_RANGE: f32 = 80.0 * PI / 180.0; // horizontal look limit
const INC_RANGE: f32 = 45.0 * PI / 180.0; // vertical look limit
const AZM_RATE: f32 = 360.0 * PI / 180.0; // rad/s convergence speed
const INC_RATE: f32 = 180.0 * PI / 180.0; // rad/s convergence speed
const BLEND_IN: f32 = 2.0; // blend weight gained per second
const BLEND_OUT: f32 = 2.0; // blend weight lost per second
const TRACK_HEAD_Y: f32 = 0.1; // y-offset when looking at a tracked actor's head
/// Max distance at which `TrackClosest` will acquire a new target.  Matches
/// the typical `animHeadIKTuning::TrackClosestDist` from the shipped tuning.
const TRACK_CLOSEST_DIST: f32 = 8.0;
/// Hysteresis band: already-tracked targets retain lock up to this many
/// meters beyond `TRACK_CLOSEST_DIST` before being dropped, preventing
/// rapid flicker between near-equidistant creatures.
const TRACK_CLOSEST_HYSTERESIS: f32 = 1.0;

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Persistent IK runtime state.  Survives mode changes so blending is smooth.
#[derive(Component, Default, Debug, Clone)]
pub struct HeadIKState {
    pub current_azimuth: f32,
    pub current_incline: f32,
    /// 0 = full animation pose, 1 = full IK pose
    pub blend: f32,
    /// Accumulated time used by the Scan mode
    pub scan_elapsed: f32,
    /// Last entity acquired by `TrackClosest`.  Persists so hysteresis can
    /// prefer the current target over switching between near-equidistant
    /// creatures every frame.  Cleared when the target leaves range or loses
    /// its neck bone.  Mirrors `animHeadIK::ClosestActor`.
    pub tracked_actor: Option<Entity>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Proportional clip: scales (x, y) toward zero preserving direction so
/// neither component exceeds its respective limit sx / sy.
fn clip(x: &mut f32, y: &mut f32, sx: f32, sy: f32) {
    let mut tx = 1.0_f32;
    let mut ty = 1.0_f32;
    if *x < -sx {
        tx = -sx / *x;
    } else if *x > sx {
        tx = sx / *x;
    }
    if *y < -sy {
        ty = -sy / *y;
    } else if *y > sy {
        ty = sy / *y;
    }
    let t = tx.min(ty);
    *x *= t;
    *y *= t;
}

/// Linear approach: move `current` toward `target` by at most `rate * dt`.
fn approach(current: &mut f32, target: f32, rate: f32, dt: f32) {
    let delta = target - *current;
    let step = rate * dt;
    if delta.abs() <= step {
        *current = target;
    } else {
        *current += delta.signum() * step;
    }
}

// ---------------------------------------------------------------------------
// Setup system — adds HeadIKState the first time an entity gets ActiveHeadIK
// ---------------------------------------------------------------------------

pub fn head_ik_setup_system(
    mut commands: Commands,
    query: Query<Entity, (With<ActiveHeadIK>, Without<HeadIKState>)>,
) {
    for entity in &query {
        commands.entity(entity).insert(HeadIKState::default());
    }
}

// ---------------------------------------------------------------------------
// Main system
// ---------------------------------------------------------------------------

pub fn head_ik_system(
    time: Res<Time>,
    mut ik_query: Query<(
        Entity,
        &ActiveHeadIK,
        &mut HeadIKState,
        &Oni2AnimState,
        &GlobalTransform,
    )>,
    mut joint_tf_query: Query<&mut Transform>,
    target_query: Query<&GlobalTransform>,
    // Candidates for TrackClosest: every entity with an animator + transform.
    // The neck-bone gate is applied inside the arm; candidates without a neck
    // are silently skipped (matches legacy animHeadIK::HasNeck filter).
    neck_candidates: Query<(Entity, &Oni2AnimState, &GlobalTransform)>,
) {
    let dt = time.delta_secs();

    for (self_entity, ik, mut state, anim_state, entity_gtf) in &mut ik_query {
        let skel = &anim_state.skeleton;

        // ---- Find bone indices -------------------------------------------
        let Some(spine_idx) = find_bone(&skel.names, &["spine_02", "spine_01", "spine"]) else {
            continue;
        };
        let Some(neck_idx) = find_bone(&skel.names, &["neck_01", "neck"]) else {
            continue;
        };
        let Some(head_idx) = find_bone(&skel.names, &["head"]) else {
            continue;
        };

        let Some(&spine_ent) = anim_state.joint_entities.get(spine_idx) else {
            continue;
        };
        let Some(&neck_ent) = anim_state.joint_entities.get(neck_idx) else {
            continue;
        };
        let Some(&head_ent) = anim_state.joint_entities.get(head_idx) else {
            continue;
        };

        // ---- Read current joint world transforms (copy before any write) --
        let Ok(spine_tf) = joint_tf_query.get(spine_ent).copied() else {
            continue;
        };
        let Ok(neck_anim) = joint_tf_query.get(neck_ent).copied() else {
            continue;
        };
        let Ok(head_anim) = joint_tf_query.get(head_ent).copied() else {
            continue;
        };

        let spine_world_rot = spine_tf.rotation;
        let spine_world_pos = spine_tf.translation;

        // ---- Determine goal and whether to blend out ----------------------
        let mut blend_out = false;
        let mut target_world_pos: Option<Vec3> = None;

        match &ik.task {
            ControlHeadTask::Disable => {
                blend_out = true;
            }

            ControlHeadTask::TrackActor(target_ent) => {
                if let Ok(tgt_gtf) = target_query.get(*target_ent) {
                    // Aim a little above the target's origin (approximate head height)
                    target_world_pos = Some(tgt_gtf.translation() + Vec3::Y * TRACK_HEAD_Y);
                } else {
                    blend_out = true;
                }
            }

            ControlHeadTask::TrackPos(oni2_pos) => {
                // Script vectors are in Oni2 left-handed space; convert to Bevy.
                target_world_pos = Some(space::to_bevy_space_pos(*oni2_pos));
            }

            ControlHeadTask::TrackClosest => {
                // Mirrors animHeadIK::TrackClosest — find the nearest other
                // creature with a neck bone within TRACK_CLOSEST_DIST, with
                // hysteresis so the lock doesn't flicker.  `self_gtf` already
                // sourced from entity_gtf.
                let self_pos = entity_gtf.translation();

                // 1. Validate the existing lock, if any.  If still in range
                //    (range + hysteresis) and still has a neck, tighten the
                //    effective max-dist so only STRICTLY closer candidates
                //    steal focus.
                let mut effective_max = TRACK_CLOSEST_DIST;
                let mut validated_current = false;
                if let Some(current) = state.tracked_actor {
                    if let Ok((_, cand_anim, cand_gtf)) = neck_candidates.get(current) {
                        let dist = (cand_gtf.translation() - self_pos).length();
                        let cap = TRACK_CLOSEST_DIST + TRACK_CLOSEST_HYSTERESIS;
                        let has_neck =
                            find_bone(&cand_anim.skeleton.names, &["neck_01", "neck"]).is_some();
                        if dist <= cap && has_neck {
                            effective_max = (dist - TRACK_CLOSEST_HYSTERESIS).max(0.0);
                            validated_current = true;
                        } else {
                            state.tracked_actor = None;
                        }
                    } else {
                        state.tracked_actor = None;
                    }
                }

                // 2. Sweep every candidate for the closest valid one.
                let mut best_ent: Option<Entity> = None;
                let mut best_dist_sq = effective_max * effective_max;
                if effective_max > 0.0 {
                    for (cand_ent, cand_anim, cand_gtf) in &neck_candidates {
                        if cand_ent == self_entity {
                            continue;
                        }
                        let diff = cand_gtf.translation() - self_pos;
                        let d2 = diff.length_squared();
                        if d2 > best_dist_sq {
                            continue;
                        }
                        if find_bone(&cand_anim.skeleton.names, &["neck_01", "neck"]).is_none() {
                            continue;
                        }
                        best_dist_sq = d2;
                        best_ent = Some(cand_ent);
                    }
                }

                // 3. Adopt the new target, or fall back to the validated prior
                //    lock if nothing closer was found.
                let chosen = best_ent.or_else(|| {
                    if validated_current {
                        state.tracked_actor
                    } else {
                        None
                    }
                });

                if let Some(target_ent) = chosen {
                    if let Ok((_, cand_anim, cand_gtf)) = neck_candidates.get(target_ent) {
                        // Aim at the head bone if we can find it — else head
                        // height offset above the origin.
                        let head_world = find_bone(&cand_anim.skeleton.names, &["head"])
                            .and_then(|idx| cand_anim.joint_entities.get(idx).copied())
                            .and_then(|head_ent| {
                                joint_tf_query.get(head_ent).ok().map(|t| t.translation)
                            });
                        target_world_pos = Some(match head_world {
                            Some(p) => p,
                            None => cand_gtf.translation() + Vec3::Y * TRACK_HEAD_Y,
                        });
                        state.tracked_actor = Some(target_ent);
                    } else {
                        state.tracked_actor = None;
                        blend_out = true;
                    }
                } else {
                    state.tracked_actor = None;
                    blend_out = true;
                }
            }

            ControlHeadTask::Set { azimuth, incline } => {
                // azimuth / incline are in degrees (matching Oni2 script convention).
                // Build a world-space target along the actor's facing direction.
                let az_rad = azimuth * PI / 180.0;
                let inc_rad = incline * PI / 180.0;
                let forward = entity_gtf.forward(); // Bevy -Z forward
                let dir = Quat::from_rotation_y(az_rad) * Quat::from_rotation_x(-inc_rad) * forward;
                target_world_pos = Some(entity_gtf.translation() + *dir * 5.0);
            }

            ControlHeadTask::Scan { range, period } => {
                state.scan_elapsed += dt;
                // range is in degrees, period in seconds (matching Oni2 script defaults)
                let az_rad =
                    (*range * PI / 180.0) * (state.scan_elapsed * 2.0 * PI / *period).sin();
                let forward = entity_gtf.forward();
                let dir = Quat::from_rotation_y(az_rad) * forward;
                target_world_pos = Some(entity_gtf.translation() + *dir * 5.0);
            }
        }

        // ---- Compute desired angles in spine-local space ------------------
        if let Some(world_target) = target_world_pos {
            // Transform goal into spine-local Bevy space
            let local_target = spine_world_rot.inverse() * (world_target - spine_world_pos);

            let dist = local_target.length();
            let mut inc = if dist > 0.001 {
                (local_target.y / dist).asin()
            } else {
                0.0
            };
            // atan2(-x, -z): forward faces -Z in local space, so negate to get world azimuth
            let mut azm = f32::atan2(-local_target.x, -local_target.z);

            clip(&mut azm, &mut inc, AZM_RANGE, INC_RANGE);
            approach(&mut state.current_azimuth, azm, AZM_RATE, dt);
            approach(&mut state.current_incline, inc, INC_RATE, dt);

            state.blend = (state.blend + dt * BLEND_IN).min(1.0);
        } else if blend_out {
            state.blend = (state.blend - dt * BLEND_OUT).max(0.0);
            if state.blend <= 0.0 {
                state.current_azimuth = 0.0;
                state.current_incline = 0.0;
            }
        }

        if state.blend <= 0.0 {
            continue;
        }

        // ---- Build IK local rotation ------------------------------------------
        // neck: RotX(incline*0.5) then RotY(azimuth*0.5), shared with head bone.
        // In Bevy quaternion order (right-to-left): Ry * Rx = apply Rx first.
        let ik_local_rot = Quat::from_rotation_y(state.current_azimuth * 0.5)
            * Quat::from_rotation_x(state.current_incline * 0.5);

        // IK world rotation = spine_world_rot * ik_local_rot
        let ik_rot = spine_world_rot * ik_local_rot;

        // IK world positions from bind-pose local offsets (Oni2 → Bevy coord flip)
        let neck_off_oni2 = Vec3::from(skel.local_offsets[neck_idx]);
        let neck_off_bevy = space::to_bevy_space_pos(neck_off_oni2);
        let ik_neck_pos = spine_world_pos + spine_world_rot * neck_off_bevy;

        let head_off_oni2 = Vec3::from(skel.local_offsets[head_idx]);
        let head_off_bevy = space::to_bevy_space_pos(head_off_oni2);
        // Head is a child of neck, so use ik_rot (not spine_world_rot) for propagation
        let ik_head_pos = ik_neck_pos + ik_rot * head_off_bevy;

        // ---- Blend animation ↔ IK and write back -------------------------
        let b = state.blend;

        if let Ok(mut neck_tf) = joint_tf_query.get_mut(neck_ent) {
            neck_tf.rotation = neck_anim.rotation.slerp(ik_rot, b);
            neck_tf.translation = neck_anim.translation.lerp(ik_neck_pos, b);
        }
        if let Ok(mut head_tf) = joint_tf_query.get_mut(head_ent) {
            head_tf.rotation = head_anim.rotation.slerp(ik_rot, b);
            head_tf.translation = head_anim.translation.lerp(ik_head_pos, b);
        }
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn find_bone(names: &[String], candidates: &[&str]) -> Option<usize> {
    for &candidate in candidates {
        if let Some(idx) = names.iter().position(|n| n.eq_ignore_ascii_case(candidate)) {
            return Some(idx);
        }
    }
    None
}
