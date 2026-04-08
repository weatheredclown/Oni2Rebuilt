use bevy::prelude::*;
use crate::vfs;

pub struct GraphPoint {
    pub position: Vec3,
    pub flags: u32,
    pub name: String,
}

pub struct GraphEdge {
    pub a: usize,
    pub b: usize,
    pub cost: f32,
    pub flags: u32,
    pub subgraph: u32,
}

pub struct LayoutGraph {
    pub name: String,
    pub points: Vec<GraphPoint>,
    pub edges: Vec<GraphEdge>,
}

pub fn parse_layout_graphs(dir: &str) -> Vec<LayoutGraph> {
    let content = match vfs::read_to_string(dir, "layout.graphs") {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    
    let mut names = Vec::new();
    let mut lines = content.lines().filter_map(|l| {
        let s = l.trim();
        if s.is_empty() { None } else { Some(s) }
    }).skip(1); // skip the count
    
    for line in lines {
        names.push(line.to_string());
    }
    
    let mut parsed = Vec::new();
    for name in names {
        if let Some(graph) = parse_single_graph(dir, &name) {
            parsed.push(graph);
        }
    }
    
    parsed
}

fn parse_single_graph(dir: &str, name: &str) -> Option<LayoutGraph> {
    let content = vfs::read_to_string(dir, &format!("{}.graph", name)).ok()?;
    
    let mut points = Vec::new();
    let mut edges = Vec::new();
    
    let mut parsing_points = false;
    let mut parsing_edges = false;
    
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "{" || trimmed == "}" {
            continue;
        }
        
        if trimmed.starts_with("PreCalcData") {
            break;
        }
        
        if trimmed.starts_with("NumPoints") {
            parsing_points = true;
            parsing_edges = false;
            continue;
        }
        if trimmed.starts_with("NumEdges") {
            parsing_points = false;
            parsing_edges = true;
            continue;
        }
        
        if parsing_points && trimmed.starts_with('{') {
            let inner = trimmed.trim_start_matches('{').trim_end_matches('}').trim();
            let parts: Vec<&str> = inner.split_whitespace().collect();
            if parts.len() >= 5 {
                let x = parts[0].parse().unwrap_or(0.0);
                let y = parts[1].parse().unwrap_or(0.0);
                let z = parts[2].parse().unwrap_or(0.0);
                let flags = parts[4].parse().unwrap_or(0);
                
                let mut pt_name = String::new();
                if parts.len() >= 6 {
                    pt_name = parts[5..].join(" ").trim_matches('"').to_string();
                }
                
                points.push(GraphPoint {
                    position: Vec3::new(-x, y, -z), // Left-Handed to Right-Handed
                    flags,
                    name: pt_name,
                });
            }
        }
        
        if parsing_edges && trimmed.starts_with('{') {
            let inner = trimmed.trim_start_matches('{').trim_end_matches('}').trim();
            let parts: Vec<&str> = inner.split_whitespace().collect();
            if parts.len() >= 5 {
                let a = parts[0].parse().unwrap_or(0);
                let b = parts[1].parse().unwrap_or(0);
                let cost = parts[2].parse().unwrap_or(0.0);
                let flags = parts[4].parse().unwrap_or(0);
                let subgraph = if parts.len() >= 6 { parts[5].parse().unwrap_or(0) } else { 0 };
                
                edges.push(GraphEdge {
                    a,
                    b,
                    cost,
                    flags,
                    subgraph,
                });
            }
        }
    }
    
    Some(LayoutGraph {
        name: name.to_string(),
        points,
        edges,
    })
}
