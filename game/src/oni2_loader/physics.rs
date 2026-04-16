use avian3d::prelude::*;
use bevy::prelude::*;
use crate::debug::debug_name;

pub fn octree_one_way_contact_system(
    mut contact_graph: ResMut<ContactGraph>,
    octree_query: Query<Option<&ColliderAabb>, With<crate::oni2_loader::components::OneWayOctreeBound>>,
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
    octree_query: &Query<Option<&ColliderAabb>, With<crate::oni2_loader::components::OneWayOctreeBound>>,
    actor_query: &Query<(&Position, &LinearVelocity)>,
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

    let Ok((actor_pos, actor_velocity)) = actor_query.get(actor_ent) else {
        return;
    };

    // If the actor is already inside this octree's world-space AABB it's a resident of the
    // level — keep all contacts solid so it can't pass through the floor or walls.
    // Actors that start outside (Tim, Ted) will have positions outside the AABB and will
    // be allowed through until they cross the boundary.
    if let Ok(Some(oct_aabb)) = octree_query.get(octree_ent) {
        let p = actor_pos.0;
        if p.cmpge(oct_aabb.min).all() && p.cmple(oct_aabb.max).all() {
            return;
        }
    }

    // Actor is outside the level boundary — suppress contacts where it is moving into the
    // surface so it can enter freely.
    let mut should_disable = false;
    for manifold in contacts.manifolds.iter() {
        // normal points from collider1 to collider2; flip so it always points octree→actor.
        let normal = if e1 == actor_ent {
            -manifold.normal
        } else {
            manifold.normal
        };

        if actor_velocity.0.dot(normal) < -0.05 {
            should_disable = true;
            info!("Disabling one-way contact between {} ({}) and {} ({}) - Normal: {:?}",
                crate::debug::debug_type(e1), debug_name(e1),
                crate::debug::debug_type(e2), debug_name(e2), normal);
            break;
        }
    }

    if should_disable {
        // Avian3D panics if we empty contact.manifolds in iter_active_touching_mut.
        // Clear penetration depth instead to bypass collision response.
        for manifold in contacts.manifolds.iter_mut() {
            for contact in manifold.points.iter_mut() {
                contact.penetration = -100.0;
            }
        }
    }
}
