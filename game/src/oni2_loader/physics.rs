use avian3d::prelude::*;
use bevy::prelude::*;

use crate::oni2_loader::components::{OctreeInteriorRef, OneWayOctreeBound};
use crate::fight::components::FighterState;

pub fn octree_one_way_contact_system(
    mut contact_graph: ResMut<ContactGraph>,
    octree_query: Query<(&GlobalTransform, &OctreeInteriorRef), With<OneWayOctreeBound>>,
    actor_query: Query<(&Position, &LinearVelocity), Without<OneWayOctreeBound>>,
) {
    for contacts in contact_graph.iter_active_touching_mut() {
        process_one_way(contacts, &octree_query, &actor_query);
    }
    for contacts in contact_graph.iter_sleeping_touching_mut() {
        process_one_way(contacts, &octree_query, &actor_query);
    }
}

fn process_one_way(
    contacts: &mut ContactPair,
    octree_query: &Query<(&GlobalTransform, &OctreeInteriorRef), With<OneWayOctreeBound>>,
    actor_query: &Query<(&Position, &LinearVelocity), Without<OneWayOctreeBound>>,
) {
    let e1 = contacts.collider1;
    let e2 = contacts.collider2;

    let (octree_ent, actor_ent) = if octree_query.contains(e1) && actor_query.contains(e2) {
        (e1, e2)
    } else if octree_query.contains(e2) && actor_query.contains(e1) {
        (e2, e1)
    } else {
        return;
    };

    let Ok((octree_gt, interior_local)) = octree_query.get(octree_ent) else {
        return;
    };
    let Ok((actor_pos, actor_velocity)) = actor_query.get(actor_ent) else {
        return;
    };

    // The sub-bound centroid is a point known to lie inside the playable cell.
    // Transform it into world space so we can side-test contact points against
    // a stable interior reference, independent of contact-normal orientation
    // (which by Avian convention always points from surface toward the body
    // and therefore can't tell us which side of the wall is "interior").
    let interior_world = octree_gt.transform_point(interior_local.0);

    let suppress: Vec<bool> = contacts
        .manifolds
        .iter()
        .map(|manifold| {
            // Avian's manifold.normal points collider1 -> collider2; flip so it
            // always points octree -> actor (the contact push direction onto
            // the actor's body).
            let push_normal = if e1 == actor_ent {
                -manifold.normal
            } else {
                manifold.normal
            };

            // Floor case: surface pushes the actor mostly upward.  Legacy
            // "walk-through" / one-way octree boundaries are walls — entering
            // a room horizontally — so a near-vertical push direction means
            // we're standing on a floor/ledge and must stay solid regardless
            // of which side of the centroid the actor sits on.  Without this,
            // a freshly teleported / ground-snapped actor on top of any
            // OneWayOctreeBound surface gets ghosted as soon as gravity gives
            // it any downward velocity (the velocity check below trips), and
            // they tunnel through the floor down to whatever's beneath.  The
            // player's post-cutscene teleport-onto-BCStart was the canonical
            // case caught.  Threshold 0.5 = ≥30° from horizontal, which still
            // treats steep ramps as floors but excludes near-vertical walls.
            if push_normal.y > 0.5 {
                return false;
            }

            // Side test: is the actor on the same side of the contact point as
            // the known interior centroid? If yes, this is a normal collision
            // from inside the playable cell — keep it solid.
            let actor_on_interior = manifold.points.iter().any(|cp| {
                let to_actor = actor_pos.0 - cp.point;
                let to_interior = interior_world - cp.point;
                to_actor.dot(to_interior) > 0.0
            });

            if actor_on_interior {
                return false;
            }

            // Actor is outside the cell — ghost the manifold only while it is
            // actively moving into the surface, so stationary actors don't
            // tunnel through under gravity alone.
            actor_velocity.0.dot(push_normal) < -0.05
        })
        .collect();

    // Avian3D panics if we empty `manifolds` while iterating iter_*_touching_mut,
    // so we drive the solver to ignore the contact by setting a large negative
    // penetration on every contact point in the suppressed manifold instead.
    for (manifold, should_suppress) in contacts.manifolds.iter_mut().zip(suppress.iter()) {
        if *should_suppress {
            for contact in manifold.points.iter_mut() {
                contact.penetration = -100.0;
            }
        }
    }
}

/// Keeps characters solid against each other while ensuring no character's
/// locomotion ever pushes another — the legacy `SetMoveable(false)` rule, where
/// a moving character runs into others as if they were immovable walls.
///
/// Characters don't physically collide in Avian (they share the `Character`
/// collision layer, which only collides with the world — see
/// `combat::bundles::character_collision_layers`), so the dynamic solver never
/// transfers one character's momentum into another.  That means this analytic
/// pass is the *only* thing keeping characters from interpenetrating.  Because
/// the upright capsules can't rotate (axes locked) and all share the same
/// radius, their overlap is just two circles in the XZ plane — cheap to resolve
/// exactly without touching the contact graph.
///
/// Runs in `FixedUpdate` (after the movement systems write velocity, before the
/// physics step integrates it) and mutates `Position`/`LinearVelocity`
/// directly.  Doing this *outside* Avian's narrow-phase/solver sidesteps the
/// contact-graph/island bookkeeping that panics when mutated mid-step.
pub fn character_separation_system(
    mut characters: Query<(&mut Position, &mut LinearVelocity, &FighterState)>,
) {
    // Upright capsule: radius 0.4, cylinder length 1.2 -> total height 2.0.
    // Matches `combat::CreaturePhysicsBundle::new` and the Tnua bundle.
    const RADIUS: f32 = 0.4;
    const MIN_DIST: f32 = 2.0 * RADIUS; // centres closer than this overlap in XZ
    const HEIGHT: f32 = 2.0; // vertical extent; skip pairs on different levels
    const MOVE_EPS: f32 = 0.05;

    let mut pairs = characters.iter_combinations_mut::<2>();
    while let Some([(mut pos1, mut vel1, fs1), (mut pos2, mut vel2, fs2)]) = pairs.fetch_next() {
        // The grab system owns the transforms of a grappling pair.
        let grappling = |fs: &FighterState| fs.grapple_target.is_some() || fs.is_being_grappled();
        if grappling(fs1) || grappling(fs2) {
            continue;
        }

        let (p1, p2) = (pos1.0, pos2.0);

        // Vertical gate: capsules at very different heights (e.g. one on a
        // platform above the other) don't overlap, so don't separate them.
        if (p1.y - p2.y).abs() >= HEIGHT {
            continue;
        }

        // XZ overlap of two upright equal-radius capsules == two circles.
        let dx = p2.x - p1.x;
        let dz = p2.z - p1.z;
        let dist_sq = dx * dx + dz * dz;

        // Detect contact within a small margin to prevent frame-to-frame physics 
        // integration oscillation (strobe between RUN and STAND animations).
        // A margin of 0.05 (5cm) is sufficient to catch characters running at
        // max speed (6.0 m/s) before they penetrate on the next physics step.
        const MARGIN: f32 = 0.05;
        const DETECT_DIST: f32 = MIN_DIST + MARGIN;
        if dist_sq >= DETECT_DIST * DETECT_DIST {
            continue;
        }

        let dist = dist_sq.sqrt();

        // Normal points from char 1 toward char 2 (XZ).  Exact-overlap fallback
        // picks an arbitrary horizontal direction so they still split apart.
        let normal = if dist > 1e-4 {
            Vec3::new(dx / dist, 0.0, dz / dist)
        } else {
            Vec3::X
        };

        // Who is moving into the contact?  normal points 1 -> 2, so v1n > 0
        // means char 1 closes on char 2, and v2n < 0 means char 2 closes on 1.
        let v1n = vel1.0.dot(normal);
        let v2n = vel2.0.dot(normal);
        let e1_in = v1n > MOVE_EPS;
        let e2_in = v2n < -MOVE_EPS;

        // Project out only a *moving* character's inward velocity so it slides
        // along the other instead of driving through it.  A stationary or
        // receding character keeps its velocity — it's an immovable wall.
        if e1_in {
            vel1.0 -= v1n * normal;
        }
        if e2_in {
            vel2.0 -= v2n * normal;
        }

        // Depenetrate only if they are actually interpenetrating (dist < MIN_DIST).
        if dist < MIN_DIST {
            let penetration = MIN_DIST - dist;
            let half = penetration * 0.5;
            match (e1_in, e2_in) {
                (true, false) => pos1.0 -= normal * penetration,
                (false, true) => pos2.0 += normal * penetration,
                _ => {
                    pos1.0 -= normal * half;
                    pos2.0 += normal * half;
                }
            }
        }
    }
}
