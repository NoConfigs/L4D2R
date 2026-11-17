//! CPU decoding of common Source texture formats, selecting a bounded mip.
use crate::binary::*;
use std::io;
#[derive(Clone)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub mips: Vec<Vec<u8>>,
    pub clamp: [bool; 2],
    pub no_mip: bool,
    /// Full-resolution header size (before `max_dimension`): Source's
    /// material mapping width/height for texture coordinates.
    pub full: [u32; 2],
    /// DXT1/3/5 data kept as the file stores it (BC1/2/3 blocks, every
    /// uploaded mip from largest to smallest), so the GPU copy stays 4-8x
    /// smaller than RGBA. `mips` is then left empty; `rgba` is still decoded
    /// for CPU readers.
    pub blocks: Option<Blocks>,
}
#[derive(Clone)]
pub struct Blocks {
    /// VTF format: 13/20 DXT1, 14 DXT3, 15 DXT5.
    pub format: u32,
    pub levels: u32,
    pub data: Vec<u8>,
}
impl Texture {
    /// RGBA8 texels of every mip, largest first (decoding `blocks` if the
    /// texture kept them instead of `mips`).
    pub fn rgba_mips(&self) -> Vec<u8> {
        let Some(b) = &self.blocks else {
            return self.mips.concat();
        };
        let mut out = Vec::new();
        let mut at = 0;
        for level in 0..b.levels as usize {
            let (w, h) = ((self.width as usize >> level).max(1), (self.height as usize >> level).max(1));
            let Ok(size) = bytes_for(b.format, w, h) else { break };
            let Some(Ok(rgba)) = b.data.get(at..at + size).map(|d| decode_pixels(d, b.format, w, h)) else { break };
            out.extend(rgba);
            at += size;
        }
        out
    }
    /// Number of mips `rgba_mips` / `blocks` hold.
    pub fn mip_count(&self) -> u32 {
        self.blocks.as_ref().map_or(self.mips.len() as u32, |b| b.levels)
    }
}
fn bytes_for(format: u32, w: usize, h: usize) -> io::Result<usize> {
    let pixels = w
        .checked_mul(h)
        .ok_or_else(|| invalid("Texture size overflow"))?;
    match format {
        13 | 20 | 38 => Ok(w.div_ceil(4) * h.div_ceil(4) * 8),
        14 | 15 | 37 => Ok(w.div_ceil(4) * h.div_ceil(4) * 16),
        0 | 1 | 11 | 12 | 16 => Ok(pixels * 4),
        2 | 3 | 9 | 10 => Ok(pixels * 3),
        4 | 6 | 17 | 18 | 19 | 21 | 22 => Ok(pixels * 2),
        5 | 8 => Ok(pixels),
        _ => Err(invalid(format!("Unsupported VTF pixel format {format}"))),
    }
}
/// Phones share one memory pool between CPU and GPU, iOS kills an app past
/// 3 GB, and their GPUs lack BC, so DXT textures upload as RGBA8 (4-8x).
/// Mobile builds cap every texture at 512 (the menu's lab alone passed 3 GB).
pub const MOBILE_MAX_DIMENSION: u32 = 512;
/// A separate budget for 3D surfaces; UI and palette lookup rows retain their
/// existing decoding path. NOMIP controls filtering, not allocation limits.
pub fn decode_world(b: &[u8], max_dimension: u32) -> io::Result<Texture> {
    let mobile = cfg!(target_os = "ios") || std::env::args().any(|a| a == "--profile-mobile-memory");
    let limit = if mobile { max_dimension.min(256) } else { max_dimension };
    decode_world_limit(b, limit, mobile)
}
fn decode_world_limit(b: &[u8], limit: u32, mobile: bool) -> io::Result<Texture> {
    let limit = limit.max(1);
    let mut texture = decode(b, limit)?;
    if mobile && (texture.width > limit || texture.height > limit) {
        let scale = texture.width.max(texture.height).div_ceil(limit);
        let (w, h) = ((texture.width / scale).max(1), (texture.height / scale).max(1));
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let at = (((y * scale).min(texture.height-1) * texture.width + (x * scale).min(texture.width-1)) * 4) as usize;
                rgba.extend_from_slice(&texture.rgba[at..at+4]);
            }
        }
        texture.width = w;
        texture.height = h;
        texture.rgba = rgba;
        texture.mips = vec![texture.rgba.clone()];
        texture.blocks = None;
    }
    Ok(texture)
}
pub fn decode(b: &[u8], max_dimension: u32) -> io::Result<Texture> {
    let max_dimension = if cfg!(any(target_os = "ios", target_os = "android"))
        || std::env::args().any(|a| a == "--profile-mobile-memory")
    {
        max_dimension.min(MOBILE_MAX_DIMENSION)
    } else {
        max_dimension
    };
    if range(b, 0, 4)? != b"VTF\0" || u32_at(b, 4)? != 7 {
        return Err(invalid("Expected VTF version 7"));
    }
    let minor = u32_at(b, 8)?;
    let header = count(b, 12, 4096)?;
    let w = u16_at(b, 16)? as usize;
    let h = u16_at(b, 18)? as usize;
    let full = [w as u32, h as u32];
    let frames = u16_at(b, 24)?.max(1) as usize;
    let format = u32_at(b, 52)?;
    let levels = *range(b, 56, 1)?.first().unwrap() as usize;
    let depth = if minor >= 2 {
        u16_at(b, 63)?.max(1) as usize
    } else {
        1
    };
    if w == 0
        || h == 0
        || w > 16384
        || h > 16384
        || frames > 256
        || depth > 256
        || !(1..=15).contains(&levels)
    {
        return Err(invalid("Invalid VTF dimensions or mip count"));
    }
    let flags = u32_at(b, 20)?;
    let faces = if flags & 0x4000 != 0 {
        if minor < 5 && u16_at(b, 26)? != 0xffff {
            7
        } else {
            6
        }
    } else {
        1
    };
    let mut offset = header;
    if minor >= 3 {
        let resources = count(b, 68, 64)?;
        let mut found = false;
        for i in 0..resources {
            let r = range(b, 80 + i * 8, 8)?;
            if r[..3] == [0x30, 0, 0] {
                if r[3] & 2 != 0 {
                    return Err(invalid("Inline high-resolution texture resource"));
                }
                offset = u32_at(r, 4)? as usize;
                found = true;
            }
        }
        if !found {
            return Err(invalid("VTF high-resolution resource missing"));
        }
    } else {
        let thumb_w = range(b, 61, 1)?[0] as usize;
        let thumb_h = range(b, 62, 1)?[0] as usize;
        if thumb_w > 0 && thumb_h > 0 {
            offset += bytes_for(u32_at(b, 57)?, thumb_w, thumb_h)?;
        }
    }
    let mut target = 0;
    while flags & 0x100 == 0 && target + 1 < levels
        && ((w >> target).max(1) > max_dimension as usize
            || (h >> target).max(1) > max_dimension as usize)
    {
        target += 1;
    }
    let compressed = matches!(format, 13 | 14 | 15 | 20)
        && (w >> target).max(1) % 4 == 0
        && (h >> target).max(1) % 4 == 0;
    let mut smaller = vec![];
    let mut smaller_blocks: Vec<&[u8]> = vec![];
    for level in (target + 1..levels).rev() {
        let size = bytes_for(format, (w >> level).max(1), (h >> level).max(1))?;
        if compressed {
            smaller_blocks.push(range(b, offset, size)?);
        } else {
            smaller.push(decode_pixels(
                range(b, offset, size)?,
                format,
                (w >> level).max(1),
                (h >> level).max(1),
            )?);
        }
        offset = offset
            .checked_add(size * frames * faces * (depth >> level).max(1))
            .ok_or_else(|| invalid("VTF mip offset overflow"))?;
    }
    let w = (w >> target).max(1);
    let h = (h >> target).max(1);
    let data = range(b, offset, bytes_for(format, w, h)?)?;
    let rgba = decode_pixels(data, format, w, h)?;
    // NOMIP palettes can contain stored mip data, but those levels average
    // unrelated colour rows and must never be exposed to the GPU sampler.
    let (mips, blocks) = if compressed {
        let mut all = data.to_vec();
        let mut levels = 1;
        if flags & 0x100 == 0 {
            for level in smaller_blocks.into_iter().rev() {
                all.extend_from_slice(level);
                levels += 1;
            }
        }
        (vec![], Some(Blocks { format, levels, data: all }))
    } else {
        let mut mips = vec![rgba.clone()];
        if flags & 0x100 == 0 { mips.extend(smaller.into_iter().rev()); }
        (mips, None)
    };
    Ok(Texture {
        width: w as u32,
        height: h as u32,
        rgba,
        mips,
        clamp: [flags & 4 != 0, flags & 8 != 0],
        no_mip: flags & 0x100 != 0,
        full,
        blocks,
    })
}
fn rgb565(v: u16) -> [u8; 4] {
    [
        (((v >> 11) & 31) * 255 / 31) as u8,
        (((v >> 5) & 63) * 255 / 63) as u8,
        ((v & 31) * 255 / 31) as u8,
        255,
    ]
}
fn alpha_block(b: &[u8]) -> [u8; 16] {
    let mut values = [0u8; 8];
    values[0] = b[0];
    values[1] = b[1];
    if b[0] > b[1] {
        for i in 1..7 {
            values[i + 1] = (((7 - i) as u16 * b[0] as u16 + i as u16 * b[1] as u16) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            values[i + 1] = (((5 - i) as u16 * b[0] as u16 + i as u16 * b[1] as u16) / 5) as u8;
        }
        values[6] = 0;
        values[7] = 255;
    }
    let bits = b[2..8]
        .iter()
        .enumerate()
        .fold(0u64, |v, (i, b)| v | ((*b as u64) << (8 * i)));
    std::array::from_fn(|i| values[((bits >> (i * 3)) & 7) as usize])
}
fn decode_pixels(b: &[u8], format: u32, w: usize, h: usize) -> io::Result<Vec<u8>> {
    let mut output = vec![0; w * h * 4];
    if matches!(format, 13 | 14 | 15 | 20 | 37 | 38) {
        let size = if matches!(format, 13 | 20 | 38) {
            8
        } else {
            16
        };
        for by in 0..h.div_ceil(4) {
            for bx in 0..w.div_ceil(4) {
                let block = range(b, (by * w.div_ceil(4) + bx) * size, size)?;
                let pixels: [[u8; 4]; 16] = if format == 37 || format == 38 {
                    let x = alpha_block(&block[..8]);
                    let y = if format == 37 {
                        alpha_block(&block[8..])
                    } else {
                        [128; 16]
                    };
                    std::array::from_fn(|i| {
                        if format == 38 {
                            [x[i], x[i], x[i], 255]
                        } else {
                            let nx = x[i] as f32 / 127.5 - 1.;
                            let ny = y[i] as f32 / 127.5 - 1.;
                            let z = (1. - nx * nx - ny * ny).max(0.).sqrt();
                            [x[i], y[i], ((z + 1.) * 127.5) as u8, 255]
                        }
                    })
                } else {
                    let color = if format == 14 || format == 15 {
                        &block[8..]
                    } else {
                        block
                    };
                    let a = u16_at(color, 0)?;
                    let z = u16_at(color, 2)?;
                    let mut palette = [rgb565(a), rgb565(z), [0; 4], [0; 4]];
                    if a > z || matches!(format, 14 | 15) {
                        for c in 0..3 {
                            palette[2][c] =
                                ((2 * palette[0][c] as u16 + palette[1][c] as u16) / 3) as u8;
                            palette[3][c] =
                                ((palette[0][c] as u16 + 2 * palette[1][c] as u16) / 3) as u8;
                        }
                        palette[2][3] = 255;
                        palette[3][3] = 255;
                    } else {
                        for c in 0..3 {
                            palette[2][c] =
                                ((palette[0][c] as u16 + palette[1][c] as u16) / 2) as u8;
                        }
                        palette[2][3] = 255;
                    }
                    let indices = u32_at(color, 4)?;
                    let alpha = if format == 15 {
                        alpha_block(&block[..8])
                    } else {
                        [255; 16]
                    };
                    std::array::from_fn(|i| {
                        let mut pixel = palette[((indices >> (i * 2)) & 3) as usize];
                        if format == 14 {
                            pixel[3] = ((block[i / 2] >> (4 * (i % 2))) & 15) * 17;
                        } else if format == 15 {
                            pixel[3] = alpha[i];
                        }
                        pixel
                    })
                };
                for (i, p) in pixels.into_iter().enumerate() {
                    let x = bx * 4 + i % 4;
                    let y = by * 4 + i / 4;
                    if x < w && y < h {
                        output[(y * w + x) * 4..(y * w + x + 1) * 4].copy_from_slice(&p);
                    }
                }
            }
        }
    } else {
        let stride = bytes_for(format, 1, 1)?;
        for (src, dst) in b.chunks_exact(stride).zip(output.chunks_exact_mut(4)) {
            let p = match format {
                0 => [src[0], src[1], src[2], src[3]],
                11 => [src[1], src[2], src[3], src[0]],
                1 => [src[3], src[2], src[1], src[0]],
                2 | 9 => [src[0], src[1], src[2], 255],
                3 | 10 => [src[2], src[1], src[0], 255],
                12 => [src[2], src[1], src[0], src[3]],
                16 => [src[2], src[1], src[0], 255],
                4 => {
                    let p = rgb565(u16::from_le_bytes([src[0], src[1]]));
                    [p[2], p[1], p[0], 255]
                }
                5 => [src[0], src[0], src[0], 255],
                6 => [src[0], src[0], src[0], src[1]],
                8 => [255, 255, 255, src[0]],
                17 => rgb565(u16::from_le_bytes([src[0], src[1]])),
                18 | 21 => {
                    let n = u16::from_le_bytes([src[0], src[1]]);
                    [
                        (((n >> 10) & 31) * 255 / 31) as u8,
                        (((n >> 5) & 31) * 255 / 31) as u8,
                        ((n & 31) * 255 / 31) as u8,
                        if format == 18 || n & 0x8000 != 0 {
                            255
                        } else {
                            0
                        },
                    ]
                }
                19 => {
                    let n = u16::from_le_bytes([src[0], src[1]]);
                    [
                        ((n >> 8) & 15) as u8 * 17,
                        ((n >> 4) & 15) as u8 * 17,
                        (n & 15) as u8 * 17,
                        ((n >> 12) & 15) as u8 * 17,
                    ]
                }
                22 => [src[0], src[1], 255, 255],
                _ => return Err(invalid("Unsupported unpacked VTF format")),
            };
            dst.copy_from_slice(&p);
        }
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn world_nomip_budget_preserves_mapping_and_does_not_change_ui_decode() {
        let mut b = vec![0; 80];
        b[..4].copy_from_slice(b"VTF\0");
        b[4..8].copy_from_slice(&7u32.to_le_bytes());
        b[8..12].copy_from_slice(&2u32.to_le_bytes());
        b[12..16].copy_from_slice(&80u32.to_le_bytes());
        b[16..18].copy_from_slice(&8u16.to_le_bytes());
        b[18..20].copy_from_slice(&4u16.to_le_bytes());
        b[20..24].copy_from_slice(&0x104u32.to_le_bytes());
        b[24..26].copy_from_slice(&1u16.to_le_bytes());
        b[56] = 1;
        b[63] = 1;
        for y in 0..4u8 { for x in 0..8u8 { b.extend([x, y, 42, 255]); } }
        let ui = decode(&b, 2).unwrap();
        assert_eq!((ui.width, ui.height), (8, 4));
        let world = decode_world_limit(&b, 2, true).unwrap();
        assert_eq!((world.width, world.height), (2, 1));
        assert_eq!(world.full, [8, 4]);
        assert!(world.no_mip);
        assert_eq!(world.clamp, [true, false]);
        assert_eq!(world.mips.len(), 1);
        assert_eq!(world.rgba_mips(), vec![0, 0, 42, 255, 4, 0, 42, 255]);
        assert!(world.blocks.is_none());
    }
    #[test]
    fn selects_mip_from_high_resolution_resource() {
        let mut b = vec![0; 88];
        b[..4].copy_from_slice(b"VTF\0");
        b[4..8].copy_from_slice(&7u32.to_le_bytes());
        b[8..12].copy_from_slice(&3u32.to_le_bytes());
        b[12..16].copy_from_slice(&88u32.to_le_bytes());
        b[16..18].copy_from_slice(&4u16.to_le_bytes());
        b[18..20].copy_from_slice(&4u16.to_le_bytes());
        b[24..26].copy_from_slice(&1u16.to_le_bytes());
        b[56] = 3;
        b[63] = 1;
        b[68] = 1;
        b[80] = 0x30;
        b[84..88].copy_from_slice(&88u32.to_le_bytes());
        b.extend([0, 0, 255, 255]);
        for _ in 0..4 {
            b.extend([0, 255, 0, 255]);
        }
        for _ in 0..16 {
            b.extend([255, 0, 0, 255]);
        }
        let t = decode(&b, 2).unwrap();
        assert_eq!((t.width, t.height), (2, 2));
        assert_eq!(&t.rgba[..4], &[0, 255, 0, 255]);
        assert_eq!(t.mips.len(), 2);
        assert_eq!(t.mips[1], vec![0, 0, 255, 255]);
        let t = decode(&b, 4).unwrap();
        assert_eq!(&t.rgba[..4], &[255, 0, 0, 255]);
        b[20..24].copy_from_slice(&0x30cu32.to_le_bytes());
        let t = decode(&b, 2).unwrap();
        assert_eq!((t.width, t.height), (4, 4));
        assert!(t.no_mip);
        assert_eq!(t.mips.len(), 1);
        assert_eq!(&t.mips[0][..4], &[255, 0, 0, 255]);
    }
    #[test]
    fn bc1_decodes_red_block_and_transparency() {
        let p = decode_pixels(&[0, 248, 0, 0, 0, 0, 0, 0], 13, 4, 4).unwrap();
        assert_eq!(&p[..4], &[255, 0, 0, 255]);
        let p = decode_pixels(&[0, 0, 255, 255, 255, 255, 255, 255], 13, 4, 4).unwrap();
        assert_eq!(p[3], 0);
    }
    #[test]
    fn dxt_textures_keep_their_blocks_for_the_gpu() {
        let mut b = vec![0; 88];
        b[..4].copy_from_slice(b"VTF ");
        b[4..8].copy_from_slice(&7u32.to_le_bytes());
        b[8..12].copy_from_slice(&3u32.to_le_bytes());
        b[12..16].copy_from_slice(&88u32.to_le_bytes());
        b[16..18].copy_from_slice(&8u16.to_le_bytes());
        b[18..20].copy_from_slice(&8u16.to_le_bytes());
        b[24..26].copy_from_slice(&1u16.to_le_bytes());
        b[52..56].copy_from_slice(&13u32.to_le_bytes());
        b[56] = 3;
        b[63] = 1;
        b[68] = 1;
        b[80] = 0x30;
        b[84..88].copy_from_slice(&88u32.to_le_bytes());
        let block = |c: u16| [c.to_le_bytes()[0], c.to_le_bytes()[1], 0, 0, 0, 0, 0, 0];
        // Smallest mip first: 2x2 blue, 4x4 green, 8x8 red.
        b.extend(block(0x001f));
        b.extend(block(0x07e0));
        for _ in 0..4 {
            b.extend(block(0xf800));
        }
        let t = decode(&b, 1024).unwrap();
        let blocks = t.blocks.as_ref().unwrap();
        assert_eq!((blocks.format, blocks.levels, blocks.data.len()), (13, 3, 48));
        assert_eq!(&blocks.data[..2], &0xf800u16.to_le_bytes());
        assert!(t.mips.is_empty());
        assert_eq!(&t.rgba[..4], &[255, 0, 0, 255]);
        let all = t.rgba_mips();
        assert_eq!(all.len(), (64 + 16 + 4) * 4);
        assert_eq!(&all[256..260], &[0, 255, 0, 255]);
        assert_eq!(&all[320..324], &[0, 0, 255, 255]);
        // Sizes that are not whole blocks stay RGBA.
        b[16..18].copy_from_slice(&6u16.to_le_bytes());
        assert!(decode(&b, 1024).unwrap().blocks.is_none());
    }
    #[test]
    fn rejects_truncated_texture() {
        assert!(decode(b"VTF\0", 1024).is_err());
    }
    #[test]
    fn bc3_reads_alpha_indices() {
        let mut b = vec![0; 16];
        b[0] = 64;
        b[1] = 128;
        b[8..10].copy_from_slice(&0xf800u16.to_le_bytes());
        let p = decode_pixels(&b, 15, 4, 4).unwrap();
        assert_eq!(&p[..4], &[255, 0, 0, 64]);
    }
}

/// An HDR environment cubemap (RGBA16161616F, VTF format 24): six faces of
/// half-float RGBA, laid out face by face with every mip from largest to
/// smallest (wgpu's layer-major order). Source stores the faces right, left,
/// back, front, up, down, which are +X, -X, +Y, -Y, +Z, -Z in Source world
/// axes; the shaders sample them with Source-space directions.
pub struct Cubemap {
    pub size: u32,
    pub levels: u32,
    pub data: Vec<u8>,
}
pub fn decode_cubemap_hdr(b: &[u8]) -> io::Result<Cubemap> {
    if range(b, 0, 4)? != b"VTF\0" || u32_at(b, 4)? != 7 {
        return Err(invalid("Expected VTF version 7"));
    }
    let minor = u32_at(b, 8)?;
    let header = count(b, 12, 4096)?;
    let w = u16_at(b, 16)? as usize;
    let h = u16_at(b, 18)? as usize;
    let flags = u32_at(b, 20)?;
    let frames = u16_at(b, 24)?.max(1) as usize;
    let format = u32_at(b, 52)?;
    let levels = *range(b, 56, 1)?.first().unwrap() as usize;
    if format != 24 || flags & 0x4000 == 0 || w != h || w == 0 || w > 4096 || !(1..=15).contains(&levels) {
        return Err(invalid("Not an RGBA16F cubemap"));
    }
    let faces = if minor < 5 && u16_at(b, 26)? != 0xffff { 7 } else { 6 };
    let mut offset = header;
    if minor >= 3 {
        let resources = count(b, 68, 64)?;
        let mut found = false;
        for i in 0..resources {
            let r = range(b, 80 + i * 8, 8)?;
            if r[..3] == [0x30, 0, 0] {
                offset = u32_at(r, 4)? as usize;
                found = true;
            }
        }
        if !found {
            return Err(invalid("VTF high-resolution resource missing"));
        }
    } else {
        let thumb_w = range(b, 61, 1)?[0] as usize;
        let thumb_h = range(b, 62, 1)?[0] as usize;
        if thumb_w > 0 && thumb_h > 0 {
            offset += bytes_for(u32_at(b, 57)?, thumb_w, thumb_h)?;
        }
    }
    // Stored smallest mip first; each mip holds frames x faces images.
    let level_size = |level: usize| {
        let s = (w >> level).max(1);
        s * s * 8
    };
    let mut starts = vec![0usize; levels];
    for level in (0..levels).rev() {
        starts[level] = offset;
        offset = offset
            .checked_add(level_size(level) * frames * faces)
            .ok_or_else(|| invalid("VTF mip offset overflow"))?;
    }
    let mut data = Vec::with_capacity((0..levels).map(level_size).sum::<usize>() * 6);
    for face in 0..6 {
        for (level, start) in starts.iter().enumerate() {
            let size = level_size(level);
            data.extend_from_slice(range(b, start + face * size, size)?);
        }
    }
    Ok(Cubemap { size: w as u32, levels: levels as u32, data })
}
