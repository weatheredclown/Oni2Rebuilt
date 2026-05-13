/*
 * ai/cover.rs — CoverPointManager: nav-graph cover-spot reservation manager.
 *
 * Port of `bhCoverPointManager`.
 * Owns the set of `POINT_COVER` graph points (flag BIT2) and tracks
 * per-actor reservations so that
 * concurrent `takecover` requests don't all converge on the same spot.
 *
 * The C++ original ran a multi-frame request/grant handshake with hysteresis
 * and player-occupancy checks; for the first cut we keep semantics simple:
 * one Entity per cover index, request-time first-fit-by-distance, no LOS
 * probe.  That's enough to make `scavenger_cover.oni` and friends compile,
 * load, and exhibit the visible behavior (an actor walks to the nearest
 * unreserved cover spot).  Multi-spot competition + LOS shaping land later.
 */
use bevy::prelude::*;
use std::collections::HashMap;

use crate::ai::navigation::NavGraph;

/// `POINT_COVER` flag bit from the legacy graph-element flags.
pub const POINT_COVER: u32 = 1 << 2;

/// Per-level resource: indices into NavGraph.points that are flagged
/// POINT_COVER, plus a reservation map.  Built by `build_cover_points`
/// during layout load.  Empty when the level has no cover-flagged
/// waypoints — `takecover` then resolves Failed and ScrOni's
/// `do while blockingcommandfailed` retry loops fall through.
#[derive(Resource, Default, Clone)]
pub struct CoverPointManager {
    /// Indices into `NavGraph.points` whose flags include POINT_COVER.
    pub points: Vec<usize>,
    /// Active reservation: cover-points-list-index → owning actor.  Cleared
    /// when the actor releases (TakeCoverBehavior::on_exit) or when a
    /// stricter request preempts (not yet implemented).
    pub reservations: HashMap<usize, Entity>,
}

impl CoverPointManager {
    /// True when no cover points exist at all — handy short-circuit for
    /// behaviors that want to fail fast instead of running search code on
    /// an empty list.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Find a NavGraph point index for the closest unreserved cover spot
    /// to `actor_pos`.  Returns the (cover_list_index, nav_point_index)
    /// pair so callers can both write the reservation back and read the
    /// world position out of NavGraph.  None means "no spot available".
    pub fn closest_unreserved(
        &self,
        actor: Entity,
        actor_pos: Vec3,
        nav: &NavGraph,
    ) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize, f32)> = None;
        for (cover_idx, &nav_idx) in self.points.iter().enumerate() {
            // Allow an actor to "find" their own already-reserved spot —
            // mirrors the C++ `Reservations[j].Guid == guid` short-circuit
            // in `bhCoverPointManager::FindPoint`.
            match self.reservations.get(&cover_idx) {
                Some(&owner) if owner != actor => continue,
                _ => {}
            }
            let pos = match nav.points.get(nav_idx) {
                Some(&p) => p,
                None => continue,
            };
            let d2 = pos.distance_squared(actor_pos);
            if best.map_or(true, |(_, _, b)| d2 < b) {
                best = Some((cover_idx, nav_idx, d2));
            }
        }
        best.map(|(c, n, _)| (c, n))
    }

    /// Reserve `cover_idx` for `actor`.  Overwrites any prior holder —
    /// behaviors are expected to release on exit, so a collision here is
    /// the new-comer paying the cost.  No-op if `cover_idx` is out of
    /// range.
    pub fn reserve(&mut self, cover_idx: usize, actor: Entity) {
        if cover_idx < self.points.len() {
            self.reservations.insert(cover_idx, actor);
        }
    }

    /// Release whichever cover point `actor` currently holds, if any.
    /// Idempotent — calling on a non-reserving actor is a no-op.
    pub fn release_actor(&mut self, actor: Entity) {
        self.reservations.retain(|_, owner| *owner != actor);
    }
}

/// Startup-time builder: scan NavGraph for POINT_COVER bits and seed the
/// resource.  Idempotent against re-runs (rebuilds from scratch each
/// call).  The layout loader calls this right after constructing the
/// NavGraph so the resource is in place before any actor's
/// `BehaviorRuntime` ticks.
pub fn build_cover_points(nav: &NavGraph) -> CoverPointManager {
    let points: Vec<usize> = nav
        .point_flags
        .iter()
        .enumerate()
        .filter_map(|(i, &flags)| {
            if flags & POINT_COVER != 0 {
                Some(i)
            } else {
                None
            }
        })
        .collect();
    CoverPointManager {
        points,
        reservations: HashMap::new(),
    }
}
