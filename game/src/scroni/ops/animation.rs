/*
 * scroni/ops/animation.rs — animation script opcodes.
 *
 * Handles GotoCurvePhase and any other animation-related Stmt variants.
 * Issues BlockingAction::GotoCurvePhase to pause the script thread until
 * the entity reaches the specified curve phase.
 */
use super::OpsCtx;
use crate::scroni::ast::Stmt;
use crate::scroni::vm::BlockingAction;

pub fn exec(ctx: &mut OpsCtx, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::GotoCurvePhase { phase, seconds } => {
            let p = ctx.eval_float(phase);
            let s = ctx.eval_float(seconds);
            ctx.block(BlockingAction::GotoCurvePhase {
                target: p,
                seconds: s,
            });
            true
        }
        Stmt::GotoCurveKnot { knot, seconds } => {
            let k = ctx.eval_int(knot);
            let s = ctx.eval_float(seconds);
            ctx.block(BlockingAction::GotoCurveKnot {
                knot: k,
                seconds: s,
            });
            true
        }
        Stmt::GotoCurveLerp { lerp, seconds } => {
            let l = ctx.eval_float(lerp);
            let s = ctx.eval_float(seconds);
            ctx.block(BlockingAction::GotoCurveLerp {
                target: l,
                seconds: s,
            });
            true
        }
        Stmt::SetCurvePhase(expr) => {
            let v = ctx.eval(expr);
            ctx.exec.set_var(ctx.tid, "__curve_phase".into(), v);
            true
        }
        Stmt::SetCurveSpeed(expr) => {
            let v = ctx.eval(expr);
            ctx.set_var("__curve_speed".into(), v);
            true
        }
        Stmt::SetCurveKs(expr) => {
            let v = ctx.eval(expr);
            ctx.set_var("__curve_ks".into(), v);
            true
        }
        Stmt::SetCurvePingPong(expr) => {
            let v = ctx.eval(expr);
            ctx.set_var("__curve_pingpong".into(), v);
            true
        }
        Stmt::SetCurve { name, at_phase } => {
            let n = ctx.eval(name);
            ctx.set_var("__curve_name".into(), n);
            if let Some(p) = at_phase {
                let v = ctx.eval(p);
                ctx.set_var("__curve_phase".into(), v);
            }
            true
        }
        Stmt::SetLerpCurve(expr) => {
            let v = ctx.eval(expr);
            ctx.set_var("__lerp_curve".into(), v);
            true
        }
        Stmt::SetLookUpCurve(expr) => {
            let v = ctx.eval(expr);
            ctx.set_var("__lookup_curve".into(), v);
            true
        }
        Stmt::SetCurveLookAtActor(expr) => {
            let v = ctx.eval(expr);
            ctx.set_var("__curve_lookat".into(), v);
            true
        }
        Stmt::SetCurveLookAlongDistance(expr) => {
            let v = ctx.eval(expr);
            ctx.set_var("__curve_lookalong_dist".into(), v);
            true
        }
        Stmt::SetCurveLookAlongDirection(expr) => {
            let v = ctx.eval(expr);
            ctx.set_var("__curve_lookalong_dir".into(), v);
            true
        }
        Stmt::PlayAnimation {
            name,
            hold,
            loop_anim,
            rate,
            duration,
        } => {
            let n = ctx.eval_string(name);
            let r = rate.as_ref().map(|e| ctx.eval_float(e));
            let d = duration.as_ref().map(|e| ctx.eval_float(e));
            ctx.block(BlockingAction::PlayAnimation {
                name: n,
                hold: *hold,
                loop_anim: *loop_anim,
                rate: r,
                duration: d,
            });
            true
        }
        Stmt::PlayActionAnimation {
            name,
            hold,
            loop_anim,
            duration,
        } => {
            let n = ctx.eval_string(name);
            let d = duration.as_ref().map(|e| ctx.eval_float(e));
            ctx.block(BlockingAction::PlayAnimation {
                name: n,
                hold: *hold,
                loop_anim: *loop_anim,
                rate: None,
                duration: d,
            });
            true
        }
        Stmt::ControlAnimation { name: _ } => {
            // Control animation isn't fully implemented in the main loop either
            true
        }
        _ => false,
    }
}
