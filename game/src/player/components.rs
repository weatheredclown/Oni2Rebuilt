use bevy::prelude::*;

/// Marker component for the player entity.
#[derive(Component)]
pub struct Player;

/// Stores accumulated input state each frame for FixedUpdate systems to read.
#[derive(Component, Default)]
pub struct InputState {
    pub movement: Vec2,
    pub light_attack: bool,
    pub heavy_attack: bool,
    pub blocking: bool,
    pub grab: bool,
    pub jump: bool,
    pub yaw_delta: f32,
    pub attack_direction: f32,
}
