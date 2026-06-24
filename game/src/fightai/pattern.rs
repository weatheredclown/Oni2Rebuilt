/*
 * fightai/pattern.rs — squad slot assignment (aiFightPatternManager::CalcMapping).
 *
 * The legacy pattern manager maps a squad's attackers onto a target's ring of
 * positions by fitting a data-driven "pattern" (settings/rb.patterns lists, per
 * fighter count, the candidate slot-sets) — rotating the pattern to best match
 * the fighters' current angles, respecting per-fighter disabled positions and
 * priority.  Its observable effect is a *collision-free, evenly-distributed,
 * angular-order* assignment so attackers don't pile onto the same slot.
 *
 * This is that observable behavior without the data file: angular-sort the
 * squad's members around the target and hand each the nearest free,
 * non-disabled slot, written to `FightSlotState.assigned_position` (the
 * request path already prefers it over closest-at-request-time).
 *
 * TODO(pattern): port the data-driven matcher — load `settings/rb.patterns`
 * (per-count candidate slot-sets), and pick + rotate the best-fitting pattern
 * via `aiFightPattern::ComputeDistance`/`InitMapping` (priority + LOCK_IN/OUT
 * flags + the FindNClosest culling for when there are more fighters than
 * positions).  The current heuristic ignores the pattern sets and priority.
 */
use bevy::prelude::*;

use crate::fightai::components::Squad;
use crate::fightai::position::{closest_direction, FightResources, FightSlotState, NUM_POSITIONS};

/// Find the free, non-disabled slot nearest `desired` on the ring, searching
/// outward by ring distance.  `None` if every slot is taken/disabled.
fn nearest_free_slot(desired: usize, used: &[bool], disabled: &[bool]) -> Option<usize> {
    let n = NUM_POSITIONS;
    // offset 0, ±1, ±2, … around the ring from the desired slot.
    for step in 0..=(n / 2) {
        for &cand in &[(desired + step) % n, (desired + n - step % n) % n] {
            if !used[cand] && !disabled[cand] {
                return Some(cand);
            }
        }
    }
    None
}

/// Per squad: assign each member a distinct ring slot on the target in angular
/// order (pragmatic `CalcMapping`).  Writes `FightSlotState.assigned_position`.
pub fn squad_pattern_mapping_system(
    squads: Query<&Squad>,
    transforms: Query<&GlobalTransform>,
    resources_q: Query<&FightResources>,
    mut slots: Query<&mut FightSlotState>,
) {
    for squad in &squads {
        let Some(target) = squad.target else {
            continue;
        };
        let Ok(tgt_tf) = transforms.get(target) else {
            continue;
        };
        let center = tgt_tf.translation();

        // Slots already unavailable on the target (held / owner-reserved /
        // grapple-disabled) can't be assigned.
        let disabled: [bool; NUM_POSITIONS] = resources_q
            .get(target)
            .map(|r| {
                std::array::from_fn(|i| {
                    let s = &r.positions[i];
                    s.holder.is_some() || s.grabbed_by_owner || s.grapple_disabled
                })
            })
            .unwrap_or([false; NUM_POSITIONS]);

        // Members that fight this target, sorted by their angle around it so
        // the assignment goes around the ring without crossings.
        let mut members: Vec<(Entity, f32)> = squad
            .members
            .iter()
            .filter(|&&m| m != target && slots.contains(m))
            .filter_map(|&m| {
                let tf = transforms.get(m).ok()?;
                let d = tf.translation() - center;
                Some((m, d.z.atan2(d.x)))
            })
            .collect();
        if members.is_empty() {
            continue;
        }
        members.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut used = [false; NUM_POSITIONS];
        for (member, _) in &members {
            let Ok(tf) = transforms.get(*member) else {
                continue;
            };
            let desired = closest_direction(tf.translation(), center) as usize % NUM_POSITIONS;
            if let Some(slot) = nearest_free_slot(desired, &used, &disabled) {
                used[slot] = true;
                if let Ok(mut st) = slots.get_mut(*member) {
                    st.assigned_position = slot as i32;
                }
            }
        }
    }
}
