use std::env;
use std::fs::File;
use std::io::{Read, Write};

const INDEX_TABLE: [i32; 16] = [
    -1, -1, -1, -1, 2, 4, 6, 8,
    -1, -1, -1, -1, 2, 4, 6, 8,
];

const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408,
    449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630,
    9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];

fn decode_nibble(nibble: u8, valpred: &mut i32, index: &mut i32) -> i16 {
    let step = STEP_TABLE[*index as usize];
    let mut diff = step >> 3;
    if (nibble & 1) != 0 { diff += step >> 2; }
    if (nibble & 2) != 0 { diff += step >> 1; }
    if (nibble & 4) != 0 { diff += step; }
    if (nibble & 8) != 0 {
        *valpred -= diff;
    } else {
        *valpred += diff;
    }
    *valpred = (*valpred).clamp(-32768, 32767);
    
    *index += INDEX_TABLE[nibble as usize];
    *index = (*index).clamp(0, 88);
    
    *valpred as i16
}

fn write_wav(filename: &str, samples: &[i16], sample_rate: u32, channels: u16) {
    let mut file = File::create(filename).unwrap();
    let data_size = (samples.len() * 2) as u32;
    let file_size = 36 + data_size;
    
    file.write_all(b"RIFF").unwrap();
    file.write_all(&file_size.to_le_bytes()).unwrap();
    file.write_all(b"WAVE").unwrap();
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    let byte_rate = sample_rate * channels as u32 * 2;
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    let block_align = channels * 2;
    file.write_all(&block_align.to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    
    file.write_all(b"data").unwrap();
    file.write_all(&data_size.to_le_bytes()).unwrap();
    
    for &sample in samples {
        file.write_all(&sample.to_le_bytes()).unwrap();
    }
}

fn main() {
    let in_file = "../oni2/zips/assets/movies/angel.imf";
    let out_file = "angel.wav";
    
    let mut f = File::open(in_file).expect("Failed to open imf file");
    let mut data = Vec::new();
    f.read_to_end(&mut data).unwrap();
    
    println!("File size: {}", data.len());
    
    // Parse header
    let magic = &data[0..4];
    println!("Magic: {:?}", std::str::from_utf8(magic).unwrap_or("invalid"));
    
    let sample_rate = u32::from_le_bytes(data[8..12].try_into().unwrap());
    println!("Sample rate field: {}", sample_rate);
    
    let mut valpred = 0;
    let mut index = 0;
    let mut samples = Vec::new();
    
    // Start after 0x800 header
    for &byte in &data[0x800..] {
        // usually IMA ADPCM decodes low nibble then high nibble
        let nibble1 = byte & 0x0F;
        let nibble2 = byte >> 4;
        
        samples.push(decode_nibble(nibble1, &mut valpred, &mut index));
        samples.push(decode_nibble(nibble2, &mut valpred, &mut index));
    }
    
    println!("Decoded {} samples", samples.len());
    
    // Fallback sample rate to 32768 or 44100 if the field looks wrong
    let sr = if sample_rate > 8000 && sample_rate < 48000 { sample_rate } else { 32768 };
    
    write_wav(out_file, &samples, sr, 1);
    println!("Wrote to {}", out_file);
}
