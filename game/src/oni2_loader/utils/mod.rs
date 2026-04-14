/*
 * oni2_loader/utils/mod.rs — parsing utility sub-modules.
 *
 * binary: little-endian read helpers (read_u16_le, read_u32_le, read_f32_le).
 * bone: world-space → bone-local vertex conversion for win32 model format.
 * parse: text parsing utilities (parse_vec3, extract XML attributes, etc.).
 */
pub mod binary;
pub mod bone;
pub mod parse;
