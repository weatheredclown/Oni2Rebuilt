/*
 * behavior/impls/mod.rs — concrete Behavior implementations.
 *
 * One submodule per behavior kind.  Each `impl Behavior` owns its own
 * per-tick state (timers, cached geometry) and is constructed afresh by
 * the dispatcher on every transition — identity lives in the FSM cursor
 * + the current `ActiveBehavior` box, not in any global table.
 */

pub mod fight;
pub mod goto;
pub mod idle;

use super::{Behavior, BehaviorRegistry};
use crate::statemachine::drivers::behavior::BehaviorKind;

/// Populate the registry with every Behavior we've ported.  Called from
/// `BehaviorPlugin::build`.
pub fn register_default_behaviors(registry: &mut BehaviorRegistry) {
    registry.register(BehaviorKind::Idle, || {
        Box::new(idle::IdleBehavior::default()) as Box<dyn Behavior>
    });
    registry.register(BehaviorKind::Goto, || {
        Box::new(goto::GotoBehavior::default()) as Box<dyn Behavior>
    });
    registry.register(BehaviorKind::Fight, || {
        Box::new(fight::FightBehavior::default()) as Box<dyn Behavior>
    });
    // Follow / Patrol / Retreat / Pad — ported in follow-ups.
}
