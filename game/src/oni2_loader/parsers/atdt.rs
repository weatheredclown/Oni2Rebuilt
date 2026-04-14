/*
 * oni2_loader/parsers/atdt.rs — .atdt attack-data parser.
 *
 * AtdtStrike: one active frame window — radius, height, slice angles
 * (slicestartradians / sliceendradians / sliceheadingradiansb), damage, and
 * reaction animation index.  parse_atdt returns a Vec<AtdtStrike> consumed by
 * attack_sync_system and hit_detection_system.
 */
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
    // Field layout (from the .atdt binary format):
    //   AtkNoQueueThreshold      = before this phase: input is ignored entirely
    //   AtkBeginRedirectThreshold= redirect-window open (earliest branch point)
    //   AtkEndRedirectThreshold  = CRITICAL_FRAME queue window opens
    //   Opp2CritStart            = CRITICAL_FRAME condition becomes true (branch fires)
    //   Opp2DoCritStart          = critical "do" threshold (fires queued input immediately)
    //   AtkEndRedirectLimit      = CRITICAL_FRAME window closes
    //   Opp3QStart               = queue window for 3rd opportunity
    //   Opp3DoStart              = execute window for 3rd opportunity
    pub opp2_q_start: f32,           // AtkNoQueueThreshold
    pub opp2_begin_redirect: f32,    // AtkBeginRedirectThreshold
    pub opp2_do_start: f32,          // AtkEndRedirectThreshold: CRITICAL_FRAME queue opens
    pub opp2_crit_start: f32,        // Opp2CritStart: CRITICAL_FRAME fires
    pub opp2_do_crit_start: f32,     // Opp2DoCritStart
    pub opp2_do_end: f32,            // AtkEndRedirectLimit: CRITICAL_FRAME window closes
    pub opp3_q_start: f32,           // Opp3QStart
    pub opp3_do_start: f32,          // Opp3DoStart
    pub queue_next_attack: bool,     // if false, don't queue input at all
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
            // Default values for combo-linking timing windows
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
    /// guardtype from the AttackData-level field (0=normal, 2=unblockable, etc.)
    pub guardtype: u8,
}

pub fn parse_atdt_content(content: &str) -> AtdtData {
    let mut data = AtdtData::default();
    let mut current_strike = AtdtStrike::default();
    let mut in_strike = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        if trimmed.starts_with("Strike") && trimmed.contains("{") {
            in_strike = true;
            continue;
        }

        // We only care about closing the AttackData block, which has damage at the end usually
        // But let's just parse globally if `in_strike` is true or false
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let key = parts[0].to_lowercase();
        let val = if parts.len() > 1 { parts[1] } else { "" };

        if in_strike {
            if key == "}" {
                in_strike = false;
                data.strike = Some(current_strike.clone());
                continue;
            }

            match key.as_str() {
                "framenum" => current_strike.framenum = val.parse().unwrap_or(0.0),
                "frameduration" => current_strike.frameduration = val.parse().unwrap_or(0.0),
                "reactdiskradius" => current_strike.reactdiskradius = val.parse::<f32>().unwrap_or(0.0),
                "minreactdiskradius" => current_strike.minreactdiskradius = val.parse::<f32>().unwrap_or(0.0),
                "reactdiskheight" => current_strike.reactdiskheight = val.parse::<f32>().unwrap_or(1.0),
                "reactdiskheighttolerance" => current_strike.reactdiskheighttolerance = val.parse::<f32>().unwrap_or(0.1),
                "minradiusframe" => current_strike.minradiusframe = val.parse().unwrap_or(0.0),
                "maxradiusframe" => current_strike.maxradiusframe = val.parse().unwrap_or(0.0),
                "slicestartradians" => current_strike.slicestartradians = val.parse().unwrap_or(0.0),
                "sliceendradians" => current_strike.sliceendradians = val.parse().unwrap_or(0.0),
                "sliceheadingradiansb" => current_strike.sliceheadingradiansb = val.parse().unwrap_or(0.0),
                "hittype" => current_strike.hittype = val.parse().unwrap_or(0),
                "sliderphase0" => current_strike.sliderphase[0] = val.parse().unwrap_or(0.0),
                "sliderphase1" => current_strike.sliderphase[1] = val.parse().unwrap_or(0.0),
                "sliderphase2" => current_strike.sliderphase[2] = val.parse().unwrap_or(0.0),
                "sliderphase3" => current_strike.sliderphase[3] = val.parse().unwrap_or(0.0),
                "hitspeed0" => current_strike.hitspeed[0] = val.parse().unwrap_or(0.0),
                "hitspeed1" => current_strike.hitspeed[1] = val.parse().unwrap_or(0.0),
                "hitspeed2" => current_strike.hitspeed[2] = val.parse().unwrap_or(0.0),
                "hitspeed3" => current_strike.hitspeed[3] = val.parse().unwrap_or(0.0),
                // React phase array
                "reactphase0" => current_strike.reactphase[0] = val.parse().unwrap_or(0.0),
                "reactphase1" => current_strike.reactphase[1] = val.parse().unwrap_or(0.0),
                "reactphase2" => current_strike.reactphase[2] = val.parse().unwrap_or(0.0),
                "reactphase3" => current_strike.reactphase[3] = val.parse().unwrap_or(0.0),
                // React distance
                "reactdistance0" => current_strike.reactdistance[0] = val.parse::<f32>().unwrap_or(0.0),
                "reactdistance1" => current_strike.reactdistance[1] = val.parse::<f32>().unwrap_or(0.0),
                "reactdistance2" => current_strike.reactdistance[2] = val.parse::<f32>().unwrap_or(0.0),
                "reactdistance3" => current_strike.reactdistance[3] = val.parse::<f32>().unwrap_or(0.0),
                "reactanim0" => current_strike.reactanim[0] = val.parse().unwrap_or(0),
                "reactanim1" => current_strike.reactanim[1] = val.parse().unwrap_or(0),
                "reactanim2" => current_strike.reactanim[2] = val.parse().unwrap_or(0),
                "reactanim3" => current_strike.reactanim[3] = val.parse().unwrap_or(0),
                // Combo timing windows
                "atknoqueuethreshold"      => current_strike.opp2_q_start        = val.parse().unwrap_or(0.0),
                "atkbeginredirectthreshold"=> current_strike.opp2_begin_redirect  = val.parse().unwrap_or(0.0),
                "atkendredirectthreshold"  => current_strike.opp2_do_start        = val.parse().unwrap_or(0.75),
                "opp2critstart"            => current_strike.opp2_crit_start      = val.parse().unwrap_or(0.75),
                "opp2docritstart"          => current_strike.opp2_do_crit_start   = val.parse().unwrap_or(0.75),
                "atkendredirectlimit"      => current_strike.opp2_do_end          = val.parse().unwrap_or(0.95),
                "opp3qstart"               => current_strike.opp3_q_start         = val.parse().unwrap_or(0.975),
                "opp3dostart"              => current_strike.opp3_do_start         = val.parse().unwrap_or(1.0),
                "queuenextattack"          => current_strike.queue_next_attack     = val.parse::<i32>().unwrap_or(1) != 0,
                "blockreaction"            => data.block_reaction                 = val.parse().unwrap_or(0),
                _ => {}
            }
        } else {
            match key.as_str() {
                "damage"        => data.damage         = val.parse().unwrap_or(0.0),
                "blockreaction" => data.block_reaction = val.parse().unwrap_or(0),
                "guardtype"     => data.guardtype       = val.parse().unwrap_or(0),
                _ => {}
            }
        }
    }

    data
}
