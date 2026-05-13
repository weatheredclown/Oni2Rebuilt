/*
 * frontend/ — port of the legacy `rbfrontend` subsystem.
 *
 * Layer 1 (parser): `.ui` files shipped in assets/Settings/ are
 * parsed into an AST mirroring the C++ rbUIManager / uiPage /
 * uiItem / uiItemEvent / uiItemEventHandler hierarchy.  See
 * `parser::parse_ui`.
 *
 * Layer 2 (runtime): `FrontendPlugin` loads `rbfrontend.ui` at
 * startup, routes the `ON_STARTUP` page into view, and drives
 * page transitions via `RequestPageChange` events.  During
 * `AppState::FrontEnd`, renderer systems spawn Bevy UI for the
 * current page's items, handle keyboard input + focus navigation,
 * and execute the emitted event handlers (GO_TO_PAGE,
 * GO_TO_PREVIOUS_PAGE, PLAY_SOUND, etc.).
 *
 * PAGE_MOVIE (intro logos) and PAGE_3D (Main Menu projector) are
 * stubbed here and handed a black background + auto-advance so
 * the page graph stays navigable without the movie decoder / 3D
 * camera rig.  Phases 4 and 5 replace those stubs.
 */
pub mod ast;
pub mod camera;
pub mod parser;
pub mod render;
pub mod runtime;

pub use ast::*;
pub use parser::parse_ui;
pub use runtime::FrontendPlugin;
