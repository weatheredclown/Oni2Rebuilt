/*
 * oni2_loader/parsers/ammo.rs — rb.ammo parser.
 *
 * Parses:
 *   AMMO <name>
 *   {
 *       <optional tuning fields — currently unused by the runtime>
 *   }
 *
 * The rb.ammo file in the shipped assets is effectively just a type-name
 * registry (empty bodies today), but may carry damage / hit-type overrides in
 * newer revisions; we parse those fields if present and leave defaults
 * otherwise.
 */
use super::block_parser::BlockParser;
use crate::weapons::components::AmmoType;
use std::collections::HashMap;
use std::sync::Arc;

pub fn parse_ammo_content(content: &str) -> HashMap<String, Arc<AmmoType>> {
    let mut hm = HashMap::new();
    let mut p = BlockParser::new(content);

    // Top-level file may be wrapped in a single outer brace pair.
    if p.peek() == Some("{") {
        p.next();
    }

    while !p.endblock() {
        if p.peek() != Some("AMMO") {
            p.next();
            continue;
        }

        let mut def = AmmoType::default();
        if let Some(name) = p.read_string_opt("AMMO") {
            def.name = name;
        }

        if p.start_anonymous() {
            while !p.endblock() {
                let key = p.peek().unwrap_or("").to_lowercase();
                let a_key = p.peek().unwrap_or("").to_string();
                if key == "end" {
                    p.next();
                    break;
                }
                match key.as_str() {
                    "damage" => def.damage = p.read_float(&a_key, 0.0),
                    "hittype" | "hit_type" => def.hit_type = p.read_i32(&a_key, 0),
                    "display" | "displayname" => def.display = p.read_string(&a_key, ""),
                    _ => {
                        p.next();
                    }
                }
            }
        }

        if !def.name.is_empty() {
            hm.insert(def.name.to_lowercase(), Arc::new(def));
        }
    }

    hm
}
