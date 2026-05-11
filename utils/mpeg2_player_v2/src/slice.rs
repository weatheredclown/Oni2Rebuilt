//! Slice-level decode.
//!
//! A slice is a contiguous run of macroblocks in raster order within a row.
//! The slice header carries the initial `quantiser_scale_code` plus an
//! `extra_bit_slice` extension hook (8-bit payload, loop-on-1).  The
//! macroblock loop itself walks `macroblock_address_increment` codewords
//! (Table B.1) until the next start-code prefix is detected.
//!
//! Milestone 4 scope: I-pictures only.  P/B slice handling (skipped MB
//! inheritance, motion-vector predictor state) lands in milestone 5.

use crate::bitstream::BitReader;
use crate::error::{Error, Result};
use crate::mb::{check_skipped_mb, decode_intra_mb, SliceState};
use crate::picture::PictureBuffer;
use crate::picture_params::PictureParams;
use crate::tables::mba;
use crate::vlc;

/// Decode one slice payload.  `slice_start_code` is the low byte of the
/// start-code word (slice rows are encoded as `0x01..=0xaf`, with the value
/// being the 1-based MB-row number).  `payload` is the bitstream between
/// this slice's start code and the next start code.
pub fn decode_slice(
    payload: &[u8],
    slice_start_code: u8,
    pic: &mut PictureBuffer,
    params: &PictureParams,
    intra_quantiser: &[u8; 64],
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
    // extra_bit_slice — 1-bit flag followed by an optional 8-bit payload,
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
        check_skipped_mb(picture_type, incr, first)?;
        mb_addr += incr as i32;
        if mb_addr >= (mb_row + 1) * mb_width {
            return Err(Error::invalid("slice: MB address past end of row"));
        }
        first = false;

        let mb_x = (mb_addr % mb_width) as usize;
        let mb_y = (mb_addr / mb_width) as usize;
        decode_intra_mb(&mut br, &mut state, pic, params, intra_quantiser, mb_x, mb_y)?;

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
