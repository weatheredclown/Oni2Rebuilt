/*
 * combat/bundles.rs — grouped component spawn helpers for fighter entities.
 *
 * Every actor that participates in the combat / fight pipeline needs the
 * same core set of components at spawn time.  The list was duplicated at
 * three spawn sites (setup_scene's attach-to-layout path, the fallback
 * capsule spawn, and layout_loader's AI creature attach).  Bundling the
 * list here means adding a new combat subsystem component is a one-file
 * edit — the spawn sites don't need to know.
 *
 * Why present-at-spawn and not lazy-attach via `ensure_*` systems:
 * `commands.insert` flushes at end-of-stage, so the FIRST tick after
 * spawn would see missing components.  Bundles (like `AnimatorBundle`)
 * are the race-free path.
 */
use avian3d::prelude::*;
use bevy::prelude::*;

use super::components::{AboutToBeHit, AttackState, ComboTracker, HitReaction};
use crate::fight::components::{BlockLibrary, FighterState, FighterType};

/// Full combat + fight loadout applied to every entity that can attack,
/// be attacked, react, combo, block, or participate in fight-stance
/// logic.  Applies to both the player and AI-driven creatures.
///
/// Add a field here when a new combat subsystem needs a per-entity
/// component at spawn time.  Intentionally duplicates the old
/// three-spawn-site inserts so they collapse to a single `.insert(FighterBundle::default())`.
#[derive(Bundle, Default)]
pub struct FighterBundle {
    pub attack_state: AttackState,
    pub combo_tracker: ComboTracker,
    pub hit_reaction: HitReaction,
    pub about_to_be_hit: AboutToBeHit,
    pub fighter_state: FighterState,
    pub fighter_type: FighterType,
    pub block_library: BlockLibrary,
    /// Per-character mover tuning (speeds, turn rate/inertia, capsule hotdog)
    /// + steering state.  This is the `Default` safety fallback; the creature
    /// spawn path overrides it with a `MoverData` resolved from `components.xml`
    /// + the actor's `<Mover>` block.  See `mover::steering`.
    pub mover: crate::mover::steering::MoverData,
}

// ---------------------------------------------------------------------------
// Collision layers
// ---------------------------------------------------------------------------

/// Physics collision layers.  The first variant maps to bit 0, which is also
/// Avian's `LayerMask::DEFAULT` — i.e. the membership every collider that
/// doesn't set `CollisionLayers` (all world / level geometry, projectiles,
/// triggers) carries.  So `Default` means "everything that isn't explicitly
/// layered."
#[derive(PhysicsLayer, Default, Clone, Copy, Debug)]
pub enum GameLayer {
    /// Bit 0 — world geometry and everything without an explicit layer.
    #[default]
    Default,
    /// Characters (player + AI creatures).
    Character,
}

/// `CollisionLayers` for a character body: a member of `Character` that
/// collides only with `Default` (the world).  Characters therefore generate
/// **no** physics contacts with each other — Avian's solver never pushes one
/// character with another's momentum.  Character-vs-character overlap is kept
/// solid manually by `character_separation_system` (an analytic upright-capsule
/// depenetration), where the moving character yields and the other is an
/// immovable wall.  This is the legacy `SetMoveable(false)` behaviour without
/// fighting the dynamic solver.
pub fn character_collision_layers() -> CollisionLayers {
    CollisionLayers::new(GameLayer::Character, GameLayer::Default)
}

// ---------------------------------------------------------------------------
// CreaturePhysicsBundle
// ---------------------------------------------------------------------------

/// Physics capsule + locked rotational axes + linear velocity + ground
/// shape-caster.  Applied to every entity that needs Avian-driven
/// movement — the fallback player, AI creatures loaded from a layout,
/// etc.
///
/// Construct via [`CreaturePhysicsBundle::new`] since capsule radius /
/// length vary per creature.  No `Default` impl on purpose — a capsule
/// with zero dimensions would silently pass through the world.
#[derive(Bundle)]
pub struct CreaturePhysicsBundle {
    pub rigid_body: RigidBody,
    pub collider: Collider,
    pub locked_axes: LockedAxes,
    pub linear_velocity: LinearVelocity,
    pub shape_caster: ShapeCaster,
    /// Explicit GravityScale so `animator::gravity::gravity_sync_system`
    /// can see and update it.  Without this component, Avian still uses
    /// a default 1.0 scale but the sync query skips the entity — so
    /// jump's `jump=3.2` gravity multiplier never reaches Avian and the
    /// character falls at base gravity (floaty).  Default 1.0 here
    /// mirrors the `GravityModifiers` base.
    pub gravity_scale: GravityScale,
    /// Characters never sleep.  `character_separation_system` resolves
    /// character-vs-character overlap by editing `Position` directly; a
    /// sleeping body ignores those edits, so two idle actors would otherwise
    /// stay frozen standing inside each other.  There are only a handful of
    /// characters, so keeping them awake costs nothing.
    pub sleeping_disabled: SleepingDisabled,
    /// Members of the `Character` layer don't collide with each other, so the
    /// solver never pushes one character with another's momentum.  See
    /// [`character_collision_layers`].
    pub collision_layers: CollisionLayers,
}

impl CreaturePhysicsBundle {
    /// Build a Dynamic capsule rigid body with rotation locked on all
    /// three axes (upright) and a downward sphere shape-caster for
    /// ground detection.  `capsule_radius` / `capsule_length` match
    /// Avian's `Collider::capsule` parameters.
    pub fn new(capsule_radius: f32, capsule_length: f32) -> Self {
        Self {
            rigid_body: RigidBody::Dynamic,
            collider: Collider::capsule(capsule_radius, capsule_length),
            locked_axes: LockedAxes::new()
                .lock_rotation_x()
                .lock_rotation_y()
                .lock_rotation_z(),
            linear_velocity: LinearVelocity::default(),
            shape_caster: ShapeCaster::new(
                Collider::sphere(0.35),
                Vec3::NEG_Y * 0.5,
                Quat::default(),
                Dir3::NEG_Y,
            )
            .with_max_distance(0.3),
            gravity_scale: GravityScale(1.0),
            sleeping_disabled: SleepingDisabled,
            collision_layers: character_collision_layers(),
        }
    }
}
