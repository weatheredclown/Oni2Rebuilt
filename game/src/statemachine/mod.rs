/*
 * statemachine/mod.rs — StateMachinePlugin: player animation FSM.
 *
 * insert_player_fsm attaches `FsmRuntime` to any newly spawned Player entity
 * using the FSM name carried in its `PadFsmName` component (sourced from the
 * actor XML's `<Pad FSM="..."/>` attribute).  Input-side FSMs — player, enemy,
 * enemy_combo, etc. — all flow through the shared `PadFsmCache` since they
 * share the legacy `aiInputStateMachineData` loader.
 *
 * fsm_update_system (in PlayerPlugin) evaluates input pad flags each frame
 * and drives ONI2 animation transitions.
 */
pub mod core;
pub mod drivers;
pub mod pad_fsm;
pub mod runtime;
pub mod types;

pub use pad_fsm::PadFsmCache;
pub use runtime::{
    AnimatorBroadcastEvent, AnimatorRuntime, FsmRuntime, animator_update_system, fsm_update_system,
};

use bevy::prelude::*;
use std::sync::Arc;

use crate::player::components::{PadFsmName, Player};

use core::SmData;

/// Bevy resource wrapping the top-level embedded orchestration FSM data.
#[derive(Resource)]
pub struct PlayerAnimatorData(
    pub Arc<SmData<crate::statemachine::drivers::animator::AnimatorDriver>>,
);

pub struct StateMachinePlugin;

impl Plugin for StateMachinePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<AnimatorBroadcastEvent>();
        app.init_resource::<PadFsmCache>();
        app.add_systems(Startup, load_animator_fsm);
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

/// Attach FsmRuntime + AnimatorRuntime to any player entity that was just
/// spawned.  The input FSM name comes from the entity's `PadFsmName`
/// component; falls back to `"player"` if one is missing (sandbox path).
fn insert_player_fsm(
    query: Query<(Entity, Option<&PadFsmName>), Added<Player>>,
    animator_res: Option<Res<PlayerAnimatorData>>,
    mut pad_fsm_cache: ResMut<PadFsmCache>,
    mut pad_mapper: ResMut<crate::control_map::PadMapper>,
    mut commands: Commands,
) {
    let animator_opt = animator_res.map(|r| r.0.clone());

    let mut spawned = false;
    for (entity, pad_fsm) in &query {
        let fsm_name = pad_fsm.map(|p| p.0.as_str()).unwrap_or("player");

        let fsm_arc = pad_fsm_cache.get_or_load(fsm_name);

        let mut ec = commands.entity(entity);
        if let Some(fsm) = fsm_arc {
            ec.insert(FsmRuntime::new(fsm));
        } else {
            warn!(
                "FSM: could not load '{}.fsm' for player {:?}",
                fsm_name, entity
            );
        }
        if let Some(animator) = animator_opt.clone() {
            ec.insert(AnimatorRuntime::new(animator));
        }
        info!(
            "FSM: runtimes attached to player {:?} (pad_fsm='{}')",
            entity, fsm_name
        );
        spawned = true;
    }

    if spawned {
        pad_mapper.clear();
    }
}
