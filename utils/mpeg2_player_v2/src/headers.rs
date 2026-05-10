use crate::bitstream::{
    BitReader, EXTENSION_START, GOP_START, PICTURE_START, SEQUENCE_HEADER, start_code_payloads,
};
use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AspectRatio {
    Square,
    Ratio4_3,
    Ratio16_9,
    Ratio221_1,
    Reserved(u8),
}

impl From<u8> for AspectRatio {
    fn from(value: u8) -> Self {
        match value {
            1 => Self::Square,
            2 => Self::Ratio4_3,
            3 => Self::Ratio16_9,
            4 => Self::Ratio221_1,
            other => Self::Reserved(other),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PictureType {
    I,
    P,
    B,
    D,
    Reserved(u8),
}

impl From<u8> for PictureType {
    fn from(value: u8) -> Self {
        match value {
            1 => Self::I,
            2 => Self::P,
            3 => Self::B,
            4 => Self::D,
            other => Self::Reserved(other),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SequenceInfo {
    pub width: u32,
    pub height: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    pub aspect_ratio: AspectRatio,
    pub progressive_sequence: bool,
}

#[derive(Clone, Debug)]
pub struct SequenceHeader {
    pub info: SequenceInfo,
    pub bit_rate_value: u32,
    pub vbv_buffer_size_value: u16,
    pub constrained_parameters_flag: bool,
    pub intra_quantiser_matrix: [u8; 64],
    pub non_intra_quantiser_matrix: [u8; 64],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SequenceExtension {
    pub profile_and_level_indication: u8,
    pub progressive_sequence: bool,
    pub chroma_format: u8,
    pub horizontal_size_extension: u8,
    pub vertical_size_extension: u8,
    pub frame_rate_extension_n: u8,
    pub frame_rate_extension_d: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PictureHeader {
    pub temporal_reference: u16,
    pub picture_coding_type: PictureType,
    pub vbv_delay: u16,
    pub full_pel_forward_vector: bool,
    pub forward_f_code: u8,
    pub full_pel_backward_vector: bool,
    pub backward_f_code: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PictureCodingExtension {
    pub f_code: [[u8; 2]; 2],
    pub intra_dc_precision: u8,
    pub picture_structure: u8,
    pub top_field_first: bool,
    pub frame_pred_frame_dct: bool,
    pub concealment_motion_vectors: bool,
    pub q_scale_type: bool,
    pub intra_vlc_format: bool,
    pub alternate_scan: bool,
    pub repeat_first_field: bool,
    pub chroma_420_type: bool,
    pub progressive_frame: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GopHeader {
    pub drop_frame: bool,
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub closed_gop: bool,
    pub broken_link: bool,
}

pub fn parse_sequence_header(payload: &[u8]) -> Result<SequenceHeader> {
    let mut br = BitReader::new(payload);
    let width = br.read(12)?;
    let height = br.read(12)?;
    let aspect_ratio = AspectRatio::from(br.read(4)? as u8);
    let frame_rate_code = br.read(4)? as u8;
    let (frame_rate_num, frame_rate_den) = frame_rate_from_code(frame_rate_code)?;
    let bit_rate_value = br.read(18)?;
    let marker = br.read(1)?;
    if marker != 1 {
        return Err(Error::invalid("sequence header marker_bit was not set"));
    }
    let vbv_buffer_size_value = br.read(10)? as u16;
    let constrained_parameters_flag = br.read_bool()?;
    let mut intra_quantiser_matrix = crate::tables::q_matrices::DEFAULT_INTRA_QUANTISER_MATRIX;
    if br.read_bool()? {
        for value in &mut intra_quantiser_matrix {
            *value = br.read(8)? as u8;
        }
    }
    let mut non_intra_quantiser_matrix =
        crate::tables::q_matrices::DEFAULT_NON_INTRA_QUANTISER_MATRIX;
    if br.remaining_bits() > 0 && br.read_bool()? {
        for value in &mut non_intra_quantiser_matrix {
            *value = br.read(8)? as u8;
        }
    }
    Ok(SequenceHeader {
        info: SequenceInfo {
            width,
            height,
            frame_rate_num,
            frame_rate_den,
            aspect_ratio,
            progressive_sequence: true,
        },
        bit_rate_value,
        vbv_buffer_size_value,
        constrained_parameters_flag,
        intra_quantiser_matrix,
        non_intra_quantiser_matrix,
    })
}

pub fn parse_sequence_extension(payload: &[u8]) -> Result<SequenceExtension> {
    let mut br = BitReader::new(payload);
    let extension_id = br.read(4)?;
    if extension_id != 1 {
        return Err(Error::invalid("not a sequence extension"));
    }
    let profile_and_level_indication = br.read(8)? as u8;
    let progressive_sequence = br.read_bool()?;
    let chroma_format = br.read(2)? as u8;
    if chroma_format != 1 {
        return Err(Error::unsupported(format!(
            "chroma_format {chroma_format}; only 4:2:0 is supported"
        )));
    }
    Ok(SequenceExtension {
        profile_and_level_indication,
        progressive_sequence,
        chroma_format,
        horizontal_size_extension: br.read(2)? as u8,
        vertical_size_extension: br.read(2)? as u8,
        frame_rate_extension_n: {
            br.skip(12)?;
            if br.read(1)? != 1 {
                return Err(Error::invalid("sequence extension marker_bit was not set"));
            }
            br.skip(9)?;
            br.read(2)? as u8
        },
        frame_rate_extension_d: br.read(5)? as u8,
    })
}

pub fn parse_picture_header(payload: &[u8]) -> Result<PictureHeader> {
    let mut br = BitReader::new(payload);
    let temporal_reference = br.read(10)? as u16;
    let picture_coding_type = PictureType::from(br.read(3)? as u8);
    let vbv_delay = br.read(16)? as u16;
    let mut out = PictureHeader {
        temporal_reference,
        picture_coding_type,
        vbv_delay,
        full_pel_forward_vector: false,
        forward_f_code: 0,
        full_pel_backward_vector: false,
        backward_f_code: 0,
    };
    if matches!(out.picture_coding_type, PictureType::P | PictureType::B) {
        out.full_pel_forward_vector = br.read_bool()?;
        out.forward_f_code = br.read(3)? as u8;
    }
    if matches!(out.picture_coding_type, PictureType::B) {
        out.full_pel_backward_vector = br.read_bool()?;
        out.backward_f_code = br.read(3)? as u8;
    }
    Ok(out)
}

pub fn parse_picture_coding_extension(payload: &[u8]) -> Result<PictureCodingExtension> {
    let mut br = BitReader::new(payload);
    if br.read(4)? != 8 {
        return Err(Error::invalid("not a picture coding extension"));
    }
    let mut f_code = [[0; 2]; 2];
    for row in &mut f_code {
        for value in row {
            *value = br.read(4)? as u8;
        }
    }
    let ext = PictureCodingExtension {
        f_code,
        intra_dc_precision: br.read(2)? as u8,
        picture_structure: br.read(2)? as u8,
        top_field_first: br.read_bool()?,
        frame_pred_frame_dct: br.read_bool()?,
        concealment_motion_vectors: br.read_bool()?,
        q_scale_type: br.read_bool()?,
        intra_vlc_format: br.read_bool()?,
        alternate_scan: br.read_bool()?,
        repeat_first_field: br.read_bool()?,
        chroma_420_type: br.read_bool()?,
        progressive_frame: br.read_bool()?,
    };
    if ext.picture_structure != 0b11 {
        return Err(Error::unsupported(
            "field pictures are out of scope; only frame pictures are supported",
        ));
    }
    if ext.concealment_motion_vectors {
        return Err(Error::unsupported("concealment motion vectors"));
    }
    Ok(ext)
}

pub fn parse_gop_header(payload: &[u8]) -> Result<GopHeader> {
    let mut br = BitReader::new(payload);
    Ok(GopHeader {
        drop_frame: br.read_bool()?,
        hours: br.read(5)? as u8,
        minutes: br.read(6)? as u8,
        seconds: {
            br.read(1)?;
            br.read(6)? as u8
        },
        frames: br.read(6)? as u8,
        closed_gop: br.read_bool()?,
        broken_link: br.read_bool()?,
    })
}

pub fn parse_sequence_info(es: &[u8]) -> Result<SequenceInfo> {
    let mut sequence = None;
    for (start, payload) in start_code_payloads(es) {
        match start.code {
            SEQUENCE_HEADER => sequence = Some(parse_sequence_header(payload)?),
            EXTENSION_START if payload.first().map(|b| b >> 4) == Some(1) => {
                let ext = parse_sequence_extension(payload)?;
                if let Some(seq) = sequence.as_mut() {
                    seq.info.progressive_sequence = ext.progressive_sequence;
                    seq.info.width |= u32::from(ext.horizontal_size_extension) << 12;
                    seq.info.height |= u32::from(ext.vertical_size_extension) << 12;
                    seq.info.frame_rate_num *= u32::from(ext.frame_rate_extension_n) + 1;
                    seq.info.frame_rate_den *= u32::from(ext.frame_rate_extension_d) + 1;
                }
            }
            PICTURE_START | GOP_START => break,
            _ => {}
        }
    }
    sequence
        .map(|seq| seq.info)
        .ok_or(Error::NeedSequenceHeader)
}

fn frame_rate_from_code(code: u8) -> Result<(u32, u32)> {
    match code {
        1 => Ok((24000, 1001)),
        2 => Ok((24, 1)),
        3 => Ok((25, 1)),
        4 => Ok((30000, 1001)),
        5 => Ok((30, 1)),
        6 => Ok((50, 1)),
        7 => Ok((60000, 1001)),
        8 => Ok((60, 1)),
        _ => Err(Error::invalid(format!("reserved frame_rate_code {code}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hand_built_sequence_header() {
        let payload = [0x20, 0x01, 0xc0, 0x34, 0x00, 0x00, 0x20, 0x00];
        let seq = parse_sequence_header(&payload).unwrap();
        assert_eq!(seq.info.width, 512);
        assert_eq!(seq.info.height, 448);
        assert_eq!(seq.info.frame_rate_num, 30000);
        assert_eq!(seq.info.frame_rate_den, 1001);
    }
}
