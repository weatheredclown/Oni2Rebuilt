use avian3d::prelude::*;
use bevy::prelude::*;

pub fn octree_one_way_contact_system(
    mut contact_graph: ResMut<ContactGraph>,
    octree_query: Query<&GlobalTransform, With<crate::oni2_loader::components::OneWayOctreeBound>>,
    actor_query: Query<(&Position, &LinearVelocity)>,
) {
    // Iterate active and sleeping touching pairs separately (two sequential mut borrows).
    for contacts in contact_graph.iter_active_touching_mut() {
        process_one_way(contacts, &octree_query, &actor_query);
    }
    for contacts in contact_graph.iter_sleeping_touching_mut() {
        process_one_way(contacts, &octree_query, &actor_query);
    }
}

fn process_one_way(
    contacts: &mut ContactPair,
    octree_query: &Query<&GlobalTransform, With<crate::oni2_loader::components::OneWayOctreeBound>>,
    actor_query: &Query<(&Position, &LinearVelocity)>,
) {
    let e1 = contacts.collider1;
    let e2 = contacts.collider2;

    let (_octree_ent, actor_ent) = if octree_query.contains(e1) && actor_query.contains(e2) {
        (e1, e2)
    } else if octree_query.contains(e2) && actor_query.contains(e1) {
        (e2, e1)
    } else {
        return;
    };

    if let Ok((_, actor_velocity)) = actor_query.get(actor_ent) {
        let mut should_disable = false;
        for manifold in contacts.manifolds.iter() {
            // normal points from collider1 to collider2; flip if the actor is collider1.
            let normal = if e1 == actor_ent {
                -manifold.normal
            } else {
                manifold.normal
            };

            // Positive dot product means the actor is moving away from (or parallel to) the
            // surface — suppress the contact so the actor can pass through.
            if actor_velocity.0.dot(normal) > -0.05 {
                should_disable = true;
                break;
            }
        }
        if should_disable {
            contacts.manifolds.clear();
        }
    }
}
