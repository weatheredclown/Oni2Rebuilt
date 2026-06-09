/*
 * inventory/mod.rs — InventoryPlugin.
 *
 * Registers:
 *   • Resources: WeaponItemRegistry, ItemRegistry
 *   • Messages: Add/Delete/Drop/Use/PickUp/Select/SetActive/InventoryFire|Stop|Aim
 *               (inbound), Weapon|ItemPickedUp / Weapon|ItemDropped /
 *               ItemEffectApplied (outbound), DropAllMessage (internal routing).
 *   • Systems (FixedUpdate, gated on AppState::InGame) in the order:
 *       add_* / delete_* / drop_* / drop_all / use_item / pickup → selection →
 *       forward_fire → drop_on_death
 *
 * Depends on weapons:: (to spawn Weapon components + forward Fire/Aim),
 * fight::SuperMeterAddEvent (for super-meter effects), and
 * combat::{Health, DeathMessage}.
 */
pub mod components;
pub mod events;
pub mod pickup_fx;
pub mod systems;

use bevy::prelude::*;

use crate::menu::AppState;

pub use components::{ItemRegistry, WeaponItemRegistry};
pub use events::{
    AddItemByNameMessage, AddWeaponByNameMessage, DeleteItemMessage, DeleteWeaponMessage,
    DropAllMessage, DropItemMessage, DropWeaponMessage, InventoryAimMessage, InventoryFireMessage,
    InventoryStopFiringMessage, ItemDroppedMessage, ItemEffectAppliedMessage, ItemPickedUpMessage,
    PickUpItemsMessage, SelectNextItemMessage, SelectNextWeaponMessage, SelectPrevItemMessage,
    SelectPrevWeaponMessage, SetActiveWeaponMessage, UseItemMessage, WeaponDroppedMessage,
    WeaponPickedUpMessage,
};

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app
            // --- Resources ---
            .init_resource::<WeaponItemRegistry>()
            .init_resource::<ItemRegistry>()
            .init_resource::<pickup_fx::PickupIndicatorMesh>()
            // --- Messages (inbound) ---
            .add_message::<AddWeaponByNameMessage>()
            .add_message::<AddItemByNameMessage>()
            .add_message::<DeleteWeaponMessage>()
            .add_message::<DeleteItemMessage>()
            .add_message::<DropWeaponMessage>()
            .add_message::<DropItemMessage>()
            .add_message::<DropAllMessage>()
            .add_message::<UseItemMessage>()
            .add_message::<PickUpItemsMessage>()
            .add_message::<SelectNextWeaponMessage>()
            .add_message::<SelectPrevWeaponMessage>()
            .add_message::<SelectNextItemMessage>()
            .add_message::<SelectPrevItemMessage>()
            .add_message::<SetActiveWeaponMessage>()
            .add_message::<InventoryFireMessage>()
            .add_message::<InventoryStopFiringMessage>()
            .add_message::<InventoryAimMessage>()
            // --- Messages (outbound) ---
            .add_message::<WeaponPickedUpMessage>()
            .add_message::<ItemPickedUpMessage>()
            .add_message::<WeaponDroppedMessage>()
            .add_message::<ItemDroppedMessage>()
            .add_message::<ItemEffectAppliedMessage>()
            // --- Systems ---
            //
            // pending_inventory_system → add_weapon_system must be strictly
            // ordered AND command-buffer-flushed between them.  The pending
            // system inserts an `Inventory` component via deferred Commands
            // and then writes an `AddWeaponByNameMessage`; without a flush
            // between the two, add_weapon_system's `query.get_mut(msg.entity)`
            // runs against a world where the Inventory isn't yet present and
            // silently drops the message.  `.chain()` inserts an automatic
            // apply_deferred between consecutive entries, which fixes the
            // race while still letting every other system run in parallel.
            .add_systems(
                FixedUpdate,
                (
                    (
                        systems::pending_inventory_system,
                        systems::add_weapon_system,
                    )
                        .chain(),
                    systems::add_item_system,
                    systems::delete_weapon_system,
                    systems::delete_item_system,
                    systems::drop_weapon_system,
                    systems::drop_item_system,
                    systems::drop_all_system
                        .before(systems::drop_weapon_system)
                        .before(systems::drop_item_system),
                    systems::use_item_system,
                    systems::player_auto_pickup_system
                        .before(systems::pickup_system),
                    systems::pickup_system
                        .before(systems::add_weapon_system)
                        .before(systems::add_item_system),
                    systems::select_next_weapon_system,
                    systems::select_prev_weapon_system,
                    systems::select_next_item_system,
                    systems::select_prev_item_system,
                    systems::set_active_weapon_system,
                    systems::inventory_forward_fire_system,
                    systems::scheduled_stop_firing_system
                        .before(systems::inventory_forward_fire_system),
                    systems::drop_on_death_system.before(systems::drop_all_system),
                    pickup_fx::pickup_physics_system,
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                (
                    pickup_fx::attach_pickup_indicator_system,
                    pickup_fx::animate_pickup_indicator_system,
                )
                    .run_if(in_state(AppState::InGame)),
            );
    }
}
