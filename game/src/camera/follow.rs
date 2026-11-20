/*
 * camera/follow.rs — navigation / follow camera mode (rb camnewFollow).
 *
 * follow_camera_system runs when the active package camera is a
 * camnewFollow (navigation or fighting state).  Port of
 * camera_new/follow.cpp `camnewFollow::UpdateChannel`: heading zones, the
 * standing dead zone, bump movement and the "desired azimuth = focus
 * azimuth" chase once the focus leaves the dead zone.
 */
use super::channel::{CameraBumpDirection, CameraChannel, FocusHeadingZone};
use super::components::CameraController;
use super::polar::{fix_current_azimuth_wrap, radians_difference, wrap_angle};
use crate::oni2_loader::parsers::camera::{CameraClass, CameraParameterSet};
use bevy::prelude::*;
use std::f32::consts::{FRAC_PI_2, PI};

/// camnewFollow::UpdateChannel (camera_new/follow.cpp:121-295).
pub fn follow_camera_system(
    time: Res<Time>,
    mut camera_query: Query<(&CameraController, &mut CameraChannel)>,
) {
    let dt = time.delta_secs();

    for (controller, mut channel) in &mut camera_query {
        // The package maps the game state to a camera; that camera's class
        // decides the update (camera_new/manager.cpp:956).
        if !super::runs_game_camera(controller.active_mode)
            || !matches!(
                ch_class(&channel),
                CameraClass::Follow | CameraClass::RBPolar
            )
        {
            continue;
        }
        // First frame: update_camera_channel already set desired = focus
        // heading / InclineOffset / Distance (follow.cpp:138-145).
        if !channel.has_snapped_to_target {
            continue;
        }
        let ch = &mut *channel;
        let p = ch.params.clone();
        let stick = ch.cam_stick;
        let use_reticle_x = stick.x.abs() > stick.y.abs();

        // Heading zone (follow.cpp:158-161): acos of the flat dot of the
        // camera's and the focus's back vectors.
        let flat_dot = ch.current_incline.cos()
            * radians_difference(ch.current_azimuth, ch.current_focus_azimuth).cos();
        ch.focus_camera_heading_diff = flat_dot.clamp(-1.0, 1.0).acos();
        ch.focus_heading_zone = if ch.focus_camera_heading_diff <= p.zone_angles[0] {
            FocusHeadingZone::Zone1
        } else if ch.focus_camera_heading_diff <= p.zone_angles[1] {
            FocusHeadingZone::Zone2
        } else if ch.focus_camera_heading_diff <= p.zone_angles[2] {
            FocusHeadingZone::Zone3
        } else {
            FocusHeadingZone::Zone4
        };

        if (ch.current_focus_azimuth - ch.previous_focus_azimuth).abs() > p.spin_threshold {
            ch.dead_zone_center = ch.current_focus_pos;
            ch.check_standing_zone = true;
            ch.spin_threshold_exceeded = true;
        }

        // Standing still: reset the dead zone (follow.cpp:171-180).
        if ch.current_focus_pos.distance(ch.previous_focus_pos) < 0.001 || ch.time_running == 0.0 {
            ch.is_moving = false;
            ch.check_standing_zone = true;
            ch.dead_zone_center = ch.current_focus_pos;
            ch.spin_threshold_exceeded = false;
        } else {
            ch.is_moving = true;
        }

        if ch.check_standing_zone {
            let d = ch.dead_zone_center.distance(ch.current_focus_pos);
            ch.in_dead_zone = d < p.dead_zone_inner_radius;
            ch.in_transition_zone = d < p.dead_zone_outer_radius && !ch.in_dead_zone;
        } else {
            ch.in_transition_zone = false;
            ch.in_dead_zone = false;
            ch.spin_threshold_exceeded = false;
        }

        if ch.rotate_until_desired
            && radians_difference(ch.desired_azimuth, ch.current_azimuth).abs() < 0.1
        {
            ch.rotate_until_desired = false;
        }

        follow_bump_movement(ch, &p, dt);

        // (m_UseTargetAzimuth is never enabled by a shipped layout.)
        if !ch.rotate_until_desired {
            if ch.in_dead_zone {
                if radians_difference(ch.desired_azimuth, ch.current_azimuth).abs() < FRAC_PI_2
                    && use_reticle_x
                {
                    let offset = if ch.time_since_reticle_zero > p.bump_max_time {
                        stick.x * p.rotation_increment
                    } else {
                        0.0
                    };
                    ch.desired_azimuth = wrap_angle(ch.desired_azimuth - offset, -PI, PI);
                }
            } else {
                if !ch.in_transition_zone {
                    ch.check_standing_zone = false;
                }
                if ch.bump_direction == CameraBumpDirection::None {
                    if stick.x != 0.0 && ch.time_since_reticle_zero > p.bump_max_time {
                        // Clamp to MaxAzimuth around the focus heading (follow.cpp:262-266).
                        let lo = ch.current_focus_azimuth - p.max_azimuth;
                        let hi = ch.current_focus_azimuth + p.max_azimuth;
                        ch.desired_azimuth =
                            (ch.desired_azimuth - stick.x * p.rotation_increment).clamp(lo, hi);
                    } else {
                        ch.desired_azimuth = ch.current_focus_azimuth;
                    }
                }
            }
        }

        fix_current_azimuth_wrap(ch);

        let incline_step = if ch.time_since_reticle_zero > p.bump_max_time {
            stick.y * p.rotation_increment
        } else {
            0.0
        };
        ch.desired_incline = if stick.y != 0.0 && !use_reticle_x {
            (ch.desired_incline - incline_step).clamp(p.min_incline, p.max_incline)
        } else {
            follow_incline_offset(ch, &p)
        };
        if !ch.is_colliding {
            ch.desired_distance = follow_distance(ch, &p);
        }
        ch.desired_fov = p.fov;
    }
}

fn ch_class(ch: &CameraChannel) -> CameraClass {
    ch.params.class
}

/// camnewFollow::GetInclineOffset (follow.cpp:445-469).
fn follow_incline_offset(ch: &CameraChannel, p: &CameraParameterSet) -> f32 {
    if ch.is_colliding {
        p.collision_incline_offset
    } else if ch.is_moving {
        p.incline_offset_running
    } else {
        p.incline_offset
    }
}

/// camnewFollow::GetDistance (follow.cpp:579-600).
fn follow_distance(ch: &CameraChannel, p: &CameraParameterSet) -> f32 {
    let zone = ch.focus_heading_zone as i32; // HEADING_ZONE_1 == 0
    if ch.is_jumping || ch.time_since_jumping < p.jumping_time {
        p.jumping_distance
    } else if ch.time_running > 0.0 && zone >= p.min_lock_heading_zone {
        p.distance_running_towards_cam
    } else {
        p.distance
    }
}

/// camnewFollow::UpdateChannelBumpMovement (follow.cpp:301-390), without the
/// PADDATA stored-heading bookkeeping (player control, not camera).
fn follow_bump_movement(ch: &mut CameraChannel, p: &CameraParameterSet, dt: f32) {
    let stick = ch.cam_stick;
    let neutral = stick == Vec2::ZERO;
    let mut use_bump = false;

    if ch.time_since_reticle_zero > p.bump_max_time {
        ch.bump_direction = CameraBumpDirection::None;
        ch.bump_magnitude = 0.0;
    }

    if neutral {
        use_bump = ch.time_since_reticle_zero > 0.0
            && ch.time_since_reticle_zero < p.bump_max_time
            && ch.bump_is_ready_to_use;
        ch.time_since_reticle_zero = 0.0;
    } else {
        if ch.time_since_reticle_zero < p.bump_max_time {
            track_bump_direction(ch, stick);
        }
        ch.time_since_reticle_zero += dt;
    }

    if use_bump {
        match ch.bump_direction {
            CameraBumpDirection::PositiveX => ch.desired_azimuth -= p.bump_angle_rotate - 0.1,
            CameraBumpDirection::NegativeX => ch.desired_azimuth += p.bump_angle_rotate - 0.1,
            CameraBumpDirection::PositiveY => ch.desired_azimuth -= PI - 0.1,
            CameraBumpDirection::NegativeY => ch.desired_azimuth += PI - 0.1,
            CameraBumpDirection::None => {}
        }
        ch.desired_azimuth = wrap_angle(ch.desired_azimuth, -PI, PI);
        ch.bump_magnitude = 0.0;
        if matches!(
            ch.bump_direction,
            CameraBumpDirection::PositiveY | CameraBumpDirection::NegativeY
        ) {
            ch.rotate_until_desired = true;
        }
    } else if neutral {
        ch.bump_is_ready_to_use = true;
    }
}

/// Largest stick axis wins the bump direction (follow.cpp:326-337, fight.cpp:230-241).
pub(super) fn track_bump_direction(ch: &mut CameraChannel, stick: Vec2) {
    if stick.x.abs() > ch.bump_magnitude {
        ch.bump_magnitude = stick.x.abs();
        ch.bump_direction = if stick.x > 0.0 {
            CameraBumpDirection::PositiveX
        } else {
            CameraBumpDirection::NegativeX
        };
    }
    if stick.y.abs() > ch.bump_magnitude {
        ch.bump_magnitude = stick.y.abs();
        ch.bump_direction = if stick.y > 0.0 {
            CameraBumpDirection::PositiveY
        } else {
            CameraBumpDirection::NegativeY
        };
    }
}
