use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CombatEvent {
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub event_type: CombatEventType,
    pub attacker_id: Uuid,
    pub target_id: Uuid,
    pub damage: f32,
    pub was_blocked: bool,
    pub combo_count: u32,
    pub attack_kind: String,
    pub position: [f32; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CombatEventType {
    Damage,
    Death,
    Block,
    ComboHit,
}

impl CombatEvent {
    pub fn damage(
        attacker_id: Uuid,
        target_id: Uuid,
        damage: f32,
        was_blocked: bool,
        combo_count: u32,
        attack_kind: &str,
        position: [f32; 3],
    ) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            event_type: CombatEventType::Damage,
            attacker_id,
            target_id,
            damage,
            was_blocked,
            combo_count,
            attack_kind: attack_kind.to_string(),
            position,
        }
    }

    pub fn death(target_id: Uuid, attacker_id: Uuid, position: [f32; 3]) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            event_type: CombatEventType::Death,
            attacker_id,
            target_id,
            damage: 0.0,
            was_blocked: false,
            combo_count: 0,
            attack_kind: String::new(),
            position,
        }
    }
}
