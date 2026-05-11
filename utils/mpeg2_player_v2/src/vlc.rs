//! Variable-length code decoder.
//!
//! Annex B of ISO/IEC 13818-2 enumerates VLC tables as `(codeword, length,
//! symbol)` triples with code lengths up to 17 bits.  This module ships a
//! generic table type that:
//!
//!   1. Caches `max_bits` so `decode()` doesn't re-scan to figure out how
//!      many bits to peek.
//!   2. Precomputes a 9-bit prefix LUT that resolves any codeword of length
//!      ≤ 9 bits in a single array lookup.
//!
//! Codewords longer than 9 bits fall through to a linear scan.  The LUT
//! width is enough to catch the vast majority of symbols on real content
//! while keeping the table small (512 entries per VLC).

use crate::bitstream::BitReader;
use crate::error::{Error, Result};

/// One entry in a VLC table.  `code` occupies the low `bits` bits, MSB-first.
#[derive(Clone, Copy, Debug)]
pub struct VlcEntry<T: Copy> {
    pub code: u32,
    pub bits: u8,
    pub value: T,
}

impl<T: Copy> VlcEntry<T> {
    pub const fn new(bits: u8, code: u32, value: T) -> Self {
        Self { code, bits, value }
    }
}

#[derive(Clone, Copy)]
struct LutEntry<T: Copy> {
    /// `None` means "this prefix is ambiguous — fall back to linear scan."
    /// Otherwise this is the decoded symbol and `bits` is how many to
    /// consume from the stream.
    value: Option<T>,
    bits: u8,
}

impl<T: Copy> LutEntry<T> {
    const EMPTY: Self = Self {
        value: None,
        bits: 0,
    };
}

const LUT_BITS: u8 = 9;
const LUT_SIZE: usize = 1 << LUT_BITS;

/// A VLC table with pre-computed `max_bits` and a short-prefix LUT.
pub struct VlcTable<T: Copy + 'static> {
    pub entries: Vec<VlcEntry<T>>,
    pub max_bits: u8,
    /// `min(LUT_BITS, max_bits)`.  When `max_bits` ≤ LUT_BITS the LUT
    /// covers everything and the linear fallback is only hit on malformed
    /// streams.
    lut_bits: u8,
    /// Boxed so the 512-entry table isn't copied on every lookup.
    lut: Box<[LutEntry<T>; LUT_SIZE]>,
}

impl<T: Copy + 'static> VlcTable<T> {
    pub fn new(entries: Vec<VlcEntry<T>>) -> Self {
        let max_bits = entries.iter().map(|e| e.bits).max().unwrap_or(0);
        let lut_bits = max_bits.min(LUT_BITS);
        let mut lut = Box::new([LutEntry::EMPTY; LUT_SIZE]);
        if lut_bits > 0 {
            for e in entries.iter() {
                if e.bits <= lut_bits {
                    // A `b`-bit code with value `c` matches prefixes
                    // `(c << (lut_bits - b)) | s` for every `s` in
                    // `0..2^(lut_bits - b)`.  Populate them all.
                    let shift = (lut_bits - e.bits) as u32;
                    let base = (e.code << shift) as usize;
                    let count = 1usize << shift;
                    for k in 0..count {
                        lut[base | k] = LutEntry {
                            value: Some(e.value),
                            bits: e.bits,
                        };
                    }
                }
            }
        }
        Self {
            entries,
            max_bits,
            lut_bits,
            lut,
        }
    }
}

/// Decode one symbol from the table by reading bits from `br`.
#[inline]
pub fn decode<T: Copy>(br: &mut BitReader<'_>, table: &VlcTable<T>) -> Result<T> {
    let max_bits = table.max_bits as u32;
    if max_bits == 0 {
        return Err(Error::invalid("vlc: empty table"));
    }
    let remaining = br.remaining_bits() as u32;
    let lut_bits = table.lut_bits as u32;

    // Fast path: peek `lut_bits` and look up.
    if lut_bits > 0 && remaining >= lut_bits {
        let prefix = br.peek(lut_bits as u8)? as usize;
        let entry = &table.lut[prefix];
        if let Some(v) = entry.value {
            br.skip(entry.bits as usize)?;
            return Ok(v);
        }
        // Ambiguous prefix → fall through to linear scan.
    }

    // Slow path: linear scan, useful for codes > LUT_BITS and for the
    // streams-tail case where fewer than `lut_bits` are available.
    let peek_bits = max_bits.min(remaining);
    if peek_bits == 0 {
        return Err(Error::invalid("vlc: no bits available"));
    }
    let peeked = br.peek(peek_bits as u8)?;
    let peeked_full = peeked << (max_bits - peek_bits);
    for e in &table.entries {
        if u32::from(e.bits) > peek_bits {
            continue;
        }
        let shift = max_bits - u32::from(e.bits);
        let prefix = peeked_full >> shift;
        if prefix == e.code {
            br.skip(e.bits as usize)?;
            return Ok(e.value);
        }
    }
    Err(Error::invalid("vlc: no matching codeword"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_two_codeword_table() {
        // Tiny table: "0" → 'A', "10" → 'B', "11" → 'C'.
        let entries = vec![
            VlcEntry::new(1, 0b0, 'A'),
            VlcEntry::new(2, 0b10, 'B'),
            VlcEntry::new(2, 0b11, 'C'),
        ];
        let tbl = VlcTable::new(entries);
        // Bitstream: 0 11 10 0 → A C B A.
        let data = [0b0_11_10_0_00];
        let mut br = BitReader::new(&data);
        assert_eq!(decode(&mut br, &tbl).unwrap(), 'A');
        assert_eq!(decode(&mut br, &tbl).unwrap(), 'C');
        assert_eq!(decode(&mut br, &tbl).unwrap(), 'B');
        assert_eq!(decode(&mut br, &tbl).unwrap(), 'A');
    }

    #[test]
    fn rejects_invalid_codeword() {
        // Table with only "00" — any other prefix fails to decode.
        let entries = vec![VlcEntry::new(2, 0b00, 'A')];
        let tbl = VlcTable::new(entries);
        let data = [0b11_000000];
        let mut br = BitReader::new(&data);
        assert!(decode(&mut br, &tbl).is_err());
    }
}
