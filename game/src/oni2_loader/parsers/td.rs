use bevy::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct TdSplit {
    pub name: String,
    pub vag_index: usize,
    pub parm_index: usize,
    pub loop_flag: bool,
}

#[derive(Debug, Clone)]
pub struct TdProgram {
    pub name: String,
    pub splits: Vec<TdSplit>,
}

#[derive(Resource, Default, Debug, Clone)]
pub struct SoundBankDirectory {
    /// Maps "ProgramName:SplitName" -> (BankFileName, VAGIndex)
    pub sounds: HashMap<String, (String, usize)>,
}

pub fn parse_td_file(path: &Path) -> Option<TdProgram> {
    let content = fs::read_to_string(path).ok()?;
    let mut lines = content.lines().map(|l| l.trim()).filter(|l| !l.is_empty());
    
    let mut program_name = String::new();
    let mut splits = Vec::new();
    
    while let Some(line) = lines.next() {
        if line.starts_with("PROGRAM ") {
            program_name = line["PROGRAM ".len()..].trim().to_string();
        } else if line.starts_with("NUMSPLT ") {
            let num_splits_str = line["NUMSPLT ".len()..].trim();
            if let Ok(num) = num_splits_str.parse::<usize>() {
                for _ in 0..num {
                    let split_name = match lines.next() {
                        Some(n) => n.to_string(),
                        None => break,
                    };
                    let vag_idx = lines.next().and_then(|l| l.parse::<usize>().ok()).unwrap_or(0);
                    let parm_idx = lines.next().and_then(|l| l.parse::<usize>().ok()).unwrap_or(0);
                    let loop_val = lines.next().and_then(|l| l.parse::<u8>().ok()).unwrap_or(0);
                    
                    splits.push(TdSplit {
                        name: split_name,
                        vag_index: vag_idx,
                        parm_index: parm_idx,
                        loop_flag: loop_val > 0,
                    });
                }
            }
        }
    }
    
    if program_name.is_empty() {
        None
    } else {
        Some(TdProgram {
            name: program_name,
            splits,
        })
    }
}

pub fn load_all_tds(search_dir: &Path) -> SoundBankDirectory {
    let mut dir = SoundBankDirectory::default();
    
    // We expect search_dir to be oni2/zips/assets/Audio/banks
    if let Ok(entries) = fs::read_dir(search_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("td") {
                if let Some(mut prog) = parse_td_file(&path) {
                    let bank_base = path.file_stem().unwrap().to_string_lossy().to_string();
                    
                    for split in prog.splits {
                        let full_name = format!("{}:{}", prog.name, split.name);
                        dir.sounds.insert(full_name, (bank_base.clone(), split.vag_index));
                    }
                }
            }
        }
    }
    
    dir
}
