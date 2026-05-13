/*
 * ai/navigation.rs — pathfinding and actor-follow systems.
 *
 * NavGraph resource: A* waypoint graph loaded from layout.graph.
 * path_following_system: advances entities along a NavPath toward a destination.
 * actor_follower_system: moves scripted followers to track a target entity.
 */
use bevy::prelude::*;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::f32::consts::PI;

use crate::oni2_loader::parsers::graph::LayoutGraph;

#[derive(Resource, Default, Clone)]
pub struct NavGraph {
    pub points: Vec<Vec3>,
    pub names: HashMap<String, usize>,
    pub adj: Vec<Vec<(usize, f32)>>,
    pub edge_doors: HashMap<(usize, usize), usize>,
    pub doors_open: Vec<bool>,
    /// Per-point flag bitmask carried through from layout.graph.  Bits
    /// follow the legacy `POINT_*` flags (`POINT_DOOR=BIT0`,
    /// `POINT_SHOOTING=BIT1`, `POINT_COVER=BIT2`, `POINT_RETREAT=BIT3`,
    /// `POINT_HIDING=BIT4`, …).  Same indexing as `points`.
    pub point_flags: Vec<u32>,
}

#[derive(Clone, PartialEq)]
struct AStarNode {
    cost: f32,
    index: usize,
}

impl Eq for AStarNode {}

impl PartialOrd for AStarNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        other.cost.partial_cmp(&self.cost)
    }
}

impl Ord for AStarNode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

impl NavGraph {
    pub fn new(graphs: Vec<LayoutGraph>) -> Self {
        let mut total_points = 0;
        for g in &graphs {
            total_points += g.points.len();
        }

        let mut points = Vec::with_capacity(total_points);
        let mut point_flags = Vec::with_capacity(total_points);
        let mut names = HashMap::new();
        let mut adj = vec![Vec::new(); total_points];

        let mut offset = 0;
        for g in graphs {
            for (i, p) in g.points.iter().enumerate() {
                points.push(p.position);
                point_flags.push(p.flags);
                if !p.name.is_empty() {
                    names.insert(p.name.clone(), offset + i);
                }
            }

            for e in g.edges {
                let u = offset + e.a;
                let v = offset + e.b;
                if u < total_points && v < total_points {
                    adj[u].push((v, e.cost));
                }
            }

            offset += g.points.len();
        }

        Self {
            points,
            names,
            adj,
            edge_doors: HashMap::new(),
            doors_open: Vec::new(),
            point_flags,
        }
    }

    pub fn add_door(&mut self, pos: Vec3, radius: f32) -> usize {
        let door_id = self.doors_open.len();
        self.doors_open.push(false); // doors start closed by default

        let r_sq = radius * radius;
        for u in 0..self.points.len() {
            let mut edges_to_tag = Vec::new();
            for &(v, _) in &self.adj[u] {
                let p1 = self.points[u];
                let p2 = self.points[v];

                let p1_2d = Vec2::new(p1.x, p1.z);
                let p2_2d = Vec2::new(p2.x, p2.z);
                let pos_2d = Vec2::new(pos.x, pos.z);

                let line_dir = p2_2d - p1_2d;
                let length_sq = line_dir.length_squared();

                let dist_sq = if length_sq == 0.0 {
                    pos_2d.distance_squared(p1_2d)
                } else {
                    let t = ((pos_2d - p1_2d).dot(line_dir) / length_sq).clamp(0.0, 1.0);
                    let projection = p1_2d + t * line_dir;
                    pos_2d.distance_squared(projection)
                };

                let y_min = pos.y - 1.0;
                let y_max = pos.y + 4.0;
                let edge_y_min = p1.y.min(p2.y);
                let edge_y_max = p1.y.max(p2.y);
                let vertically_overlapping = edge_y_min <= y_max && edge_y_max >= y_min;

                if dist_sq <= r_sq && vertically_overlapping {
                    edges_to_tag.push(v);
                }
            }
            for v in edges_to_tag {
                self.edge_doors.insert((u, v), door_id);
            }
        }
        door_id
    }

    pub fn set_door_state(&mut self, door_id: usize, is_open: bool) {
        if door_id < self.doors_open.len() {
            self.doors_open[door_id] = is_open;
        }
    }

    pub fn find_closest_point(&self, pos: Vec3) -> usize {
        let mut min_dist = f32::MAX;
        let mut idx = 0;
        for (i, p) in self.points.iter().enumerate() {
            let d = p.distance_squared(pos);
            if d < min_dist {
                min_dist = d;
                idx = i;
            }
        }
        idx
    }

    pub fn find_path(&self, start: Vec3, target_name: &str) -> Option<Vec<Vec3>> {
        let target_idx = *self.names.get(target_name)?;

        // Find closest point to start
        let mut start_idx = 0;
        let mut min_dist = f32::MAX;
        for (i, pos) in self.points.iter().enumerate() {
            let d = pos.distance_squared(start);
            if d < min_dist {
                min_dist = d;
                start_idx = i;
            }
        }

        if min_dist > 400.0 {
            // 20m threshold
            return None;
        }

        self.a_star(start_idx, target_idx)
    }

    pub fn find_path_to_point(&self, start: Vec3, end: Vec3) -> Option<Vec<Vec3>> {
        if self.points.is_empty() {
            return Some(vec![end]);
        }

        let mut start_idx = 0;
        let mut min_dist_s = f32::MAX;

        let mut end_idx = 0;
        let mut min_dist_e = f32::MAX;

        for (i, pos) in self.points.iter().enumerate() {
            let ds = pos.distance_squared(start);
            if ds < min_dist_s {
                min_dist_s = ds;
                start_idx = i;
            }

            let de = pos.distance_squared(end);
            if de < min_dist_e {
                min_dist_e = de;
                end_idx = i;
            }
        }

        let path_opt = self.a_star(start_idx, end_idx);
        path_opt.map(|mut path| {
            if path.last().map(|&last_pos| last_pos.distance_squared(end) > 0.01).unwrap_or(true) {
                path.push(end);
            }
            path
        })
    }

    fn a_star(&self, start_idx: usize, target_idx: usize) -> Option<Vec<Vec3>> {
        if start_idx >= self.points.len() || target_idx >= self.points.len() {
            return None;
        }

        let mut dists = vec![f32::MAX; self.points.len()];
        let mut parents = vec![usize::MAX; self.points.len()];
        let mut pq = BinaryHeap::new();

        dists[start_idx] = 0.0;
        pq.push(AStarNode {
            cost: 0.0,
            index: start_idx,
        });

        while let Some(AStarNode { cost, index }) = pq.pop() {
            if index == target_idx {
                let mut path = Vec::new();
                let mut curr = target_idx;
                while curr != usize::MAX {
                    path.push(self.points[curr]);
                    curr = parents[curr];
                }
                path.reverse();
                return Some(path);
            }

            if cost > dists[index] {
                continue;
            }

            for &(next, w) in &self.adj[index] {
                // Check if edge is blocked by a closed door
                if let Some(&door_id) = self.edge_doors.get(&(index, next))
                    && !self.doors_open[door_id]
                {
                    continue;
                }

                let next_cost = cost + w;
                if next_cost < dists[next] {
                    dists[next] = next_cost;
                    parents[next] = index;

                    let h = self.points[next].distance(self.points[target_idx]);
                    pq.push(AStarNode {
                        cost: next_cost + h,
                        index: next,
                    });
                }
            }
        }

        None
    }
}

#[derive(Component)]
pub struct ActorPathfollower {
    pub path: Vec<Vec3>,
    pub current_wp: usize,
    pub speed_throttle: f32,
    pub within: Option<f32>,
}

pub fn path_following_system(
    mut commands: Commands,
    time: Res<Time>,
    mut query: Query<(
        Entity,
        &mut ActorPathfollower,
        &mut Transform,
        &mut avian3d::prelude::LinearVelocity,
        &mut crate::combat::components::Fighter,
        Option<&mut crate::ai::components::AiDrivenVelocityThisTick>,
    )>,
) {
    let speed_multiplier = 4.5;
    let dt = time.delta_secs();

    for (entity, mut follower, mut tf, mut vel, mut fighter, mut driven_opt) in &mut query {
        if follower.current_wp >= follower.path.len() {
            commands.entity(entity).remove::<ActorPathfollower>();
            vel.x = 0.0;
            vel.z = 0.0;
            continue;
        }

        let target = follower.path[follower.current_wp];
        let mut to_target = target - tf.translation;
        to_target.y = 0.0;

        let dist = to_target.length();
        let is_last_wp = follower.current_wp == follower.path.len() - 1;
        let tolerance = if is_last_wp {
            follower.within.unwrap_or(2.5).max(1.0)
        } else {
            1.0
        };

        if dist <= tolerance {
            follower.current_wp += 1;
            continue;
        }

        let dir = to_target / dist;

        let desired = dir * speed_multiplier * follower.speed_throttle;
        vel.x = desired.x;
        vel.z = desired.z;
        if let Some(mut driven) = driven_opt {
            driven.0 = true;
        }

        // Rotate to face movement direction. Oni2 models face +Z in local space;
        // look_at makes -Z face the target, so rotate 180° Y afterward.
        let look_target = tf.translation + dir;
        let mut target_tf = *tf;
        target_tf.look_at(look_target, Vec3::Y);
        target_tf.rotate_y(std::f32::consts::PI);
        tf.rotation = tf.rotation.slerp(target_tf.rotation, (10.0 * dt).min(1.0));
        
        fighter.facing = (tf.rotation * Vec3::Z).normalize_or_zero();
        if fighter.facing.length_squared() < 0.1 {
            fighter.facing = dir; // Fallback
        }
    }
}

pub fn actor_follower_system(
    time: Res<Time>,
    mut query: Query<(
        &crate::ai::components::ActorFollower,
        &mut Transform,
        &mut avian3d::prelude::LinearVelocity,
        &mut crate::combat::components::Fighter,
        Option<&mut crate::ai::components::AiDrivenVelocityThisTick>,
        Option<&crate::oni2_loader::animation::Oni2AnimState>,
    )>,
    targets: Query<&Transform, Without<crate::ai::components::ActorFollower>>,
) {
    let speed_multiplier = 4.5;
    let dt = time.delta_secs();

    for (follower, mut tf, mut vel, mut fighter, mut driven_opt, anim_state_opt) in &mut query {
        if let Ok(target_tf) = targets.get(follower.target) {
            let mut to_target = target_tf.translation - tf.translation;
            to_target.y = 0.0;

            let dist = to_target.length();
            let tolerance = follower.within.max(1.0);
            if dist <= tolerance {
                vel.x = 0.0;
                vel.z = 0.0;
                continue;
            }

            let dir = to_target / dist;
            let desired = dir * speed_multiplier;
            vel.x = desired.x;
            vel.z = desired.z;
            if let Some(mut driven) = driven_opt {
                driven.0 = true;
            }

            if !anim_state_opt.is_some_and(crate::combat::locked_movement) {
                let look_target = tf.translation + dir;
                let mut expected_tf = *tf;
                expected_tf.look_at(look_target, Vec3::Y);
                expected_tf.rotate_y(std::f32::consts::PI);
                tf.rotation = tf
                    .rotation
                    .slerp(expected_tf.rotation, (10.0 * dt).min(1.0));
                    
                fighter.facing = (tf.rotation * Vec3::Z).normalize_or_zero();
                if fighter.facing.length_squared() < 0.1 {
                    fighter.facing = dir; // Fallback
                }
            }
        }
    }
}

pub fn retreat_steering_system(
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &crate::ai::components::ActorRetreating,
        &Transform,
        &crate::combat::faction::Faction,
    )>,
    all_factions: Query<(Entity, &Transform, &crate::combat::faction::Faction)>,
    faction_manager: Res<crate::combat::faction::FactionManager>,
    nav_graph: Option<Res<NavGraph>>,
) {
    let Some(nav_graph) = nav_graph else {
        return;
    };
    if nav_graph.points.is_empty() {
        return;
    }

    for (entity, retreating, tf, my_faction) in &mut query {
        let pos = tf.translation;
        let mut combined_threat = Vec3::ZERO;
        let mut enemies_found = 0;

        for (other_ent, other_tf, other_faction) in &all_factions {
            if entity == other_ent {
                continue;
            }
            let status = faction_manager.get_status(&my_faction.0, &other_faction.0);
            if status == crate::combat::faction::FactionStatus::Enemy {
                let dist_sq = pos.distance_squared(other_tf.translation);
                if dist_sq < 225.0 {
                    // 15 meters radius
                    let dist = dist_sq.sqrt().max(0.1);
                    combined_threat += (other_tf.translation - pos) / dist; // Normalize and weight by dist inversely if needed
                    enemies_found += 1;
                }
            }
        }

        let mut escape_dir = if enemies_found > 0 {
            // Direction away from enemies
            -(combined_threat / enemies_found as f32).normalize_or_zero()
        } else {
            // No enemies explicitly near? Just run anywhere safe. Pick forward
            tf.local_z().normalize_or_zero()
        };

        if let Some(target) = retreating.avoid_target
            && let Ok((_, target_tf, _)) = all_factions.get(target)
        {
            // If script explicitly requested retreating from this specific person, override with away vector
            let away = pos - target_tf.translation;
            escape_dir = away.normalize_or_zero();
        }

        escape_dir.y = 0.0;
        if escape_dir.length_squared() > 0.0 {
            escape_dir = escape_dir.normalize();
        } else {
            escape_dir = Vec3::Z;
        }

        // Find safest waypoint furthest in `escape_dir` out of the closest 10-20 reachable points
        let current_pt = nav_graph.find_closest_point(pos);
        let mut best_target_pt = current_pt;
        let mut best_score = f32::MIN;

        for (i, p) in nav_graph.points.iter().enumerate() {
            let dist_sq = pos.distance_squared(*p);
            if dist_sq > 25.0 && dist_sq < 900.0 {
                // Between 5m and 30m away
                let dir_to_pt = (*p - pos).normalize_or_zero();
                let alignment = dir_to_pt.dot(escape_dir);
                let dist_to_pt = dist_sq.sqrt(); // Need actual linear distance for scoring
                let score = alignment * dist_to_pt;
                if score > best_score {
                    best_score = score;
                    best_target_pt = i;
                }
            }
        }

        // Execute pathfinding towards this safe point
        if let Some(path) = nav_graph.a_star(current_pt, best_target_pt) {
            commands.entity(entity).insert(ActorPathfollower {
                path,
                current_wp: 0,
                speed_throttle: 1.0,
                within: None,
            });
        }
        // Purge retreating state so we don't recalculate next frame
        commands
            .entity(entity)
            .remove::<crate::ai::components::ActorRetreating>();
    }
}

pub fn ai_velocity_residual_clamp_system(
    mut query: Query<(
        &mut avian3d::prelude::LinearVelocity,
        &mut crate::ai::components::AiDrivenVelocityThisTick,
        Option<&crate::animator::components::ActionPlayer>,
        Option<&crate::oni2_loader::animation::Oni2AnimState>,
    ), With<crate::ai::components::AiFighter>>,
) {
    for (mut vel, mut driven, ap_opt, anim_state_opt) in &mut query {
        let mut is_reacting_or_airborne = false;
        
        if let Some(ap) = ap_opt {
            if ap.sub_state_1 == crate::animator::components::sub_state_1::REACT {
                is_reacting_or_airborne = true;
            }
        }
        
        if let Some(anim_state) = anim_state_opt {
            if !anim_state.is_grounded {
                is_reacting_or_airborne = true;
            }
        }

        if !driven.0 && !is_reacting_or_airborne {
            vel.x = 0.0;
            vel.z = 0.0; // leave y for gravity
        }
        driven.0 = false;
    }
}
