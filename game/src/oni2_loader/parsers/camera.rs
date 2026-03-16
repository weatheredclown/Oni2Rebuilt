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

#[derive(Debug, Clone, Default)]
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

/// Parses layout.campacknew into a dictionary of CameraPackageDefs
pub fn parse_campacknew(dir: &str) -> HashMap<String, CameraPackageDef> {
    let mut packages = HashMap::new();
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
                    "FightModeRunningAwayTime" => pkg.fight_mode_running_away_time = value.parse().unwrap_or(0.0),
                    _ => {}
                }
            }
        }
    }

    packages
}

/// Parses a cam_*.xml file into a CameraParameterSet
pub fn parse_camera_xml(dir: &str, filename: &str) -> Option<CameraParameterSet> {
    let content = crate::vfs::read_to_string(dir, filename).ok()?;
    
    let mut params = CameraParameterSet::default();
    // Default FOV and distance to sensible values in case missing
    params.fov = 50.0;
    params.distance = 3.0;

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
            "FOV" => params.fov = value_str.parse().unwrap_or(50.0),
            "Distance" => params.distance = value_str.parse().unwrap_or(3.0),
            "InclineOffset" => params.incline_offset = value_str.parse().unwrap_or(0.0),
            "InclineOffsetRunning" => params.incline_offset_running = value_str.parse().unwrap_or(0.0),
            "DeadZoneInnerRadius" => params.dead_zone_inner_radius = value_str.parse().unwrap_or(0.0),
            "DeadZoneOuterRadius" => params.dead_zone_outer_radius = value_str.parse().unwrap_or(0.0),
            "LerpRateAzimuthZone1" => params.lerp_rate_azimuth_zone1 = value_str.parse().unwrap_or(0.0),
            "LerpRateAzimuthZone2" => params.lerp_rate_azimuth_zone2 = value_str.parse().unwrap_or(0.0),
            "LerpRateAzimuthZone3" => params.lerp_rate_azimuth_zone3 = value_str.parse().unwrap_or(0.0),
            "LerpRateAzimuthZone4" => params.lerp_rate_azimuth_zone4 = value_str.parse().unwrap_or(0.0),
            "LockHeadingUntilMove" => params.lock_heading_until_move = value_str == "1" || value_str.eq_ignore_ascii_case("true"),
            "SpinThreshold" => params.spin_threshold = value_str.parse().unwrap_or(0.0),
            "InnerRadius" => params.inner_radius = value_str.parse().unwrap_or(0.0), // from fight schema
            "OuterRadius" => params.outer_radius = value_str.parse().unwrap_or(0.0), // from fight schema
            "FocusOffset" => {
                let parts: Vec<f32> = value_str.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                if parts.len() >= 3 {
                    params.focus_offset = [parts[0], parts[1], parts[2]];
                }
            }
            _ => {}
        }
    }

    Some(params)
}
