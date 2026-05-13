/*
 * oni2_loader/parsers/gait.rs — .gait file parser.
 *
 * Format: plain-text, two keyed integers.
 *   Axis: <int>       ; ignored (legacy orientation hint)
 *   Normalize: <int>  ; 0=None, 1=Walk (deprecated), 2=Root, 3=Tracker (deprecated)
 *
 * The C++ reference (legacy `animAnimatorType`) uses this
 * value to decide how to condition per-anim root motion at load time:
 *   0 (None)    — no processing; sanity-check that frame delta is zero.
 *   1 (Walk)    — asserts in C++ ("no longer handled by RB code").
 *   2 (Root)    — if the anim is NOT pre-normalized, compute per-frame root
 *                 deltas and strip translation from the channel data.  If
 *                 pre-normalized, the anim file already stores deltas
 *                 separately and needs no further work.
 *   3 (Tracker) — asserts in C++ (dead path).
 *
 * Our port parses the file so we can gate root-translation stripping on the
 * Normalize value (see animation.rs) and warn loudly for 1/3 so any new
 * anims that land with those values are caught early.
 */
use bevy::prelude::*;

/// Root-motion conditioning mode declared in a `.gait` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaitNormalize {
    /// No processing.  Frame-0→frame-1 translation delta should be zero.
    None,
    /// DEPRECATED — "no longer handled by RB code" in the C++ reference.
    /// If seen, the anim won't animate correctly.
    Walk,
    /// Strip root XZ translation from channel data; deltas are either
    /// pre-baked into the anim file or computed at load time (we currently
    /// only strip; delta extraction is a known gap).
    Root,
    /// DEPRECATED — dead path in the C++ reference.
    Tracker,
}

impl GaitNormalize {
    /// Map the raw int out of the .gait file to the enum.  Unknown values
    /// fall back to `None` with a warning so we don't corrupt load state.
    pub fn from_int(v: i32, filename: &str) -> Self {
        match v {
            0 => GaitNormalize::None,
            1 => {
                warn!(
                    "gait '{}': Normalize=1 (Walk) — this mode is deprecated \
                     in the engine and unsupported in our port.  The anim will \
                     likely animate incorrectly.",
                    filename
                );
                GaitNormalize::Walk
            }
            2 => GaitNormalize::Root,
            3 => {
                warn!(
                    "gait '{}': Normalize=3 (Tracker) — this mode was dead code \
                     (asserts) in the engine and is unsupported in our port.",
                    filename
                );
                GaitNormalize::Tracker
            }
            other => {
                warn!(
                    "gait '{}': Normalize={} — unknown value; defaulting to None.",
                    filename, other
                );
                GaitNormalize::None
            }
        }
    }

    /// True iff this mode should strip root XZ translation from channel
    /// data during playback.  Only `Root` qualifies.
    pub fn should_strip_root(self) -> bool {
        matches!(self, GaitNormalize::Root)
    }
}

/// Parsed `.gait` content.
#[derive(Debug, Clone, Copy)]
pub struct GaitData {
    pub normalize: GaitNormalize,
    /// Legacy axis hint — captured for completeness; not consulted.
    pub axis: i32,
}

/// Parse the (very small) .gait text format.  Returns `None` only on
/// complete garbage — missing keys are logged and absent values default to
/// zero.  Keys are case-insensitive; whitespace-tolerant.
pub fn parse_gait(content: &str, filename: &str) -> Option<GaitData> {
    let mut axis: Option<i32> = None;
    let mut normalize_raw: Option<i32> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        if key.eq_ignore_ascii_case("Axis") {
            axis = value.parse::<i32>().ok();
        } else if key.eq_ignore_ascii_case("Normalize") {
            normalize_raw = value.parse::<i32>().ok();
        }
    }

    if axis.is_none() && normalize_raw.is_none() {
        return None;
    }

    let normalize = normalize_raw
        .map(|v| GaitNormalize::from_int(v, filename))
        .unwrap_or(GaitNormalize::None);

    Some(GaitData {
        normalize,
        axis: axis.unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_root_normalize() {
        let g = parse_gait("Axis: 2\nNormalize: 2\n", "test.gait").unwrap();
        assert_eq!(g.normalize, GaitNormalize::Root);
        assert_eq!(g.axis, 2);
        assert!(g.normalize.should_strip_root());
    }

    #[test]
    fn parses_none() {
        let g = parse_gait("Axis: 0\nNormalize: 0\n", "test.gait").unwrap();
        assert_eq!(g.normalize, GaitNormalize::None);
        assert!(!g.normalize.should_strip_root());
    }

    #[test]
    fn parses_deprecated_walk() {
        // Should parse and produce Walk (with a warning — not asserted here).
        let g = parse_gait("Axis: 0\nNormalize: 1\n", "test.gait").unwrap();
        assert_eq!(g.normalize, GaitNormalize::Walk);
        assert!(!g.normalize.should_strip_root());
    }

    #[test]
    fn parses_deprecated_tracker() {
        let g = parse_gait("Axis: 0\nNormalize: 3\n", "test.gait").unwrap();
        assert_eq!(g.normalize, GaitNormalize::Tracker);
    }

    #[test]
    fn case_insensitive_keys() {
        let g = parse_gait("axis: 0\nnormalize: 2\n", "test.gait").unwrap();
        assert_eq!(g.normalize, GaitNormalize::Root);
    }

    #[test]
    fn missing_normalize_defaults_to_none() {
        let g = parse_gait("Axis: 1\n", "test.gait").unwrap();
        assert_eq!(g.normalize, GaitNormalize::None);
        assert_eq!(g.axis, 1);
    }

    #[test]
    fn empty_content_returns_none() {
        assert!(parse_gait("", "test.gait").is_none());
    }
}
