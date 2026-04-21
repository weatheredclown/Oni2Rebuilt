/*
 * oni2_loader/parsers/types.rs — shared parsed data types.
 *
 * Plain Rust structs shared across multiple parsers:
 * Oni2Model (vertices, normals, UVs, materials, packets, adjuncts),
 * Oni2Skeleton (positions, parent indices, names, channels),
 * Oni2Bound, Oni2Animation, Oni2EntityType, Oni2Material / Oni2Packet, etc.
 */
// === Parsed .mod file data ===

#[derive(Debug, Clone)]
pub struct Oni2Model {
    pub vertices: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub tex_coords: Vec<[f32; 2]>,
    pub materials: Vec<Oni2Material>,
    pub packets: Vec<Oni2Packet>,
    /// Per-bone world-space positions from skeleton (bind pose or animated).
    pub bone_world_positions: Vec<[f32; 3]>,
    /// Per-bone world-space rotations as quaternions [x, y, z, w]. Defaults to identity.
    pub bone_rotations: Vec<[f32; 4]>,
    /// If true, vertices are already in world space (win32 build) — skip bone transforms.
    pub world_space_verts: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Oni2MaterialPass {
    pub texture_name: Option<String>,
    pub lighting: Option<String>,
    pub blendset: Option<String>,
    pub texcombine: Option<String>,
    pub texsrc: Option<u32>,
    pub alphafunc: Option<String>,
    pub slides: Option<String>,
    pub slidet: Option<String>,
    pub rotate: Option<String>,
    pub scalet: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Oni2Material {
    pub name: String,
    pub diffuse: [f32; 3],
    pub texture_name: Option<String>,
    pub primitive_count: u32,
    pub packet_count: u32,
    pub passes: Vec<Oni2MaterialPass>,
}

#[derive(Debug, Clone)]
pub struct Oni2Packet {
    pub adjuncts: Vec<Oni2Adjunct>,
    pub strips: Vec<Vec<u32>>, // each strip is a list of adjunct indices
    /// Per-strip type: 1=str (normal winding), 2=stp (swapped initial parity).
    pub strip_types: Vec<u32>,
    pub material_index: usize,
    /// Maps local bone index → global bone index (from mtx line).
    pub bone_map: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Oni2Adjunct {
    pub vertex_idx: u32,
    pub normal_idx: u32,
    pub color_idx: u32,
    pub tex1_idx: i32, // can be -1
    pub bone_idx: u32, // local bone index within packet's bone_map
}

// === Parsed .bnd file data ===

/// One section from a composite .bnd file (or the entire file for simple bounds).
/// Vertices are in entity-local Bevy space (X and Z already negated).
/// Edge/quad/tri indices are local to THIS sub-bound's vertex array (always base-0).
#[derive(Debug, Clone, Default)]
pub struct Oni2SubBound {
    pub bound_type: Option<String>,
    pub material_type: Option<String>,
    pub vertices: Vec<[f32; 3]>,
    pub centroid: [f32; 3],
    pub edges: Vec<[u32; 2]>,
    pub quads: Vec<[u32; 4]>,
    pub tris: Vec<[u32; 3]>,
}

/// Parsed .bnd file — a list of sub-bounds plus an overall composite centroid.
#[derive(Debug, Clone, Default)]
pub struct Oni2Bound {
    pub sub_bounds: Vec<Oni2SubBound>,
    pub centroid: [f32; 3],
}

// === Parsed .skel file data ===

#[derive(Clone, Default, Debug)]
pub struct Oni2BoneChannels {
    pub has_trans_x: bool,
    pub has_trans_y: bool,
    pub has_trans_z: bool,
    pub has_rot_x: bool,
    pub has_rot_y: bool,
    pub has_rot_z: bool,
}

#[derive(Clone, Default, Debug)]
pub struct Oni2Skeleton {
    pub positions: Vec<[f32; 3]>, // Bind-pose positions in world space (pre-offsetting)
    pub parent_indices: Vec<Option<usize>>,
    pub names: Vec<String>,
    pub local_offsets: Vec<[f32; 3]>, // Explicit local offsets from parent
    pub channels: Vec<Oni2BoneChannels>, // Track explicit explicit dimensions per-node mappings
    pub channel_is_rot: Vec<bool>, // Flattened boolean flags mapping index layout directly to angle/pos tracking
}

impl Oni2Skeleton {
    /// Evaluates the total number of channels natively defined spanning all constituent bones combined natively.
    pub fn expected_anim_channels(&self) -> usize {
        self.channel_is_rot.len()
    }

    pub fn build_channel_map(&mut self) {
        self.channel_is_rot.clear();
        for c in &self.channels {
            if c.has_trans_x {
                self.channel_is_rot.push(false);
            }
            if c.has_trans_y {
                self.channel_is_rot.push(false);
            }
            if c.has_trans_z {
                self.channel_is_rot.push(false);
            }
            if c.has_rot_x {
                self.channel_is_rot.push(true);
            }
            if c.has_rot_y {
                self.channel_is_rot.push(true);
            }
            if c.has_rot_z {
                self.channel_is_rot.push(true);
            }
        }
    }
}

// === Parsed Entity.type ===

#[derive(Debug)]
pub struct Oni2EntityType {
    pub model_file: Option<String>,
    pub bound_file: Option<String>,
    pub skel_file: Option<String>,
    pub lod_radius: f32,
    pub jump_controller: Option<crate::oni2_loader::parsers::jump::JumpController>,
}

// === Parsed .anim file data ===

#[derive(Debug, Clone, Default)]
pub struct Oni2Animation {
    pub num_frames: u32,
    pub num_channels: u32,
    pub stride_z: f32,
    pub is_loop: bool,
    pub frames: Vec<Vec<f32>>, // frames[frame_idx][channel_idx]
    pub attack_data: Option<crate::oni2_loader::parsers::atdt::AtdtData>,
    pub react_data: Option<crate::oni2_loader::parsers::rct::ReactData>,
    /// Phase-triggered FX entries loaded from ANIM_FX blocks (rbAnimFXData).
    /// Each entry fires its fx list once when the animation's normalized phase
    /// crosses `phase` (ascending).  Reset when a new anim id starts playing.
    pub anim_fx: Vec<crate::animator::AnimFxEntry>,
    /// Root-motion conditioning from the sibling `.gait` file, if present.
    /// Drives the `strip_root_xz` decision during playback — see animation.rs.
    /// `None` = no .gait file existed; we fall back to a loop-based heuristic.
    pub gait_normalize: Option<crate::oni2_loader::parsers::gait::GaitNormalize>,
}

// === Parsed .expl explosion data ===

#[derive(Debug, Clone)]
pub struct ExplodeFXDef {
    pub fx_type: String,
    pub offset: [f32; 3],
    pub delay: f32,
}

#[derive(Debug, Clone)]
pub struct EllipsoidDamageDef {
    pub offset: [f32; 3],
    pub max_radii: [f32; 3],
    pub orientation: [f32; 3],
    pub start_radius_percentage: f32,
    pub blast_duration: f32,
    pub max_damage: f32,
    pub max_damage_radius_percentage: f32,
    pub continuous_damage: bool,
}

#[derive(Debug, Clone)]
pub struct BoxDamageDef {
    pub offset: [f32; 3],
    pub orientation: [f32; 3],
    pub blast_duration: f32,
    pub continuous_damage: bool,
    pub start_damage: f32,
    pub end_damage: f32,
    pub start_dimensions: [f32; 3],
    pub end_dimensions: [f32; 3],
    pub end_translation: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct BasicExplosionDef {
    pub name: String,
    pub fx: Vec<ExplodeFXDef>,
    pub ellipsoid: Option<EllipsoidDamageDef>,
    pub r#box: Option<BoxDamageDef>,
}
