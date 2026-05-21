/*
 * combat/hitbox.rs — geometric hit-test utilities.
 *
 * cone_hit_test: returns true if a target position falls within a cone defined
 * by origin, forward direction, half-angle, and range.  Used by hit_detection_system
 * as a fast first-pass before the full ATDT cylinder-slice check.
 */
use bevy::prelude::*;

/// Cone-shaped hit test.
///
/// Returns true if `target_pos` is within a cone defined by:
/// - `origin`: base of the cone
/// - `direction`: normalized forward direction
/// - `half_angle_rad`: half-angle of the cone in radians
/// - `range`: maximum reach distance
pub fn cone_hit_test(
    origin: Vec3,
    direction: Vec3,
    half_angle_rad: f32,
    range: f32,
    target_pos: Vec3,
) -> bool {
    let to_target = target_pos - origin;
    let distance = to_target.length();

    if distance > range || distance < 0.01 {
        return false;
    }

    let to_target_norm = to_target / distance;
    let dot = direction.normalize().dot(to_target_norm);
    let angle = dot.acos();

    angle <= half_angle_rad
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn target_in_front_within_cone() {
        assert!(cone_hit_test(
            Vec3::ZERO,
            Vec3::NEG_Z,
            PI / 4.0,
            5.0,
            Vec3::new(0.0, 0.0, -3.0),
        ));
    }

    #[test]
    fn target_behind_outside_cone() {
        assert!(!cone_hit_test(
            Vec3::ZERO,
            Vec3::NEG_Z,
            PI / 4.0,
            5.0,
            Vec3::new(0.0, 0.0, 3.0),
        ));
    }

    #[test]
    fn target_out_of_range() {
        assert!(!cone_hit_test(
            Vec3::ZERO,
            Vec3::NEG_Z,
            PI / 4.0,
            2.0,
            Vec3::new(0.0, 0.0, -3.0),
        ));
    }

    // --- EvaluatedWedge geometry tests --------------------------------------
    //
    // These guard against the recurring "fight collision is nowhere near the
    // player" class of bug.  Each test fabricates an `AtdtStrike` with the
    // bare-minimum geometry fields, runs `EvaluatedWedge::evaluate`, and
    // asserts `contains_target` with concrete Vec3 positions.
    //
    // Coordinate sanity: attacker at world origin, attacker_forward = -Z
    // (Bevy default forward).  All angles in Bevy space (slice fields are
    // post-`oni2_to_bevy_yaw_rads` + `wrap_angle_to_pi` from the parser).

    use crate::oni2_loader::parsers::atdt::AtdtStrike;

    fn forward_punch_strike() -> AtdtStrike {
        AtdtStrike {
            framenum: 0.0,
            frameduration: 1.0, // active across the whole 1-frame test
            reactdiskradius: 2.0,
            reactdiskheight: 1.0,
            reactdiskheighttolerance: 1.0,
            slicestartradians: -PI / 6.0, // -30°
            sliceendradians: PI / 6.0,    // +30° (60° wedge centered on forward)
            sliceheadingradiansb: 0.0,
            ..AtdtStrike::default()
        }
    }

    /// Synthetic forward punch with NON-ZERO vanishingpoint, mirroring the
    /// real ANIMATTACK_PUNCH_COMBO2 shape (vp ≈ 3.81, outer ≈ 4.83, narrow
    /// forward arc).  The whole point of this fixture is that vp != 0, so
    /// flipping the pivot-shift sign is observable.
    fn forward_punch_strike_with_vp() -> AtdtStrike {
        AtdtStrike {
            framenum: 0.0,
            frameduration: 1.0,
            reactdiskradius: 4.0,
            minreactdiskradius: 4.0,
            reactdiskheight: 1.0,
            reactdiskheighttolerance: 1.0,
            slicestartradians: -0.1,
            sliceendradians: 0.1,
            sliceheadingradiansb: 0.0,
            vanishingpoint: 3.0,
            ..AtdtStrike::default()
        }
    }

    fn back_kick_strike_after_wrap() -> AtdtStrike {
        // Reproduces `kno_atk_comb_bak_k_bskckLH.atdt` after the parser fix.
        // Original Oni2 values: slicestart -3.3847, sliceend -3.1847.
        // After oni2_to_bevy_yaw_rads (negate): +3.3847, +3.1847.
        // After wrap_angle_to_pi:  ≈ -2.8985, -3.0985.  (Both back-side, one
        // either side of -π.)  The min/max canonicalization in the parser
        // then puts start = -3.0985, end = -2.8985 — a ~11° wedge directly
        // behind the attacker.
        AtdtStrike {
            framenum: 0.0,
            frameduration: 1.0,
            reactdiskradius: 7.04,
            reactdiskheight: 1.0,
            reactdiskheighttolerance: 1.0,
            slicestartradians: -3.0985,
            sliceendradians: -2.8985,
            sliceheadingradiansb: 0.0,
            vanishingpoint: 5.28,
            ..AtdtStrike::default()
        }
    }

    #[test]
    fn forward_punch_hits_target_in_front() {
        let strike = forward_punch_strike();
        let wedge = super::EvaluatedWedge::evaluate(&strike, Vec3::ZERO, Vec3::NEG_Z, 0.5, 1.0);
        assert!(wedge.is_active);
        // Target 1m in front — should be inside the 60° wedge.
        assert!(wedge.contains_target(Vec3::new(0.0, 1.0, -1.0), 0.5, 1.5));
    }

    #[test]
    fn forward_punch_misses_target_behind() {
        let strike = forward_punch_strike();
        let wedge = super::EvaluatedWedge::evaluate(&strike, Vec3::ZERO, Vec3::NEG_Z, 0.5, 1.0);
        assert!(!wedge.contains_target(Vec3::new(0.0, 1.0, 1.5), 0.5, 1.5));
    }

    #[test]
    fn forward_punch_misses_target_far_to_side() {
        let strike = forward_punch_strike();
        let wedge = super::EvaluatedWedge::evaluate(&strike, Vec3::ZERO, Vec3::NEG_Z, 0.5, 1.0);
        // Target is within the 2m radius but at 90° to the side — outside
        // the ±30° angular bound.
        assert!(!wedge.contains_target(Vec3::new(1.5, 1.0, 0.0), 0.5, 1.5));
    }

    #[test]
    fn forward_punch_misses_target_out_of_range() {
        let strike = forward_punch_strike();
        let wedge = super::EvaluatedWedge::evaluate(&strike, Vec3::ZERO, Vec3::NEG_Z, 0.5, 1.0);
        // Target dead-ahead but at 5m, beyond the 2m reactdiskradius.
        assert!(!wedge.contains_target(Vec3::new(0.0, 1.0, -5.0), 0.5, 1.5));
    }

    #[test]
    fn back_kick_after_wrap_hits_target_behind() {
        // Back-kick wedge: slicestart=-3.0985, sliceend=-2.8985, vp=5.28,
        // outer=7.04.  Geometry:
        //   csh             = -2.9985
        //   slice_heading   = R_y(csh) * (0,0,-1) ≈ (0.143, 0, 0.989)
        //                                          (back-and-slightly-right)
        //   pivot           = attacker - slice_heading * 5.28
        //                   ≈ (-0.755, 0, -5.222)   (5.22m FORWARD of attacker)
        //   reach           = outer - vp = 1.76m
        //   active hit zone = 0..1.76m BACK of attacker, narrow ±~6° around
        //                     slice_heading direction.
        // A target ~0.71m behind the attacker (slightly right) lands in the
        // wedge midpoint — exactly what a back kick should connect with.
        let strike = back_kick_strike_after_wrap();
        let wedge = super::EvaluatedWedge::evaluate(&strike, Vec3::ZERO, Vec3::NEG_Z, 0.5, 1.0);
        assert!(wedge.is_active);
        assert!(
            wedge.contains_target(Vec3::new(0.10, 1.0, 0.71), 0.5, 1.5),
            "back kick wedge should connect with a target slightly back-and-right"
        );
    }

    #[test]
    fn back_kick_misses_target_in_front() {
        // A target in front of the attacker is INSIDE the inner cut — the
        // pivot is itself 5.22m forward and the inner radius is 5.28m, so
        // anything between the attacker and the pivot falls inside the
        // vp exclusion sphere.
        let strike = back_kick_strike_after_wrap();
        let wedge = super::EvaluatedWedge::evaluate(&strike, Vec3::ZERO, Vec3::NEG_Z, 0.5, 1.0);
        assert!(!wedge.contains_target(Vec3::new(0.0, 1.0, -3.0), 0.5, 1.5));
    }

    #[test]
    fn back_kick_misses_target_too_far_behind() {
        // The reach is only ~1.76m back; a target 5m behind exceeds the
        // outer radius from the forward-shifted pivot.  Guards against
        // regressions that would collapse the wedge into "any target
        // behind the attacker hits".
        let strike = back_kick_strike_after_wrap();
        let wedge = super::EvaluatedWedge::evaluate(&strike, Vec3::ZERO, Vec3::NEG_Z, 0.5, 1.0);
        assert!(!wedge.contains_target(Vec3::new(0.0, 1.0, 5.0), 0.5, 1.5));
    }

    #[test]
    fn back_kick_misses_target_perpendicular() {
        // Perpendicular to the slice heading: angle from slice_heading
        // exceeds halfWidth (0.1 rad), so the angular bound rejects.
        let strike = back_kick_strike_after_wrap();
        let wedge = super::EvaluatedWedge::evaluate(&strike, Vec3::ZERO, Vec3::NEG_Z, 0.5, 1.0);
        assert!(!wedge.contains_target(Vec3::new(3.0, 1.0, 0.0), 0.5, 1.5));
    }

    // -- Sign-guard tests for the pivot shift -------------------------------
    //
    // hitbox.rs:`evaluate` shifts the pivot BACKWARD by `vanishingpoint`
    // along the slice heading: `center = attacker_base - slice_heading_xz * vp`.
    // The minus sign is critical and load-bearing — it places the active
    // hit zone IN FRONT OF the attacker rather than 2*vp meters past them.
    // These tests pin the sign down so that anyone "fixing" it back to `+`
    // because they don't think the formula looks right will see them fail.
    //
    // Setup: vp=3, outer=4, narrow forward arc.
    //   With `-` (correct): pivot at +3m back of attacker (Bevy +Z),
    //                       active hit zone 0..1m in front of attacker.
    //   With `+` (wrong):   pivot at +3m in front of attacker,
    //                       active hit zone 6..7m in front (way too far).

    #[test]
    fn forward_punch_with_vp_hits_target_at_close_reach() {
        // Target 0.5m in front of attacker.
        //   `-` sign: dist from pivot = 3.5m, in [3, 4] annulus, on axis → HIT.
        //   `+` sign: dist from pivot = 2.5m, INSIDE inner cut (3) → MISS.
        let strike = forward_punch_strike_with_vp();
        let wedge = super::EvaluatedWedge::evaluate(&strike, Vec3::ZERO, Vec3::NEG_Z, 0.5, 1.0);
        assert!(wedge.is_active);
        assert!(
            wedge.contains_target(Vec3::new(0.0, 1.0, -0.5), 0.5, 1.5),
            "with vp=3 and a backward-shifted pivot, a target 0.5m in front \
             should land in the annulus.  If this fails, someone probably \
             flipped the pivot shift sign in `EvaluatedWedge::evaluate`."
        );
    }

    #[test]
    fn forward_punch_with_vp_misses_target_at_double_reach() {
        // Target 6.5m in front of attacker.
        //   `-` sign: dist from pivot (at +3 back) = 9.5m, > outer 4 → MISS.
        //   `+` sign: dist from pivot (at -3 forward) = 3.5m, in [3, 4]
        //             annulus, on axis → HIT (incorrectly, since this far
        //             out is well past the punch's reach).
        let strike = forward_punch_strike_with_vp();
        let wedge = super::EvaluatedWedge::evaluate(&strike, Vec3::ZERO, Vec3::NEG_Z, 0.5, 1.0);
        assert!(
            !wedge.contains_target(Vec3::new(0.0, 1.0, -6.5), 0.5, 1.5),
            "a target 6.5m in front of attacker is well outside a punch's \
             reach (vp=3, outer=4 ⇒ ~1m reach).  If this hits, the pivot \
             shift sign was flipped to `+` and the wedge is now centered \
             at +vp instead of -vp."
        );
    }

    #[test]
    fn parsed_back_kick_atdt_resolves_to_back_facing_wedge() {
        // Round-trip: a synthetic .atdt body with the real back-kick
        // angles must parse to slice bounds whose midpoint is ≈ ±π
        // (directly behind) and whose half-width is the intended narrow
        // arc.  The parser is NOT allowed to wrap these to [-π, π]
        // individually — that splits them across the discontinuity and
        // collapses the wedge into the wrong-axis 280° arc that was
        // the original bug (see EvaluatedWedge docs for the postmortem).
        let src = r#"
strike {
    framenum 8.65
    frameduration 1.95
    reactdiskradius 7.04
    minreactdiskradius 7.04
    slicestartradians -3.3847
    sliceendradians -3.1847
    sliceheadingradiansb 0.0
    vanishingpoint 5.28
}
"#;
        let data = crate::oni2_loader::parsers::atdt::parse_atdt_content(src);
        let strike = data.strike.expect("strike block should parse");
        // After negate-only (no wrap), both should sit in the same
        // rotation cycle (+3.1847 .. +3.3847).
        assert!(strike.slicestartradians <= strike.sliceendradians);
        let mid = (strike.slicestartradians + strike.sliceendradians) * 0.5;
        let half = (strike.sliceendradians - strike.slicestartradians).abs() * 0.5;
        // The runtime feeds `mid` through `Quat::from_rotation_y` which
        // handles out-of-range angles natively, so what matters is the
        // resulting heading direction — not the raw radian value.  A
        // back-facing wedge produces a slice heading dotting strongly
        // with +Z when forward is -Z.
        let heading = Quat::from_rotation_y(mid) * Vec3::NEG_Z;
        let dot_back = heading.dot(Vec3::Z);
        assert!(
            dot_back > 0.95,
            "slice heading {:?} should point back (+Z); dot(+Z) = {}",
            heading,
            dot_back,
        );
        // Half-width = 0.1 rad (~6°) — the narrow back arc the file
        // authored.  If this comes out as ~141° (≈ π/2 × 0.9), the wrap
        // is back and the wedge is broken.
        assert!(
            (half - 0.1).abs() < 0.01,
            "half-width {} should be ~0.1 rad",
            half
        );
    }

    #[test]
    fn target_at_cone_edge() {
        // 45 degree offset from forward at distance 3
        let target = Vec3::new(3.0, 0.0, -3.0);
        // half_angle = PI/4 (45 degrees), target is at ~45 degrees
        assert!(cone_hit_test(
            Vec3::ZERO,
            Vec3::NEG_Z,
            PI / 4.0 + 0.01,
            5.0,
            target,
        ));
    }
}

use crate::oni2_loader::parsers::atdt::AtdtStrike;

/// An evaluated snapshot of an ATDT attack wedge for a specific animation frame.
/// Encapsulates the geometry transformation logic so both hit detection
/// and debug rendering share the exact same spatial bounds and inner cuts.
///
/// `swept_heading` (the slice center in actor-local radians, 0 = forward) is
/// derived the way C++ does in `crAttackSliceData::Init`: the
/// midpoint of `slicestartradians`
/// and `sliceendradians`.  `sliceheadingradiansb` is consulted only when
/// `sweepheading != 0`, as the destination heading for the sweep.  Using
/// `sliceheadingradiansb` directly (the previous bug) produced wedges that
/// were aligned to the wrong axis whenever slicestart/sliceend weren't
/// centered on 0 — i.e. for nearly every melee attack.
pub struct EvaluatedWedge {
    /// The pivot of the slice in world XZ — attacker base plus a `vanishingpoint`
    /// shift along `slice_heading_xz`.  Annulus distances are measured from here.
    pub center: Vec3,
    /// World-space slice heading direction in XZ (= forward rotated by `swept_heading`).
    /// Hit-test signed-angle bounds are RELATIVE to this direction.
    pub slice_heading_xz: Vec3,
    /// The actor-local heading of the slice (radians, 0 = forward).
    pub swept_heading: f32,
    /// The maximum reach distance computed for this frame (respecting expanding radius).
    pub max_radius: f32,
    /// The inner cut distance (= `vanishingpoint`, matching C++ inner test
    /// `dist2ToTarg < VanishingPointOffset²`).
    pub inner_radius: f32,
    /// Starting angle bound (radians, relative to `slice_heading_xz`):
    /// `-halfWidth` for non-sweep, `slicestart - swept_heading` adjusted for sweep.
    pub start_rad: f32,
    /// Ending angle bound (radians, relative to `slice_heading_xz`).
    pub end_rad: f32,
    /// Minimum height of the slice block (relative to world 0, pre-computed against attacker base).
    pub min_y: f32,
    /// Maximum height of the slice block (relative to world 0).
    pub max_y: f32,
    /// Whether this wedge is currently active for this specific frame.
    pub is_active: bool,
}

impl EvaluatedWedge {
    /// Evaluates the wedge geometry from an ATDT strike.
    /// `attacker_base` should be the position of the character's feet.
    pub fn evaluate(
        strike: &AtdtStrike,
        attacker_base: Vec3,
        attacker_forward: Vec3,
        frame: f32,
        num_frames: f32,
    ) -> Self {
        let is_active = if strike.frameduration > 0.0 {
            frame >= strike.framenum && frame <= strike.framenum + strike.frameduration
        } else if strike.maxradiusframe > strike.minradiusframe {
            frame >= strike.minradiusframe && frame <= strike.maxradiusframe
        } else {
            frame >= 0.0 && frame <= (num_frames - 1.0).max(0.0)
        };

        let phase = if num_frames > 1.0 {
            frame / (num_frames - 1.0).max(1.0)
        } else {
            0.0
        };

        // Derive the slice center the way C++ does in
        // `crAttackSliceData::Init`: the file
        // stores slicestart/sliceend, and `SliceHeadingRadiansA` is their
        // midpoint.  `sliceheadingradiansb` is the END of a sweep — it's
        // used only when `sweepheading != 0`.
        let slice_a = (strike.slicestartradians + strike.sliceendradians) * 0.5;
        let half_width = (strike.sliceendradians - strike.slicestartradians).abs() * 0.5;

        let swept_heading = if strike.sweepheading != 0 {
            let sweep_t = if strike.frameduration > 0.0 {
                ((frame - strike.framenum) / strike.frameduration).clamp(0.0, 1.0)
            } else {
                1.0
            };
            slice_a + (strike.sliceheadingradiansb - slice_a) * sweep_t
        } else {
            slice_a
        };

        // Slice heading in world XZ — forward rotated by `swept_heading`.
        // Critically, `swept_heading` is the DERIVED midpoint of slicestart/
        // sliceend, NOT `sliceheadingradiansb`.  Using the wrong field caused
        // the "wedge floating tens of meters off-axis" bug for any attack
        // whose slicestart/sliceend wasn't centered on 0 (jabs at ±π/2,
        // back kicks at ±π).
        let slice_heading = Quat::from_rotation_y(swept_heading) * attacker_forward;
        let slice_heading_xz = Vec3::new(slice_heading.x, 0.0, slice_heading.z).normalize_or_zero();

        // Pivot shifts BACKWARD by `vanishingpoint` along the slice heading.
        // Combined with the inner-cut at exactly `vanishingpoint` and an
        // annulus extending out to `reactdiskradius`, this puts the active
        // hit zone in front of the attacker along the slice heading,
        // bounded between 0 and `(reactdiskradius - vanishingpoint)`m —
        // i.e. the actual reach of the swing.  Verified against in-game
        // ATDT data: PUNCH_COMBO2 (vp=3.81, outer=4.83) → 1.02m forward
        // reach; JAB_RIGHT (vp=8.9, outer=10) → 1.1m sideways reach;
        // KICK_ROUNDHOUSE_BACK (vp=1.79, outer=3.08) → 1.29m back reach.
        let center = attacker_base - slice_heading_xz * strike.vanishingpoint;

        let max_radius = if strike.use_expanding_radius {
            let min_phase = strike.minradiusframe / num_frames.max(1.0);
            let max_phase = strike.maxradiusframe / num_frames.max(1.0);

            if phase <= min_phase {
                strike.minreactdiskradius
            } else if phase >= max_phase {
                strike.reactdiskradius
            } else if max_phase > min_phase {
                let t = (phase - min_phase) / (max_phase - min_phase);
                strike.minreactdiskradius + t * (strike.reactdiskradius - strike.minreactdiskradius)
            } else {
                strike.reactdiskradius
            }
        } else {
            strike.reactdiskradius
        };

        Self {
            center,
            slice_heading_xz,
            swept_heading,
            max_radius,
            inner_radius: strike.vanishingpoint,
            // Bounds RELATIVE to slice_heading_xz (= the actor-local rotation
            // applied to forward to get slice_heading).  For non-sweep this is
            // exactly `±halfWidth = (slicestart..sliceend) - midpoint`.
            start_rad: -half_width,
            end_rad: half_width,
            min_y: attacker_base.y + strike.reactdiskheight - strike.reactdiskheighttolerance,
            max_y: attacker_base.y + strike.reactdiskheight + strike.reactdiskheighttolerance,
            is_active,
        }
    }

    /// Checks if a cylindrical target intersects this wedge.
    /// `target_pos` is the XZ center (Y is ignored in the diff).
    /// `target_min_y` and `target_max_y` define the vertical bounds.
    pub fn contains_target(&self, target_pos: Vec3, target_min_y: f32, target_max_y: f32) -> bool {
        if target_min_y > self.max_y || target_max_y < self.min_y {
            return false;
        }

        let diff = target_pos - self.center;
        let dist_sq = diff.x * diff.x + diff.z * diff.z;

        if dist_sq > self.max_radius * self.max_radius {
            return false;
        }
        if dist_sq < self.inner_radius * self.inner_radius {
            return false;
        }

        let dir_to_target = Vec3::new(diff.x, 0.0, diff.z).normalize_or_zero();
        let dot = self.slice_heading_xz.dot(dir_to_target);
        let angle = dot.clamp(-1.0, 1.0).acos();
        let cross_y = self.slice_heading_xz.cross(dir_to_target).y;
        let signed_angle = if cross_y > 0.0 { angle } else { -angle };

        if signed_angle < self.start_rad || signed_angle > self.end_rad {
            return false;
        }

        true
    }
}
