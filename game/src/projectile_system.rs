use bevy::prelude::*;
use crate::oni2_loader::parsers::projectile::{ProjectileDef, HitType, ExplosionMatrix};
use crate::oni2_loader::registries::ProjLibrary;
use crate::fx_system::SpawnFx;

#[derive(Event, Debug, Clone)]
pub struct SpawnProjectileEvent {
    pub name: String,
    pub position: Vec3,
    pub velocity: Vec3,
    pub owner: Entity,
    pub team: u8,
}

#[derive(Component)]
pub struct ProjectileInstance {
    pub owner: Entity,
    pub team: u8,
    pub def: ProjectileDef,
}

#[derive(Component)]
pub struct LinearVelocity(pub Vec3);

#[derive(Component)]
pub struct TumblingRotation(pub Vec3);

#[derive(Component)]
pub struct Lifetime {
    pub timer: Timer,
    pub explode_on_timeout: bool,
}

#[derive(Message)]
pub struct ImpactMessage {
    pub hit_entity: Option<Entity>,
    pub hit_position: Vec3,
    pub hit_normal: Vec3,
    pub hit_type: HitType,
    pub damage: f32,
    pub impulse: Vec3,
}

pub struct ProjectilePlugin;

impl Plugin for ProjectilePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ImpactMessage>()
           .add_observer(handle_spawn_projectile)
           .add_systems(Update, (
               projectile_kinematics_system,
               projectile_collision_system,
               projectile_lifetime_system,
               damage_router_system,
           ).chain());
    }
}

fn handle_spawn_projectile(
    trigger: On<SpawnProjectileEvent>,
    mut commands: Commands,
    proj_lib: Res<ProjLibrary>,
) {
    let ev = trigger.event();
    let lower_name = ev.name.to_lowercase();
    if let Some(def) = proj_lib.projectiles.get(&lower_name) {
        let mut ec = commands.spawn((
            Transform::from_translation(ev.position),
            GlobalTransform::default(),
            ProjectileInstance {
                owner: ev.owner,
                team: ev.team,
                def: def.clone(),
            },
            LinearVelocity(ev.velocity),
            Lifetime {
                timer: Timer::from_seconds(def.lifetime().max(0.1), TimerMode::Once),
                explode_on_timeout: def.explode_on_timeout(),
            },
        ));

        // Add tumbling rotation based on the class tumbling rate if applicable
        if def.proj_class().to_lowercase() == "modelprojectile" {
            // Placeholder: tumble rate logic based on game data
            ec.insert(TumblingRotation(Vec3::new(2.0, 1.0, 0.5))); // Generic tumble
        }

        let entity = ec.id();

        if let Some(ref fx) = def.flight_fx() {
            commands.trigger(SpawnFx {
                name: fx.clone(),
                at: None,
                parent: Some(entity),
            });
        }
    } else {
        warn!("SpawnProjectile: '{}' not found in ProjLibrary", ev.name);
    }
}

fn projectile_kinematics_system(
    time: Res<Time>,
    mut query: Query<(&mut Transform, &mut LinearVelocity, Option<&TumblingRotation>, &ProjectileInstance)>,
) {
    let dt = time.delta_secs();
    let gravity = Vec3::new(0.0, -9.81, 0.0);

    for (mut transform, mut velocity, tumble, instance) in &mut query {
        let grav_force = gravity * instance.def.gravity_factor();
        velocity.0 += grav_force * dt;
        transform.translation += velocity.0 * dt;

        if let Some(t) = tumble {
            transform.rotate_local_x(t.0.x * dt);
            transform.rotate_local_y(t.0.y * dt);
            transform.rotate_local_z(t.0.z * dt);
        }
    }
}

fn projectile_collision_system(
    mut commands: Commands,
    query: Query<(Entity, &Transform, &LinearVelocity, &ProjectileInstance)>,
    // TODO: Need physics raycasting, using rapier or simple distance checks for now
) {
    // For now, no-op since physics integration is required.
    // In actual implementation, cast a ray from prev_position to current_position.
    // If hit checking team: if hit_actor.team == instance.team { continue; }
}

fn projectile_lifetime_system(
    mut commands: Commands,
    time: Res<Time>,
    mut query: Query<(Entity, &Transform, &ProjectileInstance, &mut Lifetime)>,
) {
    for (entity, transform, instance, mut lifetime) in &mut query {
        if lifetime.timer.tick(time.delta()).just_finished() {
            if lifetime.explode_on_timeout {
                if let Some(ref explode_fx) = instance.def.explode_fx() {
                    commands.trigger(SpawnFx {
                        name: explode_fx.clone(),
                        at: Some(transform.translation),
                        parent: None,
                    });
                }
            }
            commands.entity(entity).despawn();
        }
    }
}

fn damage_router_system(
    mut messages: MessageReader<ImpactMessage>,
    mut health_query: Query<&mut crate::combat::components::Health>,
) {
    for ev in messages.read() {
        if let Some(hit_entity) = ev.hit_entity {
            if let Ok(mut health) = health_query.get_mut(hit_entity) {
                health.current -= ev.damage;
                if health.current <= 0.0 {
                    info!("Entity {:?} destroyed by projectile damage", hit_entity);
                }
            }
        }
    }
}
