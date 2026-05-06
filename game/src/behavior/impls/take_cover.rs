/*
 * behavior/impls/take_cover.rs — TakeCoverBehavior: walk to a reserved
 * POINT_COVER nav node.
 *
 * Port of `bhTakeCover` + `bhActionCover` (rb/src/behavior/takecover.cpp).
 * The state-machine-on-top-of-state-machine collapses into something
 * simpler here because the heavy lifting moved to the dispatcher:
 *
 *   - Cover-point selection + reservation + path build run in
 *     `behavior_start_dispatch_system` BEFORE this behavior's on_enter,
 *     so by the time we land here `params.path` either has the route or
 *     is empty (nothing reachable / no cover spots in the level).
 *   - We walk the path identically to GotoBehavior — same waypoint
 *     stepping, same facing/velocity math.  Different behavior identity
 *     keeps the FSM's behavior-kind cursor accurate (so a fight request
 *     mid-cover is a clean TakeCover→Fight swap rather than re-entering
 *     a Goto loop).
 *   - The reservation release also lives in the dispatcher (matches on
 *     `kind == TakeCover` when the behavior ends), so this struct
 *     doesn't need access to CoverPointManager.  Ownership of the
 *     reservation is transferred via the dispatcher and the behavior
 *     just walks.
 *
 * Empty `params.path` means "selection failed" — we report `Failed` on
 * the first update so the script's `blockingcommandfailed` retry loop
 * (e.g. scavenger_cover.oni:53) sees the failure.
 */
use super::super::{Behavior, BehaviorRunCtx, BehaviorUpdate};
use crate::statemachine::drivers::behavior::BehaviorKind;
use bevy::prelude::*;

const MOVE_SPEED: f32 = 4.5;
const ARRIVAL_TOLERANCE: f32 = 0.5;
const INTER_WAYPOINT_TOLERANCE: f32 = 1.0;

#[derive(Default)]
pub struct TakeCoverBehavior {
    path: Vec<Vec3>,
    cursor: usize,
    /// Latched on entry: when true, the dispatcher couldn't find a cover
    /// point or build a path, so `update` reports Failed immediately.
    selection_failed: bool,
}

impl Behavior for TakeCoverBehavior {
    fn kind(&self) -> BehaviorKind {
        BehaviorKind::TakeCover
    }

    fn on_enter(&mut self, ctx: &mut BehaviorRunCtx<'_>) {
        self.path = ctx.params.path.clone();
        self.cursor = 0;
        self.selection_failed = self.path.is_empty();
    }

    fn update(&mut self, ctx: &mut BehaviorRunCtx<'_>, dt: f32) -> BehaviorUpdate {
        if self.selection_failed {
            ctx.velocity.x = 0.0;
            ctx.velocity.z = 0.0;
            return BehaviorUpdate::Failed;
        }

        if self.cursor >= self.path.len() {
            ctx.velocity.x = 0.0;
            ctx.velocity.z = 0.0;
            return BehaviorUpdate::Finished;
        }

        let target = self.path[self.cursor];
        let here = ctx.transform.translation;
        let mut delta = target - here;
        delta.y = 0.0;
        let dist = delta.length();

        let is_last = self.cursor + 1 == self.path.len();
        let tolerance = if is_last {
            ARRIVAL_TOLERANCE
        } else {
            INTER_WAYPOINT_TOLERANCE
        };

        if dist <= tolerance {
            self.cursor += 1;
            if self.cursor >= self.path.len() {
                ctx.velocity.x = 0.0;
                ctx.velocity.z = 0.0;
                return BehaviorUpdate::Finished;
            }
        }

        let dir = if dist > 0.001 {
            delta / dist
        } else {
            Vec3::ZERO
        };
        let desired = dir * MOVE_SPEED;
        ctx.velocity.x = desired.x;
        ctx.velocity.z = desired.z;
        if dir.length_squared() > 0.0 {
            ctx.fighter.facing = dir;
            // Same +Z-forward → -Z look_at flip GotoBehavior uses.
            let look_target = here + dir;
            let mut rot_tf = *ctx.transform;
            rot_tf.look_at(look_target, Vec3::Y);
            rot_tf.rotate_y(std::f32::consts::PI);
            ctx.transform.rotation = ctx
                .transform
                .rotation
                .slerp(rot_tf.rotation, (10.0 * dt).min(1.0));
        }

        BehaviorUpdate::Continue
    }

    fn on_exit(&mut self, ctx: &mut BehaviorRunCtx<'_>) {
        ctx.velocity.x = 0.0;
        ctx.velocity.z = 0.0;
    }
}
