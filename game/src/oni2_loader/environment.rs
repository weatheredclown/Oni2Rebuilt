use super::*;

#[derive(Resource)]
pub struct LayoutFogSettings {
    pub color: Color,
    pub start: f32,
    pub end: f32,
}

/// Navigation curves from layout.paths. Each curve is a named list of waypoints.
#[derive(Resource, Default)]
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
    pub packages: std::collections::HashMap<String, crate::oni2_loader::parsers::camera::CameraPackageDef>,
}

#[derive(Resource, Default)]
pub struct CameraParameterSets {
    pub sets: std::collections::HashMap<String, crate::oni2_loader::parsers::camera::CameraParameterSet>,
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
pub struct SpawnAssets<'a, 'commands, 'c1, 'c2, 'c3, 'c4, 'c5, 'c6, 'c7> {
    pub commands: &'a mut Commands<'commands, 'c1>,
    pub meshes: &'a mut ResMut<'c2, Assets<Mesh>>,
    pub materials: &'a mut ResMut<'c3, Assets<StandardMaterial>>,
    pub images: &'a mut ResMut<'c4, Assets<Image>>,
    pub skinned_mesh_ibp: &'a mut ResMut<'c5, Assets<SkinnedMeshInverseBindposes>>,
    pub entity_lib: &'a mut ResMut<'c6, crate::oni2_loader::registries::EntityLibrary>,
    pub anim_registry: &'a mut ResMut<'c7, crate::oni2_loader::registries::AnimRegistry>,
    pub texture_collections: &'a mut TextureCollections,
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
