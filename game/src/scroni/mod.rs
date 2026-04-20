/*
 * scroni/mod.rs — ScroniPlugin: ONI2 scripting runtime.
 *
 * ScroniPlugin initialises ScroniTextState and registers the
 * scroni_sys_event_observer.  The scripting VM (vm.rs) executes .oni script
 * files compiled from the text AST.  System bindings bridge VM SysRequests
 * to Bevy ECS operations (spawning entities, playing sound, camera commands, etc.).
 */
pub mod ast;
pub mod compiler;
pub mod ops;
pub mod system_bindings;
pub mod token;
pub mod tokenizer;
pub mod vm;

use bevy::prelude::*;

pub struct ScroniPlugin;

impl Plugin for ScroniPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<vm::ScroniTextState>()
            .add_observer(system_bindings::scroni_sys_event_observer);
    }
}
