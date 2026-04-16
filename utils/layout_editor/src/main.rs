use anyhow::{Context, Result};
use bevy::app::AppExit;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use image::{ImageBuffer, Rgba};
use rb_game::oni2_loader::parsers::actor_xml::parse_actor_xml;
use rb_game::oni2_loader::registries::{AnimRegistry, EntityLibrary};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let config = EditorConfig::from_args()?;
    let assets_root = config.assets_root.to_string_lossy().to_string();
    rb_game::set_assets_path(assets_root.clone());
    rb_game::vfs::set_vfs(Box::new(rb_game::vfs::DiskVfs::new(assets_root)));

    let mut layout =
        LayoutDocument::load(&config.layout_dir, &config.layout_name, &config.entity_dir)
            .with_context(|| {
                format!("failed to load layout from {}", config.layout_dir.display())
            })?;
    layout.refresh_thumbnails(&config.entity_dir);

    App::new()
        .insert_resource(ClearColor(Color::srgb(0.04, 0.04, 0.05)))
        .insert_resource(config)
        .insert_resource(layout)
        .insert_resource(EditorState::default())
        .insert_resource(LayoutPicker::default())
        .insert_resource(ThumbnailGenerator::default())
        .insert_resource(EntityLibrary::default())
        .insert_resource(AnimRegistry::default())
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
    assets_root: PathBuf,
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
            assets_root: assets_root.clone(),
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
    left_panel_tab: LeftPanelTab,
    dirty: bool,
    status: String,
    show_load_dialog: bool,
    show_new_warning: bool,
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

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum LeftPanelTab {
    #[default]
    AllEntities,
    LayoutActors,
}

#[derive(Resource, Default)]
struct LayoutPicker {
    names: Vec<String>,
}

#[derive(Resource, Default)]
struct LayoutDocument {
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
    fn load(layout_dir: &Path, layout_name: &str, entity_dir: &Path) -> Result<Self> {
        let actors_file = layout_dir.join("layout.actors");
        let actor_names = parse_layout_actors(&actors_file)?;

        // Use the VFS-relative path so parse_actor_xml can resolve template chains.
        let layout_vfs_rel = format!("layout/{}", layout_name);

        let mut actors = Vec::new();
        for actor_name in &actor_names {
            let xml_filename = format!("{}.xml", actor_name);

            // parse_actor_xml handles base= template inheritance and all ONI2 XML formats.
            if let Some(parsed) = parse_actor_xml(&layout_vfs_rel, &xml_filename, "template") {
                let raw_xml =
                    fs::read_to_string(layout_dir.join(&xml_filename)).unwrap_or_default();
                actors.push(ActorRecord {
                    name: actor_name.clone(),
                    entity_type: parsed.entity_type.to_lowercase(),
                    position: parsed.position,
                    orientation: parsed.orientation,
                    raw_xml,
                });
            }
        }

        // "All entities" is every subdirectory under assets/entity/, not just what
        // the current layout references.
        let entity_types = discover_entity_types(entity_dir);

        Ok(Self {
            actors,
            entity_types,
        })
    }

    fn save(&mut self, layout_dir: &Path) -> Result<()> {
        // Write layout.actors (count + one name per line).
        let actors_file = layout_dir.join("layout.actors");
        let mut actor_lines = vec![self.actors.len().to_string()];
        actor_lines.extend(self.actors.iter().map(|a| a.name.clone()));
        fs::write(actors_file, actor_lines.join("\n") + "\n")?;

        for actor in &self.actors {
            let xml_path = layout_dir.join(format!("{}.xml", actor.name));
            // Strip any existing XML/DOCTYPE headers from raw_xml before re-wrapping.
            let body: String = actor
                .raw_xml
                .lines()
                .filter(|l| {
                    let t = l.trim_start();
                    !t.starts_with("<?xml") && !t.starts_with("<!DOCTYPE")
                })
                .collect::<Vec<_>>()
                .join("\n");
            let wrapped = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE actor SYSTEM \"component.dtd\">\n{}\n",
                body
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
    mut images: ResMut<Assets<Image>>,
    mut skinned_mesh_ibp: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut entity_lib: ResMut<EntityLibrary>,
    mut anim_registry: ResMut<AnimRegistry>,
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
        Mesh3d(meshes.add(create_wireframe_grid(10.0, 10))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.5, 0.5, 0.5),
            unlit: true,
            ..default()
        })),
    ));

    spawn_actor_visuals(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        &mut skinned_mesh_ibp,
        &mut entity_lib,
        &mut anim_registry,
        &layout,
    );

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

fn placeholder_color(entity_type: &str) -> Color {
    match entity_type {
        "kno" => Color::srgb(0.3, 0.9, 0.5),
        "tctf" => Color::srgb(0.8, 0.45, 0.9),
        _ => Color::srgb(0.75, 0.75, 0.82),
    }
}

fn spawn_actor_visuals(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    images: &mut ResMut<Assets<Image>>,
    skinned_mesh_ibp: &mut ResMut<Assets<SkinnedMeshInverseBindposes>>,
    entity_lib: &mut ResMut<EntityLibrary>,
    anim_registry: &mut ResMut<AnimRegistry>,
    layout: &LayoutDocument,
) {
    for actor in &layout.actors {
        let entity_type = actor.entity_name();
        let color = placeholder_color(&entity_type);

        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            actor.rotation_radians().x,
            actor.rotation_radians().y,
            actor.rotation_radians().z,
        );
        let entity_dir = format!("entity/{}", actor.entity_name());
        let spawned = rb_game::oni2_loader::spawn_oni2_entity_with_rotation(
            commands,
            meshes,
            materials,
            images,
            skinned_mesh_ibp,
            entity_lib,
            anim_registry,
            &entity_dir,
            actor.position(),
            rotation,
            &actor.name,
            None,
            Some(&entity_type),
        );

        // spawn_oni2_entity_with_rotation populates entity_lib as a synchronous side effect,
        // so we can inspect sub_meshes immediately to know if anything renderable was produced.
        let has_mesh = spawned.is_some()
            && entity_lib
                .entities
                .get(&entity_dir)
                .map(|et| !et.sub_meshes.is_empty())
                .unwrap_or(false);

        if has_mesh {
            let entity = spawned.unwrap();
            commands.entity(entity).insert((
                ActorHandle {
                    name: actor.name.clone(),
                },
                ActorVisual,
            ));
        } else {
            // Either Entity.type was missing (None) or a model was not found / produced no
            // geometry.  In both cases keep the spawned entity (for its bounds/colliders) if
            // present, but add a clearly visible placeholder child so the actor shows up in
            // the editor viewport.  If nothing was spawned at all, fall back to a root cuboid.
            if let Some(entity) = spawned {
                commands.entity(entity).insert((
                    ActorHandle {
                        name: actor.name.clone(),
                    },
                    ActorVisual,
                ));
                let placeholder = commands
                    .spawn((
                        Mesh3d(meshes.add(Cuboid::new(0.8, 0.8, 0.8))),
                        MeshMaterial3d(materials.add(StandardMaterial {
                            base_color: color,
                            ..default()
                        })),
                        Transform::default(),
                    ))
                    .id();
                commands.entity(entity).add_child(placeholder);
            } else {
                commands.spawn((
                    Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: color,
                        ..default()
                    })),
                    Transform::from_translation(actor.position()).with_rotation(rotation),
                    ActorHandle {
                        name: actor.name.clone(),
                    },
                    ActorVisual,
                ));
            }
        }
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
                if ui.button("New").clicked() {
                    if state.dirty {
                        state.show_new_warning = true;
                    } else {
                        create_blank_layout(&mut config, &mut layout, &mut state);
                    }
                    ui.close();
                }
                if ui.button("Load").clicked() {
                    state.show_load_dialog = true;
                    picker.names = discover_layout_names(&config.layout_root);
                    ui.close();
                }
                if ui.button("Save").clicked() {
                    if config.layout_name.trim().is_empty() {
                        state.save_as_name = "new_layout".to_string();
                        state.show_save_as_dialog = true;
                        state.status = "No layout selected; choose a name to save.".to_string();
                    } else {
                        match layout.save(&config.layout_dir) {
                            Ok(()) => {
                                state.dirty = false;
                                state.status = format!("Saved {}", config.layout_name);
                            }
                            Err(err) => {
                                state.status = format!("Save failed: {err}");
                            }
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
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut state.left_panel_tab,
                    LeftPanelTab::AllEntities,
                    "All entities",
                );
                ui.selectable_value(
                    &mut state.left_panel_tab,
                    LeftPanelTab::LayoutActors,
                    "Layout actors",
                );
            });
            ui.separator();

            match state.left_panel_tab {
                LeftPanelTab::AllEntities => {
                    ui.heading("All available entities");
                    ui.label(format!(
                        "{} entries • {} thumbnails",
                        layout.entity_types.len(),
                        layout.thumbnail_count()
                    ));

                    let mut to_insert: Option<String> = None;
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
                                        if ui
                                            .add_sized([120.0, 120.0], egui::Button::new(label))
                                            .clicked()
                                        {
                                            to_insert = Some(entry.entity_type.clone());
                                        }
                                    });
                                    if i % 2 == 1 {
                                        ui.end_row();
                                    }
                                }
                            });
                    });

                    if let Some(entity_type) = to_insert {
                        let actor_name = next_actor_name_for_entity(layout.as_ref(), &entity_type);
                        layout
                            .actors
                            .push(make_new_actor(&actor_name, &entity_type));
                        state.selected_actor = Some(actor_name.clone());
                        state.dirty = true;
                        state.status =
                            format!("Inserted entity '{entity_type}' as actor '{actor_name}'");
                    }
                }
                LeftPanelTab::LayoutActors => {
                    ui.heading("Actor list from layout");
                    ui.label(format!("{} actors in level", layout.actors.len()));

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for actor in &layout.actors {
                            let selected = state
                                .selected_actor
                                .as_ref()
                                .is_some_and(|name| name == &actor.name);
                            if ui.selectable_label(selected, &actor.name).clicked() {
                                state.selected_actor = Some(actor.name.clone());
                                state.status = format!("Selected actor '{}'", actor.name);
                            }
                        }
                    });
                }
            }
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

    if state.show_new_warning {
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("You have unsaved changes. Create a blank layout anyway?");
                ui.horizontal(|ui| {
                    if ui.button("Create blank").clicked() {
                        create_blank_layout(&mut config, &mut layout, &mut state);
                        state.show_new_warning = false;
                    }
                    if ui.button("Cancel").clicked() {
                        state.show_new_warning = false;
                    }
                });
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

fn next_actor_name_for_entity(layout: &LayoutDocument, entity_type: &str) -> String {
    let mut max_n = 0u32;
    let prefix = format!("actor_{entity_type}");
    for actor in &layout.actors {
        if let Some(suffix) = actor.name.strip_prefix(&prefix) {
            if let Ok(n) = suffix.parse::<u32>() {
                max_n = max_n.max(n);
            }
        }
    }
    format!("{prefix}{}", max_n + 1)
}

fn make_new_actor(actor_name: &str, entity_type: &str) -> ActorRecord {
    let raw_xml = format!(
        "<actor name=\"{name}\">\n\
         \x20 <Entity><attributes><EntityType value=\"{et}\"/></attributes></Entity>\n\
         \x20 <Prop name=\"Actor Specific\"><attributes>\n\
         \x20   <Position value=\"0 0 0\"/>\n\
         \x20   <Orientation value=\"0 0 0\"/>\n\
         \x20 </attributes></Prop>\n\
         </actor>",
        name = actor_name,
        et = entity_type,
    );
    ActorRecord {
        name: actor_name.to_string(),
        entity_type: entity_type.to_lowercase(),
        position: Vec3::ZERO,
        orientation: Vec3::ZERO,
        raw_xml,
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
    rb_game::oni2_loader::parsers::actor_xml::clear_xml_cache();
    match LayoutDocument::load(&config.layout_dir, &config.layout_name, &config.entity_dir) {
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

fn create_blank_layout(
    config: &mut EditorConfig,
    layout: &mut LayoutDocument,
    state: &mut EditorState,
) {
    config.set_layout("".to_string());
    layout.actors.clear();
    state.selected_actor = None;
    state.dirty = true;
    state.status = "New blank layout created in memory".to_string();
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
    mut images: ResMut<Assets<Image>>,
    mut skinned_mesh_ibp: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut entity_lib: ResMut<EntityLibrary>,
    mut anim_registry: ResMut<AnimRegistry>,
    existing: Query<Entity, With<ActorVisual>>,
    layout: Res<LayoutDocument>,
) {
    if !layout.is_changed() {
        return;
    }

    for e in &existing {
        commands.entity(e).despawn();
    }

    spawn_actor_visuals(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        &mut skinned_mesh_ibp,
        &mut entity_lib,
        &mut anim_registry,
        &layout,
    );
}

fn keyboard_shortcuts_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<EditorState>,
    mut layout: ResMut<LayoutDocument>,
    mut config: ResMut<EditorConfig>,
) {
    if keys.just_pressed(KeyCode::KeyT) {
        state.mode = EditMode::Transform;
        state.status = "Transform mode".to_string();
    } else if keys.just_pressed(KeyCode::KeyR) {
        state.mode = EditMode::Rotate;
        state.status = "Rotate mode".to_string();
    } else if keys.pressed(KeyCode::ControlLeft) && keys.just_pressed(KeyCode::KeyS) {
        if config.layout_name.trim().is_empty() {
            state.save_as_name = "new_layout".to_string();
            state.show_save_as_dialog = true;
            state.status = "No layout selected; choose a name to save.".to_string();
        } else {
            match layout.save(&config.layout_dir) {
                Ok(()) => {
                    state.dirty = false;
                    state.status = "Saved layout with Ctrl+S".to_string();
                }
                Err(err) => state.status = format!("Save failed: {err}"),
            }
        }
    } else if keys.pressed(KeyCode::ControlLeft) && keys.just_pressed(KeyCode::KeyN) {
        create_blank_layout(&mut config, &mut layout, &mut state);
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
    for (tf, handle) in &actors {
        let is_selected = state
            .selected_actor
            .as_ref()
            .is_some_and(|n| n == &handle.name);

        if state.show_bounds {
            let color = if is_selected {
                Color::srgb(0.2, 0.95, 0.4)
            } else {
                Color::srgb(1.0, 0.8, 0.2)
            };
            gizmos.cube(*tf, color);
        }

        if is_selected {
            let origin = tf.translation;
            let rotation = tf.rotation;
            let axis_len = 1.5;
            gizmos.line(
                origin,
                origin + rotation * Vec3::X * axis_len,
                Color::srgb(1.0, 0.2, 0.2),
            );
            gizmos.line(
                origin,
                origin + rotation * Vec3::Y * axis_len,
                Color::srgb(0.2, 1.0, 0.2),
            );
            gizmos.line(
                origin,
                origin + rotation * Vec3::Z * axis_len,
                Color::srgb(0.2, 0.55, 1.0),
            );
        }
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
    mut contexts: EguiContexts,
    mut query: Query<(&mut Transform, &mut OrbitCamera)>,
) {
    let egui_wants_scroll = contexts
        .ctx_mut()
        .ok()
        .is_some_and(|ctx| ctx.wants_pointer_input() || ctx.is_pointer_over_area());

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
            if !egui_wants_scroll {
                orbit.radius = (orbit.radius - ev.y * 0.8).clamp(2.0, 400.0);
            }
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
    /// Lowercase entity type name (e.g. "kno", "tctf").
    entity_type: String,
    /// Position in Bevy (right-handed) space.
    position: Vec3,
    /// Euler orientation in degrees XYZ, as stored in the ONI2 XML.
    orientation: Vec3,
    /// Full XML text of the actor file; updated in-place when position/orientation change.
    raw_xml: String,
}

impl ActorRecord {
    fn entity_name(&self) -> String {
        self.entity_type.clone()
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn rotation_radians(&self) -> Vec3 {
        self.orientation * (std::f32::consts::PI / 180.0)
    }

    fn set_position(&mut self, value: Vec3) {
        self.position = value;
        // ONI2 uses left-handed coordinates: negate X and Z when writing back.
        let oni2 = format!("{} {} {}", -value.x, value.y, -value.z);
        self.raw_xml = set_or_insert_xml_attr(&self.raw_xml, "Position", &oni2);
    }

    fn set_rotation(&mut self, euler_xyz: (f32, f32, f32)) {
        let deg = Vec3::new(
            euler_xyz.0.to_degrees(),
            euler_xyz.1.to_degrees(),
            euler_xyz.2.to_degrees(),
        );
        self.orientation = deg;
        let s = format!("{} {} {}", deg.x, deg.y, deg.z);
        self.raw_xml = set_or_insert_xml_attr(&self.raw_xml, "Orientation", &s);
    }
}

/// Replace the value="..." of a `<Tag value="..."/>` element in an XML string.
/// If the tag is not present, inserts a line for it just before `</actor>`.
/// This keeps the raw XML round-trippable while updating only what changed.
fn set_or_insert_xml_attr(xml: &str, tag: &str, new_value: &str) -> String {
    let search = format!("<{} value=\"", tag);
    if let Some(start) = xml.find(&search) {
        let val_start = start + search.len();
        if let Some(end_offset) = xml[val_start..].find('"') {
            let end = val_start + end_offset;
            return format!("{}{}{}", &xml[..val_start], new_value, &xml[end..]);
        }
    }
    // Tag not present — insert before closing </actor>.
    let insert = format!("  <{} value=\"{}\"/>\n", tag, new_value);
    if let Some(pos) = xml.rfind("</actor>") {
        format!("{}{}{}", &xml[..pos], insert, &xml[pos..])
    } else {
        format!("{}\n{}", xml, insert)
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

/// Return every subdirectory name under `entity_dir` as an EntityTypeEntry, sorted.
/// This is the full palette of placeable entity types regardless of which layout is loaded.
fn discover_entity_types(entity_dir: &Path) -> Vec<EntityTypeEntry> {
    let Ok(entries) = fs::read_dir(entity_dir) else {
        return Vec::new();
    };

    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .collect();
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

    names
        .into_iter()
        .map(|name| EntityTypeEntry {
            entity_type: name,
            thumbnail_path: None,
        })
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

fn create_wireframe_grid(size: f32, count: u32) -> Mesh {
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::PrimitiveTopology;

    let mut positions = Vec::new();
    let half_size = size / 2.0;
    let step = size / count as f32;

    for i in 0..=count {
        let t = -half_size + i as f32 * step;

        // Line along fixed X (t)
        positions.push([t, 0.0, -half_size]);
        positions.push([t, 0.0, half_size]);

        // Line along fixed Z (t)
        positions.push([-half_size, 0.0, t]);
        positions.push([half_size, 0.0, t]);
    }

    let mut mesh = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh
}
