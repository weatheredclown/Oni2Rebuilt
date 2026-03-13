use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;


#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Menu,
    LoadingLayout,
    InGame,
}

#[derive(Resource)]
pub struct SelectedLayout(pub String);

#[derive(Component)]
struct MenuRoot;

#[derive(Component)]
struct LayoutButton(String);

#[derive(Component)]
struct ScrollableList;

/// Marker for all entities spawned during InGame state. Cleaned up on exit.
#[derive(Component, Clone)]
pub struct InGameEntity;

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Menu), setup_menu)
            .add_systems(
                Update,
                (menu_interaction, scroll_list).run_if(in_state(AppState::Menu)),
            )
            .add_systems(OnExit(AppState::Menu), cleanup_menu)
            .add_systems(OnEnter(AppState::LoadingLayout), setup_loading_screen)
            .add_systems(
                Update,
                update_loading_screen.run_if(in_state(AppState::LoadingLayout)),
            )
            .add_systems(OnExit(AppState::LoadingLayout), cleanup_loading_screen)
            .add_systems(
                Update,
                escape_to_menu.run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnExit(AppState::InGame), cleanup_game);
    }
}

#[derive(Component)]
pub struct LoadingScreenEntity;

fn setup_loading_screen(
    mut commands: Commands,
    selected_layout: Option<Res<SelectedLayout>>,
    mut images: ResMut<Assets<Image>>,
) {
    let layout_name = selected_layout.as_ref().map(|s| s.0.as_str()).unwrap_or("tim06");
    let tex_filename = format!("texture/load_{}.tex", layout_name);
    let tga_filename = format!("texture/load_{}.tga", layout_name);
    let mut loaded_handle = None;

    if crate::vfs::exists("", &tex_filename) {
        if let Ok(tex_bytes) = crate::vfs::read("", &tex_filename) {
            if let Some((width, height, rgba, _)) = crate::oni2_loader::parsers::texture::decode_tex(&tex_bytes) {
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
                loaded_handle = Some(images.add(image));
            }
        }
    } else if crate::vfs::exists("", &tga_filename) {
        if let Some((handle, _)) = crate::oni2_loader::parsers::texture::load_tga_file("", &tga_filename, &mut images) {
            loaded_handle = Some(handle);
        }
    }

    commands.spawn((Camera2d, LoadingScreenEntity));

    if let Some(handle) = loaded_handle {
        commands.spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            LoadingScreenEntity,
        )).with_children(|parent| {
            parent.spawn((
                ImageNode::new(handle),
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ));
        });
    } else {
        commands.spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::BLACK),
            LoadingScreenEntity,
        ));
    }
}

fn update_loading_screen(
    mut frames_waited: Local<usize>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if *frames_waited >= 2 {
        next_state.set(AppState::InGame);
        *frames_waited = 0;
    } else {
        *frames_waited += 1;
    }
}

fn cleanup_loading_screen(
    mut commands: Commands,
    query: Query<Entity, With<LoadingScreenEntity>>,
) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

fn scan_layouts() -> Vec<(String, String)> {
    let target_dir = "layout".to_string();
    let mut all_folders = Vec::new();
    match crate::vfs::read_dir(&target_dir) {
        Ok(entries) => {
            for entry in entries {
                if entry.is_dir {
                    if let Some(name) = entry.path.split('/').last() {
                        all_folders.push(name.to_string());
                    }
                }
            }
        }
        Err(e) => {
            info!("scan_layouts: read_dir Err: {}", e);
        }
    }

    let mut descriptions = std::collections::HashMap::new();
    if let Ok(content) = crate::vfs::read_to_string("Settings", "rb.gamedata") {
        for line in content.lines() {
            if let Some(desc_idx) = line.find(" DESCRIPTION \"") {
                let folder = line[..desc_idx].trim().to_string();
                let desc_start = desc_idx + " DESCRIPTION \"".len();
                if let Some(desc_end) = line[desc_start..].find('"') {
                    let desc = line[desc_start..desc_start + desc_end].to_string();
                    descriptions.insert(folder, desc);
                }
            }
        }
    }

    let mut with_desc = Vec::new();
    let mut without_desc = Vec::new();

    for folder in all_folders {
        if let Some(desc) = descriptions.get(&folder) {
            with_desc.push((folder, desc.clone()));
        } else {
            without_desc.push((folder.clone(), folder));
        }
    }

    // Sort layouts with descriptions alphabetically by description
    with_desc.sort_by(|a, b| a.1.cmp(&b.1));
    // Sort remaining layouts alphabetically by folder name
    without_desc.sort_by(|a, b| a.1.cmp(&b.1));

    with_desc.extend(without_desc);
    with_desc
}

fn setup_menu(mut commands: Commands) {
    let layouts = scan_layouts();

    // Camera for menu UI rendering
    commands.spawn((Camera2d, MenuRoot));

    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::FlexStart,
                padding: UiRect::all(Val::Px(40.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.1, 0.1, 0.12)),
            MenuRoot,
        ))
        .with_children(|root| {
            // Title
            root.spawn((
                Text::new("Select Layout"),
                TextFont {
                    font_size: 48.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                Node {
                    margin: UiRect::bottom(Val::Px(20.0)),
                    ..default()
                },
            ));

            // Scrollable list container
            root.spawn((
                Node {
                    width: Val::Px(500.0),
                    flex_grow: 1.0,
                    flex_direction: FlexDirection::Column,
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
                ScrollableList,
            ))
            .with_children(|list| {
                for (folder_name, display_name) in &layouts {
                    list.spawn((
                        Button,
                        Node {
                            width: Val::Percent(100.0),
                            padding: UiRect::axes(Val::Px(16.0), Val::Px(10.0)),
                            margin: UiRect::bottom(Val::Px(4.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                        LayoutButton(folder_name.clone()),
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            Text::new(display_name.as_str()),
                            TextFont {
                                font_size: 20.0,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        ));
                    });
                }
            });
        });
}

fn menu_interaction(
    mut query: Query<
        (&Interaction, &LayoutButton, &mut BackgroundColor),
        Changed<Interaction>,
    >,
    mut next_state: ResMut<NextState<AppState>>,
    mut commands: Commands,
) {
    for (interaction, layout_btn, mut bg) in &mut query {
        match *interaction {
            Interaction::Pressed => {
                commands.insert_resource(SelectedLayout(layout_btn.0.clone()));
                next_state.set(AppState::LoadingLayout);
            }
            Interaction::Hovered => {
                *bg = BackgroundColor(Color::srgb(0.35, 0.35, 0.4));
            }
            Interaction::None => {
                *bg = BackgroundColor(Color::srgb(0.2, 0.2, 0.25));
            }
        }
    }
}

fn scroll_list(
    scroll: Res<AccumulatedMouseScroll>,
    mut query: Query<&mut ScrollPosition, With<ScrollableList>>,
) {
    if scroll.delta.y.abs() < f32::EPSILON {
        return;
    }
    for mut pos in &mut query {
        pos.y -= scroll.delta.y * 30.0;
        pos.y = pos.y.max(0.0);
    }
}

fn escape_to_menu(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if keyboard.just_pressed(KeyCode::Escape) {
        next_state.set(AppState::Menu);
    }
}

fn cleanup_game(mut commands: Commands, query: Query<Entity, With<InGameEntity>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

fn cleanup_menu(mut commands: Commands, query: Query<Entity, With<MenuRoot>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}
