/*
 * oni2_loader/parsers/blk.rs — .blk block-data parser.
 *
 * `parse_blk_content` reads the text-form `.blk` files that live alongside
 * each entity's animation set (e.g. `entity.tune/kno/ANIMBLOCK_NORMAL.blk`).
 * Mirrors `crBlockData::Load` field-for-field so every value the legacy
 * engine consumes ends up on the returned `BlockDef`.
 *
 * Schema (loaded in this exact order — each optional field only matches
 * when its name is literally the next token, so the format is order-strict
 * the same way the legacy tokenizer is):
 *
 *   StartPhase  EndPhase  Rate  HeadingDegrees  WidthDegrees       (required)
 *   AnimMidPoint [AnimMidPointEnd] HoldButton                      (optional triple)
 *   BlockableGuardTypes                                            (optional u32)
 *   ReactAnim0 .. ReactAnim15                                      (optional ints, one per hit type)
 *   CounterAtk <name>                                              (optional, "" = none)
 *   AtkNoQueueThreshold .. Opp3QStart                              (optional 8-float AnimControlBlock)
 *   SuccessfulBlockAnim <name>                                     (optional, "" = none)
 *   ComboCountBeforeCausingReact                                   (optional int)
 *   AutoCounter                                                    (optional 0/1)
 *
 * Degrees → radians conversion is done at parse time so callers never deal
 * with the legacy unit. The number of hit types (`NUM_HIT_TYPES`) is fixed
 * at 16 to match the on-disk authoring data — every sample `.blk` writes
 * exactly `ReactAnim0..ReactAnim15`.
 */
use super::block_parser::BlockParser;
use crate::fight::components::{AnimControlBlock, BlockDef, BlockLibrary};
use bevy::log::info;

/// `animBlockEnum` name table — index = enum value, used to derive the
/// `<ANIMBLOCK_X>.blk` filename to look up. Order MUST match the legacy
/// `animBlockEnum` declaration: after `ANIMBLOCK_INVALID = -1`, the named
/// entries start at 0.
pub const ANIMBLOCK_NAMES: &[&str] = &[
    "ANIMBLOCK_NORMAL", // 0
    "ANIMBLOCK_SLOT_0", // 1
    "ANIMBLOCK_SLOT_1", // 2
    "ANIMBLOCK_SLOT_2", // 3
    "ANIMBLOCK_SLOT_3", // 4
    "ANIMBLOCK_SLOT_4", // 5
];

/// Reverse lookup: alias string (case-insensitive) → `animBlockEnum` index.
/// Returns `None` for non-block aliases — callers use this to gate
/// block-only state work.
pub fn block_index_for_alias(alias: &str) -> Option<i32> {
    ANIMBLOCK_NAMES
        .iter()
        .position(|name| name.eq_ignore_ascii_case(alias))
        .map(|i| i as i32)
}

/// Number of hit types `crHitTypeMgr` enumerated in the shipped game data.
/// Every authored `.blk` writes `ReactAnim0..ReactAnim15`.
pub const NUM_HIT_TYPES: usize = 16;

/// `ANIMREACT_INVALID` — the sentinel meaning "no failed-block react
/// configured for this hit type."
pub const ANIMREACT_INVALID: i32 = -1;

/// Default bitmask when `BlockableGuardTypes` is missing from the file:
/// all hit types blockable. Mirrors `HITTYPEMGR.GetDefault()` in spirit
/// (we don't have access to the actual table, but every hit type set is
/// the most permissive choice and matches how the C++ default constructor
/// initializes `BlockableHitTypes`).
pub const DEFAULT_BLOCKABLE_HIT_TYPES: u32 = 0xFFFF_FFFF;

/// Parse a `.blk` text file into a `BlockDef`. `anim_index` is the
/// `animBlockEnum` value the file was loaded for (caller knows it from
/// the load path / enum iteration). Returns `None` if the file is so
/// malformed that even the required prefix can't be read.
pub fn parse_blk_content(content: &str, anim_index: i32) -> Option<BlockDef> {
    let mut p = BlockParser::new(content);

    // Required prefix — five floats in declared order. `MatchFloat` in C++
    // demands the keyword be the next token; we use `match_float` which
    // does the same. If any of these fail the file isn't a `.blk`.
    let start_phase = p.match_float("StartPhase").ok()?;
    let end_phase = p.match_float("EndPhase").ok()?;
    let rate = p.match_float("Rate").ok()?;
    let heading_degrees = p.match_float("HeadingDegrees").ok()?;
    let width_degrees = p.match_float("WidthDegrees").ok()?;

    // Optional AnimMidPoint group. The C++ `CheckToken("AnimMidPoint",false)`
    // peeks without consuming, then `MatchFloat` reads keyword + value. The
    // peek-only behavior is important: if AnimMidPoint is absent the
    // following blocks (BlockableGuardTypes, ReactAnim*, …) need an
    // un-consumed parse position.
    let (anim_mid_point, anim_mid_point_end, max_hold_button) = if p
        .peek()
        .map(|s| s.eq_ignore_ascii_case("AnimMidPoint"))
        .unwrap_or(false)
    {
        let mp = p.match_float("AnimMidPoint").ok()?;
        // AnimMidPointEnd is optional inside the AnimMidPoint group;
        // when absent it equals AnimMidPoint (instantaneous, no loop).
        let mpe = if p
            .peek()
            .map(|s| s.eq_ignore_ascii_case("AnimMidPointEnd"))
            .unwrap_or(false)
        {
            p.match_float("AnimMidPointEnd").ok()?
        } else {
            mp
        };
        // HoldButton is REQUIRED once we entered the AnimMidPoint group
        // (C++ uses `MatchFloat` not `CheckToken`).
        let hb = p.match_float("HoldButton").ok()?;
        (mp, mpe, hb)
    } else {
        (0.5, 0.5, 0.0)
    };

    let blockable_hit_types = p
        .read_i32_opt("BlockableGuardTypes")
        .map(|v| v as u32)
        .unwrap_or(DEFAULT_BLOCKABLE_HIT_TYPES);

    let mut failed_block_react_anims = vec![ANIMREACT_INVALID; NUM_HIT_TYPES];
    for i in 0..NUM_HIT_TYPES {
        let key = format!("ReactAnim{}", i);
        if let Some(v) = p.read_i32_opt(&key) {
            failed_block_react_anims[i] = v;
        }
    }

    // CounterAtk "" means "no counter" — drop empty strings to None so
    // downstream code doesn't have to special-case the sentinel.
    let counter_atk = p.read_string_opt("CounterAtk").and_then(|s| {
        let trimmed = s.trim_matches('"');
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    // AnimControlBlock — only present when the next token literally is
    // `AtkNoQueueThreshold`. The 8 floats inside are then required in
    // sequence (C++ uses MatchFloat for each). Peek-only first so we
    // don't consume the keyword before the inner `match_float` does.
    let anim_control_block = if p
        .peek()
        .map(|s| s.eq_ignore_ascii_case("AtkNoQueueThreshold"))
        .unwrap_or(false)
    {
        Some(parse_anim_control_block(&mut p)?)
    } else {
        None
    };

    let successful_block_anim = p.read_string_opt("SuccessfulBlockAnim").and_then(|s| {
        let trimmed = s.trim_matches('"');
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    let combo_count_before_react = p.read_i32("ComboCountBeforeCausingReact", 0);
    let auto_counter = p.read_i32("AutoCounter", 0) != 0;

    Some(BlockDef {
        anim_index,
        // Degrees → radians at the parse boundary so no downstream consumer
        // ever sees Oni2-authoring units.
        heading_radians: heading_degrees.to_radians(),
        width_radians: width_degrees.to_radians(),
        start_phase,
        end_phase,
        rate,
        anim_mid_point,
        anim_mid_point_end,
        max_hold_button,
        blockable_hit_types,
        combo_count_before_react,
        auto_counter,
        failed_block_react_anims,
        counter_atk,
        successful_block_anim,
        anim_control_block: anim_control_block.unwrap_or_default(),
    })
}

/// Parse the 8-float `AnimControlBlock` window-of-opportunity sub-block.
/// Mirrors `crAtkAnimCtrlBlock::Load`. The field-name → struct-slot mapping
/// is load-bearing, NOT cosmetic: `AtkNoQueueThreshold` writes to
/// `opp2_q_start`, `AtkBeginRedirectThreshold` to `opp1_do_end`, etc.
fn parse_anim_control_block(p: &mut BlockParser) -> Option<AnimControlBlock> {
    let opp2_q_start = p.match_float("AtkNoQueueThreshold").ok()?;
    let opp1_do_end = p.match_float("AtkBeginRedirectThreshold").ok()?;
    let opp2_do_start = p.match_float("AtkEndRedirectThreshold").ok()?;
    let opp2_q_crit_start = p.match_float("Opp2CritStart").ok()?;
    let opp2_do_crit_start = p.match_float("Opp2DoCritStart").ok()?;
    let opp2_do_end = p.match_float("AtkEndRedirectLimit").ok()?;
    let opp3_do_start = p.match_float("Opp3DoStart").ok()?;
    let opp3_q_start = p.match_float("Opp3QStart").ok()?;
    Some(AnimControlBlock {
        opp1_do_end,
        opp2_q_start,
        opp2_do_start,
        opp2_q_crit_start,
        opp2_do_crit_start,
        opp2_do_end,
        opp3_q_start,
        opp3_do_start,
    })
}

/// Walk every `animBlockEnum` value for `entity_name`, attempt to load
/// `entity.tune/<entity_name>/<ANIMBLOCK_X>.blk` (with a fallback to
/// `Entity/<entity_name>/...` and to the anim-name prefix dir for entities
/// whose blocks live elsewhere), and assemble a `BlockLibrary` indexed
/// by enum value. Empty slots are filled with `BlockDef::default()` so
/// `BlockLibrary::get(i)` stays well-defined even when an actor only
/// authors a subset of the blocks.
pub fn load_block_library(entity_name: &str, entity_dir: &str) -> BlockLibrary {
    let mut blocks: Vec<BlockDef> = Vec::with_capacity(ANIMBLOCK_NAMES.len());
    let mut loaded = 0;

    for (idx, name) in ANIMBLOCK_NAMES.iter().enumerate() {
        let anim_index = idx as i32;
        let candidates = [
            format!("entity.tune/{}/{}.blk", entity_name, name),
            format!("{}/{}.blk", entity_dir, name),
        ];

        let mut def: Option<BlockDef> = None;
        for path in &candidates {
            if let Ok(content) = crate::vfs::read_to_string("", path)
                && let Some(parsed) = parse_blk_content(&content, anim_index)
            {
                def = Some(parsed);
                break;
            }
        }

        match def {
            Some(d) => {
                loaded += 1;
                blocks.push(d);
            }
            None => {
                // Placeholder so `BlockLibrary.blocks[i]` is always indexable
                // by enum value. anim_index is filled in so `get(i)` can
                // still surface the slot identity for debug purposes.
                blocks.push(BlockDef {
                    anim_index,
                    ..BlockDef::default()
                });
            }
        }
    }

    info!(
        "load_block_library({}): loaded {}/{} .blk files",
        entity_name,
        loaded,
        ANIMBLOCK_NAMES.len(),
    );
    BlockLibrary { blocks }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: ANIMBLOCK_NORMAL.blk shipped on disk parses end-to-end and
    /// every field round-trips to the value in the file.
    #[test]
    fn parses_animblock_normal() {
        let content = "
            StartPhase 0.0
            EndPhase 1.0
            Rate 1.0
            HeadingDegrees 0.0
            WidthDegrees 360.0
            AnimMidPoint 0.079994
            AnimMidPointEnd 0.919999
            HoldButton 0.319999
            BlockableGuardTypes 2047
            ReactAnim0 -1
            ReactAnim1 -1
            ReactAnim2 -1
            ReactAnim3 -1
            ReactAnim4 -1
            ReactAnim5 -1
            ReactAnim6 -1
            ReactAnim7 -1
            ReactAnim8 -1
            ReactAnim9 -1
            ReactAnim10 -1
            ReactAnim11 -1
            ReactAnim12 -1
            ReactAnim13 -1
            ReactAnim14 -1
            ReactAnim15 -1
            CounterAtk \"\"
            AtkNoQueueThreshold 0.0
            AtkBeginRedirectThreshold 0.0
            AtkEndRedirectThreshold 0.0
            Opp2CritStart 0.599993
            Opp2DoCritStart 0.599993
            AtkEndRedirectLimit 1.0
            Opp3DoStart 1.0
            Opp3QStart 1.0
            SuccessfulBlockAnim ANIMBLOCK_SLOT_0
        ";
        let def = parse_blk_content(content, 0).expect("parse failed");
        assert_eq!(def.anim_index, 0);
        assert!((def.start_phase - 0.0).abs() < 1e-6);
        assert!((def.end_phase - 1.0).abs() < 1e-6);
        assert!((def.rate - 1.0).abs() < 1e-6);
        // 360° → 2π
        assert!((def.width_radians - std::f32::consts::TAU).abs() < 1e-4);
        assert!((def.heading_radians - 0.0).abs() < 1e-6);
        assert!((def.anim_mid_point - 0.079994).abs() < 1e-4);
        assert!((def.anim_mid_point_end - 0.919999).abs() < 1e-4);
        assert!((def.max_hold_button - 0.319999).abs() < 1e-4);
        assert_eq!(def.blockable_hit_types, 2047);
        assert_eq!(def.failed_block_react_anims.len(), NUM_HIT_TYPES);
        assert!(
            def.failed_block_react_anims
                .iter()
                .all(|&v| v == ANIMREACT_INVALID)
        );
        assert!(
            def.counter_atk.is_none(),
            "empty CounterAtk should map to None"
        );
        assert_eq!(
            def.successful_block_anim.as_deref(),
            Some("ANIMBLOCK_SLOT_0")
        );
        // AnimControlBlock — Opp2CritStart 0.6 lands in opp2_q_crit_start
        assert!((def.anim_control_block.opp2_q_crit_start - 0.599993).abs() < 1e-4);
        assert!((def.anim_control_block.opp2_do_end - 1.0).abs() < 1e-6);
    }

    /// Slot block: non-empty CounterAtk, identical mid points
    /// (instantaneous hold), full 0xF7FF blockable mask.
    #[test]
    fn parses_animblock_slot_0() {
        let content = "
            StartPhase 0.0
            EndPhase 1.0
            Rate 1.0
            HeadingDegrees 0.0
            WidthDegrees 360.0
            AnimMidPoint 0.5
            AnimMidPointEnd 0.5
            HoldButton 0.439998
            BlockableGuardTypes 63487
            ReactAnim0 -1
            CounterAtk ANIMGA_COUNTER_1
            AtkNoQueueThreshold 0.0
            AtkBeginRedirectThreshold 0.0
            AtkEndRedirectThreshold 0.0
            Opp2CritStart 1.0
            Opp2DoCritStart 1.0
            AtkEndRedirectLimit 1.0
            Opp3DoStart 1.0
            Opp3QStart 1.0
            SuccessfulBlockAnim ANIMBLOCK_SLOT_0
        ";
        let def = parse_blk_content(content, 1).expect("parse failed");
        assert_eq!(def.blockable_hit_types, 63487);
        assert_eq!(def.counter_atk.as_deref(), Some("ANIMGA_COUNTER_1"));
        assert!((def.anim_mid_point - def.anim_mid_point_end).abs() < 1e-6);
    }
}
