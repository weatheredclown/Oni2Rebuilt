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
