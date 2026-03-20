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

#[derive(Debug, Clone)]
pub struct Oni2Bound {
    pub vertices: Vec<[f32; 3]>,
    pub centroid: [f32; 3],
    pub edges: Vec<[u32; 2]>,
    pub quads: Vec<[u32; 4]>,
    pub tris: Vec<[u32; 3]>,
}

// === Parsed .skel file data ===

#[derive(Debug, Clone, Default)]
pub struct Oni2Skeleton {
    pub positions: Vec<[f32; 3]>,
    pub parent_indices: Vec<Option<usize>>,
    pub names: Vec<String>,
    /// Raw local offsets from parent (before accumulation to world positions).
    pub local_offsets: Vec<[f32; 3]>,
}

// === Parsed Entity.type ===

#[derive(Debug)]
pub struct Oni2EntityType {
    pub model_file: Option<String>,
    pub bound_file: Option<String>,
    pub skel_file: Option<String>,
    pub lod_radius: f32,
}

// === Parsed .anim file data ===

#[derive(Debug, Clone, Default)]
pub struct Oni2Animation {
    pub num_frames: u32,
    pub num_channels: u32,
    pub stride_z: f32,
    pub is_loop: bool,
    pub frames: Vec<Vec<f32>>,  // frames[frame_idx][channel_idx]
}
