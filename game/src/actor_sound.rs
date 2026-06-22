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

#[derive(Debug, Clone, Default)]
pub struct AudioChannelState {
    /// Currently-spawned playback child.  `None` when idle or
    /// between one-shot retriggers; `Some(child)` while audio is
    /// live so the driver knows whether to start a fresh cue.
    pub playing: Option<Entity>,
    /// Cue currently waiting out its per-nugget `DELAY` before it
    /// spawns: `(nugget_index, rolled_volume, rolled_pitch)`.  Mirrors
    /// `rbAudioPackagePlayer`'s `MODE_DELAY` channel state — the nugget
    /// is selected and its random volume/pitch rolled at cue time, then
    /// playback is held off until `cue_delay` elapses.
    pub pending_cue: Option<(usize, f32, f32)>,
    /// Seconds remaining before the next cue may spawn.  Drives the
    /// per-nugget `DELAY` countdown.
    pub cue_delay: f32,
    /// Random per-nugget volume scalar rolled at cue time for the currently
    /// playing child, so the per-frame distance-attenuation update can fold
    /// it back into the child's `AudioGroup.base_volume` (otherwise volume
    /// would jump after the first frame).
    pub current_package_volume: f32,
}

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
    /// Concurrent playback channels, sized to `data.num_channels`.
    pub channels: Vec<AudioChannelState>,
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
    /// Index of the most recently selected nugget, so random play modes
    /// can avoid immediately repeating it (mirrors `CueNextSound`'s
    /// re-roll-while-equal loop for packages with >2 nuggets).
    pub last_nugget: Option<usize>,
    /// Backlog of play triggers/requests queued for processing.
    pub play_requests: u32,
    /// Speech cookie held while playing under `CookieMode` (mirrors
    /// `rbAudioSoundComponent::AudioCookie`).  `COOKIE_INVALID` when none
    /// held.  Limits how many cookie-mode sources play at once.
    pub audio_cookie: i32,
}

/// Back-off applied after a failed package `PROBABILITY` roll so the
/// per-tick re-cue loop doesn't spin the gate every frame.  The C++
/// gate is evaluated once per explicit `Play()` trigger; our driver
/// re-cues idle one-shots each tick, so this throttles the retry rate.
const PROBABILITY_RETRY_BACKOFF: f32 = 0.5;

impl ActorSound {
    pub fn new(data: SoundComponentData) -> Self {
        let active = data.start_active;
        let mut channels = Vec::new();
        channels.resize(data.num_channels.max(1) as usize, AudioChannelState::default());
        let play_requests = if active { 1 } else { 0 };
        Self {
            data,
            active,
            channels,
            nugget_cursor: 0,
            volume_override: None,
            pitch_override: None,
            paused: false,
            last_nugget: None,
            play_requests,
            audio_cookie: crate::rbaudio::COOKIE_INVALID,
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
                self.play_requests += 1;
            }
            ActorSoundVerb::PlayNamed(pkg) => {
                self.paused = false;
                self.active = true;
                self.data.audio_package = pkg;
                self.nugget_cursor = 0;
                self.last_nugget = None;
                self.play_requests += 1;
            }
            ActorSoundVerb::Pause => {
                self.paused = true;
            }
            ActorSoundVerb::Stop => {
                self.active = false;
                self.paused = false;
                self.play_requests = 0;
                for channel in &mut self.channels {
                    channel.pending_cue = None;
                    channel.cue_delay = 0.0;
                    if let Some(child) = channel.playing.take() {
                        commands.entity(child).despawn();
                    }
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
            ActorSoundVerb::PlayPlayerApproach => {
                // kPlayPlayerApproach: swap to the approach package and play,
                // but only if one is configured (matches the C++ `if
                // (GetType().PlayerApproachPackage)` guard).
                if let Some(pkg) = self.data.player_approach_package.clone() {
                    self.paused = false;
                    self.active = true;
                    self.data.audio_package = pkg;
                    self.nugget_cursor = 0;
                    self.last_nugget = None;
                    self.play_requests += 1;
                }
            }
            ActorSoundVerb::PlayPlayerKnockdown => {
                if let Some(pkg) = self.data.player_knockdown_package.clone() {
                    self.paused = false;
                    self.active = true;
                    self.data.audio_package = pkg;
                    self.nugget_cursor = 0;
                    self.last_nugget = None;
                    self.play_requests += 1;
                }
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

/// Engine-side trigger for the `<Sound>` component's special play verbs
/// (player-approach / player-knockdown taunts).  Gameplay code writes this to
/// fire the corresponding package on `entity` without going through ScrOni —
/// mirroring how the C++ engine `Broadcast`s an `audMsgPlaySound` to the
/// relevant actor.
#[derive(Message)]
pub struct ActorSoundTrigger {
    pub entity: Entity,
    pub verb: ActorSoundVerb,
}

pub struct ActorSoundPlugin;

impl Plugin for ActorSoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ActorSoundTrigger>().add_systems(
            Update,
            (
                player_approach_sound_system,
                apply_actor_sound_triggers,
                drive_actor_sound_system,
            )
                .chain(),
        );
    }
}

/// Fires the `PlayerApproach` taunt on the rising edge of an AI fighter
/// acquiring the player as its target — the port's stand-in for
/// `aiFightAIComponent::StartFight`, which broadcasts `kPlayPlayerApproach`
/// to the fighter when it engages.
pub fn player_approach_sound_system(
    player: Query<Entity, With<crate::player::components::Player>>,
    fighters: Query<(Entity, &crate::ai::components::AiFighter)>,
    mut trigger: MessageWriter<ActorSoundTrigger>,
    mut was_engaging: Local<bevy::ecs::entity::EntityHashMap<bool>>,
) {
    let Some(player) = player.iter().next() else {
        return;
    };
    for (entity, af) in &fighters {
        let engaging = af.target == Some(player);
        let prev = was_engaging.get(&entity).copied().unwrap_or(false);
        if engaging && !prev {
            trigger.write(ActorSoundTrigger {
                entity,
                verb: ActorSoundVerb::PlayPlayerApproach,
            });
        }
        was_engaging.insert(entity, engaging);
    }
}

/// Applies queued [`ActorSoundTrigger`]s by invoking `apply_verb` on the
/// target actor's [`ActorSound`].  Actors without a Sound component silently
/// no-op, matching the C++ broadcast semantics.
pub fn apply_actor_sound_triggers(
    mut commands: Commands,
    mut reader: MessageReader<ActorSoundTrigger>,
    mut sounds: Query<&mut ActorSound>,
) {
    for msg in reader.read() {
        if let Ok(mut sound) = sounds.get_mut(msg.entity) {
            sound.apply_verb(0, msg.verb.clone(), &mut commands);
        }
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
    time: Res<Time>,
    mut audio_sources: ResMut<Assets<bevy::audio::AudioSource>>,
    mut sounds: Query<(Entity, &mut ActorSound, &GlobalTransform)>,
    listener: Query<&GlobalTransform, With<crate::player::components::Player>>,
    mut sinks: Query<&mut bevy::audio::AudioSink>,
    mut groups: Query<&mut crate::rbaudio::AudioGroup>,
    mut rb_audio: ResMut<crate::rbaudio::RbAudioManager>,
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
        let mut finished_channels = Vec::new();
        for (c_idx, channel) in sound.channels.iter_mut().enumerate() {
            if let Some(child) = channel.playing {
                if commands.get_entity(child).is_err() {
                    channel.playing = None;
                    finished_channels.push(c_idx);
                }
            }
        }

        // Cookie release: once all channels of a cookie-mode source have stopped playing,
        // hand its speech cookie back (mirrors the CookieMode branch of
        // `rbAudioSoundComponent::HandleUpdate`).
        let any_busy = sound
            .channels
            .iter()
            .any(|c| c.playing.is_some() || c.pending_cue.is_some());
        if sound.data.cookie_mode
            && !any_busy
            && sound.audio_cookie != crate::rbaudio::COOKIE_INVALID
        {
            rb_audio.release_cookie(sound.audio_cookie);
            sound.audio_cookie = crate::rbaudio::COOKIE_INVALID;
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

        // Drive the active source's volume in real time so the listener
        // hears the attenuation update as the player walks toward or away
        // from the source.  Volume is written to the child's `AudioGroup`
        // base (the master `apply_audio_groups_system` multiplies in the
        // group gain); pitch still goes straight to the sink.
        for channel in &sound.channels {
            if let Some(child) = channel.playing {
                if let Ok(mut group) = groups.get_mut(child) {
                    group.base_volume = target_volume * channel.current_package_volume;
                }
                if let Some(p) = sound.pitch_override
                    && let Ok(mut sink) = sinks.get_mut(child)
                {
                    sink.set_speed(p);
                }
            }
        }

        if !sound.active || sound.paused {
            continue;
        }

        // If the play mode is looped and one of the nuggets has finished playing,
        // we queue a play request to start the next nugget.
        if sound.data.play_mode.is_looped() {
            sound.play_requests += finished_channels.len() as u32;
        }

        // Empty package = dormant component waiting for a runtime
        // swap (kPlayNamed from a script or future approach/knockdown
        // trigger).  No warning — this is the common case for every
        // NPC that just inherits template_grunt's `<Sound>` block.
        if sound.data.audio_package.is_empty() {
            sound.play_requests = 0;
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
            sound.play_requests = 0;
            continue;
        };
        if pkg.nuggets.is_empty() {
            sound.active = false;
            sound.play_requests = 0;
            continue;
        }

        // 1. Process pending/delayed cues.
        let dt = time.delta_secs();
        let pitch_override = sound.pitch_override;
        for channel in sound.channels.iter_mut() {
            if let Some(cue) = channel.pending_cue {
                if channel.cue_delay > 0.0 {
                    channel.cue_delay -= dt;
                } else {
                    channel.pending_cue = None;
                    let (nugget_idx, vol, pitch) = cue;
                    let nugget = &pkg.nuggets[nugget_idx];
                    if let Some(source) = resolve_sound_handle(&nugget.sound, dir, &mut audio_sources) {
                        let settings = bevy::audio::PlaybackSettings::DESPAWN
                            .with_volume(bevy::audio::Volume::Linear(target_volume * vol))
                            .with_speed(pitch_override.unwrap_or(pitch));

                        channel.current_package_volume = vol;
                        let child = commands
                            .spawn((
                                bevy::audio::AudioPlayer(source),
                                settings,
                                crate::rbaudio::AudioGroup::new(
                                    crate::rbaudio::AudioGroupId::Player,
                                    target_volume * vol,
                                ),
                                ActorSoundPlayback { owner: entity },
                                crate::menu::InGameEntity,
                            ))
                            .id();
                        channel.playing = Some(child);
                    } else {
                        warn!(
                            "ActorSound: nugget `{}` failed to resolve during delayed spawn (actor {:?})",
                            nugget.sound, entity
                        );
                    }
                }
            }
        }

        // 2. Process fresh play requests.
        while sound.play_requests > 0 {
            // Find an idle channel (neither playing nor pending a delay).
            let free_channel_idx = sound
                .channels
                .iter()
                .position(|c| c.playing.is_none() && c.pending_cue.is_none());

            let Some(c_idx) = free_channel_idx else {
                // No free channels available — stop processing play requests for this frame.
                break;
            };

            sound.play_requests -= 1;

            // Cookie gate (CookieMode): acquire a speech cookie before a
            // cookie-mode source may start.  If none is free, skip this tick —
            // mirrors `rbAudioSoundComponent::Play`'s early-out when
            // `RequestCookie` returns `COOKIE_INVALID`.  Held until playback
            // ends (released above).
            if sound.data.cookie_mode && sound.audio_cookie == crate::rbaudio::COOKIE_INVALID {
                let cookie = rb_audio.request_cookie();
                if cookie == crate::rbaudio::COOKIE_INVALID {
                    continue;
                }
                sound.audio_cookie = cookie;
            }

            // PROBABILITY gate (rbAudioPackagePlayer::Play): the package
            // only plays probability*100% of the time.  On a failed roll,
            // skip this play request.
            if pkg.probability < 1.0 {
                use rand::Rng;
                if rand::rng().random::<f32>() > pkg.probability {
                    continue;
                }
            }

            let idx = match sound.data.play_mode {
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
                    let mut rng = rand::rng();
                    let n = pkg.nuggets.len();
                    let mut idx = rng.random_range(0..n);
                    // CueNextSound: avoid immediately repeating the previous
                    // nugget when there are more than 2 to pick from.
                    if n > 2 {
                        while Some(idx) == sound.last_nugget {
                            idx = rng.random_range(0..n);
                        }
                    }
                    idx
                }
            };

            let nugget = &pkg.nuggets[idx];
            let is_random = matches!(
                sound.data.play_mode,
                SoundPlayMode::RandomOne | SoundPlayMode::RandomLoop
            );
            let (vol, pitch, delay) = {
                use rand::Rng;
                let mut rng = rand::rng();
                let low_v = nugget.random_min_volume.min(nugget.random_max_volume);
                let high_v = nugget.random_min_volume.max(nugget.random_max_volume);
                let low_p = nugget.random_min_pitch.min(nugget.random_max_pitch);
                let high_p = nugget.random_min_pitch.max(nugget.random_max_pitch);
                let low_d = nugget.random_min_delay.min(nugget.random_max_delay);
                let high_d = nugget.random_min_delay.max(nugget.random_max_delay);

                let vol = if is_random {
                    rng.random_range(low_v..=high_v.max(low_v))
                } else {
                    nugget.volume
                };
                let pitch = if is_random {
                    rng.random_range(low_p..=high_p.max(low_p))
                } else {
                    nugget.pitch
                };
                // Per-nugget delay: use the random range when authored,
                // otherwise the fixed DELAY field.
                let delay = if is_random && (high_d - low_d).abs() > f32::EPSILON {
                    rng.random_range(low_d..=high_d.max(low_d))
                } else {
                    nugget.delay
                };
                (vol, pitch, delay)
            };

            sound.last_nugget = Some(idx);

            // Hold the cue for `delay` seconds before it actually spawns.
            if delay > 0.0 {
                sound.channels[c_idx].pending_cue = Some((idx, vol, pitch));
                sound.channels[c_idx].cue_delay = delay;
                continue;
            }

            if let Some(source) = resolve_sound_handle(&nugget.sound, dir, &mut audio_sources) {
                let settings = bevy::audio::PlaybackSettings::DESPAWN
                    .with_volume(bevy::audio::Volume::Linear(target_volume * vol))
                    .with_speed(sound.pitch_override.unwrap_or(pitch));

                sound.channels[c_idx].current_package_volume = vol;

                let child = commands
                    .spawn((
                        bevy::audio::AudioPlayer(source),
                        settings,
                        crate::rbaudio::AudioGroup::new(
                            crate::rbaudio::AudioGroupId::Player,
                            target_volume * vol,
                        ),
                        ActorSoundPlayback { owner: entity },
                        crate::menu::InGameEntity,
                    ))
                    .id();
                sound.channels[c_idx].playing = Some(child);
            } else {
                warn!(
                    "ActorSound: nugget `{}` from package `{}` failed to resolve (bank/HD/BD missing)",
                    nugget.sound, sound.data.audio_package
                );
            }
        }
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
