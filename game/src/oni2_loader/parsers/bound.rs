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

pub fn parse_bound(content: &str) -> Oni2Bound {
    let mut composite_centroid = [0.0f32; 3];
    let mut first_centroid = true;
    let mut sub_bounds: Vec<Oni2SubBound> = Vec::new();
    // Active sub-bound being accumulated; starts as None until we see "bound:"
    // or the first geometry line (for files without explicit "bound:" headers).
    let mut current: Option<Oni2SubBound> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("type:") {
            let parts: Vec<&str> = trimmed.split(':').collect();
            if parts.len() >= 2 {
                let ty = parts[1].trim().to_string();
                current.get_or_insert_with(Oni2SubBound::default).bound_type = Some(ty);
            }
        } else if trimmed.starts_with("bound:") {
            // Push the previous sub-bound (if any) and start a fresh one.
            if let Some(sub) = current.take() {
                sub_bounds.push(sub);
            }
            current = Some(Oni2SubBound::default());
        } else if trimmed.starts_with("v ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let x: f32 = parts[1].parse().unwrap_or(0.0);
                let y: f32 = parts[2].parse().unwrap_or(0.0);
                let z: f32 = parts[3].parse().unwrap_or(0.0);
                // Lazily create a sub-bound for files with no "bound:" header.
                current.get_or_insert_with(Oni2SubBound::default).vertices.push([x, y, z]);
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
                current.get_or_insert_with(Oni2SubBound::default).edges.push([a, b]);
            }
        } else if trimmed.starts_with("quad ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 5 {
                let a: u32 = parts[1].parse().unwrap_or(0);
                let b: u32 = parts[2].parse().unwrap_or(0);
                let c: u32 = parts[3].parse().unwrap_or(0);
                let d: u32 = parts[4].parse().unwrap_or(0);
                current.get_or_insert_with(Oni2SubBound::default).quads.push([a, b, c, d]);
            }
        } else if trimmed.starts_with("tri ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let a: u32 = parts[1].parse().unwrap_or(0);
                let b: u32 = parts[2].parse().unwrap_or(0);
                let c: u32 = parts[3].parse().unwrap_or(0);
                current.get_or_insert_with(Oni2SubBound::default).tris.push([a, b, c]);
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
