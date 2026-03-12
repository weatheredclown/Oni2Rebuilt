use bevy::prelude::*;

use crate::oni2_loader::utils::parse::{extract_xml_base_attr, extract_xml_attr, parse_vec3};

/// Parsed actor from a layout XML file.
pub struct LayoutActor {
    pub entity_type: String,
    pub position: Vec3,
    pub orientation: Vec3, // euler angles in degrees (rx, ry, rz)
    /// AnimatorType from Animator component (resolved through templates).
    pub animator_type: Option<String>,
    /// Whether this actor has a Creature component (animated character).
    pub is_creature: bool,
    /// Whether this creature is the player (Player="1" in Creature component).
    pub is_player: bool,
    /// Curve name from <Curve> component (for path-following entities).
    pub curve_name: Option<String>,
    /// Whether to constrain orientation to the XZ plane.
    pub curve_look_xz: bool,
    /// PingPong mode for curve traversal.
    pub curve_ping_pong: bool,
    /// Speed value from Curve component (knots/sec).
    pub curve_speed: f32,
    /// ScrOni script filename (from <ScrOni><Filename>). '$' prefix = layout-local.
    pub script_filename: Option<String>,
    /// ScrOni entry-point script name (from <ScrOni><MainScript>).
    pub script_main: Option<String>,
}

/// Resolve the full template chain for an actor XML file.
/// Returns a list of XML contents ordered from most-base to most-derived (template first, actor last).
fn resolve_template_chain(path: &str, template_dir: &str) -> Vec<String> {
    let mut chain = Vec::new();

    let content = match crate::vfs::read_to_string("", path) {
        Ok(c) => c,
        Err(_) => return chain,
    };

    // Resolve base template recursively
    if let Some(base_name) = extract_xml_base_attr(&content) {
        // Try template directory
        let template_filename = format!("{}.xml", base_name);
        if crate::vfs::exists(template_dir, &template_filename) {
            let template_path = format!("{}/{}", template_dir, template_filename);
            let parent_chain = resolve_template_chain(&template_path, template_dir);
            chain.extend(parent_chain);
        } else {
            let mut parts: Vec<&str> = path.split('/').collect();
            parts.pop();
            if !parts.is_empty() {
                let parent_dir = parts.join("/");
                // Try sibling file in same directory
                let sibling_filename = format!("{}.xml", base_name);
                if crate::vfs::exists(&parent_dir, &sibling_filename) {
                    let sibling = format!("{}/{}", parent_dir, sibling_filename);
                    let parent_chain = resolve_template_chain(&sibling, template_dir);
                    chain.extend(parent_chain);
                }
            }
        }
    }

    chain.push(content);
    chain
}

/// Parse an actor XML file, resolving full template inheritance chain.
/// Template values are base; actor values override. Supports multi-level inheritance.
pub fn parse_actor_xml(dir: &str, filename: &str, template_dir: &str) -> Option<LayoutActor> {
    let full_path = format!("{}/{}", dir, filename);
    let mut chain = resolve_template_chain(&full_path, template_dir);
    if chain.is_empty() {
        return None;
    }

    // Prepend components.xml as the root defaults if available
    let root_dir = "";
    let mut has_components_xml = false;
    if let Ok(comp) = crate::vfs::read_to_string(root_dir, "components.xml")
        .or_else(|_| crate::vfs::read_to_string(template_dir, "components.xml"))
        .or_else(|_| crate::vfs::read_to_string("Entity", "components.xml"))
        .or_else(|_| crate::vfs::read_to_string("template", "components.xml"))
    {
        // Insert at index 0 so it's processed first and later files override it
        chain.insert(0, comp);
        has_components_xml = true;
    }

    // Merge attributes: iterate from base to derived, later values override
    let mut entity_type: Option<String> = None;
    let mut position = Vec3::ZERO;
    let mut orientation = Vec3::ZERO;
    let mut animator_type: Option<String> = None;
    let mut is_creature = false;
    let mut is_player = false;
    let mut curve_name: Option<String> = None;
    let mut curve_look_xz = false;
    let mut curve_ping_pong = false;
    let mut curve_speed = 0.0f32;
    let mut script_filename: Option<String> = None;
    let mut script_main: Option<String> = None;

    for (i, content) in chain.iter().enumerate() {
        if content.contains("<Creature") && !(i == 0 && has_components_xml) {
            is_creature = true;
        }
        if let Some(v) = extract_xml_attr(content, "EntityType") {
            entity_type = Some(v);
        }
        if let Some(v) = extract_xml_attr(content, "Position").and_then(|s| parse_vec3(&s)) {
            position = v;
        }
        if let Some(v) = extract_xml_attr(content, "Orientation").and_then(|s| parse_vec3(&s)) {
            orientation = v;
        }
        if let Some(v) = extract_xml_attr(content, "AnimatorType") {
            animator_type = Some(v);
        }
        if let Some(v) = extract_xml_attr(content, "Player") {
            is_player = v == "1";
        }
        // Curve component attributes
        if let Some(v) = extract_xml_attr(content, "CurveName") {
            curve_name = Some(v);
        }
        if let Some(v) = extract_xml_attr(content, "LookAlongXZPlane") {
            curve_look_xz = v == "1";
        }
        if let Some(v) = extract_xml_attr(content, "PingPong") {
            curve_ping_pong = v == "1";
        }
        if let Some(v) = extract_xml_attr(content, "Speed") {
            curve_speed = v.parse().unwrap_or(0.0);
        }
        // ScrOni component attributes
        if let Some(v) = extract_xml_attr(content, "Filename") {
            script_filename = Some(v);
        }
        if let Some(v) = extract_xml_attr(content, "MainScript") {
            script_main = Some(v);
        }
    }

    let entity_type = entity_type?;

    // Convert from left-handed to right-handed: 180° Y rotation (negate X and Z)
    let position = Vec3::new(-position.x, position.y, -position.z);

    Some(LayoutActor {
        entity_type,
        position,
        orientation,
        animator_type,
        is_creature,
        is_player,
        curve_name,
        curve_look_xz,
        curve_ping_pong,
        curve_speed,
        script_filename,
        script_main,
    })
}
