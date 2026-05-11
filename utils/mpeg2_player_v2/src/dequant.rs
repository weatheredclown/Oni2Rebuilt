use crate::tables::q_matrices::{
    DEFAULT_INTRA_QUANTISER_MATRIX, DEFAULT_NON_INTRA_QUANTISER_MATRIX,
};

const NON_LINEAR_QUANTISER_SCALE: [i32; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 18, 20, 22, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64,
    72, 80, 88, 96, 104, 112,
];

pub fn quantiser_scale(code: u8, q_scale_type: bool) -> i32 {
    if q_scale_type {
        NON_LINEAR_QUANTISER_SCALE[code as usize]
    } else {
        i32::from(code) * 2
    }
}

pub fn dequantise_intra(
    block: &[i16; 64],
    quantiser_scale_code: u8,
    q_scale_type: bool,
    matrix: Option<&[u8; 64]>,
) -> [i16; 64] {
    let scale = quantiser_scale(quantiser_scale_code, q_scale_type);
    let matrix = matrix.unwrap_or(&DEFAULT_INTRA_QUANTISER_MATRIX);
    let mut out = [0; 64];
    out[0] = block[0];
    for idx in 1..64 {
        let value = (i32::from(block[idx]) * i32::from(matrix[idx]) * scale) / 16;
        out[idx] = value.clamp(-2048, 2047) as i16;
    }
    apply_mismatch_control(&mut out);
    out
}

pub fn dequantise_non_intra(
    block: &[i16; 64],
    quantiser_scale_code: u8,
    q_scale_type: bool,
    matrix: Option<&[u8; 64]>,
) -> [i16; 64] {
    let scale = quantiser_scale(quantiser_scale_code, q_scale_type);
    let matrix = matrix.unwrap_or(&DEFAULT_NON_INTRA_QUANTISER_MATRIX);
    let mut out = [0; 64];
    for idx in 0..64 {
        let coeff = i32::from(block[idx]);
        if coeff == 0 {
            continue;
        }
        let sign = coeff.signum();
        let value = sign * (((2 * coeff.abs() + 1) * i32::from(matrix[idx]) * scale) / 32);
        out[idx] = value.clamp(-2048, 2047) as i16;
    }
    apply_mismatch_control(&mut out);
    out
}

pub fn apply_mismatch_control(block: &mut [i16; 64]) {
    let sum: i32 = block.iter().map(|&v| i32::from(v)).sum();
    if sum & 1 == 0 {
        block[63] ^= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_linear_quantiser_scale_matches_table() {
        assert_eq!(quantiser_scale(8, true), 8);
        assert_eq!(quantiser_scale(9, true), 10);
        assert_eq!(quantiser_scale(31, true), 112);
    }
}
