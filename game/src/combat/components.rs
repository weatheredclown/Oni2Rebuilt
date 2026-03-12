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
    pub cooldown_until: f64,
}

pub struct ActiveAttack {
    pub class: AttackClass,
    pub strength: AttackStrength,
    pub target: AttackTarget,
    pub attack_start_phase: f32,
    pub damage_end_phase: f32,
    pub total_duration: f32,
    pub elapsed: f32,
    pub damage: f32,
    pub hit_entities: Vec<Entity>,
    pub super_power_up: f32,
    pub hit_type: u8,
    pub direction_offset: f32,
}

impl ActiveAttack {
    pub fn new(class: AttackClass, strength: AttackStrength, target: AttackTarget) -> Self {
        let (total_duration, attack_start_phase, damage_end_phase, damage, super_power_up) =
            match (class, strength) {
                (AttackClass::Punch, AttackStrength::Low) => (0.5, 0.25, 0.5, 10.0, 5.0),
                (AttackClass::Punch, AttackStrength::High) => (0.8, 0.3, 0.55, 25.0, 10.0),
                (AttackClass::Kick, AttackStrength::Low) => (0.6, 0.25, 0.5, 12.0, 6.0),
                (AttackClass::Kick, AttackStrength::High) => (0.9, 0.3, 0.55, 30.0, 12.0),
                (_, AttackStrength::Super) => (1.2, 0.2, 0.6, 50.0, 0.0),
                (AttackClass::Grab, _) => (0.4, 0.2, 0.5, 0.0, 0.0),
                (AttackClass::RangedShot, _) => (0.3, 0.1, 0.4, 8.0, 3.0),
            };

        let hit_type = Self::compute_hit_type(class, target);

        Self {
            class,
            strength,
            target,
            attack_start_phase,
            damage_end_phase,
            total_duration,
            elapsed: 0.0,
            damage,
            hit_entities: Vec::new(),
            super_power_up,
            hit_type,
            direction_offset: 0.0,
        }
    }

    /// Creates an attack modified by scalar combat modifiers.
    pub fn new_with_modifiers(
        class: AttackClass,
        strength: AttackStrength,
        target: AttackTarget,
        damage_multiplier: f32,
        speed_multiplier: f32,
        direction_offset: f32,
    ) -> Self {
        let mut attack = Self::new(class, strength, target);
        attack.damage *= damage_multiplier;
        attack.total_duration *= speed_multiplier;
        attack.direction_offset = direction_offset;
        attack
    }

    pub fn phase(&self) -> AttackPhase {
        let p = self.phase_f32();
        if p >= 1.0 {
            AttackPhase::Done
        } else if p >= self.damage_end_phase {
            AttackPhase::Recovery
        } else if p >= self.attack_start_phase {
            AttackPhase::Active
        } else {
            AttackPhase::Startup
        }
    }

    pub fn phase_f32(&self) -> f32 {
        if self.total_duration <= 0.0 {
            1.0
        } else {
            self.elapsed / self.total_duration
        }
    }

    fn compute_hit_type(class: AttackClass, target: AttackTarget) -> u8 {
        let class_offset: u8 = match class {
            AttackClass::Punch => 0,
            AttackClass::Kick => 3,
            AttackClass::Grab => 6,
            AttackClass::RangedShot => return 9,
        };
        let target_offset: u8 = match target {
            AttackTarget::Head => 0,
            AttackTarget::Body => 1,
            AttackTarget::Legs => 2,
        };
        class_offset + target_offset
    }
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
}

impl ActiveReaction {
    pub fn new(kind: ReactionKind, direction: Vec3) -> Self {
        let duration = match kind {
            ReactionKind::Flinch => 0.2,
            ReactionKind::Knockback => 0.4,
            ReactionKind::Knockdown => 0.8,
            ReactionKind::GuardBreak => 0.5,
        };
        Self {
            kind,
            duration,
            elapsed: 0.0,
            direction,
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

// === Enemy Marker ===

#[derive(Component)]
pub struct Enemy;
