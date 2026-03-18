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
    mut commands: Commands,
    fx_lib: Res<FxLibrary>,
    ptx_lib: Res<ParticleLibrary>,
    mut effects: ResMut<Assets<EffectAsset>>,
    mut effect_cache: Local<std::collections::HashMap<String, Handle<EffectAsset>>>,
) {
    let ev = trigger.event();
    let lower_name = ev.name.to_lowercase();
    
    if let Some(fx_def) = fx_lib.effects.get(&lower_name) {
        // Extract the particle system ref based on effect type
        let ptx_ref = match fx_def {
            crate::oni2_loader::parsers::effect::EffectDef::Particle(p) => Some(&p.system),
            crate::oni2_loader::parsers::effect::EffectDef::DelayedParticle(p) => Some(&p.system),
            crate::oni2_loader::parsers::effect::EffectDef::HealthIndicator(p) => Some(&p.system),
            _ => None,
        };

        if let Some(pref) = ptx_ref {
            let ptx_name = pref.system_name.to_lowercase();
            if let Some(ptx_def) = ptx_lib.systems.get(&ptx_name) {
                // Get or create Hanabi EffectAsset
                let handle = effect_cache.entry(ptx_name.clone()).or_insert_with(|| {
                    let mut color_gradient = bevy_hanabi::Gradient::new();
                    color_gradient.add_key(0.0, ptx_def.color_birth.to_linear().to_vec4());
                    color_gradient.add_key(1.0, ptx_def.color_death.to_linear().to_vec4());

                    let mut size_gradient = bevy_hanabi::Gradient::new();
                    size_gradient.add_key(0.0, Vec3::splat(ptx_def.radius_birth.x.max(0.1)));
                    size_gradient.add_key(1.0, Vec3::splat(ptx_def.radius_birth.y.max(0.1)));

                    let writer = ExprWriter::new();
                    let age = writer.lit(0.).expr();
                    let init_pos = SetPositionSphereModifier {
                        center: writer.lit(Vec3::ZERO).expr(),
                        radius: writer.lit(ptx_def.position_var.length()).expr(),
                        dimension: ShapeDimension::Volume,
                    };
                    
                    let init_vel = SetVelocitySphereModifier {
                        center: writer.lit(Vec3::ZERO).expr(),
                        speed: writer.lit(ptx_def.velocity.length().max(1.0)).expr(),
                    };
                    
                    let init_age = SetAttributeModifier::new(Attribute::AGE, age);
                    let init_lifetime = SetAttributeModifier::new(Attribute::LIFETIME, writer.lit(ptx_def.life).expr());
                    let drag_coef = writer.lit(ptx_def.velocity_damping.length()).expr();

                    let spawner = bevy_hanabi::SpawnerSettings::rate(ptx_def.rate.into());

                    effects.add(EffectAsset::new(32768, spawner, writer.finish())
                        .init(init_pos)
                        .init(init_vel)
                        .init(init_age)
                        .init(init_lifetime)
                        .update(LinearDragModifier::new(drag_coef))
                        .render(ColorOverLifetimeModifier { gradient: color_gradient, ..Default::default() })
                        .render(SizeOverLifetimeModifier { gradient: size_gradient, screen_space_size: false, ..Default::default() })
                    )
                }).clone();

                let mut ec = commands.spawn((
                    ParticleEffect::new(handle),
                    Transform::from_translation(ev.at.unwrap_or(Vec3::ZERO)),
                ));

                if let Some(parent) = ev.parent {
                    ec.set_parent_in_place(parent);
                }
            }
        }
    } else {
        warn!("SpawnFx: Effect '{}' not found in FxLibrary", ev.name);
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
            commands.trigger(SpawnFx {
                name: fx_type.fx_name.clone(),
                at: None,
                parent: Some(entity),
            });
        } else {
            warn!("ActorFxType: Effect '{}' not found in FxLibrary", fx_type.fx_name);
        }
    }
}
