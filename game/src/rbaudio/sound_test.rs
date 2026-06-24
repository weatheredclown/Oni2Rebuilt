use bevy::prelude::*;
use super::resolve::AudioAssets;
use crate::menu::AppState;

#[derive(Resource, Default)]
pub struct SoundTestState {
    /// List of sorted bank names
    pub bank_names: Vec<String>,
    /// Selected bank (if in sub-menu)
    pub selected_bank: Option<String>,
    /// List of named VAG files inside selected_bank
    pub vags: Vec<String>,
    /// Currently highlighted index
    pub highlighted_index: usize,
    /// Currently playing sound entity
    pub playing_sound: Option<Entity>,
}

#[derive(Component)]
pub struct SoundTestUiRoot;

#[derive(Component)]
pub struct SoundTestListContainer;

#[derive(Component)]
pub struct SoundTestItem {
    pub index: usize,
}

#[derive(Component)]
pub struct SoundTestTitleText;

pub struct SoundTestPlugin;

impl Plugin for SoundTestPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SoundTestState>()
            .add_systems(OnEnter(AppState::SoundMenu), setup_sound_test)
            .add_systems(OnExit(AppState::SoundMenu), cleanup_sound_test)
            .add_systems(
                Update,
                (
                    sound_test_keyboard_navigation,
                    sound_test_mouse_interaction,
                    sound_test_highlight_system,
                    watch_sound_test_state_rebuild,
                )
                    .run_if(in_state(AppState::SoundMenu)),
            );
    }
}

fn setup_sound_test(
    mut commands: Commands,
    mut assets: ResMut<AudioAssets>,
    mut state: ResMut<SoundTestState>,
) {
    // Ensure assets are loaded
    assets.ensure_loaded();

    // Reset state
    state.selected_bank = None;
    state.vags.clear();
    state.highlighted_index = 0;
    state.playing_sound = None;

    // Gather and sort bank names
    let mut banks = Vec::new();
    if let Some(td) = &assets.td {
        for name in td.bank_vags.keys() {
            banks.push(name.clone());
        }
    }
    banks.sort();
    state.bank_names = banks;

    // Spawn 2D Camera
    commands.spawn((Camera2d, SoundTestUiRoot));

    // Spawn UI hierarchy
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
            SoundTestUiRoot,
        ))
        .with_children(|root| {
            // Header
            root.spawn((
                Text::new("Sound Test"),
                TextFont {
                    font_size: 40.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                Node {
                    margin: UiRect::bottom(Val::Px(20.0)),
                    ..default()
                },
                SoundTestTitleText,
            ));

            // Scrollable list container
            root.spawn((
                Node {
                    width: Val::Px(600.0),
                    flex_grow: 1.0,
                    flex_direction: FlexDirection::Column,
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
                SoundTestListContainer,
                ScrollPosition::default(),
            ));
        });
}

fn cleanup_sound_test(
    mut commands: Commands,
    query: Query<Entity, With<SoundTestUiRoot>>,
    mut state: ResMut<SoundTestState>,
) {
    for entity in &query {
        commands.entity(entity).try_despawn();
    }
    if let Some(prev) = state.playing_sound.take() {
        commands.entity(prev).try_despawn();
    }
}

/// Watches for changes in `selected_bank` to rebuild list items.
fn watch_sound_test_state_rebuild(
    mut commands: Commands,
    state: Res<SoundTestState>,
    container_query: Query<Entity, With<SoundTestListContainer>>,
    mut title_query: Query<&mut Text, With<SoundTestTitleText>>,
    mut last_bank: Local<Option<String>>,
    mut initial: Local<bool>,
) {
    let current_bank = state.selected_bank.clone();
    let is_changed = current_bank != *last_bank || !*initial;

    if is_changed {
        *last_bank = current_bank.clone();
        *initial = true;

        if let Some(container) = container_query.iter().next() {
            // Clear existing children
            commands.entity(container).despawn_children();

            // Rebuild children under the container
            commands.entity(container).with_children(|list| {
                if let Some(ref bank_name) = current_bank {
                    // Update Title
                    if let Some(mut text) = title_query.iter_mut().next() {
                        text.0 = format!("Bank: {}", bank_name);
                    }

                    // Render VAG list
                    for (i, vag_name) in state.vags.iter().enumerate() {
                        list.spawn((
                            Button,
                            Node {
                                width: Val::Percent(100.0),
                                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                                margin: UiRect::bottom(Val::Px(4.0)),
                                ..default()
                            },
                            BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                            SoundTestItem { index: i },
                        ))
                        .with_children(|btn| {
                            btn.spawn((
                                Text::new(format!("{:03}: {}", i, vag_name)),
                                TextFont {
                                    font_size: 18.0,
                                    ..default()
                                },
                                TextColor(Color::WHITE),
                            ));
                        });
                    }
                } else {
                    // Update Title
                    if let Some(mut text) = title_query.iter_mut().next() {
                        text.0 = "Sound Test — Select Bank".to_string();
                    }

                    // Render Bank list
                    for (i, bank_name) in state.bank_names.iter().enumerate() {
                        list.spawn((
                            Button,
                            Node {
                                width: Val::Percent(100.0),
                                padding: UiRect::axes(Val::Px(16.0), Val::Px(10.0)),
                                margin: UiRect::bottom(Val::Px(4.0)),
                                ..default()
                            },
                            BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                            SoundTestItem { index: i },
                        ))
                        .with_children(|btn| {
                            btn.spawn((
                                Text::new(bank_name.as_str()),
                                TextFont {
                                    font_size: 20.0,
                                    ..default()
                                },
                                TextColor(Color::WHITE),
                            ));
                        });
                    }
                }
            });
        }
    }
}

fn sound_test_keyboard_navigation(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<SoundTestState>,
    mut next_state: ResMut<NextState<AppState>>,
    assets: Res<AudioAssets>,
    mut commands: Commands,
    mut audio_sources: ResMut<Assets<bevy::audio::AudioSource>>,
) {
    let current_len = if state.selected_bank.is_some() {
        state.vags.len()
    } else {
        state.bank_names.len()
    };

    if current_len == 0 {
        return;
    }

    if keyboard.just_pressed(KeyCode::ArrowDown) {
        state.highlighted_index = (state.highlighted_index + 1) % current_len;
    }

    if keyboard.just_pressed(KeyCode::ArrowUp) {
        state.highlighted_index = (state.highlighted_index + current_len - 1) % current_len;
    }

    if keyboard.just_pressed(KeyCode::Enter) || keyboard.just_pressed(KeyCode::NumpadEnter) {
        let index = state.highlighted_index;
        if state.selected_bank.is_none() {
            // Select bank
            let bank_name = state.bank_names[index].clone();
            let mut vags = Vec::new();
            if let Some(td) = &assets.td
                && let Some(list) = td.bank_vags.get(&bank_name)
            {
                vags = list.clone();
            }
            state.vags = vags;
            state.selected_bank = Some(bank_name);
            state.highlighted_index = 0;
        } else {
            // Play sound
            play_sound_by_index(&mut state, index, &mut commands, &assets, &mut audio_sources);
        }
    }

    if keyboard.just_pressed(KeyCode::Escape) {
        if let Some(bank_name) = state.selected_bank.take() {
            state.vags.clear();
            // Try to restore highlight to the exited bank
            if let Some(pos) = state.bank_names.iter().position(|b| b == &bank_name) {
                state.highlighted_index = pos;
            } else {
                state.highlighted_index = 0;
            }
        } else {
            // Exit back to main layout menu
            next_state.set(AppState::Menu);
        }
    }
}

fn sound_test_mouse_interaction(
    mut query: Query<(&Interaction, &SoundTestItem), Changed<Interaction>>,
    mut state: ResMut<SoundTestState>,
    assets: Res<AudioAssets>,
    mut commands: Commands,
    mut audio_sources: ResMut<Assets<bevy::audio::AudioSource>>,
) {
    for (interaction, item) in &mut query {
        match *interaction {
            Interaction::Pressed => {
                if state.selected_bank.is_none() {
                    let bank_name = state.bank_names[item.index].clone();
                    let mut vags = Vec::new();
                    if let Some(td) = &assets.td
                        && let Some(list) = td.bank_vags.get(&bank_name)
                    {
                        vags = list.clone();
                    }
                    state.vags = vags;
                    state.selected_bank = Some(bank_name);
                    state.highlighted_index = 0;
                } else {
                    play_sound_by_index(&mut state, item.index, &mut commands, &assets, &mut audio_sources);
                }
            }
            Interaction::Hovered => {
                state.highlighted_index = item.index;
            }
            _ => {}
        }
    }
}

fn sound_test_highlight_system(
    state: Res<SoundTestState>,
    mut item_query: Query<(&SoundTestItem, &mut BackgroundColor)>,
    mut scroll_query: Query<&mut ScrollPosition, With<SoundTestListContainer>>,
) {
    let target = state.highlighted_index;

    // Highlight items visually
    for (item, mut bg) in &mut item_query {
        if item.index == target {
            *bg = BackgroundColor(Color::srgb(0.25, 0.45, 0.85));
        } else {
            *bg = BackgroundColor(Color::srgb(0.18, 0.18, 0.22));
        }
    }

    // Auto-scroll logic (keep highlighted item in view)
    // Item height is ~44px (height + margin)
    // Viewport height is estimated to fill container, let's keep it safe.
    if let Some(mut scroll) = scroll_query.iter_mut().next() {
        let item_height = 44.0;
        let viewport_height = 480.0;
        let item_top = target as f32 * item_height;
        let item_bottom = item_top + item_height;

        let scroll_y = scroll.y;
        if item_top < scroll_y {
            scroll.y = item_top;
        } else if item_bottom > scroll_y + viewport_height {
            scroll.y = item_bottom - viewport_height;
        }
    }
}

fn play_sound_by_index(
    state: &mut SoundTestState,
    index: usize,
    commands: &mut Commands,
    assets: &AudioAssets,
    audio_sources: &mut Assets<bevy::audio::AudioSource>,
) {
    let Some(bank_name) = &state.selected_bank else {
        return;
    };
    if index >= state.vags.len() {
        return;
    }

    // Stop previous sound
    if let Some(prev) = state.playing_sound.take() {
        commands.entity(prev).try_despawn();
    }

    // Decode and play
    if let Some((source, _loop)) = super::resolve::decode_bank_sound(bank_name, index, audio_sources) {
        let child = commands
            .spawn((
                bevy::audio::AudioPlayer(source),
                bevy::audio::PlaybackSettings::DESPAWN,
                super::AudioGroup::new(super::AudioGroupId::Player, 1.0),
                crate::menu::InGameEntity,
            ))
            .id();
        state.playing_sound = Some(child);
        info!("SoundTest: Playing VAG index {} ({}) in bank {}", index, state.vags[index], bank_name);
    } else {
        warn!("SoundTest: Failed to decode VAG index {} ({}) in bank {}", index, state.vags[index], bank_name);
    }
}
