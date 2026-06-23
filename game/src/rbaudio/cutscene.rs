/*
 * rbaudio/cutscene.rs — cutscene audio (rbAudioManager::PlayCutScene/EndCutScene).
 *
 * Plays a one-shot cutscene track and ducks the ambient volume group to the
 * requested background level for its duration.  The original engine ducked
 * but never restored (a documented post-M9 TODO); we restore the saved
 * ambient gain when the cutscene is explicitly ended OR when its track
 * finishes, which is the intended "full functionality".
 */
use bevy::prelude::*;

use super::resolve::AudioAssets;
use super::{AudioGroup, AudioGroupId, CutSceneState, RbAudioManager};

impl RbAudioManager {
    /// `rbAudioManager::PlayCutScene` — queue a cutscene track and duck the
    /// ambient group to `bg_volume`.  `cs_volume = None` keeps the source's
    /// default volume (mirrors the `csvolume < 0` skip).
    pub fn play_cutscene(
        &mut self,
        source: Handle<bevy::audio::AudioSource>,
        cs_volume: Option<f32>,
        bg_volume: f32,
    ) {
        self.cutscene_pending = Some((source, cs_volume, bg_volume));
    }

    /// `rbAudioManager::EndCutScene`.
    pub fn end_cutscene(&mut self) {
        self.cutscene_end_requested = true;
    }
}

/// Spawns queued cutscene tracks, applies/restores the ambient duck, and
/// watches for the track ending.
pub fn cutscene_watch_system(
    mut commands: Commands,
    mut manager: ResMut<RbAudioManager>,
    mut assets: ResMut<AudioAssets>,
    mut audio_sources: ResMut<Assets<bevy::audio::AudioSource>>,
) {
    // --- End request ---------------------------------------------------
    if std::mem::take(&mut manager.cutscene_end_requested)
        && let Some(state) = manager.cutscene.take()
    {
        if let Ok(mut ec) = commands.get_entity(state.entity) {
            ec.despawn();
        }
        manager.set_group_volume(AudioGroupId::Ambient, state.saved_ambient_gain);
    }

    // --- New cutscene queued ------------------------------------------
    if let Some((source, cs_volume, bg_volume)) = manager.cutscene_pending.take() {
        assets.ensure_loaded();

        // Replacing an in-flight cutscene: drop the old track but keep the
        // originally-saved ambient gain so we still restore correctly.
        let saved_ambient_gain = match manager.cutscene.take() {
            Some(prev) => {
                if let Ok(mut ec) = commands.get_entity(prev.entity) {
                    ec.despawn();
                }
                prev.saved_ambient_gain
            }
            None => manager.group_gain(AudioGroupId::Ambient),
        };

        let mut settings = bevy::audio::PlaybackSettings::DESPAWN;
        if let Some(v) = cs_volume {
            settings = settings.with_volume(bevy::audio::Volume::Linear(v));
        }
        // Cutscene track rides the Player group (full gain) so the ambient
        // duck doesn't attenuate it.
        let base = cs_volume.unwrap_or(1.0);
        let entity = commands
            .spawn((
                bevy::audio::AudioPlayer(source),
                settings,
                AudioGroup::new(AudioGroupId::Player, base),
                crate::menu::InGameEntity,
            ))
            .id();

        manager.set_group_volume(AudioGroupId::Ambient, bg_volume);
        manager.cutscene = Some(CutSceneState {
            entity,
            saved_ambient_gain,
        });
    }

    // --- Natural end of the cutscene track -----------------------------
    let finished = manager
        .cutscene
        .as_ref()
        .map(|s| commands.get_entity(s.entity).is_err())
        .unwrap_or(false);
    if finished && let Some(state) = manager.cutscene.take() {
        manager.set_group_volume(AudioGroupId::Ambient, state.saved_ambient_gain);
    }
}
