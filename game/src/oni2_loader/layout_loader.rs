use super::*;
use crate::oni2_loader::parsers::texture::decode_tex;
use crate::oni2_loader::parsers::texture::load_tga_texture;

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
    _asset_server: &AssetServer,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    skinned_mesh_ibp: &mut ResMut<Assets<SkinnedMeshInverseBindposes>>,
    entity_lib: &mut ResMut<crate::oni2_loader::registries::EntityLibrary>,
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
        curves: parsers::layout::parse_layout_paths(layout_path),
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
    let assets_base = if parts.is_empty() {
        String::new()
    } else {
        parts.join("/")
    };
    let _template_dir = format!("{}/template", assets_base);

    let mut texture_collections = TextureCollections::default();

    // Insert LayoutContext for dynamic spawning by scripts
    let layout_ctx = LayoutContext {
        layout_dir: layout_path.to_string(),
        entity_base: entity_base.to_string(),
        basic_types,
    };
    commands.insert_resource(layout_ctx.clone());

    let mut spawned = 0;
    let mut creatures = 0;
    let mut skipped = 0;
    let mut player_info: Option<LayoutPlayerInfo> = None;
    for line in actors_content.lines() {
        let actor_name = line.trim();
        if actor_name.is_empty() || actor_name.parse::<u32>().is_ok() {
            continue; // skip count line and blank lines
        }

        let mut spawn_assets = SpawnAssets {
            commands,
            meshes,
            materials,
            images,
            skinned_mesh_ibp: &mut *skinned_mesh_ibp,
            entity_lib: &mut *entity_lib,
            texture_collections: &mut texture_collections,
        };

        if let Some((entity, actor)) = spawn_layout_actor(
            &mut spawn_assets,
            actor_name,
            &layout_ctx,
            &layout_paths,
            None,
        ) {
            if actor.is_creature {
                creatures += 1;
                if actor.is_player && player_info.is_none() {
                    player_info = Some(LayoutPlayerInfo {
                        entity,
                        position: actor.position,
                        entity_type: actor.entity_type.clone(),
                        animator_type: actor.animator_type.clone().unwrap_or_default(),
                    });
                }
            } else {
                spawned += 1;
            }
        } else {
            // Not spawned because it failed to parse or wasn't a basic type
            skipped += 1;
        }
    }
    info!(
        "Layout: spawned {} entities, {} creatures, skipped {}",
        spawned, creatures, skipped
    );
    if let Some(ref pi) = player_info {
        info!(
            "Layout: player creature found: type={} animator={}",
            pi.entity_type, pi.animator_type
        );
    }

    // Insert LayoutPaths resource for potential future use
    if !layout_paths.curves.is_empty() {
        commands.insert_resource(layout_paths);
    }

    // Insert TextureCollections resource for the texture_movie_system observer
    commands.insert_resource(texture_collections);

    // Load camera packages and parameters
    let mut camera_packages = CameraPackages {
        packages: crate::oni2_loader::parsers::camera::parse_campacknew(layout_dir),
    };
    let mut camera_sets = CameraParameterSets::default();
    
    // We only need to load the xml files referenced in the packages
    let mut files_to_load = std::collections::HashSet::new();
    for pkg in camera_packages.packages.values() {
        if !pkg.navigation.is_empty() { files_to_load.insert(pkg.navigation.clone()); }
        if !pkg.targeting.is_empty() { files_to_load.insert(pkg.targeting.clone()); }
        if !pkg.fighting.is_empty() { files_to_load.insert(pkg.fighting.clone()); }
    }

    for file_base in files_to_load {
        let xml_name = format!("{}.xml", file_base);
        if let Some(params) = crate::oni2_loader::parsers::camera::parse_camera_xml(layout_dir, &xml_name) {
            camera_sets.sets.insert(file_base, params);
        } else {
            warn!("Failed to load camera xml: {}", xml_name);
        }
    }

    info!("Layout: loaded {} camera packages, {} parameter sets", camera_packages.packages.len(), camera_sets.sets.len());

    commands.insert_resource(camera_packages);
    commands.insert_resource(camera_sets);
    commands.insert_resource(ActiveCameraPackage::default());

    // Load lights, fog, skyhat
    load_layout_lights(commands, meshes, materials, images, layout_dir);

    player_info
}

/// Spawns a single actor by name, parsing its XML internally. Can override position.
pub fn spawn_layout_actor(
    assets: &mut SpawnAssets,
    actor_name: &str,
    layout_ctx: &LayoutContext,
    layout_paths: &LayoutPaths,
    pos_override: Option<Vec3>,
) -> Option<(Entity, LayoutActor)> {
    // Find template dir
    let template_dir = "template".to_string();

    // Parse the actor XML
    let actor = match crate::oni2_loader::parsers::actor_xml::parse_actor_xml(
        &layout_ctx.layout_dir,
        &format!("{}.xml", actor_name),
        &template_dir,
    ) {
        Some(a) => a,
        None => return None,
    };

    // Find the entity directory
    let entity_dir = format!("{}/{}", layout_ctx.entity_base, actor.entity_type);

    // Try parsing .sha to find .tc (Texture Collection) and preload textures
    if !assets
        .texture_collections
        .collections
        .contains_key(&actor.entity_type)
    {
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
                        if trimmed.is_empty()
                            || trimmed.starts_with("version:")
                            || trimmed.starts_with("texCount:")
                        {
                            continue;
                        }

                        // Load the texture handle using the asset server
                        let tex_name = match trimmed.strip_suffix(".tex") {
                            Some(stripped) => stripped.to_string(),
                            None => trimmed.to_string(),
                        };

                        if let Some((tex_handle, _)) =
                            load_tga_texture(&entity_dir, &tex_name, assets.images)
                        {
                            frames.push(tex_handle);
                        }
                    }
                    info!(
                        "Loaded Texture Collection for {}: {} frames",
                        actor.entity_type,
                        frames.len()
                    );
                }
            }
        }
        assets
            .texture_collections
            .collections
            .insert(actor.entity_type.clone(), frames);
    }

    if actor.is_creature {
        // Position already in Bevy coordinates (Z negated at parse time)
        let position = pos_override.unwrap_or(actor.position);
        // 180° Y rotation flips X and Z rotation directions
        let rotation = Quat::from_rotation_x(-actor.orientation.x.to_radians())
            * Quat::from_rotation_y(actor.orientation.y.to_radians())
            * Quat::from_rotation_z(-actor.orientation.z.to_radians());

        if let Some(ref anim_type) = actor.animator_type {
            info!(
                "Creature {} type={} animator={} player={}",
                actor_name, actor.entity_type, anim_type, actor.is_player
            );
        }

        if let Some(entity) = spawn_oni2_creature(
            assets.commands,
            assets.meshes,
            assets.materials,
            assets.images,
            assets.skinned_mesh_ibp,
            assets.entity_lib,
            &entity_dir,
            position,
            rotation,
            actor_name,
            &actor.entity_type,
            actor.animator_type.as_deref(),
            &layout_ctx.entity_base,
        ) {
            if !actor.is_player {
                // Non-player creature: attach AI + combat components
                assets.commands.entity(entity).insert((
                    crate::combat::components::Enemy,
                    crate::ai::components::AiFighter::default(),
                    crate::combat::components::Fighter::default(),
                    crate::combat::components::FighterId(uuid::Uuid::new_v4()),
                    crate::combat::components::Health::new(100.0),
                ));
                assets.commands.entity(entity).insert((
                    crate::combat::components::AttackState::default(),
                    crate::combat::components::BlockState::new(),
                    crate::combat::components::ComboTracker::default(),
                    crate::combat::components::SuperMeter::default(),
                    crate::combat::components::GrabState::default(),
                    crate::combat::components::HitReaction::default(),
                    crate::combat::components::AboutToBeHit::default(),
                ));
                assets
                    .commands
                    .entity(entity)
                    .insert(crate::camera::components::PrototypeElement);
            }
            return Some((entity, actor));
        }
    } else {
        // Static entity (BASICENTITY check) or trigger (has broadcast_radius)
        let mut is_basic = layout_ctx.basic_types.contains(&actor.entity_type)
            || layout_ctx
                .basic_types
                .iter()
                .any(|t| t.eq_ignore_ascii_case(&actor.entity_type));
        let is_trigger = actor.broadcast_radius.is_some();
        if !is_basic && !is_trigger {
            // User feedback: missing entity types from layout.et should be loaded on demand as basic entities
            is_basic = true;
        }

        let position = pos_override.unwrap_or(actor.position);
        // 180° Y rotation flips X and Z rotation directions
        let rotation = Quat::from_rotation_x(-actor.orientation.x.to_radians())
            * Quat::from_rotation_y(actor.orientation.y.to_radians())
            * Quat::from_rotation_z(-actor.orientation.z.to_radians());

        let entity = if is_basic {
            spawn_oni2_entity_with_rotation(
                assets.commands,
                assets.meshes,
                assets.materials,
                assets.images,
                assets.skinned_mesh_ibp,
                assets.entity_lib,
                &entity_dir,
                position,
                rotation,
                &actor.entity_type,
                None,
                Some(&actor.entity_type),
            )
        } else {
            // It's just a trigger without a visual model, so spawn an empty entity
            Some(
                assets
                    .commands
                    .spawn((
                        Transform::from_translation(position).with_rotation(rotation),
                        GlobalTransform::default(),
                        Name::new(actor.entity_type.clone()),
                        crate::menu::InGameEntity,
                    ))
                    .id(),
            )
        };

        if let Some(entity) = entity {
            // Attach CurveFollower if actor references a named curve
            if let Some(ref cname) = actor.curve_name {
                if let Some((_, pts)) = layout_paths
                    .curves
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(cname))
                {
                    if pts.len() >= 4 {
                        let curve = NurbsCurve::new(pts.clone());
                        let has_script = actor.script_filename.is_some();
                        let speed = if has_script {
                            0.0 // script will set speed via GotoCurvePhase
                        } else if actor.curve_speed > 0.0 {
                            actor.curve_speed
                        } else {
                            0.2 // 1.0 / 5.0 seconds
                        };
                        assets.commands.entity(entity).insert((
                            CurveFollower {
                                curve,
                                phase: 0.0,
                                speed,
                                target_phase: if has_script { 0.0 } else { 1.0 },
                                wrap_around: if has_script {
                                    false
                                } else {
                                    !actor.curve_ping_pong
                                },
                                ping_pong: actor.curve_ping_pong,
                                look_along_xz: actor.curve_look_xz,
                                fixed_orientation: actor.curve_fixed_orientation,
                                reached_target: has_script,
                            },
                            avian3d::prelude::RigidBody::Kinematic,
                        ));
                        info!(
                            "Attached CurveFollower '{}' to {} ({} control points)",
                            cname,
                            actor.entity_type,
                            pts.len()
                        );
                    } else {
                        warn!(
                            "Curve '{}' has {} points (need >= 4), skipping",
                            cname,
                            pts.len()
                        );
                    }
                } else {
                    warn!(
                        "Curve '{}' not found in layout.paths for {}",
                        cname, actor.entity_type
                    );
                }
            }

            // Attach ScrOni script if actor has a <ScrOni> component
            if let Some(ref filename) = actor.script_filename {
                let default_main = std::path::Path::new(filename)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let main_script = actor.script_main.as_ref().unwrap_or(&default_main);

                let (script_dir, script_fname) =
                    resolve_script_path(&layout_ctx.layout_dir, filename);
                match scroni::vm::load_script_file(&script_dir, &script_fname) {
                    Ok(file) => {
                        if let Some(script_def) = file
                            .scripts
                            .iter()
                            .find(|s| s.name.eq_ignore_ascii_case(main_script))
                        {
                            let mut exec = scroni::vm::ScriptExec::new(script_def.clone(), entity, 0.0);
                            for s in &file.scripts {
                                exec.available_scripts.insert(s.name.clone(), s.clone());
                            }
                            assets
                                .commands
                                .entity(entity)
                                .insert(scroni::vm::ScrOniScript { exec });
                            info!(
                                "Attached ScrOni script '{}:{}' to {}",
                                filename, main_script, actor.entity_type
                            );
                        } else {
                            warn!(
                                "Script '{}' not found in {}/{} (available: {})",
                                main_script,
                                script_dir,
                                script_fname,
                                file.scripts
                                    .iter()
                                    .map(|s| s.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to compile script {}/{}: {}",
                            script_dir, script_fname, e
                        );
                    }
                }
            }

            // Attach BroadcastTrigger if present
            if let Some(radius) = actor.broadcast_radius {
                assets
                    .commands
                    .entity(entity)
                    .insert(crate::scroni::vm::BroadcastTrigger {
                        radius,
                        ..Default::default()
                    });
                info!(
                    "Attached BroadcastTrigger (radius {}) to {} at position {:?}",
                    radius, actor.entity_type, position
                );
            }

            // Attach FXType component if present
            if let Some(ref fx) = actor.fx_type {
                assets.commands.entity(entity).insert(crate::oni2_loader::components::ActorFxType {
                    fx_name: fx.clone(),
                });
                info!("Attached FXType '{}' to {}", fx, actor.entity_type);
            }

            return Some((entity, actor));
        }
    }
    None
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
            Transform::from_xyz(
                env.light_direction.x,
                env.light_direction.y,
                env.light_direction.z,
            )
            .looking_at(Vec3::ZERO, Vec3::Y),
            InGameEntity,
        ));

        commands.spawn((
            AmbientLight {
                color: Color::srgb(
                    env.ambient_color[0],
                    env.ambient_color[1],
                    env.ambient_color[2],
                ),
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

        info!(
            "Layout: loaded environment (dir_light=({:.2},{:.2},{:.2}), fog_start={:.1}, fog_end={:.1})",
            env.light_direction.x,
            env.light_direction.y,
            env.light_direction.z,
            env.fog_start,
            env.fog_end
        );
    } else if let Some(ref fog) = fog_data {
        // No environment file — use layout.fog for lighting + fog
        for (i, light) in fog.lights.iter().enumerate() {
            if !light.enabled {
                continue;
            }
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
            info!(
                "Layout: loaded fog from layout.fog (start={:.1}, end={:.1})",
                fog.start, fog.end
            );
        }

        info!(
            "Layout: loaded lighting from layout.fog ({} lights)",
            fog.lights.len()
        );
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
        info!(
            "Layout: loaded {} point lights, {} ambient lights",
            point_count, ambient_count
        );
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
    if !crate::vfs::exists("", &skyhat_path) {
        return;
    }

    let model = match load_mod_file(&skyhat_path) {
        Some(m) => m,
        None => return,
    };

    // Look for sky texture in the layout directory
    let sky_texture = find_sky_texture(layout_path, images);

    let sub_meshes = build_meshes_by_material(&model);
    if sub_meshes.is_empty() {
        return;
    }

    // Spawn parent entity for skyhat
    let parent = commands
        .spawn((
            Transform::default(),
            Visibility::Visible,
            SkyHat,
            InGameEntity,
        ))
        .id();

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
        let child = commands
            .spawn((
                Mesh3d(mesh_handle),
                MeshMaterial3d(mat),
                Transform::default(),
            ))
            .id();
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
            let name = entry
                .path
                .split('/')
                .last()
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if name.contains("sky") {
                if name.ends_with(".tex") && !name.ends_with(".tex.tga") {
                    if let Ok(tex_bytes) = crate::vfs::read("", &entry.path) {
                        if let Some((width, height, rgba, _)) = decode_tex(&tex_bytes) {
                            info!("Loaded sky texture: {} ({}x{})", entry.path, width, height);
                            let mut image = Image::new(
                                bevy::render::render_resource::Extent3d {
                                    width,
                                    height,
                                    depth_or_array_layers: 1,
                                },
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
                    if let Some((handle, _)) = super::spawn::load_tga_file(&entry.path, images) {
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
pub(crate) fn load_mod_file(path: &str) -> Option<Oni2Model> {
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
    let parts: Vec<f32> = s
        .split_whitespace()
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
