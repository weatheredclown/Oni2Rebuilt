use super::block_parser::BlockParser;
use crate::inventory::components::{ConsumableItemType, ItemEffect, WeaponItemType};
use bevy::prelude::*;
use std::collections::HashMap;

pub fn parse_inventory_content(
    content: &str,
) -> (
    HashMap<String, ConsumableItemType>,
    HashMap<String, WeaponItemType>,
) {
    let mut items = HashMap::new();
    let mut weapons = HashMap::new();
    let mut p = BlockParser::new(content);

    while !p.endblock() {
        let peek = p.peek().unwrap_or("").to_lowercase();
        match peek.as_str() {
            "item_type" => {
                p.next(); // Consume ITEM_TYPE
                if let Some(name) = p.next() {
                    let mut def = ConsumableItemType::default();
                    def.base.name = name.clone();

                    while !p.endblock() {
                        let inner_key = p.peek().unwrap_or("").to_lowercase();
                        let a_key = p.peek().unwrap_or("").to_string();

                        if inner_key == "end" {
                            p.next();
                            break;
                        }

                        match inner_key.as_str() {
                            "spawn_template" => def.base.spawn_template = p.read_string_opt(&a_key),
                            "edge_model" => {
                                p.next();
                                p.next(); /* gracefully ignore edge model */
                            }
                            "model" => {
                                p.next();
                                p.next(); /* ignore model */
                            }
                            "color_icon" => def.base.icon_color = p.read_rgba_val(&a_key, 255.0),
                            "color_line_start" => {
                                def.base.line_color_start = p.read_rgba_val(&a_key, 255.0)
                            }
                            "color_line_end" => {
                                def.base.line_color_end = p.read_rgba_val(&a_key, 255.0)
                            }
                            "weight" => def.base.weight = p.read_float(&a_key, 0.0),
                            "auto_use" => def.auto_use_on_pickup = p.read_i32(&a_key, 0) != 0,
                            "useable" | "usable" => def.usable = p.read_i32(&a_key, 0) != 0,
                            "restore_health" => {
                                let amt = p.read_float(&a_key, 0.0);
                                def.effects.push(ItemEffect::RestoreHealth { amount: amt });
                            }
                            "restore_super_meter" => {
                                let amt = p.read_float(&a_key, 0.0);
                                def.effects
                                    .push(ItemEffect::RestoreSuperMeter { amount: amt });
                            }
                            _ => {
                                p.next();
                            } // skip unknown property
                        }
                    }
                    items.insert(name.to_lowercase(), def);
                }
            }
            "weapon_type" => {
                p.next(); // Consume WEAPON_TYPE
                if let Some(name) = p.next() {
                    let mut def = WeaponItemType::default();
                    def.base.name = name.clone();

                    while !p.endblock() {
                        let inner_key = p.peek().unwrap_or("").to_lowercase();
                        let a_key = p.peek().unwrap_or("").to_string();

                        if inner_key == "end" {
                            p.next();
                            break;
                        }

                        match inner_key.as_str() {
                            "spawn_template" => def.base.spawn_template = p.read_string_opt(&a_key),
                            "edge_model" | "model" => {
                                p.next();
                                p.next();
                            }
                            "color_icon" => def.base.icon_color = p.read_rgba_val(&a_key, 255.0),
                            "color_line_start" => {
                                def.base.line_color_start = p.read_rgba_val(&a_key, 255.0)
                            }
                            "color_line_end" => {
                                def.base.line_color_end = p.read_rgba_val(&a_key, 255.0)
                            }
                            "weight" => def.base.weight = p.read_float(&a_key, 0.0),
                            "max_ammo" => def.max_ammo = p.read_float(&a_key, 0.0),
                            "initial_ammo" => def.initial_ammo = p.read_float(&a_key, 0.0),
                            "weapon_type" => def.weapon_type_name = p.read_string(&a_key, ""),
                            "ammo_type" => def.ammo_type_name = p.read_string(&a_key, ""),
                            _ => {
                                p.next();
                            }
                        }
                    }
                    weapons.insert(name.to_lowercase(), def);
                }
            }
            _ => {
                p.next();
            } // Garbage
        }
    }

    (items, weapons)
}
