/*
 * debug.rs — DebugPlugin: in-game developer tools.
 *
 * FPS counter (top-right overlay), physics collider wireframe toggle (F7),
 * player-attached point light toggle (F8), AI creature kill key (K), and
 * F11 geometry scanner that prints all entities within 5 m of the player.
 * Avian3d physics debug gizmos are disabled at startup and re-enabled by F7.
 */
use avian3d::debug_render::PhysicsGizmos;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::ecs::relationship::Relationship;
use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

pub static DEBUG_NAMES: LazyLock<RwLock<HashMap<Entity, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub static DEBUG_TYPES: LazyLock<RwLock<HashMap<Entity, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub struct DebugPlugin;

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LightGizmoOn>()
            .add_systems(Startup, (setup_fps_counter, disable_physics_debug))
            .add_systems(
                Update,
                (
                    update_fps_counter,
                    toggle_physics_debug,
                    toggle_debug_light,
                    toggle_light_gizmos,
                    draw_light_gizmos,
                    log_player_teleports,
                    log_player_grounded_transitions,
                    debug_kill_creatures,
                    debug_scan_player_geometry,
                    sync_debug_names,
                    cleanup_debug_names,
                    sync_debug_types,
                    cleanup_debug_types,
                ),
            );
    }
}

// ---------------------------------------------------------------------------
// FPS counter
// ---------------------------------------------------------------------------

#[derive(Component)]
pub struct FpsText;

fn setup_fps_counter(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(5.0),
                right: Val::Px(10.0),
                ..default()
            },
            GlobalZIndex(i32::MAX),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("FPS: --"),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(1.0, 1.0, 0.0)),
                FpsText,
            ));
        });
}

fn update_fps_counter(
    diagnostics: Res<DiagnosticsStore>,
    mut query: Query<&mut Text, With<FpsText>>,
) {
    for mut text in &mut query {
        if let Some(diag) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS)
            && let Some(val) = diag.smoothed()
        {
            *text = Text::new(format!("FPS: {val:.0}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Physics debug toggle
// ---------------------------------------------------------------------------

fn disable_physics_debug(mut store: ResMut<GizmoConfigStore>) {
    store.config_mut::<PhysicsGizmos>().0.enabled = false;
}

/// F7 toggles avian3d's native physics debug rendering.
fn toggle_physics_debug(keyboard: Res<ButtonInput<KeyCode>>, mut store: ResMut<GizmoConfigStore>) {
    if keyboard.just_pressed(KeyCode::F7) {
        let config = store.config_mut::<PhysicsGizmos>().0;
        config.enabled = !config.enabled;
        info!(
            "Physics debug rendering: {}",
            if config.enabled { "ON" } else { "OFF" }
        );
    }
}

// ---------------------------------------------------------------------------
// Debug point light (F8 — follows player)
// ---------------------------------------------------------------------------

#[derive(Component)]
struct DebugPointLight;

fn toggle_debug_light(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    player_query: Query<Entity, With<crate::player::components::Player>>,
    light_query: Query<Entity, With<DebugPointLight>>,
) {
    if keyboard.just_pressed(KeyCode::F8) {
        if let Some(light_entity) = light_query.iter().next() {
            commands.entity(light_entity).despawn();
            info!("Debug point light OFF");
        } else if let Some(player_entity) = player_query.iter().next() {
            commands.entity(player_entity).with_children(|parent| {
                parent.spawn((
                    PointLight {
                        color: Color::WHITE,
                        intensity: 1_000_000.0,
                        range: 100.0,
                        shadows_enabled: true,
                        ..default()
                    },
                    Transform::from_xyz(0.0, 5.0, 0.0),
                    DebugPointLight,
                ));
            });
            info!("Debug point light ON");
        }
    }
}

// ---------------------------------------------------------------------------
// Light gizmos (F9 — wireframe each named PointLight's range)
// ---------------------------------------------------------------------------
//
// "Is the light actually in the world?" answer-er. Toggleable because the
// per-light spheres are noisy in a populated layout.

#[derive(Resource, Default)]
struct LightGizmoOn(bool);

fn toggle_light_gizmos(keyboard: Res<ButtonInput<KeyCode>>, mut on: ResMut<LightGizmoOn>) {
    if keyboard.just_pressed(KeyCode::F9) {
        on.0 = !on.0;
        info!("Light gizmos: {}", if on.0 { "ON" } else { "OFF" });
    }
}

fn draw_light_gizmos(
    on: Res<LightGizmoOn>,
    mut gizmos: Gizmos,
    point_lights: Query<(&GlobalTransform, &PointLight, Option<&Name>)>,
    spot_lights: Query<(&GlobalTransform, &SpotLight, Option<&Name>)>,
) {
    if !on.0 {
        return;
    }
    for (tf, pl, _name) in &point_lights {
        let pos = tf.translation();
        // Range sphere in the light's authored color, fades when the
        // light is dark so you can still see the marker on a 0-intensity
        // (script-driven) light.
        let alpha = if pl.intensity > 1.0 { 0.8 } else { 0.4 };
        let mut col = pl.color.to_linear();
        col.alpha = alpha;
        gizmos.sphere(pos, pl.range, Color::LinearRgba(col));
        // Tiny solid marker at the position itself so a light with
        // range=0 / off-screen intensity is still findable.
        gizmos.sphere(pos, 0.15, Color::srgb(1.0, 1.0, 0.0));
    }
    for (tf, sl, _name) in &spot_lights {
        let pos = tf.translation();
        let alpha = if sl.intensity > 1.0 { 0.8 } else { 0.4 };
        let mut col = sl.color.to_linear();
        col.alpha = alpha;
        gizmos.sphere(pos, sl.range, Color::LinearRgba(col));
        gizmos.sphere(pos, 0.15, Color::srgb(1.0, 0.5, 0.0));
    }
}

// ---------------------------------------------------------------------------
// Player grounded transitions
// ---------------------------------------------------------------------------
//
// Pairs with the teleport logger: when investigating "player falls out of
// the world", knowing the moment ground contact is lost (and from where)
// is the missing half of the story. ShapeHits-driven loss of ground will
// fire here even when no teleport just happened.

fn log_player_grounded_transitions(
    time: Res<Time>,
    mut prev_grounded: Local<Option<bool>>,
    player_q: Query<
        (
            &GlobalTransform,
            &crate::oni2_loader::animation::Oni2AnimState,
            Option<&avian3d::prelude::LinearVelocity>,
        ),
        With<crate::player::components::Player>,
    >,
) {
    let Ok((gtf, anim_state, vel_opt)) = player_q.single() else {
        *prev_grounded = None;
        return;
    };
    let cur = anim_state.is_grounded;
    if let Some(prev) = *prev_grounded
        && prev != cur
    {
        let pos = gtf.translation();
        let vel = vel_opt.map(|v| v.0).unwrap_or(Vec3::ZERO);
        warn!(
            "PLAYER GROUNDED: t={:.3}s {} pos={:.2?} vel={:.2?}",
            time.elapsed_secs(),
            if cur { "ON GROUND" } else { "AIRBORNE" },
            pos,
            vel
        );
    }
    *prev_grounded = Some(cur);
}

// ---------------------------------------------------------------------------
// Player teleport logger
// ---------------------------------------------------------------------------
//
// Per-frame position diff vs. the previous frame, with a threshold tuned to
// catch teleports without drowning in normal locomotion (typical
// per-frame movement at 60 fps with run speed ~6 m/s is < 0.1 units; the
// threshold below is well above that). Prints the from→to position, the
// delta magnitude, and a millis-resolution timestamp so logs can be
// correlated with other systems' output.
//
// Useful when a recent commit caused a "player falls out of the world"
// regression: scrub the log for jumps and check what was running just
// before each one.

const PLAYER_TELEPORT_THRESHOLD: f32 = 1.5;

fn log_player_teleports(
    time: Res<Time>,
    mut last_pos: Local<Option<Vec3>>,
    player_q: Query<&GlobalTransform, With<crate::player::components::Player>>,
) {
    let Ok(gtf) = player_q.single() else {
        // No player yet (frontend, loading), or two — either way reset
        // so the next single-player frame doesn't fire a false jump.
        *last_pos = None;
        return;
    };
    let cur = gtf.translation();
    if let Some(prev) = *last_pos {
        let delta = cur - prev;
        let mag = delta.length();
        if mag >= PLAYER_TELEPORT_THRESHOLD {
            warn!(
                "PLAYER TELEPORT: t={:.3}s mag={:.2} from={:.2?} to={:.2?} delta={:.2?}",
                time.elapsed_secs(),
                mag,
                prev,
                cur,
                delta
            );
        }
    }
    *last_pos = Some(cur);
}

// ---------------------------------------------------------------------------
// Debug kill (K) and geometry scan (F11)
// ---------------------------------------------------------------------------

/// K — kills all non-player AI creatures.
fn debug_kill_creatures(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    creature_query: Query<
        Entity,
        (
            // [AUDIT]: Prototype leakage. Filters by `AiFighter` purely as an AI marker to kill them.
            With<crate::ai::components::AiFighter>,
            Without<crate::player::components::Player>,
        ),
    >,
) {
    if keyboard.just_pressed(KeyCode::KeyK) {
        let mut count = 0;
        for entity in &creature_query {
            commands.entity(entity).despawn();
            count += 1;
        }
        info!("Killed {} active AiFighter creatures!", count);
    }
}

/// F11 — scans entities within 5 m of the player and prints them.
fn debug_scan_player_geometry(
    keyboard: Res<ButtonInput<KeyCode>>,
    player_query: Query<&GlobalTransform, With<crate::player::components::Player>>,
    query: Query<(Entity, &GlobalTransform, Option<&Name>, Option<&ChildOf>)>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
) {
    if keyboard.just_pressed(KeyCode::F11) {
        let origin: Vec3 = if let Some(player_tf) = player_query.iter().next() {
            player_tf.translation()
        } else {
            Vec3::ZERO
        };

        info!(
            "--- SCANNING PLAYER GEOMETRY (< 5m from {:.2}, {:.2}, {:.2}) ---",
            origin.x, origin.y, origin.z
        );
        let mut count = 0;
        for (entity, global_transform, name_opt, parent_opt) in &query {
            let dist = global_transform.translation().distance(origin);
            if dist <= 5.0 {
                count += 1;
                let name = name_opt.map(|n: &Name| n.as_str()).unwrap_or("<unnamed>");

                let mut path = String::new();
                let mut curr_parent: Option<&ChildOf> = parent_opt;
                while let Some(p) = curr_parent {
                    let p_name = names
                        .get(p.get())
                        .map(|n| n.as_str())
                        .unwrap_or("<unnamed_parent>");
                    path = format!("{} -> {}", p_name, path);
                    curr_parent = parents.get(p.get()).ok();
                }

                info!(
                    "Entity: {:?} | Name: '{}' | Dist: {:.2} | Path: {}",
                    entity, name, dist, path
                );
            }
        }
        info!("--- END SCAN (Total: {}) ---", count);
    }
}

fn sync_debug_names(query: Query<(Entity, &Name), Changed<Name>>) {
    if let Ok(mut map) = DEBUG_NAMES.write() {
        for (entity, name) in &query {
            map.insert(entity, name.to_string());
        }
    }
}

fn cleanup_debug_names(mut removed: RemovedComponents<Name>) {
    if removed.is_empty() {
        return;
    }
    if let Ok(mut map) = DEBUG_NAMES.write() {
        for entity in removed.read() {
            map.remove(&entity);
        }
    }
}

/// Formats an Entity's name for debug printing, falling back to its ID if no Name is present.
pub fn debug_name(entity: Entity) -> String {
    if let Ok(map) = DEBUG_NAMES.read()
        && let Some(name) = map.get(&entity)
    {
        return name.clone();
    }
    format!("{:?}", entity)
}

fn sync_debug_types(
    query: Query<
        (Entity, &crate::oni2_loader::components::BoundType),
        Changed<crate::oni2_loader::components::BoundType>,
    >,
) {
    if let Ok(mut map) = DEBUG_TYPES.write() {
        for (entity, bt) in &query {
            map.insert(entity, bt.0.clone());
        }
    }
}

fn cleanup_debug_types(mut removed: RemovedComponents<crate::oni2_loader::components::BoundType>) {
    if removed.is_empty() {
        return;
    }
    if let Ok(mut map) = DEBUG_TYPES.write() {
        for entity in removed.read() {
            map.remove(&entity);
        }
    }
}

/// Retrieves an Entity's BoundType string if it has one, otherwise returns "Actor/Prop".
pub fn debug_type(entity: Entity) -> String {
    if let Ok(map) = DEBUG_TYPES.read()
        && let Some(bt) = map.get(&entity)
    {
        return bt.clone();
    }
    "Actor/Prop".to_string()
}
