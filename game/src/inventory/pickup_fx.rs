/*
 * inventory/pickup_fx.rs — ballistic toss + ground-snap + rotating
 * indicator for dropped pickups.
 *
 * Two interlocked subsystems:
 *
 *   1. PickupBallistic — a per-pickup velocity carrier.  When
 *      drop_{weapon,item}_system spawns a Pickup, it attaches this
 *      with a randomized direction (X/Z fully random in -1..1, Y
 *      biased 0.85..0.95 upward, normalized and scaled).  Each
 *      FixedUpdate `pickup_physics_system` applies gravity, advances
 *      Transform, and raycasts straight down; when the pickup hits
 *      ground it is snapped to the surface, the velocity is zeroed,
 *      and the component is removed (the pickup is then idle until
 *      consumed).  Mirrors the legacy SpawnPickup launch.
 *
 *   2. PickupIndicator — a child quad spawned alongside every
 *      Pickup that hasn't already been visually decorated.  It floats
 *      a fixed local offset above the pickup and spins around its
 *      local Y axis at a constant rate.  Mirrors the legacy
 *      indicator's rotating-billboard hover (raycast ground snap +
 *      camera-facing quad); we keep it simpler by sticking to a
 *      world-up quad and a fixed local offset, since the pickup
 *      already lands flush on the surface.
 *
 * Both halves are gated by InventoryPlugin's AppState::InGame.
 */
use bevy::prelude::*;

use super::components::Pickup;

/// Initial speed magnitude (m/s) applied to a dropped pickup along
/// its randomized launch direction.  Tuned to give a visible arc on
/// a ~1m drop without flinging the item across the room.
const PICKUP_LAUNCH_SPEED: f32 = 3.5;

/// Gravity (m/s²) applied to airborne pickups.  Separate from the
/// global physics gravity so a tweak here doesn't drag the whole
/// world with it.
const PICKUP_GRAVITY: f32 = 9.81;

/// Maximum downward ray length when probing for landing surface.
const PICKUP_GROUND_PROBE: f32 = 50.0;

/// Local-space hover offset of the indicator above the pickup origin.
const INDICATOR_HOVER_OFFSET: f32 = 0.45;

/// Indicator quad side length (uniform scale on the X/Z plane).
const INDICATOR_SIZE: f32 = 0.6;

/// Radians/sec the indicator spins about its local +Y axis.  Matches
/// the legacy `m_RotationRate` default of ~2.0.
const INDICATOR_ROTATION_RATE: f32 = 2.0;

/// Tint used for weapon pickups.  Cool blue, lifted from the legacy
/// indicator default color.
const INDICATOR_WEAPON_COLOR: Color = Color::srgba(0.35, 0.55, 1.0, 0.55);

/// Tint used for consumable item pickups.  Warm green to visually
/// separate from weapons at a glance.
const INDICATOR_ITEM_COLOR: Color = Color::srgba(0.45, 1.0, 0.55, 0.55);

/// Airborne pickup carrier.  Inserted by drop_{weapon,item}_system
/// at spawn time; removed by `pickup_physics_system` when the item
/// touches ground.
#[derive(Component, Debug, Clone)]
pub struct PickupBallistic {
    pub velocity: Vec3,
}

impl PickupBallistic {
    /// Randomized cone-up launch direction × `PICKUP_LAUNCH_SPEED`.
    /// Mirrors the C++ launch: random X/Z, Y biased 0.85..0.95
    /// upward, vector normalized then scaled.
    pub fn random_toss() -> Self {
        use rand::Rng;
        let mut rng = rand::rng();
        let dir = Vec3::new(
            rng.random_range(-1.0..1.0),
            rng.random_range(0.85..0.95),
            rng.random_range(-1.0..1.0),
        )
        .normalize_or_zero();
        Self {
            velocity: dir * PICKUP_LAUNCH_SPEED,
        }
    }
}

/// Marker on the child entity that owns the indicator quad mesh.
/// Used both to recognize already-decorated pickups (so we don't
/// re-attach a second indicator) and as the query target for the
/// per-frame spin system.
#[derive(Component, Debug, Clone, Copy)]
pub struct PickupIndicator;

/// Cached unit-disk mesh shared by every indicator instance.  Built
/// lazily on first attach to avoid creating it before the asset
/// servers exist.
#[derive(Resource, Default)]
pub struct PickupIndicatorMesh(pub Option<Handle<Mesh>>);

fn get_or_create_indicator_mesh(
    cached: &mut PickupIndicatorMesh,
    meshes: &mut Assets<Mesh>,
) -> Handle<Mesh> {
    if let Some(h) = &cached.0 {
        return h.clone();
    }
    // Flat disk in the local XZ plane (normal = +Y), so the default
    // child rotation already lays it horizontal above the pickup.
    let h = meshes.add(Mesh::from(Circle::new(INDICATOR_SIZE * 0.5)));
    cached.0 = Some(h.clone());
    h
}

/// Integrate ballistic pickups one FixedUpdate step.  Applies
/// gravity, advances Transform, then casts a downward ray to
/// detect the landing surface.  When the integrated position would
/// dip below the ground hit, the pickup is snapped to the hit
/// point and `PickupBallistic` is removed.
pub fn pickup_physics_system(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut Transform, &mut PickupBallistic)>,
    spatial: avian3d::prelude::SpatialQuery,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (entity, mut tf, mut bal) in &mut query {
        // Gravity is applied as a half-step (symplectic Euler keeps
        // the arc shape stable for the low tick rates we run at).
        bal.velocity.y -= PICKUP_GRAVITY * dt;

        let start = tf.translation;
        let next = start + bal.velocity * dt;

        // Probe downward from the pre-step position to find the
        // ground beneath the arc.  Excluding the pickup itself
        // matters once we add it as a physics body in the future;
        // it's a harmless no-op today.
        let filter = avian3d::prelude::SpatialQueryFilter::from_excluded_entities([entity]);
        let ground_hit = spatial.cast_ray(
            start + Vec3::Y * 0.05,
            Dir3::NEG_Y,
            PICKUP_GROUND_PROBE,
            true,
            &filter,
        );

        if let Some(hit) = ground_hit {
            let ground_y = (start.y + 0.05) - hit.distance;
            if next.y <= ground_y + 0.02 && bal.velocity.y <= 0.0 {
                // Touchdown — snap to surface, lock in place, drop
                // the ballistic component so the pickup is idle.
                tf.translation = Vec3::new(next.x, ground_y + 0.02, next.z);
                commands.entity(entity).remove::<PickupBallistic>();
                continue;
            }
        }
        tf.translation = next;
    }
}

/// Attach a spinning indicator child quad to any Pickup that doesn't
/// already have one.  Runs on Update because the child mesh isn't
/// physics-critical and we want it visible as soon as a pickup is
/// in the world.
pub fn attach_pickup_indicator_system(
    mut commands: Commands,
    mut cached_mesh: ResMut<PickupIndicatorMesh>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    pickups: Query<(Entity, &Pickup), Added<Pickup>>,
) {
    for (pickup_entity, pickup) in &pickups {
        let mesh = get_or_create_indicator_mesh(&mut cached_mesh, &mut meshes);
        let color = match pickup {
            Pickup::Weapon { .. } => INDICATOR_WEAPON_COLOR,
            Pickup::Item { .. } => INDICATOR_ITEM_COLOR,
        };
        let material = materials.add(StandardMaterial {
            base_color: color,
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            cull_mode: None,
            double_sided: true,
            ..default()
        });

        commands.spawn((
            Name::new("PickupIndicator"),
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_translation(Vec3::Y * INDICATOR_HOVER_OFFSET),
            Visibility::Visible,
            bevy::light::NotShadowCaster,
            bevy::light::NotShadowReceiver,
            ChildOf(pickup_entity),
            PickupIndicator,
        ));
    }
}

/// Spin every indicator about its local +Y axis at a constant rate.
/// The mesh is laid flat (normal +Y) so this gives the classic
/// hovering, rotating disk look above the pickup.
pub fn animate_pickup_indicator_system(
    time: Res<Time>,
    mut indicators: Query<&mut Transform, With<PickupIndicator>>,
) {
    let dt = time.delta_secs();
    let step = Quat::from_rotation_y(INDICATOR_ROTATION_RATE * dt);
    for mut tf in &mut indicators {
        tf.rotation = step * tf.rotation;
    }
}
