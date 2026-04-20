/*
 * inventory/events.rs — Message types for the inventory subsystem.
 *
 * Inbound:
 *   AddWeaponByNameMessage / AddItemByNameMessage
 *   DropWeaponMessage / DropItemMessage
 *   DeleteWeaponMessage / DeleteItemMessage
 *   UseItemMessage
 *   PickUpItemsMessage           — sphere pickup around the requesting entity
 *   SelectNext/PrevWeapon/Item Messages
 *   SetActiveWeaponMessage
 *   InventoryFireMessage / InventoryAimMessage / InventoryStopFiringMessage
 *       — translate to weapons::*Message on the current weapon entity
 *
 * Outbound:
 *   WeaponPickedUpMessage / ItemPickedUpMessage
 *   WeaponDroppedMessage / ItemDroppedMessage
 *   ItemEffectAppliedMessage      — informational (e.g. HUD flash)
 */
use bevy::prelude::*;

use super::components::ItemEffect;
use crate::weapons::AimTarget;

// ---------------------------------------------------------------------------
// Inbound — registry operations
// ---------------------------------------------------------------------------

#[derive(Message, Clone)]
pub struct AddWeaponByNameMessage {
    pub entity: Entity,
    pub weapon_name: String,
}

#[derive(Message, Clone)]
pub struct AddItemByNameMessage {
    pub entity: Entity,
    pub item_name: String,
    pub quantity: u32,
}

#[derive(Message, Clone)]
pub struct DeleteWeaponMessage {
    pub entity: Entity,
    pub slot_index: usize,
}

#[derive(Message, Clone)]
pub struct DeleteItemMessage {
    pub entity: Entity,
    pub slot_index: usize,
}

#[derive(Message, Clone)]
pub struct DropWeaponMessage {
    pub entity: Entity,
    pub slot_index: usize,
}

#[derive(Message, Clone)]
pub struct DropItemMessage {
    pub entity: Entity,
    pub slot_index: usize,
}

/// Drop every held weapon and item.  Used by the death handler.
#[derive(Message, Clone)]
pub struct DropAllMessage {
    pub entity: Entity,
}

// ---------------------------------------------------------------------------
// Inbound — use / pickup
// ---------------------------------------------------------------------------

#[derive(Message, Clone)]
pub struct UseItemMessage {
    pub entity: Entity,
    pub slot_index: usize,
    /// Optional target entity for effects that need one (heal-ally etc.).
    /// If None, effects are applied to `entity`.
    pub target: Option<Entity>,
}

/// Sphere pickup around the wielder's position.  Processed by `pickup_system`.
#[derive(Message, Clone)]
pub struct PickUpItemsMessage {
    pub entity: Entity,
    /// If Some, only pick up this specific pickup entity; else sweep the radius.
    pub specific_pickup: Option<Entity>,
}

// ---------------------------------------------------------------------------
// Inbound — selection / forwarding to the held weapon
// ---------------------------------------------------------------------------

#[derive(Message, Clone)]
pub struct SelectNextWeaponMessage {
    pub entity: Entity,
}
#[derive(Message, Clone)]
pub struct SelectPrevWeaponMessage {
    pub entity: Entity,
}
#[derive(Message, Clone)]
pub struct SelectNextItemMessage {
    pub entity: Entity,
}
#[derive(Message, Clone)]
pub struct SelectPrevItemMessage {
    pub entity: Entity,
}
#[derive(Message, Clone)]
pub struct SetActiveWeaponMessage {
    pub entity: Entity,
    pub slot_index: usize,
}

/// Forward a trigger-pull to the currently-selected weapon.
#[derive(Message, Clone)]
pub struct InventoryFireMessage {
    pub entity: Entity,
    pub multi_dir: bool,
}
#[derive(Message, Clone)]
pub struct InventoryStopFiringMessage {
    pub entity: Entity,
}
#[derive(Message, Clone)]
pub struct InventoryAimMessage {
    pub entity: Entity,
    pub target: AimTarget,
}

// ---------------------------------------------------------------------------
// Outbound
// ---------------------------------------------------------------------------

#[derive(Message, Clone)]
pub struct WeaponPickedUpMessage {
    pub wielder: Entity,
    pub weapon_name: String,
    pub weapon_entity: Entity,
}

#[derive(Message, Clone)]
pub struct ItemPickedUpMessage {
    pub wielder: Entity,
    pub item_name: String,
    pub quantity: u32,
}

#[derive(Message, Clone)]
pub struct WeaponDroppedMessage {
    pub wielder: Entity,
    pub weapon_name: String,
    pub pickup_entity: Entity,
}

#[derive(Message, Clone)]
pub struct ItemDroppedMessage {
    pub wielder: Entity,
    pub item_name: String,
    pub quantity: u32,
    pub pickup_entity: Entity,
}

/// Emitted whenever an ItemEffect has been resolved on a target.  Useful for
/// HUD feedback / sound cues.
#[derive(Message, Clone)]
pub struct ItemEffectAppliedMessage {
    pub source: Entity,
    pub target: Entity,
    pub effect: ItemEffect,
}
