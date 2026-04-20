/*
 * camera/mod.rs — CameraPlugin: multi-mode camera rig.
 *
 * Orchestrates the camera pipeline each frame:
 *   update_camera_channel → mode evaluators (follow / fight / script / targeting)
 *   → polar_interpolation_system → apply_camera_transform.
 * PrototypeVisible resource and prototype_toggle_system (F2) also live here.
 * freecam_system handles the F5 debug free-fly camera independently.
 */
pub mod channel;
pub mod components;
pub mod fight;
pub mod follow;
pub mod freecam;
pub mod polar;
pub mod script;
pub mod systems;
pub mod targeting;

use bevy::prelude::*;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<components::PrototypeVisible>()
            .add_systems(
                Update,
                (
                    systems::camera_mode_toggle_system,
                    systems::prototype_toggle_system,
                    systems::debug_render_camera_targets,
                    systems::update_camera_channel,
                    // Mode-specific evaluators (all read from & write to channel)
                    follow::follow_camera_system.after(systems::update_camera_channel),
                    fight::fight_camera_system.after(systems::update_camera_channel),
                    script::script_camera_system.after(systems::update_camera_channel),
                    targeting::targeting_camera_system.after(systems::update_camera_channel),
                    // Final interpolators
                    polar::polar_interpolation_system
                        .after(follow::follow_camera_system)
                        .after(fight::fight_camera_system)
                        .after(script::script_camera_system)
                        .after(targeting::targeting_camera_system),
                    // Transform modifiers
                    polar::apply_camera_transform.after(polar::polar_interpolation_system),
                    freecam::freecam_system.after(systems::update_camera_channel), // Bypasses channel completely
                ),
            );
    }
}
