use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ParticleSystemDef {
    pub name: String,
    pub texture: Handle<Image>,
    pub position_var: Vec3,
    pub radius_birth: Vec2, // Could be min/max
    pub life: f32,
    pub life_var: f32,
    pub velocity: Vec3,
    pub velocity_var: Vec3,
    pub velocity_damping: Vec3,
    pub gravity: f32,
    pub color_birth: Color,
    pub color_death: Color,
    pub rate: f32,
    pub blend_set: i32,
    pub frame_rate: f32,
}

// Very basic custom parser for .ptx format which is:
// type: a
// Particle {
//   Key Value
//   Key Value Value Value
// }
pub fn parse_ptx(content: &str, name: String, asset_server: &AssetServer) -> Option<ParticleSystemDef> {
    let mut texture = asset_server.add(Image::default()); // Placeholder
    let mut position_var = Vec3::ZERO;
    let mut radius_birth = Vec2::ZERO;
    let mut life = 1.0;
    let mut life_var = 0.0;
    let mut velocity = Vec3::ZERO;
    let mut velocity_var = Vec3::ZERO;
    let mut velocity_damping = Vec3::ZERO;
    let mut gravity = 0.0;
    let mut color_birth = Color::WHITE;
    let mut color_death = Color::WHITE;
    let mut rate = 1.0;
    let mut blend_set = 0;
    let mut frame_rate = 0.0;

    for line in content.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() { continue; }

        match tokens[0] {
            "TextureName" => {
                if tokens.len() > 1 {
                    // Typical .ptx textures might just be names like ptx_explosion4x4
                    texture = asset_server.load(format!("textures/{}.png", tokens[1]));
                }
            }
            "PositionVar" => if tokens.len() > 3 { position_var = Vec3::new(tokens[1].parse().unwrap_or(0.0), tokens[2].parse().unwrap_or(0.0), tokens[3].parse().unwrap_or(0.0)); },
            "RadiusBirth" => if tokens.len() > 2 { radius_birth = Vec2::new(tokens[1].parse().unwrap_or(0.0), tokens[2].parse().unwrap_or(0.0)); },
            "Life" => if tokens.len() > 1 { life = tokens[1].parse().unwrap_or(1.0); },
            "LifeVar" => if tokens.len() > 1 { life_var = tokens[1].parse().unwrap_or(0.0); },
            "Velocity" => if tokens.len() > 3 { velocity = Vec3::new(tokens[1].parse().unwrap_or(0.0), tokens[2].parse().unwrap_or(0.0), tokens[3].parse().unwrap_or(0.0)); },
            "VelocityVar" => if tokens.len() > 3 { velocity_var = Vec3::new(tokens[1].parse().unwrap_or(0.0), tokens[2].parse().unwrap_or(0.0), tokens[3].parse().unwrap_or(0.0)); },
            "VelocityDamping" => if tokens.len() > 3 { velocity_damping = Vec3::new(tokens[1].parse().unwrap_or(0.0), tokens[2].parse().unwrap_or(0.0), tokens[3].parse().unwrap_or(0.0)); },
            "Gravity" => if tokens.len() > 1 { gravity = tokens[1].parse().unwrap_or(0.0); },
            "ColorBirth" => {
                if tokens.len() > 4 { 
                    color_birth = Color::srgba(tokens[1].parse().unwrap_or(1.0), tokens[2].parse().unwrap_or(1.0), tokens[3].parse().unwrap_or(1.0), tokens[4].parse().unwrap_or(1.0)); 
                }
            }
            "ColorDeath" => {
                if tokens.len() > 4 { 
                    color_death = Color::srgba(tokens[1].parse().unwrap_or(1.0), tokens[2].parse().unwrap_or(1.0), tokens[3].parse().unwrap_or(1.0), tokens[4].parse().unwrap_or(1.0)); 
                }
            }
            "Rate" => if tokens.len() > 1 { rate = tokens[1].parse().unwrap_or(1.0); },
            "BlendSet" => if tokens.len() > 1 { blend_set = tokens[1].parse().unwrap_or(0); },
            "FrameRate" => if tokens.len() > 1 { frame_rate = tokens[1].parse().unwrap_or(0.0); },
            _ => {}
        }
    }

    Some(ParticleSystemDef {
        name,
        texture,
        position_var,
        radius_birth,
        life,
        life_var,
        velocity,
        velocity_var,
        velocity_damping,
        gravity,
        color_birth,
        color_death,
        rate,
        blend_set,
        frame_rate,
    })
}
