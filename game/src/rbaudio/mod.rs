/*
 * rbaudio/mod.rs — port of the `rbAudioManager` master audio layer.
 *
 * The data layer (audio packages / nuggets / per-actor playback) lives in
 * `actor_sound.rs` + `parsers/audiopackages.rs`; this module ports the
 * *manager* responsibilities that were previously absent:
 *
 *   • Volume groups (Player / Opponent / Ambient / Music) — a master mixing
 *     bus.  Every spawned audio entity carries an `AudioGroup { group,
 *     base_volume }`; `apply_audio_groups_system` writes the final sink
 *     volume = base_volume * group_gain each frame, so dynamic group changes
 *     (pause ducking, cutscene ducking) affect every live sound.
 *   • Global pause ducking (PauseOn/PauseOff).
 *   • Cookie speech limiter (RequestCookie/ReleaseCookie).
 *   • Music crossfade (MusicPlay/MusicStop, two blended channels).
 *   • Cutscene audio (PlayCutScene/EndCutScene) with ambient ducking.
 *   • Hard channel caps (kMaxAmbients / kMaxMusicActive) and the startup bank
 *     preload list.
 *
 * Bevy has no native group bus, so we model it explicitly with the
 * `AudioGroup` component + a per-frame apply system.
 */
use bevy::prelude::*;

pub mod resolve;

// ---------------------------------------------------------------------------
// Tuning constants (mirror rbAudioManager)
// ---------------------------------------------------------------------------

/// Max simultaneous ambient channels (`kMaxAmbients`).
pub const MAX_AMBIENTS: usize = 12;
/// Max simultaneous (blending) music channels (`kMaxMusicActive`).
pub const MAX_MUSIC_ACTIVE: usize = 2;
/// Speech cookies available at once (`COOKIE_COUNT`).
pub const COOKIE_COUNT: usize = 1;
/// Sentinel for "no cookie" (`COOKIE_INVALID`).
pub const COOKIE_INVALID: i32 = -1;
/// Cookie hold time after release — workaround for stream IsPlaying latency
/// (`CookieReleaseTimer = 1.0f`).
pub const COOKIE_RELEASE_TIME: f32 = 1.0;
/// Default ambient group volume set at construction
/// (`AmbientSetVolumeGroup(0.5f)`).
pub const AMBIENT_GROUP_DEFAULT: f32 = 0.5;
/// All groups ducked to this while paused (`PauseOn` → `SetVolume(0.05)`).
pub const PAUSE_DUCK_VOLUME: f32 = 0.05;
/// Music blend volume floor (`Max(ratio * blendVolume, 0.02f)` / `Clamp(...,0.02,1.0)`).
pub const MUSIC_BLEND_FLOOR: f32 = 0.02;

/// Banks the original manager preloads at construction.  We resolve banks
/// on demand from the `.td` directory, so "preloading" warms a decode cache
/// rather than uploading to SPU RAM; the list is kept verbatim for parity.
pub const PRELOAD_BANKS: &[&str] = &[
    "SFX_Fight_Foley",
    "SFX_Foot_Foley",
    "SFX_Blast_Chamber",
    "Konoko_Gun",
    "SFX_Konoko_Grunts",
    "SFX_Grunts_Male_Average",
    "wpn1",
    "Streams",
];

// ---------------------------------------------------------------------------
// Volume groups
// ---------------------------------------------------------------------------

/// The four `sndControlVolumeGroup`s the manager creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioGroupId {
    Player = 0,
    Opponent = 1,
    Ambient = 2,
    Music = 3,
}

impl AudioGroupId {
    pub const COUNT: usize = 4;
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Attached to every audio entity routed through the manager.  The apply
/// system multiplies `base_volume` (the sound's own intended linear volume —
/// already including distance attenuation / ramp / per-nugget scalars) by
/// the owning group's gain to produce the sink volume.
#[derive(Component, Debug, Clone, Copy)]
pub struct AudioGroup {
    pub group: AudioGroupId,
    pub base_volume: f32,
}

impl AudioGroup {
    pub fn new(group: AudioGroupId, base_volume: f32) -> Self {
        Self { group, base_volume }
    }
}

// ---------------------------------------------------------------------------
// Manager resource
// ---------------------------------------------------------------------------

/// One blending music channel.
#[derive(Default)]
struct MusicChannel {
    entity: Option<Entity>,
    /// Target (post-blend) linear volume for this channel.
    volume: f32,
}

struct CutSceneState {
    entity: Entity,
    /// Ambient group gain to restore when the cutscene ends.
    saved_ambient_gain: f32,
}

#[derive(Resource)]
pub struct RbAudioManager {
    /// Per-group master gain, indexed by `AudioGroupId::index()`.
    group_gain: [f32; AudioGroupId::COUNT],
    paused: bool,

    // Cookie limiter ----------------------------------------------------
    cookies_taken: [bool; COOKIE_COUNT],
    cookie_release_timer: [f32; COOKIE_COUNT],

    // Music crossfade ---------------------------------------------------
    music: [MusicChannel; MAX_MUSIC_ACTIVE],
    music_index: usize,
    music_blend_mode: bool,
    music_blend_volume: f32,
    music_blend_volume_old: f32,
    music_blend_time: f32,
    music_blend_elapsed: f32,
    music_blend_index_to_fade_out: usize,
    /// Pending music cue: (resolved source, target volume, blend time).
    /// Set by `MusicPlay`, consumed by the blend tick which owns spawning.
    music_pending: Option<(Handle<bevy::audio::AudioSource>, f32, f32)>,
    music_stop_requested: bool,

    // Cutscene ----------------------------------------------------------
    cutscene: Option<CutSceneState>,
    /// Pending cutscene cue: (source, optional cs-volume, bg-volume) set by
    /// `play_cutscene`, consumed by `cutscene_watch_system` which spawns it.
    cutscene_pending: Option<(Handle<bevy::audio::AudioSource>, Option<f32>, f32)>,
    cutscene_end_requested: bool,
}

impl Default for RbAudioManager {
    fn default() -> Self {
        let mut group_gain = [1.0; AudioGroupId::COUNT];
        group_gain[AudioGroupId::Ambient.index()] = AMBIENT_GROUP_DEFAULT;
        Self {
            group_gain,
            paused: false,
            cookies_taken: [false; COOKIE_COUNT],
            cookie_release_timer: [0.0; COOKIE_COUNT],
            music: Default::default(),
            music_index: 0,
            music_blend_mode: false,
            music_blend_volume: 1.0,
            music_blend_volume_old: 0.0,
            music_blend_time: 0.0,
            music_blend_elapsed: 0.0,
            music_blend_index_to_fade_out: 0,
            music_pending: None,
            music_stop_requested: false,
            cutscene: None,
            cutscene_pending: None,
            cutscene_end_requested: false,
        }
    }
}

impl RbAudioManager {
    #[inline]
    pub fn group_gain(&self, group: AudioGroupId) -> f32 {
        self.group_gain[group.index()]
    }

    /// Set one group's master volume (`sndControlVolumeGroup::SetVolume`).
    pub fn set_group_volume(&mut self, group: AudioGroupId, volume: f32) {
        self.group_gain[group.index()] = volume.max(0.0);
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// `rbAudioManager::PauseOn` — duck every group to 0.05.
    pub fn pause_on(&mut self) {
        if self.paused {
            return;
        }
        self.paused = true;
        for g in self.group_gain.iter_mut() {
            *g = PAUSE_DUCK_VOLUME;
        }
    }

    /// `rbAudioManager::PauseOff` — restore every group to 1.0.  (Like the
    /// original, this clobbers the ambient 0.5 default — faithful quirk.)
    pub fn pause_off(&mut self) {
        if !self.paused {
            return;
        }
        self.paused = false;
        for g in self.group_gain.iter_mut() {
            *g = 1.0;
        }
    }

    // --- Cookie limiter ------------------------------------------------

    /// `rbAudioManager::RequestCookie` — grant a free speech cookie or
    /// return `COOKIE_INVALID` when all are taken / cooling down.
    pub fn request_cookie(&mut self) -> i32 {
        for a in 0..COOKIE_COUNT {
            if self.cookie_release_timer[a] <= 0.0 && !self.cookies_taken[a] {
                self.cookies_taken[a] = true;
                return a as i32;
            }
        }
        COOKIE_INVALID
    }

    /// `rbAudioManager::ReleaseCookie` — free a cookie and start its
    /// 1.0s cool-down.
    pub fn release_cookie(&mut self, cookie: i32) {
        if cookie < 0 || cookie as usize >= COOKIE_COUNT {
            return;
        }
        let a = cookie as usize;
        self.cookies_taken[a] = false;
        self.cookie_release_timer[a] = COOKIE_RELEASE_TIME;
    }

    pub fn release_all_cookies(&mut self) {
        for a in 0..COOKIE_COUNT {
            self.cookies_taken[a] = false;
            self.cookie_release_timer[a] = 0.0;
        }
    }

    // --- Music ---------------------------------------------------------

    /// `rbAudioManager::MusicPlay` — queue a crossfade to `source` at
    /// `volume`, blending over `blend_time` seconds.
    pub fn music_play(
        &mut self,
        source: Handle<bevy::audio::AudioSource>,
        volume: f32,
        blend_time: f32,
    ) {
        self.music_pending = Some((source, volume.clamp(MUSIC_BLEND_FLOOR, 1.0), blend_time));
    }

    /// `rbAudioManager::MusicStop`.
    pub fn music_stop(&mut self) {
        self.music_stop_requested = true;
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// The master mixing pass: `sink.volume = base_volume * group_gain`.  Runs in
/// `PostUpdate` so it observes the `base_volume` writes done by per-actor
/// attenuation (`actor_sound`) and ambient ramps (`audio_ramp_system`) in
/// `Update`.  This is what makes pause / cutscene ducking reach *every* live
/// sound, including steady one-shots nothing else updates per frame.
pub fn apply_audio_groups_system(
    manager: Res<RbAudioManager>,
    mut sinks: Query<(&AudioGroup, &mut bevy::audio::AudioSink)>,
) {
    for (group, mut sink) in &mut sinks {
        let gain = manager.group_gain(group.group);
        sink.set_volume(bevy::audio::Volume::Linear(group.base_volume * gain));
    }
}

/// Mirrors `rbAudioManager::PauseOn/PauseOff`: ducks every volume group while
/// game time is paused and restores them when it resumes.  Hooked to
/// `Time<Virtual>` (paused or frozen at zero speed — the debug frame-stepper
/// and any future pause menu both flow through it).  `pause_on/pause_off` are
/// idempotent, so calling every frame is fine.
pub fn pause_duck_system(virtual_time: Res<Time<Virtual>>, mut manager: ResMut<RbAudioManager>) {
    let paused = virtual_time.is_paused() || virtual_time.relative_speed() <= f32::EPSILON;
    if paused {
        manager.pause_on();
    } else {
        manager.pause_off();
    }
}

/// Decrements cookie cool-down timers every frame (`rbAudioManager::Update`
/// top-of-loop, unconditional even while paused).
pub fn cookie_timer_system(time: Res<Time<Real>>, mut manager: ResMut<RbAudioManager>) {
    let dt = time.delta_secs();
    for c in 0..COOKIE_COUNT {
        if manager.cookie_release_timer[c] > 0.0 {
            manager.cookie_release_timer[c] -= dt;
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct RbAudioPlugin;

impl Plugin for RbAudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RbAudioManager>()
            .init_resource::<resolve::AudioAssets>()
            .add_systems(Startup, preload_banks_system)
            .add_systems(
                Update,
                (
                    cookie_timer_system,
                    pause_duck_system,
                    crate::scroni::vm::audio_ramp_system,
                ),
            )
            .add_systems(
                Update,
                (music::music_blend_system, cutscene::cutscene_watch_system),
            )
            .add_systems(PostUpdate, apply_audio_groups_system)
            .add_observer(scroni_bridge::scroni_audio_observer);
    }
}

/// Warms the banks the original manager `LoadBank`s at construction.  Our
/// playback decodes on demand from the `.td` directory rather than uploading
/// to SPU RAM, so "preloading" here means making sure the bank's `.hd`/`.bd`
/// payloads are resolvable (and read once to warm the VFS/OS cache).  The
/// list is kept verbatim from `rbAudioManager::rbAudioManager` for parity.
pub fn preload_banks_system(mut assets: ResMut<resolve::AudioAssets>) {
    assets.ensure_loaded();
    let mut warmed = 0;
    for bank in PRELOAD_BANKS {
        // Streams ship as `.stm` files rather than `.hd`/`.bd` banks.
        let candidates = [format!("{}.hd", bank), format!("{}.bd", bank)];
        if candidates.iter().any(|p| crate::vfs::read("", p).is_ok()) {
            warmed += 1;
        }
    }
    bevy::log::info!(
        "rbaudio: preload warmed {}/{} banks",
        warmed,
        PRELOAD_BANKS.len()
    );
}

pub mod cutscene;
pub mod music;
pub mod scroni_bridge;
