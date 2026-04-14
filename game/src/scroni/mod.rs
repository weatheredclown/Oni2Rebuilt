pub mod ast;
pub mod compiler;
pub mod token;
pub mod tokenizer;
pub mod vm;
pub mod ops;
pub mod system_bindings;

use bevy::prelude::*;

pub struct ScroniPlugin;

impl Plugin for ScroniPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<vm::ScroniTextState>()
            .add_observer(system_bindings::scroni_sys_event_observer);
    }
}
