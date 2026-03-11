use bevy::prelude::*;


/// Parse the content of a .anims file, populating alias_map with (ALIAS -> anim_filename) pairs.
pub fn parse_anims_content(
    content: &str,
    alias_map: &mut std::collections::HashMap<String, String>,
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

        // Skip loco/jump package lines
        if trimmed.starts_with("locopkg ") || trimmed.starts_with("jumppkg ") {
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
