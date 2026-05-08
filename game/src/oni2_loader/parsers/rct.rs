/*
 * oni2_loader/parsers/rct.rs — reaction animation enum table.
 *
 * ANIMREACT_NAMES: string table mapping integer animReactEnum values (from
 * .atdt reactanim* fields) to reaction animation name strings, in the order
 * defined by the animReactEnum format specification.  Used by attack_sync_system to resolve
 * which reaction animation to play on the hit target.
 */
use bevy::prelude::*;

/// Reaction animation name table, indexed by animReactEnum integer value.
/// Index == animReactEnum integer value stored in .atdt reactanim* fields.
pub const ANIMREACT_NAMES: &[&str] = &[
    "ANIMREACT_REGULAR",             // 0
    "ANIMREACT_FIGHTSTANCE",         // 1
    "ANIMREACT_FROMBACK",            // 2
    "ANIMREACT_GRAPPLE_SHOT",        // 3
    "ANIMREACT_KNOCKDOWN",           // 4
    "ANIMREACT_GRAPPLE",             // 5
    "ANIMREACT_GENERAL_SLOT_0",      // 6
    "ANIMREACT_GENERAL_SLOT_1",      // 7
    "ANIMREACT_GENERAL_SLOT_2",      // 8
    "ANIMREACT_GENERAL_SLOT_3",      // 9
    "ANIMREACT_GENERAL_SLOT_4",      // 10
    "ANIMREACT_GENERAL_SLOT_5",      // 11
    "ANIMREACT_GENERAL_SLOT_6",      // 12
    "ANIMREACT_GENERAL_SLOT_7",      // 13
    "ANIMREACT_GENERAL_SLOT_8",      // 14
    "ANIMREACT_GENERAL_SLOT_9",      // 15
    "ANIMREACT_GENERAL_SLOT_10",     // 16
    "ANIMREACT_GENERAL_SLOT_11",     // 17
    "ANIMREACT_BLOCKED",             // 18
    "ANIMREACT_ATK_ATTACK_1_MASH",   // 19
    "ANIMREACT_ATK_ATTACK_1_SKILL",  // 20
    "ANIMREACT_ATK_ATTACK_2_MASH",   // 21
    "ANIMREACT_ATK_ATTACK_2_SKILL",  // 22
    "ANIMREACT_ATK_CROUCHING",       // 23
    "ANIMREACT_ATK_STANDING_JUMP",   // 24
    "ANIMREACT_ATK_RUNNING_JUMP",    // 25
    "ANIMREACT_ATK_RUNNING_ATTACK",  // 26
    "ANIMREACT_ATK_GRAPPLE",         // 27
    "ANIMREACT_ATK_SWEEP",           // 28
    "ANIMREACT_ATK_SWEEP_CHRYSALIS", // 29
    "ANIMREACT_GA_SLOT_0",           // 30
    "ANIMREACT_GA_SLOT_1",           // 31
    "ANIMREACT_GA_SLOT_2",           // 32
    "ANIMREACT_GA_SLOT_3",           // 33
    "ANIMREACT_GA_SLOT_4",           // 34
    "ANIMREACT_GA_SLOT_5",           // 35
    "ANIMREACT_GA_SLOT_6",           // 36
    "ANIMREACT_GA_SLOT_7",           // 37
    "ANIMREACT_COUNTER_SLOT_0",      // 38
    "ANIMREACT_COUNTER_SLOT_1",      // 39
    "ANIMREACT_COUNTER_SLOT_2",      // 40
    "ANIMREACT_COUNTER_SLOT_3",      // 41
    "ANIMREACT_COUNTER_SLOT_4",      // 42
    "ANIMREACT_COUNTER_SLOT_5",      // 43
    "ANIMREACT_COUNTER_SLOT_6",      // 44
    "ANIMREACT_COUNTER_SLOT_7",      // 45
    "ANIMREACT_WALL",                // 46
    "ANIMREACT_WALL_GETUP",          // 47
    "ANIMREACT_CRANESTRUGGLE",       // 48
    "ANIMREACT_FROMBACK_STAND_SFT",  // 49
    "ANIMREACT_FROMBACK_STAND_MED",  // 50
    "ANIMREACT_FROMBACK_STAND_HRD",  // 51
    "ANIMREACT_FROMBACK_RUN_SFT",    // 52
    "ANIMREACT_FROMBACK_RUN_MED",    // 53
    "ANIMREACT_FROMBACK_RUN_HRD",    // 54
    "ANIMREACT_FROM_ABOVE",          // 55
    // ANIMDIE_GENERAL shares the animReactEnum sequence in the legacy
    // engine (animator/animenums.h:177 — "sort of a hack for backwards
    // compatibility to put this here").  Keeping it in the same table
    // means the death-anim lookup in the action dispatcher can index
    // either react aliases or the death fallback uniformly.
    "ANIMDIE_GENERAL",               // 56
];

/// Index of `ANIMDIE_GENERAL` in `ANIMREACT_NAMES`.  Used as the death-anim
/// fallback when the killing blow's react doesn't have CanBeDieAnimation.
pub const ANIMDIE_GENERAL_INDEX: i32 = 56;

/// Load all .rct files for an entity into a Vec indexed by animReactEnum integer.
/// Missing entries (entity doesn't override a react) are None.
pub fn load_react_library(entity_name: &str) -> crate::combat::components::ReactLibrary {
    let tune_dir = format!("entity.tune/{}", entity_name);

    let mut entries: Vec<Option<ReactData>> = vec![None; ANIMREACT_NAMES.len()];
    let mut loaded = 0usize;

    for (i, name) in ANIMREACT_NAMES.iter().enumerate() {
        let path = format!("{}/{}.rct", tune_dir, name);
        if let Ok(content) = crate::vfs::read_to_string("", &path) {
            entries[i] = Some(parse_rct_content(&content));
            loaded += 1;
        }
    }

    info!(
        "ReactLibrary for {}: {}/{} reacts loaded",
        entity_name,
        loaded,
        ANIMREACT_NAMES.len()
    );
    crate::combat::components::ReactLibrary { entries }
}

#[derive(Debug, Clone, Reflect, Default)]
pub struct ReactData {
    pub sound_package: String,
    pub invulnerability_start_phase: f32,
    pub get_up_anim: String,
    pub end_rotation_notches: i32,
    pub does_not_take_damage_after_phase: i32,
    pub can_follow_with_wall_react: i32,
    pub sound_frame: i32,
    /// Whether this react animation may be used as the death animation
    /// when the actor is killed by the corresponding strike.  Mirrors
    /// `crReactData::CanBeDieAnimation` (rb/src/fight/reactdata.h:55,
    /// parsed at reactdata.cpp:107).  Defaults to false; the legacy
    /// `ActionStartDie` falls back to `ANIMDIE_GENERAL` when the
    /// killing-blow react can't be reused as a death anim.
    pub can_be_die_animation: bool,
}

pub fn parse_rct_content(content: &str) -> ReactData {
    let mut data = ReactData::default();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let key = parts[0];
        let val = if parts.len() > 1 { parts[1] } else { "" };

        match key {
            "SoundPackage" => data.sound_package = val.to_string(),
            "InvulnerabilityStartPhase" => {
                data.invulnerability_start_phase = val.parse().unwrap_or(0.0)
            }
            "GetUpAnim" => data.get_up_anim = val.replace("\"", ""),
            "EndRotationNotches" => data.end_rotation_notches = val.parse().unwrap_or(0),
            "DoesNotTakeDamageAfterPhase" => {
                data.does_not_take_damage_after_phase = val.parse().unwrap_or(0)
            }
            "CanFollowWithWallReact" => data.can_follow_with_wall_react = val.parse().unwrap_or(0),
            "SoundFrame" => data.sound_frame = val.parse().unwrap_or(0),
            "CanBeDieAnimation" => {
                // Legacy stores this as `1`/`0` (reactdata.cpp:108
                // `CanBeDieAnimation = tok.GetInt() != 0`).
                data.can_be_die_animation = val.parse::<i32>().map(|n| n != 0).unwrap_or(false);
            }
            _ => {}
        }
    }

    data
}
