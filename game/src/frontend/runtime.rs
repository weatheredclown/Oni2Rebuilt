/*
 * frontend/runtime.rs — FrontendPlugin + state resources.
 *
 * Mirrors the legacy `rbUIManager` / `uiManager`:
 *   - Startup loader parses rbfrontend.ui into `UiFile`.
 *   - `FrontendState` holds the current page name + history stack
 *     (legacy `uiManager::CurrentPage` + `PageHistoryList`).
 *   - `RequestPageChange` message drives transitions; handled
 *     between frames so UI teardown/respawn is consistent (legacy
 *     deferred `PageToSelectAtEndOfFrame`).
 *   - `OnEnter(AppState::FrontEnd)` routes to `UiFile.startup_page`
 *     via the same mechanism — mirrors `PostLoadSetup` at
 *     `PostLoadSetup`.
 *
 * Rendering + input + handler execution lives in `render.rs`.
 */
use bevy::prelude::*;

use super::ast::{HandlerKind, UiFile};
use super::parser::parse_ui;
use crate::menu::AppState;

pub struct FrontendPlugin;

impl Plugin for FrontendPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrontendState>()
            .init_resource::<FrontendLevelList>()
            .init_resource::<super::render::FrontendRoot>()
            .add_message::<RequestPageChange>()
            .add_message::<FireFrontendHandler>()
            .add_systems(Startup, load_frontend_ui)
            .add_systems(
                Update,
                (
                    // Boots the page graph on the first Update tick
                    // after `load_frontend_ui` finishes.  We can't use
                    // OnEnter(FrontEnd) because FrontEnd is the
                    // *initial* state — Bevy fires OnEnter only on
                    // transitions, so the initial-state entry is
                    // silent.  The gate `current_page.is_none()` makes
                    // this a one-shot that arms itself again on
                    // OnExit (which clears current_page, see
                    // `teardown_frontend`).
                    route_to_startup_page
                        .run_if(|state: Res<FrontendState>| state.current_page.is_none()),
                    super::render::rebuild_current_page.run_if(
                        state_changed::<AppState>
                            .or(frontend_current_page_changed)
                            .or(resource_changed::<FrontendUi>)
                            .or(resource_changed::<FrontendLevelList>),
                    ),
                    super::render::input_dispatch_system,
                    super::render::handler_exec_system.after(super::render::input_dispatch_system),
                    apply_requested_page_change.after(super::render::handler_exec_system),
                    super::render::stub_autoadvance_system,
                    super::camera::frontend_menu_camera_apply_seq
                        .after(super::render::rebuild_current_page),
                    super::render::project_3d_items_to_viewport
                        .after(super::render::rebuild_current_page)
                        .after(super::camera::frontend_menu_camera_apply_seq),
                )
                    .run_if(in_state(AppState::FrontEnd)),
            )
            .add_observer(super::camera::frontend_menu_camera_scroni_observer)
            .add_systems(OnExit(AppState::FrontEnd), super::render::teardown_frontend);
    }
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// The parsed `rbfrontend.ui`.  `None` until `load_frontend_ui` runs
/// (Startup schedule).  Parsing failures log an error and leave the
/// resource `None` — the frontend state then sits idle until either
/// a CLI bypass flag skips it or the user exits.
#[derive(Resource, Default)]
pub struct FrontendUi(pub Option<UiFile>);

/// Current frontend navigation cursor.  `current_page` is the page
/// name (matches `Page.name` in the parsed AST).  `page_history` is
/// the back-stack used by `GO_TO_PREVIOUS_PAGE` — legacy
/// `uiManager::PageHistoryList`.
#[derive(Resource, Default)]
pub struct FrontendState {
    pub current_page: Option<String>,
    pub page_history: Vec<String>,
    /// Seconds since the current page was entered — compared against
    /// `page.time_to_disable_events` in the input dispatcher.
    pub time_on_page: f32,
    /// Index of the currently focused item within the current page's
    /// item list (matches `uiPage::CurrentItem`).
    pub focus_index: usize,
    /// Index of the item that lost focus last frame — mirrors
    /// `uiPage::ItemLostFocusLastFrame` (used by ON_FOCUS_LOSE).
    pub focus_lost_last_frame: Option<usize>,
    /// Set on the frame the page was switched — the page-start
    /// detector uses this, cleared on the NEXT frame.  Mirrors
    /// `uiManager::PageSwitchedFromLastFrame`.
    pub page_just_changed: bool,
}

/// Backs ITEM_LEVEL_LIST items on pages like `Choose_Level`.
/// Populated on page entry by scanning the `layout/` directory via
/// the VFS (same source the legacy --testlayout picker uses).  The
/// legacy uiItemLevelList kept this list + selection index inside
/// the item itself; we lift it to a resource because the Rect2DText
/// / focus pipeline in `render.rs` doesn't have per-item mutable
/// state yet.
#[derive(Resource, Default)]
pub struct FrontendLevelList {
    /// (folder, description) pairs.  Description falls back to the
    /// folder name when `rb.gamedata` doesn't name it.
    pub entries: Vec<(String, String)>,
    /// Index into `entries` of the currently highlighted row.
    pub selected: usize,
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Request a page transition.  Queued by handlers (GO_TO_PAGE /
/// GO_TO_PREVIOUS_PAGE) and applied by `apply_requested_page_change`
/// at the end of the frame so the UI teardown + respawn happen
/// together.
#[derive(Message, Clone, Debug)]
pub enum RequestPageChange {
    /// Absolute — go to the named page.
    Named(String),
    /// Pop one off the history stack.
    Previous,
}

/// An emitted handler ready to execute.  The input dispatcher fires
/// these; `handler_exec_system` drains them and applies the effect.
/// Kept as a message (not an inline call) so IF_* conditionals stay
/// simple — they expand their nested handler lists into these
/// messages at fire time.
#[derive(Message, Clone, Debug)]
pub struct FireFrontendHandler {
    pub kind: HandlerKind,
}

// ---------------------------------------------------------------------------
// Run-if gates
// ---------------------------------------------------------------------------

/// Triggers rebuild only when `FrontendState.current_page` actually
/// changes.  Using `resource_changed::<FrontendState>` would fire every
/// frame because `input_dispatch_system` ticks `time_on_page` via
/// `ResMut<FrontendState>` each frame — that would despawn + respawn
/// the whole UI tree (and re-request the background texture) every
/// tick, flooding the log with "Path not found" and
/// "Entity despawned" errors for assets the rebuild races against.
fn frontend_current_page_changed(
    state: Res<FrontendState>,
    mut last_page: Local<Option<String>>,
) -> bool {
    if last_page.as_deref() != state.current_page.as_deref() {
        *last_page = state.current_page.clone();
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Startup loader
// ---------------------------------------------------------------------------

fn load_frontend_ui(mut commands: Commands) {
    // Legacy path: `assets/Settings/rbfrontend.ui`.  Using the VFS so
    // DAT-archive mounts work for shipped builds too.
    let path = "Settings";
    let file = "rbfrontend.ui";
    match crate::vfs::read_to_string(path, file) {
        Ok(src) => match parse_ui(&src) {
            Ok(ui) => {
                info!(
                    "frontend: loaded {}/{} — {} pages, startup='{}'",
                    path,
                    file,
                    ui.pages.len(),
                    ui.startup_page.as_deref().unwrap_or("<none>"),
                );
                commands.insert_resource(FrontendUi(Some(ui)));
            }
            Err(e) => {
                error!("frontend: parse error in {}/{}: {}", path, file, e);
                commands.insert_resource(FrontendUi(None));
            }
        },
        Err(e) => {
            error!(
                "frontend: failed to read {}/{}: {} — frontend will remain idle",
                path, file, e
            );
            commands.insert_resource(FrontendUi(None));
        }
    }
}

// ---------------------------------------------------------------------------
// Startup-page routing
// ---------------------------------------------------------------------------

fn route_to_startup_page(
    ui: Option<Res<FrontendUi>>,
    mut state: ResMut<FrontendState>,
    mut writer: MessageWriter<RequestPageChange>,
) {
    let Some(ui_res) = ui else {
        return;
    };
    let Some(ref ui_file) = ui_res.0 else {
        return;
    };
    // First preference: ON_STARTUP (release builds).  Fallback: the
    // first page in declaration order — mirrors `PageList.GetHead()`.
    let target = ui_file
        .startup_page
        .clone()
        .or_else(|| ui_file.pages.first().map(|p| p.name.clone()));
    if let Some(name) = target {
        state.current_page = None; // force a transition
        state.page_history.clear();
        writer.write(RequestPageChange::Named(name));
    } else {
        warn!("frontend: UI file has no pages to display");
    }
}

// ---------------------------------------------------------------------------
// Page-change application
// ---------------------------------------------------------------------------

fn apply_requested_page_change(
    mut reader: MessageReader<RequestPageChange>,
    mut state: ResMut<FrontendState>,
    ui: Option<Res<FrontendUi>>,
    mut levels: ResMut<FrontendLevelList>,
) {
    // Read all pending requests; last one wins.  Legacy
    // `PageToSelectAtEndOfFrame` had the same "last write wins"
    // behavior — only one page change per tick applied.
    let mut final_target: Option<RequestPageChange> = None;
    for msg in reader.read() {
        final_target = Some(msg.clone());
    }
    let Some(req) = final_target else {
        return;
    };
    let Some(ui_res) = ui else {
        return;
    };
    let Some(ref ui_file) = ui_res.0 else {
        return;
    };

    let new_page = match req {
        RequestPageChange::Named(name) => {
            // Validate the page exists; warn and stay put if not.
            if !ui_file.pages.iter().any(|p| p.name == name) {
                warn!("frontend: GO_TO_PAGE target '{}' not found", name);
                return;
            }
            name
        }
        RequestPageChange::Previous => match state.page_history.pop() {
            Some(name) => name,
            None => {
                warn!("frontend: GO_TO_PREVIOUS_PAGE with empty history");
                return;
            }
        },
    };

    if state.current_page.as_deref() == Some(new_page.as_str()) {
        // No-op, don't push history.
        return;
    }

    if let Some(prev) = state.current_page.take() {
        state.page_history.push(prev);
    }
    state.current_page = Some(new_page.clone());
    state.time_on_page = 0.0;
    state.focus_index = 0;
    state.focus_lost_last_frame = None;
    state.page_just_changed = true;
    info!("frontend: → page '{}'", new_page);

    // If the destination page has an ITEM_LEVEL_LIST, refresh the
    // level list from disk now so the renderer sees up-to-date
    // entries.  Re-scanning every visit (rather than caching once at
    // startup) is cheap and lets the user drop new layouts into the
    // assets tree without restarting.  Selection resets to 0 — the
    // legacy code restored the last-played level from save data; we
    // punt on that until a save system lands.
    if let Some(page) = ui_file.pages.iter().find(|p| p.name == new_page) {
        let needs_levels = page.items.iter().any(|i| {
            matches!(
                i.kind,
                super::ast::ItemKind::LevelList(_) | super::ast::ItemKind::LevelSavePointList(_)
            )
        });
        if needs_levels {
            // Use rb.gamedata declaration order (legacy campaign list)
            // — that's what `RunGame Level <idx>` indexes into and
            // what the original Choose_Level page rendered.  The
            // broader folder-scan (`scan_layouts`) is reserved for
            // the dev `--testlayout` picker.
            levels.entries = crate::menu::parse_gamedata_levels();
            levels.selected = 0;
            info!(
                "frontend: populated level list with {} entries (from rb.gamedata)",
                levels.entries.len()
            );
        }
    }
}
