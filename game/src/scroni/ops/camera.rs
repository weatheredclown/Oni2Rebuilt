/*
 * scroni/ops/camera.rs — camera script opcodes.
 *
 * Handles CameraReset, CameraScript (begin scripted sequence), and any
 * camera-mode overrides.  Issues SysRequest::CameraReset / CameraScript
 * which system_bindings forwards to CameraController.
 */
use super::OpsCtx;
use crate::scroni::ast::Stmt;
use crate::scroni::vm::{SysRequest, Value};

pub fn exec(ctx: &mut OpsCtx, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::CameraReset => {
            ctx.sys_request(SysRequest::CameraReset);
            true
        }
        Stmt::CameraMode { mode, time } => {
            let mode_str = ctx.eval_string(mode);
            let dur = time.as_ref().map(|t| ctx.eval_float(t));
            ctx.sys_request(SysRequest::CameraMode(mode_str, dur));
            true
        }
        Stmt::CameraLetterbox(expr) => {
            let _on = ctx.eval_bool(expr);
            true
        }
        Stmt::CameraFollowActor { args } => {
            if !args.is_empty()
                && let Value::Actor(ent) = ctx.eval(&args[0])
            {
                ctx.sys_request(SysRequest::CameraFollowActor(ent));
            }
            true
        }
        Stmt::CameraTrackActor { args } => {
            if !args.is_empty()
                && let Value::Actor(ent) = ctx.eval(&args[0])
            {
                ctx.sys_request(SysRequest::CameraTrackActor(ent));
            }
            true
        }
        Stmt::CameraTrackPoint { args } => {
            if !args.is_empty()
                && let Value::Vector(v) = ctx.eval(&args[0])
            {
                ctx.sys_request(SysRequest::CameraTrackPoint(v));
            }
            true
        }
        Stmt::CameraMoveToActor { args } => {
            if args.len() >= 2
                && let Value::Actor(ent) = ctx.eval(&args[0])
            {
                let dur = ctx.eval_float(&args[1]);
                ctx.sys_request(SysRequest::CameraMoveToActor(ent, dur));
            }
            true
        }
        Stmt::CameraMoveToPoint { args } => {
            if args.len() >= 2
                && let Value::Vector(v) = ctx.eval(&args[0])
            {
                let dur = ctx.eval_float(&args[1]);
                ctx.sys_request(SysRequest::CameraMoveToPoint(v, dur));
            }
            true
        }
        Stmt::CameraCutToActor { args } => {
            if !args.is_empty()
                && let Value::Actor(ent) = ctx.eval(&args[0])
            {
                ctx.sys_request(SysRequest::CameraMoveToActor(ent, 0.0));
            }
            true
        }
        Stmt::CameraCutToPoint { args } => {
            if !args.is_empty()
                && let Value::Vector(v) = ctx.eval(&args[0])
            {
                ctx.sys_request(SysRequest::CameraMoveToPoint(v, 0.0));
            }
            true
        }
        Stmt::CameraSetFOV { args } => {
            if args.len() >= 2 {
                let fov = ctx.eval_float(&args[0]);
                let dur = ctx.eval_float(&args[1]);
                ctx.sys_request(SysRequest::CameraSetFOV(fov, dur));
            }
            true
        }
        Stmt::CameraSetPackage(expr) => {
            let pkg_name = ctx.eval_string(expr);
            ctx.sys_request(SysRequest::CameraSetPackage(pkg_name));
            true
        }
        Stmt::CameraShake => {
            ctx.sys_request(SysRequest::CameraShake);
            true
        }
        _ => false,
    }
}
