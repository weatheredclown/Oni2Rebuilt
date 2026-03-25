use bevy::prelude::*;
use bevy_hanabi::prelude::*;

use crate::oni2_loader::components::ActorFxType;
use crate::oni2_loader::registries::{FxLibrary, ParticleLibrary, try_load_ptx};
use crate::scroni::vm::ScrOniSysEvent;

#[derive(Event, Debug, Clone)]
pub struct SpawnFx {
    pub name: String,
    pub at: Option<Vec3>,
    pub parent: Option<Entity>,
    pub start_active: bool,
}

#[derive(Event, Debug, Clone)]
pub struct SpawnPtx {
    pub ptx_name: String,
    pub rate: f32, // BirthRate
    pub num_particles: i32,
    pub at: Option<Vec3>, // Offset
    pub parent: Option<Entity>,
    pub start_active: bool,
}

#[derive(Component)]
pub struct FxStartInactive;

/// Component storing intended active state for FX.
#[derive(Component)]
pub struct IntendedFxState(pub bool);

pub struct FxPlugin;

impl Plugin for FxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HanabiPlugin)
           .add_observer(handle_spawn_fx)
           .add_observer(handle_spawn_ptx)
           .add_observer(handle_fx_action)
           .add_systems(Update, handle_actor_fx_attachments)
           .add_systems(Update, apply_fx_start_inactive)
           .add_systems(Update, uv_animator_system)
           .add_systems(Update, (handle_actor_sleep, handle_actor_awake));
    }
}

fn apply_fx_start_inactive(
    mut commands: Commands,
    mut q: Query<(Entity, &mut EffectSpawner), (Added<EffectSpawner>, With<FxStartInactive>)>,
) {
    for (entity, mut spawner) in &mut q {
        spawner.active = false;
        commands.entity(entity).remove::<FxStartInactive>();
        // Explicitly clear initial reset if it tries to spawn once
        spawner.reset();
    }
}

/// System for animating mesh UV coordinates dynamically.
/// FUTURE ARCHITECTURE NOTE:
/// Modifying `ResMut<Assets<Mesh>>` applies the UV transformation globally to ALL entities sharing 
/// this mesh buffer. For environmental geometry (like ambient rising steam/fire instances), this natively
/// synchronizes the scrolling while remaining extremely fast (CPU only visits the vertices once per level).
/// However, if future designs require isolated, parameterized rendering per-instance (e.g. a character 
/// walking through steam causing localized warping), these static mesh buffers should be decoupled per-entity,
/// or ideally offloaded into a custom Bevy `MaterialExtension` shader where UV offsets are passed as
/// lightweight instance Uniforms directly to the GPU instead of mutating native float vertices on the CPU.
pub fn uv_animator_system(
    time: Res<Time>,
    entity_lib: Option<Res<crate::oni2_loader::registries::EntityLibrary>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let dt = time.delta_secs();
    if dt == 0.0 { return; }
    
    let lib = if let Some(l) = entity_lib { l } else { return; };
    
    // Natively iterate the layout definitions rather than the instantiated entities so that
    // we strictly perform mathematical mutations only once per pooled Handle<Mesh> memory allocation.
    for ent_type in lib.entities.values() {
        for (_i, (mat_idx, handle)) in ent_type.sub_meshes.iter().enumerate() {
            let animators = ent_type.material_animators.get(*mat_idx);
            if let Some(anims) = animators {
                let mut u_speed = 0.0;
                let mut v_speed = 0.0;
                let mut r_speed = 0.0;
                let mut s_speed = 0.0;
                
                for anim in anims {
                    if anim.slides_speed != 0.0 { u_speed = anim.slides_speed; }
                    if anim.slidet_speed != 0.0 { v_speed = anim.slidet_speed; }
                    if anim.rotate_speed != 0.0 { r_speed = anim.rotate_speed; }
                    if anim.scalet_speed != 0.0 { s_speed = anim.scalet_speed; }
                }
                
                if u_speed == 0.0 && v_speed == 0.0 && r_speed == 0.0 && s_speed == 0.0 {
                    continue;
                }
                
                if let Some(mesh) = meshes.get_mut(handle) {
                    if let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) = mesh.attribute_mut(Mesh::ATTRIBUTE_UV_0) {
                        let du = u_speed * dt;
                        let dv = v_speed * dt;
                        let cos_r = (r_speed * dt).cos();
                        let sin_r = (r_speed * dt).sin();
                        let scale = if s_speed != 0.0 { 1.0 + s_speed * dt } else { 1.0 };
                        
                        for uv in uvs.iter_mut() {
                            let mut u = uv[0];
                            let mut v = uv[1];
                            
                            u -= du;
                            v -= dv;
                            
                            // Rotation around 0.5 (texture center pivot point)
                            if r_speed != 0.0 || s_speed != 0.0 {
                                let cu = u - 0.5;
                                let cv = v - 0.5;
                                
                                let rotated_u = cu * cos_r - cv * sin_r;
                                let rotated_v = cu * sin_r + cv * cos_r;
                                
                                u = rotated_u * scale + 0.5;
                                v = rotated_v * scale + 0.5;
                            }
                            
                            // Float wrapping logic to prevent precision degradation
                            // Only safely bounds if geometry does not natively span across multiple units smoothly.
                            // Left unbounded purposely to preserve identical tiled tiling mappings.
                            uv[0] = u;
                            uv[1] = v;
                        }
                    }
                }
            }
        }
    }
}

fn get_or_create_ptx_asset(
    ptx_name: &str,
    ptx_def: &crate::oni2_loader::parsers::particle::ParticleSystemDef,
    rate_override: f32,
    num_particles_override: i32,
    effects: &mut ResMut<Assets<EffectAsset>>,
    effect_cache: &mut Local<std::collections::HashMap<String, Handle<EffectAsset>>>,
) -> Handle<EffectAsset> {
    let cache_key = format!("{}_{}_{}", ptx_name, rate_override, num_particles_override);
    if let Some(handle) = effect_cache.get(&cache_key) {
        return handle.clone();
    }

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

    let rate = if rate_override > 0.0 { rate_override } else { ptx_def.rate };
    let spawner = if num_particles_override > 0 {
        bevy_hanabi::SpawnerSettings::burst((num_particles_override as f32).into(), bevy_hanabi::CpuValue::Single(rate))
    } else {
        bevy_hanabi::SpawnerSettings::rate(rate.into())
    };

    let texture_slot = writer.lit(0u32).expr();

    let mut init_sprite_index = None;
    let mut update_sprite_index = None;

    if ptx_def.grid_x > 1 || ptx_def.grid_y > 1 {
        let frames = (ptx_def.end_tile as i32 - ptx_def.start_tile as i32 + 1).max(1);
        let start_tile = writer.lit(ptx_def.start_tile as i32);
        
        if ptx_def.frame_rate > 0.0 {
            // Animated
            let age = writer.attr(Attribute::AGE);
            let frame_rate = writer.lit(ptx_def.frame_rate);
            let frame_idx = age.mul(frame_rate).cast(ScalarType::Int).rem(writer.lit(frames));
            update_sprite_index = Some(SetAttributeModifier::new(Attribute::SPRITE_INDEX, start_tile.add(frame_idx).expr()));
        } else {
            // Static randomize
            let rnd = writer.rand(ScalarType::Float).mul(writer.lit(frames as f32)).cast(ScalarType::Int);
            init_sprite_index = Some(SetAttributeModifier::new(Attribute::SPRITE_INDEX, start_tile.add(rnd).expr()));
        }
    }

    let mut module = writer.finish();
    module.add_texture_slot("color");

    let mut asset = EffectAsset::new(32768, spawner, module)
        .init(init_pos)
        .init(init_vel)
        .init(init_age)
        .init(init_lifetime)
        .update(LinearDragModifier::new(drag_coef))
        .render(ColorOverLifetimeModifier { gradient: color_gradient, ..Default::default() })
        .render(SizeOverLifetimeModifier { gradient: size_gradient, screen_space_size: false, ..Default::default() })
        .render(ParticleTextureModifier {
            texture_slot,
            sample_mapping: ImageSampleMapping::Modulate,
        })
        .render(OrientModifier::new(OrientMode::FaceCameraPosition));

    if let Some(mut init_sp) = init_sprite_index {
        asset = asset.init(init_sp);
    }
    if let Some(update_sp) = update_sprite_index {
        asset = asset.update(update_sp);
    }
    if ptx_def.grid_x > 1 || ptx_def.grid_y > 1 {
        asset = asset.render(FlipbookModifier {
            sprite_grid_size: UVec2::new(ptx_def.grid_x, ptx_def.grid_y),
        });
    }

    let handle = effects.add(asset);

    effect_cache.insert(cache_key, handle.clone());
    handle
}

/// Observer that listens for SpawnFx events and spawns the appropriate effect
fn handle_spawn_fx(
    trigger: On<SpawnFx>,
    mut commands: Commands,
    fx_lib: Res<FxLibrary>,
    ptx_lib: Res<ParticleLibrary>,
    mut effects: ResMut<Assets<EffectAsset>>,
    mut effect_cache: Local<std::collections::HashMap<String, Handle<EffectAsset>>>,
    actor_sleep_query: Query<&crate::oni2_loader::components::ActorAsleep>,
) {
    let ev = trigger.event();
    let lower_name = ev.name.to_lowercase();
    
    if let Some(fx_def) = fx_lib.effects.get(&lower_name) {
        let ptx_ref = match fx_def {
            crate::oni2_loader::parsers::effect::EffectDef::Particle(p) => Some(&p.system),
            crate::oni2_loader::parsers::effect::EffectDef::DelayedParticle(p) => Some(&p.system),
            crate::oni2_loader::parsers::effect::EffectDef::HealthIndicator(p) => Some(&p.system),
            _ => None,
        };

        if let Some(pref) = ptx_ref {
            let ptx_name = pref.system_name.to_lowercase();
            if let Some(ptx_def) = ptx_lib.systems.get(&ptx_name) {
                let handle = get_or_create_ptx_asset(&ptx_name, ptx_def, 0.0, 0, &mut effects, &mut effect_cache);

                let is_asleep = ev.parent.and_then(|p| actor_sleep_query.get(p).ok()).is_some();
                let effective_start_active = ev.start_active && !is_asleep;

                let mut ec = commands.spawn((
                    ParticleEffect::new(handle),
                    EffectMaterial {
                        images: vec![ptx_def.texture.clone()],
                    },
                    Transform::from_translation(ev.at.unwrap_or(Vec3::ZERO)),
                    IntendedFxState(ev.start_active),
                ));

                if !effective_start_active {
                    ec.insert(FxStartInactive);
                }

                if let Some(parent) = ev.parent {
                    ec.set_parent_in_place(parent);
                }
            }
        }
    } else {
        warn!("SpawnFx: Effect '{}' not found in FxLibrary", ev.name);
    }
}

/// Observer that listens for SpawnPtx events and spawns a particle system directly
fn handle_spawn_ptx(
    trigger: On<SpawnPtx>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut ptx_lib: ResMut<ParticleLibrary>,
    mut effects: ResMut<Assets<EffectAsset>>,
    mut effect_cache: Local<std::collections::HashMap<String, Handle<EffectAsset>>>,
    actor_sleep_query: Query<&crate::oni2_loader::components::ActorAsleep>,
) {
    let ev = trigger.event();
    let mut ptx_name = ev.ptx_name.to_lowercase();
    if ptx_name.ends_with(".ptx") {
        ptx_name = ptx_name.strip_suffix(".ptx").unwrap().to_string();
    }

    try_load_ptx(&ptx_name, &asset_server, &mut ptx_lib, &mut images);

    if let Some(ptx_def) = ptx_lib.systems.get(&ptx_name) {
        let handle = get_or_create_ptx_asset(&ptx_name, ptx_def, ev.rate, ev.num_particles, &mut effects, &mut effect_cache);

        let is_asleep = ev.parent.and_then(|p| actor_sleep_query.get(p).ok()).is_some();
        let effective_start_active = ev.start_active && !is_asleep;

        let mut ec = commands.spawn((
            ParticleEffect::new(handle),
            EffectMaterial {
                images: vec![ptx_def.texture.clone()],
            },
            Transform::from_translation(ev.at.unwrap_or(Vec3::ZERO)),
            IntendedFxState(ev.start_active),
        ));

        if !effective_start_active {
            ec.insert(FxStartInactive);
        }

        if let Some(parent) = ev.parent {
            ec.set_parent_in_place(parent);
        }
    } else {
        warn!("SpawnPtx: Particle system '{}' not found in ParticleLibrary", ev.ptx_name);
    }
}

/// System that watches for newly spawned entities with `ActorFxType` and attaches their effects
fn handle_actor_fx_attachments(
    mut commands: Commands,
    query: Query<(Entity, &ActorFxType), Added<ActorFxType>>,
    fx_lib: Res<FxLibrary>,
    _ptx_lib: Res<ParticleLibrary>,
) {
    for (entity, fx_type) in query.iter() {
        if let Some(ref fx_name) = fx_type.fx_name {
            let lower_name = fx_name.to_lowercase();
            if let Some(_fx_def) = fx_lib.effects.get(&lower_name) {
                info!("Attaching FX {} to entity {:?} (StartActive={})", fx_name, entity, fx_type.start_active);
                commands.trigger(SpawnFx {
                    name: fx_name.clone(),
                    at: Some(fx_type.ptx_offset),
                    parent: Some(entity),
                    start_active: fx_type.start_active,
                });
            } else {
                warn!("ActorFxType: Effect '{}' not found in FxLibrary", fx_name);
            }
        }
        
        if let Some(ref ptx_name) = fx_type.ptx_name {
            info!("Attaching Particle System {} to entity {:?} (StartActive={})", ptx_name, entity, fx_type.start_active);
            commands.trigger(SpawnPtx {
                ptx_name: ptx_name.clone(),
                rate: fx_type.ptx_birth_rate,
                num_particles: fx_type.ptx_num_particles,
                at: Some(fx_type.ptx_offset),
                parent: Some(entity),
                start_active: fx_type.start_active,
            });
        }
    }
}

#[derive(Event, Debug, Clone)]
pub struct FxAction {
    pub action: String,
    pub target: Entity,
}

/// Observer to intercept FxAction triggers and execute FX activation/deactivation.
fn handle_fx_action(
    trigger: On<FxAction>,
    children_query: Query<&Children>,
    mut spawner_query: Query<(&mut EffectSpawner, &mut IntendedFxState)>,
    actor_sleep_query: Query<&crate::oni2_loader::components::ActorAsleep>,
) {
    let evt = trigger.event();
    let act_lower = evt.action.to_lowercase();
    if act_lower == "activate" || act_lower == "deactivate" {
        let to_active = act_lower == "activate";
        let is_asleep = actor_sleep_query.get(evt.target).is_ok();
        let mut stack = vec![evt.target];
        while let Some(ent) = stack.pop() {
            if let Ok((mut spawner, mut intended)) = spawner_query.get_mut(ent) {
                intended.0 = to_active;
                spawner.active = to_active && !is_asleep;
            }
            if let Ok(children) = children_query.get(ent) {
                stack.extend(children.iter());
            }
        }
    }
}

fn handle_actor_sleep(
    query: Query<Entity, Added<crate::oni2_loader::components::ActorAsleep>>,
    children_query: Query<&Children>,
    mut spawner_query: Query<&mut EffectSpawner, With<IntendedFxState>>,
) {
    for entity in query.iter() {
        let mut stack = vec![entity];
        while let Some(ent) = stack.pop() {
            if let Ok(mut spawner) = spawner_query.get_mut(ent) {
                spawner.active = false;
            }
            if let Ok(children) = children_query.get(ent) {
                stack.extend(children.iter());
            }
        }
    }
}

fn handle_actor_awake(
    mut removed: RemovedComponents<crate::oni2_loader::components::ActorAsleep>,
    children_query: Query<&Children>,
    mut spawner_query: Query<(&mut EffectSpawner, &IntendedFxState)>,
) {
    for entity in removed.read() {
        let mut stack = vec![entity];
        while let Some(ent) = stack.pop() {
            if let Ok((mut spawner, intended)) = spawner_query.get_mut(ent) {
                spawner.active = intended.0;
            }
            if let Ok(children) = children_query.get(ent) {
                stack.extend(children.iter());
            }
        }
    }
}
