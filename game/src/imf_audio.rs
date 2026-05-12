/*
 * imf_audio.rs — decoder for Angel Engine `.imf` audio files.
 *
 * `.imf` is an engine-level container — not documented by any community
 * we could find, but file analysis points to a PS2 SPU-ADPCM payload
 * wrapped in an Angel Studios container.  The `IMF1` / `IMF2` magic, the
 * 2 KB fixed header, and the 18-byte (`count` at offset 0x04) per-stream
 * setup table are container metadata; the actual sample data starts at
 * `0x800` and is decoded here as standard Sony PS2 ADPCM (also known as
 * VAG ADPCM, the form fed straight into the PS2's SPU2 DMA).
 *
 * If this decode produces coherent (even if mis-rated) audio, we've
 * identified the codec and just need to figure out channel layout / per-
 * stream seeds.  If it produces static, the codec is something more
 * exotic and IMF needs proper reverse engineering against another title
 * that ships these files.
 */

use bevy::audio::AudioSource;
use std::sync::Arc;

/// PS2 SPU-ADPCM predictor coefficients (numerators, denominator = 64).
/// `predictor` index in `[0, 4]` selects a pair; index 5+ shows up
/// occasionally in malformed blocks — clamp to 0 (no prediction).
const ADPCM_POS_FILTER: [i32; 5] = [0, 60, 115, 98, 122];
const ADPCM_NEG_FILTER: [i32; 5] = [0, 0, -52, -55, -60];

/// Decode one 16-byte PS2 ADPCM block to 28 PCM samples.  The shift/
/// predictor live in byte 0 (predictor = high nibble, shift = low
/// nibble); byte 1 is the per-block flag byte (LOOP_END / LOOP_REPEAT /
/// LOOP_START — ignored here, we just walk straight through).
fn decode_ps2_adpcm_block(block: &[u8; 16], out: &mut Vec<i16>, prev1: &mut i32, prev2: &mut i32) {
    let mut predictor = ((block[0] >> 4) & 0x0F) as usize;
    let shift = (block[0] & 0x0F) as i32;
    if predictor > 4 {
        predictor = 0;
    }
    let pos = ADPCM_POS_FILTER[predictor];
    let neg = ADPCM_NEG_FILTER[predictor];

    for byte_idx in 0..14 {
        let byte = block[2 + byte_idx];
        // PS2 ADPCM packs the earlier sample in the low nibble.
        let nibbles = [byte & 0x0F, (byte >> 4) & 0x0F];
        for &n in &nibbles {
            // Sign-extend 4-bit value to i32.
            let signed = if (n & 0x08) != 0 {
                (n as i32) - 16
            } else {
                n as i32
            };
            // Scale up into the 16-bit range, then shift down by the
            // block's encoded shift factor.
            let mut sample = (signed << 12) >> shift;
            // Apply the linear predictor: sample += (prev1*pos + prev2*neg) / 64
            sample += (*prev1 * pos + *prev2 * neg) >> 6;
            let clamped = sample.clamp(-32768, 32767);
            *prev2 = *prev1;
            *prev1 = clamped;
            out.push(clamped as i16);
        }
    }
}

pub fn decode_imf_to_wav(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 0x800 {
        return None;
    }

    let magic = &data[0..4];
    if magic != b"IMF2" && magic != b"IMF1" {
        return None;
    }

    let sample_rate = u32::from_le_bytes(data[8..12].try_into().unwrap());
    let sr = if sample_rate > 8000 && sample_rate < 48000 {
        sample_rate
    } else {
        32768
    };

    // Walk the payload as 16-byte PS2 ADPCM blocks.  No per-block re-seed
    // for this first attempt — predictor state runs continuously across
    // blocks.  If audio comes out clean within a block but drifts across
    // them, the 18-byte setup table at offset 0x10 is the likely source
    // of per-block seed values.
    let payload = &data[0x800..];
    let block_count = payload.len() / 16;
    let mut samples: Vec<i16> = Vec::with_capacity(block_count * 28);
    let mut prev1: i32 = 0;
    let mut prev2: i32 = 0;
    for b in 0..block_count {
        let off = b * 16;
        let block: &[u8; 16] = payload[off..off + 16].try_into().ok()?;
        decode_ps2_adpcm_block(block, &mut samples, &mut prev1, &mut prev2);
    }

    let channels = 1u16;
    let data_size = (samples.len() * 2) as u32;
    let file_size = 36 + data_size;
    let byte_rate = sr * (channels as u32) * 2;
    let block_align = channels * 2;

    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sr.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for s in &samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }

    Some(wav)
}

pub fn create_audio_source(data: &[u8]) -> Option<AudioSource> {
    let wav_bytes = decode_imf_to_wav(data)?;
    Some(AudioSource {
        bytes: Arc::from(wav_bytes.into_boxed_slice()),
    })
}
