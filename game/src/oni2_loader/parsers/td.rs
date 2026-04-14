/*
 * oni2_loader/parsers/td.rs — TD (trigger/dialogue) audio split parser.
 *
 * TdSplit: named entry with VAG and parameter indices into an audio bank.
 * parse_td: reads the .td text format for a dialogue bank and returns a list
 * of TdSplits used to locate and play voiced dialogue lines at runtime.
 */
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

pub fn parse_td_file(content: &str) -> Option<TdProgram> {
    let mut lines = content.lines().filter_map(|l| {
        let no_comment = if let Some(idx) = l.find("//") { &l[..idx] } else { l };
        let trimmed = no_comment.trim();
        if trimmed.is_empty() { None } else { Some(trimmed) }
    });
    
    let mut program_name = String::new();
    let mut splits = Vec::new();
    
    while let Some(line) = lines.next() {
        if line.starts_with("PROGRAM") {
            program_name = line.strip_prefix("PROGRAM").unwrap().trim().to_string();
        } else if line.starts_with("NUMSPLT") {
            let num_splits_str = line.strip_prefix("NUMSPLT").unwrap().trim();
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

pub fn load_all_tds() -> SoundBankDirectory {
    let mut dir = SoundBankDirectory::default();
    
    let search_dir = "Audio/banks";
    match crate::vfs::read_dir(search_dir) {
        Ok(entries) => {
            let mut td_count = 0;
            for entry in entries {
                let path = entry.path;
                if path.to_lowercase().ends_with(".td") {
                    if let Ok(content) = crate::vfs::read_to_string("", &path) {
                        if let Some(mut prog) = parse_td_file(&content) {
                            let bank_base = std::path::Path::new(&path).file_stem().unwrap().to_string_lossy().to_string();
                            
                            for split in prog.splits {
                                let full_name = format!("{}:{}", prog.name, split.name);
                                dir.sounds.insert(full_name, (bank_base.clone(), split.vag_index));
                            }
                            td_count += 1;
                        } else {
                            warn!("Failed to parse valid TdProgram from: {}", path);
                        }
                    }
                }
            }
            info!("Loaded {} .td banks from VFS namespace '{}'", td_count, search_dir);
        }
        Err(e) => {
            warn!("Failed to read .td manifest directory at VFS path {}: {}", search_dir, e);
        }
    }
    
    dir
}
