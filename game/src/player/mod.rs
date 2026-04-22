/*
 * player/mod.rs — PlayerPlugin: input handling and physics movement.
 *
 * Update chain: keyboard_input_system → gamepad_input_system →
 * player_mouse_look_system → fsm_update_system (animation state machine).
 * FixedUpdate: moving_platform_system → player_movement_system (camera-relative
 * WASD + jump via Avian3d LinearVelocity).
 */
pub mod bundles;
pub mod components;
pub mod systems;

pub use bundles::PlayerIdentityBundle;

use bevy::prelude::*;

use crate::menu::AppState;
use crate::oni2_loader;
use crate::statemachine::runtime::{animator_update_system, fsm_update_system};

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<components::MenuTransitionInputCushion>()
            .add_systems(
                Update,
                (
                    // 1. Raw hardware → InputState (movement physics path)
                    systems::keyboard_input_system,
                    systems::gamepad_input_system,
                    // 2. Raw hardware → RawInputFrame → PadMapper (FSM / attack path)
                    systems::pad_mapper_update_system,
                    // 3. Mouse look uses InputState
                    systems::player_mouse_look_system,
                    // 4. Animator FSM reads PadMapper
                    animator_update_system,
                    // 5. FSM reads PadMapper values
                    fsm_update_system,
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnEnter(AppState::InGame), systems::clear_inputs_on_enter)
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
