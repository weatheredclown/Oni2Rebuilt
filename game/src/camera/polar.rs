/*
 * camera/polar.rs — rb camnewPolar / camnewRBPolar location + collision.
 *
 * apply_camera_transform is the tail of rb's `camnewCamera::Update`
 * (camera_new/camera.cpp:233-264): GetDesiredLocationAndFOV lerps each polar
 * value one step from current toward desired, UpdateCollision
 * (rbpolar.cpp:257-334) may re-aim desired incline/distance and recompute the
 * location, then the camera looks at the focus point.  There is no further
 * position smoothing: next frame the current polar values are re-read from
 * the camera transform (systems::update_camera_channel).
 *
 * Also hosts the small rb angle helpers the mode evaluators share.
 */
use super::channel::CameraChannel;
use super::components::CameraController;
use crate::oni2_loader::parsers::camera::{CameraClass, CameraParameterSet};
use bevy::prelude::*;
use std::f32::consts::{PI, TAU};

/// rb `Wrap(x, lo, hi)`: wrap into [lo, hi).
pub fn wrap_angle(x: f32, lo: f32, hi: f32) -> f32 {
    let range = hi - lo;
    lo + (x - lo).rem_euclid(range)
}

/// rb `GetRadiansDifference(a, b)`: a - b wrapped into [-pi, pi).
pub fn radians_difference(a: f32, b: f32) -> f32 {
    wrap_angle(a - b, -PI, PI)
}

/// Keep current within pi of desired so the lerp goes the short way
/// (follow.cpp:279-285, fight.cpp:198-204, polar.cpp:249-255).
pub fn fix_current_azimuth_wrap(ch: &mut CameraChannel) {
    if ch.current_azimuth < 0.0 && ch.desired_azimuth > ch.current_azimuth + PI {
        ch.current_azimuth += TAU;
    } else if ch.current_azimuth > 0.0 && ch.desired_azimuth < ch.current_azimuth - PI {
        ch.current_azimuth -= TAU;
    }
}

/// camnewPolar::GetDesiredLocationAndFOV (polar.cpp:198-210).
///
/// The lerp-rate getters it calls are camnewPolar's by-value overloads
/// (polar.h:45-48); camnewRBPolar/Follow/Fight declare `const camnewChannel&`
/// versions (rbpolar.h:38-41, follow.h:38, fight.h:33), which hide rather
/// than override them.  So in the shipped game the azimuth/incline/distance/
/// FOV steps always use the plain m_LerpRate* of the active camera — the
/// heading-zone, bump and collision lerp rates never reach this lerp.  (The
/// third-person camera's azimuth rate is the exception, see below.)
fn desired_location(ch: &CameraChannel, p: &CameraParameterSet, dt: f32) -> Vec3 {
    let t = |rate: f32| (dt * rate).clamp(0.0, 1.0);
    // camnewRBThirdPerson's GetAzimuthLerpRate does take the channel by
    // value, so it is the one override that reaches here (rbthirdperson.h:37).
    let azimuth_rate =
        if p.class == CameraClass::RBThirdPerson && ch.cam_stick == Vec2::ZERO && ch.is_moving {
            p.lerp_rate_follow_azimuth
        } else {
            p.lerp_rate_azimuth
        };
    let azimuth = ch.current_azimuth.lerp(ch.desired_azimuth, t(azimuth_rate));
    let incline = ch
        .current_incline
        .lerp(ch.desired_incline, t(p.lerp_rate_incline));
    let distance = ch
        .current_distance
        .lerp(ch.desired_distance, t(p.lerp_rate_distance));
    ch.current_focus_point
        + Vec3::new(
            incline.cos() * azimuth.sin(),
            incline.sin(),
            incline.cos() * azimuth.cos(),
        ) * distance
}

/// Collision response: rb aims the camera over the obstruction instead of
/// just pulling it in.  `isect_flat` is the flat distance from the focus
/// actor to the hit; the incline rises from InclineOffset (nothing in the
/// way) toward CollisionInclineOffset (wall at the focus) and the distance is
/// chosen so the camera's flat distance equals `isect_flat`
/// (rbpolar.cpp:292-300 / 306-314).
fn aim_over_hit(ch: &mut CameraChannel, p: &CameraParameterSet, hit: Vec3) {
    let r = p.collision_sphere_radius;
    let desired_flat = p.distance * ch.current_incline.cos() + r;
    let isect_flat = hit
        .xz()
        .distance(ch.current_focus_pos.xz())
        .clamp(0.0, desired_flat);
    ch.desired_incline = if ch.cam_stick.y != 0.0 {
        ch.current_incline.clamp(p.min_incline, p.max_incline)
    } else {
        p.collision_incline_offset
            .lerp(p.incline_offset, isect_flat / desired_flat)
            .clamp(p.min_incline, p.max_incline)
    };
    ch.desired_distance = isect_flat / ch.desired_incline.cos();
    ch.is_colliding = true;
}

/// Applies the polar camera (and script overrides) to the camera Transform.
pub fn apply_camera_transform(
    time: Res<Time>,
    mut camera_query: Query<(
        &CameraController,
        &mut CameraChannel,
        &mut Transform,
        &mut GlobalTransform,
        &mut Projection,
    )>,
    spatial_query: avian3d::prelude::SpatialQuery,
    sensors: Query<(), With<avian3d::prelude::Sensor>>,
) {
    let dt = time.delta_secs();

    for (_controller, mut channel, mut cam_tf, mut cam_gtf, mut projection) in &mut camera_query {
        if let Some(script_tf) = channel.script_override_transform {
            if let Some(fov) = channel.script_fov_override {
                channel.current_fov = fov;
            }
            if let Projection::Perspective(ref mut persp) = *projection {
                persp.fov = channel.current_fov.to_radians();
            }
            cam_tf.translation = script_tf.translation;
            cam_tf.rotation = script_tf.rotation;
            apply_camera_shake(&mut cam_tf, &mut channel, dt);
            // Mirror onto GlobalTransform so render extract this same
            // frame sees the new pose — this system runs in PostUpdate
            // AFTER `TransformSystems::Propagate`, so without this
            // line the camera's GlobalTransform would lag by one
            // frame (manifests as the world appearing to "double-jitter"
            // because the camera view matrix uses the stale GTF).
            *cam_gtf = GlobalTransform::from(*cam_tf);
            continue;
        }

        let first_frame = !channel.has_snapped_to_target;
        let ch = &mut *channel;
        let p = ch.params.clone();
        if let Some(fov) = ch.script_fov_override {
            ch.desired_fov = fov;
        }

        let mut location = desired_location(ch, &p, dt);
        let fov = ch
            .current_fov
            .lerp(ch.desired_fov, (dt * p.lerp_rate_fov).clamp(0.0, 1.0));

        // camnewRBPolar::UpdateCollision (rbpolar.cpp:257-334).  rb's probes
        // only test BOUNDFLAG_MOVER bounds — the world movers stand on —
        // so creatures and trigger volumes never block the camera.
        if !first_frame {
            let filter = avian3d::prelude::SpatialQueryFilter::from_mask(
                crate::combat::bundles::GameLayer::Default,
            )
            .with_excluded_entities([ch.focus_actor]);
            let not_sensor = |e: Entity| !sensors.contains(e);
            let r = p.collision_sphere_radius;
            let was_colliding = ch.is_colliding;
            ch.is_colliding = false;

            // Line of sight from the focus point to where the camera was last
            // frame, pushed back by the sphere radius "to avoid near clip
            // plane problems" (rbpolar.cpp:272-281).
            let seg_a = ch.current_focus_point;
            let seg_b = cam_tf.translation + cam_tf.back() * r;
            let seg = seg_b - seg_a;
            let probe_hit = Dir3::new(seg).ok().and_then(|dir| {
                spatial_query
                    .cast_ray_predicate(seg_a, dir, seg.length(), true, &filter, &not_sensor)
                    .map(|hit| seg_a + dir * hit.distance)
            });
            // TestSphere(location, radius): nearest world surface point.
            let sphere_hit = |radius: f32, at: Vec3| {
                spatial_query
                    .project_point_predicate(at, false, &filter, &not_sensor)
                    .map(|proj| proj.point)
                    .filter(|pt| pt.distance(at) < radius)
            };

            if let Some(hit) = probe_hit {
                aim_over_hit(ch, &p, hit);
                location = desired_location(ch, &p, dt);
            } else if let Some(hit) = sphere_hit(r, location) {
                aim_over_hit(ch, &p, hit);
                location = desired_location(ch, &p, dt);
            } else if was_colliding && sphere_hit(1.25 * r, location).is_some() {
                // Still grazing: hold the distance (rbpolar.cpp:319-331).
                ch.desired_distance = ch.current_distance;
                ch.is_colliding = true;
                location = desired_location(ch, &p, dt);
            }
        }

        ch.current_fov = fov;
        if let Projection::Perspective(ref mut persp) = *projection {
            persp.fov = fov.to_radians();
        }
        cam_tf.translation = location;
        if location.distance_squared(ch.current_focus_point) > 1e-8 {
            cam_tf.look_at(ch.current_focus_point, Vec3::Y);
        }
        ch.has_snapped_to_target = true;

        apply_camera_shake(&mut cam_tf, &mut channel, dt);
        // Mirror onto GlobalTransform — this system runs in PostUpdate
        // after `TransformSystems::Propagate`, so writing only Transform
        // would leave GlobalTransform stale until next frame, and
        // render extract uses GlobalTransform for the view matrix.
        *cam_gtf = GlobalTransform::from(*cam_tf);
    }
}

/// Applies and decays any active camera shake as a random rotation perturbation.
/// Applies a random Euler-angle perturbation, damped as the shake timer approaches expiry.
fn apply_camera_shake(cam_tf: &mut Transform, channel: &mut CameraChannel, dt: f32) {
    if channel.shake_time_remaining <= 0.0 {
        return;
    }

    let scale = if channel.shake_time_remaining < channel.shake_time_to_damp {
        channel.shake_time_remaining / channel.shake_time_to_damp
    } else {
        1.0
    };

    // Pseudo-random per-frame perturbation using the remaining time as a seed.
    // Different frequencies per axis to avoid correlated motion.
    let t = channel.shake_time_remaining;
    let rx = (t * 97.3).sin() * channel.shake_amplitude.x * scale;
    let ry = (t * 53.7).sin() * channel.shake_amplitude.y * scale;
    let rz = (t * 31.1).sin() * channel.shake_amplitude.z * scale;

    cam_tf.rotation *= Quat::from_euler(EulerRot::XYZ, rx, ry, rz);
    channel.shake_time_remaining -= dt;
    if channel.shake_time_remaining < 0.0 {
        channel.shake_time_remaining = 0.0;
    }
}
