use bevy::prelude::*;

use crate::ai::components::{AiFighter, AiInterceptor};
use crate::combat::components::Health;
use crate::combat::faction::{Faction, FactionManager, FactionStatus};

/// High-fidelity port of the target-acquisition half of
/// `aiInterceptor` / `crFighter::GetClosestCreature`.
///
/// For each AI fighter, scan every faction-bearing candidate in the
/// world.  A candidate is eligible iff:
///   • It isn't the AI itself.
///   • It's alive (`Health.current > 0.0`).
///   • Its faction is `Enemy` relative to the AI's faction
///     (`FactionManager::get_status(...) == Enemy`).
///   • It's within the AI's `perception_radius`.
///
/// The closest eligible candidate wins.  If the AI already has a
/// target (or `manual_target` is set by a script), the auto-acquire
/// step is skipped — but the interceptor's active flag and
/// intercept-point still update against the existing target so the
/// fight FSM keeps steering toward it.
///
/// This replaces the earlier hardcoded "always pick the player" path
/// — Joes in m01_assault carry the TCTF faction matching the player,
/// so the player must never be selected by them as an enemy target.
/// The fix: route everything through faction filtering and let the
/// data drive who the AI considers hostile.
pub fn ai_interceptor_system(
    mut query: Query<(
        Entity,
        &mut AiInterceptor,
        &mut AiFighter,
        &GlobalTransform,
        Option<&Faction>,
    )>,
    candidates: Query<(Entity, &GlobalTransform, &Faction, &Health)>,
    factions: Res<FactionManager>,
) {
    for (self_entity, mut interceptor, mut fighter, self_tf, self_faction_opt) in &mut query {
        let self_pos = self_tf.translation();
        let radius = fighter.perception_radius();
        let radius_sq = radius * radius;

        // Acquire-target path: only run when we don't already have a
        // target and a script hasn't manually locked us out.  An AI
        // with no Faction component can't resolve enemy status — skip
        // auto-acquire for it (the legacy `GetClosestCreature` with
        // `enemiesOnly=true` returns NULL when faction info is
        // missing, same effect).
        let mut chosen: Option<(Entity, Vec3, f32)> = None; // (entity, pos, dist²)
        if !fighter.manual_target && fighter.target.is_none() {
            if let Some(self_faction) = self_faction_opt {
                for (cand_entity, cand_tf, cand_faction, cand_health) in &candidates {
                    if cand_entity == self_entity {
                        continue;
                    }
                    if cand_health.current <= 0.0 {
                        continue;
                    }
                    if factions.get_status(&self_faction.0, &cand_faction.0) != FactionStatus::Enemy
                    {
                        continue;
                    }
                    let cand_pos = cand_tf.translation();
                    let dist_sq = self_pos.distance_squared(cand_pos);
                    if dist_sq > radius_sq {
                        continue;
                    }
                    if chosen.map(|(_, _, d)| dist_sq < d).unwrap_or(true) {
                        chosen = Some((cand_entity, cand_pos, dist_sq));
                    }
                }
            }
        }

        // Resolve the position we steer toward this tick.  Preference
        // order: newly-acquired target → existing target (if alive
        // and still in candidate set) → none.
        let intercept_target_pos = if let Some((ent, pos, _)) = chosen {
            fighter.target = Some(ent);
            Some(pos)
        } else if let Some(existing) = fighter.target {
            candidates
                .get(existing)
                .ok()
                .map(|(_, tf, _, _)| tf.translation())
        } else {
            None
        };

        if let Some(target_pos) = intercept_target_pos {
            let dist_sq = self_pos.distance_squared(target_pos);
            if dist_sq < radius_sq {
                interceptor.active = true;
                interceptor.intercept_point = Some(target_pos);
            } else {
                // Target slipped beyond perception — go inactive but
                // KEEP fighter.target.  Mirrors the legacy: the AI
                // doesn't forget who they were chasing the instant the
                // target steps out of sight.  A separate target-drop
                // path (scripts, death, faction change) clears it.
                interceptor.active = false;
            }
        } else {
            interceptor.active = false;
            interceptor.intercept_point = None;
        }
    }
}
