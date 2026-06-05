/*
 * actor_sound.rs — per-actor <Sound> component runtime.
 *
 * Port of rbAudioSoundComponent: picks a nugget from the configured
 * audio package, resolves through the TD manifest to an HD/BD bank
 * pair, decodes PSX ADPCM, and spawns a child AudioPlayer.  Honors
 * the legacy PlayMode enum (one-shot vs looped variants), applies the
 * authored VolumeScalar, and scales runtime volume against player
 * distance using RangeMaxVolume / RangeZeroVolume.  StartActive=true
 * actors begin playing on the first tick after spawn.
 */
use bevy::prelude::*;

use crate::oni2_loader::parsers::actor_xml::{SoundComponentData, SoundPlayMode};
use crate::scroni::vm::ActorSoundVerb;

/// Per-actor component carrying the parsed `<Sound>` data plus
/// runtime playback bookkeeping.  Attached by the layout loader when
/// the actor's XML declared a non-empty AudioPackage.
#[derive(Component, Debug, Clone)]
pub struct ActorSound {
    pub data: SoundComponentData,
    /// True if the engine should keep this source playing.  Mirrors
    /// the activate/deactivate ActorSpecific dispatch used by C++
    /// `audMsgPlaySound`'s kPlay/kStop verbs.  Initialized from
    /// `data.start_active` and flipped via future activate hooks.
    pub active: bool,
    /// Currently-spawned playback child.  `None` when idle or
    /// between one-shot retriggers; `Some(child)` while audio is
    /// live so the driver knows whether to start a fresh cue.
    pub playing: Option<Entity>,
    /// Cursor for the `NextOne` / `CurrentOne(Looped)` play modes —
    /// indexes the package's nugget list.  Wraps on overflow.
    pub nugget_cursor: usize,
    /// Runtime overrides set by `sound <ch> pitch/volume` ops.
    /// `None` = use the per-nugget / `data.volume_scalar` defaults.
    pub volume_override: Option<f32>,
    pub pitch_override: Option<f32>,
    /// True when a `sound <ch> pause` has suspended playback but
    /// not torn down the child entity.  Translates to muting the
    /// sink until a `play` re-enables.  C++ tracks pause separately
    /// from active; we collapse them for now since the only way to
    /// resume in legacy is another `sound play` anyway.
    pub paused: bool,
}

impl ActorSound {
    pub fn new(data: SoundComponentData) -> Self {
        let active = data.start_active;
        Self {
            data,
            active,
            playing: None,
            nugget_cursor: 0,
            volume_override: None,
            pitch_override: None,
            paused: false,
        }
    }

    /// Apply a `sound <channel> <verb>` script op to this
    /// component.  Mirrors `audMsgPlaySound` dispatch: `PlayNamed`
    /// permanently swaps the package; `Play` (re-)starts the
    /// current package; `Stop` despawns any playback child;
    /// `Pause` mutes without tearing down; `Pitch`/`Volume`
    /// override the per-nugget multipliers; `FadeIn`/`FadeOut`
    /// are TODO placeholders.  `channel` is accepted but not yet
    /// multiplexed — first cut runs one stream per actor.
    pub fn apply_verb(
        &mut self,
        _channel: i32,
        verb: ActorSoundVerb,
        commands: &mut Commands,
    ) {
        match verb {
            ActorSoundVerb::Play => {
                self.paused = false;
                self.active = true;
                // Drop any in-flight playback so the driver re-cues
                // on its next tick, picking a fresh nugget per the
                // PlayMode.  Without this, looped modes would keep
                // their existing stream rather than restart.
                if let Some(child) = self.playing.take() {
                    commands.entity(child).despawn();
                }
            }
            ActorSoundVerb::PlayNamed(pkg) => {
                self.paused = false;
                self.active = true;
                self.data.audio_package = pkg;
                self.nugget_cursor = 0;
                if let Some(child) = self.playing.take() {
                    commands.entity(child).despawn();
                }
            }
            ActorSoundVerb::Pause => {
                self.paused = true;
            }
            ActorSoundVerb::Stop => {
                self.active = false;
                self.paused = false;
                if let Some(child) = self.playing.take() {
                    commands.entity(child).despawn();
                }
            }
            ActorSoundVerb::Pitch(v) => {
                self.pitch_override = Some(v.max(0.0));
            }
            ActorSoundVerb::Volume(v) => {
                self.volume_override = Some(v.clamp(0.0, 4.0));
            }
            ActorSoundVerb::FadeIn(_) | ActorSoundVerb::FadeOut(_) => {
                // Placeholder.  Legacy fade is a per-channel volume
                // ramp; would map onto AudioVolumeRamp once we
                // generalise it off the AmbientSound path.
            }
        }
    }
}

/// Child marker attached to the spawned AudioPlayer entity so the
/// driver can detect when playback ends (the child despawns when
/// PlaybackMode::Despawn finishes a one-shot) and re-cue the next
/// nugget for looping play modes that need new-nugget selection.
#[derive(Component, Debug, Clone)]
pub struct ActorSoundPlayback {
    /// The parent actor that owns the [`ActorSound`] driving this
    /// playback.  Used by the driver to clear `playing` when this
    /// child despawns.
    pub owner: Entity,
}

pub struct ActorSoundPlugin;

impl Plugin for ActorSoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, drive_actor_sound_system);
    }
}

/// Drives one-shot start, loop re-cue, distance-attenuation volume,
/// and pause-on-deactivate for every `ActorSound`.  Lazy-loads the
/// audio-package manifest and TD bank directory on first invocation
/// so unrelated tests / headless harnesses can boot without paying
/// the audio-asset cost.
#[allow(clippy::too_many_arguments)]
pub fn drive_actor_sound_system(
    mut commands: Commands,
    mut audio_sources: ResMut<Assets<bevy::audio::AudioSource>>,
    mut sounds: Query<(Entity, &mut ActorSound, &GlobalTransform)>,
    listener: Query<&GlobalTransform, With<crate::player::components::Player>>,
    mut sinks: Query<&mut bevy::audio::AudioSink>,
    mut td_directory: Local<Option<crate::oni2_loader::parsers::td::SoundBankDirectory>>,
    mut audio_packages: Local<
        Option<crate::oni2_loader::parsers::audiopackages::AudioPackagesDirectory>,
    >,
) {
    if td_directory.is_none() {
        *td_directory = Some(crate::oni2_loader::parsers::td::load_all_tds());
    }
    if audio_packages.is_none() {
        if let Ok(content) = crate::vfs::read_to_string("Audio", "rb.audiopackages") {
            *audio_packages = Some(
                crate::oni2_loader::parsers::audiopackages::parse_audiopackages(&content),
            );
        } else {
            let assets_path = crate::get_assets_path();
            let pkgs_path = std::path::Path::new(assets_path)
                .join("Audio")
                .join("rb.audiopackages");
            if let Ok(content) = std::fs::read_to_string(&pkgs_path) {
                *audio_packages = Some(
                    crate::oni2_loader::parsers::audiopackages::parse_audiopackages(&content),
                );
            } else {
                *audio_packages = Some(std::collections::HashMap::new());
            }
        }
    }

    let listener_pos = listener.iter().next().map(|tf| tf.translation());

    for (entity, mut sound, tf) in sounds.iter_mut() {
        // Drop the playback handle if the child entity is gone.  Bevy
        // despawns PlaybackMode::Despawn AudioPlayers when their
        // source ends, which is how we detect one-shot completion.
        if let Some(child) = sound.playing
            && commands.get_entity(child).is_err()
        {
            sound.playing = None;
        }

        // Distance attenuation: linear ramp between range_max_volume
        // (full volume) and range_zero_volume (silent).  Below the
        // max-volume radius we clamp to 1.0 of the authored scalar;
        // above zero-volume radius we mute.
        let attenuation = if let Some(lp) = listener_pos {
            let dist = lp.distance(tf.translation());
            let max_r = sound.data.range_max_volume.max(0.0);
            let zero_r = sound.data.range_zero_volume.max(max_r + 0.001);
            if dist <= max_r {
                1.0
            } else if dist >= zero_r {
                0.0
            } else {
                1.0 - (dist - max_r) / (zero_r - max_r)
            }
        } else {
            1.0
        };
        let base_volume = sound.volume_override.unwrap_or(sound.data.volume_scalar);
        // `Pause` mutes without tearing down so resuming via `Play`
        // doesn't restart from the top — matches legacy
        // audMsgPlaySound::kPause/kPlay round-trips.
        let target_volume = if sound.paused {
            0.0
        } else {
            base_volume * attenuation
        };

        // Drive the active sink's volume in real time so the listener
        // hears the attenuation update as the player walks toward or
        // away from the source.  Sinks live on the child playback
        // entity; nothing to do if it hasn't materialized yet.
        if let Some(child) = sound.playing
            && let Ok(mut sink) = sinks.get_mut(child)
        {
            sink.set_volume(bevy::audio::Volume::Linear(target_volume));
            if let Some(p) = sound.pitch_override {
                sink.set_speed(p);
            }
        }

        if !sound.active || sound.paused {
            continue;
        }

        // Looped play modes hold the AudioPlayer indefinitely via
        // PlaybackMode::Loop, so once `playing` is set we have
        // nothing more to do until something deactivates us.  Non-
        // looped modes re-fire from this branch every time the child
        // finishes (we cleared `playing` above when it despawned).
        if sound.playing.is_some() {
            continue;
        }

        // Empty package = dormant component waiting for a runtime
        // swap (kPlayNamed from a script or future approach/knockdown
        // trigger).  No warning — this is the common case for every
        // NPC that just inherits template_grunt's `<Sound>` block.
        if sound.data.audio_package.is_empty() {
            continue;
        }

        let Some(pkgs) = audio_packages.as_ref() else {
            continue;
        };
        let Some(dir) = td_directory.as_ref() else {
            continue;
        };

        let Some((_, pkg)) = pkgs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&sound.data.audio_package))
        else {
            // Once-per-actor warn so misspelled or missing packages
            // get flagged but don't spam every tick.
            warn!(
                "ActorSound: package `{}` not found in rb.audiopackages (actor {:?})",
                sound.data.audio_package, entity
            );
            sound.active = false;
            continue;
        };
        if pkg.nuggets.is_empty() {
            sound.active = false;
            continue;
        }

        let nugget_idx = match sound.data.play_mode {
            SoundPlayMode::CurrentOne | SoundPlayMode::CurrentOneLooped => {
                sound.nugget_cursor % pkg.nuggets.len()
            }
            SoundPlayMode::NextOne | SoundPlayMode::FullLoop => {
                let idx = sound.nugget_cursor % pkg.nuggets.len();
                sound.nugget_cursor = (sound.nugget_cursor + 1) % pkg.nuggets.len();
                idx
            }
            SoundPlayMode::RandomOne | SoundPlayMode::RandomLoop => {
                use rand::Rng;
                rand::rng().random_range(0..pkg.nuggets.len())
            }
        };

        let nugget = &pkg.nuggets[nugget_idx];
        let (package_volume, package_pitch) = {
            use rand::Rng;
            let mut rng = rand::rng();
            let low_v = nugget.random_min_volume.min(nugget.random_max_volume);
            let high_v = nugget.random_min_volume.max(nugget.random_max_volume);
            let low_p = nugget.random_min_pitch.min(nugget.random_max_pitch);
            let high_p = nugget.random_min_pitch.max(nugget.random_max_pitch);
            let vol = nugget.volume * rng.random_range(low_v..=high_v.max(low_v));
            let pitch = nugget.pitch * rng.random_range(low_p..=high_p.max(low_p));
            (vol, pitch)
        };

        let Some(source) = resolve_sound_handle(&nugget.sound, dir, &mut audio_sources) else {
            // Treat unresolved nuggets as fatal for this actor — the
            // package is misconfigured and retrying every tick won't
            // help.  Drop activity so we stop logging.
            warn!(
                "ActorSound: nugget `{}` from package `{}` failed to resolve (bank/HD/BD missing)",
                nugget.sound, sound.data.audio_package
            );
            sound.active = false;
            continue;
        };

        let mut settings = if sound.data.play_mode.is_looped() {
            bevy::audio::PlaybackSettings::LOOP
        } else {
            bevy::audio::PlaybackSettings::DESPAWN
        };
        let initial_speed = sound.pitch_override.unwrap_or(package_pitch);
        settings = settings
            .with_volume(bevy::audio::Volume::Linear(target_volume * package_volume))
            .with_speed(initial_speed);

        let child = commands
            .spawn((
                bevy::audio::AudioPlayer(source),
                settings,
                ActorSoundPlayback { owner: entity },
                crate::menu::InGameEntity,
            ))
            .id();
        sound.playing = Some(child);
    }
}

/// Audio-package nugget → playable AudioSource handle.  Walks the
/// TD bank directory to find which `.hd`/`.bd` pair carries the
/// nugget, parses the HD header, slices the BD payload, decodes the
/// PSX ADPCM, and registers the resulting WAV as a Bevy audio
/// source.  Returns `None` if any link in that chain is missing —
/// callers warn and deactivate the source.
fn resolve_sound_handle(
    name: &str,
    dir: &crate::oni2_loader::parsers::td::SoundBankDirectory,
    audio_sources: &mut Assets<bevy::audio::AudioSource>,
) -> Option<Handle<bevy::audio::AudioSource>> {
    let (_, v) = dir
        .sounds
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))?;
    let (bank_name, vag_index) = (&v.0, v.1);
    let hd_bytes = crate::vfs::read("", &format!("{}.hd", bank_name)).ok()?;
    let header = crate::oni2_loader::parsers::hd_bd::parse_hd(&hd_bytes).ok()?;
    let target_index = vag_index + 1;
    let subsong = header.subsongs.iter().find(|s| s.index == target_index)?;
    let bd_name = format!("{}.bd", bank_name);
    let bd_bytes = crate::vfs::read("", &bd_name)
        .ok()
        .or_else(|| crate::vfs::read("", &format!("Audio/banks/{}", bd_name)).ok())?;
    let start = subsong.stream_offset as usize;
    let end = start + subsong.stream_size as usize;
    if end > bd_bytes.len() {
        return None;
    }
    let pcm = crate::oni2_loader::parsers::hd_bd::decode_psx_adpcm(
        &bd_bytes[start..end],
        subsong.num_samples,
    )
    .ok()?;
    let wav = crate::oni2_loader::parsers::hd_bd::create_wav_bytes(
        &pcm,
        subsong.sample_rate,
        subsong.channels,
    )
    .ok()?;
    Some(audio_sources.add(bevy::audio::AudioSource {
        bytes: std::sync::Arc::from(wav),
    }))
}
