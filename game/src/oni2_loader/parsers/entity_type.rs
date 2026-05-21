/*
 * oni2_loader/parsers/entity_type.rs — Entity.type / drawable .type parsers.
 *
 * parse_drawable_type: the GENERIC text-`.type` reader.  Walks both flat
 * (top-level `HIGH`/`LOD`/`SKEL`/`BOUND`/`RADIUS`) and `LodGroup { ... }`
 * formats.  Returns raw basenames exactly as they appear in the file —
 * no extension defaulting.  Callers that expect a particular extension
 * (entities want `.mod`/`.skel`, FX want `.mesh`) apply it themselves.
 *
 * parse_entity_type: the entity-flavoured wrapper.  Calls
 * `parse_drawable_type`, then for each raw basename that's missing the
 * entity-conventional extension, appends `.mod` (model) or `.skel`
 * (skeleton).  Bound names are passed through verbatim (legacy entities
 * sometimes write `bound none` and sometimes `bound BoundFile`; no
 * extension convention).  Output: `Oni2EntityType` with extensioned
 * filenames ready to feed into the VFS.
 *
 * Used by spawn_oni2_entity to locate all sub-assets for a character.
 * The standalone-drawable loader (`load_drawable`, blast-fire fx) calls
 * `parse_drawable_type` directly so it can preserve `.mesh` references.
 */
use super::types::Oni2EntityType;

/// Generic `.type` manifest contents.  Basenames are exactly as they
/// appear in the source file (with or without extension); callers add
/// extensions as needed.
#[derive(Debug, Default, Clone)]
pub struct DrawableType {
    /// Mesh basename from the first non-`none` `HIGH`/`LOD`/`high`/`lod`
    /// entry (either flat or inside `LodGroup { ... }`).
    pub mesh_name: Option<String>,
    /// Skeleton basename from a top-level `SKEL` / `skel` line.  `none`
    /// → `None`.
    pub skel_name: Option<String>,
    /// Bound (collision shape) basename from a `BOUND` / `bound` line.
    pub bound_name: Option<String>,
    /// Edge basename from an `EDGE` / `edge` line.  FX `.type` files
    /// include this (always `none` in practice for blast fire).
    pub edge_name: Option<String>,
    /// LOD switch radius from `RADIUS` / `radius`.  Zero if unset.
    pub lod_radius: f32,
}

/// Generic `.type` reader.  See module docs for what's parsed and what
/// isn't.  Does NOT inject file extensions — that's the caller's job.
pub fn parse_drawable_type(content: &str) -> DrawableType {
    let mut out = DrawableType::default();

    for line in content.lines() {
        let trimmed = line.trim();
        let upper = trimmed.to_uppercase();

        if upper.starts_with("HIGH") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 && parts[1].to_lowercase() != "none" {
                out.mesh_name = Some(parts[1].to_string());
            }
        } else if upper.starts_with("LOD") && !upper.starts_with("LODGROUP") {
            // Only fill from LOD if HIGH wasn't already set (HIGH wins).
            if out.mesh_name.is_none() {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 && parts[1].to_lowercase() != "none" {
                    out.mesh_name = Some(parts[1].to_string());
                }
            }
        } else if upper.starts_with("SKEL") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 && parts[1].to_lowercase() != "none" {
                out.skel_name = Some(parts[1].to_string());
            }
        } else if upper.starts_with("BOUND") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 && parts[1].to_lowercase() != "none" {
                out.bound_name = Some(parts[1].to_string());
            }
        } else if upper.starts_with("EDGE") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 && parts[1].to_lowercase() != "none" {
                out.edge_name = Some(parts[1].to_string());
            }
        } else if upper.starts_with("RADIUS") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                out.lod_radius = parts[1].parse().unwrap_or(0.0);
            }
        }
    }

    out
}

/// Entity-flavoured wrapper around `parse_drawable_type`.  Appends the
/// entity-conventional extension (`.mod` for the model, `.skel` for
/// the skeleton) when the raw basename in the `.type` lacks one.  Bound
/// names are passed through verbatim because the legacy convention here
/// is messier and the loader handles both cases downstream.
pub fn parse_entity_type(content: &str) -> Oni2EntityType {
    let raw = parse_drawable_type(content);

    let model_file = raw.mesh_name.map(|n| {
        if n.ends_with(".mod") {
            n
        } else {
            format!("{}.mod", n)
        }
    });
    let skel_file = raw.skel_name.map(|n| {
        if n.ends_with(".skel") {
            n
        } else {
            format!("{}.skel", n)
        }
    });

    Oni2EntityType {
        model_file,
        bound_file: raw.bound_name,
        skel_file,
        lod_radius: raw.lod_radius,
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

    /// Regression: an FX `.type` (`blast_fire.type`) declares its mesh
    /// with an explicit `.mesh` extension.  The generic reader must
    /// preserve that verbatim — the entity wrapper's `.mod` defaulting
    /// only kicks in when no extension is present, so this exercises
    /// the FX path through `parse_drawable_type` directly.
    #[test]
    fn drawable_type_preserves_explicit_mesh_extension() {
        let text = r#"
LodGroup {
    high 	blast_fire.mesh	9999
    med  	none	9999
    low  	none	9999
    vlow 	none	9999
    radius	13
}
skel 	none
edge 	none
"#;

        let raw = parse_drawable_type(text);
        assert_eq!(raw.mesh_name.as_deref(), Some("blast_fire.mesh"));
        assert!(raw.skel_name.is_none());
        assert!(raw.bound_name.is_none());
        assert!(raw.edge_name.is_none());
        assert!((raw.lod_radius - 13.0).abs() < f32::EPSILON);
    }
}
