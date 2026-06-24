use bevy::prelude::*;

use crate::ai::components::{AiFighter, AiInterceptor};
use crate::combat::components::Health;
use crate::combat::faction::{Faction, FactionManager, FactionStatus};

/// Interceptor closing speed (≈ `MaxSpeed`, the AI run speed).
const INTERCEPT_MAX_SPEED: f32 = 6.0;
/// Cap on how far ahead of the target we aim (`aiInterceptorData::MaxLeadTime`).
const INTERCEPT_MAX_LEAD_TIME: f32 = 1.0;

/// Smallest positive interception time for a pursuer at `my_speed` chasing a
/// target offset by `to_tgt` (flat XZ) moving at `tgt_vel` — port of
/// `aiInterceptor::Solve`.  Solves `(|v|² − s²)t² + 2(v·d)t + |d|² = 0`.
fn intercept_time(to_tgt: Vec3, my_speed: f32, tgt_vel: Vec3) -> Option<f32> {
    let a = tgt_vel.length_squared() - my_speed * my_speed;
    let b = 2.0 * tgt_vel.dot(to_tgt);
    let c = to_tgt.length_squared();
    if a.abs() < 1e-5 {
        if b.abs() < 1e-5 {
            return None;
        }
        let t = -c / b;
        return (t > 0.0).then_some(t);
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    [(-b - sq) / (2.0 * a), (-b + sq) / (2.0 * a)]
        .into_iter()
        .filter(|t| *t > 0.0)
        .reduce(f32::min)
}

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
    velocities: Query<&avian3d::prelude::LinearVelocity>,
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
        let intercept_target = if let Some((ent, pos, _)) = chosen {
            fighter.target = Some(ent);
            Some((ent, pos))
        } else if let Some(existing) = fighter.target {
            candidates
                .get(existing)
                .ok()
                .map(|(e, tf, _, _)| (e, tf.translation()))
        } else {
            None
        };

        if let Some((target_ent, target_pos)) = intercept_target {
            let dist_sq = self_pos.distance_squared(target_pos);
            if dist_sq < radius_sq {
                interceptor.active = true;
                // Lead the moving target: aim at where it will be, not where it
                // is (aiInterceptor::Solve + the MaxLeadTime clamp).
                let mut to_tgt = target_pos - self_pos;
                to_tgt.y = 0.0;
                let tgt_vel = velocities
                    .get(target_ent)
                    .map(|v| Vec3::new(v.x, 0.0, v.z))
                    .unwrap_or(Vec3::ZERO);
                let lead = intercept_time(to_tgt, INTERCEPT_MAX_SPEED, tgt_vel)
                    .unwrap_or(0.0)
                    .clamp(0.0, INTERCEPT_MAX_LEAD_TIME);
                interceptor.intercept_point = Some(target_pos + tgt_vel * lead);
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
