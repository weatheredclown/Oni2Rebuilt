//! Per-picture decoded configuration.
//!
//! [`PictureParams`] groups the cross-layer information needed by the slice
//! and macroblock decoders into one struct so it can be passed by reference
//! instead of threading individual fields.  The sequence header and picture
//! coding extension are kept around for downstream consumers (deinterlace,
//! display) that need flags like `top_field_first` or `progressive_frame`.

use crate::headers::{PictureCodingExtension, PictureHeader, PictureType, SequenceInfo};

#[derive(Clone, Debug)]
pub struct PictureParams {
    pub sequence: SequenceInfo,
    pub header: PictureHeader,
    pub coding: PictureCodingExtension,
}

impl PictureParams {
    pub fn interlaced(&self) -> bool {
        !self.sequence.progressive_sequence || !self.coding.progressive_frame
    }

    pub fn picture_type(&self) -> PictureType {
        self.header.picture_coding_type
    }

    /// Reset value for the intra DC predictor at the start of a slice, per
    /// §7.2.1: `1 << (7 + intra_dc_precision)`.  Equals 128 for the default
    /// 8-bit precision (precision=0); after multiplication by
    /// `intra_dc_mult()` this seeds the pel-space DC at 1024 ⇒ spatial
    /// midpoint 128 after IDCT, the correct "no signal yet" predictor.
    pub fn intra_dc_reset_value(&self) -> i32 {
        1 << (7 + self.coding.intra_dc_precision as i32)
    }

    /// MPEG-2 §7.4.2.1 intra DC multiplier — converts the differentially
    /// decoded `QF[0][0]` into the pel-space `F[0][0] = intra_dc_mult * QF[0][0]`.
    ///   * precision = 0 → 8-bit DC → multiplier 8
    ///   * precision = 1 → 9-bit DC → multiplier 4
    ///   * precision = 2 → 10-bit DC → multiplier 2
    ///   * precision = 3 → 11-bit DC → multiplier 1 (reserved in practice)
    pub fn intra_dc_mult(&self) -> i32 {
        match self.coding.intra_dc_precision {
            0 => 8,
            1 => 4,
            2 => 2,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::{AspectRatio, PictureType};

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
    fn intra_dc_reset_default_precision() {
        // §7.2.1: reset = 1 << (7 + precision).  For 8-bit video
        // (precision=0) this is 128; combined with intra_dc_mult=8 the
        // pel-space DC seed is 1024 → IDCT gives 128 (gray midpoint).
        let p = make_params();
        assert_eq!(p.intra_dc_reset_value(), 128);
        assert_eq!(p.intra_dc_mult(), 8);
    }

    #[test]
    fn intra_dc_reset_precision_two() {
        // precision=2 → reset = 1<<9 = 512, mult = 2 → pel DC seed = 1024
        // (same spatial midpoint regardless of bit depth).
        let mut p = make_params();
        p.coding.intra_dc_precision = 2;
        assert_eq!(p.intra_dc_reset_value(), 512);
        assert_eq!(p.intra_dc_mult(), 2);
    }
}
