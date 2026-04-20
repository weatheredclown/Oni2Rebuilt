/*
 * oni2_loader/parsers/atdt.rs — .atdt attack-data parser.
 *
 * AtdtStrike: one active frame window — radius, height, slice angles
 * (slicestartradians / sliceendradians / sliceheadingradiansb), damage, and
 * reaction animation index.  parse_atdt returns a Vec<AtdtStrike> consumed by
 * attack_sync_system and hit_detection_system.
 */
use super::block_parser::BlockParser;
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
        }
    }
}

#[derive(Debug, Clone, Reflect, Default)]
pub struct AtdtData {
    pub strike: Option<AtdtStrike>,
    pub damage: f32,
    pub block_reaction: i32,
    pub guardtype: u8,
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
                                    p.read_float(&a_key, strike.reactdiskradius)
                            }
                            "minreactdiskradius" => {
                                strike.minreactdiskradius =
                                    p.read_float(&a_key, strike.minreactdiskradius)
                            }
                            "reactdiskheight" => {
                                strike.reactdiskheight =
                                    p.read_float(&a_key, strike.reactdiskheight)
                            }
                            "reactdiskheighttolerance" => {
                                strike.reactdiskheighttolerance =
                                    p.read_float(&a_key, strike.reactdiskheighttolerance)
                            }
                            "minradiusframe" => {
                                strike.minradiusframe = p.read_float(&a_key, strike.minradiusframe)
                            }
                            "maxradiusframe" => {
                                strike.maxradiusframe = p.read_float(&a_key, strike.maxradiusframe)
                            }
                            "slicestartradians" => {
                                strike.slicestartradians =
                                    p.read_float(&a_key, strike.slicestartradians)
                            }
                            "sliceendradians" => {
                                strike.sliceendradians =
                                    p.read_float(&a_key, strike.sliceendradians)
                            }
                            "sliceheadingradiansb" => {
                                strike.sliceheadingradiansb =
                                    p.read_float(&a_key, strike.sliceheadingradiansb)
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
                                    p.read_i32(&a_key, strike.end_rotation_notches)
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
                                    p.read_float(&a_key, strike.reactdistance[0])
                            }
                            "reactdistance1" => {
                                strike.reactdistance[1] =
                                    p.read_float(&a_key, strike.reactdistance[1])
                            }
                            "reactdistance2" => {
                                strike.reactdistance[2] =
                                    p.read_float(&a_key, strike.reactdistance[2])
                            }
                            "reactdistance3" => {
                                strike.reactdistance[3] =
                                    p.read_float(&a_key, strike.reactdistance[3])
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
                    data.strike = Some(strike);
                }
            }
            "damage" => data.damage = p.read_float(&actual_key, data.damage),
            "blockreaction" => data.block_reaction = p.read_i32(&actual_key, data.block_reaction),
            "guardtype" => data.guardtype = p.read_i32(&actual_key, data.guardtype as i32) as u8,
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
