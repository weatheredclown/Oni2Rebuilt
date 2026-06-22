/*
 * oni2_loader/parsers/rooms.rs — level room mesh list parser.
 *
 * Parses rooms.room to extract individual room names, geometry meshes (Room*.mesh),
 * left-to-right handed transformed matrices, and skyhat parameters.
 */
use bevy::math::{Mat3, Quat, Vec3};
use bevy::prelude::Transform;
use crate::oni2_loader::utils::space;#[derive(Debug, Clone)]
pub struct ParsedRoom {
    pub name: String,
    pub mesh_name: String,
    pub transform: Transform,
    pub render_sky_hat: bool,
    pub portals: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct ParsedRoomsFile {
    pub version: u32,
    pub rooms: Vec<ParsedRoom>,
}

pub fn parse_rooms_file(content: &str) -> Option<ParsedRoomsFile> {
    let mut version = 1;
    let mut rooms = Vec::new();

    let mut lines = content.lines().map(|l| l.trim()).peekable();

    while let Some(line) = lines.next() {
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }

        if line.starts_with("version:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                version = parts[1].parse().unwrap_or(1);
            }
            continue;
        }

        if line.starts_with("roomCount:") {
            continue;
        }

        if line.starts_with("room ") {
            let name = line.strip_prefix("room ").unwrap_or("").trim().to_string();

            // Look for opening brace `{`
            while let Some(&next_line) = lines.peek() {
                if next_line == "{" {
                    let _ = lines.next();
                    break;
                } else if next_line.starts_with('{') {
                    break;
                }
                let _ = lines.next();
            }

            let mut mesh_name = String::new();
            let mut row0 = [1.0f32, 0.0, 0.0];
            let mut row1 = [0.0f32, 1.0, 0.0];
            let mut row2 = [0.0f32, 0.0, 1.0];
            let mut row3 = [0.0f32, 0.0, 0.0];
            let mut render_sky_hat = false;
            let mut portals = Vec::new();

            // Read until closing brace `}`
            let mut depth = 1;
            while let Some(room_line) = lines.next() {
                if room_line == "{" {
                    depth += 1;
                    continue;
                }
                if room_line == "}" {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                if depth > 1 {
                    // Skip nested blocks (like portals)
                    continue;
                }

                let parts: Vec<&str> = room_line.split_whitespace().collect();
                if parts.is_empty() {
                    continue;
                }

                match parts[0] {
                    "mesh" => {
                        if parts.len() >= 2 {
                            mesh_name = parts[1].to_string();
                        }
                    }
                    "matrix" => {
                        let mut parsed_rows = 0;
                        while parsed_rows < 4 {
                            if let Some(matrix_line) = lines.next() {
                                let m_parts: Vec<&str> = matrix_line.split_whitespace().collect();
                                if m_parts.len() >= 3 {
                                    let x: f32 = m_parts[0].parse().unwrap_or(0.0);
                                    let y: f32 = m_parts[1].parse().unwrap_or(0.0);
                                    let z: f32 = m_parts[2].parse().unwrap_or(0.0);
                                    match parsed_rows {
                                        0 => row0 = [x, y, z],
                                        1 => row1 = [x, y, z],
                                        2 => row2 = [x, y, z],
                                        3 => row3 = [x, y, z],
                                        _ => {}
                                    }
                                    parsed_rows += 1;
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    "RenderSkyHat" => {
                        if parts.len() >= 2 {
                            let val: i32 = parts[1].parse().unwrap_or(0);
                            render_sky_hat = val != 0;
                        }
                    }
                    "portals" => {
                        let mut portals_depth = 0;
                        while let Some(portal_line) = lines.next() {
                            let trimmed = portal_line.trim();
                            if trimmed == "{" {
                                portals_depth += 1;
                                continue;
                            }
                            if trimmed == "}" {
                                portals_depth -= 1;
                                if portals_depth == 0 {
                                    break;
                                }
                                continue;
                            }
                            let sub_parts: Vec<&str> = trimmed.split_whitespace().collect();
                            if sub_parts.len() >= 2 && sub_parts[0] == "room" {
                                if let Ok(room_idx) = sub_parts[1].parse::<usize>() {
                                    portals.push(room_idx);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Construct transform in Bevy space
            let m = Mat3::from_cols(
                Vec3::new(row0[0], row1[0], row2[0]),
                Vec3::new(row0[1], row1[1], row2[1]),
                Vec3::new(row0[2], row1[2], row2[2]),
            );
            let s = Mat3::from_diagonal(Vec3::new(-1.0, 1.0, -1.0));
            let m_bevy = s * m * s;
            let translation_bevy = space::to_bevy_space_pos(Vec3::new(row3[0], row3[1], row3[2]));
            let rotation_bevy = Quat::from_mat3(&m_bevy);
            let transform = Transform {
                translation: translation_bevy,
                rotation: rotation_bevy,
                scale: Vec3::ONE,
            };

            rooms.push(ParsedRoom {
                name,
                mesh_name,
                transform,
                render_sky_hat,
                portals,
            });
        }
    }

    Some(ParsedRoomsFile {
        version,
        rooms,
    })
}

#[cfg(test)]
mod parser_rooms_tests {
    use super::*;

    #[test]
    fn test_parse_rooms_file() {
        let content = "version: 1\n\
        roomCount: 2\n\
        \n\
        room Start\n\
        {\n\
        \tmesh Start\n\
        \n\
        \tmatrix\n\
        \t1.000000\t0.000000\t0.000000\n\
        \t0.000000\t1.000000\t0.000000\n\
        \t0.000000\t0.000000\t1.000000\n\
        \t10.000000\t20.000000\t30.000000\n\
        \n\
        \tRenderSkyHat\t 1\n\
        \n\
        \tportals 1\n\
        \t{\n\
        \t\tportal\n\
        \t\t{\n\
        \t\t\troom 2\n\
        \t\t}\n\
        \t}\n\
        }\n";

        let parsed = parse_rooms_file(content).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.rooms.len(), 1);
        let room = &parsed.rooms[0];
        assert_eq!(room.name, "Start");
        assert_eq!(room.mesh_name, "Start");
        assert_eq!(room.render_sky_hat, true);
        assert_eq!(room.portals, vec![2]);
        
        // translation converted to Bevy space (-X, Y, -Z)
        assert_eq!(room.transform.translation, Vec3::new(-10.0, 20.0, -30.0));
    }
}
