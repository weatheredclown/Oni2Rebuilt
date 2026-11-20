/*
 * camera/thirdperson.rs — rb camnewRBThirdPerson.
 *
 * The camera of rb's built-in DEFAULT_PACKAGE (camera_new/package.cpp:48-62),
 * so it films every layout without a layout.campacknew.  Port of
 * camera_new/rbthirdperson.cpp `UpdateChannel`: chase the focus heading while
 * running (unless running at the camera), ease off when stopping, hold while
 * standing.
 */
use super::channel::CameraChannel;
use super::components::CameraController;
use super::polar::{fix_current_azimuth_wrap, radians_difference, wrap_angle};
use crate::oni2_loader::parsers::camera::CameraClass;
use bevy::prelude::*;
use std::f32::consts::{FRAC_PI_2, PI};

/// camnewRBThirdPerson::UpdateChannel (camera_new/rbthirdperson.cpp:74-143).
pub fn third_person_camera_system(
    mut camera_query: Query<(&CameraController, &mut CameraChannel)>,
) {
    for (controller, mut channel) in &mut camera_query {
        if !super::runs_game_camera(controller.active_mode)
            || channel.params.class != CameraClass::RBThirdPerson
            || !channel.has_snapped_to_target
        {
            continue;
        }
        let ch = &mut *channel;
        let p = ch.params.clone();
        let stick = ch.cam_stick;
        // Previous frame's stick / moving state (rbthirdperson.cpp:85-86).
        let was_using_stick = ch.prev_cam_stick != Vec2::ZERO;
        let was_moving = ch.is_moving;
        ch.prev_cam_stick = stick;

        let flat_dot = ch.current_incline.cos()
            * radians_difference(ch.current_azimuth, ch.current_focus_azimuth).cos();
        ch.focus_camera_heading_diff = flat_dot.clamp(-1.0, 1.0).acos();

        // "the magic percentage works like shit" (rbthirdperson.cpp:106-120)
        let magic = 0.05;
        let ease = |ch: &CameraChannel| {
            ch.current_azimuth + magic * radians_difference(ch.current_azimuth, ch.desired_azimuth)
        };
        if ch.current_focus_pos.distance(ch.previous_focus_pos) < 0.001 || ch.time_running == 0.0 {
            ch.is_moving = false;
            if (was_using_stick || was_moving) && stick == Vec2::ZERO {
                ch.desired_azimuth = ease(ch);
            }
        } else {
            ch.is_moving = true;
            if ch.focus_camera_heading_diff > p.angle_running_towards_camera {
                ch.desired_azimuth = ease(ch);
            } else if stick.x == 0.0 {
                ch.desired_azimuth = ch.current_focus_azimuth;
            }
        }

        if radians_difference(ch.desired_azimuth, ch.current_azimuth).abs() < FRAC_PI_2
            && stick.x != 0.0
        {
            ch.desired_azimuth =
                wrap_angle(ch.desired_azimuth - stick.x * p.rotation_increment, -PI, PI);
        }

        fix_current_azimuth_wrap(ch);

        // camnewRBPolar::GetInclineOffset / GetDistance (rbpolar.cpp:358-403).
        let jumping = ch.is_jumping || ch.time_since_jumping < p.jumping_time;
        ch.desired_incline = if stick.y != 0.0 {
            (ch.desired_incline - stick.y * p.rotation_increment)
                .clamp(p.min_incline, p.max_incline)
        } else if ch.is_colliding {
            p.collision_incline_offset
        } else {
            p.incline_offset
        };
        ch.desired_distance = if jumping {
            p.jumping_distance
        } else {
            p.distance
        };
        ch.desired_fov = p.fov;
    }
}
