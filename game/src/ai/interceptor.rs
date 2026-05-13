use bevy::prelude::*;

use crate::ai::components::{AiFighter, AiInterceptor};
use crate::player::components::Player;

/// High-fidelity port from legacy C++.
/// This system monitors proximity of the player. If the player enters
/// the AI's perception radius, the interceptor triggers, alerting the AI
/// and establishing the player as a target if one isn't already assigned.
pub fn ai_interceptor_system(
    mut query: Query<(Entity, &mut AiInterceptor, &mut AiFighter, &GlobalTransform)>,
    player_query: Query<(Entity, &GlobalTransform), With<Player>>,
) {
    let Some((player_ent, player_tf)) = player_query.iter().next() else {
        return;
    };

    let player_pos = player_tf.translation();

    for (_entity, mut interceptor, mut fighter, self_tf) in &mut query {
        let self_pos = self_tf.translation();
        let dist_sq = self_pos.distance_squared(player_pos);

        let radius = fighter.perception_radius();

        if dist_sq < radius * radius {
            if !interceptor.active {
                interceptor.active = true;
                // Once active, acquire player as target if we don't have one
                if !fighter.manual_target && fighter.target.is_none() {
                    fighter.target = Some(player_ent);
                }
            }
            interceptor.intercept_point = Some(player_pos);
        } else {
            interceptor.active = false;
        }
    }
}
