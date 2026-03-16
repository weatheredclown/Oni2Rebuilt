use super::*;

fn compute_animated_bone_transforms(
    skel: &Oni2Skeleton,
    frame_channels: &[f32],
) -> Vec<(Quat, Vec3)> {
    let num_bones = skel.positions.len();
    let mut result = vec![(Quat::IDENTITY, Vec3::ZERO); num_bones];

    for i in 0..num_bones {
        if i == 0 {
            // Root bone: translation from channels 0-2, rotation from channels 3-5
            let tx = *frame_channels.get(0).unwrap_or(&0.0);
            let ty = *frame_channels.get(1).unwrap_or(&0.0);
            let tz = *frame_channels.get(2).unwrap_or(&0.0);
            let euler_x = *frame_channels.get(3).unwrap_or(&0.0);
            let euler_y = *frame_channels.get(4).unwrap_or(&0.0);
            let euler_z = *frame_channels.get(5).unwrap_or(&0.0);
            // FromEulersXZY: R = Ry · Rz · Rx → glam XZY order
            let rot = Quat::from_euler(EulerRot::XZY, euler_x, euler_z, euler_y);
            result[0] = (rot, Vec3::new(tx, ty, tz));
        } else {
            // Non-root: euler rotation from channels i*3+3 .. i*3+5
            let ch_base = i * 3 + 3;
            let euler_x = *frame_channels.get(ch_base).unwrap_or(&0.0);
            let euler_y = *frame_channels.get(ch_base + 1).unwrap_or(&0.0);
            let euler_z = *frame_channels.get(ch_base + 2).unwrap_or(&0.0);
            let local_rot = Quat::from_euler(EulerRot::XZY, euler_x, euler_z, euler_y);

            let local_offset = Vec3::from(skel.local_offsets[i]);

            let parent_idx = skel.parent_indices[i].unwrap_or(0);
            let (parent_rot, parent_pos) = result[parent_idx];

            // Row-vector: global = local * parent → column-vector/quat: global = parent * local
            let global_rot = parent_rot * local_rot;
            let global_pos = parent_rot.mul_vec3(local_offset) + parent_pos;

            result[i] = (global_rot, global_pos);
        }
    }

    result
}

/// Compute inverse bind-pose matrices for GPU skinning.
/// Bind pose is translation-only (no rotation), so inverse is just negated translation.
/// Positions are in Oni2 coordinates; we apply X/Z negate for Bevy space.
pub(crate) fn compute_inverse_bind_poses(skel: &Oni2Skeleton) -> Vec<Mat4> {
    skel.positions
        .iter()
        .map(|pos| {
            // Bind-pose matrix: translation with X/Z negate for Bevy coordinate system
            let bind = Mat4::from_translation(Vec3::new(-pos[0], pos[1], -pos[2]));
            bind.inverse()
        })
        .collect()
}

/// System: advance CurveFollower phase, evaluate NURBS position, update Transform.
pub fn curve_follower_system(
    mut query: Query<(&mut CurveFollower, &mut Transform)>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut follower, mut tf) in &mut query {
        if follower.speed == 0.0 && follower.reached_target {
            continue;
        }

        let prev_phase = follower.phase;

        // Advance phase parametrically (knots/sec)
        follower.phase += follower.speed * dt;

        // Check if we've reached/passed the target
        if !follower.reached_target {
            let was_below = prev_phase < follower.target_phase;
            let now_above = follower.phase >= follower.target_phase;
            let was_above = prev_phase > follower.target_phase;
            let now_below = follower.phase <= follower.target_phase;
            if (was_below && now_above)
                || (was_above && now_below)
                || follower.phase == follower.target_phase
            {
                follower.phase = follower.target_phase;
                follower.reached_target = true;
            }
        }

        // Handle wrap-around / ping-pong
        let at_start = follower.phase <= 0.0;
        let at_end = follower.phase >= 1.0;
        if at_start || at_end {
            if follower.ping_pong {
                follower.speed = -follower.speed;
            } else if follower.wrap_around {
                while follower.phase >= 1.0 {
                    follower.phase -= 1.0;
                }
                while follower.phase <= 0.0 {
                    follower.phase += 1.0;
                }
            }
        }
        follower.phase = follower.phase.clamp(0.0, 1.0);

        // Evaluate position on curve
        let pos = follower.curve.get_curve_point(follower.phase);
        tf.translation = pos;

        // Orient along curve direction (look-ahead)
        let look_t = (follower.phase + 0.005).min(0.999);
        let ahead = follower.curve.get_curve_point(look_t);
        let dir = ahead - pos;
        if dir.length_squared() > 0.001 {
            let forward = if follower.look_along_xz {
                Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero()
            } else {
                dir.normalize()
            };
            if forward.length_squared() > 0.001 {
                let target = pos + forward;
                tf.look_at(target, Vec3::Y);
                // Oni2 models face +Z; look_at points -Z at target
                tf.rotate_y(std::f32::consts::PI);
            }
        }
    }
}

/// System: bridge ScrOni script VM to CurveFollower component.
/// Runs BEFORE scroni_tick_system so curve state is ready when the script checks it.
/// Runs AFTER scroni_tick_system for applying newly-issued commands.
/// We run it both before and after by just running it in Update alongside the others.
pub fn scroni_curve_bridge_system(
    mut query: Query<(
        &mut scroni::vm::ScrOniScript,
        Option<&mut CurveFollower>,
        Option<&Oni2AnimLibrary>,
        Option<&mut Oni2AnimState>,
        Option<&Name>,
    )>,
) {
    for (mut script, mut follower, anim_lib, mut anim_state, name_comp) in &mut query {
        let exec = &mut script.exec;
        let entity_name = name_comp.map(|n| n.as_str()).unwrap_or("Unknown Entity");

        // 1. Apply non-blocking curve variable writes from script
        if let Some(ref mut follower) = follower {
            if let Some(v) = exec.variables.remove("__curve_phase") {
                follower.phase = v.as_float();
                follower.reached_target = true;
                follower.speed = 0.0;
            }
            if let Some(v) = exec.variables.remove("__curve_speed") {
                follower.speed = v.as_float();
            }
            if let Some(v) = exec.variables.remove("__curve_pingpong") {
                let pp = v.as_int() != 0;
                follower.ping_pong = pp;
                follower.wrap_around = !pp;
            }
        }

        // 2. Handle blocking actions
        if exec.state == scroni::vm::ExecState::Blocked {
            if let Some(ref action) = exec.blocking {
                match action {
                    scroni::vm::BlockingAction::GotoCurvePhase { target, seconds } => {
                        if let Some(ref mut follower) = follower {
                            if follower.reached_target {
                                let target = *target;
                                let seconds = *seconds;
                                let dist = target - follower.phase;
                                follower.speed = if seconds > 0.0 { dist / seconds } else { 0.0 };
                                follower.target_phase = target;
                                follower.reached_target = false;
                                follower.wrap_around = false;
                                exec.blocking = Some(scroni::vm::BlockingAction::WaitingForCurve);
                            }
                        }
                    }
                    scroni::vm::BlockingAction::WaitingForCurve => {
                        if let Some(ref follower) = follower {
                            if follower.reached_target {
                                exec.clear_blocking();
                            }
                        }
                    }
                    scroni::vm::BlockingAction::PlayAnimation { name, hold, rate, .. } => {
                        if let Some(lib) = anim_lib.as_ref() {
                            if let Some(ref mut state) = anim_state.as_deref_mut() {
                                let name = name.clone();
                                let hold = *hold;
                                let rate = *rate;
                                if lib.play(&name, state) {
                                    state.looping = !hold;
                                    state.speed_multiplier = rate.unwrap_or(1.0);
                                    if hold {
                                        // Non-looping: wait for animation to finish
                                        exec.blocking =
                                            Some(scroni::vm::BlockingAction::WaitingForAnimation);
                                    } else {
                                        // Looping: unblock immediately, animation plays forever
                                        exec.clear_blocking();
                                    }
                                } else {
                                    warn!(
                                        "PlayAnimation: alias {:?} not found in anim library for entity {} ({:?})",
                                        name, entity_name, exec.owner
                                    );
                                    exec.clear_blocking();
                                }
                            } else {
                                warn!(
                                    "PlayAnimation: entity {} ({:?}) has AniLibrary but is missing AnimState",
                                    entity_name, exec.owner
                                );
                                exec.clear_blocking();
                            }
                        } else {
                            if anim_state.is_some() {
                                warn!(
                                    "PlayAnimation: entity {} ({:?}) has AnimState but is missing AniLibrary",
                                    entity_name, exec.owner
                                );
                            } else {
                                warn!(
                                    "PlayAnimation: entity {} ({:?}) is missing both AniLibrary and AnimState",
                                    entity_name, exec.owner
                                );
                            }
                            exec.clear_blocking();
                        }
                    }
                    scroni::vm::BlockingAction::WaitingForAnimation => {
                        if let Some(ref state) = anim_state.as_deref() {
                            let num_frames = state.anim.frames.len() as f32;
                            if num_frames > 0.0
                                && state.current_time >= num_frames - 1.0
                                && !state.looping
                            {
                                exec.clear_blocking();
                            }
                        } else {
                            exec.clear_blocking();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Info about a spawned player creature from the layout.
#[derive(Component)]
pub struct Oni2DebugBounds {
    pub vertices: Vec<Vec3>, // bound vertices in local space (Z-negated)
    pub edges: Vec<[u32; 2]>,
}

/// Debug component storing skeleton bone positions and parent links for gizmo rendering.
#[derive(Component)]
pub struct Oni2DebugSkeleton {
    pub positions: Vec<Vec3>, // bone world positions (Z-negated for Bevy)
    pub parent_indices: Vec<Option<usize>>,
    pub names: Vec<String>,
}

/// Animation state for cycling through frames.
#[derive(Component)]
pub struct Oni2AnimState {
    pub anim: Oni2Animation,
    pub skeleton: Oni2Skeleton,
    pub current_time: f32,
    pub fps: f32,
    pub paused: bool,
    pub looping: bool,
    pub speed_multiplier: f32,
    pub pending_step: i32, // +1 or -1 for single-frame step, 0 = none
    /// Last time that was rendered — skip joint update if unchanged
    pub last_rendered_time: f32,
    /// Joint entities for GPU skinning (one per bone, flat hierarchy)
    pub joint_entities: Vec<Entity>,
    /// Base rotation of the entity before any animation is applied.
    /// Used by single-channel property animations to compose on top of the original orientation.
    pub base_rotation: Quat,
    /// Blended current frame channel data
    pub current_frame: Vec<f32>,
}

/// Deterministic string hash used as animation identifier.
/// Use `AnimId::new("ANIMNAV_RUN_FORWARD")` in const context — compiles to a u64 literal.
/// Scripts produce the same hash at runtime via `AnimId::new(name)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnimId(pub u64);

impl AnimId {
    /// FNV-1a 64-bit hash, usable in const context.
    pub const fn new(s: &str) -> Self {
        let bytes = s.as_bytes();
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u64;
            hash = hash.wrapping_mul(0x100000001b3);
            i += 1;
        }
        AnimId(hash)
    }
}

impl std::fmt::Display for AnimId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AnimId(0x{:016x})", self.0)
    }
}

/// Compile-time animation ID from a string literal.
/// Usage: `anim_id!("ANIMNAV_RUN_FORWARD")` compiles to a constant u64.
#[macro_export]
macro_rules! anim_id {
    ($s:literal) => {
        $crate::oni2_loader::AnimId::new($s)
    };
}

/// Animation library mapping AnimId hashes to loaded animations.
/// Built from .anims + .apkg package files.
#[derive(Component)]
pub struct Oni2AnimLibrary {
    pub anims: std::collections::HashMap<AnimId, Oni2Animation>,
    /// Reverse map for debug: hash -> original alias string
    pub debug_names: std::collections::HashMap<AnimId, String>,
}

impl Oni2AnimLibrary {
    /// Set the active animation by alias string (hashed at runtime).
    pub fn play(&self, alias: &str, state: &mut Oni2AnimState) -> bool {
        self.play_id(AnimId::new(alias), state)
    }

    /// Set the active animation by pre-computed AnimId (zero-cost lookup).
    pub fn play_id(&self, id: AnimId, state: &mut Oni2AnimState) -> bool {
        if let Some(anim) = self.anims.get(&id) {
            state.anim = anim.clone();
            state.current_time = 0.0;
            state.looping = anim.is_loop;
            state.last_rendered_time = -1.0; // force rebuild

            // Resize current_frame to match animation channel count
            let num_channels = anim.num_channels as usize;
            if state.current_frame.len() != num_channels {
                state.current_frame = vec![0.0; num_channels];
            }

            true
        } else {
            false
        }
    }

    /// Check if an alias exists.
    pub fn has(&self, alias: &str) -> bool {
        self.anims.contains_key(&AnimId::new(alias))
    }

    /// List all available alias names (debug only).
    pub fn aliases(&self) -> Vec<&str> {
        self.debug_names.values().map(|s| s.as_str()).collect()
    }
}

/// Load an animation library for a character entity.
/// Reads the .anims file, resolves package references to .apkg files,
/// loads all referenced .anim files that match the skeleton's channel count.
pub fn load_anim_library(
    entity_dir: &str,
    entity_name: &str,
    skeleton: &Oni2Skeleton,
) -> Oni2AnimLibrary {
    let expected_channels = skeleton.positions.len() * 3 + 3;

    let mut alias_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    // Try entity.tune version first (more complete), then Entity version
    let tune_dir = "entity.tune".to_string();
    let tune_entity = format!("{}/{}", tune_dir, entity_name);
    let tune_anims = format!("{}/{}.anims", tune_entity, entity_name);
    let entity_anims = format!("{}/{}.anims", entity_dir, entity_name);

    let anims_path = if crate::vfs::exists("", &tune_anims) {
        tune_anims
    } else {
        entity_anims
    };

    if let Ok(content) = crate::vfs::read_to_string("", &anims_path) {
        parse_anims_content(&content, &mut alias_map);
    } else {
        info!("No .anims file found for {}", entity_name);
    }

    // Load the actual .anim files
    let mut anims = std::collections::HashMap::new();
    let mut debug_names = std::collections::HashMap::new();
    let mut loaded = 0;
    let mut skipped_channels = 0;
    let mut skipped_missing = 0;

    for (alias, anim_name) in &alias_map {
        let anim_file = format!("{}/{}.anim", entity_dir, anim_name);

        let data = match crate::vfs::read("", &anim_file) {
            Ok(d) => d,
            Err(_) => {
                // Fallback: If anim starts with a prefix like "kno_" or "tim_", check that entity dir
                if let Some(prefix) = anim_name.split('_').next() {
                    let mut parts: Vec<&str> = entity_dir.split('/').collect();
                    if let Some(last) = parts.last_mut() {
                        *last = prefix;
                    }
                    let fallback_dir = parts.join("/");
                    let fallback_file = format!("{}/{}.anim", fallback_dir, anim_name);

                    match crate::vfs::read("", &fallback_file) {
                        Ok(d) => d,
                        Err(_) => {
                            skipped_missing += 1;
                            continue;
                        }
                    }
                } else {
                    skipped_missing += 1;
                    continue;
                }
            }
        };
        let anim = match parse_anim(&data) {
            Some(a) => a,
            None => {
                skipped_missing += 1;
                continue;
            }
        };
        // Only skip if the anim has more than 1 channel but doesn't match skeleton.
        // Single-channel anims (e.g. simple rotation) are always accepted.
        if anim.num_channels > 1 && anim.num_channels as usize != expected_channels {
            skipped_channels += 1;
            continue;
        }
        let id = AnimId::new(alias);
        anims.insert(id, anim);
        debug_names.insert(id, alias.clone());
        loaded += 1;
    }

    info!(
        "AnimLibrary for {}: {} aliases loaded, {} skipped (channel mismatch), {} missing",
        entity_name, loaded, skipped_channels, skipped_missing
    );

    Oni2AnimLibrary { anims, debug_names }
}

/// Controls visibility of debug collision bounds wireframes.
#[derive(Resource)]
pub struct DebugBoundsVisible(pub bool);

/// Controls visibility of debug skeleton wireframes.
#[derive(Resource)]
pub struct DebugSkeletonVisible(pub bool);

/// Controls point cloud rendering mode (F5 toggle).
#[derive(Resource)]
pub struct PointCloudMode(pub bool);

/// Stores the parsed model for mesh rebuilding (point cloud toggle etc).
#[derive(Component)]
pub struct Oni2ModelData {
    pub model: Oni2Model,
    pub material_handles: Vec<Handle<StandardMaterial>>,
    pub fallback_material: Handle<StandardMaterial>,
}

/// Toggle debug bounds with F3.
pub fn toggle_debug_bounds(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut visible: ResMut<DebugBoundsVisible>,
) {
    if keyboard.just_pressed(KeyCode::F3) {
        visible.0 = !visible.0;
    }
}

/// Draw wireframe bounds using the actual bound edge geometry.
pub fn debug_draw_bounds(
    query: Query<(&Transform, &Oni2DebugBounds)>,
    mut gizmos: Gizmos,
    visible: Res<DebugBoundsVisible>,
) {
    if !visible.0 {
        return;
    }
    let color = Color::srgb(0.0, 1.0, 0.0);
    for (transform, bounds) in &query {
        for edge in &bounds.edges {
            if let (Some(&va), Some(&vb)) = (
                bounds.vertices.get(edge[0] as usize),
                bounds.vertices.get(edge[1] as usize),
            ) {
                let wa = transform.transform_point(va);
                let wb = transform.transform_point(vb);
                gizmos.line(wa, wb, color);
            }
        }
    }
}

/// Draw wireframe capsules for all physics colliders when debug bounds are visible (F3).
pub fn debug_draw_capsules(
    query: Query<(&Transform, &Collider)>,
    mut gizmos: Gizmos,
    visible: Res<DebugBoundsVisible>,
) {
    if !visible.0 {
        return;
    }
    let color = Color::srgb(0.0, 1.0, 1.0); // cyan to distinguish from green bounds
    for (transform, _collider) in &query {
        // Draw capsule wireframe: two circles (top/bottom of cylinder) + vertical lines
        // Capsule(radius, height) where height is the cylinder portion
        // We approximate with the standard capsule dimensions used: radius=0.4, half_height=0.6
        let radius = 0.4_f32;
        let half_height = 0.6_f32;
        let pos = transform.translation;
        let segments = 16;

        // Top and bottom circle centers
        let top_center = pos + Vec3::Y * half_height;
        let bot_center = pos - Vec3::Y * half_height;

        // Draw circles at top and bottom of cylinder section
        for i in 0..segments {
            let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
            let dx0 = a0.cos() * radius;
            let dz0 = a0.sin() * radius;
            let dx1 = a1.cos() * radius;
            let dz1 = a1.sin() * radius;

            // Top circle
            gizmos.line(
                top_center + Vec3::new(dx0, 0.0, dz0),
                top_center + Vec3::new(dx1, 0.0, dz1),
                color,
            );
            // Bottom circle
            gizmos.line(
                bot_center + Vec3::new(dx0, 0.0, dz0),
                bot_center + Vec3::new(dx1, 0.0, dz1),
                color,
            );

            // Top hemisphere arc (vertical)
            let pa0 = (i as f32 / segments as f32) * std::f32::consts::PI;
            let pa1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::PI;
            // Front-back arc
            gizmos.line(
                top_center + Vec3::new(0.0, pa0.sin() * radius, pa0.cos() * radius),
                top_center + Vec3::new(0.0, pa1.sin() * radius, pa1.cos() * radius),
                color,
            );
            // Bottom hemisphere arc
            gizmos.line(
                bot_center + Vec3::new(0.0, -pa0.sin() * radius, pa0.cos() * radius),
                bot_center + Vec3::new(0.0, -pa1.sin() * radius, pa1.cos() * radius),
                color,
            );
        }

        // Vertical lines connecting top and bottom
        for i in [0, 4, 8, 12] {
            let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let dx = angle.cos() * radius;
            let dz = angle.sin() * radius;
            gizmos.line(
                top_center + Vec3::new(dx, 0.0, dz),
                bot_center + Vec3::new(dx, 0.0, dz),
                color,
            );
        }
    }
}

/// Toggle point cloud mode with F5 — rebuilds all Oni2 meshes.
pub fn toggle_point_cloud(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut point_cloud: ResMut<PointCloudMode>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    query: Query<(Entity, &Oni2ModelData, &Children)>,
) {
    if !keyboard.just_pressed(KeyCode::F5) {
        return;
    }
    point_cloud.0 = !point_cloud.0;
    let mode = if point_cloud.0 {
        "POINT CLOUD"
    } else {
        "TRIANGLES"
    };
    info!("Render mode: {}", mode);

    for (entity, model_data, children) in &query {
        // Despawn old mesh children
        let child_entities: Vec<Entity> = children.iter().collect();
        for child in child_entities {
            commands.entity(child).despawn();
        }

        // Rebuild meshes in the new mode
        let sub_meshes = if point_cloud.0 {
            build_point_clouds_by_material(&model_data.model)
        } else {
            build_meshes_by_material(&model_data.model)
        };

        commands.entity(entity).with_children(|parent| {
            for (mat_idx, mesh) in sub_meshes {
                let mat_handle = model_data
                    .material_handles
                    .get(mat_idx)
                    .cloned()
                    .unwrap_or_else(|| model_data.fallback_material.clone());
                parent.spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(mat_handle),
                    Transform::default(),
                ));
            }
        });
    }
}

/// Toggle debug skeleton with F4.
pub fn toggle_debug_skeleton(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut visible: ResMut<DebugSkeletonVisible>,
) {
    if keyboard.just_pressed(KeyCode::F4) {
        visible.0 = !visible.0;
    }
}

/// Draw skeleton bones as lines from child to parent, with spheres at joints.
pub fn debug_draw_skeleton(
    query: Query<(
        &Transform,
        &Oni2DebugSkeleton,
        Option<&CreatureRenderOffset>,
    )>,
    trigger_query: Query<&crate::scroni::vm::BroadcastTrigger>,
    mut gizmos: Gizmos,
    visible: Res<DebugSkeletonVisible>,
) {
    if !visible.0 {
        return;
    }
    let bone_color = Color::srgb(1.0, 1.0, 0.0);
    let joint_color = Color::srgb(1.0, 0.3, 0.0);

    for (transform, skel, offset) in &query {
        let base_offset = offset.map_or(Vec3::ZERO, |o| Vec3::new(0.0, o.y_offset, 0.0));

        // Draw lines from each bone to its parent
        for (i, parent_idx) in skel.parent_indices.iter().enumerate() {
            let pos = skel.positions[i] + base_offset;
            let world_pos = transform.transform_point(pos);

            // Draw joint sphere
            gizmos.sphere(Isometry3d::from_translation(world_pos), 0.008, joint_color);

            // Draw line to parent
            if let Some(pi) = parent_idx {
                let parent_pos = skel.positions[*pi] + base_offset;
                let world_parent = transform.transform_point(parent_pos);
                gizmos.line(world_pos, world_parent, bone_color);
            }
        }
    }

    let trigger_color = Color::srgba(1.0, 0.5, 0.0, 0.5); // Semi-transparent orange

    let trigger_count = trigger_query.iter().count();
    println!(
        "DEBUG DRAW SKELETON: Found {} triggers to draw (visible = {})",
        trigger_count, visible.0
    );

    for trigger in &trigger_query {
        let world_pos = trigger.world_center;
        println!(
            "  -> Drawing trigger at {:?} with radius {}",
            world_pos, trigger.radius
        );
        gizmos.sphere(
            Isometry3d::from_translation(world_pos),
            trigger.radius,
            trigger_color,
        );
    }
}

/// Linearly interpolate between two animation frames, storing the result in `state.current_frame`.
pub fn frame_lerp(state: &mut Oni2AnimState, idx_a: usize, idx_b: usize, t: f32) {
    // Destructure to borrow `current_frame` mutably and `anim` immutably at the same time
    let Oni2AnimState {
        anim,
        current_frame,
        ..
    } = state;

    let a = &anim.frames[idx_a];
    let b = &anim.frames[idx_b];
    let len = current_frame.len().min(a.len()).min(b.len());
    for i in 0..len {
        current_frame[i] = a[i] + (b[i] - a[i]) * t;
    }
}

/// Update animation: advance frame, recompute bone transforms, rebuild meshes.
pub fn update_oni2_animation(
    time: Res<Time>,
    mut anim_query: Query<(
        Entity,
        &mut Oni2AnimState,
        Option<&mut Oni2DebugSkeleton>,
        Option<&CreatureRenderOffset>,
    )>,
    mut transform_query: Query<&mut Transform>,
) {
    for (entity, mut anim_state, debug_skel, render_offset) in &mut anim_query {
        let num_frames = anim_state.anim.num_frames as usize;
        if num_frames <= 1 {
            continue;
        }

        if anim_state.paused {
            if anim_state.pending_step == 0 {
                continue;
            }
            // Single-frame step
            let cur = anim_state.current_time as i32 + anim_state.pending_step;
            anim_state.current_time = cur.rem_euclid(num_frames as i32) as f32;
            anim_state.pending_step = 0;
        } else {
            // Advance time
            anim_state.current_time +=
                time.delta_secs() * anim_state.fps * anim_state.speed_multiplier;
            if anim_state.looping {
                if anim_state.current_time >= num_frames as f32 {
                    anim_state.current_time %= num_frames as f32;
                }
            } else {
                anim_state.current_time = anim_state.current_time.min(num_frames as f32 - 1.0);
            }
        }
        let frame_idx = (anim_state.current_time as usize).min(anim_state.anim.frames.len() - 1);
        let mut next_idx = frame_idx + 1;
        if next_idx >= anim_state.anim.frames.len() {
            if anim_state.looping {
                next_idx = 0;
            } else {
                next_idx = frame_idx;
            }
        }
        let blend = anim_state.current_time - (anim_state.current_time as usize) as f32;

        // Skip joint update if time hasn't changed
        if anim_state.current_time == anim_state.last_rendered_time {
            continue;
        }
        anim_state.last_rendered_time = anim_state.current_time;

        // Ensure current_frame has correct capacity
        let expected_len = anim_state.anim.frames[frame_idx].len();
        if anim_state.current_frame.len() != expected_len {
            anim_state.current_frame = vec![0.0; expected_len];
        }

        frame_lerp(&mut *anim_state, frame_idx, next_idx, blend);

        let frame = &anim_state.current_frame;

        // Single-channel animation: apply as Y-rotation composed on top of base orientation
        if anim_state.anim.num_channels == 1 {
            if let Some(y_rot) = frame.first() {
                if let Ok(mut tf) = transform_query.get_mut(entity) {
                    tf.rotation = anim_state.base_rotation * Quat::from_rotation_y(*y_rot);
                }
            }
            continue;
        }

        let mut bone_transforms = compute_animated_bone_transforms(&anim_state.skeleton, frame);

        // Strip root motion: zero out root bone XZ translation so the model
        // stays pinned to its entity origin. Keep Y for vertical anim motion.
        if let Some(root) = bone_transforms.get_mut(0) {
            root.1.x = 0.0;
            root.1.z = 0.0;
        }

        // Creature render offset (capsule Y compensation + facing)
        let y_offset = render_offset.map(|o| o.y_offset).unwrap_or(0.0);
        let facing = render_offset.map(|o| o.facing).unwrap_or(Quat::IDENTITY);

        // Update joint entity transforms for GPU skinning
        for (i, (rot, pos)) in bone_transforms.iter().enumerate() {
            if let Some(&joint_entity) = anim_state.joint_entities.get(i) {
                if let Ok(mut joint_tf) = transform_query.get_mut(joint_entity) {
                    // Convert from Oni2 coordinates to Bevy: negate X and Z
                    let bevy_pos = Vec3::new(-pos.x, pos.y + y_offset, -pos.z);
                    // Conjugate rotation by 180° Y rotation: negate X and Z components
                    let bevy_rot = Quat::from_xyzw(-rot.x, rot.y, -rot.z, rot.w);
                    // Apply facing rotation (if model needs to be rotated)
                    let final_rot = facing * bevy_rot;
                    let final_pos = facing * bevy_pos;
                    *joint_tf = Transform::from_translation(final_pos).with_rotation(final_rot);
                }
            }
        }

        // Update debug skeleton positions
        if let Some(mut ds) = debug_skel {
            ds.positions = bone_transforms
                .iter()
                .map(|(_, pos)| Vec3::new(-pos.x, pos.y, -pos.z))
                .collect();
        }
    }
}
