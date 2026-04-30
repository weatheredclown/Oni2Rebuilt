use bevy::prelude::Component;

use crate::statemachine::core::SmRuntime;
use crate::statemachine::drivers::attack::{AttackCtx, AttackDriver};
use crate::statemachine::drivers::fight::{FightCtx, FightDriver};

/// The runtime state for an actor's Fight configuration (`fight.fsm`).
/// It evaluates high-level AI directives like formation positioning,
/// combat grouping, and deciding when to swing.
#[derive(Component)]
pub struct FightRuntime {
    pub fsm: SmRuntime<FightDriver>,
    pub ctx: FightCtx,
    pub last_log: String,
}

/// The runtime state for an actor's individual attacks (`*.atk`).
/// It manages the sequence, conditions, and sub-actions that constitute
/// a single attack (from swinging an arm to leaping and recovering).
#[derive(Component)]
pub struct AttackRuntime {
    pub fsm: SmRuntime<AttackDriver>,
    pub ctx: AttackCtx,
    pub last_log: String,
    /// Per-entity tick counter — bumped each time `attack_runtime_update_system`
    /// runs on this entity.  Used to diagnose whether the system is reaching
    /// the entity at all (i.e. whether the strict component requirements on
    /// the query are satisfied).
    pub tick_count: u64,
}
