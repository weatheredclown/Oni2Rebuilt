//! Table B.9 — `coded_block_pattern` VLC.
//!
//! CBP value 0 is not representable (a MB with pattern=0 has the pattern
//! flag off in its MB-type instead of an explicit CBP=0 code).  All other
//! values 1..=63 have a codeword.
//!
//! Codeword data reproduced from libavcodec `ff_mpeg12_mbPatTable`.

use std::sync::OnceLock;

use crate::vlc::{VlcEntry, VlcTable};

const CODE: [u32; 64] = [
    0x1, 0xb, 0x9, 0xd, 0xd, 0x17, 0x13, 0x1f, 0xc, 0x16, 0x12, 0x1e, 0x13, 0x1b, 0x17, 0x13,
    0xb, 0x15, 0x11, 0x1d, 0x11, 0x19, 0x15, 0x11, 0xf, 0xf, 0xd, 0x3, 0xf, 0xb, 0x7, 0x7, 0xa,
    0x14, 0x10, 0x1c, 0xe, 0xe, 0xc, 0x2, 0x10, 0x18, 0x14, 0x10, 0xe, 0xa, 0x6, 0x6, 0x12, 0x1a,
    0x16, 0x12, 0xd, 0x9, 0x5, 0x5, 0xc, 0x8, 0x4, 0x4, 0x7, 0xa, 0x8, 0xc,
];

const BITS: [u8; 64] = [
    9, 5, 5, 6, 4, 7, 7, 8, 4, 7, 7, 8, 5, 8, 8, 8, 4, 7, 7, 8, 5, 8, 8, 8, 6, 8, 8, 9, 5, 8, 8, 9,
    4, 7, 7, 8, 6, 8, 8, 9, 5, 8, 8, 8, 5, 8, 8, 9, 5, 8, 8, 8, 5, 8, 8, 9, 5, 8, 8, 9, 3, 5, 5, 6,
];

pub fn table() -> &'static VlcTable<u8> {
    static CELL: OnceLock<VlcTable<u8>> = OnceLock::new();
    CELL.get_or_init(|| {
        VlcTable::new(
            (0..64)
                .filter(|&i| BITS[i] > 0)
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
                    "cbp entry {i} prefix-collides with entry {j}"
                );
            }
        }
    }
}
