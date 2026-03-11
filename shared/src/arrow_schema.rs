use std::sync::Arc;

use arrow::array::{
    BooleanArray, Float32Array, RecordBatch, StringArray, TimestampMillisecondArray, UInt32Array,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};

use crate::events::CombatEvent;

/// Returns the Arrow schema for combat events.
pub fn combat_event_schema() -> Schema {
    Schema::new(vec![
        Field::new("event_id", DataType::Utf8, false),
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
            false,
        ),
        Field::new("event_type", DataType::Utf8, false),
        Field::new("attacker_id", DataType::Utf8, false),
        Field::new("target_id", DataType::Utf8, false),
        Field::new("damage", DataType::Float32, false),
        Field::new("was_blocked", DataType::Boolean, false),
        Field::new("combo_count", DataType::UInt32, false),
        Field::new("attack_kind", DataType::Utf8, false),
        Field::new("position_x", DataType::Float32, false),
        Field::new("position_y", DataType::Float32, false),
        Field::new("position_z", DataType::Float32, false),
    ])
}

/// Converts a slice of CombatEvents into an Arrow RecordBatch.
pub fn combat_events_to_record_batch(
    events: &[CombatEvent],
) -> Result<RecordBatch, arrow::error::ArrowError> {
    let schema = Arc::new(combat_event_schema());

    let event_ids: Vec<String> = events.iter().map(|e| e.event_id.to_string()).collect();
    let timestamps: Vec<i64> = events.iter().map(|e| e.timestamp.timestamp_millis()).collect();
    let event_types: Vec<String> = events
        .iter()
        .map(|e| format!("{:?}", e.event_type))
        .collect();
    let attacker_ids: Vec<String> = events.iter().map(|e| e.attacker_id.to_string()).collect();
    let target_ids: Vec<String> = events.iter().map(|e| e.target_id.to_string()).collect();
    let damages: Vec<f32> = events.iter().map(|e| e.damage).collect();
    let was_blocked: Vec<bool> = events.iter().map(|e| e.was_blocked).collect();
    let combo_counts: Vec<u32> = events.iter().map(|e| e.combo_count).collect();
    let attack_kinds: Vec<String> = events.iter().map(|e| e.attack_kind.clone()).collect();
    let pos_x: Vec<f32> = events.iter().map(|e| e.position[0]).collect();
    let pos_y: Vec<f32> = events.iter().map(|e| e.position[1]).collect();
    let pos_z: Vec<f32> = events.iter().map(|e| e.position[2]).collect();

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(event_ids)),
            Arc::new(
                TimestampMillisecondArray::from(timestamps).with_timezone("UTC"),
            ),
            Arc::new(StringArray::from(event_types)),
            Arc::new(StringArray::from(attacker_ids)),
            Arc::new(StringArray::from(target_ids)),
            Arc::new(Float32Array::from(damages)),
            Arc::new(BooleanArray::from(was_blocked)),
            Arc::new(UInt32Array::from(combo_counts)),
            Arc::new(StringArray::from(attack_kinds)),
            Arc::new(Float32Array::from(pos_x)),
            Arc::new(Float32Array::from(pos_y)),
            Arc::new(Float32Array::from(pos_z)),
        ],
    )
}
