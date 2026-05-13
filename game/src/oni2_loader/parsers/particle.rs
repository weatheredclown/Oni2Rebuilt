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
    pub radius_birth: Vec2, // x = width, y = height
    pub radius_death_percent: f32,
    pub life: f32,
    pub life_var: f32,
    pub velocity: Vec3,
    pub velocity_var: Vec3,
    pub velocity_damping: Vec3,
    pub gravity: f32,
    pub color_birth: Color,
    pub color_death: Color,
    pub color_ramp: bool,
    pub rate: f32,
    pub blend_set: i32,
    pub system_type: i32,
    pub rotate_ptx: bool,
    pub rotate_speed: f32,
    pub relative_to_parent: bool,
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
        radius_death_percent: 1.0,
        life: 1.0,
        life_var: 0.0,
        velocity: Vec3::ZERO,
        velocity_var: Vec3::ZERO,
        velocity_damping: Vec3::ZERO,
        gravity: 0.0,
        color_birth: Color::WHITE,
        color_death: Color::WHITE,
        color_ramp: false,
        rate: 1.0,
        blend_set: 0,
        system_type: 0,
        rotate_ptx: false,
        rotate_speed: 5.0,
        relative_to_parent: false,
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
                    if let Some((h, _)) = crate::oni2_loader::parsers::texture::load_texture(
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
            "RadiusDeathPercent" => {
                def.radius_death_percent = p.read_float("RadiusDeathPercent", def.radius_death_percent)
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
            "ColorRamp" => def.color_ramp = p.read_i32("ColorRamp", if def.color_ramp { 1 } else { 0 }) != 0,
            "Rate" => def.rate = p.read_float("Rate", def.rate),
            "BlendSet" => def.blend_set = p.read_i32("BlendSet", def.blend_set),
            "Type" => def.system_type = p.read_i32("Type", def.system_type),
            "ActivateRotate" | "RotatePtx" => {
                p.next();
                if let Some(v_str) = p.tokens.next() {
                    def.rotate_ptx = v_str.parse::<i32>().unwrap_or(0) != 0;
                }
            }
            "RotateSpeed" => def.rotate_speed = p.read_float("RotateSpeed", def.rotate_speed),
            "RelativeToParentMatrix" => {
                p.next();
                if let Some(v_str) = p.tokens.next() {
                    def.relative_to_parent = v_str.parse::<i32>().unwrap_or(0) != 0;
                }
            }
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

    if def.system_type == 2 {
        warn!(
            "ParticleSystem '{}' uses SWARM behavior (Type 2) which is not yet fully implemented.",
            def.name
        );
        // TODO: SWARM particles (Type 2) use sine/cosine oscillation around a base position.
        // A potential approach in Hanabi would be to use a custom Expr update modifier:
        // 1. Store the initial un-oscillated position in a custom Attribute (e.g., Attribute::POSITION_BASE).
        // 2. Add an update modifier that computes: POSITION = POSITION_BASE + sin(AGE * Freq) * Amplitude.
        // 3. This would require parsing Freq/FreqVar and adding them to the Expr context.
    }

    Some(def)
}
