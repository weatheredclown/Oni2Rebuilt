/*
 * weapons/mod.rs — WeaponPlugin.
 *
 * Registers the Weapon component subsystem:
 *   • Resources: WeaponRegistry (WeaponTypeData by name) and AmmoRegistry
 *     (AmmoType by name).
 *   • Messages: Fire/Stop/SetOpMode/Aim/Reload/AddAmmo/SetShooterAccuracy
 *     (inbound) and WeaponFired/WeaponChargeChanged (outbound).
 *   • Systems (FixedUpdate, gated on AppState::InGame):
 *       inbound handlers → weapon_update_system → outbound writers
 *
 * The weapon update emits SpawnProjectileEvent into the pre-existing
 * projectile_system and SpawnFx into fx_system.
 */
pub mod components;
pub mod events;
pub mod systems;

use bevy::prelude::*;

use crate::menu::AppState;

pub use components::{AimTarget, AmmoRegistry, Weapon, WeaponRegistry};
pub use events::{
    AddAmmoMessage, AimMessage, FireWeaponMessage, ReloadMessage, SetOpModeMessage,
    SetShooterAccuracyMessage, StopFiringMessage, WeaponChargeChangedMessage, WeaponFiredMessage,
};

pub struct WeaponPlugin;

impl Plugin for WeaponPlugin {
    fn build(&self, app: &mut App) {
        app
            // --- Resources ---
            .init_resource::<WeaponRegistry>()
            .init_resource::<AmmoRegistry>()
            // --- Messages ---
            .add_message::<FireWeaponMessage>()
            .add_message::<StopFiringMessage>()
            .add_message::<SetOpModeMessage>()
            .add_message::<AimMessage>()
            .add_message::<ReloadMessage>()
            .add_message::<AddAmmoMessage>()
            .add_message::<SetShooterAccuracyMessage>()
            .add_message::<WeaponFiredMessage>()
            .add_message::<WeaponChargeChangedMessage>()
            // --- Systems ---
            .add_systems(
                FixedUpdate,
                (
                    systems::fire_weapon_system,
                    systems::stop_firing_system,
                    systems::set_op_mode_system,
                    systems::aim_system,
                    systems::ammo_system,
                    systems::accuracy_system,
                    // Body-turn component of aim IK — rotates wielder's
                    // facing toward the aim target so firing at wide
                    // angles doesn't shoot past the character's nose.
                    // Must run BEFORE weapon_update_system so the updated
                    // facing is reflected in this tick's firing_dir.
                    systems::weapon_aim_body_turn_system.after(systems::aim_system),
                    systems::weapon_update_system
                        .after(systems::fire_weapon_system)
                        .after(systems::stop_firing_system)
                        .after(systems::aim_system)
                        .after(systems::weapon_aim_body_turn_system),
                )
                    .run_if(in_state(AppState::InGame)),
            )
            // weapon_attachment_system parents each weapon entity to
            // its wielder's grip bone with a fixed local Transform.
            // Bevy's `TransformSystems::Propagate` (in PostUpdate)
            // then computes the weapon's GlobalTransform AND its mesh
            // child's GlobalTransform from the already-up-to-date bone
            // hierarchy in a single pass — eliminating the 1-frame
            // lag and jitter the previous "compute world pose by hand"
            // design caused.  Running in Update is fine since we no
            // longer read any GlobalTransforms.
            .add_systems(
                Update,
                systems::weapon_attachment_system
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
