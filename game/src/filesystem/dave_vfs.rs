use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use flate2::read::DeflateDecoder;
use bevy::log::{info, warn};

use crate::vfs::{Vfs, VfsEntry};

#[derive(Clone, Debug)]
struct DaveEntry {
    name: String,
    uncompressed_size: u32,
    file_offset: u32,
    compressed_size: u32,
}

pub struct DaveVfs {
    archive_path: String,
    files: HashMap<String, DaveEntry>,
    directories: HashMap<String, Vec<VfsEntry>>,
}

impl DaveVfs {
    pub fn new(archive_path: &str) -> std::io::Result<Self> {
        let path = std::path::Path::new(archive_path);
        
        let resolved_path = if path.is_dir() {
            path.join("RB.DAT")
        } else {
            path.to_path_buf()
        };

        if !resolved_path.exists() {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("Archive not found at {:?}", resolved_path)));
        }

        let mut file = File::open(&resolved_path)?;
        
        let mut header = [0u8; 2048];
        file.read_exact(&mut header)?;
        
        if &header[0..4] != b"DAVE" {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Not a valid Dave archive"));
        }
        
        let file_count = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let dir_size = u32::from_le_bytes(header[8..12].try_into().unwrap()) as u64;
        
        // Read directory entries block at 0x800
        file.seek(SeekFrom::Start(0x800))?;
        let mut dir_entries = vec![0u8; file_count * 16];
        file.read_exact(&mut dir_entries)?;
        
        // Read string table
        file.seek(SeekFrom::Start(0x800 + dir_size))?;
        let mut string_table = vec![0u8; 1024 * 1024 * 4]; // 4MB should cover any string table.
        let bytes_read = file.read(&mut string_table)?;
        string_table.truncate(bytes_read);
        
        let mut files = HashMap::new();
        let mut directories: HashMap<String, Vec<VfsEntry>> = HashMap::new();
        
        for i in 0..file_count {
            let chunk = &dir_entries[i*16 .. (i+1)*16];
            let name_offset = u32::from_le_bytes(chunk[0..4].try_into().unwrap()) as usize;
            let file_offset = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
            let uncompressed = u32::from_le_bytes(chunk[8..12].try_into().unwrap());
            let compressed = u32::from_le_bytes(chunk[12..16].try_into().unwrap());
            
            // Extract string from table at name_offset
            let end = string_table[name_offset..].iter().position(|&b| b == 0).unwrap_or(0);
            let raw_name = &string_table[name_offset .. name_offset+end];
            let name = String::from_utf8_lossy(raw_name).to_string().replace('\\', "/");
            
            let is_dir = name.ends_with('/');
            let normalized_name = name.trim_end_matches('/').to_string();
            
            // Keep track of paths
            if !is_dir {
                files.insert(normalized_name.to_lowercase(), DaveEntry {
                    name: normalized_name.clone(),
                    uncompressed_size: uncompressed,
                    file_offset,
                    compressed_size: compressed,
                });
            }

            // Recursively construct directories
            let mut current = normalized_name;
            let mut current_is_dir = is_dir;
            
            while let Some(slash_idx) = current.rfind('/') {
                let parent = current[..slash_idx].to_string();
                let basic_name = current[slash_idx+1..].to_string();
                
                let list = directories.entry(parent.clone()).or_insert_with(Vec::new);
                if let Some(existing) = list.iter_mut().find(|e| e.path == basic_name) {
                    if current_is_dir {
                        existing.is_dir = true;
                        existing.is_file = false;
                    }
                } else {
                    list.push(VfsEntry {
                        path: basic_name,
                        is_dir: current_is_dir,
                        is_file: !current_is_dir,
                    });
                }
                
                current = parent;
                current_is_dir = true; // The parent is a directory
            }
            
            // Top-level root directory
            if !current.is_empty() {
                let list = directories.entry("".to_string()).or_insert_with(Vec::new);
                if !list.iter().any(|e| e.path == current) {
                    list.push(VfsEntry {
                        path: current.clone(),
                        is_dir: true,
                        is_file: false,
                    });
                }
            }
        }
        
        info!("Mounted Dave VFS: {} files, {} directories", files.len(), directories.len());
        
        Ok(Self { archive_path: resolved_path.to_str().unwrap_or(archive_path).to_string(), files, directories })
    }
    
    fn resolve(&self, dir: &str, filename: &str) -> String {
        let path = if dir.is_empty() { filename.to_string() } else { format!("{}/{}", dir, filename) };
        let normalized = path.replace('\\', "/").to_lowercase();
        if normalized.starts_with('/') {
            normalized[1..].to_string()
        } else {
            normalized
        }
    }
}

impl Vfs for DaveVfs {
    fn read(&self, dir: &str, filename: &str) -> io::Result<Vec<u8>> {
        let path_key = self.resolve(dir, filename);
        if let Some(entry) = self.files.get(&path_key) {
            let mut file = File::open(&self.archive_path)?;
            file.seek(SeekFrom::Start(entry.file_offset as u64))?;
            
            let mut raw = vec![0u8; entry.compressed_size as usize];
            file.read_exact(&mut raw)?;
            
            if entry.compressed_size > 0 && entry.compressed_size < entry.uncompressed_size {
                // Compressed using Deflate
                // The python test confirmed that 1A FA 25 DD is a 4-byte header wrapper
                if raw.len() >= 4 && &raw[0..4] == &[0x1A, 0xFA, 0x25, 0xDD] {
                    let mut decoder = DeflateDecoder::new(&raw[4..]);
                    let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
                    decoder.read_to_end(&mut out)?;
                    info!("DaveVfs Read (HIT): {} -> {} bytes (decompressed)", path_key, out.len());
                    Ok(out)
                } else {
                    // Raw deflate without 4-byte header? Python script showed this didn't exist, but just in case
                    let mut decoder = DeflateDecoder::new(&raw[..]);
                    let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
                    decoder.read_to_end(&mut out)?;
                    info!("DaveVfs Read (HIT): {} -> {} bytes (decompressed)", path_key, out.len());
                    Ok(out)
                }
            } else {
                info!("DaveVfs Read (HIT): {} -> {} bytes (uncompressed)", path_key, raw.len());
                Ok(raw)
            }
        } else {
             warn!("DaveVfs Read (MISS): {} -> File not found in archive", path_key);
             Err(io::Error::new(io::ErrorKind::NotFound, "File not found in Dave VFS"))
        }
    }

    fn read_to_string(&self, dir: &str, filename: &str) -> io::Result<String> {
        let bytes = self.read(dir, filename)?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    fn read_dir(&self, path: &str) -> io::Result<Vec<VfsEntry>> {
        let normalized = path.replace('\\', "/").to_lowercase();
        let search = if normalized.starts_with('/') { normalized[1..].to_string() } else { normalized };
        
        // Find a matching directory key (since we stored them case sensitive originally, maybe lowercased would be better?)
        // Let's do a case insensitive match against the directories map
        for (k, v) in &self.directories {
            if k.to_lowercase() == search {
                // To avoid cloning all entries every time, we copy the metadata
                let mut list = Vec::new();
                for e in v {
                    let full_path = if search.is_empty() {
                        e.path.clone()
                    } else {
                        // Use the original search term to preserve its case
                        let clean_path = if path.starts_with('/') { &path[1..] } else { path };
                        format!("{}/{}", clean_path, e.path)
                    };

                    list.push(VfsEntry {
                        path: full_path,
                        is_dir: e.is_dir,
                        is_file: e.is_file,
                    });
                }
                info!("DaveVfs ReadDir (HIT): {} -> {} items", search, list.len());
                return Ok(list);
            }
        }
        warn!("DaveVfs ReadDir (MISS): {} -> Directory not found in archive", search);
        Err(io::Error::new(io::ErrorKind::NotFound, "Directory not found in Dave VFS"))
    }

    fn exists(&self, dir: &str, filename: &str) -> bool {
        let path_key = self.resolve(dir, filename);
        self.files.contains_key(&path_key)
    }
}
