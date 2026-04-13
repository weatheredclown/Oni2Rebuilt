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

/// Calculates optimal desired azimuth, incline, and distance for the Script state (camnewScript).
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
) {
    let dt = time.delta_secs();

    for (controller, mut channel, seq_opt, cam_tf) in &mut camera_query {
        if controller.active_mode != ActiveCameraMode::Script {
            // Un-override when script is done
            channel.script_override_transform = None;
            continue;
        }

        let Some(mut seq) = seq_opt else { continue };

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
        let mut look_at_pos = channel.current_focus_pos; // Default to wherever the logic was
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
                        seq.active_rail = Some(curve.1.clone());
                    } else {
                        warn!("LayoutPath rail not found: {}", name);
                        seq.active_rail_name = None; // stop trying
                    }
                }
            }
        }

        // Initialize move_start if missing
        if seq.move_target.is_some() && seq.move_start.is_none() {
            seq.move_start = Some(cam_tf.translation);
        }

        // 2. Evaluate explicit structural movement (Rails > MoveToPoint)
        if seq.active_rail.is_some() {
            seq.rail_time_elapsed += dt;
            let mut t = if seq.rail_duration > 0.0 { seq.rail_time_elapsed / seq.rail_duration } else { 1.0 };
            if t > 1.0 { t = 1.0; }
            
            let pts = seq.active_rail.as_ref().unwrap();
            let pos = evaluate_catmull_rom(pts, t);
            let mut tf = Transform::from_translation(pos);
            tf.look_at(look_at_pos, Vec3::Y);
            channel.script_override_transform = Some(tf);
            
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
            
            let mut t = if seq.move_duration > 0.0 { seq.move_time_elapsed / seq.move_duration } else { 1.0 };
            if t > 1.0 { t = 1.0; }
            
            if let Some(start) = seq.move_start {
                let pos = start.lerp(target_pos, t);
                let mut tf = Transform::from_translation(pos);
                tf.look_at(look_at_pos, Vec3::Y);
                channel.script_override_transform = Some(tf);
            }
        } else if seq.tracked_target.is_some() {
            // Track purely by looking (freeze in place)
            let mut tf = *cam_tf;
            tf.look_at(look_at_pos, Vec3::Y);
            channel.script_override_transform = Some(tf);
        } else {
            channel.script_override_transform = None;
        }
    }
}
