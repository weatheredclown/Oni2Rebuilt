pub mod arrow_schema;
pub mod events;

pub mod proto {
    pub mod telemetry {
        tonic::include_proto!("telemetry");
    }
}

// === Conversions between domain CombatEvent and proto CombatEventProto ===

impl From<events::CombatEvent> for proto::telemetry::CombatEventProto {
    fn from(e: events::CombatEvent) -> Self {
        Self {
            event_id: e.event_id.to_string(),
            timestamp_ms: e.timestamp.timestamp_millis(),
            event_type: format!("{:?}", e.event_type),
            attacker_id: e.attacker_id.to_string(),
            target_id: e.target_id.to_string(),
            damage: e.damage,
            was_blocked: e.was_blocked,
            combo_count: e.combo_count,
            attack_kind: e.attack_kind,
            position_x: e.position[0],
            position_y: e.position[1],
            position_z: e.position[2],
        }
    }
}

impl From<proto::telemetry::CombatEventProto> for events::CombatEvent {
    fn from(p: proto::telemetry::CombatEventProto) -> Self {
        Self {
            event_id: p.event_id.parse().unwrap_or_default(),
            timestamp: chrono::DateTime::from_timestamp_millis(p.timestamp_ms)
                .unwrap_or_default(),
            event_type: match p.event_type.as_str() {
                "Death" => events::CombatEventType::Death,
                "Block" => events::CombatEventType::Block,
                "ComboHit" => events::CombatEventType::ComboHit,
                _ => events::CombatEventType::Damage,
            },
            attacker_id: p.attacker_id.parse().unwrap_or_default(),
            target_id: p.target_id.parse().unwrap_or_default(),
            damage: p.damage,
            was_blocked: p.was_blocked,
            combo_count: p.combo_count,
            attack_kind: p.attack_kind,
            position: [p.position_x, p.position_y, p.position_z],
        }
    }
}
