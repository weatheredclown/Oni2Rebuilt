use crate::headers::{PictureCodingExtension, PictureHeader, SequenceInfo};

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
}
