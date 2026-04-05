pub mod channel;
pub mod components;
pub mod systems;
pub mod polar;
pub mod follow;
pub mod fight;
pub mod freecam;
pub mod script;
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
