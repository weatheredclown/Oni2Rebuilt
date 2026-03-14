use super::*;

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
    let file_stem = anim_file
        .split('/')
        .last()
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("");
    let entity_name = file_stem.split('_').next().unwrap_or(file_stem);
    let entity_dir = format!("Entity/{}", entity_name);

    info!(
        "TestAnim: file={}, entity={}, dir={}",
        anim_file, entity_name, entity_dir
    );

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
