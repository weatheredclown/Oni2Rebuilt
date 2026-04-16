/*
 * oni2_loader/components.rs — ONI2 entity component types.
 *
 * Oni2Entity (name marker), CurveFollower (NURBS path position),
 * Oni2AnimState (current anim + frame + speed), MovingPlatform, Checkpoint,
 * ActorFxType, CurrentCheckpointIndex, and other per-entity runtime state
 * shared between animation, scripting, and layout systems.
 */
use super::curve::NurbsCurve;
use bevy::prelude::*;

/// Marker component for ONI2-loaded entities.
#[derive(Component, Debug, Clone)]
pub struct Oni2Entity {
    pub name: String,
}

/// Component for entities that follow a NURBS curve path.
#[derive(Component)]
pub struct CurveFollower {
    pub curve: NurbsCurve,
    pub phase: f32,              // current t ∈ [0, 1]
    pub speed: f32,              // parametric speed or physical speed based on 'speed_is_physical'
    pub speed_is_physical: bool, // true = meters/sec, false = phase/sec
    pub target_phase: f32,       // target t value
    pub wrap_around: bool,       // loop when reaching end
    pub ping_pong: bool,         // reverse direction at ends
    pub look_along_xz: bool,     // constrain orientation to XZ plane
    pub fixed_orientation: bool, // disable rotation completely
    pub reached_target: bool,
}

/// Component indicating this entity should emit a specific Particle/FX system (from layout XML).
#[derive(Component, Debug, Clone)]
pub struct ActorFxType {
    pub fx_name: Option<String>,
    pub start_active: bool,
    pub ptx_name: Option<String>,
    pub ptx_birth_rate: f32,
    pub ptx_num_particles: i32,
    pub ptx_offset: Vec3,
}

/// Marker component for actors that are currently asleep (dormant).
#[derive(Component, Debug, Clone)]
pub struct ActorAsleep;

/// Component indicating this entity operates as a game progression checkpoint.
#[derive(Component, Debug, Clone)]
pub struct CheckpointTrigger {
    pub index: i32,
    pub radius: f32,
}

/// Global resource tracking the player's current checkpoint progress.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct CurrentCheckpointIndex(pub i32);

#[derive(Debug, Clone, PartialEq)]
pub enum ControlHeadTask {
    Disable,
    TrackClosest,
    TrackActor(Entity),
    TrackPos(Vec3),
    Set { azimuth: f32, incline: f32 },
    Scan { range: f32, period: f32 },
}

#[derive(Component, Debug, Clone)]
pub struct ActiveHeadIK {
        pub task: ControlHeadTask,
}

/// Component mapping an unattached entity to a dynamically spawned parent actor.
#[derive(Component, Debug, Clone)]
pub struct PendingParent {
    pub parent_name: String,
    pub bone_name: Option<String>,
}

/// Marker component for legacy "octree" volumes which use strict 1-way collision portal culling.
#[derive(Component, Debug, Clone)]
pub struct OneWayOctreeBound;
