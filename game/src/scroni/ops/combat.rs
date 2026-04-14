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
            true
        }
        Stmt::Fight => {
            info!("VM: Fight (unimplemented)");
            true
        }
        Stmt::Shoot => {
            info!("VM: Shoot (unimplemented)");
            true
        }
        Stmt::Hit { hit_type, victim, damage } => {
            let _t = ctx.eval(hit_type);
            let _v = ctx.eval(victim);
            let _d = ctx.eval(damage);
            info!("VM: Hit (unimplemented)");
            true
        }
        Stmt::Retreat(opt_expr) => {
            let ent = opt_expr.as_ref().map(|e| ctx.eval(e));
            info!("VM: Retreat from {:?} (unimplemented)", ent);
            true
        }
        _ => false,
    }
}
