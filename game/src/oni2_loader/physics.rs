use avian3d::prelude::*;
use bevy::prelude::*;

use crate::oni2_loader::components::{OctreeInteriorRef, OneWayOctreeBound};

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
