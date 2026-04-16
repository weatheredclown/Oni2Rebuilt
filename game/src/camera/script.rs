/*
 * camera/script.rs — scripted camera path playback.
 *
 * script_camera_system runs when mode is set to a scripted sequence by the
 * ScrOni VM.  Evaluates a Catmull-Rom spline over ScriptCameraSequence control
 * points, interpolating position and focus toward ScriptFocusTarget over time.
 */
use bevy::prelude::*;
use super::channel::CameraChannel;
use crate::camera::components::{ActiveCameraMode, CameraController, ScriptCameraSequence, ScriptFocusTarget};

fn evaluate_catmull_rom(pts: &[Vec3], t: f32) -> Vec3 {
    if pts.is_empty() { return Vec3::ZERO; }
    if pts.len() == 1 { return pts[0]; }

    let segments = (pts.len() - 1) as f32;
    let scaled_t = t * segments;
    if scaled_t >= segments {
        return *pts.last().unwrap();
    }

    let index = scaled_t.floor() as usize;
    let local_t = scaled_t - index as f32;

    // Get 4 control points P0, P1, P2, P3
    let p0 = if index == 0 { pts[0] } else { pts[index - 1] };
    let p1 = pts[index];
    let p2 = pts[index + 1];
    let p3 = if index + 2 < pts.len() { pts[index + 2] } else { pts[index + 1] };

    let t2 = local_t * local_t;
    let t3 = t2 * local_t;

    0.5 * ((2.0 * p1) +
           (-p0 + p2) * local_t +
           (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 +
           (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

/// Calculates optimal desired azimuth, incline, and distance for the scripted camera state.
pub fn script_camera_system(
    time: Res<Time>,
    mut camera_query: Query<(
        &CameraController,
        &mut CameraChannel,
        Option<&mut ScriptCameraSequence>,
        &Transform
    )>,
    transform_query: Query<&GlobalTransform>,
    layout_paths: Option<Res<crate::oni2_loader::environment::LayoutPaths>>,
    // Local state for change-detection logging (not saved across runs, just this session)
    mut prev_mode: Local<Option<ActiveCameraMode>>,
    mut prev_override_pos: Local<Option<Vec3>>,
) {
    let dt = time.delta_secs();

    for (controller, mut channel, seq_opt, cam_tf) in &mut camera_query {
        // Log mode transitions (once per change, not every frame)
        if Some(controller.active_mode) != *prev_mode {
            info!(
                "[CAM-SCRIPT] Mode: {:?} → {:?} | cam_pos={}",
                prev_mode.map(|m| format!("{:?}", m)).unwrap_or_else(|| "none".into()),
                format!("{:?}", controller.active_mode),
                cam_tf.translation
            );
            *prev_mode = Some(controller.active_mode);
        }

        if controller.active_mode != ActiveCameraMode::Script {
            if channel.script_override_transform.is_some() {
                info!(
                    "[CAM-SCRIPT] Clearing override (mode={:?}) | final cam_pos={}",
                    controller.active_mode, cam_tf.translation
                );
                channel.script_override_transform = None;
                *prev_override_pos = None;
            }
            continue;
        }

        let Some(mut seq) = seq_opt else {
            warn!("[CAM-SCRIPT] Mode=Script but camera has no ScriptCameraSequence component");
            continue;
        };

        // 1. Evaluate explicit fov changes over time
        if let (Some(start), Some(target)) = (seq.fov_start, seq.fov_target) {
            seq.fov_time_elapsed += dt;
            let t = if seq.fov_duration > 0.0 {
                (seq.fov_time_elapsed / seq.fov_duration).min(1.0)
            } else {
                1.0
            };
            let fov = start + (target - start) * t;
            channel.script_fov_override = Some(fov);
            if t >= 1.0 {
                // Animation complete — lock at target and stop interpolating
                seq.fov_start = None;
                seq.fov_target = None;
            }
        } else {
            // No active FOV script; let polar use the package value
            channel.script_fov_override = None;
        }

        // Ensure tracked_target resolves dynamically if it's an actor
        let mut look_at_pos = channel.current_focus_pos;
        if let Some(track) = &seq.tracked_target {
            match track {
                ScriptFocusTarget::Actor(e) => {
                    if let Ok(tf) = transform_query.get(*e) {
                        look_at_pos = tf.translation() + Vec3::Y * 1.5;
                    }
                }
                ScriptFocusTarget::Point(pt) => look_at_pos = *pt,
            }
        }

        // Ensure active_rail resolves dynamically from layout.paths
        if let Some(name) = &seq.active_rail_name {
            if seq.active_rail.is_none() {
                if let Some(lp) = &layout_paths {
                    if let Some(curve) = lp.curves.iter().find(|(n, _)| n == name) {
                        info!(
                            "[CAM-SCRIPT] Rail '{}' resolved: {} control points | first={:?} last={:?}",
                            name,
                            curve.1.len(),
                            curve.1.first(),
                            curve.1.last()
                        );
                        seq.active_rail = Some(curve.1.clone());
                    } else {
                        let available: Vec<&str> = lp.curves.iter().map(|(n, _)| n.as_str()).collect();
                        warn!(
                            "[CAM-SCRIPT] Rail '{}' NOT found. Available curves: {:?}",
                            name, available
                        );
                        seq.active_rail_name = None; // stop trying
                    }
                } else {
                    warn!("[CAM-SCRIPT] Rail '{}' requested but LayoutPaths resource missing", name);
                }
            }
        }

        // Lazily capture move_start from camera's current position on first script tick
        if seq.move_target.is_some() && seq.move_start.is_none() {
            seq.move_start = Some(cam_tf.translation);
            info!(
                "[CAM-SCRIPT] move_start captured: {} (current cam_pos at command dispatch)",
                cam_tf.translation
            );
        }

        // 2. Evaluate explicit structural movement (Rails > MoveToPoint > Track)
        let new_override: Option<Transform> = if seq.active_rail.is_some() {
            seq.rail_time_elapsed += dt;
            let t = (seq.rail_time_elapsed / seq.rail_duration.max(f32::EPSILON)).min(1.0);

            let pts = seq.active_rail.as_ref().unwrap();
            let pos = evaluate_catmull_rom(pts, t);
            let mut tf = Transform::from_translation(pos);
            let forward = look_at_pos - pos;
            if forward.length_squared() > 0.001 && forward.normalize().dot(Vec3::Y).abs() < 0.999 {
                tf.look_at(look_at_pos, Vec3::Y);
            }
            Some(tf)

        } else if seq.move_target.is_some() {
            seq.move_time_elapsed += dt;

            let mut target_pos = Vec3::ZERO;
            let move_target_clone = seq.move_target.clone().unwrap();
            match move_target_clone {
                ScriptFocusTarget::Actor(e) => {
                    if let Ok(tf) = transform_query.get(e) {
                        target_pos = tf.translation() + Vec3::Y * 1.5;
                    }
                }
                ScriptFocusTarget::Point(pt) => target_pos = pt,
            }

            let t = (seq.move_time_elapsed / seq.move_duration.max(f32::EPSILON)).min(1.0);

            seq.move_start.map(|start| {
                let pos = start.lerp(target_pos, t);
                let mut tf = Transform::from_translation(pos);
                let forward = look_at_pos - pos;
                if forward.length_squared() > 0.001 && forward.normalize().dot(Vec3::Y).abs() < 0.999 {
                    tf.look_at(look_at_pos, Vec3::Y);
                }
                tf
            })

        } else if seq.tracked_target.is_some() {
            // Track purely by looking (freeze in place)
            let mut tf = *cam_tf;
            let forward = look_at_pos - tf.translation;
            if forward.length_squared() > 0.001 && forward.normalize().dot(Vec3::Y).abs() < 0.999 {
                tf.look_at(look_at_pos, Vec3::Y);
            }
            Some(tf)
        } else {
            None
        };

        // Log when the override position moves more than 0.1 units (suppresses per-frame spam)
        let new_pos = new_override.map(|tf| tf.translation);
        let changed = match (new_pos, *prev_override_pos) {
            (Some(np), Some(pp)) => np.distance(pp) > 0.1,
            (Some(_), None) | (None, Some(_)) => true,
            (None, None) => false,
        };
        if changed {
            let kind = if seq.active_rail.is_some() {
                "rail"
            } else if seq.move_target.is_some() {
                "move"
            } else {
                "track"
            };
            info!(
                "[CAM-SCRIPT] [{kind}] pos={:?} | look_at={}",
                new_pos, look_at_pos
            );
            *prev_override_pos = new_pos;
        }

        channel.script_override_transform = new_override;
    }
}
