/*
 * shadow_lod.rs — distance-based shadow casting for point and spot lights.
 *
 * The legacy levels mark dozens of lights as shadow-casters, but only the
 * 1–2 closest to the camera ever produce a visible silhouette.  Bevy pays
 * the full shadow-map cost per enabled light per frame, so flipping all of
 * them on costs ~5 ms/frame in the FX Cathedral steady-state trace.
 *
 * Scheme: every `update_interval_frames`, rank all `ShadowCandidate` lights
 * by distance² to the player, enable `shadows_enabled` on the `top_k`
 * nearest within `max_shadow_distance`, disable on the rest.  Ranking (vs.
 * a hard distance threshold) gives free hysteresis — a light only flips
 * when another overtakes it, never just because the player crossed an
 * absolute boundary.
 *
 * Spawn sites in `layout_loader.rs` add the `ShadowCandidate` marker on
 * lights that the level designer originally tagged as shadow-casting (i.e.
 * `cast_shadow_range > 0`) and start them with `shadows_enabled = false`.
 * Anything not in that set never gets shadows regardless of distance.
 */
use bevy::prelude::*;

use crate::player::components::Player;

/// Marker component: this light is *eligible* to cast shadows when the
/// player is nearby.  Attached at spawn time only to lights the layout
/// flagged via `cast_shadow_range > 0`.  The runtime LOD system below
/// flips `PointLight::shadows_enabled` / `SpotLight::shadows_enabled`
/// based on rank-by-distance to the player.
#[derive(Component)]
pub struct ShadowCandidate;

/// Tunables for the LOD system.  Inserted with defaults; tweak via the
/// console or hard-edit if you want different behavior per-level.
#[derive(Resource, Clone, Copy)]
pub struct ShadowLodSettings {
    /// How many of the nearest candidate lights get shadow-casting on.
    pub top_k: usize,
    /// Lights farther than this from the player are never granted shadows
    /// even if they'd otherwise be in the top-k.  Squared internally.
    pub max_shadow_distance: f32,
    /// Re-evaluate every N Update ticks.  6 Hz (≈10 frames @ 60fps) is
    /// plenty; the player can't outrun the cadence.
    pub update_interval_frames: u32,
}

impl Default for ShadowLodSettings {
    fn default() -> Self {
        Self {
            top_k: 2,
            max_shadow_distance: 25.0,
            update_interval_frames: 10,
        }
    }
}

/// Tick counter so we can run the system every Nth frame without a Timer
/// resource.  Wraps; the comparison uses modulo.
#[derive(Resource, Default)]
struct ShadowLodFrameCounter(u32);

/// Global resource for tracking the state of realtime shadows.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RealtimeShadowsState {
    pub enabled: bool,
}

impl Default for RealtimeShadowsState {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Marker component to remember if a light originally cast shadows.
#[derive(Component)]
pub struct OriginalShadowCaster;

/// Reads keyboard input to toggle shadows.
fn toggle_shadows_input_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<RealtimeShadowsState>,
) {
    if keyboard.just_pressed(KeyCode::Digit1) {
        state.enabled = !state.enabled;
        info!("Toggled realtime shadows to: {}", state.enabled);
    }
}

/// Controls DirectionalLights shadows based on the global state.
fn apply_directional_shadows_system(
    shadows_state: Res<RealtimeShadowsState>,
    mut dir_q: Query<(Entity, &mut DirectionalLight, Option<&OriginalShadowCaster>)>,
    mut commands: Commands,
) {
    for (entity, mut light, original_caster) in &mut dir_q {
        // If we haven't tagged it yet, and it was spawned with shadows enabled, tag it.
        let is_caster = if original_caster.is_some() {
            true
        } else if light.shadows_enabled && shadows_state.enabled {
            commands.entity(entity).insert(OriginalShadowCaster);
            true
        } else {
            false
        };

        let want = shadows_state.enabled && is_caster;
        if light.shadows_enabled != want {
            light.shadows_enabled = want;
        }
    }
}

/// Controls non-LOD Point/SpotLights shadows based on the global state.
fn apply_other_shadows_system(
    shadows_state: Res<RealtimeShadowsState>,
    mut point_q: Query<(Entity, &mut PointLight, Option<&OriginalShadowCaster>), Without<ShadowCandidate>>,
    mut spot_q: Query<(Entity, &mut SpotLight, Option<&OriginalShadowCaster>), Without<ShadowCandidate>>,
    mut commands: Commands,
) {
    for (entity, mut light, original_caster) in &mut point_q {
        let is_caster = if original_caster.is_some() {
            true
        } else if light.shadows_enabled && shadows_state.enabled {
            commands.entity(entity).insert(OriginalShadowCaster);
            true
        } else {
            false
        };

        let want = shadows_state.enabled && is_caster;
        if light.shadows_enabled != want {
            light.shadows_enabled = want;
        }
    }

    for (entity, mut light, original_caster) in &mut spot_q {
        let is_caster = if original_caster.is_some() {
            true
        } else if light.shadows_enabled && shadows_state.enabled {
            commands.entity(entity).insert(OriginalShadowCaster);
            true
        } else {
            false
        };

        let want = shadows_state.enabled && is_caster;
        if light.shadows_enabled != want {
            light.shadows_enabled = want;
        }
    }
}

/// Pick the top-K nearest `ShadowCandidate` lights to the player and
/// turn their shadows on; turn off the rest.
fn update_shadow_lod_system(
    mut counter: ResMut<ShadowLodFrameCounter>,
    settings: Res<ShadowLodSettings>,
    shadows_state: Res<RealtimeShadowsState>,
    player_q: Query<&GlobalTransform, With<Player>>,
    mut point_q: Query<(Entity, &GlobalTransform, &mut PointLight), With<ShadowCandidate>>,
    mut spot_q: Query<(Entity, &GlobalTransform, &mut SpotLight), With<ShadowCandidate>>,
) {
    counter.0 = counter.0.wrapping_add(1);

    // If shadows state changed, apply immediately
    if shadows_state.is_changed() {
        if !shadows_state.enabled {
            for (_, _, mut light) in &mut point_q {
                light.shadows_enabled = false;
            }
            for (_, _, mut light) in &mut spot_q {
                light.shadows_enabled = false;
            }
        }
    }

    if !shadows_state.enabled {
        return;
    }

    if settings.update_interval_frames == 0 || counter.0 % settings.update_interval_frames != 0 {
        return;
    }

    let Ok(player_tf) = player_q.single() else {
        return;
    };
    let player_pos = player_tf.translation();
    let max_d_sq = settings.max_shadow_distance * settings.max_shadow_distance;

    // Build a single combined ranking over both point and spot lights.
    // We tag each entry with its kind so we know which mut-handle to flip
    // in the second pass.
    enum Kind {
        Point,
        Spot,
    }
    let mut ranked: Vec<(f32, Entity, Kind)> = Vec::new();
    for (e, tf, _) in point_q.iter() {
        let d_sq = tf.translation().distance_squared(player_pos);
        if d_sq <= max_d_sq {
            ranked.push((d_sq, e, Kind::Point));
        }
    }
    for (e, tf, _) in spot_q.iter() {
        let d_sq = tf.translation().distance_squared(player_pos);
        if d_sq <= max_d_sq {
            ranked.push((d_sq, e, Kind::Spot));
        }
    }
    ranked.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Build the winners set up front so the per-light mut loop is O(1)
    // per check.  K is small (1–4 in practice) so a Vec contains-check is
    // faster than a HashSet here.
    let winners: Vec<Entity> = ranked
        .iter()
        .take(settings.top_k)
        .map(|(_, e, _)| *e)
        .collect();

    for (e, _, mut light) in &mut point_q {
        let want = winners.contains(&e);
        if light.shadows_enabled != want {
            light.shadows_enabled = want;
        }
    }
    for (e, _, mut light) in &mut spot_q {
        let want = winners.contains(&e);
        if light.shadows_enabled != want {
            light.shadows_enabled = want;
        }
    }
}

pub struct ShadowLodPlugin;

impl Plugin for ShadowLodPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShadowLodSettings>()
            .init_resource::<ShadowLodFrameCounter>()
            .init_resource::<RealtimeShadowsState>()
            .add_systems(
                Update,
                (
                    toggle_shadows_input_system,
                    update_shadow_lod_system,
                    apply_directional_shadows_system,
                    apply_other_shadows_system,
                ),
            );
    }
}
