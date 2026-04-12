use std::collections::HashMap;

use super::types::*;
use super::types::{ctrl_flags, entity_flags, pad_flags};

// ---------------------------------------------------------------------------
// Flag lookup tables (built once, used for every state's rules)
// ---------------------------------------------------------------------------

fn build_pad_table() -> HashMap<&'static str, u64> {
    let mut m = HashMap::new();
    m.insert("PADCMD_ATTACK_HIGH", pad_flags::PADCMD_ATTACK_HIGH);
    m.insert("PADCMD_ATTACK_TWO_HIGH", pad_flags::PADCMD_ATTACK_TWO_HIGH);
    m.insert("PADCMD_BLOCK", pad_flags::PADCMD_BLOCK);
    m.insert("PADCMD_SWEEP_FORWARD", pad_flags::PADCMD_SWEEP_FORWARD);
    m.insert("PADCMD_GRAPPLE", pad_flags::PADCMD_GRAPPLE);
    m.insert("PADCMD_GRAPPLE_ATK", pad_flags::PADCMD_GRAPPLE_ATK);
    m.insert("PADCMD_GRAPPLE_ATK_2", pad_flags::PADCMD_GRAPPLE_ATK_2);
    m.insert("PADCMD_END_GRAPPLE", pad_flags::PADCMD_END_GRAPPLE);
    m.insert("PADCMD_LARIAT", pad_flags::PADCMD_LARIAT);
    m.insert("PADCMD_WEAPON_LOCKON", pad_flags::PADCMD_WEAPON_LOCKON);
    m.insert("PADCMD_WEAPON_FIRE_FORWARD", pad_flags::PADCMD_WEAPON_FIRE_FORWARD);
    m.insert("PADCMD_WEAPON_FIRE_LEFT", pad_flags::PADCMD_WEAPON_FIRE_LEFT);
    m.insert("PADCMD_WEAPON_FIRE_RIGHT", pad_flags::PADCMD_WEAPON_FIRE_RIGHT);
    m.insert("PADCMD_WEAPON_FIRE_BACK", pad_flags::PADCMD_WEAPON_FIRE_BACK);
    m.insert("PADCMD_WEAPON_FIRE_BACK_LEFT", pad_flags::PADCMD_WEAPON_FIRE_BACK_LEFT);
    m.insert("PADCMD_WEAPON_FIRE_BACK_RIGHT", pad_flags::PADCMD_WEAPON_FIRE_BACK_RIGHT);
    m.insert("PADCMD_WEAPON_FIRE_FWD_LEFT", pad_flags::PADCMD_WEAPON_FIRE_FWD_LEFT);
    m.insert("PADCMD_WEAPON_FIRE_FWD_RIGHT", pad_flags::PADCMD_WEAPON_FIRE_FWD_RIGHT);
    // Aliases used in player.fsm (FRONT = FWD)
    m.insert("PADCMD_WEAPON_FIRE_FRONT_LEFT", pad_flags::PADCMD_WEAPON_FIRE_FWD_LEFT);
    m.insert("PADCMD_WEAPON_FIRE_FRONT_RIGHT", pad_flags::PADCMD_WEAPON_FIRE_FWD_RIGHT);
    m.insert("PADCMD_CHR_FWD", pad_flags::PADCMD_CHR_FWD);
    m.insert("PADCMD_CHR_BAK", pad_flags::PADCMD_CHR_BAK);
    m.insert("PADCMD_CHR_LFT", pad_flags::PADCMD_CHR_LFT);
    m.insert("PADCMD_CHR_RGH", pad_flags::PADCMD_CHR_RGH);
    m.insert("PADCMD_REDIRECT_FWD_BACK", pad_flags::PADCMD_REDIRECT_FWD_BACK);
    m.insert("PADCMD_REDIRECT_FWD_BACK_LEFT", pad_flags::PADCMD_REDIRECT_FWD_BACK_LEFT);
    m.insert("PADCMD_REDIRECT_FWD_BACK_RIGHT", pad_flags::PADCMD_REDIRECT_FWD_BACK_RIGHT);
    m.insert("ACK", pad_flags::ACK);
    m.insert("ACK_LEFT", pad_flags::ACK_LEFT);
    m.insert("ACK_RIGHT", pad_flags::ACK_RIGHT);
    m.insert("ACK_FORWARD_LEFT", pad_flags::ACK_FORWARD_LEFT);
    m.insert("ACK_FORWARD_RIGHT", pad_flags::ACK_FORWARD_RIGHT);
    m.insert("ACK_BACKWARD_LEFT", pad_flags::ACK_BACKWARD_LEFT);
    m.insert("ACK_BACKWARD_RIGHT", pad_flags::ACK_BACKWARD_RIGHT);
    m.insert("ACK_TWO", pad_flags::ACK_TWO);
    m.insert("ACK_TWO_LEFT", pad_flags::ACK_TWO_LEFT);
    m.insert("ACK_TWO_RIGHT", pad_flags::ACK_TWO_RIGHT);
    m.insert("ACK_TWO_FORWARD_LEFT", pad_flags::ACK_TWO_FORWARD_LEFT);
    m.insert("ACK_TWO_FORWARD_RIGHT", pad_flags::ACK_TWO_FORWARD_RIGHT);
    m.insert("ACK_TWO_BACKWARD_LEFT", pad_flags::ACK_TWO_BACKWARD_LEFT);
    m.insert("ACK_TWO_BACKWARD_RIGHT", pad_flags::ACK_TWO_BACKWARD_RIGHT);
    m
}

fn build_ctrl_table() -> HashMap<&'static str, u32> {
    let mut m = HashMap::new();
    m.insert("REACT", ctrl_flags::REACT);
    m.insert("STANDING", ctrl_flags::STANDING);
    m.insert("WALKING", ctrl_flags::WALKING);
    m.insert("RUNNING", ctrl_flags::RUNNING);
    m.insert("REDIRECT", ctrl_flags::REDIRECT);
    m.insert("POSTHIT", ctrl_flags::POSTHIT);
    m.insert("BLOCK_SUCCESS", ctrl_flags::BLOCK_SUCCESS);
    m.insert("CRITICAL_FRAME", ctrl_flags::CRITICAL_FRAME);
    m.insert("JUMPING", ctrl_flags::JUMPING);
    m.insert("CROUCHING", ctrl_flags::CROUCHING);
    m.insert("RUNNING_JUMP", ctrl_flags::RUNNING_JUMP);
    m.insert("STANDING_JUMP", ctrl_flags::STANDING_JUMP);
    m.insert("WEAPON_DRAWN", ctrl_flags::WEAPON_DRAWN);
    m.insert("CAN_BLOCK", ctrl_flags::CAN_BLOCK);
    m.insert("HITTARGET", ctrl_flags::HITTARGET);
    m.insert("FIGHT_MODE", ctrl_flags::FIGHT_MODE);
    m.insert("ENEMY_IN_SLICE", ctrl_flags::ENEMY_IN_SLICE);
    m.insert("STARTING_CROUCH", ctrl_flags::STARTING_CROUCH);
    m.insert("ENDING_CROUCH", ctrl_flags::ENDING_CROUCH);
    m
}

fn build_entity_table() -> HashMap<&'static str, u32> {
    let mut m = HashMap::new();
    m.insert("GRAPPLING", entity_flags::GRAPPLING);
    m.insert("GETTING_GRAPPLED", entity_flags::GETTING_GRAPPLED);
    m
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse a .fsm text file into an FsmData structure.
pub fn parse_fsm(text: &str) -> Result<FsmData, String> {
    let pad = build_pad_table();
    let ctrl = build_ctrl_table();
    let ent = build_entity_table();

    // Strip semicolon comments; keep line structure
    let clean: Vec<String> = text
        .lines()
        .map(|line| {
            if let Some(idx) = line.find(';') {
                line[..idx].to_string()
            } else {
                line.to_string()
            }
        })
        .collect();

    // First pass — collect state names and their start lines
    let mut bounds: Vec<(String, usize)> = Vec::new();
    for (i, line) in clean.iter().enumerate() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('#') {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                bounds.push((name, i));
            }
        }
    }

    // Build states vec + index using just names (rules filled in second pass)
    let mut states: Vec<FsmState> = bounds
        .iter()
        .map(|(name, _)| FsmState {
            name: name.clone(),
            rules: Vec::new(),
        })
        .collect();
    let state_index: HashMap<String, usize> = bounds
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.clone(), i))
        .collect();

    // Second pass — parse rules for each state
    let n = bounds.len();
    for si in 0..n {
        let start = bounds[si].1 + 1;
        let end = if si + 1 < n { bounds[si + 1].1 } else { clean.len() };

        let rules =
            parse_state_rules(&clean[start..end], &state_index, &pad, &ctrl, &ent)?;
        states[si].rules = rules;
    }

    let total_rules: usize = states.iter().map(|s| s.rules.len()).sum();
    bevy::log::info!(
        "FSM: parsed {} states, {} rules",
        states.len(),
        total_rules
    );

    Ok(FsmData { states, state_index })
}

// ---------------------------------------------------------------------------
// Rule parsing
// ---------------------------------------------------------------------------

fn parse_state_rules(
    lines: &[String],
    state_index: &HashMap<String, usize>,
    pad: &HashMap<&str, u64>,
    ctrl: &HashMap<&str, u32>,
    ent: &HashMap<&str, u32>,
) -> Result<Vec<FsmRule>, String> {
    let mut rules = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("if ") {
            let rest = rest.trim_start();

            // Check for negation
            let (negated, rest) = if let Some(r) = rest.strip_prefix('!') {
                (true, r.trim_start())
            } else {
                (false, rest)
            };

            let event = match parse_event(rest, pad, ctrl, ent) {
                Ok(e) => e,
                Err(msg) => {
                    bevy::log::warn!("FSM parser: {}", msg);
                    i += 1;
                    continue;
                }
            };

            i += 1;

            // Scan forward for the opening '{'
            let brace_start = loop {
                if i >= lines.len() {
                    break None;
                }
                let t = lines[i].trim();
                if t.contains('{') {
                    break Some(i);
                }
                // If we hit a new #state or another 'if', the rule has no block
                if t.starts_with('#') || t.starts_with("if ") {
                    break None;
                }
                i += 1;
            };

            let Some(brace_line_idx) = brace_start else {
                continue;
            };
            i = brace_line_idx;

            let brace_line = &lines[i];
            let mut block_lines: Vec<String> = Vec::new();

            // Inline block: { ... } on one line
            if brace_line.contains('{') && brace_line.contains('}') {
                if let (Some(o), Some(c)) = (brace_line.find('{'), brace_line.rfind('}')) {
                    if o < c {
                        block_lines.push(brace_line[o + 1..c].to_string());
                    }
                }
                i += 1;
            } else {
                // Multi-line block
                i += 1; // skip the '{' line
                let mut depth: i32 = 1;
                while i < lines.len() {
                    let bl = &lines[i];
                    depth += bl.chars().filter(|&c| c == '{').count() as i32;
                    depth -= bl.chars().filter(|&c| c == '}').count() as i32;
                    if depth <= 0 {
                        i += 1;
                        break;
                    }
                    block_lines.push(bl.clone());
                    i += 1;
                }
            }

            let (actions, goto_state) = parse_rule_body(&block_lines, state_index)?;
            rules.push(FsmRule { event, negated, actions, goto_state });
        } else {
            i += 1;
        }
    }

    Ok(rules)
}

fn parse_event(
    text: &str,
    pad: &HashMap<&str, u64>,
    ctrl: &HashMap<&str, u32>,
    ent: &HashMap<&str, u32>,
) -> Result<FsmEvent, String> {
    let text = text.trim();

    // Keyword-only events (no parens)
    if text.starts_with("Timeout") {
        return Ok(FsmEvent::Timeout);
    }
    if text.starts_with("True") {
        return Ok(FsmEvent::True);
    }
    if text.starts_with("CustomAnimDone") {
        return Ok(FsmEvent::CustomAnimDone);
    }

    // Events with parenthesised arguments: Name(args)
    if let Some(paren) = text.find('(') {
        let name = text[..paren].trim();
        let close = text.rfind(')').unwrap_or(text.len());
        let args = text[paren + 1..close].trim();

        return match name {
            "Packet" => Ok(FsmEvent::Packet(parse_packet_args(args, pad, ctrl)?)),
            "Me" => Ok(FsmEvent::Me(parse_entity_flags(args, ent))),
            "Target" => Ok(FsmEvent::Target(parse_entity_flags(args, ent))),
            "Random" => Ok(FsmEvent::Random(args.parse::<f32>().unwrap_or(0.5))),
            "Timer" => Ok(FsmEvent::Timer(args.parse::<f32>().unwrap_or(1.0))),
            _ => Err(format!("unknown event '{}'", name)),
        };
    }

    // Space-separated form: "Random 0.5", "Timer 2.0"
    let mut parts = text.splitn(2, ' ');
    let name = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match name {
        "Random" => Ok(FsmEvent::Random(arg.parse::<f32>().unwrap_or(0.5))),
        "Timer" => Ok(FsmEvent::Timer(arg.parse::<f32>().unwrap_or(1.0))),
        _ => Err(format!("unknown event '{}'", name)),
    }
}

fn parse_packet_args(
    args: &str,
    pad: &HashMap<&str, u64>,
    ctrl: &HashMap<&str, u32>,
) -> Result<PacketCondition, String> {
    let mut cond = PacketCondition::default();

    for token in args.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        if let Some(rest) = token.strip_prefix("CLASSHIT:") {
            cond.class_hit = rest.trim().parse::<i32>().ok();
        } else if token.starts_with("HAS_WEAPON:") {
            // Any weapon type maps to the generic HAS_WEAPON_PIPE ctrl flag for now
            cond.ctrl_flags |= ctrl_flags::HAS_WEAPON_PIPE;
        } else if let Some(&bit) = pad.get(token) {
            cond.pad_flags |= bit;
        } else if let Some(&bit) = ctrl.get(token) {
            cond.ctrl_flags |= bit;
        } else {
            bevy::log::warn!("FSM: unknown Packet arg '{}'", token);
        }
    }

    Ok(cond)
}

fn parse_entity_flags(args: &str, ent: &HashMap<&str, u32>) -> u32 {
    let mut flags = 0u32;
    for token in args.split(',') {
        if let Some(&bit) = ent.get(token.trim()) {
            flags |= bit;
        }
    }
    flags
}

fn parse_rule_body(
    lines: &[String],
    state_index: &HashMap<String, usize>,
) -> Result<(Vec<FsmAction>, Option<usize>), String> {
    let mut actions: Vec<FsmAction> = Vec::new();
    let mut goto_state: Option<usize> = None;

    for line in lines {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }

        if let Some(target) = t.strip_prefix("goto ") {
            let name = target.trim();
            match state_index.get(name) {
                Some(&idx) => goto_state = Some(idx),
                None => bevy::log::warn!("FSM: unknown goto target '{}'", name),
            }
        } else if let Some(action) = parse_action(t, state_index) {
            actions.push(action);
        }
    }

    Ok((actions, goto_state))
}

fn parse_action(line: &str, state_index: &HashMap<String, usize>) -> Option<FsmAction> {
    // Split into at most 3 parts: verb, arg1, arg2
    let mut parts = line.splitn(3, char::is_whitespace);
    let verb = parts.next()?.trim();
    let arg1 = parts.next().unwrap_or("").trim();
    let arg2 = parts.next().unwrap_or("").trim();

    match verb {
        "DoAttack" => {
            if arg1.is_empty() {
                return None;
            }
            Some(FsmAction::DoAttack {
                anim_name: arg1.to_string(),
                rotation_notches: arg2.parse::<i32>().unwrap_or(0),
            })
        }
        "DoBlock" => {
            if arg1.is_empty() {
                return None;
            }
            Some(FsmAction::DoBlock { anim_name: arg1.to_string() })
        }
        "DoCounter" => Some(FsmAction::DoCounter),
        "DoTriggerAtk" | "TriggerAtk" => Some(FsmAction::DoTriggerAtk),
        "DoEvade" => {
            if arg1.is_empty() {
                return None;
            }
            Some(FsmAction::DoEvade {
                anim_name: arg1.to_string(),
                mirror: arg2 == "mirror" || arg2 == "true",
            })
        }
        "CtrlGrapple" => {
            Some(FsmAction::CtrlGrapple { action: arg1.to_string() })
        }
        "ChangeAttack" => {
            if arg1.is_empty() {
                return None;
            }
            Some(FsmAction::ChangeAttack {
                anim_name: arg1.to_string(),
                rotation_notches: arg2.parse::<i32>().unwrap_or(0),
            })
        }
        "CustomAnim" => {
            if arg1.is_empty() {
                return None;
            }
            Some(FsmAction::CustomAnim { anim_name: arg1.to_string() })
        }
        "Check" => {
            if arg1.is_empty() {
                return None;
            }
            match state_index.get(arg1) {
                Some(&idx) => Some(FsmAction::Check(idx)),
                None => {
                    bevy::log::warn!("FSM: Check references unknown state '{}'", arg1);
                    None
                }
            }
        }
        "ResetTimer" => Some(FsmAction::ResetTimer),
        "SetDone" | "ASetDone" => Some(FsmAction::SetDone),
        "EnablePad" | "AEnabledPad" => Some(FsmAction::EnablePad),
        "DisablePad" | "ADisablePad" => Some(FsmAction::DisablePad),
        "Display" => Some(FsmAction::Display(arg1.to_string())),
        _ => None,
    }
}
