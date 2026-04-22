/*
 * behavior/impls/idle.rs — IdleBehavior: stand still for a duration (or
 * forever until interrupted).
 *
 * Zero-out velocity on entry; tick down `idle_duration` from params.
 * Finishes when the timer reaches zero.  If no duration was specified,
 * Idle runs indefinitely (the FSM parks here by default until a scripted
 * `fight`/`goto`/`follow`/etc. arrives and the BEHAVIOR_SWITCHES check
 * transitions us out).
 */
use super::super::{Behavior, BehaviorRunCtx, BehaviorUpdate};
use crate::statemachine::drivers::behavior::BehaviorKind;

#[derive(Default)]
pub struct IdleBehavior {
    /// Seconds remaining before auto-finishing.  `None` = run until
    /// externally interrupted.  Captured from `params.idle_duration` at
    /// `on_enter` time.
    time_remaining: Option<f32>,
}

impl Behavior for IdleBehavior {
    fn kind(&self) -> BehaviorKind {
        BehaviorKind::Idle
    }

    fn on_enter(&mut self, ctx: &mut BehaviorRunCtx<'_>) {
        self.time_remaining = ctx.params.idle_duration;
        ctx.velocity.x = 0.0;
        ctx.velocity.z = 0.0;
    }

    fn update(&mut self, ctx: &mut BehaviorRunCtx<'_>, dt: f32) -> BehaviorUpdate {
        // Held idle — steering stays zero each tick in case something
        // else wrote to velocity between ticks.
        ctx.velocity.x = 0.0;
        ctx.velocity.z = 0.0;

        if let Some(t) = self.time_remaining.as_mut() {
            *t -= dt;
            if *t <= 0.0 {
                return BehaviorUpdate::Finished;
            }
        }
        BehaviorUpdate::Continue
    }

    fn on_exit(&mut self, _ctx: &mut BehaviorRunCtx<'_>) {
        // Nothing to tear down — next behavior sets its own steering.
    }
}
