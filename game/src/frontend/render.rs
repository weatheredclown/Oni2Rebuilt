/*
 * frontend/render.rs — PAGE_2D renderer + input dispatch + handler
 * executor.
 *
 * Phase 3 scope: enough of the legacy rbfrontend runtime to walk
 * the shipped `rbfrontend.ui` page graph by keyboard.
 *
 *   - `rebuild_current_page` spawns Bevy UI entities for the
 *     current page's items.  Re-runs whenever `FrontendState`
 *     changes page.
 *   - `input_dispatch_system` reads keyboard, moves focus between
 *     items (UP/DOWN/LEFT/RIGHT = direction events), fires
 *     ON_INPUT_OK / CANCEL / START on Enter/Esc/Tab.  Respects
 *     `page.time_to_disable_events` (legacy event-firing guard).
 *   - `handler_exec_system` drains `FireFrontendHandler` messages
 *     and runs the effects — GO_TO_PAGE / GO_TO_PREVIOUS_PAGE at
 *     minimum; PLAY_SOUND / unknowns log and continue.
 *   - `stub_autoadvance_system` gives PAGE_MOVIE pages a fake
 *     completion event after a short timer so the Rockstar →
 *     Angel → Oni2 intro chain drains without a real movie decoder.
 *
 * PAGE_3D currently falls through to the same PAGE_2D rendering
 * path minus the background texture — the text items are laid
 * out in a simple vertical stack so the page is still navigable.
 * Real 3D backdrop + CAMERA_INIT rendering is phase 5.
 */
use bevy::prelude::*;

use super::ast::{
    CameraInit, EventKind, HandlerKind, Item, ItemKind, List2DProps, PageKind, StringSource,
};
use super::runtime::{
    FireFrontendHandler, FrontendLevelList, FrontendState, FrontendUi, RequestPageChange,
};
use crate::menu::{AppState, SelectedLayout};

/// Marker for every UI entity spawned by the frontend renderer.
/// Cleaned up on page change + OnExit(FrontEnd).
#[derive(Component)]
pub struct FrontendUiEntity;

/// Marker for the chunk-loaded PAGE_3D backdrop scene actors (the
/// uitest layout behind Main_Menu, etc.).  These survive the per-page
/// `rebuild_current_page` sweep — only `teardown_frontend` collects
/// them on FrontEnd exit.  Without this, every SET_VISIBLE-driven
/// rebuild would despawn the backdrop and leave a black screen until
/// the next page change re-kicked the chunked load.
#[derive(Component)]
pub struct FrontendBackdrop;

/// "Chrome" of the current page — camera, lights, root container,
/// PAGE_2D background ImageNode.  Despawned only on actual page
/// transition (i.e. when `current_page` changes), NOT on the per-
/// frame SET_VISIBLE / focus-driven rebuilds that fire whenever the
/// player nudges an arrow key.  Without this split, every SET_VISIBLE
/// handler ends up freeing & re-allocating a fresh `Camera3d` (with
/// new wgpu render targets) which leaks GPU memory until OOM, AND
/// resets the camera's Transform back to the static CAMERA_INIT pose
/// — making the scene appear to "snap back" until the next
/// CameraMotion `CameraTrackPoint` lerps it forward again.
#[derive(Component)]
pub struct FrontendChromeEntity;

/// Per-page-state UI items: the text nodes, list rows, and item
/// containers that get re-evaluated against `is_visible` on every
/// SET_VISIBLE.  Spawned as children of the chrome's root node.
#[derive(Component)]
pub struct FrontendItemEntity;

/// Pointer to the current page's chrome root Node, so the item
/// rebuild can re-parent fresh items into it without recreating the
/// chrome.  Cleared on page transition (the next chrome rebuild
/// repopulates it) and on FrontEnd exit.
#[derive(Resource, Default)]
pub struct FrontendRoot(pub Option<Entity>);

/// Tags an item-text node with the index of the item it renders, so
/// focus changes can flip the color without rebuilding the tree.
#[derive(Component)]
pub struct FrontendItemText {
    pub item_index: usize,
}

/// Marker for the PAGE_3D camera — the projector system looks this up
/// to convert 3D item anchors to screen-space each frame, and the
/// frontend camera observer in `frontend::camera` lands ScrOni
/// `Camera*` sys-events on it.
#[derive(Component)]
pub struct FrontendMenuCamera;

/// Attached to the UI text node of a Rect3DText item.  Holds both rect
/// corners (authored local or world space, per the legacy `TOP_LEFT` /
/// `BOTTOM_RIGHT` tokens) plus the optional `ATTACHED_TO` actor name.
/// The projection system composes the named actor's current
/// GlobalTransform with the local-space corners, then projects each
/// through the menu camera to form a screen-space rect — mirrors
/// `uiItemRect3D::Draw`'s `AttachedTo->GetMatrix().Transform()` path.
/// When `attached_to` is None the corners are treated as world space.
#[derive(Component, Debug, Clone)]
pub struct Frontend3DAnchor {
    pub top_left: Vec3,
    pub bottom_right: Vec3,
    pub attached_to: Option<String>,
}

// Legacy UI coordinate space was 512x448 (PS2 framebuffer).  Items
// author pixel-space coords against that canvas; we map them to
// viewport percentages so the UI scales to any window size.
pub const UI_CANVAS_W: f32 = 512.0;
pub const UI_CANVAS_H: f32 = 448.0;

fn pct_x(px: i32) -> f32 {
    (px as f32 / UI_CANVAS_W) * 100.0
}
fn pct_y(px: i32) -> f32 {
    (px as f32 / UI_CANVAS_H) * 100.0
}

// ---------------------------------------------------------------------------
// Rebuild UI for the current page
// ---------------------------------------------------------------------------

pub fn rebuild_current_page(
    mut commands: Commands,
    ui: Option<Res<FrontendUi>>,
    state: Res<FrontendState>,
    levels: Res<FrontendLevelList>,
    chrome_existing: Query<Entity, (With<FrontendChromeEntity>, Without<FrontendBackdrop>)>,
    items_existing: Query<Entity, With<FrontendItemEntity>>,
    mut root_res: ResMut<FrontendRoot>,
    mut last_page: Local<Option<String>>,
    mut images: ResMut<Assets<Image>>,
    ready: Option<Res<crate::oni2_loader::layout_loader::FrontendLayoutReady>>,
    pending_load: Option<Res<crate::oni2_loader::layout_loader::PendingLayoutLoad>>,
) {
    let Some(ui_res) = ui else {
        // No UI loaded — wipe everything and bail.
        for e in &chrome_existing {
            commands.entity(e).despawn();
        }
        for e in &items_existing {
            commands.entity(e).despawn();
        }
        root_res.0 = None;
        *last_page = None;
        return;
    };
    let Some(ref ui_file) = ui_res.0 else {
        return;
    };
    let Some(ref page_name) = state.current_page else {
        // Page cleared (teardown is about to fire) — drop chrome + items.
        for e in &chrome_existing {
            commands.entity(e).despawn();
        }
        for e in &items_existing {
            commands.entity(e).despawn();
        }
        root_res.0 = None;
        *last_page = None;
        return;
    };
    let Some(page) = ui_file.pages.iter().find(|p| p.name == *page_name) else {
        return;
    };

    let page_changed = last_page.as_deref() != Some(page_name.as_str());
    *last_page = Some(page_name.clone());

    // Items always go — they're the cheap, content-driven layer.
    for e in &items_existing {
        commands.entity(e).despawn();
    }

    // Chrome (camera + lights + root + PAGE_2D background) only swaps
    // on actual page transition.  Bevy 0.18's `commands.entity(e).despawn()`
    // recursively despawns descendants, so the root takes its
    // background ImageNode child with it.
    let root: Entity = if page_changed {
        for e in &chrome_existing {
            commands.entity(e).despawn();
        }
        let new_root = spawn_page_chrome(
            &mut commands,
            page,
            &mut images,
            ready.as_deref(),
            pending_load.as_deref(),
        );
        root_res.0 = Some(new_root);
        new_root
    } else {
        // Reuse the existing root.  If it's missing (shouldn't happen
        // — chrome rebuild always sets it), fall back to a full
        // rebuild rather than leak items into nowhere.
        match root_res.0 {
            Some(r) => r,
            None => {
                let new_root = spawn_page_chrome(
                    &mut commands,
                    page,
                    &mut images,
                    ready.as_deref(),
                    pending_load.as_deref(),
                );
                root_res.0 = Some(new_root);
                new_root
            }
        }
    };

    // Spawn each item under the (possibly-reused) root.  For PAGE_2D,
    // use its authored pixel coords.  For PAGE_3D, items project
    // through the menu camera each frame.
    let is_3d = matches!(page.kind, PageKind::Page3D { .. });
    for (idx, item) in page.items.iter().enumerate() {
        if !item.is_visible {
            continue;
        }
        spawn_item(
            &mut commands,
            root,
            idx,
            item,
            is_3d,
            idx == state.focus_index,
            &levels,
        );
    }
}

/// Spawn the page chrome (camera, lights, root, PAGE_2D background,
/// PAGE_3D layout-load kick) and return the root entity.  Tags every
/// entity with both `FrontendUiEntity` and `FrontendChromeEntity` so
/// the per-frame item rebuild leaves them alone.
fn spawn_page_chrome(
    commands: &mut Commands,
    page: &super::ast::Page,
    images: &mut Assets<Image>,
    ready: Option<&crate::oni2_loader::layout_loader::FrontendLayoutReady>,
    pending_load: Option<&crate::oni2_loader::layout_loader::PendingLayoutLoad>,
) -> Entity {
    // Camera choice follows the page kind.  PAGE_3D gets a real
    // Camera3d at the authored CAMERA_INIT transform so the UI items'
    // world-space anchors project correctly; PAGE_2D/PAGE_MOVIE get a
    // plain Camera2d.
    match &page.kind {
        PageKind::Page3D { camera_init, .. } => {
            spawn_menu_camera_3d(commands, camera_init.as_ref());
        }
        _ => {
            commands.spawn((
                Camera2d,
                Camera {
                    clear_color: ClearColorConfig::Custom(Color::BLACK),
                    ..default()
                },
                FrontendUiEntity,
                FrontendChromeEntity,
                crate::crt_post::CrtSettings::ps2_crt(),
            ));
        }
    }

    // Fullscreen root.  For PAGE_2D this is the canvas the BACKGROUND
    // ImageNode fills; for PAGE_3D it's a transparent overlay so the
    // 3D scene shows through and our Rect3DText UI nodes project on
    // top of it.
    let is_3d_page = matches!(page.kind, PageKind::Page3D { .. });
    let root_bg = if is_3d_page {
        Color::NONE
    } else {
        Color::BLACK
    };
    let root = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Relative,
                ..default()
            },
            BackgroundColor(root_bg),
            FrontendUiEntity,
            FrontendChromeEntity,
        ))
        .id();

    match &page.kind {
        PageKind::Page2D { background } => {
            if let Some(bg_name) = background {
                spawn_background(commands, root, bg_name, images);
            }
        }
        PageKind::Page3D { layout, .. } => {
            // Kick off the chunked load of the page's LAYOUT directive
            // (e.g. Main_Menu → uitest) through the shared loader —
            // same pipeline the in-game load uses, but scoped
            // `Frontend` so the spawned actors pick up FrontendBackdrop
            // for cleanup-on-page-exit.  Skip the kick if we've already
            // loaded this backdrop (navigating away to a sub-page and
            // back to the same PAGE_3D shouldn't re-parse / re-spawn).
            let layout_dir = format!("layout/{}", layout);
            let already_loaded = matches!(
                ready,
                Some(r) if r.layout_name == layout_dir
            );
            let already_loading = pending_load.is_some();
            if !already_loaded && !already_loading {
                if let Some(pending) = crate::oni2_loader::layout_loader::begin_frontend_layout_load(
                    commands,
                    &layout_dir,
                    "Entity",
                ) {
                    commands.insert_resource(pending);
                } else {
                    warn!(
                        "frontend: PAGE_3D layout '{}' failed to start loading",
                        layout
                    );
                }
            }
            // Minimal scene lighting while the backdrop loads.  The
            // layout's own lights come in through `finalize_chunked_
            // layout_load`, but they arrive asynchronously across the
            // chunked load — give the user something to see in the
            // meantime.
            commands.spawn((
                DirectionalLight {
                    illuminance: 6_000.0,
                    shadows_enabled: false,
                    ..default()
                },
                Transform::from_xyz(5.0, 10.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
                FrontendUiEntity,
                FrontendChromeEntity,
            ));
            commands.spawn((
                AmbientLight {
                    color: Color::srgb(0.4, 0.4, 0.5),
                    brightness: 600.0,
                    ..default()
                },
                FrontendUiEntity,
                FrontendChromeEntity,
            ));
        }
        PageKind::PageMovie { movie } => {
            // Phase 4: real MPEG-2 decoder.  Show a label so the
            // player sees the stub is intentional.  This label is
            // page-static (doesn't change with item visibility) so
            // it lives on the chrome layer.
            commands.entity(root).with_children(|parent| {
                parent.spawn((
                    Text::new(format!("[movie stub: {}]", movie)),
                    TextFont {
                        font_size: 18.0,
                        ..default()
                    },
                    TextColor(Color::srgba(1.0, 1.0, 1.0, 0.6)),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(40.0),
                        top: Val::Percent(48.0),
                        ..default()
                    },
                    FrontendUiEntity,
                    FrontendChromeEntity,
                ));
            });
        }
    }

    root
}

fn spawn_background(
    commands: &mut Commands,
    root: Entity,
    bg_name: &str,
    images: &mut Assets<Image>,
) {
    // Backgrounds live at `texture/<name>.tga` in the mounted VFS
    // (e.g. `oni2/zips/assets/texture/ui_bg_oni2.tga`).  We can't go
    // through `asset_server.load` — that resolves against Bevy's
    // hardcoded `assets/` root (our `game/assets/` which is empty by
    // design, BYO assets).
    let filename = format!("{}.tga", bg_name);
    let Some((handle, _)) =
        crate::oni2_loader::parsers::texture::load_tga_file("texture", &filename, images)
    else {
        warn!("frontend: background texture/{} not found in VFS", filename);
        return;
    };
    commands.entity(root).with_children(|parent| {
        parent.spawn((
            ImageNode::new(handle),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            FrontendUiEntity,
            FrontendChromeEntity,
        ));
    });
}

fn spawn_item(
    commands: &mut Commands,
    root: Entity,
    idx: usize,
    item: &Item,
    is_3d: bool,
    is_focused: bool,
    levels: &FrontendLevelList,
) {
    let color_focus = Color::srgba(1.0, 0.85, 0.2, 1.0);
    let color_normal = Color::srgba(1.0, 1.0, 1.0, 0.75);
    let text_color = if is_focused {
        color_focus
    } else {
        color_normal
    };

    // LevelList + LevelSavePointList: render the actual scrolled
    // list in-place, not a bare label.
    if let ItemKind::LevelList(props) | ItemKind::LevelSavePointList(props) = &item.kind {
        spawn_level_list(commands, root, idx, props, levels);
        return;
    }

    // Figure out a label (item's string id, or name if no string).
    let label = match &item.kind {
        ItemKind::Rect2DText { string, .. } | ItemKind::Rect3DText { string, .. } => match string {
            StringSource::StringId(s) => s.clone(),
            StringSource::LevelName => "<level>".to_string(),
        },
        ItemKind::Slider2D { .. } => format!("{} (slider)", item.name),
        ItemKind::Rect2D(_) | ItemKind::Rect3D(_) | ItemKind::QuadList2D(_) => item.name.clone(),
        ItemKind::Invisible | ItemKind::Actor { .. } | ItemKind::Rect2DGrid { .. } => {
            // No visible representation in this pass.
            return;
        }
        ItemKind::LevelList(_) | ItemKind::LevelSavePointList(_) => {
            unreachable!("LevelList / LevelSavePointList handled by spawn_level_list above",)
        }
    };

    // 3D pages: the legacy `uiItemRect3DText::Draw` rendered each
    // string into an offscreen texture and drew a world-space quad
    // between TOP_LEFT and BOTTOM_RIGHT, optionally transformed by
    // the `ATTACHED_TO` actor's matrix.  We approximate that by
    // stashing both corners + the attachment name on a
    // `Frontend3DAnchor`; the per-frame `project_3d_items_to_viewport`
    // system resolves the named actor, composes its current
    // `GlobalTransform` with the local corners, projects both through
    // the menu camera, and fits the text node into the resulting
    // screen-space rect.  That's what makes the Main_Menu labels ride
    // the spinning `actor_Projector`.
    if is_3d {
        let (top_left, bottom_right, attached_to, color) = match &item.kind {
            ItemKind::Rect3DText { rect, .. } | ItemKind::Rect3D(rect) => {
                let authored = Color::srgba(
                    rect.color[0] as f32 / 255.0,
                    rect.color[1] as f32 / 255.0,
                    rect.color[2] as f32 / 255.0,
                    rect.color[3] as f32 / 255.0,
                );
                let tinted = if is_focused {
                    authored
                } else {
                    let mut linear = authored.to_linear();
                    linear.alpha *= 0.5;
                    linear.into()
                };
                (
                    rect.top_left,
                    rect.bottom_right,
                    rect.attached_to.clone(),
                    tinted,
                )
            }
            _ => (Vec3::ZERO, Vec3::new(0.5, -0.1, 0.0), None, text_color),
        };
        commands.entity(root).with_children(|parent| {
            parent.spawn((
                Text::new(label),
                TextFont {
                    font_size: 16.0, // project system overwrites per frame
                    ..default()
                },
                TextColor(color),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(-10_000.0),
                    top: Val::Px(-10_000.0),
                    ..default()
                },
                FrontendUiEntity,
                FrontendItemEntity,
                FrontendItemText { item_index: idx },
                Frontend3DAnchor {
                    top_left,
                    bottom_right,
                    attached_to,
                },
            ));
        });
        return;
    }

    // 2D pages: honor the authored rect.  Rect2D family uses int
    // pixel coords on the 512×448 virtual canvas.
    let rect = match &item.kind {
        ItemKind::Rect2D(r) => Some(r),
        ItemKind::Rect2DText { rect, .. } => Some(rect),
        ItemKind::Rect2DGrid { rect, .. } => Some(rect),
        ItemKind::Slider2D { rect, .. } => Some(rect),
        ItemKind::LevelList(p) | ItemKind::LevelSavePointList(p) => Some(&p.rect),
        _ => None,
    };

    let (left_pct, top_pct, w_pct, h_pct) = if let Some(r) = rect {
        (
            pct_x(r.top_left.0),
            pct_y(r.top_left.1),
            pct_x(r.width),
            pct_y(r.height),
        )
    } else {
        // Fallback — stack like 3D.
        (25.0, 10.0 + idx as f32 * 6.0, 50.0, 5.0)
    };

    let font_size = (h_pct * UI_CANVAS_H / 100.0 * 0.7).clamp(12.0, 28.0);

    commands.entity(root).with_children(|parent| {
        parent.spawn((
            Text::new(label),
            TextFont {
                font_size,
                ..default()
            },
            TextColor(text_color),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(left_pct),
                top: Val::Percent(top_pct),
                width: Val::Percent(w_pct),
                height: Val::Percent(h_pct),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            FrontendUiEntity,
            FrontendItemEntity,
            FrontendItemText { item_index: idx },
        ));
    });
}

/// Render an ITEM_LEVEL_LIST / ITEM_LEVEL_SAVE_POINT_LIST inside its
/// authored rect.  Each entry in `FrontendLevelList.entries` becomes
/// a row of text; the `selected` row gets the highlight color, all
/// others get the normal color.  We don't page/scroll yet — if the
/// list is taller than the rect, overflow clips.
///
/// Every row carries `FrontendItemText { item_index }` tagging it
/// with the OWNING ITEM'S index (not the row index), so the focus-
/// color-flip logic in `input_dispatch_system` keeps working
/// uniformly: when the LevelList item gains/loses focus, all rows
/// recolor together.  Per-row selection color is applied here at
/// build time and refreshed via the FrontendLevelList resource-
/// changed rebuild trigger.
fn spawn_level_list(
    commands: &mut Commands,
    root: Entity,
    item_idx: usize,
    props: &List2DProps,
    levels: &FrontendLevelList,
) {
    let r = &props.rect;
    let left_pct = pct_x(r.top_left.0);
    let top_pct = pct_y(r.top_left.1);
    let w_pct = pct_x(r.width);
    let h_pct = pct_y(r.height);

    // Author colors fall back to reasonable defaults matching the
    // Choose_Level shipped values (amber highlight, dim amber idle).
    fn srgb_from_rgba8(c: [u8; 4]) -> Color {
        Color::srgba(
            c[0] as f32 / 255.0,
            c[1] as f32 / 255.0,
            c[2] as f32 / 255.0,
            c[3] as f32 / 255.0,
        )
    }
    let col_highlight = props
        .color_highlight
        .map(srgb_from_rgba8)
        .unwrap_or(Color::srgba(1.0, 0.6, 0.0, 0.8));
    let col_normal = props
        .color_normal
        .map(srgb_from_rgba8)
        .unwrap_or(Color::srgba(1.0, 0.6, 0.0, 0.35));

    // Row height and font in virtual canvas px, then converted.
    // VERTICAL_SPACING in the .ui file is a line-height multiplier;
    // a plausible base line is 20px of the 448-tall canvas.
    let spacing = props.vertical_spacing.max(1.0);
    let row_px = 20.0 * spacing;
    let row_h_pct = row_px / UI_CANVAS_H * 100.0;
    let font_size = (row_px * 0.75).clamp(12.0, 28.0);

    commands.entity(root).with_children(|parent| {
        // Outer bounded node to clip overflow to the authored rect.
        parent
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(left_pct),
                    top: Val::Percent(top_pct),
                    width: Val::Percent(w_pct),
                    height: Val::Percent(h_pct),
                    flex_direction: FlexDirection::Column,
                    overflow: Overflow::clip(),
                    ..default()
                },
                FrontendUiEntity,
                FrontendItemEntity,
            ))
            .with_children(|col| {
                if levels.entries.is_empty() {
                    col.spawn((
                        Text::new("(no levels found)"),
                        TextFont {
                            font_size,
                            ..default()
                        },
                        TextColor(col_normal),
                        Node {
                            position_type: PositionType::Relative,
                            width: Val::Percent(100.0),
                            height: Val::Percent(row_h_pct.min(100.0)),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                        FrontendUiEntity,
                        FrontendItemText {
                            item_index: item_idx,
                        },
                    ));
                    return;
                }
                for (i, (_folder, desc)) in levels.entries.iter().enumerate() {
                    let color = if i == levels.selected {
                        col_highlight
                    } else {
                        col_normal
                    };
                    col.spawn((
                        Text::new(desc.clone()),
                        TextFont {
                            font_size,
                            ..default()
                        },
                        TextColor(color),
                        Node {
                            position_type: PositionType::Relative,
                            width: Val::Percent(100.0),
                            height: Val::Px(row_px),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                        FrontendUiEntity,
                        FrontendItemText {
                            item_index: item_idx,
                        },
                    ));
                }
            });
    });
}

// ---------------------------------------------------------------------------
// PAGE_3D camera + per-frame anchor projection
// ---------------------------------------------------------------------------
//
// FUTURE: load the uitest layout behind the menu.
//
// The legacy Main_Menu page renders against a real 3D scene — the
// `uitest` layout — with a rotating projector actor, tunnel geometry,
// and a camera-swoop driven by the `actor_Tunnel1` script
// (`$newgame:CameraMotion`).  Our current pass spawns only a camera +
// lights against an empty scene, so the text floats over black.
//
// Integration sketch when we come back to this:
//   1. Factor `oni2_loader::layout_loader::begin_chunked_layout_load`
//      + `spawn_next_chunk` + `finalize_chunked_layout_load` so they
//      take a target `InGameEntity`-like marker + world rather than
//      being bound to `AppState::LoadingLayout`.
//   2. On `OnEnter(PAGE_3D-with-layout)` kick the chunked loader.
//      Park the input dispatcher until the loader finalizes (reuse
//      `TIME_TO_DISABLE_EVENTS` as a grace period that starts from
//      "finalize complete" rather than "page entered").
//   3. Teach `spawn_item` (the ATTACHED_TO branch) to stash the
//      actor name + offset on the Rect3DText anchor so the project
//      system composes the actor's current transform with the rect's
//      local-space midpoint each frame — that's what makes the
//      labels ride the spinning projector.
//   4. Wire ANIMATION and SCRIPT handlers to the real AnimEvent /
//      scroni dispatch paths so ON_PAGE_START kicks off the
//      projector spin + camera motion scripts.
//   5. Kill the uitest layout on page exit via the existing
//      `FrontendUiEntity` sweep (needs the loader to parent all its
//      spawns under that marker).
//
// Until then this is a functional but flat menu: text appears at the
// correct projected positions, focus navigation works, the handler
// pipeline fires — just no backdrop art.

fn spawn_menu_camera_3d(commands: &mut Commands, cam: Option<&CameraInit>) {
    // Fallback camera for PAGE_3D pages that didn't author CAMERA_INIT
    // — rbfrontend.ui always does, but defensive defaults keep a
    // malformed or stripped-down .ui file from booting into a black
    // screen.
    let (pos, look, fov_deg) = match cam {
        Some(c) => (c.position, c.track_point, c.fov),
        None => (Vec3::new(0.0, 1.5, -4.0), Vec3::new(0.0, 0.5, 0.0), 60.0),
    };
    let transform = Transform::from_translation(pos).looking_at(look, Vec3::Y);
    // Spawn Camera3d + Projection together in one tuple.  Bevy 0.18's
    // `Camera3d` require chain adds Camera + CameraRenderGraph; an
    // earlier comment in this file warned that including Projection
    // in the spawn tuple short-circuits the chain, so we used to
    // post-insert it via `commands.entity(...).insert(Projection)`.
    // The "Camera has no render graph configured" warning persists
    // either way (see investigation in task #16) — try this form
    // first since it's the more conventional pattern.
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: fov_deg.to_radians(),
            ..default()
        }),
        // Clear with solid black instead of Bevy's default neutral
        // gray.  The legacy frontend renders against pure black wherever
        // the 3D scene doesn't cover, and the gray was bleeding around
        // the projector ring on Main_Menu.
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        transform,
        FrontendMenuCamera,
        IsDefaultUiCamera,
        FrontendUiEntity,
        FrontendChromeEntity,
        // Pre-attach the per-frame lerp state so the first ScrOni
        // Camera* event fires cleanly into an existing component
        // instead of the observer's "insert + bail" path.
        super::camera::FrontendMenuCameraSeq::default(),
        // PS2-CRT-look post-process — gamma curve + black crush +
        // saturation lift, applied after tonemapping.
        crate::crt_post::CrtSettings::ps2_crt(),
    ));
}

/// Projects both corners of every `Frontend3DAnchor` through the menu
/// camera and sizes the UI text node to the resulting screen-space
/// rect.  Matches `uiItemRect3D::Draw` — when `attached_to` names an
/// actor, each local-space corner gets composed with that actor's
/// current `GlobalTransform` before projection, so labels ride the
/// spinning projector.  Without a named actor (or if it hasn't
/// spawned yet), corners are treated as world space directly.
///
/// We take the axis-aligned bounding rect of the two projected
/// corners and fit the text inside it — a Bevy UI node can't tilt
/// with the projected quad, so we settle for position + size
/// tracking.  If either corner ends up behind the camera we park the
/// label off-screen rather than mirror-projecting.  Font size is
/// derived from the on-screen height so text stays readable at any
/// camera distance.
pub fn project_3d_items_to_viewport(
    camera_q: Query<(&Camera, &GlobalTransform), With<FrontendMenuCamera>>,
    mut items_q: Query<(&Frontend3DAnchor, &mut Node, &mut TextFont)>,
    actors_q: Query<(&Name, &GlobalTransform), Without<FrontendMenuCamera>>,
) {
    let Ok((camera, cam_xform)) = camera_q.single() else {
        return;
    };
    for (anchor, mut node, mut font) in &mut items_q {
        // If ATTACHED_TO is set, resolve the actor's current world
        // transform and compose with the local-space corners.
        // Otherwise treat corners as world space.  The lookup is
        // linear today; Main_Menu has ~6 items and uitest has ~3
        // actors, so a HashMap isn't justified yet.
        let world_corners = if let Some(name) = anchor.attached_to.as_deref() {
            actors_q
                .iter()
                .find(|(actor_name, _)| actor_name.as_str() == name)
                .map(|(_, xform)| {
                    (
                        xform.transform_point(anchor.top_left),
                        xform.transform_point(anchor.bottom_right),
                    )
                })
        } else {
            Some((anchor.top_left, anchor.bottom_right))
        };

        let Some((tl_world, br_world)) = world_corners else {
            // Named actor not spawned yet (backdrop still chunking) —
            // park off-screen until it resolves.  Next frame's retry
            // picks it up once the chunked loader has advanced far
            // enough.
            node.left = Val::Px(-10_000.0);
            node.top = Val::Px(-10_000.0);
            continue;
        };

        let tl = camera.world_to_viewport(cam_xform, tl_world);
        let br = camera.world_to_viewport(cam_xform, br_world);
        match (tl, br) {
            (Ok(a), Ok(b)) => {
                let left = a.x.min(b.x);
                let top = a.y.min(b.y);
                let width = (a.x - b.x).abs().max(1.0);
                let height = (a.y - b.y).abs().max(1.0);
                node.left = Val::Px(left);
                node.top = Val::Px(top);
                node.width = Val::Px(width);
                node.height = Val::Px(height);
                node.align_items = AlignItems::Center;
                node.justify_content = JustifyContent::Center;
                font.font_size = (height * 0.7).clamp(12.0, 40.0);
            }
            _ => {
                node.left = Val::Px(-10_000.0);
                node.top = Val::Px(-10_000.0);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Input → events → handler-fire messages
// ---------------------------------------------------------------------------

pub fn input_dispatch_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    ui: Option<Res<FrontendUi>>,
    mut state: ResMut<FrontendState>,
    mut fire: MessageWriter<FireFrontendHandler>,
    mut text_q: Query<(&FrontendItemText, &mut TextColor)>,
    mut levels: ResMut<FrontendLevelList>,
    mut next_state: ResMut<NextState<AppState>>,
    mut commands: Commands,
    layout_ready: Option<Res<crate::oni2_loader::layout_loader::FrontendLayoutReady>>,
    pending_load: Option<Res<crate::oni2_loader::layout_loader::PendingLayoutLoad>>,
) {
    // Tick page timer.
    state.time_on_page += time.delta_secs();
    let time_on_page = state.time_on_page;

    let Some(ui_res) = ui else {
        return;
    };
    let Some(ref ui_file) = ui_res.0 else {
        return;
    };
    let Some(ref page_name) = state.current_page else {
        return;
    };
    let Some(page) = ui_file.pages.iter().find(|p| p.name == *page_name) else {
        return;
    };
    // For PAGE_3D pages that reference a LAYOUT, gate input until the
    // chunked backdrop load finalizes.  Legacy `TIME_TO_DISABLE_EVENTS`
    // is authored assuming the layout is mounted when the page begins
    // — our load is async, so we anchor the grace period to "backdrop
    // ready" rather than "page entered" and require both.
    let is_3d_with_layout = matches!(page.kind, PageKind::Page3D { .. });
    let backdrop_pending = is_3d_with_layout && (pending_load.is_some() || layout_ready.is_none());
    let events_armed = time_on_page >= page.time_to_disable_events && !backdrop_pending;

    // Fire ON_PAGE_START on the first tick of the new page, plus the
    // initially-focused item's ON_FOCUS_GAIN.  The focus-gain fire on
    // page entry is what makes items like Main_Menu's UI_MM_NewGame
    // run their SET_VISIBLE bookkeeping (hiding the dim `_Off`
    // variant, showing the active text) so the page lands in a
    // consistent state.
    //
    // Gated on `events_armed` so PAGE_3D pages with a LAYOUT defer
    // until the chunked backdrop load resolves — Main_Menu's
    // ON_PAGE_START SCRIPT handler targets `actor_Tunnel1`, which
    // doesn't exist until the load lands.  `page_just_changed` stays
    // true across the wait so we still fire exactly once, just later.
    if state.page_just_changed && events_armed {
        for item in &page.items {
            for ev in &item.events {
                if matches!(ev.kind, EventKind::PageStart) {
                    queue_handlers(&mut fire, &ev.handlers);
                }
            }
        }
        if let Some(focused_item) = page.items.get(state.focus_index) {
            for ev in &focused_item.events {
                if matches!(ev.kind, EventKind::FocusGain) {
                    queue_handlers(&mut fire, &ev.handlers);
                }
            }
        }
        state.page_just_changed = false;
    }

    // Handle focus change from the previous frame.  `focus_lost_last_frame`
    // holds the PREVIOUSLY focused index; current `state.focus_index` is
    // the new one.  Fire ON_FOCUS_LOSE on the old item and ON_FOCUS_GAIN
    // on the new one — both are common carriers for the authored
    // SET_VISIBLE bookkeeping that toggles bright/dim variants of the
    // same label on PAGE_3D menus.
    if state.focus_lost_last_frame.is_some() {
        let prev_focus = state.focus_lost_last_frame.take().unwrap();
        if let Some(lost_item) = page.items.get(prev_focus) {
            for ev in &lost_item.events {
                if matches!(ev.kind, EventKind::FocusLose) {
                    queue_handlers(&mut fire, &ev.handlers);
                }
            }
        }
        if let Some(gained_item) = page.items.get(state.focus_index) {
            for ev in &gained_item.events {
                if matches!(ev.kind, EventKind::FocusGain) {
                    queue_handlers(&mut fire, &ev.handlers);
                }
            }
        }
    }

    if !events_armed {
        return;
    }

    // Directional navigation.  Each arrow key fires the matching
    // ON_INPUT_* event on the current item; if that event has a
    // GO_TO_ITEM handler it'll redirect focus itself.  Otherwise the
    // default behavior (cycle vertically for up/down) kicks in.
    let pressed_up = keys.just_pressed(KeyCode::ArrowUp) || keys.just_pressed(KeyCode::KeyW);
    let pressed_down = keys.just_pressed(KeyCode::ArrowDown) || keys.just_pressed(KeyCode::KeyS);
    let pressed_left = keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::KeyA);
    let pressed_right = keys.just_pressed(KeyCode::ArrowRight) || keys.just_pressed(KeyCode::KeyD);
    let pressed_ok = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space);
    let pressed_cancel =
        keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace);
    let pressed_start = keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::Space)
        || keys.just_pressed(KeyCode::Escape);

    let mut handled_navigation = false;
    // Track whether the current item's authored events already own this
    // direction.  If they do, the GO_TO_ITEM handler will set focus
    // itself — the default linear fallback cycle below must NOT also
    // run, or one arrow press advances focus twice.
    let mut nav_owned_by_handler = false;
    // LevelList intercept: when the focused item is a level list,
    // arrow keys move the internal selection cursor and OK kicks off
    // the layout load.  We still let ON_INPUT_CANCEL fire through the
    // normal event pipeline so `Choose_Level`'s cancel → Main_Menu
    // handler runs.  The legacy C++ bound OK to SCRIPT
    // `$newgame:RunLoadedLevel`; since we don't have that script
    // backend, we short-circuit here: stash SelectedLayout + kick the
    // AppState transition into LoadingLayout directly.
    let focused_item_is_level_list = page
        .items
        .get(state.focus_index)
        .map(|i| {
            matches!(
                i.kind,
                ItemKind::LevelList(_) | ItemKind::LevelSavePointList(_)
            )
        })
        .unwrap_or(false);

    if focused_item_is_level_list {
        let n = levels.entries.len();
        if n > 0 {
            if pressed_up {
                levels.selected = (levels.selected + n - 1) % n;
                handled_navigation = true;
            }
            if pressed_down {
                levels.selected = (levels.selected + 1) % n;
                handled_navigation = true;
            }
            if pressed_ok {
                if let Some((folder, desc)) = levels.entries.get(levels.selected) {
                    info!("frontend: launching level '{}' ({})", folder, desc);
                    commands.insert_resource(SelectedLayout(folder.clone()));
                    next_state.set(AppState::LoadingLayout);
                }
            }
        }
        // Don't fall through to directional cycling — the LevelList
        // owns Up/Down for this focus context.  Cancel still flows.
        if pressed_cancel && let Some(current_item) = page.items.get(state.focus_index) {
            fire_event_on_item(&mut fire, current_item, EventKind::InputCancel);
        }
    } else if let Some(current_item) = page.items.get(state.focus_index) {
        if pressed_up {
            fire_event_on_item(&mut fire, current_item, EventKind::InputUp);
            handled_navigation = true;
            if item_has_event(current_item, EventKind::InputUp) {
                nav_owned_by_handler = true;
            }
        }
        if pressed_down {
            fire_event_on_item(&mut fire, current_item, EventKind::InputDown);
            handled_navigation = true;
            if item_has_event(current_item, EventKind::InputDown) {
                nav_owned_by_handler = true;
            }
        }
        if pressed_left {
            fire_event_on_item(&mut fire, current_item, EventKind::InputLeft);
            handled_navigation = true;
            if item_has_event(current_item, EventKind::InputLeft) {
                nav_owned_by_handler = true;
            }
        }
        if pressed_right {
            fire_event_on_item(&mut fire, current_item, EventKind::InputRight);
            handled_navigation = true;
            if item_has_event(current_item, EventKind::InputRight) {
                nav_owned_by_handler = true;
            }
        }
        if pressed_ok {
            fire_event_on_item(&mut fire, current_item, EventKind::InputOk);
        }
        if pressed_cancel {
            fire_event_on_item(&mut fire, current_item, EventKind::InputCancel);
        }
        if pressed_start {
            fire_event_on_item(&mut fire, current_item, EventKind::InputStart);
        }

        // ON_TIME_ELAPSED — check every frame, fire once when the
        // page timer crosses the delay.  Legacy sets the delay to
        // FLT_MAX after firing; we approximate by tracking via the
        // time_on_page field + a resource-level fired-flag map,
        // but for now we just fire every frame past the threshold
        // (the GO_TO_PAGE handlers are idempotent — repeat requests
        // to a page we're already on are no-ops).
        for ev in &current_item.events {
            if let EventKind::TimeElapsed { delay } = ev.kind
                && time_on_page >= delay
                && time_on_page - time.delta_secs() < delay
            {
                queue_handlers(&mut fire, &ev.handlers);
            }
        }
    }

    // Default navigation fallback: if arrow keys fired but no
    // GO_TO_ITEM handler (queued as fire messages) will redirect,
    // cycle focus linearly through visible items.  We don't have
    // visibility into "did a handler redirect" at this point, so we
    // just always do linear cycling — the handler executor's
    // SetCurrentItem wins if it fires in the same frame since the
    // render rebuild happens next frame anyway.  LevelList owns its
    // own Up/Down semantics (row selection) so skip the cycle there.
    let n = page.items.len();
    if n > 0 && handled_navigation && !focused_item_is_level_list && !nav_owned_by_handler {
        let mut new_focus = state.focus_index;
        if pressed_up || pressed_left {
            new_focus = (state.focus_index + n - 1) % n;
        } else if pressed_down || pressed_right {
            new_focus = (state.focus_index + 1) % n;
        }
        if new_focus != state.focus_index {
            state.focus_lost_last_frame = Some(state.focus_index);
            state.focus_index = new_focus;
            // Update text colors in place.
            let focus_color = Color::srgba(1.0, 0.85, 0.2, 1.0);
            let normal_color = Color::srgba(1.0, 1.0, 1.0, 0.75);
            for (marker, mut color) in &mut text_q {
                color.0 = if marker.item_index == new_focus {
                    focus_color
                } else {
                    normal_color
                };
            }
        }
    }
}

fn fire_event_on_item(
    fire: &mut MessageWriter<FireFrontendHandler>,
    item: &Item,
    which: EventKind,
) {
    for ev in &item.events {
        if std::mem::discriminant(&ev.kind) == std::mem::discriminant(&which) {
            queue_handlers(fire, &ev.handlers);
        }
    }
}

fn item_has_event(item: &Item, which: EventKind) -> bool {
    item.events.iter().any(|ev| {
        std::mem::discriminant(&ev.kind) == std::mem::discriminant(&which)
            && !ev.handlers.is_empty()
    })
}

fn queue_handlers(fire: &mut MessageWriter<FireFrontendHandler>, handlers: &[super::ast::Handler]) {
    for h in handlers {
        fire.write(FireFrontendHandler {
            kind: h.kind.clone(),
        });
    }
}

// ---------------------------------------------------------------------------
// Handler execution
// ---------------------------------------------------------------------------

pub fn handler_exec_system(
    mut reader: MessageReader<FireFrontendHandler>,
    mut page_req: MessageWriter<RequestPageChange>,
    mut state: ResMut<FrontendState>,
    ui: Option<ResMut<FrontendUi>>,
    actor_q: Query<(Entity, &Name)>,
    mut script_q: Query<&mut crate::scroni::vm::ScrOniScript>,
    mut anim_writer: MessageWriter<crate::animator::ControlAnimMessage>,
    // RunScript dispatches that landed before the chunked layout
    // loader's `commands.entity(actor).insert(ScrOniScript)` had
    // flushed.  Each entry is `(actor, script, frames_remaining)`;
    // we retry every frame while frames_remaining > 0 and clear on
    // success or expiry.  ~120 frames @ 60fps = 2 s — plenty for
    // any chunked layout to finish even on a slow disk.
    mut deferred_scripts: Local<Vec<(String, String, u32)>>,
) {
    let Some(mut ui_res) = ui else {
        return;
    };
    let Some(ref page_name) = state.current_page.clone() else {
        return;
    };
    // Cache the page index once so we can reborrow `ui_res` mutably
    // for SET_VISIBLE without holding an outstanding immutable
    // borrow on the AST.
    let page_idx = {
        let Some(ref ui_file) = ui_res.0 else {
            return;
        };
        match ui_file.pages.iter().position(|p| p.name == *page_name) {
            Some(i) => i,
            None => return,
        }
    };

    for msg in reader.read() {
        match &msg.kind {
            HandlerKind::GoToPage(name) => {
                page_req.write(RequestPageChange::Named(name.clone()));
            }
            HandlerKind::GoToPreviousPage => {
                page_req.write(RequestPageChange::Previous);
            }
            HandlerKind::GoToItem(name) => {
                let ui_file = ui_res.0.as_ref().unwrap();
                let page = &ui_file.pages[page_idx];
                if let Some(idx) = page.items.iter().position(|i| i.name == *name) {
                    if state.focus_index != idx {
                        state.focus_lost_last_frame = Some(state.focus_index);
                        state.focus_index = idx;
                    }
                } else {
                    warn!("frontend: GO_TO_ITEM target '{}' not found", name);
                }
            }
            HandlerKind::SetVisible { visible, item } => {
                // Mutates the cached AST's `is_visible` on either the
                // current item (when `item` is None — legacy default)
                // or a named sibling.  The rebuild gating on
                // `resource_changed::<FrontendUi>` picks up the
                // change and respawns the item tree on the next
                // frame.
                let current_focus = state.focus_index;
                let ui_file = ui_res.0.as_mut().unwrap();
                let page = &mut ui_file.pages[page_idx];
                let target_idx = match item {
                    Some(name) => page.items.iter().position(|i| i.name == *name),
                    None => Some(current_focus),
                };
                match target_idx {
                    Some(i) if i < page.items.len() => {
                        page.items[i].is_visible = *visible;
                    }
                    _ => {
                        warn!(
                            "frontend: SET_VISIBLE target {:?} not found on page '{}'",
                            item, page_name
                        );
                    }
                }
            }
            HandlerKind::PlaySound { name, .. } => {
                // Phase later: route to audio.  For now log so the
                // sound cues are visible.
                debug!("frontend PLAY_SOUND: {}", name);
            }
            HandlerKind::RunScript { actor, script } => {
                // Defer to the helper below.  `Retry` means the
                // actor or its `ScrOniScript` component isn't
                // available yet (chunked layout load still flushing);
                // queue for retry on the next frame.
                if let TryRunScriptResult::Retry =
                    try_run_script(actor, script, &actor_q, &mut script_q)
                {
                    deferred_scripts.push((actor.clone(), script.clone(), 120));
                }
            }
            HandlerKind::PlayAnimation {
                pause,
                loop_anim,
                actor,
                animation,
            } => {
                // Mirror legacy `uiItemEventHandlerAnimation::Execute`
                // — find the actor by name and dispatch a
                // `ControlAnimMessage` with RESTART (so the alias
                // re-plays from frame 0) plus LOOP/PAUSE bits as
                // authored.  The animator handles `animation_alias`
                // by calling `lib.play(alias, &mut state)` so a single
                // message both swaps and configures the playback.
                let Some((entity, _)) = actor_q.iter().find(|(_, n)| n.as_str() == actor.as_str())
                else {
                    warn!("frontend ANIMATION: actor '{}' not found", actor);
                    continue;
                };
                let mut control = crate::animator::events::control_anim_bits::RESTART
                    | crate::animator::events::control_anim_bits::LOOP;
                if *pause {
                    control |= crate::animator::events::control_anim_bits::PAUSE;
                }
                anim_writer.write(crate::animator::ControlAnimMessage {
                    entity,
                    animation_alias: Some(animation.clone()),
                    control,
                    rate: 1.0,
                    loop_anim: *loop_anim,
                    hold: false,
                    pause: *pause,
                });
                debug!(
                    "frontend ANIMATION: '{}' on '{}' (loop={}, pause={})",
                    animation, actor, loop_anim, pause
                );
            }
            HandlerKind::GameAbort | HandlerKind::GameReset => {
                warn!(
                    "frontend: {:?} handler — no game-lifecycle backend yet",
                    msg.kind
                );
            }
            // Conditional handlers: evaluate the condition against
            // state we can check; skip the branch otherwise.
            HandlerKind::IfDevFrontEnd { is_it, handlers } => {
                // We currently treat "dev front end" as always false —
                // release semantics.  If we add a --devfrontend flag
                // later this branch can read it.
                let actual = false;
                if actual == *is_it {
                    queue_handler_forward(&mut page_req, handlers);
                }
            }
            // Everything else: stub + log.  Phase 3 doesn't need
            // full game-lifecycle, memory-card, or volume handlers.
            other => {
                debug!("frontend: unhandled handler kind {:?}", other);
            }
        }
    }

    // Retry deferred RunScript dispatches.  These accumulate when the
    // SCRIPT handler fires before the chunked layout loader's
    // `insert(ScrOniScript)` command has flushed onto the target
    // actor — most commonly Main_Menu's
    // `SCRIPT "actor_Tunnel1" "$newgame:CameraMotion"` racing the
    // uitest backdrop spawn.  Decrement each entry; drop on success
    // or expiry.
    if !deferred_scripts.is_empty() {
        let mut still_pending = Vec::new();
        for (actor, script, frames_remaining) in deferred_scripts.drain(..) {
            match try_run_script(&actor, &script, &actor_q, &mut script_q) {
                TryRunScriptResult::Done => continue,
                TryRunScriptResult::Retry => {
                    if frames_remaining > 1 {
                        still_pending.push((actor, script, frames_remaining - 1));
                    } else {
                        // Final failure — dump what we DID find so we
                        // can tell whether the actor name was wrong
                        // (no entity), or the layout-loader's
                        // ScrOniScript insert never landed (entity
                        // exists but no component), or some other
                        // race.
                        let actor_match_count =
                            actor_q.iter().filter(|(_, n)| n.as_str() == actor).count();
                        let total_scripts = script_q.iter().count();
                        let names_with_script: Vec<String> = actor_q
                            .iter()
                            .filter(|(e, _)| script_q.get(*e).is_ok())
                            .map(|(_, n)| n.as_str().to_string())
                            .collect();
                        warn!(
                            "frontend SCRIPT: gave up '{}' on '{}' after retry window. \
                             entities-by-name={} scripts-in-world={} named-with-scripts={:?}",
                            script, actor, actor_match_count, total_scripts, names_with_script
                        );
                    }
                }
            }
        }
        *deferred_scripts = still_pending;
    }
}

/// Result of one `try_run_script` attempt.  `Done` means the caller
/// should drop the entry (success OR a permanent failure logged
/// inside).  `Retry` means the caller should re-queue and try again
/// next frame (transient — actor or component not yet ready).
enum TryRunScriptResult {
    Done,
    Retry,
}

/// Try to dispatch `SCRIPT "<actor>" "<script>"` once.  Returns
/// `Retry` while the chunked layout load is still in flight; `Done`
/// once we've either swapped the script or determined the request is
/// unreachable.
fn try_run_script(
    actor: &str,
    script: &str,
    actor_q: &Query<(Entity, &Name)>,
    script_q: &mut Query<&mut crate::scroni::vm::ScrOniScript>,
) -> TryRunScriptResult {
    // Script names from .ui authors come in as `$file:scriptname` (or
    // sometimes bare); the file-prefix is informational since the
    // layout loader merges every script from the file into
    // `available_scripts` at spawn time.  Key on the bare script name
    // (post-colon, with `$` stripped).
    let bare = script
        .rsplit_once(':')
        .map(|(_, s)| s)
        .unwrap_or(script)
        .trim_start_matches('$');
    let matching: Vec<Entity> = actor_q
        .iter()
        .filter(|(_, n)| n.as_str() == actor)
        .map(|(e, _)| e)
        .collect();
    let Some(&entity) = matching.first() else {
        return TryRunScriptResult::Retry;
    };
    let Ok(mut comp) = script_q.get_mut(entity) else {
        return TryRunScriptResult::Retry;
    };
    let Some(script_def) = comp.exec.available_scripts.get(bare).cloned() else {
        // Script genuinely not in the file — no amount of retries
        // helps.  Dump the available list so it's clear what got loaded.
        let names: Vec<&str> = comp
            .exec
            .available_scripts
            .keys()
            .map(|k| k.as_str())
            .collect();
        warn!(
            "frontend SCRIPT: '{}' not in available_scripts on '{}' — giving up. Available: {:?}",
            bare, actor, names
        );
        return TryRunScriptResult::Done;
    };
    comp.exec.main_thread = crate::scroni::vm::ScrOniThread::new(0, None, script_def);
    comp.exec.child_threads.clear();
    comp.exec.next_thread_id = 1;
    comp.exec.message_queue.clear();
    comp.exec.outgoing_messages.clear();
    comp.exec.sys_requests.clear();
    comp.exec.active = true;
    comp.exec.ticks_alive = 0;
    info!(
        "frontend SCRIPT: '{}' running on {} ({} matching entities)",
        bare,
        actor,
        matching.len()
    );
    TryRunScriptResult::Done
}

fn queue_handler_forward(
    page_req: &mut MessageWriter<RequestPageChange>,
    handlers: &[super::ast::Handler],
) {
    // Only forwards the subset we handle synchronously — namely
    // GO_TO_PAGE / GO_TO_PREVIOUS_PAGE / GO_TO_ITEM.  The rest would
    // need another trip through the message queue, which is fine in
    // practice since IF_* bodies in shipped files only hold nav.
    for h in handlers {
        match &h.kind {
            HandlerKind::GoToPage(name) => {
                page_req.write(RequestPageChange::Named(name.clone()));
            }
            HandlerKind::GoToPreviousPage => {
                page_req.write(RequestPageChange::Previous);
            }
            other => debug!("frontend: nested {:?} skipped (phase 3)", other),
        }
    }
}

// ---------------------------------------------------------------------------
// PAGE_MOVIE auto-advance stub
// ---------------------------------------------------------------------------
//
// FUTURE: real PAGE_MOVIE playback (phase 4 revisit).
//
// Why we ended up here: the shipped `.m2v` files under
// `oni2/zips/assets/movies/` (rockstarlogo.m2v, angel.m2v) are
// **interlaced** MPEG-2 Main Profile — the PS2 composite-era encode.
// We evaluated `oxideav-mpeg12video` (the only pure-Rust MPEG-2
// decoder on crates.io); it's progressive-only by design and rejects
// interlaced streams with `Error::Unsupported` rather than silently
// mis-decoding.  The paired `.imf` sidecars are a custom Angel
// Studios IMA-ADPCM audio format — no off-the-shelf decoder exists.
// The legacy C++ `rbMoviePlayer` was itself entirely stubbed out
// (every `gfxMPEG::*` call commented out in `rbMoviePlayer`).
// Asset-pipeline preprocessing is off the table because users bring
// their own RB.DAT; we never own or redistribute these files.
//
// The chosen design is a runtime shell-out to `ffmpeg.exe`, opt-in
// per user machine.  Sketch:
//
//   1. Startup probe: `which ffmpeg` + check alongside the binary.
//      Cache `Option<PathBuf>` in a `FfmpegBin` resource.
//   2. On entering a PAGE_MOVIE page: if `FfmpegBin` is Some, spawn
//        ffmpeg -hide_banner -loglevel error \
//               -i <movies/<name>> \
//               -vf yadif           (deinterlace)
//               -f rawvideo -pix_fmt yuv420p -
//      Keep the Child + its stdout on a `MoviePlayback` resource.
//      Read the movie's duration/framerate from `ffprobe` or parse
//      ffmpeg's stderr for `Stream #0:0 ... 640x448, 30 fps`.
//   3. Per-frame: read exactly `w*h*3/2` bytes from the stdout pipe
//      (one YUV420P frame).  A background thread + bounded channel
//      avoids blocking Bevy's Update.  Convert to RGB (same loop as
//      the removed `mpeg2_smoke.rs` test) and upload into a
//      pre-allocated `Image` handle that the PAGE_MOVIE background
//      `ImageNode` points at.
//   4. EOF on stdout → fire `ON_MOVIE_COMPLETE`.  OK/Start pressed
//      → kill the Child, fire `ON_MOVIE_COMPLETE` early (legacy
//      skip behavior).
//   5. IMF audio: separate problem.  The .imf format is Angel
//      Studios proprietary IMA-ADPCM; likely need to hand-roll the
//      decode (the format is documented in community PS2 reversing
//      notes).  Or punt on audio for PAGE_MOVIE entirely — the
//      legacy code never finished audio sync either.
//   6. Absent ffmpeg: keep the current stub behavior — label +
//      TIME_TO_DISABLE_EVENTS-gated ON_MOVIE_COMPLETE fire.  The
//      game boots, the page chain still walks from Rockstar →
//      Angel → Oni2_Logo → Main Menu with no visible intros.  This
//      is the BYO-tools equivalent of BYO-assets and fits the
//      project's overall model.
//
// Alternative we deliberately rejected: forking oxideav-mpeg12video
// to add interlaced + field pictures + alternate scan order +
// dual-prime MVs.  ~1-2 weeks of ISO 13818-2 spec work for a
// maintainer who has explicitly scoped those out.  Not worth it for
// eight seconds of logo playback.

/// For PAGE_MOVIE pages, fire the ON_MOVIE_COMPLETE event on the
/// current item after a short timer — approximates legacy movie
/// playback ending.  Phase 4 replaces this with a real decoder.
pub fn stub_autoadvance_system(
    ui: Option<Res<FrontendUi>>,
    state: Res<FrontendState>,
    mut fire: MessageWriter<FireFrontendHandler>,
) {
    let Some(ui_res) = ui else {
        return;
    };
    let Some(ref ui_file) = ui_res.0 else {
        return;
    };
    let Some(ref page_name) = state.current_page else {
        return;
    };
    let Some(page) = ui_file.pages.iter().find(|p| p.name == *page_name) else {
        return;
    };

    // Only auto-advance PAGE_MOVIE.
    if !matches!(page.kind, PageKind::PageMovie { .. }) {
        return;
    }

    // Legacy `TIME_TO_DISABLE_EVENTS` often serves as the minimum
    // wait on intro movies (3-4 seconds for Rockstar/Angel).  Fire
    // the completion event exactly once when we cross that
    // threshold.
    let threshold = page.time_to_disable_events.max(0.5);
    let prev = state.time_on_page - 1e-4; // close-enough edge test
    if state.time_on_page >= threshold && prev < threshold {
        for item in &page.items {
            for ev in &item.events {
                if matches!(ev.kind, EventKind::MovieComplete) {
                    for h in &ev.handlers {
                        fire.write(FireFrontendHandler {
                            kind: h.kind.clone(),
                        });
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Teardown
// ---------------------------------------------------------------------------

pub fn teardown_frontend(
    mut commands: Commands,
    mut state: ResMut<FrontendState>,
    mut root: ResMut<FrontendRoot>,
    query: Query<Entity, Or<(With<FrontendUiEntity>, With<FrontendBackdrop>)>>,
) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
    // Re-arm the bootstrap: the Update-scheduled `route_to_startup_page`
    // fires only when `current_page` is None.  Clearing here lets us
    // re-enter FrontEnd (e.g. GO_TO_PAGE on GameComplete) and have
    // the page graph walked again.
    state.current_page = None;
    state.page_history.clear();
    state.focus_index = 0;
    state.focus_lost_last_frame = None;
    state.time_on_page = 0.0;
    state.page_just_changed = false;
    root.0 = None;
    // Drop any frontend-scope backdrop bookkeeping so a re-entry to
    // FrontEnd kicks a fresh chunked load from scratch.
    commands.remove_resource::<crate::oni2_loader::layout_loader::FrontendLayoutReady>();
    commands.remove_resource::<crate::oni2_loader::layout_loader::PendingLayoutLoad>();
}
