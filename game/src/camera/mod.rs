/*
 * camera/mod.rs — CameraPlugin: multi-mode camera rig.
 *
 * Camera pipeline runs in PostUpdate after `TransformSystems::Propagate`
 * so it reads CURRENT-frame `GlobalTransform`s for the player + targets,
 * not last-frame's stale values.  Order within the pass:
 *   update_camera_channel → mode evaluators (follow / fight / script /
 *   targeting) → polar_interpolation_system → apply_camera_transform.
 *
 * Why PostUpdate:
 *   - Avian's physics writes player `Transform` in FixedUpdate
 *   - `update_oni2_animation` writes bone Transforms in Update
 *   - Bevy propagates GlobalTransforms in PostUpdate (TransformSystems::Propagate)
 * Reading any of those before propagation gives the previous frame's
 * `GlobalTransform`, which at high render rates manifested as the
 * camera "stepping" at FixedUpdate's 60 Hz cadence (i.e. 30 Hz at
 * 144 Hz render).
 *
 * `apply_camera_transform` writes the camera's own `Transform` directly;
 * the camera has no children whose GlobalTransform we'd need
 * propagation to fix this same frame, so render extract picks it up
 * cleanly from PostUpdate's tail.
 *
 * `prototype_toggle_system`, `camera_mode_toggle_system`,
 * `camera_mode_transition_tick`, `freecam_system`, and
 * `debug_render_camera_targets` stay in Update — they're input /
 * timer / gizmo systems, not transform-tracking, so PostUpdate ordering
 * doesn't buy them anything.
 */
pub mod channel;
pub mod components;
pub mod fight;
pub mod follow;
pub mod freecam;
pub mod polar;
pub mod script;
pub mod systems;
pub mod targeting;

use bevy::prelude::*;
use bevy::transform::TransformSystems;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<components::PrototypeVisible>()
            // Input / debug / timer systems — Update is fine.
            .add_systems(
                Update,
                (
                    systems::camera_mode_transition_tick,
                    systems::camera_mode_toggle_system,
                    systems::prototype_toggle_system,
                    systems::debug_render_camera_targets,
                    freecam::freecam_system,
                ),
            )
            // Transform-tracking systems — PostUpdate after propagation.
            .add_systems(
                PostUpdate,
                (
                    systems::update_camera_channel,
                    follow::follow_camera_system.after(systems::update_camera_channel),
                    fight::fight_camera_system.after(systems::update_camera_channel),
                    script::script_camera_system.after(systems::update_camera_channel),
                    targeting::targeting_camera_system.after(systems::update_camera_channel),
                    polar::polar_interpolation_system
                        .after(follow::follow_camera_system)
                        .after(fight::fight_camera_system)
                        .after(script::script_camera_system)
                        .after(targeting::targeting_camera_system),
                    polar::apply_camera_transform.after(polar::polar_interpolation_system),
                )
                    .after(TransformSystems::Propagate),
            );
    }
}
