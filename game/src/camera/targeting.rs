use bevy::prelude::*;
use super::channel::CameraChannel;
use super::components::{ActiveCameraMode, CameraController};

/// Calculates optimal desired azimuth, incline, and distance for the Targeting state (camnewTargeting).
/// This applies an over-the-shoulder, zoomed-in view, matching sniper ADS behavior natively.
pub fn targeting_camera_system(
    time: Res<Time>,
    mut camera_query: Query<(&CameraController, &mut CameraChannel)>,
) {
    let _dt = time.delta_secs();

    for (controller, mut channel) in &mut camera_query {
        if controller.active_mode != ActiveCameraMode::GameTargeting {
            continue;
        }

        // --- Targeting Mode Logic (camnewTargeting Parity) ---
        // Lock desired heading directly onto the reticle's X/Y aim.
        // It ignores zone tracking natively, matching the C++ behavior 1:1.
        channel.desired_azimuth = channel.reticle_x;
        channel.desired_incline = channel.reticle_y;

        // Force camera tightly behind character over the right shoulder.
        channel.desired_distance = 1.8;
        
        // ADS zoom (FOV mapped strictly smaller natively)
        // We override the package default FOV actively
        channel.desired_fov = 25.0;

        // Custom focus spatial tracking
        channel.current_focus_offset = Vec3::new(0.5, 1.3, 0.0); // Right and Up offset over shoulder
    }
}
