use bevy::prelude::*;
use std::sync::Arc;

use crate::ai::components::AiFighter;
use crate::fightai::SquadFsmCache;
use crate::fightai::components::{
    DesiredFightGroup, FighterFightGroup, FighterOrder, Leader, Squad, SquadMember,
};
use crate::statemachine::core::SmRuntime;
use crate::statemachine::drivers::squad::{SquadAction, SquadCtx, SquadOrder};

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
