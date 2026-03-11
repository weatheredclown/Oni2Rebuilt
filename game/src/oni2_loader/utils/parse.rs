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
pub fn extract_xml_attr(content: &str, tag: &str) -> Option<String> {
    let pattern = format!("<{}", tag);
    let idx = content.find(&pattern)?;
    let after = &content[idx..];
    let val_start = after.find("value=\"")? + 7;
    let val_end = after[val_start..].find('"')? + val_start;
    Some(after[val_start..val_end].to_string())
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
