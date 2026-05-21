/*
 * fx_system.rs — FxPlugin: visual effect and particle spawning.
 *
 * SpawnFx and SpawnPtx events trigger entity creation for named effects from
 * FxLibrary / ParticleLibrary.  Handles Hanabi GPU particle systems, screen-space
 * sprites, and delayed effects.  Receives ScrOniSysEvent::PlayFx from the
 * scripting VM via the observer pipeline.
 */
use bevy::prelude::*;
use bevy_hanabi::prelude::*;

use crate::oni2_loader::components::ActorFxType;
use crate::oni2_loader::registries::{FxLibrary, ParticleLibrary, try_load_ptx};

#[derive(Event, Debug, Clone)]
pub struct SpawnFx {
    pub name: String,
    pub at: Option<Vec3>,
    pub parent: Option<Entity>,
    pub start_active: bool,
}

#[derive(Event, Debug, Clone)]
pub struct SpawnPtx {
    pub ptx_name: String,
    pub rate: f32, // BirthRate
    pub num_particles: i32,
    pub at: Option<Vec3>, // Offset
    pub parent: Option<Entity>,
    pub start_active: bool,
}

/// Request playback of a named sound through the audio pipeline.  Routes
/// through the audio-package → TD manifest → HD/BD decode chain, ultimately
/// spawning a Bevy `AudioPlayer` entity.  Owned by the FX system so it can
/// be triggered by any subsystem (combat hits, explosions, scroni scripts,
/// FX tables) without pulling in scripting-layer concepts.
///
/// * `script_entity` — the entity the sound is associated with (used for 3D
///   audio positioning once spatial audio lands; currently informational).
/// * `actor` — optional actor context for per-actor sound packages; forwarded
///   to the lookup but not yet consulted by the default dispatcher.
/// * `name` — audio-package name OR direct TD manifest entry; package names
///   win if they resolve (they randomize nugget / volume / pitch).
#[derive(Event, Debug, Clone)]
pub struct PlaySound {
    pub script_entity: Entity,
    pub actor: Option<String>,
    pub name: String,
}

#[derive(Component)]
pub struct FxStartInactive;

/// Component storing intended active state for FX.
#[derive(Component)]
pub struct IntendedFxState(pub bool);

pub struct FxPlugin;

impl Plugin for FxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HanabiPlugin)
            .init_resource::<crate::blast_fx::BlastFxAssets>()
            .add_observer(handle_spawn_fx)
            .add_observer(handle_spawn_ptx)
            .add_observer(handle_fx_action)
            .add_observer(handle_play_sound)
            .add_systems(Startup, crate::blast_fx::load_blast_fx_assets)
            .add_systems(Update, handle_actor_fx_attachments)
            .add_systems(Update, apply_fx_start_inactive)
            .add_systems(Update, sync_intended_fx_state)
            .add_systems(Update, uv_animator_system)
            .add_systems(Update, crate::blast_fx::update_blast_fire_system);
    }
}

// ---------------------------------------------------------------------------
// PlaySound dispatch
// ---------------------------------------------------------------------------

/// Observer for `PlaySound` events.  Resolves the name against the loaded
/// audio packages + TD manifest directory, decodes PSX ADPCM out of the
/// HD/BD bank pair, and spawns a Bevy `AudioPlayer`.  Called by combat,
/// explosion, scroni script dispatch, and the fight FX table.
fn handle_play_sound(
    trigger: On<PlaySound>,
    mut commands: Commands,
    mut audio_sources: ResMut<Assets<bevy::audio::AudioSource>>,
    mut td_directory: Local<Option<crate::oni2_loader::parsers::td::SoundBankDirectory>>,
    mut audio_packages: Local<
        Option<crate::oni2_loader::parsers::audiopackages::AudioPackagesDirectory>,
    >,
) {
    // Lazy-load the audio directories on first event.  Mirrors the init
    // behaviour that previously lived in scroni::system_bindings.
    if td_directory.is_none() {
        *td_directory = Some(crate::oni2_loader::parsers::td::load_all_tds());
    }
    if audio_packages.is_none() {
        if let Ok(content) = crate::vfs::read_to_string("Audio", "rb.audiopackages") {
            *audio_packages =
                Some(crate::oni2_loader::parsers::audiopackages::parse_audiopackages(&content));
        } else {
            let assets_path = crate::get_assets_path();
            let pkgs_path = std::path::Path::new(assets_path)
                .join("Audio")
                .join("rb.audiopackages");
            if let Ok(content) = std::fs::read_to_string(&pkgs_path) {
                *audio_packages =
                    Some(crate::oni2_loader::parsers::audiopackages::parse_audiopackages(&content));
            } else {
                warn!("Failed to load Audio/rb.audiopackages, audio package lookups will fail.");
                *audio_packages = Some(std::collections::HashMap::new());
            }
        }
    }

    let ev = (*trigger).clone();
    let mut resolved_name = ev.name.clone();
    let mut final_volume = 1.0;
    let mut final_pitch = 1.0;

    // Audio package: optionally randomize among nuggets.
    if let Some(pkgs) = audio_packages.as_ref() {
        if let Some((_, pkg)) = pkgs.iter().find(|(k, _)| k.eq_ignore_ascii_case(&ev.name)) {
            if !pkg.nuggets.is_empty() {
                use rand::Rng;
                let mut rng = rand::rng();
                let idx = rng.random_range(0..pkg.nuggets.len());
                let nugget = &pkg.nuggets[idx];
                resolved_name = nugget.sound.clone();
                final_volume = nugget.volume
                    * rng.random_range(nugget.random_min_volume..=nugget.random_max_volume);
                final_pitch = nugget.pitch
                    * rng.random_range(nugget.random_min_pitch..=nugget.random_max_pitch);
            } else {
                warn!("Audio package `{}` found, but it has no nuggets.", ev.name);
            }
        } else {
            warn!(
                "Audio package `{}` not found in rb.audiopackages (falling back to direct .td lookup)",
                ev.name
            );
        }
    }

    // TD manifest: locate bank + vag index, decode and spawn.
    let Some(dir) = td_directory.as_ref() else {
        return;
    };
    let Some((_, v)) = dir
        .sounds
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&resolved_name))
    else {
        if resolved_name != ev.name {
            warn!(
                "Sound `{}` (resolved from package `{}`) not found in .td manifest directory.",
                resolved_name, ev.name
            );
        } else {
            warn!("Sound `{}` not found in .td manifest directory.", ev.name);
        }
        return;
    };

    let bank_name = &v.0;
    let vag_index = v.1;
    let hd_name = format!("{}.hd", bank_name);
    let bd_name = format!("{}.bd", bank_name);

    let hd_bytes_opt = crate::vfs::read("", &hd_name).ok();
    let Some(hd_bytes) = hd_bytes_opt else {
        warn!("HD file not found in VFS: {}", hd_name);
        return;
    };

    let Ok(header) = crate::oni2_loader::parsers::hd_bd::parse_hd(&hd_bytes) else {
        warn!("Failed to parse HD header for {}", hd_name);
        return;
    };

    // HD subsongs are 1-indexed; TD vag_index is 0-indexed.
    let target_index = vag_index + 1;
    let Some(subsong) = header.subsongs.iter().find(|s| s.index == target_index) else {
        warn!("Subsong {} not found in HD header.", target_index);
        return;
    };

    let bd_paths = [bd_name.clone(), format!("Audio/banks/{}", bd_name)];
    let mut bd_bytes_opt = None;
    for p in &bd_paths {
        if let Ok(b) = crate::vfs::read("", p) {
            bd_bytes_opt = Some(b);
            break;
        }
    }
    let Some(bd_bytes) = bd_bytes_opt else {
        warn!("BD file not found in VFS: {}", bd_name);
        return;
    };

    let start = subsong.stream_offset as usize;
    let end = start + subsong.stream_size as usize;
    if end > bd_bytes.len() {
        warn!("Subsong payload overflows BD file.");
        return;
    }

    let payload = &bd_bytes[start..end];
    let Ok(pcm) =
        crate::oni2_loader::parsers::hd_bd::decode_psx_adpcm(payload, subsong.num_samples)
    else {
        return;
    };
    let Ok(wav) = crate::oni2_loader::parsers::hd_bd::create_wav_bytes(
        &pcm,
        subsong.sample_rate,
        subsong.channels,
    ) else {
        return;
    };

    let source_handle = audio_sources.add(bevy::audio::AudioSource {
        bytes: std::sync::Arc::from(wav),
    });

    commands.spawn((
        bevy::audio::AudioPlayer(source_handle),
        bevy::audio::PlaybackSettings {
            mode: if subsong.loop_flag {
                bevy::audio::PlaybackMode::Loop
            } else {
                bevy::audio::PlaybackMode::Despawn
            },
            volume: bevy::audio::Volume::Linear(final_volume),
            speed: final_pitch,
            ..Default::default()
        },
        // Loop-mode audio doesn't auto-despawn, and even Despawn-
        // mode one-shots can outlive a layout if the layout exits
        // mid-playback.  Tagging both ensures cleanup_game catches
        // them on InGame exit.
        crate::menu::InGameEntity,
    ));

    // `ev.script_entity` / `ev.actor` are currently informational — consumed
    // once 3D audio / per-actor audio-attachment lands.
    let _ = ev.script_entity;
    let _ = ev.actor;
}

fn apply_fx_start_inactive(
    mut commands: Commands,
    mut q: Query<(Entity, &mut EffectSpawner), (Added<EffectSpawner>, With<FxStartInactive>)>,
) {
    for (entity, mut spawner) in &mut q {
        spawner.active = false;
        commands.entity(entity).remove::<FxStartInactive>();
        // Explicitly clear initial reset if it tries to spawn once
        spawner.reset();
    }
}

/// System for animating mesh UV coordinates dynamically.
/// FUTURE ARCHITECTURE NOTE:
/// Modifying `ResMut<Assets<Mesh>>` applies the UV transformation globally to ALL entities sharing
/// this mesh buffer. For environmental geometry (like ambient rising steam/fire instances), this natively
/// synchronizes the scrolling while remaining extremely fast (CPU only visits the vertices once per level).
/// However, if future designs require isolated, parameterized rendering per-instance (e.g. a character
/// walking through steam causing localized warping), these static mesh buffers should be decoupled per-entity,
/// or ideally offloaded into a custom Bevy `MaterialExtension` shader where UV offsets are passed as
/// lightweight instance Uniforms directly to the GPU instead of mutating native float vertices on the CPU.
pub fn uv_animator_system(
    time: Res<Time>,
    drawables: Option<Res<crate::oni2_loader::registries::DrawableLibrary>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let dt = time.delta_secs();
    if dt == 0.0 {
        return;
    }

    let lib = if let Some(l) = drawables {
        l
    } else {
        return;
    };

    // Walk the unified drawable registry — entity sub-meshes are mirrored
    // in by `sync_drawables_from_entities_system`, FX drawables register
    // directly via `load_drawable`.  Mesh handles are shared across
    // instances, so each UV mutation propagates to every entity rendering
    // that mesh (acceptable for ambient steam/fire; per-instance variants
    // would need a custom MaterialExtension shader with instance uniforms).
    for entry in &lib.entries {
        let anims = &entry.animators;
        if anims.is_empty() {
            continue;
        }
        let mut u_speed = 0.0;
        let mut v_speed = 0.0;
        let mut r_speed = 0.0;
        let mut s_speed = 0.0;

        for anim in anims {
            if anim.slides_speed != 0.0 {
                u_speed = anim.slides_speed;
            }
            if anim.slidet_speed != 0.0 {
                v_speed = anim.slidet_speed;
            }
            if anim.rotate_speed != 0.0 {
                r_speed = anim.rotate_speed;
            }
            if anim.scalet_speed != 0.0 {
                s_speed = anim.scalet_speed;
            }
        }

        if u_speed == 0.0 && v_speed == 0.0 && r_speed == 0.0 && s_speed == 0.0 {
            continue;
        }

        {
            let handle = &entry.mesh;
            if let Some(mesh) = meshes.get_mut(handle)
                && let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) =
                    mesh.attribute_mut(Mesh::ATTRIBUTE_UV_0)
            {
                let du = u_speed * dt;
                let dv = v_speed * dt;
                let cos_r = (r_speed * dt).cos();
                let sin_r = (r_speed * dt).sin();
                let scale = if s_speed != 0.0 {
                    1.0 + s_speed * dt
                } else {
                    1.0
                };

                for uv in uvs.iter_mut() {
                    let mut u = uv[0];
                    let mut v = uv[1];

                    u -= du;
                    v -= dv;

                    // Rotation around 0.5 (texture center pivot point)
                    if r_speed != 0.0 || s_speed != 0.0 {
                        let cu = u - 0.5;
                        let cv = v - 0.5;

                        let rotated_u = cu * cos_r - cv * sin_r;
                        let rotated_v = cu * sin_r + cv * cos_r;

                        u = rotated_u * scale + 0.5;
                        v = rotated_v * scale + 0.5;
                    }

                    // Float wrapping logic to prevent precision degradation
                    // Only safely bounds if geometry does not natively span across multiple units smoothly.
                    // Left unbounded purposely to preserve identical tiled tiling mappings.
                    uv[0] = u;
                    uv[1] = v;
                }
            }
        }
    }
}

fn get_or_create_ptx_asset(
    ptx_name: &str,
    ptx_def: &crate::oni2_loader::parsers::particle::ParticleSystemDef,
    rate_override: f32,
    num_particles_override: i32,
    effects: &mut ResMut<Assets<EffectAsset>>,
    effect_cache: &mut Local<std::collections::HashMap<String, Handle<EffectAsset>>>,
) -> Handle<EffectAsset> {
    let cache_key = format!("{}_{}_{}", ptx_name, rate_override, num_particles_override);
    if let Some(handle) = effect_cache.get(&cache_key) {
        return handle.clone();
    }

    let mut color_gradient = bevy_hanabi::Gradient::new();
    if ptx_def.color_ramp {
        color_gradient.add_key(0.0, ptx_def.color_birth.to_linear().to_vec4());
        color_gradient.add_key(0.5, ptx_def.color_death.to_linear().to_vec4());
        color_gradient.add_key(1.0, ptx_def.color_birth.to_linear().to_vec4());
    } else {
        color_gradient.add_key(0.0, ptx_def.color_birth.to_linear().to_vec4());
        color_gradient.add_key(1.0, ptx_def.color_death.to_linear().to_vec4());
    }

    let mut size_gradient = bevy_hanabi::Gradient::new();
    let start_size = Vec3::new(
        ptx_def.radius_birth.x.max(0.1),
        ptx_def.radius_birth.y.max(0.1),
        0.0,
    );
    let end_size = start_size * ptx_def.radius_death_percent.max(0.01);
    size_gradient.add_key(0.0, start_size);
    size_gradient.add_key(1.0, end_size);

    let writer = ExprWriter::new();
    let age = writer.lit(0.).expr();
    let init_pos = SetPositionSphereModifier {
        center: writer.lit(Vec3::ZERO).expr(),
        radius: writer.lit(ptx_def.position_var.length()).expr(),
        dimension: ShapeDimension::Volume,
    };

    let init_vel = SetVelocitySphereModifier {
        center: writer.lit(Vec3::ZERO).expr(),
        speed: writer.lit(ptx_def.velocity.length().max(1.0)).expr(),
    };

    let init_age = SetAttributeModifier::new(Attribute::AGE, age);
    let init_lifetime =
        SetAttributeModifier::new(Attribute::LIFETIME, writer.lit(ptx_def.life).expr());
    let drag_coef = writer.lit(ptx_def.velocity_damping.length()).expr();

    let mut gravity_expr = None;
    if ptx_def.gravity != 0.0 {
        gravity_expr = Some(writer.lit(Vec3::new(0.0, ptx_def.gravity, 0.0)).expr());
    }

    if ptx_def.rotate_ptx && ptx_def.rotate_speed != 0.0 {
        let age_expr = writer.attr(Attribute::AGE);
        let speed_expr = writer.lit(ptx_def.rotate_speed);
        let _rot_expr = age_expr.mul(speed_expr);
    }

    let rate = if rate_override > 0.0 {
        rate_override
    } else {
        ptx_def.rate
    };
    let spawner = if num_particles_override > 0 {
        bevy_hanabi::SpawnerSettings::burst(
            (num_particles_override as f32).into(),
            bevy_hanabi::CpuValue::Single(rate),
        )
    } else {
        bevy_hanabi::SpawnerSettings::rate(rate.into())
    };

    let texture_slot = writer.lit(0u32).expr();

    let mut init_sprite_index = None;
    let mut update_sprite_index = None;

    if ptx_def.grid_x > 1 || ptx_def.grid_y > 1 {
        let frames = (ptx_def.end_tile as i32 - ptx_def.start_tile as i32 + 1).max(1);
        let start_tile = writer.lit(ptx_def.start_tile as i32);

        if ptx_def.frame_rate > 0.0 {
            // Animated
            let age = writer.attr(Attribute::AGE);
            let frame_rate = writer.lit(ptx_def.frame_rate);
            let frame_idx = age
                .mul(frame_rate)
                .cast(ScalarType::Int)
                .rem(writer.lit(frames));
            update_sprite_index = Some(SetAttributeModifier::new(
                Attribute::SPRITE_INDEX,
                start_tile.add(frame_idx).expr(),
            ));
        } else {
            // Static randomize
            let rnd = writer
                .rand(ScalarType::Float)
                .mul(writer.lit(frames as f32))
                .cast(ScalarType::Int);
            init_sprite_index = Some(SetAttributeModifier::new(
                Attribute::SPRITE_INDEX,
                start_tile.add(rnd).expr(),
            ));
        }
    }

    let mut module = writer.finish();
    module.add_texture_slot("color");

    let mut asset = EffectAsset::new(32768, spawner, module)
        .init(init_pos)
        .init(init_vel)
        .init(init_age)
        .init(init_lifetime)
        .update(LinearDragModifier::new(drag_coef));

    if let Some(g_expr) = gravity_expr {
        asset = asset.update(AccelModifier::new(g_expr));
    }

    asset = asset
        .render(ColorOverLifetimeModifier {
            gradient: color_gradient,
            ..Default::default()
        })
        .render(SizeOverLifetimeModifier {
            gradient: size_gradient,
            screen_space_size: false,
            ..Default::default()
        })
        .render(ParticleTextureModifier {
            texture_slot,
            sample_mapping: ImageSampleMapping::Modulate,
        })
        .render(OrientModifier::new(OrientMode::FaceCameraPosition));

    asset.alpha_mode = match ptx_def.blend_set {
        0 => bevy_hanabi::AlphaMode::Add,
        1 => bevy_hanabi::AlphaMode::Blend,
        _ => bevy_hanabi::AlphaMode::Blend, // Subtractive isn't cleanly supported in standard Bevy AlphaMode
    };

    // Hanabi defaults to Global, matching C++ `!RelativeToParentMatrix`
    asset = asset.with_simulation_space(if ptx_def.relative_to_parent {
        SimulationSpace::Local
    } else {
        SimulationSpace::Global
    });

    if let Some(init_sp) = init_sprite_index {
        asset = asset.init(init_sp);
    }
    if let Some(update_sp) = update_sprite_index {
        asset = asset.update(update_sp);
    }
    if ptx_def.grid_x > 1 || ptx_def.grid_y > 1 {
        asset = asset.render(FlipbookModifier {
            sprite_grid_size: UVec2::new(ptx_def.grid_x, ptx_def.grid_y),
        });
    }

    let handle = effects.add(asset);

    effect_cache.insert(cache_key, handle.clone());
    handle
}

/// Observer that listens for SpawnFx events and spawns the appropriate effect
fn handle_spawn_fx(
    trigger: On<SpawnFx>,
    mut commands: Commands,
    fx_lib: Res<FxLibrary>,
    ptx_lib: Res<ParticleLibrary>,
    mut effects: ResMut<Assets<EffectAsset>>,
    mut effect_cache: Local<std::collections::HashMap<String, Handle<EffectAsset>>>,
    actor_sleep_query: Query<&crate::oni2_loader::components::ActorAsleep>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut quad_cache: ResMut<crate::fx_visuals::FxVisualMesh>,
    mut flash_tex_cache: ResMut<crate::fx_visuals::FxFlashTexture>,
    mut strike_mesh_cache: ResMut<crate::fx_visuals::FxStrikeMeshCache>,
    health_query: Query<&crate::combat::components::Health>,
    blast_assets: Res<crate::blast_fx::BlastFxAssets>,
) {
    let ev = trigger.event();
    let lower_name = ev.name.to_lowercase();

    if let Some(fx_def) = fx_lib.effects.get(&lower_name) {
        let kind = match fx_def {
            crate::oni2_loader::parsers::effect::EffectDef::Particle(_) => "Particle",
            crate::oni2_loader::parsers::effect::EffectDef::DelayedParticle(_) => "DelayedParticle",
            crate::oni2_loader::parsers::effect::EffectDef::Sprite(_) => "Sprite",
            crate::oni2_loader::parsers::effect::EffectDef::Flash(_) => "Flash",
            crate::oni2_loader::parsers::effect::EffectDef::Laser(_) => "Laser",
            crate::oni2_loader::parsers::effect::EffectDef::Strike(_) => "Strike",
            crate::oni2_loader::parsers::effect::EffectDef::HealthIndicator(_) => "HealthIndicator",
            crate::oni2_loader::parsers::effect::EffectDef::BlastFire(_) => "BlastFire",
            _ => "Other",
        };
        info!(
            "handle_spawn_fx: name='{}' resolved kind={} at={:?} parent={:?}",
            ev.name, kind, ev.at, ev.parent
        );

        let ptx_ref = match fx_def {
            crate::oni2_loader::parsers::effect::EffectDef::Particle(p) => Some(&p.system),
            crate::oni2_loader::parsers::effect::EffectDef::DelayedParticle(p) => Some(&p.system),
            _ => None,
        };

        if let Some(pref) = ptx_ref {
            let ptx_name = pref.system_name.to_lowercase();
            if let Some(ptx_def) = ptx_lib.systems.get(&ptx_name) {
                let handle = get_or_create_ptx_asset(
                    &ptx_name,
                    ptx_def,
                    0.0,
                    0,
                    &mut effects,
                    &mut effect_cache,
                );

                let is_asleep = ev
                    .parent
                    .and_then(|p| actor_sleep_query.get(p).ok())
                    .is_some();
                let effective_start_active = ev.start_active && !is_asleep;

                let mut ec = commands.spawn((
                    ParticleEffect::new(handle),
                    EffectMaterial {
                        images: vec![ptx_def.texture.clone()],
                    },
                    Transform::from_translation(ev.at.unwrap_or(Vec3::ZERO)),
                    IntendedFxState(ev.start_active),
                    // Cleanup-on-layout-exit.  Without this tag,
                    // particles spawned without a parent (or whose
                    // parent isn't InGameEntity) survive InGame→
                    // InGame layout transitions and accumulate.
                    crate::menu::InGameEntity,
                ));

                if !effective_start_active {
                    ec.insert(FxStartInactive);
                }

                if let Some(parent) = ev.parent {
                    let child_id = ec.id();
                    commands.entity(parent).add_child(child_id);
                }
            }
        } else if let crate::oni2_loader::parsers::effect::EffectDef::HealthIndicator(hd) = fx_def {
            // Get health percentage
            let health_pct = ev
                .parent
                .and_then(|p| health_query.get(p).ok())
                .map(|h| h.fraction() * 100.0)
                .unwrap_or(100.0);

            // Interpolate colors based on health
            let color = if health_pct >= hd.mid_percentage {
                let t = (health_pct - hd.mid_percentage) / (100.0 - hd.mid_percentage).max(0.001);
                Color::srgb(
                    hd.mid_color.to_linear().red * (1.0 - t)
                        + hd.undamaged_color.to_linear().red * t,
                    hd.mid_color.to_linear().green * (1.0 - t)
                        + hd.undamaged_color.to_linear().green * t,
                    hd.mid_color.to_linear().blue * (1.0 - t)
                        + hd.undamaged_color.to_linear().blue * t,
                )
            } else {
                let t = health_pct / hd.mid_percentage.max(0.001);
                Color::srgb(
                    hd.dead_color.to_linear().red * (1.0 - t) + hd.mid_color.to_linear().red * t,
                    hd.dead_color.to_linear().green * (1.0 - t)
                        + hd.mid_color.to_linear().green * t,
                    hd.dead_color.to_linear().blue * (1.0 - t) + hd.mid_color.to_linear().blue * t,
                )
            };

            let texture_handle =
                if let Some(ptx) = ptx_lib.systems.get(&hd.system.system_name.to_lowercase()) {
                    ptx.texture.clone()
                } else {
                    return;
                };

            let world_pos = ev.at.unwrap_or(Vec3::ZERO);

            let mock_sprite = crate::oni2_loader::parsers::effect::SpriteEffectDef {
                name: hd.name.clone(),
                texture: texture_handle,
                color,
                blend_set: 1, // additive or blend? 1 = blend, 0 = additive. Let's use 1.
                duration: if hd.duration > 0.0 { hd.duration } else { 0.5 },
                line_length: 1.0,
                line_width: 1.0,
                particle_size: 1.0,
                alignment: 0,
            };

            crate::fx_visuals::spawn_sprite(
                &mut commands,
                &mut quad_cache,
                &mut meshes,
                &mut materials,
                &mock_sprite,
                world_pos,
                ev.parent,
            );
        } else if let crate::oni2_loader::parsers::effect::EffectDef::Laser(ld) = fx_def {
            // Skip spawn entirely if the parent is asleep — the laser
            // would just retract immediately otherwise.
            let is_asleep = ev
                .parent
                .and_then(|p| actor_sleep_query.get(p).ok())
                .is_some();
            if !ev.start_active || is_asleep {
                // Without start_active the legacy laser just sits dormant;
                // our trail has no inactive state, so just don't spawn.
                // (If a hypothetical SetFx-true is later sent, the laser
                // won't materialize — matches nothing we've seen in the
                // wild yet, can be added if a real case shows up.)
                return;
            }
            let Some(source) = ev.parent else {
                warn!("SpawnFx Laser '{}' has no parent — skipped", ev.name);
                return;
            };
            crate::laser::spawn_laser(
                &mut commands,
                &mut meshes,
                &mut materials,
                ld,
                source,
                ev.at.unwrap_or(Vec3::ZERO),
                &ev.name,
            );
        } else if let crate::oni2_loader::parsers::effect::EffectDef::Flash(fd) = fx_def {
            // Flash bursts are short additive pops — usually fired
            // from explosion/impact callbacks. The position falls
            // back to the parent entity's location when ev.at is
            // None (handled implicitly by ChildOf when parent is set,
            // since the spawn translation is at world (0,0,0) relative
            // to the parent's transform).
            let world_pos = ev.at.unwrap_or(Vec3::ZERO);
            crate::fx_visuals::spawn_flash(
                &mut commands,
                &mut quad_cache,
                &mut flash_tex_cache,
                &mut meshes,
                &mut materials,
                &mut images,
                fd,
                world_pos,
                ev.parent,
            );
        } else if let crate::oni2_loader::parsers::effect::EffectDef::Sprite(sd) = fx_def {
            // Continuous-while-controlled billboards (muzzle flashes,
            // halos, etc.).  Without an FxEffect controller layer we
            // can't honor "stay until SetFx false"; the spawner uses
            // `def.duration` as a hard lifetime, with a small
            // fallback when duration is 0.
            let world_pos = ev.at.unwrap_or(Vec3::ZERO);
            crate::fx_visuals::spawn_sprite(
                &mut commands,
                &mut quad_cache,
                &mut meshes,
                &mut materials,
                sd,
                world_pos,
                ev.parent,
            );
        } else if let crate::oni2_loader::parsers::effect::EffectDef::Strike(sd) = fx_def {
            let world_pos = ev.at.unwrap_or(Vec3::ZERO);
            // Resolve the target's current health for the palette tint.
            // No parent / unknown entity → assume full health.
            let target_health = ev
                .parent
                .and_then(|p| health_query.get(p).ok())
                .map(|h| h.fraction())
                .unwrap_or(1.0);
            crate::fx_visuals::spawn_strike(
                &mut commands,
                &mut strike_mesh_cache,
                &mut meshes,
                &mut materials,
                sd,
                world_pos,
                target_health,
            );
        } else if let crate::oni2_loader::parsers::effect::EffectDef::BlastFire(bd) = fx_def {
            // Hardcoded `fxBlastFireType` asset triple was preloaded at
            // startup; spawn parents + per-pass renderable children at
            // the requested position.  The path-traversal + light fade
            // (`update_blast_fire_system`) will be wired in a follow-up
            // pass — for now the geometry sits at the spawn point so
            // the asset/material/UV-animation pipeline can be verified
            // end-to-end through the `blast_test` layout's makefx call.
            let world_pos = ev.at.unwrap_or(Vec3::ZERO);
            if let Some(entity) = crate::blast_fx::spawn_blast_fire(
                &mut commands,
                bd,
                world_pos,
                ev.parent,
                &blast_assets,
            ) {
                info!(
                    "blast_fx: spawned BlastFire '{}' entity={:?} at={:?}",
                    bd.name, entity, world_pos
                );
            } else {
                warn!(
                    "blast_fx: BlastFire '{}' requested but assets failed to load — skipping",
                    bd.name
                );
            }
        }
    } else {
        warn!("SpawnFx: Effect '{}' not found in FxLibrary", ev.name);
    }
}

/// Observer that listens for SpawnPtx events and spawns a particle system directly
fn handle_spawn_ptx(
    trigger: On<SpawnPtx>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut ptx_lib: ResMut<ParticleLibrary>,
    mut effects: ResMut<Assets<EffectAsset>>,
    mut effect_cache: Local<std::collections::HashMap<String, Handle<EffectAsset>>>,
    actor_sleep_query: Query<&crate::oni2_loader::components::ActorAsleep>,
) {
    let ev = trigger.event();
    let mut ptx_name = ev.ptx_name.to_lowercase();
    if ptx_name.ends_with(".ptx") {
        ptx_name = ptx_name.strip_suffix(".ptx").unwrap().to_string();
    }

    try_load_ptx(&ptx_name, &asset_server, &mut ptx_lib, &mut images);

    if let Some(ptx_def) = ptx_lib.systems.get(&ptx_name) {
        let handle = get_or_create_ptx_asset(
            &ptx_name,
            ptx_def,
            ev.rate,
            ev.num_particles,
            &mut effects,
            &mut effect_cache,
        );

        let is_asleep = ev
            .parent
            .and_then(|p| actor_sleep_query.get(p).ok())
            .is_some();
        let effective_start_active = ev.start_active && !is_asleep;

        let mut ec = commands.spawn((
            ParticleEffect::new(handle),
            EffectMaterial {
                images: vec![ptx_def.texture.clone()],
            },
            Transform::from_translation(ev.at.unwrap_or(Vec3::ZERO)),
            IntendedFxState(ev.start_active),
            crate::menu::InGameEntity,
        ));

        if !effective_start_active {
            ec.insert(FxStartInactive);
        }

        if let Some(parent) = ev.parent {
            let child_id = ec.id();
            commands.entity(parent).add_child(child_id);
        }
    } else {
        warn!(
            "SpawnPtx: Particle system '{}' not found in ParticleLibrary",
            ev.ptx_name
        );
    }
}

/// System that watches for newly spawned entities with `ActorFxType` and attaches their effects
fn handle_actor_fx_attachments(
    mut commands: Commands,
    query: Query<(Entity, &ActorFxType), Added<ActorFxType>>,
    fx_lib: Res<FxLibrary>,
    _ptx_lib: Res<ParticleLibrary>,
) {
    for (entity, fx_type) in query.iter() {
        commands
            .entity(entity)
            .insert(IntendedFxState(fx_type.start_active));

        if let Some(ref fx_name) = fx_type.fx_name {
            let lower_name = fx_name.to_lowercase();
            if let Some(_fx_def) = fx_lib.effects.get(&lower_name) {
                info!(
                    "Attaching FX {} to entity {:?} (StartActive={})",
                    fx_name, entity, fx_type.start_active
                );
                commands.trigger(SpawnFx {
                    name: fx_name.clone(),
                    at: Some(fx_type.ptx_offset),
                    parent: Some(entity),
                    start_active: fx_type.start_active,
                });
            } else {
                warn!("ActorFxType: Effect '{}' not found in FxLibrary", fx_name);
            }
        }

        if let Some(ref ptx_name) = fx_type.ptx_name {
            info!(
                "Attaching Particle System {} to entity {:?} (StartActive={})",
                ptx_name, entity, fx_type.start_active
            );
            commands.trigger(SpawnPtx {
                ptx_name: ptx_name.clone(),
                rate: fx_type.ptx_birth_rate,
                num_particles: fx_type.ptx_num_particles,
                at: Some(fx_type.ptx_offset),
                parent: Some(entity),
                start_active: fx_type.start_active,
            });
        }
    }
}

#[derive(Event, Debug, Clone)]
pub struct FxAction {
    pub action: String,
    pub target: Entity,
}

/// Observer to intercept FxAction triggers and execute FX activation/deactivation.
fn handle_fx_action(trigger: On<FxAction>, mut target_query: Query<&mut IntendedFxState>) {
    let evt = trigger.event();
    let act_lower = evt.action.to_lowercase();
    if act_lower == "activate" || act_lower == "deactivate" {
        let to_active = act_lower == "activate";
        if let Ok(mut intended) = target_query.get_mut(evt.target) {
            intended.0 = to_active;
        } else {
            warn!("FxAction Target {:?} missing IntendedFxState", evt.target);
        }
    }
}

/// Continuous system overriding missing initial state transfers mechanically verifying child effects
/// properly follow parent intended directives regardless of when commands natively flush spawning them.
fn sync_intended_fx_state(
    parent_q: Query<(
        &IntendedFxState,
        Option<&crate::oni2_loader::components::ActorAsleep>,
        &Children,
    )>,
    mut spawner_q: Query<(&mut EffectSpawner, &mut IntendedFxState), Without<Children>>,
) {
    for (parent_intended, maybe_asleep, children) in &parent_q {
        let should_be_active = parent_intended.0 && maybe_asleep.is_none();
        for child_ent in children.iter() {
            if let Ok((mut spawner, mut child_intended)) = spawner_q.get_mut(child_ent)
                && (child_intended.0 != parent_intended.0 || spawner.active != should_be_active)
            {
                if spawner.active && !should_be_active {
                    // Instantly visually terminate particle emissions cleanly
                    // (without popping already birthed particles violently)
                    spawner.active = false;
                } else {
                    spawner.active = should_be_active;
                }
                child_intended.0 = parent_intended.0;
            }
        }
    }
}
