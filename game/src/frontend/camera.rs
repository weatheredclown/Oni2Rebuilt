/*
 * frontend/camera.rs — ScrOni-driven menu camera control.
 *
 * The menu camera (`FrontendMenuCamera` on a bare `Camera3d`) doesn't
 * share the gameplay camera stack — no `CameraController`, no
 * `CameraChannel`, no `apply_camera_transform` polar pipeline.  The
 * existing `scroni::system_bindings::scroni_sys_event_observer`
 * routes `ScrOniSysEvent::Camera*` into a `Query<&mut CameraController, ...>`
 * and skips entities without one, so the menu camera was unreachable
 * by the `$newgame:CameraMotion` script that drives Main_Menu framing.
 *
 * This module adds a parallel observer + apply system that mirrors the
 * `ScrOniSysEvent::Camera*` handling for the menu camera only,
 * lerping `Transform` (and `Projection` FOV) directly.  Scope is the
 * subset of camera ops that `CameraMotion` actually issues:
 * `CameraMoveToPoint`, `CameraTrackPoint`, `CameraSetFOV`, plus the
 * cut variants (zero-duration moves).  Rail / actor-track aren't used
 * by the shipped menu page, so they're elided — add later if a future
 * PAGE_3D needs them.
 *
 * All inputs come pre-converted to Bevy space because the wider
 * sys-event observer applies `to_bevy_space_pos` before issuing the
 * trigger.  We stay in Bevy space throughout.
 */
use bevy::prelude::*;

use super::render::FrontendMenuCamera;
use crate::oni2_loader::utils::space;
use crate::scroni::vm::ScrOniSysEvent;

/// Per-frame lerp state owned by the menu camera.  Mirrors the subset
/// of `crate::camera::components::ScriptCameraSequence` that
/// `CameraMotion` actually drives.  Each ScrOni op overwrites the
/// relevant slots; absent slots keep whatever the previous op set.
#[derive(Component, Default)]
pub struct FrontendMenuCameraSeq {
    /// Position lerp.  `from` lazily captured from the camera's
    /// current `Transform.translation` on the first apply tick after
    /// a new move starts (matches `script_camera_system`'s laziness).
    pub move_from: Option<Vec3>,
    pub move_to: Option<Vec3>,
    pub move_duration: f32,
    pub move_elapsed: f32,

    /// Look-at point.  No interpolation — the script re-issues this
    /// every cycle, so a snap aim each event boundary is what the
    /// menu actually wants (avoids look-at chasing its own tail).
    pub track_point: Option<Vec3>,

    /// FOV lerp on the camera's `Projection::Perspective`.
    pub fov_from: Option<f32>,
    pub fov_to: Option<f32>,
    pub fov_duration: f32,
    pub fov_elapsed: f32,
}

/// Observer for `ScrOniSysEvent::Camera*` that lands the request on
/// the menu camera (if one is alive) — runs in parallel to the
/// gameplay observer in `scroni::system_bindings`, which still
/// services every `CameraController`-tagged camera independently.
///
/// Inputs are in raw script space (Oni2); we apply the same
/// `to_bevy_space_pos` flip the gameplay observer does so the menu
/// camera lands at the same world point a gameplay camera would.
pub fn frontend_menu_camera_scroni_observer(
    trigger: On<ScrOniSysEvent>,
    mut camera_q: Query<&mut FrontendMenuCameraSeq, With<FrontendMenuCamera>>,
) {
    let Ok(mut seq) = camera_q.single_mut() else {
        return;
    };
    match (*trigger).clone() {
        ScrOniSysEvent::CameraTrackPoint(pt) => {
            seq.track_point = Some(space::to_bevy_space_pos(pt));
        }
        ScrOniSysEvent::CameraMoveToPoint(pt, dur) => {
            seq.move_from = None; // lazily captured on apply
            seq.move_to = Some(space::to_bevy_space_pos(pt));
            seq.move_duration = dur;
            seq.move_elapsed = 0.0;
        }
        ScrOniSysEvent::CameraSetFOV(fov, dur) => {
            seq.fov_from = None; // lazily captured on apply
            seq.fov_to = Some(fov);
            seq.fov_duration = dur;
            seq.fov_elapsed = 0.0;
        }
        _ => {} // other ScrOni events aren't menu-camera concerns
    }
}

/// Per-frame apply: lerp position + FOV toward targets, then
/// `look_at` the track_point.  No-op until the script lands an event.
pub fn frontend_menu_camera_apply_seq(
    time: Res<Time>,
    mut camera_q: Query<
        (&mut Transform, &mut FrontendMenuCameraSeq, &mut Projection),
        With<FrontendMenuCamera>,
    >,
) {
    let dt = time.delta_secs();
    let Ok((mut tf, mut seq, mut projection)) = camera_q.single_mut() else {
        return;
    };

    // 1. Position lerp.
    if let Some(target) = seq.move_to {
        if seq.move_from.is_none() {
            seq.move_from = Some(tf.translation);
        }
        seq.move_elapsed += dt;
        let t = if seq.move_duration > 0.0 {
            (seq.move_elapsed / seq.move_duration).clamp(0.0, 1.0)
        } else {
            1.0
        };
        if let Some(from) = seq.move_from {
            tf.translation = from.lerp(target, t);
        }
        if t >= 1.0 {
            seq.move_to = None;
            seq.move_from = None;
        }
    }

    // 2. Look-at (snap; no interpolation).  Skip the degenerate case
    // where the camera sits on the track point (zero forward vector).
    if let Some(target) = seq.track_point {
        let pos = tf.translation;
        let forward = target - pos;
        if forward.length_squared() > 1e-6
            && forward.normalize().dot(Vec3::Y).abs() < 0.999
        {
            tf.look_at(target, Vec3::Y);
        }
    }

    // 3. FOV lerp on the perspective projection — only Perspective is
    // supported here (the spawn path always inserts Perspective), but
    // be defensive in case a future caller swaps to Orthographic.
    if let Some(target) = seq.fov_to {
        if let Projection::Perspective(ref mut persp) = *projection {
            if seq.fov_from.is_none() {
                seq.fov_from = Some(persp.fov);
            }
            seq.fov_elapsed += dt;
            let t = if seq.fov_duration > 0.0 {
                (seq.fov_elapsed / seq.fov_duration).clamp(0.0, 1.0)
            } else {
                1.0
            };
            if let Some(from) = seq.fov_from {
                persp.fov = from + (target - from) * t;
            }
            if t >= 1.0 {
                seq.fov_to = None;
                seq.fov_from = None;
            }
        }
    }
}
