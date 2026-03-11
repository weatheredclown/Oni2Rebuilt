pub mod components;
pub mod systems;

use bevy::prelude::*;

use crate::menu::AppState;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(components::PrototypeVisible(true))
            .add_systems(
                Update,
                (
                    systems::camera_mode_toggle_system,
                    systems::camera_follow_system,
                    systems::prototype_toggle_system,
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
