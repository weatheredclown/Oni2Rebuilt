use anyhow::{anyhow, bail, Context, Result};
use oxideav_core::{CodecId, CodecParameters, Decoder, Error as AvError, Frame, Packet, TimeBase};
use oxideav_mpeg12video::decoder::make_decoder_mpeg2;
use std::fs;
use std::path::Path;

const START_CODE_PREFIX: [u8; 3] = [0, 0, 1];
const PICTURE_START: u8 = 0x00;
const SEQUENCE_HEADER: u8 = 0xb3;
const EXTENSION_START: u8 = 0xb5;
const PACK_START: u8 = 0xba;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeinterlaceMode {
    /// Keep the decoded frame as delivered by the MPEG-2 picture layer.
    Preserve,
    /// Treat alternating scanlines as interlaced fields but keep one output frame.
    Weave,
    /// Duplicate each source field line to a full-height frame, yielding two frames
    /// for every interlaced input picture.
    Bob,
}

impl DeinterlaceMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "preserve" => Ok(Self::Preserve),
            "weave" => Ok(Self::Weave),
            "bob" => Ok(Self::Bob),
            _ => bail!("unknown deinterlace mode '{value}' (expected preserve, weave, or bob)"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Mpeg2PlayerOptions {
    pub deinterlace: DeinterlaceMode,
    pub convert_to_rgba: bool,
}

impl Default for Mpeg2PlayerOptions {
    fn default() -> Self {
        Self {
            deinterlace: DeinterlaceMode::Preserve,
            convert_to_rgba: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldOrder {
    Progressive,
    TopFirst,
    BottomFirst,
}

#[derive(Clone, Debug)]
pub struct VideoBuffer {
    pub width: usize,
    pub height: usize,
    pub pts: Option<i64>,
    pub field_order: FieldOrder,
    pub y_stride: usize,
    pub c_stride: usize,
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
    pub rgba: Option<Vec<u8>>,
}

impl VideoBuffer {
    pub fn rgba_stride(&self) -> Option<usize> {
        self.rgba.as_ref().map(|_| self.width * 4)
    }

    pub fn write_yuv420p(&self, path: &Path) -> Result<()> {
        let mut out = Vec::with_capacity(self.y.len() + self.cb.len() + self.cr.len());
        out.extend_from_slice(&self.y);
        out.extend_from_slice(&self.cb);
        out.extend_from_slice(&self.cr);
        fs::write(path, out).with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn write_rgba(&self, path: &Path) -> Result<()> {
        let rgba = self
            .rgba
            .as_ref()
            .ok_or_else(|| anyhow!("RGBA output was not requested"))?;
        fs::write(path, rgba).with_context(|| format!("failed to write {}", path.display()))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PictureInterlace {
    interlaced: bool,
    top_field_first: bool,
}

pub struct Mpeg2Player {
    decoder: Box<dyn Decoder>,
    options: Mpeg2PlayerOptions,
}

impl Mpeg2Player {
    pub fn new(options: Mpeg2PlayerOptions) -> Result<Self> {
        let params = CodecParameters::video(CodecId::new("mpeg2video"));
        let decoder = make_decoder_mpeg2(&params).map_err(map_av_error)?;
        Ok(Self { decoder, options })
    }

    pub fn decode_file(path: &Path, options: Mpeg2PlayerOptions) -> Result<Vec<VideoBuffer>> {
        let data = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
        Self::new(options)?.decode_bytes(&data)
    }

    pub fn decode_bytes(&mut self, data: &[u8]) -> Result<Vec<VideoBuffer>> {
        let es = extract_video_elementary_stream(data)?;
        let interlace = scan_picture_interlace(&es);
        let packet = Packet::new(0, TimeBase::new(1, 90_000), es);
        self.decoder.send_packet(&packet).map_err(map_av_error)?;
        self.decoder.flush().map_err(map_av_error)?;

        let mut output = Vec::new();
        let mut index = 0usize;
        loop {
            match self.decoder.receive_frame() {
                Ok(Frame::Video(frame)) => {
                    let meta = interlace.get(index).copied().unwrap_or_default();
                    index += 1;
                    output.extend(video_frame_to_buffers(frame, meta, &self.options)?);
                }
                Ok(_) => {}
                Err(AvError::Eof) => break,
                Err(AvError::NeedMore) => continue,
                Err(err) => return Err(map_av_error(err)),
            }
        }
        Ok(output)
    }
}

fn video_frame_to_buffers(
    frame: oxideav_core::VideoFrame,
    meta: PictureInterlace,
    options: &Mpeg2PlayerOptions,
) -> Result<Vec<VideoBuffer>> {
    if frame.planes.len() != 3 {
        bail!(
            "decoder returned {} planes, expected YUV420P",
            frame.planes.len()
        );
    }
    let y = &frame.planes[0];
    let cb = &frame.planes[1];
    let cr = &frame.planes[2];
    let width = y.stride;
    if width == 0 || y.data.len() % width != 0 {
        bail!("invalid luma plane stride/length");
    }
    let height = y.data.len() / width;
    let field_order = if meta.interlaced {
        if meta.top_field_first {
            FieldOrder::TopFirst
        } else {
            FieldOrder::BottomFirst
        }
    } else {
        FieldOrder::Progressive
    };

    let base = VideoBuffer {
        width,
        height,
        pts: frame.pts,
        field_order,
        y_stride: y.stride,
        c_stride: cb.stride,
        y: y.data.clone(),
        cb: cb.data.clone(),
        cr: cr.data.clone(),
        rgba: None,
    };

    let mut buffers = match (meta.interlaced, options.deinterlace) {
        (true, DeinterlaceMode::Bob) => bob_deinterlace(&base)?,
        _ => vec![base],
    };

    if options.convert_to_rgba {
        for buffer in &mut buffers {
            buffer.rgba = Some(yuv420p_to_rgba(buffer));
        }
    }
    Ok(buffers)
}

fn bob_deinterlace(input: &VideoBuffer) -> Result<Vec<VideoBuffer>> {
    if input.height < 2 || input.height % 2 != 0 {
        return Ok(vec![input.clone()]);
    }
    let first_parity = if input.field_order == FieldOrder::BottomFirst {
        1
    } else {
        0
    };
    let second_parity = 1 - first_parity;
    Ok(vec![
        bob_one_field(input, first_parity)?,
        bob_one_field(input, second_parity)?,
    ])
}

fn bob_one_field(input: &VideoBuffer, parity: usize) -> Result<VideoBuffer> {
    let mut y = vec![0u8; input.y.len()];
    for out_row in 0..input.height {
        let src_row = ((out_row / 2) * 2 + parity).min(input.height - 1);
        y[out_row * input.width..(out_row + 1) * input.width].copy_from_slice(
            &input.y[src_row * input.y_stride..src_row * input.y_stride + input.width],
        );
    }
    let cw = input.width.div_ceil(2);
    let ch = input.height.div_ceil(2);
    let chroma_parity = parity.min(ch.saturating_sub(1));
    let mut cb = vec![0u8; input.cb.len()];
    let mut cr = vec![0u8; input.cr.len()];
    for out_row in 0..ch {
        let src_row = ((out_row / 2) * 2 + chroma_parity).min(ch - 1);
        cb[out_row * cw..(out_row + 1) * cw]
            .copy_from_slice(&input.cb[src_row * input.c_stride..src_row * input.c_stride + cw]);
        cr[out_row * cw..(out_row + 1) * cw]
            .copy_from_slice(&input.cr[src_row * input.c_stride..src_row * input.c_stride + cw]);
    }
    Ok(VideoBuffer {
        y,
        cb,
        cr,
        field_order: FieldOrder::Progressive,
        rgba: None,
        ..input.clone()
    })
}

fn yuv420p_to_rgba(input: &VideoBuffer) -> Vec<u8> {
    let mut rgba = vec![0u8; input.width * input.height * 4];
    for row in 0..input.height {
        for col in 0..input.width {
            let y = input.y[row * input.y_stride + col] as i32;
            let c_row = row / 2;
            let c_col = col / 2;
            let cb = input.cb[c_row * input.c_stride + c_col] as i32 - 128;
            let cr = input.cr[c_row * input.c_stride + c_col] as i32 - 128;
            let c = y - 16;
            let r = ((298 * c + 409 * cr + 128) >> 8).clamp(0, 255) as u8;
            let g = ((298 * c - 100 * cb - 208 * cr + 128) >> 8).clamp(0, 255) as u8;
            let b = ((298 * c + 516 * cb + 128) >> 8).clamp(0, 255) as u8;
            let offset = (row * input.width + col) * 4;
            rgba[offset..offset + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    rgba
}

fn extract_video_elementary_stream(data: &[u8]) -> Result<Vec<u8>> {
    if has_program_stream_markers(data) {
        demux_program_stream(data)
    } else {
        Ok(data.to_vec())
    }
}

fn has_program_stream_markers(data: &[u8]) -> bool {
    find_start_codes(data).any(|(_, code)| code == PACK_START || (0xe0..=0xef).contains(&code))
}

fn demux_program_stream(data: &[u8]) -> Result<Vec<u8>> {
    let starts: Vec<_> = find_start_codes(data).collect();
    let mut out = Vec::new();
    for (idx, (pos, code)) in starts.iter().copied().enumerate() {
        if !(0xe0..=0xef).contains(&code) {
            continue;
        }
        let next = starts.get(idx + 1).map(|(p, _)| *p).unwrap_or(data.len());
        let mut offset = pos + 4;
        if offset + 2 > next {
            continue;
        }
        let packet_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        let packet_end = if packet_len == 0 {
            next
        } else {
            (offset + packet_len).min(data.len())
        };
        if offset + 3 > packet_end {
            continue;
        }
        let flags = data[offset];
        if flags & 0xc0 == 0x80 {
            let header_len = data[offset + 2] as usize;
            offset += 3 + header_len;
        } else {
            while offset < packet_end && data[offset] == 0xff {
                offset += 1;
            }
            if offset < packet_end && data[offset] & 0xc0 == 0x40 {
                offset += 2;
            }
            if offset < packet_end {
                let marker = data[offset] & 0xf0;
                offset += match marker {
                    0x20 => 5,
                    0x30 => 10,
                    _ => 0,
                };
            }
        }
        if offset < packet_end {
            out.extend_from_slice(&data[offset..packet_end]);
        }
    }
    if out.is_empty() {
        bail!("program stream did not contain MPEG video PES payloads");
    }
    Ok(out)
}

fn scan_picture_interlace(es: &[u8]) -> Vec<PictureInterlace> {
    let starts: Vec<_> = find_start_codes(es).collect();
    let mut result = Vec::new();
    let mut progressive_sequence = true;
    let mut pending: Option<PictureInterlace> = None;

    for (idx, (pos, code)) in starts.iter().copied().enumerate() {
        let payload_start = pos + 4;
        let payload_end = starts.get(idx + 1).map(|(p, _)| *p).unwrap_or(es.len());
        if payload_start > payload_end || payload_end > es.len() {
            continue;
        }
        let payload = &es[payload_start..payload_end];
        match code {
            SEQUENCE_HEADER => progressive_sequence = true,
            PICTURE_START => {
                if let Some(meta) = pending.take() {
                    result.push(meta);
                }
                pending = Some(PictureInterlace {
                    interlaced: !progressive_sequence,
                    top_field_first: true,
                });
            }
            EXTENSION_START => {
                if let Some(id) = read_bits(payload, 0, 4) {
                    match id {
                        1 => {
                            if let Some(flag) = read_bits(payload, 12, 1) {
                                progressive_sequence = flag == 1;
                            }
                        }
                        8 => {
                            let top_field_first = read_bits(payload, 24, 1).unwrap_or(1) == 1;
                            let progressive_frame = read_bits(payload, 32, 1).unwrap_or(1) == 1;
                            if let Some(meta) = pending.as_mut() {
                                meta.interlaced = !progressive_sequence || !progressive_frame;
                                meta.top_field_first = top_field_first;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(meta) = pending.take() {
        result.push(meta);
    }
    result
}

fn read_bits(data: &[u8], bit_offset: usize, bits: usize) -> Option<u32> {
    let mut value = 0u32;
    for bit in 0..bits {
        let absolute = bit_offset + bit;
        let byte = *data.get(absolute / 8)?;
        let shift = 7 - (absolute % 8);
        value = (value << 1) | ((byte >> shift) & 1) as u32;
    }
    Some(value)
}

fn find_start_codes(data: &[u8]) -> impl Iterator<Item = (usize, u8)> + '_ {
    data.windows(4)
        .enumerate()
        .filter_map(|(idx, window)| (window[0..3] == START_CODE_PREFIX).then_some((idx, window[3])))
}

fn map_av_error(err: AvError) -> anyhow::Error {
    match err {
        AvError::Unsupported(msg) if msg.contains("field pictures") => anyhow!(
            "unsupported MPEG-2 interlace coding: field pictures are not decoded yet; frame-picture interlaced streams are supported"
        ),
        AvError::Unsupported(msg) if msg.contains("field-DCT") => anyhow!(
            "unsupported MPEG-2 interlace coding: field-DCT/field-MC macroblocks are not decoded yet"
        ),
        other => anyhow!(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_video_pes_from_program_stream() {
        let mut ps = vec![0, 0, 1, PACK_START];
        ps.extend_from_slice(&[0; 12]);
        ps.extend_from_slice(&[0, 0, 1, 0xbb, 0, 0]);
        ps.extend_from_slice(&[0, 0, 1, 0xe0, 0, 7, 0x80, 0, 0]);
        ps.extend_from_slice(&[0, 0, 1, SEQUENCE_HEADER]);
        ps.extend_from_slice(&[0x2d, 0x02, 0x40]);
        let es = extract_video_elementary_stream(&ps).unwrap();
        assert!(es.starts_with(&[0, 0, 1, SEQUENCE_HEADER]));
    }

    #[test]
    fn scans_interlaced_picture_extension() {
        let mut es = Vec::new();
        es.extend_from_slice(&[0, 0, 1, SEQUENCE_HEADER, 0, 0, 0]);
        // sequence_extension id=1, profile byte, progressive_sequence=0.
        es.extend_from_slice(&[0, 0, 1, EXTENSION_START, 0x10, 0x00]);
        es.extend_from_slice(&[0, 0, 1, PICTURE_START, 0, 0]);
        // picture_coding_extension id=8 with top_field_first=0 and progressive_frame=0.
        es.extend_from_slice(&[0, 0, 1, EXTENSION_START, 0x8f, 0xff, 0x3f, 0x00, 0x00]);
        let info = scan_picture_interlace(&es);
        assert_eq!(info.len(), 1);
        assert!(info[0].interlaced);
        assert!(!info[0].top_field_first);
    }

    #[test]
    fn bob_deinterlace_doubles_each_field_line() {
        let input = VideoBuffer {
            width: 2,
            height: 4,
            pts: None,
            field_order: FieldOrder::TopFirst,
            y_stride: 2,
            c_stride: 1,
            y: vec![1, 1, 2, 2, 3, 3, 4, 4],
            cb: vec![10, 20],
            cr: vec![30, 40],
            rgba: None,
        };
        let out = bob_deinterlace(&input).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].y, vec![1, 1, 1, 1, 3, 3, 3, 3]);
        assert_eq!(out[1].y, vec![2, 2, 2, 2, 4, 4, 4, 4]);
    }
}
