/*
 * statemachine/pad_fsm.rs — PadFsmCache for pad-input FSMs (Format-1).
 *
 * Homogeneous cache of `Arc<SmData<InputDriver>>` keyed by filename stem.
 * Loads `player.fsm`, `enemy.fsm`, `enemy_combo.fsm`, `enemy_grapple.fsm`,
 * `enemy_pipe.fsm`, `enemy_disarm.fsm`, `noattacks.fsm`, `empty.fsm` —
 * every file that's parsed with the Format-1 `#STATE`/`if` grammar and
 * driven by `InputDriver`.  Analogous to legacy
 * `aiInputStateMachineData::GetStateMachineData` (rb/src/behavior/
 * istatemachine.cpp:1084).
 *
 * Called from two code paths:
 *   • normal gameplay — the player entity's FSM attach reads its
 *     `PadFsmName` component (sourced from the actor XML's `Pad_FSM`
 *     attribute, defaulting to `"player"`) and loads the corresponding
 *     FSM here.
 *   • `PadBehavior` (not yet implemented) — a scripted `pad N [fsm]`
 *     initial-behavior entry loads the specified FSM here, using
 *     `Pad_FSM` as the fallback when no inline name is given.
 */
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

use super::core::SmData;
use super::drivers::input::{INPUT_ACTION_PARSER, INPUT_EVENT_PARSER, InputDriver};
use super::drivers::parse::parse_sm;

/// Lazy cache of parsed input-driver FSMs keyed by short name
/// (e.g. `"player"`, `"enemy_grapple"`).  The file extension `.fsm` is
/// appended internally.  Analogous to legacy
/// `aiInputStateMachineData::GetStateMachineData`.
#[derive(Resource, Default)]
pub struct PadFsmCache {
    pub by_name: HashMap<String, Arc<SmData<InputDriver>>>,
}

impl PadFsmCache {
    /// Return the cached FSM for `name`, parsing it on first request.
    /// Subsequent calls return the same Arc.  Returns `None` on read
    /// or parse failure — caller logs; cache stays empty so the next
    /// call retries.
    pub fn get_or_load(&mut self, name: &str) -> Option<Arc<SmData<InputDriver>>> {
        if let Some(data) = self.by_name.get(name) {
            return Some(Arc::clone(data));
        }

        let filename = format!("{}.fsm", name);
        let text = match crate::vfs::read_to_string("Statemachine", &filename) {
            Ok(t) => t,
            Err(e) => {
                error!("PadFsmCache: failed to read {}: {}", filename, e);
                return None;
            }
        };

        match parse_sm::<InputDriver>(&text, INPUT_EVENT_PARSER, INPUT_ACTION_PARSER) {
            Ok(data) => {
                let n = data.states.len();
                let arc = Arc::new(data);
                info!("PadFsmCache: loaded {} ({} states)", filename, n);
                self.by_name.insert(name.to_string(), Arc::clone(&arc));
                Some(arc)
            }
            Err(e) => {
                error!("PadFsmCache: failed to parse {}: {}", filename, e);
                None
            }
        }
    }
}
