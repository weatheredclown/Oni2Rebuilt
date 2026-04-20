/*
 * oni2_loader/parsers/block_parser.rs — Robust text-block parser utility.
 */
use bevy::prelude::*;
use std::iter::Peekable;
use std::vec::IntoIter;

pub struct BlockParser {
    pub tokens: Peekable<IntoIter<String>>,
}

impl BlockParser {
    pub fn new(content: &str) -> Self {
        let mut toks = Vec::new();
        for line in content.lines() {
            let mut l = line.trim();
            if let Some(idx) = l.find('#') {
                l = l[..idx].trim();
            } else if let Some(idx) = l.find("//") {
                l = l[..idx].trim();
            }
            if !l.is_empty() {
                // Separate out braces so they parse correctly
                let replaced = l.replace("{", " { ").replace("}", " } ");
                toks.extend(replaced.split_whitespace().map(|s| s.to_string()));
            }
        }
        Self {
            tokens: toks.into_iter().peekable(),
        }
    }

    pub fn start(&mut self, block_name: &str) -> bool {
        // Find the block name and then optionally consume {
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

    // Starts an anonymous block by just looking for { directly next
    pub fn start_anonymous(&mut self) -> bool {
        if self.peek() == Some("{") {
            self.tokens.next();
            return true;
        }
        false
    }

    pub fn peek(&mut self) -> Option<&str> {
        self.tokens.peek().map(|s| s.as_str())
    }

    pub fn next(&mut self) -> Option<String> {
        self.tokens.next()
    }

    pub fn endblock(&mut self) -> bool {
        if self.peek() == Some("}") {
            self.tokens.next();
            true
        } else if self.peek().is_none() {
            true
        } else {
            false
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
        if self.consume_key(expected_key) {
            if let Some(v_str) = self.tokens.next() {
                return v_str.parse::<i32>().ok();
            }
        }
        None
    }

    pub fn read_i32(&mut self, expected_key: &str, default: i32) -> i32 {
        self.read_i32_opt(expected_key).unwrap_or(default)
    }

    pub fn read_float_opt(&mut self, expected_key: &str) -> Option<f32> {
        if self.consume_key(expected_key) {
            if let Some(v_str) = self.tokens.next() {
                return v_str.parse::<f32>().ok();
            }
        }
        None
    }

    pub fn read_float(&mut self, expected_key: &str, default: f32) -> f32 {
        self.read_float_opt(expected_key).unwrap_or(default)
    }

    pub fn read_string_opt(&mut self, expected_key: &str) -> Option<String> {
        if self.consume_key(expected_key) {
            if let Some(v_str) = self.tokens.next() {
                return Some(v_str.trim_matches('"').to_string());
            }
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
