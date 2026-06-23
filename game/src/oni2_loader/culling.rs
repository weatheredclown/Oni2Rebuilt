/*
 * oni2_loader/culling.rs — BSP and portal visibility culling systems.
 *
 * Implements camera room lookup using the BSP tree and toggles room mesh visibility
 * based on the current room's portals.
 */
use crate::oni2_loader::parsers::bsp::ParsedBspTree;
use bevy::prelude::*;
use std::collections::HashSet;

/// Global resource containing the parsed BSP tree of the level.
#[derive(Resource, Debug, Clone)]
pub struct BspTree(pub ParsedBspTree);

/// Global resource mapping each room index to the indices of adjacent rooms (portals).
#[derive(Resource, Debug, Clone, Default)]
pub struct RoomPortals {
    pub adjacencies: Vec<Vec<usize>>,
    pub room_names: Vec<String>,
}

/// Component added to each spawned room geometry root entity.
#[derive(Component, Debug, Clone)]
pub struct RoomGeometryMarker {
    pub index: usize,
}

pub fn culling_system(
    bsp: Option<Res<BspTree>>,
    portals: Option<Res<RoomPortals>>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
    mut rooms_q: Query<(&RoomGeometryMarker, &mut Visibility)>,
) {
    let Some(bsp) = bsp else {
        return;
    };
    let Some(portals) = portals else {
        return;
    };

    // Find the active camera position
    let mut camera_pos = None;
    for (cam, tf) in &camera_q {
        if cam.is_active {
            camera_pos = Some(tf.translation());
            break;
        }
    }
    let Some(camera_pos) = camera_pos else {
        return;
    };

    // Query BSP to get camera's current room name
    let Some(room_name) = bsp.0.get_room_name_from_point(camera_pos) else {
        return;
    };

    if room_name.eq_ignore_ascii_case("Invalid") {
        // Camera is outside the level boundaries - keep all rooms visible
        for (_, mut vis) in &mut rooms_q {
            *vis = Visibility::Inherited;
        }
        return;
    }

    // Find the current room index
    let Some(current_idx) = portals
        .room_names
        .iter()
        .position(|name| name.eq_ignore_ascii_case(room_name))
    else {
        return;
    };

    // Build set of visible rooms: current room + adjacent rooms via portals
    let mut visible_rooms = HashSet::new();
    visible_rooms.insert(current_idx);

    if let Some(adj) = portals.adjacencies.get(current_idx) {
        for &adj_idx in adj {
            visible_rooms.insert(adj_idx);
        }
    }

    // Update visibility of room entities
    for (marker, mut vis) in &mut rooms_q {
        if visible_rooms.contains(&marker.index) {
            *vis = Visibility::Inherited;
        } else {
            *vis = Visibility::Hidden;
        }
    }
}
