/*
 * scroni/ops/combat.rs — combat script opcodes.
 *
 * Handles Stmt::Attack (script-triggered attack on a target entity).
 * Issues SysRequest::TriggerAttack resolved by system_bindings into an
 * AttackMessage on the relevant fighter entity.
 */
use super::OpsCtx;
use crate::scroni::ast::{Expr, Stmt};
use crate::scroni::vm::{BlockingAction, ExecState, SysRequest, Value};
use bevy::prelude::*;

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
            let target_ent = opt_expr.as_ref().map(|expr| match ctx.eval(expr) {
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
            });
            ctx.sys_request(SysRequest::TriggerFight {
                actor: ctx.exec.owner,
                target: target_ent,
            });
            ctx.thread_mut().state = crate::scroni::vm::ExecState::Yielded;
            true
        }
        Stmt::Shoot { args } => {
            // Walk the arg list to pull out the target, miss flag,
            // rate/bullets, and duration.  Target is the first non-
            // keyword expr; flags/modifiers are string literals emitted
            // by the compiler (`"miss"`/`"bullets"`/`"rate"`/`"for"`)
            // each followed by the relevant value.
            let mut target_val: Option<Value> = None;
            let mut miss = false;
            let mut rate: Option<f32> = None;
            let mut bullets: Option<i32> = None;
            let mut duration: Option<f32> = None;
            let mut i = 0;
            while i < args.len() {
                match &args[i] {
                    Expr::StringLit(s) if s.eq_ignore_ascii_case("miss") => {
                        miss = true;
                    }
                    Expr::StringLit(s) if s.eq_ignore_ascii_case("rate") => {
                        if i + 1 < args.len() {
                            rate = Some(ctx.eval_float(&args[i + 1]));
                            i += 1;
                        }
                    }
                    Expr::StringLit(s) if s.eq_ignore_ascii_case("bullets") => {
                        if i + 1 < args.len() {
                            bullets = Some(ctx.eval_int(&args[i + 1]));
                            i += 1;
                        }
                    }
                    Expr::StringLit(s) if s.eq_ignore_ascii_case("for") => {
                        if i + 1 < args.len() {
                            duration = Some(ctx.eval_float(&args[i + 1]));
                            i += 1;
                        }
                    }
                    other if target_val.is_none() => {
                        target_val = Some(ctx.eval(other));
                    }
                    _ => {}
                }
                i += 1;
            }
            info!(
                "VM: Shoot target={:?} miss={} rate={:?} bullets={:?} for={:?}s",
                target_val, miss, rate, bullets, duration
            );
            // Block the thread for `for` seconds.  Mirrors `idle N` —
            // the script pauses while (presumably) the shooter loop
            // fires projectiles; we don't have that loop wired, but at
            // least the `for` value is now honored as a real wait
            // instead of yielding for exactly one tick.
            if let Some(secs) = duration {
                ctx.thread_mut().blocking = Some(BlockingAction::Idle {
                    end_time: ctx.now + secs as f64,
                });
                ctx.thread_mut().state = ExecState::Blocked;
            } else {
                ctx.thread_mut().state = ExecState::Yielded;
            }
            true
        }
        Stmt::Look { args } => {
            // `look <listvar> status <state>, <state>, ...` — walk args:
            // first non-keyword expr is the target list variable name;
            // "status" is a separator; subsequent string literals are
            // state names (alive/enemy/dead/etc.) to AND-filter on.
            let mut list_var: Option<String> = None;
            let mut states: Vec<String> = Vec::new();
            let mut saw_status = false;
            for expr in args {
                match expr {
                    Expr::Var(name) if list_var.is_none() => {
                        list_var = Some(name.clone());
                    }
                    Expr::StringLit(s) if s.eq_ignore_ascii_case("status") => {
                        saw_status = true;
                    }
                    Expr::StringLit(s) if saw_status => {
                        states.push(s.to_lowercase());
                    }
                    _ => {}
                }
            }

            if let Some(name) = list_var {
                let mut found_actors = Vec::new();

                // Get caller's perception info
                let owner = ctx.exec.owner;
                let looker_rad = ctx
                    .ctx
                    .get_perception_radius
                    .as_ref()
                    .map(|f| f(owner))
                    .unwrap_or(30.0);
                let looker_fov = ctx
                    .ctx
                    .get_perception_fov
                    .as_ref()
                    .map(|f| f(owner))
                    .unwrap_or(45.0_f32.to_radians());

                if let Ok((_, looker_tf, _)) = ctx.ctx.all_entities.get(owner) {
                    let looker_pos = looker_tf.translation();
                    // Use OniTransformExt to get the true visual forward of the ONI model
                    use crate::oni2_loader::utils::space::OniTransformExt;
                    let looker_forward = looker_tf.oni_forward().xz().normalize_or_zero();

                    for (ent, tgt_tf, _) in ctx.ctx.all_entities.iter() {
                        if ent == owner {
                            continue;
                        }

                        // Status checks first (so we can see if player is recognized as enemy even if behind them)
                        let cur_status = ctx
                            .ctx
                            .actor_statuses
                            .get(&ent)
                            .map(|s| s.as_str())
                            .unwrap_or("");
                        let mut passes_filter = true;

                        let is_player = ctx.ctx.player == Some(ent);
                        let dir_to_tgt =
                            (tgt_tf.translation() - looker_pos).xz().normalize_or_zero();
                        if is_player {
                            info!(
                                "  -> checking PLAYER entity {:?} (dist: {:.1}, angle: {:.1} deg)",
                                ent,
                                looker_pos.distance(tgt_tf.translation()),
                                looker_forward.angle_between(dir_to_tgt).to_degrees()
                            );
                        }

                        for required_state in &states {
                            let matches = match required_state.as_str() {
                                "alive" => !cur_status.is_empty() && cur_status != "dead",
                                "dead" => cur_status == "dead" || cur_status.is_empty(),
                                "fighting" => cur_status == "fighting",
                                "notdying" => cur_status != "dying" && cur_status != "dead",
                                "player" => is_player,
                                "enemy" => ctx
                                    .ctx
                                    .is_enemy
                                    .as_ref()
                                    .map(|f| f(owner, ent))
                                    .unwrap_or(ent != owner),
                                "friendly" => {
                                    let is_enemy = ctx
                                        .ctx
                                        .is_enemy
                                        .as_ref()
                                        .map(|f| f(owner, ent))
                                        .unwrap_or(ent != owner);
                                    !is_enemy
                                }
                                _ => false,
                            };
                            if !matches {
                                if is_player {
                                    info!(
                                        "  -> PLAYER {:?} failed status: required {}, had {}",
                                        ent, required_state, cur_status
                                    );
                                }
                                passes_filter = false;
                                break;
                            }
                        }

                        if !passes_filter {
                            continue;
                        }

                        // Spatial checks
                        let tgt_pos = tgt_tf.translation();
                        let dist = looker_pos.distance(tgt_pos);
                        if dist > looker_rad {
                            if is_player {
                                info!(
                                    "  -> PLAYER {:?} failed dist: {:.1} > {:.1}",
                                    ent, dist, looker_rad
                                );
                            }
                            continue;
                        }

                        let dir_to_tgt = (tgt_pos - looker_pos).xz().normalize_or_zero();
                        let angle = looker_forward.angle_between(dir_to_tgt).abs();
                        if angle > looker_fov {
                            if is_player {
                                info!(
                                    "  -> PLAYER {:?} failed FOV: {:.1} > {:.1}",
                                    ent,
                                    angle.to_degrees(),
                                    looker_fov.to_degrees()
                                );
                            }
                            continue;
                        }

                        // Optional LOS check - aim a bit higher (chest level)
                        if let Some(los_checker) = ctx.ctx.line_of_sight.as_ref() {
                            let head_offset = Vec3::Y * 1.5;
                            if !los_checker(
                                looker_pos + head_offset,
                                tgt_pos + head_offset,
                                owner,
                                ent,
                            ) {
                                trace!("  -> ent {:?} failed LOS", ent);
                                continue;
                            }
                        }

                        trace!("  -> ent {:?} passed all checks!", ent);
                        found_actors.push(ent);
                    }
                }

                if found_actors.is_empty() {
                    ctx.set_var(name.clone(), Value::Int(0));
                } else {
                    ctx.set_var(name.clone(), Value::ActorList(found_actors.clone(), 0));
                }

                let log_msg = format!(
                    "VM: Look {} status {:?} (rad: {:.1}, fov: {:.1}) → found {} actors",
                    name,
                    states,
                    looker_rad,
                    looker_fov.to_degrees(),
                    found_actors.len()
                );
                // ALWAYS print this for debugging
                info!("{}", log_msg);
            } else {
                warn!("VM: Look had no listvar target (args={:?})", args);
            }
            ctx.thread_mut().state = ExecState::Yielded;
            true
        }
        Stmt::Hit {
            hit_type,
            victim,
            damage,
        } => {
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
        Stmt::MakeExplosion {
            name,
            orientation,
            at,
        } => {
            let name_val = ctx.eval_string(name);
            let at_val = ctx.eval_vec3(at);
            let ori_val = orientation
                .as_ref()
                .map(|o| ctx.eval_vec3(o))
                .unwrap_or([0.0; 3]);

            ctx.sys_request(crate::scroni::vm::SysRequest::MakeExplosion {
                name: name_val,
                at: at_val,
                orientation: ori_val,
            });

            ctx.thread_mut().state = crate::scroni::vm::ExecState::Yielded;
            true
        }
        Stmt::Retreat(opt_expr) => {
            let target_ent = opt_expr.as_ref().map(|expr| match ctx.eval(expr) {
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
