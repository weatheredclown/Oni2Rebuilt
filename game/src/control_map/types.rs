/*
 * control_map/types.rs — parse-tree types for control.map.
 *
 * A ControlMap is a list of PadCommands. Each command has one or more Methods
 * (OR semantics — any method can satisfy it). Each Method has one or more
 * InputBlocks (AND semantics — all blocks must be satisfied). An InputBlock
 * contains Constraints plus an optional WindowOfOpportunity duration.
 *
 * An optional ExcludeInputBlock per Method vetoes the whole method when satisfied.
 */

/// A single primitive condition within an INPUT block.
#[derive(Debug, Clone)]
pub enum Constraint {
    /// Button was pressed this frame (or within WOO window).
    ButtonPress(String),
    /// Button is currently held.
    ButtonHold(String),
    /// Analog axis is currently in [min, max].
    AnalogRange { axis: String, min: f32, max: f32 },
    /// Analog axis *just* entered [min, max] this frame (edge / rising-edge detection).
    AnalogDebounce { axis: String, min: f32, max: f32 },
    /// Analog axis has shaken (magnitude of delta exceeds threshold).
    AnalogShake { axis: String, min: f32, max: f32 },
}

/// One INPUT { … } block — AND of all constraints.
///
/// If `window_of_opportunity` is Some(t), each constraint is checked against its
/// most-recently-satisfied timestamp rather than the current frame, allowing a
/// button press that happened up to t seconds ago to still count.
#[derive(Debug, Clone, Default)]
pub struct InputBlock {
    pub constraints: Vec<Constraint>,
    /// Window-of-opportunity duration in seconds (from WINDOW_OF_OPPORTUNITY).
    pub window_of_opportunity: Option<f32>,
}

/// One METHOD { … } block — satisfied when all InputBlocks pass AND exclude is absent.
#[derive(Debug, Clone, Default)]
pub struct PadMethod {
    pub inputs: Vec<InputBlock>,
    pub exclude: Option<InputBlock>,
}

/// One COMMAND entry — satisfied (value > 0) when any Method passes (OR).
#[derive(Debug, Clone)]
pub struct PadCommand {
    pub name: String,
    pub methods: Vec<PadMethod>,
}

/// The fully parsed control.map.
#[derive(Debug, Clone, Default)]
pub struct ControlMap {
    pub commands: Vec<PadCommand>,
}
