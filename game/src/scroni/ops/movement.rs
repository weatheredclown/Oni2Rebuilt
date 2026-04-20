/*
 * scroni/ops/movement.rs — movement script opcodes.
 *
 * Handles Face, Move, Follow, Teleport, and related actor-movement statements.
 * Issues BlockingAction for timed moves and SysRequest variants that
 * system_bindings translates into Transform / LinearVelocity mutations.
 */
use super::OpsCtx;
use crate::scroni::ast::Stmt;
use crate::scroni::vm::{BlockingAction, ExecState, SysRequest, Value};
use bevy::prelude::*;

pub fn exec(ctx: &mut OpsCtx, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Face { target, seconds } => {
            let t = ctx.eval(target);
            let s = seconds.as_ref().map(|e| ctx.eval_float(e));
            ctx.block(BlockingAction::Face {
                target: t,
                seconds: s,
            });
            true
        }
        Stmt::GotoPoint {
            target,
            within,
            speed,
            duration,
        } => {
            let t = ctx.eval(target);
            let w = within.as_ref().map(|e| ctx.eval_float(e));
            let s = speed.as_ref().map(|e| ctx.eval_float(e));
            let d = duration.as_ref().map(|e| ctx.eval_float(e));
            ctx.block(BlockingAction::GotoPoint {
                target: t,
                within: w,
                speed: s,
                duration: d,
            });
            true
        }
        Stmt::Patrol(expr) => {
            let path_val = ctx.eval(expr);
            ctx.block(BlockingAction::Patrol(path_val));
            true
        }
        Stmt::Follow(expr) => {
            let target = ctx.eval(expr);
            let targets = ctx.ctx.resolve_targets(&target);
            if !targets.is_empty() {
                ctx.sys_request(SysRequest::FollowActor {
                    actor: ctx.exec.owner,
                    target: targets[0],
                });
            }
            ctx.yield_thread();
            true
        }
        Stmt::Teleport { target, to, face } => {
            if let Value::Actor(ent) = ctx.eval(target) {
                let to_vec = to.as_ref().map(|e| match ctx.eval(e) {
                    Value::Vector(v) => v,
                    _ => Vec3::ZERO,
                });
                let face_float = face.as_ref().map(|e| ctx.eval_float(e));

                ctx.sys_request(SysRequest::Teleport {
                    target: ent,
                    to: to_vec,
                    face: face_float,
                });
            }
            true
        }
        Stmt::Pickup(expr) => {
            let ent = ctx.eval(expr);
            info!("VM: Pickup {:?} (unimplemented)", ent);
            ctx.yield_thread();
            true
        }
        Stmt::Dropoff { at } => {
            let at_val = at.as_ref().map(|e| ctx.eval(e));
            info!("VM: Dropoff at {:?} (unimplemented)", at_val);
            ctx.yield_thread();
            true
        }
        _ => false,
    }
}
