/*
 * rbaudio/resolve.rs — shared sound resolution for the manager layer.
 *
 * Centralizes the "name → playable AudioSource" pipeline (stream decode or
 * .td → .hd/.bd PSX-ADPCM decode) so music / cutscene / bank-preload don't
 * each re-implement it.  `AudioAssets` caches the `.td` directory and the
 * audio-package manifest, lazily loaded on first use (the VFS and asset
 * server aren't guaranteed ready at startup).
 */
use bevy::prelude::*;

use crate::oni2_loader::parsers::audiopackages::AudioPackagesDirectory;
use crate::oni2_loader::parsers::td::SoundBankDirectory;

/// Cached audio directories shared by every manager subsystem.
#[derive(Resource, Default)]
pub struct AudioAssets {
    pub td: Option<SoundBankDirectory>,
    pub packages: Option<AudioPackagesDirectory>,
}

impl AudioAssets {
    /// Populate `td` / `packages` if not already loaded.  Mirrors the
    /// lazy-init that the existing one-shot/ambient paths do with `Local`s,
    /// but shared in one resource.
    pub fn ensure_loaded(&mut self) {
        if self.td.is_none() {
            self.td = Some(crate::oni2_loader::parsers::td::load_all_tds());
        }
        if self.packages.is_none() {
            let parsed = if let Ok(content) = crate::vfs::read_to_string("Audio", "rb.audiopackages")
            {
                crate::oni2_loader::parsers::audiopackages::parse_audiopackages(&content)
            } else {
                let assets_path = crate::get_assets_path();
                let pkgs_path = std::path::Path::new(assets_path)
                    .join("Audio")
                    .join("rb.audiopackages");
                std::fs::read_to_string(&pkgs_path)
                    .ok()
                    .map(|c| crate::oni2_loader::parsers::audiopackages::parse_audiopackages(&c))
                    .unwrap_or_default()
            };
            self.packages = Some(parsed);
        }
    }
}

/// Resolve a sound/stream/package name to a playable `AudioSource` plus its
/// natural loop flag.  Accepts:
///   • `"Stream:NAME"`  → decode `NAME.stm` (always loops).
///   • a package name   → pick a random nugget, then resolve its sound.
///   • a bank sound name → direct `.td` → `.hd`/`.bd` decode.
pub fn resolve_sound(
    name: &str,
    assets: &AudioAssets,
    audio_sources: &mut Assets<bevy::audio::AudioSource>,
) -> Option<(Handle<bevy::audio::AudioSource>, bool)> {
    if let Some(stream) = name.strip_prefix("Stream:") {
        let stream_file = format!("{}.stm", stream);
        let bytes = crate::vfs::read("", &stream_file).ok()?;
        let decoded = crate::oni2_loader::parsers::stm::decode_stm(&bytes).ok()?;
        let wav = crate::oni2_loader::parsers::stm::create_wav_bytes(&decoded).ok()?;
        return Some((
            audio_sources.add(bevy::audio::AudioSource {
                bytes: std::sync::Arc::from(wav),
            }),
            true,
        ));
    }

    // Resolve a package name down to a concrete bank sound name.
    let mut resolved = name.to_string();
    if let Some(pkgs) = assets.packages.as_ref()
        && let Some((_, pkg)) = pkgs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name))
        && !pkg.nuggets.is_empty()
    {
        use rand::Rng;
        let idx = rand::rng().random_range(0..pkg.nuggets.len());
        resolved = pkg.nuggets[idx].sound.clone();
    }

    let dir = assets.td.as_ref()?;
    let (_, v) = dir
        .sounds
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&resolved))?;
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
    Some((
        audio_sources.add(bevy::audio::AudioSource {
            bytes: std::sync::Arc::from(wav),
        }),
        subsong.loop_flag,
    ))
}
