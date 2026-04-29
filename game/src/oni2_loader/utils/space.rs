/*
 * oni2_loader/utils/space.rs
 *
 * Provides central conversion routines between the left-handed Oni2 world space
 * (X right, Y up, Z forward) and right-handed Bevy space (X right, Y up, Z backward).
 *
 * MAPPING:
 *   Bevy space mirrors the Oni2 X and Z axes, conceptually matching a 180° rotation
 *   around the global Y-axis, transitioning coordinates from left-handed to right-handed.
 */

use bevy::math::{Quat, Vec3};

pub trait IntoSpaceVec3 {
    fn into_space_vec3(self) -> Vec3;
}

impl IntoSpaceVec3 for Vec3 {
    #[inline]
    fn into_space_vec3(self) -> Vec3 {
        self
    }
}

impl IntoSpaceVec3 for [f32; 3] {
    #[inline]
    fn into_space_vec3(self) -> Vec3 {
        Vec3::from_array(self)
    }
}

impl IntoSpaceVec3 for &[f32; 3] {
    #[inline]
    fn into_space_vec3(self) -> Vec3 {
        Vec3::from_array(*self)
    }
}

impl IntoSpaceVec3 for &Vec3 {
    #[inline]
    fn into_space_vec3(self) -> Vec3 {
        *self
    }
}

impl IntoSpaceVec3 for &[f32] {
    #[inline]
    fn into_space_vec3(self) -> Vec3 {
        Vec3::new(self[0], self[1], self[2])
    }
}

impl IntoSpaceVec3 for Vec<f32> {
    #[inline]
    fn into_space_vec3(self) -> Vec3 {
        Vec3::new(self[0], self[1], self[2])
    }
}

impl IntoSpaceVec3 for &Vec<f32> {
    #[inline]
    fn into_space_vec3(self) -> Vec3 {
        Vec3::new(self[0], self[1], self[2])
    }
}

/// Convert a position in Oni2 space (left-handed: +Z forward)
/// into Bevy space (right-handed: -Z forward).
#[inline]
pub fn to_bevy_space_pos<T: IntoSpaceVec3>(pt: T) -> Vec3 {
    let pos = pt.into_space_vec3();
    Vec3::new(-pos.x, pos.y, -pos.z)
}

/// Convert a position in Bevy space (right-handed: -Z forward)
/// back to Oni2 space (left-handed: +Z forward) for export or script engine values.
#[inline]
pub fn to_oni2_space_pos<T: IntoSpaceVec3>(pt: T) -> Vec3 {
    let pos = pt.into_space_vec3();
    Vec3::new(-pos.x, pos.y, -pos.z)
}

/// Converts Oni2 Euler rotations (in radians) into a Bevy local Quaternion.
/// Oni2 typically provides orientation in Y, X, Z euler degrees.
#[inline]
pub fn to_bevy_space_rot_rad<T: IntoSpaceVec3>(yaw_pitch_roll: T) -> Quat {
    let yaw_pitch_roll = yaw_pitch_roll.into_space_vec3();
    // 180° Y rotation flips X (pitch) and Z (roll) rotation directions
    Quat::from_rotation_y(yaw_pitch_roll.y)
        * Quat::from_rotation_x(-yaw_pitch_roll.x)
        * Quat::from_rotation_z(-yaw_pitch_roll.z)
}

/// Converts a Bevy local Quaternion back into Oni2 Euler rotations (in radians).
#[inline]
pub fn to_oni2_space_rot_rad(q: Quat) -> Vec3 {
    let (yaw, pitch, roll) = q.to_euler(bevy::math::EulerRot::YXZ);
    // Reverse the negation applied in to_bevy_space_rot
    Vec3::new(-pitch, yaw, -roll)
}

/// Converts Oni2 Euler rotations (in degrees) into a Bevy local Quaternion.
#[inline]
pub fn to_bevy_space_rot<T: IntoSpaceVec3>(yaw_pitch_roll_deg: T) -> Quat {
    let mut rads = yaw_pitch_roll_deg.into_space_vec3();
    rads.x = rads.x.to_radians();
    rads.y = rads.y.to_radians();
    rads.z = rads.z.to_radians();
    to_bevy_space_rot_rad(rads)
}

/// Converts a Bevy local Quaternion back into Oni2 Euler rotations (in degrees).
#[inline]
pub fn to_oni2_space_rot(q: Quat) -> Vec3 {
    let rads = to_oni2_space_rot_rad(q);
    Vec3::new(
        rads.x.to_degrees(),
        rads.y.to_degrees(),
        rads.z.to_degrees(),
    )
}

/// Converts an Oni2 spherical camera elevation/incline angle (in degrees) to Bevy's camera polar elevation (in radians).
/// In Oni2 convention, incline offsets are stored directly in radians as positive elevation.
#[inline]
pub fn oni2_camera_incline_to_bevy(incline_rads: f32) -> f32 {
    incline_rads
}

/// Converts a yaw angle (rotation around the Y axis) from Oni2 space (left-handed, CCW)
/// to Bevy space (right-handed, CW rotation logic).
/// Inverts the sign of the angle to accommodate the coordinate system flip.
#[inline]
pub fn oni2_to_bevy_yaw_rads(rads: f32) -> f32 {
    -rads
}

/// Converts a yaw angle from Bevy space back to Oni2 space.
#[inline]
pub fn bevy_to_oni2_yaw_rads(rads: f32) -> f32 {
    -rads
}

/// Wrap an angle (radians) into the principal range `[-π, π]`.
///
/// Authoring tools and shipped `.atdt` files sometimes store yaw
/// angles outside this range (e.g. `kno_atk_comb_bak_k_bskckLH.atdt`
/// uses `slicestartradians = -3.3847` ≈ `-194°`).  Most of our hit-
/// test math (e.g. `signed_angle = acos(...)` clamped to `[0, π]`
/// then signed by cross-product Y) operates in `[-π, π]`, so any
/// bound passed in raw will silently fail the angular range check
/// and the wedge will never register a hit.  Apply this after
/// `oni2_to_bevy_yaw_rads` for slice bounds and headings to keep
/// the comparison space consistent.
#[inline]
pub fn wrap_angle_to_pi(rads: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut a = rads % TAU;
    if a > PI {
        a -= TAU;
    } else if a < -PI {
        a += TAU;
    }
    a
}

/// Extension trait for Bevy's `Transform` and `GlobalTransform` to abstract away
/// the fact that ONI models inherently face their local `+Z` axis.
pub trait OniTransformExt {
    /// Returns the true visual forward vector of the ONI model (local +Z).
    fn oni_forward(&self) -> bevy::math::Dir3;
    /// Returns the true visual backward vector of the ONI model (local -Z).
    fn oni_back(&self) -> bevy::math::Dir3;
    /// Returns the true visual left vector of the ONI model (local +X).
    fn oni_left(&self) -> bevy::math::Dir3;
    /// Returns the true visual right vector of the ONI model (local -X).
    fn oni_right(&self) -> bevy::math::Dir3;
}

impl OniTransformExt for bevy::prelude::Transform {
    #[inline]
    fn oni_forward(&self) -> bevy::math::Dir3 {
        self.back()
    }
    #[inline]
    fn oni_back(&self) -> bevy::math::Dir3 {
        self.forward()
    }
    #[inline]
    fn oni_left(&self) -> bevy::math::Dir3 {
        self.right()
    }
    #[inline]
    fn oni_right(&self) -> bevy::math::Dir3 {
        self.left()
    }
}

impl OniTransformExt for bevy::prelude::GlobalTransform {
    #[inline]
    fn oni_forward(&self) -> bevy::math::Dir3 {
        self.back()
    }
    #[inline]
    fn oni_back(&self) -> bevy::math::Dir3 {
        self.forward()
    }
    #[inline]
    fn oni_left(&self) -> bevy::math::Dir3 {
        self.right()
    }
    #[inline]
    fn oni_right(&self) -> bevy::math::Dir3 {
        self.left()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn wrap_angle_keeps_in_range() {
        let eps = 1e-5;
        assert!((wrap_angle_to_pi(0.0) - 0.0).abs() < eps);
        assert!((wrap_angle_to_pi(PI - 0.1) - (PI - 0.1)).abs() < eps);
        assert!((wrap_angle_to_pi(-PI + 0.1) - (-PI + 0.1)).abs() < eps);
    }

    #[test]
    fn wrap_angle_folds_back() {
        // -3.3847 (atdt back-kick start) wraps to +2.8985.
        let eps = 1e-4;
        let v = wrap_angle_to_pi(-3.3847);
        assert!(v.abs() <= PI, "wrapped value {v} should be in [-π, π]");
        assert!((v - (-3.3847 + 2.0 * PI)).abs() < eps);

        // +3.3847 wraps to -2.8985.
        let v = wrap_angle_to_pi(3.3847);
        assert!(v.abs() <= PI);
        assert!((v - (3.3847 - 2.0 * PI)).abs() < eps);
    }

    #[test]
    fn wrap_angle_handles_exact_pi() {
        // ±π are both valid principal-range outputs; we don't care which one,
        // only that magnitude stays ≤ π.
        assert!(wrap_angle_to_pi(PI).abs() <= PI + 1e-6);
        assert!(wrap_angle_to_pi(-PI).abs() <= PI + 1e-6);
    }
}
