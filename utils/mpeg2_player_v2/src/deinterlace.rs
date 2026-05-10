#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeinterlaceMode {
    Preserve,
    Weave,
    Bob,
}

impl DeinterlaceMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "preserve" => Some(Self::Preserve),
            "weave" => Some(Self::Weave),
            "bob" => Some(Self::Bob),
            _ => None,
        }
    }
}

pub fn bob_plane(width: usize, height: usize, plane: &[u8], top_field: bool) -> Vec<u8> {
    let parity = usize::from(!top_field);
    let mut out = vec![0; width * height];
    for row in 0..height {
        let src = ((row / 2) * 2 + parity).min(height - 1);
        let next = (src + 2).min(height - 1);
        let interpolate = row % 2 == 1 && next != src;
        for col in 0..width {
            let value = if interpolate {
                (u16::from(plane[src * width + col]) + u16::from(plane[next * width + col]))
                    .div_ceil(2) as u8
            } else {
                plane[src * width + col]
            };
            out[row * width + col] = value;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bobs_top_field_with_interpolation() {
        let out = bob_plane(1, 4, &[10, 20, 30, 40], true);
        assert_eq!(out, [10, 20, 30, 35]);
    }
}
