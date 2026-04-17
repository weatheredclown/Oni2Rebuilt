/*
 * oni2_loader/parsers/actor_xml.rs — actor XML reader.
 *
 * Parses per-entity actor.xml files (cached after first read).  Extracts the
 * base="…" entity type reference and per-property overrides used to configure
 * spawned entities (health, team, initial animation, etc.).
 */
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

static XML_CACHE: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn cached_read_to_string(dir: &str, filename: &str) -> std::io::Result<String> {
    let key = format!("{}/{}", dir, filename);
    if let Some(content) = XML_CACHE.read().unwrap().get(&key) {
        return Ok(content.clone());
    }
    let content = crate::vfs::read_to_string(dir, filename)?;
    XML_CACHE.write().unwrap().insert(key, content.clone());
    Ok(content)
}

pub fn clear_xml_cache() {
    XML_CACHE.write().unwrap().clear();
}

use crate::oni2_loader::utils::parse::{
    extract_root_xml_attr, extract_xml_attr, extract_xml_base_attr, extract_xml_block, parse_vec3,
};
use crate::oni2_loader::utils::space;

/// Parsed actor from a layout XML file.
pub struct LayoutActor {
    pub entity_type: String,
    pub position: Vec3,
    pub orientation_o2: Vec3, // euler angles in degrees (rx, ry, rz)
    /// AnimatorType from Animator component (resolved through templates).
    pub animator_type: Option<String>,
    /// Whether this actor has a Creature component (animated character).
    pub is_creature: bool,
    /// Whether this creature is the player (Player="1" in Creature component).
    pub is_player: bool,
    /// Faction allegiance (e.g. "Syndicate", "TCTF").
    pub faction: Option<String>,
    /// Curve name from <Curve> component (for path-following entities).
    pub curve_name: Option<String>,
    /// Whether the entity should NOT rotate to look along the curve.
    pub curve_fixed_orientation: bool,
    /// Whether to constrain orientation to the XZ plane.
    pub curve_look_xz: bool,
    /// PingPong mode for curve traversal.
    pub curve_ping_pong: bool,
    /// Speed value from Curve component (knots/sec).
    pub curve_speed: f32,
    /// Whether the actor possesses a FightAI component.
    pub has_fight_ai: bool,
    /// ScrOni script filename (from <ScrOni><Filename>). '$' prefix = layout-local.
    pub script_filename: Option<String>,
    /// ScrOni entry-point script name (from <ScrOni><MainScript>).
    pub script_main: Option<String>,
    /// Radius from <BroadcastTrigger><Radius>
    pub broadcast_radius: Option<f32>,
    pub fx_type: Option<String>,
    pub fx_start_active: bool,
    pub ptx_name: Option<String>,
    pub ptx_birth_rate: f32,
    pub ptx_num_particles: i32,
    pub ptx_offset: Vec3,
    /// Script update state (e.g., "Asleep", "Active"). Extracted from root `<actor>` tag.
    pub updatestate: Option<String>,
    /// Checkpoint index if this actor acts as a checkpoint trigger
    pub checkpoint_index: Option<i32>,
    /// Checkpoint trigger radius natively parsed from the component block
    pub checkpoint_radius: Option<f32>,
    /// Whether this actor should be instantly skipped by layout_loader during load
    pub spawn_later: bool,
    /// Max hitpoints from <Health><MaxHitPoints>
    pub max_hitpoints: Option<f32>,
    /// Destroy time from <Health><DestroyTime> -- how many seconds to wait until destroying the dead actor
    pub destroy_time: Option<f32>,
    /// Actor name to dynamically parent this sub-actor onto
    pub parent_actor: Option<String>,
    /// Joint name to parent this onto if the parent is skeletonized
    pub parent_bone: Option<String>,
}

/// Resolve the full template chain for an actor XML file.
/// Returns a list of XML contents ordered from most-base to most-derived (template first, actor last).
fn resolve_template_chain(path: &str, template_dir: &str) -> Vec<String> {
    let mut chain = Vec::new();

    let content = match cached_read_to_string("", path) {
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

/// Helper: Collect a single component's properties by merging the extracted blocks across the template hierarchy.
/// Returns None if the component is never actually declared in the actor's specific templates (outside of components.xml).
fn extract_component(chain: &[String], has_components_xml: bool, tag: &str) -> Option<String> {
    // Determine if the component is actually declared by the actor/templates
    let mut explicitly_declared = false;
    for (i, content) in chain.iter().enumerate() {
        if !(i == 0 && has_components_xml) {
            let open_tag = format!("<{}", tag);
            if content.contains(&open_tag) {
                explicitly_declared = true;
                break;
            }
        }
    }

    if !explicitly_declared {
        return None;
    }

    // Merge the blocks from base to derived
    let mut merged = String::new();
    for content in chain {
        if let Some(block) = extract_xml_block(content, tag) {
            merged.push_str(&block);
            merged.push_str("\n"); // Separate for extract_xml_attr safety
        }
    }

    Some(merged)
}

trait OptionExt {
    fn is_none_or_empty_hint(&self) -> bool;
}

impl OptionExt for Option<String> {
    fn is_none_or_empty_hint(&self) -> bool {
        self.as_deref().map_or(true, |v| v.eq_ignore_ascii_case("none"))
    }
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
    if let Ok(comp) = cached_read_to_string(root_dir, "components.xml") {
        // Insert at index 0 so it's processed first and later files override it
        chain.insert(0, comp);
        has_components_xml = true;
    }

    // Extract core actor properties from the raw content hierarchy
    let mut entity_type: Option<String> = None;
    let mut position = Vec3::ZERO;
    let mut orientation_o2 = Vec3::ZERO;
    let mut updatestate: Option<String> = None;
    let mut spawn_later = false;
    let mut parent_actor: Option<String> = None;
    let mut parent_bone: Option<String> = None;

    // Core property extraction is done via block extraction from 'Prop' and 'Entity' first if they exist,
    // but the old code grabbed attributes globally. We will use a safe global grab for position/orientation
    // since they are heavily nested and commonly exist in 'Prop' or the root.
    for content in &chain {
        // Fallback or override values
        if let Some(v) = extract_xml_attr(content, "EntityType") {
            if !v.eq_ignore_ascii_case("none") {
                entity_type = Some(v);
            }
        }
        if let Some(v) = extract_root_xml_attr(content, "updatestate") {
            updatestate = Some(v);
        }
        if let Some(v) = extract_root_xml_attr(content, "spawnlater") {
            spawn_later = v == "1" || v.eq_ignore_ascii_case("true");
        }
        if let Some(v) = extract_xml_attr(content, "Position").and_then(|s| parse_vec3(&s)) {
            position = v;
        }
        if let Some(v) = extract_xml_attr(content, "Orientation").and_then(|s| parse_vec3(&s)) {
            orientation_o2 = v;
        }
        if let Some(v) = extract_xml_attr(content, "ParentActor") {
            parent_actor = Some(v);
        }
        if let Some(v) = extract_xml_attr(content, "ParentBone") {
            parent_bone = Some(v);
        }
    }

    if entity_type.is_none_or_empty_hint() {
        info!("EntityType is missing/none in actor xml {}", filename);
    }
    // Now extract specific Component Blocks
    // This safely pulls out ONLY the declared components and defaults
    let creature_block = extract_component(&chain, has_components_xml, "Creature");
    let animator_block = extract_component(&chain, has_components_xml, "Animator");
    let curve_block = extract_component(&chain, has_components_xml, "Curve");
    let scroni_block = extract_component(&chain, has_components_xml, "ScrOni");
    let broadcast_block = extract_component(&chain, has_components_xml, "BroadcastTrigger");
    let fight_ai_block = extract_component(&chain, has_components_xml, "FightAI")
        .or_else(|| extract_component(&chain, has_components_xml, "FightAi"));

    // Extract Animator props
    let mut animator_type: Option<String> = None;
    if let Some(block) = animator_block {
        if let Some(v) = extract_xml_attr(&block, "AnimatorType") {
            animator_type = Some(v);
        }
    }

    // Extract Creature props
    let mut is_creature = creature_block.is_some();
    let mut is_player = false;
    let mut faction: Option<String> = None;
    if let Some(block) = &creature_block {
        if let Some(v) = extract_xml_attr(block, "Player") {
            is_player = v == "1";
        }
        if let Some(v) = extract_xml_attr(block, "Faction") {
            faction = Some(v);
        }
    }

    // Extract Curve props
    let mut curve_name: Option<String> = None;
    let mut curve_fixed_orientation = false;
    let mut curve_look_xz = false;
    let mut curve_ping_pong = false;
    let mut curve_speed = 0.0f32;
    if let Some(block) = curve_block {
        if let Some(v) = extract_xml_attr(&block, "CurveName") {
            curve_name = Some(v);
        }
        if let Some(v) = extract_xml_attr(&block, "FixedOrientation") {
            curve_fixed_orientation = v == "1";
        }
        if let Some(v) = extract_xml_attr(&block, "LookAlongXZPlane") {
            curve_look_xz = v == "1";
        }
        if let Some(v) = extract_xml_attr(&block, "PingPong") {
            curve_ping_pong = v == "1";
        }
        if let Some(v) = extract_xml_attr(&block, "Speed") {
            curve_speed = v.parse().unwrap_or(0.0);
        }
    }

    // Extract ScrOni props
    let mut script_filename: Option<String> = None;
    let mut script_main: Option<String> = None;
    if let Some(block) = scroni_block {
        if let Some(v) = extract_xml_attr(&block, "Filename") {
            script_filename = Some(v);
        }
        if let Some(v) = extract_xml_attr(&block, "MainScript") {
            script_main = Some(v);
        }
    }

    // Extract BroadcastTrigger props
    let mut broadcast_radius: Option<f32> = None;
    if let Some(block) = broadcast_block {
        if let Some(v) = extract_xml_attr(&block, "Radius") {
            broadcast_radius = v.parse().ok();
        }
    }

    let checkpoint_block = extract_component(&chain, has_components_xml, "CheckpointTrigger");
    let mut checkpoint_index: Option<i32> = None;
    let mut checkpoint_radius: Option<f32> = None;
    if let Some(block) = checkpoint_block {
        if let Some(v) = extract_xml_attr(&block, "CheckpointIndex") {
            checkpoint_index = v.parse().ok();
        }
        if let Some(v) = extract_xml_attr(&block, "Radius") {
            checkpoint_radius = v.parse().ok();
        }
    }

    // If an entity type isn't explicitly defined, fallback to the generic IconTrigger so it has a default 
    // sprite in the layout editor instead of trying to load its literal instance name as a mesh folder.
    let entity_type = entity_type.unwrap_or_else(|| {
        "IconTrigger".to_string()
    });

    // Extract FX block (e.g. <FX> ... </FX>)
    let mut fx_type: Option<String> = None;
    let mut fx_start_active = true; // Default to true if not specified
    let mut ptx_name: Option<String> = None;
    let mut ptx_birth_rate = 0.0;
    let mut ptx_num_particles = 0;
    let mut ptx_offset = Vec3::ZERO;

    let health_block = extract_component(&chain, has_components_xml, "Health");
    let mut max_hitpoints: Option<f32> = None;
    let mut destroy_time: Option<f32> = None;    
    if let Some(block) = health_block {
        if let Some(v) = extract_xml_attr(&block, "MaxHitPoints") {
            max_hitpoints = v.parse().ok();
        }
        if let Some(v) = extract_xml_attr(&block, "DestroyTime") { // how many seconds to wait until destroying the dead actor
            destroy_time = v.parse().ok();
        }
    }

    let fx_block = extract_component(&chain, has_components_xml, "FX");
    if let Some(block) = fx_block {
        if let Some(v) = extract_xml_attr(&block, "FXType") {
            fx_type = Some(v);
        }
        if let Some(v) = extract_xml_attr(&block, "ParticlesType") {
            ptx_name = Some(v);
        }
        if let Some(v) = extract_xml_attr(&block, "StartActive") {
            fx_start_active = v == "1";
        }
        if let Some(v) = extract_xml_attr(&block, "BirthRate") {
            ptx_birth_rate = v.parse().unwrap_or(0.0);
        }
        if let Some(v) = extract_xml_attr(&block, "NumParticles") {
            ptx_num_particles = v.parse().unwrap_or(0);
        }
        if let Some(v) = extract_xml_attr(&block, "Offset").and_then(|s| parse_vec3(&s)) {
            // Convert to right-handed: negate X and Z
            ptx_offset = Vec3::new(-v.x, v.y, -v.z);
        }
    } else {
        // Fallback for standalone FXType tag in old maps
        for content in &chain {
            let fx_alone = extract_xml_block(content, "FXType");
            if let Some(block) = fx_alone {
                if let Some(v) = extract_xml_attr(&block, "value") {
                    fx_type = Some(v);
                }
            }
        }
    }

    let position = space::to_bevy_space_pos(position);

    let has_fight_ai = fight_ai_block.is_some();

    Some(LayoutActor {
        entity_type,
        position,
        orientation_o2,
        animator_type,
        is_creature,
        is_player,
        faction,
        curve_name,
        curve_fixed_orientation,
        curve_look_xz,
        curve_ping_pong,
        curve_speed,
        has_fight_ai,
        script_filename,
        script_main,
        broadcast_radius,
        fx_type,
        fx_start_active,
        ptx_name,
        ptx_birth_rate,
        ptx_num_particles,
        ptx_offset,
        updatestate,
        checkpoint_index,
        checkpoint_radius,
        spawn_later,
        max_hitpoints,
        destroy_time,
        parent_actor,
        parent_bone,
    })
}
