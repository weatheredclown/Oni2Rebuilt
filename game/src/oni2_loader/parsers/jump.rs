use bevy::prelude::*;

#[derive(Debug, Clone, Reflect, Component, Default)]
#[reflect(Component)]
pub struct JumpState {
    pub height: f32,
    pub gravity_factor: f32,
    pub length: f32,
    pub jump_type: String,
}

#[derive(Debug, Clone, Reflect, Component, Default)]
#[reflect(Component)]
pub struct JumpController {
    pub jumps: Vec<JumpState>,
}

pub fn parse_jump_content(content: &str) -> JumpController {
    let mut controller = JumpController::default();
    let mut current_jump = JumpState::default();
    let mut in_jumpdata = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        if trimmed == "jumpdata" {
            continue;
        }

        if trimmed == "{" {
            in_jumpdata = true;
            continue;
        }

        if trimmed == "}" {
            if in_jumpdata && (current_jump.height > 0.0 || current_jump.length > 0.0) {
                // If it ends abruptly without a type, push the final jump
                controller.jumps.push(current_jump.clone());
            }
            in_jumpdata = false;
            continue;
        }

        if in_jumpdata {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            match parts[0] {
                "height" => {
                    // Start of a new jump block inherently happens when we loop back to 'height'
                    // but we only push if we already populated data
                    if current_jump.height > 0.0 {
                        controller.jumps.push(current_jump.clone());
                        current_jump = JumpState::default();
                    }
                    if parts.len() > 1 {
                        current_jump.height = parts[1].parse().unwrap_or(0.0);
                    }
                }
                "gravity_factor" => {
                    if parts.len() > 1 {
                        current_jump.gravity_factor = parts[1].parse().unwrap_or(1.0);
                    }
                }
                "length" => {
                    if parts.len() > 1 {
                        current_jump.length = parts[1].parse().unwrap_or(0.0);
                    }
                }
                "type" => {
                    if parts.len() > 1 {
                        current_jump.jump_type = parts[1].to_string();
                        // Type is usually the last field of a block.
                        controller.jumps.push(current_jump.clone());
                        current_jump = JumpState::default();
                    }
                }
                _ => {}
            }
        }
    }

    controller
}
