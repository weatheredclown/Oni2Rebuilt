/*
 * oni2_loader/parsers/camera.rs — camera package and parameter-set parsers.
 *
 * CameraPackageDef: per-area package linking navigation / targeting / fighting
 * parameter set names.  CameraParameterSet: numeric tuning values (FOV, distance,
 * incline offset, lerp rates, dead zones) fed into CameraChannel each frame.
 */
use crate::oni2_loader::utils::parse::parse_f32;
use crate::oni2_loader::utils::space;
use bevy::prelude::info;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct CameraPackageDef {
    pub name: String,
    pub navigation: String,
    pub targeting: String,
    pub fighting: String,
    pub fight_mode_radius: f32,
    pub fight_mode_running_away_time: f32,
}

/// Which camera_new class a parameter set belongs to (the root tag of its
/// cam_*.xml / template_cameranew_*.xml).  rb picks the template by class
/// (camera_new/camera.cpp `camnewCamera::Init`), so the class decides every
/// value the layout file leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraClass {
    #[default]
    Follow,
    Fight,
    Targeting,
    RBPolar,
    RBThirdPerson,
}

impl CameraClass {
    fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "camnewFollow" => Self::Follow,
            "camnewFight" => Self::Fight,
            "camnewTargeting" => Self::Targeting,
            "camnewRBPolar" | "camnewPolar" => Self::RBPolar,
            "camnewRBThirdPerson" => Self::RBThirdPerson,
            _ => return None,
        })
    }

    fn template_file(self) -> &'static str {
        match self {
            Self::Follow => "template_cameranew_follow.xml",
            Self::Fight => "template_cameranew_fight.xml",
            Self::Targeting => "template_cameranew_targeting.xml",
            Self::RBPolar => "template_cameranew_rbpolar.xml",
            Self::RBThirdPerson => "template_cameranew_rbthirdperson.xml",
        }
    }
}

/// One camera_new parameter set: the class template's attributes overlaid
/// with the layout's cam_*.xml.  Positions/offsets are Bevy-space, focus
/// offsets relative to the actor root (which sits 1 m above the feet).
#[derive(Debug, Clone)]
pub struct CameraParameterSet {
    pub name: String,
    pub class: CameraClass,
    // camnewCamera
    pub fov: f32,
    pub focus_offset: [f32; 3],
    pub collision_sphere_radius: f32,
    pub lerp_rate_focus_point: f32,
    pub lerp_rate_focus_offset: f32,
    // camnewPolar
    pub distance: f32,
    pub max_azimuth: f32,
    pub min_incline: f32,
    pub max_incline: f32,
    pub incline_offset: f32,
    pub analog_x_start: f32,
    pub analog_y_start: f32,
    pub rotation_increment: f32,
    pub lerp_rate_fov: f32,
    pub lerp_rate_distance: f32,
    pub lerp_rate_azimuth: f32,
    pub lerp_rate_incline: f32,
    // camnewRBPolar
    pub jumping_time: f32,
    pub jumping_distance: f32,
    pub collision_focus_offset: [f32; 3],
    pub collision_fov: f32,
    pub collision_incline_offset: f32,
    // camnewFollow
    pub incline_offset_running: f32,
    pub distance_running_towards_cam: f32,
    pub dead_zone_inner_radius: f32,
    pub dead_zone_outer_radius: f32,
    pub zone_angles: [f32; 3],
    pub lerp_rate_azimuth_zone1: f32,
    pub lerp_rate_azimuth_zone2: f32,
    pub lerp_rate_azimuth_zone3: f32,
    pub lerp_rate_azimuth_zone4: f32,
    pub lerp_rate_spin_threshold: f32,
    pub min_lock_heading_zone: i32,
    pub lock_heading_until_move: bool,
    pub spin_threshold: f32,
    pub bump_angle_rotate: f32,
    pub bump_max_time: f32,
    // camnewFight
    pub lerp_rate_bump_azimuth: f32,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub enemy_focus_threshold_angle: f32,
    pub num_enemies_to_trigger: i32,
    pub force_zlock_enabled: bool,
    pub force_zlock_distance_running_towards_cam: f32,
    pub force_zlock_time_running_towards_cam: f32,
    pub force_zlock_standing_azimuth_offset: f32,
    // camnewRBThirdPerson
    pub angle_running_towards_camera: f32,
    pub lerp_rate_follow_azimuth: f32,
}

/// The actor root sits this far above the feet; rb focus offsets are from the feet.
const ROOT_HEIGHT: f32 = 1.0;

impl Default for CameraParameterSet {
    /// camnewRBPolar::InitDefaults (camera_new/rbpolar.cpp:80, polar.cpp:64,
    /// camera.cpp) — only used where neither template nor layout file speaks.
    fn default() -> Self {
        Self {
            name: String::new(),
            class: CameraClass::Follow,
            fov: 70.0,
            focus_offset: [0.0, 1.4 - ROOT_HEIGHT, 0.0],
            collision_sphere_radius: 1.0,
            lerp_rate_focus_point: 8.0,
            lerp_rate_focus_offset: 2.0,
            distance: 2.0,
            max_azimuth: std::f32::consts::PI,
            min_incline: 0.0,
            max_incline: 70.0_f32.to_radians(),
            incline_offset: 0.0,
            analog_x_start: 0.3,
            analog_y_start: 0.3,
            rotation_increment: 0.1,
            lerp_rate_fov: 2.0,
            lerp_rate_distance: 8.0,
            lerp_rate_azimuth: 3.0,
            lerp_rate_incline: 2.0,
            jumping_time: 2.0,
            jumping_distance: 4.5,
            collision_focus_offset: [0.0, 1.4 - ROOT_HEIGHT, 0.0],
            collision_fov: 70.0,
            collision_incline_offset: 0.0,
            incline_offset_running: 20.0_f32.to_radians(),
            distance_running_towards_cam: 4.0,
            dead_zone_inner_radius: 1.5,
            dead_zone_outer_radius: 4.0,
            zone_angles: [
                20.0_f32.to_radians(),
                90.0_f32.to_radians(),
                120.0_f32.to_radians(),
            ],
            lerp_rate_azimuth_zone1: 2.0,
            lerp_rate_azimuth_zone2: 3.0,
            lerp_rate_azimuth_zone3: 3.0,
            lerp_rate_azimuth_zone4: 3.0,
            lerp_rate_spin_threshold: 2.4,
            min_lock_heading_zone: 4,
            lock_heading_until_move: false,
            spin_threshold: 0.3,
            bump_angle_rotate: std::f32::consts::PI,
            bump_max_time: 0.2,
            lerp_rate_bump_azimuth: 5.0,
            inner_radius: 4.0,
            outer_radius: 8.0,
            enemy_focus_threshold_angle: 10.0_f32.to_radians(),
            num_enemies_to_trigger: 1,
            force_zlock_enabled: false,
            force_zlock_distance_running_towards_cam: 6.0,
            force_zlock_time_running_towards_cam: 0.5,
            force_zlock_standing_azimuth_offset: 30.0_f32.to_radians(),
            angle_running_towards_camera: 2.7,
            lerp_rate_follow_azimuth: 0.75,
        }
    }
}

/// Parses layout.campacknew into a dictionary of CameraPackageDefs
///
/// Also returns the name of the first package in the file: rb makes the head
/// of its package list the current game package (camera_new/manager.cpp:1088).
pub fn parse_campacknew(dir: &str) -> (HashMap<String, CameraPackageDef>, Option<String>) {
    let mut packages = HashMap::new();
    // Default magic package based on legacy defaults.
    packages.insert(
        "DEFAULT_PACKAGE".to_string(),
        CameraPackageDef {
            name: "DEFAULT_PACKAGE".to_string(),
            // camnewPackage::LoadDefaultPackage (camera_new/package.cpp:48-62):
            // the third-person camera for both navigation and fighting.
            navigation: "DEFAULT_RBTHIRDPERSON".to_string(),
            targeting: "DEFAULT_TARGETING".to_string(),
            fighting: "DEFAULT_RBTHIRDPERSON".to_string(),
            fight_mode_radius: 5.0,
            fight_mode_running_away_time: 2.0,
        },
    );

    let content = match crate::vfs::read_to_string(dir, "layout.campacknew") {
        Ok(c) => c,
        Err(_) => return (packages, None),
    };

    let mut current_package: Option<CameraPackageDef> = None;
    let mut first: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("CAMERANEW_PACKAGE") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts[1].trim_matches('"').to_string();
                current_package = Some(CameraPackageDef {
                    name,
                    navigation: String::new(),
                    targeting: String::new(),
                    fighting: String::new(),
                    fight_mode_radius: 0.0,
                    fight_mode_running_away_time: 0.0,
                });
            }
        } else if trimmed == "{" {
            continue;
        } else if trimmed == "}" {
            if let Some(pkg) = current_package.take() {
                first.get_or_insert_with(|| pkg.name.clone());
                packages.insert(pkg.name.clone(), pkg);
            }
        } else if let Some(ref mut pkg) = current_package {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                let key = parts[0];
                let value = parts[1].trim_matches('"');
                match key {
                    "Navigation" => pkg.navigation = value.to_string(),
                    "Targeting" => pkg.targeting = value.to_string(),
                    "Fighting" => pkg.fighting = value.to_string(),
                    "FightModeRadius" => pkg.fight_mode_radius = value.parse().unwrap_or(0.0),
                    "FightModeRunningAwayTime" => {
                        pkg.fight_mode_running_away_time = value.parse().unwrap_or(0.0)
                    }
                    _ => {}
                }
            }
        }
    }

    (packages, first)
}

/// Root tag of a camera xml (`<camnewFight>` -> Fight).
fn root_class(content: &str) -> Option<CameraClass> {
    content
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("<camnew"))
        .and_then(|l| {
            let tag = l.trim_start_matches('<').trim_end_matches('>');
            CameraClass::from_tag(tag.split_whitespace().next().unwrap_or(""))
        })
}

/// Parses a cam_*.xml file into a CameraParameterSet.  Like rb's
/// `camnewCamera::Init` (camera_new/camera.cpp:113-160) the class template
/// (template/template_cameranew_<class>.xml) is loaded first and the layout
/// file overlays it.  The built-in DEFAULT_* cameras have no layout file and
/// are just their template.
pub fn parse_camera_xml(dir: &str, filename: &str) -> Option<CameraParameterSet> {
    let mut params = CameraParameterSet::default();

    let layout_content = crate::vfs::read_to_string(dir, filename).ok();
    let class = match &layout_content {
        Some(c) => root_class(c).unwrap_or_default(),
        None => match filename.trim_end_matches(".xml") {
            "DEFAULT_FOLLOW" => CameraClass::Follow,
            "DEFAULT_FIGHT" => CameraClass::Fight,
            "DEFAULT_TARGETING" => CameraClass::Targeting,
            "DEFAULT_RBTHIRDPERSON" => CameraClass::RBThirdPerson,
            _ => return None,
        },
    };
    params.class = class;

    match crate::vfs::read_to_string("template", class.template_file()) {
        Ok(t) => apply_attributes(&mut params, &t),
        Err(_) => info!("Camera template {} not found", class.template_file()),
    }
    if let Some(c) = &layout_content {
        apply_attributes(&mut params, c);
    }
    if class == CameraClass::RBThirdPerson {
        // camnewRBThirdPerson runs camnewPolar::InitDefaults and only loads
        // camnewPolar attributes (rbthirdperson.cpp:42-58), so its
        // camnewRBPolar collision/jump members are never initialised; its
        // UpdateCollision even shadows CollisionInclineOffset with
        // InclineOffset ("ACK", rbthirdperson.cpp:172-174).  Use the neutral
        // values that intent implies.
        params.collision_incline_offset = params.incline_offset;
        params.collision_focus_offset = params.focus_offset;
        params.collision_fov = params.fov;
        params.jumping_time = 0.0;
        params.jumping_distance = params.distance;
    }
    Some(params)
}

fn apply_attributes(params: &mut CameraParameterSet, content: &str) {
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("<m_") {
            continue;
        }

        // Example: <m_FOV type="double" value="50.000"/>
        let name_start = 3;
        let name_end = trimmed[name_start..]
            .find(' ')
            .map_or(trimmed.len(), |i| i + name_start);
        let name = &trimmed[name_start..name_end];

        let value_start = match trimmed.find("value=\"") {
            Some(idx) => idx + 7,
            None => continue,
        };
        let value_end = match trimmed[value_start..].find('"') {
            Some(idx) => idx + value_start,
            None => continue,
        };
        let value_str = &trimmed[value_start..value_end];
        let f = |d: f32| parse_f32(value_str, d);
        let b = value_str == "1" || value_str.eq_ignore_ascii_case("true");
        let offset = || -> Option<[f32; 3]> {
            let parts: Vec<f32> = value_str
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            (parts.len() >= 3).then(|| {
                let v = space::to_bevy_space_pos(&parts);
                [v.x, v.y - ROOT_HEIGHT, v.z]
            })
        };

        match name {
            "Name" => params.name = value_str.to_string(),
            "FOV" => params.fov = f(params.fov),
            "FocusOffset" => params.focus_offset = offset().unwrap_or(params.focus_offset),
            "CollisionSphereRadius" => params.collision_sphere_radius = f(1.0),
            "LerpRateFocusPoint" => params.lerp_rate_focus_point = f(8.0),
            "LerpRateFocusOffset" => params.lerp_rate_focus_offset = f(2.0),
            "Distance" => params.distance = f(params.distance),
            "MaxAzimuth" => params.max_azimuth = f(params.max_azimuth),
            "MinIncline" => {
                params.min_incline = space::oni2_camera_incline_to_bevy(f(params.min_incline))
            }
            "MaxIncline" => {
                params.max_incline = space::oni2_camera_incline_to_bevy(f(params.max_incline))
            }
            "InclineOffset" => params.incline_offset = space::oni2_camera_incline_to_bevy(f(0.0)),
            "AnalogXStart" => params.analog_x_start = f(0.3),
            "AnalogYStart" => params.analog_y_start = f(0.3),
            "RotationIncrement" => params.rotation_increment = f(0.1),
            "LerpRateFOV" => params.lerp_rate_fov = f(params.lerp_rate_fov),
            "LerpRateDistance" => params.lerp_rate_distance = f(params.lerp_rate_distance),
            "LerpRateAzimuth" => params.lerp_rate_azimuth = f(params.lerp_rate_azimuth),
            "LerpRateIncline" => params.lerp_rate_incline = f(params.lerp_rate_incline),
            "JumpingTime" => params.jumping_time = f(params.jumping_time),
            "JumpingDistance" => params.jumping_distance = f(params.jumping_distance),
            "CollisionFocusOffset" => {
                params.collision_focus_offset = offset().unwrap_or(params.collision_focus_offset)
            }
            "CollisionFOV" => params.collision_fov = f(params.collision_fov),
            "CollisionInclineOffset" => {
                params.collision_incline_offset = space::oni2_camera_incline_to_bevy(f(0.0))
            }
            "InclineOffsetRunning" => {
                params.incline_offset_running = space::oni2_camera_incline_to_bevy(f(0.0))
            }
            "DistanceRunningTowardsCam" => params.distance_running_towards_cam = f(4.0),
            "DeadZoneInnerRadius" => params.dead_zone_inner_radius = f(0.0),
            "DeadZoneOuterRadius" => params.dead_zone_outer_radius = f(0.0),
            "Zone1Angle" => params.zone_angles[0] = f(params.zone_angles[0]),
            "Zone2Angle" => params.zone_angles[1] = f(params.zone_angles[1]),
            "Zone3Angle" => params.zone_angles[2] = f(params.zone_angles[2]),
            "LerpRateAzimuthZone1" => params.lerp_rate_azimuth_zone1 = f(0.0),
            "LerpRateAzimuthZone2" => params.lerp_rate_azimuth_zone2 = f(0.0),
            "LerpRateAzimuthZone3" => params.lerp_rate_azimuth_zone3 = f(0.0),
            "LerpRateAzimuthZone4" => params.lerp_rate_azimuth_zone4 = f(0.0),
            "LerpRateSpinThreshold" => params.lerp_rate_spin_threshold = f(2.4),
            "MinLockHeadingZone" => params.min_lock_heading_zone = value_str.parse().unwrap_or(4),
            "LockHeadingUntilMove" => params.lock_heading_until_move = b,
            "SpinThreshold" => params.spin_threshold = f(0.0),
            "BumpAngleRotate" => params.bump_angle_rotate = f(params.bump_angle_rotate),
            "BumpMaxTime" => params.bump_max_time = f(0.2),
            "LerpRateBumpAzimuth" => params.lerp_rate_bump_azimuth = f(5.0),
            "InnerRadius" => params.inner_radius = f(0.0),
            "OuterRadius" => params.outer_radius = f(0.0),
            "EnemyFocusThresholdAngle" => params.enemy_focus_threshold_angle = f(0.0),
            "NumEnemiesToTrigger" => params.num_enemies_to_trigger = value_str.parse().unwrap_or(1),
            "ForceZLockEnabled" => params.force_zlock_enabled = b,
            "ForceZLockDistanceRunningTowardsCam" => {
                params.force_zlock_distance_running_towards_cam = f(6.0)
            }
            "ForceZLockTimeRunningTowardsCam" => {
                params.force_zlock_time_running_towards_cam = f(0.5)
            }
            "ForceZLockStandingAzimuthOffset" => {
                params.force_zlock_standing_azimuth_offset = f(0.4)
            }
            "AngleRunningTowardsCamera" => params.angle_running_towards_camera = f(2.7),
            "LerpRateFollowAzimuth" => params.lerp_rate_follow_azimuth = f(0.75),
            _ => {}
        }
    }
}
