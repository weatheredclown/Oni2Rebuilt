use avian3d::prelude::LinearVelocity;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use std::sync::Arc;

use crate::ai::components::AiFighter;
use crate::fightai::SquadFsmCache;
use crate::fightai::components::{
    DesiredFightGroup, FighterFightGroup, FighterOrder, Leader, Squad, SquadMember,
};
use crate::statemachine::core::{SmData, SmRuntime};
use crate::statemachine::drivers::squad::{SquadAction, SquadCtx, SquadDriver, SquadOrder};

// ---------------------------------------------------------------------------
// Leader locomotion (port of aiLeader::MoveToDistance / TrackTarget)
// ---------------------------------------------------------------------------
//
// NOTE: in the shipped engine these were gated behind `#if 0` — the released
// leader joined formation like a grunt.  They are the intended leader-tracking
// algorithm, so we port them faithfully and drive the leader with
// `track_target`: orbit the squad circle center within a distance band while
// side-stepping to keep the target tracked.

/// Y component of the cross product of two flat (XZ) vectors — `Vector3::CrossY`.
#[inline]
fn cross_y(a: Vec3, b: Vec3) -> f32 {
    a.z * b.x - a.x * b.z
}

/// `aiLeader::MoveToDistance` — if the leader is outside `[min_dist, max_dist]`
/// from `tgt`, return a destination at the mid distance along the line; else
/// `None` (stay put).
pub fn move_to_distance(pos: Vec3, tgt: Vec3, min_dist: f32, max_dist: f32) -> Option<Vec3> {
    let mut tgt2pos = pos - tgt;
    tgt2pos.y = 0.0;
    let dist2 = tgt2pos.x * tgt2pos.x + tgt2pos.z * tgt2pos.z;
    if dist2 <= 0.0 {
        return None;
    }
    if dist2 > min_dist * min_dist && dist2 < max_dist * max_dist {
        return None;
    }
    let mid_dist = 0.5 * (min_dist + max_dist);
    let t = mid_dist / dist2.sqrt();
    // dest = lerp(t, tgt, pos) = tgt + t*(pos - tgt)
    Some(tgt + (pos - tgt) * t)
}

/// `aiLeader::TrackTarget` — keep the leader orbiting `center` within
/// `[min_dist, max_dist]`, side-stepping to track `tgt`'s angular position.
/// Returns `Some(destination)` when the leader should move, `None` to stay.
/// `move_side` carries the side-stepping hysteresis flag (FLAG_LEADER_MOVE_SIDE).
#[allow(clippy::too_many_arguments)]
pub fn track_target(
    pos: Vec3,
    center: Vec3,
    tgt: Vec3,
    min_dist: f32,
    max_dist: f32,
    dist_threshold: f32,
    angular_threshold: f32,
    move_side: &mut bool,
) -> Option<Vec3> {
    const SIDE_TGT_DIST: f32 = 1.0;
    const MIN_T_DIST: f32 = 0.1;

    let mut c2pos = pos - center;
    c2pos.y = 0.0;
    let dist2 = c2pos.x * c2pos.x + c2pos.z * c2pos.z;
    if dist2 <= 0.0 {
        return None;
    }

    let mut c2tgt = tgt - center;
    c2tgt.y = 0.0;
    let t_dist2 = c2tgt.x * c2tgt.x + c2tgt.z * c2tgt.z;

    let mut move_now = dist2 < min_dist * min_dist || dist2 > max_dist * max_dist;

    let mut dest = Vec3::ZERO;
    let mut side = false;
    if move_now
        && (t_dist2 > dist_threshold * dist_threshold
            || (t_dist2 > MIN_T_DIST * MIN_T_DIST && *move_side))
    {
        side = true;
    }

    let mut r = -1.0_f32;
    if !move_now || side {
        let cross2 = cross_y(c2pos, c2tgt);
        let sig = cross2;
        if !side && t_dist2 > dist_threshold * dist_threshold {
            let cross2n = cross2 * cross2 / (dist2 * t_dist2);
            if cross2n > angular_threshold * angular_threshold || c2tgt.dot(c2pos) <= 0.0 {
                move_now = true;
                side = true;
            }
        }
        if side && move_now {
            let orth = Vec3::new(c2pos.z, 0.0, -c2pos.x);
            r = 1.0 / dist2.sqrt();
            dest = orth
                * (if sig < 0.0 {
                    -r * SIDE_TGT_DIST
                } else {
                    r * SIDE_TGT_DIST
                });
        }
    }

    if !move_now {
        *move_side = false;
        return None;
    }
    *move_side = side;

    if r < 0.0 {
        r = 1.0 / dist2.sqrt();
    }
    let mid_dist = 0.5 * (min_dist + max_dist);
    let t = mid_dist * r;
    dest += c2pos * t;
    dest += center;
    Some(dest)
}

/// Speed (units/sec) the leader steers toward its tracking destination.
const LEADER_MOVE_SPEED: f32 = 4.0;

/// Drives each leader toward the `track_target` destination by writing
/// `LinearVelocity` (the same channel player/AI movement uses).  Runs with the
/// AI movement systems; the leader's formation-tracking takes precedence over
/// its generic fight movement.
pub fn leader_locomotion_system(
    time: Res<Time>,
    mut leaders: Query<(
        &mut Leader,
        &AiFighter,
        &GlobalTransform,
        &mut LinearVelocity,
    )>,
    squads: Query<&Squad>,
    transforms: Query<&GlobalTransform>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (mut leader, ai, gt, mut vel) in &mut leaders {
        let (Some(squad_ent), Some(target_ent)) = (leader.squad, ai.target) else {
            leader.has_destination = false;
            continue;
        };
        let (Ok(squad), Ok(tgt_tf)) = (squads.get(squad_ent), transforms.get(target_ent)) else {
            leader.has_destination = false;
            continue;
        };
        let pos = gt.translation();
        let center = squad.circle_center;
        let tgt = tgt_tf.translation();

        // Distance band ~ the formation radius; thresholds match the legacy
        // experimental call: TrackTarget(center, tgt, 2.5, 3.5, 1.0, sin(12°)).
        let mut move_side = leader.move_side;
        let dest = track_target(
            pos,
            center,
            tgt,
            2.5,
            3.5,
            1.0,
            (12.0_f32).to_radians().sin(),
            &mut move_side,
        );
        leader.move_side = move_side;

        match dest {
            Some(d) => {
                leader.current_destination = d;
                leader.has_destination = true;
                let mut dir = d - pos;
                dir.y = 0.0;
                if let Some(dir) = dir.try_normalize() {
                    let step = dir * LEADER_MOVE_SPEED;
                    vel.x = step.x;
                    vel.z = step.z;
                }
            }
            None => {
                leader.has_destination = false;
            }
        }
    }
}

/// System to spawn a Squad entity for each newly added Leader.
/// A leader is also added as a member of their own squad.
pub fn squad_leader_lifecycle_system(
    mut commands: Commands,
    squad_cache: Res<SquadFsmCache>,
    mut query: Query<(Entity, &mut Leader), Added<Leader>>,
) {
    let Some(ref cache_data) = squad_cache.data else {
        return;
    };

    for (leader_ent, mut leader) in &mut query {
        if leader.squad.is_some() {
            continue;
        }

        let initial_state = cache_data.index_of_or_zero("S_IDLING");
        let squad_fsm = SmRuntime::new(Arc::clone(cache_data), initial_state);

        let squad_ent = commands
            .spawn(Squad {
                members: vec![leader_ent],
                leader: Some(leader_ent),
                order: SquadOrder::Idle,
                target: None,
                circle_center: Vec3::ZERO,
                distance_between_fighters: 4.0,
                circle_threshold: 70.0,
                max_num_inner: 3,
                max_in_formation: 8,
                members_changed: true,
                regroup_when_possible: false,
                fighter_lost_position: false,
                fighter_reacting: false,
                fsm: squad_fsm,
                ctx: SquadCtx::default(),
                last_state_idx: initial_state,
            })
            .id();

        leader.squad = Some(squad_ent);
        commands.entity(leader_ent).insert(SquadMember {
            squad: squad_ent,
            leader: Some(leader_ent),
        });

        bevy::log::info!("Squad created: {:?} for Leader {:?}", squad_ent, leader_ent);
    }
}

/// Spawn a fresh `Squad` entity (shared FSM-data ctor used by both the
/// leader lifecycle and the per-target attacker-squad grouping).
fn spawn_squad(
    commands: &mut Commands,
    cache: &Arc<SmData<SquadDriver>>,
    target: Option<Entity>,
    leader: Option<Entity>,
) -> Entity {
    let initial_state = cache.index_of_or_zero("S_IDLING");
    let fsm = SmRuntime::new(Arc::clone(cache), initial_state);
    commands
        .spawn(Squad {
            members: Vec::new(),
            leader,
            order: if target.is_some() {
                SquadOrder::Fight
            } else {
                SquadOrder::Idle
            },
            target,
            circle_center: Vec3::ZERO,
            distance_between_fighters: 4.0,
            circle_threshold: 70.0,
            max_num_inner: 3,
            max_in_formation: 8,
            members_changed: true,
            regroup_when_possible: false,
            fighter_lost_position: false,
            fighter_reacting: false,
            fsm,
            ctx: SquadCtx::default(),
            last_state_idx: initial_state,
        })
        .id()
}

/// Maps a fight target → the leaderless "attacker squad" of everyone currently
/// attacking it (`aiFighter::AttackerSquad`).  Leader grunt-squads are NOT in
/// here; they're owned by the leader.  `pending_empty` gives a one-frame grace
/// so a freshly-spawned squad isn't despawned before its members attach.
#[derive(Resource, Default)]
pub struct SquadRegistry {
    pub attacker_squads: HashMap<Entity, Entity>,
    pending_empty: HashSet<Entity>,
}

/// Groups leaderless AI fighters that share a target into a per-target attacker
/// squad (`aiFighter::AddToSquad` onto `target.AttackerSquad`), creating squads
/// on demand and reaping ones that stay empty.  Fighters already owned by a
/// leader (their `SquadMember.leader` is set) are left alone.
pub fn attacker_squad_membership_system(
    mut commands: Commands,
    squad_cache: Res<SquadFsmCache>,
    mut registry: ResMut<SquadRegistry>,
    fighters: Query<(Entity, &AiFighter, Option<&SquadMember>), Without<Leader>>,
    mut squads: Query<(Entity, &mut Squad)>,
) {
    let Some(cache) = &squad_cache.data else {
        return;
    };

    for (fighter, ai, member_opt) in &fighters {
        // Don't poach a leader's grunts.
        if member_opt.map(|m| m.leader.is_some()).unwrap_or(false) {
            continue;
        }
        match ai.target {
            Some(target) => {
                let squad_ent = match registry.attacker_squads.get(&target).copied() {
                    Some(s) if squads.contains(s) => s,
                    _ => {
                        let s = spawn_squad(&mut commands, cache, Some(target), None);
                        registry.attacker_squads.insert(target, s);
                        s
                    }
                };
                let in_right = member_opt.map(|m| m.squad == squad_ent).unwrap_or(false);
                if !in_right {
                    commands.entity(fighter).insert(SquadMember {
                        squad: squad_ent,
                        leader: None,
                    });
                }
            }
            None => {
                // Left combat — drop a leaderless attacker membership.
                if member_opt.is_some() {
                    commands.entity(fighter).remove::<SquadMember>();
                }
            }
        }
    }

    // Keep each attacker squad's order/target live, and reap squads that have
    // stayed empty for a full frame (the grace avoids the create→attach race).
    let mut still_empty = HashSet::default();
    for (squad_ent, mut squad) in &mut squads {
        if squad.leader.is_some() {
            continue; // leader grunt-squads are managed elsewhere
        }
        // Only touch registered attacker squads.
        if !registry.attacker_squads.values().any(|&s| s == squad_ent) {
            continue;
        }
        squad.order = if squad.target.is_some() {
            SquadOrder::Fight
        } else {
            SquadOrder::Idle
        };
        if squad.members.is_empty() {
            if registry.pending_empty.contains(&squad_ent) {
                commands.entity(squad_ent).despawn();
                if let Some(t) = squad.target {
                    registry.attacker_squads.remove(&t);
                }
            } else {
                still_empty.insert(squad_ent);
            }
        }
    }
    registry.pending_empty = still_empty;
}

// ---------------------------------------------------------------------------
// Squad merge / transfer (port of aiSquad::Empty / TransferEverybody /
// TransferNonFollowers, driven by leader EnterFight / LeaveFight)
// ---------------------------------------------------------------------------

/// `aiSquad::Empty` — remove every fighter from a squad.
pub fn empty_squad(commands: &mut Commands, members: &[Entity]) {
    for &m in members {
        commands.entity(m).remove::<SquadMember>();
    }
}

/// `aiSquad::TransferEverybody` — move every fighter into `to_squad`, marking
/// them as following `to_leader`.
pub fn transfer_everybody(
    commands: &mut Commands,
    members: &[Entity],
    to_squad: Entity,
    to_leader: Option<Entity>,
) {
    for &m in members {
        commands.entity(m).insert(SquadMember {
            squad: to_squad,
            leader: to_leader,
        });
    }
}

/// `aiSquad::TransferNonFollowers` — move fighters that are NOT following
/// `leader` out into `to_squad` (leaderless); the leader's own grunts stay.
pub fn transfer_non_followers(
    commands: &mut Commands,
    members: &[(Entity, Option<Entity>)],
    to_squad: Entity,
    leader: Entity,
) {
    for &(m, ldr) in members {
        if ldr != Some(leader) {
            commands.entity(m).insert(SquadMember {
                squad: to_squad,
                leader: None,
            });
        }
    }
}

/// Leader EnterFight/LeaveFight: when a leader engages a target that already
/// has a leaderless attacker squad, fold those attackers into the leader's
/// grunt squad and retire the attacker squad (`EnterFight` →
/// `TransferEverybody`).  When the leader disengages, the non-follower grunts
/// it absorbed are released back so `attacker_squad_membership_system`
/// re-groups them (`LeaveFight` → `TransferNonFollowers`).
pub fn leader_merge_system(
    mut commands: Commands,
    mut registry: ResMut<SquadRegistry>,
    leaders: Query<(Entity, &Leader, &AiFighter)>,
    members_q: Query<(Entity, &SquadMember)>,
) {
    for (leader_ent, leader, ai) in &leaders {
        let Some(grunt_squad) = leader.squad else {
            continue;
        };
        match ai.target {
            Some(target) => {
                if let Some(&attacker_squad) = registry.attacker_squads.get(&target)
                    && attacker_squad != grunt_squad
                {
                    let to_move: Vec<Entity> = members_q
                        .iter()
                        .filter(|(_, m)| m.squad == attacker_squad)
                        .map(|(e, _)| e)
                        .collect();
                    transfer_everybody(&mut commands, &to_move, grunt_squad, Some(leader_ent));
                    registry.attacker_squads.remove(&target);
                    commands.entity(attacker_squad).despawn();
                }
            }
            None => {
                // Disengaged: release the absorbed non-followers (those whose
                // recorded leader isn't this one) so they regroup on their own.
                let non_followers: Vec<(Entity, Option<Entity>)> = members_q
                    .iter()
                    .filter(|(_, m)| m.squad == grunt_squad)
                    .map(|(e, m)| (e, m.leader))
                    .collect();
                for (e, ldr) in non_followers {
                    if ldr != Some(leader_ent) {
                        commands.entity(e).remove::<SquadMember>();
                    }
                }
            }
        }
    }
}

/// System to synchronize Squad.members list based on entities carrying SquadMember.
pub fn squad_member_sync_system(
    mut squads: Query<(Entity, &mut Squad)>,
    members_q: Query<(Entity, &SquadMember)>,
) {
    for (squad_ent, mut squad) in &mut squads {
        let current_members: Vec<Entity> = members_q
            .iter()
            .filter(|(_, m)| m.squad == squad_ent)
            .map(|(e, _)| e)
            .collect();

        if squad.members != current_members {
            squad.members = current_members;
            squad.members_changed = true;
        }
    }
}

/// Computes the outer circle radius for a squad given the distance between fighters
/// and the number of fighters. Matches `aiFormation::ComputeCircleRadius`.
pub fn compute_circle_radius(distance_between_fighters: f32, num_fighters: usize) -> f32 {
    if num_fighters <= 1 {
        return 0.0;
    }
    distance_between_fighters / (2.0 * (std::f32::consts::PI / num_fighters as f32).sin())
}

/// System to update all Squad state machines and execute their global actions.
pub fn squad_update_system(
    time: Res<Time>,
    mut squads: Query<(Entity, &mut Squad)>,
    transforms: Query<&GlobalTransform>,
    mut member_orders: Query<(&mut FighterOrder, Option<&mut FighterFightGroup>)>,
) {
    let dt = time.delta_secs();

    for (squad_ent, mut squad) in &mut squads {
        let squad_mut = squad.as_mut();
        squad_mut.fsm.advance_clock(dt);

        // Update circle center and target-left-circle check if we have a target
        let mut target_left_circle = false;
        if let Some(target_ent) = squad_mut.target {
            if let Ok(t_tf) = transforms.get(target_ent) {
                let target_pos = t_tf.translation();
                if squad_mut.circle_center == Vec3::ZERO {
                    squad_mut.circle_center = target_pos;
                }

                // C++ check: c2Center.FlatDist2(pos) > r*r
                // where r = GetThresholdRadius() = GetOuterCircleRadius() * GetCircleThreshold() / 100
                let circle_radius = compute_circle_radius(
                    squad_mut.distance_between_fighters,
                    squad_mut.members.len(),
                )
                .max(3.0);
                let threshold_radius = circle_radius * squad_mut.circle_threshold / 100.0;

                let diff = target_pos - squad_mut.circle_center;
                let flat_dist_sq = diff.x * diff.x + diff.z * diff.z;
                if flat_dist_sq > threshold_radius * threshold_radius {
                    target_left_circle = true;
                    // Circle center moves with target in C++ aiSquad::Update
                    squad_mut.circle_center = target_pos;
                }
            }
        }

        // Fill context
        squad_mut.ctx.order = squad_mut.order;
        squad_mut.ctx.target_left_circle = target_left_circle;
        squad_mut.ctx.members_changed = squad_mut.members_changed;
        squad_mut.ctx.fighter_lost_pos = squad_mut.fighter_lost_position;
        squad_mut.ctx.fighter_reacting = squad_mut.fighter_reacting;

        let current_state = squad_mut.fsm.current_state;
        if current_state != squad_mut.last_state_idx {
            let state_name = squad_mut.fsm.data.state_name(current_state);
            bevy::log::info!("Squad {:?}: state change -> S_{}", squad_ent, state_name);
            squad_mut.last_state_idx = current_state;
        }

        // Tick FSM
        let output = squad_mut.fsm.tick(&mut squad_mut.ctx);

        // Process FSM Actions
        for action in &output.requested_actions {
            match action {
                SquadAction::GlobalIdle => {
                    for member in &squad.members {
                        if let Ok((mut fo, _)) = member_orders.get_mut(*member) {
                            fo.order = SquadOrder::Idle;
                            fo.object = None;
                        }
                    }
                }
                SquadAction::GlobalFight => {
                    for member in &squad.members {
                        if let Ok((mut fo, _)) = member_orders.get_mut(*member) {
                            fo.order = SquadOrder::Fight;
                            fo.object = squad.target;
                        }
                    }
                }
                SquadAction::GlobalRetreat => {
                    for member in &squad.members {
                        if let Ok((mut fo, _)) = member_orders.get_mut(*member) {
                            fo.order = SquadOrder::Retreat;
                            fo.object = None;
                        }
                    }
                }
                SquadAction::GlobalFormation => {
                    for member in &squad.members {
                        if let Ok((mut fo, _)) = member_orders.get_mut(*member) {
                            fo.order = SquadOrder::Formation;
                            fo.object = None;
                        }
                    }
                }
                SquadAction::Regroup => {
                    if let Some(target_ent) = squad.target {
                        if let Ok(t_tf) = transforms.get(target_ent) {
                            squad.circle_center = t_tf.translation();
                        }
                    }

                    // Assign positions/groups to members
                    for member in &squad.members {
                        if let Ok((mut fo, fg_opt)) = member_orders.get_mut(*member) {
                            let order = if let Some(fg) = fg_opt {
                                if fg.desired == DesiredFightGroup::AlwaysOuter {
                                    SquadOrder::Formation
                                } else {
                                    SquadOrder::Fight
                                }
                            } else {
                                SquadOrder::Fight
                            };
                            fo.order = order;
                            fo.object = squad.target;
                        }
                    }

                    squad.members_changed = false;
                    squad.fighter_lost_position = false;
                    squad.fighter_reacting = false;
                    squad.regroup_when_possible = false;
                }
            }
        }

        squad.members_changed = false;
    }
}

/// System to update Leader logic: propagates target/orders to GruntSquad,
/// and distributes desired fight groups among grunts.
pub fn leader_update_system(
    mut leaders: Query<(Entity, &mut Leader, &AiFighter)>,
    mut squads: Query<&mut Squad>,
    mut grunts: Query<(&SquadMember, &mut FighterFightGroup)>,
) {
    for (leader_ent, mut leader, ai_fighter) in &mut leaders {
        let Some(squad_ent) = leader.squad else {
            continue;
        };

        let Ok(mut squad) = squads.get_mut(squad_ent) else {
            continue;
        };

        if let Some(target_ent) = ai_fighter.target {
            squad.target = Some(target_ent);
            squad.order = SquadOrder::Fight;

            // grunts attack or leader attack distribution command
            match leader.distribution_command {
                0 => {
                    // LEAD_LEADER_ATTACK
                    // Leader G_ALWAYS_INNER, Grunts G_ALWAYS_OUTER
                    if let Ok((_, mut fg)) = grunts.get_mut(leader_ent) {
                        fg.desired = DesiredFightGroup::AlwaysInner;
                    }
                    for member_ent in &squad.members {
                        if *member_ent == leader_ent {
                            continue;
                        }
                        if let Ok((member, mut fg)) = grunts.get_mut(*member_ent) {
                            if member.leader == Some(leader_ent) {
                                fg.desired = DesiredFightGroup::AlwaysOuter;
                            }
                        }
                    }
                }
                1 => {
                    // LEAD_GRUNTS_ATTACK
                    // Grunts G_ALWAYS_INNER, Leader G_ALWAYS_OUTER
                    let mut num_grunts = 0;
                    for member_ent in &squad.members {
                        if *member_ent == leader_ent {
                            continue;
                        }
                        if let Ok((member, mut fg)) = grunts.get_mut(*member_ent) {
                            if member.leader == Some(leader_ent) {
                                fg.desired = DesiredFightGroup::AlwaysInner;
                                num_grunts += 1;
                            }
                        }
                    }

                    if let Ok((_, mut fg)) = grunts.get_mut(leader_ent) {
                        if num_grunts > 0 {
                            fg.desired = DesiredFightGroup::AlwaysOuter;
                        } else {
                            fg.desired = DesiredFightGroup::AlwaysInner;
                        }
                    }
                }
                _ => {}
            }

            if leader.last_distribution_command != leader.distribution_command {
                squad.fighter_lost_position = true;
            }
            leader.last_distribution_command = leader.distribution_command;
        } else {
            squad.target = None;
            squad.order = SquadOrder::Idle;
        }
    }
}
