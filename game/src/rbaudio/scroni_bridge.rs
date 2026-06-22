/*
 * rbaudio/scroni_bridge.rs — observer routing ScrOni audio events into the
 * manager.  Kept separate from the big `scroni_sys_event_observer` so the
 * manager + audio-asset resources don't bloat that system's param list.
 */
use bevy::prelude::*;

use super::resolve::{resolve_sound, AudioAssets};
use super::RbAudioManager;
use crate::scroni::vm::ScrOniSysEvent;

/// Handles `MusicPlay` / `MusicStop` / `PlayCutScene` / `EndCutScene`; ignores
/// every other `ScrOniSysEvent` (handled by the main observer).
pub fn scroni_audio_observer(
    trigger: On<ScrOniSysEvent>,
    mut manager: ResMut<RbAudioManager>,
    mut assets: ResMut<AudioAssets>,
    mut audio_sources: ResMut<Assets<bevy::audio::AudioSource>>,
) {
    match &*trigger {
        ScrOniSysEvent::MusicPlay {
            name,
            volume,
            blend_time,
        } => {
            assets.ensure_loaded();
            match resolve_sound(name, &assets, &mut audio_sources) {
                Some((source, _loop)) => {
                    manager.music_play(source, *volume, *blend_time);
                    info!("VM: MusicPlay '{}' (vol {}, blend {}s)", name, volume, blend_time);
                }
                None => warn!("VM: MusicPlay — could not resolve track '{}'", name),
            }
        }
        ScrOniSysEvent::MusicStop => {
            manager.music_stop();
            info!("VM: MusicStop");
        }
        ScrOniSysEvent::PlayCutScene {
            name,
            cs_volume,
            bg_volume,
        } => {
            assets.ensure_loaded();
            match resolve_sound(name, &assets, &mut audio_sources) {
                Some((source, _loop)) => {
                    manager.play_cutscene(source, *cs_volume, *bg_volume);
                    info!("VM: PlayCutScene '{}' (bg duck {})", name, bg_volume);
                }
                None => warn!("VM: PlayCutScene — could not resolve track '{}'", name),
            }
        }
        ScrOniSysEvent::EndCutScene => {
            manager.end_cutscene();
            info!("VM: EndCutScene");
        }
        _ => {}
    }
}
