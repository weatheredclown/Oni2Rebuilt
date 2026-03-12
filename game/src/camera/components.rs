use bevy::prelude::*;

/// Marker for prototype gameplay elements (capsules, combat markers, HUD) that can be toggled with F6.
#[derive(Component)]
pub struct PrototypeElement;

/// Resource tracking whether prototype overlay is visible.
#[derive(Resource)]
pub struct PrototypeVisible(pub bool);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    /// Classic mouse-look camera (rotates with player mouse movement)
    MouseLook,
    /// Console-friendly smart camera (zone-based auto-follow)
    SmartFollow,
    /// Free-fly camera (WASD + mouse, detached from player)
    FreeCam,
}

/// Camera rig supporting two modes: mouse-look and zone-based smart follow.
/// Toggle with Tab key.
#[derive(Component)]
pub struct CameraRig {
    pub target: Entity,
    pub mode: CameraMode,

    // === Mouse-look mode fields ===
    pub offset: Vec3,
    pub mouse_lerp_speed: f32,

    // === Smart-follow mode fields (4-zone system from rb's camnewFollow) ===
    pub current_azimuth: f32,
    pub target_azimuth: f32,
    /// Zone boundaries in radians: [20deg, 90deg, 120deg]
    pub zone_thresholds: [f32; 3],
    /// Lerp rates per zone (degrees/sec equivalent)
    pub zone_lerp_rates: [f32; 4],
    /// Sharp turn threshold - reset dead zone on rapid spin
    pub spin_threshold: f32,
    /// Inner dead zone radius - camera doesn't move
    pub dead_zone_inner: f32,
    /// Outer dead zone radius - transition zone
    pub dead_zone_outer: f32,
    /// Incline pitch offset
    pub incline_offset: f32,
    /// Base distance behind target
    pub follow_distance: f32,
    /// Height above target
    pub height: f32,
    /// Accumulated bump rotation (right-stick flick equivalent)
    pub bump_angle: f32,
    /// Bump decay rate
    pub bump_lerp_rate: f32,

    // === Free-cam mode fields ===
    pub free_yaw: f32,
    pub free_pitch: f32,
    pub free_speed: f32,
    /// The mode to return to when exiting free cam
    pub pre_free_mode: Option<CameraMode>,
}
