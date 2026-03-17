pub mod animation;
pub mod components;
pub mod curve;
pub mod environment;
pub mod layout_loader;
pub mod parsers;
pub mod spawn;
pub mod testanim;
pub mod utils;
pub mod registries;

pub use animation::*;
pub use components::*;
pub use environment::*;
pub use layout_loader::*;
pub use spawn::*;
pub use testanim::*;
pub use registries::*;

use avian3d::prelude::*;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;

use crate::menu::InGameEntity;
use crate::oni2_loader::curve::NurbsCurve;
use crate::oni2_loader::parsers::actor_xml::*;
use crate::oni2_loader::parsers::animation::*;
use crate::oni2_loader::parsers::anims::*;
use crate::oni2_loader::parsers::bound::*;
use crate::oni2_loader::parsers::entity_type::*;
use crate::oni2_loader::parsers::layout::*;
use crate::oni2_loader::parsers::mesh::*;
use crate::oni2_loader::parsers::model::*;
use crate::oni2_loader::parsers::skeleton::*;
use crate::oni2_loader::parsers::texture::load_tga_file as texture_load_tga_file;
use crate::oni2_loader::parsers::types::*;
use crate::oni2_loader::utils::bone::*;
use crate::scroni;
