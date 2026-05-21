//! Tables B.14 / B.15 — DCT-coefficient VLCs.
//!
//! Both tables encode the same 111 `(run, level)` pairs but with different
//! codewords.  B.14 is the default; B.15 is the "alternate intra" table
//! selected by `intra_vlc_format = 1` in the picture coding extension.
//!
//! The first AC coefficient of a non-intra block uses a special
//! interpretation where the codeword `1s` resolves to `(run=0, level=±1)`
//! instead of EOB; that's handled via [`first_coeff_table`].
//!
//! Code tables reproduced from libavcodec `ff_mpeg1_vlc_table` /
//! `ff_mpeg2_vlc_table` (mpeg12data.c, LGPL).  The table values are facts
//! from ISO/IEC 13818-2 Annex B and are not copyrightable; the layout
//! follows ffmpeg's convention.

use std::sync::OnceLock;

use crate::vlc::{VlcEntry, VlcTable};

/// Symbol decoded from a DCT-coefficient VLC entry.
#[derive(Clone, Copy, Debug)]
pub enum DctSym {
    /// Run of zeros followed by a coefficient of absolute magnitude
    /// `level_abs`.  Sign bit follows immediately in the bitstream.
    RunLevel { run: u8, level_abs: u16 },
    /// End-of-block.
    Eob,
    /// Escape code: caller reads `(run:6, level:12)` raw bits after the
    /// escape codeword.
    Escape,
}

const LEVEL: [u8; 111] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    13, 14, 15, 16, 17, 18, 1, 2, 3, 4, 5, 1, 2, 3, 4, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 1, 2, 1, 2,
    1, 2, 1, 2, 1, 2, 1, 2, 1, 2, 1, 2, 1, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];

const RUN: [u8; 111] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 3,
    3, 3, 3, 4, 4, 4, 5, 5, 5, 6, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14,
    15, 15, 16, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
];

/// Table B.14 — `(code, bits)` for 111 RL pairs + escape + EOB.
const TABLE_B14: [(u32, u8); 113] = [
    (0x3, 2),
    (0x4, 4),
    (0x5, 5),
    (0x6, 7),
    (0x26, 8),
    (0x21, 8),
    (0xa, 10),
    (0x1d, 12),
    (0x18, 12),
    (0x13, 12),
    (0x10, 12),
    (0x1a, 13),
    (0x19, 13),
    (0x18, 13),
    (0x17, 13),
    (0x1f, 14),
    (0x1e, 14),
    (0x1d, 14),
    (0x1c, 14),
    (0x1b, 14),
    (0x1a, 14),
    (0x19, 14),
    (0x18, 14),
    (0x17, 14),
    (0x16, 14),
    (0x15, 14),
    (0x14, 14),
    (0x13, 14),
    (0x12, 14),
    (0x11, 14),
    (0x10, 14),
    (0x18, 15),
    (0x17, 15),
    (0x16, 15),
    (0x15, 15),
    (0x14, 15),
    (0x13, 15),
    (0x12, 15),
    (0x11, 15),
    (0x10, 15),
    (0x3, 3),
    (0x6, 6),
    (0x25, 8),
    (0xc, 10),
    (0x1b, 12),
    (0x16, 13),
    (0x15, 13),
    (0x1f, 15),
    (0x1e, 15),
    (0x1d, 15),
    (0x1c, 15),
    (0x1b, 15),
    (0x1a, 15),
    (0x19, 15),
    (0x13, 16),
    (0x12, 16),
    (0x11, 16),
    (0x10, 16),
    (0x5, 4),
    (0x4, 7),
    (0xb, 10),
    (0x14, 12),
    (0x14, 13),
    (0x7, 5),
    (0x24, 8),
    (0x1c, 12),
    (0x13, 13),
    (0x6, 5),
    (0xf, 10),
    (0x12, 12),
    (0x7, 6),
    (0x9, 10),
    (0x12, 13),
    (0x5, 6),
    (0x1e, 12),
    (0x14, 16),
    (0x4, 6),
    (0x15, 12),
    (0x7, 7),
    (0x11, 12),
    (0x5, 7),
    (0x11, 13),
    (0x27, 8),
    (0x10, 13),
    (0x23, 8),
    (0x1a, 16),
    (0x22, 8),
    (0x19, 16),
    (0x20, 8),
    (0x18, 16),
    (0xe, 10),
    (0x17, 16),
    (0xd, 10),
    (0x16, 16),
    (0x8, 10),
    (0x15, 16),
    (0x1f, 12),
    (0x1a, 12),
    (0x19, 12),
    (0x17, 12),
    (0x16, 12),
    (0x1f, 13),
    (0x1e, 13),
    (0x1d, 13),
    (0x1c, 13),
    (0x1b, 13),
    (0x1f, 16),
    (0x1e, 16),
    (0x1d, 16),
    (0x1c, 16),
    (0x1b, 16),
    (0x1, 6), // escape
    (0x2, 2), // EOB
];

/// Table B.15 — alternate intra DCT-coefficient VLC.  Same RUN/LEVEL pairs,
/// different codewords.
const TABLE_B15: [(u32, u8); 113] = [
    (0x02, 2),
    (0x06, 3),
    (0x07, 4),
    (0x1c, 5),
    (0x1d, 5),
    (0x05, 6),
    (0x04, 6),
    (0x7b, 7),
    (0x7c, 7),
    (0x23, 8),
    (0x22, 8),
    (0xfa, 8),
    (0xfb, 8),
    (0xfe, 8),
    (0xff, 8),
    (0x1f, 14),
    (0x1e, 14),
    (0x1d, 14),
    (0x1c, 14),
    (0x1b, 14),
    (0x1a, 14),
    (0x19, 14),
    (0x18, 14),
    (0x17, 14),
    (0x16, 14),
    (0x15, 14),
    (0x14, 14),
    (0x13, 14),
    (0x12, 14),
    (0x11, 14),
    (0x10, 14),
    (0x18, 15),
    (0x17, 15),
    (0x16, 15),
    (0x15, 15),
    (0x14, 15),
    (0x13, 15),
    (0x12, 15),
    (0x11, 15),
    (0x10, 15),
    (0x02, 3),
    (0x06, 5),
    (0x79, 7),
    (0x27, 8),
    (0x20, 8),
    (0x16, 13),
    (0x15, 13),
    (0x1f, 15),
    (0x1e, 15),
    (0x1d, 15),
    (0x1c, 15),
    (0x1b, 15),
    (0x1a, 15),
    (0x19, 15),
    (0x13, 16),
    (0x12, 16),
    (0x11, 16),
    (0x10, 16),
    (0x05, 5),
    (0x07, 7),
    (0xfc, 8),
    (0x0c, 10),
    (0x14, 13),
    (0x07, 5),
    (0x26, 8),
    (0x1c, 12),
    (0x13, 13),
    (0x06, 6),
    (0xfd, 8),
    (0x12, 12),
    (0x07, 6),
    (0x04, 9),
    (0x12, 13),
    (0x06, 7),
    (0x1e, 12),
    (0x14, 16),
    (0x04, 7),
    (0x15, 12),
    (0x05, 7),
    (0x11, 12),
    (0x78, 7),
    (0x11, 13),
    (0x7a, 7),
    (0x10, 13),
    (0x21, 8),
    (0x1a, 16),
    (0x25, 8),
    (0x19, 16),
    (0x24, 8),
    (0x18, 16),
    (0x05, 9),
    (0x17, 16),
    (0x07, 9),
    (0x16, 16),
    (0x0d, 10),
    (0x15, 16),
    (0x1f, 12),
    (0x1a, 12),
    (0x19, 12),
    (0x17, 12),
    (0x16, 12),
    (0x1f, 13),
    (0x1e, 13),
    (0x1d, 13),
    (0x1c, 13),
    (0x1b, 13),
    (0x1f, 16),
    (0x1e, 16),
    (0x1d, 16),
    (0x1c, 16),
    (0x1b, 16),
    (0x01, 6), // escape
    (0x06, 4), // EOB
];

fn build_table(codes: &[(u32, u8); 113]) -> VlcTable<DctSym> {
    let mut v = Vec::with_capacity(113);
    for i in 0..111 {
        let (code, bits) = codes[i];
        v.push(VlcEntry::new(
            bits,
            code,
            DctSym::RunLevel {
                run: RUN[i],
                level_abs: LEVEL[i] as u16,
            },
        ));
    }
    let (esc_code, esc_bits) = codes[111];
    v.push(VlcEntry::new(esc_bits, esc_code, DctSym::Escape));
    let (eob_code, eob_bits) = codes[112];
    v.push(VlcEntry::new(eob_bits, eob_code, DctSym::Eob));
    VlcTable::new(v)
}

/// Table B.14 — default DCT-coefficient VLC (intra blocks with
/// `intra_vlc_format = 0` and all non-intra blocks after the first
/// coefficient).
pub fn table_b14() -> &'static VlcTable<DctSym> {
    static CELL: OnceLock<VlcTable<DctSym>> = OnceLock::new();
    CELL.get_or_init(|| build_table(&TABLE_B14))
}

/// Table B.15 — alternate intra DCT-coefficient VLC, used when the
/// picture's `intra_vlc_format = 1`.
pub fn table_b15() -> &'static VlcTable<DctSym> {
    static CELL: OnceLock<VlcTable<DctSym>> = OnceLock::new();
    CELL.get_or_init(|| build_table(&TABLE_B15))
}

/// First-coefficient variant of [`table_b14`].  The codeword `1` (single
/// bit) means `(run=0, level=±1)` with the sign read from the NEXT bit.
/// In the regular table that prefix carries the 2-bit EOB; first-position
/// reads can't be EOB so we patch the entry.
pub fn first_coeff_table() -> &'static VlcTable<DctSym> {
    static CELL: OnceLock<VlcTable<DctSym>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut v = Vec::with_capacity(113);
        for i in 0..111 {
            let (code, bits) = TABLE_B14[i];
            if i == 0 {
                v.push(VlcEntry::new(
                    1,
                    0b1,
                    DctSym::RunLevel {
                        run: 0,
                        level_abs: 1,
                    },
                ));
                continue;
            }
            v.push(VlcEntry::new(
                bits,
                code,
                DctSym::RunLevel {
                    run: RUN[i],
                    level_abs: LEVEL[i] as u16,
                },
            ));
        }
        // Escape — no EOB at first position (caller treats it as a stream
        // error if encountered).
        let (esc_code, esc_bits) = TABLE_B14[111];
        v.push(VlcEntry::new(esc_bits, esc_code, DctSym::Escape));
        VlcTable::new(v)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_no_collision<T: Copy>(t: &VlcTable<T>, name: &str) {
        for (i, e) in t.entries.iter().enumerate() {
            for (j, f) in t.entries.iter().enumerate() {
                if i >= j || e.bits > f.bits {
                    continue;
                }
                let f_prefix = f.code >> (f.bits - e.bits) as u32;
                assert_ne!(
                    f_prefix, e.code,
                    "{name}: entry {i} ({}/{} bits) is a prefix of entry {j} ({}/{})",
                    e.code, e.bits, f.code, f.bits
                );
            }
        }
    }

    #[test]
    fn no_collisions_b14() {
        check_no_collision(table_b14(), "b14");
    }

    #[test]
    fn no_collisions_b15() {
        check_no_collision(table_b15(), "b15");
    }

    #[test]
    fn no_collisions_first_coeff() {
        check_no_collision(first_coeff_table(), "first_coeff");
    }

    #[test]
    fn b14_eob_is_short() {
        // EOB ('10' in B.14) is the second entry from the end.
        let t = table_b14();
        let eob = t
            .entries
            .iter()
            .rev()
            .find(|e| matches!(e.value, DctSym::Eob))
            .unwrap();
        assert_eq!(eob.code, 0b10);
        assert_eq!(eob.bits, 2);
    }

    #[test]
    fn b15_eob_is_four_bits() {
        // EOB in B.15 is `0110` (4 bits) — different placement than B.14.
        let t = table_b15();
        let eob = t
            .entries
            .iter()
            .rev()
            .find(|e| matches!(e.value, DctSym::Eob))
            .unwrap();
        assert_eq!(eob.code, 0b0110);
        assert_eq!(eob.bits, 4);
    }
}
