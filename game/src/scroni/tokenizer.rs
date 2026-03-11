use std::collections::HashMap;

use super::token::{Token, TokenCode, keyword_table};

/// ScrOni tokenizer. Converts source text into a stream of tokens.
/// Comments start with # (line) or ## ... ## (block).
pub struct Tokenizer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    keywords: HashMap<String, TokenCode>,
}

impl Tokenizer {
    pub fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            keywords: keyword_table(),
        }
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            let is_eof = tok.code == TokenCode::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        tokens
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Skip whitespace
            while let Some(ch) = self.peek() {
                if ch.is_ascii_whitespace() {
                    self.advance();
                } else {
                    break;
                }
            }

            // Check for comments
            if self.peek() == Some('#') {
                if self.peek_at(1) == Some('#') {
                    // Block comment ## ... ##
                    self.advance(); // skip first #
                    self.advance(); // skip second #
                    loop {
                        match self.advance() {
                            Some('#') if self.peek() == Some('#') => {
                                self.advance();
                                break;
                            }
                            None => break,
                            _ => {}
                        }
                    }
                } else {
                    // Line comment # ...
                    while let Some(ch) = self.advance() {
                        if ch == '\n' {
                            break;
                        }
                    }
                }
                continue; // re-check for more whitespace/comments
            }

            break;
        }
    }

    fn next_token(&mut self) -> Token {
        self.skip_whitespace_and_comments();

        let line = self.line;
        let col = self.col;

        let Some(ch) = self.peek() else {
            return Token::eof(line, col);
        };

        // String literal
        if ch == '"' {
            return self.read_string(line, col);
        }

        // Number (digit or leading dot followed by digit)
        if ch.is_ascii_digit() || (ch == '.' && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()))
        {
            return self.read_number(line, col);
        }

        // Negative number: minus followed by digit or dot-digit
        if ch == '-' {
            let next = self.peek_at(1);
            if next.is_some_and(|c| c.is_ascii_digit())
                || (next == Some('.')
                    && self.peek_at(2).is_some_and(|c| c.is_ascii_digit()))
            {
                return self.read_number(line, col);
            }
        }

        // Word (keyword or identifier)
        if ch.is_ascii_alphabetic() || ch == '_' {
            return self.read_word(line, col);
        }

        // Operators and special characters
        self.advance();
        match ch {
            '+' => Token::new(TokenCode::Plus, line, col, "+".into()),
            '-' => Token::new(TokenCode::Minus, line, col, "-".into()),
            '%' => Token::new(TokenCode::Percent, line, col, "%".into()),
            '*' => Token::new(TokenCode::Star, line, col, "*".into()),
            '/' => Token::new(TokenCode::Slash, line, col, "/".into()),
            '(' => Token::new(TokenCode::LeftParen, line, col, "(".into()),
            ')' => Token::new(TokenCode::RightParen, line, col, ")".into()),
            '{' => Token::new(TokenCode::LeftCurlyBracket, line, col, "{".into()),
            '}' => Token::new(TokenCode::RightCurlyBracket, line, col, "}".into()),
            ',' => Token::new(TokenCode::Comma, line, col, ",".into()),
            '.' => Token::new(TokenCode::Period, line, col, ".".into()),
            ':' => Token::new(TokenCode::Colon, line, col, ":".into()),
            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenCode::Equal, line, col, "==".into())
                } else {
                    Token::new(TokenCode::Equal, line, col, "=".into())
                }
            }
            '!' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenCode::NotEqual, line, col, "!=".into())
                } else {
                    Token::new(TokenCode::Error, line, col, "!".into())
                }
            }
            '>' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenCode::GreaterOrEqual, line, col, ">=".into())
                } else {
                    Token::new(TokenCode::Greater, line, col, ">".into())
                }
            }
            '<' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::new(TokenCode::LessOrEqual, line, col, "<=".into())
                } else if self.peek() == Some('>') {
                    self.advance();
                    Token::new(TokenCode::NotEqual, line, col, "<>".into())
                } else {
                    Token::new(TokenCode::Less, line, col, "<".into())
                }
            }
            _ => Token::new(TokenCode::Error, line, col, ch.to_string()),
        }
    }

    fn read_string(&mut self, line: usize, col: usize) -> Token {
        self.advance(); // skip opening quote
        let mut s = String::new();
        loop {
            match self.advance() {
                Some('"') => break,
                Some(ch) => s.push(ch),
                None => break,
            }
        }
        let mut tok = Token::new(TokenCode::StringConstant, line, col, s);
        tok.int_value = 0;
        tok.float_value = 0.0;
        tok
    }

    fn read_number(&mut self, line: usize, col: usize) -> Token {
        let start = self.pos;
        let mut is_float = false;

        // Optional minus
        if self.peek() == Some('-') {
            self.advance();
        }

        // Integer part
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.advance();
        }

        // Decimal part
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.advance(); // skip dot
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.advance();
            }
        }

        let text: String = self.chars[start..self.pos].iter().collect();

        if is_float {
            let val = text.parse::<f32>().unwrap_or(0.0);
            let mut tok = Token::new(TokenCode::FloatConstant, line, col, text);
            tok.float_value = val;
            tok.int_value = val as i32;
            tok
        } else {
            let val = text.parse::<i32>().unwrap_or(0);
            let mut tok = Token::new(TokenCode::IntegerConstant, line, col, text);
            tok.int_value = val;
            tok.float_value = val as f32;
            tok
        }
    }

    fn read_word(&mut self, line: usize, col: usize) -> Token {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            self.advance();
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        let lower = text.to_lowercase();

        let code = self.keywords.get(&lower).copied().unwrap_or(TokenCode::Identifier);
        Token::new(code, line, col, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_road_script() {
        let src = r#"
Script Road
begin
    do forever
    begin
    GotoCurvePhase 1.0 in 5
    SetCurvePhase 0
    end
end
"#;
        let mut tok = Tokenizer::new(src);
        let tokens = tok.tokenize();
        let codes: Vec<_> = tokens.iter().map(|t| t.code).collect();
        assert_eq!(codes[0], TokenCode::Script);
        assert_eq!(codes[1], TokenCode::Identifier); // "Road"
        assert_eq!(codes[2], TokenCode::Begin);
        assert_eq!(codes[3], TokenCode::Do);
        assert_eq!(codes[4], TokenCode::Forever);
        assert_eq!(codes[5], TokenCode::Begin);
        assert_eq!(codes[6], TokenCode::GotoCurvePhase);
        assert!(codes.contains(&TokenCode::Eof));
    }

    #[test]
    fn tokenize_comments() {
        let src = "# this is a comment\nSet myVar to 1\n## block ## Set other to 2";
        let mut tok = Tokenizer::new(src);
        let tokens = tok.tokenize();
        // After comment: Set, Identifier(myVar), To, 1
        assert_eq!(tokens[0].code, TokenCode::Set);
        assert_eq!(tokens[1].code, TokenCode::Identifier);
        assert_eq!(tokens[1].text, "myVar");
    }

    #[test]
    fn tokenize_string_and_float() {
        let src = r#"PlayAnimation "Rotate" hold rate 0.1"#;
        let mut tok = Tokenizer::new(src);
        let tokens = tok.tokenize();
        assert_eq!(tokens[0].code, TokenCode::PlayAnimation);
        assert_eq!(tokens[1].code, TokenCode::StringConstant);
        assert_eq!(tokens[1].text, "Rotate");
        assert_eq!(tokens[4].code, TokenCode::FloatConstant);
        assert!((tokens[4].float_value - 0.1).abs() < 0.001);
    }
}
