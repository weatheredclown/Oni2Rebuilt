/*
 * oni2_loader/parsers/layout.rs — level layout text-format parser.
 *
 * Parses layout/ directory files: lights (LayoutLight), paths (Vec<Vec3>),
 * fog settings, camera package refs, and the entity placement list.
 * Returns data structs consumed by layout_loader.rs to spawn level content.
 */
use crate::oni2_loader::utils::parse::parse_vec3;
use crate::oni2_loader::utils::space;
use bevy::prelude::*;

/// A parsed layout light entry.  Mirrors the legacy
/// `lvlLight::Load` schema (rb/src/rblevel/lightmgr.cpp:53), which
/// in turn delegates to `fxLight::Load` (rb/src/fx/light.cpp:302) →
/// `lgtLight::Load` (rb/src/fx/light.cpp:156).  All vectors are
/// post-converted to Bevy space (right-handed, -Z forward) inside
/// `parse_lights_file`.
pub struct LayoutLight {
    pub name: String,
    /// Legacy `lgtLightType` name, lowercased.  Possible values:
    /// `"ambient"`, `"directional"`, `"point"`, `"spot"`.
    pub light_type: String,
    pub position: Vec3,
    pub intensity: f32,
    /// Unit vector indicating where the light points (only meaningful
    /// for `directional` and `spot`).
    pub direction: Vec3,
    /// Half-angle of the spot cone in DEGREES (for `spot` only).
    pub spot_angle: f32,
    /// Linear-space RGBA, 0–1.
    pub color: [f32; 4],
    /// `CastShadowRange` from `lvlLight::Load`.  Legacy:
    /// `bool CastShadow() const { return ( m_CastShadowRange > 0.0f ); }`
    /// (rb/src/rblevel/lightmgr.h:55).  Zero means the light still
    /// illuminates surfaces but is NOT registered with the shadow
    /// manager — designers used this for cheap "fill" lights that
    /// shine through geometry without paying the per-light shadow
    /// cost.  Default in legacy ctor is 15.0.
    pub cast_shadow_range: f32,
    /// True when the light entry includes the `fxLight` token, which
    /// means it's also a visible glow source (lens-flare-ish corona)
    /// in addition to a real light.  See `LightGlowType` /
    /// `GlowIntensityScale`.  Legacy: `fxLight::Load`
    /// (rb/src/fx/light.cpp:302).
    pub fx_light: bool,
    /// Named `fxLightGlow` effect type (e.g. `"LightConeDown"`).
    /// Resolved against the legacy `fxEffectTypeManager` registry to
    /// pick a billboard/sprite for the glow.  Only set when
    /// `fx_light` is true.
    pub light_glow_type: Option<String>,
    /// Multiplier on the glow's render intensity (NOT the light's
    /// own intensity).  Legacy stores per-fxLight; rendered as
    /// `intensity * glow_intensity_scale`.
    pub glow_intensity_scale: f32,
}

/// A parsed environment file.
pub struct LayoutEnvironment {
    pub light_direction: Vec3,
    pub light_color: [f32; 3],
    pub ambient_color: [f32; 3],
    pub fog_color: [f32; 4],
    pub fog_start: f32,
    pub fog_end: f32,
}

/// Parsed layout.fog light entry.
pub struct LayoutFogLight {
    pub enabled: bool,
    pub direction: [f32; 3],
    pub color: [f32; 3],
}

/// Parsed layout.fog file.
pub struct LayoutFogFile {
    pub enabled: bool,
    pub start: f32,
    pub end: f32,
    pub color: [f32; 3],
    pub lights: Vec<LayoutFogLight>,
}

/// Parse `<dir>/layout.lights`.  Format mirrors the legacy
/// `lvlLight::Load` schema (rb/src/rblevel/lightmgr.cpp:53), which
/// chains through `fxLight::Load` (rb/src/fx/light.cpp:302) →
/// `lgtLight::Load` (rb/src/fx/light.cpp:156).
///
/// Per-light schema (in source order, all whitespace-separated):
/// ```text
/// Light <name> {
///     Type <ambient|directional|point|spot>
///     Position <x y z>
///     Intensity <f>
///     Direction <x y z>
///     SpotAngle <degrees>
///     Color <r g b a>
///     [ fxLight                        # optional glow block
///       LightGlowType <fx-effect-name>
///       GlowIntensityScale <f>
///     ]
///     CastShadowRange <f>              # 0 = "fill light, no shadows"
/// }
/// ```
///
/// `Position` and `Direction` are returned in Bevy space (right-handed,
/// -Z forward) — converted at the parse boundary via
/// `space::to_bevy_space_pos` per the project's "convert at the
/// boundary" rule.  Downstream consumers see Bevy-space data only.
pub fn parse_lights_file(dir: &str) -> Vec<LayoutLight> {
    let content = match crate::vfs::read_to_string(dir, "layout.lights") {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut lights = Vec::new();
    let mut lines = content.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if !trimmed.starts_with("Light ") {
            continue;
        }
        // Extract name: "Light name {"
        let name = trimmed
            .strip_prefix("Light ")
            .and_then(|s| s.strip_suffix(" {"))
            .unwrap_or("unknown")
            .to_string();

        let mut light = LayoutLight {
            name,
            light_type: "point".to_string(),
            position: Vec3::ZERO,
            intensity: 0.0,
            direction: Vec3::Y,
            spot_angle: 45.0,
            color: [1.0, 1.0, 1.0, 1.0],
            // Match the legacy `lvlLight` ctor default (lightmgr.cpp:40);
            // overridden by the file's own `CastShadowRange` line if
            // present, which it always is in shipped layouts.
            cast_shadow_range: 15.0,
            fx_light: false,
            light_glow_type: None,
            glow_intensity_scale: 1.0,
        };

        // Parse fields until closing brace.
        for line in lines.by_ref() {
            let field = line.trim();
            if field == "}" {
                break;
            }
            // The `fxLight` token is a no-value flag — mark and continue.
            // Subsequent lines are `LightGlowType` and `GlowIntensityScale`.
            if field == "fxLight" {
                light.fx_light = true;
                continue;
            }
            if let Some(val) = field.strip_prefix("Type ") {
                // Lowercase so downstream match arms can use a single
                // canonical form regardless of file casing.
                light.light_type = val.trim().to_lowercase();
            } else if let Some(val) = field.strip_prefix("Position ") {
                if let Some(v) = parse_vec3(val.trim()) {
                    light.position = v;
                }
            } else if let Some(val) = field.strip_prefix("Intensity ") {
                light.intensity = val.trim().parse().unwrap_or(0.0);
            } else if let Some(val) = field.strip_prefix("Direction ") {
                if let Some(v) = parse_vec3(val.trim()) {
                    light.direction = v;
                }
            } else if let Some(val) = field.strip_prefix("SpotAngle ") {
                light.spot_angle = val.trim().parse().unwrap_or(45.0);
            } else if let Some(val) = field.strip_prefix("Color ") {
                let parts: Vec<f32> = val
                    .split_whitespace()
                    .filter_map(|p| p.parse().ok())
                    .collect();
                if parts.len() >= 4 {
                    light.color = [parts[0], parts[1], parts[2], parts[3]];
                }
            } else if let Some(val) = field.strip_prefix("LightGlowType ") {
                light.light_glow_type = Some(val.trim().to_string());
            } else if let Some(val) = field.strip_prefix("GlowIntensityScale ") {
                light.glow_intensity_scale = val.trim().parse().unwrap_or(1.0);
            } else if let Some(val) = field.strip_prefix("CastShadowRange ") {
                light.cast_shadow_range = val.trim().parse().unwrap_or(15.0);
            }
        }
        // Convert position + direction at the parse boundary so
        // downstream code sees Bevy-space data only.
        light.position = space::to_bevy_space_pos(light.position);
        light.direction = space::to_bevy_space_pos(light.direction);
        lights.push(light);
    }
    lights
}

/// Parse a default.environment file for directional light, ambient, and fog.
pub fn parse_environment(dir: &str) -> Option<LayoutEnvironment> {
    let content = crate::vfs::read_to_string(dir, "default.environment").ok()?;
    let lines: Vec<&str> = content.lines().collect();

    let mut light_dir = Vec3::new(0.5, -0.7, 0.5);
    let mut light_color = [0.4_f32, 0.4, 0.4];
    let mut ambient_color = [0.2_f32, 0.2, 0.2];
    let mut fog_color = [0.5_f32, 0.5, 0.5, 1.0];
    let mut fog_start = 0.0_f32;
    let mut fog_end = 100.0_f32;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "lightDirection:" {
            // Next line is the primary directional light vector
            if let Some(next) = lines.get(i + 1) {
                let parts: Vec<f32> = next
                    .split_whitespace()
                    .filter_map(|p| p.parse().ok())
                    .collect();
                if parts.len() >= 3 {
                    light_dir = Vec3::new(parts[0], parts[1], parts[2]);
                }
            }
        } else if trimmed == "lightColor:" {
            // Row 0 = primary light color, row 3 = ambient
            if let Some(next) = lines.get(i + 1) {
                let parts: Vec<f32> = next
                    .split_whitespace()
                    .filter_map(|p| p.parse().ok())
                    .collect();
                if parts.len() >= 3 {
                    light_color = [parts[0], parts[1], parts[2]];
                }
            }
            if let Some(row3) = lines.get(i + 4) {
                let parts: Vec<f32> = row3
                    .split_whitespace()
                    .filter_map(|p| p.parse().ok())
                    .collect();
                if parts.len() >= 3 {
                    ambient_color = [parts[0], parts[1], parts[2]];
                }
            }
        } else if let Some(val) = trimmed.strip_prefix("fogColor:") {
            let parts: Vec<f32> = val
                .split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect();
            if parts.len() >= 4 {
                fog_color = [parts[0], parts[1], parts[2], parts[3]];
            }
        } else if let Some(val) = trimmed.strip_prefix("fogStart:") {
            fog_start = val.trim().parse().unwrap_or(0.0);
        } else if let Some(val) = trimmed.strip_prefix("fogEnd:") {
            fog_end = val.trim().parse().unwrap_or(100.0);
        }
    }

    // Convert from left-handed to right-handed: 180° Y rotation (negate X and Z)
    light_dir.x = -light_dir.x;
    light_dir.z = -light_dir.z;

    Some(LayoutEnvironment {
        light_direction: light_dir,
        light_color,
        ambient_color,
        fog_color,
        fog_start,
        fog_end,
    })
}

/// Parse a layout.fog file.
/// Format (version 1):
///   Line 2: enabled fogStart fogEnd colorR colorG colorB colorA
///   Line 3: enabled fogType fogMin fogMax fogPower
///   Lines 4-6: enabled dirX dirY dirZ colorR colorG colorB (3 lights)
pub fn parse_layout_fog(dir: &str) -> Option<LayoutFogFile> {
    let content = crate::vfs::read_to_string(dir, "layout.fog").ok()?;
    let lines: Vec<&str> = content.lines().collect();

    if lines.len() < 6 {
        return None;
    }

    // Line 0: "version: 1"
    // Line 1: enabled fogStart fogEnd colorR colorG colorB colorA
    let fog_parts: Vec<f32> = lines[1]
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if fog_parts.len() < 7 {
        return None;
    }
    let fog_enabled = fog_parts[0] as i32 != 0;
    let fog_start = fog_parts[1];
    let fog_end = fog_parts[2];
    let fog_color = [fog_parts[3], fog_parts[4], fog_parts[5]];

    // Lines 3-5: directional/ambient lights
    let mut lights = Vec::new();
    for i in 3..=5 {
        if i >= lines.len() {
            break;
        }
        let parts: Vec<f32> = lines[i]
            .split_whitespace()
            .filter_map(|p| p.parse().ok())
            .collect();
        if parts.len() >= 7 {
            lights.push(LayoutFogLight {
                enabled: parts[0] as i32 != 0,
                direction: [parts[1], parts[2], parts[3]],
                color: [parts[4], parts[5], parts[6]],
            });
        }
    }

    Some(LayoutFogFile {
        enabled: fog_enabled,
        start: fog_start,
        end: fog_end,
        color: fog_color,
        lights,
    })
}

/// Parse a layout.paths file containing named Bezier/spline curves.
pub fn parse_layout_paths(dir: &str) -> Vec<(String, Vec<Vec3>)> {
    let content = match crate::vfs::read_to_string(dir, "layout.paths") {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut curves = Vec::new();
    let mut lines = content.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        // Match: curveName[count] {
        if let Some(bracket_pos) = trimmed.find('[') {
            let name = trimmed[..bracket_pos].to_string();
            if !trimmed.ends_with('{') {
                continue;
            }

            let mut points = Vec::new();
            for line in lines.by_ref() {
                let pt = line.trim();
                if pt == "}" {
                    break;
                }
                let coords: Vec<f32> = pt
                    .split_whitespace()
                    .filter_map(|p| p.parse().ok())
                    .collect();
                if coords.len() >= 3 {
                    // Convert from left-handed to right-handed
                    points.push(space::to_bevy_space_pos(&coords[0..3]));
                }
            }

            if !points.is_empty() {
                info!("Layout path: {} with {} waypoints", name, points.len());
                curves.push((name, points));
            }
        }
    }

    curves
}
