use bevy::prelude::*;
use uuid::Uuid;

// === Core Fighter Components ===

#[derive(Component)]
pub struct Fighter {
    pub facing: Vec3,
    pub is_grounded: bool,
    pub jumps_remaining: u8,
    pub max_jumps: u8,
}

impl Default for Fighter {
    fn default() -> Self {
        Self {
            facing: Vec3::NEG_Z,
            is_grounded: true,
            jumps_remaining: 2,
            max_jumps: 2,
        }
    }
}

#[derive(Component)]
pub struct FighterId(pub Uuid);

#[derive(Component)]
pub struct Health {
    pub current: f32,
    pub max: f32,
    pub invulnerable_until: f64,
}

impl Health {
    pub fn new(max: f32) -> Self {
        Self {
            current: max,
            max,
            invulnerable_until: 0.0,
        }
    }

    pub fn fraction(&self) -> f32 {
        (self.current / self.max).clamp(0.0, 1.0)
    }
}

#[derive(Component)]
pub struct DestroyOnDeath(pub f32);

#[derive(Component)]
pub struct DeathSequenceTimer(pub Timer);

// === Attack Enums (from rb's crAttackData) ===

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
}



// === Enhanced Block State (from rb's crBlockData) ===

#[derive(Component)]
pub struct BlockState {
    pub is_blocking: bool,
    pub heading_offset: f32,
    pub width_radians: f32,
    pub blockable_hit_types: u32,
    pub auto_counter: bool,
    pub damage_multiplier: f32,
    pub combo_count_before_react: u32,
    pub hits_absorbed: u32,
}

impl Default for BlockState {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockState {
    pub fn new() -> Self {
        Self {
            is_blocking: false,
            heading_offset: 0.0,
            width_radians: std::f32::consts::FRAC_PI_2,
            blockable_hit_types: 0b0010_0011_1111, // all punch, kick, and ranged types
            auto_counter: false,
            damage_multiplier: 0.25,
            combo_count_before_react: 5,
            hits_absorbed: 0,
        }
    }

    pub fn can_block_hit_type(&self, hit_type: u8) -> bool {
        self.blockable_hit_types & (1 << hit_type) != 0
    }
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

// === Super Meter (from rb's SuperPowerUp/Dn) ===

#[derive(Component)]
pub struct SuperMeter {
    pub current: f32,
    pub max: f32,
}

impl Default for SuperMeter {
    fn default() -> Self {
        Self {
            current: 0.0,
            max: 100.0,
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

// === Grab/Grapple State (from rb's crGrab) ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabPhase {
    Reaching,
    Holding,
    Throwing,
    Released,
}

#[derive(Component)]
pub struct GrabState {
    pub phase: Option<GrabPhase>,
    pub target: Option<Entity>,
    pub grab_range: f32,
    pub hold_timer: f32,
    pub shake_amount: f32,
}

impl Default for GrabState {
    fn default() -> Self {
        Self {
            phase: None,
            target: None,
            grab_range: 2.0,
            hold_timer: 0.0,
            shake_amount: 0.0,
        }
    }
}

// === Hit Reaction ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionKind {
    Flinch,
    Knockback,
    Knockdown,
    GuardBreak,
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
            ReactionKind::GuardBreak => 0.7,
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

#[derive(Component)]
pub struct ShieldVisual;

// === Combat Materials Resource ===

#[derive(Resource)]
pub struct CombatMaterials {
    pub fist_startup: Handle<StandardMaterial>,
    pub fist_active: Handle<StandardMaterial>,
    pub fist_recovery: Handle<StandardMaterial>,
    pub shield: Handle<StandardMaterial>,
    pub fist_mesh: Handle<Mesh>,
    pub shield_mesh: Handle<Mesh>,
}

// === React library (from entity's ANIMREACT_*.rct files) ===

/// All react animations for an entity, indexed by animReactEnum integer.
/// Loaded at spawn time from entity.tune/<name>/ANIMREACT_*.rct files.
/// Mirrors C++ ftFighterData::ReactData[NUMOF_ANIMREACT_ENUMS].
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
}

// === Data-driven attack sequence (from entity's .attacks file) ===

/// Ordered list of ANIMATTACK_* alias names for DoTriggerAtk combo cycling.
/// Loaded at spawn time from the entity's .attacks file; the sequence is the
/// standing forward combo chain (filenames containing `_comb_fwd_`), in file order.
/// Mirrors the C++ crAttackData / ftFighterComponent attack sequence.
#[derive(Component, Default, Clone)]
pub struct AttackData {
    pub forward_combo: Vec<String>,
}

// === Enemy Marker ===

#[derive(Component)]
pub struct Enemy;
