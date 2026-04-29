/*
 * behavior/impls/goto.rs — GotoBehavior: walk a waypoint chain to a point.
 *
 * Replaces the ad-hoc `ActorPathfollower` + `path_following_system` glue
 * ScrOni used to insert directly on the actor.  Here the steering IS the
 * behavior — GotoBehavior owns the path cursor, writes velocity each
 * tick, and reports `Finished` when the final waypoint is within
 * `params.within` (defaults to 1.0m).
 *
 * Accepts either a pre-computed `params.path` (NavGraph A* output) or a
 * single `params.target_point`; the latter becomes a 1-length path.
 */
use super::super::{Behavior, BehaviorRunCtx, BehaviorUpdate};
use crate::statemachine::drivers::behavior::BehaviorKind;
use bevy::prelude::*;

const MOVE_SPEED: f32 = 4.5;
const DEFAULT_WITHIN: f32 = 1.5;
const INTER_WAYPOINT_TOLERANCE: f32 = 1.0;

#[derive(Default)]
pub struct GotoBehavior {
    path: Vec<Vec3>,
    cursor: usize,
    within: f32,
    speed: f32,
}

impl Behavior for GotoBehavior {
    fn kind(&self) -> BehaviorKind {
        BehaviorKind::Goto
    }

    fn can_enter(&self, ctx: &BehaviorRunCtx<'_>) -> bool {
        !ctx.params.path.is_empty() || ctx.params.target_point.is_some()
    }

    fn on_enter(&mut self, ctx: &mut BehaviorRunCtx<'_>) {
        self.path = if !ctx.params.path.is_empty() {
            ctx.params.path.clone()
        } else if let Some(p) = ctx.params.target_point {
            vec![p]
        } else {
            Vec::new()
        };
        self.cursor = 0;
        self.within = ctx.params.within.unwrap_or(DEFAULT_WITHIN).max(0.01);
        self.speed = MOVE_SPEED * ctx.params.speed_throttle.unwrap_or(1.0).clamp(0.0, 4.0);
    }

    fn update(&mut self, ctx: &mut BehaviorRunCtx<'_>, dt: f32) -> BehaviorUpdate {
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
            self.within
        } else {
            INTER_WAYPOINT_TOLERANCE
        };

        if dist <= tolerance {
            self.cursor += 1;
            // If that was the last one, we're done on the next tick.
            if self.cursor >= self.path.len() {
                ctx.velocity.x = 0.0;
                ctx.velocity.z = 0.0;
                return BehaviorUpdate::Finished;
            }
            // Otherwise fall through and steer toward the new current waypoint.
        }

        let dir_to_current = if dist > 0.001 {
            delta / dist
        } else {
            Vec3::ZERO
        };
        let desired = dir_to_current * self.speed;
        ctx.velocity.x = desired.x;
        ctx.velocity.z = desired.z;
        if dir_to_current.length_squared() > 0.0 {
            ctx.fighter.facing = dir_to_current;

            // Face movement direction.  Oni2 models face +Z locally; look_at
            // points -Z at target, so rotate 180° Y afterward.
            let look_target = here + dir_to_current;
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
