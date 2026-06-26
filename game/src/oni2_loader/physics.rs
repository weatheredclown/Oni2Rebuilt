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

/// Resolves character-vs-character overlap so that a moving character runs
/// into another as if it were an immovable wall — no character's locomotion
/// ever pushes another character.  Replaces Avian's mutual dynamic shove
/// (which let the player bulldoze stationary AI and let crowds stand inside
/// each other).  This is the Avian/Tnua equivalent of the legacy crmover
/// `SetMoveable(false)` behaviour, where a passive character was a solid wall
/// the active mover swept along.
///
/// The mover is identified by velocity (who is moving *into* the contact),
/// not by attack state — locomotion is the thing that must not push.
pub fn character_collision_presolve_system(
    mut contact_graph: ResMut<ContactGraph>,
    fighter_query: Query<&FighterState>,
    mut actor_query: Query<(&mut Position, &mut LinearVelocity)>,
) {
    for contacts in contact_graph.iter_active_touching_mut() {
        let (e1, e2) = (contacts.collider1, contacts.collider2);
        if fighter_query.contains(e1) && fighter_query.contains(e2) {
            process_character_collision_presolve(e1, e2, contacts, &fighter_query, &mut actor_query);
        }
    }
    // Characters spawn with `SleepingDisabled`, so a char-vs-char pair should
    // never appear here; kept as a backstop in case a character body is ever
    // allowed to sleep.
    for contacts in contact_graph.iter_sleeping_touching_mut() {
        let (e1, e2) = (contacts.collider1, contacts.collider2);
        if fighter_query.contains(e1) && fighter_query.contains(e2) {
            process_character_collision_presolve(e1, e2, contacts, &fighter_query, &mut actor_query);
        }
    }
}

fn process_character_collision_presolve(
    e1: Entity,
    e2: Entity,
    contacts: &mut ContactPair,
    fighter_query: &Query<&FighterState>,
    actor_query: &mut Query<(&mut Position, &mut LinearVelocity)>,
) {
    let (Ok(fs1), Ok(fs2)) = (fighter_query.get(e1), fighter_query.get(e2)) else {
        return;
    };

    // Grapple exemption: while either side is grappling/being grappled the
    // grab system owns both transforms (it teleports the victim to a fixed
    // offset).  Suppress the manifold so the dynamic solver doesn't shove the
    // pair, but otherwise leave their positions to the grapple.
    let grappling = |fs: &FighterState| fs.grapple_target.is_some() || fs.is_being_grappled();
    if grappling(fs1) || grappling(fs2) {
        suppress_manifold(contacts);
        return;
    }

    // Aggregate the contact: averaged normal (points from e1 toward e2) and
    // the deepest penetration across all manifold points.
    let mut max_penetration = 0.0f32;
    let mut normal = Vec3::ZERO;
    let mut count = 0;
    for manifold in &contacts.manifolds {
        for point in &manifold.points {
            if point.penetration > max_penetration {
                max_penetration = point.penetration;
            }
        }
        normal += manifold.normal;
        count += 1;
    }
    if count == 0 || max_penetration <= 0.001 {
        return;
    }
    normal = normal.normalize_or_zero();
    if normal.length_squared() < 0.5 {
        return; // degenerate / opposing normals cancelled out
    }

    let Ok([(mut pos1, mut vel1), (mut pos2, mut vel2)]) =
        actor_query.get_many_mut([e1, e2])
    else {
        return;
    };

    // Velocity along the contact normal.  Normal points e1 -> e2, so a
    // positive `v1n` means char 1 is closing on char 2, and a negative `v2n`
    // means char 2 is closing on char 1.
    const MOVE_EPS: f32 = 0.05;
    let v1n = vel1.0.dot(normal);
    let v2n = vel2.0.dot(normal);
    let e1_moving_in = v1n > MOVE_EPS;
    let e2_moving_in = v2n < -MOVE_EPS;

    // Project out only the inward component of a *moving* character's velocity
    // so its own locomotion can't drive it through the other (it slides along
    // instead).  A stationary or receding character keeps its velocity — it's
    // an immovable wall, never shoved by the other's motion.
    if e1_moving_in {
        vel1.0 -= v1n * normal;
    }
    if e2_moving_in {
        vel2.0 -= v2n * normal;
    }

    // Depenetrate.  A character is only displaced to undo ITS OWN incursion:
    //   - exactly one moving  -> that one backs out 100%, the wall is unmoved;
    //   - both moving in       -> each backs off half (they met head-on);
    //   - neither moving in    -> residual static overlap (e.g. two idle
    //                             actors left embedded); separate gently 50/50
    //                             so they don't stand inside each other.
    let half = max_penetration * 0.5;
    match (e1_moving_in, e2_moving_in) {
        (true, false) => pos1.0 -= normal * max_penetration,
        (false, true) => pos2.0 += normal * max_penetration,
        _ => {
            pos1.0 -= normal * half;
            pos2.0 += normal * half;
        }
    }

    // Tell the solver these bodies are effectively separated so it applies no
    // positional bias push.  (NOTE: this alone does NOT stop the solver's
    // normal impulse against approaching velocity — see the velocity handling
    // above and the system ordering, which keep the mover from approaching.)
    suppress_manifold(contacts);
}

/// Mark a contact pair's points as separated so Avian's solver applies no
/// positional correction (we've resolved the overlap manually).
fn suppress_manifold(contacts: &mut ContactPair) {
    for manifold in &mut contacts.manifolds {
        for point in &mut manifold.points {
            point.penetration = -100.0;
        }
    }
}
