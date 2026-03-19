use bevy::prelude::*;
use std::collections::HashMap;
use crate::oni2_loader::AnimId;

#[derive(Component, Clone, Default, Debug)]
pub struct LocomotionController {
    pub forward_gaits: Vec<LocoBlendGait>,
    pub strafe_gaits: Vec<LocoBlendGait>,
    pub transitions: HashMap<AnimId, Vec<LocoTransition>>,
}

#[derive(Clone, Default, Debug)]
pub struct LocoBlendGait {
    pub min_throttle: f32,
    pub max_throttle: f32,
    pub ideal_throttle: f32,
    pub independent_phase: bool,
    pub anim: AnimId,
}

#[derive(Clone, Default, Debug)]
pub struct LocoTransition {
    pub channel: AnimId,
    pub transition_anim: Option<AnimId>,
    pub final_anim: Option<AnimId>,
    pub num_channels_disabled: u32,
    pub stack_action: Option<String>,
}

pub fn parse_loco_content(content: &str) -> LocomotionController {
    let mut controller = LocomotionController::default();

    let mut locodata_depth = 0;
    let mut gait_depth = 0;
    let mut current_gait: Option<LocoBlendGait> = None;
    
    let mut current_transition_event: Option<AnimId> = None;
    let mut current_transition: Option<LocoTransition> = None;
    let mut pending_name: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        if trimmed == "locodata {" {
            locodata_depth += 1;
            continue;
        } else if trimmed == "transitiondata {" {
            continue;
        }

        let is_open_brace = trimmed == "{";
        let is_close_brace = trimmed == "}";

        if is_open_brace || trimmed.ends_with("{") {
            let name = if is_open_brace {
                pending_name.take().unwrap_or_default()
            } else {
                trimmed.trim_end_matches('{').trim().to_string()
            };

            if name.is_empty() {
                continue;
            }

            if locodata_depth > 0 {
                current_gait = Some(LocoBlendGait {
                    anim: AnimId::new(&name),
                    ..default()
                });
                gait_depth = locodata_depth;
            } else {
                current_transition_event = Some(AnimId::new(&name));
                current_transition = Some(LocoTransition::default());
            }
            continue;
        } else if is_close_brace {
            if let Some(gait) = current_gait.take() {
                if gait_depth == 1 {
                    controller.forward_gaits.push(gait);
                } else {
                    controller.strafe_gaits.push(gait);
                }
            } else if let Some(trans) = current_transition.take() {
                if let Some(event_id) = current_transition_event.take() {
                    controller.transitions.entry(event_id).or_default().push(trans);
                }
            } else if locodata_depth > 0 {
                locodata_depth -= 1;
            }
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        
        if parts.len() == 1 {
            pending_name = Some(parts[0].to_string());
            continue;
        }
        
        if parts.len() < 2 {
            continue;
        }
        
        let key = parts[0];
        let val = parts[1];

        if locodata_depth > 0 && current_gait.is_some() {
            let gait = current_gait.as_mut().unwrap();
            match key {
                "min_throttle" => gait.min_throttle = val.parse().unwrap_or(0.0),
                "max_throttle" => gait.max_throttle = val.parse().unwrap_or(0.0),
                "ideal_throttle" => gait.ideal_throttle = val.parse().unwrap_or(0.0),
                "independant_phase" => gait.independent_phase = val == "1",
                _ => {}
            }
        } else if current_transition.is_some() {
            let trans = current_transition.as_mut().unwrap();
            match key {
                "channel" => trans.channel = AnimId::new(val),
                "transition_anim" => trans.transition_anim = Some(AnimId::new(val)),
                "final_anim" => trans.final_anim = Some(AnimId::new(val)),
                "numchannelsdisabled" => trans.num_channels_disabled = val.parse().unwrap_or(0),
                "stackaction" => trans.stack_action = Some(val.to_string()),
                _ => {}
            }
        }
    }

    controller
}
