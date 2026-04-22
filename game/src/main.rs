/*
 * main.rs — application entry point.
 *
 * Parses CLI flags (--layout, --sandbox, --formation, --testanim, --testentity,
 * --path, --dat, --fog, --diagnostics), mounts the virtual file system (DiskVfs +
 * DaveVfs archives), registers all Bevy plugins, and sets the initial AppState.
 *
 * setup_scene: OnEnter(InGame) system for normal / sandbox gameplay — loads the
 * ONI2 layout (or a flat sandbox ground), attaches Player + combat components to
 * the Konoko entity, and spawns the camera rig.
 */
mod ai;
mod animator;
mod behavior;
mod camera;
mod combat;
mod common;
mod control_map;
mod debug;
mod door;
mod explosion;
mod fight;
mod fightai;
mod fight_vector;
mod filesystem;
mod fx_system;
mod hud;
mod inventory;
mod menu;
mod oni2_loader;
mod player;
mod projectile_system;
mod scroni;
mod statemachine;
mod telemetry;
mod weapons;
pub use filesystem::dave_vfs;
pub use filesystem::vfs;

use avian3d::prelude::*;
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use camera::channel::CameraChannel;
use camera::components::{CameraController, PrototypeElement};
use combat::components::{CombatMaterials, FistVisual};
use menu::{AppState, InGameEntity, SelectedLayout};
use oni2_loader::TestAnimMode;
use std::sync::OnceLock;

pub static ASSETS_PATH: OnceLock<String> = OnceLock::new();
pub static ASSETS_DAT: OnceLock<String> = OnceLock::new();

pub fn get_assets_path() -> &'static str {
    ASSETS_PATH
        .get()
        .map(|s| s.as_str())
        .unwrap_or("../oni2/zips/assets")
}

pub fn get_assets_dat() -> &'static str {
    ASSETS_DAT.get().map(|s| s.as_str()).unwrap_or("RB.DAT")
}

pub fn set_assets_path(path: impl Into<String>) {
    let _ = ASSETS_PATH.set(path.into());
}

pub fn set_assets_dat(path: impl Into<String>) {
    let _ = ASSETS_DAT.set(path.into());
}



/// Resource indicating sandbox mode (flat ground + model, no layout).
#[derive(Resource)]
struct SandboxMode;

/// Resource indicating formation inspection mode.
#[derive(Resource)]
pub struct FormationMode;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut cli_paths: Vec<String> = args
        .windows(2)
        .filter(|w| w[0] == "--path")
        .map(|w| w[1].clone())
        .collect();

    if cli_paths.is_empty() {
        cli_paths.push("oni2/zips/assets".to_string());
        cli_paths.push("oni2/zips/streams".to_string());
    }

    let mut cli_dats: Vec<String> = args
        .windows(2)
        .filter(|w| w[0] == "--dat")
        .map(|w| w[1].clone())
        .collect();

    if cli_dats.is_empty() {
        cli_dats.push("RB.DAT".to_string());
        cli_dats.push("STREAMS.DAT".to_string());
        cli_dats.push("BANKS.DAT".to_string());
    }

    if !cli_paths.is_empty() {
        ASSETS_PATH.set(cli_paths[0].clone()).ok();
    }
    if !cli_dats.is_empty() {
        ASSETS_DAT.set(cli_dats[0].clone()).ok();
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
    let cli_testentity = args
        .iter()
        .position(|a| a == "--testentity" || a == "--entitytest")
        .map(|i| {
            if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                args[i + 1].clone()
            } else {
                String::new()
            }
        });
    let sandbox_mode = args.iter().any(|a| a == "--sandbox");
    let formation_mode = args.iter().any(|a| a == "--formation");
    let diagnostics_mode = args.iter().any(|a| a == "--diagnostics");
    let fog_enabled = args.iter().any(|a| a == "--fog");

    // --- VFS setup ---
    let mut multi_vfs = vfs::MultiVfs::new();

    for disk_path in &cli_paths {
        if std::path::Path::new(disk_path).exists() {
            println!("Mounting DiskVfs at: {}", disk_path);
            multi_vfs.push(Box::new(vfs::DiskVfs::new(disk_path.to_string())));
        }
    }

    for dat_path_str in &cli_dats {
        let dat_path = std::path::Path::new(dat_path_str);
        if dat_path.exists() || (dat_path.is_dir() && dat_path.join("RB.DAT").exists()) {
            match dave_vfs::DaveVfs::new(dat_path_str) {
                Ok(dave_vfs) => {
                    println!("Mounting DaveVfs archive at: {}", dat_path_str);
                    multi_vfs.push(Box::new(dave_vfs));
                }
                Err(e) => {
                    println!("Failed to initialize DaveVfs for {}: {}", dat_path_str, e);
                }
            }
        }
    }

    vfs::set_vfs(Box::new(multi_vfs));

    // --- App setup ---
    let mut app = App::new();

    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "rb-reborn".to_string(),
                    ..default()
                }),
                ..default()
            })
            .set(LogPlugin {
                filter: "info,bevy_ecs=trace,wgpu_core=warn,wgpu_hal=warn,rb_game=debug".into(),
                level: bevy::log::Level::DEBUG,
                ..default()
            }),
    )
    .insert_resource(Time::<Fixed>::from_hz(60.0))
    .add_plugins(PhysicsPlugins::default())
    .add_plugins(avian3d::debug_render::PhysicsDebugPlugin)
    .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
    .add_plugins(FrameTimeDiagnosticsPlugin::default())
    .add_plugins(telemetry::TelemetryPlugin)
    .add_plugins(menu::MenuPlugin)
    .add_plugins(combat::CombatPlugin)
    .add_plugins(fight::FightPlugin)
    .add_plugins(animator::AnimatorPlugin)
    .add_plugins(control_map::ControlMapPlugin)
    .add_plugins(fight_vector::FightVectorPlugin)
    .add_plugins(statemachine::StateMachinePlugin)
    .add_plugins(fightai::FightAiPlugin)
    .add_plugins(behavior::BehaviorPlugin)
    .add_plugins(player::PlayerPlugin)
    .add_plugins(ai::AiPlugin)
    .add_plugins(camera::CameraPlugin)
    .add_plugins(hud::HudPlugin)
    .add_plugins(fx_system::FxPlugin)
    .add_plugins(projectile_system::ProjectilePlugin)
    .add_plugins(weapons::WeaponPlugin)
    .add_plugins(inventory::InventoryPlugin)
    .add_plugins(oni2_loader::Oni2LoaderPlugin)
    .add_plugins(scroni::ScroniPlugin)
    .add_plugins(door::DoorPlugin)
    .add_plugins(debug::DebugPlugin);

    if fog_enabled {
        app.insert_resource(oni2_loader::FogEnabled);
    }

    if diagnostics_mode {
        app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
    }

    // --- Scene entry points ---
    app.add_systems(
        OnEnter(AppState::InGame),
        setup_scene.run_if(
            not(resource_exists::<TestAnimMode>)
                .and(not(resource_exists::<FormationMode>))
                .and(not(resource_exists::<oni2_loader::TestEntityMode>)),
        ),
    )
    .add_systems(
        OnEnter(AppState::InGame),
        oni2_loader::setup_formation_scene.run_if(resource_exists::<FormationMode>),
    )
    .add_systems(
        OnEnter(AppState::InGame),
        oni2_loader::setup_testanim_scene.run_if(resource_exists::<TestAnimMode>),
    )
    .add_systems(
        OnEnter(AppState::InGame),
        oni2_loader::setup_testentity_scene.run_if(resource_exists::<oni2_loader::TestEntityMode>),
    )
    .add_systems(
        Update,
        oni2_loader::free_camera_system
            .run_if(resource_exists::<FormationMode>)
            .run_if(in_state(AppState::InGame)),
    )
    .add_systems(Update, (explosion::update_explosion_system,));

    if let Some(layout_name) = &cli_layout {
        app.insert_resource(SelectedLayout(layout_name.clone()));
    }

    // --- Initial state ---
    if let Some(anim_path) = cli_testanim {
        if anim_path.to_lowercase().ends_with(".anim") {
            app.insert_resource(TestAnimMode(anim_path));
            app.insert_state(AppState::InGame);
        } else {
            app.insert_resource(crate::menu::TestAnimEntity(anim_path));
            app.insert_state(AppState::AnimMenu);
        }
    } else if let Some(entity_name) = cli_testentity {
        if entity_name.is_empty() {
            app.insert_state(AppState::EntityMenu);
        } else {
            app.insert_resource(oni2_loader::TestEntityMode(entity_name));
            app.insert_state(AppState::InGame);
        }
    } else if formation_mode {
        app.insert_resource(FormationMode);
        app.insert_state(AppState::InGame);
    } else if sandbox_mode {
        app.insert_resource(SandboxMode);
        app.insert_state(AppState::InGame);
    } else if cli_layout.is_some() {
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
    mut entity_lib: ResMut<oni2_loader::registries::EntityLibrary>,
    mut anim_registry: ResMut<oni2_loader::registries::AnimRegistry>,
    mut fight_fsm_cache: ResMut<crate::fightai::FightFsmCache>,
    mut attack_fsm_cache: ResMut<crate::fightai::AttackFsmCache>,
    combat_materials: Res<CombatMaterials>,
    selected_layout: Option<Res<SelectedLayout>>,
    sandbox: Option<Res<SandboxMode>>,
) {
    let scoped = InGameEntity;

    let layout_name = selected_layout
        .as_ref()
        .map(|s| s.0.as_str())
        .unwrap_or("tim06");
    let layout_path = format!("layout/{}", layout_name);
    let fallback_spawn = oni2_loader::find_konoko_spawn(&layout_path)
        .map(|p| p + Vec3::Y * 1.0)
        .unwrap_or(Vec3::new(0.0, 2.0, 0.0));

    // Invisible safety floor
    commands.spawn((
        Transform::from_xyz(0.0, -150.0, 0.0),
        RigidBody::Static,
        Collider::half_space(Vec3::Y),
        scoped.clone(),
    ));

    // Load layout or sandbox ground
    let layout_player_info = if sandbox.is_some() {
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
            &mut fight_fsm_cache,
            &mut attack_fsm_cache,
            &layout_path,
            &entity_base_str,
        )
    };

    // Fallback lights for sandbox
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

    // Attach player components to layout entity, or spawn a fallback capsule
    let player_id = if let Some(ref pi) = layout_player_info {
        let pad_fsm = pi
            .pad_fsm
            .clone()
            .unwrap_or_else(|| "player".to_string());
        commands.entity(pi.entity).insert((
            scoped.clone(),
            crate::player::PlayerIdentityBundle::new(
                pi.faction.clone().unwrap_or_default(),
                pi.max_hitpoints.unwrap_or(100.0),
            ),
            crate::player::components::PadFsmName(pad_fsm),
            crate::combat::FighterBundle::default(),
        ));
        pi.entity
    } else {
        spawn_fallback_player(
            &mut commands,
            &mut meshes,
            &mut materials,
            &combat_materials,
            fallback_spawn,
            scoped.clone(),
        )
    };

    // Camera
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 8.0, -12.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        scoped,
        IsDefaultUiCamera,
        CameraController::default(),
        CameraChannel {
            focus_actor: player_id,
            ..default()
        },
    ));
}

pub fn spawn_fallback_player(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    combat_materials: &CombatMaterials,
    fallback_spawn: Vec3,
    scoped: InGameEntity,
) -> Entity {
    commands
        .spawn((
            Mesh3d(meshes.add(Capsule3d::new(0.4, 1.2))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.2, 0.4, 0.9),
                ..default()
            })),
            Transform::from_translation(fallback_spawn),
            scoped,
            crate::combat::CreaturePhysicsBundle::new(0.4, 1.2),
            PrototypeElement,
            crate::player::PlayerIdentityBundle::new("TCTF", 100.0),
            crate::player::components::PadFsmName("player".to_string()),
            crate::combat::FighterBundle::default(),
            crate::animator::AnimatorBundle::default(),
        ))
        .with_children(|parent| {
            parent.spawn((
                Mesh3d(combat_materials.fist_mesh.clone()),
                MeshMaterial3d(combat_materials.fist_startup.clone()),
                Transform::from_translation(Vec3::new(0.3, 0.3, -0.5)),
                Visibility::Hidden,
                FistVisual,
            ));
        })
        .id()
}
