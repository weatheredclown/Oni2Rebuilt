/*
 * camera/components.rs — camera component types.
 *
 * CameraController: active mode (GameNavigation / GameFighting / GameTargeting)
 * and spring parameters.  ActiveCameraMode enum.  DebugFreeCamera for the F5
 * free-fly override.  PrototypeElement / PrototypeVisible for dev-overlay toggle.
 * ScriptCameraSequence / ScriptFocusTarget for scripted camera paths.
 */
use bevy::prelude::*;

/// Marker for prototype gameplay elements (capsules, combat markers, HUD) that can be toggled.
#[derive(Component)]
pub struct PrototypeElement;

/// Resource tracking whether prototype overlay is visible.
#[derive(Resource, Default)]
pub struct PrototypeVisible(pub bool);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraState {
    Game,
    Script,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameCameraMode {
    Navigation,
    Targeting,
    Fighting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugCameraMode {
    Polar,
}

/// The mode selector spanning all possible camera states natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveCameraMode {
    GameNavigation,
    GameTargeting,
    GameFighting,
    Script,
    DebugPolar,
}

impl Default for ActiveCameraMode {
    fn default() -> Self {
        ActiveCameraMode::GameNavigation
    }
}

/// CameraController tracks the active mode and handles mode switching.
#[derive(Component, Default)]
pub struct CameraController {
    pub active_mode: ActiveCameraMode,
    pub next_mode: Option<ActiveCameraMode>,

    // Smooth transition tracking between states
    pub transition_time: f32,
    pub transition_time_remaining: f32,

    // Sub-mode memory

    // Global parameters tracked natively over the manager
    pub fight_mode_radius: f32,
    pub collision_detection_on: bool,
    pub letterbox_mode: bool,
}

/// Unrelated free-flying debug camera component, separate from the main camera.
#[derive(Component, Default)]
pub struct DebugFreeCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub speed: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScriptFocusTarget {
    Actor(Entity),
    Point(Vec3),
}

/// The active instructional timeline state for the camera overriding standard bounds.
#[derive(Component, Default, Clone, Debug)]
pub struct ScriptCameraSequence {
    // Spatial Rail (from LayoutPaths via CameraMoveAlongRail)
    pub active_rail: Option<Vec<Vec3>>, // Extracted spline path
    pub rail_start_pos: Option<Vec3>,   // Original camera position before spline start
    pub active_rail_name: Option<String>,
    pub rail_duration: f32,
    pub rail_time_elapsed: f32,

    // Smooth Look-At Tracker (CameraTrackActor / CameraTrackPoint)
    pub tracked_target: Option<ScriptFocusTarget>,

    // Explicit Camera Positions over time (CameraMoveToPoint / CameraMoveToActor)
    pub move_start: Option<Vec3>,
    pub move_target: Option<ScriptFocusTarget>,
    pub move_duration: f32,
    pub move_time_elapsed: f32,

    pub fov_start: Option<f32>,
    pub fov_target: Option<f32>,
    pub fov_duration: f32,
    pub fov_time_elapsed: f32,
}
