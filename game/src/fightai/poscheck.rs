/*
 * fightai/poscheck.rs — fight-position geometry validation.
 *
 * Port of `aiValidPositionCheck` (rb/src/aifight/poscheck.cpp): is a candidate
 * fight slot actually usable, or is it blocked by a wall/pillar between it and
 * the target, or sitting over a drop-off / ledge?
 *
 * The legacy class spreads the test across frames via a small state machine
 * (LEVEL → DOWN → SLOPE_DOWN / SLOPE_UP), each step a `phLevel::TestProbe`
 * segment cast against static/mover level geometry.  We collapse it into a
 * single-shot `validate_slot` using avian ray casts (cheap enough for the
 * handful of fighters per squad); the staged logic is preserved:
 *
 *   • LEVEL    — clear line target→slot at mid-heights?
 *   • DOWN     — ground under the slot within MaxDown? (else drop-off → invalid)
 *   • SLOPEDOWN— if level was clear: clear line target→slot-at-ground-height?
 *   • SLOPEUP  — if level was blocked: clear line target→slot-raised-by-MaxUp?
 *
 * Consumed by the formation layer to flag blocked `FormationSlot`s so the mover
 * doesn't walk a fighter into a wall or off a ledge.  (The legacy
 * `aiPrimaryPositionChecker` bitmask + per-frame throttling that fed the
 * pattern manager isn't ported — see TODO below.)
 */
use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::prelude::*;

/// Capsule mid-heights and slope tolerances (`aiValidPositionCheck::Init`
/// args — creatures span Y∈[0,2], so height 2.0).
const FIGHTER_HEIGHT: f32 = 2.0;
const TARGET_HEIGHT: f32 = 2.0;
/// How far up a slope a slot may sit and still be reachable.
const MAX_UP: f32 = 1.0;
/// How far below the slot ground may be before it's a drop-off.
const MAX_DOWN: f32 = 2.0;

/// Cast the probe segment a→b against level geometry; returns the hit point if
/// something blocks it (`aiValidPositionCheck::TestProbe`).
fn probe(spatial: &SpatialQuery, a: Vec3, b: Vec3, filter: &SpatialQueryFilter) -> Option<Vec3> {
    let delta = b - a;
    let dist = delta.length();
    if dist <= 1e-4 {
        return None;
    }
    let dir = Dir3::new(delta / dist).ok()?;
    spatial
        .cast_ray(a, dir, dist, true, filter)
        .map(|hit| a + dir * hit.distance)
}

/// True if `slot` is a valid fight position relative to `target`: a clear line
/// from the target (no wall/pillar) and standing ground beneath it (no
/// drop-off).  `filter` should exclude the target and the fighter so their own
/// colliders don't block the probes.  Single-shot port of
/// `aiValidPositionCheck`'s state machine.
pub fn validate_slot(
    spatial: &SpatialQuery,
    target: Vec3,
    slot: Vec3,
    filter: &SpatialQueryFilter,
) -> bool {
    let a_level = Vec3::new(target.x, target.y + TARGET_HEIGHT * 0.5, target.z);
    let b_level = Vec3::new(slot.x, slot.y + FIGHTER_HEIGHT * 0.5, slot.z);

    if probe(spatial, a_level, b_level, filter).is_some() {
        // LEVEL blocked → valid only if reachable up a slope (SLOPE_UP):
        // target → slot raised by MAX_UP must be clear.
        let b_up = Vec3::new(slot.x, slot.y + MAX_UP, slot.z);
        probe(spatial, a_level, b_up, filter).is_none()
    } else {
        // LEVEL clear → require ground under the slot (DOWN) …
        let down_a = Vec3::new(slot.x, slot.y + FIGHTER_HEIGHT * 0.5, slot.z);
        let down_b = Vec3::new(slot.x, slot.y - MAX_DOWN, slot.z);
        match probe(spatial, down_a, down_b, filter) {
            // … and a clear line down to the ground-height slot (SLOPE_DOWN).
            Some(ground) => {
                let b_down = Vec3::new(slot.x, ground.y + TARGET_HEIGHT * 0.5, slot.z);
                probe(spatial, a_level, b_down, filter).is_none()
            }
            None => false, // nothing within MaxDown → drop-off / off a ledge
        }
    }
}

// TODO(poscheck): port `aiPrimaryPositionChecker` — the per-frame throttled
// validation of the 8 resource positions with the ValidPositions/KnownPositions
// bitmasks and `GetDisabledPositions()`, which feeds the pattern manager's slot
// assignment (aiSquad::SetPattern).  We currently validate formation slots
// inline each frame and just flag them invalid; a blocked slot falls back to
// the resource slot / walk-in rather than being excluded from assignment.
