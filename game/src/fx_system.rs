use bevy::prelude::*;
use bevy_hanabi::prelude::*;

use crate::oni2_loader::components::ActorFxType;
use crate::oni2_loader::registries::{FxLibrary, ParticleLibrary};
use crate::scroni::vm::ScrOniSysEvent;

#[derive(Event, Debug, Clone)]
pub struct SpawnFx {
    pub name: String,
    pub at: Option<Vec3>,
    pub parent: Option<Entity>,
}

pub struct FxPlugin;

impl Plugin for FxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HanabiPlugin)
           .add_observer(handle_spawn_fx)
           .add_systems(Update, handle_actor_fx_attachments);
    }
}

/// Observer that listens for SpawnFx events and spawns the appropriate effect
fn handle_spawn_fx(
    trigger: On<SpawnFx>,
    mut _commands: Commands,
    fx_lib: Res<FxLibrary>,
    _ptx_lib: Res<ParticleLibrary>,
) {
    let lower_name = trigger.name.to_lowercase();
    if let Some(_fx_def) = fx_lib.effects.get(&lower_name) {
        info!("Spawn Fx: found {} in library, spawning at {:?}", trigger.name, trigger.at);
        // TODO: Actually spawn bevy_hanabi ParticleEffectBundle here
    } else {
        warn!("SpawnFx: Effect '{}' not found in FxLibrary", trigger.name);
    }
}

/// System that watches for newly spawned entities with `ActorFxType` and attaches their effects
fn handle_actor_fx_attachments(
    mut commands: Commands,
    query: Query<(Entity, &ActorFxType), Added<ActorFxType>>,
    fx_lib: Res<FxLibrary>,
) {
    for (entity, fx_type) in query.iter() {
        let lower_name = fx_type.fx_name.to_lowercase();
        if let Some(_fx_def) = fx_lib.effects.get(&lower_name) {
            info!("Attaching FX {} to entity {:?}", fx_type.fx_name, entity);
            // Spawn particle system as a child of `entity`
        } else {
            warn!("ActorFxType: Effect '{}' not found in FxLibrary", fx_type.fx_name);
        }
    }
}
