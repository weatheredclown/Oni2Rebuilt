use bevy::prelude::{Component, Entity};

use crate::statemachine::core::SmRuntime;
use crate::statemachine::drivers::attack::{AttackCtx, AttackDriver};
use crate::statemachine::drivers::fight::{FightCtx, FightDriver};
use crate::statemachine::drivers::squad::{SquadCtx, SquadDriver, SquadOrder};

/// The runtime state for an actor's Fight configuration (`fight.fsm`).
/// It evaluates high-level AI directives like formation positioning,
/// combat grouping, and deciding when to swing.
#[derive(Component)]
pub struct FightRuntime {
    pub fsm: SmRuntime<FightDriver>,
    pub ctx: FightCtx,
    pub last_state_idx: usize,
    pub last_mode: String,
}

/// The runtime state for an actor's individual attacks (`*.atk`).
/// It manages the sequence, conditions, and sub-actions that constitute
/// a single attack (from swinging an arm to leaping and recovering).
#[derive(Component)]
pub struct AttackRuntime {
    pub fsm: SmRuntime<AttackDriver>,
    pub ctx: AttackCtx,
    pub last_state_idx: usize,
    pub last_cookie: bool,
    /// Per-entity tick counter — bumped each time `attack_runtime_update_system`
    /// runs on this entity.  Used to diagnose whether the system is reaching
    /// the entity at all (i.e. whether the strict component requirements on
    /// the query are satisfied).
    pub tick_count: u64,
}

#[derive(Component)]
pub struct Squad {
    pub members: Vec<Entity>,
    pub leader: Option<Entity>,
    pub order: SquadOrder,
    pub target: Option<Entity>,
    pub circle_center: bevy::prelude::Vec3,
    pub distance_between_fighters: f32,
    pub circle_threshold: f32,
    pub max_num_inner: i32,
    pub max_in_formation: i32,

    // Flags
    pub members_changed: bool,
    pub regroup_when_possible: bool,
    pub fighter_lost_position: bool,
    pub fighter_reacting: bool,

    // Runtime state machine
    pub fsm: SmRuntime<SquadDriver>,
    pub ctx: SquadCtx,
    pub last_state_idx: usize,
}

#[derive(Component)]
pub struct SquadMember {
    pub squad: Entity,
    pub leader: Option<Entity>,
}

#[derive(Component)]
pub struct Leader {
    pub squad: Option<Entity>,
    pub distribution_command: i32,
    pub last_distribution_command: i32,
    pub last_formation_pos: bevy::prelude::Vec3,
    pub current_destination: bevy::prelude::Vec3,
    pub has_destination: bool,
    pub move_side: bool,
}

#[derive(Component, Default)]
pub struct FighterOrder {
    pub order: SquadOrder,
    pub object: Option<Entity>,
}

/// The desired fight group for positioning (G_ANY, G_ALWAYS_INNER, G_ALWAYS_OUTER).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DesiredFightGroup {
    #[default]
    Any = 0,
    AlwaysInner = 1,
    AlwaysOuter = 2,
}

#[derive(Component, Default)]
pub struct FighterFightGroup {
    pub desired: DesiredFightGroup,
}
