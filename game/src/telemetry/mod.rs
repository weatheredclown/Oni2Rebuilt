/*
 * telemetry/mod.rs — TelemetryPlugin: async combat event reporting.
 *
 * Spawns a background thread connected to a crossbeam channel.  Game systems
 * send CombatEvents through TelemetryChannel; the thread forwards them to the
 * gRPC TelemetryService.  Startup fires a session-open event.
 */
pub mod bridge;
pub mod systems;

use bevy::prelude::*;

pub struct TelemetryPlugin;

impl Plugin for TelemetryPlugin {
    fn build(&self, app: &mut App) {
        let (sender, receiver) = crossbeam_channel::unbounded();
        app.insert_resource(bridge::TelemetryChannel { sender });
        bridge::spawn_telemetry_thread(receiver);
        app.add_systems(Startup, systems::send_startup_event);
    }
}
