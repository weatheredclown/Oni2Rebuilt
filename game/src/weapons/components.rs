/*
 * weapons/components.rs — Weapon components and supporting types.
 *
 * Weapon (per-wield instance)          — mirrors weapWeapon
 * WeaponTypeData (shared type data)    — mirrors weapWeaponType
 * OperationalMode                      — mirrors weapOperationalMode
 * ChargeState                          — mirrors weapChargeState
 * ProjectileInfo / EffectInfo          — mirrors weapProjectileInfo / weapEffectInfo
 * AmmoType + AmmoRegistry              — mirrors weapAmmoType + weapAmmoTypeManager
 * WeaponRegistry                       — mirrors weapManager's type pool
 * Aimer enum (Simple / Ballistic)      — mirrors weapAimer subclasses
 * AimTarget                            — mirrors weapUpdateParameters (Aim/AimToMiss)
 * hold_types / trig_hold_modes / etc.  — integer enums matching original Oni2
 * weapon_flags                         — WF_IS_FIRING / WF_TRIGGER_PULLED / …
 */
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Enum-like constants (mirror the original Oni2 enums)
// ---------------------------------------------------------------------------

pub const AMMO_NONE: i32 = -1;
pub const MAX_NUM_AMMO_SLOTS: usize = 4;
pub const MAX_FIRING_FX: usize = 8;
pub const MAX_CHARGE_FX: usize = 8;

/// WF_* flags for Weapon.flags.
pub mod weapon_flags {
    pub const IS_FIRING: u32 = 1 << 0;
    pub const TRIGGER_PULLED: u32 = 1 << 1;
    pub const TRIGGER_RELEASED_AFTER_FIRING: u32 = 1 << 2;
    pub const FIRST_FRAME: u32 = 1 << 3;
}

/// WEAPHOLD_* — describes which body-language grip the wielder should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i8)]
pub enum HoldType {
    #[default]
    Invalid = -1,
    Pistol = 0,
    Rifle = 1,
    Pipe = 2,
}

impl HoldType {
    pub fn from_i32(v: i32) -> Self {
        match v {
            0 => HoldType::Pistol,
            1 => HoldType::Rifle,
            2 => HoldType::Pipe,
            _ => HoldType::Invalid,
        }
    }
}

/// TRIGHOLD_* — trigger-hold behaviour for an operational mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrigHoldMode {
    #[default]
    Single,
    Auto,
    Charge,
    FirstAndCharge,
}

/// Aimer variant — simple = straight-line, ballistic = lead target under gravity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Aimer {
    #[default]
    Simple,
    Ballistic,
}

/// Top-level weapon class.  Mirrors the TYPE_CLASS_NAME subclasses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WeaponClass {
    #[default]
    BaseWeapon,
    CloseRange,
    NonRigid,
    Throwing,
    LongRange,
    AnimatedLongRange,
}

// ---------------------------------------------------------------------------
// AmmoType
// ---------------------------------------------------------------------------

/// Shared ammunition-type data.
#[derive(Debug, Clone, Default)]
pub struct AmmoType {
    pub name: String,
    /// Display name / icon key (if any).
    pub display: String,
    /// Damage per round (used by the projectile info if no override).
    pub damage: f32,
    /// Tint / element.
    pub hit_type: i32,
}

/// Ammo registry: name → AmmoType.  Populated at weapon-type load time.
#[derive(Resource, Default)]
pub struct AmmoRegistry {
    pub types: HashMap<String, Arc<AmmoType>>,
}

impl AmmoRegistry {
    pub fn get(&self, name: &str) -> Option<&Arc<AmmoType>> {
        self.types.get(&name.to_lowercase())
    }
}

// ---------------------------------------------------------------------------
// Projectile / Effect launch info (timeline entries)
// ---------------------------------------------------------------------------

/// One projectile scheduled to fire on the op-mode timeline.
#[derive(Debug, Clone, Default)]
pub struct ProjectileInfo {
    /// Name in ProjLibrary — used as the key when emitting SpawnProjectileEvent.
    pub projectile_name: String,
    /// Muzzle-local offset from the weapon's grip matrix.
    pub muzzle_offset: Vec3,
    /// Euler angles (radians) for the muzzle's orientation relative to grip.
    pub muzzle_eulers: Vec3,
    /// Initial linear speed of the projectile (m/s).
    pub speed: f32,
    /// Delay after trigger-pull before this projectile launches.
    pub delay: f32,
    /// Ammo consumed per launch.
    pub ammo_use: f32,
    /// Accuracy-tuning parameters.
    pub acc_target_range: f32,
    pub acc_target_hit_prob: f32,
}

/// One effect scheduled on the op-mode timeline (muzzle flare, smoke, shell…).
#[derive(Debug, Clone, Default)]
pub struct EffectInfo {
    /// Name in the FX library — used as the key when spawning.
    pub fx_name: String,
    /// Offset from the weapon matrix.
    pub offset: Vec3,
    /// Delay after trigger-pull before the effect starts.
    pub delay: f32,
    /// If > 0, controlled duration.
    pub duration: f32,
}

// ---------------------------------------------------------------------------
// Charge state / Operational mode
// ---------------------------------------------------------------------------

/// One "charge level" of an operational mode.  Each charge state carries its
/// own scheduled projectiles + FX.  The first state is unconditional; holding
/// the trigger cycles through later states (see TrigHoldMode::Charge).
#[derive(Debug, Clone, Default)]
pub struct ChargeState {
    /// Minimum time the trigger must be held to reach this state.
    pub min_charge_time: f32,
    /// Range of the audible "gun shot" noise event.
    pub noise_range: f32,
    /// Projectiles to launch when this state fires.
    pub projectiles: Vec<ProjectileInfo>,
    /// Firing effects scheduled on the timeline.
    pub firing_fx: Vec<EffectInfo>,
    /// Effects played while this state is being charged.
    pub charging_fx: Vec<EffectInfo>,
}

/// One operational mode (single, auto, grenade, throw, …).  A weapon type
/// has one or more — the active mode is switched via `SetOpModeMessage`.
#[derive(Debug, Clone, Default)]
pub struct OperationalMode {
    /// Aimer used by this mode.
    pub aimer: Aimer,
    /// Ammo slot (0..MAX_NUM_AMMO_SLOTS).  Which of the wielder's ammo buckets to drain.
    pub ammo_slot: i32,
    /// Name of the ammo type required (looked up in AmmoRegistry).
    pub ammo_type: Option<String>,
    /// Minimum time between shots.
    pub chamber_time: f32,
    /// Additional delay between auto-fire shots, on top of chamber_time.
    pub auto_fire_delay: f32,
    /// Grip type wielders should use for this mode.
    pub hold_type: HoldType,
    /// Trigger-hold behaviour.
    pub trig_hold_mode: TrigHoldMode,
    /// If true, firing this mode throws the weapon itself.
    pub throws_weapon: bool,
    /// Standard shooter range used for accuracy tuning.
    pub shooter_standard_range: f32,
    /// AI tuning.
    pub ai_params: AiParameters,
    /// First charge state (always runs on trigger-pull regardless of hold time).
    pub first_state: ChargeState,
    /// Additional charge states, indexed by ascending `min_charge_time`.
    pub charge_states: Vec<ChargeState>,
}

impl OperationalMode {
    pub fn auto_fire(&self) -> bool {
        matches!(self.trig_hold_mode, TrigHoldMode::Auto)
    }
    pub fn can_charge(&self) -> bool {
        matches!(
            self.trig_hold_mode,
            TrigHoldMode::Charge | TrigHoldMode::FirstAndCharge
        )
    }
    pub fn first_and_charge(&self) -> bool {
        matches!(self.trig_hold_mode, TrigHoldMode::FirstAndCharge)
    }
    /// Pick the charge state appropriate for a given trigger-held duration.
    pub fn find_state(&self, charge_time: f32) -> &ChargeState {
        let mut best = &self.first_state;
        let mut best_t = -1.0_f32;
        for s in &self.charge_states {
            if s.min_charge_time <= charge_time && s.min_charge_time > best_t {
                best = s;
                best_t = s.min_charge_time;
            }
        }
        best
    }
}

// ---------------------------------------------------------------------------
// AI parameters (mirrors weapAIParameters)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AiParameters {
    pub minimum_range: f32,
    pub maximum_range: f32,
    pub desired_range: f32,
    pub burst_duration: f32,
    pub burst_duration_randomness: f32,
    pub min_aim_time: f32,
    pub maximum_damage_range: f32,
    pub damage_cone_angle: f32,
    pub fire_delay: f32,
    pub fire_delay_randomness: f32,
}

impl Default for AiParameters {
    fn default() -> Self {
        AiParameters {
            minimum_range: 4.0,
            maximum_range: 16.0,
            desired_range: 8.0,
            burst_duration: 0.15,
            burst_duration_randomness: 0.1,
            min_aim_time: 0.5,
            maximum_damage_range: 16.0,
            damage_cone_angle: 1.0,
            fire_delay: 1.0,
            fire_delay_randomness: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// WeaponTypeData — shared per-type data
// ---------------------------------------------------------------------------

/// Per-type data.  Shared by many `Weapon` instances via `Arc`.
#[derive(Debug, Clone)]
pub struct WeaponTypeData {
    pub name: String,
    pub class: WeaponClass,
    /// Grip offset from the wielder's hand bone, in Bevy-local space
    /// relative to the bone.  The .weap source stores this in Oni2 space
    /// (left-handed, +Z forward) — the parser
    /// (`oni2_loader::parsers::weap`) converts via
    /// `space::to_bevy_space_pos` so downstream consumers get Bevy
    /// coordinates directly and don't have to remember the handedness.
    pub grip_offset: Vec3,
    /// Grip orientation as a Bevy quaternion (bone→weapon rotation).
    /// The .weap source stores Euler angles (radians, YXZ order in the
    /// Oni2 convention); the parser converts via
    /// `space::to_bevy_space_rot_rad` at parse time.
    pub grip_rot: Quat,
    /// Entity/model name (for rendering).
    pub entity_type: Option<String>,
    /// Operational modes.  The active mode is stored by index on the Weapon.
    pub op_modes: Vec<OperationalMode>,
    /// Number of ammo slots this weapon can hold.
    pub num_ammo_slots: usize,
}

impl Default for WeaponTypeData {
    fn default() -> Self {
        WeaponTypeData {
            name: String::new(),
            class: WeaponClass::BaseWeapon,
            grip_offset: Vec3::ZERO,
            grip_rot: Quat::IDENTITY,
            entity_type: None,
            op_modes: Vec::new(),
            num_ammo_slots: 1,
        }
    }
}

impl WeaponTypeData {
    pub fn get_op_mode(&self, index: usize) -> Option<&OperationalMode> {
        self.op_modes.get(index)
    }
}

/// Weapon-type registry: name → WeaponTypeData.
#[derive(Resource, Default)]
pub struct WeaponRegistry {
    pub types: HashMap<String, Arc<WeaponTypeData>>,
}

impl WeaponRegistry {
    pub fn get(&self, name: &str) -> Option<&Arc<WeaponTypeData>> {
        self.types.get(&name.to_lowercase())
    }
}

// ---------------------------------------------------------------------------
// AmmoSlot / AimTarget / ActiveLaunchCursor
// ---------------------------------------------------------------------------

/// A single ammo bucket on a Weapon instance.
#[derive(Debug, Clone, Default)]
pub struct AmmoSlot {
    /// If None, the slot is empty (AMMO_NONE).
    pub ammo_type: Option<Arc<AmmoType>>,
    /// Remaining amount (rounds).
    pub amount: f32,
}

/// Mirror of weapUpdateParameters — the aim state passed into Aim() each frame.
#[derive(Debug, Clone, Default)]
pub enum AimTarget {
    /// No aim target — weapon fires in `FiringDir`.
    #[default]
    None,
    /// Aim at the given world-space point.
    Point(Vec3),
    /// Aim at an actor entity (optionally its velocity/bone for ballistic lead).
    Actor {
        target: Entity,
        target_velocity: Vec3,
        bone_offset: Vec3,
    },
    /// Deliberately aim to miss (AimToMiss).
    Miss {
        target: Entity,
        dist_away: f32,
        target_velocity: Vec3,
    },
}

/// Cursor into the currently-firing charge state's projectile + effect list.
/// When `IS_FIRING` is set, `weapon_update_system` walks this cursor, launching
/// anything whose `delay <= elapsed` and has not yet been dispatched.
#[derive(Debug, Clone, Default)]
pub struct LaunchCursor {
    /// Time elapsed since the trigger was pulled for this firing cycle.
    pub elapsed: f32,
    /// Indices of projectiles already fired this cycle.
    pub projectiles_fired: Vec<u16>,
    /// Indices of effects already spawned this cycle.
    pub effects_spawned: Vec<u16>,
}

// ---------------------------------------------------------------------------
// Weapon component — the per-wield instance
// ---------------------------------------------------------------------------

/// Per-wield weapon instance.  Insert alongside an owner entity's inventory,
/// or attach to the owner directly for simple "held weapon" cases.
#[derive(Component, Debug, Clone)]
pub struct Weapon {
    /// Shared type data.
    pub ty: Arc<WeaponTypeData>,
    /// Entity that is wielding / will get credit for the shot.
    pub owner: Entity,
    /// Current operational-mode index into `ty.op_modes`.
    pub op_mode: usize,
    /// If `IS_FIRING`, this is the op-mode cached when the trigger was pulled
    /// (so the player can change mode mid-fire without breaking the cycle).
    pub firing_op_mode: Option<usize>,
    /// If `IS_FIRING`, the charge state currently being played back.
    pub firing_charge_state_index: Option<i32>,

    /// WF_* flags.
    pub flags: u32,

    /// Ammo slots (matches ty.num_ammo_slots in length).
    pub ammo_slots: Vec<AmmoSlot>,

    /// Muzzle direction in world space, updated by the aimer each frame.
    pub firing_dir: Vec3,
    /// Aim state — updated by AimMessage.
    pub aim: AimTarget,

    /// Time accumulator used by chamber_time / auto_fire_delay.
    pub fire_time: f32,
    /// Time the trigger was pulled (for charge lookup).
    pub charge_start_time: f32,
    /// Shots fired (ammo burned) total lifetime — for AI / UI.
    pub shots_fired: f32,
    /// Inaccuracy target — 0 = perfect, higher = more spread.
    pub shooter_inaccuracy: f32,
    /// AI charge-state selection override.
    pub ai_charge_state: i32,

    /// Cursor into the active charge-state's timeline.
    pub cursor: LaunchCursor,
}

impl Weapon {
    pub fn new(ty: Arc<WeaponTypeData>, owner: Entity) -> Self {
        let num_slots = ty.num_ammo_slots.max(1);
        Weapon {
            ty,
            owner,
            op_mode: 0,
            firing_op_mode: None,
            firing_charge_state_index: None,
            flags: 0,
            ammo_slots: vec![AmmoSlot::default(); num_slots],
            firing_dir: Vec3::NEG_Z,
            aim: AimTarget::None,
            fire_time: 0.0,
            charge_start_time: 0.0,
            shots_fired: 0.0,
            shooter_inaccuracy: 0.0,
            ai_charge_state: -1,
            cursor: LaunchCursor::default(),
        }
    }

    // --- Flag helpers ---

    pub fn has_flag(&self, f: u32) -> bool {
        (self.flags & f) != 0
    }
    pub fn set_flag(&mut self, f: u32) {
        self.flags |= f;
    }
    pub fn clear_flag(&mut self, f: u32) {
        self.flags &= !f;
    }

    pub fn is_firing(&self) -> bool {
        self.has_flag(weapon_flags::IS_FIRING)
    }
    pub fn is_charging(&self) -> bool {
        self.has_flag(weapon_flags::TRIGGER_PULLED)
            && self.active_mode().is_some_and(|m| m.can_charge())
    }

    // --- Accessors ---

    pub fn active_mode(&self) -> Option<&OperationalMode> {
        self.ty.get_op_mode(self.op_mode)
    }
    pub fn firing_mode(&self) -> Option<&OperationalMode> {
        self.firing_op_mode
            .and_then(|i| self.ty.get_op_mode(i))
            .or_else(|| self.active_mode())
    }
    pub fn hold_type(&self) -> HoldType {
        self.active_mode().map(|m| m.hold_type).unwrap_or_default()
    }

    // --- Ammo ---

    pub fn ammo_count(&self, slot: usize) -> f32 {
        self.ammo_slots.get(slot).map_or(0.0, |s| s.amount)
    }
    pub fn set_ammo(&mut self, slot: usize, ty: Arc<AmmoType>, amount: f32) -> bool {
        let Some(s) = self.ammo_slots.get_mut(slot) else {
            return false;
        };
        s.ammo_type = Some(ty);
        s.amount = amount;
        true
    }
    pub fn add_ammo(&mut self, slot: usize, ty: Arc<AmmoType>, amount: f32) -> bool {
        let Some(s) = self.ammo_slots.get_mut(slot) else {
            return false;
        };
        // Only add to a slot of matching type (or empty).
        match &s.ammo_type {
            Some(t) if t.name == ty.name => s.amount += amount,
            None => {
                s.ammo_type = Some(ty);
                s.amount = amount;
            }
            _ => return false,
        }
        true
    }
    pub fn can_use_ammo(&self, slot: usize, ty: &AmmoType) -> bool {
        self.ammo_slots
            .get(slot)
            .and_then(|s| s.ammo_type.as_ref())
            .is_some_and(|t| t.name == ty.name)
    }
}
