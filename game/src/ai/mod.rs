pub mod components;
pub mod systems;
pub mod navigation;

use bevy::prelude::*;

use crate::menu::AppState;

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (
                systems::ai_target_system,
                systems::ai_decision_system,
                systems::ai_movement_system,
                navigation::path_following_system,
                navigation::actor_follower_system,
            )
                .chain()
                .before(crate::combat::systems::ground_detection_system)
                .run_if(in_state(AppState::InGame)),
        );
    }
}
