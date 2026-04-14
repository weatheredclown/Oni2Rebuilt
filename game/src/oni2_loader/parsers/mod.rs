/*
 * oni2_loader/parsers/mod.rs — ONI2 file format parser sub-modules.
 *
 * Each sub-module handles one ONI2 binary or text format.  All parsers read
 * data through the VFS (crate::vfs) and return plain Rust structs consumed by
 * spawn.rs, layout_loader.rs, and the various registry loaders.
 */
pub mod actor_xml;
pub mod animation;
pub mod anims;
pub mod bound;
pub mod camera;
pub mod effect;
pub mod entity_type;
pub mod jump;
pub mod layout;
pub mod graph;
pub mod loco;
pub mod mesh;
pub mod model;
pub mod particle;
pub mod projectile;
pub mod settings;
pub mod skeleton;
pub mod texture;
pub mod types;
pub mod audiopackages;
pub mod hd_bd;
pub mod stm;
pub mod td;
pub mod atdt;
pub mod rct;
pub mod expl;
