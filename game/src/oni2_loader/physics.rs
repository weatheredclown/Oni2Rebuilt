use avian3d::prelude::*;
use bevy::prelude::*;

pub fn octree_one_way_contact_system(
    mut contact_graph: ResMut<ContactGraph>,
    octree_query: Query<(), With<crate::oni2_loader::components::OneWayOctreeBound>>,
    actor_query: Query<(&Position, &LinearVelocity)>,
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
    octree_query: &Query<(), With<crate::oni2_loader::components::OneWayOctreeBound>>,
    actor_query: &Query<(&Position, &LinearVelocity)>,
) {
    let e1 = contacts.collider1;
    let e2 = contacts.collider2;

    let actor_ent = if octree_query.contains(e1) && actor_query.contains(e2) {
        e2
    } else if octree_query.contains(e2) && actor_query.contains(e1) {
        e1
    } else {
        return;
    };

    let Ok((actor_pos, actor_velocity)) = actor_query.get(actor_ent) else {
        return;
    };

    // Determine per-manifold whether to suppress.
    //
    // The octree mesh normals face INTO the playable space (inward). After flipping to
    // octree→actor, `normal` points toward the level interior. We project
    // (actor_center − contact_point) onto that normal: positive means the actor's center
    // is already on the interior side, so the contact must be kept solid. Only when the
    // actor is on the exterior side AND moving into the surface do we ghost the manifold
    // by zeroing penetration depth — letting enemies walk in through the boundary.
    let suppress: Vec<bool> = contacts
        .manifolds
        .iter()
        .map(|manifold| {
            // Flip so normal always points octree→actor (into level interior).
            let normal = if e1 == actor_ent {
                -manifold.normal
            } else {
                manifold.normal
            };

            // If the actor center is on the interior side of any contact point, keep solid.
            let actor_on_interior = manifold
                .points
                .iter()
                .any(|cp| (actor_pos.0 - cp.point).dot(normal) > 0.0);

            if actor_on_interior {
                return false;
            }

            // Actor is outside — suppress only if actively moving into the surface.
            actor_velocity.0.dot(normal) < -0.05
        })
        .collect();

    // Avian3D panics if we empty manifolds in iter_*_touching_mut; zero penetration instead.
    for (manifold, should_suppress) in contacts.manifolds.iter_mut().zip(suppress.iter()) {
        if *should_suppress {
            for contact in manifold.points.iter_mut() {
                contact.penetration = -100.0;
            }
        }
    }
}
