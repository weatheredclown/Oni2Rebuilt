/*
 * oni2_loader/environment.rs — level environment resources.
 *
 * LayoutFogSettings, NavigationCurves (named NURBS paths from layout.paths),
 * ActiveCameraPackage / CameraPackages / CameraParameterSets (loaded from
 * layout.cameras), FogEnabled marker, DebugLightGridState.  Systems:
 * apply_fog_to_camera, update_skyhat, toggle_debug_fog, toggle_debug_light_grid,
 * update_debug_light_grid, scroni_curve_bridge_system.
 */
use super::*;

#[derive(Resource)]
pub struct LayoutFogSettings {
    pub color: Color,
    pub start: f32,
    pub end: f32,
}

/// Navigation curves from layout.paths. Each curve is a named list of waypoints.
#[derive(Resource, Default, Clone)]
pub struct LayoutPaths {
    pub curves: Vec<(String, Vec<Vec3>)>,
}

/// Global mapping of Texture Collection (.tc) files loaded during layout setup.
/// Maps entity type (e.g., "ProximityMine") to a list of preloaded texture handles by frame index.
#[derive(Resource, Default)]
pub struct TextureCollections {
    pub collections: std::collections::HashMap<String, Vec<Handle<Image>>>,
}

#[derive(Resource, Default)]
pub struct CameraPackages {
    pub packages:
        std::collections::HashMap<String, crate::oni2_loader::parsers::camera::CameraPackageDef>,
}

#[derive(Resource, Default)]
pub struct CameraParameterSets {
    pub sets:
        std::collections::HashMap<String, crate::oni2_loader::parsers::camera::CameraParameterSet>,
}

#[derive(Resource)]
pub struct ActiveCameraPackage {
    pub name: String,
}

impl Default for ActiveCameraPackage {
    fn default() -> Self {
        Self {
            name: "DEFAULT_PACKAGE".to_string(),
        }
    }
}

/// Context for the currently loaded layout, allowing dynamic spawning of actors later.
#[derive(Resource, Clone)]
pub struct LayoutContext {
    pub layout_dir: String,
    pub entity_base: String,
    pub basic_types: std::collections::HashSet<String>,
}

/// A struct to bundle all asset mutators required for spawning entities.
pub struct SpawnAssets<'a, 'commands, 'c1, 'c2, 'c3, 'c4, 'c5, 'c6, 'c7, 'c8, 'c9> {
    pub commands: &'a mut Commands<'commands, 'c1>,
    pub meshes: &'a mut ResMut<'c2, Assets<Mesh>>,
    pub materials: &'a mut ResMut<'c3, Assets<StandardMaterial>>,
    pub images: &'a mut ResMut<'c4, Assets<Image>>,
    pub skinned_mesh_ibp: &'a mut ResMut<'c5, Assets<SkinnedMeshInverseBindposes>>,
    pub entity_lib: &'a mut ResMut<'c6, crate::oni2_loader::registries::EntityLibrary>,
    pub anim_registry: &'a mut ResMut<'c7, crate::oni2_loader::registries::AnimRegistry>,
    pub texture_collections: &'a mut TextureCollections,
    pub fight_fsm_cache: &'a mut ResMut<'c8, crate::fightai::FightFsmCache>,
    pub attack_fsm_cache: &'a mut ResMut<'c9, crate::fightai::AttackFsmCache>,
    pub mover_backend: crate::mover::MoverBackend,
    pub mover_config: Option<Handle<crate::mover::Oni2SchemeConfig>>,
}

/// Marker resource: fog rendering is enabled (--fog flag).
#[derive(Resource)]
pub struct FogEnabled;

/// Marker for skyhat entity — follows camera XZ position.
#[derive(Component)]
pub struct SkyHat;

/// System: attach DistanceFog to the camera when LayoutFogSettings resource exists.
pub fn apply_fog_to_camera(
    mut commands: Commands,
    fog: Option<Res<LayoutFogSettings>>,
    cameras: Query<(Entity, Option<&DistanceFog>), With<Camera3d>>,
) {
    let Some(fog) = fog else { return };
    for (entity, existing) in &cameras {
        if existing.is_none() {
            commands.entity(entity).insert(DistanceFog {
                color: fog.color,
                falloff: FogFalloff::Linear {
                    start: fog.start,
                    end: fog.end,
                },
                ..default()
            });
        }
    }
}

/// System: keep skyhat positioned at camera XZ, fixed Y.
pub fn update_skyhat(
    camera_query: Query<&Transform, With<Camera3d>>,
    mut skyhat_query: Query<&mut Transform, (With<SkyHat>, Without<Camera3d>)>,
) {
    let Ok(cam_tf) = camera_query.single() else {
        return;
    };
    for mut tf in &mut skyhat_query {
        tf.translation.x = cam_tf.translation.x;
        tf.translation.z = cam_tf.translation.z;
    }
}

/// System: Toggle distance fog on/off using F9
pub fn toggle_debug_fog(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    fog_enabled: Option<Res<FogEnabled>>,
    cameras: Query<Entity, With<DistanceFog>>,
) {
    if keyboard.just_pressed(KeyCode::F9) {
        if fog_enabled.is_some() {
            commands.remove_resource::<FogEnabled>();
            for entity in &cameras {
                commands.entity(entity).remove::<DistanceFog>();
            }
            info!("Debug Fog: OFF");
        } else {
            commands.insert_resource(FogEnabled);
            info!("Debug Fog: ON");
        }
    }
}

#[derive(Component)]
pub struct DebugLightGridNode;

#[derive(Resource)]
pub struct DebugLightGridState {
    pub active: bool,
    pub last_player_y: f32,
    pub lights: std::collections::HashMap<(i32, i32), Entity>,
}

impl Default for DebugLightGridState {
    fn default() -> Self {
        Self {
            active: true,
            last_player_y: 0.0,
            lights: std::collections::HashMap::new(),
        }
    }
}

/// OnExit(InGame) cleanup.  `cleanup_game` despawns the lights
/// themselves (they're tagged InGameEntity), but the
/// `DebugLightGridState` Resource isn't touched by that — it would
/// keep its `lights` HashMap of now-stale Entity IDs across the
/// layout transition.  Next layout's `update_debug_light_grid` would
/// then try to despawn dead entities (warning spam) and only
/// re-spawn the cells whose positions don't match the previous
/// layout's player path, leaving "ghost" cells from old layouts
/// hanging around the new one.  Clearing the HashMap on exit makes
/// the next layout's first pass spawn a fresh grid from scratch.
pub fn reset_debug_light_grid_on_exit(mut state: ResMut<DebugLightGridState>) {
    state.lights.clear();
    state.last_player_y = 0.0;
}

/// System: Toggle procedural light grid on/off using F6
pub fn toggle_debug_light_grid(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut state: ResMut<DebugLightGridState>,
) {
    if keyboard.just_pressed(KeyCode::F6) {
        state.active = !state.active;
        if !state.active {
            for (_, entity) in state.lights.drain() {
                commands.entity(entity).despawn();
            }
            info!("Procedural Light Grid: OFF");
        } else {
            info!("Procedural Light Grid: ON");
        }
    }
}

/// System: Update procedural light grid positions/spawns using stable quantized pooling
pub fn update_debug_light_grid(
    mut commands: Commands,
    mut state: ResMut<DebugLightGridState>,
    player_query: Query<&Transform, With<crate::player::components::Player>>,
    spatial_query: avian3d::prelude::SpatialQuery,
) {
    if !state.active {
        return;
    }

    let Some(player_tf) = player_query.iter().next() else {
        return;
    };

    let px = player_tf.translation.x;
    let py = player_tf.translation.y;
    let pz = player_tf.translation.z;

    // Recalculate all if Y changed by > 5 meters
    if (py - state.last_player_y).abs() > 5.0 {
        for (_, entity) in state.lights.drain() {
            commands.entity(entity).despawn();
        }
        state.last_player_y = py;
    }

    let grid_size = 10.0;
    let cx = (px / grid_size).round() as i32;
    let cz = (pz / grid_size).round() as i32;

    let mut desired_cells = std::collections::HashSet::new();
    for i in -2..=2 {
        for j in -2..=2 {
            desired_cells.insert((cx + i, cz + j));
        }
    }

    let mut to_remove = Vec::new();
    for (cell, entity) in &state.lights {
        if !desired_cells.contains(cell) {
            commands.entity(*entity).despawn();
            to_remove.push(*cell);
        }
    }
    for cell in to_remove {
        state.lights.remove(&cell);
    }

    for cell in desired_cells {
        state.lights.entry(cell).or_insert_with(|| {
            let world_x = cell.0 as f32 * grid_size;
            let world_z = cell.1 as f32 * grid_size;
            let ray_origin = Vec3::new(world_x, py + 1.0, world_z);

            // Ignore player collision
            let filter = avian3d::prelude::SpatialQueryFilter::from_excluded_entities([]);

            let height = if let Some(hit) =
                spatial_query.cast_ray(ray_origin, Dir3::Y, 100.0, true, &filter)
            {
                ray_origin.y + hit.distance - 2.0
            } else {
                py + 5.0
            };

            commands
                .spawn((
                    PointLight {
                        color: Color::WHITE,
                        intensity: 500_000.0,
                        range: grid_size * 2.5,
                        shadows_enabled: true,
                        ..default()
                    },
                    Transform::from_xyz(world_x, height, world_z),
                    // Without this tag, the procedural-grid lights
                    // outlive their owning layout — so loading a new
                    // level inherits stale lights at positions
                    // anchored to the OLD player path.  Tagging
                    // makes cleanup_game catch them on InGame exit;
                    // `reset_debug_light_grid_on_exit` clears the
                    // companion HashMap so the next session starts
                    // clean.
                    crate::menu::InGameEntity,
                    DebugLightGridNode,
                ))
                .id()
        });
    }
}

/// Draw wireframe spheres to visualize the procedurally placed lightsource grid when F3 is active.
pub fn debug_draw_light_grid(
    query: Query<&Transform, With<DebugLightGridNode>>,
    mut gizmos: Gizmos,
    visible: Res<crate::oni2_loader::animation::DebugBoundsVisible>,
) {
    if !visible.0 {
        return;
    }
    
    let color = Color::srgb(1.0, 1.0, 0.0); // Yellow for lights
    for transform in &query {
        gizmos.sphere(Isometry3d::from_translation(transform.translation), 0.5, color);
    }
}
