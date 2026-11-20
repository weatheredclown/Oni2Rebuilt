/*
 * ui_overlay.rs — rb's in-game UI overlay: ScrOni `uioverlay` / `uitext` /
 * `uivisible` and the PAGE_OVERLAY pages in `layout/<Name>/layout.ui`.
 *
 * C++ reference: rb/src/rbfrontend/rbmanager.cpp (rbUIManager::ShowOverlay,
 * SetOverlayItemText/Visible, LoadLayoutUI), rbitem.cpp (uiItemRect2DText,
 * uiItemRect3D[Text] draw, uiLiteralText), age/src/text/fonttex.cpp
 * (txtFontTex), docs/designers_guide/02 §2.8.10.
 *
 * - `uioverlay "<page>"` loads the layout's layout.ui the first time, then
 *   shows that PAGE_OVERLAY (`""` hides it).  Item text / visibility live on
 *   the page, so they survive hide + show, as the C++ page objects do.
 * - `uitext` / `uivisible` act on the showing page; before `uioverlay` they
 *   warn and do nothing (C++ behaviour the designers rely on).
 * - The overlay belongs to the layout that showed it: a layout change or
 *   leaving the game drops it.
 *
 * Text is rasterised on the CPU from the `.fonttex` glyph pages into an RGBA
 * image (C++ does the same for 3D text: RenderStringIntoTexture), modulated
 * by the item's COLOR at 1x.  Then:
 * - ITEM_RECT_2D_TEXT: a Bevy UI image on the 512x448 canvas, text centred
 *   and word-wrapped to WIDTH (txtCursor SetCentered + SetWrapped).
 * - ITEM_RECT_3D_TEXT billboard: a UI image placed each frame by projecting
 *   the camera-facing rectangle round the anchor; drawn over the scene.
 * - ITEM_RECT_3D_TEXT WORLD: a real unlit, alpha-blended, fogged quad in the
 *   anchor's frame.  Unlike C++ (which draws after compositing and hides a
 *   sign whole when a probe to its centre is blocked) it is depth tested
 *   per pixel, which is what that probe approximates.
 * - NEAR <m>: drawn only while the player is within m of the anchor.
 * - ITEM_RECT_2D (dialog portraits, designers_guide ch. 27): a textured (or
 *   flat-colour) rectangle on the canvas, with rb's additions
 *   `uitexture` / `uimove` / `uicolor`, `MIRROR` and `CROSSFADE <secs>`
 *   (rbitem.cpp uiItemRect2D::MoveTo / FadeTo / Animate / ChangeTexture,
 *   rbmanager.cpp SetOverlayItemTexture).  Moves ease out (cubic), colours
 *   blend linearly, both on game time; a cross-fade draws the old picture
 *   as it was with the new one rising over it.
 */
use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::camera::components::CameraController;
use crate::frontend::ast::{ItemKind, Page, PageKind, Rect2DProps, Rect3DProps, StringSource};
use crate::frontend::render::{UI_CANVAS_H, UI_CANVAS_W};
use crate::menu::{AppState, InGameEntity, SelectedLayout};
use crate::player::components::Player;
use crate::scroni::vm::ScrOniSysEvent;

pub struct UiOverlayPlugin;

impl Plugin for UiOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OverlayState>()
            .add_observer(on_scroni_ui_event)
            .add_systems(
                Update,
                (
                    drop_overlay_on_layout_change,
                    sync_overlay_items,
                    place_overlay_items,
                    animate_overlay_rects,
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnExit(AppState::InGame), reset_overlay)
            .add_systems(Update, timed_screenshots);
    }
}

/// Dev aid for checking overlays (and anything else) without a visible
/// display: `RB_SHOT="6:/tmp/a.png,12:/tmp/b.png"` saves the primary window
/// at those seconds after startup, read back from the render target.
fn timed_screenshots(
    mut commands: Commands,
    time: Res<Time>,
    mut pending: Local<Option<Vec<(f32, String)>>>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let list = pending.get_or_insert_with(|| {
        std::env::var("RB_SHOT")
            .unwrap_or_default()
            .split(',')
            .filter_map(|e| {
                let (t, p) = e.split_once(':')?;
                Some((t.trim().parse().ok()?, p.trim().to_string()))
            })
            .collect()
    });
    let now = time.elapsed_secs();
    list.retain(|(t, path)| {
        if now < *t {
            return true;
        }
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
        false
    });
}

// ---------------------------------------------------------------------------
// .fonttex (age/src/text/fonttex.cpp txtFontTex::Load)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Glyph {
    x: u8,
    y: u8,
    w: u8,
    h: u8,
    y_off: i8,
    x_off: i8,
    page: usize,
}

struct FontTex {
    height: i32,
    spacing: i32,
    space_width: i32,
    space_char: u32,
    baseline: i32,
    glyphs: HashMap<u32, Glyph>,
    /// Decoded glyph pages: (width, height, rgba8).
    pages: Vec<(u32, u32, Vec<u8>)>,
}

impl FontTex {
    fn load(name: &str) -> Option<FontTex> {
        Self::load_with(name, |dir, file| crate::vfs::read(dir, file).ok())
    }

    /// `read(dir, file)` -> bytes; the VFS in game, plain files in tests.
    fn load_with(name: &str, read: impl Fn(&str, &str) -> Option<Vec<u8>>) -> Option<FontTex> {
        let data = read("fonts", &format!("{name}.fonttex"))?;
        let u32_at = |o: usize| -> Option<u32> {
            data.get(o..o + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let version = f32::from_bits(u32_at(0)?);
        if !(1.0..=1.1).contains(&version) {
            warn!("[UI] font '{name}': unexpected .fonttex version {version}");
            return None;
        }
        let hdr: Vec<u32> = (0..10).map(|i| u32_at(4 + i * 4).unwrap_or(0)).collect();
        let num_glyphs = u32_at(44)? as usize;
        if num_glyphs == 0 || num_glyphs > 4096 {
            return None;
        }
        let mut list = Vec::with_capacity(num_glyphs);
        let mut o = 48;
        for _ in 0..num_glyphs {
            let b = data.get(o..o + 10)?;
            let cp = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            list.push((
                cp,
                Glyph {
                    x: b[4],
                    y: b[5],
                    w: b[6],
                    h: b[7],
                    y_off: b[8] as i8,
                    x_off: b[9] as i8,
                    page: 0,
                },
            ));
            o += 10;
        }
        let num_pages = u32_at(o)? as usize;
        if num_pages == 0 || num_pages > 64 {
            return None;
        }
        o += 4;
        let mut start = 0usize;
        let mut pages = Vec::with_capacity(num_pages);
        for p in 0..num_pages {
            let end = u32_at(o + p * 4)? as usize;
            for g in list.iter_mut().take(end.min(num_glyphs)).skip(start) {
                g.1.page = p;
            }
            start = end;
            // Shipped fonts and tools/fonts/make_fonttex.py both keep the
            // pages beside the .fonttex as fonts/<name>_NN.tex.
            let page_name = format!("{name}_{p:02}.tex");
            let decoded = read("fonts", &page_name)
                .or_else(|| read("fonts/texture", &page_name))
                .and_then(|b| crate::oni2_loader::parsers::texture::decode_tex(&b));
            match decoded {
                Some((w, h, rgba, _)) => pages.push((w, h, rgba)),
                None => {
                    warn!("[UI] font '{name}': missing page texture {page_name}");
                    pages.push((0, 0, Vec::new()));
                }
            }
        }
        let mut font = FontTex {
            height: hdr[0] as i32,
            spacing: hdr[1] as i32,
            space_width: hdr[2] as i32,
            space_char: hdr[3],
            baseline: hdr[4] as i32,
            glyphs: list.into_iter().collect(),
            pages,
        };
        // A font without a baseline falls back to its tallest glyph's ascent.
        if font.baseline <= 0 {
            font.baseline = font
                .glyphs
                .values()
                .map(|g| -(g.y_off as i32))
                .max()
                .unwrap_or(0);
        }
        Some(font)
    }

    fn glyph(&self, ch: u32) -> Option<&Glyph> {
        self.glyphs.get(&ch).or_else(|| {
            // Case fallback for caps-only fonts.
            if (b'a' as u32..=b'z' as u32).contains(&ch) {
                self.glyphs.get(&(ch - 32))
            } else {
                None
            }
        })
    }

    fn advance(&self, ch: u32) -> i32 {
        if ch == self.space_char {
            return self.space_width;
        }
        match self.glyph(ch) {
            Some(g) => g.w as i32 + g.x_off as i32 + self.spacing,
            None => self.space_width,
        }
    }

    fn line_width(&self, line: &[u32]) -> i32 {
        line.iter().map(|&c| self.advance(c)).sum()
    }

    /// Width a centred line is placed by: the right edge of the last glyph
    /// box, not the summed advances (txtFontTex::JustifyWidth).
    fn justify_width(&self, line: &[u32]) -> i32 {
        let mut n = line.len();
        while n > 0 && line[n - 1] == self.space_char {
            n -= 1;
        }
        if n == 0 {
            return 0;
        }
        match self.glyph(line[n - 1]) {
            Some(g) => self.line_width(&line[..n - 1]) + g.w as i32,
            None => self.line_width(&line[..n]),
        }
    }

    /// Chars of `text` that fit in `max_width`, breaking at a space where
    /// possible; at least one (txtFontTex::BreakLine).
    fn break_line(&self, text: &[u32], max_width: i32) -> usize {
        let (mut w, mut last_space) = (0, None);
        let mut i = 0;
        while i < text.len() && text[i] != '\n' as u32 {
            if text[i] == self.space_char {
                last_space = Some(i);
            }
            w += self.advance(text[i]);
            if w > max_width && i > 0 {
                return match last_space {
                    Some(s) if s > 0 => s,
                    _ => i,
                };
            }
            i += 1;
        }
        i
    }

    /// Split into lines the way txtFontTex::DrawInternal walks them.
    fn lines(&self, text: &str, wrap_width: Option<i32>) -> Vec<Vec<u32>> {
        // uiLiteralText widens each byte (Latin-1); chars map 1:1 here.
        let chars: Vec<u32> = text.chars().map(|c| c as u32).collect();
        let mut out = Vec::new();
        let mut p = 0;
        while p < chars.len() {
            let rest = &chars[p..];
            let count = match wrap_width {
                Some(w) if w > 0 => self.break_line(rest, w),
                _ => rest
                    .iter()
                    .position(|&c| c == '\n' as u32)
                    .unwrap_or(rest.len()),
            };
            out.push(rest[..count].to_vec());
            p += count;
            while p < chars.len() && chars[p] == self.space_char {
                p += 1; // swallow the break point
            }
            if p < chars.len() && chars[p] == '\n' as u32 {
                p += 1;
            }
        }
        out
    }

    /// Rasterise into an RGBA8 image `width` wide (or the widest line when
    /// `None`), lines centred when `centred`, colour-modulated.
    fn rasterize(
        &self,
        text: &str,
        box_width: Option<i32>,
        color: [u8; 4],
    ) -> Option<(u32, u32, Vec<u8>)> {
        let lines = self.lines(text, box_width);
        if lines.is_empty() || self.height <= 0 {
            return None;
        }
        let widest = lines
            .iter()
            .map(|l| self.justify_width(l))
            .max()
            .unwrap_or(0);
        let w = box_width.unwrap_or(widest).max(widest).max(1) as u32;
        let h = (lines.len() as i32 * self.height).max(1) as u32;
        let mut img = vec![0u8; (w * h * 4) as usize];
        let col = color.map(|c| c as u32);
        for (li, line) in lines.iter().enumerate() {
            let mut pen = ((w as i32 - self.justify_width(line)) / 2).max(0);
            let line_top = li as i32 * self.height;
            for &ch in line {
                let adv = self.advance(ch);
                if ch != self.space_char
                    && let Some(g) = self.glyph(ch)
                    && let Some((pw, ph, page)) = self.pages.get(g.page)
                {
                    let gy = line_top + self.baseline + g.y_off as i32;
                    for yy in 0..g.h as i32 {
                        for xx in 0..g.w as i32 {
                            let (sx, sy) = (g.x as i32 + xx, g.y as i32 + yy);
                            let (dx, dy) = (pen + xx, gy + yy);
                            if sx >= *pw as i32
                                || sy >= *ph as i32
                                || dx < 0
                                || dy < 0
                                || dx >= w as i32
                                || dy >= h as i32
                            {
                                continue;
                            }
                            // decode_tex returns the file's rows bottom-up (for
                            // model UVs); glyph x/y index the file's own rows.
                            let row = *ph as i32 - 1 - sy;
                            let s = ((row as u32 * pw + sx as u32) * 4) as usize;
                            let d = ((dy as u32 * w + dx as u32) * 4) as usize;
                            let a = page[s + 3] as u32 * col[3] / 255;
                            if a == 0 {
                                continue;
                            }
                            // Glyphs may overlap (negative spacing): keep the max.
                            if a >= img[d + 3] as u32 {
                                for c in 0..3 {
                                    img[d + c] = (page[s + c] as u32 * col[c] / 255) as u8;
                                }
                                img[d + 3] = a as u8;
                            }
                        }
                    }
                }
                pen += adv;
            }
        }
        Some((w, h, img))
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum OverlayKind {
    Text2D(Rect2DProps),
    Text3D(Rect3DProps),
    /// ITEM_RECT_2D: a picture (or a flat colour) on the canvas.
    Rect2D(Rect2DProps),
}

/// The animated state of an ITEM_RECT_2D (uiItemRect2D's TopLeft / Color /
/// Texture and its tweens).  Times are `Time::elapsed_secs` (game time, as
/// C++ TIME.GetElapsedTime).
struct RectState {
    pos: Vec2,
    color: [f32; 4],
    texture: Option<Handle<Image>>,
    /// The picture fading out under `texture` (CROSSFADE) and when it began.
    old_texture: Option<(Handle<Image>, f32)>,
    /// (from, to, start, secs)
    moving: Option<(Vec2, Vec2, f32, f32)>,
    fading: Option<([f32; 4], [f32; 4], f32, f32)>,
    /// The fading-out picture's node, drawn under `entity`.
    under: Option<Entity>,
}

impl RectState {
    fn new(r: &Rect2DProps) -> Self {
        RectState {
            pos: Vec2::new(r.top_left.0 as f32, r.top_left.1 as f32),
            color: r.color.map(f32::from),
            texture: None,
            old_texture: None,
            moving: None,
            fading: None,
            under: None,
        }
    }

    /// uiItemRect2D::Animate: advance the tweens to `now`.
    fn animate(&mut self, now: f32) {
        if let Some((from, to, t0, secs)) = self.moving {
            let k = ease_out(((now - t0) / secs).clamp(0.0, 1.0));
            // C++ rounds to whole canvas units.
            self.pos = (from + (to - from) * k).round();
            if k >= 1.0 {
                self.moving = None;
            }
        }
        if let Some((from, to, t0, secs)) = self.fading {
            let k = ((now - t0) / secs).clamp(0.0, 1.0);
            for i in 0..4 {
                self.color[i] = (from[i] + (to[i] - from[i]) * k).round();
            }
            if k >= 1.0 {
                self.fading = None;
            }
        }
    }

    fn move_to(&mut self, now: f32, to: Vec2, secs: f32) {
        self.animate(now); // from wherever a running move has got to
        if secs <= 0.0 {
            self.pos = to;
            self.moving = None;
        } else {
            self.moving = Some((self.pos, to, now, secs));
        }
    }

    fn fade_to(&mut self, now: f32, to: [f32; 4], secs: f32) {
        self.animate(now);
        if secs <= 0.0 {
            self.color = to;
            self.fading = None;
        } else {
            self.fading = Some((self.color, to, now, secs));
        }
    }

    /// uiItemRect2D::ChangeTexture: with CROSSFADE, a change between two
    /// pictures keeps the old one fading out underneath; to or from none cuts.
    fn change_texture(&mut self, now: f32, t: Option<Handle<Image>>, crossfade: f32) {
        self.old_texture = None; // a fade still running ends now
        if crossfade > 0.0
            && let (Some(old), Some(new)) = (self.texture.as_ref(), t.as_ref())
            && old != new
        {
            self.old_texture = Some((old.clone(), now));
        }
        self.texture = t;
    }
}

/// Ease-out cubic: Oni's cinematic inserts fly in and land in their slot.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

struct OverlayItem {
    name: String,
    kind: OverlayKind,
    text: String,
    font: String,
    visible: bool,
    /// Text / visibility changed: rebuild the image (and entity).
    dirty: bool,
    entity: Option<Entity>,
    /// Bevy material for WORLD quads, so a text change only swaps its texture.
    material: Option<Handle<StandardMaterial>>,
    image: Option<Handle<Image>>,
    /// ITEM_RECT_2D only.
    rect: Option<RectState>,
}

#[derive(Resource, Default)]
pub struct OverlayState {
    /// Layout whose layout.ui has been read (C++ LayoutUILoaded).
    loaded_layout: Option<String>,
    /// PAGE_OVERLAY pages of that layout.
    pages: Vec<Page>,
    /// Per-page runtime item state, kept across hide / show.
    page_items: HashMap<String, Vec<OverlayItem>>,
    /// Showing page (lowercased key into `page_items`) and its layout.
    active: Option<(String, String)>,
    fonts: HashMap<String, Option<Arc<FontTex>>>,
    /// Window aspect relative to the 512x448 canvas, last used for 2D text.
    aspect: f32,
    /// UI textures by lower-cased name; misses are kept so a missing mood
    /// isn't searched for on disk again (beadmachinerpg dlUi::Texture).
    textures: HashMap<String, Option<Handle<Image>>>,
}

/// gfxGetTexture for UI pictures: `texture/<name>.tex`, then `.tga`.
fn load_ui_texture(
    cache: &mut HashMap<String, Option<Handle<Image>>>,
    images: &mut Assets<Image>,
    name: &str,
) -> Option<Handle<Image>> {
    cache
        .entry(name.to_ascii_lowercase())
        .or_insert_with(|| {
            use crate::oni2_loader::parsers::texture::{load_texture, load_tga_file};
            load_texture("texture", name, images)
                .or_else(|| load_tga_file("texture", &format!("{name}.tga"), images))
                .map(|(h, _)| h)
        })
        .clone()
}

impl OverlayState {
    fn font(&mut self, name: &str) -> Option<Arc<FontTex>> {
        let key = name.to_ascii_lowercase();
        self.fonts
            .entry(key)
            .or_insert_with(|| {
                let f = FontTex::load(name).map(Arc::new);
                if f.is_none() {
                    warn!("[UI] overlay font '{name}' not found in fonts/");
                }
                f
            })
            .clone()
    }

    fn active_items(&mut self) -> Option<&mut Vec<OverlayItem>> {
        let (key, _) = self.active.as_ref()?;
        self.page_items.get_mut(key)
    }

    fn load_layout_ui(&mut self, layout: &str) {
        if self.loaded_layout.as_deref() == Some(layout) {
            return;
        }
        self.loaded_layout = Some(layout.to_string());
        self.pages.clear();
        self.page_items.clear();
        let dir = format!("layout/{layout}");
        if !crate::vfs::exists(&dir, "layout.ui") {
            return;
        }
        let Ok(src) = crate::vfs::read_to_string(&dir, "layout.ui") else {
            return;
        };
        match crate::frontend::parse_ui(&src) {
            Ok(ui) => {
                self.pages = ui
                    .pages
                    .into_iter()
                    .filter(|p| matches!(p.kind, PageKind::PageOverlay))
                    .collect();
                info!(
                    "[UI] loaded {} overlay page(s) from {dir}/layout.ui",
                    self.pages.len()
                );
            }
            Err(e) => error!("[UI] {dir}/layout.ui: {e}"),
        }
    }
}

/// Without FONT the text uses the front end's menu-string font; Oni 2's
/// string table gives feTitle for those (uiLiteralText::SetFont fallback).
const DEFAULT_FONT: &str = "feTitle";

fn build_items(page: &Page) -> Vec<OverlayItem> {
    page.items
        .iter()
        .filter_map(|item| {
            let (kind, string) = match &item.kind {
                ItemKind::Rect2DText { rect, string } => (OverlayKind::Text2D(rect.clone()), string),
                ItemKind::Rect3DText { rect, string } => (OverlayKind::Text3D(rect.clone()), string),
                ItemKind::Rect2D(rect) => {
                    return Some(OverlayItem {
                        name: item.name.clone(),
                        kind: OverlayKind::Rect2D(rect.clone()),
                        text: String::new(),
                        font: String::new(),
                        visible: item.is_visible,
                        dirty: true,
                        entity: None,
                        material: None,
                        image: None,
                        rect: Some(RectState::new(rect)),
                    });
                }
                _ => {
                    warn!(
                        "[UI] overlay page '{}': item '{}' ({}) is not drawn (only text and 2D rect items are)",
                        page.name, item.name, item.class_name
                    );
                    return None;
                }
            };
            let (text, font) = match string {
                StringSource::Literal { text, font } => (text.clone(), font.clone()),
                // No string table in the port yet: show the ID.
                StringSource::StringId(id) => (id.clone(), None),
                StringSource::LevelName => (String::new(), None),
            };
            Some(OverlayItem {
                name: item.name.clone(),
                kind,
                text,
                font: font.unwrap_or_else(|| DEFAULT_FONT.to_string()),
                visible: item.is_visible,
                dirty: true,
                entity: None,
                material: None,
                image: None,
                rect: None,
            })
        })
        .collect()
}

fn despawn_items(commands: &mut Commands, items: &mut [OverlayItem]) {
    for it in items {
        if let Some(e) = it.entity.take() {
            commands.entity(e).try_despawn();
        }
        if let Some(e) = it.rect.as_mut().and_then(|r| r.under.take()) {
            commands.entity(e).try_despawn();
        }
        it.material = None;
        it.image = None;
        it.dirty = true;
    }
}

// ---------------------------------------------------------------------------
// ScrOni commands
// ---------------------------------------------------------------------------

fn on_scroni_ui_event(
    trigger: On<ScrOniSysEvent>,
    mut state: ResMut<OverlayState>,
    layout: Option<Res<SelectedLayout>>,
    time: Res<Time>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    let layout_name = layout.map(|l| l.0.clone()).unwrap_or_default();
    match trigger.event() {
        ScrOniSysEvent::UiOverlay(page) => {
            if page.is_empty() {
                if let Some(items) = state.active_items() {
                    despawn_items(&mut commands, items);
                }
                state.active = None;
                return;
            }
            state.load_layout_ui(&layout_name);
            let key = page.to_ascii_lowercase();
            if state.active.as_ref().map(|a| &a.0) == Some(&key) {
                return; // already showing
            }
            let Some(p) = state
                .pages
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(page))
                .cloned()
            else {
                warn!("uioverlay: no PAGE_OVERLAY '{page}' (in layout/{layout_name}/layout.ui)");
                return;
            };
            if let Some(items) = state.active_items() {
                despawn_items(&mut commands, items);
            }
            state
                .page_items
                .entry(key.clone())
                .or_insert_with(|| build_items(&p));
            state.active = Some((key, layout_name));
        }
        ScrOniSysEvent::UiText(item, text) => {
            let showing = state.active.is_some();
            match state
                .active_items()
                .and_then(|v| v.iter_mut().find(|i| i.name.eq_ignore_ascii_case(item)))
            {
                Some(it) => {
                    if it.text != *text {
                        it.text = text.clone();
                        it.dirty = true;
                    }
                }
                None => warn!(
                    "uitext/uivisible: no item '{item}' on the overlay page{}",
                    if showing {
                        ""
                    } else {
                        " (no overlay is showing)"
                    }
                ),
            }
        }
        ScrOniSysEvent::UiVisible(item, visible) => {
            let showing = state.active.is_some();
            match state
                .active_items()
                .and_then(|v| v.iter_mut().find(|i| i.name.eq_ignore_ascii_case(item)))
            {
                Some(it) => {
                    if it.visible != *visible {
                        it.visible = *visible;
                        it.dirty = true;
                    }
                }
                None => warn!(
                    "uitext/uivisible: no item '{item}' on the overlay page{}",
                    if showing {
                        ""
                    } else {
                        " (no overlay is showing)"
                    }
                ),
            }
        }
        ScrOniSysEvent::UiTexture(item, name) => {
            // rbUIManager::SetOverlayItemTexture: the mood, else the same name
            // ending _neutral, else the item hidden (no white box for a
            // character without art yet).  "" clears it.
            let tex = if name.is_empty() {
                None
            } else {
                let OverlayState { textures, .. } = &mut *state;
                let mut t = load_ui_texture(textures, &mut images, name);
                if t.is_none() {
                    let neutral = name
                        .rfind('_')
                        .filter(|&i| !name[i..].eq_ignore_ascii_case("_neutral"))
                        .map(|i| format!("{}_neutral", &name[..i]));
                    if let Some(n) = neutral {
                        t = load_ui_texture(textures, &mut images, &n);
                    }
                    warn!(
                        "uitexture: no texture '{name}' for item '{item}'{}",
                        if t.is_some() {
                            " (showing _neutral)"
                        } else {
                            " (hidden)"
                        }
                    );
                }
                t
            };
            with_rect(&mut state, item, |it, crossfade| {
                let r = it.rect.as_mut().unwrap();
                r.change_texture(now, tex.clone(), crossfade);
                if !name.is_empty() {
                    it.visible = tex.is_some();
                }
                it.dirty = true;
            });
        }
        ScrOniSysEvent::UiMove { item, x, y, secs } => {
            with_rect(&mut state, item, |it, _| {
                it.rect
                    .as_mut()
                    .unwrap()
                    .move_to(now, Vec2::new(*x as f32, *y as f32), *secs);
            });
        }
        ScrOniSysEvent::UiColor { item, rgba, secs } => {
            with_rect(&mut state, item, |it, _| {
                it.rect
                    .as_mut()
                    .unwrap()
                    .fade_to(now, rgba.map(f32::from), *secs);
            });
        }
        _ => {}
    }
}

/// Run `f` on a 2D rect item of the showing page (with its CROSSFADE), or warn
/// as rbUIManager::GetOverlayItem does.  Text items don't move or retexture
/// in the port yet (C++ allows it; no script uses it).
fn with_rect(state: &mut OverlayState, item: &str, f: impl FnOnce(&mut OverlayItem, f32)) {
    let showing = state.active.is_some();
    match state
        .active_items()
        .and_then(|v| v.iter_mut().find(|i| i.name.eq_ignore_ascii_case(item)))
    {
        Some(it) => {
            let crossfade = match &it.kind {
                OverlayKind::Rect2D(r) => r.crossfade,
                _ => {
                    warn!(
                        "uitexture/uimove/uicolor: item '{item}' is not an ITEM_RECT_2D (not supported in the port)"
                    );
                    return;
                }
            };
            f(it, crossfade);
        }
        None => warn!(
            "uitext/uivisible/uitexture/uimove/uicolor: no item '{item}' on the overlay page{}",
            if showing {
                ""
            } else {
                " (no overlay is showing)"
            }
        ),
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// An overlay belongs to the layout that showed it (rbUIManager::Draw).
fn drop_overlay_on_layout_change(
    mut state: ResMut<OverlayState>,
    layout: Option<Res<SelectedLayout>>,
    mut commands: Commands,
) {
    let current = layout.map(|l| l.0.clone()).unwrap_or_default();
    if state.loaded_layout.as_ref().is_some_and(|l| *l != current) {
        for items in state.page_items.values_mut() {
            despawn_items(&mut commands, items);
        }
        state.active = None;
        state.loaded_layout = None;
        state.pages.clear();
        state.page_items.clear();
    }
}

fn reset_overlay(mut state: ResMut<OverlayState>) {
    // InGameEntity cleanup already despawned the entities.
    let fonts = std::mem::take(&mut state.fonts);
    *state = OverlayState { fonts, ..default() };
}

/// Marker + placement data for a projected (2D-canvas or billboard) item.
#[derive(Component)]
struct OverlayUiNode;

#[derive(Component)]
struct OverlayWorldQuad;

/// An ITEM_RECT_2D's node (the current picture, or the one fading out).
#[derive(Component)]
struct OverlayRectNode;

/// uiItemRect2D::Draw each frame: tweens, then place, tint and texture the
/// item's node(s) on the 512x448 canvas.  A cross-fade shows the old picture
/// at the item's colour underneath and the new one at alpha x k over it, so
/// the brightness never dips.
fn animate_overlay_rects(
    mut state: ResMut<OverlayState>,
    time: Res<Time>,
    mut nodes: Query<
        (
            &mut Node,
            &mut ImageNode,
            &mut BackgroundColor,
            &mut Visibility,
        ),
        With<OverlayRectNode>,
    >,
) {
    let now = time.elapsed_secs();
    let Some((key, _)) = state.active.clone() else {
        return;
    };
    let Some(items) = state.page_items.get_mut(&key) else {
        return;
    };
    for it in items.iter_mut() {
        let (OverlayKind::Rect2D(r), Some(entity), Some(rs)) =
            (&it.kind, it.entity, it.rect.as_mut())
        else {
            continue;
        };
        rs.animate(now);
        let rgba = |c: [f32; 4], alpha_k: f32| {
            Color::srgba_u8(
                c[0] as u8,
                c[1] as u8,
                c[2] as u8,
                (c[3] * alpha_k).round().clamp(0.0, 255.0) as u8,
            )
        };
        let mut new_k = 1.0;
        if let Some((_, t0)) = rs.old_texture.as_ref() {
            let k = (now - t0) / r.crossfade.max(1e-3);
            if k >= 1.0 {
                rs.old_texture = None;
            } else {
                new_k = k.max(0.0);
            }
        }
        // As 2D text: the box's place is a share of the canvas, its size keeps
        // the shape it has on rb's 4:3 screen (a 512x448 unit is 7/6 as wide
        // as tall there: 144x168 is square), centred on where the stretched
        // box would be, so a wide window doesn't squash a face.
        const UNIT_ASPECT: f32 = (4.0 / 3.0) * UI_CANVAS_H / UI_CANVAS_W;
        let place = |node: &mut Node| {
            let w_vh = r.width as f32 * UNIT_ASPECT / UI_CANVAS_H * 100.0;
            node.left = Val::Percent((rs.pos.x + r.width as f32 / 2.0) / UI_CANVAS_W * 100.0);
            node.margin = UiRect::left(Val::Vh(-w_vh / 2.0));
            node.top = Val::Percent(rs.pos.y / UI_CANVAS_H * 100.0);
            node.width = Val::Vh(w_vh);
            node.height = Val::Vh(r.height as f32 / UI_CANVAS_H * 100.0);
        };
        if let Ok((mut node, mut img, mut bg, mut vis)) = nodes.get_mut(entity) {
            place(&mut node);
            *vis = Visibility::Visible;
            match rs.texture.as_ref() {
                Some(t) => {
                    if img.image != *t {
                        img.image = t.clone();
                    }
                    img.color = rgba(rs.color, new_k);
                    bg.0 = Color::NONE;
                }
                None => {
                    // No texture: a flat rect of the item's colour.
                    img.color = Color::NONE;
                    bg.0 = rgba(rs.color, 1.0);
                }
            }
        }
        if let Some(under) = rs.under
            && let Ok((mut node, mut img, _, mut vis)) = nodes.get_mut(under)
        {
            match rs.old_texture.as_ref() {
                Some((t, _)) => {
                    place(&mut node);
                    if img.image != *t {
                        img.image = t.clone();
                    }
                    img.color = rgba(rs.color, 1.0);
                    *vis = Visibility::Visible;
                }
                None => *vis = Visibility::Hidden,
            }
        }
    }
}

fn make_image(w: u32, h: u32, rgba: Vec<u8>) -> Image {
    Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// A quad mesh with the rectangle's four corners in the anchor's frame
/// (TOP_LEFT and BOTTOM_RIGHT share a plane: x across, y up).
fn world_quad_mesh(r: &Rect3DProps) -> Mesh {
    let (tl, br) = (r.top_left, r.bottom_right);
    let corners = [
        [tl.x, tl.y, tl.z],
        [br.x, tl.y, br.z],
        [tl.x, br.y, tl.z],
        [br.x, br.y, br.z],
    ];
    let e1 = Vec3::from(corners[1]) - Vec3::from(corners[0]);
    let e2 = Vec3::from(corners[2]) - Vec3::from(corners[0]);
    let n = e1.cross(e2).normalize_or(Vec3::Z).to_array();
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, corners.to_vec())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![n; 4])
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
    )
    .with_inserted_indices(Indices::U32(vec![0, 2, 1, 1, 2, 3]))
}

/// Rebuild images / entities for items whose text or visibility changed.
fn sync_overlay_items(
    mut state: ResMut<OverlayState>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    let Some((key, _)) = state.active.clone() else {
        return;
    };
    // Glyphs are drawn at one font pixel per 448th of the window height (C++
    // draws them 1:1 in its 640x448 window), so a 512-unit-wide box spans
    // `aspect` times as many font pixels on a wider window.  A resize changes
    // where 2D text wraps: rebuild those items.
    let aspect = windows
        .iter()
        .next()
        .map(|w| (w.width() / UI_CANVAS_W) / (w.height() / UI_CANVAS_H).max(1e-3))
        .filter(|a| a.is_finite() && *a > 0.0)
        .unwrap_or(1.0);
    if (aspect - state.aspect).abs() > 0.01 {
        state.aspect = aspect;
        for it in state.page_items.values_mut().flatten() {
            if matches!(it.kind, OverlayKind::Text2D(_)) {
                it.dirty = true;
            }
        }
    }
    // Fonts first (needs &mut state), then the items.
    let wanted: Vec<String> = state
        .page_items
        .get(&key)
        .map(|v| {
            v.iter()
                .filter(|i| i.dirty && !matches!(i.kind, OverlayKind::Rect2D(_)))
                .map(|i| i.font.clone())
                .collect()
        })
        .unwrap_or_default();
    let fonts: HashMap<String, Option<Arc<FontTex>>> = wanted
        .into_iter()
        .map(|f| (f.clone(), state.font(&f)))
        .collect();
    let OverlayState {
        page_items,
        textures,
        ..
    } = &mut *state;
    let Some(items) = page_items.get_mut(&key) else {
        return;
    };
    for it in items.iter_mut().filter(|i| i.dirty) {
        it.dirty = false;
        if let OverlayKind::Rect2D(r) = &it.kind {
            // The node exists while the item is visible; animate_overlay_rects
            // places, tints and textures it every frame.
            if !it.visible {
                if let Some(e) = it.entity.take() {
                    commands.entity(e).try_despawn();
                }
                if let Some(e) = it.rect.as_mut().and_then(|s| s.under.take()) {
                    commands.entity(e).try_despawn();
                }
            } else if it.entity.is_none() {
                let rs = it.rect.as_mut().unwrap();
                if rs.texture.is_none()
                    && let Some(name) = r.texture.as_deref()
                {
                    rs.texture = load_ui_texture(textures, &mut images, name);
                }
                let spawn = |commands: &mut Commands, z: i32, label: &str| {
                    commands
                        .spawn((
                            ImageNode {
                                flip_x: r.mirror,
                                ..default()
                            },
                            BackgroundColor(Color::NONE),
                            Node {
                                position_type: PositionType::Absolute,
                                ..default()
                            },
                            GlobalZIndex(z),
                            Pickable::IGNORE,
                            OverlayRectNode,
                            InGameEntity,
                            Name::new(format!("UiOverlay {label}")),
                        ))
                        .id()
                };
                rs.under = Some(spawn(
                    &mut commands,
                    44,
                    &format!("{} (fading out)", it.name),
                ));
                it.entity = Some(spawn(&mut commands, 45, &it.name));
            }
            continue;
        }
        let font = fonts.get(&it.font).cloned().flatten();
        let (box_width, color) = match &it.kind {
            OverlayKind::Text2D(r) => (Some((r.width as f32 * aspect).round() as i32), r.color),
            OverlayKind::Text3D(r) => (None, r.color),
            OverlayKind::Rect2D(_) => continue, // handled above
        };
        let raster = if it.visible && !it.text.is_empty() {
            font.as_ref()
                .and_then(|f| f.rasterize(&it.text, box_width, color))
        } else {
            None
        };
        let Some((w, h, rgba)) = raster else {
            // Nothing to draw: an empty string clears the item.
            if let Some(e) = it.entity.take() {
                commands.entity(e).try_despawn();
            }
            it.material = None;
            it.image = None;
            continue;
        };
        let handle = images.add(make_image(w, h, rgba));
        it.image = Some(handle.clone());
        match &it.kind {
            OverlayKind::Text2D(r) => {
                if let Some(e) = it.entity.take() {
                    commands.entity(e).try_despawn();
                }
                // Box position on the 512x448 canvas; the text image (lines
                // already centred in it) at a uniform font-pixel scale,
                // centred on the box.
                let vh = |px: u32| Val::Vh(px as f32 / UI_CANVAS_H * 100.0);
                let e = commands
                    .spawn((
                        ImageNode::new(handle),
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Percent(
                                (r.top_left.0 as f32 + r.width as f32 / 2.0) / UI_CANVAS_W * 100.0,
                            ),
                            margin: UiRect::left(Val::Vh(-(w as f32) / 2.0 / UI_CANVAS_H * 100.0)),
                            top: Val::Percent(r.top_left.1 as f32 / UI_CANVAS_H * 100.0),
                            width: vh(w),
                            height: vh(h),
                            ..default()
                        },
                        GlobalZIndex(50),
                        Pickable::IGNORE,
                        OverlayUiNode,
                        InGameEntity,
                        Name::new(format!("UiOverlay {}", it.name)),
                    ))
                    .id();
                it.entity = Some(e);
            }
            OverlayKind::Text3D(r) if r.world => {
                if let (Some(e), Some(mat)) = (it.entity, it.material.as_ref())
                    && let Some(m) = materials.get_mut(mat)
                {
                    m.base_color_texture = Some(handle);
                    let _ = e;
                    continue;
                }
                let mat = materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    base_color_texture: Some(handle),
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    cull_mode: None,
                    double_sided: true,
                    fog_enabled: true,
                    ..default()
                });
                let e = commands
                    .spawn((
                        Mesh3d(meshes.add(world_quad_mesh(r))),
                        MeshMaterial3d(mat.clone()),
                        Transform::default(),
                        Visibility::Hidden,
                        bevy::light::NotShadowCaster,
                        OverlayWorldQuad,
                        InGameEntity,
                        Name::new(format!("UiOverlay {}", it.name)),
                    ))
                    .id();
                it.entity = Some(e);
                it.material = Some(mat);
            }
            OverlayKind::Rect2D(_) => {}
            OverlayKind::Text3D(_) => {
                if let Some(e) = it.entity {
                    commands.entity(e).insert(ImageNode::new(handle));
                    continue;
                }
                let e = commands
                    .spawn((
                        ImageNode::new(handle),
                        Node {
                            position_type: PositionType::Absolute,
                            ..default()
                        },
                        Visibility::Hidden,
                        GlobalZIndex(40),
                        Pickable::IGNORE,
                        OverlayUiNode,
                        InGameEntity,
                        Name::new(format!("UiOverlay {}", it.name)),
                    ))
                    .id();
                it.entity = Some(e);
            }
        }
    }
}

/// Follow anchors, apply NEAR, project billboards.
#[allow(clippy::type_complexity)]
fn place_overlay_items(
    state: Res<OverlayState>,
    named: Query<(&Name, &GlobalTransform), Without<OverlayWorldQuad>>,
    player: Query<&GlobalTransform, With<Player>>,
    cameras: Query<(&Camera, &GlobalTransform, Has<CameraController>), With<Camera3d>>,
    mut ui_nodes: Query<
        (&mut Node, &mut Visibility),
        (With<OverlayUiNode>, Without<OverlayWorldQuad>),
    >,
    mut quads: Query<
        (&mut Transform, &mut Visibility),
        (With<OverlayWorldQuad>, Without<OverlayUiNode>),
    >,
) {
    let Some((key, _)) = state.active.as_ref() else {
        return;
    };
    let Some(items) = state.page_items.get(key) else {
        return;
    };
    // The game camera (the one with a controller), else any active 3D camera.
    let cam = cameras
        .iter()
        .filter(|(c, _, _)| c.is_active)
        .max_by_key(|(_, _, ctl)| *ctl)
        .map(|(c, t, _)| (c, t));
    let player_pos = player.iter().next().map(|t| t.translation());
    for it in items {
        let (Some(entity), OverlayKind::Text3D(r)) = (it.entity, &it.kind) else {
            continue;
        };
        // An anchor spawned later (or respawned) is looked up again each frame;
        // until it exists the item draws nothing.
        let anchor = match r.attached_to.as_deref() {
            Some(name) => named
                .iter()
                .find(|(n, _)| n.as_str().eq_ignore_ascii_case(name))
                .map(|(_, t)| *t),
            None => Some(GlobalTransform::IDENTITY),
        };
        let near_ok = |a: &GlobalTransform| match (r.near, player_pos) {
            (Some(d), Some(p)) if d > 0.0 => p.distance_squared(a.translation()) <= d * d,
            (Some(d), None) if d > 0.0 => false,
            _ => true,
        };
        if r.world {
            if let Ok((mut tf, mut vis)) = quads.get_mut(entity) {
                match anchor.filter(near_ok) {
                    Some(a) => {
                        *tf = a.compute_transform();
                        *vis = Visibility::Visible;
                    }
                    None => *vis = Visibility::Hidden,
                }
            }
            continue;
        }
        let Ok((mut node, mut vis)) = ui_nodes.get_mut(entity) else {
            continue;
        };
        let placed = (|| {
            let a = anchor.filter(near_ok)?;
            let (camera, cam_tf) = cam?;
            // Billboard: the rectangle in the camera's right/up plane at the
            // anchor.  Parsed corners are Bevy-space (x negated); the
            // authored x runs across the screen, so undo that flip.
            let at = a.translation();
            let (right, up) = (cam_tf.right().as_vec3(), cam_tf.up().as_vec3());
            let (x0, x1) = (-r.top_left.x, -r.bottom_right.x);
            let (y0, y1) = (r.top_left.y, r.bottom_right.y);
            let tl = camera
                .world_to_viewport(cam_tf, at + right * x0 + up * y0)
                .ok()?;
            let br = camera
                .world_to_viewport(cam_tf, at + right * x1 + up * y1)
                .ok()?;
            Some((tl, br))
        })();
        match placed {
            Some((tl, br)) => {
                node.left = Val::Px(tl.x.min(br.x));
                node.top = Val::Px(tl.y.min(br.y));
                node.width = Val::Px((tl.x - br.x).abs().max(1.0));
                node.height = Val::Px((tl.y - br.y).abs().max(1.0));
                *vis = Visibility::Visible;
            }
            None => *vis = Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::frontend::{ItemKind, PageKind, StringSource, parse_ui};

    /// Renders sample overlay strings from the loose rb fonts to PNGs in
    /// $UI_OVERLAY_DUMP (skipped when that or $RB_ASSETS is unset).
    #[test]
    fn dump_font_rasters() {
        let (Ok(out), Ok(root)) = (std::env::var("UI_OVERLAY_DUMP"), std::env::var("RB_ASSETS"))
        else {
            return;
        };
        let read = |dir: &str, file: &str| std::fs::read(format!("{root}/{dir}/{file}")).ok();
        for (font, text, width, color) in [
            (
                "feTitle",
                "Overlay demo: text over live gameplay.",
                Some(480),
                [255, 220, 120, 255],
            ),
            ("feTitle", "BLAST CHAMBER A", None, [255, 155, 0, 200]),
            (
                "rbhud",
                "Reach the coolant control room. Vent in 5!",
                Some(480),
                [160, 230, 255, 255],
            ),
            ("rbtitle", "CHECKPOINT", Some(360), [90, 255, 255, 255]),
        ] {
            let f = super::FontTex::load_with(font, read).expect(font);
            let (w, h, rgba) = f.rasterize(text, width, color).expect("raster");
            // Composite on dark grey so the alpha reads in a viewer.
            let mut img = image::RgbaImage::new(w, h);
            for (i, px) in img.pixels_mut().enumerate() {
                let a = rgba[i * 4 + 3] as u32;
                let mix = |c: u8| ((c as u32 * a + 30 * (255 - a)) / 255) as u8;
                *px = image::Rgba([
                    mix(rgba[i * 4]),
                    mix(rgba[i * 4 + 1]),
                    mix(rgba[i * 4 + 2]),
                    255,
                ]);
            }
            img.save(format!("{out}/{font}_{}.png", text.len()))
                .unwrap();
        }
    }

    #[test]
    fn parses_portrait_rects_with_mirror_and_crossfade() {
        let src = r#"
PAGE_OVERLAY "EXS_OutroSubs"
{
	ITEM_RECT_2D "PorL"
	{
		TOP_LEFT	-152 61
		WIDTH	144
		HEIGHT	168
		COLOR	255 255 255 0
		CROSSFADE	0.20
	}
	ITEM_RECT_2D "PorR"
	{
		TOP_LEFT	520 61
		WIDTH	144
		HEIGHT	168
		COLOR	255 255 255 0
		MIRROR
		CROSSFADE	0.20
	}
}
"#;
        let ui = parse_ui(src).expect("parse");
        let rects: Vec<_> = ui.pages[0]
            .items
            .iter()
            .map(|i| match &i.kind {
                ItemKind::Rect2D(r) => r.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(rects[0].top_left, (-152, 61));
        assert!(!rects[0].mirror && rects[1].mirror);
        assert!((rects[1].crossfade - 0.2).abs() < 1e-6);
        assert_eq!(rects[1].color, [255, 255, 255, 0]);
    }

    #[test]
    fn rect_slides_with_ease_out_and_blends_colour() {
        use bevy::prelude::Vec2;
        let props = crate::frontend::ast::Rect2DProps {
            top_left: (-152, 61),
            width: 144,
            height: 168,
            ..Default::default()
        };
        let mut r = super::RectState::new(&props);
        r.move_to(10.0, Vec2::new(16.0, 61.0), 0.3);
        r.animate(10.15); // half the time: ease-out is 7/8 of the way
        assert_eq!(r.pos, Vec2::new((-152.0 + 168.0 * 0.875f32).round(), 61.0));
        r.animate(10.3);
        assert_eq!(r.pos, Vec2::new(16.0, 61.0));
        assert!(r.moving.is_none());
        r.fade_to(11.0, [120.0, 124.0, 136.0, 255.0], 0.25);
        r.animate(11.125);
        assert_eq!(r.color[0], ((props.color[0] as f32 + 120.0) / 2.0).round());
        r.move_to(12.0, Vec2::new(0.0, 0.0), 0.0); // 0 secs: at once
        assert_eq!(r.pos, Vec2::ZERO);
    }

    #[test]
    fn crossfade_only_between_two_pictures() {
        use bevy::prelude::{Handle, Image};
        let props = crate::frontend::ast::Rect2DProps::default();
        let mut r = super::RectState::new(&props);
        let (a, b) = (
            Handle::<Image>::default(),
            Handle::<Image>::Uuid(bevy::asset::uuid::Uuid::from_u128(7), Default::default()),
        );
        r.change_texture(1.0, Some(a.clone()), 0.2); // from none: cuts
        assert!(r.old_texture.is_none());
        r.change_texture(2.0, Some(b.clone()), 0.2); // a mood change: fades
        assert_eq!(
            r.old_texture.as_ref().map(|(h, t)| (h.clone(), *t)),
            Some((a.clone(), 2.0))
        );
        r.change_texture(2.1, Some(a.clone()), 0.0); // no CROSSFADE: cuts, ends the running fade
        assert!(r.old_texture.is_none());
        r.change_texture(3.0, None, 0.2); // to none (a new speaker): cuts
        assert!(r.old_texture.is_none() && r.texture.is_none());
    }

    #[test]
    fn parses_overlay_page_with_literal_text_world_and_near() {
        let src = r#"
# comment
PAGE_OVERLAY "CW_Overlay"
{
	ITEM_RECT_2D_TEXT "Caption"
	{
		TOP_LEFT	16 384
		WIDTH	480
		HEIGHT	40
		COLOR	255 220 120 255
		TEXT "" FONT "rbhud"
	}
	ITEM_RECT_3D_TEXT "Sign"
	{
		ATTACHED_TO "actor_SignIntake"
		TOP_LEFT	-4	0.8	0.0
		BOTTOM_RIGHT	4	0	0.0
		WORLD
		NEAR 6
		COLOR	255 155 0 220
		TEXT "BLAST CHAMBER 3" FONT "rbtitle"
	}
}
"#;
        let ui = parse_ui(src).expect("parse");
        assert_eq!(ui.pages.len(), 1);
        assert!(matches!(ui.pages[0].kind, PageKind::PageOverlay));
        match &ui.pages[0].items[0].kind {
            ItemKind::Rect2DText {
                string: StringSource::Literal { text, font },
                ..
            } => {
                assert_eq!(text, "");
                assert_eq!(font.as_deref(), Some("rbhud"));
            }
            other => panic!("{other:?}"),
        }
        match &ui.pages[0].items[1].kind {
            ItemKind::Rect3DText {
                rect,
                string: StringSource::Literal { text, .. },
            } => {
                assert!(rect.world);
                assert_eq!(rect.near, Some(6.0));
                assert_eq!(text, "BLAST CHAMBER 3");
                assert_eq!(rect.color, [255, 155, 0, 220]);
            }
            other => panic!("{other:?}"),
        }
    }
}
