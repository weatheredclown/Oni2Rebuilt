/*
 * ai/components.rs — AI component types.
 *
 * AiState enum (Idle, Pursuing, Circling, Attacking, Recovering).
 * AiFighter: per-entity AI config (attack range, patience, preferred distance)
 * and runtime state (current AiState, target entity, timers).
 */
use bevy::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiState {
    Idle,
    Pursuing,
    Circling,
    Attacking,
    Recovering,
}

#[derive(Component)]
pub struct ActorFollower {
    pub target: Entity,
    pub within: f32,
}

#[derive(Component)]
pub struct AiFighter {
    pub state: AiState,
    pub target: Option<Entity>,
    pub manual_target: bool,
    pub decision_timer: f32,
    pub aggression: f32,
    pub preferred_range: f32,
    pub circle_direction: f32,
    pub circle_switch_timer: f32,
}

impl Default for AiFighter {
    fn default() -> Self {
        Self {
            state: AiState::Idle,
            target: None,
            manual_target: false,
            decision_timer: 0.5,
            aggression: 0.5,
            preferred_range: 3.0,
            circle_direction: 1.0,
            circle_switch_timer: 2.0,
        }
    }
}
