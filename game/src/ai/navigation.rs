use bevy::prelude::*;
use std::collections::{BinaryHeap, HashMap};
use std::cmp::Ordering;

use crate::oni2_loader::parsers::graph::LayoutGraph;

#[derive(Resource, Default, Clone)]
pub struct NavGraph {
    pub points: Vec<Vec3>,
    pub names: HashMap<String, usize>,
    pub adj: Vec<Vec<(usize, f32)>>,
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
        let mut names = HashMap::new();
        let mut adj = vec![Vec::new(); total_points];
        
        let mut offset = 0;
        for g in graphs {
            for (i, p) in g.points.iter().enumerate() {
                points.push(p.position);
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
        
        Self { points, names, adj }
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
        
        if min_dist > 400.0 { // 20m threshold
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
        
        self.a_star(start_idx, end_idx)
    }
    
    fn a_star(&self, start_idx: usize, target_idx: usize) -> Option<Vec<Vec3>> {
        if start_idx >= self.points.len() || target_idx >= self.points.len() {
            return None;
        }

        let mut dists = vec![f32::MAX; self.points.len()];
        let mut parents = vec![usize::MAX; self.points.len()];
        let mut pq = BinaryHeap::new();
        
        dists[start_idx] = 0.0;
        pq.push(AStarNode { cost: 0.0, index: start_idx });
        
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
            
            if cost > dists[index] { continue; }
            
            for &(next, w) in &self.adj[index] {
                let next_cost = cost + w;
                if next_cost < dists[next] {
                    dists[next] = next_cost;
                    parents[next] = index;
                    
                    let h = self.points[next].distance(self.points[target_idx]);
                    pq.push(AStarNode { cost: next_cost + h, index: next });
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
    mut query: Query<(Entity, &mut ActorPathfollower, &mut crate::ai::components::AiFighter, &mut Transform, &mut avian3d::prelude::LinearVelocity, &mut crate::combat::components::Fighter)>,
) {
    let speed_multiplier = 4.5;
    let dt = time.delta_secs();

    for (entity, mut follower, mut ai, mut tf, mut vel, mut fighter) in &mut query {
        if follower.current_wp >= follower.path.len() {
            commands.entity(entity).remove::<ActorPathfollower>();
            ai.state = crate::ai::components::AiState::Idle;
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

        fighter.facing = dir;

        // Rotate to face movement direction. Oni2 models face +Z in local space;
        // look_at makes -Z face the target, so rotate 180° Y afterward.
        let look_target = tf.translation + dir;
        let mut target_tf = *tf;
        target_tf.look_at(look_target, Vec3::Y);
        target_tf.rotate_y(std::f32::consts::PI);
        tf.rotation = tf.rotation.slerp(target_tf.rotation, (10.0 * dt).min(1.0));
    }
}

pub fn actor_follower_system(
    time: Res<Time>,
    mut query: Query<(&crate::ai::components::ActorFollower, &mut Transform, &mut avian3d::prelude::LinearVelocity, &mut crate::combat::components::Fighter)>,
    targets: Query<&Transform, Without<crate::ai::components::ActorFollower>>,
) {
    let speed_multiplier = 4.5;
    let dt = time.delta_secs();

    for (follower, mut tf, mut vel, mut fighter) in &mut query {
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
            fighter.facing = dir;
            
            let look_target = tf.translation + dir;
            let mut expected_tf = *tf;
            expected_tf.look_at(look_target, Vec3::Y);
            expected_tf.rotate_y(std::f32::consts::PI);
            tf.rotation = tf.rotation.slerp(expected_tf.rotation, (10.0 * dt).min(1.0));
        }
    }
}
