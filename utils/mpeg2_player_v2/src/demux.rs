use crate::bitstream::{PACK_START, SYSTEM_HEADER, find_start_codes};
use crate::error::{Error, Result};

pub fn extract_video_elementary_stream(data: &[u8]) -> Result<Vec<u8>> {
    if looks_like_program_stream(data) {
        demux_program_stream(data)
    } else {
        Ok(data.to_vec())
    }
}

pub fn looks_like_program_stream(data: &[u8]) -> bool {
    find_start_codes(data).any(|start| {
        start.code == PACK_START
            || start.code == SYSTEM_HEADER
            || (0xe0..=0xef).contains(&start.code)
    })
}

pub fn demux_program_stream(data: &[u8]) -> Result<Vec<u8>> {
    let starts: Vec<_> = find_start_codes(data).collect();
    let mut out = Vec::new();
    for (idx, start) in starts.iter().enumerate() {
        if !(0xe0..=0xef).contains(&start.code) {
            continue;
        }
        let next = starts
            .get(idx + 1)
            .map(|next| next.offset)
            .unwrap_or(data.len());
        let mut offset = start.offset + 4;
        if offset + 2 > next {
            continue;
        }
        let packet_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        let packet_end = if packet_len == 0 {
            next
        } else {
            (offset + packet_len).min(data.len())
        };
        if offset >= packet_end {
            continue;
        }
        offset = skip_pes_header(data, offset, packet_end)?;
        if offset < packet_end {
            out.extend_from_slice(&data[offset..packet_end]);
        }
    }
    if out.is_empty() {
        return Err(Error::invalid(
            "program stream did not contain video PES payloads",
        ));
    }
    Ok(out)
}

fn skip_pes_header(data: &[u8], mut offset: usize, packet_end: usize) -> Result<usize> {
    if offset + 3 <= packet_end && data[offset] & 0xc0 == 0x80 {
        let header_len = data[offset + 2] as usize;
        offset += 3 + header_len;
        return Ok(offset.min(packet_end));
    }

    while offset < packet_end && data[offset] == 0xff {
        offset += 1;
    }
    if offset < packet_end && data[offset] & 0xc0 == 0x40 {
        offset += 2;
    }
    if offset >= packet_end {
        return Ok(offset);
    }
    let marker = data[offset] & 0xf0;
    offset += match marker {
        0x20 => 5,
        0x30 => 10,
        0x00 => 1,
        _ => 0,
    };
    Ok(offset.min(packet_end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitstream::{PACK_START, SEQUENCE_HEADER};

    #[test]
    fn extracts_mpeg2_video_pes_payload() {
        let mut ps = vec![0, 0, 1, PACK_START];
        ps.extend_from_slice(&[0xff; 14]);
        ps.extend_from_slice(&[0, 0, 1, 0xe0, 0, 7, 0x80, 0x00, 0x00]);
        ps.extend_from_slice(&[0, 0, 1, SEQUENCE_HEADER]);
        let es = demux_program_stream(&ps).unwrap();
        assert_eq!(es, [0, 0, 1, SEQUENCE_HEADER]);
    }
}
