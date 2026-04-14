use avian3d::prelude::*;
use bevy::prelude::*;

use crate::combat::events::InjureMessage;
use crate::combat::components::Health;
use crate::oni2_loader::registries::ExplosionRegistry;

#[derive(Component)]
pub struct ActiveExplosion {
    pub name: String,
    pub at: Vec3,
    pub timer: f32, // Elapsed time since spawned
    pub blast_duration: f32, // Track max lifetime from bounding definition
    pub spawn_source: Entity,
}



pub fn update_explosion_system(
    mut commands: Commands,
    mut explosions: Query<(Entity, &mut ActiveExplosion)>,
    registry: Res<ExplosionRegistry>,
    time: Res<Time>,
    mut injure_writer: MessageWriter<InjureMessage>,
    targets: Query<(Entity, &Transform, &Health)>,
) {
    let dt = time.delta_secs();

    for (entity, mut active) in &mut explosions {
        active.timer += dt;

        let def_opt = registry.explosions.get(&active.name.to_lowercase());
        let Some(def) = def_opt else { continue };

        // Process delayed FX dynamically based on timer
        for fx in &def.fx {
            if active.timer >= fx.delay && active.timer - dt < fx.delay {
                info!("(Delayed {:.2}s) Spawning explosion FX: {}", fx.delay, fx.fx_type);
            }
        }

        // Process damage overlap checks
        if let Some(ellipsoid) = &def.ellipsoid {
            if active.timer <= ellipsoid.blast_duration {
                let max_radii = Vec3::from(ellipsoid.max_radii);
                // Simplify ellipsoid bounding as sphere for now (using largest radius)
                let radius = max_radii.max_element();
                
                for (t_ent, t_tf, t_health) in &targets {
                    let dist_sq = t_tf.translation.distance_squared(active.at);
                    if dist_sq < radius * radius {
                        // Apply damage! For continuous, we scale by dt. 
                        let dmg = if ellipsoid.continuous_damage {
                            ellipsoid.max_damage * dt
                        } else {
                            if active.timer - dt <= 0.0 {
                                ellipsoid.max_damage
                            } else {
                                0.0
                            }
                        };

                        if dmg > 0.0 {
                            injure_writer.write(InjureMessage {
                                target: t_ent,
                                attacker: None, // Or active.spawn_source if attributed to script
                                damage: dmg,
                                hit_type: "explosion".to_string(),
                                from: Some(active.at),
                                play_react: true,
                                disable_creature_detect: false,
                                attack_class: None,
                                attack_strength: None,
                                strike_react_enum: None, // Explode reaction!
                            });
                        }
                    }
                }
            }
        }

        if active.timer > active.blast_duration {
            commands.entity(entity).despawn();
        }
    }
}
