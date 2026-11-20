/*
 * camera/fight.rs — combat camera mode (rb camnewFight).
 *
 * fight_camera_system runs when the active package camera is a camnewFight.  Port of
 * camera_new/fight.cpp `camnewFight::UpdateChannel`.  The fight camera does
 * NOT chase the player's facing: with the stick idle it holds its azimuth,
 * and only auto-rotates (while standing) to put the closest fighter that is
 * beyond InnerRadius in front of the camera.  Z-lock (forced by the camera
 * set or by the player's fight-mode button) is the only thing that puts the
 * camera behind the focus heading.
 */
use super::channel::{CameraBumpDirection, CameraChannel};
use super::components::CameraController;
use super::follow::track_bump_direction;
use super::polar::{fix_current_azimuth_wrap, radians_difference, wrap_angle};
use crate::oni2_loader::parsers::camera::{CameraClass, CameraParameterSet};
use bevy::prelude::*;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// camnewFight::UpdateChannel (camera_new/fight.cpp:90-212).
pub fn fight_camera_system(
    time: Res<Time>,
    mut camera_query: Query<(&CameraController, &mut CameraChannel)>,
) {
    let dt = time.delta_secs();

    for (controller, mut channel) in &mut camera_query {
        if !super::runs_game_camera(controller.active_mode)
            || channel.params.class != CameraClass::Fight
        {
            continue;
        }
        if !channel.has_snapped_to_target {
            continue;
        }
        let ch = &mut *channel;
        let p = ch.params.clone();
        let stick = ch.cam_stick;
        let use_reticle_x = stick.x.abs() > stick.y.abs();
        let mut used_force_zlock = false;

        // Forced z-lock on the closest fighter (fight.cpp:111-151).
        if p.force_zlock_enabled && !ch.fighters.is_empty() {
            let mut closest = 0;
            let mut closest_d2 = p.outer_radius * p.outer_radius;
            for (i, f) in ch.fighters.iter().enumerate() {
                let d2 = f.distance_squared(ch.current_focus_pos);
                if d2 < closest_d2 {
                    closest_d2 = d2;
                    closest = i;
                }
            }
            ch.is_zlock_targeting = true;
            used_force_zlock = true;
            let enemy = ch.fighters[closest];
            let to_focus = (ch.current_focus_pos - enemy).normalize_or_zero();
            let focus_to_enemy_azimuth = to_focus.x.atan2(to_focus.z);
            let flat_d2 = ch.current_focus_pos.xz().distance_squared(enemy.xz());

            // "kind of a hack for when brunhilda grapples you" (fight.cpp:138)
            if flat_d2 > 0.2 && ch.time_running > 0.0 {
                ch.desired_azimuth = focus_to_enemy_azimuth;
            } else if flat_d2 > 0.2 && ch.time_running == 0.0 {
                ch.desired_azimuth = if ch.current_azimuth > focus_to_enemy_azimuth {
                    focus_to_enemy_azimuth + p.force_zlock_standing_azimuth_offset
                } else {
                    focus_to_enemy_azimuth - p.force_zlock_standing_azimuth_offset
                };
            }

            // Focus back vector . camera back vector < 0: running at the camera.
            let focus_back = Vec3::new(
                ch.current_focus_azimuth.sin(),
                0.0,
                ch.current_focus_azimuth.cos(),
            );
            let cam_back = Vec3::new(
                ch.current_incline.cos() * ch.current_azimuth.sin(),
                ch.current_incline.sin(),
                ch.current_incline.cos() * ch.current_azimuth.cos(),
            );
            if focus_back.dot(cam_back) < 0.0 && ch.time_running > 0.0 {
                ch.time_running_towards_camera += dt;
            } else {
                ch.time_running_towards_camera = 0.0;
            }
        }

        if !used_force_zlock {
            if ch.is_zlock_targeting {
                ch.desired_azimuth = ch.current_focus_azimuth;
            }

            if ch.rotate_until_desired
                && radians_difference(ch.desired_azimuth, ch.current_azimuth).abs() < 0.1
            {
                ch.rotate_until_desired = false;
                ch.bump_direction = CameraBumpDirection::None;
            }

            if !ch.rotate_until_desired
                && !ch.is_zlock_targeting
                && ch.num_local_fighters as i32 >= p.num_enemies_to_trigger
            {
                fight_bump_movement(ch, &p, dt);
                if ch.bump_direction == CameraBumpDirection::None {
                    if stick.x != 0.0 && use_reticle_x {
                        if radians_difference(ch.desired_azimuth, ch.current_azimuth).abs()
                            < FRAC_PI_2
                        {
                            ch.desired_azimuth = wrap_angle(
                                ch.desired_azimuth - stick.x * p.rotation_increment,
                                -PI,
                                PI,
                            );
                        }
                    } else {
                        fight_auto_rotate(ch, &p);
                    }
                }
            }
        }

        fix_current_azimuth_wrap(ch);

        // While colliding the collision response owns incline and distance
        // (fight.cpp:206-210).
        if !ch.is_colliding {
            ch.desired_incline = if stick.y != 0.0 && !use_reticle_x {
                (ch.desired_incline - stick.y * p.rotation_increment)
                    .clamp(p.min_incline, p.max_incline)
            } else {
                p.incline_offset
            };
            ch.desired_distance = fight_distance(ch, &p);
        }
        // camnewRBPolar::GetFOV (rbpolar.h:36).
        ch.desired_fov = if ch.is_colliding {
            p.collision_fov
        } else {
            p.fov
        };
    }
}

/// camnewFight::GetDistance (fight.cpp:386-410).
fn fight_distance(ch: &CameraChannel, p: &CameraParameterSet) -> f32 {
    if p.force_zlock_enabled
        && ch.time_running_towards_camera > p.force_zlock_time_running_towards_cam
    {
        p.force_zlock_distance_running_towards_cam
    } else if ch.is_jumping || ch.time_since_jumping < p.jumping_time {
        p.jumping_distance
    } else {
        p.distance
    }
}

/// Desired azimuth that puts the camera opposite `fighter_angle` (in
/// [0, 2pi)), keeping the lerp direction (fight.cpp:348-354, 283-289).
fn azimuth_opposite(desired: f32, fighter_angle: f32) -> f32 {
    if desired < -PI {
        wrap_angle(fighter_angle - PI, -TAU, 0.0)
    } else if desired > PI {
        wrap_angle(fighter_angle - PI, 0.0, TAU)
    } else {
        wrap_angle(fighter_angle + PI, -PI, PI)
    }
}

fn flat_angle_to(from: Vec3, to: Vec3) -> f32 {
    let v = to - from;
    wrap_angle(v.x.atan2(v.z), 0.0, TAU)
}

/// camnewFight::UpdateChannelAutoRotate (fight.cpp:311-356): only while
/// standing, and only if no fighter is inside InnerRadius — then face the
/// closest fighter inside OuterRadius.
fn fight_auto_rotate(ch: &mut CameraChannel, p: &CameraParameterSet) {
    if ch.time_running > 0.0 {
        return;
    }
    let inner2 = p.inner_radius * p.inner_radius;
    let mut min_d2 = p.outer_radius * p.outer_radius;
    let mut closest = None;
    let mut inside_inner = false;
    for (i, f) in ch.fighters.iter().enumerate() {
        let d2 = f.distance_squared(ch.current_focus_pos);
        if d2 < inner2 {
            inside_inner = true;
        } else if !inside_inner && d2 < min_d2 {
            closest = Some(i);
            min_d2 = d2;
        }
    }
    // rb indexes fighterArray[-1] when nobody is inside OuterRadius; skip.
    if !inside_inner && let Some(i) = closest {
        let angle = flat_angle_to(ch.current_focus_pos, ch.fighters[i]);
        ch.desired_azimuth = azimuth_opposite(ch.desired_azimuth, angle);
    }
}

/// camnewFight::UpdateChannelBumpMovement (fight.cpp:218-305).
fn fight_bump_movement(ch: &mut CameraChannel, p: &CameraParameterSet, dt: f32) {
    let stick = ch.cam_stick;
    let neutral = stick == Vec2::ZERO;
    let mut use_bump = false;

    if neutral {
        use_bump = ch.time_since_reticle_zero > 0.0
            && ch.time_since_reticle_zero < p.bump_max_time
            && ch.bump_is_ready_to_use;
        ch.time_since_reticle_zero = 0.0;
    } else {
        track_bump_direction(ch, stick);
        ch.time_since_reticle_zero += dt;
    }

    if use_bump {
        match ch.bump_direction {
            CameraBumpDirection::PositiveX => ch.desired_azimuth -= p.bump_angle_rotate,
            CameraBumpDirection::NegativeX => ch.desired_azimuth += p.bump_angle_rotate,
            CameraBumpDirection::PositiveY => ch.desired_azimuth -= PI - 0.1,
            CameraBumpDirection::NegativeY => ch.desired_azimuth += PI - 0.1,
            CameraBumpDirection::None => {}
        }
        ch.desired_azimuth = wrap_angle(ch.desired_azimuth, -PI, PI);

        // Snap onto a fighter roughly opposite the camera (fight.cpp:270-292).
        let opposite = wrap_angle(ch.desired_azimuth + PI, 0.0, TAU);
        for i in 0..ch.fighters.len() {
            let f = ch.fighters[i];
            if f.distance(ch.current_focus_pos) < p.outer_radius {
                let angle = flat_angle_to(ch.current_focus_pos, f);
                if (angle - opposite).abs() < p.enemy_focus_threshold_angle {
                    ch.desired_azimuth = azimuth_opposite(ch.desired_azimuth, angle);
                }
            }
        }
        ch.rotate_until_desired = true;
        ch.bump_magnitude = 0.0;
    } else {
        if !ch.rotate_until_desired && ch.time_since_reticle_zero > p.bump_max_time {
            ch.bump_direction = CameraBumpDirection::None;
        }
        if neutral {
            ch.bump_is_ready_to_use = true;
        }
    }
}
