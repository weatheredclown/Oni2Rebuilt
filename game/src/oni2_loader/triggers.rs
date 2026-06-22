/*
 * oni2_loader/triggers.rs — CameraTrigger, ForceVectorTrigger, SectionTrigger,
 * and Conveyor surface systems.
 */
use bevy::prelude::*;
use avian3d::prelude::{ContactGraph, LinearVelocity, Mass};
use super::components::{
    CameraTrigger, Conveyor, ConveyorPush, CurrentCheckpointIndex, ForceVectorTrigger,
    SectionTrigger,
};
use crate::oni2_loader::environment::ActiveCameraPackage;

/// System that switches the global `ActiveCameraPackage` resource when the player intersects a `CameraTrigger`.
pub fn update_camera_triggers(
    mut active_camera_package: ResMut<ActiveCameraPackage>,
    trigger_query: Query<(&CameraTrigger, &GlobalTransform)>,
    player_query: Query<&GlobalTransform, With<crate::player::components::Player>>,
) {
    let Some(player_tf) = player_query.iter().next() else {
        return;
    };
    let player_pos = player_tf.translation();

    for (trigger, trigger_tf) in &trigger_query {
        let dist = player_pos.distance(trigger_tf.translation());
        if dist <= trigger.radius {
            if active_camera_package.name != trigger.camera_package {
                active_camera_package.name = trigger.camera_package.clone();
                info!(
                    "CameraTrigger activated: switching active camera package to '{}'",
                    trigger.camera_package
                );
            }
        }
    }
}

/// System that applies force impulses to entities inside `ForceVectorTrigger` fields.
/// Runs in FixedUpdate after movement and controls.
pub fn apply_force_vector_triggers(
    trigger_query: Query<(&ForceVectorTrigger, &GlobalTransform)>,
    mut entity_query: Query<(&GlobalTransform, &mut LinearVelocity, Option<&Mass>)>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (trigger, trigger_tf) in &trigger_query {
        let trigger_pos = trigger_tf.translation();
        for (entity_tf, mut velocity, mass_opt) in &mut entity_query {
            let dist = entity_tf.translation().distance(trigger_pos);
            if dist <= trigger.radius {
                let mass = mass_opt.map(|m| m.0).unwrap_or(60.0);
                if mass > 0.0 {
                    // acceleration = ForceVector / mass
                    // velocity_change = acceleration * dt = (ForceVector / mass) * dt
                    let dv = (trigger.force_vector / mass) * dt;
                    velocity.x += dv.x;
                    velocity.y += dv.y;
                    velocity.z += dv.z;
                }
            }
        }
    }
}

/// System that monitors player entry to SectionTriggers, checking checkpoint bounds and triggering spawning/despawning.
pub fn update_section_triggers(
    mut trigger_query: Query<(&mut SectionTrigger, &GlobalTransform)>,
    player_query: Query<&GlobalTransform, With<crate::player::components::Player>>,
    checkpoint_idx: Res<CurrentCheckpointIndex>,
) {
    let Some(player_tf) = player_query.iter().next() else {
        return;
    };
    let player_pos = player_tf.translation();
    let current_checkpoint = checkpoint_idx.0;

    for (mut trigger, trigger_tf) in &mut trigger_query {
        let dist = player_pos.distance(trigger_tf.translation());
        let player_is_inside = dist <= trigger.radius;
        let player_was_inside = trigger.player_was_inside;
        trigger.player_was_inside = player_is_inside;

        if trigger.trigger_only_once && trigger.has_fired {
            continue;
        }

        // We only trigger when player transitions from outside to inside (just entered)
        if player_is_inside && !player_was_inside {
            // Check checkpoint conditions
            if current_checkpoint >= trigger.min_checkpoint_index
                && (trigger.max_checkpoint_index < 0 || current_checkpoint <= trigger.max_checkpoint_index)
            {
                trigger.has_fired = true;
                info!(
                    "SectionTrigger entered at checkpoint {}: sections to spawn: '{}', sections to destroy: '{}'",
                    current_checkpoint, trigger.sections_to_spawn, trigger.sections_to_destroy
                );
            }
        }
    }
}

/// Pushes movers standing on a [`Conveyor`] surface along the conveyor's
/// forward axis at its `speed`.  Mirrors `crmover::Bound`'s conveyor branch:
/// `v = GetMatrix().c; v.y = 0; v.Normalize(); v *= ConveyorSpeed;
/// SetSlide(true, v)`.
///
/// Runs in FixedUpdate *before* `tnua_basis_from_linvel`, which folds
/// `LinearVelocity` into Tnua's walk basis and then zeroes it.  Adding here
/// means the conveyor velocity becomes part of the mover's desired motion
/// (carrying them along even while idle); adding *after* the bridge would let
/// Tnua's basis friction cancel it on the next tick.
pub fn apply_conveyor_system(
    mut commands: Commands,
    contact_graph: Res<ContactGraph>,
    conveyors: Query<(&Conveyor, &GlobalTransform)>,
    parents: Query<&ChildOf>,
    mut movers: Query<&mut LinearVelocity>,
    mut pushes: Query<&mut ConveyorPush>,
) {
    // Clear last tick's recorded contribution; we re-accumulate below so the
    // anim system can subtract the belt's push from the gait-driving velocity.
    for mut p in &mut pushes {
        p.0 = Vec3::ZERO;
    }

    for pair in contact_graph.iter_active_touching() {
        let (Some(b1), Some(b2)) = (pair.body1, pair.body2) else {
            continue;
        };

        // Walk up the hierarchy from a contact body to the entity carrying the
        // `Conveyor` (the parent entity owns it; the colliders may be children).
        let find_conveyor = |mut ent: Entity| -> Option<(Conveyor, GlobalTransform)> {
            loop {
                if let Ok((conv, gt)) = conveyors.get(ent) {
                    return Some((*conv, *gt));
                }
                match parents.get(ent) {
                    Ok(child_of) => ent = child_of.parent(),
                    Err(_) => return None,
                }
            }
        };
        // A mover is any body with a `LinearVelocity` (player + AI creatures in
        // both the Tnua and Dynamic backends; conveyors are Static so they're
        // never matched here).
        let find_mover = |mut ent: Entity| -> Option<Entity> {
            loop {
                if movers.contains(ent) {
                    return Some(ent);
                }
                match parents.get(ent) {
                    Ok(child_of) => ent = child_of.parent(),
                    Err(_) => return None,
                }
            }
        };

        // Match (conveyor, mover) in either pair order.
        let resolved = match (find_conveyor(b1), find_mover(b2)) {
            (Some((conv, gt)), Some(mover)) => Some((conv, gt, mover)),
            _ => match (find_conveyor(b2), find_mover(b1)) {
                (Some((conv, gt)), Some(mover)) => Some((conv, gt, mover)),
                _ => None,
            },
        };
        let Some((conveyor, conv_gt, mover_ent)) = resolved else {
            continue;
        };

        // Direction: the engine slides along the conveyor's local +Z
        // (`GetMatrix().c`).  The Oni→Bevy conversion negates X and Z (a
        // similarity transform R_bevy = M·R_oni·M), which maps that local +Z
        // onto Bevy *local -Z* — i.e. `forward()`.  (`oni_forward()`/`back()`
        // is the visual-facing helper and yields the reversed physical
        // direction here — the "runs backwards" symptom.)  Flattened to the
        // horizontal plane and normalized: `v.y = 0; v.Normalize()`.
        //
        // A flat belt you're in contact with means you're on it, so (unlike
        // the octree floor test) we don't gate on the contact normal — that
        // gate silently rejected valid trimesh contacts.
        let mut dir = conv_gt.forward().as_vec3();
        dir.y = 0.0;
        let Some(dir) = dir.try_normalize() else {
            continue;
        };
        let push = dir * conveyor.speed;

        if let Ok(mut vel) = movers.get_mut(mover_ent) {
            vel.x += push.x;
            vel.z += push.z;
        }

        // Record the contribution so locomotion anims can treat motion
        // relative to the belt as the "real" movement.  Lazily attach the
        // component the first time a mover rides a conveyor (one-frame delay
        // before the anim system sees it — negligible).
        match pushes.get_mut(mover_ent) {
            Ok(mut p) => p.0 += push,
            Err(_) => {
                commands.entity(mover_ent).insert(ConveyorPush(push));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::components::Player;

    #[test]
    fn test_camera_trigger() {
        let mut app = App::new();
        app.insert_resource(ActiveCameraPackage {
            name: "DEFAULT_PACKAGE".to_string(),
        });

        // Spawn player at (0.0, 0.0, 0.0)
        let _player_entity = app.world_mut().spawn((
            Player,
            GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 0.0)),
        )).id();

        // Spawn camera trigger at (1.0, 0.0, 0.0) with radius 2.0
        app.world_mut().spawn((
            CameraTrigger {
                radius: 2.0,
                camera_package: "TARGET_PACKAGE".to_string(),
            },
            GlobalTransform::from(Transform::from_xyz(1.0, 0.0, 0.0)),
        ));

        let mut schedule = Schedule::new(Update);
        schedule.add_systems(update_camera_triggers);
        app.add_schedule(schedule);

        app.update();

        let active_cam = app.world().resource::<ActiveCameraPackage>();
        assert_eq!(active_cam.name, "TARGET_PACKAGE");
    }

    #[test]
    fn test_force_vector_trigger() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.init_resource::<Time>();

        // Spawn entity with velocity at (0.0, 0.0, 0.0)
        let entity = app.world_mut().spawn((
            LinearVelocity(Vec3::ZERO),
            GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 0.0)),
            Mass(60.0),
        )).id();

        // Spawn force trigger at (1.0, 0.0, 0.0) with radius 2.0 and force vector (600.0, 0.0, 0.0)
        app.world_mut().spawn((
            ForceVectorTrigger {
                radius: 2.0,
                force_vector: Vec3::new(600.0, 0.0, 0.0),
            },
            GlobalTransform::from(Transform::from_xyz(1.0, 0.0, 0.0)),
        ));

        // Advance time to mock time delta of 0.5s
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(500));

        // Run the system once
        app.world_mut().run_system_once(apply_force_vector_triggers).unwrap();

        let velocity = app.world().entity(entity).get::<LinearVelocity>().unwrap();
        // dv = (ForceVector / mass) * dt = (600.0 / 60.0) * 0.5 = 5.0
        assert!(velocity.x > 4.9 && velocity.x < 5.1, "velocity.x was {}", velocity.x);
    }

    #[test]
    fn test_section_trigger() {
        let mut app = App::new();
        app.insert_resource(CurrentCheckpointIndex(1));

        // Spawn player at (5.0, 0.0, 0.0) (outside trigger)
        let player_entity = app.world_mut().spawn((
            Player,
            GlobalTransform::from(Transform::from_xyz(5.0, 0.0, 0.0)),
        )).id();

        // Spawn section trigger at (0.0, 0.0, 0.0) with radius 2.0
        let trigger_entity = app.world_mut().spawn((
            SectionTrigger {
                radius: 2.0,
                sections_to_spawn: "SpawnSec".to_string(),
                sections_to_destroy: "DestroySec".to_string(),
                trigger_only_once: false,
                min_checkpoint_index: 0,
                max_checkpoint_index: 2,
                has_fired: false,
                player_was_inside: false,
            },
            GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 0.0)),
        )).id();

        let mut schedule = Schedule::new(Update);
        schedule.add_systems(update_section_triggers);
        app.add_schedule(schedule);

        // Run app - player is outside, nothing should fire
        app.update();
        let trigger = app.world().entity(trigger_entity).get::<SectionTrigger>().unwrap();
        assert!(!trigger.has_fired);
        assert!(!trigger.player_was_inside);

        // Move player inside trigger
        app.world_mut().entity_mut(player_entity).insert(GlobalTransform::from(Transform::from_xyz(1.0, 0.0, 0.0)));

        // Run app - player enters, should fire!
        app.update();
        let trigger = app.world().entity(trigger_entity).get::<SectionTrigger>().unwrap();
        assert!(trigger.has_fired);
        assert!(trigger.player_was_inside);
    }
}
