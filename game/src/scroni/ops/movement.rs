/*
 * scroni/ops/movement.rs — movement script opcodes.
 *
 * Handles Face, Move, Follow, Teleport, and related actor-movement statements.
 * Issues BlockingAction for timed moves and SysRequest variants that
 * system_bindings translates into Transform / LinearVelocity mutations.
 */
use super::OpsCtx;
use crate::scroni::ast::Stmt;
use crate::scroni::vm::{BlockingAction, CranePhase, SysRequest, Value};
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
            // ScrOni `pickup <guid>` — the script's parent actor
            // is the crane (this is the only way the legacy
            // engine dispatches the pickup behavior); the
            // argument is the actor to grab.  Drain handler
            // resolves the hitch + drives the IK; we park on
            // `WaitingForCrane { Pickup }` until the IK reports
            // the chain landed (state == Attached).
            let script_actor = ctx.exec.owner;
            let raw_arg = ctx.eval(expr);
            // Log the reach regardless of arg type so it's
            // discoverable when the sequence makes it here but the
            // value isn't what the script author expected
            // (e.g. `next` returned None because the actor list
            // was already exhausted this frame).
            info!(
                "[crane] ScrOni pickup REACHED: crane={:?} arg_expr={:?} arg_value={:?}",
                script_actor, expr, raw_arg
            );
            match raw_arg {
                Value::Actor(target) => {
                    info!(
                        "[crane] ScrOni pickup issued: crane={:?} target={:?}",
                        script_actor, target
                    );
                    ctx.sys_request(SysRequest::CranePickup {
                        actor: script_actor,
                        target,
                    });
                    ctx.block(BlockingAction::WaitingForCrane {
                        actor: script_actor,
                        phase: CranePhase::Pickup,
                    });
                }
                other => {
                    warn!(
                        "[crane] ScrOni pickup arg evaluated to non-actor {:?} (no-op)",
                        other
                    );
                    ctx.yield_thread();
                }
            }
            true
        }
        Stmt::Dropoff { at } => {
            let script_actor = ctx.exec.owner;
            let Some(at_expr) = at else {
                warn!("[crane] ScrOni dropoff without `at <vec>` clause (no-op)");
                ctx.yield_thread();
                return true;
            };
            match ctx.eval(at_expr) {
                Value::Vector(world_pos) => {
                    // ScrOni vectors are in Oni left-handed
                    // space; flip to Bevy at the parse boundary
                    // (matches every other vec-consuming op).
                    let bevy_pos = crate::oni2_loader::utils::space::to_bevy_space_pos(world_pos);
                    info!(
                        "[crane] ScrOni dropoff issued: crane={:?} oni={:?} bevy={:?}",
                        script_actor, world_pos, bevy_pos
                    );
                    ctx.sys_request(SysRequest::CraneDropoff {
                        actor: script_actor,
                        world_pos: bevy_pos,
                    });
                    ctx.block(BlockingAction::WaitingForCrane {
                        actor: script_actor,
                        phase: CranePhase::Dropoff,
                    });
                }
                other => {
                    warn!(
                        "[crane] ScrOni dropoff target evaluated to non-vector {:?} (no-op)",
                        other
                    );
                    ctx.yield_thread();
                }
            }
            true
        }
        _ => false,
    }
}
