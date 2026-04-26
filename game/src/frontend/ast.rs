/*
 * frontend/ast.rs — AST types for a parsed .ui file.
 *
 * One-to-one shape match against the C++ rbfrontend class tree:
 *   UiFile      — rbUIManager
 *   Page        — uiPage (kind-tagged: Page2D / Page3D / PageMovie)
 *   Item        — uiItem (kind-tagged: Invisible / Actor / Rect2D / ...)
 *   Event       — uiItemEvent (kind-tagged: InputUp / OnTimeElapsed / ...)
 *   Handler     — uiItemEventHandler (kind-tagged: GoToPage / PlaySound / ...)
 *
 * Where legacy struct members correspond to tokens the parser reads,
 * the AST field uses the same semantic name (e.g. `top_left`,
 * `camera_init`, `string_id`) so grepping C++ → Rust is painless.
 */
use bevy::math::Vec3;

// ---------------------------------------------------------------------------
// Top level — mirrors rbUIManager
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct UiFile {
    /// `PRELOAD_LAYOUT "<name>"` — layout loaded as a 3D backdrop for
    /// PAGE_3D pages.  Stored as a GAMEDATA layout name hint.
    pub preload_layout: Option<String>,
    /// `PRELOAD_TEXTURE "<name>"` — textures pre-cached before any
    /// page renders.  Legacy calls `gfxGetTexture` on each at load
    /// time.  Order-preserving.
    pub preload_textures: Vec<String>,
    /// `STRING_TABLE "<name>"` — localization table filename.
    pub string_table: Option<String>,
    /// All parsed PAGE_* blocks, in source order.
    pub pages: Vec<Page>,
    /// `ON_STARTUP "<page>"` — initial page when the UI starts in
    /// normal startup state.
    pub startup_page: Option<String>,
    /// `ON_DEV_STARTUP "<page>"` — initial page when dev front-end
    /// flag is set (command-line / debug builds).
    pub dev_startup_page: Option<String>,
    /// `ON_ABORT_GAME "<page>"` — shown when player aborts to front.
    pub abort_game_page: Option<String>,
    /// `ON_GAME_COMPLETE "<page>"` — shown when the game is won.
    pub game_complete_page: Option<String>,
    /// `ON_IN_GAME "<page>"` — in-game pause root (mostly rbgame.ui).
    pub in_game_page: Option<String>,
    /// `ON_LEVEL_COMPLETE <index> "<page>"` — per-level completion
    /// screen.  Legacy caps at MAX_LEVELS=32.
    pub level_complete_pages: Vec<(i32, String)>,
}

// ---------------------------------------------------------------------------
// Page — mirrors uiPage / uiPage2D / uiPage3D / uiPageMovie
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Page {
    /// `PAGE_2D` / `PAGE_3D` / `PAGE_MOVIE` — from the token itself.
    pub class_name: String,
    /// User-given name, used by GO_TO_PAGE lookups.
    pub name: String,
    /// Default 0.5.  Grace period after StartPage during which input
    /// events are ignored (prevents the OK-button-from-previous-page
    /// from instantly firing the new page's OK handler).
    pub time_to_disable_events: f32,
    pub kind: PageKind,
    /// Items in declaration order.  First one gets initial focus
    /// (uiPage::InitPostLoad).
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum PageKind {
    Page2D {
        /// `BACKGROUND "<texname>"` — drawn full-screen before items.
        background: Option<String>,
    },
    Page3D {
        /// `LAYOUT "<name>"` — required by the C++ parser
        /// (uses GetDelimiter, not CheckToken).
        layout: String,
        /// `CAMERA_INIT ( pos ) , ( lookat ) , fov` — optional in the
        /// C++ parser (only read when CheckToken succeeds).
        camera_init: Option<CameraInit>,
    },
    PageMovie {
        /// `MOVIE "<filename>"` — required (GetDelimiter).
        movie: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct CameraInit {
    pub position: Vec3,
    pub track_point: Vec3,
    pub fov: f32,
}

// ---------------------------------------------------------------------------
// Item — mirrors uiItem and subclasses
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Item {
    /// Token as authored: `ITEM_RECT_2D`, `ITEM_INVISIBLE`, etc.
    pub class_name: String,
    pub name: String,
    /// `DISABLED` flag — default true; `DISABLED` sets to false.
    pub is_enabled: bool,
    /// `INVISIBLE` flag — default true; `INVISIBLE` sets to false.
    pub is_visible: bool,
    pub kind: ItemKind,
    /// ON_* blocks attached to this item.
    pub events: Vec<Event>,
}

#[derive(Debug, Clone)]
pub enum ItemKind {
    /// `ITEM_INVISIBLE` — no payload; typically used as an event
    /// trap (invisible item that fires ON_INPUT_OK etc. because the
    /// current item on startup).
    Invisible,
    /// `ITEM_ACTOR` — an item that targets a named actor in the
    /// preloaded layout (used for PAGE_3D scripting hooks).
    Actor { actor: Option<String> },
    Rect2D(Rect2DProps),
    Rect2DText {
        rect: Rect2DProps,
        string: StringSource,
    },
    Rect2DGrid {
        rect: Rect2DProps,
        /// `CELL_DIMENSIONS <w> <h>` — optional.
        cell_dimensions: Option<(i32, i32)>,
    },
    Rect3D(Rect3DProps),
    Rect3DText {
        rect: Rect3DProps,
        string: StringSource,
    },
    Slider2D {
        rect: Rect2DProps,
        props: Slider2DProps,
    },
    QuadList2D(QuadList2DProps),
    LevelList(List2DProps),
    LevelSavePointList(List2DProps),
}

#[derive(Debug, Clone, Default)]
pub struct Rect2DProps {
    /// `TOP_LEFT <x> <y>` — required.
    pub top_left: (i32, i32),
    /// `WIDTH <w>` — required.
    pub width: i32,
    /// `HEIGHT <h>` — required.
    pub height: i32,
    /// `COLOR <r> <g> <b> <a>` — default (255,255,255,255).
    pub color: [u8; 4],
    /// `TEXTURE "<name>"` — optional.
    pub texture: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Rect3DProps {
    /// `ATTACHED_TO "<actor>"` — optional; the rect's corners live in
    /// the actor's local space when attached.
    pub attached_to: Option<String>,
    /// `TOP_LEFT <x> <y> <z>`.
    pub top_left: Vec3,
    /// `BOTTOM_RIGHT <x> <y> <z>`.
    pub bottom_right: Vec3,
    /// `COLOR <r> <g> <b> <a>` — default (255,255,255,255).
    pub color: [u8; 4],
    /// `BLEND_MODE <int>` — default 1 (blendSet_One_One in legacy
    /// enum).
    pub blend_mode: i32,
    /// `TEXTURE "<name>"` — optional.
    pub texture: Option<String>,
}

/// `STRING_ID "<id>"` | `STRING_LEVEL_NAME`.  The second form maps to
/// the current layout name at runtime.
#[derive(Debug, Clone)]
pub enum StringSource {
    StringId(String),
    LevelName,
}

#[derive(Debug, Clone, Default)]
pub struct Slider2DProps {
    /// `INDICATOR_WIDTH <w>` — required.
    pub indicator_width: i32,
    /// `INDICATOR_HEIGHT <h>` — required.
    pub indicator_height: i32,
    /// `INDICATOR_MARGIN_X <x>` — optional, default 0.
    pub indicator_margin_x: i32,
    /// `INDICATOR_TEXTURE "<name>"` — optional.
    pub indicator_texture: Option<String>,
    /// `STEP <f>` — optional; legacy forces min 0.01 when set.
    pub step: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct QuadList2DProps {
    /// `TOP_LEFT <x> <y>` — optional (acts as an origin offset).
    pub top_left: Option<(i32, i32)>,
    /// `QUAD x0 y0 x1 y1 x2 y2 x3 y3` — each is four 2D points.
    pub quads: Vec<[(i32, i32); 4]>,
    /// `COLOR ...` — default white.
    pub color: [u8; 4],
    /// `TEXTURE "<name>"` — optional.
    pub texture: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct List2DProps {
    pub rect: Rect2DProps,
    /// `COLOR_HIGHLIGHT r g b a` — optional.
    pub color_highlight: Option<[u8; 4]>,
    /// `COLOR_NORMAL r g b a` — optional.
    pub color_normal: Option<[u8; 4]>,
    /// `FONT "<name>"` — optional.
    pub font: Option<String>,
    /// `VERTICAL_SPACING <f>` — required via `MatchFloat` (errors if
    /// the keyword is absent in legacy).
    pub vertical_spacing: f32,
}

// ---------------------------------------------------------------------------
// Event — mirrors uiItemEvent and subclasses
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Event {
    /// Token as authored: `ON_INPUT_OK`, `ON_TIME_ELAPSED`, ...
    pub class_name: String,
    pub kind: EventKind,
    /// `{ … }` block of handlers that fire when the event triggers.
    pub handlers: Vec<Handler>,
}

#[derive(Debug, Clone)]
pub enum EventKind {
    InputUp,
    InputDown,
    InputLeft,
    InputRight,
    InputOk,
    InputCancel,
    InputStart,
    FocusGain,
    FocusLose,
    PageStart,
    PageEnd,
    MovieComplete,
    /// `ON_TIME_ELAPSED <delay>` — fires once when the page has been
    /// running for >= `delay` seconds.
    TimeElapsed {
        delay: f32,
    },
    /// `ON_MEM_CARD <stateName>` — fires on a memory-card state
    /// transition.  State names are game-specific strings (legacy
    /// `rbMemoryCardManager::GetCardStateByString`).
    MemCard {
        state: String,
    },
}

// ---------------------------------------------------------------------------
// Handler — mirrors uiItemEventHandler and subclasses
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Handler {
    /// Token as authored: `GO_TO_PAGE`, `PLAY_SOUND`, `IF_VISIBLE`, ...
    pub class_name: String,
    pub kind: HandlerKind,
}

#[derive(Debug, Clone)]
pub enum HandlerKind {
    // ----- Navigation -----
    GoToPage(String),
    GoToPreviousPage,
    GoToItem(String),

    // ----- Actor / script -----
    PlayAnimation {
        pause: bool,
        loop_anim: bool,
        actor: String,
        animation: String,
    },
    RunScript {
        actor: String,
        script: String,
    },

    // ----- Item mutation -----
    ChangeTexture(String),
    ChangeColor {
        color: [u8; 4],
        item: Option<String>,
    },
    ChangeString {
        string_id: String,
        item: Option<String>,
    },

    // ----- Audio volume (read / write) -----
    GetMusicVolume {
        item: Option<String>,
    },
    SetMusicVolume {
        item: Option<String>,
    },
    GetSoundFxVolume {
        item: Option<String>,
    },
    SetSoundFxVolume {
        item: Option<String>,
    },

    // ----- Audio playback -----
    /// `PLAY_SOUND "<name>" [VOLUME <f>] [PITCH <f>]`.
    PlaySound {
        name: String,
        volume: Option<f32>,
        pitch: Option<f32>,
    },

    // ----- Game lifecycle -----
    GameUnpause,
    GameAbort,
    GameReset,
    RunNextLevel,
    CareerReset,

    // ----- Memory card -----
    MemCardScanSlots,
    MemCardWaitUntilReady,
    MemCardGetStatus,
    MemCardLoad,
    MemCardSave,
    MemCardFormat,

    // ----- Item enable / visibility toggles -----
    SetEnabled {
        enabled: bool,
        item: Option<String>,
    },
    SetVisible {
        visible: bool,
        item: Option<String>,
    },

    // ----- Gameplay flags -----
    SetInvincibility(bool),
    SetVibration(bool),

    // ----- Conditional (nested handler list on true) -----
    IfVisible {
        is_visible: bool,
        item: Option<String>,
        handlers: Vec<Handler>,
    },
    IfInvincibilityOn {
        is_it: bool,
        handlers: Vec<Handler>,
    },
    IfVibrationOn {
        is_it: bool,
        handlers: Vec<Handler>,
    },
    IfDevFrontEnd {
        is_it: bool,
        handlers: Vec<Handler>,
    },
    IfStringId {
        string_id: String,
        item: Option<String>,
        handlers: Vec<Handler>,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

impl Default for Page {
    fn default() -> Self {
        Self {
            class_name: String::new(),
            name: String::new(),
            time_to_disable_events: 0.5,
            kind: PageKind::Page2D { background: None },
            items: Vec::new(),
        }
    }
}

impl Default for Item {
    fn default() -> Self {
        Self {
            class_name: String::new(),
            name: String::new(),
            is_enabled: true,
            is_visible: true,
            kind: ItemKind::Invisible,
            events: Vec::new(),
        }
    }
}

/// Default color used when `COLOR` is absent in an item block.
pub const WHITE_RGBA: [u8; 4] = [255, 255, 255, 255];
/// Default BlendMode used when `BLEND_MODE` is absent — legacy
/// `blendSet_One_One` (additive).
pub const DEFAULT_BLEND_MODE: i32 = 1;
