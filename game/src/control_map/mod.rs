/*
 * control_map/mod.rs — Bevy plugin + public API.
 *
 * ControlMapPlugin
 * ────────────────
 * Loaded at startup.  Reads Settings/control.map from the VFS (falling back to
 * a bare file path), parses it, and inserts a `PadMapper` resource.
 *
 * Each frame the `pad_mapper_update_system` is called from PlayerPlugin BEFORE
 * the FSM tick.  It builds a `RawInputFrame` from Bevy's keyboard/mouse/gamepad
 * state and calls `PadMapper::update()`.
 *
 * Public re-exports used by player/systems.rs and statemachine/runtime.rs:
 *   PadMapper, RawInputFrame, ButtonState
 */

pub mod evaluator;
pub mod parser;
pub mod types;

pub use evaluator::{PadMapper, RawInputFrame};

use crate::filesystem::vfs;
use bevy::prelude::*;

pub struct ControlMapPlugin;

impl Plugin for ControlMapPlugin {
    fn build(&self, app: &mut App) {
        // Load and parse control.map at startup; insert PadMapper as a resource.
        let src = load_control_map_source();
        match parser::parse(&src) {
            Ok(map) => {
                let n = map.commands.len();
                app.insert_resource(PadMapper::new(map));
                info!("ControlMap: loaded {} commands", n);
            }
            Err(e) => {
                warn!("ControlMap: parse error — {}.  PadMapper will be empty.", e);
                app.insert_resource(PadMapper::new(types::ControlMap::default()));
            }
        }
    }
}

/// Try to load Settings/control.map from the VFS; fall back to a disk path
/// relative to the working directory.
fn load_control_map_source() -> String {
    // Try VFS first (DiskVfs or DaveVfs)
    if let Ok(s) = vfs::read_to_string("Settings", "control.map") {
        return s;
    }
    // Fallback: bare disk path (useful in tests / dev without VFS mounted)
    let paths = [
        "oni2/zips/assets/Settings/control.map",
        "assets/Settings/control.map",
    ];
    for p in &paths {
        if let Ok(s) = std::fs::read_to_string(p) {
            return s;
        }
    }
    warn!("ControlMap: could not find Settings/control.map; starting with empty map");
    String::new()
}
