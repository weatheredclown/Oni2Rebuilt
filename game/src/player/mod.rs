/*
 * player/mod.rs — PlayerPlugin: input handling and physics movement.
 *
 * Update chain: keyboard_input_system → gamepad_input_system →
 * player_mouse_look_system → fsm_update_system (animation state machine).
 * FixedUpdate: moving_platform_system → player_movement_system (camera-relative
 * WASD + jump via Avian3d LinearVelocity).
 */
pub mod components;
pub mod systems;

use bevy::prelude::*;

use crate::menu::AppState;
use crate::oni2_loader;
use crate::statemachine::runtime::fsm_update_system;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // Input backends — keyboard first, gamepad merges on top
                systems::keyboard_input_system,
                systems::gamepad_input_system,
                systems::player_mouse_look_system,
                // FSM ticks after input is settled
                fsm_update_system,
            )
                .chain()
                .run_if(in_state(AppState::InGame)),
        )
        .add_systems(
            FixedUpdate,
            (
                oni2_loader::moving_platform_system,
                systems::player_movement_system,
            )
                .chain()
                .run_if(in_state(AppState::InGame)),
        );
    }
}
