/*
 * fightai/formation_data.rs — custom formation files (rb.formations + *.frm).
 *
 * Port of `aiFormationManager` / `aiFormationData` / `aiFormationCoordSet`
 * (rb/src/aifight/formation.cpp).  `settings/rb.formations` lists formation
 * names; each `settings/<name>.frm` holds one or more coord sets, each valid
 * for a range of position counts and giving per-slot layout data.
 *
 * These power the `FORMATION_CUSTOM` path (column / wedge / line) used by
 * leader-led squads — in the shipped data the formation is selected at runtime
 * by the ScrOni `form <name>` command (see `DoForm`), e.g. the Blast Chambers'
 * `LeaderScript`.  Coordinates are converted to Bevy space at load (negate X/Z;
 * heading degrees→radians, yaw negated), per the convert-at-boundary rule.
 */
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::oni2_loader::utils::space;

/// One slot in a custom formation (`aiFormationPosData`).
#[derive(Debug, Clone, Copy)]
pub struct FormationPosData {
    /// Slot position relative to the formation origin (Bevy space).
    pub local_absolute_pos: Vec3,
    /// Slot heading in radians (0 = forward), Bevy yaw.
    pub absolute_heading: f32,
    /// Index of the slot this one follows (a column's "guy ahead"); < 0 = none.
    pub relative_to: i32,
    /// 0 = follow `relative_to`, 1 = use the absolute slot, between = lerp.
    pub lerp_to_absolute: f32,
    /// `LocalAbsolutePos - PosData[relative_to].LocalAbsolutePos` (computed).
    pub local_relative_pos: Vec3,
    /// Heading of the relative-position vector (computed).
    pub heading_to_relative_pos_angle: f32,
}

/// A coord set valid for `[min_positions, max_positions]` fighters
/// (`aiFormationCoordSet`).
#[derive(Debug, Clone)]
pub struct FormationCoordSet {
    pub min_positions: i32,
    pub max_positions: i32,
    pub positions: Vec<FormationPosData>,
}

impl FormationCoordSet {
    fn matches(&self, num_positions: usize) -> bool {
        let n = num_positions as i32;
        n >= self.min_positions && n <= self.max_positions
    }
}

/// A named formation (`aiFormationData`) — its coord sets keyed by count range.
#[derive(Debug, Clone)]
pub struct FormationData {
    pub coord_sets: Vec<FormationCoordSet>,
}

impl FormationData {
    /// Coord set for the given fighter count (`aiFormationData::Get`); the last
    /// one is the fallback when none matches, mirroring the legacy clamp.
    pub fn get(&self, num_positions: usize) -> Option<&FormationCoordSet> {
        self.coord_sets
            .iter()
            .find(|cs| cs.matches(num_positions))
            .or_else(|| self.coord_sets.last())
    }
}

/// The custom formation a leader currently wants (`aiLeader`'s `Formation`,
/// set at spawn from `FightAILeader.InitialFormation` and at runtime by the
/// ScrOni `form <name>` command).  Empty/unresolved name → fall back to the
/// circular formation.
#[derive(Component, Debug, Clone, Default)]
pub struct LeaderFormation {
    pub name: String,
}

/// All loaded custom formations, by lowercased name (`aiFormationManager`).
#[derive(Resource, Default)]
pub struct FormationLibrary {
    pub formations: HashMap<String, FormationData>,
}

impl FormationLibrary {
    pub fn get(&self, name: &str) -> Option<&FormationData> {
        self.formations.get(&name.to_lowercase())
    }
}

/// Whitespace tokenizer that treats `{` and `}` as standalone tokens.
struct Tok<'a> {
    toks: Vec<&'a str>,
    i: usize,
}

impl<'a> Tok<'a> {
    fn new(s: &'a str) -> Self {
        // The braces always sit on their own or adjacent to whitespace in the
        // shipped files, but split them defensively anyway.
        let toks = s
            .split(|c: char| c.is_whitespace())
            .filter(|t| !t.is_empty())
            .collect();
        Tok { toks, i: 0 }
    }
    fn next(&mut self) -> Option<&'a str> {
        let t = self.toks.get(self.i).copied();
        if t.is_some() {
            self.i += 1;
        }
        t
    }
    fn int(&mut self) -> Option<i32> {
        self.next()?.parse().ok()
    }
    fn float(&mut self) -> Option<f32> {
        self.next()?.parse().ok()
    }
    fn expect(&mut self, tok: &str) -> Option<()> {
        if self.next()? == tok { Some(()) } else { None }
    }
}

/// Parse one `.frm` file body into a [`FormationData`].
pub fn parse_frm(content: &str) -> Option<FormationData> {
    // Ensure braces are tokenizable even if glued to numbers.
    let spaced = content.replace('{', " { ").replace('}', " } ");
    let mut t = Tok::new(&spaced);

    let num_sets = t.int()?;
    t.expect("{")?;

    let mut coord_sets = Vec::with_capacity(num_sets.max(0) as usize);
    for _ in 0..num_sets {
        let min_positions = t.int()?;
        let max_positions = t.int()?;
        t.expect("{")?;

        let mut positions = Vec::with_capacity(max_positions.max(0) as usize);
        for _ in 0..max_positions {
            t.expect("{")?;
            // x y z heading(deg) relativeTo lerp randX randZ
            let raw = Vec3::new(t.float()?, t.float()?, t.float()?);
            let heading_deg = t.float()?;
            let relative_to = t.int()?;
            let lerp_to_absolute = t.float()?;
            let _rand_x = t.float()?;
            let _rand_z = t.float()?;
            t.expect("}")?;

            positions.push(FormationPosData {
                local_absolute_pos: space::to_bevy_space_pos(raw),
                absolute_heading: space::oni2_to_bevy_yaw_rads(heading_deg.to_radians()),
                relative_to,
                lerp_to_absolute,
                local_relative_pos: Vec3::ZERO,
                heading_to_relative_pos_angle: 0.0,
            });
        }
        t.expect("}")?;

        // Resolve relative positions/headings (aiFormationCoordSet::Init tail).
        for i in 0..positions.len() {
            let rel = positions[i].relative_to;
            if rel >= 0 && (rel as usize) < i {
                let rel_pos =
                    positions[i].local_absolute_pos - positions[rel as usize].local_absolute_pos;
                positions[i].local_relative_pos = rel_pos;
                // dirToRelPos = -atan2(relPos.x, relPos.z)
                positions[i].heading_to_relative_pos_angle = -rel_pos.x.atan2(rel_pos.z);
            }
        }

        coord_sets.push(FormationCoordSet {
            min_positions,
            max_positions,
            positions,
        });
    }
    t.expect("}")?;

    Some(FormationData { coord_sets })
}

/// Read `settings/rb.formations` and every `.frm` it lists into the library.
/// Mirrors `aiFormationManager::Init`.
pub fn load_formations(library: &mut FormationLibrary) {
    let Some(list) = read_settings("rb.formations") else {
        bevy::log::warn!("formations: rb.formations not found — custom formations disabled");
        return;
    };
    let mut t = Tok::new(&list);
    let Some(count) = t.int() else {
        return;
    };
    let mut loaded = 0;
    for _ in 0..count {
        let Some(name) = t.next() else { break };
        let filename = format!("{}.frm", name);
        let Some(body) = read_settings(&filename) else {
            bevy::log::warn!("formations: missing {}", filename);
            continue;
        };
        match parse_frm(&body) {
            Some(data) => {
                library.formations.insert(name.to_lowercase(), data);
                loaded += 1;
            }
            None => bevy::log::warn!("formations: failed to parse {}", filename),
        }
    }
    bevy::log::info!("formations: loaded {}/{} custom formations", loaded, count);
}

/// Startup system: populate the [`FormationLibrary`] from disk.
pub fn load_formation_library_system(mut library: ResMut<FormationLibrary>) {
    load_formations(&mut library);
}

/// Read a file from the `Settings` namespace, VFS first then a filesystem
/// fallback (mirrors the other aifight loaders).
fn read_settings(filename: &str) -> Option<String> {
    if let Ok(s) = crate::vfs::read_to_string("Settings", filename) {
        return Some(s);
    }
    let base = crate::get_assets_path();
    std::fs::read_to_string(format!("{}/Settings/{}", base, filename)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEDGE: &str = r#"1
{
	1 4
	{
		{	0	0	0	0	-1	0	0	0	}
		{	1	0	1	0	0	1.0	0	0	}
		{	-1	0	1	0	0	1.0	0	0	}
		{	2	0	2	0	1	0.2	0	0	}
	}
}
"#;

    #[test]
    fn parses_wedge_frm() {
        let data = parse_frm(WEDGE).expect("parse");
        assert_eq!(data.coord_sets.len(), 1);
        let cs = &data.coord_sets[0];
        assert_eq!(cs.min_positions, 1);
        assert_eq!(cs.max_positions, 4);
        assert_eq!(cs.positions.len(), 4);
        // Row 0 is the root (relative_to = -1).
        assert_eq!(cs.positions[0].relative_to, -1);
        // Row 3 follows row 1 with lerp 0.2.
        assert_eq!(cs.positions[3].relative_to, 1);
        assert!((cs.positions[3].lerp_to_absolute - 0.2).abs() < 1e-6);
        // X is negated into Bevy space: file x=2 → -2.
        assert!((cs.positions[3].local_absolute_pos.x + 2.0).abs() < 1e-6);
        // get() picks the only coord set for any in-range count.
        assert!(data.get(3).is_some());
    }
}
