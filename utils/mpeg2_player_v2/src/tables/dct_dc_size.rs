//! Tables B.12 / B.13 — DC-coefficient size VLCs.
//!
//! Separate tables for luminance and chrominance.  Decoded value is the
//! `dct_dc_size` (0..=11 in MPEG-1, ≤ 8 in practice for 8-bit video).  The
//! call site then reads `size` further bits as the DC differential, with
//! the spec's signed-extend rule.
//!
//! Codeword data reproduced from libavcodec
//! `ff_mpeg12_vlc_dc_lum_{code,bits}` / `ff_mpeg12_vlc_dc_chroma_{code,bits}`.

use std::sync::OnceLock;

use crate::vlc::{VlcEntry, VlcTable};

const LUMA_CODE: [u32; 12] = [
    0x004, 0x000, 0x001, 0x005, 0x006, 0x00e, 0x01e, 0x03e, 0x07e, 0x0fe, 0x1fe, 0x1ff,
];
const LUMA_BITS: [u8; 12] = [3, 2, 2, 3, 3, 4, 5, 6, 7, 8, 9, 9];

const CHROMA_CODE: [u32; 12] = [
    0x000, 0x001, 0x002, 0x006, 0x00e, 0x01e, 0x03e, 0x07e, 0x0fe, 0x1fe, 0x3fe, 0x3ff,
];
const CHROMA_BITS: [u8; 12] = [2, 2, 2, 3, 4, 5, 6, 7, 8, 9, 10, 10];

pub fn luma() -> &'static VlcTable<u8> {
    static CELL: OnceLock<VlcTable<u8>> = OnceLock::new();
    CELL.get_or_init(|| {
        VlcTable::new(
            (0..12)
                .map(|i| VlcEntry::new(LUMA_BITS[i], LUMA_CODE[i], i as u8))
                .collect(),
        )
    })
}

pub fn chroma() -> &'static VlcTable<u8> {
    static CELL: OnceLock<VlcTable<u8>> = OnceLock::new();
    CELL.get_or_init(|| {
        VlcTable::new(
            (0..12)
                .map(|i| VlcEntry::new(CHROMA_BITS[i], CHROMA_CODE[i], i as u8))
                .collect(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_collisions() {
        for (name, t) in [("luma", luma()), ("chroma", chroma())] {
            for (i, e) in t.entries.iter().enumerate() {
                for (j, f) in t.entries.iter().enumerate() {
                    if i >= j || e.bits > f.bits {
                        continue;
                    }
                    let f_prefix = f.code >> (f.bits - e.bits) as u32;
                    assert_ne!(
                        f_prefix, e.code,
                        "{name}: entry {i} prefix-collides with {j}"
                    );
                }
            }
        }
    }

    #[test]
    fn luma_size_zero_is_three_bits() {
        // DC size = 0 has codeword `100` per Table B.12.
        let t = luma();
        let e = t.entries.iter().find(|e| e.value == 0).unwrap();
        assert_eq!(e.bits, 3);
        assert_eq!(e.code, 0b100);
    }
}
