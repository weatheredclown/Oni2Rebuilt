/*
 * door.rs — Door logic subsystem.
 *
 * Implements the ActorSpecificAction observer and DoorComponent.
 * Integrates doors dynamically into the AI NavGraph for path blocking.
 */
use crate::ai::navigation::NavGraph;
use crate::oni2_loader::Oni2Entity;
use crate::oni2_loader::animation::{Oni2AnimLibrary, Oni2AnimState};
use bevy::prelude::*;

pub struct DoorPlugin;

impl Plugin for DoorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (register_door_system,));

        // Register the observer for ActorSpecificAction
        app.add_observer(handle_actor_specific_action);
    }
}

#[derive(Event, Clone, Debug)]
pub struct ActorSpecificAction {
    pub action: String,
    pub target: Entity,
}

#[derive(Component)]
pub struct DoorComponent {
    pub is_open: bool,
    pub graph_door_index: Option<usize>,
}

impl Default for DoorComponent {
    fn default() -> Self {
        Self {
            is_open: false,
            graph_door_index: None,
        }
    }
}

#[derive(Component)]
pub struct RegisteredDoor;

fn register_door_system(
    mut commands: Commands,
    query: Query<
        (Entity, &Oni2Entity, &Transform),
        (Without<RegisteredDoor>, Without<DoorComponent>),
    >,
    mut opt_nav_graph: Option<ResMut<NavGraph>>,
) {
    let Some(mut nav_graph) = opt_nav_graph else {
        return; // NavGraph hasn't been loaded yet
    };

    for (entity, oni_ent, tf) in &query {
        let name_lower = oni_ent.name.to_lowercase();
        if name_lower.contains("actorspecific") || name_lower.contains("door") {
            let door_idx = nav_graph.add_door(tf.translation, 2.0); // Assuming 2.0m radius cylinder
            commands.entity(entity).insert((
                RegisteredDoor,
                DoorComponent {
                    is_open: false, // Default state is closed
                    graph_door_index: Some(door_idx),
                },
            ));
            info!(
                "Registered door {} with nav graph door index {}",
                oni_ent.name, door_idx
            );
        }
    }
}

fn handle_actor_specific_action(
    trigger: On<ActorSpecificAction>,
    mut commands: Commands,
    mut q_doors: Query<(
        Option<&mut DoorComponent>,
        Option<&Oni2AnimLibrary>,
        Option<&mut Oni2AnimState>,
    )>,
    mut opt_nav_graph: Option<ResMut<NavGraph>>,
) {
    let ev = trigger.event();

    if ev.action.eq_ignore_ascii_case("pulse") {
        if let Ok((mut opt_door, opt_lib, opt_state)) = q_doors.get_mut(ev.target) {
            let mut door_idx = None;
            let is_now_open;

            if let Some(ref mut door) = opt_door {
                door.is_open = !door.is_open;
                is_now_open = door.is_open;
                door_idx = door.graph_door_index;
            } else {
                // Lazy insert if it didn't register naturally from the name check
                commands.entity(ev.target).insert(DoorComponent {
                    is_open: true,
                    graph_door_index: None,
                });
                is_now_open = true;
            }

            // Play the appropriate Open or Close animation sequence
            let anim_name = if is_now_open { "OpenDoor" } else { "CloseDoor" };

            if let (Some(lib), Some(mut state)) = (opt_lib, opt_state) {
                if lib.play(anim_name, &mut state) {
                    state.looping = false; // Corresponds to ANIM_HOLD / ANIM_RESTART
                    state.speed_multiplier = 1.0;
                } else {
                    warn!(
                        "Failed to play animation {} on door {:?}",
                        anim_name, ev.target
                    );
                }
            } else {
                warn!(
                    "Door {:?} missing animator, skipping visual sequence",
                    ev.target
                );
            }

            // Sync the NavGraph state
            if let Some(idx) = door_idx {
                if let Some(mut nav_graph) = opt_nav_graph.as_mut() {
                    nav_graph.set_door_state(idx, is_now_open);
                }
            }

            info!(
                "Action processed on Door {:?}! Current state: {}",
                ev.target,
                if is_now_open { "OPEN" } else { "CLOSED" }
            );
        }
    } else {
        warn!("Unrecognized ActorSpecific action '{}'", ev.action);
    }
}
