use bevy::prelude::*;
use super::types::Oni2Animation;
use crate::oni2_loader::utils::binary::{read_u32_le, read_f32_le};

// Magic numbers matching C++ MAKE_MAGIC_NUMBER macro (little-endian u32)
const ANIM_MAGIC_ANI0: u32 = u32::from_le_bytes([b'a', b'n', b'i', 0]);   // 0x00696E61
const ANIM_MAGIC_ANI1: u32 = u32::from_le_bytes([b'A', b'N', b'I', b'1']); // 0x31494E41

/// Parse a binary .anim file. Supports three format variants:
/// - format_id=0: old format (17-byte header, raw float channels)
/// - format_id='ani0': extended format with format_flags, conditional stride
/// - format_id='ANI1': newest format with full stride vector
pub fn parse_anim(data: &[u8]) -> Option<Oni2Animation> {
    if data.len() < 17 {
        warn!("Anim file too small: {} bytes", data.len());
        return None;
    }

    let format_id = read_u32_le(data, 0);

    match format_id {
        ANIM_MAGIC_ANI0 => parse_anim_ani0(data),
        ANIM_MAGIC_ANI1 => parse_anim_ani1(data),
        _ => {
            warn!("Unsupported anim format_id: 0x{:08X} ({})", format_id, format_id);
            None
        }
    }
}

/// 'ani0' format (format_id=0x00696E61):
/// Header: [u32:'ani0'] [u32:format_flags] [u32:num_frames] [u32:num_channels]
///   then stride: if flags&1 → [f32×3], else [f32:stride_z]
///   then [u8:loop_flag]
/// Per frame: if flags&2 → skip [f32×3 delta], then [num_channels × f32]
fn parse_anim_ani0(data: &[u8]) -> Option<Oni2Animation> {
    if data.len() < 20 {
        warn!("ani0 header too small");
        return None;
    }

    let format_flags = read_u32_le(data, 4);
    let num_frames = read_u32_le(data, 8);
    let num_channels = read_u32_le(data, 12);

    let mut off = 16usize;

    // Stride
    let stride_z;
    if format_flags & 1 != 0 {
        // Full stride: 3 floats (x, y, z)
        if off + 12 > data.len() { return None; }
        let _sx = read_f32_le(data, off);
        let _sy = read_f32_le(data, off + 4);
        stride_z = read_f32_le(data, off + 8);
        off += 12;
    } else {
        // Z-only stride
        if off + 4 > data.len() { return None; }
        stride_z = read_f32_le(data, off);
        off += 4;
    }

    // Loop flag (single byte)
    if off >= data.len() { return None; }
    let is_loop = data[off] != 0;
    off += 1;

    // Track-object flag: BIT(1) means each frame has 3 extra delta floats before channels
    let has_delta = format_flags & 2 != 0;

    let frames = read_frames(data, &mut off, num_frames, num_channels, 0, has_delta)?;

    info!("Parsed anim (ani0): {} frames, {} channels, stride_z={}, loop={}, flags=0x{:X}",
        num_frames, num_channels, stride_z, is_loop, format_flags);

    Some(Oni2Animation { num_frames, num_channels, stride_z, is_loop, frames })
}

/// 'ANI1' format (format_id=0x31494E41):
/// Header: [u32:'ANI1'] [u32:format_flags] [u32:num_frames] [u32:num_channels]
///         [f32:stride_x] [f32:stride_y] [f32:stride_z]
/// Per frame: [num_channels × f32], then if flags&BIT(31) → skip [f32×3 delta]
/// Loop: flags & BIT(0)
fn parse_anim_ani1(data: &[u8]) -> Option<Oni2Animation> {
    if data.len() < 28 {
        warn!("ANI1 header too small");
        return None;
    }

    let format_flags = read_u32_le(data, 4);
    let num_frames = read_u32_le(data, 8);
    let num_channels = read_u32_le(data, 12);
    let _stride_x = read_f32_le(data, 16);
    let _stride_y = read_f32_le(data, 20);
    let stride_z = read_f32_le(data, 24);

    let is_loop = format_flags & 1 != 0;
    // BIT(31): normalized trans → delta floats AFTER channel data per frame
    let has_trailing_delta = format_flags & (1 << 31) != 0;

    let mut off = 28usize;
    let frames = read_frames(data, &mut off, num_frames, num_channels, if has_trailing_delta { 3 } else { 0 }, false)?;

    info!("Parsed anim (ANI1): {} frames, {} channels, stride_z={}, loop={}, flags=0x{:X}",
        num_frames, num_channels, stride_z, is_loop, format_flags);

    Some(Oni2Animation { num_frames, num_channels, stride_z, is_loop, frames })
}

/// Read frame data from a byte slice. Handles optional leading/trailing delta floats.
/// `trailing_skip`: number of extra f32s to skip AFTER channel data per frame.
/// `has_leading_delta`: if true, skip 3 f32s BEFORE channel data per frame.
fn read_frames(
    data: &[u8],
    off: &mut usize,
    num_frames: u32,
    num_channels: u32,
    trailing_skip: usize,
    has_leading_delta: bool,
) -> Option<Vec<Vec<f32>>> {
    let leading = if has_leading_delta { 3 } else { 0 };
    let per_frame_floats = leading + num_channels as usize + trailing_skip;
    let expected = *off + num_frames as usize * per_frame_floats * 4;
    if data.len() < expected {
        warn!("Anim file truncated: {} bytes, expected at least {}", data.len(), expected);
        return None;
    }

    let mut frames = Vec::with_capacity(num_frames as usize);
    for _ in 0..num_frames {
        // Skip leading delta if present
        if has_leading_delta {
            *off += 12; // 3 × f32
        }
        let mut channels = Vec::with_capacity(num_channels as usize);
        for _ in 0..num_channels {
            channels.push(read_f32_le(data, *off));
            *off += 4;
        }
        // Skip trailing delta if present
        *off += trailing_skip * 4;
        frames.push(channels);
    }
    Some(frames)
}
