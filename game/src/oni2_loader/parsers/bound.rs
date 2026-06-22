/*
 * oni2_loader/parsers/bound.rs — .bound bounding geometry parser.
 *
 * parse_bound: parses both simple (single-bound) and composite (multi-bound)
 * .bnd files into a Vec<Oni2SubBound> — one entry per "bound: N" section.
 * Each sub-bound owns its vertices and has indices that are always 0-based
 * relative to its own vertex array.  Coordinate conversion (Oni2 left-handed
 * → Bevy right-handed, negate X and Z) is applied to every sub-bound.
 */
use super::types::{Oni2Bound, Oni2SubBound};

/// If the polygon's material index maps to a conveyor material, record its
/// speed on the sub-bound.  The fastest conveyor material wins if a single
/// sub-bound mixes several (in practice a conveyor bound is uniform).
fn apply_poly_material(
    sub: &mut Oni2SubBound,
    mtl_index: Option<usize>,
    material_conveyor: &[Option<f32>],
) {
    if let Some(speed) = mtl_index
        .and_then(|i| material_conveyor.get(i))
        .copied()
        .flatten()
    {
        sub.conveyor_speed = Some(match sub.conveyor_speed {
            Some(existing) => existing.max(speed),
            None => speed,
        });
    }
}

pub fn parse_bound(content: &str) -> Oni2Bound {
    let mut composite_centroid = [0.0f32; 3];
    let mut first_centroid = true;
    let mut sub_bounds: Vec<Oni2SubBound> = Vec::new();
    // Active sub-bound being accumulated; starts as None until we see "bound:"
    // or the first geometry line (for files without explicit "bound:" headers).
    let mut current: Option<Oni2SubBound> = None;

    // Per-material conveyor speed for the materials declared in `mtl { ... }`
    // blocks of the CURRENT sub-bound, in declaration order.  A poly's trailing
    // material index selects into this list.  `None` = not a conveyor material.
    // Reset whenever a new "bound:" section starts.
    let mut material_conveyor: Vec<Option<f32>> = Vec::new();
    // State while inside an `mtl <name> { ... }` block.
    let mut in_mtl_block = false;
    let mut mtl_conveyor = false;
    let mut mtl_conveyor_speed = 0.0f32;

    for line in content.lines() {
        let trimmed = line.trim();

        // --- Physics-material block (mirrors rbPhysMaterial::LoadData) -------
        // Conveyor / ConveyorSpeed are the only flags the runtime consumes; the
        // rest (Fatal, WallJump, Injure, ...) are parsed-past until the closing
        // brace.  Handled before the geometry branches so the inner tokens
        // (e.g. "Conveyor: 1") don't fall through to other arms.
        if in_mtl_block {
            if trimmed.starts_with('}') {
                material_conveyor.push(
                    (mtl_conveyor && mtl_conveyor_speed > 0.0).then_some(mtl_conveyor_speed),
                );
                in_mtl_block = false;
            } else if let Some(rest) = trimmed.strip_prefix("Conveyor:") {
                mtl_conveyor = rest.trim().parse::<i32>().unwrap_or(0) != 0;
            } else if let Some(rest) = trimmed.strip_prefix("ConveyorSpeed:") {
                mtl_conveyor_speed = rest.trim().parse().unwrap_or(0.0);
            }
            continue;
        }
        if trimmed.starts_with("mtl ") {
            in_mtl_block = true;
            mtl_conveyor = false;
            mtl_conveyor_speed = 0.0;
            continue;
        }

        if trimmed.starts_with("type:") {
            let parts: Vec<&str> = trimmed.split(':').collect();
            if parts.len() >= 2 {
                let ty = parts[1].trim().to_string();
                let sub = current.get_or_insert_with(Oni2SubBound::default);
                if sub.bound_type.is_none() {
                    sub.bound_type = Some(ty);
                } else if sub.material_type.is_none() {
                    sub.material_type = Some(ty);
                }
            }
        } else if trimmed.starts_with("bound:") {
            // Push the previous sub-bound (if any) and start a fresh one.
            if let Some(sub) = current.take() {
                sub_bounds.push(sub);
            }
            current = Some(Oni2SubBound::default());
            // Materials are declared per sub-bound section; start clean.
            material_conveyor.clear();
        } else if trimmed.starts_with("v ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let x: f32 = parts[1].parse().unwrap_or(0.0);
                let y: f32 = parts[2].parse().unwrap_or(0.0);
                let z: f32 = parts[3].parse().unwrap_or(0.0);
                // Lazily create a sub-bound for files with no "bound:" header.
                current
                    .get_or_insert_with(Oni2SubBound::default)
                    .vertices
                    .push([x, y, z]);
            }
        } else if trimmed.starts_with("centroid:") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let cx: f32 = parts[1].parse().unwrap_or(0.0);
                let cy: f32 = parts[2].parse().unwrap_or(0.0);
                let cz: f32 = parts[3].parse().unwrap_or(0.0);
                // The very first centroid line is the composite centroid (top of
                // the file before any "bound:" section).  Each sub-bound also has
                // its own centroid line — store it on the current sub-bound.
                if first_centroid {
                    composite_centroid = [cx, cy, cz];
                    first_centroid = false;
                }
                if let Some(ref mut sub) = current {
                    sub.centroid = [cx, cy, cz];
                }
            }
        } else if trimmed.starts_with("edge ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let a: u32 = parts[1].parse().unwrap_or(0);
                let b: u32 = parts[2].parse().unwrap_or(0);
                // Edge normals may follow indices on the same line — we only need a, b.
                current
                    .get_or_insert_with(Oni2SubBound::default)
                    .edges
                    .push([a, b]);
            }
        } else if trimmed.starts_with("quad ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 5 {
                let a: u32 = parts[1].parse().unwrap_or(0);
                let b: u32 = parts[2].parse().unwrap_or(0);
                let c: u32 = parts[3].parse().unwrap_or(0);
                let d: u32 = parts[4].parse().unwrap_or(0);
                // Format: `quad v0 v1 v2 v3 mtlIndex e0 e1 e2 e3` — the material
                // index follows the four vertex indices.
                let mtl = parts.get(5).and_then(|s| s.parse::<usize>().ok());
                let sub = current.get_or_insert_with(Oni2SubBound::default);
                sub.quads.push([a, b, c, d]);
                apply_poly_material(sub, mtl, &material_conveyor);
            }
        } else if trimmed.starts_with("tri ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let a: u32 = parts[1].parse().unwrap_or(0);
                let b: u32 = parts[2].parse().unwrap_or(0);
                let c: u32 = parts[3].parse().unwrap_or(0);
                // Format: `tri v0 v1 v2 mtlIndex [e0 e1 e2]` — material index
                // follows the three vertex indices.
                let mtl = parts.get(4).and_then(|s| s.parse::<usize>().ok());
                let sub = current.get_or_insert_with(Oni2SubBound::default);
                sub.tris.push([a, b, c]);
                apply_poly_material(sub, mtl, &material_conveyor);
            }
        }
    }

    // Flush the final (or only) sub-bound.
    if let Some(sub) = current.take() {
        sub_bounds.push(sub);
    }

    // Convert from Oni2 left-handed to Bevy right-handed: negate X and Z.
    for sub in &mut sub_bounds {
        for v in &mut sub.vertices {
            v[0] = -v[0];
            v[2] = -v[2];
        }
        sub.centroid[0] = -sub.centroid[0];
        sub.centroid[2] = -sub.centroid[2];
    }
    composite_centroid[0] = -composite_centroid[0];
    composite_centroid[2] = -composite_centroid[2];

    Oni2Bound {
        sub_bounds,
        centroid: composite_centroid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim shape of `Entity/TestConveyor/Bound.bnd`: two materials
    /// (`default` non-conveyor, `rbPhysMat1SG` conveyor @ speed 1.0) and a
    /// single quad selecting material index 1.
    const CONVEYOR_BND: &str = "\
version: 1.10
type: geometry
verts: 4
materials: 2
edges: 4
polys: 1
readedgenormals: 1

v 2.985560 0.044274 -26.047058
v -2.985560 0.044274 -26.047058
v 2.985560 0.044274 26.047058
v -2.985560 0.044274 26.047058

type: rbPhysMat
mtl default {
\telasticity: 0.099998
\tfriction: 0.500000
\tConveyor: 0
\tConveyorSpeed: 0.000000
}

type: rbPhysMat
mtl rbPhysMat1SG {
\telasticity: 0.010000
\tfriction: 1.000000
\tConveyor: 1
\tConveyorSpeed: 1.000000
}

edge 2 0 0.0 1.0 0.0
edge 0 1 0.0 1.0 0.0
edge 1 3 0.0 1.0 0.0
edge 3 2 0.0 1.0 0.0

quad 0 1 3 2 1 1 2 3 0
";

    #[test]
    fn conveyor_material_index_resolves_to_speed() {
        let bound = parse_bound(CONVEYOR_BND);
        assert_eq!(bound.sub_bounds.len(), 1);
        let sub = &bound.sub_bounds[0];
        assert_eq!(sub.quads.len(), 1);
        // The quad selects material index 1 (rbPhysMat1SG), so the sub-bound
        // is a conveyor running at speed 1.0.
        assert_eq!(sub.conveyor_speed, Some(1.0));
    }

    #[test]
    fn non_conveyor_quad_has_no_speed() {
        // Same geometry but the quad selects material 0 (Conveyor: 0).
        let bnd = CONVEYOR_BND.replace("quad 0 1 3 2 1 1 2 3 0", "quad 0 1 3 2 0 1 2 3 0");
        let bound = parse_bound(&bnd);
        assert_eq!(bound.sub_bounds[0].conveyor_speed, None);
    }
}
