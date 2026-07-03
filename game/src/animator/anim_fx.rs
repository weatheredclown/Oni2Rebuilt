/*
 * animator/anim_fx.rs — rbAnimFXData: FX tied to animation phases.
 *
 * Each Oni2Animation carries a list of AnimFxEntry values.  When the
 * animation's normalized phase (current_time / (num_frames-1)) crosses an
 * entry's `phase` threshold (ascending, once per play-cycle), the runtime
 * emits a SpawnFx event for every fx name in that entry.
 *
 * Mirrors the legacy animation effects node system:
 *
 *     ANIM_FX {
 *         ANIMATION_INDEX  <int>       // (ignored in Rust — entries are
 *                                      //  stored per-animation already)
 *         ANIMATION_PHASE  <float>     // 0..1 fire threshold
 *         FX_SET / FX_LIST <block>     // one or more fx names
 *     }
 *
 * The AnimFxTracker component carries the fired-state bitmask + last phase
 * so we can detect rising-edge crossings across ticks and reset on anim swap.
 */
use bevy::prelude::*;

use crate::fx_system::SpawnFx;
use crate::oni2_loader::animation::{AnimId, Oni2AnimState};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// One entry from an ANIM_FX block — fires `fx_names` when the animation's
/// phase crosses `phase`.  Stored directly on `Oni2Animation.anim_fx`.
#[derive(Debug, Clone, Default)]
pub struct AnimFxEntry {
    /// Normalized phase in [0.0, 1.0] at which this entry fires.
    pub phase: f32,
    /// One or more FX names — each resolved against the FxLibrary and spawned
    /// as its own fxEffect (mirrors fxEffectSet::PlayEffects).
    pub fx_names: Vec<String>,
}

/// Runtime tracker for an entity playing animations with anim_fx entries.
/// Automatically inserted/updated by `anim_fx_system`.
#[derive(Component, Debug, Clone)]
pub struct AnimFxTracker {
    /// AnimId of the animation the tracker is currently following.
    current_anim: Option<AnimId>,
    /// Normalized phase at the end of the previous tick.  Used to detect
    /// rising-edge crossings ( prev < entry.phase <= current ).
    previous_phase: f32,
    /// Bitmask of entries already fired this play-cycle (max 64 per anim).
    fired_mask: u64,
}

impl Default for AnimFxTracker {
    fn default() -> Self {
        AnimFxTracker {
            current_anim: None,
            previous_phase: 0.0,
            fired_mask: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Parser (ANIM_FX blocks)
// ---------------------------------------------------------------------------

/// Parse a single ANIM_FX block body (already positioned inside the outer
/// `{`).  Consumes tokens up to and including the closing `}`.  Used by any
/// loader that wants to hydrate an `Oni2Animation.anim_fx`.
///
/// The on-disk format in the original Oni2 build accepts both `FX_SET` and `FX_LIST`
/// keywords for the effect block — we treat them identically.
pub fn parse_anim_fx_block(
    p: &mut crate::oni2_loader::parsers::block_parser::BlockParser,
) -> AnimFxEntry {
    let mut entry = AnimFxEntry::default();

    while !p.endblock() {
        let key = p.peek().unwrap_or("").to_lowercase();
        let a_key = p.peek().unwrap_or("").to_string();
        if key == "end" {
            p.next();
            break;
        }
        match key.as_str() {
            "animation_index" => {
                // Per-animation tables already encode the index; we still
                // consume the value to stay in sync with the stream.
                let _ = p.read_i32(&a_key, 0);
            }
            "animation_phase" => {
                entry.phase = p.read_float(&a_key, 0.0).clamp(0.0, 1.0);
            }
            "fx_set" | "fx_list" => {
                // Either a single inline name or a nested { FX { FXType "..." } ... }
                // block — the cheap path extracts whatever string tokens we
                // find inside the block.  Consumers that need the full set
                // should pre-resolve against FxLibrary.
                p.next(); // consume the keyword
                if p.start_anonymous() {
                    while !p.endblock() {
                        let inner = p.peek().unwrap_or("").to_lowercase();
                        if inner == "end" {
                            p.next();
                            break;
                        }
                        if inner == "fx" || inner == "fxtype" {
                            p.next(); // consume "FX" / "FXType"
                            if let Some(name) = p.next() {
                                entry.fx_names.push(name);
                            }
                        } else {
                            p.next();
                        }
                    }
                } else if let Some(name) = p.next() {
                    entry.fx_names.push(name);
                }
            }
            _ => {
                p.next();
            }
        }
    }
    entry
}

/// Parse a complete top-level stream of zero-or-more ANIM_FX blocks into a Vec
/// of entries.  Returns the entries; callers attach them to the relevant
/// `Oni2Animation.anim_fx`.
pub fn parse_anim_fx_content(content: &str) -> Vec<AnimFxEntry> {
    let mut entries = Vec::new();
    let mut p = crate::oni2_loader::parsers::block_parser::BlockParser::new(content);

    if p.peek() == Some("{") {
        p.next();
    }

    while !p.endblock() {
        if p.peek().map(|s| s.eq_ignore_ascii_case("ANIM_FX")) != Some(true) {
            p.next();
            continue;
        }
        p.next(); // consume ANIM_FX
        if p.start_anonymous() {
            entries.push(parse_anim_fx_block(&mut p));
        }
    }

    entries
}

// ---------------------------------------------------------------------------
// Runtime system
// ---------------------------------------------------------------------------

/// Walks every entity with an Oni2AnimState and fires SpawnFx events for any
/// AnimFxEntry whose phase threshold was crossed this tick.  Mirrors
/// `rbAnimFXData::Update(animPlayer, matrix, health)` — we use `entity`
/// position for the spawn transform and parent the FX to the entity so it
/// tracks movement.
///
/// Runs in FixedUpdate after the animation advance (handled by
/// update_oni2_animation in Update, but the phase check here is tick-rate
/// stable since we use current_time directly).
pub fn anim_fx_system(
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &Oni2AnimState,
        &Transform,
        Option<&mut AnimFxTracker>,
    )>,
) {
    for (entity, anim_state, tf, tracker_opt) in &mut query {
        // No FX on this animation? Skip cheaply.
        if anim_state.anim.anim_fx.is_empty() {
            continue;
        }

        // Compute current normalized phase.
        let phase = if anim_state.anim.num_frames > 1 {
            (anim_state.current_time / (anim_state.anim.num_frames as f32 - 1.0).max(1.0))
                .clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Ensure a tracker exists.
        let mut tracker = match tracker_opt {
            Some(t) => t,
            None => {
                commands.entity(entity).insert(AnimFxTracker::default());
                continue; // let next tick run with the tracker in place
            }
        };

        // Reset if the animation changed (new anim or looped back).
        let anim_changed = tracker.current_anim != anim_state.current_anim_id;
        let looped_back = !anim_changed && phase + 1e-4 < tracker.previous_phase;
        if anim_changed || looped_back {
            tracker.current_anim = anim_state.current_anim_id;
            tracker.previous_phase = 0.0;
            tracker.fired_mask = 0;
        }

        let prev = tracker.previous_phase;

        // Fire every entry whose phase sits in (prev, phase] and hasn't
        // already fired this cycle.  Cap at 64 entries (bitmask width);
        // anything beyond never fires — matches the typical "couple of hits
        // per anim" rbAnimFXData usage.
        let max_entries = anim_state.anim.anim_fx.len().min(64);
        for (idx, entry) in anim_state.anim.anim_fx.iter().enumerate().take(max_entries) {
            let bit = 1u64 << idx;
            if tracker.fired_mask & bit != 0 {
                continue;
            }
            let crossed = entry.phase > prev && entry.phase <= phase;
            // Edge case: entry.phase == 0.0 on the first tick after an anim
            // change should still fire (prev starts at 0.0).
            let at_start = entry.phase <= 0.0 && prev <= 0.0 && anim_state.has_anim();
            if !crossed && !at_start {
                continue;
            }
            for fx_name in &entry.fx_names {
                commands.trigger(SpawnFx {
                    name: fx_name.clone(),
                    at: Some(tf.translation),
                    parent: Some(entity),
                    start_active: true,
                });
            }
            tracker.fired_mask |= bit;
        }

        tracker.previous_phase = phase;
    }
}
