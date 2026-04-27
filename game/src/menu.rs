/*
 * menu.rs — MenuPlugin: main menu, animation browser, entity browser, and
 * layout-loading flow.
 *
 * Defines AppState (Menu → LoadingLayout / AnimMenu / EntityMenu → InGame),
 * SelectedLayout / TestAnimEntity resources, and all UI systems for scrollable
 * layout / anim / entity picker lists.  InGameEntity is the scoped marker
 * component used to despawn everything when leaving InGame.
 */
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;

#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    /// Drives the legacy `rbfrontend.ui` page graph (Rockstar intro
    /// → Angel intro → Oni2 title → Main Menu → Level Select →
    /// into game).  This is the polished player-facing menu; while
    /// it's still being wired up it's hidden behind `--ogmenu`.
    FrontEnd,
    /// Default startup state — the dev-friendly debug picker
    /// (formerly `--testlayout`).  Lists every layout folder on disk
    /// + descriptions from `Settings/rb.gamedata`, and is what the
    /// game also drops back into on Escape from a level.
    #[default]
    Menu,
    AnimMenu,
    EntityMenu,
    LoadingLayout,
    InGame,
}

#[derive(Resource, Default)]
pub struct MenuScrollState {
    pub layout: f32,
    pub anim: f32,
    pub entity: f32,
}

/// Per-layout actor-load progress.  `total_actors` is read from
/// `layout.actors` (first line is the count) at the start of the
/// LoadingLayout state; `loaded_actors` ticks up at a fixed rate per
/// frame so the bar fills in a visible, count-proportional way.
///
/// Why count-proportional at a fake rate rather than true per-actor:
/// our Rust layout load is synchronous, runs in a single `setup_scene`
/// system at OnEnter(InGame), and takes ~100ms — the render loop
/// doesn't tick during it, so a bar driven by "actually spawned"
/// would visibly jump from 0 → 100 in one frame.  Ticking at
/// `PER_ACTOR_SECS` per actor gives a fill whose DURATION scales with
/// real level size (172 actors → ~1.72s, 30 → ~0.3s), which is what
/// the user sees and expects.  Legacy ran at 90 seconds for a PS2 disc
/// load (rb/src/rbgame/loadthread.cpp:75) — totally fake too, but a
/// constant.
#[derive(Resource, Default)]
pub struct LoadingProgress {
    /// Total actor count parsed from `layout.actors` header.
    pub total_actors: usize,
    /// Virtual loaded count — ticks up from 0 to `total_actors` at
    /// `PER_ACTOR_SECS` per actor during LoadingLayout.
    pub loaded_actors: usize,
    /// Fractional carry so sub-tick progress accumulates across frames.
    pub carry: f32,
}

impl LoadingProgress {
    pub fn fraction(&self) -> f32 {
        if self.total_actors == 0 {
            1.0
        } else {
            (self.loaded_actors as f32 / self.total_actors as f32).clamp(0.0, 1.0)
        }
    }
}

/// Virtual time budget per actor.  Short enough that small levels don't
/// linger, long enough that big levels give the player a beat to read
/// the bar.  Legacy used ~90s of fake-time regardless of level size —
/// this is the per-actor equivalent at a reasonable modern pace.
pub const PER_ACTOR_SECS: f32 = 0.01;

/// Legacy bar rect on a 512×448 framebuffer: `(348, 56)` origin,
/// `122×10` pixels (rb/src/rbgame/loadthread.cpp:164).  Normalized
/// percentages so the bar lands in the same relative screen spot at
/// any window size.
pub const LOADING_BAR_X_PCT: f32 = 348.0 / 512.0 * 100.0;
pub const LOADING_BAR_Y_PCT: f32 = 56.0 / 448.0 * 100.0;
pub const LOADING_BAR_W_PCT: f32 = 122.0 / 512.0 * 100.0;
pub const LOADING_BAR_H_PCT: f32 = 10.0 / 448.0 * 100.0;

/// Marker for the foreground (filled) rectangle of the progress bar —
/// `update_loading_screen` scales its width each frame.
#[derive(Component)]
pub struct LoadingBarFill;

/// Marker for the "X / Y actors" text label next to the bar.
#[derive(Component)]
pub struct LoadingCountText;

#[derive(Resource)]
pub struct TestAnimEntity(pub String);

#[derive(Component)]
struct AnimMenuRoot;

#[derive(Component)]
struct AnimButton(String);

#[derive(Resource)]
pub struct SelectedLayout(pub String);

/// Inserted at startup by `main.rs` when the `--ogmenu` CLI flag is
/// present — the user is opting into the in-progress `rbfrontend.ui`
/// menu.  `escape_to_menu` checks for this marker so leaving a level
/// returns to that frontend rather than the default debug picker.
/// Absent in normal `--testlayout`-style boots (which is the default
/// while the frontend is being wired up).
#[derive(Resource)]
pub struct OgMenuMode;

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
        app.init_resource::<MenuScrollState>()
            .init_resource::<LoadingProgress>()
            .add_systems(OnEnter(AppState::Menu), setup_menu)
            .add_systems(
                Update,
                (menu_interaction, scroll_list).run_if(in_state(AppState::Menu)),
            )
            .add_systems(OnExit(AppState::Menu), cleanup_menu)
            .add_systems(OnEnter(AppState::AnimMenu), setup_anim_menu)
            .add_systems(
                Update,
                (anim_menu_interaction, scroll_list).run_if(in_state(AppState::AnimMenu)),
            )
            .add_systems(OnExit(AppState::AnimMenu), cleanup_anim_menu)
            .add_systems(OnEnter(AppState::EntityMenu), setup_entity_menu)
            .add_systems(
                Update,
                (entity_menu_interaction, scroll_list).run_if(in_state(AppState::EntityMenu)),
            )
            .add_systems(OnExit(AppState::EntityMenu), cleanup_entity_menu)
            .add_systems(OnEnter(AppState::LoadingLayout), setup_loading_screen)
            // Actor-spawn driver lives in the loader module and is
            // always-on — gated by `PendingLayoutLoad` resource
            // presence, not AppState.  The frontend (PAGE_3D backdrop
            // load) drops the resource while in AppState::FrontEnd,
            // so an in_state gate here would strand the backdrop.
            .add_systems(
                Update,
                crate::oni2_loader::layout_loader::drive_chunked_actor_spawn_system,
            )
            // Tag layout spawns with FrontendBackdrop when the load is
            // scoped to the frontend — runs alongside the driver so
            // actors spawned mid-chunk get the marker immediately.
            .add_systems(
                Update,
                crate::oni2_loader::layout_loader::tag_frontend_layout_spawns_system
                    .after(crate::oni2_loader::layout_loader::drive_chunked_actor_spawn_system),
            )
            // Loading-screen progress bar is LoadingLayout-only.
            .add_systems(
                Update,
                update_loading_screen.run_if(in_state(AppState::LoadingLayout)),
            )
            .add_systems(OnExit(AppState::LoadingLayout), cleanup_loading_screen)
            .add_systems(Update, escape_to_menu.run_if(in_state(AppState::InGame)))
            .add_systems(OnExit(AppState::InGame), cleanup_game);
    }
}

#[derive(Component)]
pub struct LoadingScreenEntity;

fn setup_loading_screen(
    mut commands: Commands,
    selected_layout: Option<Res<SelectedLayout>>,
    mut images: ResMut<Assets<Image>>,
    mut progress: ResMut<LoadingProgress>,
) {
    // Purge any leftovers from a prior load.  If the previous session
    // left a finished `PendingLayoutLoad` in the ECS and this load
    // returns None (e.g. sandbox / fallback layout path missing), the
    // `update_loading_screen` "done" branch reads that stale state,
    // reports it as complete against the old actor count, and
    // flashes the old layout's progress numbers before transitioning.
    // Reset both the sentinel resource and the progress fields so
    // we always start from a clean slate.
    commands.remove_resource::<crate::oni2_loader::layout_loader::PendingLayoutLoad>();
    commands.remove_resource::<crate::oni2_loader::layout_loader::LoadedLayoutPlayer>();
    *progress = LoadingProgress::default();

    let layout_name = selected_layout
        .as_ref()
        .map(|s| s.0.as_str())
        .unwrap_or("tim06");

    // Kick off the chunked layout load.  `begin_chunked_layout_load`
    // runs the pre-actor phase synchronously (parse layout.et /
    // layout.paths / layout.graphs, queue actor names) and returns a
    // `PendingLayoutLoad` state that `drive_chunked_actor_spawn`
    // drains one actor per tick.  The total actor count we display
    // on the loading screen comes from the real queue length, not
    // the first-line integer — that's an authoring hint that doesn't
    // always match the full list when actors are blank-/comment-
    // skipped.
    let layout_dir = format!("layout/{}", layout_name);
    let pending = crate::oni2_loader::layout_loader::begin_chunked_layout_load(
        &mut commands,
        &layout_dir,
        "Entity",
    );
    let total_actors = pending.as_ref().map(|p| p.actor_names.len()).unwrap_or(0);
    if let Some(p) = pending {
        commands.insert_resource(p);
    }
    *progress = LoadingProgress {
        total_actors,
        loaded_actors: 0,
        carry: 0.0,
    };

    let tex_filename = format!("texture/load_{}.tex", layout_name);
    let tga_filename = format!("texture/load_{}.tga", layout_name);
    let mut loaded_handle = None;

    if crate::vfs::exists("", &tex_filename) {
        if let Ok(tex_bytes) = crate::vfs::read("", &tex_filename)
            && let Some((width, height, rgba, _)) =
                crate::oni2_loader::parsers::texture::decode_tex(&tex_bytes)
        {
            let image = Image::new(
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
    } else if crate::vfs::exists("", &tga_filename)
        && let Some((handle, _)) =
            crate::oni2_loader::parsers::texture::load_tga_file("", &tga_filename, &mut images)
    {
        loaded_handle = Some(handle);
    }

    commands.spawn((Camera2d, LoadingScreenEntity));

    // Fullscreen root that holds either the layout's load_<layout>.tga
    // backdrop or a fallback black background, with the progress bar
    // positioned absolutely in both cases.  Legacy reserved a specific
    // 122×10-pixel slot on a 512×448 canvas at (348, 56) for the bar;
    // rendering it at the same normalized position gives authored
    // `load_<layout>.tga` images (which have a blank area there) the
    // expected fill, and for layouts without a load image we still show
    // a small red bar in the same spot so players have a visible
    // progress cue.
    let root = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Relative,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            LoadingScreenEntity,
        ))
        .id();

    if let Some(handle) = loaded_handle {
        commands.entity(root).with_children(|parent| {
            parent.spawn((
                ImageNode::new(handle),
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    position_type: PositionType::Absolute,
                    ..default()
                },
            ));
        });
    }

    // Progress bar + "X / Y actors" text label, positioned at the
    // legacy rect coordinates.  The background rect draws a semi-
    // transparent container; a red child inside scales its width with
    // the actual `loaded_actors / total_actors` ratio.  Text sits just
    // to the right of the bar so the count is readable alongside it.
    commands.entity(root).with_children(|parent| {
        parent
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(LOADING_BAR_X_PCT),
                    top: Val::Percent(LOADING_BAR_Y_PCT),
                    width: Val::Percent(LOADING_BAR_W_PCT),
                    height: Val::Percent(LOADING_BAR_H_PCT),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
                LoadingScreenEntity,
            ))
            .with_children(|inner| {
                inner.spawn((
                    Node {
                        width: Val::Percent(0.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(1.0, 0.0, 0.0)),
                    LoadingBarFill,
                ));
            });

        // Count label: "X / Y actors" positioned just below the bar.
        parent.spawn((
            Text::new(format!("0 / {} actors", total_actors)),
            TextFont {
                font_size: 14.0,
                ..default()
            },
            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(LOADING_BAR_X_PCT),
                top: Val::Percent(LOADING_BAR_Y_PCT + LOADING_BAR_H_PCT + 0.5),
                ..default()
            },
            LoadingCountText,
            LoadingScreenEntity,
        ));
    });
}

fn update_loading_screen(
    mut progress: ResMut<LoadingProgress>,
    pending: Option<Res<crate::oni2_loader::layout_loader::PendingLayoutLoad>>,
    mut bar_q: Query<&mut Node, With<LoadingBarFill>>,
    mut text_q: Query<&mut Text, With<LoadingCountText>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    // Mirror the real actor-spawn cursor onto `LoadingProgress`
    // every tick.  `drive_chunked_actor_spawn` (runs just before us
    // in the chained set) bumps the cursor by the chunk size, then
    // `post_done` flips once the post-actor phase has run.
    if let Some(p) = &pending {
        progress.loaded_actors = p.cursor;
        if progress.total_actors == 0 {
            progress.total_actors = p.actor_names.len();
        }
    }

    let fraction = progress.fraction();
    for mut node in &mut bar_q {
        node.width = Val::Percent(fraction * 100.0);
    }
    for mut text in &mut text_q {
        text.0 = format!(
            "{} / {} actors",
            progress.loaded_actors, progress.total_actors
        );
    }

    // Only transition once the queue is empty AND the post-actor
    // phase has finalized — otherwise camera packages / lights would
    // still be missing on OnEnter(InGame).
    let done = match pending.as_deref() {
        Some(p) => p.cursor >= p.actor_names.len() && p.post_done,
        // No pending load (edge case: layout.actors couldn't be read,
        // or we're on a sandbox / fallback path).  Fall through to
        // transition immediately.
        None => true,
    };
    if done {
        next_state.set(AppState::InGame);
        *progress = LoadingProgress::default();
    }
}

// NOTE: the chunked actor-spawn driver used to live here as
// `drive_chunked_actor_spawn`.  It moved to
// `oni2_loader::layout_loader::drive_chunked_actor_spawn_system`
// and is now registered at the plugin level above, always-on, so
// PAGE_3D backdrop loads share the same pipeline without AppState
// churn.  See that function for behavior + scope semantics.

fn cleanup_loading_screen(
    mut commands: Commands,
    mut progress: ResMut<LoadingProgress>,
    query: Query<Entity, With<LoadingScreenEntity>>,
) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
    // Reset the progress bar + drop the PendingLayoutLoad resource here.
    // We DO NOT drop LoadedLayoutPlayer here because `setup_scene` (OnEnter InGame)
    // needs to consume it to spawn the player. `setup_scene` will remove it
    // once consumed.
    *progress = LoadingProgress::default();
    commands.remove_resource::<crate::oni2_loader::layout_loader::PendingLayoutLoad>();
}

/// Parse `Settings/rb.gamedata` into `(folder, description)` pairs in
/// the order the file declares them.  The first line is the entry
/// count; subsequent lines look like `<folder> DESCRIPTION "<desc>"`.
/// Returns an empty Vec if the file is missing or unparseable.
///
/// This is the legacy "campaign list" — `RunGame Level <N>` indexes
/// into it, the frontend's `Choose_Level` LevelList renders it in
/// the same order, and `RunGame` (no args) re-runs the layout at
/// whichever index was last selected.
pub fn parse_gamedata_levels() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(content) = crate::vfs::read_to_string("Settings", "rb.gamedata") else {
        return out;
    };
    for line in content.lines() {
        let Some(desc_idx) = line.find(" DESCRIPTION \"") else {
            continue;
        };
        let folder = line[..desc_idx].trim().to_string();
        if folder.is_empty() {
            continue;
        }
        let desc_start = desc_idx + " DESCRIPTION \"".len();
        let Some(desc_end) = line[desc_start..].find('"') else {
            continue;
        };
        let desc = line[desc_start..desc_start + desc_end].to_string();
        out.push((folder, desc));
    }
    out
}

/// Folder-scan + rb.gamedata description merge.  Returns every layout
/// folder present on disk, with descriptions filled in where the
/// gamedata file names them.  Used by the legacy `--testlayout`
/// picker, which wants to surface every layout (including dev/test
/// scenes that aren't in rb.gamedata).  The frontend's `Choose_Level`
/// LevelList does NOT use this — it uses `parse_gamedata_levels`
/// directly so its ordering and contents match the campaign and
/// `RunGame Level <idx>` lines up correctly.
pub fn scan_layouts() -> Vec<(String, String)> {
    let target_dir = "layout".to_string();
    let mut all_folders = Vec::new();
    match crate::vfs::read_dir(&target_dir) {
        Ok(entries) => {
            for entry in entries {
                if entry.is_dir
                    && let Some(name) = entry.path.split('/').next_back()
                {
                    all_folders.push(name.to_string());
                }
            }
        }
        Err(e) => {
            info!("scan_layouts: read_dir Err: {}", e);
        }
    }

    let descriptions: std::collections::HashMap<String, String> =
        parse_gamedata_levels().into_iter().collect();

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

fn setup_menu(mut commands: Commands, scroll_state: Res<MenuScrollState>) {
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
                ScrollPosition(Vec2::new(0.0, scroll_state.layout.max(0.0))),
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
    mut query: Query<(&Interaction, &LayoutButton, &mut BackgroundColor), Changed<Interaction>>,
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
    app_state: Res<State<AppState>>,
    mut scroll_state: ResMut<MenuScrollState>,
    scroll: Res<AccumulatedMouseScroll>,
    mut query: Query<&mut ScrollPosition, With<ScrollableList>>,
) {
    if scroll.delta.y.abs() < f32::EPSILON {
        return;
    }
    for mut pos in &mut query {
        pos.y -= scroll.delta.y * 30.0;
        pos.y = pos.y.max(0.0);

        match app_state.get() {
            AppState::Menu => scroll_state.layout = pos.y,
            AppState::AnimMenu => scroll_state.anim = pos.y,
            AppState::EntityMenu => scroll_state.entity = pos.y,
            _ => {}
        }
    }
}

fn escape_to_menu(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut next_state: ResMut<NextState<AppState>>,
    test_anim_mode: Option<Res<crate::oni2_loader::TestAnimMode>>,
    test_entity_mode: Option<Res<crate::oni2_loader::TestEntityMode>>,
    ogmenu_mode: Option<Res<OgMenuMode>>,
) {
    if keyboard.just_pressed(KeyCode::Escape) {
        if test_anim_mode.is_some() {
            commands.remove_resource::<crate::oni2_loader::TestAnimMode>();
            next_state.set(AppState::AnimMenu);
        } else if test_entity_mode.is_some() {
            commands.remove_resource::<crate::oni2_loader::TestEntityMode>();
            next_state.set(AppState::EntityMenu);
        } else if ogmenu_mode.is_some() {
            // User opted into the new frontend via `--ogmenu` —
            // return them there.
            next_state.set(AppState::FrontEnd);
        } else {
            // Default: drop back to the dev test-layout picker (the
            // formerly `--testlayout` UI) so iterating on layouts
            // stays one Esc-press away.
            next_state.set(AppState::Menu);
        }
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

fn cleanup_anim_menu(mut commands: Commands, query: Query<Entity, With<AnimMenuRoot>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

fn scan_anims_for_entity(entity: &str) -> Vec<(String, String)> {
    let entity_dir = format!("Entity/{}", entity);
    let tune_dir = "entity.tune".to_string();
    let tune_entity = format!("{}/{}", tune_dir, entity);
    let tune_anims = format!("{}/{}.anims", tune_entity, entity);
    let entity_anims = format!("{}/{}.anims", entity_dir, entity);

    let anims_path = if crate::vfs::exists("", &tune_anims) {
        tune_anims
    } else {
        entity_anims
    };

    let mut alias_map = std::collections::HashMap::new();
    let mut loco_pkg = None;

    if let Ok(content) = crate::vfs::read_to_string("", &anims_path) {
        crate::oni2_loader::parsers::anims::parse_anims_content(
            &content,
            &mut alias_map,
            &mut loco_pkg,
            &mut None,
        );
    }

    let mut results = Vec::new();
    for (alias, anim_name) in &alias_map {
        let anim_file = format!("{}/{}.anim", entity_dir, anim_name);
        if crate::vfs::exists("", &anim_file) {
            results.push((alias.clone(), anim_file));
        } else {
            // Check prefix fallback matching core spawn logic
            if let Some(prefix) = anim_name.split('_').next() {
                let mut parts: Vec<&str> = entity_dir.split('/').collect();
                if let Some(last) = parts.last_mut() {
                    *last = prefix;
                }
                let fallback_dir = parts.join("/");
                let fallback_file = format!("{}/{}.anim", fallback_dir, anim_name);
                if crate::vfs::exists("", &fallback_file) {
                    results.push((alias.clone(), fallback_file));
                }
            }
        }
    }
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

fn setup_anim_menu(
    mut commands: Commands,
    test_ent: Res<TestAnimEntity>,
    scroll_state: Res<MenuScrollState>,
) {
    let anims = scan_anims_for_entity(&test_ent.0);

    // Camera for menu UI rendering
    commands.spawn((Camera2d, AnimMenuRoot));

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
            AnimMenuRoot,
        ))
        .with_children(|root| {
            // Title
            root.spawn((
                Text::new(format!("Select Animation For: {}", test_ent.0)),
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
                ScrollPosition(Vec2::new(0.0, scroll_state.anim.max(0.0))),
            ))
            .with_children(|list| {
                for (alias, file_path) in &anims {
                    list.spawn((
                        Button,
                        Node {
                            width: Val::Percent(100.0),
                            padding: UiRect::axes(Val::Px(16.0), Val::Px(10.0)),
                            margin: UiRect::bottom(Val::Px(4.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                        AnimButton(file_path.clone()),
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            Text::new(format!(
                                "{}  ->  {}",
                                alias,
                                file_path.split('/').next_back().unwrap_or(file_path)
                            )),
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

fn anim_menu_interaction(
    mut query: Query<(&Interaction, &AnimButton, &mut BackgroundColor), Changed<Interaction>>,
    mut next_state: ResMut<NextState<AppState>>,
    mut commands: Commands,
) {
    for (interaction, anim_btn, mut bg) in &mut query {
        match *interaction {
            Interaction::Pressed => {
                // The TestAnimMode resource instructs testanim.rs to spawn this anim natively from the CLI
                commands.insert_resource(crate::oni2_loader::TestAnimMode(anim_btn.0.clone()));
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

#[derive(Component)]
struct EntityMenuRoot;

#[derive(Component)]
struct EntityButton(String);

fn scan_entities(filter_layout: Option<&str>) -> Vec<String> {
    let mut allowed_entities = std::collections::HashSet::new();
    let mut use_filter = false;

    if let Some(layout_name) = filter_layout {
        let layout_et_path = format!("layout/{}/layout.et", layout_name);
        if let Ok(content) = crate::vfs::read_to_string("", &layout_et_path) {
            use_filter = true;
            for line in content.lines() {
                let tokens: Vec<&str> = line.split_whitespace().collect();
                if tokens.len() >= 3 && tokens[0].eq_ignore_ascii_case("BASICENTITY") {
                    allowed_entities.insert(tokens[2].to_lowercase());
                }
            }
        } else {
            warn!(
                "Could not read layout.et for '{}', showing all entities instead.",
                layout_name
            );
        }
    }

    let target_dir = "Entity".to_string();
    let mut all_folders = Vec::new();
    if let Ok(entries) = crate::vfs::read_dir(&target_dir) {
        for entry in entries {
            if entry.is_dir
                && let Some(name) = entry.path.split('/').next_back()
            {
                let name_str = name.to_string();
                if !use_filter || allowed_entities.contains(&name_str.to_lowercase()) {
                    all_folders.push(name_str);
                }
            }
        }
    }
    all_folders.sort();
    all_folders
}

fn setup_entity_menu(
    mut commands: Commands,
    scroll_state: Res<MenuScrollState>,
    selected_layout: Option<Res<SelectedLayout>>,
) {
    let entities = scan_entities(selected_layout.as_ref().map(|sl| sl.0.as_str()));

    commands.spawn((Camera2d, EntityMenuRoot));

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
            EntityMenuRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("Select Entity"),
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

            root.spawn((
                Node {
                    width: Val::Px(500.0),
                    flex_grow: 1.0,
                    flex_direction: FlexDirection::Column,
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
                ScrollableList,
                ScrollPosition(Vec2::new(0.0, scroll_state.entity.max(0.0))),
            ))
            .with_children(|list| {
                for folder_name in &entities {
                    list.spawn((
                        Button,
                        Node {
                            width: Val::Percent(100.0),
                            padding: UiRect::axes(Val::Px(16.0), Val::Px(10.0)),
                            margin: UiRect::bottom(Val::Px(4.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
                        EntityButton(folder_name.clone()),
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            Text::new(folder_name.as_str()),
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

fn entity_menu_interaction(
    mut query: Query<(&Interaction, &EntityButton, &mut BackgroundColor), Changed<Interaction>>,
    mut next_state: ResMut<NextState<AppState>>,
    mut commands: Commands,
) {
    for (interaction, entity_btn, mut bg) in &mut query {
        match *interaction {
            Interaction::Pressed => {
                commands.insert_resource(crate::oni2_loader::TestEntityMode(entity_btn.0.clone()));
                next_state.set(AppState::InGame);
            }
            Interaction::Hovered => *bg = BackgroundColor(Color::srgb(0.35, 0.35, 0.4)),
            Interaction::None => *bg = BackgroundColor(Color::srgb(0.2, 0.2, 0.25)),
        }
    }
}

fn cleanup_entity_menu(mut commands: Commands, query: Query<Entity, With<EntityMenuRoot>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}
