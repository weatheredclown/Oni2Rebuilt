fn extract_xml_attr(content: &str, tag: &str) -> Option<String> {
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

fn main() {
    let xml = r#"
<?xml version="1.0" encoding="UTF-8"?>
<!-- edited by AUTOMATED_UNIT_0 -->
<!DOCTYPE actor SYSTEM "actor.dtd">
<actor name="actor_BCStart" updatestate="Asleep" spawnlater="0" base="template_level_chunk">
	<contents>
		<Prop>
			<attributes>
				<Position value="-0.03611786102 23.22070946 13.11189154"/>
				<Orientation value="0 180 0"/>
			</attributes>
		</Prop>
		<Entity>
			<attributes>
				<EntityType value="BCStart"/>
			</attributes>
		</Entity>
		<ScrOni>
			<attributes>
				<Filename value="$cs1_intro"/>
			</attributes>
		</ScrOni>
	</contents>
</actor>
    "#;
    
    println!("Position: {:?}", extract_xml_attr(xml, "Position"));
}
