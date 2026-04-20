/*
 * oni2_loader/parsers/stm.rs — STM streaming audio asset loader.
 *
 * StmAssetLoader: Bevy AssetLoader for the ONI2 .stm streaming audio format.
 * Decodes the container and produces an AudioSource for Bevy's audio system.
 * Registered as a Bevy asset loader so .stm files can be loaded via AssetServer.
 */
use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use bevy::audio::AudioSource;
use bevy::prelude::*;
use std::io::Cursor;
use std::sync::Arc;

#[derive(Clone, Default, bevy::prelude::Reflect)]
#[reflect(opaque)]
pub struct StmAssetLoader;

impl AssetLoader for StmAssetLoader {
    type Asset = AudioSource;
    type Settings = ();
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let decoded = decode_stm(&bytes)?;
        let wav_bytes = create_wav_bytes(&decoded)?;

        Ok(AudioSource {
            bytes: Arc::from(wav_bytes),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["stm"]
    }
}

const START_OFFSET: usize = 0x800;
const IMA_DEFAULT_BLOCK: usize = 0x40;
const IMA_ALT_BLOCK: usize = 0x80;

#[derive(Debug)]
pub struct StmDecoded {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>, // interleaved
    pub loop_start: Option<u32>,
    pub loop_end: Option<u32>,
}

pub fn decode_stm(
    data: &[u8],
) -> Result<StmDecoded, Box<dyn std::error::Error + Send + Sync + 'static>> {
    if data.len() < START_OFFSET {
        return Err("File too small to be STM".into());
    }

    let magic = &data[0..4];
    let big_endian = match magic {
        b"STMA" => false,
        b"AMTS" => true,
        _ => return Err("Not an STM/STMA file".into()),
    };

    let read_u32 =
        |offset: usize| -> Result<u32, Box<dyn std::error::Error + Send + Sync + 'static>> {
            if offset + 4 > data.len() {
                return Err("Unexpected EOF while reading header".into());
            }
            Ok(if big_endian {
                u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap())
            } else {
                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
            })
        };

    let interleave = read_u32(0x08)? as usize;
    let sample_rate = read_u32(0x0c)?;
    let bps = read_u32(0x10)?;
    let channels = read_u32(0x14)? as u16;
    let data_size = read_u32(0x18)? as usize;
    let loop_end_offset = read_u32(0x1c)? as usize;

    if START_OFFSET + data_size > data.len() {
        return Err("STM reports audio past end of file".into());
    }

    let audio = &data[START_OFFSET..START_OFFSET + data_size];

    let mut loop_start_samples = None;
    let mut loop_end_samples = None;

    let samples = match (big_endian, bps) {
        (false, 4) => {
            let block = if interleave == 0xc000 {
                IMA_ALT_BLOCK
            } else {
                IMA_DEFAULT_BLOCK
            };
            if read_u32(0x20)? == 1 {
                loop_start_samples = Some(read_u32(0x24)?);
            }
            loop_end_samples = Some(ima_loop_sample(
                loop_end_offset,
                channels,
                START_OFFSET,
                block,
            ));
            decode_ima(audio, channels as usize, block)?
        }
        (false, 16) => {
            loop_start_samples = Some(read_u32(0x24)?);
            loop_end_samples = Some(pcm_loop_sample(
                loop_end_offset,
                channels,
                START_OFFSET,
                bps as usize,
            ));
            decode_pcm(audio, channels as usize, false)?
        }
        (true, 16) => {
            loop_start_samples = Some(pcm_loop_sample(
                loop_end_offset,
                channels,
                START_OFFSET,
                bps as usize,
            ));
            decode_pcm(audio, channels as usize, true)?
        }
        (true, 4) => {
            return Err("GC DSP STMA files are not supported yet".into());
        }
        _ => {
            return Err(
                format!("Unsupported STM encoding (bps={bps}, big_endian={big_endian})").into(),
            );
        }
    };

    Ok(StmDecoded {
        sample_rate,
        channels,
        samples,
        loop_start: loop_start_samples,
        loop_end: loop_end_samples,
    })
}

fn decode_pcm(
    data: &[u8],
    channels: usize,
    big_endian: bool,
) -> Result<Vec<i16>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    if data.len() % 2 != 0 {
        return Err("PCM data not aligned".into());
    }
    let mut samples = Vec::with_capacity(data.len() / 2);
    for chunk in data.chunks_exact(2) {
        let sample = if big_endian {
            i16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            i16::from_le_bytes([chunk[0], chunk[1]])
        };
        samples.push(sample);
    }
    if samples.len() % channels != 0 {
        return Err("PCM sample count not divisible by channel count".into());
    }
    Ok(samples)
}

fn decode_ima(
    data: &[u8],
    channels: usize,
    block_size: usize,
) -> Result<Vec<i16>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    if block_size < 4 {
        return Err("IMA block size too small".into());
    }
    let frame_size = block_size * channels;
    if data.len() % frame_size != 0 {
        return Err("IMA data doesn't align to channel blocks".into());
    }

    let mut per_channel: Vec<Vec<i16>> = vec![Vec::new(); channels];
    for frame in data.chunks_exact(frame_size) {
        for ch in 0..channels {
            let block = &frame[ch * block_size..(ch + 1) * block_size];
            let decoded = decode_ima_block(block)?;
            per_channel[ch].extend(decoded);
        }
    }

    let expected = per_channel[0].len();
    for (idx, channel) in per_channel.iter().enumerate() {
        if channel.len() != expected {
            return Err(format!("Channel {idx} length mismatch").into());
        }
    }

    let mut interleaved = Vec::with_capacity(expected * channels);
    for i in 0..expected {
        for channel in &per_channel {
            interleaved.push(channel[i]);
        }
    }
    Ok(interleaved)
}

fn decode_ima_block(
    block: &[u8],
) -> Result<Vec<i16>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    if block.len() < 4 {
        return Err("IMA block shorter than header".into());
    }

    let mut state = ImaState {
        predictor: i16::from_le_bytes([block[0], block[1]]) as i32,
        step_index: block[2].min(88) as i32,
    };
    // block[3] reserved

    let mut samples = Vec::with_capacity((block.len() - 4) * 2 + 1);
    samples.push(state.predictor as i16);

    for &byte in &block[4..] {
        let low = decode_ima_nibble(byte & 0x0F, &mut state);
        samples.push(low);
        let high = decode_ima_nibble(byte >> 4, &mut state);
        samples.push(high);
    }

    Ok(samples)
}

#[derive(Clone, Copy)]
struct ImaState {
    predictor: i32,
    step_index: i32,
}

fn decode_ima_nibble(nibble: u8, state: &mut ImaState) -> i16 {
    const STEP_TABLE: [i32; 89] = [
        7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60,
        66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371,
        408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878,
        2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845,
        8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086,
        29794, 32767,
    ];
    const INDEX_TABLE: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

    let step = STEP_TABLE[state.step_index as usize];
    let mut diff = step >> 3;
    if nibble & 0x01 != 0 {
        diff += step >> 2;
    }
    if nibble & 0x02 != 0 {
        diff += step >> 1;
    }
    if nibble & 0x04 != 0 {
        diff += step;
    }

    if nibble & 0x08 != 0 {
        state.predictor -= diff;
    } else {
        state.predictor += diff;
    }
    state.predictor = state.predictor.clamp(i16::MIN as i32, i16::MAX as i32);

    state.step_index += INDEX_TABLE[nibble as usize];
    state.step_index = state.step_index.clamp(0, 88);

    state.predictor as i16
}

fn ima_loop_sample(
    loop_end_offset: usize,
    channels: u16,
    start_offset: usize,
    block_size: usize,
) -> u32 {
    if loop_end_offset <= start_offset {
        return 0;
    }
    let bytes = loop_end_offset - start_offset;
    let per_channel = bytes / channels as usize;
    if block_size <= 4 {
        return 0;
    }
    let samples_per_block = (block_size - 4) * 2 + 1;
    ((per_channel / block_size) * samples_per_block) as u32
}

fn pcm_loop_sample(loop_end_offset: usize, channels: u16, start_offset: usize, bps: usize) -> u32 {
    if loop_end_offset <= start_offset || bps == 0 {
        return 0;
    }
    ((loop_end_offset - start_offset) * 8 / (bps * channels as usize)) as u32
}

pub fn create_wav_bytes(decoded: &StmDecoded) -> Result<Vec<u8>, hound::Error> {
    let mut cursor = Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: decoded.channels,
        sample_rate: decoded.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
    for sample in &decoded.samples {
        writer.write_sample(*sample)?;
    }
    writer.finalize()?;
    Ok(cursor.into_inner())
}
