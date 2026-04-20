/*
 * statemachine/drivers/parse.rs — generic state/rule skeleton parser.
 *
 * All three ONI2 state-machine dialects (.fsm, aifight .sm, .atk) share the
 * same surface syntax — a `#` state header followed by `if [!]Event { actions;
 * [goto Target;] }` blocks.  Only the Event and Action vocabularies differ.
 *
 * This module exposes `parse_sm<D, ...>`, a generic reader that takes two
 * driver-supplied callbacks (`event_parser`, `action_parser`) and returns a
 * fully-populated `SmData<D>`.  Comments use `;`, blocks may be single-line or
 * multi-line, and `goto Target` references resolve to a state index (`usize`)
 * via the two-pass scan.
 */
use std::collections::HashMap;

use super::super::core::{SmData, SmDriver, SmRule, SmState};

/// Parsed form of a single rule body before the driver's action parser lifts
/// each action line into a `D::Action`.  `action_lines` are the text lines
/// *inside* the `{ ... }`, whitespace-trimmed, with `goto` lines stripped out.
pub struct RawRuleBody<'a> {
    pub action_lines: Vec<&'a str>,
    /// Resolved target state index for any `goto` encountered in the block.
    pub goto_state: Option<usize>,
}

/// Driver-supplied callback to convert one event-text line into `D::Event`.
pub type EventParser<D> =
    fn(text: &str, state_index: &HashMap<String, usize>) -> Result<<D as SmDriver>::Event, String>;

/// Driver-supplied callback to convert one action-text line into `D::Action`.
/// Returning `Ok(None)` silently skips the line (used for actions the driver
/// does not yet implement — keeps unknown-action warnings driver-local).
pub type ActionParser<D> = fn(
    line: &str,
    state_index: &HashMap<String, usize>,
) -> Result<Option<<D as SmDriver>::Action>, String>;

/// Parse one text source into `SmData<D>`.  The state/rule skeleton is shared
/// across all drivers; the driver supplies only the two leaf parsers.
pub fn parse_sm<D: SmDriver>(
    text: &str,
    event_parser: EventParser<D>,
    action_parser: ActionParser<D>,
) -> Result<SmData<D>, String> {
    // Strip `;` comments but keep the line structure intact so diagnostics line
    // up with the source file.  `;` INSIDE an inline `{ ... }` block is the
    // action separator (e.g. `{ Broadcast Foo; goto BAR }`), not a comment —
    // so only treat `;` at brace depth 0 as a comment start.
    let clean: Vec<String> = text
        .lines()
        .map(|line| {
            let mut depth: i32 = 0;
            let mut out = String::new();
            for c in line.chars() {
                match c {
                    '{' => {
                        depth += 1;
                        out.push(c);
                    }
                    '}' => {
                        depth -= 1;
                        out.push(c);
                    }
                    ';' if depth <= 0 => break,
                    _ => out.push(c),
                }
            }
            out
        })
        .collect();

    // Pass 1 — collect `# StateName` headers and their line positions.
    let mut bounds: Vec<(String, usize)> = Vec::new();
    for (i, line) in clean.iter().enumerate() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('#') {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                bounds.push((name, i));
            }
        }
    }

    // Build name→index up-front so `goto`s in pass 2 can resolve forward
    // references.
    let mut states: Vec<SmState<D>> = bounds
        .iter()
        .map(|(name, _)| SmState {
            name: name.clone(),
            rules: Vec::new(),
        })
        .collect();
    let state_index: HashMap<String, usize> = bounds
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.clone(), i))
        .collect();

    // Pass 2 — for each state, parse every `if ... { ... }` block inside its
    // range.
    let n = bounds.len();
    for si in 0..n {
        let start = bounds[si].1 + 1;
        let end = if si + 1 < n {
            bounds[si + 1].1
        } else {
            clean.len()
        };

        let rules = parse_state_rules::<D>(
            &clean[start..end],
            &state_index,
            event_parser,
            action_parser,
        )?;
        states[si].rules = rules;
    }

    Ok(SmData {
        states,
        state_index,
    })
}

/// Walk the lines of one state block and collect rules.  Each rule begins at
/// an `if` line and extends through the matching `}`.
fn parse_state_rules<D: SmDriver>(
    lines: &[String],
    state_index: &HashMap<String, usize>,
    event_parser: EventParser<D>,
    action_parser: ActionParser<D>,
) -> Result<Vec<SmRule<D>>, String> {
    let mut rules = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        let Some(rest) = trimmed.strip_prefix("if ") else {
            i += 1;
            continue;
        };
        let rest = rest.trim_start();

        // Split the `if` line into (event_text, optional brace tail).  The
        // tail lets us handle `if Foo { ... }` all on one line without the
        // brace bleeding into the event text we hand to the driver.
        let (event_text_raw, same_line_brace_tail) = match rest.find('{') {
            Some(idx) => (rest[..idx].trim_end(), Some(&rest[idx..])),
            None => (rest, None),
        };

        // Optional leading `!` negates the event match.
        let (negated, event_text) = if let Some(r) = event_text_raw.strip_prefix('!') {
            (true, r.trim_start())
        } else {
            (false, event_text_raw)
        };

        let event = match event_parser(event_text, state_index) {
            Ok(e) => e,
            Err(msg) => {
                bevy::log::warn!("sm parser: {}", msg);
                i += 1;
                continue;
            }
        };

        let mut block_lines: Vec<String> = Vec::new();

        if let Some(tail) = same_line_brace_tail {
            // Brace (and possibly the closing one too) is on the SAME line as
            // `if`.  Two sub-cases:
            //   a) fully inline `{ ... }` — body is between the two braces.
            //   b) open-only `{` — body starts on the next line; scan until
            //      the matching close brace.
            let open = tail.find('{').unwrap(); // guaranteed
            if let Some(close) = tail.rfind('}') {
                if close > open {
                    // (a) inline
                    block_lines.push(tail[open + 1..close].to_string());
                    i += 1;
                } else {
                    // `{` appears but `}` is to its left — malformed; bail.
                    i += 1;
                    continue;
                }
            } else {
                // (b) open-only on this line; body on subsequent lines until
                // balanced close.
                let after_brace = tail[open + 1..].trim();
                if !after_brace.is_empty() {
                    block_lines.push(after_brace.to_string());
                }
                i += 1;
                let mut depth: i32 = 1;
                while i < lines.len() {
                    let bl = &lines[i];
                    depth += bl.chars().filter(|&c| c == '{').count() as i32;
                    depth -= bl.chars().filter(|&c| c == '}').count() as i32;
                    if depth <= 0 {
                        i += 1;
                        break;
                    }
                    block_lines.push(bl.clone());
                    i += 1;
                }
            }
        } else {
            // Brace is on a subsequent line.  Find it, then parse exactly as
            // the open-only case above.
            i += 1;
            let brace_start = loop {
                if i >= lines.len() {
                    break None;
                }
                let t = lines[i].trim();
                if t.contains('{') {
                    break Some(i);
                }
                if t.starts_with('#') || t.starts_with("if ") {
                    break None;
                }
                i += 1;
            };

            let Some(brace_line_idx) = brace_start else {
                continue;
            };
            i = brace_line_idx;

            let brace_line = &lines[i];

            if brace_line.contains('{') && brace_line.contains('}') {
                if let (Some(o), Some(c)) = (brace_line.find('{'), brace_line.rfind('}')) {
                    if o < c {
                        block_lines.push(brace_line[o + 1..c].to_string());
                    }
                }
                i += 1;
            } else {
                i += 1;
                let mut depth: i32 = 1;
                while i < lines.len() {
                    let bl = &lines[i];
                    depth += bl.chars().filter(|&c| c == '{').count() as i32;
                    depth -= bl.chars().filter(|&c| c == '}').count() as i32;
                    if depth <= 0 {
                        i += 1;
                        break;
                    }
                    block_lines.push(bl.clone());
                    i += 1;
                }
            }
        }

        let (actions, goto_state) = parse_rule_body::<D>(&block_lines, state_index, action_parser)?;
        rules.push(SmRule {
            event,
            negated,
            actions,
            goto_state,
        });
    }

    Ok(rules)
}

fn parse_rule_body<D: SmDriver>(
    lines: &[String],
    state_index: &HashMap<String, usize>,
    action_parser: ActionParser<D>,
) -> Result<(Vec<D::Action>, Option<usize>), String> {
    let mut actions: Vec<D::Action> = Vec::new();
    let mut goto_state: Option<usize> = None;

    // A rule body may come in two shapes:
    //   • multi-line: one action per input line
    //   • inline:     `{ Action; Action; goto Foo }` — a single input "line"
    //     carrying `;`-separated statements.
    // Flatten both by splitting every input line on `;`, trimming, and
    // dropping empties.  Everything downstream handles a clean list of
    // statements.
    for raw_line in lines {
        for piece in raw_line.split(';') {
            let t = piece.trim();
            if t.is_empty() {
                continue;
            }

            if let Some(target) = t.strip_prefix("goto ") {
                let name = target.trim();
                match state_index.get(name) {
                    Some(&idx) => goto_state = Some(idx),
                    None => bevy::log::warn!("sm parser: unknown goto target '{}'", name),
                }
                continue;
            }

            match action_parser(t, state_index) {
                Ok(Some(action)) => actions.push(action),
                Ok(None) => {} // silently skipped
                Err(msg) => bevy::log::warn!("sm parser: {}", msg),
            }
        }
    }

    Ok((actions, goto_state))
}

/// Split a call-like token `Name(arg1, arg2, ...)` into (name, args).  Returns
/// (whole_text, "") if no parens.  Utility used by every driver's event/action
/// leaf parser so they don't re-implement the split.
pub fn split_call(text: &str) -> (&str, &str) {
    match (text.find('('), text.rfind(')')) {
        (Some(o), Some(c)) if o < c => (text[..o].trim(), text[o + 1..c].trim()),
        _ => (text.trim(), ""),
    }
}
