pub mod systems;

use bevy::prelude::*;

use crate::menu::AppState;
use crate::oni2_loader::TestAnimMode;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(AppState::InGame),
            systems::setup_hud.run_if(not(resource_exists::<TestAnimMode>)),
        )
        .add_systems(
            Update,
            (
                systems::update_health_bars,
                systems::update_super_meter_bar,
                systems::update_combo_display,
                systems::update_status_display,
                systems::update_combat_display,
            )
                .run_if(in_state(AppState::InGame))
                .run_if(not(resource_exists::<TestAnimMode>)),
        );
    }
}
