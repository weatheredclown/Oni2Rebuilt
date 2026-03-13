use bevy::prelude::*;


pub fn decode_tex(data: &[u8]) -> Option<(u32, u32, Vec<u8>, bool)> {
    if data.len() < 14 { return None; }
    let width = u16::from_le_bytes([data[0], data[1]]) as u32;
    let height = u16::from_le_bytes([data[2], data[3]]) as u32;
    let tex_type = u16::from_le_bytes([data[4], data[5]]);

    if width == 0 || height == 0 || width > 4096 || height > 4096 { return None; }

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let mut has_alpha = false;

    match tex_type {
        0x01 | 0x0E => {
            // 8-bit indexed with BGRA palette (256 entries at offset 0x0E)
            if data.len() < 0x40E + (width * height) as usize { return None; }
            let clut_off = 0x0E;
            let data_off = 0x40E;
            for y in 0..height {
                for x in 0..width {
                    let src = data_off + ((height - 1 - y) * width + x) as usize;
                    let idx = data[src] as usize;
                    let c_off = clut_off + idx * 4;
                    let dst = (y * width + x) as usize * 4;
                    rgba[dst] = data[c_off + 2];     // R (from BGRA)
                    rgba[dst + 1] = data[c_off + 1]; // G
                    rgba[dst + 2] = data[c_off];     // B
                    let a = data[c_off + 3]; // A
                    rgba[dst + 3] = a;
                    if a < 255 { has_alpha = true; }
                }
            }
        }
        0x0F | 0x10 => {
            // 4-bit indexed with BGRA palette (16 entries at offset 0x0E)
            if data.len() < 0x4E + (width * height / 2) as usize { return None; }
            let clut_off = 0x0E;
            let data_off = 0x4E;
            for y in 0..height {
                for x in (0..width).step_by(2) {
                    let src = data_off + ((height - 1 - y) * width / 2 + x / 2) as usize;
                    let p = data[src];
                    for nibble in 0..2u32 {
                        let idx = if nibble == 0 { (p & 0xF) as usize } else { ((p >> 4) & 0xF) as usize };
                        let c_off = clut_off + idx * 4;
                        let dst = (y * width + x + nibble) as usize * 4;
                        rgba[dst] = data[c_off + 2];
                        rgba[dst + 1] = data[c_off + 1];
                        rgba[dst + 2] = data[c_off];
                        let a = data[c_off + 3];
                        rgba[dst + 3] = a;
                        if a < 255 { has_alpha = true; }
                    }
                }
            }
        }
        0x11 => {
            // RGB888
            let data_off = 0x0E;
            if data.len() < data_off + (width * height * 3) as usize { return None; }
            for y in 0..height {
                for x in 0..width {
                    let src = data_off + ((height - 1 - y) * width + x) as usize * 3;
                    let dst = (y * width + x) as usize * 4;
                    rgba[dst] = data[src];
                    rgba[dst + 1] = data[src + 1];
                    rgba[dst + 2] = data[src + 2];
                    rgba[dst + 3] = 0xFF;
                }
            }
        }
        0x12 => {
            // RGBA8888
            let data_off = 0x0E;
            if data.len() < data_off + (width * height * 4) as usize { return None; }
            for y in 0..height {
                for x in 0..width {
                    let src = data_off + ((height - 1 - y) * width + x) as usize * 4;
                    let dst = (y * width + x) as usize * 4;
                    rgba[dst..dst + 4].copy_from_slice(&data[src..src + 4]);
                    if data[src + 3] < 255 { has_alpha = true; }
                }
            }
        }
        0x06 => {
            // RGBA4444
            let data_off = 0x0E;
            if data.len() < data_off + (width * height * 2) as usize { return None; }
            for y in 0..height {
                for x in 0..width {
                    let src = data_off + ((height - 1 - y) * width + x) as usize * 2;
                    let p = u16::from_le_bytes([data[src], data[src + 1]]);
                    let dst = (y * width + x) as usize * 4;
                    rgba[dst] = ((p & 0xF) as u8) * 17;
                    rgba[dst + 1] = (((p >> 4) & 0xF) as u8) * 17;
                    rgba[dst + 2] = (((p >> 8) & 0xF) as u8) * 17;
                    let a = (((p >> 12) & 0xF) as u8) * 17;
                    rgba[dst + 3] = a;
                    if a < 255 { has_alpha = true; }
                }
            }
        }
        _ => {
            warn!("Unsupported .tex type: 0x{:X}", tex_type);
            return None;
        }
    }

    Some((width, height, rgba, has_alpha))
}

/// Load texture for an entity: tries .tex (native), then .tex.tga (pre-converted).
pub fn load_tga_file(dir: &str, filename: &str, images: &mut ResMut<Assets<Image>>) -> Option<(Handle<Image>, bool)> {
    let bytes = crate::vfs::read(dir, filename).ok()?;
    let dyn_image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Tga).ok()?;
    let rgba = dyn_image.to_rgba8();
    let (width, height) = rgba.dimensions();

    let mut has_alpha = false;
    for p in rgba.pixels() {
        if p[3] < 255 {
            has_alpha = true;
            break;
        }
    }

    let mut image = Image::new(
        bevy::render::render_resource::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        rgba.into_raw(),
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        default(),
    );
    image.sampler = bevy::image::ImageSampler::Descriptor(
        bevy::image::ImageSamplerDescriptor {
            address_mode_u: bevy::image::ImageAddressMode::Repeat,
            address_mode_v: bevy::image::ImageAddressMode::Repeat,
            ..default()
        },
    );
    Some((images.add(image), has_alpha))
}

pub fn load_tga_texture(
    entity_dir: &str,
    texture_name: &str,
    images: &mut ResMut<Assets<Image>>,
) -> Option<(Handle<Image>, bool)> {
    // Try native .tex format first
    let tex_filename = format!("{}.tex", texture_name);
    if crate::vfs::exists(entity_dir, &tex_filename) {
        if let Ok(tex_bytes) = crate::vfs::read(entity_dir, &tex_filename) {
            if let Some((width, height, rgba, has_alpha)) = decode_tex(&tex_bytes) {
                info!("Loaded native .tex texture: {:?} ({}x{}) alpha={}", tex_filename, width, height, has_alpha);
                let mut image = Image::new(
                    bevy::render::render_resource::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    bevy::render::render_resource::TextureDimension::D2,
                    rgba,
                    bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                    default(),
                );
                image.sampler = bevy::image::ImageSampler::Descriptor(
                    bevy::image::ImageSamplerDescriptor {
                        address_mode_u: bevy::image::ImageAddressMode::Repeat,
                        address_mode_v: bevy::image::ImageAddressMode::Repeat,
                        ..default()
                    },
                );
                return Some((images.add(image), has_alpha));
            }
        }
    }

    // Try shader indirection (e.g. perimstructSG.shader -> texture perimstruct01.tex)
    let shader_filename = format!("{}.shader", texture_name);
    if crate::vfs::exists(entity_dir, &shader_filename) {
        if let Ok(shader_data) = crate::vfs::read_to_string(entity_dir, &shader_filename) {
            for line in shader_data.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("texture ") {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let actual_tex = parts[1].trim_matches('"').trim_end_matches(".tex");
                        info!("Shader {} redirects to texture {}", shader_filename, actual_tex);
                        return load_tga_texture(entity_dir, actual_tex, images);
                    }
                }
            }
        }
    }

    // Fallback: pre-converted .tex.tga
    let tga_filename = format!("{}.tex.tga", texture_name);
    if crate::vfs::exists(entity_dir, &tga_filename) {
        if let Some((handle, has_alpha)) = load_tga_file(entity_dir, &tga_filename, images) {
            info!("Loaded .tex.tga texture: {:?} alpha={}", tga_filename, has_alpha);
            return Some((handle, has_alpha));
        }
    }

    // Last resort: bare .tga
    let bare_tga = format!("{}.tga", texture_name);
    if crate::vfs::exists(entity_dir, &bare_tga) {
        if let Some(handle) = load_tga_file(entity_dir, &bare_tga, images) {
            info!("Loaded .tga texture: {:?}", bare_tga);
            return Some(handle);
        }
    }

    info!("Texture not found: {} (.tex/.tex.tga/.tga)", texture_name);
    None
}
