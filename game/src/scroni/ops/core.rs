/*
 * scroni/ops/core.rs — core control-flow and variable opcodes.
 *
 * Handles: if/elif/else, while/break/continue, fork/kill thread, message
 * send/receive, sleep, print, var assignment, function call, return, and
 * the ScrOni-specific loop/once/whenever constructs.
 */
use super::OpsCtx;
use crate::oni2_loader::utils::space;
use crate::scroni::ast::{Stmt, VarType};
use crate::scroni::vm::{
    BlockingAction, CallFrame, ExecState, LoopState, ScrOniThread, ScriptMessage, SysRequest,
    Value, hash_name, init_variables,
};
use bevy::prelude::*;

pub fn exec(ctx: &mut OpsCtx, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Set { var, value } => {
            let val = ctx.eval(value);
            ctx.set_var(var.clone(), val);
            true
        }
        Stmt::AddToList { expr, list } => {
            let val = ctx.eval(expr);
            let mut current_list = ctx.get_var(list);

            if matches!(current_list, Value::None | Value::Int(0)) {
                current_list = Value::ActorList(Vec::new(), 0);
            }

            if let Value::ActorList(mut vec, idx) = current_list {
                vec.extend(ctx.ctx.resolve_targets(&val));
                ctx.set_var(list.clone(), Value::ActorList(vec, idx));
            }
            true
        }
        Stmt::RemoveFromList { expr, list } => {
            let val = ctx.eval(expr);
            let targets = match val {
                Value::Actor(act) => vec![act],
                Value::ActorList(acts, _) => acts,
                Value::Int(_) => ctx.ctx.resolve_targets(&val),
                _ => Vec::new(),
            };

            if !targets.is_empty() {
                let current_list = ctx.get_var(list);
                if let Value::ActorList(mut vec, mut idx) = current_list {
                    let original_len = vec.len();
                    vec.retain(|e| !targets.contains(e));
                    let removed_count = original_len - vec.len();
                    if removed_count > 0 && idx > 0 {
                        idx = idx.saturating_sub(removed_count);
                    }
                    ctx.set_var(list.clone(), Value::ActorList(vec, idx));
                }
            }
            true
        }
        Stmt::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond = ctx.eval(condition);
            if cond.as_bool() {
                ctx.exec.exec_stmt(ctx.tid, then_branch, ctx.now, ctx.ctx);
            } else if let Some(else_b) = else_branch {
                ctx.exec.exec_stmt(ctx.tid, else_b, ctx.now, ctx.ctx);
            }
            true
        }
        Stmt::Block(stmts) => {
            if ctx.thread().in_whenever {
                for s in stmts {
                    ctx.exec.exec_stmt(ctx.tid, s, ctx.now, ctx.ctx);
                }
            } else {
                ctx.thread_mut().loop_stack.push(LoopState::Block {
                    stmts: stmts.clone(),
                    pc: 0,
                });
                ctx.thread_mut().state = ExecState::PushLoop;
            }
            true
        }
        Stmt::DoForever(body) => {
            if ctx.thread().in_whenever {
                let mut count = 0;
                loop {
                    ctx.exec.exec_stmt(ctx.tid, body, ctx.now, ctx.ctx);
                    count += 1;
                    if count > 1000 {
                        warn!("Whenever block DoForever loop exceeded 1000 iterations!");
                        break;
                    }
                }
            } else {
                let stmts = ctx.exec.flatten_to_block(body);
                ctx.thread_mut()
                    .loop_stack
                    .push(LoopState::Forever { body: stmts, pc: 0 });
                ctx.thread_mut().state = ExecState::PushLoop;
            }
            true
        }
        Stmt::DoWhile { condition, body } => {
            if ctx.thread().in_whenever {
                let mut count = 0;
                while ctx.eval_bool(condition) {
                    ctx.exec.exec_stmt(ctx.tid, body, ctx.now, ctx.ctx);
                    count += 1;
                    if count > 1000 {
                        warn!("Whenever block DoWhile loop exceeded 1000 iterations!");
                        break;
                    }
                }
            } else {
                let stmts = ctx.exec.flatten_to_block(body);
                ctx.thread_mut().loop_stack.push(LoopState::While {
                    condition: condition.clone(),
                    body: stmts,
                    pc: 0,
                });
                ctx.thread_mut().state = ExecState::PushLoop;
            }
            true
        }
        Stmt::DoNTimes { count, body } => {
            let n = ctx.eval_int(count);
            if ctx.thread().in_whenever {
                let mut loop_count = 0;
                for _ in 0..n {
                    ctx.exec.exec_stmt(ctx.tid, body, ctx.now, ctx.ctx);
                    loop_count += 1;
                    if loop_count > 1000 {
                        warn!("Whenever block DoNTimes loop exceeded 1000 iterations!");
                        break;
                    }
                }
            } else {
                let stmts = ctx.exec.flatten_to_block(body);
                ctx.thread_mut().loop_stack.push(LoopState::NTimes {
                    remaining: n,
                    body: stmts,
                    pc: 0,
                });
                ctx.thread_mut().state = ExecState::PushLoop;
            }
            true
        }
        Stmt::DoForSeconds { seconds, body } => {
            let secs = ctx.eval_float(seconds);
            if ctx.thread().in_whenever {
                let key = format!("{:?}", body);
                let mut is_active = false;
                let now = ctx.now;
                {
                    let timers = &mut ctx.thread_mut().whenever_timers;
                    if !timers.contains_key(&key) {
                        timers.insert(key.clone(), now + secs as f64);
                    }
                    let end_time = timers.get(&key).unwrap();
                    if now < *end_time {
                        is_active = true;
                    }
                }

                if is_active {
                    let stmts = ctx.exec.flatten_to_block(body);
                    for s in stmts {
                        ctx.exec.exec_stmt(ctx.tid, &s, ctx.now, ctx.ctx);
                        if ctx.thread().state == ExecState::Yielded {
                            break;
                        }
                    }
                }
            } else {
                let stmts = ctx.exec.flatten_to_block(body);
                let now = ctx.now;
                ctx.thread_mut().loop_stack.push(LoopState::ForSeconds {
                    end_time: now + secs as f64,
                    body: stmts,
                    pc: 0,
                });
                ctx.thread_mut().state = ExecState::PushLoop;
            }
            true
        }
        Stmt::Exit => {
            ctx.thread_mut().state = ExecState::Yielded;
            true
        }
        Stmt::SetFaction(expr) => {
            let faction_val = ctx.eval_string(expr);
            ctx.sys_request(SysRequest::SetFaction {
                actor: ctx.exec.owner,
                faction: faction_val,
            });
            true
        }
        Stmt::Destroy(target) => {
            let val = ctx.eval(target);
            match val {
                Value::Actor(ent) => {
                    ctx.sys_request(SysRequest::Destroy(ent));
                }
                Value::Int(guid) => {
                    for (e, _, name_opt) in ctx.ctx.all_entities.iter() {
                        if let Some(n) = name_opt
                            && hash_name(n.as_str()) == guid
                        {
                            ctx.sys_request(SysRequest::Destroy(e));
                        }
                    }
                }
                _ => {}
            }
            true
        }
        Stmt::Done => {
            let frame = ctx.thread_mut().call_stack.pop();
            if let Some(frame) = frame {
                let t = ctx.thread_mut();
                t.script = frame.script;
                t.variables = frame.variables;
                t.seq_pc = frame.seq_pc;
                t.loop_stack = frame.loop_stack;
                t.state = ExecState::AbortSequence;
            } else {
                ctx.thread_mut().state = ExecState::Done;
            }
            true
        }
        Stmt::Home => {
            let t = ctx.thread_mut();
            t.seq_pc = 0;
            t.loop_stack.clear();
            t.state = ExecState::AbortSequence;
            true
        }
        Stmt::Log(exprs) => {
            let parts: Vec<String> = exprs
                .iter()
                .map(|e| {
                    let v = ctx.eval(e);
                    v.as_string()
                })
                .collect();
            info!("[ScrOni][{}] {}", ctx.thread().script.name, parts.join(" "));
            true
        }
        Stmt::Idle(expr) => {
            let secs = ctx.eval_float(expr);
            ctx.thread_mut().blocking = Some(BlockingAction::Idle {
                end_time: ctx.now + secs as f64,
            });
            ctx.thread_mut().state = ExecState::Blocked;
            true
        }
        Stmt::InlineVarDecl(decl) => {
            let val = if let Some(init) = &decl.initializer {
                ctx.eval(init)
            } else {
                match decl.var_type {
                    VarType::Integer => Value::Int(0),
                    VarType::Float => Value::Float(0.0),
                    VarType::Vector => Value::Vector(Vec3::ZERO),
                    VarType::String => Value::String(String::new()),
                    _ => Value::None,
                }
            };
            ctx.set_var(decl.name.clone(), val);
            true
        }
        Stmt::Find {
            list_var,
            conditions,
            range,
        } => {
            let eval_conds: Vec<(String, Value)> = conditions
                .iter()
                .map(|(k, e)| (k.clone(), ctx.eval(e)))
                .collect();
            let eval_range = range.as_ref().map(|e| ctx.eval_float(e));

            let mut found = Vec::new();
            let mut my_pos = match ctx.ctx.all_entities.get(ctx.exec.owner) {
                Ok((_, tf, _)) => tf.translation(),
                _ => bevy::math::Vec3::ZERO,
            };
            let max_dist = eval_range.unwrap_or(9999.0);
            let max_dist_sq = max_dist * max_dist;

            for (k, v) in &eval_conds {
                if k.to_lowercase() == "at"
                    && let Value::Vector(vec) = v
                {
                    my_pos = space::to_bevy_space_pos(vec);
                }
            }

            for (other_ent, other_tf, name_opt) in ctx.ctx.all_entities {
                if ctx.exec.owner == other_ent {
                    continue;
                }

                let dist_sq = my_pos.distance_squared(other_tf.translation());
                if dist_sq <= max_dist_sq {
                    let mut matches_all = true;
                    for (k, v) in &eval_conds {
                        let k_lower = k.to_lowercase();
                        if k_lower == "name" || k_lower == "group" {
                            let expected_name = v.as_string();
                            let actual_name = name_opt.map(|n| n.as_str()).unwrap_or("");
                            if actual_name != expected_name {
                                matches_all = false;
                                break;
                            }
                        } else if k_lower == "status" {
                            let expected_status = v.as_string().to_lowercase();
                            let actual_status = ctx
                                .ctx
                                .actor_statuses
                                .get(&other_ent)
                                .map(|s| s.as_str())
                                .unwrap_or("dead");
                            if expected_status == "enemy" {
                                if actual_status == "dead" {
                                    matches_all = false;
                                    break;
                                }
                                // Very rudimentry check for now: Is it the player?
                                if Some(other_ent) != ctx.ctx.player {
                                    matches_all = false;
                                    break;
                                }
                            } else if actual_status != expected_status {
                                matches_all = false;
                                break;
                            }
                        }
                    }
                    if matches_all {
                        found.push(other_ent);
                    }
                }
            }

            ctx.set_var(list_var.clone(), Value::ActorList(found, 0));
            true
        }
        Stmt::SendMessage { msg, to, with } => {
            let msg_str = ctx.eval_string(msg);
            let target = ctx.eval(to);

            let mut args = Vec::new();
            for a in with {
                args.push(ctx.eval(a));
            }

            let targets = ctx.ctx.resolve_targets(&target);
            if targets.is_empty() {
                warn!(
                    "[ScrOni][{}] VM: SendMessage '{}' failed, target {:?} unresolved",
                    ctx.thread().script.name,
                    msg_str,
                    target
                );
            }

            for entity in targets {
                let mut target_name = format!("{:?}", entity);
                if let Ok((_, _, Some(n))) = ctx.ctx.all_entities.get(entity) {
                    target_name = format!("{} ({:?})", n.as_str(), entity);
                }

                let mut sender_name = format!("{:?}", ctx.exec.owner);
                let mut sender_pos = bevy::math::Vec3::ZERO;
                if let Ok((_, tf, name_opt)) = ctx.ctx.all_entities.get(ctx.exec.owner) {
                    sender_pos = tf.translation();
                    if let Some(n) = name_opt {
                        sender_name = format!("{} ({:?})", n.as_str(), ctx.exec.owner);
                    }
                }

                info!(
                    "[ScrOni][{}] VM: SendMessage '{}' from {} (pos={:.2?}) to {} [seq_pc={}]",
                    ctx.thread().script.name,
                    msg_str,
                    sender_name,
                    sender_pos,
                    target_name,
                    ctx.thread().seq_pc
                );
                ctx.exec.outgoing_messages.push(ScriptMessage {
                    msg: msg_str.clone(),
                    from: ctx.exec.owner,
                    to: entity,
                    args: args.clone(),
                    is_action: false,
                });
            }
            true
        }
        Stmt::SendAction {
            action,
            target,
            component,
        } => {
            use crate::scroni::ast::Expr;
            let act_str = match action {
                Expr::Var(n) => n.clone(),
                Expr::StringLit(s) => s.clone(),
                _ => ctx.eval_string(action),
            };

            let targets = if let Some(target_expr) = target {
                let tgt = ctx.eval(target_expr);
                let res = ctx.ctx.resolve_targets(&tgt);
                if res.is_empty() {
                    warn!(
                        "VM: SendAction '{}' failed, target {:?} unresolved",
                        act_str, tgt
                    );
                }
                res
            } else {
                vec![ctx.exec.owner]
            };

            for entity in targets {
                if let Some(comp_expr) = component {
                    let comp_str = match comp_expr {
                        Expr::Var(n) => n.clone(),
                        Expr::StringLit(s) => s.clone(),
                        _ => ctx.eval_string(comp_expr),
                    };
                    ctx.sys_request(SysRequest::SendAction {
                        action: act_str.clone(),
                        target: entity,
                        component: comp_str,
                    });
                } else {
                    ctx.exec.outgoing_messages.push(ScriptMessage {
                        msg: act_str.clone(),
                        from: ctx.exec.owner,
                        to: entity,
                        args: Vec::new(),
                        is_action: true,
                    });
                }
            }
            true
        }
        Stmt::Spawn {
            script,
            assign_to,
            at,
            name,
        } => {
            let script_str = ctx.eval_string(script);
            let assign = assign_to.clone();
            let at_pos = at.as_ref().map(|e| match ctx.eval(e) {
                Value::Vector(v) => v,
                _ => Vec3::ZERO,
            });
            let target_name = name.as_ref().map(|e| ctx.eval_string(e));

            ctx.sys_request(SysRequest::Spawn {
                script: script_str,
                assign_to: assign,
                at: at_pos,
                name: target_name,
            });
            true
        }
        Stmt::MakeFx { name, at } => {
            let fx_name = ctx.eval_string(name);
            let fx_pos = at.as_ref().map(|e| match ctx.eval(e) {
                Value::Vector(v) => v,
                Value::Actor(ent) => {
                    if let Ok((_, tf, _)) = ctx.ctx.all_entities.get(ent) {
                        tf.translation()
                    } else {
                        Vec3::ZERO
                    }
                }
                _ => Vec3::ZERO,
            });

            ctx.sys_request(SysRequest::MakeFx {
                script_entity: ctx.exec.owner,
                name: fx_name,
                at: fx_pos,
            });
            true
        }
        Stmt::Stack(name_expr) => {
            let name = ctx.eval_string(name_expr);
            if let Some(new_script) = ctx.exec.resolve_script(&name, ctx.ctx) {
                let t = ctx.thread_mut();
                let frame = CallFrame {
                    script: t.script.clone(),
                    variables: t.variables.clone(),
                    seq_pc: t.seq_pc,
                    loop_stack: t.loop_stack.clone(),
                };
                t.call_stack.push(frame);
                t.script = new_script.clone();
                init_variables(&mut t.variables, &t.script.variables);
                t.seq_pc = 0;
                t.loop_stack.clear();
                t.state = ExecState::AbortSequence;
            } else {
                warn!(
                    "[ScrOni][{}] Fork: Target Script '{}' not found",
                    ctx.thread().script.name,
                    name
                );
            }
            true
        }
        Stmt::Switch(name_expr) => {
            let name = ctx.eval_string(name_expr);
            if let Some(new_script) = ctx.exec.resolve_script(&name, ctx.ctx) {
                let t = ctx.thread_mut();
                t.script = new_script;
                init_variables(&mut t.variables, &t.script.variables);
                t.seq_pc = 0;
                t.loop_stack.clear();

                if ctx.tid == 0 {
                    ctx.exec.message_queue.clear();
                }

                ctx.thread_mut().state = ExecState::AbortSequence;
            } else {
                warn!(
                    "[ScrOni][{}] Switch: Target Script '{}' not found",
                    ctx.thread().script.name,
                    name
                );
            }
            true
        }
        Stmt::ChildStack { var, script } => {
            let script_name = ctx.eval_string(script);
            if let Some(new_script) = ctx.exec.resolve_script(&script_name, ctx.ctx) {
                let new_tid = ctx.exec.next_thread_id;
                ctx.exec.next_thread_id += 1;
                let mut new_thread = ScrOniThread::new(new_tid, Some(ctx.tid), new_script);
                for var_decl in &new_thread.script.variables {
                    if var_decl.is_parent {
                        continue;
                    }
                    new_thread.variables.insert(
                        var_decl.name.clone(),
                        Value::default_for_type(&var_decl.var_type),
                    );
                }
                ctx.exec.child_threads.push(new_thread);
                ctx.set_var(var.clone(), Value::Int(new_tid as i32));
            } else {
                warn!(
                    "[ScrOni][{}] ChildStack: Target Script '{}' not found",
                    ctx.thread().script.name,
                    script_name
                );
            }
            true
        }
        Stmt::ChildSwitch { var, script } => {
            let script_name = ctx.eval_string(script);
            if let Some(new_script) = ctx.exec.resolve_script(&script_name, ctx.ctx) {
                let new_tid = ctx.exec.next_thread_id;
                ctx.exec.next_thread_id += 1;
                let new_thread = ScrOniThread::new(new_tid, Some(ctx.tid), new_script);
                ctx.exec.child_threads.push(new_thread);
                ctx.set_var(var.clone(), Value::Int(new_tid as i32));
            } else {
                warn!(
                    "[ScrOni][{}] ChildSwitch: Target Script '{}' not found",
                    ctx.thread().script.name,
                    script_name
                );
            }
            true
        }
        Stmt::UsePad => {
            if ctx.ctx.player != Some(ctx.exec.owner) {
                ctx.sys_request(SysRequest::UsePad(ctx.exec.owner));
            }
            ctx.thread_mut().state = ExecState::Yielded;
            true
        }
        Stmt::At(x_expr, y_expr) => {
            let x = ctx.eval_float(x_expr);
            let y = ctx.eval_float(y_expr);
            ctx.sys_request(SysRequest::At(x, y));
            true
        }
        Stmt::DrawText(text_expr) => {
            let text = ctx.eval_string(text_expr);
            ctx.sys_request(SysRequest::DrawText(text));
            true
        }
        Stmt::TextureMovie {
            name,
            pass: _,
            action,
            arg,
        } => {
            let target_name = ctx.eval_string(name);
            let arg_val = ctx.eval(arg);
            ctx.sys_request(SysRequest::TextureMovie {
                target_name,
                action: *action,
                arg: arg_val,
            });
            true
        }
        Stmt::SetLightParameter { args } => {
            if let Some(light_name_expr) = args.first() {
                let name = ctx.eval_string(light_name_expr);
                ctx.exec.current_light = Some(name);
            }
            true
        }
        Stmt::Intensity { args } => {
            if let Some(intensity_expr) = args.first() {
                let intensity = ctx.eval_float(intensity_expr);
                if let Some(light_name) = &ctx.exec.current_light {
                    ctx.sys_request(SysRequest::SetLightIntensity {
                        light: light_name.clone(),
                        intensity,
                    });
                }
            }
            true
        }
        Stmt::PlayerTaskBegin(timeout_expr) => {
            let timeout = timeout_expr.as_ref().map(|e| ctx.eval_float(e));
            ctx.sys_request(SysRequest::PlayerTaskBegin { timeout });
            true
        }
        Stmt::PlayerTaskSuccessful => {
            ctx.sys_request(SysRequest::PlayerTaskSuccessful);
            true
        }
        Stmt::PlayerTaskFailure => {
            ctx.sys_request(SysRequest::PlayerTaskFailure);
            true
        }
        _ => false,
    }
}
