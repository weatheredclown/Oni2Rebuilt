use bevy::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiState {
    Idle,
    Pursuing,
    Circling,
    Attacking,
    Blocking,
    Recovering,
}

#[derive(Component)]
pub struct AiFighter {
    pub state: AiState,
    pub target: Option<Entity>,
    pub decision_timer: f32,
    pub block_probability: f32,
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
            decision_timer: 0.5,
            block_probability: 0.6,
            aggression: 0.5,
            preferred_range: 3.0,
            circle_direction: 1.0,
            circle_switch_timer: 2.0,
        }
    }
}
