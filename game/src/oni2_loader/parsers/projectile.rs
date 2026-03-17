use bevy::prelude::*;
use std::collections::HashMap;

use super::settings::{SettingsBlock, SettingsValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitType {
    Blunt = 12,
    Energy = 13,
    Fire = 14,
    Plasma = 15,
    Ballistic = 16,
    Unknown = 0,
}

impl From<i32> for HitType {
    fn from(val: i32) -> Self {
        match val {
            12 => HitType::Blunt,
            13 => HitType::Energy,
            14 => HitType::Fire,
            15 => HitType::Plasma,
            16 => HitType::Ballistic,
            _ => HitType::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplosionMatrix {
    Flat,
    Identity,
    Velocity,
}

impl From<&str> for ExplosionMatrix {
    fn from(s: &str) -> Self {
        match s {
            "Flat" => ExplosionMatrix::Flat,
            "Align z with velocity" => ExplosionMatrix::Velocity,
            _ => ExplosionMatrix::Identity,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DamageDef {
    pub hit_points: f32,
    pub impulse: f32,
    pub hit_type: HitType,
}

#[derive(Debug, Clone)]
pub enum ProjectileDef {
    Basic(BasicProjectileDef),
    Actor(ActorProjectileDef),
    Model(ModelProjectileDef),
    Shard(ShardDef),
    Rocket(RocketDef),
    BulletCasing(BulletCasingDef),
}

#[derive(Debug, Clone)]
pub struct BasicProjectileDef {
    pub name: String,
    pub life_time: f32,
    pub gravity_factor: f32,
    pub color: Color,
    pub flight_fx: Option<String>,
    pub explosion_fx: Option<String>,
    pub explode_on_impact: bool,
    pub explode_on_time_out: bool,
    pub explosion_matrix: ExplosionMatrix,
    pub damage: DamageDef,
}

#[derive(Debug, Clone)]
pub struct ModelProjectileDef {
    pub name: String,
    pub life_time: f32,
    pub gravity_factor: f32,
    pub color: Color,
    pub model_name: String,
    pub rotation_speed: Vec3,
    pub enable_collisions: bool,
    pub flight_fx: Option<String>,
    pub explosion_fx: Option<String>,
    pub explode_on_impact: bool,
    pub explode_on_time_out: bool,
    pub explosion_matrix: ExplosionMatrix,
    pub damage: DamageDef,
}

#[derive(Debug, Clone)]
pub struct ShardDef {
    pub name: String,
    pub life_time: f32,
    pub life_time_var: f32,
    pub gravity_factor: f32,
    pub angular_velocity: f32,
    pub size: f32,
    pub color1: Color,
    pub color2: Color,
}

#[derive(Debug, Clone)]
pub struct RocketDef {
    pub name: String,
    pub inner_def: Box<ProjectileDef>, // The inner nested projectile config
    pub model_name: String,
    pub rotation_speed: Vec3,
    pub ignition_time: f32,
    pub burn_time: f32,
    pub thrust: f32,
    pub burn_fx: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ActorProjectileDef {
    pub name: String,
    pub life_time: f32,
    pub gravity_factor: f32,
    pub mass: f32,
    pub angular_inertia: f32,
    pub entity_bound: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BulletCasingDef {
    pub name: String,
    pub projectile_type: String,
    pub initial_velocity: Vec3,
}

impl ProjectileDef {
    pub fn lifetime(&self) -> f32 {
        match self {
            Self::Basic(d) => d.life_time,
            Self::Actor(d) => d.life_time,
            Self::Model(d) => d.life_time,
            Self::Shard(d) => d.life_time,
            Self::Rocket(d) => d.inner_def.lifetime(),
            Self::BulletCasing(_) => 2.0, // Default for casings
        }
    }

    pub fn explode_on_timeout(&self) -> bool {
        match self {
            Self::Basic(d) => d.explode_on_time_out,
            Self::Model(d) => d.explode_on_time_out,
            _ => false,
        }
    }

    pub fn gravity_factor(&self) -> f32 {
        match self {
            Self::Basic(d) => d.gravity_factor,
            Self::Actor(d) => d.gravity_factor,
            Self::Model(d) => d.gravity_factor,
            Self::Shard(d) => d.gravity_factor,
            Self::Rocket(_) => 1.0, 
            Self::BulletCasing(_) => 1.0,
        }
    }

    pub fn flight_fx(&self) -> Option<String> {
        match self {
            Self::Basic(d) => d.flight_fx.clone(),
            Self::Model(d) => d.flight_fx.clone(),
            Self::Rocket(d) => d.burn_fx.clone(),
            _ => None,
        }
    }

    pub fn explode_fx(&self) -> Option<String> {
        match self {
            Self::Basic(d) => d.explosion_fx.clone(),
            Self::Model(d) => d.explosion_fx.clone(),
            _ => None,
        }
    }

    pub fn proj_class(&self) -> &'static str {
        match self {
            Self::Basic(_) => "BasicProjectile",
            Self::Actor(_) => "ActorProjectile",
            Self::Model(_) => "ModelProjectile",
            Self::Shard(_) => "Shard",
            Self::Rocket(_) => "Rocket",
            Self::BulletCasing(_) => "BulletCasing",
        }
    }
}

// Helper to extract properties cleanly
pub trait SettingsExt {
    fn get_f32(&self, key: &str, default: f32) -> f32;
    fn get_i32(&self, key: &str, default: i32) -> i32;
    fn get_bool(&self, key: &str, default: bool) -> bool;
    fn get_string(&self, key: &str) -> Option<String>;
    fn get_color(&self, key: &str, default: Color) -> Color;
    fn get_vec3(&self, key: &str, default: Vec3) -> Vec3;
    fn get_block(&self, key: &str) -> Option<&SettingsBlock>;
}

impl SettingsExt for SettingsBlock {
    fn get_f32(&self, key: &str, default: f32) -> f32 {
        match self.properties.get(key) {
            Some(SettingsValue::Float(f)) => *f,
            Some(SettingsValue::Int(i)) => *i as f32,
            _ => default,
        }
    }

    fn get_i32(&self, key: &str, default: i32) -> i32 {
        match self.properties.get(key) {
            Some(SettingsValue::Int(i)) => *i,
            Some(SettingsValue::Float(f)) => *f as i32,
            _ => default,
        }
    }

    fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.properties.get(key) {
            Some(SettingsValue::Int(i)) => *i != 0,
            _ => default,
        }
    }

    fn get_string(&self, key: &str) -> Option<String> {
        match self.properties.get(key) {
            Some(SettingsValue::String(s)) => Some(s.clone()),
            _ => None,
        }
    }

    fn get_color(&self, key: &str, default: Color) -> Color {
        match self.properties.get(key) {
            Some(SettingsValue::FloatArray(arr)) if arr.len() >= 3 => {
                let a = if arr.len() >= 4 && arr[3] > 0.0 { arr[3] } else { 1.0 }; // Assume opacity entirely driven by fx if alpha=0
                Color::srgba(arr[0], arr[1], arr[2], a)
            }
            _ => default,
        }
    }

    fn get_vec3(&self, key: &str, default: Vec3) -> Vec3 {
        match self.properties.get(key) {
            Some(SettingsValue::FloatArray(arr)) if arr.len() >= 3 => {
                Vec3::new(arr[0], arr[1], arr[2])
            }
            _ => default,
        }
    }

    fn get_block(&self, key: &str) -> Option<&SettingsBlock> {
        match self.properties.get(key) {
            Some(SettingsValue::Block(b)) => Some(b),
            _ => None,
        }
    }
}

pub fn parse_projectile(def_type: &str, name: &str, block: &SettingsBlock, asset_server: &AssetServer) -> Option<ProjectileDef> {
    let damage_block = block.get_block("Damage");
    let damage = if let Some(db) = damage_block {
        DamageDef {
            hit_points: db.get_f32("HitPoints", 0.0),
            impulse: db.get_f32("Impulse", 0.0),
            hit_type: HitType::from(db.get_i32("HitType", 0)),
        }
    } else {
        DamageDef { hit_points: 0.0, impulse: 0.0, hit_type: HitType::Unknown }
    };

    match def_type {
        "BASICPROJECTILE" => {
            Some(ProjectileDef::Basic(BasicProjectileDef {
                name: name.to_string(),
                life_time: block.get_f32("LifeTime", 1.0),
                gravity_factor: block.get_f32("GravityFactor", 1.0),
                color: block.get_color("Color", Color::WHITE),
                flight_fx: block.get_string("FlightFX"),
                explosion_fx: block.get_string("Explosion").or_else(|| block.get_string("ImpactEvent")),
                explode_on_impact: block.get_bool("ExplodeOnImpact", true), // default to true if missing usually
                explode_on_time_out: block.get_bool("ExplodeOnTimeOut", true),
                explosion_matrix: ExplosionMatrix::from(block.get_string("ExplosionMatrix").unwrap_or_default().as_str()),
                damage,
            }))
        }
        "MODELPROJECTILE" => {
            let model_name = block.get_string("ModelName").unwrap_or_default();
            // Oni2 model names refer to Entity folders. Store the name to spawn later.
            // TODO: make sure the Oni2EntityType is loaded for this model_name immediately right here
            
            // Model configs actually nest the basic projectile params inside an anonymous block `{...}`
            // We'll search for it if we see one.
            let mut params_block = block;
            if let Some(child) = block.children.first() {
                params_block = child; // Inner block holds LifeTime, GravityFactor, Color, Damage, etc.
            }
            
            let nested_damage_block = params_block.get_block("Damage");
            let nested_damage = if let Some(db) = nested_damage_block {
                DamageDef {
                    hit_points: db.get_f32("HitPoints", 0.0),
                    impulse: db.get_f32("Impulse", 0.0),
                    hit_type: HitType::from(db.get_i32("HitType", 0)),
                }
            } else {
                damage
            };

            Some(ProjectileDef::Model(ModelProjectileDef {
                name: name.to_string(),
                model_name,
                rotation_speed: block.get_vec3("RotationSpeed", Vec3::ZERO),
                enable_collisions: params_block.get_bool("EnableCollisions", true),
                life_time: params_block.get_f32("LifeTime", 1.0),
                gravity_factor: params_block.get_f32("GravityFactor", 1.0),
                color: params_block.get_color("Color", Color::WHITE),
                flight_fx: params_block.get_string("FlightFX"),
                explosion_fx: params_block.get_string("Explosion").or_else(|| params_block.get_string("ImpactEvent")),
                explode_on_impact: params_block.get_bool("ExplodeOnImpact", false), 
                explode_on_time_out: params_block.get_bool("ExplodeOnTimeOut", false),
                explosion_matrix: ExplosionMatrix::from(params_block.get_string("ExplosionMatrix").unwrap_or_default().as_str()),
                damage: nested_damage,
            }))
        }
        "SHARD" => {
            Some(ProjectileDef::Shard(ShardDef {
                name: name.to_string(),
                life_time: block.get_f32("LifeTime", 1.0),
                life_time_var: block.get_f32("LifeTimeVar", 0.0),
                gravity_factor: block.get_f32("GravityFactor", 1.0),
                angular_velocity: block.get_f32("AngularVelocity", 0.0),
                size: block.get_f32("Size", 1.0),
                color1: block.get_color("Color1", Color::WHITE),
                color2: block.get_color("Color2", Color::WHITE),
            }))
        }
        "ROCKET" => {
            // Nested project config inside anonymous block again
            let mut params_block = block;
            if let Some(child) = block.children.first() {
                params_block = child;
            }
            // And its nested basic config:
            let mut base_block = params_block;
            if let Some(child) = params_block.children.first() {
                base_block = child; 
            }

            let nested_damage_block = base_block.get_block("Damage");
            let nested_damage = if let Some(db) = nested_damage_block {
                DamageDef {
                    hit_points: db.get_f32("HitPoints", 0.0),
                    impulse: db.get_f32("Impulse", 0.0),
                    hit_type: HitType::from(db.get_i32("HitType", 0)),
                }
            } else {
                damage
            };

            let inner = Box::new(ProjectileDef::Basic(BasicProjectileDef {
                name: format!("{}_inner", name),
                life_time: base_block.get_f32("LifeTime", 1.0),
                gravity_factor: base_block.get_f32("GravityFactor", 1.0),
                color: base_block.get_color("Color", Color::WHITE),
                flight_fx: None,
                explosion_fx: base_block.get_string("Explosion").or_else(|| base_block.get_string("ImpactEvent")),
                explode_on_impact: base_block.get_bool("ExplodeOnImpact", true),
                explode_on_time_out: base_block.get_bool("ExplodeOnTimeOut", true),
                explosion_matrix: ExplosionMatrix::from(base_block.get_string("ExplosionMatrix").unwrap_or_default().as_str()),
                damage: nested_damage,
            }));

            let model_name = params_block.get_string("ModelName").unwrap_or_default();

            Some(ProjectileDef::Rocket(RocketDef {
                name: name.to_string(),
                inner_def: inner,
                model_name,
                rotation_speed: params_block.get_vec3("RotationSpeed", Vec3::ZERO),
                ignition_time: block.get_f32("IgnitionTime", 0.0),
                burn_time: block.get_f32("BurnTime", 1.0),
                thrust: block.get_f32("Thrust", 10.0),
                burn_fx: block.get_string("BurnFX"),
            }))
        }
        "ACTORPROJECTILE" => {
            let mut base_block = block;
            if let Some(child) = block.children.first() {
                base_block = child;
            }

            Some(ProjectileDef::Actor(ActorProjectileDef {
                name: name.to_string(),
                mass: block.get_f32("Mass", 1.0),
                angular_inertia: block.get_f32("AngularInertia", 1.0),
                entity_bound: block.get_string("EntityBound"),
                life_time: base_block.get_f32("LifeTime", 3.0),
                gravity_factor: base_block.get_f32("GravityFactor", 1.0),
            }))
        }
        "BULLETCASING" => {
            Some(ProjectileDef::BulletCasing(BulletCasingDef {
                name: name.to_string(),
                projectile_type: block.get_string("ProjectileType").unwrap_or_default(),
                initial_velocity: block.get_vec3("InitialVelocity", Vec3::ZERO),
            }))
        }
        _ => None
    }
}
