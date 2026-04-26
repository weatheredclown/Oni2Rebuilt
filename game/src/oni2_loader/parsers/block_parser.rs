/*
 * oni2_loader/parsers/block_parser.rs — token-stream parser for
 * Angel-era text assets.
 *
 * Covers every text format the ONI2 asset pipeline ships: .weap,
 * .atk, .ptx, .jump, .ui, etc.  Each of those legacy files was
 * originally read by `datAsciiTokenizer` in the C++ source — a
 * simple token stream with keyword "delimiters" (bareword matches),
 * quoted strings, numbers, and single-char punctuation.  This
 * module is the one Rust equivalent.  If you ever find yourself
 * writing another char-by-char tokenizer for an Angel format, it
 * belongs here.
 *
 * Public API is split into two layers:
 *
 *   • Low-level primitives (mirrors datAsciiTokenizer):
 *     `peek`, `next`, `check_token`, `get_token`, `get_delimiter`,
 *     `get_int`, `get_float`, `get_vec3`, `match_float`, `get_color`.
 *     Use these when you're porting a C++ parser line-for-line —
 *     each primitive maps 1:1 to a `datAsciiTokenizer` method.
 *
 *   • High-level helpers (peek-guarded key/value readers):
 *     `start_if_peek`, `start_anonymous`, `endblock`, `read_*_opt`,
 *     `read_*`.  These are more convenient for `key VALUE` patterns
 *     common in .weap-style files.  `start` (skip-until-found) is
 *     kept for a couple of scan-mode callers, but prefer
 *     `start_if_peek` — `start` can silently swallow trailing
 *     sibling keys if the named block isn't next.
 *
 * Tokenizer handles:
 *   • Whitespace separation.
 *   • `#`, `//`, `;` as line comments.
 *   • Quoted strings (quotes stripped, content kept whole — multi-word
 *     quoted strings stay ONE token).
 *   • Single-char delimiters: `{` `}` `(` `)` `,`.
 *   • Everything else is a bareword (identifier, number, keyword).
 */
use bevy::prelude::*;
use std::iter::Peekable;
use std::vec::IntoIter;

pub struct BlockParser {
    pub tokens: Peekable<IntoIter<String>>,
}

impl BlockParser {
    pub fn new(content: &str) -> Self {
        Self {
            tokens: tokenize(content).into_iter().peekable(),
        }
    }

    // -----------------------------------------------------------------
    // Low-level primitives (datAsciiTokenizer mirror)
    // -----------------------------------------------------------------

    pub fn peek(&mut self) -> Option<&str> {
        self.tokens.peek().map(|s| s.as_str())
    }

    pub fn next(&mut self) -> Option<String> {
        self.tokens.next()
    }

    /// `check_token(name)` — peek-only: if the next token matches
    /// `name` (case-insensitive), consume it and return true.
    /// Otherwise leave position unchanged.  Mirrors
    /// `datAsciiTokenizer::CheckToken`.
    pub fn check_token(&mut self, name: &str) -> bool {
        match self.peek() {
            Some(t) if t.eq_ignore_ascii_case(name) => {
                self.tokens.next();
                true
            }
            _ => false,
        }
    }

    /// Consume the next token and return it as an owned String.
    /// Errors on EOF.  Mirrors `datAsciiTokenizer::GetToken`.
    pub fn get_token(&mut self) -> Result<String, String> {
        self.tokens
            .next()
            .ok_or_else(|| "block parse: unexpected end of file".to_string())
    }

    /// Consume the next token and require it match `name`.  Works for
    /// both bareword delimiters (`{`) and keyword delimiters
    /// (`TOP_LEFT`).  Mirrors `datAsciiTokenizer::GetDelimiter`.
    pub fn get_delimiter(&mut self, name: &str) -> Result<(), String> {
        match self.tokens.next() {
            Some(ref t) if t.eq_ignore_ascii_case(name) => Ok(()),
            other => Err(format!(
                "block parse: expected delimiter '{}', got {:?}",
                name, other
            )),
        }
    }

    /// Consume the next token and parse as i32.
    pub fn get_int(&mut self) -> Result<i32, String> {
        let t = self.get_token()?;
        t.parse::<i32>()
            .map_err(|_| format!("block parse: expected int, got '{}'", t))
    }

    /// Consume the next token and parse as f32.
    pub fn get_float(&mut self) -> Result<f32, String> {
        let t = self.get_token()?;
        t.parse::<f32>()
            .map_err(|_| format!("block parse: expected float, got '{}'", t))
    }

    /// Consume three floats as a Vec3.  Caller handles any
    /// surrounding `(` / `)` delimiters separately.
    pub fn get_vec3(&mut self) -> Result<Vec3, String> {
        let x = self.get_float()?;
        let y = self.get_float()?;
        let z = self.get_float()?;
        Ok(Vec3::new(x, y, z))
    }

    /// `match_float(name)` — require keyword `name` then read a
    /// float.  Mirrors `datAsciiTokenizer::MatchFloat`.
    pub fn match_float(&mut self, name: &str) -> Result<f32, String> {
        self.get_delimiter(name)?;
        self.get_float()
    }

    /// Consume four ints as an RGBA color (legacy `COLOR r g b a`
    /// form — byte-range 0-255 per channel).
    pub fn get_color(&mut self) -> Result<[u8; 4], String> {
        let r = self.get_int()? as u8;
        let g = self.get_int()? as u8;
        let b = self.get_int()? as u8;
        let a = self.get_int()? as u8;
        Ok([r, g, b, a])
    }

    // -----------------------------------------------------------------
    // High-level helpers (block / key-value readers)
    // -----------------------------------------------------------------

    /// Skip tokens until one matches `block_name`, then consume it
    /// and the optional `{` that follows.  WARNING: will swallow
    /// every intervening token if the name never shows up.  For any
    /// caller that expects siblings after this call, prefer
    /// `start_if_peek`.
    pub fn start(&mut self, block_name: &str) -> bool {
        while let Some(t) = self.tokens.peek() {
            if t == block_name {
                self.tokens.next();
                if self.peek() == Some("{") {
                    self.tokens.next();
                }
                return true;
            }
            self.tokens.next();
        }
        false
    }

    /// Peek-only variant of `start`: returns true and consumes the
    /// header (+ optional `{`) ONLY if the current token matches.
    /// Does NOT skip intervening tokens.  Safer default for nested
    /// block grammars — see the comment on `start` for the caveat.
    pub fn start_if_peek(&mut self, block_name: &str) -> bool {
        if self.peek() == Some(block_name) {
            self.tokens.next();
            if self.peek() == Some("{") {
                self.tokens.next();
            }
            true
        } else {
            false
        }
    }

    /// Anonymous block: consume `{` if present.
    pub fn start_anonymous(&mut self) -> bool {
        if self.peek() == Some("{") {
            self.tokens.next();
            return true;
        }
        false
    }

    /// True when the next token is `}` (consumed) or there are no
    /// tokens left.  Loop driver for `while !p.endblock() { ... }`.
    pub fn endblock(&mut self) -> bool {
        if self.peek() == Some("}") {
            self.tokens.next();
            true
        } else {
            self.peek().is_none()
        }
    }

    pub fn consume_key(&mut self, expected_key: &str) -> bool {
        if self.peek() == Some(expected_key) {
            self.tokens.next();
            true
        } else {
            false
        }
    }

    pub fn read_i32_opt(&mut self, expected_key: &str) -> Option<i32> {
        if self.consume_key(expected_key)
            && let Some(v_str) = self.tokens.next()
        {
            return v_str.parse::<i32>().ok();
        }
        None
    }

    pub fn read_i32(&mut self, expected_key: &str, default: i32) -> i32 {
        self.read_i32_opt(expected_key).unwrap_or(default)
    }

    pub fn read_float_opt(&mut self, expected_key: &str) -> Option<f32> {
        if self.consume_key(expected_key)
            && let Some(v_str) = self.tokens.next()
        {
            return v_str.parse::<f32>().ok();
        }
        None
    }

    pub fn read_float(&mut self, expected_key: &str, default: f32) -> f32 {
        self.read_float_opt(expected_key).unwrap_or(default)
    }

    pub fn read_string_opt(&mut self, expected_key: &str) -> Option<String> {
        if self.consume_key(expected_key)
            && let Some(v_str) = self.tokens.next()
        {
            // The tokenizer already strips quotes from `"…"` tokens,
            // so `trim_matches` is a belt-and-suspenders no-op for
            // tokens that slipped through as-is.
            return Some(v_str.trim_matches('"').to_string());
        }
        None
    }

    pub fn read_string(&mut self, expected_key: &str, default: &str) -> String {
        self.read_string_opt(expected_key)
            .unwrap_or_else(|| default.to_string())
    }

    pub fn read_vec3_opt(&mut self, expected_key: &str) -> Option<Vec3> {
        if self.consume_key(expected_key) {
            let x = self.tokens.next()?.parse::<f32>().unwrap_or(0.0);
            let y = self.tokens.next()?.parse::<f32>().unwrap_or(0.0);
            let z = self.tokens.next()?.parse::<f32>().unwrap_or(0.0);
            return Some(Vec3::new(x, y, z));
        }
        None
    }

    pub fn read_vec3(&mut self, expected_key: &str, default: Vec3) -> Vec3 {
        self.read_vec3_opt(expected_key).unwrap_or(default)
    }

    pub fn read_vec2_opt(&mut self, expected_key: &str) -> Option<Vec2> {
        if self.consume_key(expected_key) {
            let x = self.tokens.next()?.parse::<f32>().unwrap_or(0.0);
            let y = self.tokens.next()?.parse::<f32>().unwrap_or(0.0);
            return Some(Vec2::new(x, y));
        }
        None
    }

    pub fn read_vec2(&mut self, expected_key: &str, default: Vec2) -> Vec2 {
        self.read_vec2_opt(expected_key).unwrap_or(default)
    }

    pub fn read_float_val(&mut self, default: f32) -> f32 {
        if let Some(v_str) = self.tokens.next() {
            return v_str.parse::<f32>().unwrap_or(default);
        }
        default
    }

    pub fn read_rgba(&mut self, expected_key: &str, default_base: f32) -> Option<Color> {
        while let Some(k) = self.peek() {
            if k.eq_ignore_ascii_case(expected_key) {
                self.next();
                let r = self.read_float_val(default_base) / default_base;
                let g = self.read_float_val(default_base) / default_base;
                let b = self.read_float_val(default_base) / default_base;
                let a = self.read_float_val(default_base) / default_base;
                return Some(Color::srgba(r, g, b, a));
            }
            if k == "{" || k == "}" {
                break;
            }
            self.next();
        }
        None
    }

    pub fn read_rgba_val(&mut self, expected_key: &str, default_base: f32) -> Color {
        self.read_rgba(expected_key, default_base)
            .unwrap_or(Color::srgba(1.0, 1.0, 1.0, 1.0))
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// Char-by-char tokenizer.  Strips `#` / `//` / `;` line comments,
/// preserves quoted strings as single tokens (quotes dropped),
/// treats `{` `}` `(` `)` `,` as single-char delimiters that don't
/// need surrounding whitespace.
fn tokenize(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        // Whitespace.
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Line comments: `#`, `;`, `//`.
        if c == b'#' || c == b';' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Single-char delimiters.
        if matches!(c, b'{' | b'}' | b'(' | b')' | b',') {
            out.push((c as char).to_string());
            i += 1;
            continue;
        }
        // Quoted string — slurp until closing quote (no escape
        // handling; Angel files don't use escapes).
        if c == b'"' {
            let start = i + 1;
            i = start;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let s = std::str::from_utf8(&bytes[start..i])
                .unwrap_or("")
                .to_string();
            out.push(s);
            if i < bytes.len() {
                i += 1; // closing quote
            }
            continue;
        }
        // Bareword — until whitespace, delimiter, or comment char.
        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && !matches!(
                bytes[i],
                b'{' | b'}' | b'(' | b')' | b',' | b'"' | b'#' | b';'
            )
        {
            // `//` as a mid-word boundary too.
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                break;
            }
            i += 1;
        }
        if i > start {
            let w = std::str::from_utf8(&bytes[start..i])
                .unwrap_or("")
                .to_string();
            if !w.is_empty() {
                out.push(w);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_handles_all_flavors() {
        let src = r#"
            PRELOAD_LAYOUT "uitest"
            # line comment
            ; also line comment
            // double-slash comment
            CAMERA_INIT ( -1.5 1.5 3.0 ) , ( 0 0 0 ) , 60
            "quoted multi word"
        "#;
        let toks = tokenize(src);
        assert!(toks.contains(&"PRELOAD_LAYOUT".to_string()));
        assert!(toks.contains(&"uitest".to_string()));
        // `(`, `)`, `,` are all separate tokens.
        assert!(toks.iter().any(|t| t == "("));
        assert!(toks.iter().any(|t| t == ")"));
        assert!(toks.iter().any(|t| t == ","));
        // Negative floats and their commas survive as separate tokens.
        assert!(toks.contains(&"-1.5".to_string()));
        // No comments leaked.
        assert!(!toks.iter().any(|t| t.contains("line comment")));
        assert!(!toks.iter().any(|t| t.contains("double-slash")));
        // Quoted multi-word string kept whole.
        assert!(toks.contains(&"quoted multi word".to_string()));
    }

    #[test]
    fn primitives_read_values() {
        let src = r#"KEY 42 3.14 ( 1 2 3 )"#;
        let mut p = BlockParser::new(src);
        assert!(p.check_token("KEY"));
        assert_eq!(p.get_int().unwrap(), 42);
        assert!((p.get_float().unwrap() - 3.14).abs() < 1e-4);
        p.get_delimiter("(").unwrap();
        let v = p.get_vec3().unwrap();
        assert_eq!(v, Vec3::new(1.0, 2.0, 3.0));
        p.get_delimiter(")").unwrap();
    }

    #[test]
    fn quoted_strings_drop_quotes() {
        let src = r#"LAYOUT "Main Menu Layout""#;
        let mut p = BlockParser::new(src);
        assert!(p.check_token("LAYOUT"));
        assert_eq!(p.get_token().unwrap(), "Main Menu Layout");
    }
}
