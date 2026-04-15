/*
 * statemachine/mod.rs — StateMachinePlugin: player animation FSM.
 *
 * Loads player.fsm at Startup into PlayerFsmData (shared Arc).  Attaches
 * FsmRuntime to any newly spawned Player entity via insert_player_fsm.
 * fsm_update_system (in PlayerPlugin) evaluates input pad flags each frame
 * and drives ONI2 animation transitions.
 */
pub mod parser;
pub mod runtime;
pub mod types;

pub use runtime::{FsmRuntime, fsm_update_system};
pub use types::FsmData;

use bevy::prelude::*;
use std::sync::Arc;

use crate::player::components::Player;

/// Bevy resource wrapping the shared player FSM data.
#[derive(Resource)]
pub struct PlayerFsmData(pub Arc<FsmData>);

pub struct StateMachinePlugin;

impl Plugin for StateMachinePlugin {
    fn build(&self, app: &mut App) {
        // Load FSM data before any game state runs
        app.add_systems(Startup, load_player_fsm);
        // Insert FsmRuntime on newly spawned player entities
        app.add_systems(Update, insert_player_fsm);
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

fn load_player_fsm(mut commands: Commands) {
    match crate::vfs::read_to_string("Statemachine", "player.fsm") {
        Ok(text) => match parser::parse_fsm(&text) {
            Ok(data) => {
                let n = data.states.len();
                commands.insert_resource(PlayerFsmData(Arc::new(data)));
                info!("FSM: player.fsm loaded — {} states", n);
            }
            Err(e) => error!("FSM: failed to parse player.fsm: {}", e),
        },
        Err(e) => error!("FSM: failed to read player.fsm: {}", e),
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
