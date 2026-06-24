/*
 * fightai/formation.rs — circular squad formation (port of aiFormation's
 * shipped `UpdateCircular` core: even-spaced slots + SortFighters assignment).
 *
 * Each `Squad` arranges its members on a circle of radius
 * `ComputeCircleRadius(distanceBetweenFighters, N)` around `circle_center`.
 * `SortFighters` (formation.cpp `Compare`) orders the members by their current
 * angle around the center and assigns each the slot at that angular rank, so
 * assignments never cross and stay stable as fighters move.
 *
 * The orientation `Theta` is fixed per squad (the legacy
 * `ComputeCircleOrientation` notes the circle orientation "is actually fixed
 * now because it didn't work well with the forces and random positioning" —
 * the function survived only to assign fighters to positions).  We derive the
 * per-squad random orientation deterministically from the squad entity so no
 * persistent drift state is needed; the force-based slot drift
 * (`ComputeOffsets`/`Force*`) depends on unported `aiFightManager` tuning and
 * is intentionally omitted.
 *
 * Output is a `FormationSlot` on each member (ideal position + face target),
 * the analog of `aiFighter::SetFormationPos`.
 */
use std::f32::consts::TAU;

use bevy::prelude::*;

use crate::ai::components::AiFighter;
use crate::fightai::components::{Squad, SquadMember};
use crate::fightai::squad_leader::compute_circle_radius;

/// Minimum outer-circle radius (`aiSquad::CalcOuterCircleRadius` MAGIC 3.0).
const MIN_RADIUS: f32 = 3.0;

/// The formation slot a squad member is assigned to (`aiSquadPositionCircular`).
/// Written every tick by [`formation_update_system`]; the mover/fight layer
/// steers the fighter toward `center` and faces `face_target`.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct FormationSlot {
    /// Ideal world position on the circle (`GetIdealPosition`).
    pub center: Vec3,
    /// Point to face from the slot (`GetIdealFaceTarget`) — the target if any,
    /// else the circle center.
    pub face_target: Vec3,
    pub valid: bool,
}

/// Stable per-squad orientation offset in `[0, 1)` (`RandomOrientation`).
/// Derived from the squad entity so it's deterministic and needs no stored
/// drift state — golden-ratio hashing of the entity index spreads squads.
fn random_orientation(squad: Entity) -> f32 {
    // Golden-ratio hash of the low entity bits → stable pseudo-random [0, 1).
    let lo = (squad.to_bits() & 0xFFFF) as f32;
    (lo * 0.618_034).fract()
}

/// `SortInfo` for one member: the normalized XZ direction from the circle
/// center to the fighter, plus the fighter entity.
struct SortInfo {
    dir: Vec3,
    entity: Entity,
}

/// `aiFormation::Compare` — total order along the circle by the direction's
/// angle, without an `atan2` (matches the legacy branch logic exactly).
fn compare(a: &SortInfo, b: &SortInfo) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (ax, az) = (a.dir.x, a.dir.z);
    let (bx, bz) = (b.dir.x, b.dir.z);
    let result = if az >= 0.0 {
        if bz < 0.0 {
            Ordering::Less
        } else if ax < bx {
            Ordering::Greater
        } else {
            Ordering::Less
        }
    } else if bz > 0.0 {
        Ordering::Greater
    } else if ax < bx {
        Ordering::Less
    } else {
        Ordering::Greater
    };
    result
}

/// Per-squad: lay the members out on the circle and write each one's
/// [`FormationSlot`].  Runs after `circle_center` / `members` are current.
pub fn formation_update_system(
    mut commands: Commands,
    squads: Query<(Entity, &Squad)>,
    fighters: Query<&GlobalTransform, With<AiFighter>>,
    members_q: Query<&SquadMember>,
    transforms: Query<&GlobalTransform>,
    library: Res<crate::fightai::formation_data::FormationLibrary>,
    leader_formations: Query<&crate::fightai::formation_data::LeaderFormation>,
    spatial: avian3d::prelude::SpatialQuery,
) {
    use avian3d::prelude::SpatialQueryFilter;
    use crate::fightai::poscheck::validate_slot;

    for (squad_ent, squad) in &squads {
        // --- Custom (leader-led) formation: column / wedge / line ----------
        // When the squad has a leader with a resolvable custom formation, place
        // grunts at their slots in the leader's frame (aiFormation::UpdateCustom,
        // absolute-placement core: `squadMatrix.Transform(LocalAbsolutePos)`).
        //
        // TODO(formation): port the relative-follow + LerpToAbsolute smoothing
        // from aiFormation::UpdateCustom (formation.cpp ~410-470).  Right now we
        // use pure absolute placement (the dominant term, correct shape).  The
        // refinement: for slots with RelativeTo >= 0, build the rel matrix from
        // the followed fighter's current position/heading, transform
        // LocalRelativePos, then lerp(LerpToAbsolute) between that and the
        // absolute slot — and LerpAngle the facing similarly.  Gives the
        // snake-like column tracking instead of a rigid offset.
        if let Some(leader_ent) = squad.leader
            && let Ok(lf) = leader_formations.get(leader_ent)
            && let Some(fdata) = library.get(&lf.name)
            && let Ok(leader_gt) = fighters.get(leader_ent)
        {
            let grunts: Vec<Entity> = squad
                .members
                .iter()
                .copied()
                .filter(|&m| {
                    m != leader_ent
                        && members_q.get(m).map(|x| x.squad) == Ok(squad_ent)
                        && fighters.contains(m)
                })
                .collect();
            if !grunts.is_empty()
                && let Some(cs) = fdata.get(grunts.len() + 1)
            {
                let leader_pos = leader_gt.translation();
                // Grunts face the squad target if any, else the leader.
                let face = squad
                    .target
                    .and_then(|t| transforms.get(t).ok())
                    .map(|tf| tf.translation())
                    .unwrap_or(leader_pos);
                for (i, &g) in grunts.iter().enumerate() {
                    // Slot 0 is the leader; grunts take slots 1..N.
                    let local = cs
                        .positions
                        .get(i + 1)
                        .map(|p| p.local_absolute_pos)
                        .unwrap_or(Vec3::ZERO);
                    let world = leader_gt.transform_point(local);
                    // Geometry-validate the slot (poscheck): clear line from the
                    // target + standing ground, ignoring the target's and the
                    // fighter's own colliders.
                    let mut excluded = vec![g];
                    if let Some(t) = squad.target {
                        excluded.push(t);
                    }
                    let filter = SpatialQueryFilter::from_excluded_entities(excluded);
                    let valid = validate_slot(&spatial, face, world, &filter);
                    commands.entity(g).insert(FormationSlot {
                        center: world,
                        face_target: face,
                        valid,
                    });
                }
                continue; // custom formation placed — skip the circular path
            }
        }

        // Center the ring on the live target so the formation tracks a moving
        // target every frame (the squad's `circle_center` only re-snaps on
        // `target_left_circle`, which would lag).  Fall back to the squad
        // center for targetless squads.
        let target_pos = squad
            .target
            .and_then(|t| transforms.get(t).ok())
            .map(|tf| tf.translation());
        let center = target_pos.unwrap_or(squad.circle_center);

        // Only members that still belong to this squad and have a transform.
        let mut infos: Vec<SortInfo> = Vec::with_capacity(squad.members.len());
        for &member in &squad.members {
            // Skip the leader: in a leader formation it sits at the gap; the
            // grunts ring the circle (matches ComputeDirections skipping the
            // leader).
            if squad.leader == Some(member) {
                continue;
            }
            if members_q.get(member).map(|m| m.squad) != Ok(squad_ent) {
                continue;
            }
            let Ok(tf) = fighters.get(member) else {
                continue;
            };
            let mut dir = tf.translation() - center;
            dir.y = 0.0;
            let mag = (dir.x * dir.x + dir.z * dir.z).sqrt();
            if mag > 0.0 {
                dir /= mag;
            }
            infos.push(SortInfo {
                dir,
                entity: member,
            });
        }

        let n = infos.len();
        if n == 0 {
            continue;
        }

        // Even angular spacing.  A leader formation leaves one extra gap (the
        // leader's slot), as in UpdateCircular's `2*PI/(NumPositions + 1)`.
        let has_leader = squad.leader.is_some();
        let denom = if has_leader { n + 1 } else { n } as f32;
        let angle_between = TAU / denom;

        let radius = compute_circle_radius(squad.distance_between_fighters, n).max(MIN_RADIUS);

        // Fixed orientation (see module note).
        let theta0 = angle_between * random_orientation(squad_ent);

        // SortFighters: order members by their current angle around the center
        // so each takes the nearest slot in ring order (no path crossing).
        infos.sort_by(compare);

        // Face target: the squad's target if present, else the center.
        let face_target = target_pos.unwrap_or(center);

        for (i, info) in infos.iter().enumerate() {
            let a = theta0 + i as f32 * angle_between;
            let (sx, sz) = (a.cos(), a.sin());
            let pos = Vec3::new(center.x + sx * radius, center.y, center.z + sz * radius);
            // Geometry-validate against the ring center (the target), ignoring
            // the target's and the fighter's own colliders.
            let mut excluded = vec![info.entity];
            if let Some(t) = squad.target {
                excluded.push(t);
            }
            let filter = SpatialQueryFilter::from_excluded_entities(excluded);
            let valid = validate_slot(&spatial, center, pos, &filter);
            commands.entity(info.entity).insert(FormationSlot {
                center: pos,
                face_target,
                valid,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Angle in `[0, 2π)` — the order `compare` realizes.
    fn angle01(dir: Vec3) -> f32 {
        let mut a = dir.z.atan2(dir.x);
        if a < 0.0 {
            a += TAU;
        }
        a
    }

    #[test]
    fn compare_orders_around_circle() {
        // A shuffled set of evenly-spaced directions, once `compare`-sorted,
        // must come out in ascending [0, 2π) angle — a single clean ring with
        // no crossings.
        let n = 8;
        let mut infos: Vec<SortInfo> = (0..n)
            .map(|i| {
                let a = (i as f32) * TAU / n as f32; // 0 .. 2π
                SortInfo {
                    dir: Vec3::new(a.cos(), 0.0, a.sin()),
                    entity: Entity::PLACEHOLDER, // unused by `compare`
                }
            })
            .collect();
        infos.reverse(); // shuffle deterministically
        infos.sort_by(compare);
        let angles: Vec<f32> = infos.iter().map(|s| angle01(s.dir)).collect();
        for w in angles.windows(2) {
            assert!(w[0] <= w[1] + 1e-4, "not ascending: {:?}", angles);
        }
    }
}
