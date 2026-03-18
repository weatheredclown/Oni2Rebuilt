use super::types::Oni2Bound;

pub fn parse_bound(content: &str) -> Oni2Bound {
    let mut vertices = Vec::new();
    let mut centroid = [0.0; 3];
    let mut edges = Vec::new();
    let mut quads = Vec::new();
    let mut tris = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("v ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let x: f32 = parts[1].parse().unwrap_or(0.0);
                let y: f32 = parts[2].parse().unwrap_or(0.0);
                let z: f32 = parts[3].parse().unwrap_or(0.0);
                vertices.push([x, y, z]);
            }
        } else if trimmed.starts_with("centroid:") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                centroid[0] = parts[1].parse().unwrap_or(0.0);
                centroid[1] = parts[2].parse().unwrap_or(0.0);
                centroid[2] = parts[3].parse().unwrap_or(0.0);
            }
        } else if trimmed.starts_with("edge ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let a: u32 = parts[1].parse().unwrap_or(0);
                let b: u32 = parts[2].parse().unwrap_or(0);
                edges.push([a, b]);
            }
        } else if trimmed.starts_with("quad ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 5 {
                let a: u32 = parts[1].parse().unwrap_or(0);
                let b: u32 = parts[2].parse().unwrap_or(0);
                let c: u32 = parts[3].parse().unwrap_or(0);
                let d: u32 = parts[4].parse().unwrap_or(0);
                quads.push([a, b, c, d]);
            }
        } else if trimmed.starts_with("tri ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let a: u32 = parts[1].parse().unwrap_or(0);
                let b: u32 = parts[2].parse().unwrap_or(0);
                let c: u32 = parts[3].parse().unwrap_or(0);
                tris.push([a, b, c]);
            }
        }
    }

    // Convert from left-handed to right-handed: 180° Y rotation (negate X and Z)
    for v in &mut vertices {
        v[0] = -v[0];
        v[2] = -v[2];
    }
    centroid[0] = -centroid[0];
    centroid[2] = -centroid[2];

    Oni2Bound { vertices, centroid, edges, quads, tris }
}
