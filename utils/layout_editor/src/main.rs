use anyhow::{Context, Result};
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use quick_xml::de::from_str;
use quick_xml::se::to_string;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let config = EditorConfig::from_args()?;
    let layout = LayoutDocument::load(&config.layout_dir)
        .with_context(|| format!("failed to load layout from {}", config.layout_dir.display()))?;

    App::new()
        .insert_resource(ClearColor(Color::srgb(0.04, 0.04, 0.05)))
        .insert_resource(config)
        .insert_resource(layout)
        .insert_resource(EditorState::default())
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                keyboard_shortcuts_system,
                camera_orbit_system,
                mouse_pick_system,
                transform_edit_system,
                sync_actor_transforms,
                update_window_title_system,
            ),
        )
        .run();

    Ok(())
}

#[derive(Resource, Clone)]
struct EditorConfig {
    layout_dir: PathBuf,
    entity_dir: PathBuf,
}

impl EditorConfig {
    fn from_args() -> Result<Self> {
        let mut layout_dir: Option<PathBuf> = None;
        let mut entity_dir: Option<PathBuf> = None;

        let args: Vec<String> = env::args().collect();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--layout" => {
                    i += 1;
                    layout_dir = args.get(i).map(PathBuf::from);
                }
                "--entities" => {
                    i += 1;
                    entity_dir = args.get(i).map(PathBuf::from);
                }
                "-h" | "--help" => {
                    println!(
                        "Usage: layout_editor --layout <layout_dir> --entities <entity_dir>\n\
                        Example: cargo run -p layout_editor -- --layout data/layouts/tim06 --entities data/entity"
                    );
                    std::process::exit(0);
                }
                _ => {}
            }
            i += 1;
        }

        let layout_dir = layout_dir.context("missing --layout <layout_dir>")?;
        let entity_dir = entity_dir.context("missing --entities <entity_dir>")?;

        Ok(Self {
            layout_dir,
            entity_dir,
        })
    }
}

#[derive(Resource, Default)]
struct EditorState {
    mode: EditMode,
    selected_actor: Option<String>,
    dirty: bool,
    status: String,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum EditMode {
    #[default]
    Transform,
    Rotate,
}

#[derive(Resource, Default)]
struct LayoutDocument {
    actors_file_names: Vec<String>,
    actors: Vec<ActorRecord>,
    entity_types: Vec<EntityTypeEntry>,
}

#[derive(Clone)]
struct EntityTypeEntry {
    entity_type: String,
    thumbnail_path: Option<PathBuf>,
}

#[derive(Component)]
struct ActorHandle {
    name: String,
}

#[derive(Component)]
struct OrbitCamera {
    focus: Vec3,
    radius: f32,
    yaw: f32,
    pitch: f32,
}

impl LayoutDocument {
    fn load(layout_dir: &Path) -> Result<Self> {
        let actors_file = layout_dir.join("layout.actors");
        let et_file = layout_dir.join("layout.et");

        let actor_names = parse_layout_actors(&actors_file)?;
        let et_names = parse_layout_et(&et_file)?;

        let mut actors = Vec::new();
        for actor_name in &actor_names {
            let actor_xml = layout_dir.join(format!("{actor_name}.xml"));
            if let Ok(actor) = load_actor_xml(&actor_xml, actor_name) {
                actors.push(actor);
            }
        }

        let entity_types = et_names
            .into_iter()
            .map(|name| EntityTypeEntry {
                entity_type: name,
                thumbnail_path: None,
            })
            .collect();

        Ok(Self {
            actors_file_names: actor_names,
            actors,
            entity_types,
        })
    }

    fn save(&mut self, layout_dir: &Path) -> Result<()> {
        let actors_file = layout_dir.join("layout.actors");
        let mut actor_lines = vec![self.actors_file_names.len().to_string()];
        actor_lines.extend(self.actors_file_names.iter().cloned());
        fs::write(actors_file, actor_lines.join("\n") + "\n")?;

        for actor in &self.actors {
            let xml_path = layout_dir.join(format!("{}.xml", actor.name));
            let xml = to_string(&actor.xml_root)?;
            let wrapped = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE actor SYSTEM \"component.dtd\">\n{}\n",
                xml
            );
            fs::write(xml_path, wrapped)?;
        }

        Ok(())
    }

    fn refresh_thumbnails(&mut self, entity_dir: &Path) {
        for entry in &mut self.entity_types {
            let path = entity_dir.join(&entry.entity_type).join("thumbnail.png");
            if path.exists() {
                entry.thumbnail_path = Some(path);
            }
        }
    }

    fn thumbnail_count(&self) -> usize {
        self.entity_types
            .iter()
            .filter(|e| e.thumbnail_path.is_some())
            .count()
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut layout: ResMut<LayoutDocument>,
    config: Res<EditorConfig>,
) {
    layout.refresh_thumbnails(&config.entity_dir);

    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            illuminance: 10_000.0,
            ..default()
        },
        Transform::from_xyz(15.0, 18.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(500.0, 500.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.08, 0.09, 0.11),
            perceptual_roughness: 0.95,
            ..default()
        })),
    ));

    for actor in &layout.actors {
        let color = if actor.xml_root.contents.entity.attributes.entity_type.value == "kno" {
            Color::srgb(0.3, 0.9, 0.5)
        } else {
            Color::srgb(0.75, 0.75, 0.82)
        };

        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                ..default()
            })),
            Transform {
                translation: actor.position(),
                rotation: Quat::from_euler(
                    EulerRot::XYZ,
                    actor.rotation_radians().x,
                    actor.rotation_radians().y,
                    actor.rotation_radians().z,
                ),
                scale: Vec3::ONE,
            },
            ActorHandle {
                name: actor.name.clone(),
            },
        ));
    }

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(16.0, 12.0, 16.0).looking_at(Vec3::ZERO, Vec3::Y),
        OrbitCamera {
            focus: Vec3::ZERO,
            radius: 25.0,
            yaw: -0.65,
            pitch: -0.45,
        },
    ));
}

fn keyboard_shortcuts_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<EditorState>,
    mut layout: ResMut<LayoutDocument>,
    config: Res<EditorConfig>,
) {
    if keys.just_pressed(KeyCode::KeyT) {
        state.mode = EditMode::Transform;
        state.status = "Transform mode".to_string();
    }
    if keys.just_pressed(KeyCode::KeyR) {
        state.mode = EditMode::Rotate;
        state.status = "Rotate mode".to_string();
    }

    if keys.pressed(KeyCode::ControlLeft) && keys.just_pressed(KeyCode::KeyS) {
        match layout.save(&config.layout_dir) {
            Ok(()) => {
                state.dirty = false;
                state.status = "Saved layout with Ctrl+S".to_string();
            }
            Err(err) => state.status = format!("Save failed: {err}"),
        }
    }

    if keys.pressed(KeyCode::ControlLeft) && keys.just_pressed(KeyCode::KeyN) {
        layout.actors.clear();
        layout.actors_file_names.clear();
        state.selected_actor = None;
        state.dirty = true;
        state.status = "New layout created in memory".to_string();
    }
}

fn update_window_title_system(
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    state: Res<EditorState>,
    layout: Res<LayoutDocument>,
) {
    let Ok(mut window) = windows.single_mut() else {
        return;
    };

    let mode = if state.mode == EditMode::Transform {
        "Transform[T]"
    } else {
        "Rotate[R]"
    };
    let selected = state.selected_actor.as_deref().unwrap_or("none");
    let dirty = if state.dirty { "*" } else { "" };

    window.title = format!(
        "Layout Editor {dirty} | mode={mode} | actors={} | entities={} (thumbs={}) | selected={} | {}",
        layout.actors.len(),
        layout.entity_types.len(),
        layout.thumbnail_count(),
        selected,
        if state.status.is_empty() {
            "LMB select • RMB orbit • WASD pan • Wheel zoom • Arrows/QE edit • Ctrl+S save"
        } else {
            state.status.as_str()
        }
    );
}

fn camera_orbit_system(
    time: Res<Time>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    mut motion: MessageReader<MouseMotion>,
    mut query: Query<(&mut Transform, &mut OrbitCamera)>,
) {
    for (mut transform, mut orbit) in &mut query {
        let mut move_delta = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) {
            move_delta.z -= 1.0;
        }
        if keys.pressed(KeyCode::KeyS) {
            move_delta.z += 1.0;
        }
        if keys.pressed(KeyCode::KeyA) {
            move_delta.x -= 1.0;
        }
        if keys.pressed(KeyCode::KeyD) {
            move_delta.x += 1.0;
        }

        if move_delta.length_squared() > 0.0 {
            orbit.focus += move_delta.normalize() * time.delta_secs() * 12.0;
        }

        if mouse_buttons.pressed(MouseButton::Right) {
            for ev in motion.read() {
                orbit.yaw -= ev.delta.x * 0.005;
                orbit.pitch = (orbit.pitch - ev.delta.y * 0.005).clamp(-1.45, 1.45);
            }
        }

        for ev in wheel.read() {
            orbit.radius = (orbit.radius - ev.y * 0.8).clamp(2.0, 400.0);
        }

        let rot = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        transform.translation = orbit.focus + rot * Vec3::new(0.0, 0.0, orbit.radius);
        transform.look_at(orbit.focus, Vec3::Y);
    }
}

fn mouse_pick_system(
    windows: Query<&Window, With<PrimaryWindow>>,
    buttons: Res<ButtonInput<MouseButton>>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
    actors: Query<(&GlobalTransform, &ActorHandle)>,
    mut state: ResMut<EditorState>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };

    let Ok((camera, camera_tf)) = camera_q.single() else {
        return;
    };

    let Ok(ray) = camera.viewport_to_world(camera_tf, cursor) else {
        return;
    };

    let mut best: Option<(f32, String)> = None;
    for (tf, handle) in &actors {
        let center = tf.translation();
        let distance = distance_point_to_ray(center, ray.origin, *ray.direction);
        if distance < 1.2 {
            let hit = center.distance(ray.origin);
            if best.as_ref().is_none_or(|(d, _)| hit < *d) {
                best = Some((hit, handle.name.clone()));
            }
        }
    }

    state.selected_actor = best.map(|(_, name)| name);
}

fn transform_edit_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<EditorState>,
    mut actors: Query<(&mut Transform, &ActorHandle)>,
) {
    let mut delta = Vec3::ZERO;
    if keys.just_pressed(KeyCode::ArrowUp) {
        delta.z -= 1.0;
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        delta.z += 1.0;
    }
    if keys.just_pressed(KeyCode::ArrowLeft) {
        delta.x -= 1.0;
    }
    if keys.just_pressed(KeyCode::ArrowRight) {
        delta.x += 1.0;
    }

    let selected = state.selected_actor.clone();
    for (mut tf, handle) in &mut actors {
        let is_selected = selected.as_ref().is_some_and(|n| n == &handle.name);
        if !is_selected {
            continue;
        }

        if state.mode == EditMode::Transform && delta != Vec3::ZERO {
            tf.translation += delta;
            state.dirty = true;
        }
        if state.mode == EditMode::Rotate {
            if keys.just_pressed(KeyCode::KeyQ) {
                tf.rotate_y(10f32.to_radians());
                state.dirty = true;
            }
            if keys.just_pressed(KeyCode::KeyE) {
                tf.rotate_y(-10f32.to_radians());
                state.dirty = true;
            }
        }
    }
}

fn sync_actor_transforms(
    actors_query: Query<(&Transform, &ActorHandle), Changed<Transform>>,
    mut layout: ResMut<LayoutDocument>,
) {
    let map: HashMap<String, usize> = layout
        .actors
        .iter()
        .enumerate()
        .map(|(i, a)| (a.name.clone(), i))
        .collect();

    for (tf, handle) in &actors_query {
        if let Some(index) = map.get(&handle.name) {
            if let Some(actor) = layout.actors.get_mut(*index) {
                actor.set_position(tf.translation);
                actor.set_rotation(tf.rotation.to_euler(EulerRot::XYZ));
            }
        }
    }
}

fn distance_point_to_ray(point: Vec3, ray_origin: Vec3, ray_dir: Vec3) -> f32 {
    let to_point = point - ray_origin;
    let projected = to_point.dot(ray_dir);
    let closest = ray_origin + ray_dir * projected.max(0.0);
    point.distance(closest)
}

#[derive(Clone)]
struct ActorRecord {
    name: String,
    xml_root: ActorXml,
}

impl ActorRecord {
    fn position(&self) -> Vec3 {
        parse_triplet(
            &self
                .xml_root
                .contents
                .actor_specific
                .attributes
                .position
                .value,
        )
    }

    fn set_position(&mut self, value: Vec3) {
        self.xml_root
            .contents
            .actor_specific
            .attributes
            .position
            .value = format!("{} {} {}", value.x, value.y, value.z);
    }

    fn rotation_radians(&self) -> Vec3 {
        self.xml_root
            .contents
            .actor_specific
            .attributes
            .orientation
            .as_ref()
            .map(|o| parse_triplet(&o.value).to_radians())
            .unwrap_or(Vec3::ZERO)
    }

    fn set_rotation(&mut self, euler_xyz: (f32, f32, f32)) {
        let degrees = Vec3::new(
            euler_xyz.0.to_degrees(),
            euler_xyz.1.to_degrees(),
            euler_xyz.2.to_degrees(),
        );
        self.xml_root
            .contents
            .actor_specific
            .attributes
            .orientation
            .get_or_insert_with(|| ValueAttribute {
                value: "0 0 0".to_string(),
            })
            .value = format!("{} {} {}", degrees.x, degrees.y, degrees.z);
    }
}

fn parse_triplet(input: &str) -> Vec3 {
    let parts: Vec<f32> = input
        .split_whitespace()
        .filter_map(|v| v.parse::<f32>().ok())
        .collect();
    if parts.len() == 3 {
        Vec3::new(parts[0], parts[1], parts[2])
    } else {
        Vec3::ZERO
    }
}

trait Vec3DegExt {
    fn to_radians(self) -> Vec3;
}

impl Vec3DegExt for Vec3 {
    fn to_radians(self) -> Vec3 {
        Vec3::new(
            self.x.to_radians(),
            self.y.to_radians(),
            self.z.to_radians(),
        )
    }
}

fn parse_layout_actors(path: &Path) -> Result<Vec<String>> {
    let content = fs::read_to_string(path)?;
    let mut names = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.parse::<usize>().is_ok() {
            continue;
        }
        names.push(trimmed.to_string());
    }
    Ok(names)
}

fn parse_layout_et(path: &Path) -> Result<Vec<String>> {
    let content = fs::read_to_string(path)?;
    let mut names = HashSet::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("BASICENTITY") || trimmed.starts_with("ANIMATEDENTITY") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                names.insert(parts[1].to_string());
            }
        }
    }

    let mut out: Vec<String> = names.into_iter().collect();
    out.sort();
    Ok(out)
}

fn load_actor_xml(path: &Path, actor_name: &str) -> Result<ActorRecord> {
    let xml = fs::read_to_string(path)?;
    let xml = xml
        .lines()
        .filter(|line| !line.trim_start().starts_with("<?xml") && !line.contains("<!DOCTYPE"))
        .collect::<Vec<&str>>()
        .join("\n");

    let parsed: ActorXml =
        from_str(&xml).with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(ActorRecord {
        name: actor_name.to_string(),
        xml_root: parsed,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename = "actor")]
struct ActorXml {
    #[serde(rename = "@name")]
    name: String,
    contents: ActorContents,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ActorContents {
    #[serde(rename = "Entity")]
    entity: EntityBlock,
    #[serde(rename = "Prop")]
    actor_specific: ActorSpecific,
    #[serde(rename = "Editable")]
    editable: Option<EditableBlock>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EntityBlock {
    attributes: EntityAttributes,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EntityAttributes {
    #[serde(rename = "EntityType")]
    entity_type: ValueAttribute,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ActorSpecific {
    #[serde(rename = "@name")]
    _name: String,
    attributes: ActorSpecificAttributes,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ActorSpecificAttributes {
    #[serde(rename = "Position")]
    position: ValueAttribute,
    #[serde(rename = "Orientation")]
    orientation: Option<ValueAttribute>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ValueAttribute {
    #[serde(rename = "@value")]
    value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EditableBlock {}
