use super::*;
use crate::oni2_loader::parsers::texture::load_tga_texture;
use crate::oni2_loader::utils::bone::compute_inverse_bind_poses;

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
    pub capsule_half_height: f32,
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
        let capsule_half_height = snap.capsule_half_height;

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
            // Cast from slightly above down to well below
            let ray_origin = Vec3::new(probe_pos.x, origin.y + 1.0, probe_pos.z);

            if let Some(hit) = spatial_query.cast_ray(ray_origin, Dir3::NEG_Y, 100.0, true, &filter)
            {
                let ground_y = ray_origin.y - hit.distance;
                let dist_from_origin = offset.length();

                // Prefer the hit closest to the original spawn point
                if best_hit.is_none() || dist_from_origin < best_hit.unwrap().1 {
                    best_hit = Some((
                        Vec3::new(probe_pos.x, ground_y, probe_pos.z),
                        dist_from_origin,
                    ));
                    // If center probe hit, use it immediately
                    if dist_from_origin < 0.01 {
                        break;
                    }
                }
            }
        }

        if let Some((ground_pos, _)) = best_hit {
            // Place capsule center above ground (capsule half-height above ground surface)
            let final_pos = Vec3::new(
                ground_pos.x,
                ground_pos.y + capsule_half_height + 0.1, // small buffer
                ground_pos.z,
            );
            transform.translation = final_pos;
            commands.entity(entity).insert(avian3d::prelude::Position(final_pos));
            info!(
                "Ground-snapped creature {:?} from ({:.1}, {:.1}, {:.1}) to ({:.1}, {:.1}, {:.1})",
                entity,
                origin.x,
                origin.y,
                origin.z,
                final_pos.x,
                final_pos.y,
                final_pos.z,
            );
        } else {
            // No ground found — spawn above origin and hope for the best
            let final_pos = Vec3::new(origin.x, origin.y + 2.0, origin.z);
            transform.translation = final_pos;
            commands.entity(entity).insert(avian3d::prelude::Position(final_pos));
            warn!(
                "No ground found for creature {:?} near ({:.1}, {:.1}, {:.1}), spawning above origin",
                entity, origin.x, origin.y, origin.z
            );
        }

        // Zero out velocity so they don't keep falling from the old position
        commands
            .entity(entity)
            .remove::<NeedsGroundSnap>()
            .insert(LinearVelocity::default());
    }
}

// ---------------------------------------------------------------------------
// Moving platform / local frame of reference
// ---------------------------------------------------------------------------

/// Tracks which kinematic platform this entity is riding so we can apply
/// the platform's per-frame delta transform before the physics step.
///
/// This implements the "delta buffer" pattern: the pre-physics system reads
/// the platform's current Transform, diffs it against last frame, and injects
/// that displacement (both linear and rotational about the platform origin)
/// directly into the rider's `Position`.  The player then effectively lives
/// in the platform's local frame for the duration of the frame.
#[derive(Component, Default)]
pub struct PlatformRider {
    /// The kinematic entity currently being ridden, if any.
    pub platform: Option<Entity>,
    /// The platform's Transform as recorded at the end of last FixedUpdate.
    pub last_transform: Transform,
}

/// Pre-physics system: apply the kinematic platform's delta transform to any
/// riding entity.  Must run in `FixedUpdate` BEFORE Avian's physics step and
/// BEFORE `player_movement_system` so that movement input is applied on top
/// of the already-moved position.
pub fn moving_platform_system(
    mut riders: Query<(
        Entity,
        &mut avian3d::prelude::Position,
        &mut avian3d::prelude::Rotation,
        &mut PlatformRider,
        &avian3d::prelude::ShapeHits,
    )>,
    platforms: Query<(&Transform, &avian3d::prelude::RigidBody)>,
) {
    for (rider_entity, mut pos, mut rider_rot, mut rider, hits) in &mut riders {
        // Find first ground contact that is on a kinematic or static body (important for ScrOni mutated static props).
        let new_platform: Option<Entity> = hits.iter().find_map(|hit| {
            if hit.entity == rider_entity {
                return None; // Skip self collisions!
            }
            if let Ok((_, rb)) = platforms.get(hit.entity) {
                if matches!(rb, avian3d::prelude::RigidBody::Kinematic | avian3d::prelude::RigidBody::Static) {
                    return Some(hit.entity);
                }
            }
            None
        });

        // Apply delta only when we're on the same platform as last tick.
        if let Some(platform_entity) = new_platform {
            if rider.platform == Some(platform_entity) {
                if let Ok((platform_tf, _)) = platforms.get(platform_entity) {
                    let prev = rider.last_transform;

                    // Rotation delta between frames.
                    let rot_delta = platform_tf.rotation * prev.rotation.inverse();

                    // Rotate the rider's world offset around the platform's
                    // *previous* origin, then re-anchor to its *new* origin.
                    let offset: Vec3 = pos.0 - prev.translation;
                    let new_world_pos = platform_tf.translation + rot_delta * offset;
                    pos.0 = new_world_pos;

                    // Apply yaw-only rotation to the rider's facing so they
                    // turn with the platform (e.g. a spinning disk).
                    let (yaw, _, _) = rot_delta.to_euler(EulerRot::YXZ);
                    if yaw.abs() > 1e-6 {
                        rider_rot.0 = Quat::from_rotation_y(yaw) * rider_rot.0;
                    }
                }
            }
        }

        // Record state for next tick.
        rider.platform = new_platform;
        if let Some(entity) = new_platform {
            if let Ok((tf, _)) = platforms.get(entity) {
                rider.last_transform = *tf;
            }
        }
    }
}

/// Tracks which movement animation is currently playing to avoid re-triggering.
#[derive(Component, Default, PartialEq, Eq)]
pub enum CreatureMovementAnim {
    #[default]
    Stand,
    Walk,
    Run,
    WalkLeft,
    WalkRight,
    RunLeft,
    RunRight,
}

/// Pick stand/walk/run animations based on horizontal velocity for all creatures.
pub fn creature_movement_anim_system(
    mut creatures: Query<(
        &crate::oni2_loader::animation::Oni2AnimLibrary,
        &mut crate::oni2_loader::animation::Oni2AnimState,
        &mut CreatureMovementAnim,
        &LinearVelocity,
        &GlobalTransform,
        Option<&crate::oni2_loader::parsers::loco::LocomotionController>,
        Option<&crate::statemachine::runtime::FsmRuntime>,
        Option<&crate::combat::components::HitReaction>,
    )>,
) {
    const WALK_THRESHOLD: f32 = 0.5;
    const RUN_THRESHOLD: f32 = 3.0;
    const MAX_RUN_SPEED: f32 = 6.0;

    for (library, mut anim_state, mut move_anim, vel, transform, loco_opt, fsm_opt, react_opt) in &mut creatures {
        // Block locomotion while an FSM-driven animation (attack, etc.) is playing.
        if let Some(fsm) = fsm_opt {
            if fsm.active_anim.is_some() && !fsm.timed_out {
                continue;
            }
        }

        // Block locomotion while a hit reaction animation is playing.
        if react_opt.map_or(false, |r| r.active.is_some()) {
            continue;
        }

        let horiz_speed = Vec2::new(vel.x, vel.z).length();

        // Get character forward and right directions
        let forward = transform.forward().xz().normalize_or_zero();
        let right = transform.right().xz().normalize_or_zero();

        let vel_xz = Vec2::new(vel.x, vel.z);
        let forward_speed = vel_xz.dot(forward);
        let right_speed = vel_xz.dot(-right);

        if let Some(loco) = loco_opt {
            let mut throttle_fwd = -forward_speed / MAX_RUN_SPEED;
            let mut throttle_right = -right_speed / MAX_RUN_SPEED;

            // Snap small movements to 0
            if throttle_fwd.abs() < 0.05 {
                throttle_fwd = 0.0;
            }
            if throttle_right.abs() < 0.05 {
                throttle_right = 0.0;
            }

            let (gaits, throttle) = if throttle_fwd.abs() >= throttle_right.abs() {
                (loco.forward_gaits.as_slice(), throttle_fwd)
            } else {
                (loco.strafe_gaits.as_slice(), throttle_right)
            };

            let mut best_gait = None;

            // Hysteresis: prevent physics micro-fluctuations from dropping states (bias against falling out below)
            if let Some(cur_id) = anim_state.current_anim_id {
                if let Some(cur_gait) = gaits.iter().find(|g| g.anim == cur_id) {
                    let lower = cur_gait.min_throttle.min(cur_gait.max_throttle) - 0.15;
                    let upper = cur_gait.min_throttle.max(cur_gait.max_throttle);
                    if throttle >= lower && throttle <= upper {
                        best_gait = Some(cur_gait);
                    }
                }
            }

            let best_gait = best_gait.or_else(|| {
                gaits
                    .iter()
                    .find(|g| {
                        let lower = g.min_throttle.min(g.max_throttle);
                        let upper = g.min_throttle.max(g.max_throttle);
                        throttle >= lower && throttle <= upper
                    })
                    .or_else(|| {
                        gaits.iter().min_by(|a, b| {
                            (a.ideal_throttle - throttle)
                                .abs()
                                .total_cmp(&(b.ideal_throttle - throttle).abs())
                        })
                    })
            });

            if let Some(gait) = best_gait {
                if Some(gait.anim) != anim_state.current_anim_id {
                    let prev_num_frames = anim_state.anim.frames.len().max(1) as f32;
                    let prev_phase = if prev_num_frames > 1.0 {
                        anim_state.current_time / (prev_num_frames - 1.0)
                    } else {
                        0.0
                    };

                    if library.play_id(gait.anim, &mut anim_state) {
                        *move_anim = CreatureMovementAnim::Stand; // Prevent legacy code from desyncing state

                        // Sync the new animation length directly to the previous phase percentage
                        let new_num_frames = anim_state.anim.frames.len().max(1) as f32;
                        if new_num_frames > 1.0 {
                            anim_state.current_time = prev_phase * (new_num_frames - 1.0);
                        }
                    } else {
                        warn!("Loco gait requested missing animation ID: {}", gait.anim);
                    }
                }
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
    let model = super::layout_loader::load_mod_file(path)?;
    let dir = entity_dir;

    let sub_meshes = build_meshes_by_material(&model);

    let bevy_materials: Vec<Vec<Handle<StandardMaterial>>> = model
        .materials
        .iter()
        .map(|oni_mat| {
            let mut handles = Vec::new();
            if !oni_mat.passes.is_empty() {
                for (pass_idx, pass) in oni_mat.passes.iter().enumerate() {
                    let (texture_handle, has_alpha) = match pass.texture_name.as_ref() {
                        Some(tex_name) => match load_tga_texture(dir, tex_name, images) {
                            Some((h, alpha)) => (Some(h), alpha),
                            None => (None, false),
                        },
                        None => (None, false),
                    };

                    let diffuse =
                        Color::srgb(oni_mat.diffuse[0], oni_mat.diffuse[1], oni_mat.diffuse[2]);
                    let is_decal = pass
                        .texcombine
                        .as_ref()
                        .map(|s| s == "decal")
                        .unwrap_or(false);
                    let is_blend = pass
                        .blendset
                        .as_ref()
                        .map(|s| s != "opaque")
                        .unwrap_or(false);

                    handles.push(materials.add(StandardMaterial {
                        base_color: if texture_handle.is_some() {
                            Color::WHITE
                        } else {
                            diffuse
                        },
                        base_color_texture: texture_handle,
                        cull_mode: None,
                        alpha_mode: if has_alpha || is_decal || is_blend {
                            AlphaMode::Blend
                        } else {
                            AlphaMode::Opaque
                        },
                        perceptual_roughness: 1.0,
                        reflectance: 0.0,
                        depth_bias: pass_idx as f32 * 10.0,
                        ..default()
                    }));
                }
            } else {
                let (texture_handle, has_alpha) = match oni_mat.texture_name.as_ref() {
                    Some(tex_name) => match load_tga_texture(dir, tex_name, images) {
                        Some((h, alpha)) => (Some(h), alpha),
                        None => (None, false),
                    },
                    None => (None, false),
                };

                let diffuse =
                    Color::srgb(oni_mat.diffuse[0], oni_mat.diffuse[1], oni_mat.diffuse[2]);
                handles.push(materials.add(StandardMaterial {
                    base_color: if texture_handle.is_some() {
                        Color::WHITE
                    } else {
                        diffuse
                    },
                    base_color_texture: texture_handle,
                    cull_mode: None,
                    alpha_mode: if has_alpha {
                        AlphaMode::Blend
                    } else {
                        AlphaMode::Opaque
                    },
                    perceptual_roughness: 1.0,
                    reflectance: 0.0,
                    ..default()
                }));
            }
            handles
        })
        .collect();

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
            positions: skel
                .positions
                .iter()
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
        crate::oni2_loader::Oni2Entity {
            name: label.to_string(),
        },
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

    let parent = ec
        .with_children(|parent| {
            for (mat_idx, mesh) in sub_meshes {
                let mesh_handle = meshes.add(mesh);
                let pass_handles = bevy_materials
                    .get(mat_idx)
                    .cloned()
                    .unwrap_or_else(|| vec![fallback_mat.clone()]);

                for pass_mat_handle in pass_handles {
                    parent.spawn((
                        Mesh3d(mesh_handle.clone()),
                        MeshMaterial3d(pass_mat_handle),
                        Transform::default(),
                    ));
                }
            }
        })
        .id();

    info!(
        "Spawned model '{}' from {:?} at {:?}",
        label, mod_path, position
    );
    Some(parent)
}

/// Load an ONI2 entity from a directory and spawn it in the world (no rotation).
pub fn spawn_oni2_entity(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    skinned_mesh_ibp: &mut ResMut<Assets<SkinnedMeshInverseBindposes>>,
    entity_lib: &mut ResMut<crate::oni2_loader::registries::EntityLibrary>,
    anim_registry: &mut ResMut<crate::oni2_loader::registries::AnimRegistry>,
    entity_dir: &str,
    position: Vec3,
    name: &str,
) -> Option<Entity> {
    spawn_oni2_entity_with_rotation(
        commands,
        meshes,
        materials,
        images,
        skinned_mesh_ibp,
        entity_lib,
        anim_registry,
        entity_dir,
        position,
        Quat::IDENTITY,
        name,
        None,
        None,
    )
}

/// Load an ONI2 entity from a directory and spawn it with position and rotation.
/// If `anim_path` is provided, load that specific anim file instead of the default.

fn parse_sawtooth_speed(payload: &str) -> f32 {
    let parts: Vec<&str> = payload.split_whitespace().collect();
    if let Some(idx) = parts.iter().position(|&x| x == "sawtooth") {
        if idx + 1 < parts.len() {
            return parts[idx + 1].parse().unwrap_or(0.0);
        }
    }
    0.0
}

fn parse_uv_animator(
    pass: &crate::oni2_loader::parsers::types::Oni2MaterialPass,
) -> crate::oni2_loader::registries::TextureUVAnimator {
    let mut anim = crate::oni2_loader::registries::TextureUVAnimator::default();
    if let Some(s) = &pass.slides {
        anim.slides_speed = parse_sawtooth_speed(s);
    }
    if let Some(s) = &pass.slidet {
        anim.slidet_speed = parse_sawtooth_speed(s);
    }
    if let Some(s) = &pass.rotate {
        anim.rotate_speed = parse_sawtooth_speed(s);
    }
    if let Some(s) = &pass.scalet {
        anim.scalet_speed = parse_sawtooth_speed(s);
    }
    anim
}

pub fn load_oni2_entity_type(
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    skinned_mesh_ibp: &mut Assets<SkinnedMeshInverseBindposes>,
    anim_registry: &mut crate::oni2_loader::registries::AnimRegistry,
    entity_dir: &str,
    name: &str,
    entity_type_name: Option<&str>,
) -> Option<crate::oni2_loader::registries::Oni2EntityType> {
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
                    Some(skel)
                }
                Err(e) => {
                    warn!(
                        "Entity '{}' references skeleton '{}', but it could not be read: {}",
                        name, skel_path, e
                    );
                    None
                }
            }
        }
        None => None,
    };

    // Read and parse the .mod file
    let model = if let Some(ref model_file) = entity_type.model_file {
        let mut m: Option<Oni2Model> = None;

        let mut mod_path = format!("{}/{}", dir, model_file);
        if !crate::vfs::exists("", &mod_path) {
            if model_file.ends_with(".mod") {
                let fallback = model_file.replace(".mod", "0.mod");
                let fallback_path = format!("{}/{}", dir, fallback);
                if crate::vfs::exists("", &fallback_path) {
                    mod_path = fallback_path;
                }
            }
        }

        if crate::vfs::exists("", &mod_path) {
            if let Some(mut model) = super::layout_loader::load_mod_file(&mod_path) {
                m = Some(model);
            }
        }

        if m.is_none() {
            let base_mod = format!("{}/{}.mod", dir, name);
            if crate::vfs::exists("", &base_mod) {
                if let Some(mut text_model) = super::layout_loader::load_mod_file(&base_mod) {
                    text_model.world_space_verts = true;
                    m = Some(text_model);
                }
            }
        }

        if m.is_none() {
            let win32_mod = format!("{}/win32_{}", dir, model_file);
            if crate::vfs::exists("", &win32_mod) {
                if let Some(mut model) = super::layout_loader::load_mod_file(&win32_mod) {
                    model.world_space_verts = true;
                    m = Some(model);
                }
            }
        }

        let mut was_world_space = false;
        if let (Some(model), Some(skel)) = (&mut m, &skeleton) {
            if skel.positions.len() > 1 {
                model.world_space_verts = false; // All multi-bone characters strictly map natively as bone-local!
            }
            was_world_space = model.world_space_verts;
            if model.world_space_verts {
                convert_world_to_bone_local(model, skel);
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

    let mut bound_verts: Vec<Vec3> = bound
        .as_ref()
        .map(|b| {
            b.vertices
                .iter()
                .map(|v| Vec3::new(v[0], v[1], v[2]))
                .collect()
        })
        .unwrap_or_default();

    if let (Some(skel), Some(model_ref)) = (&skeleton, &model) {
        if model_ref.world_space_verts == false {
            // Note: world_space_verts is set to false after convert_world_to_bone_local.
            // If the model had it originally true, it was converted. But we only check here if it's false, meaning it is bone-local,
            // or if it was converted. To be safe, let's just use the fact that skeleton exists and we want it attached.
            // Actually, we should only translate if bounds were historically world-space and the model is bone local.
            // But since we can't easily pass was_world_space, let's observe: oni bounds are always world space.
            // If we are attaching to bone 0, we must subtract bone 0 origin.
            if let Some(bp) = skel.positions.get(0) {
                // Actually, only offset bounds if it's a 1-bone rigid model because characters use capsule bounds unaffected by .bnd
                if skel.positions.len() == 1 {
                    for v in &mut bound_verts {
                        v.x -= bp[0];
                        v.y -= bp[1];
                        v.z -= bp[2];
                    }
                }
            }
        }
    }

    let bound_edges: Vec<[u32; 2]> = bound.as_ref().map(|b| b.edges.clone()).unwrap_or_default();
    let bound_quads: Vec<[u32; 4]> = bound.as_ref().map(|b| b.quads.clone()).unwrap_or_default();
    let bound_tris: Vec<[u32; 3]> = bound.as_ref().map(|b| b.tris.clone()).unwrap_or_default();

    let debug_bounds = crate::oni2_loader::spawn::Oni2DebugBounds {
        vertices: bound_verts,
        edges: bound_edges,
    };

    let debug_skeleton = skeleton.as_ref().map(|skel| {
        let positions = if let Some(ref m) = model {
            if !m.bone_world_positions.is_empty() {
                m.bone_world_positions
                    .iter()
                    .map(|p| Vec3::new(-p[0], p[1], -p[2]))
                    .collect()
            } else {
                skel.positions
                    .iter()
                    .map(|p| Vec3::new(-p[0], p[1], -p[2]))
                    .collect()
            }
        } else {
            skel.positions
                .iter()
                .map(|p| Vec3::new(-p[0], p[1], -p[2]))
                .collect()
        };
        crate::oni2_loader::spawn::Oni2DebugSkeleton {
            positions,
            parent_indices: skel.parent_indices.clone(),
            names: skel.names.clone(),
        }
    });

    let (library, locomotion, jump_controller) = if let Some(ref skel) = skeleton {
        let cache_key = format!("{}/{}", entity_dir, entity_type_name.unwrap_or(name));
        let (lib, loco, jump) = if let Some(cached) = anim_registry.libraries.get(&cache_key) {
            cached.clone()
        } else {
            let loaded = load_anim_library(entity_dir, entity_type_name.unwrap_or(name), skel);
            anim_registry
                .libraries
                .insert(cache_key.clone(), loaded.clone());
            loaded
        };
        let lib_opt = if !lib.anims.is_empty() {
            Some(lib)
        } else {
            None
        };
        (lib_opt, loco, jump)
    } else {
        (None, None, None)
    };

    let use_gpu_skinning = skeleton.is_some() && library.is_some();

    let sub_meshes = if let Some(ref m) = model {
        if use_gpu_skinning {
            build_skinned_meshes_by_material(m, skeleton.as_ref().unwrap())
        } else {
            if let Some(ref skel) = skeleton {
                let mut model_copy = m.clone();
                model_copy.bone_world_positions = skel.positions.clone();
                model_copy.bone_rotations = vec![[0.0, 0.0, 0.0, 1.0]; skel.positions.len()];
                build_meshes_by_material(&model_copy)
            } else {
                build_meshes_by_material(m)
            }
        }
    } else {
        Vec::new()
    };

    let ibp_handle = if use_gpu_skinning {
        let inverse_bind_poses = compute_inverse_bind_poses(skeleton.as_ref().unwrap());
        Some(skinned_mesh_ibp.add(SkinnedMeshInverseBindposes::from(inverse_bind_poses)))
    } else {
        None
    };

    let (bevy_materials, material_animators): (
        Vec<Vec<Handle<StandardMaterial>>>,
        Vec<Vec<crate::oni2_loader::registries::TextureUVAnimator>>,
    ) = if let Some(ref m) = model {
        m.materials
            .iter()
            .map(|oni_mat| {
                let mut handles = Vec::new();
                let mut anims = Vec::new();
                if !oni_mat.passes.is_empty() {
                    for (pass_idx, pass) in oni_mat.passes.iter().enumerate() {
                        let (texture_handle, has_alpha) = match pass.texture_name.as_ref() {
                            Some(tex_name) => {
                                match crate::oni2_loader::parsers::texture::load_tga_texture(
                                    dir, tex_name, images,
                                ) {
                                    Some((h, alpha)) => (Some(h), alpha),
                                    None => (None, false),
                                }
                            }
                            None => (None, false),
                        };

                        let diffuse =
                            Color::srgb(oni_mat.diffuse[0], oni_mat.diffuse[1], oni_mat.diffuse[2]);
                        let is_decal = pass
                            .texcombine
                            .as_ref()
                            .map(|s| s == "decal")
                            .unwrap_or(false);
                        let is_blend = pass
                            .blendset
                            .as_ref()
                            .map(|s| s != "opaque")
                            .unwrap_or(false);

                        handles.push(materials.add(StandardMaterial {
                            base_color: if texture_handle.is_some() {
                                Color::WHITE
                            } else {
                                diffuse
                            },
                            base_color_texture: texture_handle,
                            cull_mode: None,
                            alpha_mode: if has_alpha || is_decal || is_blend {
                                AlphaMode::Blend
                            } else {
                                AlphaMode::Opaque
                            },
                            perceptual_roughness: 1.0,
                            reflectance: 0.0,
                            depth_bias: pass_idx as f32 * 10.0,
                            ..default()
                        }));
                        anims.push(parse_uv_animator(pass));
                    }
                } else {
                    let (texture_handle, has_alpha) = match oni_mat.texture_name.as_ref() {
                        Some(tex_name) => {
                            match crate::oni2_loader::parsers::texture::load_tga_texture(
                                dir, tex_name, images,
                            ) {
                                Some((h, alpha)) => (Some(h), alpha),
                                None => (None, false),
                            }
                        }
                        None => (None, false),
                    };

                    let diffuse =
                        Color::srgb(oni_mat.diffuse[0], oni_mat.diffuse[1], oni_mat.diffuse[2]);
                    handles.push(materials.add(StandardMaterial {
                        base_color: if texture_handle.is_some() {
                            Color::WHITE
                        } else {
                            diffuse
                        },
                        base_color_texture: texture_handle,
                        cull_mode: None,
                        alpha_mode: if has_alpha {
                            AlphaMode::Blend
                        } else {
                            AlphaMode::Opaque
                        },
                        perceptual_roughness: 1.0,
                        reflectance: 0.0,
                        ..default()
                    }));
                    anims.push(crate::oni2_loader::registries::TextureUVAnimator::default());
                }
                (handles, anims)
            })
            .unzip()
    } else {
        (Vec::new(), Vec::new())
    };

    Some(crate::oni2_loader::registries::Oni2EntityType {
        name: name.to_string(),
        sub_meshes: sub_meshes
            .into_iter()
            .map(|(id, mesh)| (id, meshes.add(mesh)))
            .collect(),
        materials: bevy_materials,
        material_animators,
        skeleton,
        inverse_bind_poses: ibp_handle,
        bounds: debug_bounds,
        bound_quads,
        bound_tris,
        anim_library: library,
        locomotion,
        jump_controller,
        debug_skeleton,
    })
}

pub fn spawn_oni2_entity_with_rotation(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    skinned_mesh_ibp: &mut ResMut<Assets<SkinnedMeshInverseBindposes>>,
    entity_lib: &mut ResMut<crate::oni2_loader::registries::EntityLibrary>,
    anim_registry: &mut ResMut<crate::oni2_loader::registries::AnimRegistry>,
    entity_dir: &str,
    position: Vec3,
    rotation: Quat,
    name: &str,
    anim_path: Option<&str>,
    entity_type_name: Option<&str>,
) -> Option<Entity> {
    let dir = entity_dir;

    // 1. Get or load the EntityType
    let cache_key = dir.to_string();
    if !entity_lib.entities.contains_key(&cache_key) {
        if let Some(loaded) = load_oni2_entity_type(
            meshes.as_mut(),
            materials.as_mut(),
            images.as_mut(),
            skinned_mesh_ibp.as_mut(),
            anim_registry.as_mut(),
            entity_dir,
            name,
            entity_type_name,
        ) {
            entity_lib.entities.insert(cache_key.clone(), loaded);
        } else {
            // Provide a fast path log here if it utterly failed to read Entity.type
            warn!("Failed to load EntityType data for {}", dir);
            return None;
        }
    }
    let ent_type = entity_lib.entities.get(&cache_key)?;

    // 2. Build Collider from bounds on the fly
    let collider = if !ent_type.bounds.vertices.is_empty()
        && (!ent_type.bound_quads.is_empty() || !ent_type.bound_tris.is_empty())
    {
        let mut tri_indices: Vec<[u32; 3]> =
            Vec::with_capacity(ent_type.bound_quads.len() * 2 + ent_type.bound_tris.len());
        for q in &ent_type.bound_quads {
            tri_indices.push([q[0], q[1], q[2]]);
            tri_indices.push([q[0], q[2], q[3]]);
        }
        for t in &ent_type.bound_tris {
            tri_indices.push(*t);
        }
        Collider::try_trimesh(ent_type.bounds.vertices.clone(), tri_indices)
            .ok()
            .or_else(|| Collider::convex_hull(ent_type.bounds.vertices.clone()))
    } else if !ent_type.bounds.vertices.is_empty() {
        Collider::convex_hull(ent_type.bounds.vertices.clone())
    } else {
        None
    };

    let transform = Transform::from_translation(position).with_rotation(rotation);

    // Fallback material if missing
    let fallback_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.5, 0.5, 0.5),
        ..default()
    });

    let mut ec = commands.spawn((
        transform,
        Visibility::Visible,
        crate::oni2_loader::Oni2Entity {
            name: name.to_string(),
        },
        Name::new(name.to_string()),
        ent_type.bounds.clone(),
        InGameEntity,
    ));

    if collider.is_some() && ent_type.skeleton.is_none() && ent_type.anim_library.is_none() {
        ec.insert(RigidBody::Static); // Truly static objects get static bodies
    }

    if let Some(ref ds) = ent_type.debug_skeleton {
        ec.insert(ds.clone());
    }

    let parent_entity = ec.id();

    // 3. Spawning skeleton and animation
    let use_gpu_skinning = ent_type.skeleton.is_some() && ent_type.anim_library.is_some();

    let joint_entities = if use_gpu_skinning {
        let skel = ent_type.skeleton.as_ref().unwrap();
        let num_bones = skel.positions.len();
        let mut joints = Vec::with_capacity(num_bones);
        for _ in 0..num_bones {
            let joint = commands
                .spawn((
                    Transform::IDENTITY,
                    GlobalTransform::default(),
                    Visibility::Hidden,
                ))
                .id();
            commands.entity(parent_entity).add_child(joint);
            joints.push(joint);
        }
        joints
    } else {
        Vec::new()
    };

    if let Some(c) = collider {
        if use_gpu_skinning
            && ent_type
                .skeleton
                .as_ref()
                .map(|s| s.positions.len())
                .unwrap_or(0)
                == 1
        {
            commands.entity(joint_entities[0]).insert((
                c,
                RigidBody::Kinematic,
                avian3d::prelude::LinearVelocity::default(),
                avian3d::prelude::AngularVelocity::default(),
            ));
        } else if use_gpu_skinning {
            commands.entity(parent_entity).insert((
                c,
                RigidBody::Kinematic,
                avian3d::prelude::LinearVelocity::default(),
                avian3d::prelude::AngularVelocity::default(),
            ));
        } else {
            commands.entity(parent_entity).insert(c);
        }
    }

    // Determine the default animation
    // Note: anim_path logic isn't preserved directly inside Oni2EntityType as it specifies instance overrides,
    // so we parse the single instance animation requested right now natively
    let loaded_anim = anim_path.and_then(|path| {
        crate::vfs::read("", path)
            .ok()
            .and_then(|data| parse_anim(&data))
    });

    let default_anim = loaded_anim.or_else(|| {
        ent_type
            .anim_library
            .as_ref()
            .and_then(|lib| lib.anims.values().next().cloned())
    });

    // Insert AnimState and Library even if there is no skeleton (for root-motion only animations)
    let has_anim = default_anim.is_some() || !joint_entities.is_empty();
    if has_anim {
        let skel = ent_type.skeleton.clone().unwrap_or_default();
        let current_anim = default_anim.unwrap_or_default();
        let looping = current_anim.is_loop;

        commands
            .entity(parent_entity)
            .insert(crate::oni2_loader::animation::Oni2AnimState {
                anim: current_anim,
                skeleton: skel,
                current_time: 0.0,
                fps: 20.0,
                paused: false,
                looping,
                speed_multiplier: 1.0,
                pending_step: 0,
                last_rendered_time: -1.0,
                joint_entities: joint_entities.clone(),
                base_rotation: rotation,
                current_frame: vec![0.0; ent_type.anim_library.as_ref().and_then(|lib| lib.anims.values().next()).map_or(0, |a| a.num_channels) as usize],
                current_anim_id: None,
                previous_anim_id: None,
            });
    }

    if let Some(ref lib) = ent_type.anim_library {
        commands.entity(parent_entity).insert(lib.clone());
    }

    if let Some(ref loco) = ent_type.locomotion {
        commands.entity(parent_entity).insert(loco.clone());
    }

    if let Some(ref jump) = ent_type.jump_controller {
        commands.entity(parent_entity).insert(jump.clone());
    }

    // 4. Mesh sub_meshes
    for (mat_idx, mesh_handle) in &ent_type.sub_meshes {
        let pass_handles = ent_type
            .materials
            .get(*mat_idx)
            .cloned()
            .unwrap_or_else(|| vec![fallback_mat.clone()]);

        let pass_animators = ent_type
            .material_animators
            .get(*mat_idx)
            .cloned()
            .unwrap_or_default();

        for (pass_idx, pass_mat_handle) in pass_handles.into_iter().enumerate() {
            let mut mesh_ec = commands.spawn((
                Mesh3d(mesh_handle.clone()),
                MeshMaterial3d(pass_mat_handle),
                Transform::default(),
            ));

            if let Some(anim) = pass_animators.get(pass_idx) {
                if anim.slides_speed != 0.0
                    || anim.slidet_speed != 0.0
                    || anim.rotate_speed != 0.0
                    || anim.scalet_speed != 0.0
                {
                    mesh_ec.insert(anim.clone());
                }
            }

            if let Some(ref ibp) = ent_type.inverse_bind_poses {
                mesh_ec.insert(SkinnedMesh {
                    inverse_bindposes: ibp.clone(),
                    joints: joint_entities.clone(),
                });
            }

            let mesh_entity = mesh_ec.id();
            commands.entity(parent_entity).add_child(mesh_entity);
        }
    }

    Some(parent_entity)
}

pub fn spawn_oni2_creature(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    skinned_mesh_ibp: &mut ResMut<Assets<SkinnedMeshInverseBindposes>>,
    entity_lib: &mut ResMut<crate::oni2_loader::registries::EntityLibrary>,
    anim_registry: &mut ResMut<crate::oni2_loader::registries::AnimRegistry>,
    entity_dir: &str,
    position: Vec3,
    rotation: Quat,
    actor_name: &str,
    entity_type: &str,
    animator_type: Option<&str>,
    entity_base: &str,
) -> Option<Entity> {
    let anim_name = animator_type.unwrap_or(entity_type);

    let anim_entity_dir = format!("{}/{}", entity_base, anim_name);

    // Spawn above intended position; ground_snap_system will find solid ground
    let spawn_position = position + Vec3::Y * 2.0;

    let entity = spawn_oni2_entity_with_rotation(
        commands,
        meshes,
        materials,
        images,
        skinned_mesh_ibp,
        entity_lib,
        anim_registry,
        entity_dir,
        spawn_position,
        rotation,
        actor_name,
        None,
        Some(anim_name),
    )?;

    let mut min_y = 0.0;
    let mut max_y = 2.0;

    // Reverted bounding volume processing. Collision bounds naturally have differing "invisible padding"
    // margins beneath the physical feet vertices per-character, causing variable floats when math'd dynamically.
    // Instead, since all visual skeleton hips structurally pivot 1.0 meter above their feet relative origin,
    // and ground_snap offsets by exactly 1.1m (1.0 half height + 0.1 buffer), we apply -2.1 offset exactly.
    let capsule_radius = 0.4;
    let capsule_length = 1.2;
    let capsule_half_height = 1.0;

    let y_offset = -2.1;

    bevy::log::info!(
        "Creature '{}': bounds Y [{:.2}, {:.2}], offset applied: {:.2}",
        anim_name, min_y, max_y, y_offset
    );

    // Every creature gets a physics capsule + render offset + ground snap
    commands.entity(entity).insert((
        RigidBody::Dynamic,
        Collider::capsule(capsule_radius, capsule_length),
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
        CreatureRenderOffset {
            y_offset,
            facing: Quat::IDENTITY,
        },
        PlatformRider::default(),
        NeedsGroundSnap {
            origin: position,
            wait_frames: 3,
            capsule_half_height,
        },
    ));

    // Load animation library
    let skel_path = format!("{}/{}.skel", entity_dir, entity_type);
    let skel_data = crate::vfs::read_to_string("", &skel_path).ok();
    let skeleton = skel_data.map(|s| parse_skel(&s));

    if let Some(ref skel) = skeleton {
        let cache_key = format!("{}/{}", anim_entity_dir, anim_name);
        let (library, locomotion, jump_controller) =
            if let Some(cached) = anim_registry.libraries.get(&cache_key) {
                cached.clone()
            } else {
                let loaded = load_anim_library(&anim_entity_dir, anim_name, skel);
                anim_registry
                    .libraries
                    .insert(cache_key.clone(), loaded.clone());
                loaded
            };
        if !library.anims.is_empty() {
            commands.entity(entity).insert(library);
            if let Some(loco) = locomotion {
                commands.entity(entity).insert(loco);
            }
            if let Some(jump) = jump_controller {
                commands.entity(entity).insert(jump);
            }
            commands.entity(entity).insert(CreatureMovementAnim::Run);

            // Load attack combo sequence from .attacks file (data-driven DoTriggerAtk)
            let attack_data = crate::oni2_loader::parsers::anims::load_attack_data(
                &anim_entity_dir,
                anim_name,
            );
            if !attack_data.forward_combo.is_empty() {
                commands.entity(entity).insert(attack_data);
            }

            // Load react library (ANIMREACT_*.rct files, indexed by animReactEnum)
            let react_lib = crate::oni2_loader::parsers::rct::load_react_library(anim_name);
            if !react_lib.entries.is_empty() {
                commands.entity(entity).insert(react_lib);
            }
        }
    }

    Some(entity)
}

/// Load texture for an entity: tries .tex (native), then .tex.tga (pre-converted).
pub(crate) fn load_tga_file(
    path: &str,
    images: &mut ResMut<Assets<Image>>,
) -> Option<(Handle<Image>, bool)> {
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
    image.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::Repeat,
        address_mode_v: bevy::image::ImageAddressMode::Repeat,
        ..default()
    });
    Some((images.add(image), has_alpha))
}

/// Dynamically resolves cross-actor parenting rules designated by layout props.
/// Children spawn autonomously but will aggressively seek to snap their Transform to specific Bones on their target Parent.
pub fn resolve_pending_parents_system(
    mut commands: Commands,
    pending_query: Query<(Entity, &crate::oni2_loader::components::PendingParent)>,
    target_query: Query<(Entity, &Name, Option<&crate::oni2_loader::animation::Oni2AnimState>)>,
) {
    for (child_entity, pending) in &pending_query {
        // Find matching parent by name
        let mut resolved_parent = None;

        for (target_entity, name, anim_state_opt) in &target_query {
            if name.as_str() == pending.parent_name {
                let mut parent_to_use = target_entity;

                // Try assigning to a direct bone matrix if requested and available
                if let (Some(bone_name), Some(anim_state)) = (&pending.bone_name, anim_state_opt) {
                    // Search skeleton name mappings for index
                    for (i, skeleton_bone_name) in anim_state.skeleton.names.iter().enumerate() {
                        if skeleton_bone_name.eq_ignore_ascii_case(bone_name) {
                            if let Some(joint_ent) = anim_state.joint_entities.get(i) {
                                parent_to_use = *joint_ent;
                            }
                            break;
                        }
                    }
                }

                resolved_parent = Some(parent_to_use);
                break;
            }
        }

        if let Some(parent) = resolved_parent {
            commands.entity(parent).add_child(child_entity);
            commands.entity(child_entity).remove::<crate::oni2_loader::components::PendingParent>();
            info!(
                "Successfully reparented layout actor to {}",
                pending.parent_name
            );
        }
    }
}
