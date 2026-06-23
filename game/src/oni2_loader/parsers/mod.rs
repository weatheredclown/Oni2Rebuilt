/*
 * oni2_loader/parsers/mod.rs — ONI2 file format parser sub-modules.
 *
 * Each sub-module handles one ONI2 binary or text format.  All parsers read
 * data through the VFS (crate::vfs) and return plain Rust structs consumed by
 * spawn.rs, layout_loader.rs, and the various registry loaders.
 */
pub mod actor_weap;
pub mod actor_xml;
pub mod ammo;
pub mod animation;
pub mod anims;
pub mod atdt;
pub mod audiopackages;
pub mod blk;
pub mod block_parser;
pub mod bound;
pub mod bsp;
pub mod camera;
pub mod effect;
pub mod entity_type;
pub mod expl;
pub mod fxl;
pub mod gait;
pub mod graph;
pub mod hd_bd;
pub mod inventory;
pub mod jump;
pub mod layout;
pub mod loco;
pub mod mesh;
pub mod model;
pub mod particle;
pub mod projectile;
pub mod rct;
pub mod rooms;
pub mod settings;
pub mod skeleton;
pub mod stm;
pub mod td;
pub mod texture;
pub mod types;
pub mod weap;
