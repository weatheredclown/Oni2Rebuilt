/*
 * scroni/ops/audio.rs — audio script opcodes.
 *
 * Handles Stmt::Sound (play / stop sound on an actor) and any other
 * audio-related statement variants.  Issues SysRequest::PlaySound which the
 * system_bindings observer routes to Bevy's audio system.
 */
use crate::scroni::ast::Stmt;
use crate::scroni::vm::{SysRequest, Value};
use bevy::prelude::*;
use super::OpsCtx;

pub fn exec(ctx: &mut OpsCtx, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Sound { args } => {
            if args.len() >= 3 {
                // sound [actor] play [name]
                let actor_val = ctx.eval(&args[0]);
                let action = ctx.eval_string(&args[1]);
                let name = ctx.eval_string(&args[2]);
                if action.eq_ignore_ascii_case("play") {
                    let actor_str = if let Value::Int(0) = actor_val {
                        None
                    } else {
                        Some(actor_val.as_string())
                    };
                    ctx.sys_request(SysRequest::PlaySound(actor_str, name));
                } else {
                    info!("VM: Sound unsupported action {}", action);
                }
            } else if args.len() >= 2 {
                 // sound play [name]
                 let action = ctx.eval_string(&args[0]);
                 let name = ctx.eval_string(&args[1]);
                 if action.eq_ignore_ascii_case("play") {
                     ctx.sys_request(SysRequest::PlaySound(None, name));
                 }
            } else {
                info!("VM: Sound {:?} (invalid args)", args);
            }
            true
        }
        Stmt::PlayAmbientSound { name: _, volume: _, pitch: _, volume_ramp: _, pitch_ramp: _ } => {
            // Implemented inside AmbientSound typically or directly dispatched
            true
        }
        Stmt::AmbientSound { args } => {
            if args.len() == 2 {
                let handle = ctx.eval_int(&args[0]);
                let action = ctx.eval_string(&args[1]);
                if action.eq_ignore_ascii_case("stop") {
                    ctx.sys_request(SysRequest::AmbientSoundStop(handle));
                    info!("VM: AmbientSound Stop {}", handle);
                } else {
                    info!("VM: AmbientSound {:?} (unsupported action: {})", args, action);
                }
            } else if args.len() == 4 {
                let handle = ctx.eval_int(&args[0]);
                let action = ctx.eval_string(&args[1]);
                if action.eq_ignore_ascii_case("volumeramp") {
                    let target_vol = ctx.eval_float(&args[2]);
                    let duration = ctx.eval_float(&args[3]);
                    ctx.sys_request(SysRequest::AmbientSoundVolumeRamp(handle, target_vol, duration));
                    info!("VM: AmbientSound VolumeRamp {} -> {} in {}", handle, target_vol, duration);
                } else if action.eq_ignore_ascii_case("pitchramp") {
                    let target_pitch = ctx.eval_float(&args[2]);
                    let duration = ctx.eval_float(&args[3]);
                    ctx.sys_request(SysRequest::AmbientSoundPitchRamp(handle, target_pitch, duration));
                    info!("VM: AmbientSound PitchRamp {} -> {} in {}", handle, target_pitch, duration);
                } else {
                    info!("VM: AmbientSound {:?} (unsupported action: {})", args, action);
                }
            } else if args.len() == 3 {
                let handle = ctx.eval_int(&args[0]);
                let action = ctx.eval_string(&args[1]);
                if action.eq_ignore_ascii_case("pitch") {
                    let target_pitch = ctx.eval_float(&args[2]);
                    ctx.sys_request(SysRequest::AmbientSoundPitchRamp(handle, target_pitch, 0.0));
                    info!("VM: AmbientSound Pitch {} -> {}", handle, target_pitch);
                } else if action.eq_ignore_ascii_case("volume") {
                    let target_vol = ctx.eval_float(&args[2]);
                    ctx.sys_request(SysRequest::AmbientSoundVolumeRamp(handle, target_vol, 0.0));
                    info!("VM: AmbientSound Volume {} -> {}", handle, target_vol);
                } else {
                    info!("VM: AmbientSound {:?} (unsupported action: {})", args, action);
                }
            } else {
                info!("VM: AmbientSound {:?} (unimplemented format)", args);
            }
            true
        }
        Stmt::MusicPlay(_expr) => {
            true // not natively wired locally
        }
        Stmt::MusicStop => {
            true
        }
        _ => false,
    }
}
