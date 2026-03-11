pub mod components;
pub mod events;
pub mod hitbox;
pub mod systems;

use bevy::prelude::*;

use crate::menu::AppState;

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<events::AttackMessage>()
            .add_message::<events::DamageMessage>()
            .add_message::<events::DeathMessage>()
            .add_message::<events::GrabMessage>()
            .add_message::<events::AboutToBeHitMessage>()
            .add_message::<events::HitReactionMessage>()
            .add_message::<events::BlockSuccessMessage>()
            .add_systems(
                FixedUpdate,
                (
                    systems::ground_detection_system,
                    systems::attack_input_system,
                    systems::grab_input_system,
                    systems::attack_advance_system,
                    systems::hit_detection_system,
                    systems::about_to_be_hit_system,
                    systems::grab_system,
                    systems::hit_reaction_system,
                    systems::combo_tracking_system,
                    systems::super_meter_system,
                    systems::death_system,
                    systems::telemetry_combat_system,
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                (
                    systems::fist_visual_system,
                    systems::shield_visual_system,
                )
                    .run_if(in_state(AppState::InGame))
                    .run_if(resource_exists::<components::CombatMaterials>),
            );
    }
}
