/*
 * filesystem/vfs.rs — VFS trait, DiskVfs, and MultiVfs implementations.
 *
 * Vfs trait: read_file(dir, name), read_to_string, exists, read_dir.
 * DiskVfs: reads from a real directory on disk.
 * MultiVfs: chains multiple Vfs backends — searches in insertion order.
 * set_vfs() / get_vfs() provide a global singleton used throughout the codebase.
 */
use std::fs;
use std::path::{Path, PathBuf};

use bevy::log::warn;

pub struct VfsEntry {
    pub path: String,
    pub is_dir: bool,
    pub is_file: bool,
}

pub trait Vfs: Send + Sync {
    fn read(&self, dir: &str, filename: &str) -> std::io::Result<Vec<u8>>;
    fn read_to_string(&self, dir: &str, filename: &str) -> std::io::Result<String>;
    fn read_dir(&self, path: &str) -> std::io::Result<Vec<VfsEntry>>;
    fn exists(&self, dir: &str, filename: &str) -> bool;
}

pub struct DiskVfs {
    pub base_path: String,
}

pub struct MultiVfs {
    mounts: Vec<Box<dyn Vfs>>,
}

impl MultiVfs {
    pub fn new() -> Self {
        Self { mounts: Vec::new() }
    }

    pub fn push(&mut self, vfs: Box<dyn Vfs>) {
        self.mounts.push(vfs);
    }
}

impl Default for MultiVfs {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfs for MultiVfs {
    fn read(&self, dir: &str, filename: &str) -> std::io::Result<Vec<u8>> {
        for vfs in &self.mounts {
            if vfs.exists(dir, filename) {
                return vfs.read(dir, filename);
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "File not found in any VFS mount",
        ))
    }

    fn read_to_string(&self, dir: &str, filename: &str) -> std::io::Result<String> {
        for vfs in &self.mounts {
            if vfs.exists(dir, filename) {
                return vfs.read_to_string(dir, filename);
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "File not found in any VFS mount",
        ))
    }

    fn read_dir(&self, path: &str) -> std::io::Result<Vec<VfsEntry>> {
        let mut all_entries = Vec::new();
        let mut any_success = false;

        for vfs in &self.mounts {
            if let Ok(entries) = vfs.read_dir(path) {
                any_success = true;
                for e in entries {
                    if !all_entries
                        .iter()
                        .any(|existing: &VfsEntry| existing.path.eq_ignore_ascii_case(&e.path))
                    {
                        all_entries.push(e);
                    }
                }
            }
        }

        if !any_success {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Directory not found in any VFS mount",
            ));
        }

        Ok(all_entries)
    }

    fn exists(&self, dir: &str, filename: &str) -> bool {
        self.mounts.iter().any(|vfs| vfs.exists(dir, filename))
    }
}

impl Vfs for DiskVfs {
    fn read(&self, dir: &str, filename: &str) -> std::io::Result<Vec<u8>> {
        let path = if dir.is_empty() {
            filename.to_string()
        } else {
            format!("{}/{}", dir, filename)
        };
        let resolved = self.resolve(&path);
        match fs::read(&resolved) {
            Ok(data) => Ok(data),
            Err(e) => {
                warn!("VFS Read (MISS): {} -> {}", path, e);
                Err(e)
            }
        }
    }

    fn read_to_string(&self, dir: &str, filename: &str) -> std::io::Result<String> {
        let path = if dir.is_empty() {
            filename.to_string()
        } else {
            format!("{}/{}", dir, filename)
        };
        let resolved = self.resolve(&path);
        match fs::read_to_string(&resolved) {
            Ok(data) => Ok(data),
            Err(e) => {
                warn!("VFS ReadString (MISS): {} -> {}", path, e);
                Err(e)
            }
        }
    }

    fn read_dir(&self, path: &str) -> std::io::Result<Vec<VfsEntry>> {
        let resolved = self.resolve(path);
        let dir_res = fs::read_dir(&resolved);

        if let Err(ref e) = dir_res {
            warn!("VFS ReadDir (MISS): {} -> {}", path, e);
            return Err(dir_res.unwrap_err());
        }

        let mut result = Vec::new();
        for entry in dir_res.unwrap() {
            let entry = entry?;
            let ft = entry.file_type()?;
            let full_path = entry.path();

            // Strip the base path to return relative paths mapping to our VFS
            let rel_path = full_path
                .strip_prefix(Path::new(&self.base_path))
                .unwrap_or(&full_path)
                .to_string_lossy()
                .to_string()
                // Normalize slashes for consistency
                .replace('\\', "/");

            // Avoid adding leading slash
            let final_path = if rel_path.starts_with('/') {
                rel_path[1..].to_string()
            } else {
                rel_path
            };

            result.push(VfsEntry {
                path: final_path,
                is_dir: ft.is_dir(),
                is_file: ft.is_file(),
            });
        }
        Ok(result)
    }

    fn exists(&self, dir: &str, filename: &str) -> bool {
        let path = if dir.is_empty() {
            filename.to_string()
        } else {
            format!("{}/{}", dir, filename)
        };

        // TODO: Think about how to warn about expected files (e.g. files referenced by other files
        // are expected to exist because the inter-asset integrity is expected to be correct),
        // whereas other files are optional and loaded greedily if available.
        // It would be great to have a way to distinguish this in the VFS API so that we only warn
        // when a strictly expected file is missing, rather than generating warnings for optional probing.
        Path::new(&self.resolve(&path)).exists()
    }
}

impl DiskVfs {
    pub fn new(base_path: String) -> Self {
        Self { base_path }
    }

    fn resolve(&self, path: &str) -> String {
        let normalized = path.replace('\\', "/");
        // Strip leading slashes to prevent PathBuf from treating it as absolute
        let clean_path = if normalized.starts_with('/') {
            &normalized[1..]
        } else {
            &normalized
        };

        let exact = Path::new(&self.base_path).join(clean_path);
        if exact.exists() {
            return exact.to_string_lossy().to_string();
        }

        // Case-insensitive traversal fallback
        let mut current_path = PathBuf::from(&self.base_path);
        let components: Vec<&str> = clean_path.split('/').collect();

        for comp in components {
            if comp.is_empty() {
                continue;
            }
            let exact_comp = current_path.join(comp);
            if exact_comp.exists() {
                current_path = exact_comp;
            } else {
                let lower_comp = comp.to_lowercase();
                let mut found = false;
                if let Ok(entries) = fs::read_dir(&current_path) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.to_lowercase() == lower_comp {
                            current_path.push(name);
                            found = true;
                            break;
                        }
                    }
                }
                if !found {
                    current_path.push(comp);
                }
            }
        }
        current_path.to_string_lossy().to_string()
    }
}

// Global VFS Singleton
static VFS_INSTANCE: std::sync::OnceLock<Box<dyn Vfs>> = std::sync::OnceLock::new();

/// Initialize the global virtual file system. Must be called exactly once at startup.
pub fn set_vfs(vfs: Box<dyn Vfs>) {
    if VFS_INSTANCE.set(vfs).is_err() {
        println!("Vfs already initialized");
    }
}

fn get_vfs() -> &'static dyn Vfs {
    VFS_INSTANCE
        .get()
        .expect("Vfs not initialized! Call vfs::set_vfs() at startup.")
        .as_ref()
}

// Proxy functions acting like std::fs
pub fn read(dir: &str, filename: &str) -> std::io::Result<Vec<u8>> {
    get_vfs().read(dir, filename)
}

pub fn read_to_string(dir: &str, filename: &str) -> std::io::Result<String> {
    get_vfs().read_to_string(dir, filename)
}

pub fn read_dir(path: &str) -> std::io::Result<Vec<VfsEntry>> {
    get_vfs().read_dir(path)
}

pub fn exists(dir: &str, filename: &str) -> bool {
    get_vfs().exists(dir, filename)
}
