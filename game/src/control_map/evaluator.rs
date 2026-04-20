/*
 * control_map/evaluator.rs — per-frame evaluation of a parsed ControlMap.
 *
 * RawInputFrame
 * ─────────────
 * Primitive hardware state that both keyboard/mouse and gamepad backends fill.
 * Button names and analog axis names match those used in control.map (e.g. "Rup",
 * "AnalogCharacterForward").  Backends map their hardware onto this vocabulary.
 *
 * PadMapper (Bevy Resource)
 * ─────────────────────────
 * Holds the parsed ControlMap plus per-frame state (prev analog values, last button-
 * press times).  Call `update()` once per frame with the current RawInputFrame; then
 * read command values via `get("PADCMD_ATTACK_HIGH")`.
 *
 * Evaluation semantics
 * ────────────────────
 * Command: OR of methods
 * Method:  AND of input-blocks, vetoed if exclude-block is satisfied
 * InputBlock (no WOO): constraints checked against the CURRENT frame
 * InputBlock (with WOO t): BUTTON_PRESS constraints check last-press-time ≤ t seconds ago;
 *   ANALOG_RANGE checks current OR previous frame (approximation for tiny windows like 0.01 s).
 * ANALOG_DEBOUNCE: fires on the rising edge — axis was below min last frame, ≥ min this frame.
 */

use super::types::*;
use bevy::prelude::Resource;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// RawInputFrame
// ---------------------------------------------------------------------------

/// Per-button state for the current frame.
#[derive(Default, Clone)]
pub struct ButtonState {
    /// True only on the frame the button transitions from released → pressed.
    pub just_pressed: bool,
    /// True on every frame the button is held (includes the just_pressed frame).
    pub held: bool,
}

/// Primitive hardware input for one frame.
///
/// Both the keyboard+mouse backend and the gamepad backend write into this using
/// the button/axis names from control.map.  The PadMapper consumes one of these
/// each frame.
#[derive(Default)]
pub struct RawInputFrame {
    /// Elapsed time in seconds (from Bevy's `Time`).
    pub time: f32,
    /// Button state keyed by control.map button name ("Rup", "L2", …).
    pub buttons: HashMap<String, ButtonState>,
    /// Analog axis values (0..=1) keyed by control.map axis name
    /// ("AnalogCharacterForward", "AnalogLmag", …).
    pub analogs: HashMap<String, f32>,
}

impl RawInputFrame {
    pub fn button_pressed(&self, name: &str) -> bool {
        self.buttons
            .get(name)
            .map(|b| b.just_pressed)
            .unwrap_or(false)
    }
    pub fn button_held(&self, name: &str) -> bool {
        self.buttons.get(name).map(|b| b.held).unwrap_or(false)
    }
    pub fn analog(&self, name: &str) -> f32 {
        *self.analogs.get(name).unwrap_or(&0.0)
    }

    /// Convenience: set a button as just-pressed (also marks held = true).
    pub fn press(&mut self, name: &str) {
        let s = self.buttons.entry(name.to_string()).or_default();
        s.just_pressed = true;
        s.held = true;
    }

    /// Convenience: set a button as held but not just-pressed.
    pub fn hold(&mut self, name: &str) {
        let s = self.buttons.entry(name.to_string()).or_default();
        s.held = true;
    }

    /// Convenience: set an analog axis value.
    pub fn set_analog(&mut self, name: &str, value: f32) {
        self.analogs.insert(name.to_string(), value);
    }
}

// ---------------------------------------------------------------------------
// PadMapper
// ---------------------------------------------------------------------------

/// Holds the parsed ControlMap and evaluates command values each frame.
/// Registered as a Bevy `Resource`.
#[derive(Resource)]
pub struct PadMapper {
    commands: Vec<PadCommand>,
    /// Name → index in `commands` / `values`.
    name_to_index: HashMap<String, usize>,

    // ── per-frame persistent state ──────────────────────────────────────────
    /// Analog values from the *previous* frame (for ANALOG_DEBOUNCE edge detection).
    analog_prev: HashMap<String, f32>,
    /// Most recent time each named button was `just_pressed`.
    button_press_time: HashMap<String, f32>,

    // ── output ──────────────────────────────────────────────────────────────
    /// Evaluated value per command index, updated by `update()`.
    pub values: Vec<f32>,
}

impl PadMapper {
    pub fn new(map: ControlMap) -> Self {
        let n = map.commands.len();
        let name_to_index = map
            .commands
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name.clone(), i))
            .collect();
        PadMapper {
            commands: map.commands,
            name_to_index,
            analog_prev: HashMap::new(),
            button_press_time: HashMap::new(),
            values: vec![0.0; n],
        }
    }

    /// Flush all historic input buffers.
    pub fn clear(&mut self) {
        self.analog_prev.clear();
        self.button_press_time.clear();
        self.values.fill(0.0);
    }

    /// Returns the current value (0 = inactive, >0 = active) for a named command.
    pub fn get(&self, name: &str) -> f32 {
        self.name_to_index
            .get(name)
            .map(|&i| self.values[i])
            .unwrap_or(0.0)
    }

    /// Evaluate all commands against `frame` and update `self.values`.
    /// Must be called exactly once per game frame before any consumer reads values.
    pub fn update(&mut self, frame: &RawInputFrame) {
        let t = frame.time;

        // 1. Record button-press timestamps BEFORE evaluation so a press that fires
        //    this frame is immediately visible to WOO checks in the same frame.
        for (name, bs) in &frame.buttons {
            if bs.just_pressed {
                self.button_press_time.insert(name.clone(), t);
            }
        }

        // 2. Evaluate every command using the *previous* frame's analog snapshot.
        //    Index-based loop so each iteration borrows self.commands[i] immutably,
        //    then assigns self.values[i] — Rust allows disjoint field borrows here.
        for i in 0..self.commands.len() {
            self.values[i] = eval_command(
                &self.commands[i],
                frame,
                &self.analog_prev,
                &self.button_press_time,
                t,
            );
        }

        // 3. Advance the analog snapshot to the current frame for next tick's debounce.
        for (axis, &val) in &frame.analogs {
            self.analog_prev.insert(axis.clone(), val);
        }
    }
}

// ---------------------------------------------------------------------------
// Evaluation helpers (pure functions, no &mut)
// ---------------------------------------------------------------------------

fn eval_command(
    cmd: &PadCommand,
    frame: &RawInputFrame,
    analog_prev: &HashMap<String, f32>,
    button_press_time: &HashMap<String, f32>,
    t: f32,
) -> f32 {
    for method in &cmd.methods {
        if eval_method(method, frame, analog_prev, button_press_time, t) {
            return 1.0;
        }
    }
    0.0
}

fn eval_method(
    method: &PadMethod,
    frame: &RawInputFrame,
    analog_prev: &HashMap<String, f32>,
    button_press_time: &HashMap<String, f32>,
    t: f32,
) -> bool {
    if method.inputs.is_empty() {
        return false;
    }
    // All input blocks must pass (AND)
    for block in &method.inputs {
        if !eval_input_block(block, frame, analog_prev, button_press_time, t) {
            return false;
        }
    }
    // Exclude block must NOT pass
    if let Some(excl) = &method.exclude {
        if eval_input_block(excl, frame, analog_prev, button_press_time, t) {
            return false;
        }
    }
    true
}

fn eval_input_block(
    block: &InputBlock,
    frame: &RawInputFrame,
    analog_prev: &HashMap<String, f32>,
    button_press_time: &HashMap<String, f32>,
    t: f32,
) -> bool {
    if block.constraints.is_empty() {
        return false;
    }
    // All constraints within the block must pass (AND)
    let woo = block.window_of_opportunity;
    for c in &block.constraints {
        if !eval_constraint(c, frame, analog_prev, button_press_time, t, woo) {
            return false;
        }
    }
    true
}

/// Evaluate a single constraint.
///
/// `woo` — when `Some(w)`, use "was satisfied within the last w seconds" semantics
/// instead of "is satisfied right now".  For BUTTON_PRESS this uses the stored
/// last-press timestamp; for ANALOG_RANGE it also checks the previous-frame value.
fn eval_constraint(
    c: &Constraint,
    frame: &RawInputFrame,
    analog_prev: &HashMap<String, f32>,
    button_press_time: &HashMap<String, f32>,
    t: f32,
    woo: Option<f32>,
) -> bool {
    match c {
        Constraint::ButtonPress(btn) => {
            if let Some(w) = woo {
                // WOO: was pressed within the last w seconds
                button_press_time
                    .get(btn.as_str())
                    .map(|&pt| t - pt <= w)
                    .unwrap_or(false)
            } else {
                frame.button_pressed(btn)
            }
        }

        Constraint::ButtonHold(btn) => frame.button_held(btn),

        Constraint::AnalogRange { axis, min, max } => {
            let cur = frame.analog(axis);
            if let Some(w) = woo {
                // WOO: current frame OR previous frame was in range.
                // (accurate enough for the tiny 0.01 s windows seen in control.map)
                let prev = *analog_prev.get(axis.as_str()).unwrap_or(&0.0);
                let in_range = |v: f32| v >= *min && v <= *max;
                in_range(cur) || in_range(prev)
                    // For larger windows we'd need a timestamp, but in practice
                    // the only non-trivial ANALOG_RANGE + WOO is the 0.01 s exclude.
                    || (w > 0.05
                        && button_press_time  // re-use as sentinel: analog was live recently
                            .get(&format!("analog_range_{}_{:.3}_{:.3}", axis, min, max))
                            .map(|&at| t - at <= w)
                            .unwrap_or(false))
            } else {
                cur >= *min && cur <= *max
            }
        }

        Constraint::AnalogDebounce { axis, min, max } => {
            // Rising-edge: was outside range last frame, inside range this frame.
            let cur = frame.analog(axis);
            let prev = *analog_prev.get(axis.as_str()).unwrap_or(&0.0);
            let in_range = |v: f32| v >= *min && v <= *max;
            in_range(cur) && !in_range(prev)
            // WOO does not meaningfully apply to debounce (it would need its own
            // last-debounce timestamp; in control.map WOO is always on the *other*
            // INPUT block containing a BUTTON_PRESS).
        }

        Constraint::AnalogShake { axis, min, max } => {
            // Simplified: the magnitude of the per-frame delta exceeds `min`.
            let cur = frame.analog(axis);
            let prev = *analog_prev.get(axis.as_str()).unwrap_or(&0.0);
            let delta = (cur - prev).abs();
            delta >= *min && delta <= *max
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_map::parser;

    fn make_frame(t: f32) -> RawInputFrame {
        RawInputFrame {
            time: t,
            ..Default::default()
        }
    }

    #[test]
    fn button_press_fires_once() {
        let src = "COMMAND PADCMD_JUMP { METHOD { INPUT { BUTTON_PRESS Rdown } } }";
        let map = parser::parse(src).unwrap();
        let mut pm = PadMapper::new(map);

        let mut frame = make_frame(0.016);
        frame.press("Rdown");
        pm.update(&frame);
        assert_eq!(pm.get("PADCMD_JUMP"), 1.0);

        // Next frame: held, not just_pressed
        let mut frame2 = make_frame(0.032);
        frame2.hold("Rdown");
        pm.update(&frame2);
        assert_eq!(pm.get("PADCMD_JUMP"), 0.0);
    }

    #[test]
    fn analog_debounce_rising_edge() {
        let src = "COMMAND PADCMD_CHR_FWD { METHOD { INPUT { ANALOG_DEBOUNCE AnalogCharacterForward 0.6 1.0 } } }";
        let map = parser::parse(src).unwrap();
        let mut pm = PadMapper::new(map);

        // Frame 1: analog = 0 (below threshold) — no debounce
        let mut f1 = make_frame(0.016);
        f1.set_analog("AnalogCharacterForward", 0.0);
        pm.update(&f1);
        assert_eq!(pm.get("PADCMD_CHR_FWD"), 0.0);

        // Frame 2: analog jumps to 1.0 — debounce fires
        let mut f2 = make_frame(0.032);
        f2.set_analog("AnalogCharacterForward", 1.0);
        pm.update(&f2);
        assert_eq!(pm.get("PADCMD_CHR_FWD"), 1.0);

        // Frame 3: analog still 1.0 — debounce does NOT fire again
        let mut f3 = make_frame(0.048);
        f3.set_analog("AnalogCharacterForward", 1.0);
        pm.update(&f3);
        assert_eq!(pm.get("PADCMD_CHR_FWD"), 0.0);
    }

    #[test]
    fn woo_button_press() {
        // ACK_RIGHT: fires when stick debounces right this frame AND Rup pressed ≤ 0.2 s ago
        let src = r#"
COMMAND ACK_RIGHT
{
    METHOD
    {
        INPUT { ANALOG_DEBOUNCE AnalogCharacterRight 0.75 1 }
        INPUT { BUTTON_PRESS Rup  WINDOW_OF_OPPORTUNITY .2 }
    }
}
"#;
        let map = parser::parse(src).unwrap();
        let mut pm = PadMapper::new(map);

        // t=0.0: press Rup
        let mut f0 = make_frame(0.0);
        f0.press("Rup");
        f0.set_analog("AnalogCharacterRight", 0.0);
        pm.update(&f0);
        assert_eq!(pm.get("ACK_RIGHT"), 0.0); // stick not debounced yet

        // t=0.1: analog crosses threshold — Rup was pressed 0.1 s ago, within WOO 0.2
        let mut f1 = make_frame(0.1);
        f1.set_analog("AnalogCharacterRight", 1.0); // debounce: prev=0, cur=1
        pm.update(&f1);
        assert_eq!(pm.get("ACK_RIGHT"), 1.0);

        // t=0.3: analog still high (prev=1, cur=1 → no debounce) — should not fire
        let mut f2 = make_frame(0.3);
        f2.set_analog("AnalogCharacterRight", 1.0);
        pm.update(&f2);
        assert_eq!(pm.get("ACK_RIGHT"), 0.0);
    }
}
