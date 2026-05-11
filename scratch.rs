enum DirPrediction {
    /// No motion in this direction (intra MB, or no fwd/bwd flag).
    #[default]
    None,
    /// Single MV pair.  Used for frame-MC and for P-skip / B-skip
    /// inheritance, where the inherited MV is always frame-mode (vertical
    /// in half-frame-pel).
    Frame { mv: (i32, i32) },
    /// Two MV pairs with field selects.  `top_mv` predicts the top-field
    /// rows of the destination MB; `bot_mv` predicts the bottom-field rows.
    /// `top_field_select` / `bot_field_select` choose which field of the
    /// reference frame each MV reads from.  Vertical MV components are in
    /// half-FIELD-LINE units.  Fields are populated by the parse layer but
    /// the prediction stage that consumes them isn't wired yet — see
    /// `decode_inter_mb`.
    #[allow(dead_code)]
    Field {
        top_mv: (i32, i32),
        top_field_select: u8,
        bot_mv: (i32, i32),
        bot_field_select: u8,
    },
}

impl DirPrediction {
    /// Frame-MC view for the legacy build_prediction path and B-skip
    /// inheritance.  Field-MC entries collapse to `None` here — they go
    /// through `apply_field_mc` instead.
    fn frame_mv(&self) -> Option<(i32, i32)> {
        match *self {
            DirPrediction::Frame { mv } => Some(mv),
            _ => None,
        }
    }

    #[allow(dead_code)]
    fn has_any(&self) -> bool {
        !matches!(self, DirPrediction::None)
    }
}

/// Parse the MV(s) for one direction and update the appropriate predictor
/// pair(s) in `SliceState`.  Handles frame-MC, field-MC, and the P-only
/// "no-MC inter MB" implicit zero MV.
#[allow(clippy::too_many_arguments)]
fn parse_direction_mvs(
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
        // Implicit zero MV for P MBs without `motion_forward` (P frame
        // inter MB with no MC).  All other no-motion cases fall through.
        if matches!(picture_type, PictureType::P) && !is_intra && s == DIR_FWD {
            state.reset_dir(DIR_FWD);
            return Ok(DirPrediction::Frame { mv: (0, 0) });
        }
        return Ok(DirPrediction::None);
    }

    match motion_type {
        Mpeg2MotionType::Frame => {
            // Single MV pair, vertical in half-frame-pel units.  If the
            // last update for this direction was field-MC, the stored
            // predictor's vertical is in half-field-line units; double it
            // back to frame units before use (§7.6.3.6).
            if state.last_field_mc[s] {
                state.pmv[s][0].y = state.pmv[s][0].y * 2;
            }
            let mx = motion::decode_motion_component(
                br,
                f_code_h,
                full_pel,
                &mut state.pmv[s][0].x,
            )?;
            let my = motion::decode_motion_component(
                br,
                f_code_v,
                full_pel,
                &mut state.pmv[s][0].y,
            )?;
            // Frame MC: PMV[1][s] is reset to be equal to PMV[0][s].
            state.pmv[s][1] = state.pmv[s][0];
            state.last_field_mc[s] = false;
            Ok(DirPrediction::Frame { mv: (mx, my) })
        }
        Mpeg2MotionType::Field => {
            // Two MV pairs, each preceded by a 1-bit field_select.
            // Vertical predictors must be in half-field-line units; if the
            // last update was frame-MC, halve the stored value first
            // (§7.6.3.6).  Halving uses integer floor division (i32 >> 1
            // performs arithmetic shift; for the negative case this is
            // toward -infinity which matches libavcodec).
            if !state.last_field_mc[s] {
                state.pmv[s][0].y >>= 1;
                state.pmv[s][1].y >>= 1;
            }
            let top_field_select = br.read(1)? as u8;
            let mx0 = motion::decode_motion_component(
                br,
                f_code_h,
                full_pel,
                &mut state.pmv[s][0].x,
            )?;
            let my0 = motion::decode_motion_component(
                br,
                f_code_v,
                full_pel,
                &mut state.pmv[s][0].y,
            )?;
            let bot_field_select = br.read(1)? as u8;
            let mx1 = motion::decode_motion_component(
                br,
                f_code_h,
                full_pel,
                &mut state.pmv[s][1].x,
            )?;
            let my1 = motion::decode_motion_component(
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

/// Macroblock motion-compensation mode, per H.262 §6.3.17.1 Table 6-17.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mpeg2MotionType {
    /// Frame-MC (`frame_motion_type = 0b10`): one MV per direction, vertical
    /// component in half-frame-pel units.
    #[default]
    Frame,
    /// Field-MC (`frame_motion_type = 0b01`): two MV pairs per direction
    /// with `motion_vertical_field_select` bits; one MV predicts the
    /// top-field rows of the MB, the other predicts the bottom-field rows.
    /// Vertical components are in half-FIELD-LINE units (§7.6.4).
    Field,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Mpeg2MacroblockModes {
    /// Motion-compensation type for this MB.  Always `Frame` when
    /// `frame_pred_frame_dct = 1` or when the MB has no motion.
    motion_type: Mpeg2MotionType,
    /// MPEG-2 `dct_type` side bit: false = frame DCT, true = field DCT.
    field_dct: bool,
}

fn parse_mpeg2_macroblock_modes(
    br: &mut BitReader<'_>,
    params: &PictureParams,
    mb_type_flags: mb_type::MbTypeFlags,
) -> Result<Mpeg2MacroblockModes> {
    let mut modes = Mpeg2MacroblockModes::default();
    if !true || params.coding.frame_pred_frame_dct {
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

/// Block layout for a single 8×8 block of an MB.  Returns
/// `(is_chroma, comp_idx, dst_x0, dst_y0, stride)`.  When `field_dct` is true,
/// the four luma blocks (0..=3) are organised as field-DCT: blocks 0/1 carry
/// the top-field rows (0,2,…,14) and blocks 2/3 carry the bottom-field rows
/// (1,3,…,15).  The output stride doubles so the IDCT's 8 contiguous rows
/// land on every-other picture row; chroma is unaffected in 4:2:0 (§6.1.3).
fn luma_block_layout_field_dct(
    b: usize,
    mb_x: usize,
    mb_y: usize,
    y_stride: usize,
) -> (usize, usize, usize) {
    // Returns (dst_x0, dst_y0, stride).
    let x_off = if b & 1 == 0 { 0 } else { 8 };
    let y_off = if b < 2 { 0 } else { 1 };
    (mb_x * 16 + x_off, mb_y * 16 + y_off, y_stride * 2)
}

/// Decode one slice.
#[allow(clippy::too_many_arguments)]
fn decode_inter_mb(
    br: &mut BitReader<'_>,
    seq: &SequenceHeader,
    params: &PictureParams,
    _state: &mut SliceState,
    pic: &mut PictureBuffer,
    fwd_ref: Option<&PictureBuffer>,
    bwd_ref: Option<&PictureBuffer>,
    mb_x: usize,
    mb_y: usize,
    quant_scale: u8,
    fwd_pred: DirPrediction,
    bwd_pred: DirPrediction,
    cbp_bits: u8,
    field_dct: bool,
) -> Result<()> {
    // Build 16x16 luma + 8x8 Cb + 8x8 Cr prediction.
    let mut pred_y = [0u8; 16 * 16];
    let mut pred_cb = [0u8; 8 * 8];
    let mut pred_cr = [0u8; 8 * 8];

    // Field-MC prediction is not yet wired through `build_prediction`.  The
    // MV parsing layer (including `frame_motion_type=01` two-MV-pair side
    // bits and predictor unit conversion) is in and tested; the remaining
    // work is the field-aware prediction read in `motion::predict_field_half`
    // plus the chroma-MV scaling for 4:2:0.  Until that lands we return a
    // clear unsupported error at the prediction stage so the bitstream
    // alignment of in-progress slices doesn't desync.
    if matches!(fwd_pred, DirPrediction::Field { .. })
        || matches!(bwd_pred, DirPrediction::Field { .. })
    {
        return Err(Error::unsupported(
            "mpeg2video: field-MC frame macroblock prediction not implemented (parsing done)",
        ));
    }

    let fwd_mv = fwd_pred.frame_mv();
    let bwd_mv = bwd_pred.frame_mv();

    build_prediction(
        fwd_ref,
        bwd_ref,
        mb_x,
        mb_y,
        fwd_mv,
        bwd_mv,
        &mut pred_y,
        &mut pred_cb,
        &mut pred_cr,
    )?;

    // For each block, either decode non-intra residual + add, or just copy
    // prediction.
    for b in 0..6usize {
        let (is_chroma, comp_idx, dst_x0, dst_y0, stride_ptr) = block_layout(b, mb_x, mb_y, pic);

        // CBP order per §2.4.3.6: bits 5..0 map to blocks (Y0,Y1,Y2,Y3,Cb,Cr).
        // Bit 5 (0x20) = block 0; bit 0 (0x01) = block 5.
        let coded = (cbp_bits & (1 << (5 - b))) != 0;

        // Frame-DCT luma: blocks 0/1 = top half rows 0..=7, blocks 2/3 =
        // bottom half rows 8..=15.  Field-DCT luma: blocks 0/1 = top-field
        // rows 0,2,…,14, blocks 2/3 = bottom-field rows 1,3,…,15 — both
        // the prediction slice AND the picture destination need the
        // de-interleaved layout, otherwise the residual lands on the
        // wrong rows.  Chroma is frame-organised regardless.
        let (pred_slice, pred_stride): (&[u8], usize) = if field_dct && !is_chroma {
            match b {
                0 => (&pred_y[0..], 16 * 2),
                1 => (&pred_y[8..], 16 * 2),
                2 => (&pred_y[16..], 16 * 2),       // row 1 = byte 16
                3 => (&pred_y[16 + 8..], 16 * 2),   // row 1, col 8
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
                quant_scale,
                &seq.non_intra_quantiser,
                params,
                pred_slice,
                pred_stride,
                sub,
                dst_stride,
            )?;
        } else {
            let _ = is_chroma;
            crate::block::copy_prediction(pred_slice, pred_stride, 8, sub, dst_stride);
        }
    }
    Ok(())
}

/// Returns (is_chroma, component_index, dst_x0, dst_y0, plane_stride).
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

/// Build 16x16 luma + 8x8 chroma prediction buffers. Handles forward-only,
/// backward-only, or interpolated (average) prediction.
#[allow(clippy::too_many_arguments)]
fn build_prediction(
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
    // Collect the 1 or 2 motion-compensated reference patches.
    let mut have_fwd = false;
    let mut have_bwd = false;
    let mut fwd_y = [0u8; 16 * 16];
    let mut fwd_cb = [0u8; 8 * 8];
    let mut fwd_cr = [0u8; 8 * 8];
    let mut bwd_y = [0u8; 16 * 16];
    let mut bwd_cb = [0u8; 8 * 8];
    let mut bwd_cr = [0u8; 8 * 8];

    if let Some((mx, my)) = fwd_mv {
        let Some(refp) = fwd_ref else {
            return Err(Error::invalid("forward MV without forward reference"));
        };
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
        let Some(refp) = bwd_ref else {
            return Err(Error::invalid("backward MV without backward reference"));
        };
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
        (false, false) => {
            return Err(Error::invalid(
                "inter MB with neither forward nor backward MV",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn mc_mb(
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
    motion::predict_block(
        &refp.y,
        refp.y_stride,
        refp.y_stride as i32,
        (refp.y.len() / refp.y_stride) as i32,
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
    let mv_cx = motion::scale_mv_to_chroma(mv_x);
    let mv_cy = motion::scale_mv_to_chroma(mv_y);
    motion::predict_block(
        &refp.cb,
        refp.c_stride,
        refp.c_stride as i32,
        (refp.cb.len() / refp.c_stride) as i32,
        c_px,
        c_py,
        mv_cx,
        mv_cy,
        8,
        dst_cb,
        8,
    );
    motion::predict_block(
        &refp.cr,
        refp.c_stride,
        refp.c_stride as i32,
        (refp.cr.len() / refp.c_stride) as i32,
        c_px,
        c_py,
        mv_cx,
        mv_cy,
        8,
        dst_cr,
        8,
    );
}

fn fill_forward_predict(
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

fn fill_bidir_predict(
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

fn write_mb(
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
mod tests {
    use super::*;
    use crate::coding_mode::Codec;
    use oxideav_core::bits::BitWriter;

    fn params(frame_pred_frame_dct: bool) -> PictureParams {
        PictureParams {
            codec: Codec::Mpeg2,
            intra_dc_precision: 0,
            alternate_scan: false,
            intra_vlc_format: false,
            q_scale_type: false,
            frame_pred_frame_dct,
            f_code: [[1, 1], [1, 1]],
            full_pel_fwd: false,
            full_pel_bwd: false,
        }
    }

    fn bits(entries: &[(u32, u32)]) -> Vec<u8> {
        let mut bw = BitWriter::new();
        for &(value, len) in entries {
            bw.write_bits(value, len);
        }
        bw.align_to_byte();
        bw.finish()
    }

    #[test]
    fn parses_frame_pred_zero_frame_motion_and_frame_dct_side_bits() {
        // frame_motion_type = 10 (frame MC), dct_type = 0 (frame DCT).
        let data = bits(&[(0b10, 2), (0, 1)]);
        let mut br = BitReader::new(&data);
        let modes = parse_mpeg2_macroblock_modes(
            &mut br,
            &params(false),
            mb_type::MbTypeFlags::new(false, true, false, true, false),
        )
        .unwrap();

        assert_eq!(modes.motion_type, Mpeg2MotionType::Frame);
        assert!(!modes.field_dct);
    }

    #[test]
    fn parses_field_mc_macroblock_without_rejecting() {
        // frame_motion_type = 01 (field MC).  Now accepted — the slice
        // decoder reads two MV pairs with intervening field_select bits.
        let data = bits(&[(0b01, 2)]);
        let mut br = BitReader::new(&data);
        let modes = parse_mpeg2_macroblock_modes(
            &mut br,
            &params(false),
            mb_type::MbTypeFlags::new(false, true, false, false, false),
        )
        .unwrap();

        assert_eq!(modes.motion_type, Mpeg2MotionType::Field);
    }

    #[test]
    fn parses_field_dct_bit_without_rejecting() {
        // dct_type = 1 (field DCT) on a coded no-motion macroblock.  Now
        // accepted — the MB-level decoder de-interleaves luma rows when
        // this flag is set.
        let data = bits(&[(1, 1)]);
        let mut br = BitReader::new(&data);
        let modes = parse_mpeg2_macroblock_modes(
            &mut br,
            &params(false),
            mb_type::MbTypeFlags::new(false, false, false, true, false),
        )
        .unwrap();

        assert!(modes.field_dct);
        assert_eq!(modes.motion_type, Mpeg2MotionType::Frame);
    }

    #[test]
    fn field_dct_luma_layout_top_field_blocks() {
        // Blocks 0 and 1 carry top-field rows (0,2,…) of left/right halves.
        let (x0, y0, stride) = luma_block_layout_field_dct(0, /*mb_x=*/ 2, /*mb_y=*/ 3, 256);
        assert_eq!((x0, y0, stride), (2 * 16, 3 * 16, 512));
        let (x1, y1, _) = luma_block_layout_field_dct(1, 2, 3, 256);
        assert_eq!((x1, y1), (2 * 16 + 8, 3 * 16));
    }

    #[test]
    fn field_dct_luma_layout_bottom_field_blocks() {
        // Blocks 2 and 3 carry bottom-field rows (1,3,…) — start at row+1.
        let (x2, y2, stride) = luma_block_layout_field_dct(2, /*mb_x=*/ 2, /*mb_y=*/ 3, 256);
        assert_eq!((x2, y2, stride), (2 * 16, 3 * 16 + 1, 512));
        let (x3, y3, _) = luma_block_layout_field_dct(3, 2, 3, 256);
        assert_eq!((x3, y3), (2 * 16 + 8, 3 * 16 + 1));
    }

    #[test]
    fn skips_side_bits_when_frame_pred_frame_dct_is_set() {
        let data = bits(&[(0b01, 2), (1, 1)]);
        let mut br = BitReader::new(&data);
        let modes = parse_mpeg2_macroblock_modes(
            &mut br,
            &params(true),
            mb_type::MbTypeFlags::new(false, true, false, true, false),
        )
        .unwrap();

        assert_eq!(modes, Mpeg2MacroblockModes::default());
        assert_eq!(br.bits_remaining(), 8);
    }
}
fn decode_inter_mb(
    br: &mut BitReader<'_>,
    seq: &SequenceHeader,
    params: &PictureParams,
    _state: &mut SliceState,
    pic: &mut PictureBuffer,
    fwd_ref: Option<&PictureBuffer>,
    bwd_ref: Option<&PictureBuffer>,
    mb_x: usize,
    mb_y: usize,
    quant_scale: u8,
    fwd_pred: DirPrediction,
    bwd_pred: DirPrediction,
    cbp_bits: u8,
    field_dct: bool,
) -> Result<()> {
    // Build 16x16 luma + 8x8 Cb + 8x8 Cr prediction.
    let mut pred_y = [0u8; 16 * 16];
    let mut pred_cb = [0u8; 8 * 8];
    let mut pred_cr = [0u8; 8 * 8];

    // Field-MC prediction is not yet wired through `build_prediction`.  The
    // MV parsing layer (including `frame_motion_type=01` two-MV-pair side
    // bits and predictor unit conversion) is in and tested; the remaining
    // work is the field-aware prediction read in `motion::predict_field_half`
    // plus the chroma-MV scaling for 4:2:0.  Until that lands we return a
    // clear unsupported error at the prediction stage so the bitstream
    // alignment of in-progress slices doesn't desync.
    if matches!(fwd_pred, DirPrediction::Field { .. })
        || matches!(bwd_pred, DirPrediction::Field { .. })
    {
        return Err(Error::unsupported(
            "mpeg2video: field-MC frame macroblock prediction not implemented (parsing done)",
        ));
    }

    let fwd_mv = fwd_pred.frame_mv();
    let bwd_mv = bwd_pred.frame_mv();

    build_prediction(
        fwd_ref,
        bwd_ref,
        mb_x,
        mb_y,
        fwd_mv,
        bwd_mv,
        &mut pred_y,
        &mut pred_cb,
        &mut pred_cr,
    )?;

    // For each block, either decode non-intra residual + add, or just copy
    // prediction.
    for b in 0..6usize {
        let (is_chroma, comp_idx, dst_x0, dst_y0, stride_ptr) = block_layout(b, mb_x, mb_y, pic);

        // CBP order per §2.4.3.6: bits 5..0 map to blocks (Y0,Y1,Y2,Y3,Cb,Cr).
        // Bit 5 (0x20) = block 0; bit 0 (0x01) = block 5.
        let coded = (cbp_bits & (1 << (5 - b))) != 0;

        // Frame-DCT luma: blocks 0/1 = top half rows 0..=7, blocks 2/3 =
        // bottom half rows 8..=15.  Field-DCT luma: blocks 0/1 = top-field
        // rows 0,2,…,14, blocks 2/3 = bottom-field rows 1,3,…,15 — both
        // the prediction slice AND the picture destination need the
        // de-interleaved layout, otherwise the residual lands on the
        // wrong rows.  Chroma is frame-organised regardless.
        let (pred_slice, pred_stride): (&[u8], usize) = if field_dct && !is_chroma {
            match b {
                0 => (&pred_y[0..], 16 * 2),
                1 => (&pred_y[8..], 16 * 2),
                2 => (&pred_y[16..], 16 * 2),       // row 1 = byte 16
                3 => (&pred_y[16 + 8..], 16 * 2),   // row 1, col 8
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
                quant_scale,
                &seq.non_intra_quantiser,
                params,
                pred_slice,
                pred_stride,
                sub,
                dst_stride,
            )?;
        } else {
            let _ = is_chroma;
            crate::block::copy_prediction(pred_slice, pred_stride, 8, sub, dst_stride);
        }
    }
    Ok(())
}

/// Returns (is_chroma, component_index, dst_x0, dst_y0, plane_stride).
fn build_prediction(
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
    // Collect the 1 or 2 motion-compensated reference patches.
    let mut have_fwd = false;
    let mut have_bwd = false;
    let mut fwd_y = [0u8; 16 * 16];
    let mut fwd_cb = [0u8; 8 * 8];
    let mut fwd_cr = [0u8; 8 * 8];
    let mut bwd_y = [0u8; 16 * 16];
    let mut bwd_cb = [0u8; 8 * 8];
    let mut bwd_cr = [0u8; 8 * 8];

    if let Some((mx, my)) = fwd_mv {
        let Some(refp) = fwd_ref else {
            return Err(Error::invalid("forward MV without forward reference"));
        };
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
        let Some(refp) = bwd_ref else {
            return Err(Error::invalid("backward MV without backward reference"));
        };
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
        (false, false) => {
            return Err(Error::invalid(
                "inter MB with neither forward nor backward MV",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn mc_mb(
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
    motion::predict_block(
        &refp.y,
        refp.y_stride,
        refp.y_stride as i32,
        (refp.y.len() / refp.y_stride) as i32,
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
    let mv_cx = motion::scale_mv_to_chroma(mv_x);
    let mv_cy = motion::scale_mv_to_chroma(mv_y);
    motion::predict_block(
        &refp.cb,
        refp.c_stride,
        refp.c_stride as i32,
        (refp.cb.len() / refp.c_stride) as i32,
        c_px,
        c_py,
        mv_cx,
        mv_cy,
        8,
        dst_cb,
        8,
    );
    motion::predict_block(
        &refp.cr,
        refp.c_stride,
        refp.c_stride as i32,
        (refp.cr.len() / refp.c_stride) as i32,
        c_px,
        c_py,
        mv_cx,
        mv_cy,
        8,
        dst_cr,
        8,
    );
}

fn fill_forward_predict(
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

fn fill_bidir_predict(
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

fn write_mb(
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

