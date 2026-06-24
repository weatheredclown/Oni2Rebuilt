/*
 * combat/components.rs — combat component types.
 *
 * Fighter (facing, grounded, jumps), FighterId (UUID), Health, AttackState,
 * AttackClass / AttackStrength enums, ComboTracker, HitReaction / ActiveReaction,
 * AboutToBeHit, ReactionKind, DestroyOnDeath, FistVisual, CombatMaterials resource.
 */
use bevy::prelude::*;
use uuid::Uuid;

// === Core Fighter Components ===

#[derive(Component)]
pub struct Fighter {
    pub facing: Vec3,
    pub throttle: f32, // Generic locomotion speed requested (0.0 to 1.0+)
    pub material_stood_on: Option<String>,
}

impl Default for Fighter {
    fn default() -> Self {
        Self {
            facing: Vec3::NEG_Z,
            throttle: 0.0,
            material_stood_on: None,
        }
    }
}

#[derive(Component)]
pub struct FighterId(pub Uuid);

#[derive(Component)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    pub fn new(max: f32) -> Self {
        Self { current: max, max }
    }

    pub fn fraction(&self) -> f32 {
        (self.current / self.max).clamp(0.0, 1.0)
    }
}

#[derive(Component)]
pub struct DestroyOnDeath(pub f32);

/// Marker for any actor that declared a `<Target>` block in its XML.
/// Mirrors legacy `tarTarget`: tags the entity as a valid auto-aim
/// / lock-on candidate.  Today its only consumer is the script
/// `Stmt::Shoot` opcode, which refuses to fire on entities that
/// aren't flagged shootable.
#[derive(Component)]
pub struct Targetable;

/// Detailed target properties parsed from `<Target>` XML.
/// Matches C++ `tgtTargetComponent` and `tgtTargetComponentType`.
#[derive(Component, Debug, Clone)]
pub struct TargetComponent {
    pub magnet_radius: f32,
    pub magnet_strength: f32,
    pub target_offset: Vec3,
    pub is_bump_targetable: bool,
    pub parent_bone: Option<String>,
    pub bone_index: Option<usize>,
}

/// Wielder-side reticle settings parsed from `<Reticle>` XML.
/// Matches C++ `tgtReticleComponentType` and `tgtReticleComponent`.
#[derive(Component, Debug, Clone)]
pub struct ReticleComponent {
    pub bump_targeting_enabled: bool,
    pub manual_targeting_enabled: bool,
    pub max_lock_on_distance: f32,
    pub movement_rate: f32,
    pub reticle_size: Vec2,
    pub bump_targeting_max_time: f32,
    pub bump_max_delta_angle: f32,
    pub bump_world_dist_factor: f32,
    pub bump_angle_delta_factor: f32,
    pub bump_screen_dist_factor: f32,
    pub min_angle_x: f32,
    pub max_angle_x: f32,
    pub max_angle_y: f32,
    pub max_angular_velocity: f32,
    pub reticle_neutralization_rate: f32,
    pub bump_magnitude: f32,
    pub idle_color: Vec3,
    pub ally_color: Vec3,
    pub neutral_color: Vec3,
    pub enemy_color: Vec3,
    pub invincible_color: Vec3,
    pub idle_transparency: f32,
    pub starting_lock_on_transparency: f32,
    pub ending_lock_on_transparency: f32,
    pub lock_on_transparency_lerp_rate: f32,
    pub starting_blur_transparency: f32,
    pub ending_blur_transparency: f32,
    pub blur_transparency_lerp_rate: f32,
    pub blur_frame_extrapolation: f32,
    pub reticle_a_starting_scale: Vec3,
    pub reticle_a_ending_scale: Vec3,
    pub reticle_a_starting_distance: f32,
    pub reticle_a_ending_distance: f32,
    pub reticle_a_starting_rotation_rate: f32,
    pub reticle_a_ending_rotation_rate: f32,
    pub reticle_a_lerp_rate_distance: f32,
    pub reticle_a_lerp_rate_scale: f32,
    pub reticle_a_lerp_rate_rotation: f32,
    pub reticle_b_starting_scale: Vec3,
    pub reticle_b_ending_scale: Vec3,
    pub reticle_b_starting_distance: f32,
    pub reticle_b_ending_distance: f32,
    pub reticle_b_starting_rotation_rate: f32,
    pub reticle_b_ending_rotation_rate: f32,
    pub reticle_b_lerp_rate_distance: f32,
    pub reticle_b_lerp_rate_scale: f32,
    pub reticle_b_lerp_rate_rotation: f32,
    pub reticle_a_entity: Option<String>,
    pub reticle_b_entity: Option<String>,

    // --- Runtime State ---
    pub is_active: bool,
    pub target_point: Vec3,
    pub target_point_screen: Vec2,
    pub target_locked_on: Option<Entity>,
    pub target_locked_on_last: Option<Entity>,
    pub reticle_input_x: f32,
    pub reticle_input_y: f32,
    pub lock_on_enabled: bool,
    
    // Bump targeting state
    pub bump_is_ready_to_use: bool,
    pub time_since_reticle_input_zero: f32,
    pub bump_angle: f32,
    
    // Z / Lerping / Visuals state (AGZ updates)
    pub muzzle_position: Vec3,
    pub current_position_a: Vec3,
    pub current_position_b: Vec3,
    pub current_scale_a: Vec3,
    pub current_scale_b: Vec3,
    pub current_distance_a: f32,
    pub current_distance_b: f32,
    pub current_rotation_rate_a: f32,
    pub current_rotation_rate_b: f32,
    pub current_rotation_a: f32,
    pub current_rotation_b: f32,
    pub reached_destination_a: bool,
    pub reached_destination_b: bool,
    pub new_lock_on: bool,
    pub activation_frame: bool,
    pub current_lock_on_transparency: f32,
    pub current_blur_transparency: f32,
    pub current_extrapolation_a: f32,
    pub current_lock_on_distance: f32,
    
    pub current_azimuth: f32,
    pub current_incline: f32,
}

impl Default for ReticleComponent {
    fn default() -> Self {
        Self {
            bump_targeting_enabled: true,
            manual_targeting_enabled: true,
            max_lock_on_distance: 30.0,
            movement_rate: 1800.0,
            reticle_size: Vec2::new(32.0, 32.0),
            bump_targeting_max_time: 0.2,
            bump_max_delta_angle: 1.57,
            bump_world_dist_factor: 0.08,
            bump_angle_delta_factor: 1.0,
            bump_screen_dist_factor: 0.2,
            min_angle_x: -0.5,
            max_angle_x: 0.5,
            max_angle_y: 3.15,
            max_angular_velocity: 4.0,
            reticle_neutralization_rate: 1.2,
            bump_magnitude: 0.98,
            idle_color: Vec3::new(1.0, 1.0, 0.25),
            ally_color: Vec3::new(0.25, 1.0, 0.25),
            neutral_color: Vec3::new(0.5, 0.5, 0.5),
            enemy_color: Vec3::new(1.0, 0.25, 0.25),
            invincible_color: Vec3::new(1.0, 0.0, 1.0),
            idle_transparency: 0.5,
            starting_lock_on_transparency: 1.0,
            ending_lock_on_transparency: 1.0,
            lock_on_transparency_lerp_rate: 5.0,
            starting_blur_transparency: 0.5,
            ending_blur_transparency: 0.0,
            blur_transparency_lerp_rate: 10.0,
            blur_frame_extrapolation: 2.0,
            reticle_a_starting_scale: Vec3::ONE,
            reticle_a_ending_scale: Vec3::ONE,
            reticle_a_starting_distance: 1.0,
            reticle_a_ending_distance: 1.0,
            reticle_a_starting_rotation_rate: 0.0,
            reticle_a_ending_rotation_rate: 0.0,
            reticle_a_lerp_rate_distance: 5.0,
            reticle_a_lerp_rate_scale: 5.0,
            reticle_a_lerp_rate_rotation: 5.0,
            reticle_b_starting_scale: Vec3::ONE,
            reticle_b_ending_scale: Vec3::ONE,
            reticle_b_starting_distance: 1.0,
            reticle_b_ending_distance: 1.0,
            reticle_b_starting_rotation_rate: 0.0,
            reticle_b_ending_rotation_rate: 0.0,
            reticle_b_lerp_rate_distance: 5.0,
            reticle_b_lerp_rate_scale: 5.0,
            reticle_b_lerp_rate_rotation: 5.0,
            reticle_a_entity: None,
            reticle_b_entity: None,

            is_active: false,
            target_point: Vec3::ZERO,
            target_point_screen: Vec2::ZERO,
            target_locked_on: None,
            target_locked_on_last: None,
            reticle_input_x: 0.0,
            reticle_input_y: 0.0,
            lock_on_enabled: true,
            bump_is_ready_to_use: true,
            time_since_reticle_input_zero: 0.0,
            bump_angle: 0.0,
            muzzle_position: Vec3::ZERO,
            current_position_a: Vec3::ZERO,
            current_position_b: Vec3::ZERO,
            current_scale_a: Vec3::ONE,
            current_scale_b: Vec3::ONE,
            current_distance_a: 0.0,
            current_distance_b: 0.0,
            current_rotation_rate_a: 0.0,
            current_rotation_rate_b: 0.0,
            current_rotation_a: 0.0,
            current_rotation_b: 0.0,
            reached_destination_a: true,
            reached_destination_b: true,
            new_lock_on: true,
            activation_frame: false,
            current_lock_on_transparency: 0.0,
            current_blur_transparency: 0.0,
            current_extrapolation_a: 0.0,
            current_lock_on_distance: 10.0,
            current_azimuth: 0.0,
            current_incline: 0.0,
        }
    }
}

#[derive(Component)]
pub struct DeathSequenceTimer(pub Timer);

// === Attack Enums ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackClass {
    Punch,
    Kick,
    Grab,
    RangedShot,
}

impl AttackClass {
    pub fn name(&self) -> &'static str {
        match self {
            AttackClass::Punch => "Punch",
            AttackClass::Kick => "Kick",
            AttackClass::Grab => "Grab",
            AttackClass::RangedShot => "RangedShot",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackStrength {
    Low,
    High,
    Super,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackTarget {
    Head,
    Body,
    Legs,
}

// === Phase-Based Attack System (rb's 0.0-1.0 system) ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackPhase {
    Startup,
    Active,
    Recovery,
    Done,
}

#[derive(Component, Default)]
pub struct AttackState {
    pub active_attack: Option<ActiveAttack>,
}

#[derive(Default)]
pub struct ActiveAttack {
    pub hit_entities: Vec<Entity>,
    pub has_fired_projectile: bool,
    pub end_rotation_notches: i32,
    /// One-shot guard for grab-attack disarms: set the first frame the
    /// anim crosses `AtdtGrab::removes_weapon_phase` so we don't fire
    /// `DropWeaponMessage` every frame the phase remains exceeded.
    /// Mirrors the `PassedPhase` edge-trigger on `crGrab`.
    pub has_disarmed: bool,
    /// One-shot guard for grapple attack damage: set when the damage is applied
    /// at grap_atk.damage_phase.
    pub has_damaged: bool,
}

// === Combo Tracker ===

#[derive(Component)]
pub struct ComboTracker {
    pub hit_count: u32,
    pub last_hit_time: f64,
    pub combo_window: f64,
}

impl Default for ComboTracker {
    fn default() -> Self {
        Self {
            hit_count: 0,
            last_hit_time: 0.0,
            combo_window: 1.5,
        }
    }
}

// === About-to-be-Hit Warning (from rb's SetAboutToBeHit) ===

#[derive(Component, Default)]
pub struct AboutToBeHit {
    pub active: Option<AboutToBeHitData>,
}

pub struct AboutToBeHitData {
    pub eta: f32,
    pub hit_type: u8,
    pub from: Vec3,
    pub attacker: Entity,
}

// === Hit Reaction ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionKind {
    Flinch,
    Knockback,
    Knockdown,
}

#[derive(Component, Default)]
pub struct HitReaction {
    pub active: Option<ActiveReaction>,
}

pub struct ActiveReaction {
    pub kind: ReactionKind,
    pub duration: f32,
    pub elapsed: f32,
    pub direction: Vec3,
    /// animReactEnum integer used to look up the animation alias.
    pub react_enum: i32,
    /// AnimId hash of the react animation that was started (0 = none played).
    /// Used to detect when the animation finishes so we can clear the reaction.
    pub react_anim_id: u64,
}

impl ActiveReaction {
    pub fn new(kind: ReactionKind, direction: Vec3, react_enum: i32) -> Self {
        let duration = match kind {
            ReactionKind::Flinch => 0.5,
            ReactionKind::Knockback => 0.8,
            ReactionKind::Knockdown => 1.5,
        };
        Self {
            kind,
            duration,
            elapsed: 0.0,
            direction,
            react_enum,
            react_anim_id: 0,
        }
    }
}

// === Visual Marker Components ===

#[derive(Component)]
pub struct FistVisual;

// === Combat Materials Resource ===

#[derive(Resource)]
pub struct CombatMaterials {
    pub fist_startup: Handle<StandardMaterial>,
    pub fist_active: Handle<StandardMaterial>,
    pub fist_recovery: Handle<StandardMaterial>,
    pub fist_mesh: Handle<Mesh>,
}

// === React library (from entity's ANIMREACT_*.rct files) ===

/// All react animations for an entity, indexed by animReactEnum integer.
/// Loaded at spawn time from entity.tune/<name>/ANIMREACT_*.rct files.
/// Indexed reaction animation library for an entity.
#[derive(Component, Default, Clone)]
pub struct ReactLibrary {
    /// Indexed by animReactEnum value (0 = ANIMREACT_REGULAR, etc.).
    pub entries: Vec<Option<crate::oni2_loader::parsers::rct::ReactData>>,
}

impl ReactLibrary {
    /// Look up react data by animReactEnum integer from a .atdt reactanim field.
    pub fn get(&self, react_enum: i32) -> Option<&crate::oni2_loader::parsers::rct::ReactData> {
        if react_enum < 0 {
            return None;
        }
        self.entries.get(react_enum as usize)?.as_ref()
    }

    /// Pick the death animation enum given the killing-blow's `react_enum`.
    /// Mirrors `crReactData::GetCanBeDieAnimation`: if the killing react has
    /// `CanBeDieAnimation = true`, reuse that react as the death anim;
    /// otherwise fall back to `ANIMDIE_GENERAL` (index 56 in the unified
    /// react/die enum table).
    pub fn pick_die_enum(&self, killing_react_enum: i32) -> i32 {
        if killing_react_enum >= 0
            && let Some(rd) = self.get(killing_react_enum)
            && rd.can_be_die_animation
        {
            return killing_react_enum;
        }
        crate::oni2_loader::parsers::rct::ANIMDIE_GENERAL_INDEX
    }
}

// === Data-driven attack sequence (from entity's .attacks file) ===

/// Ordered list of ANIMATTACK_* alias names for DoTriggerAtk combo cycling.
/// Loaded at spawn time from the entity's .attacks file; the sequence is the
/// standing forward combo chain (filenames containing `_comb_fwd_`), in file order.
/// Ordered forward combo chain for DoTriggerAtk cycling.
#[derive(Component, Default, Clone)]
pub struct AttackData {
    pub forward_combo: Vec<String>,
}

// === Enemy Marker ===

#[derive(Component)]
pub struct Enemy;
