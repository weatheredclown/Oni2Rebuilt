/*
 * combat/mod.rs — CombatPlugin: hit detection, reactions, combos, and death.
 *
 * Startup: setup_combat_materials creates and inserts CombatMaterials (fist
 * sphere meshes / materials used for sandbox visual feedback).
 * FixedUpdate chain: ground detection → attack sync → hit detection → about-to-
 * be-hit → hit reaction → combo tracking → death → telemetry → cleanup → timers.
 */
pub mod bundles;
pub mod components;
pub mod events;
pub mod faction;
pub mod hitbox;
pub mod locks;
pub mod systems;

pub use locks::locked_movement;

pub use bundles::{CreaturePhysicsBundle, FighterBundle};

use bevy::prelude::*;

use crate::menu::AppState;

fn setup_combat_materials(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let fist_startup = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 1.0, 0.2),
        emissive: LinearRgba::new(1.0, 1.0, 0.2, 1.0),
        ..default()
    });
    let fist_active = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.4, 0.1),
        emissive: LinearRgba::new(3.0, 1.2, 0.3, 1.0),
        ..default()
    });
    let fist_recovery = materials.add(StandardMaterial {
        base_color: Color::srgb(0.8, 0.5, 0.2),
        emissive: LinearRgba::new(0.4, 0.25, 0.1, 1.0),
        ..default()
    });
    let fist_mesh = meshes.add(Sphere::new(0.15));

    commands.insert_resource(components::CombatMaterials {
        fist_startup,
        fist_active,
        fist_recovery,
        fist_mesh,
    });
}

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(faction::FactionPlugin)
            .add_systems(Startup, setup_combat_materials)
            .add_message::<events::AttackMessage>()
            .add_message::<events::DamageMessage>()
            .add_message::<events::DeathMessage>()
            .add_message::<events::AboutToBeHitMessage>()
            .add_message::<events::HitReactionMessage>()
            .add_message::<events::InjureMessage>()
            .add_message::<events::StrikeConnectedEvent>()
            .add_systems(
                FixedUpdate,
                (
                    // Apply post-attack `end_rotation_notches` on the
                    // SAME tick the anim crosses its last frame.  Must
                    // run AFTER `update_oni2_animation` (which emits
                    // `AnimEndedMessage` when the anim crosses its
                    // last frame) and BEFORE the rest of the combat
                    // chain (and before any FSM/animator system that
                    // might switch to a new anim) so the rotation
                    // lands before the new anim's first rendered
                    // frame.  Without the explicit `.after`, the
                    // scheduler could place this system before
                    // `update_oni2_animation` in the same tick — the
                    // `AnimEndedMessage` then waits one tick for the
                    // reader cursor, leaving a one-frame visual where
                    // the new anim already rendered at the old
                    // (un-rotated) Transform.
                    systems::apply_end_rotation_on_anim_end_system
                        .after(crate::oni2_loader::animation::update_oni2_animation),
                    systems::ground_detection_system,
                    systems::hit_detection_system,
                    systems::process_strike_connections_system,
                    systems::about_to_be_hit_system,
                    systems::injure_system,
                    systems::hit_reaction_system,
                    systems::combo_tracking_system,
                    systems::death_system,
                    systems::telemetry_combat_system,
                    systems::death_cleanup_system,
                    systems::death_timer_system,
                    systems::fighter_rotation_sync_system,
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                systems::attack_sync_system
                    .after(crate::statemachine::fsm_update_system)
                    .after(crate::statemachine::animator_update_system)
                    .after(crate::oni2_loader::animation::update_oni2_animation)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
