/*
 * scroni/ops/audio.rs — audio script opcodes.
 *
 * Handles Stmt::Sound (play / stop sound on an actor) and any other
 * audio-related statement variants.  Issues SysRequest::PlaySound which the
 * system_bindings observer routes to Bevy's audio system.
 */
use super::OpsCtx;
use crate::scroni::ast::Stmt;
use crate::scroni::vm::{ActorSoundVerb, SysRequest};
use bevy::prelude::*;

pub fn exec(ctx: &mut OpsCtx, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Sound { args } => {
            // Mirror C++ DoSound (XSoundCommand.cpp): args lay out as
            //   [channel-expr, action-string-lit, optional value-expr]
            // Action determines what (if anything) the value is — a
            // package-name string for `play`, a numeric for
            // pitch/volume/fadeIn/fadeOut, nothing for pause/stop.
            if args.len() < 2 {
                warn!(
                    "VM: sound: expected `<channel> <action> [arg]`, got {:?}",
                    args
                );
                return true;
            }

            let channel = ctx.eval_int(&args[0]);
            let action = ctx.eval_string(&args[1]).to_ascii_lowercase();

            let verb = match action.as_str() {
                "play" => {
                    if let Some(pkg_expr) = args.get(2) {
                        let pkg = ctx.eval_string(pkg_expr);
                        if pkg.is_empty() {
                            ActorSoundVerb::Play
                        } else {
                            ActorSoundVerb::PlayNamed(pkg)
                        }
                    } else {
                        ActorSoundVerb::Play
                    }
                }
                "pause" => ActorSoundVerb::Pause,
                "stop" => ActorSoundVerb::Stop,
                "pitch" => {
                    let v = args.get(2).map(|e| ctx.eval_float(e)).unwrap_or(1.0);
                    ActorSoundVerb::Pitch(v)
                }
                "volume" => {
                    let v = args.get(2).map(|e| ctx.eval_float(e)).unwrap_or(1.0);
                    ActorSoundVerb::Volume(v)
                }
                "fadein" => {
                    let v = args.get(2).map(|e| ctx.eval_float(e)).unwrap_or(0.0);
                    ActorSoundVerb::FadeIn(v)
                }
                "fadeout" => {
                    let v = args.get(2).map(|e| ctx.eval_float(e)).unwrap_or(0.0);
                    ActorSoundVerb::FadeOut(v)
                }
                other => {
                    warn!("VM: sound: unknown action `{}` (args={:?})", other, args);
                    return true;
                }
            };

            ctx.sys_request(SysRequest::ActorSoundCommand {
                actor: ctx.exec.owner,
                channel,
                verb,
            });
            true
        }
        Stmt::PlayAmbientSound {
            name: _,
            volume: _,
            pitch: _,
            volume_ramp: _,
            pitch_ramp: _,
        } => {
            // Implemented inside AmbientSound typically or directly dispatched
            true
        }
        Stmt::AmbientSound { args } => {
            if args.len() == 2 {
                // "all" is the shorthand for stop-every-running-ambient
                // (M03 LevelMgr.oni:1517 `AmbientSound all Stop`).
                // Detect it by looking at the raw arg — it comes through
                // as either an identifier or a string "all", which
                // `eval_int` would collapse to 0 (matching no handle).
                let action = ctx.eval_string(&args[1]);
                let first_as_str = ctx.eval_string(&args[0]);
                let first_is_all = first_as_str.eq_ignore_ascii_case("all");
                if action.eq_ignore_ascii_case("stop") && first_is_all {
                    ctx.sys_request(SysRequest::AmbientSoundStopAll);
                    info!("VM: AmbientSound all Stop");
                } else if action.eq_ignore_ascii_case("stop") {
                    let handle = ctx.eval_int(&args[0]);
                    ctx.sys_request(SysRequest::AmbientSoundStop(handle));
                    info!("VM: AmbientSound Stop {}", handle);
                } else {
                    info!(
                        "VM: AmbientSound {:?} (unsupported action: {})",
                        args, action
                    );
                }
            } else if args.len() == 4 {
                let handle = ctx.eval_int(&args[0]);
                let action = ctx.eval_string(&args[1]);
                if action.eq_ignore_ascii_case("volumeramp") {
                    let target_vol = ctx.eval_float(&args[2]);
                    let duration = ctx.eval_float(&args[3]);
                    ctx.sys_request(SysRequest::AmbientSoundVolumeRamp(
                        handle, target_vol, duration,
                    ));
                    info!(
                        "VM: AmbientSound VolumeRamp {} -> {} in {}",
                        handle, target_vol, duration
                    );
                } else if action.eq_ignore_ascii_case("pitchramp") {
                    let target_pitch = ctx.eval_float(&args[2]);
                    let duration = ctx.eval_float(&args[3]);
                    ctx.sys_request(SysRequest::AmbientSoundPitchRamp(
                        handle,
                        target_pitch,
                        duration,
                    ));
                    info!(
                        "VM: AmbientSound PitchRamp {} -> {} in {}",
                        handle, target_pitch, duration
                    );
                } else {
                    info!(
                        "VM: AmbientSound {:?} (unsupported action: {})",
                        args, action
                    );
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
                    info!(
                        "VM: AmbientSound {:?} (unsupported action: {})",
                        args, action
                    );
                }
            } else {
                info!("VM: AmbientSound {:?} (unimplemented format)", args);
            }
            true
        }
        Stmt::MusicPlay(expr) => {
            // `MusicPlay <name>` — resolve the track and hand it to the
            // manager's two-channel crossfade.  ScrOni only supplies a
            // name; volume defaults to full and the blend is instant
            // (blend_time 0) unless future args extend this.
            let name = ctx.eval_string(expr);
            ctx.sys_request(SysRequest::MusicPlay(name, 1.0, 0.0));
            true
        }
        Stmt::MusicStop => {
            ctx.sys_request(SysRequest::MusicStop);
            true
        }
        Stmt::StartCutSceneAudio { args } => {
            // `StartCutSceneAudio <name> [csVolume] [bgVolume]`.  Matches
            // rbAudioManager::PlayCutScene defaults: csvolume<0 (None) keeps
            // the source volume, bgvolume defaults to 0 (full ambient duck).
            if args.is_empty() {
                warn!("VM: StartCutSceneAudio: expected a name");
                return true;
            }
            let name = ctx.eval_string(&args[0]);
            let cs_volume = args.get(1).map(|e| ctx.eval_float(e)).filter(|v| *v >= 0.0);
            let bg_volume = args.get(2).map(|e| ctx.eval_float(e)).unwrap_or(0.0);
            ctx.sys_request(SysRequest::PlayCutScene(name, cs_volume, bg_volume));
            true
        }
        Stmt::EndCutSceneAudio => {
            ctx.sys_request(SysRequest::EndCutScene);
            true
        }
        _ => false,
    }
}
