use bevy::prelude::*;
use std::collections::HashMap;

use super::block_parser::BlockParser;
use crate::inventory::components::{ActorWeaponMounts, WeaponMount};

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
                                                out_mount.offset = p.read_vec3(&a_key, Vec3::ZERO)
                                            }
                                            "gripeulers" => {
                                                out_mount.eulers = p.read_vec3(&a_key, Vec3::ZERO)
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
                                                away_mount.offset = p.read_vec3(&a_key, Vec3::ZERO)
                                            }
                                            "gripeulers" => {
                                                away_mount.eulers = p.read_vec3(&a_key, Vec3::ZERO)
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

                    mounts.insert(weapon_type, (out_mount, away_mount));
                }
            } else {
                break;
            }
        }
    }

    ActorWeaponMounts { mounts }
}
