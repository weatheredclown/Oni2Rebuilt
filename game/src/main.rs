mod ai;
mod camera;
mod combat;
mod filesystem;
mod hud;
mod menu;
mod oni2_loader;
mod player;
mod scroni;
mod telemetry;
mod fx_system;
mod projectile_system;
pub use filesystem::dave_vfs;
pub use filesystem::vfs;

use avian3d::prelude::*;
use bevy::diagnostic::{FrameTimeDiagnosticsPlugin, DiagnosticsStore};
use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use uuid::Uuid;

use camera::components::{CameraRig, PrototypeElement};
use combat::components::*;
use menu::{AppState, InGameEntity, SelectedLayout};
use oni2_loader::TestAnimMode;
use player::components::*;
use std::sync::OnceLock;
use vfs::DiskVfs;

pub static ASSETS_PATH: OnceLock<String> = OnceLock::new();
pub static ASSETS_DAT: OnceLock<String> = OnceLock::new();

pub fn get_assets_path() -> &'static str {
    ASSETS_PATH.get().map(|s| s.as_str()).unwrap_or("oni2/zips/assets")
}

pub fn get_assets_dat() -> &'static str {
    ASSETS_DAT.get().map(|s| s.as_str()).unwrap_or("RB.DAT")
}

/// Resource indicating sandbox mode (flat ground + model, no layout).
#[derive(Resource)]
struct SandboxMode;

/// Resource indicating formation inspection mode.
#[derive(Resource)]
struct FormationMode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    
    let cli_path = args.windows(2).find_map(|w| {
        if w[0] == "--path" {
            Some(w[1].clone())
        } else {
            None
        }
    });

    if let Some(ref p) = cli_path {
        ASSETS_PATH.set(p.clone()).ok();
    }
    
    let cli_dat = args.windows(2).find_map(|w| {
        if w[0] == "--dat" {
            Some(w[1].clone())
        } else {
            None
        }
    });

    if let Some(ref p) = cli_dat {
        ASSETS_DAT.set(p.clone()).ok();
    }

    let cli_layout = args.windows(2).find_map(|w| {
        if w[0] == "--layout" {
            Some(w[1].clone())
        } else {
            None
        }
    });
    let cli_testanim = args.windows(2).find_map(|w| {
        if w[0] == "--testanim" || w[0] == "--animtest" {
            Some(w[1].clone())
        } else {
            None
        }
    });
    let sandbox_mode = args.iter().any(|a| a == "--sandbox");
    let formation_mode = args.iter().any(|a| a == "--formation");
    let diagnostics_mode = args.iter().any(|a| a == "--diagnostics");
    let fog_enabled = args.iter().any(|a| a == "--fog");

    let mut app = App::new();

    let disk_vfs = Box::new(vfs::DiskVfs::new(get_assets_path().to_string()));
    
    let dave_path_str = get_assets_dat();
    let dave_path = std::path::Path::new(dave_path_str);
        
    if dave_path.exists() {
        println!("Found Dave archive at {:?}, enabling DaveVfs", dave_path);
        match dave_vfs::DaveVfs::new(dave_path_str) {
            Ok(dave_vfs) => {
                if cli_path.is_some() {
                    println!("--path provided. Using FallbackVfs (Disk primary, Dave fallback).");
                    let fallback = Box::new(vfs::FallbackVfs::new(disk_vfs, Box::new(dave_vfs)));
                    vfs::set_vfs(fallback);
                } else {
                    println!("No --path provided. Using DaveVfs exclusively.");
                    vfs::set_vfs(Box::new(dave_vfs));
                }
            }
            Err(e) => {
                println!("Failed to initialize DaveVfs: {}", e);
                println!("Falling back to DiskVfs only.");
                vfs::set_vfs(disk_vfs);
            }
        }
    } else {
        println!("Using DiskVfs only at {}", get_assets_path());
        vfs::set_vfs(disk_vfs);
    }

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "rb-reborn".to_string(),
            ..default()
        }),
        ..default()
    }))
    .insert_resource(Time::<Fixed>::from_hz(60.0))
    .add_plugins(PhysicsPlugins::default())
    .add_plugins(avian3d::debug_render::PhysicsDebugPlugin)
    .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
    .add_plugins(FrameTimeDiagnosticsPlugin::default())
    .add_plugins(telemetry::TelemetryPlugin)
    .add_plugins(menu::MenuPlugin)
    .add_plugins(combat::CombatPlugin)
    .add_plugins(player::PlayerPlugin)
    .add_plugins(ai::AiPlugin)
    .add_plugins(camera::CameraPlugin)
    .add_plugins(hud::HudPlugin)
    .add_plugins(fx_system::FxPlugin)
    .add_plugins(projectile_system::ProjectilePlugin)
    .insert_resource(oni2_loader::DebugBoundsVisible(false))
    .insert_resource(oni2_loader::DebugSkeletonVisible(false))
    .insert_resource(oni2_loader::PointCloudMode(false));

    if fog_enabled {
        app.insert_resource(oni2_loader::FogEnabled);
    }

    app.init_resource::<scroni::vm::ScroniTextState>()
       .init_resource::<oni2_loader::DebugLightGridState>()
       .init_resource::<oni2_loader::registries::EntityLibrary>()
       .init_resource::<oni2_loader::registries::AnimRegistry>()
       .init_resource::<oni2_loader::registries::ProjLibrary>()
       .init_resource::<oni2_loader::registries::FxLibrary>()
       .init_resource::<oni2_loader::registries::ParticleLibrary>()
       .add_observer(scroni::vm::scroni_sys_event_observer);

    app
    .add_systems(
        OnEnter(AppState::InGame),
        setup_scene.run_if(
            not(resource_exists::<TestAnimMode>)
                .and(not(resource_exists::<FormationMode>)),
        ),
    )
    .add_systems(
        OnEnter(AppState::InGame),
        setup_formation_scene.run_if(resource_exists::<FormationMode>),
    )
    .add_systems(
        OnEnter(AppState::InGame),
        oni2_loader::setup_testanim_scene.run_if(resource_exists::<TestAnimMode>),
    )
    .add_systems(
        Update,
        free_camera_system
            .run_if(resource_exists::<FormationMode>)
            .run_if(in_state(AppState::InGame)),
    )
    .add_systems(
        Update,
        (
            oni2_loader::toggle_debug_bounds,
            oni2_loader::toggle_debug_skeleton,
            oni2_loader::toggle_point_cloud,
            oni2_loader::update_oni2_animation,
            oni2_loader::creature_movement_anim_system,
            oni2_loader::ground_snap_system,
            oni2_loader::apply_fog_to_camera
                .run_if(resource_exists::<oni2_loader::FogEnabled>),
            oni2_loader::update_skyhat,
            scroni::vm::update_broadcast_triggers.before(scroni::vm::scroni_tick_system),
            scroni::vm::scroni_tick_system,
            scroni::vm::cleanup_scroni_text,
            oni2_loader::scroni_curve_bridge_system,
            oni2_loader::curve_follower_system,
        )
            .run_if(in_state(AppState::InGame)),
    )
    .add_systems(
        Update,
        (
            oni2_loader::debug_draw_bounds,
            oni2_loader::debug_draw_capsules,
        )
            .run_if(in_state(AppState::InGame))
            .run_if(|v: Res<oni2_loader::DebugBoundsVisible>| v.0),
    )
    .add_systems(
        Update,
        oni2_loader::debug_draw_skeleton
            .run_if(in_state(AppState::InGame))
            .run_if(|v: Res<oni2_loader::DebugSkeletonVisible>| v.0),
    )
    .add_systems(
        Update,
        (
            oni2_loader::testanim_input_system,
            oni2_loader::update_testanim_hud,
            oni2_loader::orbit_camera_system,
        ).run_if(in_state(AppState::InGame).and(resource_exists::<TestAnimMode>)),
    )
    .add_systems(Startup, (setup_fps_counter, disable_physics_debug, oni2_loader::load_global_registries))
    .add_systems(Update, (update_fps_counter, toggle_physics_debug, toggle_debug_light, oni2_loader::toggle_debug_fog, oni2_loader::toggle_debug_light_grid, oni2_loader::update_debug_light_grid));

    if diagnostics_mode {
        app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
    }

    if let Some(anim_path) = cli_testanim {
        if anim_path.to_lowercase().ends_with(".anim") {
            app.insert_resource(TestAnimMode(anim_path));
            app.insert_state(AppState::InGame);
        } else {
            app.insert_resource(crate::menu::TestAnimEntity(anim_path));
            app.insert_state(AppState::AnimMenu);
        }
    } else if formation_mode {
        app.insert_resource(FormationMode);
        app.insert_state(AppState::InGame);
    } else if sandbox_mode {
        app.insert_resource(SandboxMode);
        app.insert_state(AppState::InGame);
    } else if let Some(layout_name) = cli_layout {
        app.insert_resource(SelectedLayout(layout_name));
        app.insert_state(AppState::LoadingLayout);
    } else {
        app.init_state::<AppState>();
    }


    app.run();
}

fn setup_scene(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut skinned_mesh_ibp: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut entity_lib: ResMut<crate::oni2_loader::registries::EntityLibrary>,
    mut anim_registry: ResMut<crate::oni2_loader::registries::AnimRegistry>,
    selected_layout: Option<Res<SelectedLayout>>,
    sandbox: Option<Res<SandboxMode>>,
) {
    // Create combat materials
    let fist_startup = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 1.0, 0.2),
        emissive: LinearRgba::new(1.0, 1.0, 0.2, 1.0),
        ..default()
    });
    let fist_active = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.4, 0.1),
        emissive: LinearRgba::new(3.0, 1.2, 0.3, 1.0),
        ..default()
    });
    let fist_recovery = materials.add(StandardMaterial {
        base_color: Color::srgb(0.8, 0.5, 0.2),
        emissive: LinearRgba::new(0.4, 0.25, 0.1, 1.0),
        ..default()
    });
    let shield_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.0, 0.8, 1.0, 0.5),
        emissive: LinearRgba::new(0.0, 0.5, 0.8, 1.0),
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    let fist_mesh = meshes.add(Sphere::new(0.15));
    let shield_mesh = meshes.add(Circle::new(0.5));


    commands.insert_resource(CombatMaterials {
        fist_startup: fist_startup.clone(),
        fist_active: fist_active.clone(),
        fist_recovery: fist_recovery.clone(),
        shield: shield_mat.clone(),
        fist_mesh: fist_mesh.clone(),
        shield_mesh: shield_mesh.clone(),
    });

    let scoped = InGameEntity;

    // Determine layout path
    let layout_name = selected_layout
        .as_ref()
        .map(|s| s.0.as_str())
        .unwrap_or("tim06");
    let layout_path = format!("layout/{}", layout_name);
    // Spawn pos will be determined after layout loads (from Player="1" creature)
    let fallback_spawn = oni2_loader::find_konoko_spawn(&layout_path)
        .map(|p| p + Vec3::Y * 1.0)
        .unwrap_or(Vec3::new(0.0, 2.0, 0.0));

    // Invisible safety floor so nothing falls forever
    commands.spawn((
        Transform::from_xyz(0.0, -150.0, 0.0),
        RigidBody::Static,
        Collider::half_space(Vec3::Y),
        scoped.clone(),
    ));

    // === Load ONI2 Layout or Sandbox (before player spawn so we know the spawn point) ===
    let layout_player_info = if sandbox.is_some() {
        // Sandbox mode: flat ground plane + kno model
        commands.spawn((
            Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(50.0)))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.3, 0.35, 0.3),
                ..default()
            })),
            Transform::default(),
            RigidBody::Static,
            Collider::half_space(Vec3::Y),
            scoped.clone(),
        ));

        // Load kno entity
        let entity_path_kno = "Entity/kno".to_string();
        oni2_loader::spawn_oni2_entity(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &mut skinned_mesh_ibp,
            &mut entity_lib,
            &mut anim_registry,
            &entity_path_kno,
            Vec3::new(0.0, 2.0, 0.0),
            "kno",
        );
        None
    } else {
        let entity_base_str = "Entity".to_string();
        oni2_loader::load_layout(
            &mut commands,
            &asset_server,
            &mut meshes,
            &mut materials,
            &mut images,
            &mut skinned_mesh_ibp,
            &mut entity_lib,
            &mut anim_registry,
            &layout_path,
            &entity_base_str,
        )
    };

    // Fallback lights for sandbox mode (layout mode gets lights from layout.lights / default.environment)
    if sandbox.is_some() {
        commands.spawn((
            DirectionalLight {
                illuminance: 10_000.0,
                shadows_enabled: true,
                ..default()
            },
            Transform::from_xyz(50.0, 50.0, 50.0).looking_at(Vec3::ZERO, Vec3::Y),
            scoped.clone(),
        ));
        commands.spawn((
            AmbientLight {
                color: Color::WHITE,
                brightness: 300.0,
                ..default()
            },
            scoped.clone(),
        ));
    }

    // Attach player components to the creature entity from the layout,
    // or spawn a fallback capsule if no layout player was found (sandbox mode).
    let player_id = if let Some(ref pi) = layout_player_info {
        // The creature entity already has: model, physics capsule, animation library.
        // Just add player + combat components to it.
        commands.entity(pi.entity).insert((
            scoped.clone(),
            Player,
            InputState::default(),
            Fighter::default(),
            FighterId(Uuid::new_v4()),
            Health::new(100.0),
        ));
        commands.entity(pi.entity).insert((
            AttackState::default(),
            BlockState::new(),
            ComboTracker::default(),
            SuperMeter::default(),
            GrabState::default(),
            HitReaction::default(),
            AboutToBeHit::default(),
        ));
        pi.entity
    } else {
        // Sandbox fallback: spawn a blue capsule as the player
        commands
            .spawn((
                Mesh3d(meshes.add(Capsule3d::new(0.4, 1.2))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.2, 0.4, 0.9),
                    ..default()
                })),
                Transform::from_translation(fallback_spawn),
                scoped.clone(),
                (
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
                ),
                PrototypeElement,
                (
                    Player,
                    InputState::default(),
                    Fighter::default(),
                    FighterId(Uuid::new_v4()),
                    Health::new(100.0),
                ),
                (
                    AttackState::default(),
                    BlockState::new(),
                    ComboTracker::default(),
                    SuperMeter::default(),
                    GrabState::default(),
                    HitReaction::default(),
                    AboutToBeHit::default(),
                ),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Mesh3d(fist_mesh.clone()),
                    MeshMaterial3d(fist_startup.clone()),
                    Transform::from_translation(Vec3::new(0.3, 0.3, -0.5)),
                    Visibility::Hidden,
                    FistVisual,
                ));
                parent.spawn((
                    Mesh3d(shield_mesh.clone()),
                    MeshMaterial3d(shield_mat.clone()),
                    Transform::from_translation(Vec3::new(0.0, 0.3, -0.6))
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
                    Visibility::Hidden,
                    ShieldVisual,
                ));
            })
            .id()
    };

    // Camera with zone-based rig
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 8.0, -12.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        scoped,
        IsDefaultUiCamera,
        CameraRig {
            target: player_id,
            mode: camera::components::CameraMode::MouseLook,
            // Mouse-look fields
            offset: Vec3::new(0.0, 7.0, -12.0),
            mouse_lerp_speed: 5.0,
            // Zone-based fields
            current_azimuth: 0.0,
            target_azimuth: 0.0,
            zone_thresholds: [
                20.0_f32.to_radians(),
                90.0_f32.to_radians(),
                120.0_f32.to_radians(),
            ],
            zone_lerp_rates: [2.0, 3.0, 3.0, 3.0],
            spin_threshold: 63.0_f32.to_radians(),
            dead_zone_inner: 1.5,
            dead_zone_outer: 4.0,
            incline_offset: 10.0_f32.to_radians(),
            follow_distance: 12.0,
            height: 7.0,
            bump_angle: 0.0,
            bump_lerp_rate: 4.0,
            free_yaw: 0.0,
            free_pitch: -0.1,
            free_speed: 10.0,
            pre_free_mode: None,
        },
    ));
}

/// Marker for the free-fly camera in formation mode.
#[derive(Component)]
struct FreeCamera {
    yaw: f32,
    pitch: f32,
    speed: f32,
}

/// Formation inspection scene: spawn characters side by side with different LODs in rows.
fn setup_formation_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut skinned_mesh_ibp: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut entity_lib: ResMut<crate::oni2_loader::registries::EntityLibrary>,
    mut anim_registry: ResMut<crate::oni2_loader::registries::AnimRegistry>,
) {
    let scoped = InGameEntity;

    // Ground plane
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(50.0)))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.2, 0.25, 0.2),
            ..default()
        })),
        Transform::default(),
        scoped.clone(),
    ));

    // Auto-discover all entities with Entity.type files
    let entity_base_str = "Entity".to_string();
    let mut entity_dirs: Vec<(String, String)> = Vec::new(); // (dir_path, name)

    if let Ok(mut entries) = crate::vfs::read_dir(&entity_base_str) {
        entries.sort_by(|a, b| a.path.split('/').last().cmp(&b.path.split('/').last()));

        for entry in entries {
            if entry.is_dir {
                let dir_path_str = &entry.path;
                let dir_name = dir_path_str.split('/').last().unwrap_or_default().to_string();

                // 3-letter character directories with Entity.type
                if dir_name.len() == 3 && crate::vfs::exists(dir_path_str, "Entity.type") {
                    entity_dirs.push((dir_path_str.to_string(), dir_name));
                }
            }
        }
    }

    info!("Formation: {} entities with Entity.type", entity_dirs.len());

    // Formation layout: grid of entities
    let col_spacing = 3.0;
    let cols = 10; // entities per row

    for (idx, (entity_dir, name)) in entity_dirs.iter().enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let x = col as f32 * col_spacing - (cols as f32 - 1.0) * col_spacing / 2.0;
        let z = -(row as f32 * col_spacing) - 5.0;
        let pos = Vec3::new(x, 0.0, z);

        oni2_loader::spawn_oni2_entity(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &mut skinned_mesh_ibp,
            &mut entity_lib,
            &mut anim_registry,
            entity_dir,
            pos,
            name,
        );
    }

    // Enable skeleton debug by default
    commands.insert_resource(oni2_loader::DebugSkeletonVisible(true));

    // Directional light
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadows_enabled: true,
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

    // Free camera — positioned in front of the formation, looking at them
    // yaw=0 means looking down -Z in Bevy (toward the formation)
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 2.0, 5.0),
        IsDefaultUiCamera,
        FreeCamera {
            yaw: 0.0,
            pitch: -0.1,
            speed: 5.0,
        },
        scoped,
    ));
}

/// WASD + mouse free-fly camera for formation inspection.
fn free_camera_system(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    accumulated_motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    mut query: Query<(&mut Transform, &mut FreeCamera)>,
) {
    let Ok((mut transform, mut cam)) = query.single_mut() else { return };

    // Mouse look (hold right mouse button)
    if mouse_button.pressed(MouseButton::Right) {
        let sensitivity = 0.003;
        let delta = accumulated_motion.delta;
        cam.yaw -= delta.x * sensitivity;
        cam.pitch = (cam.pitch - delta.y * sensitivity).clamp(-1.4, 1.4);
    }

    // Speed boost with shift
    let speed = if keyboard.pressed(KeyCode::ShiftLeft) {
        cam.speed * 3.0
    } else {
        cam.speed
    };

    // WASD movement relative to camera orientation
    let forward = Vec3::new(cam.yaw.sin(), 0.0, cam.yaw.cos()).normalize();
    let right = Vec3::new(-cam.yaw.cos(), 0.0, cam.yaw.sin()).normalize();
    let mut velocity = Vec3::ZERO;

    if keyboard.pressed(KeyCode::KeyS) { velocity += forward; }
    if keyboard.pressed(KeyCode::KeyW) { velocity -= forward; }
    if keyboard.pressed(KeyCode::KeyA) { velocity += right; }
    if keyboard.pressed(KeyCode::KeyD) { velocity -= right; }
    if keyboard.pressed(KeyCode::Space) { velocity += Vec3::Y; }
    if keyboard.pressed(KeyCode::ControlLeft) { velocity -= Vec3::Y; }

    if velocity.length_squared() > 0.0 {
        velocity = velocity.normalize() * speed * time.delta_secs();
        transform.translation += velocity;
    }

    // Apply rotation
    transform.rotation = Quat::from_rotation_y(cam.yaw) * Quat::from_rotation_x(cam.pitch);
}

#[derive(Component)]
struct FpsText;

fn setup_fps_counter(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(5.0),
                right: Val::Px(10.0),
                ..default()
            },
            GlobalZIndex(i32::MAX),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("FPS: --"),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(1.0, 1.0, 0.0)),
                FpsText,
            ));
        });
}

fn update_fps_counter(
    diagnostics: Res<DiagnosticsStore>,
    mut query: Query<&mut Text, With<FpsText>>,
) {
    use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
    for mut text in &mut query {
        if let Some(diag) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS) {
            if let Some(val) = diag.smoothed() {
                *text = Text::new(format!("FPS: {val:.0}"));
            }
        }
    }
}

/// Start with avian3d physics debug gizmos disabled.
fn disable_physics_debug(mut store: ResMut<GizmoConfigStore>) {
    store.config_mut::<avian3d::debug_render::PhysicsGizmos>().0.enabled = false;
}

/// F7 toggles avian3d's native physics debug rendering (collider wireframes, contacts, etc).
fn toggle_physics_debug(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut store: ResMut<GizmoConfigStore>,
) {
    if keyboard.just_pressed(KeyCode::F7) {
        let config = store.config_mut::<avian3d::debug_render::PhysicsGizmos>().0;
        config.enabled = !config.enabled;
        info!("Physics debug rendering: {}", if config.enabled { "ON" } else { "OFF" });
    }
}

#[derive(Component)]
struct DebugPointLight;

/// F8 toggles a bright white point light above the player to help light environments.
fn toggle_debug_light(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    player_query: Query<Entity, With<player::components::Player>>,
    light_query: Query<Entity, With<DebugPointLight>>,
) {
    if keyboard.just_pressed(KeyCode::F8) {
        if let Some(light_entity) = light_query.iter().next() {
            commands.entity(light_entity).despawn();
            info!("Debug point light OFF");
        } else if let Some(player_entity) = player_query.iter().next() {
            commands.entity(player_entity).with_children(|parent| {
                parent.spawn((
                    PointLight {
                        color: Color::WHITE,
                        intensity: 1_000_000.0,
                        range: 100.0,
                        shadows_enabled: true,
                        ..default()
                    },
                    Transform::from_xyz(0.0, 5.0, 0.0),
                    DebugPointLight,
                ));
            });
            info!("Debug point light ON");
        }
    }
}
