//! Tables B.2 / B.3 / B.4 — `macroblock_type` VLCs per picture type.
//!
//! Decodes into [`MbTypeFlags`], a five-flag struct mirroring the spec's
//! macroblock-type bit positions (intra / pattern / motion_forward /
//! motion_backward / quant).  Codes verified against libavcodec
//! `table_mb_ptype` / `table_mb_btype`.

use std::sync::OnceLock;

use crate::vlc::{VlcEntry, VlcTable};

/// The decoded macroblock-type flags.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MbTypeFlags {
    pub quant: bool,
    pub motion_forward: bool,
    pub motion_backward: bool,
    pub pattern: bool,
    pub intra: bool,
}

impl MbTypeFlags {
    pub const fn new(quant: bool, fwd: bool, bwd: bool, pat: bool, intra: bool) -> Self {
        Self {
            quant,
            motion_forward: fwd,
            motion_backward: bwd,
            pattern: pat,
            intra,
        }
    }
}

/// Table B.2 — `macroblock_type` in I-pictures.
///   `1`  → intra
///   `01` → intra, quant
const I_ENTRIES: &[VlcEntry<MbTypeFlags>] = &[
    VlcEntry::new(1, 0b1, MbTypeFlags::new(false, false, false, false, true)),
    VlcEntry::new(2, 0b01, MbTypeFlags::new(true, false, false, false, true)),
];

/// Table B.3 — `macroblock_type` in P-pictures.
///   `1`      → MC, coded                       (fwd + pattern)
///   `01`     → no MC, coded                    (pattern)
///   `001`    → MC, not coded                   (fwd)
///   `00011`  → intra
///   `00010`  → MC, coded, quant                (fwd + pattern + quant)
///   `00001`  → no MC, coded, quant             (pattern + quant)
///   `000001` → intra, quant
const P_ENTRIES: &[VlcEntry<MbTypeFlags>] = &[
    VlcEntry::new(1, 0b1, MbTypeFlags::new(false, true, false, true, false)),
    VlcEntry::new(2, 0b01, MbTypeFlags::new(false, false, false, true, false)),
    VlcEntry::new(3, 0b001, MbTypeFlags::new(false, true, false, false, false)),
    VlcEntry::new(
        5,
        0b00011,
        MbTypeFlags::new(false, false, false, false, true),
    ),
    VlcEntry::new(5, 0b00010, MbTypeFlags::new(true, true, false, true, false)),
    VlcEntry::new(
        5,
        0b00001,
        MbTypeFlags::new(true, false, false, true, false),
    ),
    VlcEntry::new(
        6,
        0b000001,
        MbTypeFlags::new(true, false, false, false, true),
    ),
];

/// Table B.4 — `macroblock_type` in B-pictures.
///   `10`     → interpolated (fwd + bwd)
///   `11`     → interpolated, coded
///   `010`    → backward only
///   `011`    → backward, coded
///   `0010`   → forward only
///   `0011`   → forward, coded
///   `00010`  → interpolated, coded, quant
///   `00011`  → intra
///   `000001` → intra, quant
///   `000010` → backward, coded, quant
///   `000011` → forward, coded, quant
const B_ENTRIES: &[VlcEntry<MbTypeFlags>] = &[
    VlcEntry::new(2, 0b10, MbTypeFlags::new(false, true, true, false, false)),
    VlcEntry::new(2, 0b11, MbTypeFlags::new(false, true, true, true, false)),
    VlcEntry::new(3, 0b010, MbTypeFlags::new(false, false, true, false, false)),
    VlcEntry::new(3, 0b011, MbTypeFlags::new(false, false, true, true, false)),
    VlcEntry::new(
        4,
        0b0010,
        MbTypeFlags::new(false, true, false, false, false),
    ),
    VlcEntry::new(4, 0b0011, MbTypeFlags::new(false, true, false, true, false)),
    VlcEntry::new(5, 0b00010, MbTypeFlags::new(true, true, true, true, false)),
    VlcEntry::new(
        5,
        0b00011,
        MbTypeFlags::new(false, false, false, false, true),
    ),
    VlcEntry::new(
        6,
        0b000001,
        MbTypeFlags::new(true, false, false, false, true),
    ),
    VlcEntry::new(
        6,
        0b000010,
        MbTypeFlags::new(true, false, true, true, false),
    ),
    VlcEntry::new(
        6,
        0b000011,
        MbTypeFlags::new(true, true, false, true, false),
    ),
];

pub fn i_table() -> &'static VlcTable<MbTypeFlags> {
    static CELL: OnceLock<VlcTable<MbTypeFlags>> = OnceLock::new();
    CELL.get_or_init(|| VlcTable::new(I_ENTRIES.to_vec()))
}

pub fn p_table() -> &'static VlcTable<MbTypeFlags> {
    static CELL: OnceLock<VlcTable<MbTypeFlags>> = OnceLock::new();
    CELL.get_or_init(|| VlcTable::new(P_ENTRIES.to_vec()))
}

pub fn b_table() -> &'static VlcTable<MbTypeFlags> {
    static CELL: OnceLock<VlcTable<MbTypeFlags>> = OnceLock::new();
    CELL.get_or_init(|| VlcTable::new(B_ENTRIES.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_no_collision<T: Copy + std::fmt::Debug>(t: &VlcTable<T>, name: &str) {
        for (i, e) in t.entries.iter().enumerate() {
            for (j, f) in t.entries.iter().enumerate() {
                if i >= j || e.bits > f.bits {
                    continue;
                }
                let f_prefix = f.code >> (f.bits - e.bits) as u32;
                assert_ne!(
                    f_prefix, e.code,
                    "{name}: entry {i} ({}/{}) is a prefix of entry {j}",
                    e.code, e.bits
                );
            }
        }
    }

    #[test]
    fn no_collisions() {
        check_no_collision(i_table(), "i_table");
        check_no_collision(p_table(), "p_table");
        check_no_collision(b_table(), "b_table");
    }

    #[test]
    fn i_picture_intra_is_one_bit() {
        // In I-pictures every MB is intra; the cheapest MB-type code is
        // the single-bit `1`.
        let t = i_table();
        assert_eq!(t.entries[0].bits, 1);
        assert!(t.entries[0].value.intra);
        assert!(!t.entries[0].value.quant);
    }
}
