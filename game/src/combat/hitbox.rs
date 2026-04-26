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
pub struct EvaluatedWedge {
    /// The physical origin center point of the slice (anchored to character spine).
    pub center: Vec3,
    /// The directional heading of the center of the slice span in world space.
    pub slice_heading_xz: Vec3,
    /// The yaw angle (in radians) defining the center of the arc sweep.
    pub swept_heading: f32,
    /// The maximum reach distance computed for this frame (respecting expanding radius).
    pub max_radius: f32,
    /// The inner cut distance (derived from vanishing point).
    pub inner_radius: f32,
    /// Starting angle bound (relative to slice_heading_xz).
    pub start_rad: f32,
    /// Ending angle bound (relative to slice_heading_xz).
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

        let swept_heading = if strike.sweepheading != 0 {
            let sweep_t = if strike.frameduration > 0.0 {
                ((frame - strike.framenum) / strike.frameduration).clamp(0.0, 1.0)
            } else {
                1.0
            };
            sweep_t * strike.sliceheadingradiansb
        } else {
            strike.sliceheadingradiansb
        };

        let slice_heading = Quat::from_rotation_y(swept_heading) * attacker_forward;
        let slice_heading_xz = Vec3::new(slice_heading.x, 0.0, slice_heading.z).normalize_or_zero();

        // Shift the virtual pivot FORWARD along the slice heading by
        // `vanishingpoint`.  This mirrors rb/src/fight/attackdata.cpp:817
        // (`ApplyVanishingPointToMatrix` does `m.d += R * v` where
        // `v = RotateY(-csh) * (0,0,VP)`, net effect `pivot += slice_heading * VP`).
        //
        // Wedge angles determine what happens next: narrow angles around the
        // slice heading (forward attack) keep the wedge ahead of the pivot —
        // a long-reach swing.  Angles near ±π from slice_heading (back-kick
        // style — see `kno_atk_comb_bak_bakkckCH.atdt` with 166°–191°) flip the
        // arc BACK through the attacker's body, so a forward-VP shift with
        // backward wedge angles lands the hit zone a short distance behind
        // the attacker.  Sign is crucial: `-` here put the pivot 1.96m behind
        // and a 166°-191° wedge then reached 3.9-5.4m *further* behind — the
        // visible "floating 1-2m+ away" bug.  With `+`, the same wedge lands
        // 0-1.5m behind the attacker, which is the intended kick hitbox.
        let center = attacker_base + slice_heading_xz * strike.vanishingpoint;

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
            start_rad: strike.slicestartradians,
            end_rad: strike.sliceendradians,
            min_y: attacker_base.y + strike.reactdiskheight - strike.reactdiskheighttolerance,
            max_y: attacker_base.y + strike.reactdiskheight + strike.reactdiskheighttolerance,
            is_active,
        }
    }

    /// Checks if a cylindrical target intersects this wedge.
    /// `target_pos` is the XZ center (Y is ignored in the diff).
    /// `target_min_y` and `target_max_y` define the vertical bounds.
    pub fn contains_target(&self, target_pos: Vec3, target_min_y: f32, target_max_y: f32) -> bool {
        // Height band test
        if target_min_y > self.max_y || target_max_y < self.min_y {
            return false;
        }

        let diff = target_pos - self.center;
        let dist_sq = diff.x * diff.x + diff.z * diff.z;

        // Outer radius check
        if dist_sq > self.max_radius * self.max_radius {
            return false;
        }

        // Inner radius cut
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
