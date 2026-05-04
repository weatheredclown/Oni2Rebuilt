pub mod ai;
pub mod animator;
pub mod behavior;
pub mod camera;
pub mod combat;
pub mod common;
pub mod control_map;
pub mod crt_post;
pub mod debug;
pub mod debug_atdt;
pub mod door;
pub mod env_reflect_material;
pub mod explosion;
pub mod fight;
pub mod fight_vector;
pub mod fightai;
pub mod frontend;
pub mod filesystem;
pub mod fx_system;
pub mod fx_visuals;
pub mod hud;
pub mod inventory;
pub mod laser;
pub mod menu;
pub mod mover;
pub mod oni2_loader;
pub mod player;
pub mod projectile_system;
pub mod scroni;
pub mod shadow_lod;
pub mod statemachine;
pub mod telemetry;
pub mod weapons;

pub use filesystem::dave_vfs;
pub use filesystem::vfs;
pub use menu::InGameEntity;
pub use oni2_loader::*;

use std::sync::OnceLock;

pub static ASSETS_PATH: OnceLock<String> = OnceLock::new();
pub static ASSETS_DAT: OnceLock<String> = OnceLock::new();

pub fn get_assets_path() -> &'static str {
    ASSETS_PATH
        .get()
        .map(|s| s.as_str())
        .unwrap_or("../oni2/zips/assets")
}

pub fn get_assets_dat() -> &'static str {
    ASSETS_DAT.get().map(|s| s.as_str()).unwrap_or("RB.DAT")
}

pub fn set_assets_path(path: impl Into<String>) {
    let _ = ASSETS_PATH.set(path.into());
}

pub fn set_assets_dat(path: impl Into<String>) {
    let _ = ASSETS_DAT.set(path.into());
}
