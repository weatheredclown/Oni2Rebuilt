/*
 * camera/polar.rs — polar coordinate interpolation and transform application.
 *
 * polar_interpolation_system: lerps current_azimuth / current_incline /
 * current_distance / current_fov toward their desired_ counterparts each frame.
 * apply_camera_transform: converts the current polar values into a world-space
 * Transform (position = focus + spherical offset, looking_at focus point).
 */
use bevy::prelude::*;
use super::channel::CameraChannel;
use super::components::CameraController;

/// Iterates `current_` polar values towards `desired_` polar values using mode-dependent lerp rates (if any).
pub fn polar_interpolation_system(
    time: Res<Time>,
    // In a full implementation, we might get mode-specific lerp rates from the controller or channel.
    // Right now we just do a generic smooth approach until all modes output their specific rates cleanly.
    mut query: Query<(&CameraController, &mut CameraChannel)>,
) {
    let dt = time.delta_secs();

    for (_controller, mut channel) in &mut query {
        
        // Script FOV override takes precedence over the package FOV
        channel.desired_fov = channel.script_fov_override.unwrap_or(channel.package_fov);
        
        // Mode-specific lerp rates
        let fov_speed = 5.0;
        let dist_speed = 3.0;
        let incline_speed = 4.0;
        
        let mut azimuth_speed = match channel.focus_heading_zone {
            super::channel::FocusHeadingZone::Zone1 => channel.package_zone_lerp_rates[0],
            super::channel::FocusHeadingZone::Zone2 => channel.package_zone_lerp_rates[1],
            super::channel::FocusHeadingZone::Zone3 => channel.package_zone_lerp_rates[2],
            super::channel::FocusHeadingZone::Zone4 => channel.package_zone_lerp_rates[3],
        };
        
        // If combat, we don't care about zone fallback usually, but this is a rough approximation 
        // fallback in case package zeros out azimuth_speed unintentionally.
        if azimuth_speed <= 0.01 {
            azimuth_speed = 3.0;
        }

        // Smooth FOV
        channel.current_fov = channel.current_fov + (channel.desired_fov - channel.current_fov) * (fov_speed * dt).clamp(0.0, 1.0);
        
        // Smooth Distance
        channel.current_distance = channel.current_distance + (channel.desired_distance - channel.current_distance) * (dist_speed * dt).clamp(0.0, 1.0);
        
        // Smooth Incline
        channel.current_incline = channel.current_incline + (channel.desired_incline - channel.current_incline) * (incline_speed * dt).clamp(0.0, 1.0);
        
        // Target azimuth normalization
        // Spherical paths cross 180/-180 constantly, we must map them locally.
        let mut heading_diff = channel.desired_azimuth - channel.current_azimuth;
        while heading_diff > std::f32::consts::PI {
            heading_diff -= 2.0 * std::f32::consts::PI;
        }
        while heading_diff < -std::f32::consts::PI {
            heading_diff += 2.0 * std::f32::consts::PI;
        }

        channel.current_azimuth += heading_diff * (azimuth_speed * dt).clamp(0.0, 1.0);

        // Optional sharp turn spin threshold (from channel)
        if channel.spin_threshold_exceeded {
             channel.current_azimuth += heading_diff * (azimuth_speed * 1.5 * dt).clamp(0.0, 1.0);
        }
        
        // Decay bump
        channel.bump_magnitude *= (1.0 - 4.0 * dt).max(0.0);
    }
}

/// Computes the final Transform orientation and position from the interpolated spherical coordinates.
pub fn apply_camera_transform(
    time: Res<Time>,
    mut camera_query: Query<(&CameraController, &mut CameraChannel, &mut Transform)>,
    spatial_query: avian3d::prelude::SpatialQuery,
) {
    let dt = time.delta_secs();
    
    for (controller, mut channel, mut cam_tf) in &mut camera_query {
        
        if let Some(script_tf) = channel.script_override_transform {
            cam_tf.translation = script_tf.translation;
            cam_tf.rotation = script_tf.rotation;
            apply_camera_shake(&mut cam_tf, &mut channel, dt);
            continue;
        }
        
        let total_azimuth = channel.current_azimuth + channel.bump_magnitude;
        let total_incline = channel.current_incline;
        
        let dist = channel.current_distance;
        let pitch_cos = total_incline.cos().max(0.01); 
        let pitch_sin = total_incline.sin();
        let yaw_sin = total_azimuth.sin();
        let yaw_cos = total_azimuth.cos();
        
        // Add custom tracking focus offset dynamically updated by follow modes
        let target_base = channel.current_focus_pos + channel.current_focus_offset;
        
        let offset = Vec3::new(
            pitch_cos * yaw_sin * dist,
            pitch_sin * dist,
            pitch_cos * yaw_cos * dist, 
        );
        
        let mut desired_pos = target_base + offset;
        
        // Environmental collision spherecast 
        let dir = desired_pos - target_base;
        let cast_dist = dir.length();
        let cast_dir = Dir3::new(dir).unwrap_or(Dir3::Z);

        if cast_dist > 0.01 {
            // Exclude the player/focus entity so camera does not hit them
            let filter = avian3d::prelude::SpatialQueryFilter::from_excluded_entities([channel.focus_actor]);
            
            // Ray cast to keep camera comfortably out of walls
            if let Some(hit) = spatial_query.cast_ray(
                target_base,
                cast_dir,
                cast_dist,
                true,
                &filter
            ) {
                // If hit, push camera closer across the vector 
                desired_pos = target_base + cast_dir * (hit.distance * 0.85); // 0.85 to add buffer
                channel.is_colliding = true;
            } else {
                channel.is_colliding = false;
            }
        }
        
        let t = (6.0 * dt).clamp(0.0, 1.0);
        cam_tf.translation = cam_tf.translation.lerp(desired_pos, t);

        // Lock looking at the target center
        cam_tf.look_at(target_base + Vec3::Y * 0.5, Vec3::Y);

        apply_camera_shake(&mut cam_tf, &mut channel, dt);
    }
}

/// Applies and decays any active camera shake as a random rotation perturbation.
/// Applies a random Euler-angle perturbation, damped as the shake timer approaches expiry.
fn apply_camera_shake(cam_tf: &mut Transform, channel: &mut CameraChannel, dt: f32) {
    if channel.shake_time_remaining <= 0.0 {
        return;
    }

    let scale = if channel.shake_time_remaining < channel.shake_time_to_damp {
        channel.shake_time_remaining / channel.shake_time_to_damp
    } else {
        1.0
    };

    // Pseudo-random per-frame perturbation using the remaining time as a seed.
    // Different frequencies per axis to avoid correlated motion.
    let t = channel.shake_time_remaining;
    let rx = (t * 97.3).sin() * channel.shake_amplitude.x * scale;
    let ry = (t * 53.7).sin() * channel.shake_amplitude.y * scale;
    let rz = (t * 31.1).sin() * channel.shake_amplitude.z * scale;

    cam_tf.rotation *= Quat::from_euler(EulerRot::XYZ, rx, ry, rz);
    channel.shake_time_remaining -= dt;
    if channel.shake_time_remaining < 0.0 {
        channel.shake_time_remaining = 0.0;
    }
}
