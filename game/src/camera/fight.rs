/*
 * camera/fight.rs — combat camera mode.
 *
 * fight_camera_system runs when mode is GameFighting.  Applies Z-lock distance
 * constraints around the target and handles evasive bump inputs (CameraBumpDirection)
 * to temporarily offset the camera for visual clarity during combat.
 */
use super::channel::CameraChannel;
use super::components::{ActiveCameraMode, CameraController};
use bevy::prelude::*;

/// Calculates optimal desired azimuth and distance for the combat state.
pub fn fight_camera_system(
    time: Res<Time>,
    mut camera_query: Query<(&CameraController, &mut CameraChannel)>,
) {
    let dt = time.delta_secs();

    for (controller, mut channel) in &mut camera_query {
        if controller.active_mode != ActiveCameraMode::GameFighting {
            continue;
        }

        // --- Z-Lock Parsing ---
        // If we are fighting, distance constraints lock on.
        // Let's abstract the bump camera moving logic natively
        if channel.bump_direction != super::channel::CameraBumpDirection::None {
            match channel.bump_direction {
                super::channel::CameraBumpDirection::PositiveX => {
                    channel.bump_magnitude += 0.2 * dt
                }
                super::channel::CameraBumpDirection::NegativeX => {
                    channel.bump_magnitude -= 0.2 * dt
                }
                _ => {}
            }
        }

        let inner_dz = channel.package_inner_radius;

        let focus_dist = channel.current_distance;

        if focus_dist > inner_dz {
            // Spin if outer radius reached natively.
            channel.desired_azimuth = channel.current_focus_azimuth;
        }

        // Adjust desired distance natively based on running
        if channel.is_moving {
            channel.desired_distance += 0.5 * dt;
        } else {
            channel.desired_distance -= 0.5 * dt;
        }

        // Z-Lock
        channel.desired_distance = channel.desired_distance.clamp(
            (channel.package_distance - 2.0).max(2.0),
            channel.package_distance + 2.0,
        );
    }
}
