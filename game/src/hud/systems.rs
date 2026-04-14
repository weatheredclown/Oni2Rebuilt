/*
 * hud/systems.rs — HUD update systems (currently stubbed).
 *
 * HUD was removed; these systems are no-op stubs retained so the plugin
 * compiles and can be re-enabled without structural changes.  Health /
 * ComboTracker / AttackState components remain on entities for game logic.
 */
use bevy::prelude::*;

// HUD has been removed — systems are stubs that do nothing.
// Combat data components (Health, ComboTracker, etc.) remain on entities
// for gameplay logic but are no longer rendered on screen.

pub fn setup_hud(_commands: Commands) {}
pub fn update_health_bars() {}
pub fn update_super_meter_bar() {}
pub fn update_combo_display() {}
pub fn update_combat_display() {}
pub fn update_status_display() {}
