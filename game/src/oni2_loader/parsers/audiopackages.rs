/*
 * oni2_loader/parsers/audiopackages.rs — audio package manifest parser.
 *
 * Parses AudioNugget definitions (sound name, volume, pitch, delay, random
 * ranges).  AudioPackage groups nuggets by logical name and is loaded into
 * a registry used by the ScrOni sound opcodes and fx_system.
 */
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct AudioNugget {
    pub sound: String,
    pub volume: f32,
    pub pitch: f32,
    pub delay: f32,
    pub random_min_volume: f32,
    pub random_max_volume: f32,
    pub random_min_pitch: f32,
    pub random_max_pitch: f32,
    pub random_min_delay: f32,
    pub random_max_delay: f32,
}

#[derive(Debug, Clone)]
pub struct AudioPackage {
    pub nuggets: Vec<AudioNugget>,
}

pub type AudioPackagesDirectory = HashMap<String, AudioPackage>;

pub fn parse_audiopackages(content: &str) -> AudioPackagesDirectory {
    let mut dir = HashMap::new();
    let mut current_pkg_name = None;
    let mut current_pkg_nuggets = Vec::new();
    let mut current_nugget: Option<AudioNugget> = None;

    for line_raw in content.lines() {
        let line = line_raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }

        if line.starts_with("PACKAGE") {
            if let Some(name) = current_pkg_name.take() {
                if let Some(n) = current_nugget.take() {
                    current_pkg_nuggets.push(n);
                }
                dir.insert(
                    name,
                    AudioPackage {
                        nuggets: std::mem::take(&mut current_pkg_nuggets),
                    },
                );
            }
            let name_str = line
                .strip_prefix("PACKAGE")
                .unwrap()
                .trim()
                .trim_matches('"');
            current_pkg_name = Some(name_str.to_string());
        } else if line == "NUGGET" {
            if let Some(n) = current_nugget.take() {
                current_pkg_nuggets.push(n);
            }
            current_nugget = Some(AudioNugget {
                sound: String::new(),
                volume: 1.0,
                pitch: 1.0,
                delay: 0.0,
                random_min_volume: 1.0,
                random_max_volume: 1.0,
                random_min_pitch: 1.0,
                random_max_pitch: 1.0,
                random_min_delay: 0.0,
                random_max_delay: 0.0,
            });
        } else if line == "{" {
            continue;
        } else if line == "}" {
            if let Some(n) = current_nugget.take() {
                current_pkg_nuggets.push(n);
            }
        } else if let Some(nugget) = &mut current_nugget {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let key = parts[0].to_uppercase();
                let value_str = parts[1..].join(" ");
                let clean_val = value_str.trim_matches('"');
                match key.as_str() {
                    "SOUND" => nugget.sound = clean_val.to_string(),
                    "VOLUME" => {
                        if let Ok(v) = clean_val.parse() {
                            nugget.volume = v
                        }
                    }
                    "PITCH" => {
                        if let Ok(v) = clean_val.parse() {
                            nugget.pitch = v
                        }
                    }
                    "DELAY" => {
                        if let Ok(v) = clean_val.parse() {
                            nugget.delay = v
                        }
                    }
                    "RANDOM_MIN_VOLUME" => {
                        if let Ok(v) = clean_val.parse() {
                            nugget.random_min_volume = v
                        }
                    }
                    "RANDOM_MAX_VOLUME" => {
                        if let Ok(v) = clean_val.parse() {
                            nugget.random_max_volume = v
                        }
                    }
                    "RANDOM_MIN_PITCH" => {
                        if let Ok(v) = clean_val.parse() {
                            nugget.random_min_pitch = v
                        }
                    }
                    "RANDOM_MAX_PITCH" => {
                        if let Ok(v) = clean_val.parse() {
                            nugget.random_max_pitch = v
                        }
                    }
                    "RANDOM_MIN_DELAY" => {
                        if let Ok(v) = clean_val.parse() {
                            nugget.random_min_delay = v
                        }
                    }
                    "RANDOM_MAX_DELAY" => {
                        if let Ok(v) = clean_val.parse() {
                            nugget.random_max_delay = v
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if let Some(name) = current_pkg_name {
        if let Some(n) = current_nugget.take() {
            current_pkg_nuggets.push(n);
        }
        dir.insert(
            name,
            AudioPackage {
                nuggets: current_pkg_nuggets,
            },
        );
    }

    dir
}
