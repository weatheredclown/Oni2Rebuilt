use super::types::Oni2Skeleton;

/// Parse a .skel file and return skeleton with bone world positions, parent indices, and names.
/// Bones are numbered in depth-first pre-order traversal.
pub fn parse_skel(content: &str) -> Oni2Skeleton {
    let mut positions = Vec::new();
    let mut parent_indices = Vec::new();
    let mut names = Vec::new();
    let mut local_offsets = Vec::new();
    let mut channels = Vec::new();
    // Stack of (parent world position, parent bone index or None for root level)
    let mut parent_stack: Vec<([f32; 3], Option<usize>)> = vec![([0.0; 3], None)];

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("bone ") && trimmed.ends_with('{') {
            let name = trimmed.strip_prefix("bone ").unwrap()
                .strip_suffix(" {").unwrap().trim().to_string();
            let (parent_pos, parent_idx) = *parent_stack.last().unwrap_or(&([0.0; 3], None));
            let bone_idx = positions.len();
            // Placeholder position — will be updated by offset line
            positions.push(parent_pos);
            parent_indices.push(parent_idx);
            names.push(name);
            local_offsets.push([0.0; 3]); // placeholder
            channels.push(crate::oni2_loader::parsers::types::Oni2BoneChannels::default());
            parent_stack.push((parent_pos, Some(bone_idx)));
        } else if trimmed.starts_with("offset ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let ox: f32 = parts[1].parse().unwrap_or(0.0);
                let oy: f32 = parts[2].parse().unwrap_or(0.0);
                let oz: f32 = parts[3].parse().unwrap_or(0.0);
                // Store raw local offset
                if let Some(last) = local_offsets.last_mut() {
                    *last = [ox, oy, oz];
                }
                // The parent position is one level up in the stack
                let parent_pos = if parent_stack.len() >= 2 {
                    parent_stack[parent_stack.len() - 2].0
                } else {
                    [0.0; 3]
                };
                let world_pos = [parent_pos[0] + ox, parent_pos[1] + oy, parent_pos[2] + oz];
                // Update the current bone's world position
                if let Some(last) = positions.last_mut() {
                    *last = world_pos;
                }
                // Update the stack top so children inherit this position
                if let Some(top) = parent_stack.last_mut() {
                    top.0 = world_pos;
                }
            }
        } else if trimmed.starts_with("trans") || trimmed.starts_with("rot") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() > 1 && parts[1] == "lock" {
                // Locked track, does not consume anim channels (typically)
                continue;
            }
            if let Some(ch) = channels.last_mut() {
                match parts[0] {
                    "transX" => ch.has_trans_x = true,
                    "transY" => ch.has_trans_y = true,
                    "transZ" => ch.has_trans_z = true,
                    "rotX" => ch.has_rot_x = true,
                    "rotY" => ch.has_rot_y = true,
                    "rotZ" => ch.has_rot_z = true,
                    _ => {}
                }
            }
        } else if trimmed == "}" {
            parent_stack.pop();
        }
    }

    let mut skel = Oni2Skeleton { positions, parent_indices, names, local_offsets, channels, channel_is_rot: Vec::new() };
    skel.build_channel_map();
    skel
}
