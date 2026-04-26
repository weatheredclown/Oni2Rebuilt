use bevy::prelude::*;
use std::collections::HashMap;

use super::block_parser::BlockParser;
use crate::inventory::components::{ActorWeaponMounts, WeaponMount};
use crate::oni2_loader::utils::space;

pub fn parse_actor_weap(content: &str) -> ActorWeaponMounts {
    let mut mounts = HashMap::new();
    let mut p = BlockParser::new(content);

    // Skip NUMWEAPONS token and its argument
    if p.peek() == Some("NUMWEAPONS") {
        p.next();
        p.next();
    }

    if p.start_anonymous() {
        while !p.endblock() {
            if let Some(mut weapon_type) = p.next() {
                weapon_type = weapon_type.trim_matches('"').to_string();

                // Sometimes random garbage can be left outside, make sure we actually start a weapon block
                if p.start_anonymous() {
                    let mut out_mount = WeaponMount::default();
                    let mut away_mount = WeaponMount::default();

                    while !p.endblock() {
                        let inner = p.peek().unwrap_or("");
                        let inner_lower = inner.to_lowercase();
                        match inner_lower.as_str() {
                            "out" => {
                                p.next();
                                if p.start_anonymous() {
                                    while !p.endblock() {
                                        let key = p.peek().unwrap_or("").to_lowercase();
                                        let a_key = p.peek().unwrap_or("").to_string();
                                        match key.as_str() {
                                            "parentbone" => {
                                                out_mount.parent_bone =
                                                    p.read_i32(&a_key, 0) as usize
                                            }
                                            "gripoffset" => {
                                                // Convert from Oni2 (left-handed)
                                                // to Bevy (right-handed) at parse.
                                                let v = p.read_vec3(&a_key, Vec3::ZERO);
                                                out_mount.offset = space::to_bevy_space_pos(v);
                                            }
                                            "gripeulers" => {
                                                let e = p.read_vec3(&a_key, Vec3::ZERO);
                                                out_mount.rot =
                                                    space::to_bevy_space_rot_rad(e);
                                            }
                                            _ => {
                                                p.next();
                                            }
                                        }
                                    }
                                }
                            }
                            "away" => {
                                p.next();
                                if p.start_anonymous() {
                                    while !p.endblock() {
                                        let key = p.peek().unwrap_or("").to_lowercase();
                                        let a_key = p.peek().unwrap_or("").to_string();
                                        match key.as_str() {
                                            "parentbone" => {
                                                away_mount.parent_bone =
                                                    p.read_i32(&a_key, 0) as usize
                                            }
                                            "gripoffset" => {
                                                let v = p.read_vec3(&a_key, Vec3::ZERO);
                                                away_mount.offset = space::to_bevy_space_pos(v);
                                            }
                                            "gripeulers" => {
                                                let e = p.read_vec3(&a_key, Vec3::ZERO);
                                                away_mount.rot =
                                                    space::to_bevy_space_rot_rad(e);
                                            }
                                            _ => {
                                                p.next();
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {
                                p.next();
                            }
                        }
                    }

                    // Key by lowercase so the `weapon_attachment_system`
                    // lookup (`weapon.ty.name.to_lowercase()`) finds the
                    // mount regardless of case in the .weap source file
                    // (kno.weap uses `"Pistol"`, the weapon ty name is
                    // also `"Pistol"` — a case-sensitive match here
                    // silently failed and the weapon fell back to the
                    // default bone 11 / 0).
                    mounts.insert(weapon_type.to_lowercase(), (out_mount, away_mount));
                }
            } else {
                break;
            }
        }
    }

    ActorWeaponMounts { mounts }
}
