use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;


#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Menu,
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
            .add_systems(
                Update,
                escape_to_menu.run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnExit(AppState::InGame), cleanup_game);
    }
}

fn scan_layouts() -> Vec<String> {
    let target_dir = "layout".to_string();
    let mut names = Vec::new();
    match crate::vfs::read_dir(&target_dir) {
        Ok(entries) => {
            for entry in entries {
                if entry.is_dir {
                    if let Some(name) = entry.path.split('/').last() {
                        names.push(name.to_string());
                    }
                }
            }
        }
        Err(e) => {
            println!("scan_layouts: read_dir Err: {}", e);
        }
    }
    names.sort();
    names
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
                for name in &layouts {
                    list.spawn((
                        Button,
                        Node {
                            width: Val::Percent(100.0),
                            padding: UiRect::axes(Val::Px(16.0), Val::Px(10.0)),
                            margin: UiRect::bottom(Val::Px(4.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                        LayoutButton(name.clone()),
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            Text::new(name.as_str()),
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
                next_state.set(AppState::InGame);
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
