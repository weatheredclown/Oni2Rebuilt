/*
 * oni2_loader/parsers/weap.rs — weapon parameter data parser.
 *
 * Defines the weapons and their operational modes, firing fx, and charge states.
 */
use super::block_parser::BlockParser;
use crate::weapons::components::*;
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

pub fn parse_weap_content(content: &str) -> HashMap<String, Arc<WeaponTypeData>> {
    let mut hm = HashMap::new();
    let mut p = BlockParser::new(content);

    // Some files start entirely wrapped in braces
    if p.peek() == Some("{") {
        p.next();
    }

    while !p.endblock() {
        if p.peek() != Some("BASEWEAPON") {
            p.next();
            continue;
        }

        let mut w = WeaponTypeData::default();
        if let Some(name) = p.read_string_opt("BASEWEAPON") {
            w.name = name;
        }

        if p.start_anonymous() {
            while !p.endblock() {
                if p.start("OpMode") {
                    let mut mode = OperationalMode::default();
                    mode.ammo_slot = p.read_i32("AmmoSlot", 0);
                    mode.ammo_type = p.read_string_opt("AmmoType");
                    let _weapon_animation = p.read_i32("WeaponAnimation", -1);
                    mode.chamber_time = p.read_float("ChamberTime", 0.0);
                    mode.auto_fire_delay = p.read_float("AutoFireDelay", 0.2);
                    mode.throws_weapon = p.read_i32("Throw", 0) != 0;

                    let trig = p.read_string("TrigHoldMode", "");
                    mode.trig_hold_mode = match trig.as_str() {
                        "Single" => TrigHoldMode::Single,
                        "Auto" => TrigHoldMode::Auto,
                        "Charge" => TrigHoldMode::Charge,
                        "FirstAndCharge" => TrigHoldMode::FirstAndCharge,
                        _ => TrigHoldMode::Single,
                    };

                    let aimer = p.read_i32("Aimer", 1);
                    mode.aimer = match aimer {
                        2 => Aimer::Ballistic,
                        _ => Aimer::Simple,
                    };

                    let hold = p.read_string("HoldType", "");
                    mode.hold_type = match hold.as_str() {
                        "Rifle" => HoldType::Rifle,
                        "Pipe" => HoldType::Pipe,
                        "Pistol" => HoldType::Pistol,
                        _ => HoldType::Invalid,
                    };

                    mode.shooter_standard_range = p.read_float("ShooterStandardRange", 25.0);

                    if p.start("AIParameters") {
                        let mut ai = AiParameters::default();
                        ai.minimum_range = p.read_float("MinimumRange", ai.minimum_range);
                        ai.maximum_range = p.read_float("MaximumRange", ai.maximum_range);
                        ai.desired_range = p.read_float("DesiredRange", ai.desired_range);
                        ai.burst_duration = p.read_float("BurstDuration", ai.burst_duration);
                        ai.burst_duration_randomness =
                            p.read_float("BurstDurationRandomness", ai.burst_duration_randomness);
                        ai.min_aim_time = p.read_float("MinAimTime", ai.min_aim_time);
                        ai.maximum_damage_range =
                            p.read_float("MaximumDamageRange", ai.maximum_damage_range);
                        ai.damage_cone_angle =
                            p.read_float("DamageConeAngle", ai.damage_cone_angle);
                        ai.fire_delay = p.read_float("FireDelay", ai.fire_delay);
                        ai.fire_delay_randomness =
                            p.read_float("FireDelayRandomness", ai.fire_delay_randomness);
                        mode.ai_params = ai;
                        while !p.endblock() {
                            p.next();
                        } // fast-forward any untracked AI params
                    }

                    mode.first_state.min_charge_time = p.read_float("MinChargeTime", 0.0);

                    if p.start("Projectile") {
                        let mut proj = ProjectileInfo::default();
                        proj.projectile_name = p.read_string("ProjectileType", "");
                        proj.muzzle_offset = p.read_vec3("MuzzleOffset", Vec3::ZERO);
                        proj.muzzle_eulers = p.read_vec3("MuzzleEulers", Vec3::ZERO);
                        proj.acc_target_range = p.read_float("AccTargetRange", 0.0);
                        proj.acc_target_hit_prob = p.read_float("AccTargetHitProb", 0.0);
                        proj.speed = p.read_float("Speed", 0.0);
                        proj.delay = p.read_float("Delay", 0.0);
                        proj.ammo_use = p.read_float("AmmoUse", 0.0);
                        mode.first_state.projectiles.push(proj);
                        while !p.endblock() {
                            p.next();
                        }
                    }

                    // Repeated FiringFX
                    while p.start("FiringFX") {
                        let mut fx = EffectInfo::default();
                        fx.fx_name = p.read_string("FXType", "");
                        fx.offset = p.read_vec3("Offset", Vec3::ZERO);
                        fx.delay = p.read_float("Delay", 0.0);
                        mode.first_state.firing_fx.push(fx);
                        while !p.endblock() {
                            p.next();
                        }
                    }

                    if p.start("ChargeStates") {
                        while !p.endblock() {
                            if p.start_anonymous() {
                                let mut cs = ChargeState::default();
                                cs.min_charge_time = p.read_float("MinChargeTime", 0.0);

                                if p.start("Projectile") {
                                    let mut proj = ProjectileInfo::default();
                                    proj.projectile_name = p.read_string("ProjectileType", "");
                                    proj.muzzle_offset = p.read_vec3("MuzzleOffset", Vec3::ZERO);
                                    proj.muzzle_eulers = p.read_vec3("MuzzleEulers", Vec3::ZERO);
                                    proj.acc_target_range = p.read_float("AccTargetRange", 0.0);
                                    proj.acc_target_hit_prob =
                                        p.read_float("AccTargetHitProb", 0.0);
                                    proj.speed = p.read_float("Speed", 0.0);
                                    proj.delay = p.read_float("Delay", 0.0);
                                    proj.ammo_use = p.read_float("AmmoUse", 0.0);
                                    cs.projectiles.push(proj);
                                    while !p.endblock() {
                                        p.next();
                                    }
                                }

                                while p.start("FiringFX") {
                                    let mut fx = EffectInfo::default();
                                    fx.fx_name = p.read_string("FXType", "");
                                    fx.offset = p.read_vec3("Offset", Vec3::ZERO);
                                    fx.delay = p.read_float("Delay", 0.0);
                                    cs.firing_fx.push(fx);
                                    while !p.endblock() {
                                        p.next();
                                    }
                                }

                                while p.start("ChargingFX") {
                                    let mut fx = EffectInfo::default();
                                    fx.fx_name = p.read_string("FXType", "");
                                    fx.offset = p.read_vec3("Offset", Vec3::ZERO);
                                    fx.delay = p.read_float("Delay", 0.0);
                                    cs.charging_fx.push(fx);
                                    while !p.endblock() {
                                        p.next();
                                    }
                                }

                                mode.charge_states.push(cs);
                                while !p.endblock() {
                                    p.next();
                                } // Catch trailing stuff in the anon block
                            } else {
                                p.next();
                            }
                        }
                    }
                    w.op_modes.push(mode);
                    while !p.endblock() {
                        p.next();
                    } // Close out OpMode
                } else if p.peek() == Some("EntityType") {
                    w.entity_type = p.read_string_opt("EntityType");
                } else if p.peek() == Some("GripOffset") {
                    w.grip_offset = p.read_vec3("GripOffset", Vec3::ZERO);
                } else if p.peek() == Some("GripEulers") {
                    w.grip_eulers = p.read_vec3("GripEulers", Vec3::ZERO);
                } else {
                    p.next();
                }
            }
        }

        hm.insert(w.name.to_lowercase(), Arc::new(w));
    }

    hm
}
