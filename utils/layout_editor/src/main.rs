use anyhow::{Context, Result};
use bevy::app::AppExit;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use image::{ImageBuffer, Rgba};
use quick_xml::de::from_str;
use quick_xml::se::to_string;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let config = EditorConfig::from_args()?;
    let mut layout = LayoutDocument::load(&config.layout_dir)
        .with_context(|| format!("failed to load layout from {}", config.layout_dir.display()))?;
    layout.refresh_thumbnails(&config.entity_dir);

    App::new()
        .insert_resource(ClearColor(Color::srgb(0.04, 0.04, 0.05)))
        .insert_resource(config)
        .insert_resource(layout)
        .insert_resource(EditorState::default())
        .insert_resource(LayoutPicker::default())
        .insert_resource(ThumbnailGenerator::default())
        .add_plugins(DefaultPlugins)
        .add_plugins(EguiPlugin::default())
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                keyboard_shortcuts_system,
                camera_orbit_system,
                mouse_pick_system,
                transform_edit_system,
                sync_actor_transforms,
                regenerate_actor_meshes,
                update_window_title_system,
                thumbnail_background_generation_system,
                debug_bounds_toggle_system,
                draw_debug_bounds_system,
            ),
        )
        .add_systems(EguiPrimaryContextPass, ui_system)
        .run();

    Ok(())
}

#[derive(Resource, Clone)]
struct EditorConfig {
    layout_root: PathBuf,
    entity_dir: PathBuf,
    layout_name: String,
    layout_dir: PathBuf,
}

impl EditorConfig {
    fn from_args() -> Result<Self> {
        let mut path_root: Option<PathBuf> = None;

        let args: Vec<String> = env::args().collect();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--path" => {
                    i += 1;
                    path_root = args.get(i).map(PathBuf::from);
                }
                "-h" | "--help" => {
                    println!(
                        "Usage: layout_editor --path <assets_root> [--layout <layout_name>]\n\
                        Example: cargo run -p layout_editor -- --path ../oni2/zips/assets --layout tim06"
                    );
                    std::process::exit(0);
                }
                _ => {}
            }
            i += 1;
        }

        let assets_root = path_root.context("missing --path <assets_root>")?;
        let layout_root = assets_root.join("layout");
        let entity_dir = assets_root.join("entity");

        let explicit_layout = args.windows(2).find_map(|w| {
            if w[0] == "--layout" {
                Some(w[1].clone())
            } else {
                None
            }
        });

        let mut available = discover_layout_names(&layout_root);
        available.sort();
        let layout_name = explicit_layout
            .or_else(|| available.into_iter().next())
            .context(format!(
                "could not choose layout; no folders found under {}",
                layout_root.display()
            ))?;

        let layout_dir = layout_root.join(&layout_name);

        Ok(Self {
            layout_root,
            entity_dir,
            layout_name,
            layout_dir,
        })
    }

    fn set_layout(&mut self, layout_name: String) {
        self.layout_name = layout_name;
        self.layout_dir = self.layout_root.join(&self.layout_name);
    }
}

#[derive(Resource, Default)]
struct EditorState {
    mode: EditMode,
    selected_actor: Option<String>,
    dirty: bool,
    status: String,
    show_load_dialog: bool,
    show_unsaved_warning: bool,
    pending_load_layout: Option<String>,
    show_save_as_dialog: bool,
    save_as_name: String,
    show_overwrite_warning: bool,
    pending_save_as: Option<String>,
    show_bounds: bool,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum EditMode {
    #[default]
    Transform,
    Rotate,
}

#[derive(Resource, Default)]
struct LayoutPicker {
    names: Vec<String>,
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

#[derive(Resource, Default)]
struct ThumbnailGenerator {
    queue: Vec<String>,
    index: usize,
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

#[derive(Component)]
struct ActorVisual;

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
    mut picker: ResMut<LayoutPicker>,
    mut thumbs: ResMut<ThumbnailGenerator>,
    config: Res<EditorConfig>,
    layout: Res<LayoutDocument>,
) {
    picker.names = discover_layout_names(&config.layout_root);
    thumbs.queue = layout
        .entity_types
        .iter()
        .map(|e| e.entity_type.clone())
        .collect();

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

    spawn_actor_visuals(&mut commands, &mut meshes, &mut materials, &layout);

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

fn spawn_actor_visuals(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    layout: &LayoutDocument,
) {
    for actor in &layout.actors {
        let entity_type = actor
            .xml_root
            .contents
            .entity
            .attributes
            .entity_type
            .value
            .to_lowercase();
        let color = match entity_type.as_str() {
            "kno" => Color::srgb(0.3, 0.9, 0.5),
            "tctf" => Color::srgb(0.8, 0.45, 0.9),
            _ => Color::srgb(0.75, 0.75, 0.82),
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
            ActorVisual,
        ));
    }
}

fn ui_system(
    mut contexts: EguiContexts,
    mut exit: MessageWriter<AppExit>,
    mut state: ResMut<EditorState>,
    mut config: ResMut<EditorConfig>,
    mut layout: ResMut<LayoutDocument>,
    mut picker: ResMut<LayoutPicker>,
    mut thumbs: ResMut<ThumbnailGenerator>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    egui::TopBottomPanel::top("menu").show(ctx, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Load").clicked() {
                    state.show_load_dialog = true;
                    picker.names = discover_layout_names(&config.layout_root);
                    ui.close();
                }
                if ui.button("Save").clicked() {
                    match layout.save(&config.layout_dir) {
                        Ok(()) => {
                            state.dirty = false;
                            state.status = format!("Saved {}", config.layout_name);
                        }
                        Err(err) => {
                            state.status = format!("Save failed: {err}");
                        }
                    }
                    ui.close();
                }
                if ui.button("Save As").clicked() {
                    state.save_as_name = config.layout_name.clone();
                    state.show_save_as_dialog = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    exit.write(AppExit::Success);
                }
            });
        });
    });

    egui::SidePanel::left("entity_palette")
        .resizable(true)
        .default_width(300.0)
        .show(ctx, |ui| {
            ui.heading("Entities");
            ui.label(format!(
                "{} entries • {} thumbnails",
                layout.entity_types.len(),
                layout.thumbnail_count()
            ));

            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("thumb_grid")
                    .num_columns(2)
                    .spacing([8.0, 8.0])
                    .show(ui, |ui| {
                        for (i, entry) in layout.entity_types.iter().enumerate() {
                            ui.group(|ui| {
                                ui.set_min_size(egui::vec2(130.0, 130.0));
                                let label = if entry.thumbnail_path.is_some() {
                                    format!("{}\nthumbnail.png", entry.entity_type)
                                } else {
                                    format!("{}\n(generating)", entry.entity_type)
                                };
                                ui.add_sized([120.0, 120.0], egui::Button::new(label));
                            });
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
            });
        });

    if state.show_load_dialog {
        egui::Window::new("Load layout")
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label("Select a layout folder:");
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for name in &picker.names {
                            if ui.button(name).clicked() {
                                if state.dirty {
                                    state.pending_load_layout = Some(name.clone());
                                    state.show_unsaved_warning = true;
                                } else {
                                    load_named_layout(
                                        name.clone(),
                                        &mut config,
                                        &mut layout,
                                        &mut thumbs,
                                        &mut state,
                                    );
                                }
                                state.show_load_dialog = false;
                            }
                        }
                    });
                if ui.button("Cancel").clicked() {
                    state.show_load_dialog = false;
                }
            });
    }

    if state.show_unsaved_warning {
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("You have unsaved changes. Continue loading anyway?");
                ui.horizontal(|ui| {
                    if ui.button("Load anyway").clicked() {
                        if let Some(next) = state.pending_load_layout.take() {
                            load_named_layout(
                                next,
                                &mut config,
                                &mut layout,
                                &mut thumbs,
                                &mut state,
                            );
                        }
                        state.show_unsaved_warning = false;
                    }
                    if ui.button("Cancel").clicked() {
                        state.pending_load_layout = None;
                        state.show_unsaved_warning = false;
                    }
                });
            });
    }

    if state.show_save_as_dialog {
        egui::Window::new("Save As")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Layout folder name:");
                ui.text_edit_singleline(&mut state.save_as_name);

                ui.horizontal(|ui| {
                    if ui.button("Save As").clicked() {
                        let target = state.save_as_name.trim().to_string();
                        if !target.is_empty() {
                            let target_dir = config.layout_root.join(&target);
                            if target_dir.exists() {
                                state.pending_save_as = Some(target);
                                state.show_overwrite_warning = true;
                            } else {
                                save_as_layout(target, &mut config, &mut layout, &mut state);
                            }
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        state.show_save_as_dialog = false;
                    }
                });
            });
    }

    if state.show_overwrite_warning {
        egui::Window::new("Overwrite layout?")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("A layout with this name already exists. Overwrite it?");
                ui.horizontal(|ui| {
                    if ui.button("Overwrite").clicked() {
                        if let Some(target) = state.pending_save_as.take() {
                            save_as_layout(target, &mut config, &mut layout, &mut state);
                        }
                        state.show_overwrite_warning = false;
                    }
                    if ui.button("Cancel").clicked() {
                        state.pending_save_as = None;
                        state.show_overwrite_warning = false;
                    }
                });
            });
    }
}

fn load_named_layout(
    name: String,
    config: &mut EditorConfig,
    layout: &mut LayoutDocument,
    thumbs: &mut ThumbnailGenerator,
    state: &mut EditorState,
) {
    config.set_layout(name.clone());
    match LayoutDocument::load(&config.layout_dir) {
        Ok(mut new_doc) => {
            new_doc.refresh_thumbnails(&config.entity_dir);
            *layout = new_doc;
            thumbs.queue = layout
                .entity_types
                .iter()
                .map(|e| e.entity_type.clone())
                .collect();
            thumbs.index = 0;
            state.selected_actor = None;
            state.dirty = false;
            state.status = format!("Loaded layout {name}");
        }
        Err(err) => {
            state.status = format!("Load failed: {err}");
        }
    }
}

fn save_as_layout(
    target: String,
    config: &mut EditorConfig,
    layout: &mut LayoutDocument,
    state: &mut EditorState,
) {
    let target_dir = config.layout_root.join(&target);
    if let Err(err) = fs::create_dir_all(&target_dir) {
        state.status = format!("Save As failed: {err}");
        return;
    }
    if let Err(err) = layout.save(&target_dir) {
        state.status = format!("Save As failed: {err}");
        return;
    }

    config.set_layout(target.clone());
    state.dirty = false;
    state.show_save_as_dialog = false;
    state.status = format!("Saved as {target}");
}

fn thumbnail_background_generation_system(
    config: Res<EditorConfig>,
    mut generator: ResMut<ThumbnailGenerator>,
    mut layout: ResMut<LayoutDocument>,
) {
    if generator.index >= generator.queue.len() {
        return;
    }

    let entity_name = &generator.queue[generator.index];
    let thumbnail_path = config.entity_dir.join(entity_name).join("thumbnail.png");
    if !thumbnail_path.exists() {
        if let Some(parent) = thumbnail_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(128, 128);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            let checker = ((x / 16) + (y / 16)) % 2 == 0;
            let base = if checker { 64 } else { 96 };
            *pixel = Rgba([base, base, base + 20, 255]);
        }
        let _ = img.save(&thumbnail_path);
    }

    layout.refresh_thumbnails(&config.entity_dir);
    generator.index += 1;
}

fn regenerate_actor_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<ActorVisual>>,
    layout: Res<LayoutDocument>,
) {
    if !layout.is_changed() {
        return;
    }

    for e in &existing {
        commands.entity(e).despawn();
    }

    spawn_actor_visuals(&mut commands, &mut meshes, &mut materials, &layout);
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

fn debug_bounds_toggle_system(keys: Res<ButtonInput<KeyCode>>, mut state: ResMut<EditorState>) {
    if keys.just_pressed(KeyCode::KeyB) {
        state.show_bounds = !state.show_bounds;
    }
}

fn draw_debug_bounds_system(
    mut gizmos: Gizmos,
    state: Res<EditorState>,
    actors: Query<(&Transform, &ActorHandle)>,
) {
    if !state.show_bounds {
        return;
    }

    for (tf, handle) in &actors {
        let color = if state
            .selected_actor
            .as_ref()
            .is_some_and(|n| n == &handle.name)
        {
            Color::srgb(0.2, 0.95, 0.4)
        } else {
            Color::srgb(1.0, 0.8, 0.2)
        };
        gizmos.cube(*tf, color);
    }
}

fn update_window_title_system(
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    state: Res<EditorState>,
    layout: Res<LayoutDocument>,
    config: Res<EditorConfig>,
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
        "Layout Editor {dirty} | layout={} | mode={mode} | actors={} | entities={} (thumbs={}) | selected={} | {}",
        config.layout_name,
        layout.actors.len(),
        layout.entity_types.len(),
        layout.thumbnail_count(),
        selected,
        if state.status.is_empty() {
            "LMB select • RMB orbit • WASD pan • Wheel zoom • Arrows/QE edit • Ctrl+S save • B bounds"
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

fn discover_layout_names(layout_root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(layout_root) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(|s| s.to_string()))
        .collect()
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
