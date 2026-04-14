/*
 * scroni/ops/combat.rs — combat script opcodes.
 *
 * Handles Stmt::Attack (script-triggered attack on a target entity).
 * Issues SysRequest::TriggerAttack resolved by system_bindings into an
 * AttackMessage on the relevant fighter entity.
 */
use crate::scroni::ast::Stmt;
use bevy::prelude::*;
use crate::scroni::vm::{SysRequest, Value};
use super::OpsCtx;

pub fn exec(ctx: &mut OpsCtx, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Attack(expr) => {
            let ent = ctx.eval(expr);
            let target_ent = match ent {
                Value::Actor(act) => act,
                Value::Int(_) | Value::String(_) => {
                    let targets = ctx.ctx.resolve_targets(&ent);
                    if !targets.is_empty() {
                        targets[0]
                    } else {
                        Entity::PLACEHOLDER
                    }
                }
                _ => Entity::PLACEHOLDER,
            };
            ctx.sys_request(SysRequest::SetAiTarget {
                actor: ctx.exec.owner,
                target: target_ent,
            });
            ctx.thread_mut().state = crate::scroni::vm::ExecState::Yielded;
            true
        }
        Stmt::Fight(opt_expr) => {
            let target_ent = opt_expr.as_ref().map(|expr| {
                match ctx.eval(expr) {
                    Value::Actor(act) => act,
                    Value::Int(_) | Value::String(_) => {
                        let evaluated_val = ctx.eval(expr);
                        let targets = ctx.ctx.resolve_targets(&evaluated_val);
                        if !targets.is_empty() {
                            targets[0]
                        } else {
                            Entity::PLACEHOLDER
                        }
                    }
                    _ => Entity::PLACEHOLDER,
                }
            });
            ctx.sys_request(SysRequest::TriggerFight { 
                actor: ctx.exec.owner, 
                target: target_ent 
            });
            ctx.thread_mut().state = crate::scroni::vm::ExecState::Yielded;
            true
        }
        Stmt::Shoot => {
            info!("VM: Shoot (unimplemented)");
            ctx.thread_mut().state = crate::scroni::vm::ExecState::Yielded;
            true
        }
        Stmt::Hit { hit_type, victim, damage } => {
            let eval_hit_type = ctx.eval_string(hit_type);
            let eval_victim = ctx.eval(victim);
            let eval_damage = ctx.eval_float(damage);

            let targets = ctx.ctx.resolve_targets(&eval_victim);
            for target in targets {
                ctx.sys_request(SysRequest::Hit {
                    target,
                    hit_type: eval_hit_type.clone(),
                    damage: eval_damage,
                });
            }

            ctx.thread_mut().state = crate::scroni::vm::ExecState::Yielded;
            true
        }
        Stmt::Retreat(opt_expr) => {
            let target_ent = opt_expr.as_ref().map(|expr| {
                match ctx.eval(expr) {
                    Value::Actor(act) => act,
                    Value::Int(_) | Value::String(_) => {
                        let evaluated_val = ctx.eval(expr);
                        let targets = ctx.ctx.resolve_targets(&evaluated_val);
                        if !targets.is_empty() {
                            targets[0]
                        } else {
                            Entity::PLACEHOLDER
                        }
                    }
                    _ => Entity::PLACEHOLDER,
                }
            });
            ctx.sys_request(SysRequest::Retreat {
                actor: ctx.exec.owner,
                target: target_ent,
            });
            ctx.thread_mut().state = crate::scroni::vm::ExecState::Yielded;
            true
        }
        _ => false,
    }
}
