/*
 * oni2_loader/parsers/particle.rs — particle system definition parser.
 *
 * ParticleSystemDef: texture handle, spawn parameters (position variance,
 * radius, life, velocity, color, birth rate, particle count).  parse_ptx reads
 * the .ptx settings block and loads the referenced texture into Assets<Image>.
 * Stored in ParticleLibrary and instantiated by fx_system via SpawnPtx events.
 */
use super::block_parser::BlockParser;
use bevy::prelude::*;

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
    pub grid_x: u32,
    pub grid_y: u32,
    pub start_tile: u32,
    pub end_tile: u32,
}

pub fn parse_ptx(
    content: &str,
    name: String,
    asset_server: &AssetServer,
    images: &mut Assets<Image>,
) -> Option<ParticleSystemDef> {
    let mut def = ParticleSystemDef {
        name,
        texture: asset_server.add(Image::default()), // Placeholder
        position_var: Vec3::ZERO,
        radius_birth: Vec2::ZERO,
        life: 1.0,
        life_var: 0.0,
        velocity: Vec3::ZERO,
        velocity_var: Vec3::ZERO,
        velocity_damping: Vec3::ZERO,
        gravity: 0.0,
        color_birth: Color::WHITE,
        color_death: Color::WHITE,
        rate: 1.0,
        blend_set: 0,
        frame_rate: 0.0,
        grid_x: 1,
        grid_y: 1,
        start_tile: 0,
        end_tile: 0,
    };

    let mut p = BlockParser::new(content);

    // .ptx files usually start with `type: a\nParticle {\n...`
    // We scan until we hit "Particle"
    let mut found = false;
    while !p.endblock() {
        if p.start("Particle") {
            found = true;
            break;
        } else {
            p.next();
        }
    }

    if !found {
        return None;
    }

    // Now parse properties inside Particle { ... }
    while !p.endblock() {
        let key = p.peek().unwrap_or("").to_string();
        match key.as_str() {
            "TextureName" => {
                if let Some(tex_name) = p.read_string_opt("TextureName") {
                    if let Some((h, _)) = crate::oni2_loader::parsers::texture::load_tga_texture(
                        "texture", &tex_name, images,
                    ) {
                        def.texture = h;
                    } else {
                        def.texture = asset_server.load(format!("texture/{}.tga", tex_name));
                    }
                }
            }
            "PositionVar" => def.position_var = p.read_vec3("PositionVar", def.position_var),
            "RadiusBirth" => {
                let vec = p.read_vec2("RadiusBirth", def.radius_birth);
                def.radius_birth = vec;
            }
            "Life" => def.life = p.read_float("Life", def.life),
            "LifeVar" => def.life_var = p.read_float("LifeVar", def.life_var),
            "Velocity" => def.velocity = p.read_vec3("Velocity", def.velocity),
            "VelocityVar" => def.velocity_var = p.read_vec3("VelocityVar", def.velocity_var),
            "VelocityDamping" => {
                def.velocity_damping = p.read_vec3("VelocityDamping", def.velocity_damping)
            }
            "Gravity" => def.gravity = p.read_float("Gravity", def.gravity),
            "ColorBirth" => {
                p.next(); // string match ColorBirth
                let r = p.read_float_val(1.0);
                let g = p.read_float_val(1.0);
                let b = p.read_float_val(1.0);
                let a = p.read_float_val(1.0);
                def.color_birth = Color::srgba(r, g, b, a);
            }
            "ColorDeath" => {
                p.next(); // string match ColorDeath
                let r = p.read_float_val(1.0);
                let g = p.read_float_val(1.0);
                let b = p.read_float_val(1.0);
                let a = p.read_float_val(1.0);
                def.color_death = Color::srgba(r, g, b, a);
            }
            "Rate" => def.rate = p.read_float("Rate", def.rate),
            "BlendSet" => def.blend_set = p.read_i32("BlendSet", def.blend_set),
            "FrameRate" => def.frame_rate = p.read_float("FrameRate", def.frame_rate),
            "NumTextureTilesX" => {
                def.grid_x = p.read_i32("NumTextureTilesX", def.grid_x as i32) as u32
            }
            "NumTextureTilesY" => {
                def.grid_y = p.read_i32("NumTextureTilesY", def.grid_y as i32) as u32
            }
            "StartTextureTile" => {
                def.start_tile = p.read_i32("StartTextureTile", def.start_tile as i32) as u32
            }
            "EndTextureTile" => {
                def.end_tile = p.read_i32("EndTextureTile", def.end_tile as i32) as u32
            }
            _ => {
                p.next();
            } // skip unknown fields gracefully
        }
    }

    Some(def)
}
