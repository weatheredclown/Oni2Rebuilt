#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChromaUpsample {
    Nearest,
    Bilinear,
}

pub fn yuv420p_to_rgba8(
    width: usize,
    height: usize,
    y: &[u8],
    cb: &[u8],
    cr: &[u8],
    upsample: ChromaUpsample,
) -> Vec<u8> {
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let mut rgba = vec![0; width * height * 4];
    for row in 0..height {
        for col in 0..width {
            let yy = y[row * width + col] as i32;
            let (u, v) = match upsample {
                ChromaUpsample::Nearest => {
                    let cidx = (row / 2).min(ch - 1) * cw + (col / 2).min(cw - 1);
                    (cb[cidx] as i32, cr[cidx] as i32)
                }
                ChromaUpsample::Bilinear => sample_chroma_bilinear(width, height, cb, cr, row, col),
            };
            let c = yy - 16;
            let d = u - 128;
            let e = v - 128;
            let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
            let out = (row * width + col) * 4;
            rgba[out..out + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    rgba
}

fn sample_chroma_bilinear(
    width: usize,
    height: usize,
    cb: &[u8],
    cr: &[u8],
    row: usize,
    col: usize,
) -> (i32, i32) {
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let x2 = col as isize - 1;
    let y2 = row as isize - 1;
    let x0 = (x2 / 2).clamp(0, cw as isize - 1) as usize;
    let y0 = (y2 / 2).clamp(0, ch as isize - 1) as usize;
    let x1 = (x0 + 1).min(cw - 1);
    let y1 = (y0 + 1).min(ch - 1);
    let wx = if col % 2 == 0 { 1 } else { 3 };
    let wy = if row % 2 == 0 { 1 } else { 3 };
    let sample = |plane: &[u8]| -> i32 {
        let p00 = plane[y0 * cw + x0] as i32;
        let p01 = plane[y0 * cw + x1] as i32;
        let p10 = plane[y1 * cw + x0] as i32;
        let p11 = plane[y1 * cw + x1] as i32;
        let top = p00 * (4 - wx) + p01 * wx;
        let bottom = p10 * (4 - wx) + p11 * wx;
        (top * (4 - wy) + bottom * wy + 8) / 16
    };
    (sample(cb), sample(cr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_black_converts_to_opaque_black() {
        let rgba = yuv420p_to_rgba8(1, 1, &[16], &[128], &[128], ChromaUpsample::Nearest);
        assert_eq!(rgba, [0, 0, 0, 255]);
    }
}
