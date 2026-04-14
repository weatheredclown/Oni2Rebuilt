use super::*;
use bevy::prelude::*;

use crate::menu::InGameEntity;

/// Marker for the free-fly camera in formation inspection mode.
#[derive(Component)]
pub struct FreeCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub speed: f32,
}

/// Formation inspection scene: spawn characters side-by-side in a grid.
pub fn setup_formation_scene(
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
    let mut entity_dirs: Vec<(String, String)> = Vec::new();

    if let Ok(mut entries) = crate::vfs::read_dir(&entity_base_str) {
        entries.sort_by(|a, b| a.path.split('/').last().cmp(&b.path.split('/').last()));

        for entry in entries {
            if entry.is_dir {
                let dir_path_str = &entry.path;
                let dir_name = dir_path_str
                    .split('/')
                    .last()
                    .unwrap_or_default()
                    .to_string();

                if dir_name.len() == 3 && crate::vfs::exists(dir_path_str, "Entity.type") {
                    entity_dirs.push((dir_path_str.to_string(), dir_name));
                }
            }
        }
    }

    info!("Formation: {} entities with Entity.type", entity_dirs.len());

    let col_spacing = 3.0;
    let cols = 10;

    for (idx, (entity_dir, name)) in entity_dirs.iter().enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let x = col as f32 * col_spacing - (cols as f32 - 1.0) * col_spacing / 2.0;
        let z = -(row as f32 * col_spacing) - 5.0;
        let pos = Vec3::new(x, 0.0, z);

        spawn_oni2_entity(
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

    commands.insert_resource(DebugSkeletonVisible(true));

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
            brightness: 500.0,
            affects_lightmapped_meshes: false,
        },
        scoped.clone(),
    ));

    // Free camera positioned in front of the formation
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

/// WASD + right-mouse free-fly camera for formation inspection.
pub fn free_camera_system(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    accumulated_motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    mut query: Query<(&mut Transform, &mut FreeCamera)>,
) {
    let Ok((mut transform, mut cam)) = query.single_mut() else {
        return;
    };

    if mouse_button.pressed(MouseButton::Right) {
        let sensitivity = 0.003;
        let delta = accumulated_motion.delta;
        cam.yaw -= delta.x * sensitivity;
        cam.pitch = (cam.pitch - delta.y * sensitivity).clamp(-1.4, 1.4);
    }

    let speed = if keyboard.pressed(KeyCode::ShiftLeft) {
        cam.speed * 3.0
    } else {
        cam.speed
    };

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

    transform.rotation = Quat::from_rotation_y(cam.yaw) * Quat::from_rotation_x(cam.pitch);
}
