pub fn idct_8x8(input: &[i16; 64]) -> [i16; 64] {
    let mut out = [0i16; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0.0;
            for v in 0..8 {
                for u in 0..8 {
                    let cu = if u == 0 {
                        std::f64::consts::FRAC_1_SQRT_2
                    } else {
                        1.0
                    };
                    let cv = if v == 0 {
                        std::f64::consts::FRAC_1_SQRT_2
                    } else {
                        1.0
                    };
                    sum += cu
                        * cv
                        * f64::from(input[v * 8 + u])
                        * (((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI) / 16.0).cos()
                        * (((2 * y + 1) as f64 * v as f64 * std::f64::consts::PI) / 16.0).cos();
                }
            }
            out[y * 8 + x] = (sum / 4.0).round().clamp(-2048.0, 2047.0) as i16;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_only_block_is_constant() {
        let mut input = [0; 64];
        input[0] = 80;
        assert_eq!(idct_8x8(&input), [10; 64]);
    }
}
