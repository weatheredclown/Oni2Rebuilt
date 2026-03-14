use super::*;

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
            transform.translation = Vec3::new(
                ground_pos.x,
                ground_pos.y + capsule_half_height + 0.1, // small buffer
                ground_pos.z,
            );
            info!(
                "Ground-snapped creature {:?} from ({:.1}, {:.1}, {:.1}) to ({:.1}, {:.1}, {:.1})",
                entity,
                origin.x,
                origin.y,
                origin.z,
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
            );
        } else {
            // No ground found — spawn above origin and hope for the best
            transform.translation = Vec3::new(origin.x, origin.y + 2.0, origin.z);
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
    mut creatures: Query<(
        &Oni2AnimLibrary,
        &mut Oni2AnimState,
        &mut CreatureMovementAnim,
        &LinearVelocity,
    )>,
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
                warn!(
                    "Creature missing expected movement animation alias: {}",
                    alias
                );
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

    let bevy_materials: Vec<Handle<StandardMaterial>> = model
        .materials
        .iter()
        .map(|oni_mat| {
            let (texture_handle, has_alpha) = match oni_mat.texture_name.as_ref() {
                Some(tex_name) => match load_tga_texture(dir, tex_name, images) {
                    Some((h, alpha)) => (Some(h), alpha),
                    None => (None, false),
                },
                None => (None, false),
            };
            let diffuse = Color::srgb(oni_mat.diffuse[0], oni_mat.diffuse[1], oni_mat.diffuse[2]);
            materials.add(StandardMaterial {
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
                ..default()
            })
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
        Oni2Entity {
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
                let mat_handle = bevy_materials
                    .get(mat_idx)
                    .cloned()
                    .unwrap_or_else(|| fallback_mat.clone());
                parent.spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(mat_handle),
                    Transform::default(),
                ));
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
                    info!(
                        "Loaded skeleton: {} bones from {}",
                        skel.positions.len(),
                        skel_path
                    );
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
        None => {
            info!(
                "Entity '{}' has no skeleton file defined in {}/Entity.type; skipping animation features.",
                name, dir
            );
            None
        }
    };

    // Read and parse the .mod file
    // Prefer win32 binary LOD model (world-space vertices, correct bone count).
    // Fall back to standard binary LOD, then text base model.
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
            if let Some(mut model) = super::layout_loader::load_mod_file(&mod_path) {
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
                if let Some(text_model) = super::layout_loader::load_mod_file(&base_mod) {
                    info!("Falling back to text model {}", base_mod);
                    m = Some(text_model);
                }
            }
        }

        // Last resort: win32 binary LOD model (world-space vertices)
        if m.is_none() {
            let win32_mod = format!("{}/win32_{}", dir, model_file);
            if crate::vfs::exists("", &win32_mod) {
                if let Some(model) = super::layout_loader::load_mod_file(&win32_mod) {
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
                info!(
                    "Converted world-space vertices to bone-local ({} verts)",
                    model.vertices.len()
                );
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
        .map(|b| {
            b.vertices
                .iter()
                .map(|v| Vec3::new(v[0], v[1], v[2]))
                .collect()
        })
        .unwrap_or_default();
    let bound_edges: Vec<[u32; 2]> = bound.as_ref().map(|b| b.edges.clone()).unwrap_or_default();

    // Build trimesh collider from bound quads (thin shell, not solid volume).
    // Convex hull fills the interior and blocks raycasts/physics through the
    // bounding volume — trimesh only collides at the actual faces.
    let bound_quads: Vec<[u32; 4]> = bound.as_ref().map(|b| b.quads.clone()).unwrap_or_default();

    let collider = if !bound_verts.is_empty() && !bound_quads.is_empty() {
        // Split quads into triangles
        let mut tri_indices: Vec<[u32; 3]> = Vec::with_capacity(bound_quads.len() * 2);
        for q in &bound_quads {
            tri_indices.push([q[0], q[1], q[2]]);
            tri_indices.push([q[0], q[2], q[3]]);
        }
        Collider::try_trimesh(bound_verts.clone(), tri_indices).unwrap_or_else(|_| {
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
        Oni2DebugSkeleton {
            positions,
            parent_indices: skel.parent_indices.clone(),
            names: skel.names.clone(),
        }
    });

    // Load explicitly requested animation, if any
    let loaded_anim = anim_path.and_then(|path| {
        crate::vfs::read("", path)
            .ok()
            .and_then(|data| parse_anim(&data))
    });

    // Load animation library from .anims + .apkg packages if skeleton available
    let library = if let Some(ref skel) = skeleton {
        let lib = load_anim_library(entity_dir, entity_type_name.unwrap_or(name), skel);
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
            Oni2Entity {
                name: name.to_string(),
            },
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
        library
            .as_ref()
            .and_then(|lib| lib.anims.values().next().cloned())
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
        let inverse_bind_poses =
            super::animation::compute_inverse_bind_poses(skeleton.as_ref().unwrap());
        Some(skinned_mesh_ibp.add(SkinnedMeshInverseBindposes::from(inverse_bind_poses)))
    } else {
        None
    };

    // Load textures and create Bevy materials for each ONI2 material
    let bevy_materials: Vec<Handle<StandardMaterial>> = model
        .materials
        .iter()
        .map(|oni_mat| {
            let (texture_handle, has_alpha) = match oni_mat.texture_name.as_ref() {
                Some(tex_name) => match load_tga_texture(dir, tex_name, images) {
                    Some((h, alpha)) => (Some(h), alpha),
                    None => (None, false),
                },
                None => (None, false),
            };

            let diffuse = Color::srgb(oni_mat.diffuse[0], oni_mat.diffuse[1], oni_mat.diffuse[2]);

            materials.add(StandardMaterial {
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
                ..default()
            })
        })
        .collect();

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
        Oni2Entity {
            name: name.to_string(),
        },
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
            let joint = commands
                .spawn((Transform::IDENTITY, Visibility::Hidden))
                .id();
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
        let mat_handle = bevy_materials
            .get(mat_idx)
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
pub fn spawn_oni2_creature(
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
    entity_base: &str,
) -> Option<Entity> {
    let anim_name = animator_type.unwrap_or(entity_type);

    let anim_entity_dir = format!("{}/{}", entity_base, anim_name);

    // Spawn above intended position; ground_snap_system will find solid ground
    let spawn_position = position + Vec3::Y * 2.0;

    // Spawn the visual entity
    let entity = spawn_oni2_entity_with_rotation(
        commands,
        meshes,
        materials,
        images,
        skinned_mesh_ibp,
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
        let library = load_anim_library(&anim_entity_dir, anim_name, skel);
        if !library.anims.is_empty() {
            commands.entity(entity).insert(library);
            commands.entity(entity).insert(CreatureMovementAnim::Run);
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
