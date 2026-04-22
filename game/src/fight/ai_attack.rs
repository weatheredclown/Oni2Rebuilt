/*
 * fight/ai_attack.rs — AI strike selection.
 *
 * Mirrors `ftAIAttackData` + `aiFighter::CanStrike` /
 * `ChooseRandomStrike` from rb/src/fight/fightertype.h:185 and
 * rb/src/aifight/fighter.cpp:3035.  Each entry describes one attack an AI
 * can throw and carries the geometry needed to answer "can I hit THIS
 * target with THIS attack right now?" — range, cone, height window.
 *
 * The table is built per-fighter-type at spawn time by scanning the
 * creature's `Oni2AnimLibrary` for anims that carry attack_data; AI
 * decision code (FightDriver) reads the table when picking a strike.
 */
use bevy::prelude::*;
use rand::Rng;
use std::collections::HashMap;

use crate::combat::components::{AttackClass, AttackStrength, AttackTarget};
use crate::oni2_loader::animation::{AnimId, Oni2AnimLibrary};

// ---------------------------------------------------------------------------
// AiAttackData — one entry per AI-usable attack
// ---------------------------------------------------------------------------

/// Geometry + classification for one attack an AI character can throw.
/// Mirrors `ftAIAttackData` but keyed on our `AnimId` rather than the
/// legacy `crAttackIndex`.  Fields are lifted from the attack's ATDT strike
/// data at build time so runtime lookups are pure reads.
#[derive(Debug, Clone)]
pub struct AiAttackData {
    /// Animation alias (e.g. "ANIMKNO_NAV_PUNCH_HIGH") used for debug /
    /// matching, and the handle gameplay would pass to the animation
    /// library to actually play the attack.
    pub anim_name: String,
    /// Animation ID (FNV hash of the alias) for efficient equality.
    pub anim_id: AnimId,

    // --- Range ---
    /// Outer reach of the attack, in world units.  Matches
    /// `AtdtStrike.reactdiskradius`.
    pub range: f32,
    /// Pre-computed `range * range` to dodge a sqrt in the range gate.
    pub range_squared: f32,

    // --- Slice cone (all in attacker-local radians; 0 = forward) ---
    /// Angle offset from attacker-forward to the cone's center line.
    pub slice_heading_rads: f32,
    /// Cone's left edge relative to the heading (usually negative).
    pub slice_start_rads: f32,
    /// Cone's right edge relative to the heading (usually positive).
    pub slice_end_rads: f32,

    // --- Height window (attacker-relative vertical) ---
    /// Y height above attacker base at which the strike lands.
    pub height: f32,
    /// Vertical tolerance around `height`.
    pub height_tolerance: f32,

    /// Damage dealt on hit — surfaced so AI can prefer higher-damage
    /// attacks without having to look up the full attack_data twice.
    pub damage: f32,

    // --- Classification (from ATDT targetclass/strengthclass/attackclass) ---
    pub target_class: Option<AttackTarget>,
    pub strength_class: Option<AttackStrength>,
    pub attack_class: Option<AttackClass>,
}

impl AiAttackData {
    /// Can this attack hit the given target right now?  Gates are applied
    /// in the same order as `ftAIAttackData::CanHit`:
    ///   1. Range:  target's XZ distance² must be within `range_squared`.
    ///   2. Height: target's Y-extent must overlap the strike's height
    ///              window (`height ± height_tolerance`).  `target_height`
    ///              is the target's capsule full-height (head-to-feet).
    ///   3. Cone:   target's bearing (in attacker-local frame) must fall
    ///              between `slice_start_rads` and `slice_end_rads`
    ///              relative to `slice_heading_rads`.
    ///
    /// `attacker_facing` is expected to be a world-space XZ unit vector
    /// (Fighter.facing).  `target_pos` is the target's feet position;
    /// `target_height` is its total vertical extent.
    pub fn can_hit(
        &self,
        attacker_pos: Vec3,
        attacker_facing: Vec3,
        target_pos: Vec3,
        target_height: f32,
    ) -> bool {
        let diff = target_pos - attacker_pos;

        // 1. Range gate (XZ only).
        let range_sq = diff.x * diff.x + diff.z * diff.z;
        if range_sq > self.range_squared {
            return false;
        }

        // 2. Height gate.  The C++ treats `heightDiff` as target.y - attacker.y
        // and asks `Height ∈ [heightDiff, heightDiff + targetHeight]`.  We
        // relax both edges by `height_tolerance` — the same tolerance that
        // governs our melee-slice disk extent on the defender side.
        let height_diff = diff.y;
        if self.height < height_diff - self.height_tolerance {
            return false;
        }
        if self.height > height_diff + target_height + self.height_tolerance {
            return false;
        }

        // 3. Cone gate.  Rotate attacker-forward by `slice_heading_rads`
        // to get the world-space slice center, then take the signed angle
        // between that center and the XZ direction to target.  Matches the
        // signed-angle check in hit_detection_system.
        let facing_xz = Vec3::new(attacker_facing.x, 0.0, attacker_facing.z).normalize_or_zero();
        if facing_xz.length_squared() < 0.0001 {
            // Degenerate facing — can't resolve a cone; reject conservatively.
            return false;
        }
        let slice_center = Quat::from_rotation_y(self.slice_heading_rads) * facing_xz;
        let slice_center_xz = Vec3::new(slice_center.x, 0.0, slice_center.z).normalize_or_zero();
        let dir_to_target = Vec3::new(diff.x, 0.0, diff.z).normalize_or_zero();
        if dir_to_target.length_squared() < 0.0001 {
            // Target directly overhead / beneath — conservatively accept
            // (height gate already passed and range is 0).
            return true;
        }
        let dot = slice_center_xz.dot(dir_to_target).clamp(-1.0, 1.0);
        let angle = dot.acos();
        let cross_y = slice_center_xz.cross(dir_to_target).y;
        let signed_angle = if cross_y > 0.0 { angle } else { -angle };

        signed_angle >= self.slice_start_rads && signed_angle <= self.slice_end_rads
    }
}

// ---------------------------------------------------------------------------
// AiAttackTable — per-fighter-type attack roster
// ---------------------------------------------------------------------------

/// Per-fighter-type roster of AI-usable attacks.  Attached as a component
/// so AI decision code reads it without a resource lookup.  Mirrors
/// `ftFighterData::AIAttackData[]` + `ftFighterData::GetAIAttackData`.
///
/// The roster is type-intrinsic (every Konoko has the same attacks), so
/// it's built once per fighter-type name via `AiAttackTableCache` and
/// then cloned into each instance's component.  The Vec clone is small
/// (~80 bytes × attack count) and keeps the read API a zero-cost
/// `&[AiAttackData]` slice at call time — no Arc deref in the hot path.
#[derive(Component, Default, Debug, Clone)]
pub struct AiAttackTable {
    pub attacks: Vec<AiAttackData>,
}

impl AiAttackTable {
    pub fn new(attacks: Vec<AiAttackData>) -> Self {
        Self { attacks }
    }

    /// Scan every anim in `lib` that carries attack_data and build an AI
    /// entry for it.  Mirrors `crAttacksData::InitStrikeInfo` — an attack
    /// is skipped when:
    ///   • It has no strike (e.g. setup-only anim),
    ///   • Its reach is zero (non-geometric attacks),
    ///   • It's a weapon-fire strike (handled by projectile AI instead).
    pub fn build_from_library(lib: &Oni2AnimLibrary) -> Self {
        let mut attacks = Vec::new();
        // Build once to minimize HashMap churn.
        let name_lookup: &HashMap<AnimId, String> = &lib.debug_names;

        for (id, anim) in &lib.anims {
            let Some(attack_data) = &anim.attack_data else {
                continue;
            };
            let Some(strike) = &attack_data.strike else {
                continue;
            };
            if strike.reactdiskradius <= 0.0 {
                continue;
            }
            if strike.fire {
                // Weapon-fire attacks don't use melee cone geometry.
                continue;
            }

            let name = name_lookup.get(id).cloned().unwrap_or_default();
            let range = strike.reactdiskradius;

            attacks.push(AiAttackData {
                anim_name: name,
                anim_id: *id,
                range,
                range_squared: range * range,
                slice_heading_rads: strike.sliceheadingradiansb,
                slice_start_rads: strike.slicestartradians,
                slice_end_rads: strike.sliceendradians,
                height: strike.reactdiskheight,
                height_tolerance: strike.reactdiskheighttolerance,
                damage: attack_data.damage,
                target_class: attack_data.target_class,
                strength_class: attack_data.strength_class,
                attack_class: attack_data.attack_class,
            });
        }

        Self { attacks }
    }

    /// True iff ANY attack in the table can land on `target` from the
    /// attacker's current pose.  Mirrors `aiFighter::CanStrike`.
    pub fn can_strike(
        &self,
        attacker_pos: Vec3,
        attacker_facing: Vec3,
        target_pos: Vec3,
        target_height: f32,
    ) -> bool {
        self.attacks
            .iter()
            .any(|a| a.can_hit(attacker_pos, attacker_facing, target_pos, target_height))
    }

    /// Pick a random attack from the full roster, ignoring range/cone.
    /// Used for warmup/feint behavior.  Mirrors the no-arg overload of
    /// `aiFighter::ChooseRandomStrike`.
    pub fn choose_random_strike<R: Rng + ?Sized>(&self, rng: &mut R) -> Option<&AiAttackData> {
        if self.attacks.is_empty() {
            return None;
        }
        let idx = rng.random_range(0..self.attacks.len());
        self.attacks.get(idx)
    }

    /// Pick a random attack uniformly from the subset whose `can_hit`
    /// passes against the given target.  Mirrors the geometric overload
    /// of `aiFighter::ChooseRandomStrike`.  Returns `None` when no attack
    /// is in range / aligned.
    pub fn choose_random_strike_for<R: Rng + ?Sized>(
        &self,
        rng: &mut R,
        attacker_pos: Vec3,
        attacker_facing: Vec3,
        target_pos: Vec3,
        target_height: f32,
    ) -> Option<&AiAttackData> {
        let usable: Vec<&AiAttackData> = self
            .attacks
            .iter()
            .filter(|a| a.can_hit(attacker_pos, attacker_facing, target_pos, target_height))
            .collect();
        if usable.is_empty() {
            return None;
        }
        let idx = rng.random_range(0..usable.len());
        usable.get(idx).copied()
    }
}

// ---------------------------------------------------------------------------
// Per-type cache
// ---------------------------------------------------------------------------

/// Cache of `AiAttackTable`s keyed on `FighterType.name`.  The table is
/// intrinsic to the fighter-type — every Konoko has the same attack
/// roster — so we build it once per distinct name and clone the small
/// Vec into each instance's component on spawn.
///
/// Without this cache we'd re-scan the full `Oni2AnimLibrary` (and walk
/// every anim's attack_data) once per entity, which scales badly when a
/// level spawns N copies of the same fighter type.  The `get_or_build`
/// path runs the library scan exactly once per unique name.
#[derive(Resource, Default, Debug)]
pub struct AiAttackTableCache {
    by_type: std::collections::HashMap<String, AiAttackTable>,
}

impl AiAttackTableCache {
    /// Return the cached table for `fighter_type_name`, building it from
    /// `lib` on first access.  Subsequent calls with the same name skip
    /// the build and return a reference to the cached entry.
    pub fn get_or_build(
        &mut self,
        fighter_type_name: &str,
        lib: &Oni2AnimLibrary,
    ) -> &AiAttackTable {
        // `entry().or_insert_with` keeps the allocation-free path when the
        // name is already cached — the closure only runs on first insert.
        self.by_type
            .entry(fighter_type_name.to_string())
            .or_insert_with(|| AiAttackTable::build_from_library(lib))
    }

    pub fn len(&self) -> usize {
        self.by_type.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_type.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Auto-attach system
// ---------------------------------------------------------------------------

/// Bevy system that spots entities with `FighterType + Oni2AnimLibrary`
/// but no `AiAttackTable` and attaches a cached one.  The cache keys on
/// `FighterType.name`, so the library scan runs exactly once per unique
/// fighter-type regardless of how many entities of that type spawn.
pub fn build_ai_attack_tables_system(
    mut commands: Commands,
    mut cache: ResMut<AiAttackTableCache>,
    query: Query<(Entity, &super::FighterType, &Oni2AnimLibrary, &Name), Without<AiAttackTable>>,
) {
    for (entity, fighter_type, lib, name) in &query {
        let cached = cache.get_or_build(&fighter_type.name, lib);
        if cached.attacks.is_empty() {
            debug!(
                "ai_attack: '{}' (type '{}') produced no usable attacks \
                 from its library — skipping table insert (will retry \
                 on next tick in case the library hot-reloads).",
                name.as_str(),
                fighter_type.name,
            );
            continue;
        }
        let attack_count = cached.attacks.len();
        let table = cached.clone();
        debug!(
            "ai_attack: attached AI roster to '{}' (type '{}') — {} attacks \
             (cache size: {})",
            name.as_str(),
            fighter_type.name,
            attack_count,
            cache.len(),
        );
        commands.entity(entity).insert(table);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_attack(
        range: f32,
        slice_heading: f32,
        slice_start: f32,
        slice_end: f32,
        height: f32,
        tolerance: f32,
    ) -> AiAttackData {
        AiAttackData {
            anim_name: "TEST_ATK".to_string(),
            anim_id: AnimId::new("TEST_ATK"),
            range,
            range_squared: range * range,
            slice_heading_rads: slice_heading,
            slice_start_rads: slice_start,
            slice_end_rads: slice_end,
            height,
            height_tolerance: tolerance,
            damage: 10.0,
            target_class: Some(AttackTarget::Body),
            strength_class: Some(AttackStrength::Low),
            attack_class: Some(AttackClass::Punch),
        }
    }

    #[test]
    fn range_gate_rejects_out_of_reach() {
        let atk = test_attack(2.0, 0.0, -0.5, 0.5, 1.0, 0.5);
        let attacker = Vec3::ZERO;
        let facing = Vec3::Z;
        let in_range = Vec3::new(0.0, 1.0, 1.5);
        let out_of_range = Vec3::new(0.0, 1.0, 5.0);

        assert!(atk.can_hit(attacker, facing, in_range, 2.0));
        assert!(!atk.can_hit(attacker, facing, out_of_range, 2.0));
    }

    #[test]
    fn cone_gate_rejects_behind_attacker() {
        let atk = test_attack(5.0, 0.0, -0.5, 0.5, 1.0, 0.5);
        let attacker = Vec3::ZERO;
        let facing = Vec3::Z; // forward = +Z
        let in_front = Vec3::new(0.0, 1.0, 2.0);
        let behind = Vec3::new(0.0, 1.0, -2.0);

        assert!(atk.can_hit(attacker, facing, in_front, 2.0));
        assert!(!atk.can_hit(attacker, facing, behind, 2.0));
    }

    #[test]
    fn height_gate_rejects_too_low_or_too_high() {
        // Strike height = 3.0 ± 0.1.  Target at feet=0 with height 2.0 spans
        // Y=[0, 2], which does NOT overlap strike plane 3.0 ± 0.1.
        let atk = test_attack(5.0, 0.0, -1.0, 1.0, 3.0, 0.1);
        let attacker = Vec3::ZERO;
        let facing = Vec3::Z;
        let low_target = Vec3::new(0.0, 0.0, 2.0);
        let tall_target = Vec3::new(0.0, 0.0, 2.0); // height 5

        assert!(!atk.can_hit(attacker, facing, low_target, 2.0));
        assert!(atk.can_hit(attacker, facing, tall_target, 5.0));
    }

    #[test]
    fn can_strike_any_of_many() {
        let short = test_attack(1.0, 0.0, -0.5, 0.5, 1.0, 0.5);
        let long = test_attack(4.0, 0.0, -0.5, 0.5, 1.0, 0.5);
        let table = AiAttackTable::new(vec![short, long]);

        let attacker = Vec3::ZERO;
        let facing = Vec3::Z;
        let mid = Vec3::new(0.0, 1.0, 3.0); // out of short, in of long

        assert!(table.can_strike(attacker, facing, mid, 2.0));

        let far = Vec3::new(0.0, 1.0, 10.0); // out of both
        assert!(!table.can_strike(attacker, facing, far, 2.0));
    }

    #[test]
    fn choose_random_strike_skips_unusable() {
        let short = test_attack(1.0, 0.0, -0.5, 0.5, 1.0, 0.5);
        let long = test_attack(4.0, 0.0, -0.5, 0.5, 1.0, 0.5);
        let table = AiAttackTable::new(vec![short, long.clone()]);

        let attacker = Vec3::ZERO;
        let facing = Vec3::Z;
        let mid = Vec3::new(0.0, 1.0, 3.0); // only long reaches

        // Drive enough iterations that if unusable attacks were being
        // returned we'd see them — seeded by thread_rng.
        let mut rng = rand::rng();
        for _ in 0..50 {
            let picked = table
                .choose_random_strike_for(&mut rng, attacker, facing, mid, 2.0)
                .expect("at least one attack is in range");
            assert_eq!(picked.anim_name, long.anim_name);
        }
    }

    #[test]
    fn choose_random_from_empty_returns_none() {
        let table = AiAttackTable::default();
        let mut rng = rand::rng();
        assert!(table.choose_random_strike(&mut rng).is_none());
        assert!(
            table
                .choose_random_strike_for(&mut rng, Vec3::ZERO, Vec3::Z, Vec3::Z, 2.0)
                .is_none()
        );
    }

    #[test]
    fn cache_builds_once_per_type() {
        // The cache must scan the library only the first time we ask for a
        // given fighter-type name.  We verify by seeding the cache with a
        // sentinel entry under the type name and observing that a second
        // get_or_build call returns the sentinel rather than re-deriving
        // from the empty library.
        let mut cache = AiAttackTableCache::default();
        let sentinel = AiAttackTable::new(vec![test_attack(1.0, 0.0, -0.5, 0.5, 1.0, 0.5)]);
        cache.by_type.insert("konoko".to_string(), sentinel.clone());

        // Empty library would normally produce an empty table; the cache
        // hit short-circuits that path.
        let empty_lib = Oni2AnimLibrary {
            anims: Default::default(),
            debug_names: Default::default(),
        };
        let hit = cache.get_or_build("konoko", &empty_lib);
        assert_eq!(hit.attacks.len(), 1, "expected sentinel entry");
        assert_eq!(cache.len(), 1, "no new entry inserted on hit");

        // A different type name misses and falls through to the (empty)
        // build — proving we don't share across names.
        let miss = cache.get_or_build("striker", &empty_lib);
        assert_eq!(miss.attacks.len(), 0);
        assert_eq!(cache.len(), 2);
    }
}
