/*
 * oni2_loader/parsers/bsp.rs — BSP tree structure parser for level layouts.
 *
 * Parses rooms.bsp into a tree of planes and child indices/room names, used for
 * camera and actor room lookup.
 */
use bevy::math::{Vec3, Vec4};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BspChild {
    Node(usize),
    Room(String),
}

#[derive(Debug, Clone)]
pub struct BspNode {
    pub plane: Vec4,
    pub negative: BspChild,
    pub positive: BspChild,
}

#[derive(Debug, Clone)]
pub struct ParsedBspTree {
    pub nodes: Vec<BspNode>,
}

impl ParsedBspTree {
    pub fn get_room_name_from_point(&self, point: Vec3) -> Option<&str> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut current_idx = 0;
        loop {
            let node = &self.nodes[current_idx];
            let normal = Vec3::new(node.plane.x, node.plane.y, node.plane.z);
            let dist = normal.dot(point) + node.plane.w;

            let child = if dist > 0.0 {
                &node.positive
            } else {
                &node.negative
            };

            match child {
                BspChild::Node(next) => {
                    if *next >= self.nodes.len() {
                        return None;
                    }
                    current_idx = *next;
                }
                BspChild::Room(name) => {
                    return Some(name.as_str());
                }
            }
        }
    }
}

pub fn parse_bsp_file(content: &str) -> Option<ParsedBspTree> {
    let mut tokens = content.split_whitespace().peekable();

    let count_str = tokens.next()?;
    let count: usize = count_str.parse().ok()?;

    let mut nodes = Vec::with_capacity(count);

    for _ in 0..count {
        let x: f32 = tokens.next()?.parse().ok()?;
        let y: f32 = tokens.next()?.parse().ok()?;
        let z: f32 = tokens.next()?.parse().ok()?;
        let w: f32 = tokens.next()?.parse().ok()?;

        // Convert LH normal (negate X and Z) to Bevy RH space normal, keep Y and offset W
        let plane = Vec4::new(-x, y, -z, w);

        let neg_type = tokens.next()?;
        let neg_child = if neg_type == "room" {
            BspChild::Room(tokens.next()?.to_string())
        } else if neg_type == "node" {
            BspChild::Node(tokens.next()?.parse().ok()?)
        } else {
            return None;
        };

        let pos_type = tokens.next()?;
        let pos_child = if pos_type == "room" {
            BspChild::Room(tokens.next()?.to_string())
        } else if pos_type == "node" {
            BspChild::Node(tokens.next()?.parse().ok()?)
        } else {
            return None;
        };

        nodes.push(BspNode {
            plane,
            negative: neg_child,
            positive: pos_child,
        });
    }

    Some(ParsedBspTree { nodes })
}

#[cfg(test)]
mod bsp_parser_tests {
    use super::*;

    #[test]
    fn test_parse_bsp() {
        let content = "2\n\
        1.0 0.0 0.0 -10.0\n\
        room Start\n\
        node 1\n\
        0.0 1.0 0.0 5.0\n\
        room Invalid\n\
        room Room1\n";

        let parsed = parse_bsp_file(content).unwrap();
        assert_eq!(parsed.nodes.len(), 2);

        assert_eq!(parsed.nodes[0].plane, Vec4::new(-1.0, 0.0, -0.0, -10.0));
        assert_eq!(
            parsed.nodes[0].negative,
            BspChild::Room("Start".to_string())
        );
        assert_eq!(parsed.nodes[0].positive, BspChild::Node(1));

        // Query P_bevy = (-15.0, -10.0, 0.0) -> distance = -10.0 + 5.0 = -5.0 < 0 -> negative -> room Invalid
        // Query P_bevy = (-15.0, 10.0, 0.0) -> distance = 10.0 + 5.0 = 15.0 > 0 -> positive -> room Room1
        assert_eq!(
            parsed.get_room_name_from_point(Vec3::new(-15.0, -10.0, 0.0)),
            Some("Invalid")
        );
        assert_eq!(
            parsed.get_room_name_from_point(Vec3::new(-15.0, 10.0, 0.0)),
            Some("Room1")
        );
        assert_eq!(
            parsed.get_room_name_from_point(Vec3::new(5.0, 0.0, 0.0)),
            Some("Start")
        );
    }
}
