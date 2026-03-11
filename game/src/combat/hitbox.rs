use bevy::prelude::*;

/// Cone-shaped hit test inspired by ONI 2's crStrike sweep system.
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
