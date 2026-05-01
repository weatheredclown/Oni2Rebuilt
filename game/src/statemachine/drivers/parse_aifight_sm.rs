/*
 * statemachine/drivers/parse_aifight_sm.rs — `.fsm` parser for the
 * aifight-style nested-brace grammar.
 *
 * Used by FightDriver (fight.fsm) and SquadDriver (squad.fsm).
 *
 * Grammar:
 *
 *     {
 *         Parameters
 *         {
 *             Float P_NAME
 *             {
 *                 Title       "..."
 *                 MinValue    0.01
 *                 MaxValue    100
 *                 Step        0.1
 *             }
 *         }
 *
 *         ParameterValues
 *         {
 *             P_NAME { 0.2 }
 *         }
 *
 *         S_STATE_NAME
 *         {
 *             [!]Event(args)            { Action arg1 / TargetState }
 *             [!]Event(args)            { / TargetState }
 *                                       { Action / TargetState }   ; default rule (no event = always)
 *         }
 *
 *         S_NEXT_STATE
 *         {
 *             ...
 *         }
 *     }
 *
 * Outer `{}` mandatory.  States are `S_NAME { … }` only — `#NAME` is NOT
 * a state header in this dialect.  State bodies ARE brace-wrapped.
 * Rules use the compact form (no `if` keyword); `/ Target` is the goto
 * separator.  An empty event clause (rule starts straight with `{`) is a
 * default-true rule.
 *
 * `Parameters { ... }` and `ParameterValues { ... }` blocks at the top
 * are tuning data — skipped silently here, since we don't have a typed
 * model for them yet.
 */

use std::collections::HashMap;

use super::super::core::{SmData, SmDriver, SmRule, SmState};
use super::parse::{ActionParser, EventParser, Token, tokenize_fsm};

/// Parse one aifight-style `.fsm` source into `SmData<D>`.
pub fn parse_aifight_sm<D: SmDriver>(
    text: &str,
    event_parser: EventParser<D>,
    action_parser: ActionParser<D>,
) -> Result<SmData<D>, String> {
    let tokens = tokenize_fsm(text);
    let mut sm_states: Vec<SmState<D>> = Vec::new();
    let mut state_index: HashMap<String, usize> = HashMap::new();

    // Pass 1: discover states.  `S_*` words at depth 1 (inside the outer
    // `{}`) are state headers; the `Parameters` / `ParameterValues` blocks
    // are at the same depth but their own contents nest deeper, so the
    // depth tracking naturally avoids picking up identifiers inside them.
    let mut depth: i32 = 0;
    let mut state_bounds: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Punct('{') => depth += 1,
            Token::Punct('}') => depth -= 1,
            Token::Word(w) if depth == 1 && w.starts_with("S_") => {
                state_index.insert(w.clone(), sm_states.len());
                sm_states.push(SmState {
                    name: w.clone(),
                    rules: Vec::new(),
                });
                state_bounds.push(i);
            }
            _ => {}
        }
        i += 1;
    }

    // Pass 2: parse each state's body.  Each state is `S_NAME { rules }`,
    // so we expect the next token after the header to be `{`, then rules
    // until the matching `}`.
    for cur in 0..sm_states.len() {
        let header_idx = state_bounds[cur];
        let mut i = header_idx + 1;
        // Skip newlines/whitespace tokens.
        while i < tokens.len() && matches!(tokens[i], Token::Newline) {
            i += 1;
        }
        if i >= tokens.len() || tokens[i] != Token::Punct('{') {
            bevy::log::warn!(
                "aifight_sm parser: state '{}' has no opening brace; skipping",
                sm_states[cur].name
            );
            continue;
        }
        i += 1; // consume `{`
        let body_end = find_matching_close(&tokens, i);
        parse_rules_block(
            &tokens,
            i,
            body_end,
            cur,
            event_parser,
            action_parser,
            &state_index,
            &mut sm_states,
        );
    }

    Ok(SmData {
        states: sm_states,
        state_index,
    })
}

/// Given the index of the token AFTER an opening `{`, return the index of
/// the matching `}` (or `tokens.len()` if no match — indicates malformed
/// input).
fn find_matching_close(tokens: &[Token], start_after_open: usize) -> usize {
    let mut depth = 1;
    let mut i = start_after_open;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Punct('{') => depth += 1,
            Token::Punct('}') => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    tokens.len()
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

        // Collect event text until `{` or `}`.  In this grammar the event
        // is the bare event call (no `if` keyword) — e.g. `E_MODE(IDLE)`,
        // `!E_HAS_TARGET`, or empty (default true rule).
        let mut event_str = String::new();
        while i < end
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
        i += 1; // consume opening `{` of the rule body

        let event_str = event_str.trim();
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
                bevy::log::warn!("aifight_sm parser: {}", msg);
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

        // Collect actions and `/ Target` goto.  `goto Target` is also
        // accepted for parity with the input dialect (some shipped
        // fight-style files use it).
        let mut actions = Vec::new();
        let mut goto_state: Option<usize> = None;
        let mut current_action_str = String::new();

        let mut flush_action = |act_str: &mut String, actions: &mut Vec<_>| {
            let s = act_str.trim();
            if !s.is_empty() {
                match action_parser(s, state_index) {
                    Ok(Some(a)) => actions.push(a),
                    Ok(None) => {}
                    Err(msg) => bevy::log::warn!("aifight_sm parser: {}", msg),
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
                                    "aifight_sm parser: unknown goto target '{}'",
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
                                    "aifight_sm parser: unknown goto target '{}'",
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
            i += 1;
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

    /// Smoke test: a minimal aifight-style file with the canonical structure.
    #[test]
    fn parses_outer_brace_with_two_states() {
        let src = r#"
{
    S_FIRST
    {
                                { / S_SECOND }
    }

    S_SECOND
    {
                                { / S_FIRST }
    }
}
"#;
        let data =
            parse_aifight_sm::<SquadDriver>(src, SQUAD_EVENT_PARSER, SQUAD_ACTION_PARSER)
                .expect("parse should not fail");
        assert_eq!(data.state_index.get("S_FIRST").copied(), Some(0));
        assert_eq!(data.state_index.get("S_SECOND").copied(), Some(1));
    }

    /// Parameter blocks at the top should be skipped (their nested
    /// identifiers aren't `S_*` so depth tracking ignores them).
    #[test]
    fn skips_parameter_blocks() {
        let src = r#"
{
    Parameters
    {
        Float P_TIMEOUTDELAY
        {
            Title       "TimeOutDelay"
            MinValue    0.01
        }
    }

    ParameterValues
    {
        P_TIMEOUTDELAY  { 0.2 }
    }

    S_ONLY
    {
                                { / S_ONLY }
    }
}
"#;
        let data =
            parse_aifight_sm::<SquadDriver>(src, SQUAD_EVENT_PARSER, SQUAD_ACTION_PARSER)
                .expect("parse should not fail");
        assert_eq!(data.states.len(), 1);
        assert!(data.state_index.contains_key("S_ONLY"));
        // Crucially, no spurious `Float`/`Title`/`P_TIMEOUTDELAY` states.
        assert!(!data.state_index.contains_key("Float"));
        assert!(!data.state_index.contains_key("P_TIMEOUTDELAY"));
    }
}
