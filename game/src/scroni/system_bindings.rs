/*
 * scroni/system_bindings.rs — Bevy observer that executes ScrOni VM system requests.
 *
 * scroni_sys_event_observer: handles every ScrOniSysEvent variant emitted by the VM:
 * SpawnEntity, PlaySound, PlayFx, CameraScript, ScreenFade, TriggerLayout, etc.
 * Acts as the bridge between the pure-Rust scripting VM and Bevy ECS world mutations.
 */
use crate::scroni::vm::*;
use bevy::prelude::*;
pub fn scroni_sys_event_observer(
    trigger: On<ScrOniSysEvent>,
    mut commands: Commands,
    assets: (
        ResMut<Assets<StandardMaterial>>,
        ResMut<Assets<Mesh>>,
        ResMut<Assets<Image>>,
        ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
        Res<AssetServer>,
        ResMut<Assets<bevy::audio::AudioSource>>,
        ResMut<crate::oni2_loader::TextureCollections>,
        ResMut<crate::oni2_loader::registries::EntityLibrary>,
        ResMut<crate::oni2_loader::registries::AnimRegistry>,
        Local<Option<crate::oni2_loader::parsers::td::SoundBankDirectory>>,
        Local<Option<crate::oni2_loader::parsers::audiopackages::AudioPackagesDirectory>>,
    ),
    layout_data: (
        Option<Res<crate::oni2_loader::LayoutContext>>,
        Option<Res<crate::oni2_loader::LayoutPaths>>,
    ),
    active_camera_package: Option<ResMut<crate::oni2_loader::ActiveCameraPackage>>,
    mut scroni_text_state: ResMut<ScroniTextState>,
    time: Res<Time>,
    screen_fade: Option<Res<ScreenFadeState>>,
    mut ai_target_query: Query<&mut crate::ai::components::AiFighter>,
    explosion_registry: Res<crate::oni2_loader::registries::ExplosionRegistry>,
    misc_queries: (
        Query<(
            &mut Transform,
            Option<&mut avian3d::prelude::LinearVelocity>,
            Option<&mut avian3d::prelude::Position>,
        )>,
        Query<&Children>,
        Query<&mut MeshMaterial3d<StandardMaterial>>,
        Query<(Entity, &mut crate::camera::components::CameraController, &mut crate::camera::channel::CameraChannel, Option<&crate::camera::components::ScriptCameraSequence>)>,
        Query<(&Name, Option<&mut PointLight>, Option<&mut SpotLight>)>,
        Query<(Entity, &mut ScrOniScript, Option<&Name>, Option<&mut ShaderLocals>)>,
        Query<(), With<crate::player::components::Player>>,
        Query<(Entity, &ActiveAmbientSound)>,
    ),
) {
    let ev = (*trigger).clone();
    let (mut transform_query, children_query, mut materials_query, mut camera_query, mut lights_query, mut script_query, player_query, ambient_sound_query) = misc_queries;
    let (
        mut materials,
        mut meshes,
        mut images,
        mut skinned_mesh_ibp,
        asset_server,
        mut audio_sources,
        mut texture_collections,
        mut entity_lib,
        mut anim_registry,
        mut td_directory,
        mut audio_packages,
    ) = assets;
    
    if td_directory.is_none() {
        *td_directory = Some(crate::oni2_loader::parsers::td::load_all_tds());
    }

    if audio_packages.is_none() {
        if let Ok(content) = crate::vfs::read_to_string("Audio", "rb.audiopackages") {
            *audio_packages = Some(crate::oni2_loader::parsers::audiopackages::parse_audiopackages(&content));
        } else {
            let assets_path = crate::get_assets_path();
            let pkgs_path = std::path::Path::new(assets_path).join("Audio").join("rb.audiopackages");
            if let Ok(content) = std::fs::read_to_string(&pkgs_path) {
                *audio_packages = Some(crate::oni2_loader::parsers::audiopackages::parse_audiopackages(&content));
            } else {
                warn!("Failed to load Audio/rb.audiopackages, audio package lookups will fail.");
                *audio_packages = Some(std::collections::HashMap::new());
            }
        }
    }

    match ev {
        ScrOniSysEvent::SetFullScreenColor { color, duration } => {
            let start_color = screen_fade.as_ref().map(|s| s.current_color).unwrap_or(Vec3::ONE);
            commands.insert_resource(ScreenFadeState {
                current_color: start_color,
                start_color,
                target_color: color,
                timer: 0.0,
                duration,
            });
        }
        ScrOniSysEvent::MakeExplosion { script_entity, name, orientation, at } => {
            info!("MakeExplosion requested: {} at {:?}", name, at);
            if let Some(def) = explosion_registry.explosions.get(&name.to_lowercase()) {
                let position = Vec3::new(at[0], at[1], at[2]);

                commands.spawn((
                    crate::explosion::ActiveExplosion {
                        name: name.clone(),
                        at: position,
                        timer: 0.0,
                        blast_duration: def.ellipsoid.as_ref().map(|e| e.blast_duration).unwrap_or(1.0),
                        spawn_source: script_entity, // Store who requested it
                    },
                    Transform::from_translation(position),
                    GlobalTransform::default(),
                ));

                for fx in &def.fx {
                    if fx.delay <= 0.0 {
                        commands.trigger(crate::fx_system::SpawnFx {
                            name: fx.fx_type.clone(),
                            at: Some(position),
                            parent: None,
                            start_active: true,
                        });
                        
                        commands.trigger(ScrOniSysEvent::PlaySound {
                            script_entity,
                            actor: None,
                            name: fx.fx_type.clone(),
                        });
                    }
                }
            } else {
                warn!("MakeExplosion {} not found in ExplosionRegistry", name);
            }
        }
        ScrOniSysEvent::PlaySound { script_entity, actor, name } => {
            let mut resolved_name = name.clone();
            let mut final_volume = 1.0;
            let mut final_pitch = 1.0;

            if let Some(pkgs) = audio_packages.as_ref() {
                if let Some((_, pkg)) = pkgs.iter().find(|(k, _)| k.eq_ignore_ascii_case(&name)) {
                    if !pkg.nuggets.is_empty() {
                        use rand::Rng;
                        let mut rng = rand::rng();
                        let idx = rng.random_range(0..pkg.nuggets.len());
                        let nugget = &pkg.nuggets[idx];
                        
                        resolved_name = nugget.sound.clone();
                        final_volume = nugget.volume * rng.random_range(nugget.random_min_volume..=nugget.random_max_volume);
                        final_pitch = nugget.pitch * rng.random_range(nugget.random_min_pitch..=nugget.random_max_pitch);
                    } else {
                        warn!("Audio package `{}` found, but it has no nuggets.", name);
                    }
                } else {
                    warn!("Audio package `{}` not found in rb.audiopackages (falling back to direct .td lookup)", name);
                }
            }

            if let Some(dir) = td_directory.as_ref() {
                if let Some((_, v)) = dir.sounds.iter().find(|(k, _)| k.eq_ignore_ascii_case(&resolved_name)) {
                    let bank_name = &v.0;
                    let vag_index = v.1;
                    let hd_name = format!("{}.hd", bank_name);
                    let bd_name = format!("{}.bd", bank_name);
                    
                    let hd_paths = [
                        hd_name.clone(),
                    ];
                    
                    let mut hd_bytes_opt = None;
                    for p in &hd_paths {
                        if let Ok(b) = crate::vfs::read("", p) {
                            hd_bytes_opt = Some(b);
                            break;
                        }
                    }
                    
                    if let Some(hd_bytes) = hd_bytes_opt {
                        if let Ok(header) = crate::oni2_loader::parsers::hd_bd::parse_hd(&hd_bytes) {
                            // Find the target subsong (1-indexed but vag_index is 0-indexed)
                            // Wait, the user said NUMVAGS 13, and the split has vag_index 12.
                            // The HD subsongs are usually accessed 1..=total_subsongs.
                            // So we need vag_index + 1
                            let target_index = vag_index + 1;
                            if let Some(subsong) = header.subsongs.iter().find(|s| s.index == target_index) {
                                let bd_paths = [
                                    bd_name.clone(),
                                    format!("Audio/banks/{}", bd_name),
                                ];
                                
                                let mut bd_bytes_opt = None;
                                for p in &bd_paths {
                                    if let Ok(b) = crate::vfs::read("", p) {
                                        bd_bytes_opt = Some(b);
                                        break;
                                    }
                                }
                                
                                if let Some(bd_bytes) = bd_bytes_opt {
                                    let start = subsong.stream_offset as usize;
                                    let end = start + subsong.stream_size as usize;
                                    if end <= bd_bytes.len() {
                                        let payload = &bd_bytes[start..end];
                                        if let Ok(pcm) = crate::oni2_loader::parsers::hd_bd::decode_psx_adpcm(payload, subsong.num_samples) {
                                            if let Ok(wav) = crate::oni2_loader::parsers::hd_bd::create_wav_bytes(&pcm, subsong.sample_rate, subsong.channels) {
                                                let source_handle = audio_sources.add(bevy::audio::AudioSource {
                                                    bytes: std::sync::Arc::from(wav),
                                                });
                                                
                                                // 2D Ambient Playback Requested
                                                commands.spawn((
                                                    bevy::audio::AudioPlayer(source_handle),
                                                    bevy::audio::PlaybackSettings {
                                                        mode: if subsong.loop_flag { bevy::audio::PlaybackMode::Loop } else { bevy::audio::PlaybackMode::Despawn },
                                                        volume: bevy::audio::Volume::Linear(final_volume),
                                                        speed: final_pitch,
                                                        ..Default::default()
                                                    },
                                                ));
                                            }
                                        }
                                    } else {
                                        warn!("Subsong payload overflows BD file.");
                                    }
                                } else {
                                    warn!("BD file not found in VFS: {}", bd_name);
                                }
                            } else {
                                warn!("Subsong {} not found in HD header.", target_index);
                            }
                        } else {
                            warn!("Failed to parse HD header for {}", hd_name);
                        }
                    } else {
                        warn!("HD file not found in VFS: {}", hd_name);
                    }
                } else {
                    if resolved_name != name {
                        warn!("Sound `{}` (resolved from package `{}`) not found in .td manifest directory.", resolved_name, name);
                    } else {
                        warn!("Sound `{}` not found in .td manifest directory.", name);
                    }
                }
            }
        }
        ScrOniSysEvent::At(x, y) => {
            scroni_text_state.current_x = x;
            scroni_text_state.current_y = y;
        }
        ScrOniSysEvent::DrawText(text) => {
            // Coordinate system is top-left based, so (0.5, 0.5) is center.
            let px = scroni_text_state.current_x * 100.0;
            let py = scroni_text_state.current_y * 100.0;

            commands.spawn((
                Text::new(text),
                TextFont {
                    font_size: 24.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(px),
                    top: Val::Percent(py),
                    ..default()
                },
                crate::menu::InGameEntity,
                ScroniTextElement {
                    // Ephemeral: lasts slightly longer than 1 frame at 60fps (16ms)
                    expires_at: time.elapsed_secs_f64() + 0.05,
                },
            ));
        }
        ScrOniSysEvent::CameraSetPackage(pkg_name) => {
            if let Some(mut active_pkg) = active_camera_package {
                if active_pkg.name != pkg_name {
                    info!("Changing active camera package from {} to {}", active_pkg.name, pkg_name);
                    active_pkg.name = pkg_name;
                }
            } else {
                warn!("CameraSetPackage called but no ActiveCameraPackage resource found.");
            }
            for (_cam_ent, mut controller, _, _) in &mut camera_query {
                controller.active_mode = crate::camera::components::ActiveCameraMode::GameNavigation;
            }
        }
        ScrOniSysEvent::CameraReset => {
            for (_, mut controller, mut channel, _) in &mut camera_query {
                controller.active_mode = crate::camera::components::ActiveCameraMode::GameNavigation;
                channel.script_override_transform = None;
            }
        }
        ScrOniSysEvent::CameraMode(mode) => {
            let active = match mode.as_str() {
                "navigation" => crate::camera::components::ActiveCameraMode::GameNavigation,
                "fight" => crate::camera::components::ActiveCameraMode::GameFighting,
                "targeting" => crate::camera::components::ActiveCameraMode::GameTargeting,
                _ => crate::camera::components::ActiveCameraMode::GameNavigation,
            };
            for (_, mut controller, _, _) in &mut camera_query {
                controller.active_mode = active;
            }
        }
        ScrOniSysEvent::CameraSetFOV(fov, dur) => {
            for (ent, mut controller, mut channel, seq_opt) in &mut camera_query {
                controller.active_mode = crate::camera::components::ActiveCameraMode::Script;
                let mut seq = seq_opt.cloned().unwrap_or_default();
                seq.fov_start = Some(channel.current_fov);
                seq.fov_target = Some(fov);
                seq.fov_duration = dur;
                seq.fov_time_elapsed = 0.0;
                commands.entity(ent).insert(seq);
            }
        }
        ScrOniSysEvent::CameraShake => {
            // Fixed shake parameters:
            // ShakeCamera(Vector3(0.1, 0.06, 0.03), 1.5s, 1.5s)
            for (_, _, mut channel, _) in &mut camera_query {
                channel.shake_amplitude = Vec3::new(0.1, 0.06, 0.03);
                channel.shake_time_remaining = 1.5;
                channel.shake_time_to_damp = 1.5;
            }
        }
        ScrOniSysEvent::CameraTrackActor(actor_ent) => {
            for (ent, mut controller, mut channel, seq_opt) in &mut camera_query {
                controller.active_mode = crate::camera::components::ActiveCameraMode::Script;
                channel.focus_actor = actor_ent;
                info!("📽️ [Camera] Script commanding TRACK on Actor {:?}", actor_ent);
                let mut seq = seq_opt.cloned().unwrap_or_default();
                seq.tracked_target = Some(crate::camera::components::ScriptFocusTarget::Actor(actor_ent));
                commands.entity(ent).insert(seq);
            }
        }
        ScrOniSysEvent::CameraTrackPoint(pt) => {
            for (ent, mut controller, _, seq_opt) in &mut camera_query {
                controller.active_mode = crate::camera::components::ActiveCameraMode::Script;
                let mut seq = seq_opt.cloned().unwrap_or_default();
                // Map the point with X and Z inverted (Oni right-handed mapping vs Bevy)
                let mapped_pt = Vec3::new(-pt.x, pt.y, -pt.z); 
                info!("📽️ [Camera] Script commanding TRACK on Point {}", mapped_pt);
                seq.tracked_target = Some(crate::camera::components::ScriptFocusTarget::Point(mapped_pt));
                commands.entity(ent).insert(seq);
            }
        }
        ScrOniSysEvent::CameraMoveToActor(actor_ent, dur) => {
            for (ent, mut controller, mut channel, seq_opt) in &mut camera_query {
                controller.active_mode = crate::camera::components::ActiveCameraMode::Script;
                channel.focus_actor = actor_ent;
                info!("📽️ [Camera] Script commanding MOVE to Actor {:?} over {:.2}s", actor_ent, dur);
                let mut seq = seq_opt.cloned().unwrap_or_default();
                seq.move_target = Some(crate::camera::components::ScriptFocusTarget::Actor(actor_ent));
                seq.move_duration = dur;
                seq.move_time_elapsed = 0.0;
                seq.move_start = None; // lazily captured from camera position on first script tick
                commands.entity(ent).insert(seq);
            }
        }
        ScrOniSysEvent::CameraMoveToPoint(pt, dur) => {
            for (ent, mut controller, _, seq_opt) in &mut camera_query {
                controller.active_mode = crate::camera::components::ActiveCameraMode::Script;
                let mut seq = seq_opt.cloned().unwrap_or_default();
                let mapped_pt = Vec3::new(-pt.x, pt.y, -pt.z);
                info!("📽️ [Camera] Script commanding MOVE to Point {} over {:.2}s", mapped_pt, dur);
                seq.move_target = Some(crate::camera::components::ScriptFocusTarget::Point(mapped_pt));
                seq.move_duration = dur;
                seq.move_time_elapsed = 0.0;
                seq.move_start = None; // lazily captured from camera position on first script tick
                commands.entity(ent).insert(seq);
            }
        }
        ScrOniSysEvent::CameraMoveAlongRail(rail, dur) => {
            for (ent, mut controller, _, seq_opt) in &mut camera_query {
                controller.active_mode = crate::camera::components::ActiveCameraMode::Script;
                info!("📽️ [Camera] Script commanding MOVE along Rail '{}' over {:.2}s", rail, dur);
                let mut seq = seq_opt.cloned().unwrap_or_default();
                seq.active_rail_name = Some(rail.clone());
                seq.rail_duration = dur;
                commands.entity(ent).insert(seq);
            }
        }
        ScrOniSysEvent::TextureMovie {
            script_entity,
            target_name,
            action,
            arg,
        } => {
            match action {
                super::ast::TextureMovieAction::SetFrame => {
                    let frame = arg.as_int() as usize;

                    // Get preloaded texture handle directly from the collections resource
                    if let Some(frames) = texture_collections.collections.get(&target_name) {
                        if frame < frames.len() {
                            let tex_handle = frames[frame].clone();
                            let mut stack = vec![script_entity];
                            while let Some(ent) = stack.pop() {
                                if let Ok(mut mat_handle) = materials_query.get_mut(ent) {
                                    if let Some(old_mat) = materials.get(&mat_handle.0) {
                                        let mut new_mat = old_mat.clone();
                                        new_mat.base_color_texture = Some(tex_handle.clone());
                                        new_mat.base_color = Color::WHITE;
                                        let new_handle = materials.add(new_mat);
                                        mat_handle.0 = new_handle;
                                    }
                                }
                                if let Ok(children) = children_query.get(ent) {
                                    stack.extend(children.iter());
                                }
                            }
                        } else {
                            warn!(
                                "TextureMovie SetFrame {} out of bounds for {}",
                                frame, target_name
                            );
                        }
                    } else {
                        warn!(
                            "TextureMovie: No preloaded textures found for {}",
                            target_name
                        );
                    }
                }
                _ => {}
            }
        }
        ScrOniSysEvent::Spawn {
            script_entity: _,
            script,
            assign_to,
            at,
            name,
        } => {
            info!(
                "Received spawn request: script={}, at={:?}, name={:?}",
                script, at, name
            );

            // Scripts operate purely in ONI coordinates recursively natively. Translate down into Bevy spatial coordinates physically here.
            let pos_opt = at.map(|p| Vec3::new(-p.x, p.y, -p.z));
            let actor_name = name.clone().unwrap_or(script.clone());

            if let (Some(ctx), Some(paths)) = (layout_data.0.as_ref(), layout_data.1.as_ref()) {
                let mut spawn_assets = crate::oni2_loader::SpawnAssets {
                    commands: &mut commands,
                    meshes: &mut meshes,
                    materials: &mut materials,
                    images: &mut images,
                    skinned_mesh_ibp: &mut skinned_mesh_ibp,
                    entity_lib: &mut entity_lib,
                    anim_registry: &mut anim_registry,
                    texture_collections: &mut texture_collections,
                };

                // Call the shared spawn function
                if let Some((_new_entity, _actor)) = crate::oni2_loader::spawn_layout_actor(
                    &mut spawn_assets,
                    &script,
                    ctx,
                    paths,
                    pos_opt,
                    true,
                    Some(&actor_name),
                ) {
                    info!(
                        "Spawned {} with position override {:?}",
                        actor_name, pos_opt
                    );
                    if let Some(var_name) = assign_to {
                        warn!(
                            "Assigning spawn result to {} is not yet supported synchronously.",
                            var_name
                        );
                    }
                    return; // Successfully spawned!
                } else {
                    warn!(
                        "Failed to spawn actor {} using spawn_layout_actor",
                        actor_name
                    );
                }
            } else {
                warn!(
                    "Spawn command needs a LayoutContext and LayoutPaths resource to fully spawn {}.",
                    script
                );
            }

            // Fallback Stub: spawn a basic entity placeholder instead if proper spawning fails
            let pos_fallback = pos_opt.unwrap_or(Vec3::ZERO);
            let _new_entity = commands
                .spawn((
                    Transform::from_translation(pos_fallback),
                    Visibility::Visible,
                    crate::oni2_loader::Oni2Entity {
                        name: actor_name.clone(),
                    },
                    Name::new(actor_name.clone()),
                    crate::menu::InGameEntity,
                ))
                .id();

            if let Some(var_name) = assign_to {
                warn!(
                    "Assigning spawn result to {} is not yet supported synchronously.",
                    var_name
                );
            }
        }
        ScrOniSysEvent::Teleport {
            script_entity,
            target,
            to,
            face,
        } => {
            let mut caller_name = "UnknownScript".to_string();
            if let Ok((_, script, _, _)) = script_query.get(script_entity) {
                caller_name = script.exec.main_thread.script.name.clone();
            }
            
            if player_query.get(target).is_ok() {
                info!("VM: [{}] Requested TELEPORT on PLAYER! Raw coords: {:?}, face: {:?}", caller_name, to, face);
            }

            if let Ok((mut transform, mut opt_vel, mut opt_phys_pos)) = transform_query.get_mut(target) {
                if let Some(pos) = to {
                    let bevy_pos = Vec3::new(-pos.x, pos.y, -pos.z);
                    if player_query.get(target).is_ok() {
                        info!("VM: [{}] Translating PLAYER to mapped Bevy Coords: {:?}", caller_name, bevy_pos);
                    }
                    transform.translation = bevy_pos;
                    // Also update Avian3D's authoritative Position so the physics engine
                    // doesn't overwrite the translation on the next physics step.
                    if let Some(ref mut phys_pos) = opt_phys_pos {
                        phys_pos.0 = bevy_pos;
                    }
                    commands
                        .entity(target)
                        .insert(crate::oni2_loader::spawn::NeedsGroundSnap {
                            origin: bevy_pos,
                            wait_frames: 4,
                            capsule_half_height: 1.0,
                        });
                }
                if let Some(angles_y) = face {
                    let rad = angles_y.to_radians();
                    let current_rot = transform.rotation.to_euler(EulerRot::YXZ);
                    transform.rotation =
                        Quat::from_euler(EulerRot::YXZ, rad, current_rot.1, current_rot.2);
                }

                if let Some(vel) = opt_vel.as_deref_mut() {
                    vel.0 = Vec3::ZERO;
                }
            }
        }
        ScrOniSysEvent::MakeFx {
            script_entity,
            name,
            at,
        } => {
            commands.trigger(crate::fx_system::SpawnFx {
                name: name,
                at: at,
                parent: Some(script_entity),
                start_active: true,
            });
        }
        ScrOniSysEvent::SendAction {
            action,
            target,
            component,
        } => {
            if component.eq_ignore_ascii_case("fx") {
                commands.trigger(crate::fx_system::FxAction {
                    action: action.clone(),
                    target: target,
                });
            } else if component.eq_ignore_ascii_case("actorspecific") {
                commands.trigger(crate::door::ActorSpecificAction {
                    action: action.clone(),
                    target: target,
                });
            } else {
                warn!("SendAction: Unrecognized component '{}'", component);
            }
        }
        ScrOniSysEvent::ControlHead { actor, task } => {
            use crate::oni2_loader::components::ActiveHeadIK;
            if let Ok(mut entity_cmds) = commands.get_entity(actor.clone()) {
                entity_cmds.insert(ActiveHeadIK { task: task.clone() });
                debug!("VM: Observed ControlHead {:?} onto actor {:?}", task, actor);
            }
        }
        ScrOniSysEvent::SetAiTarget { actor, target } => {
            // [AUDIT]: Prototype leakage. Direct writes to `ai.target` and hardcoding `AiState::Pursuing`.
            // A real combat AI should interpret "Target = X" as an input, letting its behavior tree manage its own enum transitions.
            if let Ok(mut ai) = ai_target_query.get_mut(actor) {
                ai.target = Some(target);
                ai.manual_target = true; // Lock it so auto-awareness doesn't overwrite it immediately.
                ai.state = crate::ai::components::AiState::Pursuing;
            }
            info!("VM: AI {:?} ordered to Attack {:?}", actor, target);
        }
        ScrOniSysEvent::TriggerFight { actor, target } => {
            if let Ok(mut ai) = ai_target_query.get_mut(actor) {
                ai.state = crate::ai::components::AiState::Pursuing;
                if let Some(t) = target {
                    ai.target = Some(t);
                    ai.manual_target = true; // Lock the optional target
                } else {
                    ai.manual_target = false; // Unlock so auto-awareness can pick nearest targets
                }
            }
        }
        ScrOniSysEvent::FollowActor { actor, target } => {
            // [AUDIT]: Prototype leakage. `ActorFollower` is a hacky prototype component used for steering.
            // "Follow" simply paths closely to the target but does not engage in combat. 
            // We assign an active ActorFollower pointing directly to the target.
            commands.entity(actor).insert(crate::ai::components::ActorFollower {
                target,
                within: 3.0,
            });
            info!("VM: AI {:?} ordered to Follow {:?}", actor, target);
        }
        ScrOniSysEvent::SetLightIntensity {
            script_entity: _,
            light,
            intensity,
        } => {
            for (name, mut point, mut spot) in &mut lights_query {
                if name.as_str().eq_ignore_ascii_case(&light) {
                    // Multiply scaling heuristic to adapt Oni floats to PBR luminous intensity
                    if let Some(p) = point.as_deref_mut() {
                        p.intensity = intensity * 100.0;
                    }
                    if let Some(s) = spot.as_deref_mut() {
                        s.intensity = intensity * 100.0;
                    }
                }
            }
        }
        ScrOniSysEvent::SetShaderLocal {
            script_entity,
            name,
            val,
        } => {
            if let Ok((_, _, _, mut locals_opt)) = script_query.get_mut(script_entity) {
                if let Some(mut locals) = locals_opt.as_deref_mut() {
                    locals.locals.insert(name.clone(), val);
                } else {
                    let mut locals = ShaderLocals::default();
                    locals.locals.insert(name.clone(), val);
                    commands.entity(script_entity).insert(locals);
                }
            } else {
                let mut locals = ShaderLocals::default();
                locals.locals.insert(name.clone(), val);
                commands.entity(script_entity).insert(locals);
            }
        }
        ScrOniSysEvent::SetUpdateState { target, state } => {
            let active = state.eq_ignore_ascii_case("Active");
            let target_hash = target.parse::<i32>().ok();

            for (entity, mut script, name_opt, _) in &mut script_query {
                if let Some(n) = name_opt {
                    let mut is_match = n.as_str() == target;

                    if !is_match {
                        if let Some(h) = target_hash {
                            let mut hasher = std::collections::hash_map::DefaultHasher::new();
                            std::hash::Hash::hash(n.as_str(), &mut hasher);
                            let hashed = (std::hash::Hasher::finish(&hasher) % 100000) as i32;
                            if hashed == h {
                                is_match = true;
                            }
                        }
                    }

                    if is_match {
                        if active {
                            commands
                                .entity(entity)
                                .remove::<crate::oni2_loader::components::ActorAsleep>();
                        } else {
                            commands
                                .entity(entity)
                                .insert(crate::oni2_loader::components::ActorAsleep);
                        }
                        script.exec.active = active;
                        info!(
                            "SetUpdateState: toggled '{}' script active={}",
                            n.as_str(),
                            active
                        );
                    }
                }
            }
        }
        ScrOniSysEvent::UsePad { script_entity } => {
            // [AUDIT]: Prototype leakage. Using `AiFighter` exclusively as an "is AI" boolean marker.
            // True AI character structures likely have many more components that need pausing/disabling during possession!
            commands
                .entity(script_entity)
                .insert(crate::player::components::Player);
            commands
                .entity(script_entity)
                .remove::<crate::ai::components::AiFighter>();
            for (_, _, mut channel, _) in &mut camera_query {
                channel.focus_actor = script_entity;
            }
            info!("VM: Actor {:?} took player pad controls", script_entity);
        }
        ScrOniSysEvent::PlayAmbientSound {
            script_entity: _,
            handle,
            name,
            mut volume,
            mut pitch,
            volume_ramp,
            pitch_ramp,
        } => {
            let mut source_handle = None;
            let mut file_name = name.clone();
            if file_name.starts_with("Stream:") {
                let mut stream_name = file_name.replace("Stream:", "");
                stream_name.push_str(".stm");
                
                if let Ok(bytes) = crate::vfs::read("", &stream_name) {
                    if let Ok(decoded) = crate::oni2_loader::parsers::stm::decode_stm(&bytes) {
                        if let Ok(wav) = crate::oni2_loader::parsers::stm::create_wav_bytes(&decoded) {
                            source_handle = Some(audio_sources.add(bevy::audio::AudioSource {
                                bytes: std::sync::Arc::from(wav),
                            }));
                        } else {
                            warn!("Failed to create wav for stream: {}", stream_name);
                        }
                    } else {
                        warn!("Failed to decode stream: {}", stream_name);
                    }
                } else {
                    warn!("Stream file not found in VFS: {}", stream_name);
                }
            } else if file_name.starts_with("SFX_Blast_Chamber:") || file_name.starts_with("SFX_") {
                let mut resolved_name = file_name.clone();
                let mut p_vol = 1.0;
                let mut p_pitch = 1.0;
                
                if let Some(pkgs) = audio_packages.as_ref() {
                    if let Some((_, pkg)) = pkgs.iter().find(|(k, _)| k.eq_ignore_ascii_case(&resolved_name)) {
                        if !pkg.nuggets.is_empty() {
                            use rand::Rng;
                            let mut rng = rand::rng();
                            let idx = rng.random_range(0..pkg.nuggets.len());
                            let nugget = &pkg.nuggets[idx];
                            resolved_name = nugget.sound.clone();
                            p_vol = nugget.volume * rng.random_range(nugget.random_min_volume..=nugget.random_max_volume);
                            p_pitch = nugget.pitch * rng.random_range(nugget.random_min_pitch..=nugget.random_max_pitch);
                        }
                    }
                }
                
                if let Some(dir) = td_directory.as_ref() {
                    if let Some((_, v)) = dir.sounds.iter().find(|(k, _)| k.eq_ignore_ascii_case(&resolved_name)) {
                        let bank_name = &v.0;
                        let vag_index = v.1;
                        let hd_name = format!("{}.hd", bank_name);
                        let bd_name = format!("{}.bd", bank_name);
                        let hd_paths = [hd_name.clone()];
                        let mut hd_bytes_opt = None;
                        for p in &hd_paths {
                            if let Ok(b) = crate::vfs::read("", p) {
                                hd_bytes_opt = Some(b);
                                break;
                            }
                        }
                        if let Some(hd_bytes) = hd_bytes_opt {
                            if let Ok(header) = crate::oni2_loader::parsers::hd_bd::parse_hd(&hd_bytes) {
                                let target_index = vag_index + 1;
                                if let Some(subsong) = header.subsongs.iter().find(|s| s.index == target_index) {
                                    let bd_paths = [bd_name.clone(), format!("Audio/banks/{}", bd_name)];
                                    let mut bd_bytes_opt = None;
                                    for p in &bd_paths {
                                        if let Ok(b) = crate::vfs::read("", p) {
                                            bd_bytes_opt = Some(b);
                                            break;
                                        }
                                    }
                                    if let Some(bd_bytes) = bd_bytes_opt {
                                        let start = subsong.stream_offset as usize;
                                        let end = start + subsong.stream_size as usize;
                                        if end <= bd_bytes.len() {
                                            let payload = &bd_bytes[start..end];
                                            if let Ok(pcm) = crate::oni2_loader::parsers::hd_bd::decode_psx_adpcm(payload, subsong.num_samples) {
                                                if let Ok(wav) = crate::oni2_loader::parsers::hd_bd::create_wav_bytes(&pcm, subsong.sample_rate, subsong.channels) {
                                                    source_handle = Some(audio_sources.add(bevy::audio::AudioSource {
                                                        bytes: std::sync::Arc::from(wav),
                                                    }));
                                                    
                                                    // Apply package multipliers to our volume arguments
                                                    if let Some(target) = volume.as_mut() {
                                                        *target *= p_vol;
                                                    } else {
                                                        volume = Some(p_vol);
                                                    }
                                                    if let Some(target) = pitch.as_mut() {
                                                        *target *= p_pitch;
                                                    } else {
                                                        pitch = Some(p_pitch);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                info!("Unknown audio prefix for playambientsound: {}", file_name);
                return;
            }

            let Some(source_handle) = source_handle else {
                return;
            };
            let mut settings = bevy::audio::PlaybackSettings::DESPAWN;
            if let Some(vol) = volume {
                settings = settings.with_volume(bevy::audio::Volume::Linear(vol));
            } else if let Some((start_vol, _, _)) = volume_ramp {
                settings = settings.with_volume(bevy::audio::Volume::Linear(start_vol));
            }
            if let Some(p) = pitch {
                settings = settings.with_speed(p);
            }
            else if let Some((start_pitch, _, _)) = pitch_ramp {
                settings = settings.with_speed(start_pitch);
            }

            let mut ent = commands.spawn((
                bevy::audio::AudioPlayer(source_handle),
                settings,
                ActiveAmbientSound { handle },
            ));

            if let Some((start_vol, end_vol, duration)) = volume_ramp {
                ent.insert(AudioVolumeRamp {
                    start_vol,
                    end_vol,
                    duration,
                    elapsed: 0.0,
                });
            }

            if let Some((start_pitch, end_pitch, duration)) = pitch_ramp {
                ent.insert(AudioPitchRamp {
                    start_pitch,
                    end_pitch,
                    duration,
                    elapsed: 0.0,
                });
            }
        }
        ScrOniSysEvent::AmbientSoundVolumeRamp {
            script_entity: _,
            handle,
            target_vol,
            duration,
        } => {
            for (entity, active_sound) in &ambient_sound_query {
                if active_sound.handle == handle {
                    info!("VM: Applying AmbientSoundVolumeRamp to handle {}", handle);
                    commands.entity(entity).insert(AudioVolumeRamp {
                        start_vol: -1.0, // Marker to initialize from current sink volume
                        end_vol: target_vol,
                        duration,
                        elapsed: 0.0,
                    });
                }
            }
        }
        ScrOniSysEvent::AmbientSoundPitchRamp {
            script_entity: _,
            handle,
            target_pitch,
            duration,
        } => {
            for (entity, active_sound) in &ambient_sound_query {
                if active_sound.handle == handle {
                    info!("VM: Applying AmbientSoundPitchRamp to handle {}", handle);
                    commands.entity(entity).insert(AudioPitchRamp {
                        start_pitch: -1.0,
                        end_pitch: target_pitch,
                        duration,
                        elapsed: 0.0,
                    });
                }
            }
        }
        ScrOniSysEvent::AmbientSoundStop { script_entity: _, handle } => {
            for (entity, active_sound) in &ambient_sound_query {
                if active_sound.handle == handle {
                    info!("VM: Despawning AmbientSound handle {}", handle);
                    commands.entity(entity).try_despawn();
                }
            }
        }
        ScrOniSysEvent::Destroy(target) => {
            info!("VM: Received native script request to permanently despawn {:?}", target);
            if let Ok(mut entity_cmds) = commands.get_entity(target) {
                entity_cmds.despawn();
            }
        }
    }
}
