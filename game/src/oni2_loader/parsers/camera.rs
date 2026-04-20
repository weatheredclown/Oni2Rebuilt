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

#[derive(Debug, Clone)]
pub struct CameraParameterSet {
    pub name: String,
    pub fov: f32,
    pub distance: f32,
    pub incline_offset: f32,
    pub incline_offset_running: f32,
    pub dead_zone_inner_radius: f32,
    pub dead_zone_outer_radius: f32,
    pub lerp_rate_azimuth_zone1: f32,
    pub lerp_rate_azimuth_zone2: f32,
    pub lerp_rate_azimuth_zone3: f32,
    pub lerp_rate_azimuth_zone4: f32,
    pub lock_heading_until_move: bool,
    pub spin_threshold: f32,
    pub focus_offset: [f32; 3],
    pub inner_radius: f32,
    pub outer_radius: f32,
}

impl Default for CameraParameterSet {
    fn default() -> Self {
        Self {
            name: String::new(),
            fov: 50.0,
            distance: 3.0,
            // Base polar defaults
            focus_offset: [0.0, 1.4 - 1.0, 0.0],
            incline_offset: 0.0,
            incline_offset_running: 0.0,
            dead_zone_inner_radius: 0.0,
            dead_zone_outer_radius: 0.0,
            lerp_rate_azimuth_zone1: 0.0,
            lerp_rate_azimuth_zone2: 0.0,
            lerp_rate_azimuth_zone3: 0.0,
            lerp_rate_azimuth_zone4: 0.0,
            lock_heading_until_move: false,
            spin_threshold: 0.0,
            inner_radius: 0.0,
            outer_radius: 0.0,
        }
    }
}

/// Parses layout.campacknew into a dictionary of CameraPackageDefs
pub fn parse_campacknew(dir: &str) -> HashMap<String, CameraPackageDef> {
    let mut packages = HashMap::new();
    // Default magic package based on defaults present in cpp files.
    packages.insert(
        "DEFAULT_PACKAGE".to_string(),
        CameraPackageDef {
            name: "DEFAULT_PACKAGE".to_string(),
            navigation: "DEFAULT_FOLLOW".to_string(),
            targeting: "DEFAULT_TARGETING".to_string(),
            fighting: "DEFAULT_FOLLOW".to_string(),
            fight_mode_radius: 5.0,
            fight_mode_running_away_time: 2.0,
        },
    );

    let content = match crate::vfs::read_to_string(dir, "layout.campacknew") {
        Ok(c) => c,
        Err(_) => return packages,
    };

    let mut current_package: Option<CameraPackageDef> = None;

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

    packages
}

/// Parses a cam_*.xml file into a CameraParameterSet
pub fn parse_camera_xml(dir: &str, filename: &str) -> Option<CameraParameterSet> {
    let mut params = CameraParameterSet::default();

    let content = match crate::vfs::read_to_string(dir, filename) {
        Ok(c) => c,
        Err(_) => {
            // Apply magic defaults if the file is one of the hardcoded defaults
            if filename == "DEFAULT_FOLLOW.xml" {
                info!("File {} not found, using magic defaults.", filename);
                params.name = "DEFAULT_FOLLOW".to_string();
                params.incline_offset = 10.0_f32.to_radians();
                params.incline_offset_running = 20.0_f32.to_radians();
                params.dead_zone_inner_radius = 1.5;
                params.dead_zone_outer_radius = 4.0;
                params.lerp_rate_azimuth_zone1 = 2.0;
                params.lerp_rate_azimuth_zone2 = 3.0;
                params.lerp_rate_azimuth_zone3 = 3.0;
                params.lerp_rate_azimuth_zone4 = 3.0;
                params.spin_threshold = 45.0_f32.to_radians();
                return Some(params);
            } else if filename == "DEFAULT_FIGHT.xml" {
                info!("File {} not found, using magic defaults.", filename);
                params.name = "DEFAULT_FIGHT".to_string();
                params.incline_offset = 20.0_f32.to_radians();
                params.inner_radius = 4.0;
                params.outer_radius = 8.0;
                return Some(params);
            } else if filename == "DEFAULT_TARGETING.xml" {
                info!("File {} not found, using magic defaults.", filename);
                params.name = "DEFAULT_TARGETING".to_string();
                params.distance = 2.5; // close up
                return Some(params);
            }
            return None;
        }
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("<m_") {
            continue;
        }

        // Example: <m_FOV type="double" value="50.000"/>
        let name_start = trimmed.find("<m_").unwrap() + 3;
        let name_end = trimmed[name_start..].find(' ').unwrap_or(0) + name_start;
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

        match name {
            "Name" => params.name = value_str.to_string(),
            "FOV" => params.fov = parse_f32(value_str, 50.0),
            "Distance" => params.distance = parse_f32(value_str, 3.0),
            "InclineOffset" => {
                params.incline_offset =
                    space::oni2_camera_incline_to_bevy(parse_f32(value_str, 0.0))
            }
            "InclineOffsetRunning" => {
                params.incline_offset_running =
                    space::oni2_camera_incline_to_bevy(parse_f32(value_str, 0.0))
            }
            "DeadZoneInnerRadius" => params.dead_zone_inner_radius = parse_f32(value_str, 0.0),
            "DeadZoneOuterRadius" => params.dead_zone_outer_radius = parse_f32(value_str, 0.0),
            "LerpRateAzimuthZone1" => params.lerp_rate_azimuth_zone1 = parse_f32(value_str, 0.0),
            "LerpRateAzimuthZone2" => params.lerp_rate_azimuth_zone2 = parse_f32(value_str, 0.0),
            "LerpRateAzimuthZone3" => params.lerp_rate_azimuth_zone3 = parse_f32(value_str, 0.0),
            "LerpRateAzimuthZone4" => params.lerp_rate_azimuth_zone4 = parse_f32(value_str, 0.0),
            "LockHeadingUntilMove" => {
                params.lock_heading_until_move =
                    value_str == "1" || value_str.eq_ignore_ascii_case("true")
            }
            "SpinThreshold" => params.spin_threshold = parse_f32(value_str, 0.0),
            "InnerRadius" => params.inner_radius = parse_f32(value_str, 0.0), // from fight schema
            "OuterRadius" => params.outer_radius = parse_f32(value_str, 0.0), // from fight schema
            "FocusOffset" => {
                let parts: Vec<f32> = value_str
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if parts.len() >= 3 {
                    let v = space::to_bevy_space_pos(&parts);
                    params.focus_offset = [v.x, v.y - 1.0, v.z];
                }
            }
            _ => {}
        }
    }

    Some(params)
}
