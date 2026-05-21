//! Intra 8×8 block decode.
//!
//! For each block:
//!   1. Decode `dct_dc_size` (Table B.12 luma / B.13 chroma).
//!   2. Read `size` raw bits, sign-extend, add to the per-component DC
//!      predictor.
//!   3. Reconstruct `F[0][0] = intra_dc_mult * QF[0][0]` per §7.4.2.1.
//!   4. Loop reading AC `(run, level)` pairs via Table B.14 (or B.15 when
//!      `intra_vlc_format = 1`) until EOB; escape codewords carry raw
//!      6-bit run + 12-bit signed level.
//!   5. Inverse zigzag, dequantise per §7.4.2.3, apply mismatch control.
//!   6. IDCT → clamp to 0..=255 → write to the picture plane.

use crate::bitstream::BitReader;
use crate::dequant::{apply_mismatch_control, quantiser_scale};
use crate::error::{Error, Result};
use crate::idct::idct_8x8;
use crate::picture_params::PictureParams;
use crate::scan::{ALTERNATE_SCAN, ZIGZAG_SCAN};
use crate::tables::dct_coeffs::{self, DctSym};
use crate::tables::dct_dc_size;
use crate::vlc;

/// Sign-extend a `size`-bit unsigned DC-differential to i32 per §7.2.1.
fn sign_extend_dc(value: u32, size: u32) -> i32 {
    if size == 0 {
        return 0;
    }
    let half = 1u32 << (size - 1);
    if value < half {
        (value as i32) - ((1i32 << size) - 1)
    } else {
        value as i32
    }
}

/// Read a Table B.14/B.15 escape codeword's raw `(run, level)`.  MPEG-2
/// escape format is 6-bit run + 12-bit signed level (no two-stage MPEG-1
/// hack); the level is in two's complement.
fn decode_escape(br: &mut BitReader<'_>) -> Result<(usize, i32)> {
    let run = br.read(6)? as usize;
    let level_bits = br.read(12)?;
    let level = if level_bits & 0x800 != 0 {
        (level_bits as i32) - 0x1000
    } else {
        level_bits as i32
    };
    if level == 0 || level == -2048 {
        return Err(Error::invalid("dct escape: forbidden level value"));
    }
    Ok((run, level))
}

/// Decode one intra 8×8 block.
///
/// * `is_chroma` picks Table B.13 vs B.12 for DC-size decode.
/// * `dc_pred` is the running DC predictor for this component (Y / Cb / Cr).
///   Updated in place to the new `QF[0][0]` after this block.
/// * `quant_code` is the current effective `quantiser_scale_code` (the
///   slice-header value, possibly overridden by an MB-quant escape).
/// * `out_samples` is a mutable slice into the picture plane starting at
///   this block's top-left pixel; `out_stride` is the plane row stride.
#[allow(clippy::too_many_arguments)]
pub fn decode_intra_block(
    br: &mut BitReader<'_>,
    is_chroma: bool,
    dc_pred: &mut i32,
    quant_code: u8,
    intra_quantiser: &[u8; 64],
    params: &PictureParams,
    out_samples: &mut [u8],
    out_stride: usize,
) -> Result<()> {
    // --- DC differential ----------------------------------------------------
    let dc_tbl = if is_chroma {
        dct_dc_size::chroma()
    } else {
        dct_dc_size::luma()
    };
    let dc_size = vlc::decode(br, dc_tbl)?;
    let dc_diff = if dc_size == 0 {
        0
    } else {
        let bits = br.read(dc_size)?;
        sign_extend_dc(bits, dc_size as u32)
    };
    *dc_pred = dc_pred.wrapping_add(dc_diff);

    // Reconstruct pel-space DC and seed the coefficient buffer.
    let mut coeffs = [0i16; 64];
    let f00 = (*dc_pred).wrapping_mul(params.intra_dc_mult());
    coeffs[0] = f00.clamp(-2048, 2047) as i16;

    // --- AC run/level loop --------------------------------------------------
    let ac_tbl = if params.coding.intra_vlc_format {
        dct_coeffs::table_b15()
    } else {
        dct_coeffs::table_b14()
    };
    let scan: &[usize; 64] = if params.coding.alternate_scan {
        &ALTERNATE_SCAN
    } else {
        &ZIGZAG_SCAN
    };

    let mut k: usize = 1;
    loop {
        let sym = vlc::decode(br, ac_tbl)?;
        let (run, level) = match sym {
            DctSym::Eob => break,
            DctSym::RunLevel { run, level_abs } => {
                let sign = br.read(1)?;
                let mut lv = level_abs as i32;
                if sign == 1 {
                    lv = -lv;
                }
                (run as usize, lv)
            }
            DctSym::Escape => decode_escape(br)?,
        };
        k += run;
        if k >= 64 {
            return Err(Error::invalid("intra block: AC run past end"));
        }
        let nat = scan[k];
        // Dequantise AC per §7.4.2.3 intra:
        //   F[v][u] = (QF[v][u] * W[v][u] * scale) / 16    (truncation)
        // Then clamp to ±2048.
        let w = intra_quantiser[nat] as i32;
        let scale = quantiser_scale(quant_code, params.coding.q_scale_type);
        let rec = (level * w * scale) / 16;
        coeffs[nat] = rec.clamp(-2048, 2047) as i16;
        k += 1;
    }

    // --- Mismatch control + IDCT + write ------------------------------------
    apply_mismatch_control(&mut coeffs);
    let pixels = idct_8x8(&coeffs);
    for j in 0..8 {
        for i in 0..8 {
            let v = pixels[j * 8 + i].clamp(0, 255) as u8;
            out_samples[j * out_stride + i] = v;
        }
    }
    Ok(())
}

/// Decode one non-intra block.
#[allow(clippy::too_many_arguments)]
pub fn decode_non_intra_block(
    br: &mut BitReader<'_>,
    quant_code: u8,
    non_intra_quant: &[u8; 64],
    params: &PictureParams,
    prediction: &[u8],
    prediction_stride: usize,
    out_samples: &mut [u8],
    dst_stride: usize,
) -> Result<()> {
    let mut coeffs = [0i16; 64];

    let first_tbl = dct_coeffs::first_coeff_table();
    let ac_tbl = dct_coeffs::table_b14();
    let scan: &[usize; 64] = if params.coding.alternate_scan {
        &ALTERNATE_SCAN
    } else {
        &ZIGZAG_SCAN
    };

    let mut k: usize = 0;
    let mut first = true;
    loop {
        let sym = if first {
            vlc::decode(br, first_tbl)?
        } else {
            vlc::decode(br, ac_tbl)?
        };
        let (run, level) = match sym {
            DctSym::Eob => {
                if first {
                    return Err(Error::invalid("non-intra block: EOB as first symbol"));
                }
                break;
            }
            DctSym::RunLevel { run, level_abs } => {
                let sign = br.read(1)?;
                let mut lv = level_abs as i32;
                if sign == 1 {
                    lv = -lv;
                }
                (run as usize, lv)
            }
            DctSym::Escape => decode_escape(br)?,
        };
        first = false;
        k += run;
        if k >= 64 {
            return Err(Error::invalid("non-intra block: AC run past end"));
        }

        let nat = scan[k];
        let w = non_intra_quant[nat] as i32;
        let add = if level > 0 { 1 } else { -1 };
        let scale = quantiser_scale(quant_code, params.coding.q_scale_type);
        let rec = ((2 * level + add) * w * scale) / 32;
        coeffs[nat] = rec.clamp(-2048, 2047) as i16;
        k += 1;
    }

    apply_mismatch_control(&mut coeffs);

    let pixels = idct_8x8(&coeffs);

    for j in 0..8 {
        for i in 0..8 {
            let p = prediction[j * prediction_stride + i] as i32;
            let r = pixels[j * 8 + i] as i32;
            let v = (p + r).clamp(0, 255);
            out_samples[j * dst_stride + i] = v as u8;
        }
    }
    Ok(())
}

/// Copy a prediction block to the output (no residual).
pub fn copy_prediction(
    prediction: &[u8],
    prediction_stride: usize,
    size: usize,
    out_samples: &mut [u8],
    dst_stride: usize,
) {
    for j in 0..size {
        out_samples[j * dst_stride..j * dst_stride + size]
            .copy_from_slice(&prediction[j * prediction_stride..j * prediction_stride + size]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::{
        AspectRatio, PictureCodingExtension, PictureHeader, PictureType, SequenceInfo,
    };

    fn make_params() -> PictureParams {
        PictureParams {
            sequence: SequenceInfo {
                width: 16,
                height: 16,
                frame_rate_num: 30000,
                frame_rate_den: 1001,
                aspect_ratio: AspectRatio::Square,
                progressive_sequence: true,
            },
            header: PictureHeader {
                temporal_reference: 0,
                picture_coding_type: PictureType::I,
                vbv_delay: 0xffff,
                full_pel_forward_vector: false,
                forward_f_code: 0,
                full_pel_backward_vector: false,
                backward_f_code: 0,
            },
            coding: PictureCodingExtension {
                f_code: [[1, 1], [1, 1]],
                intra_dc_precision: 0,
                picture_structure: 0b11,
                top_field_first: false,
                frame_pred_frame_dct: true,
                concealment_motion_vectors: false,
                q_scale_type: false,
                intra_vlc_format: false,
                alternate_scan: false,
                repeat_first_field: false,
                chroma_420_type: false,
                progressive_frame: true,
            },
        }
    }

    #[test]
    fn sign_extend_dc_basics() {
        // size=0 always yields 0.
        assert_eq!(sign_extend_dc(0, 0), 0);
        // size=1: half=1; value<half=0 → 0 - (2^1-1) = -1; value≥half=1 → 1.
        assert_eq!(sign_extend_dc(0, 1), -1);
        assert_eq!(sign_extend_dc(1, 1), 1);
        // size=3: half=4. value 3 → 3-7=-4; value 4 → 4; value 7 → 7.
        assert_eq!(sign_extend_dc(3, 3), -4);
        assert_eq!(sign_extend_dc(4, 3), 4);
        assert_eq!(sign_extend_dc(7, 3), 7);
    }

    /// DC-only intra block at the slice-start reset: `dc_size=0`,
    /// `dc_diff=0`, immediate EOB.  Verifies the predictor seeds the
    /// pel-space midpoint (Y≈128).
    #[test]
    fn dc_only_block_writes_midgray() {
        // Tiny bitstream: dc_size=0 (luma B.12 code `100`, 3 bits) then EOB
        // (B.14 code `10`, 2 bits) = 5 bits total.  Pack MSB-first:
        //   100_10_000 = 0b1001_0000 = 0x90.
        let data = [0x90u8];
        let mut br = BitReader::new(&data);
        // intra DC reset at precision=0 is 1 << (7+0) = 128 (§7.2.1).
        let mut dc_pred = 128i32;
        let params = make_params();
        let intra_q = crate::tables::q_matrices::DEFAULT_INTRA_QUANTISER_MATRIX;
        let mut out = [0u8; 8 * 8];
        decode_intra_block(
            &mut br,
            false,
            &mut dc_pred,
            8,
            &intra_q,
            &params,
            &mut out,
            8,
        )
        .unwrap();
        // dc_diff=0 → predictor stays at 128 → F[0][0] = 128*8 = 1024 →
        // IDCT of a DC-only block with coeff[0]=1024 spreads ~128 per
        // pixel.  Mismatch control may flip the LSB of coeff[63] which
        // perturbs a couple of pixels by ±1 LSB but the bulk should be
        // within a small window of 128.
        for &v in &out {
            assert!(
                (120..=136).contains(&v),
                "expected near-midgray DC, got {:?}",
                out
            );
        }
    }
}
