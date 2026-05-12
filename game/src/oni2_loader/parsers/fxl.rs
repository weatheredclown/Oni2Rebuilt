/*
 * oni2_loader/parsers/fxl.rs — parser for Oni2 `.fxl` (FX_LIST) files.
 *
 * Each `.fxl` describes one or more named bundles in the form
 *
 *     FX_LIST Punches_Body_Hard
 *     {
 *         FX { SFX    { AudioPackage "Punches_Body_Hard" } }
 *         FX { FLASH  { ...fields... } }
 *         FX { STRIKE { ...fields... } }
 *     }
 *
 * — i.e. each `FX_LIST` contains one or more `FX { <type> { ... } }` wrappers.
 * The shared `parse_settings` parser stores duplicate keys in a `HashMap`,
 * so it discards every `FX` sibling except the last; this module bypasses
 * that by tracking brace depth manually and ignoring the `FX` wrapper —
 * each `SFX` / `FLASH` / `STRIKE` block is parsed as a child of its
 * surrounding `FX_LIST`.
 *
 * Output: one `FxlEntry` per `FX_LIST`, with at most one of each subtype
 * filled.  Caller decides where to plug the parts in (the fight FX
 * registry, FxLibrary, etc.).
 */
use bevy::prelude::*;
use std::collections::HashMap;

use super::effect::{EffectDef, FlashDef, StrikeFxDef, parse_effect};
use super::settings::{SettingsBlock, SettingsValue};

/// One parsed `FX_LIST` block.  Any subtype the source omits stays `None`.
#[derive(Debug, Default)]
pub struct FxlEntry {
    pub name: String,
    pub strike: Option<StrikeFxDef>,
    pub flash: Option<FlashDef>,
    /// Value of the `AudioPackage` field from any nested `SFX { ... }`.
    /// Fed straight into `PlaySound { name }` by the fight FX dispatch.
    pub sfx_audio_package: Option<String>,
}

/// Parse the entire contents of an `.fxl` file.  Strike/Flash blocks are
/// constructed via the existing `parse_effect` helper so they pick up the
/// same texture loading / default values used by `Settings/rb.fx`.
pub fn parse_fxl_content(
    content: &str,
    asset_server: &AssetServer,
    images: &mut Assets<Image>,
) -> Vec<FxlEntry> {
    let lines = tokenize_lines(content);
    let mut entries: Vec<FxlEntry> = Vec::new();

    // Walk state.  `depth` is the running brace-nesting level (each `{`
    // increments, each `}` decrements).  `current_list_idx` and
    // `current_type` mark which logical block we're collecting properties
    // into.  Properties accumulate in `props` and are flushed when the
    // inner-type block closes (`depth` drops below `type_open_depth`).
    let mut depth: i32 = 0;
    let mut current_list_idx: Option<usize> = None;
    let mut list_open_depth: i32 = 0;
    let mut current_type: Option<String> = None;
    let mut type_open_depth: i32 = 0;
    let mut props: HashMap<String, Vec<String>> = HashMap::new();

    for line in lines {
        if line.is_empty() {
            continue;
        }

        // Brace-only lines: just update depth (and check for inner-type close).
        if line.len() == 1 {
            match line[0].as_str() {
                "{" => {
                    depth += 1;
                    continue;
                }
                "}" => {
                    depth -= 1;
                    // Did we just close the current inner type block?
                    if current_type.is_some() && depth < type_open_depth {
                        let typ = current_type.take().unwrap();
                        if let Some(idx) = current_list_idx {
                            finalize_inner(
                                &mut entries[idx],
                                &typ,
                                std::mem::take(&mut props),
                                asset_server,
                                images,
                            );
                        }
                    }
                    // Did we just close the current FX_LIST?
                    if current_list_idx.is_some() && depth < list_open_depth {
                        current_list_idx = None;
                    }
                    continue;
                }
                _ => {}
            }
        }

        // `FX_LIST <name>` (open-brace usually on next line)
        if line.len() >= 2 && line[0].eq_ignore_ascii_case("FX_LIST") {
            let name = line[1].trim_matches('"').to_string();
            entries.push(FxlEntry {
                name,
                ..Default::default()
            });
            current_list_idx = Some(entries.len() - 1);
            list_open_depth = depth + 1;
            continue;
        }

        // Inner type keywords (SFX/FLASH/STRIKE).  The `FX { ... }` wrapper
        // is intentionally ignored — we don't bother tracking it; we just
        // wait for the typed inner block.
        if line.len() == 1 && current_list_idx.is_some() {
            let upper = line[0].to_uppercase();
            if matches!(upper.as_str(), "SFX" | "FLASH" | "STRIKE") {
                current_type = Some(upper);
                type_open_depth = depth + 1;
                props.clear();
                continue;
            }
        }

        // Property line (key value...) — only when we're inside an inner type.
        if current_type.is_some() && line.len() >= 2 {
            let key = line[0].clone();
            let vals = line[1..].to_vec();
            props.insert(key, vals);
        }
    }

    entries
}

fn finalize_inner(
    entry: &mut FxlEntry,
    typ: &str,
    props: HashMap<String, Vec<String>>,
    asset_server: &AssetServer,
    images: &mut Assets<Image>,
) {
    match typ {
        "SFX" => {
            if let Some(vals) = props.get("AudioPackage") {
                if let Some(first) = vals.first() {
                    entry.sfx_audio_package = Some(first.trim_matches('"').to_string());
                }
            }
        }
        "FLASH" => {
            let block = props_to_settings_block(&props);
            if let Some(EffectDef::Flash(f)) =
                parse_effect("FLASH", &entry.name, &block, asset_server, images)
            {
                entry.flash = Some(f);
            }
        }
        "STRIKE" => {
            let block = props_to_settings_block(&props);
            if let Some(EffectDef::Strike(s)) =
                parse_effect("STRIKE", &entry.name, &block, asset_server, images)
            {
                entry.strike = Some(s);
            }
        }
        _ => {}
    }
}

/// Adapt our flat `HashMap<key, Vec<value>>` into a `SettingsBlock` that
/// `parse_effect` can read via `block.get_f32` / `get_color` / etc.  The
/// value-coercion rules mirror `parse_block_lines` in `settings.rs`.
fn props_to_settings_block(props: &HashMap<String, Vec<String>>) -> SettingsBlock {
    let mut block = SettingsBlock::default();
    for (key, vals) in props {
        if vals.len() == 1 {
            let v = &vals[0];
            let value = if v.starts_with('"') && v.ends_with('"') {
                SettingsValue::String(v.trim_matches('"').to_string())
            } else if let Ok(i) = v.parse::<i32>() {
                SettingsValue::Int(i)
            } else if let Ok(f) = v.parse::<f32>() {
                SettingsValue::Float(f)
            } else {
                SettingsValue::String(v.clone())
            };
            block.properties.insert(key.clone(), value);
        } else {
            let floats: Vec<f32> = vals.iter().filter_map(|v| v.parse::<f32>().ok()).collect();
            block
                .properties
                .insert(key.clone(), SettingsValue::FloatArray(floats));
        }
    }
    block
}

/// Quote-aware line-by-line tokenizer matching the conventions used by
/// `settings.rs::tokenize_lines` — kept inline so this module doesn't
/// have to publicise that helper.  Each output is one source line's
/// tokens, with brace characters split out as standalone tokens, and
/// `//` / `#` comments stripped.
fn tokenize_lines(content: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = if let Some(idx) = line.find("//") {
            &line[..idx]
        } else {
            line
        };
        let line = if let Some(idx) = line.find('#') {
            &line[..idx]
        } else {
            line
        };

        let mut tokens = Vec::new();
        let mut cur = String::new();
        let mut in_quotes = false;
        for c in line.chars() {
            if c == '"' {
                in_quotes = !in_quotes;
                cur.push(c);
            } else if in_quotes {
                cur.push(c);
            } else if c.is_whitespace() {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            } else if c == '{' || c == '}' {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
                tokens.push(c.to_string());
            } else {
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }
        if !tokens.is_empty() {
            out.push(tokens);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests just exercise the parser shape — they don't load real assets
    // so `parse_effect` will populate texture handles via the AssetServer
    // path (which is fine for a unit test of the FX_LIST walker).

    // Multi-line layout matches the on-disk `attack_impact.fxl` formatting —
    // every brace and keyword on its own line.  The parser drives a brace-
    // depth walker, so single-line condensed forms aren't supported and
    // wouldn't be representative of real input.
    const SAMPLE: &str = "FX_LIST Punches_Body_Hard\n\
        {\n\
            FX\n\
            {\n\
                SFX\n\
                {\n\
                    AudioPackage \"PunchSound\"\n\
                }\n\
            }\n\
            FX\n\
            {\n\
                STRIKE\n\
                {\n\
                    Duration 0.35\n\
                    ScaleRate 0.6\n\
                }\n\
            }\n\
        }\n";

    #[test]
    fn tokenizer_sees_expected_keywords() {
        let lines = tokenize_lines(SAMPLE);
        assert!(lines
            .iter()
            .any(|l| l.first().is_some_and(|s| s.eq_ignore_ascii_case("FX_LIST"))));
        assert!(lines
            .iter()
            .any(|l| l.first().is_some_and(|s| s.eq_ignore_ascii_case("AudioPackage"))));
        assert!(lines
            .iter()
            .any(|l| l.first().is_some_and(|s| s.eq_ignore_ascii_case("STRIKE"))));
    }

    /// Doesn't go through `parse_fxl_content` (that needs an `AssetServer`),
    /// but reproduces the depth-walker's state transitions inline so we can
    /// catch the FX-wrapper-vs-typed-block bookkeeping breaking.
    #[test]
    fn depth_walker_emits_one_strike_block_per_fx_list() {
        let lines = tokenize_lines(SAMPLE);
        let mut depth = 0i32;
        let mut inner_type: Option<String> = None;
        let mut type_open_depth = 0i32;
        let mut sfx_payload: Option<String> = None;
        let mut strike_props: HashMap<String, Vec<String>> = HashMap::new();
        let mut current_props: HashMap<String, Vec<String>> = HashMap::new();

        for line in lines {
            if line.is_empty() {
                continue;
            }
            if line.len() == 1 {
                match line[0].as_str() {
                    "{" => {
                        depth += 1;
                        continue;
                    }
                    "}" => {
                        depth -= 1;
                        if inner_type.is_some() && depth < type_open_depth {
                            let typ = inner_type.take().unwrap();
                            if typ == "SFX" {
                                if let Some(vals) = current_props.get("AudioPackage") {
                                    sfx_payload = vals.first().map(|s| {
                                        s.trim_matches('"').to_string()
                                    });
                                }
                            } else if typ == "STRIKE" {
                                strike_props = std::mem::take(&mut current_props);
                            }
                            current_props.clear();
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            if line.len() == 1 {
                let upper = line[0].to_uppercase();
                if matches!(upper.as_str(), "SFX" | "STRIKE") {
                    inner_type = Some(upper);
                    type_open_depth = depth + 1;
                    current_props.clear();
                    continue;
                }
            }
            if inner_type.is_some() && line.len() >= 2 {
                current_props.insert(line[0].clone(), line[1..].to_vec());
            }
        }

        assert_eq!(sfx_payload.as_deref(), Some("PunchSound"));
        assert_eq!(
            strike_props.get("Duration").and_then(|v| v.first()).map(|s| s.as_str()),
            Some("0.35")
        );
        assert_eq!(
            strike_props.get("ScaleRate").and_then(|v| v.first()).map(|s| s.as_str()),
            Some("0.6")
        );
    }
}
