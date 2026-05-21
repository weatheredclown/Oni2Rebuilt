//! Macroblock-level decode.
//!
//! Milestone 4 scope: intra macroblocks only.  Inter MBs (P/B with motion
//! compensation) come in milestone 5.

use crate::bitstream::BitReader;
use crate::motion;

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
    pub pmv: [[crate::motion::MvPredictor; 2]; 2],
    pub last_field_mc: [bool; 2],
    pub last_had_forward: bool,
    pub last_had_backward: bool,
}

impl SliceState {
    pub fn new(dc_reset: i32, quant_code: u8) -> Self {
        Self {
            dc_pred: [dc_reset; 3],
            quant_code,
            pmv: [[motion::MvPredictor::default(); 2]; 2],
            last_field_mc: [false; 2],
            last_had_forward: false,
            last_had_backward: false,
        }
    }

    pub fn reset_dir(&mut self, s: usize) {
        self.pmv[s][0].reset();
        self.pmv[s][1].reset();
        self.last_field_mc[s] = false;
    }
}

/// Decode the six 8×8 blocks of an intra macroblock at `(mb_x, mb_y)`.
///
/// The caller (`slice::decode_slice`) has already consumed the MB-type,
/// the optional `quantiser_scale_code` override, and the MPEG-2 side bits
/// (`frame_motion_type` / `dct_type`).  The caller supplies the decoded
/// `field_dct` flag so the luma blocks can be de-interleaved into top-
/// field / bottom-field rows on write (§6.1.3); chroma is unaffected in
/// 4:2:0.
#[allow(clippy::too_many_arguments)]
pub fn decode_intra_mb(
    br: &mut BitReader<'_>,
    state: &mut SliceState,
    pic: &mut PictureBuffer,
    params: &PictureParams,
    intra_quantiser: &[u8; 64],
    mb_x: usize,
    mb_y: usize,
    field_dct: bool,
) -> Result<()> {
    for b in 0..6usize {
        let (is_chroma, comp_idx, dst_x0, dst_y0, stride) = block_layout(b, mb_x, mb_y, pic);
        let (dst_x0, dst_y0, dst_stride) = if field_dct && !is_chroma {
            luma_block_layout_field_dct(b, mb_x, mb_y, pic.y_stride)
        } else {
            (dst_x0, dst_y0, stride)
        };
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
            dst_stride,
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

pub const DIR_FWD: usize = 0;
pub const DIR_BWD: usize = 1;
pub const AXIS_H: usize = 0;
pub const AXIS_V: usize = 1;

#[derive(Clone, Copy, Debug, Default)]
pub enum DirPrediction {
    #[default]
    None,
    Frame {
        mv: (i32, i32),
    },
    #[allow(dead_code)]
    Field {
        top_mv: (i32, i32),
        top_field_select: u8,
        bot_mv: (i32, i32),
        bot_field_select: u8,
    },
}

impl DirPrediction {
    pub fn frame_mv(&self) -> Option<(i32, i32)> {
        match *self {
            DirPrediction::Frame { mv } => Some(mv),
            _ => None,
        }
    }
    pub fn has_any(&self) -> bool {
        !matches!(self, DirPrediction::None)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mpeg2MotionType {
    #[default]
    Frame,
    Field,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Mpeg2MacroblockModes {
    pub motion_type: Mpeg2MotionType,
    pub field_dct: bool,
}

pub fn parse_mpeg2_macroblock_modes(
    br: &mut BitReader<'_>,
    params: &PictureParams,
    mb_type_flags: mb_type::MbTypeFlags,
) -> Result<Mpeg2MacroblockModes> {
    let mut modes = Mpeg2MacroblockModes::default();
    if params.coding.frame_pred_frame_dct {
        return Ok(modes);
    }

    if mb_type_flags.motion_forward || mb_type_flags.motion_backward {
        let frame_motion_type = br.read(2)? as u8;
        match frame_motion_type {
            2 => modes.motion_type = Mpeg2MotionType::Frame,
            1 => modes.motion_type = Mpeg2MotionType::Field,
            3 => {
                return Err(Error::unsupported(
                    "mpeg2video: dual-prime frame macroblocks not supported",
                ));
            }
            _ => return Err(Error::invalid("mpeg2video: invalid frame_motion_type=0")),
        }
    }

    if mb_type_flags.pattern || mb_type_flags.intra {
        modes.field_dct = br.read(1)? != 0;
    }

    Ok(modes)
}

#[allow(clippy::too_many_arguments)]
pub fn parse_direction_mvs(
    br: &mut BitReader<'_>,
    picture_type: PictureType,
    has_motion: bool,
    is_intra: bool,
    motion_type: Mpeg2MotionType,
    f_code_pair: [u8; 2],
    full_pel: bool,
    s: usize,
    state: &mut SliceState,
) -> Result<DirPrediction> {
    let f_code_h = f_code_pair[AXIS_H];
    let f_code_v = f_code_pair[AXIS_V];

    if !has_motion {
        if matches!(picture_type, PictureType::P) && !is_intra && s == DIR_FWD {
            state.reset_dir(DIR_FWD);
            return Ok(DirPrediction::Frame { mv: (0, 0) });
        }
        return Ok(DirPrediction::None);
    }

    match motion_type {
        Mpeg2MotionType::Frame => {
            if state.last_field_mc[s] {
                state.pmv[s][0].y = state.pmv[s][0].y * 2;
            }
            let mx = crate::motion::decode_motion_component(
                br,
                f_code_h,
                full_pel,
                &mut state.pmv[s][0].x,
            )?;
            let my = crate::motion::decode_motion_component(
                br,
                f_code_v,
                full_pel,
                &mut state.pmv[s][0].y,
            )?;
            state.pmv[s][1] = state.pmv[s][0];
            state.last_field_mc[s] = false;
            Ok(DirPrediction::Frame { mv: (mx, my) })
        }
        Mpeg2MotionType::Field => {
            if !state.last_field_mc[s] {
                state.pmv[s][0].y >>= 1;
                state.pmv[s][1].y >>= 1;
            }
            let top_field_select = br.read(1)? as u8;
            let mx0 = crate::motion::decode_motion_component(
                br,
                f_code_h,
                full_pel,
                &mut state.pmv[s][0].x,
            )?;
            let my0 = crate::motion::decode_motion_component(
                br,
                f_code_v,
                full_pel,
                &mut state.pmv[s][0].y,
            )?;
            let bot_field_select = br.read(1)? as u8;
            let mx1 = crate::motion::decode_motion_component(
                br,
                f_code_h,
                full_pel,
                &mut state.pmv[s][1].x,
            )?;
            let my1 = crate::motion::decode_motion_component(
                br,
                f_code_v,
                full_pel,
                &mut state.pmv[s][1].y,
            )?;
            state.last_field_mc[s] = true;
            Ok(DirPrediction::Field {
                top_mv: (mx0, my0),
                top_field_select,
                bot_mv: (mx1, my1),
                bot_field_select,
            })
        }
    }
}

pub fn luma_block_layout_field_dct(
    b: usize,
    mb_x: usize,
    mb_y: usize,
    y_stride: usize,
) -> (usize, usize, usize) {
    let x_off = if b & 1 == 0 { 0 } else { 8 };
    let y_off = if b < 2 { 0 } else { 1 };
    (mb_x * 16 + x_off, mb_y * 16 + y_off, y_stride * 2)
}

#[allow(clippy::too_many_arguments)]
pub fn decode_inter_mb(
    br: &mut BitReader<'_>,
    state: &mut SliceState,
    pic: &mut PictureBuffer,
    fwd_ref: Option<&PictureBuffer>,
    bwd_ref: Option<&PictureBuffer>,
    params: &PictureParams,
    non_intra_quantiser: &[u8; 64],
    mb_x: usize,
    mb_y: usize,
    fwd_pred: DirPrediction,
    bwd_pred: DirPrediction,
    cbp_bits: u8,
    field_dct: bool,
) -> Result<()> {
    let mut pred_y = [0u8; 16 * 16];
    let mut pred_cb = [0u8; 8 * 8];
    let mut pred_cr = [0u8; 8 * 8];

    build_dir_prediction(
        fwd_ref,
        bwd_ref,
        mb_x,
        mb_y,
        fwd_pred,
        bwd_pred,
        &mut pred_y,
        &mut pred_cb,
        &mut pred_cr,
    )?;

    for b in 0..6usize {
        let (is_chroma, comp_idx, dst_x0, dst_y0, stride_ptr) = block_layout(b, mb_x, mb_y, pic);
        let coded = (cbp_bits & (1 << (5 - b))) != 0;

        let (pred_slice, pred_stride): (&[u8], usize) = if field_dct && !is_chroma {
            match b {
                0 => (&pred_y[0..], 16 * 2),
                1 => (&pred_y[8..], 16 * 2),
                2 => (&pred_y[16..], 16 * 2),
                3 => (&pred_y[16 + 8..], 16 * 2),
                _ => unreachable!(),
            }
        } else {
            match b {
                0 => (&pred_y[0..], 16),
                1 => (&pred_y[8..], 16),
                2 => (&pred_y[16 * 8..], 16),
                3 => (&pred_y[16 * 8 + 8..], 16),
                4 => (&pred_cb[..], 8),
                5 => (&pred_cr[..], 8),
                _ => unreachable!(),
            }
        };

        let (dst_x0, dst_y0, dst_stride) = if field_dct && !is_chroma {
            luma_block_layout_field_dct(b, mb_x, mb_y, pic.y_stride)
        } else {
            (dst_x0, dst_y0, stride_ptr)
        };

        let buf: &mut [u8] = match comp_idx {
            0 => &mut pic.y[..],
            1 => &mut pic.cb[..],
            _ => &mut pic.cr[..],
        };
        let sub = &mut buf[dst_y0 * stride_ptr + dst_x0..];

        if coded {
            crate::block::decode_non_intra_block(
                br,
                state.quant_code,
                non_intra_quantiser,
                params,
                pred_slice,
                pred_stride,
                sub,
                dst_stride,
            )?;
        } else {
            crate::block::copy_prediction(pred_slice, pred_stride, 8, sub, dst_stride);
        }
    }
    Ok(())
}

/// Per-direction MC that handles both frame-MC and field-MC and writes
/// directly into spatial-order planes (`out_y` is 16×16 row-major, `out_cb`
/// / `out_cr` are 8×8).  Returns `Ok(true)` if the direction had motion,
/// `Ok(false)` if it was `DirPrediction::None`.
#[allow(clippy::too_many_arguments)]
fn build_one_direction_prediction(
    pred: DirPrediction,
    refp: &PictureBuffer,
    mb_x: usize,
    mb_y: usize,
    out_y: &mut [u8; 16 * 16],
    out_cb: &mut [u8; 8 * 8],
    out_cr: &mut [u8; 8 * 8],
) -> bool {
    match pred {
        DirPrediction::None => false,
        DirPrediction::Frame { mv: (mx, my) } => {
            mc_mb(refp, mb_x, mb_y, mx, my, out_y, out_cb, out_cr);
            true
        }
        DirPrediction::Field {
            top_mv: (top_mx, top_my),
            top_field_select,
            bot_mv: (bot_mx, bot_my),
            bot_field_select,
        } => {
            // Luma: two 16×8 reads, top into rows 0,2,…,14 (stride 32 from
            // pred_y[0]); bottom into rows 1,3,…,15 (stride 32 from
            // pred_y[16]).  Vertical MV is in half-FIELD-LINE units; the
            // field-aware predictor reads with doubled reference stride.
            let mb_px = (mb_x * 16) as i32;
            let mb_py = (mb_y * 16) as i32;
            let ref_w = refp.display_width as i32;
            let ref_h = refp.display_height as i32;

            crate::motion::predict_field_half(
                &refp.y,
                refp.y_stride,
                ref_w,
                ref_h,
                mb_px,
                mb_py,
                top_mx,
                top_my,
                top_field_select,
                16,
                8,
                &mut out_y[0..],
                32,
            );
            crate::motion::predict_field_half(
                &refp.y,
                refp.y_stride,
                ref_w,
                ref_h,
                mb_px,
                mb_py,
                bot_mx,
                bot_my,
                bot_field_select,
                16,
                8,
                &mut out_y[16..],
                32,
            );

            // Chroma: 4:2:0 → 8 wide × 4 tall per field half.  MV
            // components halve (luma → chroma half-pel scaling).  mb base
            // in chroma coords is mb_x*8, mb_y*8 (frame rows).
            let c_px = (mb_x * 8) as i32;
            let c_py = (mb_y * 8) as i32;
            let c_w = (refp.display_width / 2) as i32;
            let c_h = (refp.display_height / 2) as i32;
            let top_cx = crate::motion::scale_mv_to_chroma(top_mx);
            let top_cy = crate::motion::scale_mv_to_chroma(top_my);
            let bot_cx = crate::motion::scale_mv_to_chroma(bot_mx);
            let bot_cy = crate::motion::scale_mv_to_chroma(bot_my);

            crate::motion::predict_field_half(
                &refp.cb,
                refp.c_stride,
                c_w,
                c_h,
                c_px,
                c_py,
                top_cx,
                top_cy,
                top_field_select,
                8,
                4,
                &mut out_cb[0..],
                16,
            );
            crate::motion::predict_field_half(
                &refp.cb,
                refp.c_stride,
                c_w,
                c_h,
                c_px,
                c_py,
                bot_cx,
                bot_cy,
                bot_field_select,
                8,
                4,
                &mut out_cb[8..],
                16,
            );
            crate::motion::predict_field_half(
                &refp.cr,
                refp.c_stride,
                c_w,
                c_h,
                c_px,
                c_py,
                top_cx,
                top_cy,
                top_field_select,
                8,
                4,
                &mut out_cr[0..],
                16,
            );
            crate::motion::predict_field_half(
                &refp.cr,
                refp.c_stride,
                c_w,
                c_h,
                c_px,
                c_py,
                bot_cx,
                bot_cy,
                bot_field_select,
                8,
                4,
                &mut out_cr[8..],
                16,
            );
            true
        }
    }
}

/// Build the MB-level prediction for one or two motion-compensation
/// directions, handling frame-MC and field-MC uniformly.  Bidirectional
/// prediction averages the two contributions with `(a + b + 1) >> 1`.
#[allow(clippy::too_many_arguments)]
pub fn build_dir_prediction(
    fwd_ref: Option<&PictureBuffer>,
    bwd_ref: Option<&PictureBuffer>,
    mb_x: usize,
    mb_y: usize,
    fwd_pred: DirPrediction,
    bwd_pred: DirPrediction,
    pred_y: &mut [u8; 16 * 16],
    pred_cb: &mut [u8; 8 * 8],
    pred_cr: &mut [u8; 8 * 8],
) -> Result<()> {
    let mut fwd_y = [0u8; 16 * 16];
    let mut fwd_cb = [0u8; 8 * 8];
    let mut fwd_cr = [0u8; 8 * 8];
    let mut bwd_y = [0u8; 16 * 16];
    let mut bwd_cb = [0u8; 8 * 8];
    let mut bwd_cr = [0u8; 8 * 8];

    let have_fwd = if matches!(fwd_pred, DirPrediction::None) {
        false
    } else {
        let refp = fwd_ref.ok_or_else(|| {
            Error::invalid("forward prediction requested without forward reference")
        })?;
        build_one_direction_prediction(
            fwd_pred,
            refp,
            mb_x,
            mb_y,
            &mut fwd_y,
            &mut fwd_cb,
            &mut fwd_cr,
        )
    };
    let have_bwd = if matches!(bwd_pred, DirPrediction::None) {
        false
    } else {
        let refp = bwd_ref.ok_or_else(|| {
            Error::invalid("backward prediction requested without backward reference")
        })?;
        build_one_direction_prediction(
            bwd_pred,
            refp,
            mb_x,
            mb_y,
            &mut bwd_y,
            &mut bwd_cb,
            &mut bwd_cr,
        )
    };

    match (have_fwd, have_bwd) {
        (true, false) => {
            pred_y.copy_from_slice(&fwd_y);
            pred_cb.copy_from_slice(&fwd_cb);
            pred_cr.copy_from_slice(&fwd_cr);
        }
        (false, true) => {
            pred_y.copy_from_slice(&bwd_y);
            pred_cb.copy_from_slice(&bwd_cb);
            pred_cr.copy_from_slice(&bwd_cr);
        }
        (true, true) => {
            for i in 0..16 * 16 {
                pred_y[i] = ((fwd_y[i] as u32 + bwd_y[i] as u32 + 1) >> 1) as u8;
            }
            for i in 0..8 * 8 {
                pred_cb[i] = ((fwd_cb[i] as u32 + bwd_cb[i] as u32 + 1) >> 1) as u8;
                pred_cr[i] = ((fwd_cr[i] as u32 + bwd_cr[i] as u32 + 1) >> 1) as u8;
            }
        }
        (false, false) => {
            return Err(Error::invalid(
                "inter MB with neither fwd nor bwd prediction",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn build_prediction(
    fwd_ref: Option<&PictureBuffer>,
    bwd_ref: Option<&PictureBuffer>,
    mb_x: usize,
    mb_y: usize,
    fwd_mv: Option<(i32, i32)>,
    bwd_mv: Option<(i32, i32)>,
    pred_y: &mut [u8; 16 * 16],
    pred_cb: &mut [u8; 8 * 8],
    pred_cr: &mut [u8; 8 * 8],
) -> Result<()> {
    let mut have_fwd = false;
    let mut have_bwd = false;
    let mut fwd_y = [0u8; 16 * 16];
    let mut fwd_cb = [0u8; 8 * 8];
    let mut fwd_cr = [0u8; 8 * 8];
    let mut bwd_y = [0u8; 16 * 16];
    let mut bwd_cb = [0u8; 8 * 8];
    let mut bwd_cr = [0u8; 8 * 8];

    if let Some((mx, my)) = fwd_mv {
        let refp = fwd_ref.ok_or_else(|| Error::invalid("forward MV without forward reference"))?;
        mc_mb(
            refp,
            mb_x,
            mb_y,
            mx,
            my,
            &mut fwd_y,
            &mut fwd_cb,
            &mut fwd_cr,
        );
        have_fwd = true;
    }
    if let Some((mx, my)) = bwd_mv {
        let refp =
            bwd_ref.ok_or_else(|| Error::invalid("backward MV without backward reference"))?;
        mc_mb(
            refp,
            mb_x,
            mb_y,
            mx,
            my,
            &mut bwd_y,
            &mut bwd_cb,
            &mut bwd_cr,
        );
        have_bwd = true;
    }

    match (have_fwd, have_bwd) {
        (true, false) => {
            pred_y.copy_from_slice(&fwd_y);
            pred_cb.copy_from_slice(&fwd_cb);
            pred_cr.copy_from_slice(&fwd_cr);
        }
        (false, true) => {
            pred_y.copy_from_slice(&bwd_y);
            pred_cb.copy_from_slice(&bwd_cb);
            pred_cr.copy_from_slice(&bwd_cr);
        }
        (true, true) => {
            for i in 0..16 * 16 {
                pred_y[i] = ((fwd_y[i] as u32 + bwd_y[i] as u32 + 1) >> 1) as u8;
            }
            for i in 0..8 * 8 {
                pred_cb[i] = ((fwd_cb[i] as u32 + bwd_cb[i] as u32 + 1) >> 1) as u8;
            }
            for i in 0..8 * 8 {
                pred_cr[i] = ((fwd_cr[i] as u32 + bwd_cr[i] as u32 + 1) >> 1) as u8;
            }
        }
        (false, false) => return Err(Error::invalid("inter MB with neither fwd nor bwd MV")),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn mc_mb(
    refp: &PictureBuffer,
    mb_x: usize,
    mb_y: usize,
    mv_x: i32,
    mv_y: i32,
    dst_y: &mut [u8; 16 * 16],
    dst_cb: &mut [u8; 8 * 8],
    dst_cr: &mut [u8; 8 * 8],
) {
    let mb_px = (mb_x * 16) as i32;
    let mb_py = (mb_y * 16) as i32;
    crate::motion::predict_block(
        &refp.y,
        refp.y_stride,
        refp.display_width as i32,
        refp.display_height as i32,
        mb_px,
        mb_py,
        mv_x,
        mv_y,
        16,
        dst_y,
        16,
    );
    let c_px = (mb_x * 8) as i32;
    let c_py = (mb_y * 8) as i32;
    let mv_cx = crate::motion::scale_mv_to_chroma(mv_x);
    let mv_cy = crate::motion::scale_mv_to_chroma(mv_y);
    crate::motion::predict_block(
        &refp.cb,
        refp.c_stride,
        (refp.display_width / 2) as i32,
        (refp.display_height / 2) as i32,
        c_px,
        c_py,
        mv_cx,
        mv_cy,
        8,
        dst_cb,
        8,
    );
    crate::motion::predict_block(
        &refp.cr,
        refp.c_stride,
        (refp.display_width / 2) as i32,
        (refp.display_height / 2) as i32,
        c_px,
        c_py,
        mv_cx,
        mv_cy,
        8,
        dst_cr,
        8,
    );
}

pub fn fill_forward_predict(
    pic: &mut PictureBuffer,
    fwd_ref: Option<&PictureBuffer>,
    mb_x: usize,
    mb_y: usize,
    mv_x: i32,
    mv_y: i32,
) -> Result<()> {
    let refp = fwd_ref.ok_or_else(|| Error::invalid("skip MB without forward ref"))?;
    let mut y = [0u8; 16 * 16];
    let mut cb = [0u8; 8 * 8];
    let mut cr = [0u8; 8 * 8];
    mc_mb(refp, mb_x, mb_y, mv_x, mv_y, &mut y, &mut cb, &mut cr);
    write_mb(pic, mb_x, mb_y, &y, &cb, &cr);
    Ok(())
}

pub fn fill_bidir_predict(
    pic: &mut PictureBuffer,
    fwd_ref: Option<&PictureBuffer>,
    bwd_ref: Option<&PictureBuffer>,
    mb_x: usize,
    mb_y: usize,
    fwd_mv: Option<(i32, i32)>,
    bwd_mv: Option<(i32, i32)>,
) -> Result<()> {
    let mut y = [0u8; 16 * 16];
    let mut cb = [0u8; 8 * 8];
    let mut cr = [0u8; 8 * 8];
    build_prediction(
        fwd_ref, bwd_ref, mb_x, mb_y, fwd_mv, bwd_mv, &mut y, &mut cb, &mut cr,
    )?;
    write_mb(pic, mb_x, mb_y, &y, &cb, &cr);
    Ok(())
}

pub fn write_mb(
    pic: &mut PictureBuffer,
    mb_x: usize,
    mb_y: usize,
    y: &[u8; 16 * 16],
    cb: &[u8; 8 * 8],
    cr: &[u8; 8 * 8],
) {
    let ys = pic.y_stride;
    let cs = pic.c_stride;
    for j in 0..16 {
        let off = (mb_y * 16 + j) * ys + mb_x * 16;
        pic.y[off..off + 16].copy_from_slice(&y[j * 16..j * 16 + 16]);
    }
    for j in 0..8 {
        let off = (mb_y * 8 + j) * cs + mb_x * 8;
        pic.cb[off..off + 8].copy_from_slice(&cb[j * 8..j * 8 + 8]);
        pic.cr[off..off + 8].copy_from_slice(&cr[j * 8..j * 8 + 8]);
    }
}

#[cfg(test)]
mod inter_tests {
    use super::*;

    #[test]
    fn test_dir_prediction_has_any() {
        let none = DirPrediction::None;
        assert!(!none.has_any());
        let frame = DirPrediction::Frame { mv: (1, 1) };
        assert!(frame.has_any());
    }
}
