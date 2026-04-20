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

pub use components::{
    AMMO_NONE, AimTarget, Aimer, AmmoRegistry, AmmoSlot, AmmoType, ChargeState, EffectInfo,
    HoldType, LaunchCursor, MAX_CHARGE_FX, MAX_FIRING_FX, MAX_NUM_AMMO_SLOTS, OperationalMode,
    ProjectileInfo, TrigHoldMode, Weapon, WeaponClass, WeaponRegistry, WeaponTypeData,
    weapon_flags,
};
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
                    systems::weapon_update_system
                        .after(systems::fire_weapon_system)
                        .after(systems::stop_firing_system)
                        .after(systems::aim_system),
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                systems::weapon_attachment_system
                    .after(crate::oni2_loader::animation::update_oni2_animation)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
