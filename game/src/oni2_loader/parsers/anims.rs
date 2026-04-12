use bevy::prelude::*;

pub fn parse_anims_content(
    content: &str,
    alias_map: &mut std::collections::HashMap<String, String>,
    loco_pkg: &mut Option<String>,
    jump_pkg: &mut Option<String>,
) {
    let mut in_anims_block = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip comments and blank lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Package reference: "package animpkg.nav.kno"
        if let Some(pkg_ref) = trimmed.strip_prefix("package ") {
            load_apkg_aliases(pkg_ref.trim(), alias_map);
            continue;
        }

        if let Some(jump_ref) = trimmed.strip_prefix("jumppkg ") {
            *jump_pkg = Some(jump_ref.trim().to_string());
            continue;
        }

        // Read locopkg lines
        if let Some(loco_ref) = trimmed.strip_prefix("locopkg ") {
            *loco_pkg = Some(loco_ref.trim().to_string());
            continue;
        }
        if let Some(jump_ref) = trimmed.strip_prefix("jumppkg ") {
            //load_apkg_aliases(jump_ref.trim(), alias_map);
            // TODO: implement jump pkg loading (*.jump)
            continue;
        }

        // Anims block
        if trimmed == "Anims {" {
            in_anims_block = true;
            continue;
        }
        if trimmed == "}" {
            in_anims_block = false;
            continue;
        }

        if in_anims_block {
            // Parse "anim_name ALIAS_NAME" pairs
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 && !parts[0].starts_with('#') {
                alias_map.insert(parts[1].to_string(), parts[0].to_string());
            }
        }
    }
}

/// Parse an entity's .attacks file and return the standing forward combo chain.
/// Entries are kept only when the anim filename contains `_comb_fwd_`, preserving
/// file order — this matches how ONI2's crAttackData drives DoTriggerAtk.
pub fn load_attack_data(entity_dir: &str, entity_name: &str) -> crate::combat::components::AttackData {
    let tune_path = format!("entity.tune/{}/{}.attacks", entity_name, entity_name);
    let entity_path = format!("{}/{}.attacks", entity_dir, entity_name);

    let attacks_path = if crate::vfs::exists("", &tune_path) {
        tune_path
    } else if crate::vfs::exists("", &entity_path) {
        entity_path
    } else {
        return crate::combat::components::AttackData::default();
    };

    let content = match crate::vfs::read_to_string("", &attacks_path) {
        Ok(c) => c,
        Err(_) => return crate::combat::components::AttackData::default(),
    };

    let mut forward_combo = Vec::new();
    let mut in_anims = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "Anims {" { in_anims = true; continue; }
        if trimmed == "}" { in_anims = false; continue; }
        if in_anims {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 && parts[0].contains("_comb_fwd_") {
                forward_combo.push(parts[1].to_string());
            }
        }
    }

    info!("AttackData for {}: {} forward combo entries", entity_name, forward_combo.len());
    crate::combat::components::AttackData { forward_combo }
}

/// Load aliases from a .apkg file referenced by dotted path (e.g. "animpkg.nav.kno").
pub fn load_apkg_aliases(
    dotted_path: &str,
    alias_map: &mut std::collections::HashMap<String, String>,
) {
    // Convert dotted path to file path: "animpkg.nav.kno" -> "entity.tune/animpkg/nav/kno.apkg"
    let parts: Vec<&str> = dotted_path.split('.').collect();
    if parts.len() < 2 {
        return;
    }

    let filename = parts.last().unwrap();
    let dir_parts = &parts[..parts.len() - 1];
    let mut apkg_dir = "entity.tune".to_string();
    for p in dir_parts {
        apkg_dir = format!("{}/{}", apkg_dir, p);
    }
    let apkg_filename = format!("{}.apkg", filename);

    let content = match crate::vfs::read_to_string(&apkg_dir, &apkg_filename) {
        Ok(c) => c,
        Err(_) => {
            warn!("Could not read apkg: {}/{}", apkg_dir, apkg_filename);
            return;
        }
    };

    // Parse Anims { } block inside apkg
    let mut in_anims = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "Anims {" {
            in_anims = true;
            continue;
        }
        if trimmed == "}" {
            in_anims = false;
            continue;
        }
        if in_anims {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 && !parts[0].starts_with('#') {
                alias_map.insert(parts[1].to_string(), parts[0].to_string());
            }
        }
    }
}
