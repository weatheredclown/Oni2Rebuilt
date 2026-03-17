use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum SettingsValue {
    String(String),
    Float(f32),
    Int(i32),
    FloatArray(Vec<f32>),
    Block(SettingsBlock),
}

#[derive(Debug, Clone, Default)]
pub struct SettingsBlock {
    pub properties: HashMap<String, SettingsValue>,
    pub children: Vec<SettingsBlock>,
}

#[derive(Debug, Clone)]
pub struct SettingsDefinition {
    pub def_type: String,
    pub name: String,
    pub block: SettingsBlock,
}

/// Parses a file containing multiple definitions like `TYPE "Name" { ... }`
pub fn parse_settings_file(content: &str) -> Vec<SettingsDefinition> {
    let mut tokens = tokenize_settings(content);
    let mut defs = Vec::new();
    
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "{" || tokens[i] == "}" {
            i += 1;
            continue;
        }
        
        // Expect: TYPE "Name" {
        if i + 2 < tokens.len() && tokens[i+2] == "{" {
            let def_type = tokens[i].clone();
            let name = tokens[i+1].trim_matches('"').to_string();
            i += 3; // skip TYPE, "Name", and {
            
            let (block, next_i) = parse_block(&tokens, i);
            defs.push(SettingsDefinition {
                def_type,
                name,
                block,
            });
            i = next_i;
        } else {
            i += 1;
        }
    }
    
    defs
}

fn parse_block(tokens: &[String], start_idx: usize) -> (SettingsBlock, usize) {
    let mut block = SettingsBlock::default();
    let mut i = start_idx;
    
    while i < tokens.len() {
        let token = &tokens[i];
        if token == "}" {
            return (block, i + 1);
        }
        
        // Property Name
        let key = token.clone();
        i += 1;
        
        // Is it a nested block? e.g. Damage { ... }
        if i < tokens.len() && tokens[i] == "{" {
            i += 1;
            let (child_block, next_i) = parse_block(tokens, i);
            block.properties.insert(key, SettingsValue::Block(child_block));
            i = next_i;
            continue;
        }
        
        // Otherwise, gather values until the next newline or recognizable key
        // In our simplistic tokenizer, we just grab until the next token that looks like a key
        // but we'll cheat by just grabbing the rest of the line (tokenizer keeps lines separate)
        let mut _vals: Vec<String> = Vec::new();
        while i < tokens.len() && tokens[i] != "}" && tokens[i] != "{" {
            // Very naive line-break detection isn't in this token stream, 
            // so we just grab strings/numbers.
            // Actually, we'll need a slightly smarter tokenizer that groups by line,
            // or we just assume all subsequent tokens on this line are part of the value.
            // Let's refine the tokenizer to yield Line(Vec<Token>)
            unreachable!("Use parse_settings_file_lines instead");
        }
    }
    
    (block, i)
}

/// Tokenizes preserving lines to handle key-value pairs without semicolons.
fn tokenize_lines(content: &str) -> Vec<Vec<String>> {
    let mut lines = Vec::new();
    for line in content.lines() {
        // Strip comments
        let line = if let Some(idx) = line.find("//") { &line[..idx] } else { line };
        let line = if let Some(idx) = line.find('#') { &line[..idx] } else { line };
        
        let mut tokens = Vec::new();
        let mut current_token = String::new();
        let mut in_quotes = false;
        
        for c in line.chars() {
            if c == '"' {
                in_quotes = !in_quotes;
                current_token.push(c);
            } else if in_quotes {
                current_token.push(c);
            } else if c.is_whitespace() {
                if !current_token.is_empty() {
                    tokens.push(current_token.clone());
                    current_token.clear();
                }
            } else if c == '{' || c == '}' {
                if !current_token.is_empty() {
                    tokens.push(current_token.clone());
                    current_token.clear();
                }
                tokens.push(c.to_string());
            } else {
                current_token.push(c);
            }
        }
        if !current_token.is_empty() {
            tokens.push(current_token);
        }
        
        if !tokens.is_empty() {
            lines.push(tokens);
        }
    }
    lines
}

pub fn parse_settings(content: &str) -> Vec<SettingsDefinition> {
    let lines = tokenize_lines(content);
    let mut defs = Vec::new();
    
    let mut iter = lines.into_iter().peekable();
    
    while let Some(line) = iter.next() {
        if line.len() == 2 || (line.len() == 3 && line[2] == "{") {
            let def_type = line[0].clone();
            let name = line[1].trim_matches('"').to_string();
            
            // Consume opening brace if it's on the next line
            if line.len() == 2 {
                if let Some(next) = iter.peek() {
                    if next.len() == 1 && next[0] == "{" {
                        iter.next();
                    }
                }
            }
            
            let block = parse_block_lines(&mut iter);
            defs.push(SettingsDefinition { def_type, name, block });
        }
    }
    
    defs
}

fn parse_block_lines(iter: &mut std::iter::Peekable<std::vec::IntoIter<Vec<String>>>) -> SettingsBlock {
    let mut block = SettingsBlock::default();
    
    while let Some(line) = iter.next() {
        if line.len() == 1 && line[0] == "}" {
            break;
        }
        if line.len() == 1 && line[0] == "{" {
            // Anonymous block (e.g. nested array of blocks)
            let child = parse_block_lines(iter);
            block.children.push(child);
            continue;
        }
        
        let key = line[0].clone();
        
        if line.len() == 1 {
            // Lookahead for opening brace
            if let Some(next) = iter.peek() {
                if next.len() == 1 && next[0] == "{" {
                    iter.next(); // consume '{'
                    let child = parse_block_lines(iter);
                    block.properties.insert(key, SettingsValue::Block(child));
                    continue;
                }
            }
        }
        
        // Values are the rest of the line
        if line.len() > 1 {
            let vals = &line[1..];
            if vals.len() == 1 {
                let v = &vals[0];
                if v.starts_with('"') && v.ends_with('"') {
                    block.properties.insert(key, SettingsValue::String(v.trim_matches('"').to_string()));
                } else if let Ok(i) = v.parse::<i32>() {
                    block.properties.insert(key, SettingsValue::Int(i));
                } else if let Ok(f) = v.parse::<f32>() {
                    block.properties.insert(key, SettingsValue::Float(f));
                } else {
                    block.properties.insert(key, SettingsValue::String(v.clone()));
                }
            } else {
                // Array of floats
                let mut floats = Vec::new();
                for v in vals {
                    if let Ok(f) = v.parse::<f32>() {
                        floats.push(f);
                    }
                }
                block.properties.insert(key, SettingsValue::FloatArray(floats));
            }
        }
    }
    
    block
}

fn tokenize_settings(_content: &str) -> Vec<String> { vec![] }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rb_proj_and_fx() {
        let proj_content = std::fs::read_to_string("../oni2/zips/assets/Settings/rb.proj").unwrap();
        let proj_defs = parse_settings(&proj_content);
        println!("Parsed {} projectiles.", proj_defs.len());
        if let Some(def) = proj_defs.first() {
            println!("First proj: {} '{}'", def.def_type, def.name);
            println!("{:#?}", def.block);
        }

        let fx_content = std::fs::read_to_string("../oni2/zips/assets/Settings/rb.fx").unwrap();
        let fx_defs = parse_settings(&fx_content);
        println!("Parsed {} fx definitions.", fx_defs.len());
        if let Some(def) = fx_defs.first() {
            println!("First fx: {} '{}'", def.def_type, def.name);
            println!("{:#?}", def.block);
        }
    }
}
