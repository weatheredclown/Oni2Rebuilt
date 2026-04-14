use avian3d::debug_render::PhysicsGizmos;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::ecs::relationship::Relationship;
use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;

pub struct DebugPlugin;

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (setup_fps_counter, disable_physics_debug))
            .add_systems(
                Update,
                (
                    update_fps_counter,
                    toggle_physics_debug,
                    toggle_debug_light,
                    debug_kill_creatures,
                    debug_scan_player_geometry,
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
        if let Some(diag) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS) {
            if let Some(val) = diag.smoothed() {
                *text = Text::new(format!("FPS: {val:.0}"));
            }
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
// Debug kill (K) and geometry scan (F11)
// ---------------------------------------------------------------------------

/// K — kills all non-player AI creatures.
fn debug_kill_creatures(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    creature_query: Query<
        Entity,
        (
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
