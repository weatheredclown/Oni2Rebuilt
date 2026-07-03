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

/// Frames per second the legacy `MaxTurnRate` (degrees per frame) is quoted at.
pub const MOVER_FRAME_HZ: f32 = 60.0;

/// Per-character mover tuning + runtime state — the consumed subset of the
/// legacy `mvrMoverComponent` (`<Mover>` component).  Speeds, turn/inertia and
/// the collision "hotdog" are resolved from `components.xml` (the base) with
/// per-actor `<Mover>` overrides; `current_yaw` is the running control state.
#[derive(Component, Debug, Clone)]
pub struct MoverData {
    // --- Speeds (m/s) ---
    /// `MaxForwardSpeed`.
    pub max_forward_speed: f32,
    /// `MaxReverseSpeed`.
    pub max_reverse_speed: f32,
    /// `MaxSideStepSpeed`.
    pub max_sidestep_speed: f32,

    // --- Turn / inertia ---
    /// Maximum turn rate in radians/second.  `MaxTurnRate` is authored in
    /// degrees per (1/60)s frame; the rad/s form is `MaxTurnRate * DtoR * 60`.
    pub max_turn_rate: f32,
    /// `YawAccel` — rate the yaw control ramps up toward its target, per second.
    pub yaw_accel: f32,
    /// `YawDecel` — rate the yaw control ramps down toward zero, per second.
    pub yaw_decel: f32,
    /// `InertiaEnable` — master enable for the accel/decel filter.  When false,
    /// turning is still clamped to `max_turn_rate` but with no ramp.
    pub inertia_enable: bool,

    // --- Collision "hotdog" (capsule) ---
    /// `CollisionHotdogRadius` (m) — the character capsule radius.
    pub hotdog_radius: f32,
    /// `CollisionHotdogLength` (m) — the character capsule cylinder length.
    pub hotdog_length: f32,

    // --- Runtime state ---
    /// Filtered yaw control in `[-1, 1]` — legacy `CurrentYaw`.
    pub current_yaw: f32,
}

impl Default for MoverData {
    /// Safety fallback ONLY — the authoritative base values come from
    /// `components.xml` via [`MoverData::from_defaults`].  These are used just
    /// when neither the actor nor components.xml provides a value (e.g. the
    /// components.xml load failed), so the character still moves sanely.
    fn default() -> Self {
        Self {
            max_forward_speed: 7.5,
            max_reverse_speed: 4.5,
            max_sidestep_speed: 5.0,
            max_turn_rate: deg_per_frame_to_rad_s(23.3),
            yaw_accel: 3.0,
            yaw_decel: 5.0,
            inertia_enable: true,
            hotdog_radius: 0.25,
            hotdog_length: 1.25,
            current_yaw: 0.0,
        }
    }
}

/// Convert legacy `MaxTurnRate` (degrees per 1/60 s frame) to radians/second.
pub fn deg_per_frame_to_rad_s(deg_per_frame: f32) -> f32 {
    deg_per_frame.to_radians() * MOVER_FRAME_HZ
}

impl MoverData {
    /// Build from a `<Mover>` component block.  The block is the actor's
    /// *merged* Mover XML from `parse_actor_xml`: `components.xml`'s base
    /// attributes are prepended to the template chain, so `extract_xml_attr`
    /// (last-wins) already yields "the actor's explicit value, else the
    /// components.xml default" for every field — no hardcoded base numbers.
    /// The `Default` fallback only bites if `block` is `None` or a field is
    /// missing from components.xml too (shouldn't happen).
    pub fn from_mover_xml(block: Option<&str>) -> Self {
        use crate::oni2_loader::utils::parse::{extract_xml_attr, parse_xml_bool};
        let fb = Self::default();
        let Some(block) = block else { return fb };
        let f = |attr: &str, d: f32| {
            extract_xml_attr(block, attr)
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(d)
        };
        Self {
            max_forward_speed: f("MaxForwardSpeed", fb.max_forward_speed),
            max_reverse_speed: f("MaxReverseSpeed", fb.max_reverse_speed),
            max_sidestep_speed: f("MaxSideStepSpeed", fb.max_sidestep_speed),
            max_turn_rate: deg_per_frame_to_rad_s(f(
                "MaxTurnRate",
                fb.max_turn_rate / (MOVER_FRAME_HZ * std::f32::consts::PI / 180.0),
            )),
            yaw_accel: f("YawAccel", fb.yaw_accel),
            yaw_decel: f("YawDecel", fb.yaw_decel),
            inertia_enable: extract_xml_attr(block, "InertiaEnable")
                .map(|v| parse_xml_bool(&v))
                .unwrap_or(fb.inertia_enable),
            hotdog_radius: f("CollisionHotdogRadius", fb.hotdog_radius),
            hotdog_length: f("CollisionHotdogLength", fb.hotdog_length),
            current_yaw: 0.0,
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
    steering: &mut MoverData,
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
        let mut st = MoverData::default();
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
        let mut st = MoverData::default();
        let mut facing = Vec3::Z;
        let desired = Vec3::X;
        for _ in 0..240 {
            facing = steer_facing(facing, desired, &mut st, 1.0 / 60.0);
        }
        assert!(facing.angle_between(Vec3::X) < 0.05, "did not converge: {facing:?}");
    }
}
