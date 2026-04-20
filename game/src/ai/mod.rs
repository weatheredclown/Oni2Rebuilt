/*
 * ai/mod.rs — AiPlugin: enemy AI decision loop.
 *
 * Runs in FixedUpdate before combat: target selection, behavioural state machine
 * (Idle / Pursuing / Circling / Attacking / Recovering), movement steering, and
 * A* path following.  Scheduled before ground_detection_system so physics sees
 * intent-derived velocity each tick.
 */
pub mod components;
pub mod navigation;
pub mod systems;

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
                navigation::retreat_steering_system,
            )
                .chain()
                .before(crate::combat::systems::ground_detection_system)
                .run_if(in_state(AppState::InGame)),
        );
    }
}
