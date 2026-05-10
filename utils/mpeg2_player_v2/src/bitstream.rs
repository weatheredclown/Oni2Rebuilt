use crate::error::{Error, Result};

pub const START_CODE_PREFIX: [u8; 3] = [0, 0, 1];
pub const PICTURE_START: u8 = 0x00;
pub const SLICE_MIN: u8 = 0x01;
pub const SLICE_MAX: u8 = 0xaf;
pub const USER_DATA: u8 = 0xb2;
pub const SEQUENCE_HEADER: u8 = 0xb3;
pub const EXTENSION_START: u8 = 0xb5;
pub const SEQUENCE_END: u8 = 0xb7;
pub const GOP_START: u8 = 0xb8;
pub const PACK_START: u8 = 0xba;
pub const SYSTEM_HEADER: u8 = 0xbb;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StartCode {
    pub offset: usize,
    pub code: u8,
}

pub fn find_start_codes(data: &[u8]) -> impl Iterator<Item = StartCode> + '_ {
    data.windows(4).enumerate().filter_map(|(offset, window)| {
        (window[0..3] == START_CODE_PREFIX).then_some(StartCode {
            offset,
            code: window[3],
        })
    })
}

pub fn start_code_payloads(data: &[u8]) -> Vec<(StartCode, &[u8])> {
    let starts: Vec<_> = find_start_codes(data).collect();
    starts
        .iter()
        .enumerate()
        .map(|(idx, start)| {
            let payload_start = start.offset + 4;
            let payload_end = starts
                .get(idx + 1)
                .map(|next| next.offset)
                .unwrap_or(data.len());
            (*start, &data[payload_start..payload_end])
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    pub fn remaining_bits(&self) -> usize {
        self.data.len() * 8 - self.bit_pos
    }

    pub fn bit_pos(&self) -> usize {
        self.bit_pos
    }

    pub fn read(&mut self, bits: u8) -> Result<u32> {
        if bits > 32 {
            return Err(Error::invalid("BitReader::read supports at most 32 bits"));
        }
        if self.remaining_bits() < bits as usize {
            return Err(Error::EndOfStream);
        }
        let mut value = 0u32;
        for _ in 0..bits {
            let byte = self.data[self.bit_pos / 8];
            let shift = 7 - (self.bit_pos % 8);
            value = (value << 1) | u32::from((byte >> shift) & 1);
            self.bit_pos += 1;
        }
        Ok(value)
    }

    pub fn read_bool(&mut self) -> Result<bool> {
        Ok(self.read(1)? != 0)
    }

    pub fn peek(&self, bits: u8) -> Result<u32> {
        let mut clone = self.clone();
        clone.read(bits)
    }

    pub fn skip(&mut self, bits: usize) -> Result<()> {
        if self.remaining_bits() < bits {
            return Err(Error::EndOfStream);
        }
        self.bit_pos += bits;
        Ok(())
    }

    pub fn align_to_byte(&mut self) {
        self.bit_pos = self.bit_pos.div_ceil(8) * 8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bits_across_byte_boundaries() {
        let mut br = BitReader::new(&[0b1011_0010, 0b0110_0000]);
        assert_eq!(br.read(3).unwrap(), 0b101);
        assert_eq!(br.read(7).unwrap(), 0b100_1001);
        br.align_to_byte();
        assert_eq!(br.bit_pos(), 16);
    }

    #[test]
    fn finds_start_codes() {
        let data = [9, 0, 0, 1, 0xb3, 1, 2, 0, 0, 1, 0x00];
        let codes: Vec<_> = find_start_codes(&data).collect();
        assert_eq!(
            codes[0],
            StartCode {
                offset: 1,
                code: 0xb3
            }
        );
        assert_eq!(
            codes[1],
            StartCode {
                offset: 7,
                code: 0x00
            }
        );
    }
}
