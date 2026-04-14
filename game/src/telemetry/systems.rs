/*
 * telemetry/systems.rs — Bevy systems that emit telemetry events.
 *
 * send_startup_event fires once at Startup to record a session-begin event.
 * Additional per-combat systems (damage, death) are wired from combat observers.
 */
use bevy::prelude::*;
use rb_shared::events::CombatEvent;
use uuid::Uuid;

use super::bridge::TelemetryChannel;

pub fn send_startup_event(channel: Res<TelemetryChannel>) {
    let event = CombatEvent::damage(
        Uuid::nil(),
        Uuid::nil(),
        0.0,
        false,
        0,
        "startup_test",
        [0.0, 0.0, 0.0],
    );
    let _ = channel.sender.send(event);
    info!("sent startup telemetry test event");
}
