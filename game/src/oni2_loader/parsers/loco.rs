use bevy::prelude::*;
use std::collections::HashMap;
use crate::oni2_loader::AnimId;

#[derive(Component, Clone, Default, Debug)]
pub struct LocomotionController {
    pub blend_spaces: Vec<LocoBlendGait>,
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

    let mut in_locodata = false;
    let mut current_gait: Option<LocoBlendGait> = None;
    
    let mut current_transition_event: Option<AnimId> = None;
    let mut current_transition: Option<LocoTransition> = None;
    let mut nesting_level = 0;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        if trimmed == "locodata {" {
            in_locodata = true;
            nesting_level += 1;
            continue;
        } else if trimmed.ends_with("{") {
            let name = trimmed.trim_end_matches('{').trim();
            if in_locodata {
                current_gait = Some(LocoBlendGait {
                    anim: AnimId::new(name),
                    ..default()
                });
            } else {
                current_transition_event = Some(AnimId::new(name));
                current_transition = Some(LocoTransition::default());
            }
            nesting_level += 1;
            continue;
        } else if trimmed == "}" {
            nesting_level -= 1;
            if in_locodata && nesting_level == 1 {
                if let Some(gait) = current_gait.take() {
                    controller.blend_spaces.push(gait);
                }
            } else if nesting_level == 0 {
                if in_locodata {
                    in_locodata = false;
                } else if let Some(event_id) = current_transition_event.take() {
                    if let Some(trans) = current_transition.take() {
                        controller.transitions.entry(event_id).or_default().push(trans);
                    }
                }
            }
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let key = parts[0];
        let val = parts[1];

        if in_locodata && current_gait.is_some() {
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
