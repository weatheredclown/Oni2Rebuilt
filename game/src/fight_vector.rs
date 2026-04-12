/// Fight vector trigger system — matches ONI2's trigFightVectorComponent.
///
/// Designers place FightVectorTrigger entities in the world. When a fighter steps
/// inside a trigger's radius, DoTriggerAtk queries the best one and uses its
/// recommended attack. Two modes (mirrors C++):
///
///   Directional — trigger has a normalized direction (offset); scored by flat
///                 projection of (target_pt - fighter_pos) onto that direction.
///                 Best = highest projection (fighter is most "behind" the vector).
///
///   Positional  — trigger has a target point offset; scored by flat distance.
///                 Best = closest target point.

use bevy::prelude::*;

pub struct FightVectorPlugin;

impl Plugin for FightVectorPlugin {
    fn build(&self, _app: &mut App) {
        // TODO: load FightVectorTrigger entities from level layout files.
        // Until then, DoTriggerAtk falls back to AttackData.forward_combo (standing combo chain).
        // Triggers are passive query targets — no per-frame systems needed here.
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// A level-placed fight vector trigger.  Spawn as a regular Bevy entity with a
/// Transform; the system queries GlobalTransform at runtime.
#[derive(Component)]
pub struct FightVectorTrigger {
    /// Sphere radius (flat / XZ) in which the trigger is active.
    pub radius: f32,

    /// true  → directional mode: `offset` is a world-space direction vector.
    ///          The trigger "points" fighters toward the attack.
    /// false → positional mode: `offset` is added to the trigger's world
    ///          position to give the target point fighters attack toward.
    pub directional: bool,

    /// In directional mode: the direction vector (need not be normalised — it
    ///   is normalised at query time).
    /// In positional mode: offset from the trigger entity's origin to the
    ///   target point the fighter should face.
    pub offset: Vec3,

    /// The ANIMATTACK_* alias to play when this trigger wins the selection.
    pub attack_alias: String,

    pub enabled: bool,
}

impl FightVectorTrigger {
    /// Returns true when `fighter_pos` is inside the flat (XZ) sphere.
    fn contains(&self, trigger_origin: Vec3, fighter_pos: Vec3) -> bool {
        let dx = fighter_pos.x - trigger_origin.x;
        let dz = fighter_pos.z - trigger_origin.z;
        dx * dx + dz * dz <= self.radius * self.radius
    }

    /// Relevance score (lower = more relevant).  Mirrors C++ r2 selection.
    fn score(&self, trigger_origin: Vec3, fighter_pos: Vec3) -> f32 {
        if self.directional {
            let dir = Vec3::new(self.offset.x, 0.0, self.offset.z).normalize_or_zero();
            let target_pt = trigger_origin + self.offset;
            let to_pt = Vec3::new(
                target_pt.x - fighter_pos.x,
                0.0,
                target_pt.z - fighter_pos.z,
            );
            // flat dot — how far "ahead" of the fighter the target is in the trigger's direction
            let proj = (dir.x * to_pt.x + dir.z * to_pt.z).max(0.0);
            proj * proj
        } else {
            let target_pt = trigger_origin + self.offset;
            let dx = target_pt.x - fighter_pos.x;
            let dz = target_pt.z - fighter_pos.z;
            dx * dx + dz * dz
        }
    }

    /// World-space flat direction the fighter should face / attack toward.
    fn direction(&self, trigger_origin: Vec3, fighter_pos: Vec3) -> Vec3 {
        if self.directional {
            Vec3::new(self.offset.x, 0.0, self.offset.z).normalize_or_zero()
        } else {
            let target_pt = trigger_origin + self.offset;
            Vec3::new(
                target_pt.x - fighter_pos.x,
                0.0,
                target_pt.z - fighter_pos.z,
            )
            .normalize_or_zero()
        }
    }
}

// ---------------------------------------------------------------------------
// Query helper (called from fsm_update_system)
// ---------------------------------------------------------------------------

/// Result of a successful fight vector query.
pub struct FightVectorResult {
    /// Flat (XZ) direction the fighter should attack toward.
    pub direction: Vec3,
    /// Flat distance to the target point.
    pub flat_dist: f32,
    /// ANIMATTACK_* alias recommended by the winning trigger.
    pub attack_alias: String,
}

/// Scan all active FightVectorTrigger entities and return the best one for
/// `fighter_pos`, or `None` if the fighter is outside every trigger.
///
/// Mirrors `trigFightVectorComponent::GetFightTriggerVector`.
pub fn find_fight_trigger_vector(
    fighter_pos: Vec3,
    triggers: &Query<(&GlobalTransform, &FightVectorTrigger)>,
) -> Option<FightVectorResult> {
    let mut best_score = f32::MAX;
    let mut result: Option<FightVectorResult> = None;

    for (gtf, trigger) in triggers {
        if !trigger.enabled {
            continue;
        }
        let origin = gtf.translation();

        if !trigger.contains(origin, fighter_pos) {
            continue;
        }

        let score = trigger.score(origin, fighter_pos);
        // C++: skip if score >= best; also reject score == 0 (degenerate)
        if score == 0.0 || score >= best_score {
            continue;
        }

        best_score = score;
        result = Some(FightVectorResult {
            direction: trigger.direction(origin, fighter_pos),
            flat_dist: score.sqrt(),
            attack_alias: trigger.attack_alias.clone(),
        });
    }

    result
}

// ---------------------------------------------------------------------------
// Angle check helper (mirrors C++ 30° threshold)
// ---------------------------------------------------------------------------

/// Returns true when the fighter's heading is within `threshold_deg` of `target_dir`.
/// `fighter_facing` and `target_dir` are flat (XZ) unit vectors.
pub fn facing_within(fighter_facing: Vec3, target_dir: Vec3, threshold_deg: f32) -> bool {
    // Use atan2 difference, same as C++
    let heading = f32::atan2(fighter_facing.x, fighter_facing.z);
    let desired = f32::atan2(target_dir.x, target_dir.z);
    let mut diff = (heading - desired).abs();
    if diff > std::f32::consts::PI {
        diff = 2.0 * std::f32::consts::PI - diff;
    }
    diff < threshold_deg.to_radians()
}
