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
            .add_message::<events::AboutToBeHitMessage>()
            .add_message::<events::HitReactionMessage>()
            .add_systems(
                FixedUpdate,
                (
                    systems::ground_detection_system,
                    systems::attack_sync_system,
                    systems::hit_detection_system,
                    systems::about_to_be_hit_system,
                    systems::hit_reaction_system,
                    systems::combo_tracking_system,
                    systems::death_system,
                    systems::telemetry_combat_system,
                    systems::death_cleanup_system,
                    systems::death_timer_system,
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
