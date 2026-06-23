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

/// Parsed `<Eye>` component — perception cone for any actor that
/// can `look`.  Mirrors the legacy `aiEye` component the original
/// game attached to cranes, creatures, and triggers.
///
/// `range` is in world units; `field_of_view_deg` is the *total*
/// cone angle in degrees (as authored in the XML).  ScrOni's `look`
/// op compares an actor-relative direction against `fov_half_rad()`
/// — half the cone, in radians — because the angle it computes
/// from `angle_between` is symmetric around forward.
#[derive(Component, Debug, Clone, Copy)]
pub struct Eye {
    pub range: f32,
    pub field_of_view_deg: f32,
}

impl Eye {
    /// Half the cone width, in radians.  This is what
    /// `look_op`'s `angle > looker_fov` check compares against
    /// (the `angle` it computes is always 0..π).  A FOV of 360°
    /// produces π rad here, so every direction passes — matching
    /// the legacy "see all around" intent of EricArm-style cranes.
    pub fn fov_half_rad(&self) -> f32 {
        (self.field_of_view_deg * 0.5).to_radians()
    }
}

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

/// Stores the bound type string from the .bnd file for runtime classification.
#[derive(Component, Debug, Clone)]
pub struct BoundType(pub String);

/// Stores the physical material property set assigned to the bound.
#[derive(Component, Debug, Clone)]
pub struct MaterialType(pub String);

/// Marker component for legacy "octree" volumes which use strict 1-way collision portal culling.
#[derive(Component, Debug, Clone)]
pub struct OneWayOctreeBound;

/// Per-octree-collider entity-local point that lies inside the playable cell
/// (the sub-bound centroid from the .bnd file, in the same local frame as the
/// collider vertices). Used by `octree_one_way_contact_system` to determine
/// which side of a wall faces the level interior, independent of contact
/// normal orientation.
#[derive(Component, Debug, Clone, Copy)]
pub struct OctreeInteriorRef(pub Vec3);

/// CameraTrigger component — switches the ActiveCameraPackage to the specified package name when the player enters the radius.
#[derive(Component, Debug, Clone)]
pub struct CameraTrigger {
    pub radius: f32,
    pub camera_package: String,
}

/// ForceVectorTrigger component — applies a force/acceleration to physical entities within the radius.
#[derive(Component, Debug, Clone)]
pub struct ForceVectorTrigger {
    pub radius: f32,
    pub force_vector: Vec3, // In Bevy space
}

/// SectionTrigger component — tracks checkpoint conditions and triggers actor spawning/destruction in target sections.
#[derive(Component, Debug, Clone)]
pub struct SectionTrigger {
    pub radius: f32,
    pub sections_to_spawn: String,
    pub sections_to_destroy: String,
    pub trigger_only_once: bool,
    pub min_checkpoint_index: i32,
    pub max_checkpoint_index: i32,
    pub has_fired: bool,         // Runtime state
    pub player_was_inside: bool, // Track transition for non-once triggers
}

/// The conveyor velocity (Bevy world units/sec) added to this mover's
/// `LinearVelocity` on the most recent physics tick.  Recorded by
/// `apply_conveyor_system` and subtracted by `creature_movement_anim_system`
/// so the locomotion gait is chosen from the character's motion *relative to
/// the belt* — running upstream still plays the run anim, and standing still
/// while carried plays the idle anim.  Zeroed each tick; absent = not on a
/// conveyor.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ConveyorPush(pub Vec3);

/// Conveyor surface — an entity whose physics bound carries a `Conveyor: 1`
/// material (e.g. `Entity/TestConveyor`).  A mover standing on this entity's
/// collider is pushed along the entity's forward axis (`oni_forward`) at
/// `speed` units/sec.  Mirrors `rbPhysMaterial::GetConveyor()` consumed by
/// `crmover::Bound`'s `SetSlide(true, forward * ConveyorSpeed)`.
#[derive(Component, Debug, Clone, Copy)]
pub struct Conveyor {
    /// `ConveyorSpeed` from the physics material (units/sec).
    pub speed: f32,
}
