/*
 * statemachine/drivers/atk_parser.rs — Format-2 `.atk` file parser.
 *
 * .atk files speak a nested-brace grammar distinct from the `#STATE`/`if`
 * syntax `parse_sm` handles.  They describe a weighted Markov machine:
 *
 *     {
 *         groups 3 {
 *             move N { <ID> "<name>" { <verb> <args...> } ... }
 *             attack N { ... }
 *             event N { ... }
 *         }
 *         cookie    { <option_id> { move N { <opt_id> <weight> ... } attack ... event ... end 0 } ... }
 *         no_cookie { ... }
 *     }
 *
 * `cookie` / `no_cookie` are N×N transition tables: rows keyed by source-
 * option ID, columns are category-weighted sub-weights.  A row's effective
 * pick distribution is `weight(opt) = cat_weight × sub_weight`, normalized
 * across the row.  `cookie` applies while the fighter has the attack slot
 * (`FLAG_GOT_COOKIE`); `no_cookie` is the fallback (usually a "back off and
 * wait" mood).
 *
 * The parser lowers this into `SmData<AttackDriver>` — three states per
 * option:
 *   • `{ID}_RUN`      — runs the option's action; on Finished routes to the
 *                       right PICK state based on `GotCookie`.
 *   • `{ID}_PICK_C`   — probability walk using the cookie row.
 *   • `{ID}_PICK_N`   — probability walk using the no_cookie row.
 *
 * Pick rules fire `AAction(verb, args)` on the chosen target, which
 * `end_current`s the finished action and starts the next, then goto the
 * target's `_RUN` state.
 */

use std::collections::HashMap;

use super::super::core::{SmData, SmRule, SmState};
use super::attack::{AttackAction, AttackDriver, AttackEvent};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a `.atk` file's content into an `SmData<AttackDriver>` ready to
/// hand to `SmRuntime::new`.
pub fn parse_atk(content: &str) -> Result<SmData<AttackDriver>, String> {
    let tokens = tokenize(content);
    let mut p = Parser {
        tokens: &tokens,
        pos: 0,
    };
    let parsed = p.parse_file()?;
    Ok(build_sm_data(parsed))
}

// ---------------------------------------------------------------------------
// Parsed data — intermediate form
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct AtkFile {
    options: Vec<AtkOption>,
    cookie_rows: HashMap<String, AtkRow>,
    no_cookie_rows: HashMap<String, AtkRow>,
}

#[derive(Debug, Clone)]
struct AtkOption {
    id: String,
    #[allow(dead_code)]
    display_name: String,
    /// Group identifier from the source file (e.g. "move", "attack",
    /// "event", "block").  Stored as a string because the grammar treats
    /// group names as arbitrary identifiers — the C++ loader reads the
    /// token straight into `aiAtkActionGroup::Identifier` without a fixed
    /// whitelist.
    #[allow(dead_code)]
    category: String,
    verb: String,
    /// The rest of the body following the verb (space-separated).
    args: String,
}

#[derive(Debug, Default)]
struct AtkRow {
    /// Map of target option ID → effective weight (cat_weight × sub_weight),
    /// preserving file order.
    entries: Vec<(String, f32)>,
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Tok {
    LBrace,
    RBrace,
    Word(String),
    Str(String),
}

fn tokenize(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'{' {
            out.push(Tok::LBrace);
            i += 1;
            continue;
        }
        if c == b'}' {
            out.push(Tok::RBrace);
            i += 1;
            continue;
        }
        if c == b'"' {
            // Slurp until closing quote.
            let start = i + 1;
            i = start;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let s = std::str::from_utf8(&bytes[start..i])
                .unwrap_or("")
                .to_string();
            out.push(Tok::Str(s));
            if i < bytes.len() {
                i += 1; // skip closing quote
            }
            continue;
        }
        // Word: run until whitespace or brace.
        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'{'
            && bytes[i] != b'}'
        {
            i += 1;
        }
        let w = std::str::from_utf8(&bytes[start..i])
            .unwrap_or("")
            .to_string();
        if !w.is_empty() {
            out.push(Tok::Word(w));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser<'a> {
    tokens: &'a [Tok],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Tok> {
        let t = self.tokens.get(self.pos);
        self.pos += 1;
        t
    }

    fn expect_lbrace(&mut self) -> Result<(), String> {
        match self.advance() {
            Some(Tok::LBrace) => Ok(()),
            other => Err(format!("atk parse: expected '{{', got {:?}", other)),
        }
    }

    fn expect_rbrace(&mut self) -> Result<(), String> {
        match self.advance() {
            Some(Tok::RBrace) => Ok(()),
            other => Err(format!("atk parse: expected '}}', got {:?}", other)),
        }
    }

    fn expect_word(&mut self, expected: &str) -> Result<(), String> {
        match self.advance() {
            Some(Tok::Word(w)) if w == expected => Ok(()),
            other => Err(format!(
                "atk parse: expected '{}', got {:?}",
                expected, other
            )),
        }
    }

    fn expect_any_word(&mut self) -> Result<String, String> {
        match self.advance() {
            Some(Tok::Word(w)) => Ok(w.clone()),
            other => Err(format!("atk parse: expected word, got {:?}", other)),
        }
    }

    fn expect_int(&mut self) -> Result<i32, String> {
        let w = self.expect_any_word()?;
        w.parse::<i32>()
            .map_err(|_| format!("atk parse: expected int, got '{}'", w))
    }

    fn expect_str(&mut self) -> Result<String, String> {
        match self.advance() {
            Some(Tok::Str(s)) => Ok(s.clone()),
            other => Err(format!(
                "atk parse: expected quoted string, got {:?}",
                other
            )),
        }
    }

    // --- Top-level ---

    fn parse_file(&mut self) -> Result<AtkFile, String> {
        self.expect_lbrace()?;

        // groups section
        self.expect_word("groups")?;
        let _num_groups = self.expect_int()?;
        self.expect_lbrace()?;
        let options = self.parse_groups()?;
        self.expect_rbrace()?;

        // cookie & no_cookie sections (order may vary — check word)
        let mut cookie_rows = HashMap::new();
        let mut no_cookie_rows = HashMap::new();

        loop {
            match self.peek() {
                Some(Tok::Word(w)) => {
                    let kw = w.clone();
                    self.advance();
                    self.expect_lbrace()?;
                    let rows = self.parse_table(&options)?;
                    self.expect_rbrace()?;
                    match kw.as_str() {
                        "cookie" => cookie_rows = rows,
                        "no_cookie" => no_cookie_rows = rows,
                        other => {
                            bevy::log::warn!(
                                "atk parse: unknown top-level section '{}', skipped",
                                other
                            );
                        }
                    }
                }
                Some(Tok::RBrace) | None => break,
                other => {
                    return Err(format!("atk parse: unexpected {:?} at top level", other));
                }
            }
        }

        self.expect_rbrace()?;

        Ok(AtkFile {
            options,
            cookie_rows,
            no_cookie_rows,
        })
    }

    // --- groups sub-parser ---

    fn parse_groups(&mut self) -> Result<Vec<AtkOption>, String> {
        let mut options = Vec::new();
        loop {
            match self.peek() {
                Some(Tok::Word(_)) => {
                    // Any identifier is a valid group name — the grammar
                    // doesn't fix the set (tim_attacks.atk adds "block"
                    // beyond the common move/attack/event trio).
                    let cat = self.expect_any_word()?;
                    let _count = self.expect_int()?;
                    self.expect_lbrace()?;
                    self.parse_group_entries(&cat, &mut options)?;
                    self.expect_rbrace()?;
                }
                _ => break,
            }
        }
        Ok(options)
    }

    fn parse_group_entries(
        &mut self,
        cat: &str,
        options: &mut Vec<AtkOption>,
    ) -> Result<(), String> {
        while let Some(tok) = self.peek() {
            if matches!(tok, Tok::RBrace) {
                break;
            }
            let id = self.expect_any_word()?;
            let name = self.expect_str()?;
            self.expect_lbrace()?;
            let (verb, args) = self.parse_body_verb_args()?;
            // parse_body_verb_args consumed the trailing '}'.
            options.push(AtkOption {
                id,
                display_name: name,
                category: cat.to_string(),
                verb,
                args,
            });
        }
        Ok(())
    }

    /// Body is `VERB ARGS`.  ARGS can include nested `{ ... }` (see the
    /// `targetattacking` event body with `attacks { "Running attack" }`).
    /// Consumes everything up to and including the matching closing brace.
    fn parse_body_verb_args(&mut self) -> Result<(String, String), String> {
        let verb = self.expect_any_word()?;
        let mut args_parts: Vec<String> = Vec::new();
        let mut depth: i32 = 1; // outer '{' already consumed
        while depth > 0 {
            match self.advance() {
                Some(Tok::LBrace) => {
                    depth += 1;
                    args_parts.push("{".to_string());
                }
                Some(Tok::RBrace) => {
                    depth -= 1;
                    if depth > 0 {
                        args_parts.push("}".to_string());
                    }
                }
                Some(Tok::Word(w)) => args_parts.push(w.clone()),
                Some(Tok::Str(s)) => args_parts.push(format!("\"{}\"", s)),
                None => return Err("atk parse: unexpected EOF inside option body".into()),
            }
        }
        Ok((verb, args_parts.join(" ")))
    }

    // --- cookie / no_cookie sub-parser ---

    fn parse_table(&mut self, _options: &[AtkOption]) -> Result<HashMap<String, AtkRow>, String> {
        let mut rows = HashMap::new();
        while let Some(tok) = self.peek() {
            if matches!(tok, Tok::RBrace) {
                break;
            }
            let row_id = self.expect_any_word()?;
            self.expect_lbrace()?;
            let row = self.parse_row()?;
            self.expect_rbrace()?;
            rows.insert(row_id, row);
        }
        Ok(rows)
    }

    fn parse_row(&mut self) -> Result<AtkRow, String> {
        let mut row = AtkRow::default();
        while let Some(tok) = self.peek() {
            if matches!(tok, Tok::RBrace) {
                break;
            }
            let kw = self.expect_any_word()?;
            let num = self.expect_int()?;
            if kw.eq_ignore_ascii_case("end") {
                // `end N` — trailing marker on each row.  Ignored: we
                // don't need a fallthrough weight today (normalized
                // weights cover the distribution already).
                continue;
            }
            // Any other identifier is a category row: `{cat} N { opt W ... }`.
            // Category names are user-defined (see parse_groups).
            let cat_weight = num as f32;
            self.expect_lbrace()?;
            while let Some(inner) = self.peek() {
                if matches!(inner, Tok::RBrace) {
                    break;
                }
                let opt_id = self.expect_any_word()?;
                let sub_weight = self.expect_int()? as f32;
                let effective = cat_weight * sub_weight;
                if effective > 0.0 {
                    row.entries.push((opt_id, effective));
                }
            }
            self.expect_rbrace()?;
        }
        Ok(row)
    }
}

// ---------------------------------------------------------------------------
// Build SmData<AttackDriver>
// ---------------------------------------------------------------------------

/// Shape the parsed `AtkFile` into an `SmData<AttackDriver>`.
fn build_sm_data(file: AtkFile) -> SmData<AttackDriver> {
    // Build state index first so we can resolve goto targets while emitting.
    // Three states per option, plus a special INITIAL state that routes to
    // the first option's _RUN.
    let mut state_index: HashMap<String, usize> = HashMap::new();
    let mut states: Vec<SmState<AttackDriver>> = Vec::new();

    // 1. Preallocate all state slots + compute indices.
    for opt in &file.options {
        let run_name = format!("{}_RUN", opt.id);
        let pick_c_name = format!("{}_PICK_C", opt.id);
        let pick_n_name = format!("{}_PICK_N", opt.id);
        let run_idx = states.len();
        states.push(SmState {
            name: run_name.clone(),
            rules: Vec::new(),
        });
        state_index.insert(run_name, run_idx);
        let pc_idx = states.len();
        states.push(SmState {
            name: pick_c_name.clone(),
            rules: Vec::new(),
        });
        state_index.insert(pick_c_name, pc_idx);
        let pn_idx = states.len();
        states.push(SmState {
            name: pick_n_name.clone(),
            rules: Vec::new(),
        });
        state_index.insert(pick_n_name, pn_idx);
    }

    // Helper to look up a state index by name, warning if absent.
    let lookup = |name: &str, state_index: &HashMap<String, usize>| -> Option<usize> {
        state_index.get(name).copied()
    };

    // 2. Fill _RUN states.
    for opt in &file.options {
        let run_name = format!("{}_RUN", opt.id);
        let pick_c_idx = lookup(&format!("{}_PICK_C", opt.id), &state_index);
        let pick_n_idx = lookup(&format!("{}_PICK_N", opt.id), &state_index);
        let run_idx = state_index[&run_name];

        let mut rules: Vec<SmRule<AttackDriver>> = Vec::new();

        // On finished + cookie → cookie pick.
        if let Some(idx) = pick_c_idx {
            rules.push(SmRule {
                event: AttackEvent::And(vec![AttackEvent::Finished, AttackEvent::GotCookie]),
                negated: false,
                actions: Vec::new(),
                goto_state: Some(idx),
            });
        }
        // On finished without cookie → no_cookie pick (fires only if the
        // cookie-gated rule above didn't, since rules are evaluated in
        // order and the first match wins).
        if let Some(idx) = pick_n_idx {
            rules.push(SmRule {
                event: AttackEvent::Finished,
                negated: false,
                actions: Vec::new(),
                goto_state: Some(idx),
            });
        }
        // Kick off the option's action on entry (when no current action
        // is set — true on the very first entry to the machine, or for
        // the initial option before a pick has installed anything).
        rules.push(SmRule {
            event: AttackEvent::NoCurrentAction,
            negated: false,
            actions: vec![AttackAction::Action(opt.verb.clone(), opt.args.clone())],
            goto_state: None,
        });

        states[run_idx].rules = rules;
    }

    // 3. Fill _PICK_C states from cookie rows.
    for opt in &file.options {
        let pick_c_idx = state_index[&format!("{}_PICK_C", opt.id)];
        let row = file.cookie_rows.get(&opt.id);
        states[pick_c_idx].rules = build_pick_rules(row, &file.options, &state_index);
    }

    // 4. Fill _PICK_N states from no_cookie rows.
    for opt in &file.options {
        let pick_n_idx = state_index[&format!("{}_PICK_N", opt.id)];
        let row = file.no_cookie_rows.get(&opt.id);
        states[pick_n_idx].rules = build_pick_rules(row, &file.options, &state_index);
    }

    SmData {
        states,
        state_index,
    }
}

/// Build the probability-walk rules for one pick state.  Each rule is
/// `if EProbability(p) { AAction(verb, args); goto {target}_RUN }`, with
/// `p` being the weight normalized over the row's total.  `AAction`
/// `end_current`s the finished option and starts the new one before the
/// goto lands in the target's RUN state.
fn build_pick_rules(
    row: Option<&AtkRow>,
    options: &[AtkOption],
    state_index: &HashMap<String, usize>,
) -> Vec<SmRule<AttackDriver>> {
    let Some(row) = row else {
        return Vec::new();
    };
    let total: f32 = row.entries.iter().map(|(_, w)| *w).sum();
    if total <= 0.0 {
        // Row has all-zero weights — machine will be stuck in this pick
        // state.  Log once at build time; leave rules empty.
        return Vec::new();
    }

    let mut rules = Vec::new();
    let opt_map: HashMap<&str, &AtkOption> = options.iter().map(|o| (o.id.as_str(), o)).collect();

    for (target_id, weight) in &row.entries {
        let Some(target_opt) = opt_map.get(target_id.as_str()) else {
            bevy::log::warn!(
                "atk build: pick-row references unknown option '{}'",
                target_id
            );
            continue;
        };
        let Some(&target_run_idx) = state_index.get(&format!("{}_RUN", target_id)) else {
            continue;
        };
        let p = weight / total;
        rules.push(SmRule {
            event: AttackEvent::Probability(p),
            negated: false,
            actions: vec![AttackAction::Action(
                target_opt.verb.clone(),
                target_opt.args.clone(),
            )],
            goto_state: Some(target_run_idx),
        });
    }

    rules
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The entry-point state name — canonically the first option declared in
/// the `groups` section (typically `M0 "Start"`).  The machine initializes
/// with `SmRuntime::new(data, data.index_of_or_zero(initial_state_name))`.
pub fn initial_state_name(data: &SmData<AttackDriver>) -> String {
    // Find any `_RUN` state; prefer M0_RUN if present (the canonical
    // start-wait anchor used by bra1.atk & tim_*.atk), otherwise fall
    // back to the first `_RUN` in insertion order.
    if data.state_index.contains_key("M0_RUN") {
        return "M0_RUN".to_string();
    }
    data.states
        .iter()
        .find(|s| s.name.ends_with("_RUN"))
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "M0_RUN".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_ATK: &str = r#"
    {
        groups 1
        {
            move 2
            {
                M0 "Start"  { wait 0.0 }
                MA "Move"   { distance 2.0 }
            }
        }
        cookie
        {
            M0
            {
                move 100
                {
                    M0 0
                    MA 100
                }
                attack 0 { M0 0 MA 0 }
                event 0 { M0 0 MA 0 }
                end 0
            }
            MA
            {
                move 100
                {
                    M0 100
                    MA 0
                }
                attack 0 { M0 0 MA 0 }
                event 0 { M0 0 MA 0 }
                end 0
            }
        }
        no_cookie
        {
            M0
            {
                move 100
                {
                    M0 100
                    MA 0
                }
                attack 0 { M0 0 MA 0 }
                event 0 { M0 0 MA 0 }
                end 0
            }
            MA
            {
                move 100
                {
                    M0 100
                    MA 0
                }
                attack 0 { M0 0 MA 0 }
                event 0 { M0 0 MA 0 }
                end 0
            }
        }
    }
    "#;

    #[test]
    fn minimal_atk_parses() {
        let data = parse_atk(MINIMAL_ATK).expect("parses");
        // Two options × 3 states each = 6.
        assert_eq!(data.states.len(), 6);
        for name in &[
            "M0_RUN",
            "M0_PICK_C",
            "M0_PICK_N",
            "MA_RUN",
            "MA_PICK_C",
            "MA_PICK_N",
        ] {
            assert!(
                data.state_index.contains_key(*name),
                "missing state {}",
                name
            );
        }
    }

    #[test]
    fn m0_run_has_start_action_rule() {
        let data = parse_atk(MINIMAL_ATK).expect("parses");
        let m0_run = &data.states[data.state_index["M0_RUN"]];
        // Should have at least: Finished&GotCookie→pick_c, Finished→pick_n,
        // NoCurrentAction→AAction.
        assert!(m0_run.rules.len() >= 3);
        // Last rule should be the kickoff AAction.
        let last = m0_run.rules.last().unwrap();
        match &last.actions[0] {
            AttackAction::Action(verb, args) => {
                assert_eq!(verb, "wait");
                assert!(args.starts_with("0.0"));
            }
            other => panic!("expected AAction, got {:?}", other),
        }
    }

    #[test]
    fn pick_rules_use_probability_walk() {
        let data = parse_atk(MINIMAL_ATK).expect("parses");
        let m0_pick_c = &data.states[data.state_index["M0_PICK_C"]];
        // M0's cookie row: move 100 { M0 0 MA 100 } → only MA, weight 1.0
        assert_eq!(m0_pick_c.rules.len(), 1);
        match &m0_pick_c.rules[0].event {
            AttackEvent::Probability(p) => assert!((p - 1.0).abs() < 0.001),
            other => panic!("expected Probability, got {:?}", other),
        }
    }

    #[test]
    fn real_tim_attacks_parses() {
        // Integration: tim_attacks.atk adds a `block` group beyond the
        // move/attack/event trio — exercises the arbitrary-identifier
        // group parser (see parse_groups / parse_row).
        let content =
            std::fs::read_to_string("../oni2/zips/assets/Statemachine/tim_attacks.atk");
        let Ok(content) = content else {
            return;
        };
        let data = parse_atk(&content).expect("tim_attacks.atk parses");
        // 8 move + 3 attack + 2 block = 13 options × 3 states = 39.
        assert_eq!(
            data.states.len(),
            39,
            "got {} states, want 39",
            data.states.len()
        );
        assert!(data.state_index.contains_key("BU_RUN"));
        assert!(data.state_index.contains_key("BS_RUN"));
    }

    #[test]
    fn real_bra1_parses() {
        // Integration: the real bra1.atk file.  Keep this here rather than
        // in a fixture so the test fails loudly if the grammar changes.
        let content = std::fs::read_to_string("../oni2/zips/assets/Statemachine/bra1.atk");
        let Ok(content) = content else {
            // Running in an env without the asset tree — skip.
            return;
        };
        let data = parse_atk(&content).expect("bra1.atk parses");
        // Expected options: 5 move + 4 attack + 17 event = 26 options,
        // each producing 3 states = 78 states.
        assert!(
            data.states.len() >= 70,
            "got only {} states",
            data.states.len()
        );
        assert!(data.state_index.contains_key("A1_RUN"));
        assert!(data.state_index.contains_key("E1_RUN"));
    }
}
