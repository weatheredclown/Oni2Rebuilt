/*
 * oni2_loader/parsers/atdt.rs — .atdt attack-data parser.
 *
 * AtdtStrike: one active frame window — radius, height, slice angles
 * (slicestartradians / sliceendradians / sliceheadingradiansb), damage, and
 * reaction animation index.  parse_atdt returns a Vec<AtdtStrike> consumed by
 * attack_sync_system and hit_detection_system.
 */
use super::block_parser::BlockParser;
use crate::oni2_loader::utils::space::{bevy_to_oni2_yaw_rads, oni2_to_bevy_yaw_rads};
use bevy::prelude::*;

#[derive(Debug, Clone, Reflect)]
pub struct AtdtStrike {
    pub framenum: f32,
    pub frameduration: f32,
    pub opening: f32,
    pub minreactdiskradius: f32,
    pub reactdiskradius: f32,
    pub minradiusframe: f32,
    pub maxradiusframe: f32,
    pub reactdiskheight: f32,
    pub reactdiskheighttolerance: f32,
    pub slicestartradians: f32,
    pub sliceendradians: f32,
    pub sliceheadingradiansb: f32,
    pub sweepheading: i32,
    pub use_expanding_radius: bool,
    pub minradiusphase: f32,
    pub maxradiusphase: f32,
    pub fire: bool,
    pub can_redirect: bool,
    pub end_rotation_notches: i32,
    pub stop_track_frame: f32,
    pub hittype: u8,
    pub guardtype: u8,
    pub sound: i32,
    pub soundframe: f32,
    pub sliderphase: [f32; 4],
    pub hitspeed: [f32; 4],
    pub spin: f32,
    pub holdduration: f32,
    pub reactphase: [f32; 4],
    pub reactdistance: [f32; 4],
    pub reactanim: [i32; 4],
    pub react_sliderphase: [[f32; 4]; 4],
    pub react_speed: [[f32; 4]; 4],

    // --- Combo-linking timing windows ---
    pub opp2_q_start: f32,
    pub opp2_begin_redirect: f32,
    pub opp2_do_start: f32,
    pub opp2_crit_start: f32,
    pub opp2_do_crit_start: f32,
    pub opp2_do_end: f32,
    pub opp3_q_start: f32,
    pub opp3_do_start: f32,
    pub queue_next_attack: bool,
    pub headingnotlockedtotarget: bool,
    pub vanishingpoint: f32,
}

impl Default for AtdtStrike {
    fn default() -> Self {
        Self {
            framenum: 0.0,
            frameduration: 0.0,
            opening: 0.0,
            minreactdiskradius: 0.0,
            reactdiskradius: 0.0,
            minradiusframe: 0.0,
            maxradiusframe: 0.0,
            reactdiskheight: 1.0,
            reactdiskheighttolerance: 0.1,
            slicestartradians: 0.0,
            sliceendradians: 0.0,
            sliceheadingradiansb: 0.0,
            sweepheading: 0,
            use_expanding_radius: false,
            minradiusphase: 0.0,
            maxradiusphase: 1.0,
            fire: false,
            can_redirect: false,
            end_rotation_notches: 0,
            stop_track_frame: 0.0,
            hittype: 0,
            guardtype: 0,
            sound: 0,
            soundframe: 0.0,
            sliderphase: [0.0; 4],
            hitspeed: [0.0; 4],
            spin: 0.0,
            holdduration: 0.0,
            reactphase: [0.0; 4],
            reactdistance: [0.0; 4],
            reactanim: [0; 4],
            react_sliderphase: [[0.0; 4]; 4],
            react_speed: [[0.0; 4]; 4],
            opp2_q_start: 0.0,
            opp2_begin_redirect: 0.0,
            opp2_do_start: 0.75,
            opp2_crit_start: 0.75,
            opp2_do_crit_start: 0.75,
            opp2_do_end: 0.95,
            opp3_q_start: 0.975,
            opp3_do_start: 1.0,
            queue_next_attack: true,
            headingnotlockedtotarget: false,
            vanishingpoint: 0.0,
        }
    }
}

#[derive(Debug, Clone, Reflect, Default)]
pub struct AtdtData {
    pub strike: Option<AtdtStrike>,
    pub damage: f32,
    pub block_reaction: i32,
    pub guardtype: u8,

    // Classification fields — mirror the legacy crAttackData `targetclass` /
    // `strengthclass` / `attackclass` tokens (see rb/src/fight/attackdata.cpp:276).
    // All three are Option because the engine's `NONE` variant (int value 0)
    // means "not configured" — hit-detection treats these as "unknown" and
    // skips the FX-table lookup rather than picking a bogus default.
    /// Which body zone this attack targets (Head / Body / Legs).
    /// `#[reflect(ignore)]`: combat enums don't derive Reflect and don't
    /// need to surface through the scene-reflection pipeline for an FX
    /// classification.
    #[reflect(ignore)]
    pub target_class: Option<crate::combat::components::AttackTarget>,
    /// Strength tier (Low / High / Super).  Used both for FX lookup and
    /// for combo-escalation thresholds downstream.
    #[reflect(ignore)]
    pub strength_class: Option<crate::combat::components::AttackStrength>,
    /// Attack category (Punch / Kick / Grab / RangedShot).  When present
    /// this is authoritative — overrides the hittype-derived heuristic in
    /// hit_detection_system.
    #[reflect(ignore)]
    pub attack_class: Option<crate::combat::components::AttackClass>,
}

// --- Class-token mapping helpers ---
//
// The legacy enum ordinals include a `NONE` (0) variant and are 1-based:
//   target   : 0=NONE, 1=HEAD,  2=BODY,  3=LEGS
//   strength : 0=NONE, 1=LOW,   2=HIGH,  3=SUPER
//   attack   : 0=NONE, 1=PUNCH, 2=KICK,  3=GRAB, 4=GUN_SHOT, 5=GUN_STRIKE
// We collapse `NONE` to `Option::None` so downstream doesn't have to treat
// a separate "NONE" enum value.  GUN_SHOT and GUN_STRIKE both fold into
// our single `RangedShot` variant until we care to distinguish them.

fn target_from_int(v: i32) -> Option<crate::combat::components::AttackTarget> {
    use crate::combat::components::AttackTarget;
    match v {
        0 => None,
        1 => Some(AttackTarget::Head),
        2 => Some(AttackTarget::Body),
        3 => Some(AttackTarget::Legs),
        _ => None,
    }
}

fn strength_from_int(v: i32) -> Option<crate::combat::components::AttackStrength> {
    use crate::combat::components::AttackStrength;
    match v {
        0 => None,
        1 => Some(AttackStrength::Low),
        2 => Some(AttackStrength::High),
        3 => Some(AttackStrength::Super),
        _ => None,
    }
}

fn attack_class_from_int(v: i32) -> Option<crate::combat::components::AttackClass> {
    use crate::combat::components::AttackClass;
    match v {
        0 => None,
        1 => Some(AttackClass::Punch),
        2 => Some(AttackClass::Kick),
        3 => Some(AttackClass::Grab),
        4 | 5 => Some(AttackClass::RangedShot), // GUN_SHOT + GUN_STRIKE
        _ => None,
    }
}

pub fn parse_atdt_content(content: &str) -> AtdtData {
    let mut data = AtdtData::default();
    let mut p = BlockParser::new(content);

    // Some files might be wrapped in `{` immediately
    if p.peek() == Some("{") {
        p.next();
    }

    loop {
        let peek = p.peek();
        if peek.is_none() {
            break;
        }

        let key = peek.unwrap().to_lowercase();
        let actual_key = peek.unwrap().to_string();

        match key.as_str() {
            "strike" => {
                p.next(); // Consume "strike"
                if p.start_anonymous() {
                    let mut strike = AtdtStrike::default();
                    while !p.endblock() {
                        let inner_key = p.peek().unwrap_or("").to_lowercase();
                        let a_key = p.peek().unwrap_or("").to_string();
                        match inner_key.as_str() {
                            "framenum" => strike.framenum = p.read_float(&a_key, strike.framenum),
                            "frameduration" => {
                                strike.frameduration = p.read_float(&a_key, strike.frameduration)
                            }
                            "reactdiskradius" => {
                                strike.reactdiskradius =
                                    p.read_float(&a_key, strike.reactdiskradius);
                            }
                            "minreactdiskradius" => {
                                strike.minreactdiskradius =
                                    p.read_float(&a_key, strike.minreactdiskradius);
                            }
                            "reactdiskheight" => {
                                strike.reactdiskheight =
                                    p.read_float(&a_key, strike.reactdiskheight);
                            }
                            "reactdiskheighttolerance" => {
                                strike.reactdiskheighttolerance =
                                    p.read_float(&a_key, strike.reactdiskheighttolerance);
                            }
                            "vanishingpoint" => {
                                strike.vanishingpoint = p.read_float(&a_key, strike.vanishingpoint);
                            }
                            "minradiusframe" => {
                                strike.minradiusframe = p.read_float(&a_key, strike.minradiusframe)
                            }
                            "maxradiusframe" => {
                                strike.maxradiusframe = p.read_float(&a_key, strike.maxradiusframe)
                            }
                            "slicestartradians" => {
                                let def_oni2 = bevy_to_oni2_yaw_rads(strike.slicestartradians);
                                strike.slicestartradians =
                                    oni2_to_bevy_yaw_rads(p.read_float(&a_key, def_oni2));
                            }
                            "sliceendradians" => {
                                let def_oni2 = bevy_to_oni2_yaw_rads(strike.sliceendradians);
                                strike.sliceendradians =
                                    oni2_to_bevy_yaw_rads(p.read_float(&a_key, def_oni2));
                            }
                            "sliceheadingradiansb" => {
                                let def_oni2 = bevy_to_oni2_yaw_rads(strike.sliceheadingradiansb);
                                strike.sliceheadingradiansb =
                                    oni2_to_bevy_yaw_rads(p.read_float(&a_key, def_oni2));
                            }
                            "headingnotlockedtotarget" => {
                                strike.headingnotlockedtotarget = p.read_i32(
                                    &a_key,
                                    if strike.headingnotlockedtotarget {
                                        1
                                    } else {
                                        0
                                    },
                                ) != 0
                            }
                            "sweepheading" => {
                                strike.sweepheading = p.read_i32(&a_key, strike.sweepheading)
                            }
                            "useexpandingradius" => {
                                strike.use_expanding_radius = p.read_i32(
                                    &a_key,
                                    if strike.use_expanding_radius { 1 } else { 0 },
                                ) != 0
                            }
                            "minradiusphase" => {
                                strike.minradiusphase = p.read_float(&a_key, strike.minradiusphase)
                            }
                            "maxradiusphase" => {
                                strike.maxradiusphase = p.read_float(&a_key, strike.maxradiusphase)
                            }
                            "fire" => {
                                strike.fire =
                                    p.read_i32(&a_key, if strike.fire { 1 } else { 0 }) != 0
                            }
                            "canredirect" => {
                                strike.can_redirect =
                                    p.read_i32(&a_key, if strike.can_redirect { 1 } else { 0 }) != 0
                            }
                            "endrotationnotches" => {
                                strike.end_rotation_notches =
                                    p.read_i32(&a_key, strike.end_rotation_notches);
                            }
                            "stoptrackframe" => {
                                strike.stop_track_frame =
                                    p.read_float(&a_key, strike.stop_track_frame)
                            }
                            "hittype" => {
                                strike.hittype = p.read_i32(&a_key, strike.hittype as i32) as u8
                            }
                            "sliderphase0" => {
                                strike.sliderphase[0] = p.read_float(&a_key, strike.sliderphase[0])
                            }
                            "sliderphase1" => {
                                strike.sliderphase[1] = p.read_float(&a_key, strike.sliderphase[1])
                            }
                            "sliderphase2" => {
                                strike.sliderphase[2] = p.read_float(&a_key, strike.sliderphase[2])
                            }
                            "sliderphase3" => {
                                strike.sliderphase[3] = p.read_float(&a_key, strike.sliderphase[3])
                            }
                            "hitspeed0" => {
                                strike.hitspeed[0] = p.read_float(&a_key, strike.hitspeed[0])
                            }
                            "hitspeed1" => {
                                strike.hitspeed[1] = p.read_float(&a_key, strike.hitspeed[1])
                            }
                            "hitspeed2" => {
                                strike.hitspeed[2] = p.read_float(&a_key, strike.hitspeed[2])
                            }
                            "hitspeed3" => {
                                strike.hitspeed[3] = p.read_float(&a_key, strike.hitspeed[3])
                            }
                            "reactphase0" => {
                                strike.reactphase[0] = p.read_float(&a_key, strike.reactphase[0])
                            }
                            "reactphase1" => {
                                strike.reactphase[1] = p.read_float(&a_key, strike.reactphase[1])
                            }
                            "reactphase2" => {
                                strike.reactphase[2] = p.read_float(&a_key, strike.reactphase[2])
                            }
                            "reactphase3" => {
                                strike.reactphase[3] = p.read_float(&a_key, strike.reactphase[3])
                            }
                            "reactdistance0" => {
                                strike.reactdistance[0] =
                                    p.read_float(&a_key, strike.reactdistance[0]);
                            }
                            "reactdistance1" => {
                                strike.reactdistance[1] =
                                    p.read_float(&a_key, strike.reactdistance[1]);
                            }
                            "reactdistance2" => {
                                strike.reactdistance[2] =
                                    p.read_float(&a_key, strike.reactdistance[2]);
                            }
                            "reactdistance3" => {
                                strike.reactdistance[3] =
                                    p.read_float(&a_key, strike.reactdistance[3]);
                            }
                            "reactanim0" => {
                                strike.reactanim[0] = p.read_i32(&a_key, strike.reactanim[0])
                            }
                            "reactanim1" => {
                                strike.reactanim[1] = p.read_i32(&a_key, strike.reactanim[1])
                            }
                            "reactanim2" => {
                                strike.reactanim[2] = p.read_i32(&a_key, strike.reactanim[2])
                            }
                            "reactanim3" => {
                                strike.reactanim[3] = p.read_i32(&a_key, strike.reactanim[3])
                            }
                            "atknoqueuethreshold" => {
                                strike.opp2_q_start = p.read_float(&a_key, strike.opp2_q_start)
                            }
                            "atkbeginredirectthreshold" => {
                                strike.opp2_begin_redirect =
                                    p.read_float(&a_key, strike.opp2_begin_redirect)
                            }
                            "atkendredirectthreshold" => {
                                strike.opp2_do_start = p.read_float(&a_key, strike.opp2_do_start)
                            }
                            "opp2critstart" => {
                                strike.opp2_crit_start =
                                    p.read_float(&a_key, strike.opp2_crit_start)
                            }
                            "opp2docritstart" => {
                                strike.opp2_do_crit_start =
                                    p.read_float(&a_key, strike.opp2_do_crit_start)
                            }
                            "atkendredirectlimit" => {
                                strike.opp2_do_end = p.read_float(&a_key, strike.opp2_do_end)
                            }
                            "opp3qstart" => {
                                strike.opp3_q_start = p.read_float(&a_key, strike.opp3_q_start)
                            }
                            "opp3dostart" => {
                                strike.opp3_do_start = p.read_float(&a_key, strike.opp3_do_start)
                            }
                            "queuenextattack" => {
                                strike.queue_next_attack = p
                                    .read_i32(&a_key, if strike.queue_next_attack { 1 } else { 0 })
                                    != 0
                            }
                            "blockreaction" => {
                                data.block_reaction = p.read_i32(&a_key, data.block_reaction)
                            }
                            _ => {
                                p.next();
                            }
                        }
                    }
                    // Ensure bounds are geometrically sorted (min < max) for uniform intersection checks
                    let min_bound = strike.slicestartradians.min(strike.sliceendradians);
                    let max_bound = strike.slicestartradians.max(strike.sliceendradians);
                    strike.slicestartradians = min_bound;
                    strike.sliceendradians = max_bound;

                    data.strike = Some(strike);
                }
            }
            "damage" => data.damage = p.read_float(&actual_key, data.damage),
            "blockreaction" => data.block_reaction = p.read_i32(&actual_key, data.block_reaction),
            "guardtype" => data.guardtype = p.read_i32(&actual_key, data.guardtype as i32) as u8,
            "targetclass" => data.target_class = target_from_int(p.read_i32(&actual_key, 0)),
            "strengthclass" => data.strength_class = strength_from_int(p.read_i32(&actual_key, 0)),
            "attackclass" => data.attack_class = attack_class_from_int(p.read_i32(&actual_key, 0)),
            "}" => {
                p.next();
            }
            _ => {
                p.next();
            }
        }
    }

    data
}

#[cfg(test)]
mod class_tests {
    use super::*;
    use crate::combat::components::{AttackClass, AttackStrength, AttackTarget};

    #[test]
    fn parses_all_three_classes() {
        let src = "targetclass 2\nstrengthclass 3\nattackclass 2\ndamage 10\n";
        let d = parse_atdt_content(src);
        assert_eq!(d.target_class, Some(AttackTarget::Body));
        assert_eq!(d.strength_class, Some(AttackStrength::Super));
        assert_eq!(d.attack_class, Some(AttackClass::Kick));
        assert_eq!(d.damage, 10.0);
    }

    #[test]
    fn none_value_maps_to_option_none() {
        let src = "targetclass 0\nstrengthclass 0\nattackclass 0\n";
        let d = parse_atdt_content(src);
        assert_eq!(d.target_class, None);
        assert_eq!(d.strength_class, None);
        assert_eq!(d.attack_class, None);
    }

    #[test]
    fn gun_variants_fold_to_ranged_shot() {
        for gun_val in [4, 5] {
            let src = format!("attackclass {}\n", gun_val);
            let d = parse_atdt_content(&src);
            assert_eq!(d.attack_class, Some(AttackClass::RangedShot));
        }
    }

    #[test]
    fn head_legs_round_trip() {
        let d = parse_atdt_content("targetclass 1\n");
        assert_eq!(d.target_class, Some(AttackTarget::Head));
        let d = parse_atdt_content("targetclass 3\n");
        assert_eq!(d.target_class, Some(AttackTarget::Legs));
    }
}
