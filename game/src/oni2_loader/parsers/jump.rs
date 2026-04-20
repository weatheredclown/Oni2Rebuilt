/*
 * oni2_loader/parsers/jump.rs — jump parameter data parser.
 *
 * JumpState component: height, gravity_factor, length, jump_type string.
 * Parsed from the jump package referenced in an entity's .anims file and
 * attached to the entity on spawn to drive jump physics.
 */
use super::block_parser::BlockParser;
use bevy::prelude::*;

#[derive(Debug, Clone, Reflect, Component, Default)]
#[reflect(Component)]
pub struct JumpState {
    pub height: f32,
    pub gravity_factor: f32,
    pub length: f32,
    pub jump_type: String,
    pub heading_adjust: f32,
}

#[derive(Debug, Clone, Reflect, Component, Default)]
#[reflect(Component)]
pub struct JumpController {
    pub jumps: Vec<JumpState>,
}

// deleted local BlockParser

pub fn parse_jump_content(content: &str) -> JumpController {
    let mut controller = JumpController::default();
    let mut p = BlockParser::new(content);

    if p.start("jumpdata") {
        while !p.endblock() {
            if p.peek() != Some("height") {
                p.tokens.next();
                continue;
            }

            let mut current_jump = JumpState::default();
            current_jump.height = p.read_float("height", 0.0);
            current_jump.gravity_factor = p.read_float("gravity_factor", 0.0);
            current_jump.length = p.read_float("length", 0.0);

            if let Some(val) = p.read_float_opt("heading_adjust") {
                current_jump.heading_adjust = val;
            }
            if let Some(val) = p.read_string_opt("type") {
                current_jump.jump_type = val;
            }

            controller.jumps.push(current_jump);
        }
    }

    controller
}
