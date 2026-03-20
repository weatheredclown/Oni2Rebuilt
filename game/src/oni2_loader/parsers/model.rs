use bevy::prelude::*;
use crate::oni2_loader::utils::binary::{read_u32_le, read_u16_le, read_f32_le};
use super::types::{Oni2Model, Oni2Material, Oni2Packet, Oni2Adjunct, Oni2MaterialPass};

pub fn parse_mod(content: &str, entity_dir: &str) -> Oni2Model {
    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut colors = Vec::new();
    let mut tex_coords = Vec::new();
    let mut materials = Vec::new();
    let mut packets = Vec::new();

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        // Vertex
        if trimmed.starts_with("v\t") || trimmed.starts_with("v ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let x: f32 = parts[1].parse().unwrap_or(0.0);
                let y: f32 = parts[2].parse().unwrap_or(0.0);
                let z: f32 = parts[3].parse().unwrap_or(0.0);
                vertices.push([x, y, z]);
            }
        }
        // Normal
        else if trimmed.starts_with("n\t") || trimmed.starts_with("n ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let x: f32 = parts[1].parse().unwrap_or(0.0);
                let y: f32 = parts[2].parse().unwrap_or(0.0);
                let z: f32 = parts[3].parse().unwrap_or(0.0);
                normals.push([x, y, z]);
            }
        }
        // Color
        else if trimmed.starts_with("c\t") || trimmed.starts_with("c ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 5 {
                let r: f32 = parts[1].parse().unwrap_or(1.0);
                let g: f32 = parts[2].parse().unwrap_or(1.0);
                let b: f32 = parts[3].parse().unwrap_or(1.0);
                let a: f32 = parts[4].parse().unwrap_or(1.0);
                colors.push([r, g, b, a]);
            }
        }
        // Texture coordinate
        else if trimmed.starts_with("t1\t") || trimmed.starts_with("t1 ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let u: f32 = parts[1].parse().unwrap_or(0.0);
                let v: f32 = parts[2].parse().unwrap_or(0.0);
                tex_coords.push([u, v]);
            }
        }
        // Material
        else if trimmed.starts_with("mtl ") && trimmed.contains('{') {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let name = parts[1].to_string();
            let mut diffuse = [0.8, 0.8, 0.8];
            let mut texture_name = None;
            let mut primitive_count = 0u32;

            i += 1;
            while i < lines.len() {
                let mtl_line = lines[i].trim();
                if mtl_line == "}" {
                    break;
                }
                if mtl_line.starts_with("diffuse:") {
                    let parts: Vec<&str> = mtl_line.split_whitespace().collect();
                    if parts.len() >= 4 {
                        diffuse[0] = parts[1].parse().unwrap_or(0.8);
                        diffuse[1] = parts[2].parse().unwrap_or(0.8);
                        diffuse[2] = parts[3].parse().unwrap_or(0.8);
                    }
                } else if mtl_line.starts_with("texture:") {
                    let parts: Vec<&str> = mtl_line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        let tex = parts[2].trim_matches('"').to_string();
                        texture_name = Some(tex);
                    }
                } else if mtl_line.starts_with("primitives:") {
                    let parts: Vec<&str> = mtl_line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        primitive_count = parts[1].parse().unwrap_or(0);
                    }
                }
                i += 1;
            }

            let shader_name = format!("{}.shader", name);
            if let Ok(sha_content) = crate::vfs::read_to_string(&entity_dir, &shader_name) {
                // TODO: put shader loading here
                
            }
                        let mut passes = Vec::new();
            let shader_name = format!("{}.shader", name);
            if let Ok(sha_content) = crate::vfs::read_to_string(entity_dir, &shader_name) {
                passes = parse_shader(&sha_content);
            }

            materials.push(Oni2Material {
                name,
                diffuse,
                texture_name,
                primitive_count,
                packet_count: 0, // ASCII parser sets this via count_packets_for_material
                passes,
            });
        }
        // Packet
        else if trimmed.starts_with("packet ") && trimmed.contains('{') {
            // packet <adj_count> <strip_count> <matrix_count> {
            let mut adjuncts = Vec::new();
            let mut strips = Vec::new();
            let mut strip_types: Vec<u32> = Vec::new();
            let mut bone_map = Vec::new();

            i += 1;
            while i < lines.len() {
                let pkt_line = lines[i].trim();
                if pkt_line == "}" {
                    break;
                }
                if pkt_line.starts_with("adj") {
                    let parts: Vec<&str> = pkt_line.split_whitespace().collect();
                    if parts.len() >= 5 {
                        let bone_idx = if parts.len() >= 7 {
                            parts[6].parse().unwrap_or(0)
                        } else {
                            0
                        };
                        adjuncts.push(Oni2Adjunct {
                            vertex_idx: parts[1].parse().unwrap_or(0),
                            normal_idx: parts[2].parse().unwrap_or(0),
                            color_idx: parts[3].parse().unwrap_or(0),
                            tex1_idx: parts[4].parse().unwrap_or(-1),
                            bone_idx,
                        });
                    }
                } else if pkt_line.starts_with("stp") || pkt_line.starts_with("str")
                    || pkt_line.starts_with("tri")
                {
                    let parts: Vec<&str> = pkt_line.split_whitespace().collect();
                    if pkt_line.starts_with("tri") {
                        // tri a b c — individual triangle
                        if parts.len() >= 4 {
                            let indices: Vec<u32> = parts[1..4]
                                .iter()
                                .filter_map(|s| s.parse().ok())
                                .collect();
                            strips.push(indices);
                            strip_types.push(1); // tri = normal winding
                        }
                    } else if parts.len() >= 2 {
                        let stype = if pkt_line.starts_with("stp") { 2u32 } else { 1u32 };
                        let count: usize = parts[1].parse().unwrap_or(0);
                        let indices: Vec<u32> = parts[2..2 + count]
                            .iter()
                            .filter_map(|s| s.parse().ok())
                            .collect();
                        strips.push(indices);
                        strip_types.push(stype);
                    }
                } else if pkt_line.starts_with("mtx") && !pkt_line.starts_with("mtxv") && !pkt_line.starts_with("mtxn") {
                    let parts: Vec<&str> = pkt_line.split_whitespace().collect();
                    bone_map = parts[1..].iter()
                        .filter_map(|s| s.parse().ok())
                        .collect();
                }
                i += 1;
            }

            // Determine which material this packet belongs to based on order
            let material_index = packets.len().min(materials.len().saturating_sub(1));

            packets.push(Oni2Packet {
                adjuncts,
                strips,
                strip_types,
                material_index,
                bone_map,
            });
        }

        i += 1;
    }

    // Assign packets to materials more precisely:
    // Materials list their packet counts; assign sequentially
    let mut packet_idx = 0;
    for (mat_idx, mat) in materials.iter().enumerate() {
        // Each material has "packets: N" — we parsed packet blocks in order
        // Find how many packets each material owns by looking at packets: field
        // For simplicity, look at the packet count from content
        let pkt_count = count_packets_for_material(content, &mat.name);
        for _ in 0..pkt_count {
            if packet_idx < packets.len() {
                packets[packet_idx].material_index = mat_idx;
                packet_idx += 1;
            }
        }
    }

    Oni2Model {
        vertices,
        normals,
        colors,
        tex_coords,
        materials,
        packets,
        bone_world_positions: Vec::new(),
        bone_rotations: Vec::new(),
        world_space_verts: false,
    }
}

fn count_packets_for_material(content: &str, material_name: &str) -> usize {
    let mut in_material = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("mtl ") && trimmed.contains(material_name) && trimmed.contains('{') {
            in_material = true;
            continue;
        }
        if in_material {
            if trimmed.starts_with("packets:") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                return parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
            }
            if trimmed == "}" {
                in_material = false;
            }
        }
    }
    1
}

pub fn parse_shader(content: &str) -> Vec<Oni2MaterialPass> {
    let mut passes = Vec::new();
    let mut current_pass = Oni2MaterialPass::default();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        
        if trimmed == "nextpass" {
            passes.push(current_pass);
            current_pass = Oni2MaterialPass::default();
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() { continue; }
        
        match parts[0].to_lowercase().as_str() {
            "texture" => {
                if parts.len() > 1 {
                    current_pass.texture_name = Some(parts[1].trim_matches('"').to_string());
                }
            }
            "lighting" => {
                if parts.len() > 1 { current_pass.lighting = Some(parts[1].to_string()); }
            }
            "blendset" => {
                if parts.len() > 1 { current_pass.blendset = Some(parts[1].to_string()); }
            }
            "texcombine" => {
                if parts.len() > 1 { current_pass.texcombine = Some(parts[1].to_string()); }
            }
            "texsrc" => {
                if parts.len() > 1 { current_pass.texsrc = parts[1].parse().ok(); }
            }
            "alphafunc" => {
                if parts.len() > 1 { current_pass.alphafunc = Some(parts[1].to_string()); }
            }
            _ => {}
        }
    }
    
    passes.push(current_pass);
    passes
}

pub fn parse_mod_binary(data: &[u8], entity_dir: &str) -> Option<Oni2Model> {
    if data.len() < 58 {
        warn!("Binary .mod file too small: {} bytes", data.len());
        return None;
    }

    // Verify header
    if &data[..13] != b"version: 2.10" {
        warn!("Not a v2.10 binary .mod file");
        return None;
    }

    // Read 11 u32 counts at offset 14
    let n_verts = read_u32_le(data, 14) as usize;
    let n_normals = read_u32_le(data, 18) as usize;
    let n_colors = read_u32_le(data, 22) as usize;
    let n_tex1s = read_u32_le(data, 26) as usize;
    let n_tex2s = read_u32_le(data, 30) as usize;
    let n_tangents = read_u32_le(data, 34) as usize;
    let n_materials = read_u32_le(data, 38) as usize;
    let n_adjuncts = read_u32_le(data, 42) as usize;
    let n_primitives = read_u32_le(data, 46) as usize;
    let n_matrices = read_u32_le(data, 50) as usize;
    let _n_reskins = read_u32_le(data, 54) as usize;

    info!("Binary v2.10: verts={} normals={} colors={} tex1s={} materials={} adjuncts={} primitives={} matrices={}",
        n_verts, n_normals, n_colors, n_tex1s, n_materials, n_adjuncts, n_primitives, n_matrices);

    let mut off = 58usize;

    // Read vertices: n_verts × 3 × f32
    let mut vertices = Vec::with_capacity(n_verts);
    for _ in 0..n_verts {
        if off + 12 > data.len() { break; }
        vertices.push([read_f32_le(data, off), read_f32_le(data, off + 4), read_f32_le(data, off + 8)]);
        off += 12;
    }

    // Read normals: n_normals × 3 × f32
    let mut normals = Vec::with_capacity(n_normals);
    for _ in 0..n_normals {
        if off + 12 > data.len() { break; }
        normals.push([read_f32_le(data, off), read_f32_le(data, off + 4), read_f32_le(data, off + 8)]);
        off += 12;
    }

    // Read colors: n_colors × 4 × f32
    let mut colors = Vec::with_capacity(n_colors);
    for _ in 0..n_colors {
        if off + 16 > data.len() { break; }
        colors.push([read_f32_le(data, off), read_f32_le(data, off + 4), read_f32_le(data, off + 8), read_f32_le(data, off + 12)]);
        off += 16;
    }

    // Read tex1s: n_tex1s × 2 × f32
    let mut tex_coords = Vec::with_capacity(n_tex1s);
    for _ in 0..n_tex1s {
        if off + 8 > data.len() { break; }
        tex_coords.push([read_f32_le(data, off), read_f32_le(data, off + 4)]);
        off += 8;
    }

    // Skip tex2s and tangents
    off += n_tex2s * 8;
    off += n_tangents * 12;

    // Parse materials sequentially — variable-length records, field by field.
    // AGE binary format: data is sequentially parsed, strings are inline (not fixed-width).
    let mut materials = Vec::with_capacity(n_materials);
    for _ in 0..n_materials {
        // 1. Material name: space-terminated string
        let name_start = off;
        while off < data.len() && data[off] != 0x20 && data[off] != 0x00 {
            off += 1;
        }
        let name = std::str::from_utf8(&data[name_start..off]).unwrap_or("").to_string();
        if off < data.len() { off += 1; } // skip space terminator

        // 2. u32 fields: packet_count, primitive_count, texture_count, illum
        if off + 16 > data.len() { break; }
        let pkt_count = read_u32_le(data, off);       off += 4;
        let prim_count = read_u32_le(data, off);      off += 4;
        let _tex_count = read_u32_le(data, off);      off += 4;
        let _illum = read_u32_le(data, off);           off += 4;

        // 3. 9 floats: ambient(3) + diffuse(3) + specular(3)
        if off + 36 > data.len() { break; }
        // Skip ambient (3 floats)
        off += 12;
        // Read diffuse (3 floats)
        let dr = read_f32_le(data, off);
        let dg = read_f32_le(data, off + 4);
        let db = read_f32_le(data, off + 8);
        off += 12;
        // Skip specular (3 floats)
        off += 12;

        // 4. Skip extra values until we hit a printable ASCII char (texture name start)
        while off + 4 <= data.len() {
            if data[off] >= b'a' && data[off] <= b'z' || data[off] >= b'A' && data[off] <= b'Z' {
                break;
            }
            off += 4;
        }

        // 5. Texture name: null-terminated, then skip all consecutive null bytes
        let tex_start = off;
        while off < data.len() && data[off] != 0 {
            off += 1;
        }
        let texture_raw = std::str::from_utf8(&data[tex_start..off]).unwrap_or("").trim().to_string();
        let texture_name = if texture_raw.is_empty() { None } else { Some(texture_raw) };
        // Skip null terminator + all consecutive null padding
        while off < data.len() && data[off] == 0 {
            off += 1;
        }

        info!("  Material: name='{}' pkts={} prims={} texture={:?}", name, pkt_count, prim_count, texture_name);

        let mut passes = Vec::new();
        let shader_name = format!("{}.shader", name);
        if let Ok(sha_content) = crate::vfs::read_to_string(entity_dir, &shader_name) {
            passes = parse_shader(&sha_content);
        }

        materials.push(Oni2Material {
            name,
            diffuse: [dr, dg, db],
            texture_name,
            primitive_count: prim_count,
            packet_count: pkt_count,
            passes,
        });
    }

    // --- Find mtxv marker (end of packet region) ---
    let search_start = data.len().saturating_sub(500);
    let mtxv_pos = data[search_start..].windows(4)
        .position(|w| w == b"mtxv")
        .map(|p| search_start + p)
        .unwrap_or(data.len());

    // --- Parse packet region ---
    // Each packet: [u32 adj_count, u32 strip_count, u32 mtx_count, u32 reskin_count]
    // Then: adj_count adjuncts (6×u32), reskin_count reskins (6×u32),
    //       strip_count strips (u32 type + u32 count + count×u32 indices),
    //       mtx_count bone map entries (u32 each)
    let total_pkts: u32 = materials.iter().map(|m| m.packet_count).sum();
    let mut packets = Vec::with_capacity(total_pkts as usize);
    let mut cur_mat = 0usize;
    let mut mat_pkt_counter = 0u32;

    info!("  Packet region: offset {} .. {} ({} bytes, {} packets)",
        off, mtxv_pos, mtxv_pos.saturating_sub(off), total_pkts);

    for _pkt_idx in 0..total_pkts as usize {
        // Determine current material
        while cur_mat < materials.len() && mat_pkt_counter >= materials[cur_mat].packet_count {
            cur_mat += 1;
            mat_pkt_counter = 0;
        }
        mat_pkt_counter += 1;

        if off + 16 > mtxv_pos {
            warn!("Packet header truncated at offset {}", off);
            break;
        }

        let adj_count = read_u32_le(data, off) as usize;    off += 4;
        let strip_count = read_u32_le(data, off) as usize;   off += 4;
        let mtx_count = read_u32_le(data, off) as usize;     off += 4;
        let mw_count = read_u32_le(data, off) as usize;      off += 4;

        // Adjuncts: 6 × u32 each
        let mut adjuncts = Vec::with_capacity(adj_count);
        for _ in 0..adj_count {
            if off + 24 > mtxv_pos { break; }
            let v = read_u32_le(data, off);
            let n = read_u32_le(data, off + 4);
            let c = read_u32_le(data, off + 8);
            let t1 = read_u32_le(data, off + 12) as i32;
            let _t2 = read_u32_le(data, off + 16);
            let bone = read_u32_le(data, off + 20);
            adjuncts.push(Oni2Adjunct {
                vertex_idx: v,
                normal_idx: n,
                color_idx: c,
                tex1_idx: t1,
                bone_idx: bone,
            });
            off += 24;
        }

        // Reskin/multiweight entries: 6 × 4 bytes each (skip for now)
        off += mw_count * 24;

        // Strips: each = u32 type + u32 count + count × u32 indices
        // type 1 = str (normal winding), type 2 = stp (swapped parity)
        let mut strips = Vec::with_capacity(strip_count);
        let mut strip_types = Vec::with_capacity(strip_count);
        for _ in 0..strip_count {
            if off + 8 > mtxv_pos { break; }
            let stype = read_u32_le(data, off);  off += 4;
            let scount = read_u32_le(data, off) as usize;  off += 4;
            if off + scount * 4 > mtxv_pos { break; }
            let indices: Vec<u32> = (0..scount)
                .map(|j| read_u32_le(data, off + j * 4))
                .collect();
            off += scount * 4;
            strips.push(indices);
            strip_types.push(stype);
        }

        // Bone map: mtx_count × u32
        let mut bone_map = Vec::with_capacity(mtx_count);
        if off + mtx_count * 4 <= mtxv_pos {
            for i in 0..mtx_count {
                bone_map.push(read_u32_le(data, off + i * 4));
            }
            off += mtx_count * 4;
        }

        packets.push(Oni2Packet {
            adjuncts,
            strips,
            strip_types,
            material_index: cur_mat.min(materials.len().saturating_sub(1)),
            bone_map,
        });
    }

    Some(Oni2Model {
        vertices,
        normals,
        colors,
        tex_coords,
        materials,
        packets,
        bone_world_positions: Vec::new(),
        bone_rotations: Vec::new(),
        world_space_verts: true, // binary v2.10 vertices are in world space when loaded from win32
    })
}

/// Read a null-terminated string from a fixed-size byte slice.
fn read_null_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

// === Block-structured packet region parser (binary v2.10) ===

/// A single 24-byte packet entry (6 × u32).
#[derive(Debug, Clone)]
struct PacketEntry {
    f0: u32,
    f1: u32,
    f2: u32,
    f3: u32,
    f4: u32,
    f5: u32,
}

/// A parsed bone-map group within a block.
#[derive(Debug, Clone)]
struct BoneMapGroup {
    group_type: u32,
    bone_indices: Vec<u32>,
}

/// A single parsed block from the packet region.
#[derive(Debug)]
struct ParsedBlock {
    offset: usize,
    header: [u32; 4], // A, B, C, D
    table1: Vec<PacketEntry>,       // primary packet entries
    bounds: Vec<[f32; 6]>,          // bounding data (6 floats each)
    bone_groups: Vec<BoneMapGroup>, // B groups
    table2: Vec<PacketEntry>,       // secondary packet entries
    sub_header: Vec<u32>,           // sub-header between bone groups and table2
    bone_bounds: Vec<[f32; 6]>,     // tail[1] × 24-byte bone influence bounding volumes (u32 bone_id, u32 sub_id, f32×4 as raw)
    end_offset: usize,
}

/// Read a 24-byte packet entry (6 × u32) from data at the given offset.
fn read_packet_entry(data: &[u8], off: usize) -> PacketEntry {
    PacketEntry {
        f0: read_u32_le(data, off),
        f1: read_u32_le(data, off + 4),
        f2: read_u32_le(data, off + 8),
        f3: read_u32_le(data, off + 12),
        f4: read_u32_le(data, off + 16),
        f5: read_u32_le(data, off + 20),
    }
}

/// Check if 6 u32s at the given offset look like a packet entry (f2=0, f4=0, values plausible).
fn looks_like_packet_entry(data: &[u8], off: usize) -> bool {
    if off + 24 > data.len() { return false; }
    let f0 = read_u32_le(data, off);
    let f1 = read_u32_le(data, off + 4);
    let f2 = read_u32_le(data, off + 8);
    let f4 = read_u32_le(data, off + 16);
    // Packet entries have f2=0, f4=0, and f0/f1 in reasonable range
    f2 == 0 && f4 == 0 && f0 < 10000 && f1 < 10000
}

/// Read a bone-map group: u32 type, u32 count, count × u32 bone indices.
/// Returns (group, bytes_consumed) or None.
fn read_bone_group(data: &[u8], off: usize, max_bone: u32) -> Option<(BoneMapGroup, usize)> {
    if off + 8 > data.len() { return None; }
    let group_type = read_u32_le(data, off);
    let count = read_u32_le(data, off + 4) as usize;

    // Sanity: type should be small, count should be reasonable
    if group_type > 16 || count == 0 || count > 64 { return None; }
    if off + 8 + count * 4 > data.len() { return None; }

    let mut indices = Vec::with_capacity(count);
    for i in 0..count {
        let idx = read_u32_le(data, off + 8 + i * 4);
        if idx > max_bone { return None; }
        indices.push(idx);
    }

    Some((BoneMapGroup { group_type, bone_indices: indices }, 8 + count * 4))
}

/// Check if a 4-u32 header looks like a plausible block header.
fn is_plausible_header(a: u32, b: u32, c: u32, d: u32) -> bool {
    a > 0 && a < 200
        && b < 100
        && c < 100
        && d < 200
}

/// Walk the packet region starting at `start_off` and parse blocks.
/// Each block contains adjunct records where:
///   f0=vertex_idx, f1=normal_idx, f2=color_idx(=0), f3=tex1_idx, f4=tex2_idx(=0), f5=bone_local_idx
/// Layout per block:
///   header: A, B, C, D (4 × u32)
///   table1: A × 24 bytes (adjunct records)
///   bounds: D × 24 bytes (6 floats each)
///   bone_groups: B groups (u32 type, u32 count, count × u32 bone_indices)
///   sub-header + table2 + trailing structures
fn parse_blocks(data: &[u8], start_off: usize, end_off: usize, n_adjuncts: usize, n_primitives: usize) -> Vec<ParsedBlock> {
    // Use macro that outputs to both tracing and stderr (for test visibility)
    macro_rules! blk_log {
        ($($arg:tt)*) => {
            info!($($arg)*);
            eprintln!($($arg)*);
        }
    }
    blk_log!("=== BLOCK REGION WALK (v2) ===");
    blk_log!("  region: {} .. {} ({} bytes)", start_off, end_off, end_off - start_off);
    blk_log!("  header counts: adjuncts={}, primitives={}", n_adjuncts, n_primitives);

    let mut off = start_off;
    let mut block_idx = 0;
    let mut all_blocks: Vec<ParsedBlock> = Vec::new();

    while off + 16 <= end_off {
        let a = read_u32_le(data, off);
        let b = read_u32_le(data, off + 4);
        let c = read_u32_le(data, off + 8);
        let d = read_u32_le(data, off + 12);

        if !is_plausible_header(a, b, c, d) {
            // Try scanning forward up to 256 bytes for next block header
            let scan_limit = (off + 256).min(end_off);
            let mut found_next = false;
            let mut soff = off + 4;
            while soff + 16 <= scan_limit {
                let sa = read_u32_le(data, soff);
                let sb = read_u32_le(data, soff + 4);
                let sc = read_u32_le(data, soff + 8);
                let sd = read_u32_le(data, soff + 12);
                if is_plausible_header(sa, sb, sc, sd) {
                    // Verify: first table1 entry should have f2=0, f4=0
                    if soff + 16 + 24 <= end_off {
                        let peek_f2 = read_u32_le(data, soff + 16 + 8);
                        let peek_f4 = read_u32_le(data, soff + 16 + 16);
                        if peek_f2 == 0 && peek_f4 == 0 {
                            blk_log!("  [{}] skipped {} bytes gap at offset {} to find next header at {}",
                                block_idx, soff - off, off, soff);
                            off = soff;
                            found_next = true;
                            break;
                        }
                    }
                }
                soff += 4;
            }
            if !found_next {
                blk_log!("  [{}] offset {} — no valid header within 256 bytes, stopping block walk",
                    block_idx, off);
                // Dump stop-point data
                let dump_end = (off + 128).min(data.len());
                let mut vals = Vec::new();
                let mut poff = off;
                while poff + 4 <= dump_end {
                    vals.push(read_u32_le(data, poff));
                    poff += 4;
                }
                blk_log!("  stop data u32: {:?}", &vals[..vals.len().min(32)]);
                break;
            }
            continue; // Re-enter loop at new offset
        }

        // Quick validation: first table1 entry must have f2=0, f4=0
        if a > 0 && off + 16 + 24 <= end_off {
            let peek_f2 = read_u32_le(data, off + 16 + 8);
            let peek_f4 = read_u32_le(data, off + 16 + 16);
            if peek_f2 != 0 || peek_f4 != 0 {
                off += 4;
                continue; // Silently skip — first table1 entry doesn't match packet format
            }
        }

        blk_log!("  [BLOCK {}] offset {} — header: (A={}, B={}, C={}, D={})", block_idx, off, a, b, c, d);

        let header_end = off + 16;
        let table1_size = a as usize * 24;
        let bounds_size = d as usize * 24;

        if header_end + table1_size + bounds_size > end_off {
            blk_log!("    table1+bounds would exceed region end, stopping");
            break;
        }

        // === Read table1: A × 24 bytes ===
        let mut table1 = Vec::with_capacity(a as usize);
        let mut toff = header_end;
        for _ in 0..a {
            table1.push(read_packet_entry(data, toff));
            toff += 24;
        }

        // === Read bounds: D × 24 bytes (as 6 floats each) ===
        let mut bounds = Vec::with_capacity(d as usize);
        for _ in 0..d {
            if toff + 24 > data.len() { break; }
            bounds.push([
                read_f32_le(data, toff),
                read_f32_le(data, toff + 4),
                read_f32_le(data, toff + 8),
                read_f32_le(data, toff + 12),
                read_f32_le(data, toff + 16),
                read_f32_le(data, toff + 20),
            ]);
            toff += 24;
        }

        // === Read B bone-map groups: u32 type, u32 count, count × u32 ===
        let mut bone_groups = Vec::new();
        let mut goff = toff;
        let mut groups_ok = true;
        let max_bone_idx = 255u32; // generous upper bound
        for gi in 0..b {
            match read_bone_group(data, goff, max_bone_idx) {
                Some((group, consumed)) => {
                    bone_groups.push(group);
                    goff += consumed;
                }
                None => {
                    blk_log!("    bone group {}/{} at offset {} failed to parse", gi, b, goff);
                    let dump_end = (goff + 48).min(data.len());
                    let hex: Vec<String> = data[goff..dump_end].iter().map(|b| format!("{:02X}", b)).collect();
                    blk_log!("    hex: {}", hex.join(" "));
                    groups_ok = false;
                    break;
                }
            }
        }

        // === After bone groups: parse sub-header, table2, table2 bone groups ===
        // Sub-header format (hypothesis):
        //   C u32 values (purpose TBD — C from block header)
        //   u32 table2_count
        //   u32 table2_bone_group_count
        //   u32 unknown (maybe reskin or pass index)
        //   u32 zero/terminator
        let mut sub_header = Vec::new();
        let mut table2 = Vec::new();
        let mut table2_bone_groups: Vec<BoneMapGroup> = Vec::new();
        let mut bone_bounds: Vec<[f32; 6]> = Vec::new();
        let mut scan_off = goff;
        let mut t2_count: usize = 0;
        let mut t2_group_count: usize = 0;
        let mut trailing_bone_groups: Vec<BoneMapGroup> = Vec::new();
        let mut tail0: usize = 0;

        if groups_ok {
            // Read sub-header: C values + 4 more u32s
            let sub_header_len = c as usize + 4; // C values + (table2_count, t2_groups, unknown, terminator)
            for _ in 0..sub_header_len {
                if scan_off + 4 > end_off { break; }
                sub_header.push(read_u32_le(data, scan_off));
                scan_off += 4;
            }

            // Extract table2 count and bone group count from sub-header
            if sub_header.len() >= c as usize + 2 {
                t2_count = sub_header[c as usize] as usize;
                t2_group_count = sub_header[c as usize + 1] as usize;
            }

            // Read table2 entries using the discovered count
            if t2_count > 0 && t2_count < 500 {
                for _ in 0..t2_count {
                    if scan_off + 24 > end_off { break; }
                    table2.push(read_packet_entry(data, scan_off));
                    scan_off += 24;
                }
            } else {
                // Fallback: scan for packet entries
                while scan_off + 24 <= end_off {
                    if !looks_like_packet_entry(data, scan_off) { break; }
                    table2.push(read_packet_entry(data, scan_off));
                    scan_off += 24;
                }
            }

            // Read table2 bone groups using the discovered count
            let t2_max_idx = 512u32; // generous limit for adjunct/vertex indices
            for _ in 0..t2_group_count {
                match read_bone_group(data, scan_off, t2_max_idx) {
                    Some((group, consumed)) => {
                        table2_bone_groups.push(group);
                        scan_off += consumed;
                    }
                    None => break,
                }
            }

            // Read bone influence bounding volumes: tail[1] × 24 bytes
            // Format per record: u32 bone_id, u32 sub_id, f32×4
            let bone_bounds_count = if sub_header.len() >= c as usize + 4 {
                sub_header[c as usize + 3] as usize
            } else {
                0
            };
            if bone_bounds_count > 0 && bone_bounds_count < 500 {
                for _ in 0..bone_bounds_count {
                    if scan_off + 24 > end_off { break; }
                    bone_bounds.push([
                        f32::from_bits(read_u32_le(data, scan_off)),      // bone_id as f32 bits
                        f32::from_bits(read_u32_le(data, scan_off + 4)),  // sub_id as f32 bits
                        read_f32_le(data, scan_off + 8),
                        read_f32_le(data, scan_off + 12),
                        read_f32_le(data, scan_off + 16),
                        read_f32_le(data, scan_off + 20),
                    ]);
                    scan_off += 24;
                }
            }

            // Read trailing bone groups after bone bounds
            // Hypothesis: tail[0] is the count of these groups
            tail0 = if sub_header.len() >= c as usize + 3 {
                sub_header[c as usize + 2] as usize
            } else {
                0
            };
            // Read trailing bone groups: scan until we hit a valid-looking next block header
            let trailing_max_bone = 512u32;
            while trailing_bone_groups.len() < 200 {
                // Before reading another group, check if current position is a valid block header
                if scan_off + 16 + 24 <= end_off {
                    let ha = read_u32_le(data, scan_off);
                    let hb = read_u32_le(data, scan_off + 4);
                    let hc = read_u32_le(data, scan_off + 8);
                    let hd = read_u32_le(data, scan_off + 12);
                    if is_plausible_header(ha, hb, hc, hd) {
                        // Check if the first table1 entry has f2=0 and f4=0
                        let t1_off = scan_off + 16;
                        let peek_f2 = read_u32_le(data, t1_off + 8);
                        let peek_f4 = read_u32_le(data, t1_off + 16);
                        if peek_f2 == 0 && peek_f4 == 0 {
                            break; // This looks like the next block, stop consuming groups
                        }
                    }
                }
                match read_bone_group(data, scan_off, trailing_max_bone) {
                    Some((group, consumed)) => {
                        trailing_bone_groups.push(group);
                        scan_off += consumed;
                    }
                    None => break,
                }
            }

            // After trailing bone groups, skip forward to find next block header.
            // tail[0] indicates extra u32s that may exist here, but the greedy scanner
            // may have already consumed some of them as bone groups. Just scan forward.
            // Skip any zero padding first.
            while scan_off + 4 <= end_off {
                let val = read_u32_le(data, scan_off);
                if val != 0 { break; }
                scan_off += 4;
            }
        }

        let block_end = scan_off;

        // === Diagnostic output ===

        // Table1 (summary only — individual entries suppressed for brevity)
        if !table1.is_empty() {
            let max_f0 = table1.iter().map(|e| e.f0).max().unwrap();
            let max_f1 = table1.iter().map(|e| e.f1).max().unwrap();
            let max_f3 = table1.iter().map(|e| e.f3).max().unwrap();
            let max_f5 = table1.iter().map(|e| e.f5).max().unwrap();
            let sum_d: i64 = table1.iter().map(|e| e.f1 as i64 - e.f0 as i64).sum();
            blk_log!("    TABLE1 summary: max(f0)={} max(f1)={} max(f3)={} max(f5)={} sum(Δ01)={}",
                max_f0, max_f1, max_f3, max_f5, sum_d);
            blk_log!("      f2_all_zero={} f4_all_zero={}",
                table1.iter().all(|e| e.f2 == 0), table1.iter().all(|e| e.f4 == 0));

            let mut f5_counts: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for e in &table1 { *f5_counts.entry(e.f5).or_insert(0) += 1; }
            let mut f5_dist: Vec<_> = f5_counts.into_iter().collect();
            f5_dist.sort();
            blk_log!("      f5 distribution: {:?}", f5_dist);
        }

        blk_log!("    BOUNDS: {} entries", bounds.len());

        // Bone groups
        blk_log!("    BONE GROUPS ({}/{} parsed, {} total bones):", bone_groups.len(), b,
            bone_groups.iter().map(|g| g.bone_indices.len()).sum::<usize>());
        for (i, g) in bone_groups.iter().enumerate() {
            blk_log!("      [{}] type={} count={} bones={:?}", i, g.group_type, g.bone_indices.len(), g.bone_indices);
        }

        // Sub-header
        if !sub_header.is_empty() {
            let c_values = &sub_header[..sub_header.len().min(c as usize)];
            blk_log!("    SUB-HEADER: C_data={:?} table2_count={} t2_groups={} tail={:?}",
                c_values, t2_count, t2_group_count,
                &sub_header[sub_header.len().min(c as usize + 2)..]);
        }

        // Table2 (summary only)
        if !table2.is_empty() {
            let max_f0 = table2.iter().map(|e| e.f0).max().unwrap();
            let max_f1 = table2.iter().map(|e| e.f1).max().unwrap();
            let max_f3 = table2.iter().map(|e| e.f3).max().unwrap();
            let max_f5 = table2.iter().map(|e| e.f5).max().unwrap();
            let sum_d: i64 = table2.iter().map(|e| e.f1 as i64 - e.f0 as i64).sum();
            blk_log!("    TABLE2 summary: max(f0)={} max(f1)={} max(f3)={} max(f5)={} sum(Δ01)={}",
                max_f0, max_f1, max_f3, max_f5, sum_d);
            blk_log!("      f2_all_zero={} f4_all_zero={}",
                table2.iter().all(|e| e.f2 == 0), table2.iter().all(|e| e.f4 == 0));
        }

        // Table2 bone groups
        if !table2_bone_groups.is_empty() {
            blk_log!("    TABLE2 BONE GROUPS ({} groups, {} total indices):",
                table2_bone_groups.len(),
                table2_bone_groups.iter().map(|g| g.bone_indices.len()).sum::<usize>());
            for (i, g) in table2_bone_groups.iter().enumerate() {
                blk_log!("      [{}] type={} count={} indices={:?}", i, g.group_type, g.bone_indices.len(), g.bone_indices);
            }
        }

        // Bone influence bounding volumes
        if !bone_bounds.is_empty() {
            blk_log!("    BONE BOUNDS ({} records, {} bytes):", bone_bounds.len(), bone_bounds.len() * 24);
            for (i, bb) in bone_bounds.iter().enumerate().take(8) {
                // Reinterpret first two as u32 (bone_id, sub_id)
                let bone_id = bb[0].to_bits();
                let sub_id = bb[1].to_bits();
                blk_log!("      [{}] bone={} sub={} vals=({:.3}, {:.3}, {:.3}, {:.3})",
                    i, bone_id, sub_id, bb[2], bb[3], bb[4], bb[5]);
            }
            if bone_bounds.len() > 8 {
                blk_log!("      ... ({} more)", bone_bounds.len() - 8);
            }
        }

        // Trailing bone groups
        if !trailing_bone_groups.is_empty() {
            blk_log!("    TRAILING BONE GROUPS ({} groups, tail[0]={}, {} total indices):",
                trailing_bone_groups.len(), tail0,
                trailing_bone_groups.iter().map(|g| g.bone_indices.len()).sum::<usize>());
            for (i, g) in trailing_bone_groups.iter().enumerate().take(8) {
                blk_log!("      [{}] type={} count={} indices={:?}", i, g.group_type, g.bone_indices.len(), g.bone_indices);
            }
            if trailing_bone_groups.len() > 8 {
                blk_log!("      ... ({} more)", trailing_bone_groups.len() - 8);
            }
        } else if tail0 > 0 {
            blk_log!("    TRAILING BONE GROUPS: expected {} (tail[0]) but parsed 0", tail0);
        }

        blk_log!("    block spans: {} .. {} ({} bytes)", off, block_end, block_end - off);

        // Post-parse validation: reject if bone groups failed or table1 looks wrong
        let t1_clean = table1.is_empty() || table1.iter().all(|e| e.f2 == 0 && e.f4 == 0);
        let valid_block = groups_ok && t1_clean;

        if !valid_block {
            // Quiet reject — don't spam output for every 4-byte offset attempt
            off += 4;
            continue;
        }

        // Dump post-block data (first 256 bytes after block) for gap analysis
        let peek_end = (block_end + 256).min(end_off);
        if block_end < peek_end {
            blk_log!("    POST-BLOCK peek ({} bytes at offset {}):", peek_end - block_end, block_end);
            // Show as u32s
            let mut poff = block_end;
            let mut u32_vals = Vec::new();
            while poff + 4 <= peek_end && u32_vals.len() < 64 {
                u32_vals.push(read_u32_le(data, poff));
                poff += 4;
            }
            // First 32 as u32s
            blk_log!("      as u32[0..32]: {:?}", &u32_vals[..u32_vals.len().min(32)]);
            if u32_vals.len() > 32 {
                blk_log!("      as u32[32..]: {:?}", &u32_vals[32..]);
            }
            // Also show as u16s for the first 64 bytes (index data hunt)
            let u16_end = (block_end + 64).min(end_off);
            let mut u16_vals = Vec::new();
            poff = block_end;
            while poff + 2 <= u16_end {
                u16_vals.push(read_u16_le(data, poff));
                poff += 2;
            }
            blk_log!("      as u16[0..32]: {:?}", &u16_vals[..u16_vals.len().min(32)]);
        }

        all_blocks.push(ParsedBlock {
            offset: off,
            header: [a, b, c, d],
            table1,
            bounds,
            bone_groups,
            table2,
            sub_header,
            bone_bounds,
            end_offset: block_end,
        });

        off = block_end;
        block_idx += 1;

        if block_idx > 50 {
            blk_log!("  hit block limit (50), stopping");
            break;
        }
    }

    // === Cross-block analysis ===
    blk_log!("=== CROSS-BLOCK ANALYSIS ({} blocks) ===", all_blocks.len());

    let mut g_t1_sum: i64 = 0;
    let mut g_t2_sum: i64 = 0;
    let mut g_t1_max_f1: u32 = 0;
    let mut g_t2_max_f1: u32 = 0;
    let mut g_t1_max_f3: u32 = 0;
    let mut g_t2_max_f3: u32 = 0;
    let mut g_t1_count: usize = 0;
    let mut g_t2_count: usize = 0;

    for blk in &all_blocks {
        g_t1_count += blk.table1.len();
        g_t2_count += blk.table2.len();
        for e in &blk.table1 {
            g_t1_sum += e.f1 as i64 - e.f0 as i64;
            g_t1_max_f1 = g_t1_max_f1.max(e.f1);
            g_t1_max_f3 = g_t1_max_f3.max(e.f3);
        }
        for e in &blk.table2 {
            g_t2_sum += e.f1 as i64 - e.f0 as i64;
            g_t2_max_f1 = g_t2_max_f1.max(e.f1);
            g_t2_max_f3 = g_t2_max_f3.max(e.f3);
        }
    }

    blk_log!("  TABLE1 total: {} entries, sum(Δ01)={}, max(f1)={}, max(f3)={}",
        g_t1_count, g_t1_sum, g_t1_max_f1, g_t1_max_f3);
    blk_log!("  TABLE2 total: {} entries, sum(Δ01)={}, max(f1)={}, max(f3)={}",
        g_t2_count, g_t2_sum, g_t2_max_f1, g_t2_max_f3);
    blk_log!("  Expected: adjuncts={}, primitives={}", n_adjuncts, n_primitives);
    blk_log!("  Match: t1_sum==adj? {}  t1_sum==prim? {}  t2_sum==adj? {}  t2_sum==prim? {}",
        g_t1_sum == n_adjuncts as i64, g_t1_sum == n_primitives as i64,
        g_t2_sum == n_adjuncts as i64, g_t2_sum == n_primitives as i64);
    blk_log!("  Match: t1_max_f1==adj? {}  t1_max_f1==prim? {}  t2_max_f1==adj? {}  t2_max_f1==prim? {}",
        g_t1_max_f1 == n_adjuncts as u32, g_t1_max_f1 == n_primitives as u32,
        g_t2_max_f1 == n_adjuncts as u32, g_t2_max_f1 == n_primitives as u32);

    // Remaining bytes = payload region
    // Use last block's end_offset as payload start (block walker may have overshot)
    let payload_off = if let Some(last_blk) = all_blocks.last() {
        last_blk.end_offset
    } else {
        off
    };
    let remaining = end_off.saturating_sub(payload_off);
    blk_log!("  remaining: {} bytes at offset {} ({:.1}% of region)",
        remaining, payload_off, (remaining as f64 / (end_off - start_off) as f64) * 100.0);
    let off = payload_off; // shadow off for payload analysis

    // === Payload region analysis: alignment test + raw dump ===
    if remaining >= 24 {
        blk_log!("=== PAYLOAD REGION ANALYSIS ===");
        blk_log!("  payload: offset {} .. {} ({} bytes, mod24={})",
            off, end_off, remaining, remaining % 24);

        // Test two alignments: records starting at off (no count prefix) vs off+4 (with count prefix)
        for &(label, base) in &[("align0 (no prefix)", off), ("align4 (skip first u32)", off + 4)] {
            let avail = end_off.saturating_sub(base);
            let n_recs = avail / 24;
            let sample = n_recs.min(200);
            if sample < 4 { continue; }

            let mut zeros = [0usize; 6];
            let mut maxes = [0u32; 6];
            let mut all_small = [0usize; 6]; // count of values < 500
            for i in 0..sample {
                let roff = base + i * 24;
                for j in 0..6 {
                    let v = read_u32_le(data, roff + j * 4);
                    if v == 0 { zeros[j] += 1; }
                    maxes[j] = maxes[j].max(v);
                    if v < 500 { all_small[j] += 1; }
                }
            }
            let zero_pct: Vec<String> = (0..6).map(|j| format!("f{}:{:.0}%", j, zeros[j] as f64 / sample as f64 * 100.0)).collect();
            let small_pct: Vec<String> = (0..6).map(|j| format!("f{}:{:.0}%", j, all_small[j] as f64 / sample as f64 * 100.0)).collect();
            let total_zeros: usize = zeros.iter().sum();
            let total_small: usize = all_small.iter().sum();
            blk_log!("  {} — {} recs, zero_score={}, small_score={}", label, n_recs, total_zeros, total_small);
            blk_log!("    zero%: {}", zero_pct.join(" "));
            blk_log!("    <500%: {}", small_pct.join(" "));
        }

        // === Raw record dump at both alignments (first 15 records) ===
        blk_log!("  --- RAW RECORDS (no prefix, starting at {}) ---", off);
        for i in 0..15.min((end_off - off) / 24) {
            let roff = off + i * 24;
            let r: Vec<u32> = (0..6).map(|j| read_u32_le(data, roff + j * 4)).collect();
            // Also show f0/f2 as float if they look like floats (> 10000)
            let f0f = read_f32_le(data, roff);
            let f2f = read_f32_le(data, roff + 8);
            let float_note = if r[0] > 1000 || r[2] > 1000 {
                format!("  (f0={:.3} f2={:.3})", f0f, f2f)
            } else { String::new() };
            blk_log!("    [{:3}] {:6} {:6} {:6} {:6} {:6} {:6}{}",
                i, r[0], r[1], r[2], r[3], r[4], r[5], float_note);
        }

        blk_log!("  --- RAW RECORDS (with u32 prefix={}, starting at {}) ---",
            read_u32_le(data, off), off + 4);
        let base2 = off + 4;
        for i in 0..15.min((end_off - base2) / 24) {
            let roff = base2 + i * 24;
            let r: Vec<u32> = (0..6).map(|j| read_u32_le(data, roff + j * 4)).collect();
            let f0f = read_f32_le(data, roff);
            let f2f = read_f32_le(data, roff + 8);
            let float_note = if r[0] > 1000 || r[2] > 1000 {
                format!("  (f0={:.3} f2={:.3})", f0f, f2f)
            } else { String::new() };
            blk_log!("    [{:3}] {:6} {:6} {:6} {:6} {:6} {:6}{}",
                i, r[0], r[1], r[2], r[3], r[4], r[5], float_note);
        }

        // === Sliding window zero-pattern analysis ===
        let total_recs = (end_off - off) / 24;
        let window = 20;
        blk_log!("  --- ZERO PATTERN WINDOWS (align0, {} recs of 24 bytes) ---", total_recs);
        let mut prev_pattern = String::new();
        for start in (0..total_recs).step_by(window) {
            let end = (start + window).min(total_recs);
            if end - start < 5 { break; }
            let mut zeros = [0usize; 6];
            let cnt = end - start;
            for i in start..end {
                let roff = off + i * 24;
                for j in 0..6 {
                    if read_u32_le(data, roff + j * 4) == 0 { zeros[j] += 1; }
                }
            }
            let pattern: String = (0..6).map(|j| {
                if zeros[j] == cnt { '0' } else if zeros[j] == 0 { 'X' } else { '.' }
            }).collect();
            if pattern != prev_pattern {
                blk_log!("    recs {:4}..{:4}: [{}]  zeros={:?}", start, end, pattern, zeros);
                prev_pattern = pattern;
            }
        }

        // === Records around n_adjuncts boundary (key hypothesis: pattern repeats at n_adjuncts) ===
        if n_adjuncts > 5 && n_adjuncts + 3 < total_recs {
            blk_log!("  --- RECORDS AROUND n_adjuncts={} (align0) ---", n_adjuncts);
            let show_start = n_adjuncts.saturating_sub(3);
            let show_end = (n_adjuncts + 5).min(total_recs);
            for i in show_start..show_end {
                let roff = off + i * 24;
                let r: Vec<u32> = (0..6).map(|j| read_u32_le(data, roff + j * 4)).collect();
                let marker = if i == n_adjuncts { " <== n_adjuncts" } else { "" };
                blk_log!("    [{:3}] {:6} {:6} {:6} {:6} {:6} {:6}{}",
                    i, r[0], r[1], r[2], r[3], r[4], r[5], marker);
            }
        }

        // === Records around n_primitives boundary ===
        if n_primitives > 5 && n_primitives + 3 < total_recs {
            blk_log!("  --- RECORDS AROUND n_primitives={} (align0) ---", n_primitives);
            let show_start = n_primitives.saturating_sub(3);
            let show_end = (n_primitives + 5).min(total_recs);
            for i in show_start..show_end {
                let roff = off + i * 24;
                let r: Vec<u32> = (0..6).map(|j| read_u32_le(data, roff + j * 4)).collect();
                let marker = if i == n_primitives { " <== n_primitives" } else { "" };
                blk_log!("    [{:3}] {:6} {:6} {:6} {:6} {:6} {:6}{}",
                    i, r[0], r[1], r[2], r[3], r[4], r[5], marker);
            }
        }

        // === Check n_adjuncts + n_primitives ===
        let combined = n_adjuncts + n_primitives;
        if combined > 5 && combined < total_recs {
            blk_log!("  --- RECORDS AROUND adj+prim={} (align0) ---", combined);
            let show_start = combined.saturating_sub(2);
            let show_end = (combined + 3).min(total_recs);
            for i in show_start..show_end {
                let roff = off + i * 24;
                let r: Vec<u32> = (0..6).map(|j| read_u32_le(data, roff + j * 4)).collect();
                let f_vals: Vec<f32> = (0..6).map(|j| read_f32_le(data, roff + j * 4)).collect();
                let marker = if i == combined { " <== adj+prim" } else { "" };
                blk_log!("    [{:3}] {:6} {:6} {:6} {:6} {:6} {:6}  f32=({:.3},{:.3},{:.3},{:.3},{:.3},{:.3}){}",
                    i, r[0], r[1], r[2], r[3], r[4], r[5],
                    f_vals[0], f_vals[1], f_vals[2], f_vals[3], f_vals[4], f_vals[5], marker);
            }
        }

        // === Split at n_adjuncts: adjunct section + post-adjunct section ===
        let adj_bytes = n_adjuncts * 24;
        let post_adj_off = off + adj_bytes;
        let post_adj_bytes = end_off.saturating_sub(post_adj_off);
        blk_log!("  total: {} bytes = {} adjunct recs × 24 ({} bytes) + {} post-adjunct bytes",
            remaining, n_adjuncts, adj_bytes, post_adj_bytes);

        // Adjunct section analysis
        if adj_bytes <= remaining {
            // Classify records: "index" (all fields < 5000) vs "mixed" (some field ≥ 5000)
            let mut n_pure_index = 0usize;
            let mut n_mixed = 0usize;
            let mut idx_maxes = [0u32; 6];
            let mut idx_zeros = [0usize; 6];
            let mut first_mixed: Option<usize> = None;

            for i in 0..n_adjuncts {
                let roff = off + i * 24;
                let vals: [u32; 6] = std::array::from_fn(|j| read_u32_le(data, roff + j * 4));
                let is_mixed = vals.iter().any(|&v| v > 5000);
                if is_mixed {
                    n_mixed += 1;
                    if first_mixed.is_none() { first_mixed = Some(i); }
                } else {
                    n_pure_index += 1;
                    for j in 0..6 {
                        idx_maxes[j] = idx_maxes[j].max(vals[j]);
                        if vals[j] == 0 { idx_zeros[j] += 1; }
                    }
                }
            }

            blk_log!("  ADJUNCT SECTION ({} records): {} pure index, {} mixed/float",
                n_adjuncts, n_pure_index, n_mixed);
            if n_pure_index > 0 {
                let range_str: Vec<String> = (0..6).map(|j| format!("f{}:0..{}", j, idx_maxes[j])).collect();
                let zero_str: Vec<String> = (0..6).map(|j| {
                    format!("f{}:{:.0}%", j, idx_zeros[j] as f64 / n_pure_index as f64 * 100.0)
                }).collect();
                blk_log!("    index records — max: {}", range_str.join("  "));
                blk_log!("    index records — zero%: {}", zero_str.join(" "));
            }
            if let Some(fm) = first_mixed {
                blk_log!("    first mixed record at index {}", fm);
            }

            // Show first 5, last 5, and records around first_mixed
            blk_log!("    first 5 adjunct records (align0):");
            for i in 0..5.min(n_adjuncts) {
                let roff = off + i * 24;
                let r: [u32; 6] = std::array::from_fn(|j| read_u32_le(data, roff + j * 4));
                blk_log!("      [{:4}] {:6} {:6} {:6} {:6} {:6} {:6}", i, r[0], r[1], r[2], r[3], r[4], r[5]);
            }
            if n_adjuncts > 10 {
                blk_log!("    last 5 adjunct records:");
                for i in (n_adjuncts - 5)..n_adjuncts {
                    let roff = off + i * 24;
                    let r: [u32; 6] = std::array::from_fn(|j| read_u32_le(data, roff + j * 4));
                    let is_mixed = r.iter().any(|&v| v > 5000);
                    let note = if is_mixed { "  [FLOAT]" } else { "" };
                    blk_log!("      [{:4}] {:6} {:6} {:6} {:6} {:6} {:6}{}", i, r[0], r[1], r[2], r[3], r[4], r[5], note);
                }
            }
            if let Some(fm) = first_mixed {
                if fm > 2 && fm + 3 < n_adjuncts {
                    blk_log!("    around first mixed (rec {}):", fm);
                    for i in (fm - 2)..(fm + 3).min(n_adjuncts) {
                        let roff = off + i * 24;
                        let r: [u32; 6] = std::array::from_fn(|j| read_u32_le(data, roff + j * 4));
                        let f: [f32; 6] = std::array::from_fn(|j| read_f32_le(data, roff + j * 4));
                        let is_mixed = r.iter().any(|&v| v > 5000);
                        if is_mixed {
                            blk_log!("      [{:4}] {:6} {:6} {:6} {:6} {:6} {:6}  f32=({:.3},{:.3},{:.3},{:.3},{:.3},{:.3})",
                                i, r[0], r[1], r[2], r[3], r[4], r[5], f[0], f[1], f[2], f[3], f[4], f[5]);
                        } else {
                            blk_log!("      [{:4}] {:6} {:6} {:6} {:6} {:6} {:6}", i, r[0], r[1], r[2], r[3], r[4], r[5]);
                        }
                    }
                }
            }
        }

        // Post-adjunct section analysis
        if post_adj_bytes > 0 && adj_bytes <= remaining {
            blk_log!("  POST-ADJUNCT SECTION ({} bytes at offset {}):", post_adj_bytes, post_adj_off);

            // Find where float data starts (first record with any field > 50000)
            let post_recs = post_adj_bytes / 24;
            let mut float_start: Option<usize> = None;
            let mut index_resume: Option<usize> = None;
            for i in 0..post_recs {
                let roff = post_adj_off + i * 24;
                let any_large = (0..6).any(|j| read_u32_le(data, roff + j * 4) > 50000);
                if float_start.is_none() && any_large {
                    float_start = Some(i);
                }
                if float_start.is_some() && index_resume.is_none() && !any_large {
                    // Check if this looks like small sequential indices
                    let v0 = read_u32_le(data, roff);
                    if v0 < 100 {
                        index_resume = Some(i);
                    }
                }
            }

            blk_log!("    {} 24-byte records, {} leftover bytes", post_recs, post_adj_bytes % 24);
            if let Some(fs) = float_start {
                blk_log!("    float data starts at post-adj record {} (abs offset {})",
                    fs, post_adj_off + fs * 24);
            }
            if let Some(ir) = index_resume {
                blk_log!("    index data resumes at post-adj record {} (abs offset {})",
                    ir, post_adj_off + ir * 24);
                // Dump first few index records
                let idx_off = post_adj_off + ir * 24;
                blk_log!("    index records (as flat u32 stream):");
                let mut vals = Vec::new();
                let mut ioff = idx_off;
                while ioff + 4 <= end_off && vals.len() < 48 {
                    vals.push(read_u32_le(data, ioff));
                    ioff += 4;
                }
                blk_log!("      {:?}", vals);

                // Total index bytes from resume point to end
                let idx_bytes = end_off - idx_off;
                let n_u32_indices = idx_bytes / 4;
                let n_u16_indices = idx_bytes / 2;
                blk_log!("      {} bytes = {} u32s or {} u16s", idx_bytes, n_u32_indices, n_u16_indices);
                blk_log!("      n_primitives={} × 3 = {}  match u32? {}  match u16? {}",
                    n_primitives, n_primitives * 3,
                    n_u32_indices == n_primitives * 3,
                    n_u16_indices == n_primitives * 3);
            }

            // Show first 5 and last 5 post-adjunct records
            let show = post_recs.min(5);
            blk_log!("    first {} post-adj records:", show);
            for i in 0..show {
                let roff = post_adj_off + i * 24;
                let r: Vec<u32> = (0..6).map(|j| read_u32_le(data, roff + j * 4)).collect();
                let f: Vec<f32> = (0..6).map(|j| read_f32_le(data, roff + j * 4)).collect();
                let any_large = r.iter().any(|&v| v > 50000);
                let note = if any_large {
                    format!("  f32=({:.3},{:.3},{:.3},{:.3},{:.3},{:.3})", f[0],f[1],f[2],f[3],f[4],f[5])
                } else { String::new() };
                blk_log!("      [{:3}] {:6} {:6} {:6} {:6} {:6} {:6}{}",
                    i, r[0], r[1], r[2], r[3], r[4], r[5], note);
            }
            if post_recs > 10 {
                let last_start = post_recs - 5;
                blk_log!("    last 5 post-adj records:");
                for i in last_start..post_recs {
                    let roff = post_adj_off + i * 24;
                    let r: Vec<u32> = (0..6).map(|j| read_u32_le(data, roff + j * 4)).collect();
                    blk_log!("      [{:3}] {:6} {:6} {:6} {:6} {:6} {:6}",
                        i, r[0], r[1], r[2], r[3], r[4], r[5]);
                }
            }
        }
    }

    blk_log!("=== END BLOCK WALK ===");
    all_blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oni2_loader::parsers::skeleton::parse_skel;
    use crate::oni2_loader::parsers::animation::parse_anim;
    use crate::oni2_loader::animation::load_anim_library;

    #[test]
    fn test_file_structure_analysis() {
        let path_str = format!("{}/Entity/Tim/win32_tim_LODs3.mod", crate::get_assets_path());
        let path = path_str.as_str();
        let data = crate::vfs::read("", path).expect("Failed to read tim LODs3.mod");
        eprintln!("=== TIM LOD3 FILE STRUCTURE ANALYSIS ===");
        eprintln!("File size: {} bytes", data.len());

        // =============================================
        // 1. HEADER (bytes 0..58)
        // =============================================
        let version_str = std::str::from_utf8(&data[..13]).unwrap_or("???");
        eprintln!("\n--- HEADER (bytes 0..57, 58 bytes) ---");
        eprintln!("  Version string (0..13): {:?}", version_str);
        eprintln!("  Byte 13: 0x{:02X} (null terminator)", data[13]);

        let n_verts = read_u32_le(&data, 14) as usize;
        let n_normals = read_u32_le(&data, 18) as usize;
        let n_colors = read_u32_le(&data, 22) as usize;
        let n_tex1s = read_u32_le(&data, 26) as usize;
        let n_tex2s = read_u32_le(&data, 30) as usize;
        let n_tangents = read_u32_le(&data, 34) as usize;
        let n_materials = read_u32_le(&data, 38) as usize;
        let n_adjuncts = read_u32_le(&data, 42) as usize;
        let n_primitives = read_u32_le(&data, 46) as usize;
        let n_matrices = read_u32_le(&data, 50) as usize;
        let n_reskins = read_u32_le(&data, 54) as usize;

        eprintln!("  n_verts      (14..18): {}", n_verts);
        eprintln!("  n_normals    (18..22): {}", n_normals);
        eprintln!("  n_colors     (22..26): {}", n_colors);
        eprintln!("  n_tex1s      (26..30): {}", n_tex1s);
        eprintln!("  n_tex2s      (30..34): {}", n_tex2s);
        eprintln!("  n_tangents   (34..38): {}", n_tangents);
        eprintln!("  n_materials  (38..42): {}", n_materials);
        eprintln!("  n_adjuncts   (42..46): {}", n_adjuncts);
        eprintln!("  n_primitives (46..50): {}", n_primitives);
        eprintln!("  n_matrices   (50..54): {}", n_matrices);
        eprintln!("  n_reskins    (54..58): {}", n_reskins);

        // =============================================
        // 2. VERTICES (offset 58, n_verts * 12 bytes)
        // =============================================
        let verts_off = 58;
        let verts_size = n_verts * 12;
        let verts_end = verts_off + verts_size;
        eprintln!("\n--- VERTICES (offset {}..{}, {} bytes, {} verts * 12) ---",
            verts_off, verts_end, verts_size, n_verts);

        // =============================================
        // 3. NORMALS (n_normals * 12 bytes)
        // =============================================
        let normals_off = verts_end;
        let normals_size = n_normals * 12;
        let normals_end = normals_off + normals_size;
        eprintln!("--- NORMALS (offset {}..{}, {} bytes, {} normals * 12) ---",
            normals_off, normals_end, normals_size, n_normals);

        // =============================================
        // 4. COLORS (n_colors * 16 bytes)
        // =============================================
        let colors_off = normals_end;
        let colors_size = n_colors * 16;
        let colors_end = colors_off + colors_size;
        eprintln!("--- COLORS (offset {}..{}, {} bytes, {} colors * 16) ---",
            colors_off, colors_end, colors_size, n_colors);

        // =============================================
        // 5. TEX COORDS (n_tex1s * 8 bytes)
        // =============================================
        let tex_off = colors_end;
        let tex_size = n_tex1s * 8;
        let tex_end = tex_off + tex_size;
        eprintln!("--- TEX COORDS (offset {}..{}, {} bytes, {} tex1s * 8) ---",
            tex_off, tex_end, tex_size, n_tex1s);

        // Skip tex2s and tangents
        let tex2_off = tex_end;
        let tex2_size = n_tex2s * 8;
        let tex2_end = tex2_off + tex2_size;
        let tang_off = tex2_end;
        let tang_size = n_tangents * 12;
        let tang_end = tang_off + tang_size;
        if n_tex2s > 0 {
            eprintln!("--- TEX2 (offset {}..{}, {} bytes) ---", tex2_off, tex2_end, tex2_size);
        }
        if n_tangents > 0 {
            eprintln!("--- TANGENTS (offset {}..{}, {} bytes) ---", tang_off, tang_end, tang_size);
        }

        // =============================================
        // 6. MATERIALS (n_materials * 86 bytes)
        // =============================================
        let mat_off = tang_end;
        let mat_size = n_materials * 86;
        let mat_end = mat_off + mat_size;
        eprintln!("--- MATERIALS (offset {}..{}, {} bytes, {} materials * 86) ---",
            mat_off, mat_end, mat_size, n_materials);
        for i in 0..n_materials {
            let mo = mat_off + i * 86;
            let name = read_null_string(&data[mo..mo + 12]);
            let tex_name = read_null_string(&data[mo + 74..mo + 86]);
            // Dump all u16 values at offsets 12..26 (7 u16s before the diffuse floats)
            let mut u16_vals = Vec::new();
            for off in (12..26).step_by(2) {
                u16_vals.push(format!("+{}={}", off, read_u16_le(&data, mo + off)));
            }
            // Also dump as u32 pairs at 12..20
            let u32_12 = read_u32_le(&data, mo + 12);
            let u32_16 = read_u32_le(&data, mo + 16);
            let u32_20 = read_u32_le(&data, mo + 20);
            eprintln!("  mat[{}]: name='{}' texture='{}' u16s=[{}] u32@12={} u32@16={} u32@20={}",
                i, name, tex_name, u16_vals.join(", "), u32_12, u32_16, u32_20);
        }

        // =============================================
        // 7. FIND mtxv marker
        // =============================================
        let search_start = data.len().saturating_sub(500);
        let mtxv_pos = data[search_start..].windows(4)
            .position(|w| w == b"mtxv")
            .map(|p| search_start + p);

        let mtxv_offset = mtxv_pos.expect("mtxv marker not found");
        eprintln!("\n--- mtxv MARKER found at offset {} ---", mtxv_offset);

        // =============================================
        // 7. EVERYTHING between materials end and mtxv
        // =============================================
        let gap_start = mat_end;
        let gap_end = mtxv_offset;
        let gap_size = gap_end - gap_start;
        eprintln!("\n====================================================");
        eprintln!("=== PACKET REGION: offset {}..{} ({} bytes) ===", gap_start, gap_end, gap_size);
        eprintln!("====================================================");

        eprintln!("\n  Context: n_adjuncts={}, n_primitives={}", n_adjuncts, n_primitives);
        eprintln!("  n_adjuncts * 24 = {} bytes (if adjuncts are 6*u32 records)", n_adjuncts * 24);
        eprintln!("  n_primitives * 2 = {} bytes (if primitives are u16 indices)", n_primitives * 2);
        eprintln!("  n_adjuncts*24 + n_primitives*2 = {} bytes",
            n_adjuncts * 24 + n_primitives * 2);
        eprintln!("  gap size = {} bytes", gap_size);
        eprintln!("  gap_size - n_adjuncts*24 = {} bytes (leftover for indices + headers)",
            gap_size as isize - (n_adjuncts * 24) as isize);

        // --- Dump the ENTIRE gap region as u16 values ---
        eprintln!("\n--- FULL GAP DUMP as u16 values ({} u16s) ---", gap_size / 2);
        let mut u16_vals: Vec<u16> = Vec::new();
        let mut off = gap_start;
        while off + 2 <= gap_end {
            u16_vals.push(read_u16_le(&data, off));
            off += 2;
        }

        // Print in rows of 16
        for (chunk_idx, chunk) in u16_vals.chunks(16).enumerate() {
            let byte_offset = gap_start + chunk_idx * 32;
            let vals_str: Vec<String> = chunk.iter().map(|v| format!("{:5}", v)).collect();
            eprintln!("  @{:5}: {}", byte_offset, vals_str.join(" "));
        }

        // --- Also show as u32 values for the first 256 bytes ---
        eprintln!("\n--- GAP first 256 bytes as u32 values ---");
        off = gap_start;
        let dump_end = (gap_start + 256).min(gap_end);
        let mut row = Vec::new();
        let mut row_start = off;
        while off + 4 <= dump_end {
            row.push(read_u32_le(&data, off));
            off += 4;
            if row.len() == 8 {
                let vals_str: Vec<String> = row.iter().map(|v| format!("{:8}", v)).collect();
                eprintln!("  @{:5}: {}", row_start, vals_str.join(" "));
                row.clear();
                row_start = off;
            }
        }
        if !row.is_empty() {
            let vals_str: Vec<String> = row.iter().map(|v| format!("{:8}", v)).collect();
            eprintln!("  @{:5}: {}", row_start, vals_str.join(" "));
        }

        // --- Analyze u16 values for potential triangle strip indices ---
        eprintln!("\n--- TRIANGLE STRIP INDEX ANALYSIS ---");

        // Find all u16 values in the gap that are in range 0..n_adjuncts
        let max_adj = n_adjuncts as u16;
        let mut in_range_count = 0usize;
        let mut out_of_range_count = 0usize;
        let mut in_range_positions: Vec<(usize, u16)> = Vec::new();

        for (i, &val) in u16_vals.iter().enumerate() {
            if val < max_adj {
                in_range_count += 1;
                in_range_positions.push((gap_start + i * 2, val));
            } else {
                out_of_range_count += 1;
            }
        }

        eprintln!("  u16 values < {} (n_adjuncts): {} / {} ({:.1}%)",
            n_adjuncts, in_range_count, u16_vals.len(),
            in_range_count as f64 / u16_vals.len() as f64 * 100.0);
        eprintln!("  u16 values >= {}: {} / {}", n_adjuncts, out_of_range_count, u16_vals.len());

        // Look for RUNS of consecutive u16 values all in range 0..n_adjuncts
        // (these would be candidate strip index arrays)
        eprintln!("\n--- RUNS of consecutive u16 values in range 0..{} ---", n_adjuncts);
        let mut run_start: Option<usize> = None;
        let mut runs: Vec<(usize, usize, Vec<u16>)> = Vec::new(); // (byte_offset, length, values)

        for (i, &val) in u16_vals.iter().enumerate() {
            if val < max_adj {
                if run_start.is_none() {
                    run_start = Some(i);
                }
            } else {
                if let Some(start) = run_start {
                    let len = i - start;
                    if len >= 3 { // At least 3 consecutive indices to form a triangle
                        let vals: Vec<u16> = u16_vals[start..i].to_vec();
                        runs.push((gap_start + start * 2, len, vals));
                    }
                    run_start = None;
                }
            }
        }
        // Close final run
        if let Some(start) = run_start {
            let len = u16_vals.len() - start;
            if len >= 3 {
                let vals: Vec<u16> = u16_vals[start..].to_vec();
                runs.push((gap_start + start * 2, len, vals));
            }
        }

        eprintln!("  Found {} runs of 3+ consecutive in-range u16 values:", runs.len());
        for (byte_off, len, vals) in &runs {
            let preview: Vec<String> = vals.iter().take(30).map(|v| format!("{}", v)).collect();
            let suffix = if vals.len() > 30 { format!(" ... ({} more)", vals.len() - 30) } else { String::new() };
            eprintln!("    @offset {}: {} u16 values: [{}]{}", byte_off, len, preview.join(", "), suffix);

            // Check if this run looks like a triangle strip
            // In a triangle strip, you'd see index values that repeat in patterns
            let unique: std::collections::HashSet<u16> = vals.iter().copied().collect();
            let max_val = vals.iter().max().copied().unwrap_or(0);
            let min_val = vals.iter().min().copied().unwrap_or(0);
            eprintln!("      range: {}..{}, {} unique values, density: {:.1}%",
                min_val, max_val, unique.len(),
                unique.len() as f64 / (max_val as f64 - min_val as f64 + 1.0) * 100.0);

            // Check for degenerate strip markers (repeated consecutive values)
            let mut degen_count = 0;
            for w in vals.windows(2) {
                if w[0] == w[1] { degen_count += 1; }
            }
            if degen_count > 0 {
                eprintln!("      degenerate pairs (repeated consecutive): {}", degen_count);
            }
        }

        // --- Hypothesis: the block header structure + adjunct table + index buffer ---
        // Try to find the index buffer by looking at the gap structure more carefully
        eprintln!("\n--- STRUCTURAL ANALYSIS OF GAP ---");

        // Read first 16 bytes as potential block header (A, B, C, D)
        if gap_size >= 16 {
            let a = read_u32_le(&data, gap_start);
            let b = read_u32_le(&data, gap_start + 4);
            let c = read_u32_le(&data, gap_start + 8);
            let d = read_u32_le(&data, gap_start + 12);
            eprintln!("  First 4 u32s (potential block header): A={} B={} C={} D={}", a, b, c, d);

            // If A looks like adjunct count, the adjunct table follows
            let table1_end = gap_start + 16 + a as usize * 24;
            eprintln!("  If A={} is table1 count: table1 ends at offset {}", a, table1_end);

            // After table1, D bounds records
            let bounds_end = table1_end + d as usize * 24;
            eprintln!("  If D={} is bounds count: bounds end at offset {}", d, bounds_end);

            // After bounds, B bone groups
            // Then sub-header, then... index data?
            eprintln!("  Remaining after table1+bounds: {} bytes",
                gap_end as isize - bounds_end as isize);
        }

        // --- Try interpreting data at various offsets as u16 index arrays ---
        eprintln!("\n--- SCANNING FOR u16 INDEX ARRAYS (values 0..{}) ---", n_adjuncts);

        // Scan every 2-byte aligned position and count how many consecutive
        // u16 values are in range 0..n_adjuncts
        let mut best_runs: Vec<(usize, usize)> = Vec::new(); // (offset, length)
        let mut i = 0;
        while i + 2 <= gap_size {
            let val = read_u16_le(&data, gap_start + i);
            if val < max_adj {
                let start = i;
                let mut j = i + 2;
                while j + 2 <= gap_size {
                    let v = read_u16_le(&data, gap_start + j);
                    if v >= max_adj { break; }
                    j += 2;
                }
                let count = (j - start) / 2;
                if count >= 5 {
                    best_runs.push((gap_start + start, count));
                }
                i = j;
            } else {
                i += 2;
            }
        }

        best_runs.sort_by(|a, b| b.1.cmp(&a.1));
        eprintln!("  Top runs of u16 values in range 0..{}:", n_adjuncts);
        for (off, count) in best_runs.iter().take(10) {
            let preview: Vec<u16> = (0..*count.min(&20)).map(|i| {
                read_u16_le(&data, off + i * 2)
            }).collect();
            let preview_str: Vec<String> = preview.iter().map(|v| format!("{}", v)).collect();
            let suffix = if *count > 20 { format!(" ... ({} more)", count - 20) } else { String::new() };
            eprintln!("    @offset {} ({} from gap start): {} u16 values: [{}]{}",
                off, off - gap_start, count, preview_str.join(", "), suffix);

            // Does this count relate to n_primitives?
            if *count == n_primitives || *count == n_primitives + 2 || *count == n_primitives - 1 {
                eprintln!("      *** MATCHES n_primitives ({})! ***", n_primitives);
            }
        }

        // --- Total u16 count across all runs ---
        let total_run_u16s: usize = best_runs.iter().map(|(_, c)| c).sum();
        eprintln!("\n  Total u16 values in runs of 5+: {}", total_run_u16s);
        eprintln!("  n_primitives = {} (if these are strip indices, expect close match)", n_primitives);

        // =============================================
        // 8. mtxv section to end
        // =============================================
        eprintln!("\n--- mtxv SECTION (offset {}..{}, {} bytes) ---",
            mtxv_offset, data.len(), data.len() - mtxv_offset);

        // Read mtxv entries
        let mut moff = mtxv_offset + 4; // skip "mtxv"
        let mut mtxv_entries: Vec<u32> = Vec::new();
        for _ in 0..n_matrices {
            if moff + 5 > data.len() { break; }
            if data[moff] != b' ' { break; }
            moff += 1;
            if moff + 4 > data.len() { break; }
            mtxv_entries.push(read_u32_le(&data, moff));
            moff += 4;
        }
        let sum: u32 = mtxv_entries.iter().sum();
        eprintln!("  {} entries (n_matrices={}), sum={} (n_verts={})",
            mtxv_entries.len(), n_matrices, sum, n_verts);
        for (i, &v) in mtxv_entries.iter().enumerate() {
            eprintln!("    bone[{}]: {} vertices", i, v);
        }

        // Remaining bytes after mtxv entries
        eprintln!("  mtxv entries end at offset {}", moff);
        eprintln!("  Remaining after mtxv entries: {} bytes", data.len() - moff);
        if moff < data.len() {
            let remaining = &data[moff..];
            let dump_len = remaining.len().min(128);
            let hex: Vec<String> = remaining[..dump_len].iter().map(|b| format!("{:02X}", b)).collect();
            eprintln!("  Remaining hex: {}", hex.join(" "));

            // Check for other markers
            for marker in &[b"mtx " as &[u8], b"rsem", b"bone", b"skin"] {
                if let Some(pos) = remaining.windows(marker.len()).position(|w| w == *marker) {
                    eprintln!("  Found marker '{}' at offset {} (abs {})",
                        std::str::from_utf8(marker).unwrap_or("?"), pos, moff + pos);
                }
            }
        }

        // =============================================
        // SUMMARY
        // =============================================
        eprintln!("\n====================================================");
        eprintln!("=== COMPLETE BYTE MAP SUMMARY ===");
        eprintln!("====================================================");
        eprintln!("  Header:       {:6}..{:6}  ({:5} bytes)", 0, 58, 58);
        eprintln!("  Vertices:     {:6}..{:6}  ({:5} bytes)  {} * 12", verts_off, verts_end, verts_size, n_verts);
        eprintln!("  Normals:      {:6}..{:6}  ({:5} bytes)  {} * 12", normals_off, normals_end, normals_size, n_normals);
        eprintln!("  Colors:       {:6}..{:6}  ({:5} bytes)  {} * 16", colors_off, colors_end, colors_size, n_colors);
        eprintln!("  TexCoords:    {:6}..{:6}  ({:5} bytes)  {} * 8", tex_off, tex_end, tex_size, n_tex1s);
        if n_tex2s > 0 {
            eprintln!("  Tex2:         {:6}..{:6}  ({:5} bytes)  {} * 8", tex2_off, tex2_end, tex2_size, n_tex2s);
        }
        if n_tangents > 0 {
            eprintln!("  Tangents:     {:6}..{:6}  ({:5} bytes)  {} * 12", tang_off, tang_end, tang_size, n_tangents);
        }
        eprintln!("  Materials:    {:6}..{:6}  ({:5} bytes)  {} * 86", mat_off, mat_end, mat_size, n_materials);
        eprintln!("  Packet rgn:   {:6}..{:6}  ({:5} bytes)  [blocks + indices?]", gap_start, gap_end, gap_size);
        eprintln!("  mtxv:         {:6}..{:6}  ({:5} bytes)", mtxv_offset, data.len(), data.len() - mtxv_offset);
        eprintln!("  TOTAL:        {:6} bytes", data.len());
        eprintln!("  Accounted:    {:6} bytes (header+verts+normals+colors+tex+mat+gap+mtxv)",
            58 + verts_size + normals_size + colors_size + tex_size + tex2_size + tang_size + mat_size + gap_size + (data.len() - mtxv_offset));
    }

    #[test]
    fn test_parse_mod_binary_blocks() {
        let base_str = format!("{}/Entity", crate::get_assets_path());
        let base = base_str.as_str();
        let paths: Vec<String> = vec![
            // Sci — 1 material, simple
            format!("{base}/Sci/win32_sci_LODs0.mod"),
            // Ted — 1 material each
            format!("{base}/Ted/win32_ted_LODs0.mod"),
            format!("{base}/Ted/win32_ted_LODs1.mod"),
            format!("{base}/Ted/win32_ted_LODs2.mod"),
            format!("{base}/Ted/win32_ted_LODs3.mod"),
            // Tim — 4 materials (LOD0-2), 1 material (LOD3)
            format!("{base}/Tim/win32_tim_LODs0.mod"),
            format!("{base}/Tim/win32_tim_LODs1.mod"),
            format!("{base}/Tim/win32_tim_LODs2.mod"),
            format!("{base}/Tim/win32_tim_LODs3.mod"),
            format!("{base}/Tim/win32_TimStand_LODs0.mod"),
            // frv — 1 material each
            format!("{base}/frv/win32_frv_LODs0.mod"),
            format!("{base}/frv/win32_frv_LODs1.mod"),
            format!("{base}/frv/win32_frv_LODs2.mod"),
            format!("{base}/frv/win32_frv_LODs3.mod"),
            // jae — 1 material each
            format!("{base}/jae/win32_jae_LODs0.mod"),
            format!("{base}/jae/win32_jae_LODs1.mod"),
            format!("{base}/jae/win32_jae_LODs2.mod"),
            format!("{base}/jae/win32_jae_LODs3.mod"),
            // kno — 1 material each
            format!("{base}/kno/win32_kno_LODs0.mod"),
            format!("{base}/kno/win32_kno_LODs1.mod"),
            format!("{base}/kno/win32_KonokoStand_LODs0.mod"),
            // oka — 1 material
            format!("{base}/oka/win32_oka_LODs0.mod"),
            // scv — 1 material
            format!("{base}/scv/win32_scv_LODs0.mod"),
            // scvEdgeModel — 1 material
            format!("{base}/scvEdgeModel/win32_scv_LODs0.mod"),
            // shn — 1 material each
            format!("{base}/shn/win32_shn_LODs0.mod"),
            format!("{base}/shn/win32_shn_LODs1.mod"),
            format!("{base}/shn/win32_shn_LODs2.mod"),
            format!("{base}/shn/win32_shn_LODs3.mod"),
        ];
        eprintln!("Found {} win32 LOD models", paths.len());
        for path in &paths {
            let data = match crate::vfs::read("", path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            if !data.starts_with(b"version: 2.10\0") { continue; }

            let fname = path.split('/').last().unwrap().to_string();
            let n_materials = read_u32_le(&data, 38);
            let n_adjuncts = read_u32_le(&data, 42);
            let n_primitives = read_u32_le(&data, 46);
            let n_matrices = read_u32_le(&data, 50);
            let n_reskins = read_u32_le(&data, 54);

            eprintln!("\n=== {} ({} bytes, mats={} adj={} prim={} mtx={} reskin={}) ===",
                fname, data.len(), n_materials, n_adjuncts, n_primitives, n_matrices, n_reskins);

            if let Some(model) = parse_mod_binary(&data, base_str) {
                let total_adj: usize = model.packets.iter().map(|p| p.adjuncts.len()).sum();
                let total_strips: usize = model.packets.iter().map(|p| p.strips.len()).sum();
                let total_strip_verts: usize = model.packets.iter()
                    .flat_map(|p| p.strips.iter())
                    .map(|s| s.len())
                    .sum();
                let total_tris: usize = model.packets.iter()
                    .flat_map(|p| p.strips.iter())
                    .map(|s| s.len().saturating_sub(2))
                    .sum();
                eprintln!("  → {} packets, {} adjuncts, {} strips, {} strip verts, ~{} triangles",
                    model.packets.len(), total_adj, total_strips, total_strip_verts, total_tris);
                for (pi, pkt) in model.packets.iter().enumerate() {
                    let strip_lens: Vec<usize> = pkt.strips.iter().map(|s| s.len()).collect();
                    eprintln!("    pkt[{}]: {} adj, {} strips, strip_lens={:?}",
                        pi, pkt.adjuncts.len(), pkt.strips.len(), strip_lens);
                }
            }
        }
    }

    /// Read a null-terminated string from data at offset.
    /// After the null terminator, skip ALL consecutive null bytes (u32(0) terminator pattern).
    /// Returns (string, bytes_consumed including null + padding).
    fn read_null_terminated_aligned(data: &[u8], off: usize) -> (String, usize) {
        let mut end = off;
        while end < data.len() && data[end] != 0 {
            end += 1;
        }
        let s = std::str::from_utf8(&data[off..end]).unwrap_or("").to_string();
        // Skip past the null terminator AND all consecutive null bytes
        let mut pos = end;
        while pos < data.len() && data[pos] == 0 {
            pos += 1;
        }
        let consumed = pos - off;
        (s, consumed)
    }

    /// Read a space-terminated string (0x20) from data at offset.
    /// Returns (string, bytes_consumed including the space terminator).
    fn read_space_terminated(data: &[u8], off: usize) -> (String, usize) {
        let mut end = off;
        // Read printable non-space chars (or any byte > 0x20)
        while end < data.len() && data[end] != 0x20 && data[end] != 0x00 {
            end += 1;
        }
        let s = std::str::from_utf8(&data[off..end]).unwrap_or("").to_string();
        let consumed = if end < data.len() { end - off + 1 } else { end - off }; // include terminator
        (s, consumed)
    }

    #[test]
    fn test_material_sequential_parse() {
        // Sequential field-by-field parser for binary material records.
        // AGE binary format principle: data is sequentially parsed, not fixed-size blocks.
        // Variable-length strings are inline (not in a string table), saving space.
        //
        // Hypothesized layout per material:
        //   - string (space-terminated): material name (e.g. "timbodySG")
        //   - u32: packet count
        //   - u32: primitive count
        //   - u32: texture count
        //   - u32: illum/reserved (always 1? — good validation signpost)
        //   - f32×3: ambient RGB (often 0,0,0)
        //   - f32×3: diffuse RGB (the main color)
        //   - f32×3: specular RGB (often 0,0,0)
        //   - ??? (unknown fields before texture name)
        //   - string (null-terminated): texture name
        //   - possibly more fields

        let base_str = format!("{}/Entity", crate::get_assets_path());
        let base = base_str.as_str();
        let files = vec![
            format!("{base}/Tim/win32_tim_LODs3.mod"),
            format!("{base}/kno/win32_kno_LODs0.mod"),
            format!("{base}/Tim/win32_tim_LODs0.mod"),
            format!("{base}/Sci/win32_sci_LODs0.mod"),
            format!("{base}/Ted/win32_ted_LODs0.mod"),  // single material
        ];

        for path in &files {
            let data = match crate::vfs::read("", path) {
                Ok(d) => d,
                Err(_) => { eprintln!("SKIP: {}", path); continue; }
            };
            if data.len() < 58 { continue; }

            let fname = path.rsplit('/').next().unwrap_or(path);
            let n_verts = read_u32_le(&data, 14) as usize;
            let n_normals = read_u32_le(&data, 18) as usize;
            let n_colors = read_u32_le(&data, 22) as usize;
            let n_tex1s = read_u32_le(&data, 26) as usize;
            let n_tex2s = read_u32_le(&data, 30) as usize;
            let n_tangents = read_u32_le(&data, 34) as usize;
            let n_materials = read_u32_le(&data, 38) as usize;
            let n_adjuncts = read_u32_le(&data, 42) as usize;
            let n_primitives = read_u32_le(&data, 46) as usize;

            let mat_off = 58 + n_verts * 12 + n_normals * 12 + n_colors * 16
                + n_tex1s * 8 + n_tex2s * 8 + n_tangents * 12;

            eprintln!("\n=== {} ({} mats, {} adj, {} prims) mat_off={} ===",
                fname, n_materials, n_adjuncts, n_primitives, mat_off);

            let mut off = mat_off;
            let mut total_pkts = 0u32;
            let mut total_prims = 0u32;

            for i in 0..n_materials {
                let mat_start = off;

                // 1. Material name — space-terminated
                let (name, name_len) = read_space_terminated(&data, off);
                off += name_len;

                // 2. u32 fields
                let packets = read_u32_le(&data, off);     off += 4;
                let prims = read_u32_le(&data, off);       off += 4;
                let tex_count = read_u32_le(&data, off);   off += 4;
                let illum = read_u32_le(&data, off);       off += 4;

                total_pkts += packets;
                total_prims += prims;

                // 3. Read 9 floats (ambient + diffuse + specular)
                let ambient: [f32; 3] = [
                    read_f32_le(&data, off), read_f32_le(&data, off + 4), read_f32_le(&data, off + 8)
                ];
                off += 12;
                let diffuse: [f32; 3] = [
                    read_f32_le(&data, off), read_f32_le(&data, off + 4), read_f32_le(&data, off + 8)
                ];
                off += 12;
                let specular: [f32; 3] = [
                    read_f32_le(&data, off), read_f32_le(&data, off + 4), read_f32_le(&data, off + 8)
                ];
                off += 12;

                // 4. After the 9 floats, dump remaining bytes until we find the texture name
                // Read u32s/f32s until we hit printable ASCII (texture name)
                let mut extra_vals = Vec::new();
                let scan_start = off;
                while off + 4 <= data.len() {
                    // Check if next 4 bytes look like start of a string (printable ASCII)
                    if data[off] >= b'a' && data[off] <= b'z' || data[off] >= b'A' && data[off] <= b'Z' {
                        break;
                    }
                    let val = read_u32_le(&data, off);
                    let fval = read_f32_le(&data, off);
                    extra_vals.push(format!("{}(f:{:.3})", val, fval));
                    off += 4;
                }

                // 5. Texture name — null-terminated, then padded to 4-byte boundary
                let (tex_name, tex_len) = read_null_terminated_aligned(&data, off);
                off += tex_len;

                // Print everything
                eprintln!("  mat[{}] '{}' pkts={} prims={} texs={} illum={}", i, name, packets, prims, tex_count, illum);
                eprintln!("    ambient={:?} diffuse={:?} specular={:?}", ambient, diffuse, specular);
                eprintln!("    extra after specular: [{}]", extra_vals.join(", "));
                eprintln!("    texture='{}' ({} bytes)", tex_name, tex_len);
                eprintln!("    record size: {} bytes (off={})", off - mat_start, off);
            }

            eprintln!("  TOTALS: pkts={} prims={} (header says {} prims)", total_pkts, total_prims, n_primitives);
            eprintln!("  Materials consumed {} bytes, ending at offset {}", off - mat_off, off);
        }
    }

    #[test]
    fn test_packet_sequential_parse() {
        // Strict packet parser. Aborts on any format mismatch.
        //
        // Packet format (binary):
        //   u32 adj_count, u32 strip_count, u32 matrix_count  (3 u32 header, matches ASCII)
        //   u32 multiweight_count                              (binary-only, for multiweight skinning)
        //   adj_count × 6 u32 adjunct records                  [vert, norm, color, tex1, tex2, bone_local]
        //   multiweight_count × 6 values bone influence        [u32 bone_id, u32 sub_id, f32 weight, f32 x, f32 y, f32 z]
        //   strip_count × 3 u32 triangle indices               (each tri refs local adjuncts)
        //   matrix_count × u32 bone map indices
        let base_str = format!("{}/Entity", crate::get_assets_path());
        let base = base_str.as_str();
        let files = vec![
            format!("{base}/Tim/win32_tim_LODs3.mod"),
            format!("{base}/kno/win32_kno_LODs0.mod"),
            format!("{base}/Tim/win32_tim_LODs0.mod"),
            format!("{base}/Sci/win32_sci_LODs0.mod"),
            format!("{base}/Ted/win32_ted_LODs0.mod"),
        ];

        for path in &files {
            let data = match crate::vfs::read("", path) {
                Ok(d) => d,
                Err(_) => { eprintln!("SKIP: {}", path); continue; }
            };
            if data.len() < 58 { continue; }

            let fname = path.rsplit('/').next().unwrap_or(path);
            let n_verts = read_u32_le(&data, 14) as usize;
            let n_normals = read_u32_le(&data, 18) as usize;
            let n_colors = read_u32_le(&data, 22) as usize;
            let n_tex1s = read_u32_le(&data, 26) as usize;
            let n_tex2s = read_u32_le(&data, 30) as usize;
            let n_tangents = read_u32_le(&data, 34) as usize;
            let n_materials = read_u32_le(&data, 38) as usize;
            let n_adjuncts = read_u32_le(&data, 42) as usize;
            let n_primitives = read_u32_le(&data, 46) as usize;

            // Skip to materials, parse them sequentially to find exact end
            let mut off = 58 + n_verts * 12 + n_normals * 12 + n_colors * 16
                + n_tex1s * 8 + n_tex2s * 8 + n_tangents * 12;

            let mut mat_info: Vec<(String, u32, u32)> = Vec::new();
            for _ in 0..n_materials {
                let name_start = off;
                while off < data.len() && data[off] != 0x20 && data[off] != 0x00 { off += 1; }
                let name = std::str::from_utf8(&data[name_start..off]).unwrap_or("").to_string();
                if off < data.len() { off += 1; }
                let pkts = read_u32_le(&data, off);   off += 4;
                let prims = read_u32_le(&data, off);   off += 4;
                off += 4; // tex_count
                off += 4; // illum
                off += 36; // 9 floats
                while off + 4 <= data.len() {
                    if data[off] >= b'a' && data[off] <= b'z' || data[off] >= b'A' && data[off] <= b'Z' { break; }
                    off += 4;
                }
                while off < data.len() && data[off] != 0 { off += 1; }
                while off < data.len() && data[off] == 0 { off += 1; }
                mat_info.push((name, pkts, prims));
            }

            let pkt_region_start = off;
            let search_start = data.len().saturating_sub(500);
            let mtxv_pos = data[search_start..].windows(4)
                .position(|w| w == b"mtxv")
                .map(|p| search_start + p)
                .unwrap_or(data.len());

            let total_pkts: u32 = mat_info.iter().map(|m| m.1).sum();

            eprintln!("\n=== {} ({} pkts, {} adj, {} prims, {} verts, {} norms) ===",
                fname, total_pkts, n_adjuncts, n_primitives, n_verts, n_normals);
            eprintln!("  Packet region: {}..{} ({} bytes)", pkt_region_start, mtxv_pos, mtxv_pos - pkt_region_start);

            // Strict sequential parse
            off = pkt_region_start;
            let mut total_adj = 0usize;
            let mut total_tri = 0usize;
            let mut total_mw = 0usize;

            for pkt_idx in 0..total_pkts as usize {
                let pkt_start = off;

                // --- Header: 3 u32 + 1 u32 multiweight ---
                assert!(off + 16 <= mtxv_pos, "pkt[{}] header truncated at {}", pkt_idx, off);
                let adj_count = read_u32_le(&data, off) as usize;   off += 4;
                let strip_count = read_u32_le(&data, off) as usize;  off += 4;
                let mtx_count = read_u32_le(&data, off) as usize;    off += 4;
                let mw_count = read_u32_le(&data, off) as usize;     off += 4;

                assert!(adj_count <= 256,
                    "pkt[{}] adj_count={} too large (offset {})", pkt_idx, adj_count, pkt_start);
                assert!(strip_count <= 256,
                    "pkt[{}] strip_count={} too large (offset {})", pkt_idx, strip_count, pkt_start);
                assert!(mtx_count <= 256,
                    "pkt[{}] mtx_count={} too large (offset {})", pkt_idx, mtx_count, pkt_start);
                assert!(mw_count <= 256,
                    "pkt[{}] mw_count={} too large (offset {})", pkt_idx, mw_count, pkt_start);

                // --- Adjuncts: adj_count × 6 u32 = 24 bytes each ---
                let adj_bytes = adj_count * 24;
                assert!(off + adj_bytes <= mtxv_pos,
                    "pkt[{}] adjuncts overflow at {} (need {})", pkt_idx, off, adj_bytes);
                // Validate all adjuncts
                for a in 0..adj_count {
                    let ao = off + a * 24;
                    let v = read_u32_le(&data, ao) as usize;
                    let n = read_u32_le(&data, ao + 4) as usize;
                    let c = read_u32_le(&data, ao + 8) as usize;
                    assert!(v < n_verts, "pkt[{}] adj[{}] vert={} >= n_verts={}", pkt_idx, a, v, n_verts);
                    assert!(n < n_normals, "pkt[{}] adj[{}] norm={} >= n_normals={}", pkt_idx, a, n, n_normals);
                    assert!(c <= 1, "pkt[{}] adj[{}] color={} > 1", pkt_idx, a, c);
                }
                off += adj_bytes;
                total_adj += adj_count;

                // --- Multiweight: mw_count × 24 bytes each ---
                let mw_bytes = mw_count * 24;
                assert!(off + mw_bytes <= mtxv_pos,
                    "pkt[{}] multiweight overflow at {} (need {})", pkt_idx, off, mw_bytes);
                // Validate: first 2 u32 should be small (bone indices), next 4 should be valid floats
                for m in 0..mw_count {
                    let mo = off + m * 24;
                    let bone_id = read_u32_le(&data, mo) as usize;
                    let w = read_f32_le(&data, mo + 8);
                    assert!(bone_id < 256,
                        "pkt[{}] mw[{}] bone_id={} too large", pkt_idx, m, bone_id);
                    assert!(w.is_finite() && w.abs() <= 1.0,
                        "pkt[{}] mw[{}] weight={} invalid", pkt_idx, m, w);
                }
                off += mw_bytes;
                total_mw += mw_count;

                // --- Strips: strip_count strips, each = u32 count + count × u32 indices ---
                // (ASCII: "str N i0 i1 ... iN-1" or "stp N i0 i1 ... iN-1")
                let mut pkt_prims = 0usize;
                for s in 0..strip_count {
                    assert!(off + 4 <= mtxv_pos,
                        "pkt[{}] strip[{}] count truncated at {}", pkt_idx, s, off);
                    let scount = read_u32_le(&data, off) as usize;
                    off += 4;
                    assert!(scount <= 256,
                        "pkt[{}] strip[{}] count={} too large", pkt_idx, s, scount);
                    let idx_bytes = scount * 4;
                    assert!(off + idx_bytes <= mtxv_pos,
                        "pkt[{}] strip[{}] indices overflow at {} (need {})", pkt_idx, s, off, idx_bytes);
                    for j in 0..scount {
                        let idx = read_u32_le(&data, off + j * 4) as usize;
                        assert!(idx < adj_count,
                            "pkt[{}] strip[{}] index[{}]={} >= adj_count={}",
                            pkt_idx, s, j, idx, adj_count);
                    }
                    off += idx_bytes;
                    // Triangle count: strip of N vertices = N-2 triangles (if N >= 3)
                    if scount >= 3 { pkt_prims += scount - 2; }
                }
                total_tri += pkt_prims;

                // --- Bone map: mtx_count × u32 ---
                let mtx_bytes = mtx_count * 4;
                assert!(off + mtx_bytes <= mtxv_pos,
                    "pkt[{}] bone_map overflow at {} (need {})", pkt_idx, off, mtx_bytes);
                off += mtx_bytes;

                if pkt_idx < 3 {
                    eprintln!("  pkt[{}] adj={} strip={} mtx={} mw={} prims={} size={}",
                        pkt_idx, adj_count, strip_count, mtx_count, mw_count, pkt_prims, off - pkt_start);
                    // Print strip details
                    {
                        let mut soff = pkt_start + 16 + adj_count * 24 + mw_count * 24;
                        for s in 0..strip_count {
                            let sc = read_u32_le(&data, soff) as usize;
                            let indices: Vec<u32> = (0..sc.min(10)).map(|j| read_u32_le(&data, soff + 4 + j * 4)).collect();
                            eprintln!("    strip[{}]: count={} indices={:?}{}", s, sc, indices, if sc > 10 { "..." } else { "" });
                            soff += 4 + sc * 4;
                        }
                        // Print bone map
                        let bone_map: Vec<u32> = (0..mtx_count).map(|i| read_u32_le(&data, soff + i * 4)).collect();
                        eprintln!("    bone_map: {:?}", bone_map);
                        // Print next 8 u32s after bone map (should be next header)
                        let after_bm = soff + mtx_count * 4;
                        let peek: Vec<String> = (0..8.min((mtxv_pos - after_bm) / 4)).map(|i| {
                            format!("{}", read_u32_le(&data, after_bm + i * 4))
                        }).collect();
                        eprintln!("    next after bonemap at {}: [{}]", after_bm, peek.join(", "));
                    }
                }
            }

            let consumed = off - pkt_region_start;
            let region_size = mtxv_pos - pkt_region_start;
            eprintln!("  Parsed all {} pkts: adj={} (expect {}), tri={} (expect {}), mw={}",
                total_pkts, total_adj, n_adjuncts, total_tri, n_primitives, total_mw);
            eprintln!("  Consumed {}/{} bytes, remaining: {}",
                consumed, region_size, region_size - consumed);
            if consumed == region_size {
                eprintln!("  >>> PERFECT MATCH <<<");
            }
        }
    }


    #[test]
    fn test_packet_region_analysis() {
        // Analyze the packet region between materials-end and mtxv.
        // Now that we know exact material boundaries, we can find the exact packet region start.
        let base_str = format!("{}/Entity", crate::get_assets_path());
        let base = base_str.as_str();
        let files = vec![
            // Simple: 2 materials, 9 packets, 140 adjuncts, 200 primitives
            format!("{base}/Tim/win32_tim_LODs3.mod"),
            // Medium: 2 materials, 111 packets, 2301 adjuncts, 2800 primitives
            format!("{base}/kno/win32_kno_LODs0.mod"),
        ];

        for path in &files {
            let data = match crate::vfs::read("", path) {
                Ok(d) => d,
                Err(_) => { eprintln!("SKIP: {}", path); continue; }
            };
            if data.len() < 58 { continue; }

            let fname = path.rsplit('/').next().unwrap_or(path);
            let n_verts = read_u32_le(&data, 14) as usize;
            let n_normals = read_u32_le(&data, 18) as usize;
            let n_colors = read_u32_le(&data, 22) as usize;
            let n_tex1s = read_u32_le(&data, 26) as usize;
            let n_tex2s = read_u32_le(&data, 30) as usize;
            let n_tangents = read_u32_le(&data, 34) as usize;
            let n_materials = read_u32_le(&data, 38) as usize;
            let n_adjuncts = read_u32_le(&data, 42) as usize;
            let n_primitives = read_u32_le(&data, 46) as usize;
            let n_matrices = read_u32_le(&data, 50) as usize;

            // Skip to material region
            let mut off = 58 + n_verts * 12 + n_normals * 12 + n_colors * 16
                + n_tex1s * 8 + n_tex2s * 8 + n_tangents * 12;

            // Parse materials sequentially to find exact end
            let mut mat_info: Vec<(String, u32, u32)> = Vec::new(); // (name, pkts, prims)
            for _ in 0..n_materials {
                let name_start = off;
                while off < data.len() && data[off] != 0x20 && data[off] != 0x00 { off += 1; }
                let name = std::str::from_utf8(&data[name_start..off]).unwrap_or("").to_string();
                if off < data.len() { off += 1; }
                let pkts = read_u32_le(&data, off);   off += 4;
                let prims = read_u32_le(&data, off);   off += 4;
                let _texs = read_u32_le(&data, off);   off += 4;
                let _illum = read_u32_le(&data, off);  off += 4;
                off += 36; // 9 floats
                // Skip extra values until printable ASCII
                while off + 4 <= data.len() {
                    if data[off] >= b'a' && data[off] <= b'z' || data[off] >= b'A' && data[off] <= b'Z' { break; }
                    off += 4;
                }
                // Skip texture name (null-terminated + null padding)
                while off < data.len() && data[off] != 0 { off += 1; }
                while off < data.len() && data[off] == 0 { off += 1; }
                mat_info.push((name, pkts, prims));
            }

            let pkt_region_start = off;
            // Find mtxv
            let search_start = data.len().saturating_sub(500);
            let mtxv_pos = data[search_start..].windows(4)
                .position(|w| w == b"mtxv")
                .map(|p| search_start + p)
                .unwrap_or(data.len());
            let pkt_region_end = mtxv_pos;
            let pkt_region_size = pkt_region_end - pkt_region_start;

            let total_pkts: u32 = mat_info.iter().map(|m| m.1).sum();
            let total_prims: u32 = mat_info.iter().map(|m| m.2).sum();

            eprintln!("\n=== {} ===", fname);
            eprintln!("  {} materials, {} total pkts, {} adjuncts, {} primitives, {} matrices",
                n_materials, total_pkts, n_adjuncts, n_primitives, n_matrices);
            for (i, (name, pkts, prims)) in mat_info.iter().enumerate() {
                eprintln!("    mat[{}] '{}': {} pkts, {} prims", i, name, pkts, prims);
            }
            eprintln!("  Packet region: {}..{} ({} bytes)", pkt_region_start, pkt_region_end, pkt_region_size);

            // Size analysis: what could fit in the packet region?
            // Each adjunct record in ASCII has 6 fields (vert, norm, color, tex1, tex2?, bone)
            // If stored as 6 × u32 = 24 bytes per adjunct:
            let adjunct_bytes_24 = n_adjuncts * 24;
            // If stored as 6 × u16 = 12 bytes per adjunct:
            let adjunct_bytes_12 = n_adjuncts * 12;
            // Primitives (triangle strip indices): if u16 each
            let prim_bytes_u16 = n_primitives * 2;
            let prim_bytes_u32 = n_primitives * 4;

            eprintln!("  Size analysis:");
            eprintln!("    adjuncts×24 = {} bytes ({:.1}% of region)", adjunct_bytes_24, adjunct_bytes_24 as f64 / pkt_region_size as f64 * 100.0);
            eprintln!("    adjuncts×12 = {} bytes ({:.1}% of region)", adjunct_bytes_12, adjunct_bytes_12 as f64 / pkt_region_size as f64 * 100.0);
            eprintln!("    prims×2(u16) = {} bytes", prim_bytes_u16);
            eprintln!("    prims×4(u32) = {} bytes", prim_bytes_u32);
            eprintln!("    adj×24 + prims×2 = {} bytes ({:.1}%)", adjunct_bytes_24 + prim_bytes_u16, (adjunct_bytes_24 + prim_bytes_u16) as f64 / pkt_region_size as f64 * 100.0);
            eprintln!("    adj×12 + prims×2 = {} bytes ({:.1}%)", adjunct_bytes_12 + prim_bytes_u16, (adjunct_bytes_12 + prim_bytes_u16) as f64 / pkt_region_size as f64 * 100.0);

            // Average bytes per packet
            let avg_bytes_per_pkt = pkt_region_size as f64 / total_pkts as f64;
            eprintln!("    avg bytes/packet = {:.1}", avg_bytes_per_pkt);

            // Dump the first 256 bytes of the packet region as hex + u32 interpretation
            eprintln!("  --- First 256 bytes of packet region ---");
            let dump_len = 256.min(pkt_region_size);
            for row in (0..dump_len).step_by(16) {
                let abs = pkt_region_start + row;
                let end = (row + 16).min(dump_len);
                let hex: Vec<String> = (row..end).map(|j| format!("{:02X}", data[pkt_region_start + j])).collect();
                let ascii: String = (row..end).map(|j| {
                    let b = data[pkt_region_start + j];
                    if b >= 0x20 && b < 0x7F { b as char } else { '.' }
                }).collect();
                // Also show as u32 values
                let mut u32s = Vec::new();
                let mut p = row;
                while p + 4 <= end {
                    u32s.push(format!("{}", read_u32_le(&data, pkt_region_start + p)));
                    p += 4;
                }
                eprintln!("  {:5}: {:48} |{}| u32: [{}]", abs, hex.join(" "), ascii, u32s.join(", "));
            }

            // Look for patterns: try reading as array of u32 and check for small values
            // (adjunct indices should be < n_verts, < n_normals, etc.)
            eprintln!("  --- Scanning for adjunct-like patterns (6 consecutive u32s where v<{}, n<{}) ---", n_verts, n_normals);
            let mut found = 0;
            for i in (0..pkt_region_size.saturating_sub(24)).step_by(4) {
                let abs = pkt_region_start + i;
                let v0 = read_u32_le(&data, abs) as usize;
                let v1 = read_u32_le(&data, abs + 4) as usize;
                let v2 = read_u32_le(&data, abs + 8) as usize;
                let v3 = read_u32_le(&data, abs + 12) as usize;
                // Check if this looks like an adjunct: v0<n_verts, v1<n_normals, v2 small, v3<n_tex1s or similar
                if v0 < n_verts && v1 < n_normals && v2 <= 1 && v3 < n_tex1s.max(1) + 1 {
                    if found < 10 {
                        let v4 = read_u32_le(&data, abs + 16) as usize;
                        let v5 = read_u32_le(&data, abs + 20) as usize;
                        eprintln!("    offset {}: [{}, {}, {}, {}, {}, {}]", abs, v0, v1, v2, v3, v4, v5);
                    }
                    found += 1;
                }
            }
            eprintln!("    Total adjunct-like u32×6 patterns found: {} (expected {})", found, n_adjuncts);

            // Also try u16 adjuncts
            eprintln!("  --- Scanning for adjunct-like patterns as u16×6 ---");
            let mut found16 = 0;
            for i in (0..pkt_region_size.saturating_sub(12)).step_by(2) {
                let abs = pkt_region_start + i;
                let v0 = read_u16_le(&data, abs) as usize;
                let v1 = read_u16_le(&data, abs + 2) as usize;
                let v2 = read_u16_le(&data, abs + 4) as usize;
                let v3 = read_u16_le(&data, abs + 6) as usize;
                if v0 < n_verts && v1 < n_normals && v2 <= 1 && v3 < n_tex1s.max(1) + 1 {
                    if found16 < 10 {
                        let v4 = read_u16_le(&data, abs + 8) as usize;
                        let v5 = read_u16_le(&data, abs + 10) as usize;
                        eprintln!("    offset {}: [{}, {}, {}, {}, {}, {}]", abs, v0, v1, v2, v3, v4, v5);
                    }
                    found16 += 1;
                }
            }
            eprintln!("    Total adjunct-like u16×6 patterns found: {} (expected {})", found16, n_adjuncts);
        }
    }


    #[test]
    fn test_scan_all_kno_anim_channels() {
        let base_str = format!("{}/Entity/kno", crate::get_assets_path());
        let base = base_str.as_str();
        let skel_data = crate::vfs::read_to_string(base, "kno.skel").expect("skel");
        let skel = parse_skel(&skel_data);
        let num_bones = skel.positions.len();
        let expected_channels = num_bones * 3 + 3; // 39*3+3 = 120

        eprintln!("Skeleton has {} bones, expected {} channels", num_bones, expected_channels);
        eprintln!("{:-<80}", "");

        let mut entries: Vec<_> = crate::vfs::read_dir(base).expect("read dir")
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e: &crate::vfs::VfsEntry| e.path().extension().map_or(false, |ext| ext == "anim"))
            .collect();
        entries.sort_by_key(|e: &crate::vfs::VfsEntry| e.file_name());

        let mut count_match = 0;
        let mut count_mismatch = 0;
        let mut channel_counts: std::collections::BTreeMap<u32, Vec<String>> = std::collections::BTreeMap::new();

        for entry in &entries {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let data = crate::vfs::read("", &path).expect("read anim");
            match parse_anim(&data) {
                Some(anim) => {
                    let marker = if anim.num_channels as usize == expected_channels { "MATCH" } else { "     " };
                    if anim.num_channels as usize == expected_channels { count_match += 1; } else { count_mismatch += 1; }
                    eprintln!("  {} {:40} channels={:4} frames={:4} stride_z={:.3} loop={}",
                        marker, name, anim.num_channels, anim.num_frames, anim.stride_z, anim.is_loop);
                    channel_counts.entry(anim.num_channels).or_default().push(name);
                }
                None => {
                    eprintln!("  FAIL  {:40} could not parse", name);
                }
            }
        }

        eprintln!("\n{:-<80}", "");
        eprintln!("Summary: {} match ({} channels), {} mismatch", count_match, expected_channels, count_mismatch);
        eprintln!("\nChannel count distribution:");
        for (ch, names) in &channel_counts {
            let implied_bones = if *ch >= 3 { (*ch - 3) / 3 } else { 0 };
            eprintln!("  {} channels ({} implied bones): {} anims", ch, implied_bones, names.len());
            for n in names {
                eprintln!("    {}", n);
            }
        }
    }

    #[test]
    fn test_anim_library_loading() {
        let entity_dir_str = format!("{}/Entity/kno", crate::get_assets_path());
        let entity_dir = entity_dir_str.as_str();
        let assets_base_str = crate::get_assets_path().to_string();
        let assets_base = assets_base_str.as_str();
        let skel_data = crate::vfs::read_to_string(entity_dir, "kno.skel").expect("skel");
        let skel = parse_skel(&skel_data);

        use crate::oni2_loader::{load_anim_library, AnimId};

        let library = load_anim_library(entity_dir, "kno", &skel);

        eprintln!("Loaded {} animations into library", library.anims.len());
        let mut aliases: Vec<&str> = library.aliases();
        aliases.sort();
        for alias in &aliases {
            let id = AnimId::new(alias);
            let anim = library.anims.get(&id).unwrap();
            eprintln!("  {:40} -> {} frames, ch={}, loop={}",
                alias, anim.num_frames, anim.num_channels, anim.is_loop);
        }

        // Verify const-time hash matches runtime hash
        const RUN_FWD: AnimId = AnimId::new("ANIMNAV_RUN_FORWARD");
        let runtime = AnimId::new("ANIMNAV_RUN_FORWARD");
        assert_eq!(RUN_FWD, runtime);
        eprintln!("\nConst AnimId test: ANIMNAV_RUN_FORWARD = {}", RUN_FWD);

        if library.anims.contains_key(&RUN_FWD) {
            eprintln!("ANIMNAV_RUN_FORWARD found in library!");
        } else {
            eprintln!("ANIMNAV_RUN_FORWARD NOT found (may be channel mismatch)");
        }

        assert!(!library.anims.is_empty(), "Library should not be empty");
    }
}
