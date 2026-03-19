use bevy::prelude::*;
use std::collections::HashMap;
use super::projectile::SettingsExt;
use super::settings::SettingsBlock;

#[derive(Debug, Clone)]
pub enum EffectDef {
    Sfx(SfxDef),
    Sprite(SpriteEffectDef),
    DelayedParticle(DelayedParticleDef),
    HealthIndicator(HealthIndicatorDef),
    CameraShake(CameraShakeDef),
    Lightning(LightningGeneratorDef),
    Particle(ParticleEffectDef),
    ChunkEmitter(ChunkEmitterDef),
    BulletCasing(BulletCasingFxDef), // Often found in .fx
}

#[derive(Debug, Clone)]
pub struct SfxDef {
    pub name: String,
    pub audio_package: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SpriteEffectDef {
    pub name: String,
    pub texture: Handle<Image>,
    pub color: Color,
    pub blend_set: i32,
    pub duration: f32,
    pub line_length: f32,
    pub line_width: f32,
    pub particle_size: f32,
    pub alignment: i32,
}

#[derive(Debug, Clone)]
pub struct ParticleSystemRef {
    pub system_name: String, // Maps to a .ptx file name
    pub num_initial_particles: i32,
    pub birth_rate: f32,
}

#[derive(Debug, Clone)]
pub struct DelayedParticleDef {
    pub name: String,
    pub system: ParticleSystemRef,
    pub duration: f32,
}

#[derive(Debug, Clone)]
pub struct ParticleEffectDef {
    pub name: String,
    pub system: ParticleSystemRef,
}

#[derive(Debug, Clone)]
pub struct ChunkEmitterDef {
    pub name: String,
    pub projectile_type: String, // Maps to a ProjectileDef name in rb.proj
    pub num_initial_chunks: i32,
    pub birth_rate: f32,
    pub duration: f32,
    pub initial_velocity: Vec3,
    pub velocity_var: Vec3,
}

#[derive(Debug, Clone)]
pub struct HealthIndicatorDef {
    pub name: String,
    pub system: ParticleSystemRef,
    pub duration: f32,
    pub undamaged_color: Color,
    pub mid_color: Color,
    pub dead_color: Color,
    pub mid_percentage: f32,
}

#[derive(Debug, Clone)]
pub struct CameraShakeDef {
    pub name: String,
    pub range_eulers: Vec3,
    pub time_to_shake: f32,
    pub time_left_when_start_dampening: f32,
    pub radius_max_shake: f32,
    pub radius_no_shake: f32,
}

#[derive(Debug, Clone)]
pub struct LightningGeneratorDef {
    pub name: String,
    pub start_color: Color,
    pub end_color: Color,
    pub width: f32,
    pub bolt_type: i32,
    pub lifetime: f32,
    pub birth_rate: f32,
    pub position2: Vec3,
    pub position2_var: Vec3,
}

#[derive(Debug, Clone)]
pub struct BulletCasingFxDef {
    pub name: String,
    pub projectile_type: String, // References ProjectileDef
    pub initial_velocity: Vec3,
}

pub fn parse_particle_ref(block: &SettingsBlock) -> Option<ParticleSystemRef> {
    if let Some(sys_name) = block.get_string("ParticleSystem") {
        return Some(ParticleSystemRef {
            system_name: sys_name,
            num_initial_particles: block.get_i32("NumInitialParticles", 0),
            birth_rate: block.get_f32("BirthRate", 0.0),
        });
    }
    None
}

pub fn parse_effect(def_type: &str, name: &str, block: &SettingsBlock, asset_server: &AssetServer, images: &mut Assets<Image>) -> Option<EffectDef> {
    match def_type {
        "SFX" => {
            Some(EffectDef::Sfx(SfxDef {
                name: name.to_string(),
                audio_package: block.get_string("AudioPackage"),
            }))
        }
        "SPRITEEFFECT" => {
            let tex_name = block.get_string("TextureName").unwrap_or_default();
            let tex_handle = if let Some((h, _)) = crate::oni2_loader::parsers::texture::load_tga_texture("texture", &tex_name, images) {
                h
            } else {
                asset_server.load(format!("texture/{}.tga", tex_name))
            };
            Some(EffectDef::Sprite(SpriteEffectDef {
                name: name.to_string(),
                texture: tex_handle,
                color: block.get_color("Color", Color::WHITE),
                blend_set: block.get_i32("BlendSet", 0),
                duration: block.get_f32("Duration", 0.0),
                line_length: block.get_f32("LineLength", 0.0),
                line_width: block.get_f32("LineWidth", 0.0),
                particle_size: block.get_f32("ParticleSize", 1.0),
                alignment: block.get_i32("Alignment", 0),
            }))
        }
        "DELAYEDPARTICLEEFFECT" => {
            let nested = block.children.first().or(Some(block)).unwrap();
            let system = parse_particle_ref(nested)?;
            Some(EffectDef::DelayedParticle(DelayedParticleDef {
                name: name.to_string(),
                system,
                duration: block.get_f32("Duration", 0.0),
            }))
        }
        "PARTICLEEFFECT" => {
            let system = parse_particle_ref(block)?;
            Some(EffectDef::Particle(ParticleEffectDef {
                name: name.to_string(),
                system,
            }))
        }
        "HEALTHINDICATOR" => {
            let mut top_level = block;
            if let Some(child) = block.children.first() {
                top_level = child; // Contains the particle system + duration
            }
            let nested = top_level.children.first().or(Some(top_level)).unwrap();
            let system = parse_particle_ref(nested)?;

            Some(EffectDef::HealthIndicator(HealthIndicatorDef {
                name: name.to_string(),
                system,
                duration: top_level.get_f32("Duration", 0.0),
                undamaged_color: block.get_color("UndamagedColor", Color::srgb(0.0, 1.0, 0.0)),
                mid_color: block.get_color("MidColor", Color::srgb(1.0, 1.0, 0.0)),
                dead_color: block.get_color("DeadColor", Color::srgb(1.0, 0.0, 0.0)),
                mid_percentage: block.get_f32("MidPercentage", 50.0),
            }))
        }
        "CAMERASHAKE" => {
            Some(EffectDef::CameraShake(CameraShakeDef {
                name: name.to_string(),
                range_eulers: block.get_vec3("RangeEulers", Vec3::ZERO),
                time_to_shake: block.get_f32("TimeToShake", 0.15),
                time_left_when_start_dampening: block.get_f32("TimeLeftWhenStartDampening", 0.10),
                radius_max_shake: block.get_f32("RadiusMaxShake", 2.0),
                radius_no_shake: block.get_f32("RadiusNoShake", 10.0),
            }))
        }
        "LIGHTNINGGENERATOR" => {
            let mut params = block;
            if let Some(child) = block.children.first() {
                params = child;
            }
            Some(EffectDef::Lightning(LightningGeneratorDef {
                name: name.to_string(),
                start_color: params.get_color("StartColor", Color::WHITE),
                end_color: params.get_color("EndColor", Color::BLACK),
                width: params.get_f32("Width", 0.05),
                bolt_type: params.get_i32("BoltType", 0),
                lifetime: block.get_f32("LifeTime", 0.1),
                birth_rate: block.get_f32("BirthRate", 5.0),
                position2: block.get_vec3("Position2", Vec3::ZERO),
                position2_var: block.get_vec3("Position2Var", Vec3::ZERO),
            }))
        }
        "CHUNKEMITTER" => {
            Some(EffectDef::ChunkEmitter(ChunkEmitterDef {
                name: name.to_string(),
                projectile_type: block.get_string("ProjectileType").unwrap_or_default(),
                num_initial_chunks: block.get_i32("NumInitialChunks", 0),
                birth_rate: block.get_f32("BirthRate", 1.0),
                duration: block.get_f32("Duration", 0.0),
                initial_velocity: block.get_vec3("InitialVelocity", Vec3::ZERO),
                velocity_var: block.get_vec3("InitialVelocityVar", Vec3::ZERO),
            }))
        }
        "BULLETCASING" => {
            Some(EffectDef::BulletCasing(BulletCasingFxDef {
                name: name.to_string(),
                projectile_type: block.get_string("ProjectileType").unwrap_or_default(),
                initial_velocity: block.get_vec3("InitialVelocity", Vec3::ZERO),
            }))
        }
        _ => None
    }
}
