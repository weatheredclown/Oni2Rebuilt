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
pub mod picture_params;
pub mod scan;
pub mod slice;
pub mod tables;

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
    deinterlace: DeinterlaceMode,
    pending_bob: VecDeque<VideoFrame>,
    decode_started: bool,
}

impl Mpeg2Player {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        Self::from_bytes(bytes)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let elementary_stream = demux::extract_video_elementary_stream(&bytes)?;
        let info = headers::parse_sequence_info(&elementary_stream)?;
        Ok(Self {
            elementary_stream,
            info,
            deinterlace: DeinterlaceMode::Preserve,
            pending_bob: VecDeque::new(),
            decode_started: false,
        })
    }

    pub fn info(&self) -> &SequenceInfo {
        &self.info
    }

    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>> {
        if let Some(frame) = self.pending_bob.pop_front() {
            return Ok(Some(frame));
        }
        if self.decode_started {
            return Ok(None);
        }
        self.decode_started = true;
        Err(Error::not_implemented(format!(
            "slice and macroblock reconstruction for {} bytes of elementary stream; milestones 1-3 are available",
            self.elementary_stream.len()
        )))
    }

    pub fn set_deinterlace(&mut self, mode: DeinterlaceMode) {
        self.deinterlace = mode;
    }

    pub fn deinterlace(&self) -> DeinterlaceMode {
        self.deinterlace
    }
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
