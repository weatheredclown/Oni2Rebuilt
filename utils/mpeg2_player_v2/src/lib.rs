//! Focused pure-Rust MPEG-2 video player foundation.
//!
//! This crate intentionally has no dependency on the vendored `oxideav` MPEG
//! decoder. It provides the public API, demuxing, start-code scanning, header
//! parsing, color conversion, deinterlacing helpers, scan tables, dequantisation,
//! and a reference IDCT. Full slice/macroblock reconstruction is staged behind
//! explicit `DecodeNotImplemented` errors so unsupported paths are never silently
//! decoded incorrectly.

pub mod bitstream;
pub mod block;
pub mod colorspace;
pub mod deinterlace;
pub mod demux;
pub mod dequant;
pub mod error;
pub mod headers;
pub mod idct;
pub mod mb;
pub mod motion;
pub mod picture;
pub mod picture_params;
pub mod scan;
pub mod slice;
pub mod tables;
pub mod vlc;

use std::collections::VecDeque;
use std::fs;
use std::path::Path;

pub use colorspace::ChromaUpsample;
pub use deinterlace::DeinterlaceMode;
pub use error::{Error, Result};
pub use headers::{AspectRatio, PictureType, SequenceInfo};

#[derive(Clone, Debug)]
pub enum FramePlanes {
    Yuv420p {
        y: Vec<u8>,
        cb: Vec<u8>,
        cr: Vec<u8>,
    },
    Rgba8(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub planes: FramePlanes,
    pub picture_type: PictureType,
    pub interlaced: bool,
    pub top_field_first: bool,
    pub presentation_time: f64,
}

pub struct Mpeg2Player {
    elementary_stream: Vec<u8>,
    info: SequenceInfo,
    seq_header: headers::SequenceHeader,
    deinterlace: DeinterlaceMode,
    pending_bob: VecDeque<VideoFrame>,
    /// Byte offset into `elementary_stream` where the next picture's bytes
    /// begin (specifically, where its picture_start_code prefix `00 00 01 00`
    /// begins).  `None` once the stream is exhausted.
    next_picture_offset: Option<usize>,
    /// Display PTS counter, advanced by 1/fps each emitted frame.
    presentation_time: f64,
    fwd_ref: Option<crate::picture::PictureBuffer>,
    bwd_ref: Option<crate::picture::PictureBuffer>,
}

impl Mpeg2Player {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        Self::from_bytes(bytes)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let elementary_stream = demux::extract_video_elementary_stream(&bytes)?;
        let info = headers::parse_sequence_info(&elementary_stream)?;
        let seq_header = parse_sequence_header_from_stream(&elementary_stream)?;
        let next_picture_offset = find_picture_start(&elementary_stream, 0);
        Ok(Self {
            elementary_stream,
            info,
            seq_header,
            deinterlace: DeinterlaceMode::Preserve,
            pending_bob: VecDeque::new(),
            next_picture_offset,
            presentation_time: 0.0,
            fwd_ref: None,
            bwd_ref: None,
        })
    }

    pub fn info(&self) -> &SequenceInfo {
        &self.info
    }

    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>> {
        if let Some(frame) = self.pending_bob.pop_front() {
            return Ok(Some(frame));
        }
        let Some(start) = self.next_picture_offset else {
            return Ok(None);
        };
        // Where does this picture end?  At the next picture_start_code or
        // at end-of-stream.
        let end = find_picture_start(&self.elementary_stream, start + 4)
            .unwrap_or(self.elementary_stream.len());
        // Clone the slice into a local Vec to break the borrow on `self`
        // — picture decode mutates `self.presentation_time`.  Frames are
        // ≪ 1 MB in practice so this is essentially free.
        let picture_bytes = self.elementary_stream[start..end].to_vec();
        let frame = self.decode_picture(&picture_bytes)?;
        self.next_picture_offset = if end < self.elementary_stream.len() {
            Some(end)
        } else {
            None
        };
        Ok(Some(frame))
    }

    pub fn set_deinterlace(&mut self, mode: DeinterlaceMode) {
        self.deinterlace = mode;
    }

    pub fn deinterlace(&self) -> DeinterlaceMode {
        self.deinterlace
    }

    fn decode_picture(&mut self, picture_bytes: &[u8]) -> Result<VideoFrame> {
        use bitstream::{EXTENSION_START, PICTURE_START, start_code_payloads};

        let payloads = start_code_payloads(picture_bytes);

        // Find the picture header (mandatory, must be at offset 0).
        let (pic_start, pic_payload) = payloads
            .iter()
            .find(|(sc, _)| sc.code == PICTURE_START)
            .ok_or_else(|| Error::invalid("decode_picture: no picture_start_code at offset 0"))?;
        if pic_start.offset != 0 {
            return Err(Error::invalid(
                "decode_picture: picture_start_code not at offset 0",
            ));
        }
        let pic_header = headers::parse_picture_header(pic_payload)?;

        // Picture coding extension follows.  An extension start code can
        // appear with several extension-IDs; the picture coding extension
        // has id = 8 (top nibble of first byte = `1000`).
        let pic_ext_payload = payloads
            .iter()
            .find(|(sc, payload)| {
                sc.code == EXTENSION_START && payload.first().map(|b| b >> 4) == Some(8)
            })
            .map(|(_, payload)| *payload)
            .ok_or_else(|| Error::invalid("decode_picture: missing picture coding extension"))?;
        let pic_ext = headers::parse_picture_coding_extension(pic_ext_payload)?;

        // `frame_pred_frame_dct = 0` and `intra_vlc_format = 1` are both
        // plumbed end-to-end (MB-mode side bits in mb.rs, Table B-15 in
        // block.rs, field-DCT layout in decode_inter_mb).  Field-MC
        // (`frame_motion_type = 0b01`) is rejected at the MB level by
        // decode_inter_mb until milestone 8 lands the field-aware
        // prediction read.

        let params = crate::picture_params::PictureParams {
            sequence: self.info.clone(),
            header: pic_header,
            coding: pic_ext,
        };

        let mut pic = crate::picture::PictureBuffer::new(self.info.width, self.info.height);

        let fwd_ref_pass = if matches!(params.header.picture_coding_type, PictureType::B) {
            self.fwd_ref.as_ref()
        } else {
            self.bwd_ref.as_ref()
        };
        let bwd_ref_pass = if matches!(params.header.picture_coding_type, PictureType::B) {
            self.bwd_ref.as_ref()
        } else {
            None
        };

        for (sc, payload) in &payloads {
            if (1..=0xaf).contains(&sc.code) {
                slice::decode_slice(
                    payload,
                    sc.code,
                    &mut pic,
                    fwd_ref_pass,
                    bwd_ref_pass,
                    &params,
                    &self.seq_header.intra_quantiser_matrix,
                    &self.seq_header.non_intra_quantiser_matrix,
                )?;
            }
        }

        if matches!(
            params.header.picture_coding_type,
            PictureType::I | PictureType::P
        ) {
            self.fwd_ref = self.bwd_ref.take();
            self.bwd_ref = Some(pic.clone());
        }

        // Crop the MB-padded planes down to the display rectangle.
        let display_w = self.info.width as usize;
        let display_h = self.info.height as usize;
        let mut y = Vec::with_capacity(display_w * display_h);
        for row in 0..display_h {
            let off = row * pic.y_stride;
            y.extend_from_slice(&pic.y[off..off + display_w]);
        }
        let c_w = display_w.div_ceil(2);
        let c_h = display_h.div_ceil(2);
        let mut cb = Vec::with_capacity(c_w * c_h);
        let mut cr = Vec::with_capacity(c_w * c_h);
        for row in 0..c_h {
            let off = row * pic.c_stride;
            cb.extend_from_slice(&pic.cb[off..off + c_w]);
            cr.extend_from_slice(&pic.cr[off..off + c_w]);
        }

        let pts = self.presentation_time;
        if self.info.frame_rate_num > 0 {
            self.presentation_time +=
                f64::from(self.info.frame_rate_den) / f64::from(self.info.frame_rate_num);
        }

        Ok(VideoFrame {
            width: self.info.width,
            height: self.info.height,
            planes: FramePlanes::Yuv420p { y, cb, cr },
            picture_type: params.header.picture_coding_type,
            interlaced: params.interlaced(),
            top_field_first: params.coding.top_field_first,
            presentation_time: pts,
        })
    }
}

fn parse_sequence_header_from_stream(es: &[u8]) -> Result<headers::SequenceHeader> {
    use bitstream::{SEQUENCE_HEADER, start_code_payloads};
    for (start, payload) in start_code_payloads(es) {
        if start.code == SEQUENCE_HEADER {
            return headers::parse_sequence_header(payload);
        }
    }
    Err(Error::NeedSequenceHeader)
}

fn find_picture_start(es: &[u8], from: usize) -> Option<usize> {
    use bitstream::PICTURE_START;
    for sc in bitstream::find_start_codes(&es[from..]) {
        if sc.code == PICTURE_START {
            return Some(from + sc.offset);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitstream::{EXTENSION_START, SEQUENCE_HEADER};

    #[test]
    fn opens_elementary_stream_and_reports_info() {
        let mut es = vec![0, 0, 1, SEQUENCE_HEADER];
        es.extend_from_slice(&[0x20, 0x01, 0xc0, 0x34, 0x00, 0x00, 0x20, 0x00]);
        es.extend_from_slice(&[0, 0, 1, EXTENSION_START, 0x15, 0x8a, 0x00, 0x01, 0x00, 0x00]);
        let player = Mpeg2Player::from_bytes(es).unwrap();
        assert_eq!(player.info().width, 512);
        assert_eq!(player.info().height, 448);
    }
}
