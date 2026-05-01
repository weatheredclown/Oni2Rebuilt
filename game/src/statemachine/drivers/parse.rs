/*
 * statemachine/drivers/parse.rs — shared `.fsm` lex / type helpers.
 *
 * The two `.fsm` grammars in ONI2 (input-style with `#STATENAME` headers
 * and rule `if Event { … }` blocks, vs. aifight-style nested-brace with
 * `S_NAME { … }` bodies and bare-event rule blocks) are ENTIRELY
 * different above the lexical layer — even basic tokens like `;` and
 * `{}` mean different things in different positions.  Trying to share a
 * single parser was the source of several silent-data-loss bugs (the
 * `;}` typo in `player.fsm` would silently lose every state header
 * after it, the empty `Packet()` condition would match every packet
 * because of an absent symbol, etc.).
 *
 * Splitting the two grammars onto their own parsers means each one can
 * make strict assumptions about its dialect.  The shared lexer is small
 * enough to keep here:
 *
 *   - `Token` enum + `tokenize_fsm` produce the same token stream
 *     regardless of dialect.
 *   - `EventParser<D>` / `ActionParser<D>` are the driver-side callbacks
 *     both grammars hand off to.
 *   - `split_call` is a string helper used by the driver-side event
 *     parsers.
 *
 * Concrete parsers live in `parse_input_fsm.rs` (player/enemy/behavior/
 * animator) and `parse_aifight_sm.rs` (fight/squad).  Drivers import
 * from whichever dialect their `.fsm` source is written in.
 */

use std::collections::HashMap;

use super::super::core::SmDriver;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Word(String),
    Punct(char), // '{', '}', '(', ')', '/', ';'
    Newline,
}

pub fn tokenize_fsm(text: &str) -> Vec<Token> {
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

        // `;` is a comment marker only at start-of-line (input dialect
        // also allows mid-line `;` as an action separator — the
        // tokenizer emits it as `Punct(';')` and the dialect parser
        // decides how to interpret).  `//` is a comment anywhere.
        if (c == b';' && is_start_of_line)
            || (c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/')
        {
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
            if bytes[i] == b';' || (bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/')
            {
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

/// Driver-supplied callback to convert one event-text string into `D::Event`.
pub type EventParser<D> =
    fn(text: &str, state_index: &HashMap<String, usize>) -> Result<<D as SmDriver>::Event, String>;

/// Driver-supplied callback to convert one action-text line into `D::Action`.
/// Returning `Ok(None)` silently skips the line.
pub type ActionParser<D> = fn(
    line: &str,
    state_index: &HashMap<String, usize>,
) -> Result<Option<<D as SmDriver>::Action>, String>;

/// Helper: split `EventName(args)` into `("EventName", "args")`.  Used by
/// driver-side event parsers that need to peek inside the event call.
pub fn split_call(text: &str) -> (&str, &str) {
    match (text.find('('), text.rfind(')')) {
        (Some(o), Some(c)) if o < c => (text[..o].trim(), text[o + 1..c].trim()),
        _ => (text.trim(), ""),
    }
}
