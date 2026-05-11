//! Decoded picture buffer: three Y/Cb/Cr planes with explicit strides.

/// Decoded picture in 4:2:0 planar YUV.  Strides equal the macroblock-padded
/// widths so the slice decoder can write 8×8 blocks with simple offset
/// arithmetic without per-row bounds checks.
#[derive(Clone, Debug)]
pub struct PictureBuffer {
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
    pub y_stride: usize,
    pub c_stride: usize,
    /// MB-aligned dimensions (multiples of 16 for luma, 8 for chroma).
    pub mb_width: usize,
    pub mb_height: usize,
    /// Visible (display) dimensions in pixels, ≤ MB-aligned dimensions.
    pub display_width: u32,
    pub display_height: u32,
}

impl PictureBuffer {
    /// Allocate a picture sized for `display_width × display_height`,
    /// padded out to the nearest 16-pel boundary for the macroblock grid.
    /// The plane buffers are zeroed.
    pub fn new(display_width: u32, display_height: u32) -> Self {
        let mb_width = (display_width as usize).div_ceil(16);
        let mb_height = (display_height as usize).div_ceil(16);
        let y_stride = mb_width * 16;
        let c_stride = mb_width * 8;
        let y_size = y_stride * mb_height * 16;
        let c_size = c_stride * mb_height * 8;
        Self {
            y: vec![0; y_size],
            cb: vec![0; c_size],
            cr: vec![0; c_size],
            y_stride,
            c_stride,
            mb_width,
            mb_height,
            display_width,
            display_height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_padded_planes() {
        // 24×24 rounds up to 32×32 luma.
        let p = PictureBuffer::new(24, 24);
        assert_eq!(p.mb_width, 2);
        assert_eq!(p.mb_height, 2);
        assert_eq!(p.y_stride, 32);
        assert_eq!(p.c_stride, 16);
        assert_eq!(p.y.len(), 32 * 32);
        assert_eq!(p.cb.len(), 16 * 16);
    }

    #[test]
    fn exact_macroblock_size_has_no_padding() {
        let p = PictureBuffer::new(16, 16);
        assert_eq!(p.mb_width, 1);
        assert_eq!(p.y.len(), 16 * 16);
    }
}
