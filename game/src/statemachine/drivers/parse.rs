/*
 * statemachine/drivers/parse.rs — generic state/rule skeleton parser.
 *
 * All three ONI2 state-machine dialects (.fsm, aifight .sm, .atk) share the
 * same surface syntax — a `#` state header followed by `if [!]Event { actions;
 * [goto Target;] }` blocks. Only the Event and Action vocabularies differ.
 *
 * This module exposes `parse_sm<D, ...>`, a generic token-stream reader that
 * gracefully handles comments, multi-line rules, inline transitions, and legacy
 * C++ parser quirks without relying on fragile string slicing.
 */
use std::collections::HashMap;

use super::super::core::{SmData, SmDriver, SmRule, SmState};

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Punct(char), // '{', '}', '(', ')', '/', ';'
    Newline,
}

fn tokenize_fsm(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut is_start_of_line = true;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            out.push(Token::Newline);
            is_start_of_line = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Handle // comments and ; line comments (only at start of line for ;)
        if (c == b';' && is_start_of_line) || (c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        
        is_start_of_line = false;

        if matches!(c, b'{' | b'}' | b'(' | b')' | b'/' | b';') {
            out.push(Token::Punct(c as char));
            i += 1;
            continue;
        }

        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && !matches!(bytes[i], b'{' | b'}' | b'(' | b')' | b'/' | b';')
        {
            if bytes[i] == b';' || (bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/') {
                break;
            }
            i += 1;
        }
        let w = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
        if !w.is_empty() {
            out.push(Token::Word(w.to_string()));
        }
    }
    out
}

/// Driver-supplied callback to convert one event-text line into `D::Event`.
pub type EventParser<D> =
    fn(text: &str, state_index: &HashMap<String, usize>) -> Result<<D as SmDriver>::Event, String>;

/// Driver-supplied callback to convert one action-text line into `D::Action`.
/// Returning `Ok(None)` silently skips the line.
pub type ActionParser<D> = fn(
    line: &str,
    state_index: &HashMap<String, usize>,
) -> Result<Option<<D as SmDriver>::Action>, String>;

/// Parse one text source into `SmData<D>`.
pub fn parse_sm<D: SmDriver>(
    text: &str,
    event_parser: EventParser<D>,
    action_parser: ActionParser<D>,
) -> Result<SmData<D>, String> {
    let tokens = tokenize_fsm(text);
    let mut sm_states = Vec::new();
    let mut state_index = HashMap::new();

    // Determine base depth by checking if the file is wrapped in an outer brace
    let mut base_depth = 0;
    for t in &tokens {
        match t {
            Token::Newline => {}
            Token::Punct('{') => {
                base_depth = 1;
                break;
            }
            _ => break,
        }
    }

    // Pass 1: Discover state boundaries
    let mut depth = 0;
    let mut temp_i = 0;
    
    // We store (name, header_token_index) so we can slice the token stream perfectly.
    let mut state_bounds = Vec::new();
    
    while temp_i < tokens.len() {
        match &tokens[temp_i] {
            Token::Punct('{') => depth += 1,
            Token::Punct('}') => depth -= 1,
            Token::Word(w) if depth == base_depth => {
                let name = if w == "#" && temp_i + 1 < tokens.len() {
                    if let Token::Word(n) = &tokens[temp_i + 1] {
                        temp_i += 1;
                        n.clone()
                    } else {
                        "".to_string()
                    }
                } else if w.starts_with("#") {
                    w[1..].to_string()
                } else if w == "state" && temp_i + 1 < tokens.len() {
                    if let Token::Word(n) = &tokens[temp_i + 1] {
                        temp_i += 1;
                        n.clone()
                    } else {
                        "".to_string()
                    }
                } else if w.starts_with("S_") {
                    w.clone()
                } else {
                    "".to_string()
                };

                if !name.is_empty() {
                    state_index.insert(name.clone(), sm_states.len());
                    sm_states.push(SmState {
                        name: name.clone(),
                        rules: Vec::new(),
                    });
                    state_bounds.push(temp_i);
                }
            }
            _ => {}
        }
        temp_i += 1;
    }

    // Pass 2: Parse blocks using bounds
    for current_state in 0..sm_states.len() {
        let start_idx = state_bounds[current_state] + 1;
        let end_idx = if current_state + 1 < sm_states.len() {
            state_bounds[current_state + 1]
        } else {
            tokens.len()
        };

        let mut i = start_idx;
        let mut current_end_idx = end_idx;

        // Skip leading whitespace/comments
        while i < current_end_idx && matches!(tokens[i], Token::Newline) {
            i += 1;
        }

        // Check if the state is wrapped in a structural outer brace.
        // If the first token is '{' and the block contains inner '{' tokens, it's an outer wrapper.
        if i < current_end_idx && tokens[i] == Token::Punct('{') {
            let mut inner_braces = 0;
            let mut search_depth = 1;
            let mut search_i = i + 1;
            let mut matching_brace_idx = search_i;
            while search_i < current_end_idx && search_depth > 0 {
                if tokens[search_i] == Token::Punct('{') {
                    search_depth += 1;
                    inner_braces += 1;
                } else if tokens[search_i] == Token::Punct('}') {
                    search_depth -= 1;
                    if search_depth == 0 {
                        matching_brace_idx = search_i;
                    }
                }
                search_i += 1;
            }
            if search_depth == 0 && inner_braces > 0 {
                i += 1; // Strip the outer '{'
                current_end_idx = matching_brace_idx; // Strip the outer '}'
            }
        }

        while i < current_end_idx {
            if matches!(tokens[i], Token::Newline) {
                i += 1;
                continue;
            }
            // Extract Event
            let mut event_str = String::new();
            while i < tokens.len()
                && tokens[i] != Token::Punct('{')
                && tokens[i] != Token::Punct('}')
            {
                match &tokens[i] {
                    Token::Word(w) => {
                        if !event_str.is_empty() && !event_str.ends_with('(') {
                            event_str.push(' ');
                        }
                        event_str.push_str(w);
                    }
                    Token::Punct(c) => event_str.push(*c),
                    Token::Newline => {}
                }
                i += 1;
            }

            if i >= current_end_idx || tokens[i] == Token::Punct('}') {
                break;
            }
            i += 1; // Consume rule '{'

            let event_str = event_str.strip_prefix("if ").unwrap_or(event_str.as_str()).trim();
            let mut negated = false;
            let event_str = if let Some(rest) = event_str.strip_prefix('!') {
                negated = true;
                rest.trim()
            } else {
                event_str
            };

            // println!("DEBUG EVENT: '{}'", event_str);

            let event = match event_parser(event_str, &state_index) {
                Ok(e) => e,
                Err(msg) => {
                    bevy::log::warn!("sm parser: {}", msg);
                    
                    // Recover: skip to end of rule body
                    let mut rule_depth = 1;
                    while i < current_end_idx && rule_depth > 0 {
                        match &tokens[i] {
                            Token::Punct('{') => rule_depth += 1,
                            Token::Punct('}') => rule_depth -= 1,
                            _ => {}
                        }
                        i += 1;
                    }
                    continue;
                }
            };

            // Extract Actions & Goto
            let mut actions = Vec::new();
            let mut goto_state = None;
            let mut current_action_str = String::new();

            let mut flush_action = |act_str: &mut String, actions: &mut Vec<_>| {
                let s = act_str.trim();
                if !s.is_empty() {
                    match action_parser(s, &state_index) {
                        Ok(Some(a)) => actions.push(a),
                        Ok(None) => {}
                        Err(msg) => {
                            bevy::log::warn!("sm parser: {}", msg);
                        }
                    }
                }
                act_str.clear();
            };

            while i < current_end_idx && tokens[i] != Token::Punct('}') {
                match &tokens[i] {
                    Token::Punct('/') => {
                        flush_action(&mut current_action_str, &mut actions);
                        i += 1;
                        if i < tokens.len() {
                            if let Token::Word(w) = &tokens[i] {
                                if let Some(&idx) = state_index.get(w) {
                                    goto_state = Some(idx);
                                } else {
                                    bevy::log::warn!("sm parser: unknown goto target '{}'", w);
                                }
                                i += 1;
                            }
                        }
                        continue;
                    }
                    Token::Word(w) if w == "goto" => {
                        flush_action(&mut current_action_str, &mut actions);
                        i += 1;
                        if i < tokens.len() {
                            if let Token::Word(w) = &tokens[i] {
                                if let Some(&idx) = state_index.get(w) {
                                    goto_state = Some(idx);
                                } else {
                                    bevy::log::warn!("sm parser: unknown goto target '{}'", w);
                                }
                                i += 1;
                            }
                        }
                        continue;
                    }
                    Token::Punct(';') | Token::Newline => {
                        flush_action(&mut current_action_str, &mut actions);
                    }
                    Token::Word(w) => {
                        if !current_action_str.is_empty() {
                            current_action_str.push(' ');
                        }
                        current_action_str.push_str(w);
                    }
                    Token::Punct(c) => {
                        current_action_str.push(*c);
                    }
                }
                i += 1;
            }

            flush_action(&mut current_action_str, &mut actions);

            sm_states[current_state].rules.push(SmRule {
                event,
                negated,
                actions,
                goto_state,
            });

            if i < current_end_idx && tokens[i] == Token::Punct('}') {
                i += 1; // Consume rule '}'
            }
        }
    }

    Ok(SmData {
        states: sm_states,
        state_index,
    })
}

/// Helper to split `EventName(args)` into `("EventName", "args")`.
pub fn split_call(text: &str) -> (&str, &str) {
    match (text.find('('), text.rfind(')')) {
        (Some(o), Some(c)) if o < c => (text[..o].trim(), text[o + 1..c].trim()),
        _ => (text.trim(), ""),
    }
}
