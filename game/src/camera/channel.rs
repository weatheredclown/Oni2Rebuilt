/*
 * camera/channel.rs — CameraChannel blackboard component.
 *
 * Sensors (update_camera_channel) write
 * current_focus_pos, current_focus_azimuth, is_moving, package_* parameters
 * etc.  Mode evaluators (follow, fight, script, targeting) read those and write
 * desired_azimuth / desired_incline / desired_distance / desired_fov.
 * FocusHeadingZone and CameraBumpDirection enums are also defined here.
 */
use bevy::prelude::*;
use crate::oni2_loader::utils::space;

/// Describes the heading direction of the focus actor relative to the camera heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusHeadingZone {
    #[default]
    Zone1, // Heading forward, away from camera, typically +/- 15 degrees
    Zone2, // Making a slight turn
    Zone3, // Making a large turn, typically 90 degrees or greater
    Zone4, // Turning around and heading towards the camera
}

/// The direction the right analog stick is being flicked for an evasive bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraBumpDirection {
    PositiveX, // Stick bumped right
    NegativeX, // Stick bumped left
    PositiveY, // Stick bumped up
    NegativeY, // Stick bumped down
    #[default]
    None,      // No bump
}

/// A data-only blackboard component for the camera rig.
/// It acts as a blackboard that physical sensors update and specific camera sub-modes
/// read from and write `desired_*` values into.
#[derive(Component, Clone, Debug)]
pub struct CameraChannel {
    // Spatial Output Properties (Written by Mode, Applied by Polar System)
    pub current_focus_pos: Vec3,
    pub previous_focus_pos: Vec3,
    pub desired_focus_pos: Vec3,
    pub current_focus_offset: Vec3,
    pub desired_focus_offset: Vec3,
    
    // Polar Output coordinates 
    pub current_azimuth: f32,
    pub current_incline: f32,
    pub current_distance: f32,
    pub desired_azimuth: f32,
    pub desired_incline: f32,
    pub desired_distance: f32,
    
    pub current_fov: f32,
    pub desired_fov: f32,

    // Target tracking 
    pub focus_actor: Entity, // Equivalent to m_FocusActor
    
    // Shared Temporal Input Properties (Written by Sensor/Input System, Read by Mode)
    pub reticle_x: f32, // Right stick X
    pub reticle_y: f32, // Right stick Y
    
    pub time_running: f32, // Cumulative time actor is running
    pub time_remaining_lock_heading: f32, // Time to lock heading during fight->follow switch
    pub time_running_towards_camera: f32, // Consecutive time running exactly towards camera
    pub focus_camera_heading_diff: f32, // Diff between focus heading and camera heading
    pub time_since_reticle_zero: f32, // Idle right-stick time (for bump enable)
    
    pub bump_magnitude: f32,
    pub current_focus_azimuth: f32,
    pub previous_focus_azimuth: f32,
    
    pub focus_heading_zone: FocusHeadingZone,
    pub bump_direction: CameraBumpDirection,
    
    // State Flags
    pub is_moving: bool,
    pub is_zlock_targeting: bool,
    pub bump_is_ready_to_use: bool,
    pub rotate_until_desired: bool,
    pub is_colliding: bool,
    
    // Action State Flags (Affect incline/distance automatically later)
    pub is_jumping: bool,
    pub jump_transition_out: bool,
    pub time_since_jumping: f32,
    
    pub is_sliding: bool,
    pub sliding_transition_out: bool,
    pub time_since_sliding: f32,
    
    pub spin_threshold_exceeded: bool,
    
    // Active Camera Package Parameters (populated automatically by update_camera_channel)
    pub package_fov: f32,
    pub package_distance: f32,
    pub package_incline_offset: f32,
    pub package_incline_offset_running: f32,
    pub package_inner_radius: f32,
    pub package_outer_radius: f32,
    pub package_focus_offset: Vec3,
    pub package_spin_threshold: f32,
    pub package_zone_lerp_rates: [f32; 4],
    pub active_set_name: String,
    
    // Bypass constraints entirely if VM script forces explicit camera matrix
    pub script_override_transform: Option<Transform>,

    // Camera shake state (driven by CameraShake script command or fx events)
    pub shake_amplitude: Vec3,       // Per-axis Euler rotation amplitude (radians)
    pub shake_time_remaining: f32,
    pub shake_time_to_damp: f32,     // Shake starts damping below this remaining time

    // Script FOV override: when Some, polar interpolation defers to this instead of package_fov
    pub script_fov_override: Option<f32>,
}

impl Default for CameraChannel {
    fn default() -> Self {
        Self {
            current_focus_pos: Vec3::ZERO,
            previous_focus_pos: Vec3::ZERO,
            desired_focus_pos: Vec3::ZERO,
            current_focus_offset: Vec3::new(0.0, 1.4, 0.0),
            desired_focus_offset: Vec3::new(0.0, 1.4, 0.0),
            
            current_azimuth: 0.0,
            current_incline: 0.0,
            current_distance: 5.0,
            desired_azimuth: 0.0,
            desired_incline: 0.0,
            desired_distance: 5.0,
            
            current_fov: 50.0,
            desired_fov: 50.0,
            
            focus_actor: Entity::PLACEHOLDER,
            
            reticle_x: 0.0,
            reticle_y: 0.0,
            
            time_running: 0.0,
            time_remaining_lock_heading: 0.0,
            time_running_towards_camera: 0.0,
            focus_camera_heading_diff: 0.0,
            time_since_reticle_zero: 0.0,
            
            bump_magnitude: 0.0,
            current_focus_azimuth: 0.0,
            previous_focus_azimuth: 0.0,
            
            focus_heading_zone: FocusHeadingZone::default(),
            bump_direction: CameraBumpDirection::default(),
            
            is_moving: false,
            is_zlock_targeting: false,
            bump_is_ready_to_use: true,
            rotate_until_desired: false,
            is_colliding: false,
            
            is_jumping: false,
            jump_transition_out: false,
            time_since_jumping: 0.0,
            
            is_sliding: false,
            sliding_transition_out: false,
            time_since_sliding: 0.0,
            
            spin_threshold_exceeded: false,
            
            package_fov: 50.0,
            package_distance: 5.0,
            package_incline_offset: space::oni2_camera_incline_to_bevy(10.0_f32.to_radians()),
            package_incline_offset_running: space::oni2_camera_incline_to_bevy(20.0_f32.to_radians()),
            package_inner_radius: 1.5,
            package_outer_radius: 4.0,
            package_focus_offset: Vec3::new(0.0, 1.4, 0.0),
            package_spin_threshold: 63.0_f32.to_radians(),
            package_zone_lerp_rates: [2.0, 3.0, 3.0, 3.0],
            active_set_name: String::new(),
            
            script_override_transform: None,

            shake_amplitude: Vec3::ZERO,
            shake_time_remaining: 0.0,
            shake_time_to_damp: 0.0,

            script_fov_override: None,
        }
    }
}
