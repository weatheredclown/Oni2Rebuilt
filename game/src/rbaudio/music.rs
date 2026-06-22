/*
 * rbaudio/music.rs — two-channel music crossfade (rbAudioManager music half).
 *
 * `MusicPlay` queues a new track; the blend tick spawns it on the alternate
 * of two channels and ramps the outgoing channel down while the incoming
 * ramps up over `blendTime`, then stops the old channel.  Music entities are
 * tagged with the `Music` `AudioGroup`, so the master apply pass scales them.
 */
use bevy::prelude::*;

use super::resolve::{resolve_sound, AudioAssets};
use super::{AudioGroup, AudioGroupId, RbAudioManager, MUSIC_BLEND_FLOOR};

/// Drives `music_pending` / `music_stop_requested` and the active crossfade.
pub fn music_blend_system(
    mut commands: Commands,
    time: Res<Time>,
    mut manager: ResMut<RbAudioManager>,
    mut assets: ResMut<AudioAssets>,
    mut audio_sources: ResMut<Assets<bevy::audio::AudioSource>>,
    mut groups: Query<&mut AudioGroup>,
) {
    // --- Stop request --------------------------------------------------
    if std::mem::take(&mut manager.music_stop_requested) {
        for ch in manager.music.iter_mut() {
            if let Some(e) = ch.entity.take()
                && let Ok(mut ec) = commands.get_entity(e)
            {
                ec.despawn();
            }
            ch.volume = 0.0;
        }
        manager.music_index = 0;
        manager.music_blend_mode = false;
    }

    // --- New track queued ---------------------------------------------
    if let Some((source, volume, blend_time)) = manager.music_pending.take() {
        let old_index = manager.music_index;
        manager.music_blend_volume_old = manager.music[old_index].volume;
        manager.music_blend_index_to_fade_out = old_index;

        let new_index = (old_index + 1) % manager.music.len();

        // Stop whatever was on the channel we're about to reuse.
        if let Some(e) = manager.music[new_index].entity.take()
            && let Ok(mut ec) = commands.get_entity(e)
        {
            ec.despawn();
        }

        let entity = commands
            .spawn((
                bevy::audio::AudioPlayer(source),
                bevy::audio::PlaybackSettings::LOOP
                    .with_volume(bevy::audio::Volume::Linear(MUSIC_BLEND_FLOOR)),
                AudioGroup::new(AudioGroupId::Music, MUSIC_BLEND_FLOOR),
                crate::menu::InGameEntity,
            ))
            .id();

        manager.music[new_index].entity = Some(entity);
        manager.music[new_index].volume = volume;
        manager.music_index = new_index;
        manager.music_blend_volume = volume;
        manager.music_blend_time = blend_time;
        manager.music_blend_elapsed = 0.0;
        manager.music_blend_mode = true;
    }

    // --- Advance the crossfade ----------------------------------------
    if !manager.music_blend_mode {
        return;
    }

    manager.music_blend_elapsed += time.delta_secs();
    let index = manager.music_index;
    let fade_out = manager.music_blend_index_to_fade_out;
    let blend_time = manager.music_blend_time;
    let blend_volume = manager.music_blend_volume;
    let blend_volume_old = manager.music_blend_volume_old;

    let set_base = |groups: &mut Query<&mut AudioGroup>, ent: Option<Entity>, v: f32| {
        if let Some(e) = ent
            && let Ok(mut g) = groups.get_mut(e)
        {
            g.base_volume = v;
        }
    };

    if manager.music_blend_elapsed < blend_time {
        let ratio = manager.music_blend_elapsed / blend_time;
        if fade_out != index {
            set_base(&mut groups, manager.music[fade_out].entity, (1.0 - ratio) * blend_volume_old);
        }
        set_base(
            &mut groups,
            manager.music[index].entity,
            (ratio * blend_volume).max(MUSIC_BLEND_FLOOR),
        );
    } else {
        // Blend complete: stop the others, pin the live channel to target.
        manager.music_blend_mode = false;
        for a in 0..manager.music.len() {
            if a != index {
                if let Some(e) = manager.music[a].entity.take()
                    && let Ok(mut ec) = commands.get_entity(e)
                {
                    ec.despawn();
                }
                manager.music[a].volume = 0.0;
            } else {
                set_base(&mut groups, manager.music[a].entity, blend_volume);
                manager.music[a].volume = blend_volume;
            }
        }
    }
}
