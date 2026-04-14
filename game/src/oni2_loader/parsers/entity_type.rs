/*
 * oni2_loader/parsers/entity_type.rs — Entity.type file parser.
 *
 * parse_entity_type: reads the text-format Entity.type for a character directory.
 * Returns Oni2EntityType with model_file, bound_file, skel_file, and lod_radius.
 * Used by spawn_oni2_entity to locate all sub-assets for a character.
 */
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
        jump_controller: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(content: &str) -> Oni2EntityType {
        parse_entity_type(content)
    }

    #[test]
    fn parses_flat_format_with_missing_extensions() {
        let text = r#"
        {
            BASICENTITY kno
                LOD     kno_LODs0
                SKEL    kno_skel
                BOUND   Bound
                RADIUS  42.5
        }
        "#;

        let parsed = parsed(text);
        assert_eq!(parsed.model_file.as_deref(), Some("kno_LODs0.mod"));
        assert_eq!(parsed.skel_file.as_deref(), Some("kno_skel.skel"));
        assert_eq!(parsed.bound_file.as_deref(), Some("Bound"));
        assert!((parsed.lod_radius - 42.5).abs() < f32::EPSILON);
    }

    #[test]
    fn prefers_high_over_followup_lod_entries() {
        let text = r#"
        LodGroup {
            high    HeroShape0        9999
            lod     HeroFallback      200
            radius  38.739
        }
        "#;

        let parsed = parsed(text);
        assert_eq!(parsed.model_file.as_deref(), Some("HeroShape0.mod"));
        assert!((parsed.lod_radius - 38.739).abs() < f32::EPSILON);
    }

    #[test]
    fn ignores_none_entries_and_preserves_extensions() {
        let text = r#"
        version: 101
        renderable {
            LodGroup {
                high    none    0
                lod     gadgetShape1.mod 0
                radius  15
            }
            skel    none
        }
        physics {
            bound   none
        }
        "#;

        let parsed = parsed(text);
        assert_eq!(parsed.model_file.as_deref(), Some("gadgetShape1.mod"));
        assert!(parsed.skel_file.is_none());
        assert!(parsed.bound_file.is_none());
    }
}
