/*
 * oni2_loader/parsers/hd_bd.rs — HD/BD audio subsong parser.
 *
 * SubsongInfo: index, stream offset/size, sample rate, channel count.
 * parse_hd: reads the .HD header file for a DAVE audio bank and returns the
 * list of embedded subsongs used to seek into the corresponding .BD stream.
 */
use anyhow::{Result, bail};
use std::io::Cursor;

#[derive(Debug, Clone)]
pub struct SubsongInfo {
    pub index: usize,
    pub stream_offset: u32,
    pub stream_size: u32,
    pub sample_rate: u16,
    pub channels: u16,
    pub loop_flag: bool,
    pub num_samples: u32,
    pub flags: u8,
}

#[derive(Debug)]
pub struct HdHeader {
    pub hd_size: u32,
    pub bd_size: u32,
    pub vagi_offset: u32,
    pub total_subsongs: i32,
    pub subsongs: Vec<SubsongInfo>,
}

fn read_u32le(data: &[u8], offset: usize) -> Result<u32> {
    if offset + 4 > data.len() {
        bail!("Unexpected EOF reading u32 at 0x{:x}", offset);
    }
    Ok(u32::from_le_bytes(
        data[offset..offset + 4].try_into().unwrap(),
    ))
}

fn read_s32le(data: &[u8], offset: usize) -> Result<i32> {
    if offset + 4 > data.len() {
        bail!("Unexpected EOF reading i32 at 0x{:x}", offset);
    }
    Ok(i32::from_le_bytes(
        data[offset..offset + 4].try_into().unwrap(),
    ))
}

fn read_u16le(data: &[u8], offset: usize) -> Result<u16> {
    if offset + 2 > data.len() {
        bail!("Unexpected EOF reading u16 at 0x{:x}", offset);
    }
    Ok(u16::from_le_bytes(
        data[offset..offset + 2].try_into().unwrap(),
    ))
}

fn read_u8(data: &[u8], offset: usize) -> Result<u8> {
    if offset >= data.len() {
        bail!("Unexpected EOF reading u8 at 0x{:x}", offset);
    }
    Ok(data[offset])
}

pub fn parse_hd(sf: &[u8]) -> Result<HdHeader> {
    if sf.len() < 0x24 {
        bail!("File too small to be HD");
    }

    if &sf[0..8] != b"IECSsreV" {
        bail!("Missing VersSCEI (IECSsreV) magic");
    }

    let head_offset = 0x10;
    if &sf[head_offset..head_offset + 8] != b"IECSdaeH" {
        bail!("Missing HeadSCEI (IECSdaeH) magic at 0x10");
    }

    let hd_size = read_u32le(sf, head_offset + 0x0c)?;
    let bd_size = read_u32le(sf, head_offset + 0x10)?;
    let vagi_offset = read_u32le(sf, head_offset + 0x20)? as usize;

    if sf.len() < vagi_offset + 0x10 {
        bail!("File truncated before Vagi chunk");
    }

    if &sf[vagi_offset..vagi_offset + 8] != b"IECSigaV" {
        bail!("Missing VagiSCEI (IECSigaV) magic");
    }

    let mut total_subsongs = read_s32le(sf, vagi_offset + 0x0c)?;

    if vagi_offset + 0x10 + 4 * (total_subsongs as usize) + 4 <= sf.len() {
        let null_offset = read_u32le(sf, vagi_offset + 0x10 + 4 * (total_subsongs as usize))?;
        if null_offset != 0 {
            total_subsongs += 1;
        }
    }

    if total_subsongs > 0 {
        let last_idx = (total_subsongs - 1) as usize;
        let last_offset = read_u32le(sf, vagi_offset + 0x10 + 4 * last_idx)?;
        if last_offset > 0 {
            let actual_offset_pos = vagi_offset + last_offset as usize;
            if actual_offset_pos + 4 <= sf.len() {
                if read_u32le(sf, actual_offset_pos)? == bd_size {
                    total_subsongs -= 1;
                }
            }
        }
    }

    if total_subsongs <= 0 {
        bail!("HD file contains no valid subsongs");
    }

    let mut subsongs = Vec::new();
    for target in 1..=total_subsongs {
        let target_subsong = target as usize;
        let info_offset_rel =
            read_u32le(sf, vagi_offset + 0x10 + 0x04 * (target_subsong - 1))? as usize;
        let info_offset = vagi_offset + info_offset_rel;

        let stream_offset = read_u32le(sf, info_offset)?;
        let sample_rate = read_u16le(sf, info_offset + 0x04)?;
        let flags = read_u8(sf, info_offset + 0x06)?;

        let next_offset = if target_subsong == total_subsongs as usize {
            bd_size
        } else {
            let next_info_rel =
                read_u32le(sf, vagi_offset + 0x10 + 0x04 * target_subsong)? as usize;
            read_u32le(sf, vagi_offset + next_info_rel)?
        };

        if next_offset < stream_offset {
            bail!(
                "Invalid next_offset < stream_offset for subsong {}",
                target_subsong
            );
        }
        let stream_size = next_offset - stream_offset;
        let channels = 1;

        let ps_bytes_to_samples = |bytes: u32, ch: u16| -> u32 {
            let blocks = bytes / (16 * ch as u32);
            blocks * 28
        };

        subsongs.push(SubsongInfo {
            index: target_subsong,
            stream_offset,
            stream_size,
            sample_rate,
            channels,
            loop_flag: (flags & 1) != 0,
            num_samples: ps_bytes_to_samples(stream_size, channels),
            flags,
        });
    }

    Ok(HdHeader {
        hd_size,
        bd_size,
        vagi_offset: vagi_offset as u32,
        total_subsongs,
        subsongs,
    })
}

pub fn decode_psx_adpcm(data: &[u8], expected_samples: u32) -> Result<Vec<i16>> {
    let mut out = Vec::with_capacity((data.len() / 16) * 28);
    let f0: [i32; 5] = [0, 60, 115, 98, 122];
    let f1: [i32; 5] = [0, 0, -52, -55, -60];

    let mut hist1: i32 = 0;
    let mut hist2: i32 = 0;

    for chunk in data.chunks_exact(16) {
        let shift_filter = chunk[0];
        let shift = (shift_filter & 0x0F) as i32;
        let mut filter_idx = ((shift_filter >> 4) & 0x0F) as usize;
        if filter_idx >= 5 {
            filter_idx = 0;
        }

        let f0_val = f0[filter_idx];
        let f1_val = f1[filter_idx];

        for i in 0..14 {
            let byte = chunk[2 + i];

            let mut nibble1 = (byte & 0x0F) as i8;
            nibble1 = (nibble1 << 4) >> 4;

            let mut nibble2 = (byte >> 4) as i8;
            nibble2 = (nibble2 << 4) >> 4;

            for &nibble in &[nibble1, nibble2] {
                let mut sample = (nibble as i32) << 12;
                sample >>= shift;

                let p = sample + ((f0_val * hist1 + f1_val * hist2 + 32) >> 6);
                let p = p.clamp(-32768, 32767);

                hist2 = hist1;
                hist1 = p;

                out.push(p as i16);
            }
        }
    }

    out.truncate(expected_samples as usize);
    Ok(out)
}

pub fn create_wav_bytes(samples: &[i16], sample_rate: u16, channels: u16) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: sample_rate as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut buffer, spec)?;
        for sample in samples {
            writer.write_sample(*sample)?;
        }
        writer.finalize()?;
    }
    Ok(buffer.into_inner())
}
