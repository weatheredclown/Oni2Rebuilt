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
    extract_root_xml_attr, extract_xml_attr, extract_xml_attr_bool, extract_xml_base_attr,
    extract_xml_block, parse_vec3, parse_xml_bool,
};
use crate::oni2_loader::utils::space;

/// Per-nugget cueing mode for the `<Sound>` component.  Mirrors
/// `rbAudioPlayMode` in the legacy engine.  `Looped` variants
/// auto-restart playback when the underlying audio finishes; the
/// non-looped variants play once per `Play()` invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundPlayMode {
    /// Replay the same nugget on every Play() call.
    CurrentOne,
    /// Advance through the package's nuggets sequentially.
    NextOne,
    /// Pick a random nugget on each Play() call.
    RandomOne,
    /// Replay the same nugget continuously.
    CurrentOneLooped,
    /// Walk the package end-to-end, then restart from the top.
    FullLoop,
    /// Pick a random nugget, play it, then pick another.
    RandomLoop,
}

impl SoundPlayMode {
    pub fn is_looped(self) -> bool {
        matches!(
            self,
            SoundPlayMode::CurrentOneLooped
                | SoundPlayMode::FullLoop
                | SoundPlayMode::RandomLoop
        )
    }
}

/// Per-actor sound block data extracted from `<Sound>`.  Maps to
/// `rbAudioSoundComponentType` in the C++ engine.  Distances are in
/// world units; `range_max_volume` is the radius at which the source
/// is at its full `volume_scalar`, `range_zero_volume` is the radius
/// at which it fades to silence.
#[derive(Debug, Clone)]
pub struct SoundComponentData {
    pub audio_package: String,
    pub play_mode: SoundPlayMode,
    pub volume_scalar: f32,
    pub range_max_volume: f32,
    pub range_zero_volume: f32,
    pub start_active: bool,
    pub num_channels: i32,
    pub cookie_mode: bool,
    pub player_approach_package: Option<String>,
    pub player_knockdown_package: Option<String>,
}

/// Parsed actor from a layout XML file.
pub struct LayoutActor {
    pub entity_type: String,
    pub position: Vec3,
    pub orientation_o2: Vec3, // euler angles in degrees (rx, ry, rz)
    /// AnimatorType from Animator component (resolved through templates).
    pub animator_type: Option<String>,
    /// FighterType from Fighter component (resolved through templates).
    pub fighter_type: Option<String>,
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
    /// The attack table (loads *.atk) parsed from FightAI configuration.
    pub attack_table: Option<String>,
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
    /// Pre-loaded weapon name from the <Inventory><WeaponString> attribute
    /// (resolved through the template chain).  Feeds PendingInventory at spawn.
    pub weapon_string: Option<String>,
    /// `<Inventory><CanBePickedUp/>` — when true, this actor's
    /// dropped items survive in the world for the player to grab.
    /// `None` = use the spawned `InventoryTypeData` default.
    pub inventory_can_be_picked_up: Option<bool>,
    /// `<Inventory><PickUpRange/>` — wielder-side radius in world
    /// units for proximity auto-pickup.  `None` = default.
    pub inventory_pickup_range: Option<f32>,
    /// `<Inventory><DropItemsOnDeath/>` — gates the death→drop
    /// pipeline.  `None` = default.
    pub inventory_drop_items_on_death: Option<bool>,
    /// `<Inventory><DropRange/>` — how far ahead of the corpse the
    /// dropped pickup entity spawns.  `None` = default.
    pub inventory_drop_range: Option<f32>,
    /// FSM filename from the `<Behavior>` component's `<Pad_FSM value="..."/>`
    /// attribute (e.g. "player", "enemy_combo").  Drives which input state
    /// machine gets attached at spawn, mirroring the legacy
    /// `SUB_ATTRIBUTE(Pad, FSM)` / `bhPadTuningData::FSM` pipeline.  `.fsm`
    /// extension is stripped at parse time.
    pub pad_fsm: Option<String>,
    /// True if the actor declared a `<Target>` component.  Marks an
    /// actor as a valid auto-aim / shootable lock-on candidate.  The
    /// legacy `tarTarget` block carries `Group`/`Friendliness`
    /// metadata; for now we only need the presence bit so projectiles
    /// can damage non-creature shootables like the M4 statue.
    pub has_target: bool,
    /// Parsed `<Sound>` block.  `None` when the actor only inherits
    /// the empty schema defaults from components.xml (no AudioPackage
    /// to play).  `Some(...)` when AudioPackage is non-empty in some
    /// template/leaf override.
    pub sound: Option<SoundComponentData>,
    /// Fight vector trigger radius
    pub fvt_radius: Option<f32>,
    /// Fight vector trigger directional mode
    pub fvt_directional: Option<bool>,
    /// Fight vector trigger offset vector
    pub fvt_offset: Option<Vec3>,
    /// Fight vector trigger attack alias
    pub fvt_attack: Option<String>,
    /// Parsed `<ElbowCraneIK>` block — drives the
    /// EricArm-skeleton-based crane's analytical 2-bone IK
    /// solver.  `None` when the actor doesn't declare an
    /// `<ElbowCraneIK>` component (i.e. isn't a crane).
    pub crane_ik: Option<CraneIKComponentData>,
    /// Parsed `<ElbowCraneHitch>` block — declares this actor
    /// as a crane-pickup target.  `Center` is converted to Bevy
    /// space at parse time.
    pub crane_hitch: Option<CraneHitchComponentData>,
}

/// Tunable data extracted from `<ElbowCraneIK><attributes>`.
/// Maps to `animElbowCraneIKComponentType` in the C++ engine.
/// `Speed` is in degrees/sec; `ClampOffset` is in world units and
/// is the additional reach from the wrist out to the claw centre.
#[derive(Debug, Clone)]
pub struct CraneIKComponentData {
    pub speed_deg: f32,
    pub clamp_offset: f32,
}

/// Tunable data extracted from `<ElbowCraneHitch><attributes>`.
/// Maps to `animElbowCraneHitchComponentType` in the C++.
/// `Center` is converted to Bevy space at parse time so
/// downstream consumers see Bevy coords only.
#[derive(Debug, Clone)]
pub struct CraneHitchComponentData {
    pub center: Vec3,
    pub extent: f32,
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
fn extract_component(chain: &[String], has_components: bool, tag: &str) -> Option<String> {
    let mut merged = String::new();
    let mut requested = !has_components;

    for (i, content) in chain.iter().enumerate() {
        let is_components_xml = has_components && i == 0;
        if let Some(block) = extract_xml_block(content, tag) {
            if !is_components_xml {
                requested = true;
            }
            merged.push_str(&block);
            merged.push('\n');
        }
    }

    if !requested || merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

trait OptionExt {
    fn is_none_or_empty_hint(&self) -> bool;
}

impl OptionExt for Option<String> {
    fn is_none_or_empty_hint(&self) -> bool {
        self.as_deref()
            .is_none_or(|v| v.eq_ignore_ascii_case("none"))
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

    // Validate that every component declared in the chain is one we know how
    // to consume.  Anything unrecognised — including components we know about
    // but haven't wired up yet — fires a per-(file, tag) error so silent
    // drops stop being possible.  Skips the prepended components.xml entry
    // because that file's only job is to declare *every* component's schema
    // for the editor; an actor isn't "asking for" a component just because
    // the master schema mentioned it.
    validate_xml_components(&chain, has_components_xml, &full_path);

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
        if let Some(v) = extract_xml_attr(content, "EntityType")
            && !v.eq_ignore_ascii_case("none")
        {
            entity_type = Some(v);
        }
        if let Some(v) = extract_root_xml_attr(content, "updatestate") {
            updatestate = Some(v);
        }
        if let Some(v) = extract_root_xml_attr(content, "spawnlater") {
            spawn_later = parse_xml_bool(&v);
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
    let fight_vector_block = extract_component(&chain, has_components_xml, "FightVectorTrigger");
    let broadcast_trigger_block = extract_component(&chain, has_components_xml, "BroadcastTrigger");
    let fighter_block = extract_component(&chain, has_components_xml, "Fighter");

    // Extract Animator props
    let mut animator_type: Option<String> = None;
    if let Some(block) = animator_block
        && let Some(v) = extract_xml_attr(&block, "AnimatorType")
    {
        animator_type = Some(v);
    }

    // Extract Fighter props
    let mut fighter_type: Option<String> = None;
    if let Some(block) = fighter_block
        && let Some(v) = extract_xml_attr(&block, "FighterType")
    {
        fighter_type = Some(v);
    }

    // Extract Creature props
    let is_creature = creature_block.is_some();
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
    if let Some(block) = broadcast_block
        && let Some(v) = extract_xml_attr(&block, "Radius")
    {
        broadcast_radius = v.parse().ok();
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

    // <Inventory> — resolved through the template chain so derived
    // actors (e.g. Konoko) inherit the parent's weapon string and
    // pickup/drop tuning.  WeaponString seeds the actor's starting
    // weapon; the other four fields populate `InventoryTypeData` so
    // the per-actor pickup_range / drop_range / opt-in flags drive
    // the runtime pickup + drop pipelines instead of falling back to
    // hardcoded defaults.
    let inventory_block = extract_component(&chain, has_components_xml, "Inventory");
    let mut weapon_string: Option<String> = None;
    let mut inventory_can_be_picked_up: Option<bool> = None;
    let mut inventory_pickup_range: Option<f32> = None;
    let mut inventory_drop_items_on_death: Option<bool> = None;
    let mut inventory_drop_range: Option<f32> = None;
    if let Some(block) = inventory_block {
        if let Some(w) = extract_xml_attr(&block, "WeaponString")
            && !w.is_empty()
        {
            weapon_string = Some(w);
        }
        inventory_can_be_picked_up = extract_xml_attr_bool(&block, "CanBePickedUp");
        if let Some(v) = extract_xml_attr(&block, "PickUpRange") {
            inventory_pickup_range = v.parse().ok();
        }
        inventory_drop_items_on_death = extract_xml_attr_bool(&block, "DropItemsOnDeath");
        if let Some(v) = extract_xml_attr(&block, "DropRange") {
            inventory_drop_range = v.parse().ok();
        }
    }

    // <Behavior> ... <Pad_FSM value="player"/> ... — which input FSM to load
    // for this actor.  Legacy `SUB_ATTRIBUTE(Pad, FSM)` → `bhPadTuningData::FSM`,
    // consumed by `aiInputStateMachineData::GetStateMachineData(name)`
    // in the legacy pad-tuning loader.
    let behavior_block = extract_component(&chain, has_components_xml, "Behavior");
    let mut pad_fsm: Option<String> = None;
    if let Some(block) = behavior_block
        && let Some(v) = extract_xml_attr(&block, "Pad_FSM")
    {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            // Strip trailing `.fsm` so callers can pass the bare name to the cache.
            let stripped = trimmed
                .strip_suffix(".fsm")
                .or_else(|| trimmed.strip_suffix(".FSM"))
                .unwrap_or(trimmed);
            pad_fsm = Some(stripped.to_string());
        }
    }

    // If an entity type isn't explicitly defined, fallback to the generic IconTrigger so it has a default
    // sprite in the layout editor instead of trying to load its literal instance name as a mesh folder.
    let entity_type = entity_type.unwrap_or_else(|| "IconTrigger".to_string());

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
        if let Some(v) = extract_xml_attr(&block, "DestroyTime") {
            // how many seconds to wait until destroying the dead actor
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
            if let Some(block) = fx_alone
                && let Some(v) = extract_xml_attr(&block, "value")
            {
                fx_type = Some(v);
            }
        }
    }

    let position = space::to_bevy_space_pos(position);

    let has_fight_ai = fight_ai_block.is_some();
    let mut attack_table: Option<String> = None;
    if let Some(block) = fight_ai_block
        && let Some(v) =
            extract_xml_attr(&block, "AttackTable").or_else(|| extract_xml_attr(&block, "Table"))
        && !v.is_empty()
    {
        attack_table = Some(v);
    }

    let target_block = extract_component(&chain, has_components_xml, "Target");
    let has_target = target_block.is_some();

    // Even when AudioPackage is empty we still surface a SoundComponentData
    // for any actor whose template declared `<Sound>`.  Mirrors C++
    // rbAudioSoundComponent: the component is created with an empty package
    // and stays dormant until a runtime swap (audMsgPlaySound::kPlayNamed
    // from a script, or a future PlayerApproach/Knockdown trigger) hands
    // it a real one.  Without this, NPCs that only inherit template_grunt's
    // `<Sound NumChannels="3"/>` wouldn't have a Sound component for those
    // messages to land on.
    let sound = extract_component(&chain, has_components_xml, "Sound").map(|block| {
        let audio_package = extract_xml_attr(&block, "AudioPackage").unwrap_or_default();
        let play_mode = match extract_xml_attr(&block, "PlayMode").as_deref() {
            Some("Play Current One") => SoundPlayMode::CurrentOne,
            Some("Play Next One") => SoundPlayMode::NextOne,
            Some("Play Current Loop") => SoundPlayMode::CurrentOneLooped,
            Some("Play Loop") => SoundPlayMode::FullLoop,
            Some("Play Randomized Loop") => SoundPlayMode::RandomLoop,
            // "Play Random One" is the schema default and also the
            // fallback for unknown / missing values.
            _ => SoundPlayMode::RandomOne,
        };
        let volume_scalar = extract_xml_attr(&block, "VolumeScalar")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);
        let range_max_volume = extract_xml_attr(&block, "RangeMaxVolume")
            .and_then(|v| v.parse().ok())
            .unwrap_or(5.0);
        let range_zero_volume = extract_xml_attr(&block, "RangeZeroVolume")
            .and_then(|v| v.parse().ok())
            .unwrap_or(35.0);
        let start_active = extract_xml_attr_bool(&block, "StartActive").unwrap_or(false);
        let num_channels = extract_xml_attr(&block, "NumChannels")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let cookie_mode = extract_xml_attr_bool(&block, "CookieMode").unwrap_or(false);
        let player_approach_package =
            extract_xml_attr(&block, "PlayerApproachPackage").filter(|s| !s.is_empty());
        let player_knockdown_package =
            extract_xml_attr(&block, "PlayerKnockdownPackage").filter(|s| !s.is_empty());
        SoundComponentData {
            audio_package,
            play_mode,
            volume_scalar,
            range_max_volume,
            range_zero_volume,
            start_active,
            num_channels,
            cookie_mode,
            player_approach_package,
            player_knockdown_package,
        }
    });

    // `<ElbowCraneHitch>` block — declared on pickup-target
    // actors (cnkcrane3/4/5's actor_CraneObject, actor_kno).
    // `Center` is authored in Oni-space; flip to Bevy at parse
    // time so the runtime never sees Oni coords.
    let crane_hitch =
        extract_component(&chain, has_components_xml, "ElbowCraneHitch").map(|block| {
            let center = extract_xml_attr(&block, "Center")
                .and_then(|s| parse_vec3(&s))
                .map(space::to_bevy_space_pos)
                .unwrap_or(Vec3::ZERO);
            let extent = extract_xml_attr(&block, "Extent")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.1);
            CraneHitchComponentData { center, extent }
        });

    // `<ElbowCraneIK>` block — present on the four AAA `actor_Crane*.xml`
    // and the harbor / rooftop crane families.  Only `Speed` and
    // `ClampOffset` are authored attributes; everything else is
    // derived from the EricArm skeleton at init time.
    let crane_ik = extract_component(&chain, has_components_xml, "ElbowCraneIK").map(|block| {
        let speed_deg = extract_xml_attr(&block, "Speed")
            .and_then(|v| v.parse().ok())
            .unwrap_or(275.0);
        let clamp_offset = extract_xml_attr(&block, "ClampOffset")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        CraneIKComponentData {
            speed_deg,
            clamp_offset,
        }
    });

    let mut fvt_radius: Option<f32> = None;
    let mut fvt_directional: Option<bool> = None;
    let mut fvt_offset: Option<Vec3> = None;
    let mut fvt_attack: Option<String> = None;
    if let Some(block) = fight_vector_block {
        if let Some(v) = extract_xml_attr(&block, "Radius") {
            fvt_radius = v.parse().ok();
        }
        if let Some(b) = extract_xml_attr_bool(&block, "Directional") {
            fvt_directional = Some(b);
        } else {
            fvt_directional = Some(true); // Default to directional
        }
        if let Some(v) = extract_xml_attr(&block, "Offset").and_then(|s| parse_vec3(&s)) {
            fvt_offset = Some(space::to_bevy_space_pos(v));
        }
        if let Some(v) = extract_xml_attr(&block, "Attack") {
            fvt_attack = Some(v);
        }
    }

    Some(LayoutActor {
        entity_type,
        position,
        orientation_o2,
        animator_type,
        fighter_type,
        is_creature,
        is_player,
        faction,
        curve_name,
        curve_fixed_orientation,
        curve_look_xz,
        curve_ping_pong,
        curve_speed,
        has_fight_ai,
        attack_table,
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
        weapon_string,
        inventory_can_be_picked_up,
        inventory_pickup_range,
        inventory_drop_items_on_death,
        inventory_drop_range,
        pad_fsm,
        has_target,
        sound,
        fvt_radius,
        fvt_directional,
        fvt_offset,
        fvt_attack,
        crane_ik,
        crane_hitch,
    })
}

// ───────────────────────────────────────────────────────────────────────────
// XML component-coverage validation
// ───────────────────────────────────────────────────────────────────────────

/// Every component tag this parser knows how to consume.  The
/// validator below errors on anything in actor XML that isn't on
/// this list.  When a new component gets wired up, add it here so
/// the error stops firing.
///
/// "Implemented" here means: parsed AND surfaced onto the spawned
/// entity in some form.  Markers like `Editable` (which legacy uses
/// purely to gate level-editor visibility) are included because we
/// intentionally consume them as no-ops; if you don't list a tag
/// here you'll get an error.
const KNOWN_IMPLEMENTED_COMPONENTS: &[&str] = &[
    "Animator",
    "Behavior",
    "BroadcastTrigger",
    "CheckpointTrigger",
    "Creature",
    "Curve",
    "Editable",
    "ElbowCraneHitch",
    "ElbowCraneIK",
    "Entity",
    "FX",
    "FightAI",
    "FightAi",
    "FightVectorTrigger",
    "Fighter",
    "Health",
    "Inventory",
    "Prop",
    "ScrOni",
    "Sound",
    "Target",
];

/// Walk the (already-merged) template chain for an actor and emit
/// an error for every `<TagName name="...">` declaration that isn't
/// in [`KNOWN_IMPLEMENTED_COMPONENTS`].  Deduped per (actor file,
/// tag) so each gap fires exactly once across the whole boot.
///
/// When `has_components_xml` is set the first chain entry is the
/// master `components.xml` schema; that file lists every component
/// type the editor knows about but doesn't reflect anything the
/// actor itself asked for.  We skip it for the same reason
/// `extract_component` does — only declarations in templates or the
/// leaf actor count as "this actor wants component X".
fn validate_xml_components(chain: &[String], has_components_xml: bool, actor_path: &str) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));

    let mut local_seen: HashSet<String> = HashSet::new();
    for (i, content) in chain.iter().enumerate() {
        if has_components_xml && i == 0 {
            continue;
        }
        for tag in find_component_tags(content) {
            if KNOWN_IMPLEMENTED_COMPONENTS.contains(&tag.as_str()) {
                continue;
            }
            if !local_seen.insert(tag.clone()) {
                continue;
            }
            let mut guard = seen.lock().unwrap();
            if guard.insert((actor_path.to_string(), tag.clone())) {
                error!(
                    "Unhandled XML component <{tag}> in {actor_path}: the parser drops it. \
                     Add {tag:?} to KNOWN_IMPLEMENTED_COMPONENTS in parsers/actor_xml.rs once \
                     it's parsed and attached to the spawned entity."
                );
            }
        }
    }
}

/// Find every `<TagName name="...">` opening tag in the given XML
/// content.  Components in the actor XML format always carry a
/// `name="..."` attribute on their open tag — inner data uses
/// `value="..."` instead — so this filter cleanly separates component
/// declarations from attribute payloads.
fn find_component_tags(content: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut rest = content;
    while let Some(idx) = rest.find('<') {
        let after = &rest[idx + 1..];
        let Some(first) = after.chars().next() else {
            break;
        };
        if !first.is_ascii_uppercase() {
            rest = after;
            continue;
        }
        let name_end = after
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(after.len());
        if name_end == 0 {
            rest = after;
            continue;
        }
        let tag_name = &after[..name_end];
        let tail = &after[name_end..];
        let Some(tag_close) = tail.find('>') else {
            break;
        };
        let tag_def = &tail[..tag_close];
        if tag_def.contains(" name=\"") {
            tags.push(tag_name.to_string());
        }
        rest = &tail[tag_close + 1..];
    }
    tags
}
