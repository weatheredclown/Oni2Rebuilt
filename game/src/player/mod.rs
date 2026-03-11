pub mod components;
pub mod systems;

use bevy::prelude::*;

use crate::menu::AppState;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                systems::player_input_system,
                systems::player_mouse_look_system,
            )
                .run_if(in_state(AppState::InGame)),
        )
        .add_systems(
            FixedUpdate,
            systems::player_movement_system.run_if(in_state(AppState::InGame)),
        );
    }
}
