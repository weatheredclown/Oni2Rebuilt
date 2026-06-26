/*
 * mover/steering.rs — locomotion turn rate-limiting.
 *
 * Port of the heading half of `mvrMoverComponent::ApplyInertia` +
 * `ConvertYawToDeltaYRotation` (crmover/packet.cpp, geom.cpp).  Locomotion
 * steering must NOT snap a character instantly to its desired heading: the
 * legacy engine filters the yaw control through `TendToControl` (accel/decel)
 * and then rotates by at most `MaxTurnRate` per frame.  Without this the Rust
 * port turns instantly toward whatever direction navigation wants this tick,
 * so a character at a decision boundary (e.g. a ledge/corner where the desired
 * heading flips frame-to-frame) strobes — visibly ghosting at 120 Hz.
 *
 * IMPORTANT: this is for LOCOMOTION ONLY.  Combat alignments (attack/react/
 * grapple facing snaps, end-rotation notches, grapple teleports) deliberately
 * write `Fighter.facing` directly and must stay instant — they never go through
 * this helper.  `fighter_rotation_sync_system` keeps `Fighter.facing` the single
 * source of truth either way.
 */
use bevy::prelude::*;

/// Per-character locomotion steering state + tuning.  Mirrors the heading-side
/// fields of `mvrMoverComponentType` (`MaxTurnRate`, `YawAccel`, `YawDecel`,
/// `InertiaEnable`) plus the running `CurrentYaw` control value.
#[derive(Component, Debug, Clone)]
pub struct LocomotionSteering {
    /// Filtered yaw control in `[-1, 1]` — legacy `CurrentYaw`.  How hard the
    /// character is currently turning, as a fraction of `max_turn_rate`.
    pub current_yaw: f32,
    /// Maximum turn rate in radians/second.  Legacy `MaxTurnRate` is stored in
    /// degrees per (1/60)s frame; the rad/s form is `MaxTurnRate * DtoR * 60`.
    pub max_turn_rate: f32,
    /// Rate the yaw control ramps UP toward its target, per second
    /// (legacy `YawAccel`).
    pub yaw_accel: f32,
    /// Rate the yaw control ramps DOWN toward zero, per second
    /// (legacy `YawDecel`).
    pub yaw_decel: f32,
    /// Master enable for the accel/decel filter (legacy `InertiaEnable`).  When
    /// false, turning is still clamped to `max_turn_rate` but with no inertia.
    pub inertia_enable: bool,
}

impl Default for LocomotionSteering {
    fn default() -> Self {
        // Defaults until per-character `mvrMoverComponentType` tuning is parsed.
        // ~9.4 rad/s ≈ 540°/s top turn speed; the yaw control reaches full in
        // ~0.15 s and bleeds off faster than it builds (decel > accel) so a
        // one-frame flip in the desired heading barely moves the body.
        Self {
            current_yaw: 0.0,
            max_turn_rate: 9.42,
            yaw_accel: 7.0,
            yaw_decel: 12.0,
            inertia_enable: true,
        }
    }
}

/// Port of `mvrMoverComponent::TendToControl` — moves `val` toward `control`,
/// limited by `accel` (when building toward a non-zero target) or `decel` (when
/// coasting back to zero).  Crucially, when `control` flips sign relative to
/// `val`, `val` is reset to 0 first: this is what prevents instantaneous
/// reversals (and damps per-frame strobing).
pub fn tend_to_control(control: f32, mut val: f32, decel: f32, accel: f32) -> f32 {
    if control > 0.0 {
        if val < 0.0 {
            val = 0.0; // was turning the other way — kill it before re-accelerating
        }
        val += accel;
        if val > control {
            val = control; // never overshoot the requested control
        }
    } else if control < 0.0 {
        if val > 0.0 {
            val = 0.0;
        }
        val -= accel;
        if val < control {
            val = control;
        }
    } else {
        // control == 0: coast to a stop at `decel`.
        if val < 0.0 {
            val += decel;
            if val > 0.0 {
                val = 0.0;
            }
        } else if val > 0.0 {
            val -= decel;
            if val < 0.0 {
                val = 0.0;
            }
        }
    }
    val
}

/// Rate-limited locomotion turn.  Given the current `facing` (a world-space XZ
/// direction the model's +Z points along) and a `desired` heading, advances the
/// facing toward `desired` by at most one frame's worth of turn — filtered
/// through `TendToControl` + `MaxTurnRate`.  Returns the new facing; mutates
/// `steering.current_yaw`.
///
/// Mirrors the legacy chain:
///   daz       = AngularDifference(facing, desired)
///   yawTarget = ConvertDeltaYRotationToYaw(daz)         // clamp(daz / (maxRate*dt), ±1)
///   CurrentYaw = TendToControl(yawTarget, CurrentYaw, YawDecel*dt, YawAccel*dt)
///   dAz       = ConvertYawToDeltaYRotation(CurrentYaw)  // CurrentYaw * maxRate * dt
///   facing    = RotateY(facing, dAz)
pub fn steer_facing(
    facing: Vec3,
    desired: Vec3,
    steering: &mut LocomotionSteering,
    dt: f32,
) -> Vec3 {
    if dt <= 0.0 {
        return facing;
    }
    let facing_xz = Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero();
    let desired_xz = Vec3::new(desired.x, 0.0, desired.z).normalize_or_zero();
    if desired_xz.length_squared() < 1e-6 {
        return facing; // no desired heading — hold
    }
    if facing_xz.length_squared() < 1e-6 {
        return desired_xz; // no current facing — adopt the target
    }

    // Signed angular difference about +Y, in (-π, π].  Positive = turn CCW.
    let dot = facing_xz.dot(desired_xz).clamp(-1.0, 1.0);
    let mut daz = dot.acos();
    if facing_xz.cross(desired_xz).y < 0.0 {
        daz = -daz;
    }

    let max_az = steering.max_turn_rate * dt; // most we may turn this frame

    let d_angle = if !steering.inertia_enable {
        // No inertia: turn straight toward the target, clamped to max rate.
        daz.clamp(-max_az, max_az)
    } else {
        // ConvertDeltaYRotationToYaw: the yaw control that would exactly reach
        // `daz` this frame, clamped to the [-1, 1] control range.
        let yaw_target = if max_az > 1e-6 {
            (daz / max_az).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        steering.current_yaw = tend_to_control(
            yaw_target,
            steering.current_yaw,
            steering.yaw_decel * dt,
            steering.yaw_accel * dt,
        );
        // ConvertYawToDeltaYRotation.
        steering.current_yaw * max_az
    };

    if d_angle.abs() < 1e-7 {
        return facing_xz;
    }
    (Quat::from_rotation_y(d_angle) * facing_xz).normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tend_to_control_resets_on_sign_flip() {
        // Building positive...
        let v = tend_to_control(1.0, 0.5, 0.2, 0.1);
        assert!((v - 0.6).abs() < 1e-6);
        // ...then the control flips negative: value is zeroed before accelerating.
        let v = tend_to_control(-1.0, 0.6, 0.2, 0.1);
        assert!((v - (-0.1)).abs() < 1e-6, "got {v}");
    }

    #[test]
    fn tend_to_control_coasts_to_zero() {
        let v = tend_to_control(0.0, 0.3, 0.1, 0.2);
        assert!((v - 0.2).abs() < 1e-6);
        let v = tend_to_control(0.0, 0.05, 0.1, 0.2);
        assert_eq!(v, 0.0, "decel overshoots to exactly zero, not past it");
    }

    #[test]
    fn strobe_is_damped() {
        // Desired heading flips 180° every frame; the body should barely rotate
        // because `current_yaw` keeps getting reset toward zero.
        let mut st = LocomotionSteering::default();
        let mut facing = Vec3::Z;
        let dt = 1.0 / 120.0;
        let fwd = Vec3::Z;
        let back = Vec3::new(0.0, 0.0, -1.0);
        let mut max_step = 0.0f32;
        for i in 0..20 {
            let desired = if i % 2 == 0 { back } else { fwd };
            let prev = facing;
            facing = steer_facing(facing, desired, &mut st, dt);
            let step = prev.angle_between(facing);
            max_step = max_step.max(step);
        }
        // A single uninertia'd frame toward 180° would be a full `max_az`
        // (~0.078 rad at 120 Hz); strobing should stay well under that.
        assert!(max_step < 0.05, "strobe not damped: max per-frame step {max_step}");
    }

    #[test]
    fn converges_to_steady_target() {
        let mut st = LocomotionSteering::default();
        let mut facing = Vec3::Z;
        let desired = Vec3::X;
        for _ in 0..240 {
            facing = steer_facing(facing, desired, &mut st, 1.0 / 60.0);
        }
        assert!(facing.angle_between(Vec3::X) < 0.05, "did not converge: {facing:?}");
    }
}
