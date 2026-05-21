//! Table B.1 — `macroblock_address_increment` VLC.
//!
//! Codewords for MB-address-increment values 1..=33 plus two sentinels:
//! `ESCAPE` (`0x08`, 11 bits — adds 33 to the address) and `STUFFING`
//! (`0x0F`, 11 bits — MPEG-1 stuffing, ignored on decode).
//!
//! Values reproduced from libavcodec `ff_mpeg12_mbAddrIncrTable`
//! (mpeg12data.c, LGPL); the codeword data are facts from ISO/IEC 13818-2
//! Annex B and not copyrightable.

use crate::vlc::{VlcEntry, VlcTable};

/// Sentinel: read another increment chunk and add 33 to the running total.
pub const ESCAPE: u8 = 0xFF;
/// Sentinel: MPEG-1 macroblock_stuffing.  Decoders just consume and continue.
pub const STUFFING: u8 = 0xFE;

const CODE: [u32; 35] = [
    0x01, 0x03, 0x02, 0x03, 0x02, 0x03, 0x02, 0x07, 0x06, 0x0b, 0x0a, 0x09, 0x08, 0x07, 0x06, 0x17,
    0x16, 0x15, 0x14, 0x13, 0x12, 0x23, 0x22, 0x21, 0x20, 0x1f, 0x1e, 0x1d, 0x1c, 0x1b, 0x1a, 0x19,
    0x18, 0x08, 0x0f,
];
const BITS: [u8; 35] = [
    1, 3, 3, 4, 4, 5, 5, 7, 7, 8, 8, 8, 8, 8, 8, 10, 10, 10, 10, 10, 10, 11, 11, 11, 11, 11, 11,
    11, 11, 11, 11, 11, 11, 11, 11,
];

pub fn table() -> &'static VlcTable<u8> {
    use std::sync::OnceLock;
    static CELL: OnceLock<VlcTable<u8>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut v = Vec::with_capacity(35);
        for i in 0..33 {
            v.push(VlcEntry::new(BITS[i], CODE[i], (i + 1) as u8));
        }
        v.push(VlcEntry::new(BITS[33], CODE[33], ESCAPE));
        v.push(VlcEntry::new(BITS[34], CODE[34], STUFFING));
        VlcTable::new(v)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No two distinct codewords share a prefix — required for VLC parsing.
    #[test]
    fn no_prefix_collisions() {
        let t = table();
        for (i, e) in t.entries.iter().enumerate() {
            for (j, f) in t.entries.iter().enumerate() {
                if i >= j || e.bits > f.bits {
                    continue;
                }
                let f_prefix = f.code >> (f.bits - e.bits) as u32;
                assert_ne!(
                    f_prefix, e.code,
                    "entry {i} ({}/{} bits) is a prefix of entry {j} ({}/{})",
                    e.code, e.bits, f.code, f.bits
                );
            }
        }
    }

    #[test]
    fn increment_one_is_single_bit() {
        // Increment = 1 is the single-bit codeword `1`.  Most common case
        // by far; the LUT-resolved fast path lives here.
        let t = table();
        assert_eq!(t.entries[0].bits, 1);
        assert_eq!(t.entries[0].code, 0b1);
        assert_eq!(t.entries[0].value, 1);
    }

    #[test]
    fn escape_and_stuffing_at_end() {
        let t = table();
        assert_eq!(t.entries[33].value, ESCAPE);
        assert_eq!(t.entries[34].value, STUFFING);
    }
}
