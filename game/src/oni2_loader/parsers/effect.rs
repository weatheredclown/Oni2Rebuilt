/*
 * oni2_loader/parsers/effect.rs — visual effect definition parser.
 *
 * EffectDef enum: Sfx (sound), Sprite (billboard), DelayedParticle, etc.
 * parse_effect: reads a .effect settings block and returns an EffectDef used by
 * FxLibrary and spawned at runtime via SpawnFx events.
 */
use super::projectile::SettingsExt;
use super::settings::SettingsBlock;
use bevy::prelude::*;

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
    Laser(LaserFxDef),
    BlastFire(BlastFireDef),
    LightGlow(LightGlowDef),
    Charge(ChargeDef),
    Flash(FlashDef),
    Strike(StrikeFxDef),
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

#[derive(Debug, Clone)]
pub struct LaserFxDef {
    pub name: String,
    /// Beam cross-section width in world units (meters).
    pub width: f32,
    /// Head sprite scale multiplier (applied as `0.5 * width * head_scale`
    /// per rb/src/fx/laser.cpp:280).
    pub head_scale: f32,
    /// Size of the ring-buffer of past positions — how many frames of tail
    /// are retained.  C++ `Length` (int) at rb/src/fx/laser.cpp:43.  NOT a
    /// spatial length; the beam's visible length depends on projectile
    /// speed × `length` × frame time.
    pub length: usize,
    pub head_texture: String,
    pub tail_texture: String,
    pub head_texture_handle: Option<Handle<Image>>,
    pub tail_texture_handle: Option<Handle<Image>>,
    /// Max point-light intensity at the head (raw C++ value — used to
    /// drive the `intensity *= 2` ramp in laser.rs).
    pub light_max: f32,
    pub light_color: Color,
}

#[derive(Debug, Clone)]
pub struct BlastFireDef {
    pub name: String,
    pub speed: f32,
    pub rotation: Vec3,
    pub scale: Vec3,
    pub path0: Vec3,
    pub path1: Vec3,
    pub path2: Vec3,
}

/// Mirrors legacy `fxLightGlow` (rb/src/fx/lightglows.{cpp,h}).  A
/// named billboard-corona fx-effect-type referenced by Light entries
/// in `layout.lights` (when they include the `fxLight` flag) and by
/// per-effect `LIGHTGLOW` blocks in `rb.fx`.  Fields map 1:1 to the
/// flag bits + intensity values legacy reads in
/// `fxLightGlow::Load` (lightglows.cpp:245+).
#[derive(Debug, Clone)]
pub struct LightGlowDef {
    pub name: String,
    /// Name of the texture file under `texture/` (no extension).
    pub glow_texture_name: String,
    /// Pre-loaded handle for the glow texture.  Eagerly resolved at
    /// parse time so spawn-time costs are zero.  `None` if the
    /// texture didn't load (then we'll skip rendering this glow).
    pub glow_texture_handle: Option<Handle<Image>>,
    /// `kGlowLookAt` flag — if 1, orient the quad toward the camera
    /// using the camera's full orientation matrix (full screen-space
    /// billboard).  Mutually overridden by `glow_billboard` below.
    pub glow_look_at: i32,
    /// `kGlowBillboard` flag — if 1, Y-axis billboard: face the camera
    /// horizontally (rotate around world Y) but keep "up" pointing at
    /// world Y.  Cylindrical billboard.  Takes priority over
    /// `glow_look_at` (legacy lightglows.cpp:166).
    pub glow_billboard: i32,
    /// `kOccludeGlow` flag — if 1, ray-cast from glow to camera.  When
    /// blocked by physical geometry, fade the glow alpha down (at
    /// `glow_intensity_rate_of_change` speed) toward zero; when
    /// unblocked, fade back up toward `glow_intensity`.  Also disables
    /// depth-test so the glow renders over occluders during the fade
    /// (legacy lightglows.cpp:198).
    pub occlude_glow: i32,
    /// `kRadialAttenuate` flag — if 1, scale glowsize by
    /// `dot(-light_dir, lightToCam)`, i.e. fade out as the camera
    /// moves outside the light's forward cone.  Used for spot-style
    /// lights where the glow should only be visible from in front.
    pub radial_attenuate: i32,
    /// `kScreenRotate` flag — if 1, rotate the quad around its local Z
    /// based on the light's screen-space angle (atan2 of projected XY).
    /// Used for rotational lens-flare effects.
    pub screen_rotate: i32,
    /// Maximum glow size (also acts as max alpha when occluding).  In
    /// world units — the quad spans `[-glow_intensity, +glow_intensity]`
    /// on each axis (so a value of 10 → a 20-unit-wide quad).
    pub glow_intensity: f32,
    /// Speed of intensity change toward target each frame, scaled by
    /// `glow_intensity` (legacy lightglows.cpp:130).  Only meaningful
    /// when `occlude_glow` is enabled.
    pub glow_intensity_rate_of_change: f32,
    /// Color tint applied to the texture sample.  Alpha is multiplied
    /// by the per-frame fade (when occluding).
    pub glow_color: Color,
}

#[derive(Debug, Clone)]
pub struct ChargeDef {
    pub name: String,
    pub scale: f32,
    pub color1: Color,
    pub color2: Color,
    pub color3: Color,
    pub amplitude: f32,
    pub frequency: f32,
    pub rate: f32,
    pub offset: f32,
}

#[derive(Debug, Clone)]
pub struct FlashDef {
    pub name: String,
    pub duration: f32,
    pub rate: f32,
    pub fade: f32,
    pub scale_min: f32,
    pub scale_max: f32,
    pub color1: Color,
    pub color2: Color,
    pub color3: Color,
}

#[derive(Debug, Clone)]
pub struct StrikeFxDef {
    pub name: String,
    pub exponent: f32,
    pub jumble: f32,
    pub spike_scale: f32,
    pub fade_start: f32,
    pub wave_count: i32,
    pub color1a: Color,
    pub color2a: Color,
    pub color3a: Color,
    pub color1b: Color,
    pub color2b: Color,
    pub color3b: Color,
    pub tile: Vec2,
    pub slide_rate: Vec2,
    pub scale_rate: f32,
    pub fade_duration: f32,
    pub duration: f32,
    pub position: Vec3,
    pub texture_name: String,
    pub texture_handle: Option<Handle<Image>>,
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

pub fn parse_effect(
    def_type: &str,
    name: &str,
    block: &SettingsBlock,
    asset_server: &AssetServer,
    images: &mut Assets<Image>,
) -> Option<EffectDef> {
    match def_type {
        "SFX" => Some(EffectDef::Sfx(SfxDef {
            name: name.to_string(),
            audio_package: block.get_string("AudioPackage"),
        })),
        "SPRITEEFFECT" => {
            let tex_name = block.get_string("TextureName").unwrap_or_default();
            let tex_handle = if let Some((h, _)) =
                crate::oni2_loader::parsers::texture::load_tga_texture("texture", &tex_name, images)
            {
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
            let nested = block.children.first().unwrap_or(block);
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
            let nested = top_level.children.first().unwrap_or(top_level);
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
        "CAMERASHAKE" => Some(EffectDef::CameraShake(CameraShakeDef {
            name: name.to_string(),
            range_eulers: block.get_vec3("RangeEulers", Vec3::ZERO),
            time_to_shake: block.get_f32("TimeToShake", 0.15),
            time_left_when_start_dampening: block.get_f32("TimeLeftWhenStartDampening", 0.10),
            radius_max_shake: block.get_f32("RadiusMaxShake", 2.0),
            radius_no_shake: block.get_f32("RadiusNoShake", 10.0),
        })),
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
        "CHUNKEMITTER" => Some(EffectDef::ChunkEmitter(ChunkEmitterDef {
            name: name.to_string(),
            projectile_type: block.get_string("ProjectileType").unwrap_or_default(),
            num_initial_chunks: block.get_i32("NumInitialChunks", 0),
            birth_rate: block.get_f32("BirthRate", 1.0),
            duration: block.get_f32("Duration", 0.0),
            initial_velocity: block.get_vec3("InitialVelocity", Vec3::ZERO),
            velocity_var: block.get_vec3("InitialVelocityVar", Vec3::ZERO),
        })),
        "BULLETCASING" => Some(EffectDef::BulletCasing(BulletCasingFxDef {
            name: name.to_string(),
            projectile_type: block.get_string("ProjectileType").unwrap_or_default(),
            initial_velocity: block.get_vec3("InitialVelocity", Vec3::ZERO),
        })),
        "LASER" => {
            // Legacy laser textures live under Entity/GunFx (see
            // rb/src/fx/laser.cpp:140 `RBPushFolder("entity/gunfx")`).
            // Fall back to the generic texture/ tree if not found there.
            let head_name = block.get_string("HeadTexture").unwrap_or_default();
            let tail_name = block.get_string("TailTexture").unwrap_or_default();
            let head_handle = if head_name.is_empty() || head_name.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(crate::oni2_loader::parsers::texture::load_tga_texture(
                    "Entity/GunFx",
                    &head_name,
                    images,
                )
                .map(|(h, _)| h)
                .unwrap_or_else(|| asset_server.load(format!("texture/{}.tga", head_name))))
            };
            
            let tail_handle = if tail_name.is_empty() || tail_name.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(crate::oni2_loader::parsers::texture::load_tga_texture(
                    "Entity/GunFx",
                    &tail_name,
                    images,
                )
                .map(|(h, _)| h)
                .unwrap_or_else(|| asset_server.load(format!("texture/{}.tga", tail_name))))
            };
            Some(EffectDef::Laser(LaserFxDef {
                name: name.to_string(),
                width: block.get_f32("Width", 1.0),
                head_scale: block.get_f32("HeadScale", 1.0),
                length: block.get_f32("Length", 16.0).max(2.0) as usize,
                head_texture: head_name,
                tail_texture: tail_name,
                head_texture_handle: head_handle,
                tail_texture_handle: tail_handle,
                light_max: block.get_f32("LightMax", 32.0),
                light_color: block.get_color("LightColor", Color::WHITE),
            }))
        }
        "BLASTFIRE" => Some(EffectDef::BlastFire(BlastFireDef {
            name: name.to_string(),
            speed: block.get_f32("Speed", 1.0),
            rotation: block.get_vec3("Rotation", Vec3::ZERO),
            scale: block.get_vec3("Scale", Vec3::ONE),
            path0: block.get_vec3("path0", Vec3::ZERO),
            path1: block.get_vec3("path1", Vec3::ZERO),
            path2: block.get_vec3("path2", Vec3::ZERO),
        })),
        "LIGHTGLOW" => {
            let glow_texture_name = block.get_string("GlowTextureName").unwrap_or_default();
            // Eagerly load the texture (mirrors SPRITEEFFECT path
            // above) so spawn-time of fxLight glows is asset-free.
            let glow_texture_handle = if glow_texture_name.is_empty() {
                None
            } else if let Some((h, _)) = crate::oni2_loader::parsers::texture::load_tga_texture(
                "texture",
                &glow_texture_name,
                images,
            ) {
                Some(h)
            } else {
                Some(asset_server.load(format!("texture/{}.tga", glow_texture_name)))
            };
            Some(EffectDef::LightGlow(LightGlowDef {
                name: name.to_string(),
                glow_texture_name,
                glow_texture_handle,
                glow_look_at: block.get_i32("GlowLookAt", 0),
                glow_billboard: block.get_i32("GlowBillboard", 0),
                occlude_glow: block.get_i32("OccludeGlow", 0),
                radial_attenuate: block.get_i32("RadialAttenuate", 0),
                screen_rotate: block.get_i32("ScreenRotate", 0),
                glow_intensity: block.get_f32("GlowIntensity", 1.0),
                glow_intensity_rate_of_change: block.get_f32("GlowIntensityRateOfChange", 0.0),
                glow_color: block.get_color("GlowColor", Color::WHITE),
            }))
        }
        "CHARGE" => Some(EffectDef::Charge(ChargeDef {
            name: name.to_string(),
            scale: block.get_f32("Scale", 1.0),
            color1: block.get_color("Color1", Color::WHITE),
            color2: block.get_color("Color2", Color::WHITE),
            color3: block.get_color("Color3", Color::WHITE),
            amplitude: block.get_f32("Amplitude", 0.5),
            frequency: block.get_f32("Frequency", 0.5),
            rate: block.get_f32("Rate", 1.0),
            offset: block.get_f32("Offset", 0.0),
        })),
        "FLASH" => Some(EffectDef::Flash(FlashDef {
            name: name.to_string(),
            duration: block.get_f32("Duration", 1.0),
            rate: block.get_f32("Rate", 1.0),
            fade: block.get_f32("Fade", 0.5),
            scale_min: block.get_f32("ScaleMin", 0.8),
            scale_max: block.get_f32("ScaleMax", 1.2),
            color1: block.get_color("Color1", Color::WHITE),
            color2: block.get_color("Color2", Color::WHITE),
            color3: block.get_color("Color3", Color::WHITE),
        })),
        "STRIKE" => {
            let tex_name = block.get_string("TextureName").unwrap_or_else(|| "bam".to_string());
            let tex_handle = if tex_name.is_empty() || tex_name.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(crate::oni2_loader::parsers::texture::load_tga_texture(
                    "entity/strikefx",
                    &tex_name,
                    images,
                )
                .map(|(h, _)| h)
                .unwrap_or_else(|| asset_server.load(format!("texture/{}.tga", tex_name))))
            };

            Some(EffectDef::Strike(StrikeFxDef {
                name: name.to_string(),
                exponent: block.get_f32("Exponent", 1.0),
                jumble: block.get_f32("Jumble", 0.0),
                spike_scale: block.get_f32("SpikeScale", 0.4),
                fade_start: block.get_f32("FadeStart", 0.7),
                wave_count: block.get_i32("WaveCount", 10),
                color1a: block.get_color("Color1a", Color::srgb(0.0, 0.0, 1.0)),
                color2a: block.get_color("Color2a", Color::srgb(1.0, 0.5, 0.0)),
                color3a: block.get_color("Color3a", Color::srgb(1.0, 1.0, 1.0)),
                color1b: block.get_color("Color1b", Color::srgb(0.0, 0.0, 1.0)),
                color2b: block.get_color("Color2b", Color::srgb(1.0, 0.5, 0.0)),
                color3b: block.get_color("Color3b", Color::srgb(1.0, 1.0, 1.0)),
                tile: block.get_vec2("Tile", Vec2::new(1.0, 1.0)),
                slide_rate: block.get_vec2("SlideRate", Vec2::new(0.2, -1.25)),
                scale_rate: block.get_f32("ScaleRate", 1.0),
                fade_duration: block.get_f32("FadeDuration", 0.2),
                duration: block.get_f32("Duration", 0.65),
                position: block.get_vec3("Position", Vec3::ZERO),
                texture_name: tex_name,
                texture_handle: tex_handle,
            }))
        }
        _ => None,
    }
}
