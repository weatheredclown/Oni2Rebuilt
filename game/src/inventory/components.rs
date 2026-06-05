/*
 * inventory/components.rs — Inventory components, item/weapon types, effects.
 *
 * Inventory (per-entity component)       — mirrors invInventoryComponent
 * InventoryTypeData (shared bootstrap)   — mirrors invInventoryComponentType
 *                                          (capacities + initial contents)
 * ItemBaseData                           — mirrors invBaseType (Name / Weight /
 *                                          Priority / SpawnTemplate / icons)
 * WeaponItemType                         — mirrors invWeaponType (MaxAmmo,
 *                                          InitialAmmo, WeaponType, AmmoType)
 * ConsumableItemType                     — mirrors invItemType (Usable,
 *                                          AutoUseOnPickup, effects)
 * ItemEffect enum                        — mirrors the legacy effect system
 *                                          (RestoreHealth / RestoreSuperMeter)
 * ItemSlot / WeaponSlot                  — stack-style slot entries
 * Pickup                                 — world-drop marker component
 * ItemRegistry / WeaponItemRegistry      — resource registries (invManager)
 */
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Item effects
// ---------------------------------------------------------------------------

/// Consumable item effects.  Dispatched by `use_item_system` when the player
/// uses an item slot or an AutoUseOnPickup item is picked up.
///
/// Rust enum replaces the original polymorphic `invItemEffect` hierarchy
/// class. New variants should map 1:1 to a new legacy subclass.
#[derive(Debug, Clone)]
pub enum ItemEffect {
    /// Restore `amount` HP to the target (RestoreHealth).  Clamped to max HP.
    RestoreHealth { amount: f32 },
    /// Add `amount` to the target's super meter (RestoreSuperMeter).
    RestoreSuperMeter { amount: f32 },
}

// ---------------------------------------------------------------------------
// Base item data — shared by weapon + consumable types
// ---------------------------------------------------------------------------

/// Common attributes shared by every invItemType / invWeaponType.
#[derive(Debug, Clone, Default)]
pub struct ItemBaseData {
    pub name: String,
    pub weight: f32,
    pub priority: i32,
    /// Name of the pickup-actor template (if any) spawned when dropped.
    pub spawn_template: Option<String>,
    pub icon_color: Color,
    pub line_color_start: Color,
    pub line_color_end: Color,
}

// ---------------------------------------------------------------------------
// WeaponItemType — an inventory record for a weapon
// ---------------------------------------------------------------------------

/// The "inventory definition" for a weapon.  Note this is distinct from the
/// `weapons::WeaponTypeData` (runtime firing data).  The inventory type points
/// into the weapon-type registry via `weapon_type_name`.
#[derive(Debug, Clone, Default)]
pub struct WeaponItemType {
    pub base: ItemBaseData,
    pub max_ammo: f32,
    pub initial_ammo: f32,
    /// Lookup key into `weapons::WeaponRegistry`.
    pub weapon_type_name: String,
    /// Lookup key into `weapons::AmmoRegistry` (may be empty for melee weapons).
    pub ammo_type_name: String,
}

// ---------------------------------------------------------------------------
// ConsumableItemType — an inventory record for a usable item (hypos, etc.)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ConsumableItemType {
    pub base: ItemBaseData,
    pub usable: bool,
    pub auto_use_on_pickup: bool,
    pub effects: Vec<ItemEffect>,
}

// ---------------------------------------------------------------------------
// Registries
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct WeaponItemRegistry {
    pub types: HashMap<String, Arc<WeaponItemType>>,
}

impl WeaponItemRegistry {
    pub fn get(&self, name: &str) -> Option<&Arc<WeaponItemType>> {
        self.types.get(&name.to_lowercase())
    }
}

#[derive(Resource, Default)]
pub struct ItemRegistry {
    pub types: HashMap<String, Arc<ConsumableItemType>>,
}

impl ItemRegistry {
    pub fn get(&self, name: &str) -> Option<&Arc<ConsumableItemType>> {
        self.types.get(&name.to_lowercase())
    }
}

// ---------------------------------------------------------------------------
// Slots
// ---------------------------------------------------------------------------

/// One stack of a consumable item in the inventory.
#[derive(Debug, Clone)]
pub struct ItemSlot {
    pub ty: Arc<ConsumableItemType>,
    pub quantity: u32,
}

impl ItemSlot {
    pub fn new(ty: Arc<ConsumableItemType>) -> Self {
        ItemSlot { ty, quantity: 0 }
    }
}

/// One weapon in the inventory.  Points to the weapons-module Weapon
/// component instance via `weapon_entity` (a sibling entity that the Weapon
/// component is attached to).
#[derive(Debug, Clone)]
pub struct WeaponSlot {
    pub ty: Arc<WeaponItemType>,
    /// Entity holding the weapons::Weapon component (spawned when the weapon
    /// is added to the inventory).
    pub weapon_entity: Entity,
}

// ---------------------------------------------------------------------------
// Inventory — per-entity component
// ---------------------------------------------------------------------------

/// Tuning data used to initialize a new Inventory.  The original Oni2
/// invInventoryComponentType stored capacities + initial weapons/items; we
/// collapse that into a single Bundle-like struct the caller provides.
#[derive(Debug, Clone)]
pub struct InventoryTypeData {
    pub weapon_capacity: f32,
    pub item_capacity: f32,
    pub max_item_slots: usize,
    pub drop_items_on_death: bool,
    pub drop_range: f32,
    pub can_be_picked_up: bool,
    pub pickup_range: f32,
}

impl Default for InventoryTypeData {
    fn default() -> Self {
        InventoryTypeData {
            weapon_capacity: 8.0,
            item_capacity: 1_000_000.0,
            max_item_slots: 8,
            drop_items_on_death: false,
            drop_range: 1.5,
            can_be_picked_up: true,
            pickup_range: 1.5,
        }
    }
}

/// Marks an entity to receive a specific initial weapon slot config.
/// `pending_inventory_system` consumes this, attaches an `Inventory`
/// seeded from `InventoryTypeData::default()` with these per-actor
/// overrides applied, and dispatches the seed-weapon add.
#[derive(Component, Debug, Clone, Default)]
pub struct PendingInventory {
    pub weapon_string: String,
    /// Override for `InventoryTypeData::can_be_picked_up`.
    pub can_be_picked_up: Option<bool>,
    /// Override for `InventoryTypeData::pickup_range`.
    pub pickup_range: Option<f32>,
    /// Override for `InventoryTypeData::drop_items_on_death`.
    pub drop_items_on_death: Option<bool>,
    /// Override for `InventoryTypeData::drop_range`.
    pub drop_range: Option<f32>,
}

/// Drives an auto-release of the trigger after a fixed wall-clock
/// deadline.  Inserted by the ScrOni `shoot <target> for <duration>`
/// drain handler so the held-trigger from the matching
/// `InventoryFireMessage` doesn't latch indefinitely once the
/// scripted burst ends.  `scheduled_stop_firing_system` watches the
/// component and writes an `InventoryStopFiringMessage` (then removes
/// itself) when `time.elapsed_secs_f64() >= end_time`.
#[derive(Component, Debug, Clone)]
pub struct ScheduledStopFiring {
    pub end_time: f64,
}

/// Per-entity inventory.  Insert alongside a wielder (player or AI).
#[derive(Component, Debug, Clone)]
pub struct Inventory {
    pub weapon_capacity: f32,
    pub item_capacity: f32,
    pub max_item_slots: usize,
    pub weapon_slots: Vec<WeaponSlot>,
    pub item_slots: Vec<ItemSlot>,
    /// Index into `weapon_slots` for the currently-selected weapon.
    pub current_weapon: Option<usize>,
    /// Index into `item_slots` for the currently-selected item.
    pub current_item: Option<usize>,
    /// Total weapon weight loaded.
    pub weapon_weight: f32,
    /// Total item weight loaded.
    pub item_weight: f32,
    pub drop_items_on_death: bool,
    pub drop_range: f32,
    pub can_be_picked_up: bool,
    pub pickup_range: f32,
}

impl Inventory {
    pub fn new(data: InventoryTypeData) -> Self {
        Inventory {
            weapon_capacity: data.weapon_capacity,
            item_capacity: data.item_capacity,
            max_item_slots: data.max_item_slots,
            weapon_slots: Vec::new(),
            item_slots: Vec::new(),
            current_weapon: None,
            current_item: None,
            weapon_weight: 0.0,
            item_weight: 0.0,
            drop_items_on_death: data.drop_items_on_death,
            drop_range: data.drop_range,
            can_be_picked_up: data.can_be_picked_up,
            pickup_range: data.pickup_range,
        }
    }

    /// Currently-selected weapon's entity, if any.
    pub fn current_weapon_entity(&self) -> Option<Entity> {
        self.current_weapon
            .and_then(|i| self.weapon_slots.get(i))
            .map(|s| s.weapon_entity)
    }

    /// Currently-selected weapon slot, if any.
    pub fn current_weapon_slot(&self) -> Option<&WeaponSlot> {
        self.current_weapon.and_then(|i| self.weapon_slots.get(i))
    }

    /// Returns true if the weight constraint permits adding a weapon of this type.
    pub fn can_add_weapon(&self, ty: &WeaponItemType) -> bool {
        self.weapon_weight + ty.base.weight <= self.weapon_capacity
    }

    /// Returns true if the weight constraint permits adding an item of this type.
    pub fn can_add_item(&self, ty: &ConsumableItemType) -> bool {
        self.item_weight + ty.base.weight <= self.item_capacity
            && self.item_slots.len() < self.max_item_slots
    }

    pub fn find_item_slot(&self, name: &str) -> Option<usize> {
        self.item_slots
            .iter()
            .position(|s| s.ty.base.name.eq_ignore_ascii_case(name))
    }

    pub fn find_weapon_slot(&self, name: &str) -> Option<usize> {
        self.weapon_slots
            .iter()
            .position(|s| s.ty.base.name.eq_ignore_ascii_case(name))
    }

    /// Cycle to the next / previous weapon slot.  Does nothing if the list is empty.
    pub fn select_next_weapon(&mut self) {
        if self.weapon_slots.is_empty() {
            return;
        }
        self.current_weapon = Some(match self.current_weapon {
            Some(i) => (i + 1) % self.weapon_slots.len(),
            None => 0,
        });
    }
    pub fn select_prev_weapon(&mut self) {
        if self.weapon_slots.is_empty() {
            return;
        }
        let n = self.weapon_slots.len();
        self.current_weapon = Some(match self.current_weapon {
            Some(i) => (i + n - 1) % n,
            None => n - 1,
        });
    }
    pub fn select_next_item(&mut self) {
        if self.item_slots.is_empty() {
            return;
        }
        self.current_item = Some(match self.current_item {
            Some(i) => (i + 1) % self.item_slots.len(),
            None => 0,
        });
    }
    pub fn select_prev_item(&mut self) {
        if self.item_slots.is_empty() {
            return;
        }
        let n = self.item_slots.len();
        self.current_item = Some(match self.current_item {
            Some(i) => (i + n - 1) % n,
            None => n - 1,
        });
    }
}

// ---------------------------------------------------------------------------
// Pickup — world-drop marker component
// ---------------------------------------------------------------------------

/// A dropped item / weapon in the world.  Consumed by `pickup_system` when a
/// wielder's PickUpItemsMessage overlaps with the pickup.
#[derive(Component, Debug, Clone)]
pub enum Pickup {
    Item {
        ty: Arc<ConsumableItemType>,
        quantity: u32,
    },
    Weapon {
        ty: Arc<WeaponItemType>,
        /// Remaining ammo on the dropped weapon.
        ammo: f32,
    },
}

// ---------------------------------------------------------------------------
// Actor Weapon Mounts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WeaponMount {
    pub parent_bone: usize,
    /// Grip offset in Bevy-local space relative to the parent bone.
    /// Converted at parse time from the Oni2-space value in the
    /// actor's `.weap` file via `space::to_bevy_space_pos`.
    pub offset: Vec3,
    /// Grip rotation as a Bevy quaternion (bone→weapon).  Converted
    /// at parse time from the Oni2 Euler triple via
    /// `space::to_bevy_space_rot_rad`.
    pub rot: Quat,
}

impl Default for WeaponMount {
    fn default() -> Self {
        Self {
            parent_bone: 0,
            offset: Vec3::ZERO,
            rot: Quat::IDENTITY,
        }
    }
}

#[derive(Component, Debug, Clone, Default)]
pub struct ActorWeaponMounts {
    /// Maps the `WeaponItemType.weapon_type_name` to (OUT mount, AWAY mount).
    pub mounts: HashMap<String, (WeaponMount, WeaponMount)>,
}
