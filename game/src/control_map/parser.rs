/*
 * control_map/parser.rs — text parser for control.map.
 *
 * Tokenises the whitespace-delimited brace format, then does a single-pass
 * recursive-descent parse.  Lines starting with ';' are comments.
 *
 * Grammar (informal):
 *   file        := command*
 *   command     := "COMMAND" NAME "{" method* "}"
 *   method      := "METHOD" "{" (input_block | exclude_block)* "}"
 *   input_block := "INPUT" "{" constraint* "}"
 *   exclude_block := "EXCLUDE_INPUT" "{" constraint* "}"
 *   constraint  := "BUTTON_PRESS" NAME
 *               |  "BUTTON_HOLD"  NAME
 *               |  "ANALOG_RANGE"    NAME float float
 *               |  "ANALOG_DEBOUNCE" NAME float float
 *               |  "ANALOG_SHAKE"    NAME float float
 *               |  "WINDOW_OF_OPPORTUNITY" float
 */

use super::types::*;

// ---------------------------------------------------------------------------
// Tokeniser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    LBrace,
    RBrace,
}

// Token derives Clone so Cursor::next() can return owned values, avoiding
// lifetime/borrow conflicts when the token is used in an error message alongside
// self.pos (which would otherwise trigger E0502).


fn tokenize(src: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    for line in src.lines() {
        // Strip comments: anything after ';'
        let line = match line.find(';') {
            Some(pos) => &line[..pos],
            None => line,
        };
        for word in line.split_whitespace() {
            match word {
                "{" => tokens.push(Token::LBrace),
                "}" => tokens.push(Token::RBrace),
                other => {
                    // A word might be glued to a brace, e.g. "name{" or "}name"
                    // Walk character by character and split on braces.
                    let mut buf = String::new();
                    for ch in other.chars() {
                        match ch {
                            '{' => {
                                if !buf.is_empty() {
                                    tokens.push(Token::Word(buf.clone()));
                                    buf.clear();
                                }
                                tokens.push(Token::LBrace);
                            }
                            '}' => {
                                if !buf.is_empty() {
                                    tokens.push(Token::Word(buf.clone()));
                                    buf.clear();
                                }
                                tokens.push(Token::RBrace);
                            }
                            c => buf.push(c),
                        }
                    }
                    if !buf.is_empty() {
                        tokens.push(Token::Word(buf));
                    }
                }
            }
        }
    }
    tokens
}

// ---------------------------------------------------------------------------
// Token cursor
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Cursor { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    /// Returns the next token by *value* (cloned) and advances the cursor.
    /// Returning an owned Token avoids borrow-checker conflicts when the token
    /// appears alongside `self.pos` in error messages.
    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect_word(&mut self) -> Result<String, String> {
        let pos = self.pos;
        match self.next() {
            Some(Token::Word(w)) => Ok(w),
            other => Err(format!("expected word, got {:?} at pos {}", other, pos)),
        }
    }

    fn expect_float(&mut self) -> Result<f32, String> {
        let w = self.expect_word()?;
        w.parse::<f32>()
            .map_err(|_| format!("expected float, got '{}'", w))
    }

    fn expect_lbrace(&mut self) -> Result<(), String> {
        let pos = self.pos;
        match self.next() {
            Some(Token::LBrace) => Ok(()),
            other => Err(format!("expected '{{', got {:?} at pos {}", other, pos)),
        }
    }

    fn expect_rbrace(&mut self) -> Result<(), String> {
        let pos = self.pos;
        match self.next() {
            Some(Token::RBrace) => Ok(()),
            other => Err(format!("expected '}}', got {:?} at pos {}", other, pos)),
        }
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parse a complete control.map file.
pub fn parse(src: &str) -> Result<ControlMap, String> {
    let tokens = tokenize(src);
    let mut cur = Cursor::new(&tokens);
    let mut map = ControlMap::default();

    while cur.peek().is_some() {
        match cur.peek() {
            Some(Token::Word(w)) if w == "COMMAND" => {
                cur.next(); // consume "COMMAND"
                let cmd = parse_command(&mut cur)?;
                map.commands.push(cmd);
            }
            Some(t) => {
                return Err(format!("unexpected token at top level: {:?}", t));
            }
            None => break,
        }
    }

    Ok(map)
}

fn parse_command(cur: &mut Cursor) -> Result<PadCommand, String> {
    let name = cur.expect_word()?;
    cur.expect_lbrace()?;

    let mut methods = Vec::new();
    loop {
        match cur.peek() {
            Some(Token::RBrace) => {
                cur.next();
                break;
            }
            Some(Token::Word(w)) if w == "METHOD" => {
                cur.next();
                methods.push(parse_method(cur)?);
            }
            other => {
                return Err(format!(
                    "expected METHOD or '}}' in command '{}', got {:?}",
                    name, other
                ));
            }
        }
    }

    Ok(PadCommand { name, methods })
}

fn parse_method(cur: &mut Cursor) -> Result<PadMethod, String> {
    cur.expect_lbrace()?;
    let mut method = PadMethod::default();

    loop {
        match cur.peek() {
            Some(Token::RBrace) => {
                cur.next();
                break;
            }
            Some(Token::Word(w)) if w == "INPUT" => {
                cur.next();
                let block = parse_input_block(cur)?;
                method.inputs.push(block);
            }
            Some(Token::Word(w)) if w == "EXCLUDE_INPUT" => {
                cur.next();
                let block = parse_input_block(cur)?;
                method.exclude = Some(block);
            }
            other => {
                return Err(format!("expected INPUT/EXCLUDE_INPUT/'}}, got {:?}", other));
            }
        }
    }

    Ok(method)
}

fn parse_input_block(cur: &mut Cursor) -> Result<InputBlock, String> {
    cur.expect_lbrace()?;
    let mut block = InputBlock::default();

    loop {
        match cur.peek() {
            Some(Token::RBrace) => {
                cur.next();
                break;
            }
            Some(Token::Word(w)) => {
                let kw = w.clone();
                cur.next();
                match kw.as_str() {
                    "BUTTON_PRESS" => {
                        let name = cur.expect_word()?;
                        block.constraints.push(Constraint::ButtonPress(name));
                    }
                    "BUTTON_HOLD" => {
                        let name = cur.expect_word()?;
                        block.constraints.push(Constraint::ButtonHold(name));
                    }
                    "ANALOG_RANGE" => {
                        let axis = cur.expect_word()?;
                        let min = cur.expect_float()?;
                        let max = cur.expect_float()?;
                        block.constraints.push(Constraint::AnalogRange { axis, min, max });
                    }
                    "ANALOG_DEBOUNCE" => {
                        let axis = cur.expect_word()?;
                        let min = cur.expect_float()?;
                        let max = cur.expect_float()?;
                        block.constraints.push(Constraint::AnalogDebounce { axis, min, max });
                    }
                    "ANALOG_SHAKE" => {
                        let axis = cur.expect_word()?;
                        let min = cur.expect_float()?;
                        let max = cur.expect_float()?;
                        block.constraints.push(Constraint::AnalogShake { axis, min, max });
                    }
                    "WINDOW_OF_OPPORTUNITY" => {
                        let t = cur.expect_float()?;
                        block.window_of_opportunity = Some(t);
                    }
                    other => {
                        return Err(format!("unknown constraint keyword: '{}'", other));
                    }
                }
            }
            other => {
                return Err(format!("unexpected token in INPUT block: {:?}", other));
            }
        }
    }

    Ok(block)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_button_press() {
        let src = r#"
COMMAND PADCMD_JUMP
{
    METHOD
    {
        INPUT
        {
            BUTTON_PRESS Rdown
        }
    }
}
"#;
        let map = parse(src).unwrap();
        assert_eq!(map.commands.len(), 1);
        assert_eq!(map.commands[0].name, "PADCMD_JUMP");
        assert_eq!(map.commands[0].methods.len(), 1);
        assert_eq!(map.commands[0].methods[0].inputs.len(), 1);
        let c = &map.commands[0].methods[0].inputs[0].constraints[0];
        assert!(matches!(c, Constraint::ButtonPress(n) if n == "Rdown"));
    }

    #[test]
    fn parse_multi_method_with_woo() {
        let src = r#"
COMMAND ACK_RIGHT
{
    METHOD
    {
        INPUT
        {
            BUTTON_HOLD Rup
            ANALOG_DEBOUNCE AnalogCharacterRight 0.75 1
        }
    }
    METHOD
    {
        INPUT { ANALOG_DEBOUNCE AnalogCharacterRight 0.75 1 }
        INPUT { BUTTON_PRESS Rup
                WINDOW_OF_OPPORTUNITY .2 }
    }
}
"#;
        let map = parse(src).unwrap();
        let cmd = &map.commands[0];
        assert_eq!(cmd.methods.len(), 2);
        // Second method has two input blocks
        assert_eq!(cmd.methods[1].inputs.len(), 2);
        // Second block of second method has WOO
        let woo = cmd.methods[1].inputs[1].window_of_opportunity;
        assert!((woo.unwrap() - 0.2).abs() < 1e-4);
    }

    #[test]
    fn parse_exclude_input() {
        let src = r#"
COMMAND PADCMD_ATTACK_HIGH
{
    METHOD
    {
        INPUT { BUTTON_PRESS Rup }
        EXCLUDE_INPUT
        {
            ANALOG_RANGE AnalogLmag .3 1
            WINDOW_OF_OPPORTUNITY 0.01
        }
    }
}
"#;
        let map = parse(src).unwrap();
        assert!(map.commands[0].methods[0].exclude.is_some());
    }
}
