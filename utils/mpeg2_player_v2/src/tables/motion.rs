//! Table B.10 — `motion_code` VLC.
//!
//! Encodes the absolute magnitude of a motion-code symbol on a symmetric
//! ±16 alphabet.  When `|value| > 0` the codeword is followed by a single
//! sign bit (`0`=positive, `1`=negative); when `|value| = 0` no sign bit
//! follows.  Caller handles the sign read.
//!
//! Code/bit pairs from libavcodec `ff_mpeg12_mbMotionVectorTable`.

use std::sync::OnceLock;

use crate::vlc::{VlcEntry, VlcTable};

const CODE: [u32; 17] = [
    0x01, 0x01, 0x01, 0x01, 0x03, 0x05, 0x04, 0x03, 0x0b, 0x0a, 0x09, 0x11, 0x10, 0x0f, 0x0e, 0x0d,
    0x0c,
];
const BITS: [u8; 17] = [1, 2, 3, 4, 6, 7, 7, 7, 9, 9, 9, 10, 10, 10, 10, 10, 10];

pub fn table() -> &'static VlcTable<u8> {
    static CELL: OnceLock<VlcTable<u8>> = OnceLock::new();
    CELL.get_or_init(|| {
        VlcTable::new(
            (0..17)
                .map(|i| VlcEntry::new(BITS[i], CODE[i], i as u8))
                .collect(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_collisions() {
        let t = table();
        for (i, e) in t.entries.iter().enumerate() {
            for (j, f) in t.entries.iter().enumerate() {
                if i >= j || e.bits > f.bits {
                    continue;
                }
                let f_prefix = f.code >> (f.bits - e.bits) as u32;
                assert_ne!(
                    f_prefix, e.code,
                    "motion entry {i} prefix-collides with entry {j}"
                );
            }
        }
    }

    #[test]
    fn zero_is_single_bit() {
        let t = table();
        assert_eq!(t.entries[0].bits, 1);
        assert_eq!(t.entries[0].value, 0);
    }
}
