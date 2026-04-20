/*
 * statemachine/mod.rs — StateMachinePlugin: player animation FSM.
 *
 * Loads player.fsm at Startup into PlayerFsmData (shared Arc<SmData<InputDriver>>).
 * Attaches FsmRuntime to any newly spawned Player entity via insert_player_fsm.
 * fsm_update_system (in PlayerPlugin) evaluates input pad flags each frame
 * and drives ONI2 animation transitions.
 */
pub mod core;
pub mod drivers;
pub mod runtime;
pub mod types;

pub use runtime::{FsmRuntime, fsm_update_system};

use bevy::prelude::*;
use std::sync::Arc;

use crate::player::components::Player;

use core::SmData;
use drivers::input::{INPUT_ACTION_PARSER, INPUT_EVENT_PARSER, InputDriver};
use drivers::parse::parse_sm;

/// Bevy resource wrapping the shared player FSM data.
#[derive(Resource)]
pub struct PlayerFsmData(pub Arc<SmData<InputDriver>>);

pub struct StateMachinePlugin;

impl Plugin for StateMachinePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_player_fsm);
        app.add_systems(Update, insert_player_fsm);
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

fn load_player_fsm(mut commands: Commands) {
    let text = match crate::vfs::read_to_string("Statemachine", "player.fsm") {
        Ok(t) => t,
        Err(e) => {
            error!("FSM: failed to read player.fsm: {}", e);
            return;
        }
    };

    match parse_sm::<InputDriver>(&text, INPUT_EVENT_PARSER, INPUT_ACTION_PARSER) {
        Ok(data) => {
            let n = data.states.len();
            commands.insert_resource(PlayerFsmData(Arc::new(data)));
            info!("FSM: player.fsm loaded — {} states", n);
        }
        Err(e) => error!("FSM: failed to parse player.fsm: {}", e),
    }
}

/// Attach FsmRuntime to any player entity that was just spawned.
fn insert_player_fsm(
    query: Query<Entity, Added<Player>>,
    fsm_res: Option<Res<PlayerFsmData>>,
    mut pad_mapper: ResMut<crate::control_map::PadMapper>,
    mut commands: Commands,
) {
    let Some(fsm) = fsm_res else { return };

    let mut spawned = false;
    for entity in &query {
        commands
            .entity(entity)
            .insert(FsmRuntime::new(fsm.0.clone()));
        info!("FSM: FsmRuntime attached to player {:?}", entity);
        spawned = true;
    }

    if spawned {
        pad_mapper.clear();
    }
}
