//! Slice-level decode.
//!
//! A slice is a contiguous run of macroblocks in raster order within a row.
//! The slice header carries the initial `quantiser_scale_code` plus an
//! `extra_bit_slice` extension hook (8-bit payload, loop-on-1).  The
//! macroblock loop itself walks `macroblock_address_increment` codewords
//! (Table B.1) until the next start-code prefix is detected.
//!
//! Milestone 5 scope: I/P/B pictures with P/B slice handling (skipped MB
//! inheritance, motion-vector predictor state, inter MBs).

use crate::bitstream::BitReader;
use crate::error::{Error, Result};
use crate::headers::PictureType;
use crate::mb::{
    DIR_BWD, DIR_FWD, SliceState, decode_inter_mb, decode_intra_mb, fill_bidir_predict,
    fill_forward_predict, parse_direction_mvs, parse_mpeg2_macroblock_modes,
};
use crate::picture::PictureBuffer;
use crate::picture_params::PictureParams;
use crate::tables::{cbp, mb_type, mba};
use crate::vlc;

/// Decode one slice payload.  `slice_start_code` is the low byte of the
/// start-code word (slice rows are encoded as `0x01..=0xaf`, with the value
/// being the 1-based MB-row number).  `payload` is the bitstream between
/// this slice's start code and the next start code.
#[allow(clippy::too_many_arguments)]
pub fn decode_slice(
    payload: &[u8],
    slice_start_code: u8,
    pic: &mut PictureBuffer,
    fwd_ref: Option<&PictureBuffer>,
    bwd_ref: Option<&PictureBuffer>,
    params: &PictureParams,
    intra_quantiser: &[u8; 64],
    non_intra_quantiser: &[u8; 64],
) -> Result<()> {
    let mb_row = (slice_start_code as i32) - 1;
    if mb_row < 0 || (mb_row as usize) >= pic.mb_height {
        return Err(Error::invalid("slice: row out of range"));
    }
    let mb_width = pic.mb_width as i32;

    let mut br = BitReader::new(payload);
    let qs = br.read(5)? as u8;
    if qs == 0 {
        return Err(Error::invalid("slice quantiser_scale_code = 0"));
    }
    // extra_bit_slice - 1-bit flag followed by an optional 8-bit payload,
    // repeated while the flag is set.  All ignored on decode.
    while br.read(1)? == 1 {
        let _ = br.read(8)?;
    }

    let mut state = SliceState::new(params.intra_dc_reset_value(), qs);
    let mut mb_addr: i32 = mb_row * mb_width - 1;
    let mut first = true;
    let picture_type = params.picture_type();

    loop {
        // Read a macroblock_address_increment, handling stuffing and escape
        // (each escape adds 33 to the running total).
        let mut incr: u32 = 0;
        loop {
            let sym = vlc::decode(&mut br, mba::table())?;
            if sym == mba::STUFFING {
                continue;
            }
            if sym == mba::ESCAPE {
                incr += 33;
                continue;
            }
            incr += sym as u32;
            break;
        }

        let prev_mb_addr = mb_addr;
        mb_addr += incr as i32;
        if mb_addr >= (mb_row + 1) * mb_width {
            return Err(Error::invalid("slice: MB address past end of row"));
        }

        // Handle skipped MBs
        if !first && incr > 1 {
            match picture_type {
                PictureType::I => return Err(Error::invalid("I-picture: skipped MB not allowed")),
                PictureType::P => {
                    state.reset_dir(DIR_FWD);
                    state.reset_dir(DIR_BWD);
                    state.last_had_forward = true;
                    state.last_had_backward = false;
                    for skip_addr in (prev_mb_addr + 1)..mb_addr {
                        let sx = (skip_addr % mb_width) as usize;
                        let sy = (skip_addr / mb_width) as usize;
                        fill_forward_predict(pic, fwd_ref, sx, sy, 0, 0)?;
                    }
                }
                PictureType::B => {
                    for skip_addr in (prev_mb_addr + 1)..mb_addr {
                        let sx = (skip_addr % mb_width) as usize;
                        let sy = (skip_addr / mb_width) as usize;
                        let fwd_mv = if state.last_had_forward {
                            Some((state.pmv[DIR_FWD][0].x, state.pmv[DIR_FWD][0].y))
                        } else {
                            None
                        };
                        let bwd_mv = if state.last_had_backward {
                            Some((state.pmv[DIR_BWD][0].x, state.pmv[DIR_BWD][0].y))
                        } else {
                            None
                        };
                        fill_bidir_predict(pic, fwd_ref, bwd_ref, sx, sy, fwd_mv, bwd_mv)?;
                    }
                }
                PictureType::D => return Err(Error::unsupported("D-picture not supported")),
                PictureType::Reserved(_) => return Err(Error::invalid("reserved picture type")),
            }
        }
        first = false;

        let mb_x = (mb_addr % mb_width) as usize;
        let mb_y = (mb_addr / mb_width) as usize;

        let mb_type_flags = match picture_type {
            PictureType::I => vlc::decode(&mut br, mb_type::i_table())?,
            PictureType::P => vlc::decode(&mut br, mb_type::p_table())?,
            PictureType::B => vlc::decode(&mut br, mb_type::b_table())?,
            PictureType::D => return Err(Error::unsupported("D-picture not supported")),
            PictureType::Reserved(_) => return Err(Error::invalid("reserved picture type")),
        };

        // Per H.262 §6.2.5 / §6.3.17 the spec ordering is:
        //   macroblock_modes()   ← macroblock_type + frame_motion_type + dct_type
        //   if (macroblock_quant) quantiser_scale_code
        //   motion_vectors(...)
        // Reading `quantiser_scale_code` BEFORE the MPEG-2 side bits would
        // consume 5 bits that are actually `frame_motion_type` + `dct_type`
        // + 2 bits of the next field, which on FPFD=0 content surfaces as
        // a spurious "MB quantiser_scale_code = 0" error.
        let mpeg2_modes = parse_mpeg2_macroblock_modes(&mut br, params, mb_type_flags)?;
        let field_dct = mpeg2_modes.field_dct;

        if mb_type_flags.quant {
            let nqs = br.read(5)? as u8;
            if nqs == 0 {
                return Err(Error::invalid("MB quantiser_scale_code = 0"));
            }
            state.quant_code = nqs;
        }

        let fwd_pred = parse_direction_mvs(
            &mut br,
            picture_type,
            mb_type_flags.motion_forward,
            mb_type_flags.intra,
            mpeg2_modes.motion_type,
            params.coding.f_code[DIR_FWD],
            params.header.full_pel_forward_vector,
            DIR_FWD,
            &mut state,
        )?;

        let bwd_pred = parse_direction_mvs(
            &mut br,
            picture_type,
            mb_type_flags.motion_backward,
            mb_type_flags.intra,
            mpeg2_modes.motion_type,
            params.coding.f_code[DIR_BWD],
            params.header.full_pel_backward_vector,
            DIR_BWD,
            &mut state,
        )?;

        let fwd_mv = fwd_pred.frame_mv();
        let bwd_mv = bwd_pred.frame_mv();

        if matches!(picture_type, PictureType::B) && !mb_type_flags.intra {
            // B-skip MV inheritance (§7.6.3.5) cares whether each direction
            // had MOTION, not whether it produced a frame-mode MV — use
            // `has_any()` so field-MC vectors propagate too.  `frame_mv()`
            // returns None for the Field variant, which would otherwise
            // make the next B-skip see "no inheritance available" and fail.
            state.last_had_forward = fwd_pred.has_any();
            state.last_had_backward = bwd_pred.has_any();
        }

        if !mb_type_flags.intra {
            let r = params.intra_dc_reset_value();
            state.dc_pred = [r, r, r];
        }

        let cbp_bits: u8 = if mb_type_flags.pattern {
            vlc::decode(&mut br, cbp::table())?
        } else if mb_type_flags.intra {
            0b111111
        } else {
            0
        };

        if mb_type_flags.intra {
            decode_intra_mb(
                &mut br,
                &mut state,
                pic,
                params,
                intra_quantiser,
                mb_x,
                mb_y,
                field_dct,
            )?;
        } else {
            decode_inter_mb(
                &mut br,
                &mut state,
                pic,
                fwd_ref,
                bwd_ref,
                params,
                non_intra_quantiser,
                mb_x,
                mb_y,
                fwd_pred,
                bwd_pred,
                cbp_bits,
                field_dct,
            )?;
        }

        if !matches!(picture_type, PictureType::B) {
            state.last_had_forward = mb_type_flags.motion_forward
                || (!mb_type_flags.intra && matches!(picture_type, PictureType::P));
            state.last_had_backward = mb_type_flags.motion_backward;
        }

        // Slice termination: stop at the right edge of this row OR when
        // the next 23 bits are all zero (= start-code prefix follows).
        if mb_addr + 1 >= (mb_row + 1) * mb_width {
            break;
        }
        let avail = br.remaining_bits().min(23) as u8;
        if avail == 0 {
            break;
        }
        if br.peek(avail)? == 0 {
            break;
        }
    }
    Ok(())
}
