/*
 * camera/follow.rs — navigation / follow camera mode.
 *
 * follow_camera_system runs when mode is GameNavigation or GameTargeting.
 * Computes desired_azimuth by zone-based lag tracking of the player's facing
 * direction, with a spin threshold to decide how aggressively to chase turns.
 */
use super::channel::CameraChannel;
use super::components::{ActiveCameraMode, CameraController};
use bevy::prelude::*;

/// Calculates optimal desired azimuth and incline for the standard navigation state.
pub fn follow_camera_system(
    time: Res<Time>,
    // The controller dictates if we run, the channel stores the temporal data
    mut camera_query: Query<(&CameraController, &mut CameraChannel)>,
    // Only required to know zone thresholds natively if pulling from specific parameter sets...
    // For now we'll hardcode generic constants like the previous implementation
    keyboard: Res<ButtonInput<KeyCode>>,
    velocity_query: Query<&avian3d::prelude::LinearVelocity>,
) {
    let dt = time.delta_secs();

    for (controller, mut channel) in &mut camera_query {
        if controller.active_mode != ActiveCameraMode::GameNavigation
            && controller.active_mode != ActiveCameraMode::GameTargeting
        {
            continue;
        }

        // --- Manual arrow key overrides ---
        let mut manual_yaw = 0.0;
        let manual_turn_speed = 3.0;

        if keyboard.pressed(KeyCode::ArrowLeft) {
            manual_yaw += manual_turn_speed * dt;
        }
        if keyboard.pressed(KeyCode::ArrowRight) {
            manual_yaw -= manual_turn_speed * dt;
        }

        let manual_pitch_speed = 5.0;
        if keyboard.pressed(KeyCode::ArrowUp) {
            channel.desired_incline += manual_pitch_speed * dt;
        }
        if keyboard.pressed(KeyCode::ArrowDown) {
            channel.desired_incline -= manual_pitch_speed * dt;
        }
        channel.desired_incline = channel.desired_incline.clamp(-0.5, 1.5); // allow some upward angle

        // Auto-recover towards package defaults over time unless manually overridden
        let is_pitch_overridden =
            keyboard.pressed(KeyCode::ArrowUp) || keyboard.pressed(KeyCode::ArrowDown);
        if !is_pitch_overridden {
            let mut interp = 0.0;
            if let Ok(vel) = velocity_query.get(channel.focus_actor) {
                let speed = Vec2::new(vel.x, vel.z).length();
                // 1.0 m/s standing to 4.0 m/s full run
                interp = ((speed - 1.0) / 3.0).clamp(0.0, 1.0);
            }

            let package_incline = channel.package_incline_offset * (1.0 - interp)
                + channel.package_incline_offset_running * interp;

            let recovery_speed = 1.5 * dt;
            channel.desired_incline += (package_incline - channel.desired_incline) * recovery_speed;
        }

        // Distance recovery
        let scroll_active = false; // We can't query scroll cleanly here, but smooth lerp will just battle it softly
        if !scroll_active {
            let recovery_speed = 2.0 * dt;
            channel.desired_distance +=
                (channel.package_distance - channel.desired_distance) * recovery_speed;
        }

        if manual_yaw != 0.0 {
            channel.desired_azimuth += manual_yaw;
            channel.current_azimuth += manual_yaw; // Instantly apply manual override linearly
        } else if channel.is_moving {
            // Zone-based auto-follow camera logic

            // We evaluate focusing differences
            let mut heading_diff = channel.current_focus_azimuth - channel.current_azimuth;
            while heading_diff > std::f32::consts::PI {
                heading_diff -= 2.0 * std::f32::consts::PI;
            }
            while heading_diff < -std::f32::consts::PI {
                heading_diff += 2.0 * std::f32::consts::PI;
            }

            let abs_diff = heading_diff.abs();

            let thresholds = [
                20.0_f32.to_radians(),
                90.0_f32.to_radians(),
                120.0_f32.to_radians(),
            ];

            // Assign Zone Enum
            channel.focus_heading_zone = if abs_diff < thresholds[0] {
                super::channel::FocusHeadingZone::Zone1
            } else if abs_diff < thresholds[1] {
                super::channel::FocusHeadingZone::Zone2
            } else if abs_diff < thresholds[2] {
                super::channel::FocusHeadingZone::Zone3
            } else {
                super::channel::FocusHeadingZone::Zone4
            };

            // Dead zone: compute horizontal distance
            // If we are too close, we don't automatically orient.
            // If distance is within the dead zone, we don't automatically orient.
            let cam_xz = Vec3::new(
                -channel.current_azimuth.sin() * channel.current_distance,
                0.0,
                -channel.current_azimuth.cos() * channel.current_distance,
            );

            // Assume the target is at origin relatively
            let focus_dist = cam_xz.length();

            let inner_dz = channel.package_inner_radius;
            let spin_thresh = channel.package_spin_threshold;

            if focus_dist > inner_dz {
                channel.desired_azimuth = channel.current_focus_azimuth;

                channel.spin_threshold_exceeded = abs_diff > spin_thresh;
            } else {
                channel.spin_threshold_exceeded = false;
            }
        }
    }
}
