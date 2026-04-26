/*
 * oni2_loader/asleep.rs — actor dormancy lifecycle.
 *
 * Asleep actors (layout `updatestate="Asleep"` or scroni
 * `setupdatestate Asleep`) stop their scroni script, animator, AI
 * runtimes, and combat systems (those systems `Without<ActorAsleep>`-
 * gate their own queries).  But Avian still integrates RigidBody::Dynamic
 * bodies under gravity, so a physics actor would sink through the floor
 * or drift under residual velocity even while "asleep" from a gameplay
 * POV.  This module bridges that gap:
 *
 *   • On `ActorAsleep` insert: push a `"sleep"` layer onto
 *     `GravityModifiers` with factor 0.0 (suppresses gravity integration
 *     via the existing `gravity_sync_system`), and zero `LinearVelocity`.
 *   • While asleep (every tick): pin `LinearVelocity` to zero so any
 *     stray impulse applied elsewhere doesn't budge the actor.
 *   • On `ActorAsleep` removal: pop the `"sleep"` layer so gravity
 *     resumes.  Velocity naturally starts at zero; if the actor was
 *     airborne when put to sleep, gravity will accelerate it downward
 *     from rest on wake.
 *
 * This pairs with the `Without<ActorAsleep>` filters already on
 * animation / AI systems — between the two, an asleep actor is
 * completely quiescent.
 */
use avian3d::prelude::*;
use bevy::prelude::*;

use crate::animator::gravity::GravityModifiers;
use crate::oni2_loader::components::ActorAsleep;

/// Key used to identify our gravity-suppression layer so we can remove
/// it cleanly on wake.  Distinct from other keys (`"jump"`, etc.) so
/// multiple gravity modifiers can stack without interfering.
const SLEEP_LAYER_KEY: &str = "sleep";

pub struct AsleepPlugin;

impl Plugin for AsleepPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (
                asleep_on_insert_system,
                asleep_pin_velocity_system,
                asleep_on_remove_system,
            )
                .run_if(in_state(crate::menu::AppState::InGame)),
        );
    }
}

/// Runs once per tick, catching any entity that gained `ActorAsleep`
/// since the last tick — pushes the gravity-zero layer and zeros
/// velocity as a one-shot on-enter.
fn asleep_on_insert_system(
    mut query: Query<
        (Option<&mut GravityModifiers>, Option<&mut LinearVelocity>),
        Added<ActorAsleep>,
    >,
) {
    for (gravity_opt, velocity_opt) in &mut query {
        if let Some(mut g) = gravity_opt {
            g.0.push(SLEEP_LAYER_KEY, 0.0);
        }
        if let Some(mut v) = velocity_opt {
            v.0 = Vec3::ZERO;
        }
    }
}

/// Runs every tick for asleep actors — pins LinearVelocity to zero so
/// any impulse applied elsewhere (e.g. a broadcast explosion that fires
/// before the target's sleep-gate is checked) doesn't move the body.
fn asleep_pin_velocity_system(
    mut query: Query<&mut LinearVelocity, With<ActorAsleep>>,
) {
    for mut v in &mut query {
        if v.0 != Vec3::ZERO {
            v.0 = Vec3::ZERO;
        }
    }
}

/// Runs once per tick, catching any entity whose `ActorAsleep` component
/// was just removed — pops the `"sleep"` gravity layer so normal
/// gravity integration resumes.  Uses `RemovedComponents` to observe
/// removals without racing against the component destructor.
fn asleep_on_remove_system(
    mut removed: RemovedComponents<ActorAsleep>,
    mut gravity_query: Query<&mut GravityModifiers>,
) {
    for entity in removed.read() {
        if let Ok(mut g) = gravity_query.get_mut(entity) {
            let _ = g.0.remove(SLEEP_LAYER_KEY);
        }
    }
}
