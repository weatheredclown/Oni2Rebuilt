/*
 * inventory/systems.rs — FixedUpdate systems for the inventory subsystem.
 *
 * add_weapon_system         — AddWeaponByNameMessage → look up WeaponItemType,
 *                             spawn a weapons::Weapon entity, append WeaponSlot.
 * add_item_system           — AddItemByNameMessage → look up ConsumableItemType,
 *                             append / stack into an existing ItemSlot.
 * delete_weapon_system /
 * delete_item_system        — remove a slot entry (despawning the Weapon entity).
 * drop_weapon_system /
 * drop_item_system          — spawn a Pickup in the world at the wielder's
 *                             position, then remove the slot.
 * drop_all_system           — DropAllMessage (death handler helper).
 * use_item_system           — dispatch ItemEffect variants, decrement quantity,
 *                             despawn the slot when empty.
 * pickup_system             — PickUpItemsMessage: overlap check against every
 *                             Pickup within `pickup_range`, transfer into the
 *                             inventory.
 * select_*_system           — forward selection messages to Inventory cursors.
 * set_active_weapon_system  — explicit index jump.
 * inventory_forward_system  — InventoryFire/Stop/Aim → weapons::* on the current
 *                             weapon entity.
 * drop_on_death_system      — listens for combat::DeathMessage, writes a
 *                             DropAllMessage if `drop_items_on_death`.
 */
use bevy::prelude::*;

use crate::combat::components::Health;
use crate::combat::events::DeathMessage;
use crate::fight::components::FighterState;
use crate::fight::events::SuperMeterAddEvent;
use crate::weapons::{AimMessage as WeapAimMessage, FireWeaponMessage, StopFiringMessage};
use crate::weapons::{AmmoRegistry, Weapon, WeaponRegistry};

use super::components::{
    Inventory, ItemEffect, ItemRegistry, ItemSlot, Pickup, WeaponItemRegistry, WeaponSlot,
};
use super::events::{
    AddItemByNameMessage, AddWeaponByNameMessage, DeleteItemMessage, DeleteWeaponMessage,
    DropAllMessage, DropItemMessage, DropWeaponMessage, InventoryAimMessage, InventoryFireMessage,
    InventoryStopFiringMessage, ItemDroppedMessage, ItemEffectAppliedMessage, ItemPickedUpMessage,
    PickUpItemsMessage, SelectNextItemMessage, SelectNextWeaponMessage, SelectPrevItemMessage,
    SelectPrevWeaponMessage, SetActiveWeaponMessage, UseItemMessage, WeaponDroppedMessage,
    WeaponPickedUpMessage,
};

// ---------------------------------------------------------------------------
// add_weapon_system
// ---------------------------------------------------------------------------

pub fn add_weapon_system(
    mut reader: MessageReader<AddWeaponByNameMessage>,
    mut out: MessageWriter<WeaponPickedUpMessage>,
    mut commands: Commands,
    mut query: Query<(
        &mut Inventory,
        Option<&mut crate::animator::components::ActionPlayer>,
    )>,
    inv_registry: Res<WeaponItemRegistry>,
    weap_registry: Res<WeaponRegistry>,
    ammo_registry: Res<AmmoRegistry>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut env_materials: ResMut<Assets<crate::env_reflect_material::EnvReflectMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut skinned_mesh_ibp: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut entity_lib: ResMut<crate::oni2_loader::registries::EntityLibrary>,
    mut anim_registry: ResMut<crate::oni2_loader::registries::AnimRegistry>,
) {
    for msg in reader.read() {
        let Ok((mut inv, mut ap_opt)) = query.get_mut(msg.entity) else {
            continue;
        };
        let Some(inv_ty) = inv_registry.get(&msg.weapon_name).cloned() else {
            warn!(
                "inventory: unknown weapon item '{}' (no entry in WeaponItemRegistry)",
                msg.weapon_name
            );
            continue;
        };
        if !inv.can_add_weapon(&inv_ty) {
            warn!(
                "inventory: cannot add '{}' — weight limit reached",
                inv_ty.base.name
            );
            continue;
        }
        let Some(weap_ty) = weap_registry.get(&inv_ty.weapon_type_name).cloned() else {
            warn!(
                "inventory: weapon item '{}' references unknown weapon type '{}'",
                inv_ty.base.name, inv_ty.weapon_type_name
            );
            continue;
        };

        // Spawn the Weapon component onto a dedicated sibling entity.
        // Include Visibility explicitly so the inherited-visibility chain
        // reaches the child weapon mesh spawned below — without a
        // `Visibility` on this entity, the mesh child's `ViewVisibility`
        // resolves to hidden even though its own `Visibility::Visible`
        // is set by `spawn_oni2_entity`.
        let weapon_entity = commands
            .spawn((
                Name::new(format!("Weapon:{}", inv_ty.base.name)),
                Transform::default(),
                GlobalTransform::default(),
                Visibility::default(),
                Weapon::new(weap_ty.clone(), msg.entity),
                // Cleanup-on-layout-exit.  weapon_attachment_system
                // re-parents the weapon to its wielder's bone every
                // frame, but if the wielder is despawned mid-frame
                // the weapon could be orphaned without this tag.
                crate::menu::InGameEntity,
            ))
            .id();

        if let Some(ref entity_type) = weap_ty.entity_type {
            let dir = format!("Entity/{}", entity_type);
            if let Some(mesh_entity) = crate::oni2_loader::spawn::spawn_oni2_entity(
                &mut commands,
                &mut meshes,
                &mut materials,
                &mut env_materials,
                &mut images,
                &mut skinned_mesh_ibp,
                &mut entity_lib,
                &mut anim_registry,
                &dir,
                Vec3::ZERO,
                &weap_ty.name,
            ) {
                // Attach the spawned mesh correctly
                commands.entity(weapon_entity).add_child(mesh_entity);

                // Strip colliders from the spawned weapon so it doesn't push the player around
                commands.queue(move |world: &mut World| {
                    let mut queue = vec![mesh_entity];
                    while let Some(e) = queue.pop() {
                        if let Some(children) = world.get::<Children>(e) {
                            for child in children {
                                queue.push(*child);
                            }
                        }
                        if let Ok(mut emut) = world.get_entity_mut(e) {
                            emut.remove::<avian3d::prelude::Collider>();
                            emut.remove::<avian3d::prelude::RigidBody>();
                        }
                    }
                });
            } else {
                warn!(
                    "inventory: weapon '{}' failed to spawn entity_type='{}' (dir='{}') — weapon entity will exist without a visible mesh",
                    weap_ty.name, entity_type, dir
                );
            }
        } else {
            warn!(
                "inventory: weapon '{}' has no entity_type set — no mesh will be spawned",
                weap_ty.name
            );
        }

        // Set to Holstered if it was previously completely hidden, triggering attachment
        if let Some(ref mut ap) = ap_opt {
            if ap.weapon_state == crate::animator::components::WeaponState::Invisible {
                ap.weapon_state = crate::animator::components::WeaponState::Holstered;
            }
        }

        // Seed ammo: use the message's override if supplied (pickup
        // of a dropped weapon preserves its snapshotted ammo), else
        // fall back to the weapon type's authored InitialAmmo.
        if !inv_ty.ammo_type_name.is_empty()
            && let Some(ammo_ty) = ammo_registry.get(&inv_ty.ammo_type_name).cloned()
        {
            commands.queue(move |world: &mut World| {
                if let Some(mut w) = world.get_mut::<Weapon>(weapon_entity) {
                    w.set_ammo(0, ammo_ty, 0.0);
                }
            });
            let seed = msg.ammo_override.unwrap_or(inv_ty.initial_ammo);
            commands.queue(move |world: &mut World| {
                if let Some(mut w) = world.get_mut::<Weapon>(weapon_entity)
                    && let Some(slot) = w.ammo_slots.get_mut(0)
                {
                    slot.amount = seed;
                }
            });
        }

        inv.weapon_weight += inv_ty.base.weight;
        let first = inv.weapon_slots.is_empty();
        inv.weapon_slots.push(WeaponSlot {
            ty: inv_ty.clone(),
            weapon_entity,
        });
        if first {
            inv.current_weapon = Some(0);
        }

        out.write(WeaponPickedUpMessage {
            wielder: msg.entity,
            weapon_name: inv_ty.base.name.clone(),
            weapon_entity,
        });
    }
}

// ---------------------------------------------------------------------------
// add_item_system
// ---------------------------------------------------------------------------

pub fn add_item_system(
    mut reader: MessageReader<AddItemByNameMessage>,
    mut out: MessageWriter<ItemPickedUpMessage>,
    mut auto_use: MessageWriter<UseItemMessage>,
    mut query: Query<&mut Inventory>,
    registry: Res<ItemRegistry>,
) {
    for msg in reader.read() {
        let Ok(mut inv) = query.get_mut(msg.entity) else {
            continue;
        };
        let Some(ty) = registry.get(&msg.item_name).cloned() else {
            warn!("inventory: unknown item '{}'", msg.item_name);
            continue;
        };

        let slot_index = if let Some(idx) = inv.find_item_slot(&ty.base.name) {
            inv.item_slots[idx].quantity += msg.quantity;
            idx
        } else {
            if !inv.can_add_item(&ty) {
                warn!(
                    "inventory: cannot add item '{}' — weight or slot limit reached",
                    ty.base.name
                );
                continue;
            }
            inv.item_weight += ty.base.weight * msg.quantity as f32;
            inv.item_slots.push(ItemSlot {
                ty: ty.clone(),
                quantity: msg.quantity,
            });
            let idx = inv.item_slots.len() - 1;
            if inv.current_item.is_none() {
                inv.current_item = Some(idx);
            }
            idx
        };

        out.write(ItemPickedUpMessage {
            wielder: msg.entity,
            item_name: ty.base.name.clone(),
            quantity: msg.quantity,
        });

        if ty.auto_use_on_pickup {
            auto_use.write(UseItemMessage {
                entity: msg.entity,
                slot_index,
                target: None,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// delete / drop — weapon
// ---------------------------------------------------------------------------

pub fn delete_weapon_system(
    mut reader: MessageReader<DeleteWeaponMessage>,
    mut commands: Commands,
    mut query: Query<&mut Inventory>,
) {
    for msg in reader.read() {
        let Ok(mut inv) = query.get_mut(msg.entity) else {
            continue;
        };
        if msg.slot_index >= inv.weapon_slots.len() {
            continue;
        }
        let slot = inv.weapon_slots.remove(msg.slot_index);
        inv.weapon_weight = (inv.weapon_weight - slot.ty.base.weight).max(0.0);
        commands.entity(slot.weapon_entity).despawn();

        // Re-anchor the cursor.
        inv.current_weapon = match inv.current_weapon {
            Some(i) if i == msg.slot_index => {
                if inv.weapon_slots.is_empty() {
                    None
                } else {
                    Some(i.min(inv.weapon_slots.len() - 1))
                }
            }
            Some(i) if i > msg.slot_index => Some(i - 1),
            other => other,
        };
    }
}

pub fn drop_weapon_system(
    mut reader: MessageReader<DropWeaponMessage>,
    mut out: MessageWriter<WeaponDroppedMessage>,
    mut commands: Commands,
    mut query: Query<(&mut Inventory, &Transform)>,
    weapon_q: Query<&Weapon>,
) {
    for msg in reader.read() {
        let Ok((mut inv, wielder_tf)) = query.get_mut(msg.entity) else {
            continue;
        };
        if msg.slot_index >= inv.weapon_slots.len() {
            continue;
        }
        let slot = inv.weapon_slots.remove(msg.slot_index);
        inv.weapon_weight = (inv.weapon_weight - slot.ty.base.weight).max(0.0);

        // Snapshot remaining ammo so the pickup preserves it.
        let ammo = weapon_q
            .get(slot.weapon_entity)
            .ok()
            .and_then(|w| w.ammo_slots.first())
            .map(|s| s.amount)
            .unwrap_or(0.0);

        let drop_pos =
            wielder_tf.translation + wielder_tf.rotation * Vec3::new(0.0, 0.5, -inv.drop_range);
        let pickup = commands
            .spawn((
                Name::new(format!("Pickup:{}", slot.ty.base.name)),
                Transform::from_translation(drop_pos),
                GlobalTransform::default(),
                Pickup::Weapon {
                    ty: slot.ty.clone(),
                    ammo,
                },
            ))
            .id();

        commands.entity(slot.weapon_entity).despawn();

        inv.current_weapon = match inv.current_weapon {
            Some(i) if i == msg.slot_index => {
                if inv.weapon_slots.is_empty() {
                    None
                } else {
                    Some(i.min(inv.weapon_slots.len() - 1))
                }
            }
            Some(i) if i > msg.slot_index => Some(i - 1),
            other => other,
        };

        out.write(WeaponDroppedMessage {
            wielder: msg.entity,
            weapon_name: slot.ty.base.name.clone(),
            pickup_entity: pickup,
        });
    }
}

// ---------------------------------------------------------------------------
// delete / drop — item
// ---------------------------------------------------------------------------

pub fn delete_item_system(
    mut reader: MessageReader<DeleteItemMessage>,
    mut query: Query<&mut Inventory>,
) {
    for msg in reader.read() {
        let Ok(mut inv) = query.get_mut(msg.entity) else {
            continue;
        };
        if msg.slot_index >= inv.item_slots.len() {
            continue;
        }
        let slot = inv.item_slots.remove(msg.slot_index);
        inv.item_weight = (inv.item_weight - slot.ty.base.weight * slot.quantity as f32).max(0.0);
        inv.current_item = match inv.current_item {
            Some(i) if i == msg.slot_index => {
                if inv.item_slots.is_empty() {
                    None
                } else {
                    Some(i.min(inv.item_slots.len() - 1))
                }
            }
            Some(i) if i > msg.slot_index => Some(i - 1),
            other => other,
        };
    }
}

pub fn drop_item_system(
    mut reader: MessageReader<DropItemMessage>,
    mut out: MessageWriter<ItemDroppedMessage>,
    mut commands: Commands,
    mut query: Query<(&mut Inventory, &Transform)>,
) {
    for msg in reader.read() {
        let Ok((mut inv, wielder_tf)) = query.get_mut(msg.entity) else {
            continue;
        };
        if msg.slot_index >= inv.item_slots.len() {
            continue;
        }
        let slot = inv.item_slots.remove(msg.slot_index);
        inv.item_weight = (inv.item_weight - slot.ty.base.weight * slot.quantity as f32).max(0.0);

        let drop_pos =
            wielder_tf.translation + wielder_tf.rotation * Vec3::new(0.0, 0.5, -inv.drop_range);
        let pickup = commands
            .spawn((
                Name::new(format!("Pickup:{}", slot.ty.base.name)),
                Transform::from_translation(drop_pos),
                GlobalTransform::default(),
                Pickup::Item {
                    ty: slot.ty.clone(),
                    quantity: slot.quantity,
                },
            ))
            .id();

        inv.current_item = match inv.current_item {
            Some(i) if i == msg.slot_index => {
                if inv.item_slots.is_empty() {
                    None
                } else {
                    Some(i.min(inv.item_slots.len() - 1))
                }
            }
            Some(i) if i > msg.slot_index => Some(i - 1),
            other => other,
        };

        out.write(ItemDroppedMessage {
            wielder: msg.entity,
            item_name: slot.ty.base.name.clone(),
            quantity: slot.quantity,
            pickup_entity: pickup,
        });
    }
}

// ---------------------------------------------------------------------------
// drop_all_system  — DropAllMessage
// ---------------------------------------------------------------------------

pub fn drop_all_system(
    mut reader: MessageReader<DropAllMessage>,
    mut weapon_writer: MessageWriter<DropWeaponMessage>,
    mut item_writer: MessageWriter<DropItemMessage>,
    query: Query<&Inventory>,
) {
    for msg in reader.read() {
        let Ok(inv) = query.get(msg.entity) else {
            continue;
        };
        // Drop from the back so indices remain stable.
        for idx in (0..inv.weapon_slots.len()).rev() {
            weapon_writer.write(DropWeaponMessage {
                entity: msg.entity,
                slot_index: idx,
            });
        }
        for idx in (0..inv.item_slots.len()).rev() {
            item_writer.write(DropItemMessage {
                entity: msg.entity,
                slot_index: idx,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// use_item_system
// ---------------------------------------------------------------------------

pub fn use_item_system(
    mut reader: MessageReader<UseItemMessage>,
    mut applied: MessageWriter<ItemEffectAppliedMessage>,
    mut super_meter: MessageWriter<SuperMeterAddEvent>,
    mut inventories: Query<&mut Inventory>,
    mut healths: Query<&mut Health>,
    fighter_states: Query<&FighterState>,
) {
    for msg in reader.read() {
        let Ok(mut inv) = inventories.get_mut(msg.entity) else {
            continue;
        };
        if msg.slot_index >= inv.item_slots.len() {
            continue;
        }
        // Inspect + mutate the slot inside a scoped borrow, then release it
        // before touching other inventory fields (Rust borrow checker).
        let (effects, weight_delta, empty) = {
            let slot = &mut inv.item_slots[msg.slot_index];
            if !slot.ty.usable || slot.quantity == 0 {
                continue;
            }
            let effects = slot.ty.effects.clone();
            slot.quantity -= 1;
            (effects, slot.ty.base.weight, slot.quantity == 0)
        };

        let target = msg.target.unwrap_or(msg.entity);
        inv.item_weight = (inv.item_weight - weight_delta).max(0.0);
        let slot_index = msg.slot_index;

        if empty {
            inv.item_slots.remove(slot_index);
            inv.current_item = match inv.current_item {
                Some(i) if i == slot_index => {
                    if inv.item_slots.is_empty() {
                        None
                    } else {
                        Some(i.min(inv.item_slots.len() - 1))
                    }
                }
                Some(i) if i > slot_index => Some(i - 1),
                other => other,
            };
        }

        // Release the inventory borrow before we touch sibling components.
        drop(inv);

        for eff in effects {
            match eff.clone() {
                ItemEffect::RestoreHealth { amount } => {
                    if let Ok(mut hp) = healths.get_mut(target) {
                        hp.current = (hp.current + amount).min(hp.max);
                    }
                }
                ItemEffect::RestoreSuperMeter { amount } => {
                    // Only meaningful for fighters.  Drop silently otherwise.
                    if fighter_states.get(target).is_ok() {
                        super_meter.write(SuperMeterAddEvent {
                            entity: target,
                            amount,
                        });
                    }
                }
            }
            applied.write(ItemEffectAppliedMessage {
                source: msg.entity,
                target,
                effect: eff,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// pickup_system
// ---------------------------------------------------------------------------

/// Continuously dispatches `PickUpItemsMessage` for every player
/// entity that owns an Inventory.  Mirrors the legacy ONI auto-
/// pickup feel — players collect items by walking over them within
/// their `PickUpRange`, no key-press needed.  The downstream
/// `pickup_system` does the actual radius filter + grant + despawn,
/// so a fresh message every tick is cheap: if no pickup is in range
/// the iteration completes with no side effects.
pub fn player_auto_pickup_system(
    players: Query<Entity, (With<crate::player::components::Player>, With<Inventory>)>,
    mut pick_writer: MessageWriter<PickUpItemsMessage>,
) {
    for entity in &players {
        pick_writer.write(PickUpItemsMessage {
            entity,
            specific_pickup: None,
        });
    }
}

pub fn pickup_system(
    mut reader: MessageReader<PickUpItemsMessage>,
    mut commands: Commands,
    mut add_weapon: MessageWriter<AddWeaponByNameMessage>,
    mut add_item: MessageWriter<AddItemByNameMessage>,
    inventories: Query<(&Inventory, &Transform)>,
    pickups: Query<(Entity, &Transform, &Pickup)>,
) {
    for msg in reader.read() {
        let Ok((inv, wielder_tf)) = inventories.get(msg.entity) else {
            continue;
        };

        let wielder_pos = wielder_tf.translation;
        let radius_sq = inv.pickup_range * inv.pickup_range;

        for (pe, ptf, pick) in &pickups {
            // If a specific pickup was requested, only process that one.
            if let Some(specific) = msg.specific_pickup {
                if pe != specific {
                    continue;
                }
            } else {
                let d2 = (ptf.translation - wielder_pos).length_squared();
                if d2 > radius_sq {
                    continue;
                }
            }

            match pick {
                Pickup::Item { ty, quantity } => {
                    add_item.write(AddItemByNameMessage {
                        entity: msg.entity,
                        item_name: ty.base.name.clone(),
                        quantity: *quantity,
                    });
                }
                Pickup::Weapon { ty, ammo } => {
                    // Preserve the snapshotted ammo so a half-empty
                    // dropped weapon doesn't magically refill on
                    // pickup.  C++ achieves this through the dropped
                    // actor's full invInventoryComponent state; we
                    // round-trip the value through the message.
                    add_weapon.write(AddWeaponByNameMessage {
                        entity: msg.entity,
                        weapon_name: ty.base.name.clone(),
                        ammo_override: Some(*ammo),
                    });
                }
            }
            commands.entity(pe).despawn();
        }
    }
}

// ---------------------------------------------------------------------------
// selection
// ---------------------------------------------------------------------------

pub fn select_next_weapon_system(
    mut reader: MessageReader<SelectNextWeaponMessage>,
    mut query: Query<&mut Inventory>,
) {
    for msg in reader.read() {
        if let Ok(mut inv) = query.get_mut(msg.entity) {
            inv.select_next_weapon();
        }
    }
}
pub fn select_prev_weapon_system(
    mut reader: MessageReader<SelectPrevWeaponMessage>,
    mut query: Query<&mut Inventory>,
) {
    for msg in reader.read() {
        if let Ok(mut inv) = query.get_mut(msg.entity) {
            inv.select_prev_weapon();
        }
    }
}
pub fn select_next_item_system(
    mut reader: MessageReader<SelectNextItemMessage>,
    mut query: Query<&mut Inventory>,
) {
    for msg in reader.read() {
        if let Ok(mut inv) = query.get_mut(msg.entity) {
            inv.select_next_item();
        }
    }
}
pub fn select_prev_item_system(
    mut reader: MessageReader<SelectPrevItemMessage>,
    mut query: Query<&mut Inventory>,
) {
    for msg in reader.read() {
        if let Ok(mut inv) = query.get_mut(msg.entity) {
            inv.select_prev_item();
        }
    }
}

pub fn set_active_weapon_system(
    mut reader: MessageReader<SetActiveWeaponMessage>,
    mut query: Query<&mut Inventory>,
) {
    for msg in reader.read() {
        if let Ok(mut inv) = query.get_mut(msg.entity)
            && msg.slot_index < inv.weapon_slots.len()
        {
            inv.current_weapon = Some(msg.slot_index);
        }
    }
}

// ---------------------------------------------------------------------------
// inventory-forwarding — turn inventory Fire/Stop/Aim into weapons:: messages
// on the currently-selected weapon entity.
// ---------------------------------------------------------------------------

pub fn inventory_forward_fire_system(
    mut fire_reader: MessageReader<InventoryFireMessage>,
    mut stop_reader: MessageReader<InventoryStopFiringMessage>,
    mut aim_reader: MessageReader<InventoryAimMessage>,
    mut fire_writer: MessageWriter<FireWeaponMessage>,
    mut stop_writer: MessageWriter<StopFiringMessage>,
    mut aim_writer: MessageWriter<WeapAimMessage>,
    inventories: Query<&Inventory>,
) {
    for msg in fire_reader.read() {
        if let Ok(inv) = inventories.get(msg.entity)
            && let Some(weapon_entity) = inv.current_weapon_entity()
        {
            fire_writer.write(FireWeaponMessage {
                entity: weapon_entity,
                multi_dir: msg.multi_dir,
            });
        }
    }
    for msg in stop_reader.read() {
        if let Ok(inv) = inventories.get(msg.entity)
            && let Some(weapon_entity) = inv.current_weapon_entity()
        {
            stop_writer.write(StopFiringMessage {
                entity: weapon_entity,
            });
        }
    }
    for msg in aim_reader.read() {
        if let Ok(inv) = inventories.get(msg.entity)
            && let Some(weapon_entity) = inv.current_weapon_entity()
        {
            aim_writer.write(WeapAimMessage {
                entity: weapon_entity,
                target: msg.target.clone(),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// scheduled_stop_firing_system — auto-release trigger when timer expires
// ---------------------------------------------------------------------------

/// Drains [`crate::inventory::components::ScheduledStopFiring`] markers
/// each tick.  When the deadline elapses, emits an
/// `InventoryStopFiringMessage` for the entity and removes the marker
/// so the actor stops holding the trigger.  Drives the auto-release
/// half of ScrOni's `shoot <target> for <duration>` op.
pub fn scheduled_stop_firing_system(
    mut commands: Commands,
    time: Res<Time>,
    mut stop_writer: MessageWriter<InventoryStopFiringMessage>,
    query: Query<(
        Entity,
        &crate::inventory::components::ScheduledStopFiring,
    )>,
) {
    let now = time.elapsed_secs_f64();
    for (entity, sched) in &query {
        if now >= sched.end_time {
            stop_writer.write(InventoryStopFiringMessage { entity });
            commands
                .entity(entity)
                .remove::<crate::inventory::components::ScheduledStopFiring>();
        }
    }
}

// ---------------------------------------------------------------------------
// pending_inventory_system — process PendingInventory attachments and seed weapons
// ---------------------------------------------------------------------------

pub fn pending_inventory_system(
    mut commands: Commands,
    query: Query<(Entity, &crate::inventory::components::PendingInventory)>,
    mut add_weapon: MessageWriter<AddWeaponByNameMessage>,
) {
    for (entity, pending) in &query {
        // First remove the marker so we don't process it again
        commands
            .entity(entity)
            .remove::<crate::inventory::components::PendingInventory>();

        // Setup empty inventory.  Start from the engine defaults
        // and let the per-actor `<Inventory>` block (carried through
        // PendingInventory) override pickup/drop tuning.  Anything
        // the actor XML didn't specify stays at the default.
        let mut type_data = crate::inventory::components::InventoryTypeData::default();
        if let Some(v) = pending.can_be_picked_up {
            type_data.can_be_picked_up = v;
        }
        if let Some(v) = pending.pickup_range {
            type_data.pickup_range = v;
        }
        if let Some(v) = pending.drop_items_on_death {
            type_data.drop_items_on_death = v;
        }
        if let Some(v) = pending.drop_range {
            type_data.drop_range = v;
        }
        commands.entity(entity).insert(Inventory::new(type_data));

        // If there's a weapon requested, issue the command to add it
        if !pending.weapon_string.is_empty() {
            add_weapon.write(AddWeaponByNameMessage {
                entity,
                weapon_name: pending.weapon_string.clone(),
                ammo_override: None,
            });
            info!(
                "Seed inventory weapon {} for entity {:?}",
                pending.weapon_string, entity
            );
        }
    }
}

// ---------------------------------------------------------------------------
// drop_on_death_system — bridge combat deaths to DropAllMessage.
// ---------------------------------------------------------------------------

pub fn drop_on_death_system(
    mut reader: MessageReader<DeathMessage>,
    mut writer: MessageWriter<DropAllMessage>,
    query: Query<&Inventory>,
) {
    for msg in reader.read() {
        if let Ok(inv) = query.get(msg.entity)
            && inv.drop_items_on_death
        {
            writer.write(DropAllMessage { entity: msg.entity });
        }
    }
}
