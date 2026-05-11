//! Macroblock-level decode.
//!
//! Milestone 4 scope: intra macroblocks only.  Inter MBs (P/B with motion
//! compensation) come in milestone 5.

use crate::bitstream::BitReader;
use crate::block::decode_intra_block;
use crate::error::{Error, Result};
use crate::headers::PictureType;
use crate::picture::PictureBuffer;
use crate::picture_params::PictureParams;
use crate::tables::mb_type;
use crate::vlc;

/// Per-slice running state carried between macroblocks.
#[derive(Clone, Copy, Debug)]
pub struct SliceState {
    /// DC predictors per component: 0 = luma, 1 = Cb, 2 = Cr.  Reset at
    /// slice start and after any non-intra MB.
    pub dc_pred: [i32; 3],
    /// Current effective `quantiser_scale_code`.  Initialised from the
    /// slice header; can be overridden per MB via the macroblock-type's
    /// `quant` flag.
    pub quant_code: u8,
}

impl SliceState {
    pub fn new(dc_reset: i32, quant_code: u8) -> Self {
        Self {
            dc_pred: [dc_reset; 3],
            quant_code,
        }
    }
}

/// Decode one intra macroblock at `(mb_x, mb_y)` into `pic`.  Reads the MB
/// type, optional quantiser scale, and the six 8×8 blocks (Y0..Y3, Cb, Cr).
pub fn decode_intra_mb(
    br: &mut BitReader<'_>,
    state: &mut SliceState,
    pic: &mut PictureBuffer,
    params: &PictureParams,
    intra_quantiser: &[u8; 64],
    mb_x: usize,
    mb_y: usize,
) -> Result<()> {
    // For I-pictures the only valid MB types are `intra` and `intra+quant`.
    let mb_type_flags = vlc::decode(br, mb_type::i_table())?;
    if !mb_type_flags.intra {
        return Err(Error::invalid("I-picture: non-intra MB type decoded"));
    }
    if mb_type_flags.quant {
        let qs = br.read(5)? as u8;
        if qs == 0 {
            return Err(Error::invalid("MB quantiser_scale_code = 0"));
        }
        state.quant_code = qs;
    }

    // MPEG-2 §6.3.17.1: `dct_type` side bit when
    // `frame_pred_frame_dct == 0` and the MB has pattern/intra coefficients.
    // For milestone 4 the fixture uses `frame_pred_frame_dct = 1`, so this
    // branch isn't expected to fire — but we honour it if the bitstream
    // presents it (without acting on field-DCT yet).
    let _dct_type = if !params.coding.frame_pred_frame_dct {
        Some(br.read(1)? != 0)
    } else {
        None
    };

    // Decode the six blocks.  Layout in luma: blocks 0/1 are the left/right
    // top-half, 2/3 are the left/right bottom-half.  4 = Cb, 5 = Cr.
    for b in 0..6usize {
        let (is_chroma, comp_idx, dst_x0, dst_y0, stride) = block_layout(b, mb_x, mb_y, pic);
        let plane: &mut [u8] = match comp_idx {
            0 => &mut pic.y[..],
            1 => &mut pic.cb[..],
            _ => &mut pic.cr[..],
        };
        let sub = &mut plane[dst_y0 * stride + dst_x0..];
        decode_intra_block(
            br,
            is_chroma,
            &mut state.dc_pred[comp_idx],
            state.quant_code,
            intra_quantiser,
            params,
            sub,
            stride,
        )?;
    }
    Ok(())
}

/// Returns `(is_chroma, component_index, dst_x0, dst_y0, plane_stride)` for
/// block `b ∈ 0..6` of MB `(mb_x, mb_y)`.
fn block_layout(
    b: usize,
    mb_x: usize,
    mb_y: usize,
    pic: &PictureBuffer,
) -> (bool, usize, usize, usize, usize) {
    match b {
        0 => (false, 0, mb_x * 16, mb_y * 16, pic.y_stride),
        1 => (false, 0, mb_x * 16 + 8, mb_y * 16, pic.y_stride),
        2 => (false, 0, mb_x * 16, mb_y * 16 + 8, pic.y_stride),
        3 => (false, 0, mb_x * 16 + 8, mb_y * 16 + 8, pic.y_stride),
        4 => (true, 1, mb_x * 8, mb_y * 8, pic.c_stride),
        5 => (true, 2, mb_x * 8, mb_y * 8, pic.c_stride),
        _ => unreachable!(),
    }
}

/// Sanity guard used by the slice loop: I-pictures must have every MB
/// addressed (no skipping allowed per §6.3.17).  P/B pictures relax this.
pub fn check_skipped_mb(picture_type: PictureType, incr: u32, first: bool) -> Result<()> {
    if first || incr == 1 {
        return Ok(());
    }
    if matches!(picture_type, PictureType::I) {
        Err(Error::invalid(
            "I-picture: skipped macroblocks are not allowed",
        ))
    } else {
        Ok(())
    }
}
