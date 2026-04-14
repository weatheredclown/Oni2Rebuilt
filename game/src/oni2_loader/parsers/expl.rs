use crate::oni2_loader::parsers::types::*;

pub fn parse_expl(content: &str) -> Vec<BasicExplosionDef> {
    let mut results = Vec::new();
    
    // We do a fast manual block extractor
    let mut chars = content.chars().enumerate().peekable();
    
    while let Some((_, c)) = chars.next() {
        if c == 'B' || c == 'b' {
            let offset = chars.clone().take(13).map(|(_, c)| c).collect::<String>();
            if offset.eq_ignore_ascii_case("ASICEXPLOSION") {
                // Skim chars to advance
                for _ in 0..13 { chars.next(); }
                
                // Read name
                let mut name = String::new();
                let mut in_quotes = false;
                while let Some((_, qc)) = chars.next() {
                    if qc == '"' {
                        if in_quotes { break; } else { in_quotes = true; }
                    } else if in_quotes {
                        name.push(qc);
                    } else if qc == '{' || qc == '\n' {
                        break;
                    }
                }
                
                // Read into block '{'
                while let Some((_, bc)) = chars.peek() {
                    if *bc == '{' { break; }
                    chars.next();
                }
                
                let mut brace_depth = 0;
                let mut block_content = String::new();
                while let Some((_, bc)) = chars.next() {
                    if bc == '{' { brace_depth += 1; }
                    if brace_depth > 0 { block_content.push(bc); }
                    if bc == '}' {
                        brace_depth -= 1;
                        if brace_depth == 0 { break; }
                    }
                }
                
                let parsed_def = parse_single_explosion(name, &block_content);
                results.push(parsed_def);
            }
        }
    }
    
    results
}

fn parse_single_explosion(name: String, block: &str) -> BasicExplosionDef {
    let mut fx_list = Vec::new();
    let mut ellipsoid = None;
    let mut mbox = None;
    
    let mut lines = block.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        
        // Split by whitespace
        let mut tokens = trimmed.split_whitespace();
        if let Some(kw) = tokens.next() {
            if kw.eq_ignore_ascii_case("ExplodeFX") {
                let inner = extract_inner_block(&mut lines);
                let mut fx = ExplodeFXDef { fx_type: String::new(), offset: [0.0;3], delay: 0.0 };
                for iline in inner.lines() {
                    let mut it = iline.split_whitespace();
                    if let Some(ikw) = it.next() {
                        if ikw.eq_ignore_ascii_case("FXType") {
                            fx.fx_type = it.collect::<Vec<_>>().join(" ").trim_matches('"').to_string();
                        } else if ikw.eq_ignore_ascii_case("Offset") {
                            fx.offset[0] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            fx.offset[1] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            fx.offset[2] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("Delay") {
                            fx.delay = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        }
                    }
                }
                fx_list.push(fx);
            } else if kw.eq_ignore_ascii_case("ELLIPSOID") {
                let inner = extract_inner_block(&mut lines);
                let mut ell = EllipsoidDamageDef {
                    offset: [0.0; 3], max_radii: [1.0; 3], orientation: [0.0; 3],
                    start_radius_percentage: 0.0, blast_duration: 0.0, max_damage: 0.0,
                    max_damage_radius_percentage: 0.0, continuous_damage: false,
                };
                for iline in inner.lines() {
                    let mut it = iline.split_whitespace();
                    if let Some(ikw) = it.next() {
                        if ikw.eq_ignore_ascii_case("Offset") {
                            ell.offset[0] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            ell.offset[1] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            ell.offset[2] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("MaxRadii") {
                            ell.max_radii[0] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            ell.max_radii[1] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            ell.max_radii[2] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("Orientation") {
                            ell.orientation[0] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            ell.orientation[1] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            ell.orientation[2] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("StartRadiusPercentage") {
                            ell.start_radius_percentage = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("BlastDuration") {
                            ell.blast_duration = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("MaxDamage") {
                            ell.max_damage = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("MaxDamageRadiusPercentage") {
                            ell.max_damage_radius_percentage = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("ContinuousDamage") {
                            ell.continuous_damage = it.next().unwrap_or("0").parse::<i32>().unwrap_or(0) != 0;
                        }
                    }
                }
                ellipsoid = Some(ell);
            } else if kw.eq_ignore_ascii_case("BOX") {
                let inner = extract_inner_block(&mut lines);
                let mut b = BoxDamageDef {
                    offset: [0.0; 3], orientation: [0.0; 3], blast_duration: 0.0,
                    continuous_damage: false, start_damage: 0.0, end_damage: 0.0,
                    start_dimensions: [1.0; 3], end_dimensions: [1.0; 3], end_translation: [0.0; 3],
                };
                for iline in inner.lines() {
                    let mut it = iline.split_whitespace();
                    if let Some(ikw) = it.next() {
                        if ikw.eq_ignore_ascii_case("Offset") {
                            b.offset[0] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            b.offset[1] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            b.offset[2] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("Orientation") {
                            b.orientation[0] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            b.orientation[1] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            b.orientation[2] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("BlastDuration") {
                            b.blast_duration = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("ContinuousDamage") {
                            b.continuous_damage = it.next().unwrap_or("0").parse::<i32>().unwrap_or(0) != 0;
                        } else if ikw.eq_ignore_ascii_case("StartDamage") {
                            b.start_damage = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("EndDamage") {
                            b.end_damage = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("StartDimensions") {
                            b.start_dimensions[0] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            b.start_dimensions[1] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            b.start_dimensions[2] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("EndDimensions") {
                            b.end_dimensions[0] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            b.end_dimensions[1] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            b.end_dimensions[2] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        } else if ikw.eq_ignore_ascii_case("EndTranslation") {
                            b.end_translation[0] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            b.end_translation[1] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                            b.end_translation[2] = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                        }
                    }
                }
                mbox = Some(b);
            }
        }
    }
    
    BasicExplosionDef {
        name,
        fx: fx_list,
        ellipsoid,
        r#box: mbox,
    }
}

// Scans until matching brace closes.
fn extract_inner_block<'a>(lines: &mut std::iter::Peekable<std::str::Lines<'a>>) -> String {
    let mut depth = 0;
    let mut started = false;
    let mut out = String::new();
    
    while let Some(l) = lines.next() {
        for c in l.chars() {
            if c == '{' {
                depth += 1;
                started = true;
            }
            if c == '}' {
                depth -= 1;
            }
        }
        
        if started {
            out.push_str(l);
            out.push('\n');
            if depth == 0 { break; }
        } else {
            // It might be like `ExplodeFX \n {`
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}
