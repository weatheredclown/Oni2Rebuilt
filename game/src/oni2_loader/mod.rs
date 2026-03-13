pub mod curve;
pub mod parsers;
pub mod utils;
pub mod components;

pub use components::*;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};


use crate::menu::InGameEntity;
use crate::scroni;
use crate::oni2_loader::parsers::actor_xml::*;
use crate::oni2_loader::parsers::animation::*;
use crate::oni2_loader::parsers::anims::*;
use crate::oni2_loader::parsers::bound::*;
use crate::oni2_loader::parsers::entity_type::*;
use crate::oni2_loader::parsers::layout::*;
use crate::oni2_loader::parsers::mesh::*;
use crate::oni2_loader::parsers::model::*;
use crate::oni2_loader::parsers::skeleton::*;
use crate::oni2_loader::parsers::texture::*;
use crate::oni2_loader::parsers::types::*;
use crate::oni2_loader::utils::bone::*;

use crate::oni2_loader::curve::NurbsCurve;

/// Resource indicating test-anim mode with the path to the .anim file.
#[derive(Resource)]
pub struct TestAnimMode(pub String);

/// Marker component for the testanim HUD text node.
#[derive(Component)]
pub struct TestAnimHud;

/// Orbit camera that rotates around a target point.
#[derive(Component)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
}

/// Fog settings parsed from layout.fog. Inserted as a resource so the camera
/// system can attach `DistanceFog` to the camera entity.
#[derive(Resource)]
pub struct LayoutFogSettings {
    pub color: Color,
    pub start: f32,
    pub end: f32,
}

/// Navigation curves from layout.paths. Each curve is a named list of waypoints.
#[derive(Resource, Default)]
pub struct LayoutPaths {
    pub curves: Vec<(String, Vec<Vec3>)>,
}

/// Global mapping of Texture Collection (.tc) files loaded during layout setup.
/// Maps entity type (e.g., "ProximityMine") to a list of preloaded texture handles by frame index.
#[derive(Resource, Default)]
pub struct TextureCollections {
    pub collections: std::collections::HashMap<String, Vec<Handle<Image>>>,
}

/// Marker resource: fog rendering is enabled (--fog flag).
#[derive(Resource)]
pub struct FogEnabled;

/// Marker for skyhat entity — follows camera XZ position.
#[derive(Component)]
pub struct SkyHat;

/// System: attach DistanceFog to the camera when LayoutFogSettings resource exists.
pub fn apply_fog_to_camera(
    mut commands: Commands,
    fog: Option<Res<LayoutFogSettings>>,
    cameras: Query<(Entity, Option<&DistanceFog>), With<Camera3d>>,
) {
    let Some(fog) = fog else { return };
    for (entity, existing) in &cameras {
        if existing.is_none() {
            commands.entity(entity).insert(DistanceFog {
                color: fog.color,
                falloff: FogFalloff::Linear {
                    start: fog.start,
                    end: fog.end,
                },
                ..default()
            });
        }
    }
}

/// System: keep skyhat positioned at camera XZ, fixed Y.
pub fn update_skyhat(
    camera_query: Query<&Transform, With<Camera3d>>,
    mut skyhat_query: Query<&mut Transform, (With<SkyHat>, Without<Camera3d>)>,
) {
    let Ok(cam_tf) = camera_query.single() else { return };
    for mut tf in &mut skyhat_query {
        tf.translation.x = cam_tf.translation.x;
        tf.translation.z = cam_tf.translation.z;
    }
}

/// Compute per-bone global transforms from one animation frame.
/// Uses XZY euler convention and parent-chain accumulation per AGE engine.
/// Returns Vec of (rotation_quat, world_position) per bone.
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
fn compute_inverse_bind_poses(skel: &Oni2Skeleton) -> Vec<Mat4> {
    skel.positions.iter().map(|pos| {
        // Bind-pose matrix: translation with X/Z negate for Bevy coordinate system
        let bind = Mat4::from_translation(Vec3::new(-pos[0], pos[1], -pos[2]));
        bind.inverse()
    }).collect()
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
            if (was_below && now_above) || (was_above && now_below) || follower.phase == follower.target_phase {
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
                while follower.phase >= 1.0 { follower.phase -= 1.0; }
                while follower.phase <= 0.0 { follower.phase += 1.0; }
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
                    scroni::vm::BlockingAction::PlayAnimation { name, hold, rate } => {
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
                                        exec.blocking = Some(scroni::vm::BlockingAction::WaitingForAnimation);
                                    } else {
                                        // Looping: unblock immediately, animation plays forever
                                        exec.clear_blocking();
                                    }
                                } else {
                                    warn!("PlayAnimation: alias {:?} not found in anim library for entity {} ({:?})", name, entity_name, exec.owner);
                                    exec.clear_blocking();
                                }
                            } else {
                                warn!("PlayAnimation: entity {} ({:?}) has AniLibrary but is missing AnimState", entity_name, exec.owner);
                                exec.clear_blocking();
                            }
                        } else {
                            if anim_state.is_some() {
                                warn!("PlayAnimation: entity {} ({:?}) has AnimState but is missing AniLibrary", entity_name, exec.owner);
                            } else {
                                warn!("PlayAnimation: entity {} ({:?}) is missing both AniLibrary and AnimState", entity_name, exec.owner);
                            }
                            exec.clear_blocking();
                        }
                    }
                    scroni::vm::BlockingAction::WaitingForAnimation => {
                        if let Some(ref state) = anim_state.as_deref() {
                            let num_frames = state.anim.frames.len() as f32;
                            if num_frames > 0.0 && state.current_time >= num_frames - 1.0 && !state.looping {
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
pub struct LayoutPlayerInfo {
    pub entity: Entity,
    pub position: Vec3,
    pub entity_type: String,
    pub animator_type: String,
}

/// Load an ONI2 layout directory, spawning all entities and creatures.
/// Returns info about the player creature if one was found (Player="1").
pub fn load_layout(
    commands: &mut Commands,
    asset_server: &AssetServer,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    skinned_mesh_ibp: &mut ResMut<Assets<SkinnedMeshInverseBindposes>>,
    layout_dir: &str,
    entity_base_dir: &str,
) -> Option<LayoutPlayerInfo> {
    let layout_path = layout_dir;
    let entity_base = entity_base_dir;

    // Parse layout.et to find which types are BASICENTITY
    let mut basic_types = std::collections::HashSet::new();
    if let Ok(et_content) = crate::vfs::read_to_string(layout_path, "layout.et") {
        for line in et_content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("BASICENTITY") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    basic_types.insert(parts[1].to_string());
                }
            }
        }
    }
    info!("Layout: {} basic entity types", basic_types.len());

    // Parse layout.paths early so we can look up curves during entity spawning
    let layout_paths = LayoutPaths {
        curves: parsers::layout::parse_layout_paths(layout_path)
    };
    if !layout_paths.curves.is_empty() {
        info!("Layout: loaded {} path curves", layout_paths.curves.len());
    }

    // Parse layout.actors to get actor list
    let actors_content = match crate::vfs::read_to_string(layout_path, "layout.actors") {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to read layout.actors: {}", e);
            return None;
        }
    };

    // Template directory for resolving base= references
    let mut parts: Vec<&str> = layout_path.split('/').collect();
    parts.pop();
    parts.pop();
    let assets_base = if parts.is_empty() { String::new() } else { parts.join("/") };
    let template_dir = format!("{}/template", assets_base);

    let mut texture_collections = TextureCollections::default();

    let mut spawned = 0;
    let mut creatures = 0;
    let mut skipped = 0;
    let mut player_info: Option<LayoutPlayerInfo> = None;
    for line in actors_content.lines() {
        let actor_name = line.trim();
        if actor_name.is_empty() || actor_name.parse::<u32>().is_ok() {
            continue; // skip count line and blank lines
        }

        // Parse the actor XML file (with template resolution)
        let actor = match parse_actor_xml(layout_path, &format!("{}.xml", actor_name), &template_dir) {
            Some(a) => a,
            None => {
                skipped += 1;
                continue;
            }
        };

        // Find the entity directory
        let entity_dir = format!("{}/{}", entity_base, actor.entity_type);
        
        // Try parsing .sha to find .tc (Texture Collection) and preload textures
        if !texture_collections.collections.contains_key(&actor.entity_type) {
            let sha_filename = format!("{}.sha", actor.entity_type);
            let mut frames = Vec::new();
            
            if let Ok(sha_content) = crate::vfs::read_to_string(&entity_dir, &sha_filename) {
                let mut tc_name = None;
                for line in sha_content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("texcluster ") {
                        let parts: Vec<&str> = trimmed.split_whitespace().collect();
                        if parts.len() >= 2 {
                            tc_name = Some(parts[1].to_string());
                            break;
                        }
                    }
                }
                
                if let Some(mut tc) = tc_name {
                    if !tc.to_lowercase().ends_with(".tc") {
                        tc.push_str(".tc");
                    }
                    if let Ok(tc_content) = crate::vfs::read_to_string(&entity_dir, &tc) {
                        for line in tc_content.lines() {
                            let trimmed = line.trim();
                            if trimmed.is_empty() || trimmed.starts_with("version:") || trimmed.starts_with("texCount:") {
                                continue;
                            }
                            
                            // Load the texture handle using the asset server
                            let tex_name = match trimmed.strip_suffix(".tex") {
                                Some(stripped) => stripped.to_string(),
                                None => trimmed.to_string(),
                            };
                            
                            // We use load_tga_texture from the texture parser as it correctly reads from VFS
                            // and falls back to decoding .tex natively instead of depending on Bevy AssetServer.
                            if let Some((tex_handle, _)) = load_tga_texture(&entity_dir, &tex_name, images) {
                                frames.push(tex_handle);
                            }
                        }
                        info!("Loaded Texture Collection for {}: {} frames", actor.entity_type, frames.len());
                    }
                }
            }
            texture_collections.collections.insert(actor.entity_type.clone(), frames);
        }

        if actor.is_creature {
            // Position already in Bevy coordinates (Z negated at parse time)
            let position = actor.position;
            // 180° Y rotation flips X and Z rotation directions
            let rotation = Quat::from_rotation_x(-actor.orientation.x.to_radians())
                * Quat::from_rotation_y(actor.orientation.y.to_radians())
                * Quat::from_rotation_z(-actor.orientation.z.to_radians());

            if let Some(ref anim_type) = actor.animator_type {
                info!("Creature {} type={} animator={} player={}",
                    actor_name, actor.entity_type, anim_type, actor.is_player);
            }

            if let Some(entity) = spawn_oni2_creature(
                commands, meshes, materials, images, skinned_mesh_ibp,
                &entity_dir,
                position,
                rotation,
                actor_name,
                &actor.entity_type,
                actor.animator_type.as_deref(),
                &assets_base,
            ) {
                if actor.is_player && player_info.is_none() {
                    player_info = Some(LayoutPlayerInfo {
                        entity,
                        position,
                        entity_type: actor.entity_type.clone(),
                        animator_type: actor.animator_type.clone().unwrap_or_default(),
                    });
                } else {
                    // Non-player creature: attach AI + combat components
                    commands.entity(entity).insert((
                        crate::combat::components::Enemy,
                        crate::ai::components::AiFighter::default(),
                        crate::combat::components::Fighter::default(),
                        crate::combat::components::FighterId(uuid::Uuid::new_v4()),
                        crate::combat::components::Health::new(100.0),
                    ));
                    commands.entity(entity).insert((
                        crate::combat::components::AttackState::default(),
                        crate::combat::components::BlockState::new(),
                        crate::combat::components::ComboTracker::default(),
                        crate::combat::components::SuperMeter::default(),
                        crate::combat::components::GrabState::default(),
                        crate::combat::components::HitReaction::default(),
                        crate::combat::components::AboutToBeHit::default(),
                    ));
                    commands.entity(entity).insert(crate::camera::components::PrototypeElement);
                }
            }
            creatures += 1;
        } else {
            // Static entity (BASICENTITY check)
            let is_basic = basic_types.iter().any(|t| t.eq_ignore_ascii_case(&actor.entity_type));
            if !is_basic {
                skipped += 1;
                continue;
            }

            let position = actor.position;
            // 180° Y rotation flips X and Z rotation directions
            let rotation = Quat::from_rotation_x(-actor.orientation.x.to_radians())
                * Quat::from_rotation_y(actor.orientation.y.to_radians())
                * Quat::from_rotation_z(-actor.orientation.z.to_radians());

            if let Some(entity) = spawn_oni2_entity_with_rotation(
                commands, meshes, materials, images, skinned_mesh_ibp,
                &entity_dir,
                position,
                rotation,
                &actor.entity_type,
                None,
                Some(&actor.entity_type),
            ) {
                // Attach CurveFollower if actor references a named curve
                if let Some(ref cname) = actor.curve_name {
                        if let Some((_, pts)) = layout_paths.curves.iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(cname))
                        {
                            if pts.len() >= 4 {
                                let curve = NurbsCurve::new(pts.clone());
                                // If the actor has a script, let the script drive the curve
                                // (start idle, reached_target=true so first GotoCurvePhase is picked up).
                                // Otherwise use default behavior.
                                let has_script = actor.script_filename.is_some();
                                let speed = if has_script {
                                    0.0 // script will set speed via GotoCurvePhase
                                } else if actor.curve_speed > 0.0 {
                                    actor.curve_speed
                                } else {
                                    0.2 // 1.0 / 5.0 seconds
                                };
                                commands.entity(entity).insert(CurveFollower {
                                    curve,
                                    phase: 0.0,
                                    speed,
                                    target_phase: if has_script { 0.0 } else { 1.0 },
                                    wrap_around: if has_script { false } else { !actor.curve_ping_pong },
                                    ping_pong: actor.curve_ping_pong,
                                    look_along_xz: actor.curve_look_xz,
                                    reached_target: has_script, // true so script's first command is picked up
                                });
                                info!("Attached CurveFollower '{}' to {} ({} control points)",
                                    cname, actor.entity_type, pts.len());
                            } else {
                                warn!("Curve '{}' has {} points (need >= 4), skipping",
                                    cname, pts.len());
                            }
                        } else {
                            warn!("Curve '{}' not found in layout.paths for {}",
                                cname, actor.entity_type);
                        }
                    }

                // Attach ScrOni script if actor has a <ScrOni> component
                if let Some(ref filename) = actor.script_filename {
                    if let Some(ref main_script) = actor.script_main {
                        let (script_dir, script_fname) = resolve_script_path(layout_path, filename);
                        match scroni::vm::load_script_file(&script_dir, &script_fname) {
                            Ok(file) => {
                                if let Some(script_def) = file.scripts.iter()
                                    .find(|s| s.name.eq_ignore_ascii_case(main_script))
                                {
                                    let exec = scroni::vm::ScriptExec::new(
                                        script_def.clone(), entity, 0.0,
                                    );
                                    commands.entity(entity).insert(
                                        scroni::vm::ScrOniScript { exec },
                                    );
                                    info!("Attached ScrOni script '{}:{}' to {}",
                                        filename, main_script, actor.entity_type);
                                } else {
                                    warn!("Script '{}' not found in {}/{} (available: {})",
                                        main_script, script_dir, script_fname,
                                        file.scripts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "));
                                }
                            }
                            Err(e) => {
                                warn!("Failed to compile script {}/{}: {}", script_dir, script_fname, e);
                            }
                        }
                    }
                }
            }
            spawned += 1;
        }
    }
    info!("Layout: spawned {} entities, {} creatures, skipped {}", spawned, creatures, skipped);
    if let Some(ref pi) = player_info {
        info!("Layout: player creature found: type={} animator={}", pi.entity_type, pi.animator_type);
    }

    // Insert LayoutPaths resource for potential future use
    if !layout_paths.curves.is_empty() {
        commands.insert_resource(layout_paths);
    }
    
    // Insert TextureCollections resource for the texture_movie_system observer
    commands.insert_resource(texture_collections);

    // Load lights, fog, skyhat
    load_layout_lights(commands, meshes, materials, images, layout_dir);

    player_info
}



/// Parse layout.lights, default.environment, layout.fog, layout.paths, and skyhat.
/// Spawns Bevy light entities, fog resource, paths resource, and skyhat mesh.
fn load_layout_lights(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    layout_dir: &str,
) {
    let layout_path = layout_dir;

    // Parse default.environment for directional/ambient
    let env = parse_environment(layout_path);

    // Parse layout.fog for fog + lighting (used when default.environment is absent)
    let fog_data = parse_layout_fog(layout_path);

    // Parse layout.lights for point lights
    let lights = parse_lights_file(layout_path);

    // layout.paths already parsed in load_layout — no need to re-parse here

    // Load skyhat model if present (sky dome that follows camera)
    load_skyhat(commands, meshes, materials, images, layout_path);

    // Apply lighting from environment file or fog file
    if let Some(ref env) = env {
        commands.spawn((
            DirectionalLight {
                illuminance: 20_000.0,
                shadows_enabled: true,
                color: Color::srgb(env.light_color[0], env.light_color[1], env.light_color[2]),
                ..default()
            },
            Transform::from_xyz(env.light_direction.x, env.light_direction.y, env.light_direction.z)
                .looking_at(Vec3::ZERO, Vec3::Y),
            InGameEntity,
        ));

        commands.spawn((
            AmbientLight {
                color: Color::srgb(env.ambient_color[0], env.ambient_color[1], env.ambient_color[2]),
                brightness: 800.0,
                ..default()
            },
            InGameEntity,
        ));

        // Apply fog from environment file
        if env.fog_end > env.fog_start {
            commands.insert_resource(LayoutFogSettings {
                color: Color::srgb(env.fog_color[0], env.fog_color[1], env.fog_color[2]),
                start: env.fog_start,
                end: env.fog_end,
            });
        }

        info!("Layout: loaded environment (dir_light=({:.2},{:.2},{:.2}), fog_start={:.1}, fog_end={:.1})",
            env.light_direction.x, env.light_direction.y, env.light_direction.z,
            env.fog_start, env.fog_end);
    } else if let Some(ref fog) = fog_data {
        // No environment file — use layout.fog for lighting + fog
        for (i, light) in fog.lights.iter().enumerate() {
            if !light.enabled { continue; }
            let color = Color::srgb(light.color[0], light.color[1], light.color[2]);
            let dir = Vec3::new(-light.direction[0], light.direction[1], -light.direction[2]);
            if i < 2 {
                // First two lights are directional
                commands.spawn((
                    DirectionalLight {
                        illuminance: 20_000.0,
                        shadows_enabled: i == 0,
                        color,
                        ..default()
                    },
                    Transform::from_translation(dir * 100.0).looking_at(Vec3::ZERO, Vec3::Y),
                    InGameEntity,
                ));
            } else {
                // Third light is ambient fill
                commands.spawn((
                    AmbientLight {
                        color,
                        brightness: 800.0,
                        ..default()
                    },
                    InGameEntity,
                ));
            }
        }

        // Apply fog
        if fog.enabled && fog.end > fog.start {
            commands.insert_resource(LayoutFogSettings {
                color: Color::srgb(fog.color[0], fog.color[1], fog.color[2]),
                start: fog.start,
                end: fog.end,
            });
            info!("Layout: loaded fog from layout.fog (start={:.1}, end={:.1})", fog.start, fog.end);
        }

        info!("Layout: loaded lighting from layout.fog ({} lights)", fog.lights.len());
    }

    // Spawn point lights from layout.lights
    let mut point_count = 0;
    let mut ambient_count = 0;
    for light in &lights {
        let pos = light.position;
        let color = Color::srgb(light.color[0], light.color[1], light.color[2]);

        match light.light_type.as_str() {
            "point" => {
                if light.intensity <= 0.0 {
                    continue;
                }
                let range = (light.intensity * 1.0).max(10.0);
                let lumens = light.intensity * 200.0;

                commands.spawn((
                    PointLight {
                        color,
                        intensity: lumens,
                        range,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_translation(pos),
                    InGameEntity,
                ));
                point_count += 1;
            }
            "ambient" => {
                if env.is_none() && fog_data.is_none() {
                    let brightness = light.intensity * 2.0;
                    commands.spawn((
                        AmbientLight {
                            color,
                            brightness,
                            ..default()
                        },
                        InGameEntity,
                    ));
                }
                ambient_count += 1;
            }
            _ => {}
        }
    }
    if point_count > 0 || ambient_count > 0 {
        info!("Layout: loaded {} point lights, {} ambient lights", point_count, ambient_count);
    }

    // Fallback: if no lighting data at all, add defaults
    if env.is_none() && fog_data.is_none() && ambient_count == 0 {
        commands.spawn((
            DirectionalLight {
                illuminance: 20_000.0,
                shadows_enabled: true,
                ..default()
            },
            Transform::from_xyz(50.0, 80.0, 50.0).looking_at(Vec3::ZERO, Vec3::Y),
            InGameEntity,
        ));
        commands.spawn((
            AmbientLight {
                color: Color::WHITE,
                brightness: 800.0,
                ..default()
            },
            InGameEntity,
        ));
        info!("Layout: no environment data, using placeholder lighting");
    }
}


/// Load skyhat.mod from layout directory and spawn as an unlit sky dome.
fn load_skyhat(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    layout_path: &str,
) {
    let skyhat_path = format!("{}/skyhat.mod", layout_path);
    if !crate::vfs::exists("", &skyhat_path) { return; }

    let model = match load_mod_file(&skyhat_path) {
        Some(m) => m,
        None => return,
    };

    // Look for sky texture in the layout directory
    let sky_texture = find_sky_texture(layout_path, images);

    let sub_meshes = build_meshes_by_material(&model);
    if sub_meshes.is_empty() { return; }

    // Spawn parent entity for skyhat
    let parent = commands.spawn((
        Transform::default(),
        Visibility::Visible,
        SkyHat,
        InGameEntity,
    )).id();

    for (mat_idx, mesh) in sub_meshes {
        // Use unlit material — skyhat appears as illuminated sky
        let texture = if let Some(ref tex) = sky_texture {
            Some(tex.clone())
        } else {
            model.materials.get(mat_idx).and_then(|oni_mat| {
                oni_mat.texture_name.as_ref().and_then(|tex_name| {
                    load_tga_texture(layout_path, tex_name, images).map(|(handle, _)| handle)
                })
            })
        };

        let mat = materials.add(StandardMaterial {
            base_color_texture: texture,
            unlit: true,
            cull_mode: None,
            ..default()
        });

        let mesh_handle = meshes.add(mesh);
        let child = commands.spawn((
            Mesh3d(mesh_handle),
            MeshMaterial3d(mat),
            Transform::default(),
        )).id();
        commands.entity(parent).add_child(child);
    }

    info!("Layout: loaded skyhat model from {:?}", skyhat_path);
}

/// Find a sky texture (.tex or .tga) in the layout directory.
fn find_sky_texture(
    layout_path: &str,
    images: &mut ResMut<Assets<Image>>,
) -> Option<Handle<Image>> {
    // Search for any *sky*.tex or *sky*.tga file
    if let Ok(entries) = crate::vfs::read_dir(layout_path) {
        for entry in entries {
            let name = entry.path.split('/').last()
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if name.contains("sky") {
                if name.ends_with(".tex") && !name.ends_with(".tex.tga") {
                    if let Ok(tex_bytes) = crate::vfs::read("", &entry.path) {
                        if let Some((width, height, rgba, _)) = decode_tex(&tex_bytes) {
                            info!("Loaded sky texture: {} ({}x{})", entry.path, width, height);
                            let mut image = Image::new(
                                bevy::render::render_resource::Extent3d { width, height, depth_or_array_layers: 1 },
                                bevy::render::render_resource::TextureDimension::D2,
                                rgba,
                                bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                                default(),
                            );
                            image.sampler = bevy::image::ImageSampler::Descriptor(
                                bevy::image::ImageSamplerDescriptor {
                                    address_mode_u: bevy::image::ImageAddressMode::Repeat,
                                    address_mode_v: bevy::image::ImageAddressMode::Repeat,
                                    ..default()
                                },
                            );
                            return Some(images.add(image));
                        }
                    }
                } else if name.ends_with(".tex.tga") || name.ends_with(".tga") {
                    if let Some((handle, _)) = load_tga_file(&entry.path, images) {
                        info!("Loaded sky texture: {}", entry.path);
                        return Some(handle);
                    }
                }
            }
        }
    }
    None
}

/// Load a .mod file, auto-detecting text v1.10 vs binary v2.10 format.
fn load_mod_file(path: &str) -> Option<Oni2Model> {
    let data = crate::vfs::read("", path).ok()?;
    if data.len() < 14 {
        return None;
    }

    // Check for binary v2.10 header: "version: 2.10\0"
    if data.starts_with(b"version: 2.10\0") {
        info!("Loading binary v2.10 model: {}", path);
        return parse_mod_binary(&data);
    }

    // Otherwise try text v1.10
    let text = String::from_utf8_lossy(&data);
    if text.starts_with("version: 1.10") {
        info!("Loading text v1.10 model: {}", path);
        return Some(parse_mod(&text));
    }

    warn!("Unknown .mod format: {}", path);
    None
}

/// Resolve a ScrOni script filename to a filesystem path.
/// `$name` means layout-local: `<layout_dir>/scripts/<name>.oni`
/// Otherwise the filename is a relative path from the assets root (layout_dir/../..).
fn resolve_script_path(layout_dir: &str, filename: &str) -> (String, String) {
    let add_ext = |name: &str| -> String {
        if name.to_ascii_lowercase().ends_with(".oni") {
            name.to_string()
        } else {
            format!("{}.oni", name)
        }
    };
    
    let normalized = filename.replace('\\', "/");
    
    if let Some(stripped) = normalized.strip_prefix('$') {
        // Layout-local script
        (format!("{}/scripts", layout_dir), add_ext(stripped))
    } else {
        // Relative path from assets root. layout_dir is like "layout/EndlessCity"
        let mut parts: Vec<&str> = layout_dir.split('/').collect();
        parts.pop();
        parts.pop();
        
        let path = if parts.is_empty() {
            normalized
        } else {
            format!("{}/{}", parts.join("/"), normalized)
        };
        
        let mut segments: Vec<&str> = path.split('/').collect();
        let fname = segments.pop().unwrap_or("");
        let dir = segments.join("/");
        
        (dir, add_ext(fname))
    }
}

/// Extract the base="..." attribute from an <actor> tag.
fn extract_xml_base_attr(content: &str) -> Option<String> {
    let idx = content.find("<actor ")?;
    let after = &content[idx..];
    let end = after.find('>')?;
    let tag = &after[..end];
    let base_start = tag.find("base=\"")? + 6;
    let base_end = tag[base_start..].find('"')? + base_start;
    Some(tag[base_start..base_end].to_string())
}

/// Extract value="..." from an XML attribute tag like <TagName value="..."/>
fn extract_xml_attr(content: &str, tag: &str) -> Option<String> {
    let pattern = format!("<{}", tag);
    let idx = content.find(&pattern)?;
    let after = &content[idx..];
    let val_start = after.find("value=\"")? + 7;
    let val_end = after[val_start..].find('"')? + val_start;
    Some(after[val_start..val_end].to_string())
}

/// Parse "x y z" string into Vec3.
fn parse_vec3(s: &str) -> Option<Vec3> {
    let parts: Vec<f32> = s.split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() >= 3 {
        Some(Vec3::new(parts[0], parts[1], parts[2]))
    } else {
        None
    }
}

/// Find the Konoko (player) spawn point from a layout's actor files.
/// Searches for an actor with base="template_konoko" and extracts its position.
pub fn find_konoko_spawn(layout_dir: &str) -> Option<Vec3> {
    let layout_path = layout_dir;
    let actors_path = format!("{}/layout.actors", layout_path);
    let actors_content = crate::vfs::read_to_string("", &actors_path).ok()?;

    for line in actors_content.lines() {
        let actor_name = line.trim();
        if actor_name.is_empty() || actor_name.parse::<u32>().is_ok() {
            continue;
        }

        let xml_path = format!("{}/{}.xml", layout_path, actor_name);
        let content = match crate::vfs::read_to_string("", &xml_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if !content.contains("template_konoko") {
            continue;
        }

        let position = extract_xml_attr(&content, "Position")
            .and_then(|s| parse_vec3(&s))
            .unwrap_or(Vec3::ZERO);

        // Convert from left-handed to right-handed at parse boundary
        let bevy_pos = Vec3::new(-position.x, position.y, -position.z);
        info!("Found Konoko spawn at {:?} → bevy {:?}", position, bevy_pos);
        return Some(bevy_pos);
    }

    None
}

/// Debug component storing the actual bound geometry for gizmo rendering.
#[derive(Component)]
pub struct Oni2DebugBounds {
    pub vertices: Vec<Vec3>, // bound vertices in local space (Z-negated)
    pub edges: Vec<[u32; 2]>,
}

/// Debug component storing skeleton bone positions and parent links for gizmo rendering.
#[derive(Component)]
pub struct Oni2DebugSkeleton {
    pub positions: Vec<Vec3>,       // bone world positions (Z-negated for Bevy)
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

    let anims_path = if crate::vfs::exists("", &tune_anims) { tune_anims } else { entity_anims };

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
    let mode = if point_cloud.0 { "POINT CLOUD" } else { "TRIANGLES" };
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
                let mat_handle = model_data.material_handles.get(mat_idx)
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
    query: Query<(&Transform, &Oni2DebugSkeleton)>,
    mut gizmos: Gizmos,
    visible: Res<DebugSkeletonVisible>,
) {
    if !visible.0 {
        return;
    }
    let bone_color = Color::srgb(1.0, 1.0, 0.0);
    let joint_color = Color::srgb(1.0, 0.3, 0.0);

    for (transform, skel) in &query {
        // Draw lines from each bone to its parent
        for (i, parent_idx) in skel.parent_indices.iter().enumerate() {
            let pos = skel.positions[i];
            let world_pos = transform.transform_point(pos);

            // Draw joint sphere
            gizmos.sphere(Isometry3d::from_translation(world_pos), 0.008, joint_color);

            // Draw line to parent
            if let Some(pi) = parent_idx {
                let parent_pos = skel.positions[*pi];
                let world_parent = transform.transform_point(parent_pos);
                gizmos.line(world_pos, world_parent, bone_color);
            }
        }
    }
}

/// Linearly interpolate between two animation frames, storing the result in `state.current_frame`.
pub fn frame_lerp(state: &mut Oni2AnimState, idx_a: usize, idx_b: usize, t: f32) {
    // Destructure to borrow `current_frame` mutably and `anim` immutably at the same time
    let Oni2AnimState { anim, current_frame, .. } = state;

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
    mut anim_query: Query<(Entity, &mut Oni2AnimState, Option<&mut Oni2DebugSkeleton>, Option<&CreatureRenderOffset>)>,
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
            anim_state.current_time += time.delta_secs() * anim_state.fps * anim_state.speed_multiplier;
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
            ds.positions = bone_transforms.iter()
                .map(|(_, pos)| Vec3::new(-pos.x, pos.y, -pos.z))
                .collect();
        }
    }
}

/// Render offset for creatures: compensates for physics capsule center being above
/// ground and for model facing direction (Oni2 models face -Z in local space,
/// Z-negate mirror makes them face the camera without this correction).
#[derive(Component)]
pub struct CreatureRenderOffset {
    /// Y offset to align feet with capsule bottom (negative = down)
    pub y_offset: f32,
    /// Rotation applied to mesh children (e.g. 180° Y to fix facing)
    pub facing: Quat,
}

/// Marker for creatures that need to be snapped to the ground after physics initializes.
/// Stores the original intended spawn position so we can probe around it.
#[derive(Component)]
pub struct NeedsGroundSnap {
    pub origin: Vec3,
    /// How many physics frames to wait before probing (colliders need 1 frame to register).
    pub wait_frames: u32,
}

/// System that probes for solid ground beneath newly spawned creatures and teleports them
/// to a safe position. Tries a spiral pattern within 5m of the spawn origin.
pub fn ground_snap_system(
    mut commands: Commands,
    mut query: Query<(Entity, &mut Transform, &mut NeedsGroundSnap)>,
    spatial_query: SpatialQuery,
) {
    for (entity, mut transform, mut snap) in &mut query {
        // Wait for colliders to be registered in the physics pipeline
        if snap.wait_frames > 0 {
            snap.wait_frames -= 1;
            continue;
        }

        let origin = snap.origin;
        let capsule_half_height = 1.0_f32; // capsule(0.4, 1.2) → total height ~2.0

        // Probe positions: center first, then a spiral out to 5m
        let probe_offsets = [
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(-2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::new(0.0, 0.0, -2.0),
            Vec3::new(1.5, 0.0, 1.5),
            Vec3::new(-1.5, 0.0, 1.5),
            Vec3::new(1.5, 0.0, -1.5),
            Vec3::new(-1.5, 0.0, -1.5),
            Vec3::new(3.5, 0.0, 0.0),
            Vec3::new(-3.5, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 3.5),
            Vec3::new(0.0, 0.0, -3.5),
            Vec3::new(2.5, 0.0, 2.5),
            Vec3::new(-2.5, 0.0, 2.5),
            Vec3::new(2.5, 0.0, -2.5),
            Vec3::new(-2.5, 0.0, -2.5),
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::new(-5.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::new(0.0, 0.0, -5.0),
        ];

        let filter = SpatialQueryFilter::from_excluded_entities([entity]);
        let mut best_hit: Option<(Vec3, f32)> = None;

        for offset in &probe_offsets {
            let probe_pos = origin + *offset;
            // Cast from well above down to well below
            let ray_origin = Vec3::new(probe_pos.x, origin.y + 50.0, probe_pos.z);

            if let Some(hit) = spatial_query.cast_ray(
                ray_origin,
                Dir3::NEG_Y,
                100.0,
                true,
                &filter,
            ) {
                let ground_y = ray_origin.y - hit.distance;
                let dist_from_origin = offset.length();

                // Prefer the hit closest to the original spawn point
                if best_hit.is_none() || dist_from_origin < best_hit.unwrap().1 {
                    best_hit = Some((Vec3::new(probe_pos.x, ground_y, probe_pos.z), dist_from_origin));
                    // If center probe hit, use it immediately
                    if dist_from_origin < 0.01 {
                        break;
                    }
                }
            }
        }

        if let Some((ground_pos, _)) = best_hit {
            // Place capsule center above ground (capsule half-height above ground surface)
            transform.translation = Vec3::new(
                ground_pos.x,
                ground_pos.y + capsule_half_height + 0.1, // small buffer
                ground_pos.z,
            );
            info!(
                "Ground-snapped creature {:?} from ({:.1}, {:.1}, {:.1}) to ({:.1}, {:.1}, {:.1})",
                entity,
                origin.x, origin.y, origin.z,
                transform.translation.x, transform.translation.y, transform.translation.z,
            );
        } else {
            // No ground found — spawn above origin and hope for the best
            transform.translation = Vec3::new(origin.x, origin.y + 2.0, origin.z);
            warn!("No ground found for creature {:?} near ({:.1}, {:.1}, {:.1}), spawning above origin",
                entity, origin.x, origin.y, origin.z);
        }

        // Zero out velocity so they don't keep falling from the old position
        commands.entity(entity)
            .remove::<NeedsGroundSnap>()
            .insert(LinearVelocity::default());
    }
}

/// Tracks which movement animation is currently playing to avoid re-triggering.
#[derive(Component, Default, PartialEq, Eq)]
pub enum CreatureMovementAnim {
    #[default]
    Stand,
    Walk,
    Run,
}

/// Pick stand/walk/run animations based on horizontal velocity for all creatures.
pub fn creature_movement_anim_system(
    mut creatures: Query<(&Oni2AnimLibrary, &mut Oni2AnimState, &mut CreatureMovementAnim, &LinearVelocity)>,
) {
    const WALK_THRESHOLD: f32 = 0.5;
    const RUN_THRESHOLD: f32 = 3.0;

    for (library, mut anim_state, mut move_anim, vel) in &mut creatures {
        let horiz_speed = Vec2::new(vel.x, vel.z).length();

        let desired = if horiz_speed < WALK_THRESHOLD {
            CreatureMovementAnim::Stand
        } else if horiz_speed < RUN_THRESHOLD {
            CreatureMovementAnim::Walk
        } else {
            CreatureMovementAnim::Run
        };

        if *move_anim != desired {
            let alias = match desired {
                CreatureMovementAnim::Stand => "ANIMNAV_STAND",
                CreatureMovementAnim::Walk => "ANIMNAV_WLK_FORWARD",
                CreatureMovementAnim::Run => "ANIMNAV_RUN_FORWARD",
            };
            if library.play(alias, &mut anim_state) {
                *move_anim = desired;
                info!("Creature movement animation changed to: {}", alias);
            } else {
                warn!("Creature missing expected movement animation alias: {}", alias);
            }
        }
    }
}

/// Spawn a specific .mod file as a mesh entity (no skeleton/physics, for inspection).
pub fn spawn_mod_file(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    mod_path: &str,
    entity_dir: &str,
    position: Vec3,
    label: &str,
) -> Option<Entity> {
    let path = mod_path;
    let model = load_mod_file(path)?;
    let dir = entity_dir;

    let sub_meshes = build_meshes_by_material(&model);

    let bevy_materials: Vec<Handle<StandardMaterial>> = model.materials.iter().map(|oni_mat| {
        let (texture_handle, has_alpha) = match oni_mat.texture_name.as_ref() {
            Some(tex_name) => match load_tga_texture(dir, tex_name, images) {
                Some((h, alpha)) => (Some(h), alpha),
                None => (None, false)
            },
            None => (None, false)
        };
        let diffuse = Color::srgb(oni_mat.diffuse[0], oni_mat.diffuse[1], oni_mat.diffuse[2]);
        materials.add(StandardMaterial {
            base_color: if texture_handle.is_some() { Color::WHITE } else { diffuse },
            base_color_texture: texture_handle,
            cull_mode: None,
            alpha_mode: if has_alpha { AlphaMode::Blend } else { AlphaMode::Opaque },
            ..default()
        })
    }).collect();

    let fallback_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.6, 0.6, 0.6),
        cull_mode: None,
        ..default()
    });

    // Load skeleton for debug display
    let skel_path_candidates = [
        // Try common skeleton names based on entity dir name
        format!("{}/{}.skel", dir, label.split('_').next().unwrap_or(label)),
    ];
    let debug_skeleton = skel_path_candidates.iter().find_map(|sp| {
        let content = crate::vfs::read_to_string("", sp).ok()?;
        let skel = parse_skel(&content);
        Some(Oni2DebugSkeleton {
            positions: skel.positions.iter()
                .map(|p| Vec3::new(-p[0], p[1], -p[2]))
                .collect(),
            parent_indices: skel.parent_indices.clone(),
            names: skel.names.clone(),
        })
    });

    let transform = Transform::from_translation(position);
    let mut ec = commands.spawn((
        transform,
        Visibility::Visible,
        Oni2Entity { name: label.to_string() },
        Name::new(label.to_string()),
        Oni2ModelData {
            model: model.clone(),
            material_handles: bevy_materials.clone(),
            fallback_material: fallback_mat.clone(),
        },
        InGameEntity,
    ));
    if let Some(ds) = debug_skeleton {
        ec.insert(ds);
    }

    let parent = ec.with_children(|parent| {
        for (mat_idx, mesh) in sub_meshes {
            let mat_handle = bevy_materials.get(mat_idx)
                .cloned()
                .unwrap_or_else(|| fallback_mat.clone());
            parent.spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat_handle),
                Transform::default(),
            ));
        }
    }).id();

    info!("Spawned model '{}' from {:?} at {:?}", label, mod_path, position);
    Some(parent)
}

/// Load an ONI2 entity from a directory and spawn it in the world (no rotation).
pub fn spawn_oni2_entity(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    skinned_mesh_ibp: &mut ResMut<Assets<SkinnedMeshInverseBindposes>>,
    entity_dir: &str,
    position: Vec3,
    name: &str,
) -> Option<Entity> {
    spawn_oni2_entity_with_rotation(commands, meshes, materials, images, skinned_mesh_ibp, entity_dir, position, Quat::IDENTITY, name, None, None)
}

/// Load an ONI2 entity from a directory and spawn it with position and rotation.
/// If `anim_path` is provided, load that specific anim file instead of the default.
pub fn spawn_oni2_entity_with_rotation(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    skinned_mesh_ibp: &mut ResMut<Assets<SkinnedMeshInverseBindposes>>,
    entity_dir: &str,
    position: Vec3,
    rotation: Quat,
    name: &str,
    anim_path: Option<&str>,
    entity_type_name: Option<&str>,
) -> Option<Entity> {
    let dir = entity_dir;

    // Read Entity.type
    let type_path = format!("{}/Entity.type", dir);
    let type_content = crate::vfs::read_to_string("", &type_path).ok()?;
    let entity_type = parse_entity_type(&type_content);

    // Load skeleton if present
    let skeleton = match entity_type.skel_file.as_ref() {
        Some(skel_name) => {
            let skel_path = format!("{}/{}", dir, skel_name);
            match crate::vfs::read_to_string("", &skel_path) {
                Ok(skel_content) => {
                    let skel = parse_skel(&skel_content);
                    info!("Loaded skeleton: {} bones from {}", skel.positions.len(), skel_path);
                    Some(skel)
                }
                Err(e) => {
                    warn!("Entity '{}' references skeleton '{}', but it could not be read: {}", name, skel_path, e);
                    None
                }
            }
        }
        None => {
            info!("Entity '{}' has no skeleton file defined in {}/Entity.type; skipping animation features.", name, dir);
            None
        }
    };

    // Read and parse the .mod file
    // Prefer win32 binary LOD model (world-space vertices, correct bone count).
    // Fall back to standard binary LOD, then text base model.
    let mut loaded_anim: Option<Oni2Animation> = None;
    let model = if let Some(ref model_file) = entity_type.model_file {
        let mut m: Option<Oni2Model> = None;

        // Try standard (PS2) LOD model first (bone-local vertices, supports animation)
        let mut mod_path = format!("{}/{}", dir, model_file);
        if !crate::vfs::exists("", &mod_path) {
            // Check for LOD 0 fallback (e.g. FinitePlane_LODs.mod -> FinitePlane_LODs0.mod)
            if model_file.ends_with(".mod") {
                let fallback = model_file.replace(".mod", "0.mod");
                let fallback_path = format!("{}/{}", dir, fallback);
                if crate::vfs::exists("", &fallback_path) {
                    mod_path = fallback_path;
                }
            }
        }

        if crate::vfs::exists("", &mod_path) {
            if let Some(mut model) = load_mod_file(&mod_path) {
                // PS2 binary v2.10 is bone-local despite using the same format as win32
                model.world_space_verts = false;
                info!("Using PS2 model {}", mod_path);
                m = Some(model);
            }
        }

        // Fallback: text base model
        if m.is_none() {
            let base_mod = format!("{}/{}.mod", dir, name);
            if crate::vfs::exists("", &base_mod) {
                if let Some(text_model) = load_mod_file(&base_mod) {
                    info!("Falling back to text model {}", base_mod);
                    m = Some(text_model);
                }
            }
        }

        // Last resort: win32 binary LOD model (world-space vertices)
        if m.is_none() {
            let win32_mod = format!("{}/win32_{}", dir, model_file);
            if crate::vfs::exists("", &win32_mod) {
                if let Some(model) = load_mod_file(&win32_mod) {
                    // win32 models have world-space verts — world_space_verts stays true
                    info!("Using win32 model {} (world-space vertices)", win32_mod);
                    m = Some(model);
                }
            }
        }

        // Convert world-space vertices to bone-local using skeleton bind pose.
        // This normalizes all model formats (win32, PS2, ASCII) to bone-local.
        if let (Some(model), Some(skel)) = (&mut m, &skeleton) {
            if model.world_space_verts {
                convert_world_to_bone_local(model, skel);
                info!("Converted world-space vertices to bone-local ({} verts)", model.vertices.len());
            }

            // Load explicit animation if provided (e.g. TestAnim scene)
            if let Some(ap) = anim_path {
                if let Ok(anim_data) = crate::vfs::read("", ap) {
                    if let Some(anim) = parse_anim(&anim_data) {
                        info!("Loaded animation from {}: {} frames", ap, anim.num_frames);
                        loaded_anim = Some(anim);
                    }
                }
            }
        }

        m
    } else {
        None
    };

    // Read and parse the .bnd file
    let bound = {
        let bnd_path = format!("{}/Bound.bnd", dir);
        if crate::vfs::exists("", &bnd_path) {
            let bnd_content = crate::vfs::read_to_string("", &bnd_path).ok()?;
            Some(parse_bound(&bnd_content))
        } else {
            None
        }
    };

    // Bound vertices already in Bevy coordinates (Z negated at parse time)
    let bound_verts: Vec<Vec3> = bound
        .as_ref()
        .map(|b| b.vertices.iter().map(|v| Vec3::new(v[0], v[1], v[2])).collect())
        .unwrap_or_default();
    let bound_edges: Vec<[u32; 2]> = bound
        .as_ref()
        .map(|b| b.edges.clone())
        .unwrap_or_default();

    // Build trimesh collider from bound quads (thin shell, not solid volume).
    // Convex hull fills the interior and blocks raycasts/physics through the
    // bounding volume — trimesh only collides at the actual faces.
    let bound_quads: Vec<[u32; 4]> = bound
        .as_ref()
        .map(|b| b.quads.clone())
        .unwrap_or_default();

    let collider = if !bound_verts.is_empty() && !bound_quads.is_empty() {
        // Split quads into triangles
        let mut tri_indices: Vec<[u32; 3]> = Vec::with_capacity(bound_quads.len() * 2);
        for q in &bound_quads {
            tri_indices.push([q[0], q[1], q[2]]);
            tri_indices.push([q[0], q[2], q[3]]);
        }
        Collider::try_trimesh(bound_verts.clone(), tri_indices)
            .unwrap_or_else(|_| {
                Collider::convex_hull(bound_verts.clone())
                    .unwrap_or_else(|| Collider::cuboid(1.0, 1.0, 1.0))
            })
    } else if !bound_verts.is_empty() {
        // No quads — fall back to convex hull
        Collider::convex_hull(bound_verts.clone())
            .unwrap_or_else(|| Collider::cuboid(1.0, 1.0, 1.0))
    } else {
        Collider::cuboid(1.0, 1.0, 1.0)
    };

    let debug_bounds = Oni2DebugBounds {
        vertices: bound_verts,
        edges: bound_edges,
    };

    // Build debug skeleton component if skeleton was loaded
    // Use animated bone positions from the model if available, else bind pose
    let debug_skeleton = skeleton.as_ref().map(|skel| {
        let positions = if let Some(ref m) = model {
            if !m.bone_world_positions.is_empty() {
                m.bone_world_positions.iter()
                    .map(|p| Vec3::new(-p[0], p[1], -p[2]))
                    .collect()
            } else {
                skel.positions.iter()
                    .map(|p| Vec3::new(-p[0], p[1], -p[2]))
                    .collect()
            }
        } else {
            skel.positions.iter()
                .map(|p| Vec3::new(-p[0], p[1], -p[2]))
                .collect()
        };
        Oni2DebugSkeleton {
            positions,
            parent_indices: skel.parent_indices.clone(),
            names: skel.names.clone(),
        }
    });

    // Load explicitly requested animation, if any
    let loaded_anim = anim_path.and_then(|path| {
        crate::vfs::read("", path).ok()
            .and_then(|data| parse_anim(&data))
    });

    // Load animation library from .anims + .apkg packages if skeleton available
    let library = if let Some(ref skel) = skeleton {
        let lib = load_anim_library(
            entity_dir,
            entity_type_name.unwrap_or(name),
            skel,
        );
        if !lib.anims.is_empty() {
            Some(lib)
        } else {
            None
        }
    } else {
        None
    };

    let transform = Transform::from_translation(position).with_rotation(rotation);

    let Some(model) = model else {
        // No model — spawn collider-only placeholder
        let mut ec = commands.spawn((
            transform,
            RigidBody::Static,
            collider,
            Oni2Entity { name: name.to_string() },
            Name::new(name.to_string()),
            debug_bounds,
            InGameEntity,
        ));
        if let Some(ds) = debug_skeleton {
            ec.insert(ds);
        }
        return Some(ec.id());
    };

    // Determine if this entity is skinned (has skeleton + animation)
    let default_anim = loaded_anim.or_else(|| {
        library.as_ref().and_then(|lib| lib.anims.values().next().cloned())
    });
    
    let use_gpu_skinning = skeleton.is_some() && default_anim.is_some();

    // Build meshes: skinned (with joint attributes) or static
    let sub_meshes = if use_gpu_skinning {
        build_skinned_meshes_by_material(&model, skeleton.as_ref().unwrap())
    } else {
        // Static entities still need CPU-positioned vertices for initial pose
        // Set bind-pose bone positions for the one-time build
        if let Some(ref skel) = skeleton {
            let mut m = model.clone();
            m.bone_world_positions = skel.positions.clone();
            m.bone_rotations = vec![[0.0, 0.0, 0.0, 1.0]; skel.positions.len()];
            build_meshes_by_material(&m)
        } else {
            build_meshes_by_material(&model)
        }
    };

    // Compute inverse bind poses for GPU skinning
    let ibp_handle = if use_gpu_skinning {
        let inverse_bind_poses = compute_inverse_bind_poses(skeleton.as_ref().unwrap());
        Some(skinned_mesh_ibp.add(SkinnedMeshInverseBindposes::from(inverse_bind_poses)))
    } else {
        None
    };

    // Load textures and create Bevy materials for each ONI2 material
    let bevy_materials: Vec<Handle<StandardMaterial>> = model.materials.iter().map(|oni_mat| {
        let (texture_handle, has_alpha) = match oni_mat.texture_name.as_ref() {
            Some(tex_name) => match load_tga_texture(dir, tex_name, images) {
                Some((h, alpha)) => (Some(h), alpha),
                None => (None, false)
            },
            None => (None, false)
        };

        let diffuse = Color::srgb(oni_mat.diffuse[0], oni_mat.diffuse[1], oni_mat.diffuse[2]);

        materials.add(StandardMaterial {
            base_color: if texture_handle.is_some() { Color::WHITE } else { diffuse },
            base_color_texture: texture_handle,
            cull_mode: None,
            alpha_mode: if has_alpha { AlphaMode::Blend } else { AlphaMode::Opaque },
            ..default()
        })
    }).collect();

    // Fallback material if no materials defined
    let fallback_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.5, 0.5, 0.5),
        ..default()
    });

    // Spawn parent entity with transform, convex hull collider directly on parent
    let mut ec = commands.spawn((
        transform,
        Visibility::Visible,
        RigidBody::Static,
        collider,
        Oni2Entity { name: name.to_string() },
        Name::new(name.to_string()),
        debug_bounds,
        InGameEntity,
    ));
    if let Some(ds) = debug_skeleton {
        ec.insert(ds);
    }

    let parent_entity = ec.id();

    // Spawn joint entities for GPU skinning (flat hierarchy, all children of parent)
    let joint_entities = if use_gpu_skinning {
        let skel = skeleton.as_ref().unwrap();
        let num_bones = skel.positions.len();
        let mut joints = Vec::with_capacity(num_bones);
        for _ in 0..num_bones {
            let joint = commands.spawn((
                Transform::IDENTITY,
                Visibility::Hidden,
            )).id();
            commands.entity(parent_entity).add_child(joint);
            joints.push(joint);
        }
        joints
    } else {
        Vec::new()
    };

    // Attach animation state and library when skeleton is available
    if let Some(ref skel) = skeleton {
        if let Some(anim) = default_anim {
            let looping = anim.is_loop;
            commands.entity(parent_entity).insert(Oni2AnimState {
                anim,
                skeleton: skel.clone(),
                current_time: 0.0,
                fps: 20.0,
                paused: false,
                looping,
                speed_multiplier: 1.0,
                pending_step: 0,
                last_rendered_time: -1.0, // force first render
                joint_entities: joint_entities.clone(),
                base_rotation: rotation,
                current_frame: Vec::new(),
            });
        } else if !joint_entities.is_empty() {
            // No animation yet, but we have a skeleton — create a paused AnimState
            // so PlayAnimation from scripts can populate it later
            commands.entity(parent_entity).insert(Oni2AnimState {
                anim: Oni2Animation::default(),
                skeleton: skel.clone(),
                current_time: 0.0,
                fps: 20.0,
                paused: true,
                looping: false,
                speed_multiplier: 1.0,
                pending_step: 0,
                last_rendered_time: -1.0,
                joint_entities: joint_entities.clone(),
                base_rotation: rotation,
                current_frame: Vec::new(),
            });
        }

        if let Some(library) = library {
            commands.entity(parent_entity).insert(library);
        }
    }

    // Spawn mesh children per material
    for (mat_idx, mesh) in sub_meshes {
        let mat_handle = bevy_materials.get(mat_idx)
            .cloned()
            .unwrap_or_else(|| fallback_mat.clone());

        let mesh_handle = meshes.add(mesh);
        let mut mesh_ec = commands.spawn((
            Mesh3d(mesh_handle),
            MeshMaterial3d(mat_handle),
            Transform::default(),
        ));

        // Attach SkinnedMesh component for GPU skinning
        if let Some(ref ibp) = ibp_handle {
            mesh_ec.insert(SkinnedMesh {
                inverse_bindposes: ibp.clone(),
                joints: joint_entities.clone(),
            });
        }

        let mesh_entity = mesh_ec.id();
        commands.entity(parent_entity).add_child(mesh_entity);
    }

    Some(parent_entity)
}

/// Spawn a creature (animated entity) from the layout.
/// All creatures get a physics capsule + animation library.
/// Returns the entity so the caller can attach player or AI components.
fn spawn_oni2_creature(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    skinned_mesh_ibp: &mut ResMut<Assets<SkinnedMeshInverseBindposes>>,
    entity_dir: &str,
    position: Vec3,
    rotation: Quat,
    actor_name: &str,
    entity_type: &str,
    animator_type: Option<&str>,
    assets_base: &str,
) -> Option<Entity> {
    let anim_name = animator_type.unwrap_or(entity_type);

    let entity_base = format!("{}/Entity", assets_base);
    let anim_entity_dir = format!("{}/{}", entity_base, anim_name);

    // Spawn above intended position; ground_snap_system will find solid ground
    let spawn_position = position + Vec3::Y * 2.0;

    // Spawn the visual entity
    let entity = spawn_oni2_entity_with_rotation(
        commands, meshes, materials, images, skinned_mesh_ibp,
        entity_dir,
        spawn_position,
        rotation,
        actor_name,
        None,
        Some(anim_name),
    )?;

    // Every creature gets a physics capsule + render offset + ground snap
    commands.entity(entity).insert((
        RigidBody::Dynamic,
        Collider::capsule(0.4, 1.2),
        LockedAxes::new()
            .lock_rotation_x()
            .lock_rotation_y()
            .lock_rotation_z(),
        LinearVelocity::default(),
        ShapeCaster::new(
            Collider::sphere(0.35),
            Vec3::NEG_Y * 0.5,
            Quat::default(),
            Dir3::NEG_Y,
        )
        .with_max_distance(0.3),
        // Offset mesh down to align feet with capsule bottom
        CreatureRenderOffset {
            y_offset: -1.0,
            facing: Quat::IDENTITY,
        },
        // Deferred ground probe — wait for physics colliders to register
        NeedsGroundSnap {
            origin: position,
            wait_frames: 3,
        },
    ));

    // Load animation library
    let skel_path = format!("{}/{}.skel", entity_dir, entity_type);
    let skel_data = crate::vfs::read_to_string("", &skel_path).ok();
    let skeleton = skel_data.map(|s| parse_skel(&s));

    if let Some(ref skel) = skeleton {
        let library = load_anim_library(
            &anim_entity_dir,
            anim_name,
            skel,
        );
        if !library.anims.is_empty() {
            commands.entity(entity).insert(library);
            commands.entity(entity).insert(CreatureMovementAnim::Run);
        }
    }

    Some(entity)
}

/// Load texture for an entity: tries .tex (native), then .tex.tga (pre-converted).
fn load_tga_file(path: &str, images: &mut ResMut<Assets<Image>>) -> Option<(Handle<Image>, bool)> {
    let bytes = crate::vfs::read("", path).ok()?;
    let dyn_image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Tga).ok()?;
    let rgba = dyn_image.to_rgba8();
    let (width, height) = rgba.dimensions();

    let mut has_alpha = false;
    for p in rgba.pixels() {
        if p[3] < 255 {
            has_alpha = true;
            break;
        }
    }

    let mut image = Image::new(
        bevy::render::render_resource::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        rgba.into_raw(),
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        default(),
    );
    image.sampler = bevy::image::ImageSampler::Descriptor(
        bevy::image::ImageSamplerDescriptor {
            address_mode_u: bevy::image::ImageAddressMode::Repeat,
            address_mode_v: bevy::image::ImageAddressMode::Repeat,
            ..default()
        },
    );
    Some((images.add(image), has_alpha))
}

/// Setup a minimal scene for animation preview (testanim mode).
/// Derives entity name from the anim filename prefix before first `_`.
pub fn setup_testanim_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut skinned_mesh_ibp: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    testanim: Res<TestAnimMode>,
) {
    let anim_file = &testanim.0;

    // Derive entity name from filename prefix before first '_'
    let file_stem = anim_file.split('/').last().unwrap_or("").split('.').next().unwrap_or("");
    let entity_name = file_stem.split('_').next().unwrap_or(file_stem);
    let entity_dir = format!("Entity/{}", entity_name);

    info!("TestAnim: file={}, entity={}, dir={}", anim_file, entity_name, entity_dir);

    let scoped = InGameEntity;

    // Spawn entity with specific anim
    spawn_oni2_entity_with_rotation(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        &mut skinned_mesh_ibp,
        &entity_dir,
        Vec3::new(0.0, 0.0, 0.0),
        Quat::IDENTITY,
        entity_name,
        Some(anim_file),
        Some(entity_name),
    );

    // Directional light
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(50.0, 50.0, 50.0).looking_at(Vec3::ZERO, Vec3::Y),
        scoped.clone(),
    ));

    // Ambient light
    commands.spawn((
        AmbientLight {
            color: Color::WHITE,
            brightness: 500.0,
            affects_lightmapped_meshes: false,
        },
        scoped.clone(),
    ));

    // Orbit camera centered on character
    let orbit = OrbitCamera {
        target: Vec3::new(0.0, 0.8, 0.0),
        distance: 3.0,
        yaw: 0.0,
        pitch: 0.15,
    };
    let cam_pos = orbit_camera_position(&orbit);
    commands.spawn((
        Camera3d::default(),
        Transform::from_translation(cam_pos).looking_at(orbit.target, Vec3::Y),
        orbit,
        scoped.clone(),
    ));

    // HUD text overlay
    commands.spawn((
        Text::new("Frame: 0/0  FPS: 20  PLAYING  LOOP"),
        TextFont {
            font_size: 20.0,
            ..default()
        },
        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(10.0),
            top: Val::Px(10.0),
            ..default()
        },
        TestAnimHud,
        scoped,
    ));
}

/// Handle testanim playback input (pause, step, fps, loop).
pub fn testanim_input_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut Oni2AnimState>,
) {
    for mut anim in &mut query {
        // Space: toggle pause/play
        if keyboard.just_pressed(KeyCode::Space) {
            anim.paused = !anim.paused;
        }
        // Right arrow: step forward (when paused)
        if keyboard.just_pressed(KeyCode::ArrowRight) && anim.paused {
            anim.pending_step = 1;
        }
        // Left arrow: step backward (when paused)
        if keyboard.just_pressed(KeyCode::ArrowLeft) && anim.paused {
            anim.pending_step = -1;
        }
        // Up arrow: increase FPS
        if keyboard.just_pressed(KeyCode::ArrowUp) {
            anim.fps = (anim.fps + 5.0).min(120.0);
        }
        // Down arrow: decrease FPS
        if keyboard.just_pressed(KeyCode::ArrowDown) {
            anim.fps = (anim.fps - 5.0).max(1.0);
        }
        // L: toggle loop
        if keyboard.just_pressed(KeyCode::KeyL) {
            anim.looping = !anim.looping;
        }
    }
}

/// Compute camera world position from orbit parameters.
fn orbit_camera_position(orbit: &OrbitCamera) -> Vec3 {
    let x = orbit.distance * orbit.pitch.cos() * orbit.yaw.sin();
    let y = orbit.distance * orbit.pitch.sin();
    let z = orbit.distance * orbit.pitch.cos() * orbit.yaw.cos();
    orbit.target + Vec3::new(x, y, z)
}

/// Orbit camera: right-drag to rotate, scroll to zoom.
pub fn orbit_camera_system(
    mouse: Res<ButtonInput<MouseButton>>,
    mut motion_reader: MessageReader<bevy::input::mouse::MouseMotion>,
    mut scroll_reader: MessageReader<bevy::input::mouse::MouseWheel>,
    mut query: Query<(&mut OrbitCamera, &mut Transform)>,
) {
    let mut delta = Vec2::ZERO;
    for ev in motion_reader.read() {
        delta += ev.delta;
    }
    let mut scroll: f32 = 0.0;
    for ev in scroll_reader.read() {
        scroll += ev.y;
    }

    for (mut orbit, mut transform) in &mut query {
        // Right mouse drag to rotate
        if mouse.pressed(MouseButton::Right) {
            orbit.yaw += delta.x * 0.005;
            orbit.pitch += delta.y * 0.005;
            orbit.pitch = orbit.pitch.clamp(-1.4, 1.4);
        }

        // Scroll to zoom
        if scroll.abs() > 0.01 {
            orbit.distance = (orbit.distance - scroll * 0.3).clamp(0.5, 20.0);
        }

        let pos = orbit_camera_position(&orbit);
        *transform = Transform::from_translation(pos).looking_at(orbit.target, Vec3::Y);
    }
}

/// Update the testanim HUD text with current animation state.
pub fn update_testanim_hud(
    anim_query: Query<&Oni2AnimState>,
    mut hud_query: Query<&mut Text, With<TestAnimHud>>,
) {
    let Ok(anim) = anim_query.single() else {
        return;
    };
    let Ok(mut text) = hud_query.single_mut() else {
        return;
    };

    let frame = anim.current_time as usize;
    let total = anim.anim.num_frames;
    let pause_str = if anim.paused { "PAUSED" } else { "PLAYING" };
    let loop_str = if anim.looping { "LOOP" } else { "ONCE" };

    *text = Text::new(format!(
        "Frame: {}/{}  FPS: {}  {}  {}",
        frame, total, anim.fps as u32, pause_str, loop_str
    ));
}
