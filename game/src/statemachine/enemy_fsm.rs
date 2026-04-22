/*
 * statemachine/enemy_fsm.rs — EnemyFsmCache for AI-driven input FSMs.
 *
 * enemy.fsm, enemy_combo.fsm, noattacks.fsm, squad.fsm, etc. are all
 * loaded by the same legacy class (`aiInputStateMachineData`, see
 * rb/src/behavior/istatemachine.cpp:1084) and share the player's
 * tokenizer + vocabulary.  The same is true in the port — one
 * `InputDriver`, one `parse_sm`, many `.fsm` files.  The switch
 * between them is purely data: AI entities synthesize the PADCMD
 * bits that the player's hardware input would otherwise supply.
 */
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

use super::core::SmData;
use super::drivers::input::{INPUT_ACTION_PARSER, INPUT_EVENT_PARSER, InputDriver};
use super::drivers::parse::parse_sm;

/// Lazy cache of parsed input-driver FSMs keyed by short name
/// (e.g. `"enemy"`, `"enemy_combo"`).  The file extension `.fsm` is
/// appended internally.  Analogous to legacy
/// `aiInputStateMachineData::GetStateMachineData`.
#[derive(Resource, Default)]
pub struct EnemyFsmCache {
    pub by_name: HashMap<String, Arc<SmData<InputDriver>>>,
}

impl EnemyFsmCache {
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
                error!("EnemyFsmCache: failed to read {}: {}", filename, e);
                return None;
            }
        };

        match parse_sm::<InputDriver>(&text, INPUT_EVENT_PARSER, INPUT_ACTION_PARSER) {
            Ok(data) => {
                let n = data.states.len();
                let arc = Arc::new(data);
                info!("EnemyFsmCache: loaded {} ({} states)", filename, n);
                self.by_name.insert(name.to_string(), Arc::clone(&arc));
                Some(arc)
            }
            Err(e) => {
                error!("EnemyFsmCache: failed to parse {}: {}", filename, e);
                None
            }
        }
    }
}
