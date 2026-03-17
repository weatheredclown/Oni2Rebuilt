use bevy::prelude::*;
use super::curve::NurbsCurve;

/// Marker component for ONI2-loaded entities.
#[derive(Component, Debug, Clone)]
pub struct Oni2Entity {
    pub name: String,
}

/// Component for entities that follow a NURBS curve path.
#[derive(Component)]
pub struct CurveFollower {
    pub curve: NurbsCurve,
    pub phase: f32,           // current t ∈ [0, 1]
    pub speed: f32,           // knots/sec (parametric speed)
    pub target_phase: f32,    // target t value
    pub wrap_around: bool,    // loop when reaching end
    pub ping_pong: bool,      // reverse direction at ends
    pub look_along_xz: bool,  // constrain orientation to XZ plane
    pub reached_target: bool,
}

/// Component indicating this entity should emit a specific Particle/FX system (from layout XML).
#[derive(Component)]
pub struct ActorFxType {
    pub fx_name: String,
}

