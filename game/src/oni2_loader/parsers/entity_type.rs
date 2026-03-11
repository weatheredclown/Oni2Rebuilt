use super::types::Oni2EntityType;

pub fn parse_entity_type(content: &str) -> Oni2EntityType {
    let mut model_file = None;
    let mut bound_file = None;
    let mut skel_file = None;
    let mut lod_radius = 0.0;

    for line in content.lines() {
        let trimmed = line.trim();
        let upper = trimmed.to_uppercase();

        // Handle both flat format (HIGH/LOD at top level) and braced LodGroup format
        // LodGroup format: "high kno_LODs0.mod 20" inside a LodGroup { } block
        if upper.starts_with("HIGH") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 && parts[1].to_lowercase() != "none" {
                let mut name = parts[1].to_string();
                if !name.ends_with(".mod") {
                    name.push_str(".mod");
                }
                model_file = Some(name);
            }
        } else if upper.starts_with("LOD") && !upper.starts_with("LODGROUP") {
            if model_file.is_none() {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 && parts[1].to_lowercase() != "none" {
                    let mut name = parts[1].to_string();
                    if !name.ends_with(".mod") {
                        name.push_str(".mod");
                    }
                    model_file = Some(name);
                }
            }
        } else if upper.starts_with("SKEL") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 && parts[1].to_lowercase() != "none" {
                let mut name = parts[1].to_string();
                if !name.ends_with(".skel") {
                    name.push_str(".skel");
                }
                skel_file = Some(name);
            }
        } else if upper.starts_with("BOUND") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 && parts[1].to_lowercase() != "none" {
                bound_file = Some(parts[1].to_string());
            }
        } else if upper.starts_with("RADIUS") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                lod_radius = parts[1].parse().unwrap_or(0.0);
            }
        }
    }

    Oni2EntityType {
        model_file,
        bound_file,
        skel_file,
        lod_radius,
    }
}
