use super::block_parser::BlockParser;
use crate::oni2_loader::parsers::types::*;

pub fn parse_expl(content: &str) -> Vec<BasicExplosionDef> {
    let mut results = Vec::new();
    let mut p = BlockParser::new(content);

    // .expl files often contain multiple BASICEXPLOSION blocks globally
    // or sometimes they are wrapped in an outer brace `{ BASICEXPLOSION }`
    if p.peek() == Some("{") {
        p.next();
    }

    while !p.endblock() {
        if p.peek() == Some("BASICEXPLOSION") {
            let name = p.read_string("BASICEXPLOSION", "");
            if p.start_anonymous() {
                let mut def = BasicExplosionDef {
                    name,
                    fx: Vec::new(),
                    ellipsoid: None,
                    r#box: None,
                };

                while !p.endblock() {
                    let key = p.peek().unwrap_or("").to_lowercase();
                    match key.as_str() {
                        "explodefx" => {
                            p.next(); // Consume "explodefx"
                            if p.start_anonymous() {
                                let mut fx = ExplodeFXDef {
                                    fx_type: String::new(),
                                    offset: [0.0; 3],
                                    delay: 0.0,
                                };
                                while !p.endblock() {
                                    let inner_key = p.peek().unwrap_or("").to_lowercase();
                                    let a_key = p.peek().unwrap_or("").to_string();
                                    match inner_key.as_str() {
                                        "fxtype" => fx.fx_type = p.read_string(&a_key, ""),
                                        "offset" => {
                                            let v = p.read_vec3(&a_key, bevy::math::Vec3::ZERO);
                                            fx.offset = [v.x, v.y, v.z];
                                        }
                                        "delay" => fx.delay = p.read_float(&a_key, 0.0),
                                        _ => {
                                            p.next();
                                        }
                                    }
                                }
                                def.fx.push(fx);
                            }
                        }
                        "ellipsoid" => {
                            p.next();
                            if p.start_anonymous() {
                                let mut ell = EllipsoidDamageDef {
                                    offset: [0.0; 3],
                                    max_radii: [1.0; 3],
                                    orientation: [0.0; 3],
                                    start_radius_percentage: 0.0,
                                    blast_duration: 0.0,
                                    max_damage: 0.0,
                                    max_damage_radius_percentage: 0.0,
                                    continuous_damage: false,
                                };
                                while !p.endblock() {
                                    let inner_key = p.peek().unwrap_or("").to_lowercase();
                                    let a_key = p.peek().unwrap_or("").to_string();
                                    match inner_key.as_str() {
                                        "offset" => {
                                            let v = p.read_vec3(&a_key, bevy::math::Vec3::ZERO);
                                            ell.offset = [v.x, v.y, v.z];
                                        }
                                        "maxradii" => {
                                            let v = p.read_vec3(&a_key, bevy::math::Vec3::ONE);
                                            ell.max_radii = [v.x, v.y, v.z];
                                        }
                                        "orientation" => {
                                            let v = p.read_vec3(&a_key, bevy::math::Vec3::ZERO);
                                            ell.orientation = [v.x, v.y, v.z];
                                        }
                                        "startradiuspercentage" => {
                                            ell.start_radius_percentage = p.read_float(&a_key, 0.0)
                                        }
                                        "blastduration" => {
                                            ell.blast_duration = p.read_float(&a_key, 0.0)
                                        }
                                        "maxdamage" => ell.max_damage = p.read_float(&a_key, 0.0),
                                        "maxdamageradiuspercentage" => {
                                            ell.max_damage_radius_percentage =
                                                p.read_float(&a_key, 0.0)
                                        }
                                        "continuousdamage" => {
                                            ell.continuous_damage = p.read_i32(&a_key, 0) != 0
                                        }
                                        _ => {
                                            p.next();
                                        }
                                    }
                                }
                                def.ellipsoid = Some(ell);
                            }
                        }
                        "box" => {
                            p.next();
                            if p.start_anonymous() {
                                let mut b = BoxDamageDef {
                                    offset: [0.0; 3],
                                    orientation: [0.0; 3],
                                    blast_duration: 0.0,
                                    continuous_damage: false,
                                    start_damage: 0.0,
                                    end_damage: 0.0,
                                    start_dimensions: [1.0; 3],
                                    end_dimensions: [1.0; 3],
                                    end_translation: [0.0; 3],
                                };
                                while !p.endblock() {
                                    let inner_key = p.peek().unwrap_or("").to_lowercase();
                                    let a_key = p.peek().unwrap_or("").to_string();
                                    match inner_key.as_str() {
                                        "offset" => {
                                            let v = p.read_vec3(&a_key, bevy::math::Vec3::ZERO);
                                            b.offset = [v.x, v.y, v.z];
                                        }
                                        "orientation" => {
                                            let v = p.read_vec3(&a_key, bevy::math::Vec3::ZERO);
                                            b.orientation = [v.x, v.y, v.z];
                                        }
                                        "blastduration" => {
                                            b.blast_duration = p.read_float(&a_key, 0.0)
                                        }
                                        "continuousdamage" => {
                                            b.continuous_damage = p.read_i32(&a_key, 0) != 0
                                        }
                                        "startdamage" => b.start_damage = p.read_float(&a_key, 0.0),
                                        "enddamage" => b.end_damage = p.read_float(&a_key, 0.0),
                                        "startdimensions" => {
                                            let v = p.read_vec3(&a_key, bevy::math::Vec3::ONE);
                                            b.start_dimensions = [v.x, v.y, v.z];
                                        }
                                        "enddimensions" => {
                                            let v = p.read_vec3(&a_key, bevy::math::Vec3::ONE);
                                            b.end_dimensions = [v.x, v.y, v.z];
                                        }
                                        "endtranslation" => {
                                            let v = p.read_vec3(&a_key, bevy::math::Vec3::ZERO);
                                            b.end_translation = [v.x, v.y, v.z];
                                        }
                                        _ => {
                                            p.next();
                                        }
                                    }
                                }
                                def.r#box = Some(b);
                            }
                        }
                        _ => {
                            p.next();
                        } // Gracefully skip unhandled or malformed tokens in BASICEXPLOSION
                    }
                }
                results.push(def);
            }
        } else {
            p.next(); // Skip unknown top-level tokens
        }
    }

    results
}
