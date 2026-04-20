/*
 * weapons/events.rs — Message types for the weapon subsystem.
 *
 * FireWeaponMessage        — pull trigger on an entity's weapon
 * StopFiringMessage        — release trigger (auto-fire / charge cutoff)
 * SetOpModeMessage         — switch the active operational mode
 * AimMessage               — update aim target (point / actor / actor-with-miss)
 * ReloadMessage            — add a full magazine to a slot
 * AddAmmoMessage           — add a specific amount to a slot
 * SetShooterAccuracyMessage — adjust inaccuracy (AI spread)
 *
 * WeaponFiredMessage       — OUTBOUND: emitted by weapon_update_system every
 *                            time a projectile/effect launches (for HUD, SFX,
 *                            telemetry, animation sync).
 * WeaponChargeChangedMessage — OUTBOUND: emitted whenever the active charge
 *                            state changes (for charging FX / UI meter).
 */
use bevy::prelude::*;

use super::components::AimTarget;

// ---------------------------------------------------------------------------
// Inbound messages
// ---------------------------------------------------------------------------

#[derive(Message, Clone)]
pub struct FireWeaponMessage {
    pub entity: Entity,
    /// If true, the aim direction is ignored and each projectile uses its
    /// own muzzle orientation (e.g. shotgun spread, multi-shot salvo).
    pub multi_dir: bool,
}

#[derive(Message, Clone)]
pub struct StopFiringMessage {
    pub entity: Entity,
}

#[derive(Message, Clone)]
pub struct SetOpModeMessage {
    pub entity: Entity,
    pub op_mode: usize,
}

#[derive(Message, Clone)]
pub struct AimMessage {
    pub entity: Entity,
    pub target: AimTarget,
}

#[derive(Message, Clone)]
pub struct ReloadMessage {
    pub entity: Entity,
    pub slot: usize,
    pub ammo_type_name: String,
    pub amount: f32,
}

#[derive(Message, Clone)]
pub struct AddAmmoMessage {
    pub entity: Entity,
    pub slot: usize,
    pub ammo_type_name: String,
    pub amount: f32,
}

#[derive(Message, Clone)]
pub struct SetShooterAccuracyMessage {
    pub entity: Entity,
    /// Probability of hitting a target at `shooter_standard_range`, clamped [0,1].
    pub hit_probability: f32,
}

// ---------------------------------------------------------------------------
// Outbound messages
// ---------------------------------------------------------------------------

#[derive(Message, Clone)]
pub struct WeaponFiredMessage {
    pub entity: Entity,
    pub owner: Entity,
    /// Name of the projectile type just launched (or empty for "effect only").
    pub projectile_name: String,
    pub muzzle_position: Vec3,
    pub muzzle_direction: Vec3,
    pub ammo_remaining: f32,
}

#[derive(Message, Clone)]
pub struct WeaponChargeChangedMessage {
    pub entity: Entity,
    pub charge_state_index: i32,
    pub charge_time: f32,
}
