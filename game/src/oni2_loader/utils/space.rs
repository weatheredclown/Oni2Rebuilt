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
    fn into_space_vec3(self) -> Vec3 { self }
}

impl IntoSpaceVec3 for [f32; 3] {
    #[inline]
    fn into_space_vec3(self) -> Vec3 { Vec3::from_array(self) }
}

impl IntoSpaceVec3 for &[f32; 3] {
    #[inline]
    fn into_space_vec3(self) -> Vec3 { Vec3::from_array(*self) }
}

impl IntoSpaceVec3 for &Vec3 {
    #[inline]
    fn into_space_vec3(self) -> Vec3 { *self }
}

impl IntoSpaceVec3 for &[f32] {
    #[inline]
    fn into_space_vec3(self) -> Vec3 { Vec3::new(self[0], self[1], self[2]) }
}

impl IntoSpaceVec3 for Vec<f32> {
    #[inline]
    fn into_space_vec3(self) -> Vec3 { Vec3::new(self[0], self[1], self[2]) }
}

impl IntoSpaceVec3 for &Vec<f32> {
    #[inline]
    fn into_space_vec3(self) -> Vec3 { Vec3::new(self[0], self[1], self[2]) }
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
    Vec3::new(rads.x.to_degrees(), rads.y.to_degrees(), rads.z.to_degrees())
}

/// Converts an Oni2 spherical camera elevation/incline angle (in degrees) to Bevy's camera polar elevation (in radians).
/// In Oni2 convention, incline offsets are stored directly in radians as positive elevation.
#[inline]
pub fn oni2_camera_incline_to_bevy(incline_rads: f32) -> f32 {
    incline_rads
}
