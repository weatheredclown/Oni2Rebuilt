/*
 * animator/schedule.rs — AnimSchedule, the Rust equivalent of crAnimList.
 *
 * An AnimSchedule is a queue of animations to play in sequence on a single
 * entity.  Each entry carries its own substate-1 id, playback rate, and an
 * optional "hold at end" flag (for looping anims that wait for an external
 * trigger to advance, e.g. airborne float loops).  The tick system walks the
 * cursor, auto-advancing when the head animation reaches its final frame.
 *
 * This mirrors the core responsibilities of legacy `crAnimList`:
 *   • Queue + cursor + "current record"
 *   • Rate multiplier per record (SetRate)
 *   • Substate id per record (stamped onto crAnimPlayer.SubState1)
 *   • Auto-advance on anim end, with optional hold-at-end for looping records
 *   • End-of-list callback (`ListEmptyCB`) → here we expose a `finished` flag
 *     for observer systems to consume.
 *
 * Unlike the legacy class, we do not own the animation player — `Oni2AnimLibrary`
 * + `Oni2AnimState` remain the authority for playback.  The schedule just
 * asks the library to play the right alias and updates the speed_multiplier.
 */
use bevy::prelude::*;

use crate::oni2_loader::animation::{Oni2AnimLibrary, Oni2AnimState};

use super::components::ActionPlayer;

// ---------------------------------------------------------------------------
// AnimScheduleEntry
// ---------------------------------------------------------------------------

/// One stage of a scheduled animation sequence.
///
/// Construct with `AnimScheduleEntry::new(alias, substate_1)` and customize
/// with the builder methods before appending to an `AnimSchedule`.
#[derive(Debug, Clone)]
pub struct AnimScheduleEntry {
    /// Animation alias (e.g. "ANIMJUMP_VERTICAL_3") resolved via
    /// `Oni2AnimLibrary`.
    pub alias: String,
    /// SubState1 id stamped onto `ActionPlayer` when this entry starts — e.g.
    /// `sub_state_1::JUMP_MAIN`.  Downstream systems use this to react to
    /// animation-phase transitions.
    pub substate_1: i32,
    /// Playback speed multiplier written into `Oni2AnimState.speed_multiplier`.
    pub rate: f32,
    /// When true: loop this animation indefinitely; auto-advance is disabled.
    /// The caller must explicitly `AnimSchedule::advance()` or `clear()` to
    /// move past it.  Matches `crAnimListRecord::SetLoop(true)` semantics.
    pub hold_at_end: bool,
}

impl AnimScheduleEntry {
    pub fn new(alias: impl Into<String>, substate_1: i32) -> Self {
        AnimScheduleEntry {
            alias: alias.into(),
            substate_1,
            rate: 1.0,
            hold_at_end: false,
        }
    }

    pub fn with_rate(mut self, rate: f32) -> Self {
        self.rate = rate;
        self
    }

    pub fn with_hold(mut self) -> Self {
        self.hold_at_end = true;
        self
    }
}

// ---------------------------------------------------------------------------
// AnimSchedule component
// ---------------------------------------------------------------------------

/// A FIFO list of `AnimScheduleEntry` records — equivalent to `crAnimList`.
///
/// Insert on any entity that also has `Oni2AnimLibrary` + `Oni2AnimState` and
/// the `anim_schedule_tick_system` will advance it each FixedUpdate tick.
///
/// Lifecycle:
///   1. `AnimSchedule::new(entries)` creates the list.  The `needs_play` flag
///      is set so the tick system plays `entries[0]` on the next tick.
///   2. When the active animation reaches its final frame (and isn't
///      `hold_at_end`), the cursor advances and the next entry is played.
///   3. Once the cursor passes the end of the list, `finished` is set to true
///      and the schedule is inert until `clear()` or a new one is inserted.
///
/// If try_start_action plays the first entry itself (to avoid a one-tick gap
/// between action start and animation start), call `mark_first_played()` on
/// the returned schedule before inserting it.
#[derive(Component, Debug, Clone, Default)]
pub struct AnimSchedule {
    pub entries: Vec<AnimScheduleEntry>,
    /// Index of the entry that is currently playing (or about to play).
    pub cursor: usize,
    /// True once the cursor has walked past the last entry.
    pub finished: bool,
    /// Internal: `true` when the current cursor entry has not yet been sent
    /// to the animation library.  Cleared by the tick system after playing.
    pub(crate) needs_play: bool,
}

impl AnimSchedule {
    /// Build a new schedule from a Vec of entries.  Entries play in order,
    /// starting with `entries[0]` on the next tick.
    pub fn new(entries: Vec<AnimScheduleEntry>) -> Self {
        let has = !entries.is_empty();
        AnimSchedule {
            entries,
            cursor: 0,
            finished: !has,
            needs_play: has,
        }
    }

    /// Convenience: build from a single entry.
    pub fn single(entry: AnimScheduleEntry) -> Self {
        AnimSchedule::new(vec![entry])
    }

    /// The entry currently being played, or about-to-play.
    pub fn current(&self) -> Option<&AnimScheduleEntry> {
        self.entries.get(self.cursor)
    }

    /// Force-advance to the next entry.  Used to break out of `hold_at_end`
    /// entries or to skip ahead on an external signal (e.g. landing while the
    /// airborne FLOAT loop is playing).
    pub fn advance(&mut self) {
        if self.finished {
            return;
        }
        self.cursor += 1;
        if self.cursor >= self.entries.len() {
            self.finished = true;
        } else {
            self.needs_play = true;
        }
    }

    /// Drop the entire queue.  Safe to call when the schedule is already
    /// finished.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = 0;
        self.finished = true;
        self.needs_play = false;
    }

    /// Flag the first entry as already-played (for callers that start the
    /// animation themselves to avoid a one-tick gap).  The tick system will
    /// wait for the current animation to end before advancing.
    pub fn mark_first_played(&mut self) {
        self.needs_play = false;
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() || self.finished
    }
}

// ---------------------------------------------------------------------------
// anim_schedule_tick_system
// ---------------------------------------------------------------------------

/// Per-tick driver for every entity with an `AnimSchedule`:
///
///   • If `needs_play` is set (fresh entry), ask the library to play its
///     alias and write the entry's `rate`/`substate_1`/`hold_at_end` into the
///     animation state.
///   • Otherwise, if the current animation has reached its final frame and
///     the entry isn't `hold_at_end`, advance the cursor.  The next tick
///     picks up the newly-current entry via the `needs_play` branch.
///   • Once the cursor walks off the end, mark the schedule `finished` and
///     stop doing work.
pub fn anim_schedule_tick_system(
    mut query: Query<(
        Entity,
        &mut AnimSchedule,
        &Oni2AnimLibrary,
        &mut Oni2AnimState,
        Option<&mut ActionPlayer>,
    )>,
) {
    for (_entity, mut sched, lib, mut state, ap_opt) in &mut query {
        if sched.finished {
            continue;
        }

        if sched.needs_play {
            // Start the entry at the cursor.
            let entry_opt = sched.current().cloned();
            match entry_opt {
                Some(entry) => {
                    if lib.play(&entry.alias, &mut state) {
                        state.speed_multiplier = entry.rate;
                        state.looping = entry.hold_at_end;
                        if let Some(mut ap) = ap_opt {
                            ap.record_new_substate_1(entry.substate_1);
                        }
                    } else {
                        warn!(
                            "anim_schedule: alias '{}' not in library — skipping entry {}",
                            entry.alias, sched.cursor
                        );
                    }
                    sched.needs_play = false;
                }
                None => {
                    sched.finished = true;
                    sched.needs_play = false;
                }
            }
            continue;
        }

        // Auto-advance on anim-end (non-looping).
        let at_end = {
            let total = (state.anim.num_frames as f32 - 1.0).max(0.0);
            !state.looping && state.current_time >= total && total > 0.0
        };
        let hold = sched.current().is_some_and(|e| e.hold_at_end);
        if at_end && !hold {
            sched.cursor += 1;
            if sched.cursor >= sched.entries.len() {
                sched.finished = true;
            } else {
                sched.needs_play = true;
            }
        }
    }
}
