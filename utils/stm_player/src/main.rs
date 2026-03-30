use anyhow::{Context, Result, anyhow, bail};
use rodio::{OutputStream, Source, buffer::SamplesBuffer};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const START_OFFSET: usize = 0x800;
const IMA_DEFAULT_BLOCK: usize = 0x40;
const IMA_ALT_BLOCK: usize = 0x80;

#[derive(Debug)]
struct StmDecoded {
    sample_rate: u32,
    channels: u16,
    samples: Vec<i16>, // interleaved
    loop_start: Option<u32>,
    loop_end: Option<u32>,
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: {} <input.stm> [--wav output.wav] [--no-play]",
            args[0]
        );
        return Ok(());
    }

    let input_path = PathBuf::from(&args[1]);
    let mut wav_out: Option<PathBuf> = None;
    let mut play_audio = true;

    let mut idx = 2;
    while idx < args.len() {
        match args[idx].as_str() {
            "--wav" => {
                idx += 1;
                if idx >= args.len() {
                    bail!("--wav requires a path");
                }
                wav_out = Some(PathBuf::from(&args[idx]));
            }
            "--no-play" => {
                play_audio = false;
            }
            other => bail!("Unknown argument '{other}'"),
        }
        idx += 1;
    }

    let decoded = decode_stm(&input_path)?;
    println!(
        "Decoded {} ({} Hz, {} channels, {} samples)",
        input_path.display(),
        decoded.sample_rate,
        decoded.channels,
        decoded.samples.len() / decoded.channels as usize
    );
    if let Some(loop_start) = decoded.loop_start {
        println!(
            "Loop points: start={} samples, end={}",
            loop_start,
            decoded
                .loop_end
                .unwrap_or(decoded.samples.len() as u32 / decoded.channels as u32)
        );
    }

    if let Some(path) = wav_out {
        write_wav(&decoded, &path)?;
        println!("Wrote {}", path.display());
    }

    if play_audio {
        play_pcm(&decoded)?;
    }

    Ok(())
}

fn decode_stm(path: &Path) -> Result<StmDecoded> {
    let data = fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    if data.len() < START_OFFSET {
        bail!("File too small to be STM");
    }

    let magic = &data[0..4];
    let big_endian = match magic {
        b"STMA" => false,
        b"AMTS" => true,
        _ => bail!("Not an STM/STMA file"),
    };

    let read_u32 = |offset: usize| -> Result<u32> {
        if offset + 4 > data.len() {
            bail!("Unexpected EOF while reading header");
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
        bail!("STM reports audio past end of file");
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
            bail!("GC DSP STMA files are not supported yet");
        }
        _ => bail!("Unsupported STM encoding (bps={bps}, big_endian={big_endian})"),
    };

    Ok(StmDecoded {
        sample_rate,
        channels,
        samples,
        loop_start: loop_start_samples,
        loop_end: loop_end_samples,
    })
}

fn decode_pcm(data: &[u8], channels: usize, big_endian: bool) -> Result<Vec<i16>> {
    if data.len() % 2 != 0 {
        bail!("PCM data not aligned");
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
        bail!("PCM sample count not divisible by channel count");
    }
    Ok(samples)
}

fn decode_ima(data: &[u8], channels: usize, block_size: usize) -> Result<Vec<i16>> {
    if block_size < 4 {
        bail!("IMA block size too small");
    }
    let frame_size = block_size * channels;
    if data.len() % frame_size != 0 {
        bail!("IMA data doesn't align to channel blocks");
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
            bail!("Channel {idx} length mismatch");
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

fn decode_ima_block(block: &[u8]) -> Result<Vec<i16>> {
    if block.len() < 4 {
        bail!("IMA block shorter than header");
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

fn write_wav(decoded: &StmDecoded, path: &Path) -> Result<()> {
    let spec = hound::WavSpec {
        channels: decoded.channels,
        sample_rate: decoded.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("Failed to create {}", path.display()))?;
    for sample in &decoded.samples {
        writer.write_sample(*sample)?;
    }
    writer.finalize()?;
    Ok(())
}

fn play_pcm(decoded: &StmDecoded) -> Result<()> {
    let (_stream, handle) =
        OutputStream::try_default().map_err(|e| anyhow!("Audio output unavailable: {e}"))?;

    let buffer = SamplesBuffer::new(
        decoded.channels,
        decoded.sample_rate,
        decoded.samples.clone(),
    );
    let duration = buffer.total_duration().unwrap_or_else(|| {
        Duration::from_secs_f64(
            decoded.samples.len() as f64 / decoded.sample_rate as f64 / decoded.channels as f64,
        )
    });

    handle
        .play_raw(buffer.convert_samples())
        .map_err(|e| anyhow!("Failed to start playback: {e}"))?;

    thread::sleep(duration);
    Ok(())
}
