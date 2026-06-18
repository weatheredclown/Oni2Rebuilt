/*
 * oni2_loader/parsers/skeleton.rs — .skel skeleton file parser.
 *
 * parse_skel: reads world positions, parent indices, bone names, local offsets,
 * and channel lists from the text .skel format.  Bones are numbered in
 * depth-first pre-order.  Returns Oni2Skeleton used by mesh building, IK, and
 * the bone-local vertex conversion utility.
 */
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
            let name = trimmed
                .strip_prefix("bone ")
                .unwrap()
                .strip_suffix(" {")
                .unwrap()
                .trim()
                .to_string();
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
            let has_limit = parts.len() >= 4 && parts[1] == "limit";
            let limit = if has_limit {
                let v1 = parts[2].parse::<f32>().unwrap_or(0.0);
                let v2 = parts[3].parse::<f32>().unwrap_or(0.0);
                Some([v1.min(v2), v1.max(v2)])
            } else {
                None
            };
            if let Some(ch) = channels.last_mut() {
                match parts[0] {
                    "transX" => ch.has_trans_x = true,
                    "transY" => ch.has_trans_y = true,
                    "transZ" => ch.has_trans_z = true,
                    "rotX" => {
                        ch.has_rot_x = true;
                        if has_limit {
                            ch.rot_x_limit = limit;
                        }
                    }
                    "rotY" => {
                        ch.has_rot_y = true;
                        if has_limit {
                            ch.rot_y_limit = limit;
                        }
                    }
                    "rotZ" => {
                        ch.has_rot_z = true;
                        if has_limit {
                            ch.rot_z_limit = limit;
                        }
                    }
                    _ => {}
                }
            }
        } else if trimmed == "}" {
            parent_stack.pop();
        }
    }

    let mut skel = Oni2Skeleton {
        positions,
        parent_indices,
        names,
        local_offsets,
        channels,
        channel_is_rot: Vec::new(),
    };
    skel.build_channel_map();
    skel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skel_limits() {
        let content = r#"
Version: 101
NumBones 2
bone root {
	offset 0.000000 0.000000 0.000000
	transX 
	transY 
	transZ 
	rotX 
	rotY 
	rotZ 
	bone arm_01 {
		offset 0.000000 0.214873 0.000000
		rotX limit 0.244 -2.424
		rotY 
		rotZ limit 0.000 0.000
	}
}
"#;
        let skel = parse_skel(content);
        assert_eq!(skel.names[1], "arm_01");

        let ch0 = &skel.channels[0];
        assert_eq!(ch0.rot_x_limit, None);
        assert_eq!(ch0.rot_y_limit, None);
        assert_eq!(ch0.rot_z_limit, None);

        let ch1 = &skel.channels[1];
        assert!(ch1.has_rot_x);
        assert!(ch1.has_rot_y);
        assert!(ch1.has_rot_z);
        // Note: min/max sorted by parse_skel
        assert_eq!(ch1.rot_x_limit, Some([-2.424, 0.244]));
        assert_eq!(ch1.rot_y_limit, None);
        assert_eq!(ch1.rot_z_limit, Some([0.0, 0.0]));
    }
}

