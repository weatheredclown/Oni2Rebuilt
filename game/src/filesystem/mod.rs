/*
 * filesystem/mod.rs — virtual file system facade.
 *
 * Re-exports the two VFS backends: vfs (DiskVfs / MultiVfs abstraction) and
 * dave_vfs (read-only DAVE archive format used by ONI2 .DAT files).
 * Public API: crate::vfs::read_file(), read_to_string(), exists(), read_dir().
 */
pub mod dave_vfs;
pub mod vfs;
