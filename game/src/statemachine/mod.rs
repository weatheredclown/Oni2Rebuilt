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

pub use runtime::{FsmRuntime, fsm_update_system, AnimatorRuntime, animator_update_system, AnimatorBroadcastEvent};

use bevy::prelude::*;
use std::sync::Arc;

use crate::player::components::Player;

use core::SmData;
use drivers::input::{INPUT_ACTION_PARSER, INPUT_EVENT_PARSER, InputDriver};
use drivers::parse::parse_sm;

/// Bevy resource wrapping the shared player FSM data.
#[derive(Resource)]
pub struct PlayerFsmData(pub Arc<SmData<InputDriver>>);

/// Bevy resource wrapping the top-level embedded orchestration FSM data.
#[derive(Resource)]
pub struct PlayerAnimatorData(pub Arc<SmData<crate::statemachine::drivers::animator::AnimatorDriver>>);

pub struct StateMachinePlugin;

impl Plugin for StateMachinePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<AnimatorBroadcastEvent>();
        app.add_systems(Startup, (load_player_fsm, load_animator_fsm));
        app.add_systems(Update, insert_player_fsm);
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

fn load_animator_fsm(mut commands: Commands) {
    match crate::statemachine::drivers::animator::load_embedded_arc() {
        Ok(data) => {
            let n = data.states.len();
            commands.insert_resource(PlayerAnimatorData(data));
            info!("FSM: embedded animator FSM loaded — {} states", n);
        }
        Err(e) => error!("FSM: failed to parse embedded animator FSM: {}", e),
    }
}

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
    animator_res: Option<Res<PlayerAnimatorData>>,
    mut pad_mapper: ResMut<crate::control_map::PadMapper>,
    mut commands: Commands,
) {
    let fsm_opt = fsm_res.map(|r| r.0.clone());
    let animator_opt = animator_res.map(|r| r.0.clone());

    let mut spawned = false;
    for entity in &query {
        let mut ec = commands.entity(entity);
        if let Some(fsm) = fsm_opt.clone() {
            ec.insert(FsmRuntime::new(fsm));
        }
        if let Some(animator) = animator_opt.clone() {
            ec.insert(AnimatorRuntime::new(animator));
        }
        info!("FSM: FsmRuntimes attached to player {:?}", entity);
        spawned = true;
    }

    if spawned {
        pad_mapper.clear();
    }
}
