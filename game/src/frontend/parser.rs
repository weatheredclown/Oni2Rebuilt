/*
 * frontend/parser.rs — recursive-descent parser for .ui files.
 *
 * Delegates tokenization + primitive reads to
 * `oni2_loader::parsers::block_parser::BlockParser`, which is the
 * shared datAsciiTokenizer-alike for every Angel-era text asset.
 * This module just wires the grammar: which keywords introduce a
 * Page / Item / Event / Handler, what their per-kind fields look
 * like, and how they nest.  Every parse branch cites the C++ source
 * site (rbmanager.cpp / rbpage.cpp / rbitem.cpp / rbevent.cpp) it
 * mirrors so debugging a malformed .ui file stays grep-friendly.
 */
use super::ast::*;
use crate::oni2_loader::parsers::block_parser::BlockParser;
use crate::oni2_loader::utils::space::to_bevy_space_pos;

type ParseResult<T> = Result<T, String>;

// ---------------------------------------------------------------------------
// Top-level parse — rbUIManager ctor
// ---------------------------------------------------------------------------

pub fn parse_ui(src: &str) -> ParseResult<UiFile> {
    let mut p = BlockParser::new(src);
    let mut ui = UiFile::default();

    // Header — rbmanager.cpp:30-52.
    if p.check_token("PRELOAD_LAYOUT") {
        ui.preload_layout = Some(p.get_token()?);
    }
    while p.check_token("PRELOAD_TEXTURE") {
        ui.preload_textures.push(p.get_token()?);
    }
    if p.check_token("STRING_TABLE") {
        ui.string_table = Some(p.get_token()?);
    }

    // Pages — rbmanager.cpp:191 (InitPages loop).
    loop {
        let kind_keyword = if p.check_token("PAGE_2D") {
            "PAGE_2D"
        } else if p.check_token("PAGE_3D") {
            "PAGE_3D"
        } else if p.check_token("PAGE_MOVIE") {
            "PAGE_MOVIE"
        } else {
            break;
        };
        ui.pages.push(parse_page(&mut p, kind_keyword)?);
    }

    // Manager-level ON_* tail — rbmanager.cpp:68-101.
    if p.check_token("ON_STARTUP") {
        ui.startup_page = Some(p.get_token()?);
    }
    if p.check_token("ON_DEV_STARTUP") {
        ui.dev_startup_page = Some(p.get_token()?);
    }
    if p.check_token("ON_ABORT_GAME") {
        ui.abort_game_page = Some(p.get_token()?);
    }
    if p.check_token("ON_GAME_COMPLETE") {
        ui.game_complete_page = Some(p.get_token()?);
    }
    if p.check_token("ON_IN_GAME") {
        ui.in_game_page = Some(p.get_token()?);
    }
    while p.check_token("ON_LEVEL_COMPLETE") {
        let index = p.get_int()?;
        let page = p.get_token()?;
        ui.level_complete_pages.push((index, page));
    }

    Ok(ui)
}

// ---------------------------------------------------------------------------
// Page — uiPage::Init + per-kind InitCustom
// ---------------------------------------------------------------------------

fn parse_page(p: &mut BlockParser, kind_keyword: &str) -> ParseResult<Page> {
    // uiPage::Init (page.cpp:33) reads TWO tokens: `ClassName` (__DEV
    // only; release reads the same token twice into the name buffer).
    // Shipped .ui files author ONE name token — reading one here and
    // using it for both fields matches the __DEV layout and works
    // with every file in the wild.
    let name = p.get_token()?;
    p.get_delimiter("{")?;

    // Per-kind InitCustom — rbpage.cpp.
    let kind = match kind_keyword {
        "PAGE_2D" => {
            let background = if p.check_token("BACKGROUND") {
                Some(p.get_token()?)
            } else {
                None
            };
            PageKind::Page2D { background }
        }
        "PAGE_3D" => {
            p.get_delimiter("LAYOUT")?;
            let layout = p.get_token()?;
            let camera_init = if p.check_token("CAMERA_INIT") {
                p.get_delimiter("(")?;
                let position = p.get_vec3()?;
                p.get_delimiter(")")?;
                p.get_delimiter(",")?;
                p.get_delimiter("(")?;
                let track_point = p.get_vec3()?;
                p.get_delimiter(")")?;
                p.get_delimiter(",")?;
                let fov = p.get_float()?;
                // Oni2 authors these in left-handed (+Z forward) game
                // space.  Convert at the parse boundary so the runtime
                // only ever sees Bevy-space coords.
                Some(CameraInit {
                    position: to_bevy_space_pos(position),
                    track_point: to_bevy_space_pos(track_point),
                    fov,
                })
            } else {
                None
            };
            PageKind::Page3D {
                layout,
                camera_init,
            }
        }
        "PAGE_MOVIE" => {
            p.get_delimiter("MOVIE")?;
            let movie = p.get_token()?;
            PageKind::PageMovie { movie }
        }
        other => return Err(format!("ui parse: unknown page kind '{}'", other)),
    };

    // InitCommon — page.cpp:84.
    let mut time_to_disable_events = 0.5_f32;
    if p.check_token("TIME_TO_DISABLE_EVENTS") {
        time_to_disable_events = p.get_float()?;
    }

    // Items — rbmanager.cpp:221 (InitPageItems loop).
    let mut items = Vec::new();
    while let Some(item) = parse_item_opt(p)? {
        items.push(item);
    }
    p.get_delimiter("}")?;

    Ok(Page {
        class_name: kind_keyword.to_string(),
        name,
        time_to_disable_events,
        kind,
        items,
    })
}

// ---------------------------------------------------------------------------
// Item — uiItem::Init + per-kind InitCustom
// ---------------------------------------------------------------------------

fn parse_item_opt(p: &mut BlockParser) -> ParseResult<Option<Item>> {
    let keyword = match p.peek() {
        Some(w) if is_item_keyword(w) => w.to_string(),
        _ => return Ok(None),
    };
    p.next();
    Ok(Some(parse_item(p, &keyword)?))
}

fn is_item_keyword(w: &str) -> bool {
    matches!(
        w.to_ascii_uppercase().as_str(),
        "ITEM_INVISIBLE"
            | "ITEM_ACTOR"
            | "ITEM_RECT_2D"
            | "ITEM_RECT_2D_TEXT"
            | "ITEM_RECT_2D_GRID"
            | "ITEM_RECT_3D"
            | "ITEM_RECT_3D_TEXT"
            | "ITEM_SLIDER_2D"
            | "ITEM_QUAD_LIST_2D"
            | "ITEM_LEVEL_LIST"
            | "ITEM_LEVEL_SAVE_POINT_LIST"
    )
}

fn parse_item(p: &mut BlockParser, kind_keyword: &str) -> ParseResult<Item> {
    let name = p.get_token()?;
    p.get_delimiter("{")?;

    let mut item = Item {
        class_name: kind_keyword.to_string(),
        name,
        ..Default::default()
    };

    // Common flags — item.cpp:21-28.
    if p.check_token("DISABLED") {
        item.is_enabled = false;
    }
    if p.check_token("INVISIBLE") {
        item.is_visible = false;
    }

    // Per-kind InitCustom — rbitem.cpp.
    item.kind = match kind_keyword.to_ascii_uppercase().as_str() {
        "ITEM_INVISIBLE" => ItemKind::Invisible,
        "ITEM_ACTOR" => {
            let actor = if p.check_token("ACTOR") {
                Some(p.get_token()?)
            } else {
                None
            };
            ItemKind::Actor { actor }
        }
        "ITEM_RECT_2D" => ItemKind::Rect2D(parse_rect2d(p)?),
        "ITEM_RECT_2D_TEXT" => {
            let rect = parse_rect2d(p)?;
            let string = parse_string_source(p)?;
            ItemKind::Rect2DText { rect, string }
        }
        "ITEM_RECT_2D_GRID" => {
            let rect = parse_rect2d(p)?;
            let cell_dimensions = if p.check_token("CELL_DIMENSIONS") {
                Some((p.get_int()?, p.get_int()?))
            } else {
                None
            };
            ItemKind::Rect2DGrid {
                rect,
                cell_dimensions,
            }
        }
        "ITEM_RECT_3D" => ItemKind::Rect3D(parse_rect3d(p)?),
        "ITEM_RECT_3D_TEXT" => {
            let rect = parse_rect3d(p)?;
            let string = parse_string_source(p)?;
            ItemKind::Rect3DText { rect, string }
        }
        "ITEM_SLIDER_2D" => {
            let rect = parse_rect2d(p)?;
            let props = parse_slider2d_props(p)?;
            ItemKind::Slider2D { rect, props }
        }
        "ITEM_QUAD_LIST_2D" => ItemKind::QuadList2D(parse_quad_list2d(p)?),
        "ITEM_LEVEL_LIST" => ItemKind::LevelList(parse_list2d(p)?),
        "ITEM_LEVEL_SAVE_POINT_LIST" => ItemKind::LevelSavePointList(parse_list2d(p)?),
        other => return Err(format!("ui parse: unknown item kind '{}'", other)),
    };

    // Events — rbmanager.cpp:284 (InitItemEvents loop).
    while let Some(ev) = parse_event_opt(p)? {
        item.events.push(ev);
    }
    p.get_delimiter("}")?;

    Ok(item)
}

fn parse_rect2d(p: &mut BlockParser) -> ParseResult<Rect2DProps> {
    // rbitem.cpp:69 uiItemRect2D::InitCustom.
    p.get_delimiter("TOP_LEFT")?;
    let x = p.get_int()?;
    let y = p.get_int()?;
    p.get_delimiter("WIDTH")?;
    let width = p.get_int()?;
    p.get_delimiter("HEIGHT")?;
    let height = p.get_int()?;
    let color = if p.check_token("COLOR") {
        p.get_color()?
    } else {
        WHITE_RGBA
    };
    let texture = if p.check_token("TEXTURE") {
        Some(p.get_token()?)
    } else {
        None
    };
    Ok(Rect2DProps {
        top_left: (x, y),
        width,
        height,
        color,
        texture,
    })
}

fn parse_rect3d(p: &mut BlockParser) -> ParseResult<Rect3DProps> {
    // rbitem.cpp:205 uiItemRect3D::InitCustom.
    let attached_to = if p.check_token("ATTACHED_TO") {
        Some(p.get_token()?)
    } else {
        None
    };
    p.get_delimiter("TOP_LEFT")?;
    let top_left = p.get_vec3()?;
    p.get_delimiter("BOTTOM_RIGHT")?;
    let bottom_right = p.get_vec3()?;
    let color = if p.check_token("COLOR") {
        p.get_color()?
    } else {
        WHITE_RGBA
    };
    let blend_mode = if p.check_token("BLEND_MODE") {
        p.get_int()?
    } else {
        DEFAULT_BLEND_MODE
    };
    let texture = if p.check_token("TEXTURE") {
        Some(p.get_token()?)
    } else {
        None
    };
    // Rect corners are Oni2-space (either world or actor-local when
    // ATTACHED_TO is set).  Convert at the parse boundary so both
    // branches compose with Bevy-space actor transforms / camera
    // placements — rotating X/Z in local frames is the same sign flip
    // as in world frames.
    Ok(Rect3DProps {
        attached_to,
        top_left: to_bevy_space_pos(top_left),
        bottom_right: to_bevy_space_pos(bottom_right),
        color,
        blend_mode,
        texture,
    })
}

fn parse_string_source(p: &mut BlockParser) -> ParseResult<StringSource> {
    // rbitem.cpp:347-359 / 496-508.  Either `STRING_ID "<id>"` or
    // bareword `STRING_LEVEL_NAME`; absent form defaults to level
    // name (legacy leaves StringID NULL in that case — close enough).
    if p.check_token("STRING_ID") {
        Ok(StringSource::StringId(p.get_token()?))
    } else if p.check_token("STRING_LEVEL_NAME") {
        Ok(StringSource::LevelName)
    } else {
        Ok(StringSource::LevelName)
    }
}

fn parse_slider2d_props(p: &mut BlockParser) -> ParseResult<Slider2DProps> {
    // rbitem.cpp:652 uiItemSlider2D::InitCustom.
    p.get_delimiter("INDICATOR_WIDTH")?;
    let indicator_width = p.get_int()?;
    p.get_delimiter("INDICATOR_HEIGHT")?;
    let indicator_height = p.get_int()?;
    let indicator_margin_x = if p.check_token("INDICATOR_MARGIN_X") {
        p.get_int()?
    } else {
        0
    };
    let indicator_texture = if p.check_token("INDICATOR_TEXTURE") {
        Some(p.get_token()?)
    } else {
        None
    };
    let step = if p.check_token("STEP") {
        let s = p.get_float()?;
        Some(if s <= 0.0 { 0.01 } else { s })
    } else {
        None
    };
    Ok(Slider2DProps {
        indicator_width,
        indicator_height,
        indicator_margin_x,
        indicator_texture,
        step,
    })
}

fn parse_quad_list2d(p: &mut BlockParser) -> ParseResult<QuadList2DProps> {
    // rbitem.cpp:946 uiItemQuadList2D::InitCustom.
    let top_left = if p.check_token("TOP_LEFT") {
        Some((p.get_int()?, p.get_int()?))
    } else {
        None
    };
    let mut quads = Vec::new();
    while p.check_token("QUAD") {
        let mut pts = [(0, 0); 4];
        for pt in &mut pts {
            pt.0 = p.get_int()?;
            pt.1 = p.get_int()?;
        }
        quads.push(pts);
    }
    let color = if p.check_token("COLOR") {
        p.get_color()?
    } else {
        WHITE_RGBA
    };
    let texture = if p.check_token("TEXTURE") {
        Some(p.get_token()?)
    } else {
        None
    };
    Ok(QuadList2DProps {
        top_left,
        quads,
        color,
        texture,
    })
}

fn parse_list2d(p: &mut BlockParser) -> ParseResult<List2DProps> {
    // rbitem.cpp:824 uiItemList2D::InitCustom — LEVEL_LIST and
    // LEVEL_SAVE_POINT_LIST use this unchanged.
    let rect = parse_rect2d(p)?;
    let color_highlight = if p.check_token("COLOR_HIGHLIGHT") {
        Some(p.get_color()?)
    } else {
        None
    };
    let color_normal = if p.check_token("COLOR_NORMAL") {
        Some(p.get_color()?)
    } else {
        None
    };
    let font = if p.check_token("FONT") {
        Some(p.get_token()?)
    } else {
        None
    };
    let vertical_spacing = p.match_float("VERTICAL_SPACING")?;
    Ok(List2DProps {
        rect,
        color_highlight,
        color_normal,
        font,
        vertical_spacing,
    })
}

// ---------------------------------------------------------------------------
// Event — uiItemEvent::Init + per-kind InitCustom
// ---------------------------------------------------------------------------

fn parse_event_opt(p: &mut BlockParser) -> ParseResult<Option<Event>> {
    let keyword = match p.peek() {
        Some(w) if is_event_keyword(w) => w.to_string(),
        _ => return Ok(None),
    };
    p.next();
    Ok(Some(parse_event(p, &keyword)?))
}

fn is_event_keyword(w: &str) -> bool {
    matches!(
        w.to_ascii_uppercase().as_str(),
        "ON_INPUT_UP"
            | "ON_INPUT_DOWN"
            | "ON_INPUT_LEFT"
            | "ON_INPUT_RIGHT"
            | "ON_INPUT_OK"
            | "ON_INPUT_CANCEL"
            | "ON_INPUT_START"
            | "ON_TIME_ELAPSED"
            | "ON_FOCUS_GAIN"
            | "ON_FOCUS_LOSE"
            | "ON_PAGE_START"
            | "ON_PAGE_END"
            | "ON_MOVIE_COMPLETE"
            | "ON_MEM_CARD"
    )
}

fn parse_event(p: &mut BlockParser, keyword: &str) -> ParseResult<Event> {
    // Release-build C++ reads and discards an unused ClassName token
    // after the event keyword (event.cpp:132).  No .ui file in the
    // wild authors one, so we skip that read and jump straight to
    // InitCustom's payload — which is empty for most events.
    let kind = match keyword.to_ascii_uppercase().as_str() {
        "ON_INPUT_UP" => EventKind::InputUp,
        "ON_INPUT_DOWN" => EventKind::InputDown,
        "ON_INPUT_LEFT" => EventKind::InputLeft,
        "ON_INPUT_RIGHT" => EventKind::InputRight,
        "ON_INPUT_OK" => EventKind::InputOk,
        "ON_INPUT_CANCEL" => EventKind::InputCancel,
        "ON_INPUT_START" => EventKind::InputStart,
        "ON_FOCUS_GAIN" => EventKind::FocusGain,
        "ON_FOCUS_LOSE" => EventKind::FocusLose,
        "ON_PAGE_START" => EventKind::PageStart,
        "ON_PAGE_END" => EventKind::PageEnd,
        "ON_MOVIE_COMPLETE" => EventKind::MovieComplete,
        "ON_TIME_ELAPSED" => EventKind::TimeElapsed {
            delay: p.get_float()?,
        },
        "ON_MEM_CARD" => EventKind::MemCard {
            state: p.get_token()?,
        },
        other => return Err(format!("ui parse: unknown event kind '{}'", other)),
    };

    p.get_delimiter("{")?;
    let mut handlers = Vec::new();
    while let Some(h) = parse_handler_opt(p)? {
        handlers.push(h);
    }
    p.get_delimiter("}")?;

    Ok(Event {
        class_name: keyword.to_string(),
        kind,
        handlers,
    })
}

// ---------------------------------------------------------------------------
// Handler — uiItemEventHandler::Init + per-kind InitCustom
// ---------------------------------------------------------------------------

fn parse_handler_opt(p: &mut BlockParser) -> ParseResult<Option<Handler>> {
    let keyword = match p.peek() {
        Some(w) if is_handler_keyword(w) => w.to_string(),
        _ => return Ok(None),
    };
    p.next();
    Ok(Some(parse_handler(p, &keyword)?))
}

fn is_handler_keyword(w: &str) -> bool {
    matches!(
        w.to_ascii_uppercase().as_str(),
        "GO_TO_PAGE"
            | "GO_TO_PREVIOUS_PAGE"
            | "GO_TO_ITEM"
            | "ANIMATION"
            | "SCRIPT"
            | "TEXTURE"
            | "COLOR"
            | "STRING_ID"
            | "GET_MUSIC_VOLUME"
            | "SET_MUSIC_VOLUME"
            | "GET_SOUND_FX_VOLUME"
            | "SET_SOUND_FX_VOLUME"
            | "GAME_UNPAUSE"
            | "GAME_ABORT"
            | "GAME_RESET"
            | "RUN_NEXT_LEVEL"
            | "MEM_CARD_SCAN_SLOTS"
            | "MEM_CARD_WAIT_UNTIL_READY"
            | "MEM_CARD_GET_STATUS"
            | "MEM_CARD_LOAD"
            | "MEM_CARD_SAVE"
            | "MEM_CARD_FORMAT"
            | "CAREER_RESET"
            | "PLAY_SOUND"
            | "SET_ENABLED"
            | "SET_VISIBLE"
            | "SET_INVINCIBILITY"
            | "SET_VIBRATION"
            | "IF_VISIBLE"
            | "IF_INVINCIBILITY_ON"
            | "IF_VIBRATION_ON"
            | "IF_DEV_FRONT_END"
            | "IF_STRING_ID"
    )
}

fn parse_handler(p: &mut BlockParser, keyword: &str) -> ParseResult<Handler> {
    // Same class-name dance as events — release C++ reads an unused
    // token; shipped files don't author one.  Skip straight to args.
    let upper = keyword.to_ascii_uppercase();
    let kind = match upper.as_str() {
        // Navigation
        "GO_TO_PAGE" => HandlerKind::GoToPage(p.get_token()?),
        "GO_TO_PREVIOUS_PAGE" => HandlerKind::GoToPreviousPage,
        "GO_TO_ITEM" => HandlerKind::GoToItem(p.get_token()?),
        // Actor / script
        "ANIMATION" => {
            let pause = p.check_token("PAUSE");
            let loop_anim = p.check_token("LOOP");
            let actor = p.get_token()?;
            let animation = p.get_token()?;
            HandlerKind::PlayAnimation {
                pause,
                loop_anim,
                actor,
                animation,
            }
        }
        "SCRIPT" => {
            let actor = p.get_token()?;
            let script = p.get_token()?;
            HandlerKind::RunScript { actor, script }
        }
        // Item mutation
        "TEXTURE" => HandlerKind::ChangeTexture(p.get_token()?),
        "COLOR" => {
            let color = p.get_color()?;
            let item = parse_optional_item(p)?;
            HandlerKind::ChangeColor { color, item }
        }
        "STRING_ID" => {
            let string_id = p.get_token()?;
            let item = parse_optional_item(p)?;
            HandlerKind::ChangeString { string_id, item }
        }
        // Audio volume
        "GET_MUSIC_VOLUME" => HandlerKind::GetMusicVolume {
            item: parse_optional_item(p)?,
        },
        "SET_MUSIC_VOLUME" => HandlerKind::SetMusicVolume {
            item: parse_optional_item(p)?,
        },
        "GET_SOUND_FX_VOLUME" => HandlerKind::GetSoundFxVolume {
            item: parse_optional_item(p)?,
        },
        "SET_SOUND_FX_VOLUME" => HandlerKind::SetSoundFxVolume {
            item: parse_optional_item(p)?,
        },
        // Audio playback
        "PLAY_SOUND" => {
            let name = p.get_token()?;
            let volume = if p.check_token("VOLUME") {
                Some(p.get_float()?)
            } else {
                None
            };
            let pitch = if p.check_token("PITCH") {
                Some(p.get_float()?)
            } else {
                None
            };
            HandlerKind::PlaySound {
                name,
                volume,
                pitch,
            }
        }
        // Game lifecycle
        "GAME_UNPAUSE" => HandlerKind::GameUnpause,
        "GAME_ABORT" => HandlerKind::GameAbort,
        "GAME_RESET" => HandlerKind::GameReset,
        "RUN_NEXT_LEVEL" => HandlerKind::RunNextLevel,
        "CAREER_RESET" => HandlerKind::CareerReset,
        // Memory card
        "MEM_CARD_SCAN_SLOTS" => HandlerKind::MemCardScanSlots,
        "MEM_CARD_WAIT_UNTIL_READY" => HandlerKind::MemCardWaitUntilReady,
        "MEM_CARD_GET_STATUS" => HandlerKind::MemCardGetStatus,
        "MEM_CARD_LOAD" => HandlerKind::MemCardLoad,
        "MEM_CARD_SAVE" => HandlerKind::MemCardSave,
        "MEM_CARD_FORMAT" => HandlerKind::MemCardFormat,
        // Enabled / visible toggles
        "SET_ENABLED" => {
            let enabled = p.get_int()? != 0;
            let item = parse_optional_item(p)?;
            HandlerKind::SetEnabled { enabled, item }
        }
        "SET_VISIBLE" => {
            let visible = p.get_int()? != 0;
            let item = parse_optional_item(p)?;
            HandlerKind::SetVisible { visible, item }
        }
        "SET_INVINCIBILITY" => HandlerKind::SetInvincibility(p.get_int()? != 0),
        "SET_VIBRATION" => HandlerKind::SetVibration(p.get_int()? != 0),
        // Conditional handlers
        "IF_VISIBLE" => {
            let is_visible = p.get_int()? != 0;
            let item = parse_optional_item(p)?;
            let handlers = parse_conditional_body(p)?;
            HandlerKind::IfVisible {
                is_visible,
                item,
                handlers,
            }
        }
        "IF_INVINCIBILITY_ON" => {
            let is_it = p.get_int()? != 0;
            let handlers = parse_conditional_body(p)?;
            HandlerKind::IfInvincibilityOn { is_it, handlers }
        }
        "IF_VIBRATION_ON" => {
            let is_it = p.get_int()? != 0;
            let handlers = parse_conditional_body(p)?;
            HandlerKind::IfVibrationOn { is_it, handlers }
        }
        "IF_DEV_FRONT_END" => {
            let is_it = p.get_int()? != 0;
            let handlers = parse_conditional_body(p)?;
            HandlerKind::IfDevFrontEnd { is_it, handlers }
        }
        "IF_STRING_ID" => {
            let string_id = p.get_token()?;
            let item = parse_optional_item(p)?;
            let handlers = parse_conditional_body(p)?;
            HandlerKind::IfStringId {
                string_id,
                item,
                handlers,
            }
        }
        other => return Err(format!("ui parse: unknown handler '{}'", other)),
    };
    Ok(Handler {
        class_name: keyword.to_string(),
        kind,
    })
}

fn parse_optional_item(p: &mut BlockParser) -> ParseResult<Option<String>> {
    if p.check_token("ITEM") {
        Ok(Some(p.get_token()?))
    } else {
        Ok(None)
    }
}

/// Conditional-handler body: either `{ <handlers> }` or a single
/// inline handler (event.cpp:62-72 `InitCustomConditional`).
fn parse_conditional_body(p: &mut BlockParser) -> ParseResult<Vec<Handler>> {
    let mut handlers = Vec::new();
    if p.peek() == Some("{") {
        p.next(); // consume `{`
        while let Some(h) = parse_handler_opt(p)? {
            handlers.push(h);
        }
        p.get_delimiter("}")?;
    } else if let Some(h) = parse_handler_opt(p)? {
        handlers.push(h);
    }
    Ok(handlers)
}

// ---------------------------------------------------------------------------
// Tests — exercised against the shipped assets
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const FRONTEND_UI: &str = "../oni2/zips/assets/Settings/rbfrontend.ui";
    const GAME_UI: &str = "../oni2/zips/assets/Settings/rbgame.ui";

    #[test]
    fn real_rbfrontend_parses() {
        let Ok(content) = std::fs::read_to_string(FRONTEND_UI) else {
            return;
        };
        let ui = parse_ui(&content).expect("rbfrontend.ui parses");
        assert_eq!(ui.preload_layout.as_deref(), Some("uitest"));
        assert_eq!(ui.string_table.as_deref(), Some("rbstrings"));
        assert!(
            ui.pages.len() >= 10,
            "expected many pages, got {}",
            ui.pages.len()
        );
        let first = &ui.pages[0];
        assert_eq!(first.class_name, "PAGE_MOVIE");
        assert_eq!(first.name, "Rockstar_Movie");
        match &first.kind {
            PageKind::PageMovie { movie } => assert_eq!(movie, "rockstarlogo.m2v"),
            _ => panic!("expected PageMovie, got {:?}", first.kind),
        }
        assert!(ui.startup_page.is_some());
    }

    #[test]
    fn real_rbgame_parses() {
        let Ok(content) = std::fs::read_to_string(GAME_UI) else {
            return;
        };
        let ui = parse_ui(&content).expect("rbgame.ui parses");
        assert!(
            ui.pages.len() >= 3,
            "expected at least 3 pages, got {}",
            ui.pages.len()
        );
        assert!(ui.in_game_page.is_some());
    }

    #[test]
    fn parses_minimal_page_2d() {
        let src = r#"
            PAGE_2D "Test_Page"
            {
                BACKGROUND "bg_test"
                TIME_TO_DISABLE_EVENTS 1.5

                ITEM_INVISIBLE "Trap"
                {
                    ON_INPUT_OK
                    {
                        GO_TO_PAGE "Next"
                    }
                }
            }
            ON_STARTUP "Test_Page"
        "#;
        let ui = parse_ui(src).expect("parses");
        assert_eq!(ui.pages.len(), 1);
        let page = &ui.pages[0];
        assert_eq!(page.class_name, "PAGE_2D");
        assert_eq!(page.name, "Test_Page");
        assert!((page.time_to_disable_events - 1.5).abs() < 1e-4);
        match &page.kind {
            PageKind::Page2D { background } => {
                assert_eq!(background.as_deref(), Some("bg_test"));
            }
            _ => panic!("wrong page kind"),
        }
        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        assert_eq!(item.class_name, "ITEM_INVISIBLE");
        assert_eq!(item.events.len(), 1);
        let ev = &item.events[0];
        assert!(matches!(ev.kind, EventKind::InputOk));
        assert_eq!(ev.handlers.len(), 1);
        match &ev.handlers[0].kind {
            HandlerKind::GoToPage(p) => assert_eq!(p, "Next"),
            _ => panic!("wrong handler"),
        }
        assert_eq!(ui.startup_page.as_deref(), Some("Test_Page"));
    }

    #[test]
    fn parses_page_3d_camera_init() {
        let src = r#"
            PAGE_3D "Main_Menu"
            {
                LAYOUT "uitest"
                CAMERA_INIT ( -1.5 1.5 3.0 ) , ( 0.0 0.2 0.0 ) , 60.0
                TIME_TO_DISABLE_EVENTS 0.1

                ITEM_RECT_3D_TEXT "Label"
                {
                    ATTACHED_TO "actor_Projector"
                    TOP_LEFT -4.375 3.2 0.0
                    BOTTOM_RIGHT 4.375 1.925 0.0
                    COLOR 255 155 0 128
                    STRING_ID "MM_New_Game"
                }
            }
        "#;
        let ui = parse_ui(src).expect("parses");
        let page = &ui.pages[0];
        match &page.kind {
            PageKind::Page3D {
                layout,
                camera_init,
            } => {
                assert_eq!(layout, "uitest");
                let c = camera_init.expect("camera");
                assert!((c.fov - 60.0).abs() < 1e-4);
                // Oni2 (-1.5, 1.5, 3.0) → Bevy (+1.5, 1.5, -3.0) after
                // the parse-boundary conversion.
                assert!((c.position.x - 1.5).abs() < 1e-4);
                assert!((c.position.y - 1.5).abs() < 1e-4);
                assert!((c.position.z + 3.0).abs() < 1e-4);
            }
            _ => panic!("wrong kind"),
        }
        let item = &page.items[0];
        match &item.kind {
            ItemKind::Rect3DText { rect, string } => {
                assert_eq!(rect.attached_to.as_deref(), Some("actor_Projector"));
                assert_eq!(rect.color, [255, 155, 0, 128]);
                // Oni2 (-4.375, 3.2, 0.0) → Bevy (+4.375, 3.2, 0.0).
                assert!((rect.top_left.x - 4.375).abs() < 1e-4);
                assert!((rect.bottom_right.x + 4.375).abs() < 1e-4);
                assert!(matches!(string, StringSource::StringId(s) if s == "MM_New_Game"));
            }
            _ => panic!("wrong item kind"),
        }
    }
}
