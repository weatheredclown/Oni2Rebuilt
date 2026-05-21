/*
 * oni2_loader/drawable.rs — standalone `.type` drawable loader.
 *
 * The rmcDrawable-equivalent layer the entity-spawn path doesn't expose.
 * Given a `.type` file path, reads the LodGroup manifest, loads the
 * referenced `.mesh` (geometry) and `.shader` (texture + per-pass blend
 * + UV-animation directives), builds Bevy `Mesh` + `StandardMaterial`
 * handles, and registers each (mesh, animator) pair into
 * `DrawableLibrary` so `uv_animator_system` ticks the texture scroll.
 *
 * Used by FX types like `fxBlastFireType` that need to instantiate
 * renderable geometry without the entity-skeleton machinery.  See
 * [[rmcdrawable-gap]] for the architectural background on why this
 * lives in its own file rather than inside `spawn.rs`.
 */

use bevy::prelude::*;

use crate::env_reflect_material::EnvReflectMaterial;
use crate::oni2_loader::animation::PassMaterial;
use crate::oni2_loader::parsers::entity_type::parse_drawable_type;
use crate::oni2_loader::parsers::mesh::{build_meshes_by_material, parse_mesh};
use crate::oni2_loader::parsers::model::parse_shader;
use crate::oni2_loader::registries::DrawableLibrary;
use crate::oni2_loader::spawn::{build_pass_material, parse_uv_animator};

/// One renderable layer of a loaded drawable: a mesh paired with one
/// material from one shader pass.  Multi-pass shaders (like the blast
/// fire's two-pass additive scroll) produce one entry per pass with
/// distinct `depth_bias` values so the layers render in order.
#[derive(Clone)]
pub struct LoadedDrawablePass {
    pub mesh: Handle<Mesh>,
    pub material: PassMaterial,
    /// Pre-baked depth bias for this pass (later passes nudged forward
    /// to win the Z-fight).  Already applied to the `StandardMaterial`
    /// — exposed here mainly for diagnostics.
    pub depth_bias: f32,
}

/// A fully-resolved drawable: all (sub-mesh × shader-pass) combos with
/// their built Bevy handles.  Caller spawns one Bevy entity per `pass`
/// (using `mesh` + `material`) under whatever parent makes sense for
/// the effect (player-position, world-fixed marker, etc.).
#[derive(Clone)]
pub struct LoadedDrawable {
    pub passes: Vec<LoadedDrawablePass>,
    /// LOD switch radius from the `.type`'s `radius` line.  Currently
    /// informational only — FX drawables don't LOD-swap.
    pub radius: f32,
}

/// Load a `.type` manifest + its referenced `.mesh` + `.shader` and
/// produce a renderable bundle plus DrawableLibrary registration.
///
/// `type_path` is VFS-rooted (e.g. `Entity/BlastFire/blast_fire.type`).
/// The mesh basename is read out of the `.type`'s LodGroup; the shader
/// is the same basename with `.shader` substituted for the mesh
/// extension (the AGE convention — mesh and shader share a stem).
///
/// Returns `None` if any of the three sibling files can't be read or
/// parsed.  Caller is expected to log; we don't to keep this composable.
pub fn load_drawable(
    type_path: &str,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    env_materials: &mut Assets<EnvReflectMaterial>,
    images: &mut Assets<Image>,
    drawables: &mut DrawableLibrary,
) -> Option<LoadedDrawable> {
    // -- 1. Read the .type manifest --
    let type_content = crate::vfs::read_to_string("", type_path).ok()?;
    let dtype = parse_drawable_type(&type_content);
    let mesh_filename = dtype.mesh_name?;

    // The .mesh and .shader live alongside the .type in the same
    // directory.  Build paths relative to the .type's parent.
    let dir = std::path::Path::new(type_path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .to_string();
    let dir_prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{}/", dir)
    };

    // -- 2. Read and parse the .mesh --
    let mesh_path = format!("{}{}", dir_prefix, mesh_filename);
    let mesh_bytes = crate::vfs::read("", &mesh_path).ok()?;
    let mesh_text = std::str::from_utf8(&mesh_bytes).ok()?;
    let model = parse_mesh(mesh_text, &dir)?;

    // -- 3. Build one Bevy Mesh per source material --
    let sub_meshes = build_meshes_by_material(&model);
    let mesh_handles: Vec<Handle<Mesh>> = sub_meshes
        .into_iter()
        .map(|(_mat_idx, m)| meshes.add(m))
        .collect();

    // -- 4. Read and parse the .shader (same basename as the .mesh) --
    let shader_basename = mesh_filename
        .rsplit_once('.')
        .map(|(stem, _ext)| stem.to_string())
        .unwrap_or_else(|| mesh_filename.clone());
    let shader_path = format!("{}{}.shader", dir_prefix, shader_basename);
    let shader_content = crate::vfs::read_to_string("", &shader_path).ok()?;
    let passes = parse_shader(&shader_content);

    if passes.is_empty() {
        return None;
    }

    // -- 5. For each (mesh, shader-pass) pair, build a material and
    //       collect per-pass animator directives.  All passes share the
    //       same mesh handle (multi-pass = layered renders of the same
    //       geometry with different blend modes), so we register the
    //       drawable into DrawableLibrary ONCE per sub-mesh with the
    //       UNION of all passes' animators — `uv_animator_system`
    //       reduces the list with the "last non-zero wins" rule already.
    let mut loaded_passes: Vec<LoadedDrawablePass> = Vec::new();
    let context_dir = if dir.is_empty() { "." } else { dir.as_str() };

    for (mesh_idx, mesh_handle) in mesh_handles.iter().enumerate() {
        let mut animators_for_this_mesh = Vec::with_capacity(passes.len());
        for (pass_idx, pass) in passes.iter().enumerate() {
            let material = build_pass_material(
                pass,
                pass_idx,
                Color::WHITE,
                context_dir,
                materials,
                env_materials,
                images,
            );
            loaded_passes.push(LoadedDrawablePass {
                mesh: mesh_handle.clone(),
                material,
                depth_bias: pass_idx as f32 * 10.0,
            });
            animators_for_this_mesh.push(parse_uv_animator(pass));
        }
        let key = format!("fx:{}#{}", shader_basename, mesh_idx);
        drawables.register(
            key,
            shader_basename.clone(),
            mesh_handle.clone(),
            animators_for_this_mesh,
        );
    }

    Some(LoadedDrawable {
        passes: loaded_passes,
        radius: dtype.lod_radius,
    })
}
