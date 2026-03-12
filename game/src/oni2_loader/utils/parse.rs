use bevy::prelude::Vec3;

/// Extract the base="..." attribute from an <actor> tag.
pub fn extract_xml_base_attr(content: &str) -> Option<String> {
    let idx = content.find("<actor ")?;
    let after = &content[idx..];
    let end = after.find('>')?;
    let tag = &after[..end];
    let base_start = tag.find("base=\"")? + 6;
    let base_end = tag[base_start..].find('"')? + base_start;
    Some(tag[base_start..base_end].to_string())
}

/// Extract value="..." from an XML attribute tag like <TagName value="..."/>
/// Returns the last non-empty value found (most derived). If only empty values are found, returns None.
pub fn extract_xml_attr(content: &str, tag: &str) -> Option<String> {
    let pattern = format!("<{}", tag);
    let mut last_valid = None;
    let mut current = content;

    while let Some(idx) = current.find(&pattern) {
        let after = &current[idx..];
        if let Some(val_start_offset) = after.find("value=\"") {
            let val_start = val_start_offset + 7;
            if let Some(val_end_offset) = after[val_start..].find('"') {
                let val_end = val_start + val_end_offset;
                let val = &after[val_start..val_end];
                if !val.is_empty() {
                    last_valid = Some(val.to_string());
                }
            }
        }
        current = &current[idx + pattern.len()..];
    }
    
    last_valid
}

/// Parse "x y z" string into Vec3.
pub fn parse_vec3(s: &str) -> Option<Vec3> {
    let parts: Vec<f32> = s.split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() >= 3 {
        Some(Vec3::new(parts[0], parts[1], parts[2]))
    } else {
        None
    }
}

/// Extract the entire content block enclosed by <tag ...> and </tag>.
pub fn extract_xml_block(content: &str, tag: &str) -> Option<String> {
    let open_pattern = format!("<{}", tag);
    let start_idx = content.find(&open_pattern)?;
    let close_pattern = format!("</{}>", tag);
    let end_idx = content[start_idx..].find(&close_pattern)? + start_idx + close_pattern.len();
    Some(content[start_idx..end_idx].to_string())
}
