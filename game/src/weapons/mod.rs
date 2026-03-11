pub mod components;
pub mod systems;

use bevy::prelude::*;

use crate::menu::AppState;

pub struct WeaponsPlugin;

impl Plugin for WeaponsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (
                systems::weapon_pickup_system,
                systems::ranged_attack_system,
                systems::projectile_system,
            )
                .chain()
                .after(crate::combat::systems::attack_input_system)
                .before(crate::combat::systems::attack_advance_system)
                .run_if(in_state(AppState::InGame))
                .run_if(resource_exists::<components::WeaponMaterials>),
        )
        .add_systems(
            Update,
            (
                systems::weapon_visual_system,
                systems::weapon_pickup_bob_system,
            )
                .run_if(in_state(AppState::InGame))
                .run_if(resource_exists::<components::WeaponMaterials>),
        );
    }
}
