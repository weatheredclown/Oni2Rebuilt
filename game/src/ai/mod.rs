/*
 * ai/mod.rs — AiPlugin: FSM-driven enemy AI.
 *
 * Per-entity FSM attach + tick (via `AiFsmRuntime`, `AiPadCommands`) for the
 * input-driven side, plus A* path following and ScrOni-driven actor following /
 * retreat steering.  The cooperative fight coordinator (`FightRuntime` against
 * fight.fsm) runs from the `fightai` module.  Scheduled before
 * `ground_detection_system` so physics sees intent-derived velocity each tick.
 */
pub mod components;
pub mod cover;
pub mod fsm;
pub mod navigation;

use bevy::prelude::*;

use crate::menu::AppState;

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (
                fsm::ai_attach_fsm_system,
                fsm::ai_fsm_update_system,
                navigation::path_following_system,
                navigation::actor_follower_system,
                navigation::retreat_steering_system,
            )
                .chain()
                .before(crate::combat::systems::ground_detection_system)
                .run_if(in_state(AppState::InGame)),
        );
        app.add_systems(
            FixedPostUpdate,
            navigation::ai_velocity_residual_clamp_system
                .after(avian3d::prelude::PhysicsSet::StepSimulation)
                .run_if(in_state(AppState::InGame)),
        );
    }
}
