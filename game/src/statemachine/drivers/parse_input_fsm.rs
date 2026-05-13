/*
 * statemachine/drivers/parse_input_fsm.rs — `.fsm` parser for the
 * input-style grammar.
 *
 * Used by InputDriver (player.fsm, enemy.fsm), BehaviorDriver
 * (BEHAVIOR_FSM constant), and AnimatorDriver (ANIMATOR_FSM constant).
 *
 * Grammar:
 *
 *     ;line comment              -- `;` at start of line
 *     //line comment             -- `//` anywhere
 *
 *     #STATE_NAME                -- state header
 *     if [!]Event(args)          -- rule with `if` keyword
 *     {
 *         Action arg1 arg2
 *         goto Target            -- `goto Target` anywhere in body
 *         ; or ...
 *         / Target                -- legacy slash-goto syntax
 *     }
 *     #NEXT_STATE
 *     ...
 *
 * No outer `{}`.  State bodies are NOT brace-wrapped — rules just follow
 * until the next `#STATENAME` header.  Only rule action blocks are
 * brace-wrapped.
 *
 * The `#STATENAME` recognition is intentionally depth-AGNOSTIC.  Real
 * shipped `player.fsm` has a `;}` typo on line 3045 (a commented-out
 * closing brace) that leaves Pass 1's depth counter stuck at 1 forever.
 * Tracking depth strictly there would silently swallow every subsequent
 * state header; we treat `#NAME` as an unconditional state boundary
 * instead and resync the depth counter when we encounter one at an
 * unexpected level.
 */

use std::collections::HashMap;

use super::super::core::{SmData, SmDriver, SmRule, SmState};
use super::parse::{ActionParser, EventParser, Token, tokenize_fsm};

/// Parse one input-style `.fsm` source into `SmData<D>`.
pub fn parse_input_fsm<D: SmDriver>(
    text: &str,
    event_parser: EventParser<D>,
    action_parser: ActionParser<D>,
) -> Result<SmData<D>, String> {
    let tokens = tokenize_fsm(text);
    let mut sm_states: Vec<SmState<D>> = Vec::new();
    let mut state_index: HashMap<String, usize> = HashMap::new();

    // Pass 1: discover state boundaries.  `#STATENAME` and `state NAME`
    // are the only forms; depth-agnostic recognition for `#` (see file
    // header for rationale).
    let mut depth: i32 = 0;
    let mut state_bounds: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Punct('{') => depth += 1,
            Token::Punct('}') => depth -= 1,
            Token::Word(w) => {
                let mut name = String::new();
                if w == "#" && i + 1 < tokens.len() {
                    if let Token::Word(n) = &tokens[i + 1] {
                        i += 1;
                        name = n.clone();
                    }
                } else if w.starts_with('#') {
                    name = w[1..].to_string();
                } else if depth == 0 && w == "state" && i + 1 < tokens.len() {
                    if let Token::Word(n) = &tokens[i + 1] {
                        i += 1;
                        name = n.clone();
                    }
                }

                if !name.is_empty() {
                    if depth != 0 {
                        // Narrow exception for shipped FSMs: a new `#state` implicitly
                        // closes any unterminated `if {}` blocks from the previous state.
                        depth = 0;
                    }
                    state_index.insert(name.clone(), sm_states.len());
                    sm_states.push(SmState {
                        name,
                        rules: Vec::new(),
                    });
                    state_bounds.push(i);
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Pass 2: parse rule blocks within each state's slice.
    for cur in 0..sm_states.len() {
        let start = state_bounds[cur] + 1;
        let end = if cur + 1 < sm_states.len() {
            state_bounds[cur + 1]
        } else {
            tokens.len()
        };
        parse_rules_block(&tokens, start, end, cur, event_parser, action_parser, &state_index, &mut sm_states);
    }

    Ok(SmData {
        states: sm_states,
        state_index,
    })
}

fn parse_rules_block<D: SmDriver>(
    tokens: &[Token],
    start: usize,
    end: usize,
    cur_state: usize,
    event_parser: EventParser<D>,
    action_parser: ActionParser<D>,
    state_index: &HashMap<String, usize>,
    sm_states: &mut [SmState<D>],
) {
    let mut i = start;
    while i < end {
        if matches!(tokens[i], Token::Newline) {
            i += 1;
            continue;
        }

        // Collect the event text from `if Event(args)` (or whatever
        // precedes the `{`).  Stop at `{` or `}`.
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

        if i >= end || tokens[i] == Token::Punct('}') {
            break;
        }
        i += 1; // consume the rule's opening `{`

        // Strip the `if` keyword (input-style rules require it; no-op
        // if the caller forgot it).  Then strip optional `!` for negation.
        let event_str = event_str
            .strip_prefix("if ")
            .unwrap_or(event_str.as_str())
            .trim();
        let mut negated = false;
        let event_str = if let Some(rest) = event_str.strip_prefix('!') {
            negated = true;
            rest.trim()
        } else {
            event_str
        };

        let event = match event_parser(event_str, state_index) {
            Ok(e) => e,
            Err(msg) => {
                bevy::log::warn!("input_fsm parser: {}", msg);
                // Recover: skip to end of rule body.
                let mut depth = 1;
                while i < end && depth > 0 {
                    match &tokens[i] {
                        Token::Punct('{') => depth += 1,
                        Token::Punct('}') => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                continue;
            }
        };

        // Collect actions and an optional `goto` / `/` target.
        let mut actions = Vec::new();
        let mut goto_state: Option<usize> = None;
        let mut current_action_str = String::new();

        let mut flush_action = |act_str: &mut String, actions: &mut Vec<_>| {
            let s = act_str.trim();
            if !s.is_empty() {
                match action_parser(s, state_index) {
                    Ok(Some(a)) => actions.push(a),
                    Ok(None) => {}
                    Err(msg) => bevy::log::warn!("input_fsm parser: {}", msg),
                }
            }
            act_str.clear();
        };

        while i < end && tokens[i] != Token::Punct('}') {
            match &tokens[i] {
                Token::Punct('/') => {
                    flush_action(&mut current_action_str, &mut actions);
                    i += 1;
                    if i < tokens.len() {
                        if let Token::Word(w) = &tokens[i] {
                            if let Some(&idx) = state_index.get(w) {
                                goto_state = Some(idx);
                            } else {
                                bevy::log::warn!(
                                    "input_fsm parser: unknown goto target '{}'",
                                    w
                                );
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
                                bevy::log::warn!(
                                    "input_fsm parser: unknown goto target '{}'",
                                    w
                                );
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
                Token::Punct(c) => current_action_str.push(*c),
            }
            i += 1;
        }
        flush_action(&mut current_action_str, &mut actions);

        sm_states[cur_state].rules.push(SmRule {
            event,
            negated,
            actions,
            goto_state,
        });

        if i < end && tokens[i] == Token::Punct('}') {
            i += 1; // consume rule's closing `}`
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::squad::{SQUAD_ACTION_PARSER, SQUAD_EVENT_PARSER, SquadDriver};
    use super::*;

    /// Regression: `oni2/zips/assets/Statemachine/player.fsm` line 3045
    /// has `;}` — the closing brace of one rule is commented out.  The
    /// parser must still discover the state header that follows.
    #[test]
    fn unclosed_brace_does_not_swallow_subsequent_state_header() {
        let src = r#"
#FIRST_STATE
if Always
{
    Display "in first state"
;}
; ^ closing brace above is commented out — common authoring mistake

#SECOND_STATE
if Always
{
    Display "in second state"
}
"#;
        let data =
            parse_input_fsm::<SquadDriver>(src, SQUAD_EVENT_PARSER, SQUAD_ACTION_PARSER)
                .expect("parse should not fail");
        assert!(data.state_index.contains_key("FIRST_STATE"));
        assert!(
            data.state_index.contains_key("SECOND_STATE"),
            "second state must be found despite the unclosed brace — \
             this is the regression. Got states: {:?}",
            data.state_index.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn back_to_back_state_headers() {
        let src = r#"
#A
#B
#C
"#;
        let data =
            parse_input_fsm::<SquadDriver>(src, SQUAD_EVENT_PARSER, SQUAD_ACTION_PARSER)
                .expect("parse should not fail");
        for name in ["A", "B", "C"] {
            assert!(data.state_index.contains_key(name));
        }
    }

    /// `{ actions }` with no `if`-prefixed event is the legacy
    /// "fire unconditionally" shorthand — author commented out
    /// `if True` and kept just the action block.  The driver-side
    /// event parser maps empty event text to True, so this should
    /// parse without an "unknown event ''" warning.  Confirmed in
    /// shipped `player.fsm` around line 1673.
    #[test]
    fn empty_event_is_unconditional_rule() {
        let src = r#"
#A
if True
{
    Display "first"
}
;if True              ; <- author commented out the condition
{
    Display "second"  ; <- but kept the action block
}
"#;
        let data =
            parse_input_fsm::<SquadDriver>(src, SQUAD_EVENT_PARSER, SQUAD_ACTION_PARSER)
                .expect("parse should not fail");
        // Both rules should land on state A.  SquadDriver collapses every
        // event to Always, so we just confirm the count and that no panic
        // occurred — the regression here is that the second rule used to
        // emit `unknown event ''` (a parser warning) instead of being
        // accepted.
        assert_eq!(data.states.len(), 1);
        assert_eq!(
            data.states[0].rules.len(),
            2,
            "both rules should be accepted — got {} rules",
            data.states[0].rules.len()
        );
    }

    #[test]
    fn well_formed_two_state_file() {
        let src = r#"
#A
if Always
{
    Display "a"
    goto B
}

#B
if Always
{
    Display "b"
}
"#;
        let data =
            parse_input_fsm::<SquadDriver>(src, SQUAD_EVENT_PARSER, SQUAD_ACTION_PARSER)
                .expect("parse should not fail");
        assert_eq!(data.state_index.get("A").copied(), Some(0));
        assert_eq!(data.state_index.get("B").copied(), Some(1));
    }
}
