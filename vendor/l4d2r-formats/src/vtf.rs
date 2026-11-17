//! VTF (Valve Texture Format) 7.x decoder. Produces RGBA8 mip chains.

use anyhow::{bail, Result};

pub const FLAG_CLAMPS: u32 = 0x4;
pub const FLAG_CLAMPT: u32 = 0x8;
pub const FLAG_NORMAL: u32 = 0x80;
pub const FLAG_ENVMAP: u32 = 0x4000;
pub const FLAG_ONEBITALPHA: u32 = 0x2000;
pub const FLAG_EIGHTBITALPHA: u32 = 0x3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ImageFormat {
    Rgba8888 = 0,
    Abgr8888 = 1,
    Rgb888 = 2,
    Bgr888 = 3,
    Rgb565 = 4,
    I8 = 5,
    Ia88 = 6,
    A8 = 8,
    Argb8888 = 11,
    Bgra8888 = 12,
    Dxt1 = 13,
    Dxt3 = 14,
    Dxt5 = 15,
    Bgrx8888 = 16,
    Bgr565 = 17,
    Bgrx5551 = 18,
    Bgra4444 = 19,
    Dxt1OneBitAlpha = 20,
    Bgra5551 = 21,
    Uv88 = 22,
    Uvwq8888 = 23,
    Rgba16161616F = 24,
    Rgba16161616 = 25,
    Uvlx8888 = 26,
    Unknown = -1,
}

impl ImageFormat {
    fn from_i32(v: i32) -> Self {
        use ImageFormat::*;
        match v {
            0 => Rgba8888,
            1 => Abgr8888,
            2 => Rgb888,
            3 => Bgr888,
            4 => Rgb565,
            5 => I8,
            6 => Ia88,
            8 => A8,
            11 => Argb8888,
            12 => Bgra8888,
            13 => Dxt1,
            14 => Dxt3,
            15 => Dxt5,
            16 => Bgrx8888,
            17 => Bgr565,
            18 => Bgrx5551,
            19 => Bgra4444,
            20 => Dxt1OneBitAlpha,
            21 => Bgra5551,
            22 => Uv88,
            23 => Uvwq8888,
            24 => Rgba16161616F,
            25 => Rgba16161616,
            26 => Uvlx8888,
            _ => Unknown,
        }
    }

    fn size(self, w: usize, h: usize) -> usize {
        use ImageFormat::*;
        match self {
            Dxt1 | Dxt1OneBitAlpha => w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * 8,
            Dxt3 | Dxt5 => w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * 16,
            Rgba8888 | Abgr8888 | Argb8888 | Bgra8888 | Bgrx8888 | Uvwq8888 | Uvlx8888 => w * h * 4,
            Rgb888 | Bgr888 => w * h * 3,
            Rgb565 | Bgr565 | Bgrx5551 | Bgra4444 | Bgra5551 | Ia88 | Uv88 => w * h * 2,
            I8 | A8 => w * h,
            Rgba16161616F | Rgba16161616 => w * h * 8,
            Unknown => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Vtf {
    pub width: u32,
    pub height: u32,
    pub flags: u32,
    pub frames: u32,
    pub format: ImageFormat,
    pub reflectivity: [f32; 3],
    /// Mip levels, largest first, RGBA8 (first frame / first face only).
    pub mips: Vec<Mip>,
}

#[derive(Debug, Clone)]
pub struct Mip {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn rd_f32(b: &[u8], o: usize) -> f32 {
    f32::from_bits(rd_u32(b, o))
}

impl Vtf {
    pub fn parse(b: &[u8]) -> Result<Vtf> {
        if b.len() < 64 || &b[0..4] != b"VTF\0" {
            bail!("not a VTF");
        }
        let ver_major = rd_u32(b, 4);
        let ver_minor = rd_u32(b, 8);
        if ver_major != 7 {
            bail!("unsupported VTF version {ver_major}.{ver_minor}");
        }
        let header_size = rd_u32(b, 12) as usize;
        let width = rd_u16(b, 16) as u32;
        let height = rd_u16(b, 18) as u32;
        let flags = rd_u32(b, 20);
        let frames = rd_u16(b, 24).max(1) as u32;
        let reflectivity = [rd_f32(b, 32), rd_f32(b, 36), rd_f32(b, 40)];
        let format = ImageFormat::from_i32(rd_u32(b, 52) as i32);
        let mip_count = b[56].max(1) as usize;
        let low_fmt = ImageFormat::from_i32(rd_u32(b, 57) as i32);
        let (low_w, low_h) = (b[61] as usize, b[62] as usize);
        let depth = if ver_minor >= 2 {
            rd_u16(b, 63).max(1) as usize
        } else {
            1
        };

        // Locate the high-res image resource (type 0x30). Pre-7.3 files are laid out linearly.
        let mut data_off = header_size
            + if low_w * low_h > 0 {
                low_fmt.size(low_w, low_h)
            } else {
                0
            };
        if ver_minor >= 3 {
            let n = rd_u32(b, 68) as usize;
            for i in 0..n {
                let e = 80 + i * 8;
                let tag = [b[e], b[e + 1], b[e + 2]];
                if tag == [0x30, 0, 0] {
                    data_off = rd_u32(b, e + 4) as usize;
                }
            }
        }
        let faces = if flags & FLAG_ENVMAP != 0 {
            if ver_minor < 5 {
                7
            } else {
                6
            }
        } else {
            1
        };

        // Data is stored smallest mip first; each mip holds frames * faces * depth images.
        let mut offsets = vec![0usize; mip_count];
        let mut off = data_off;
        for mip in (0..mip_count).rev() {
            let (mw, mh) = mip_dim(width as usize, height as usize, mip);
            offsets[mip] = off;
            off += format.size(mw, mh) * frames as usize * faces * depth;
        }
        if off > b.len() {
            bail!("VTF truncated ({} > {})", off, b.len());
        }
        let mut mips = Vec::with_capacity(mip_count);
        for (mip, &o) in offsets.iter().enumerate() {
            let (mw, mh) = mip_dim(width as usize, height as usize, mip);
            let sz = format.size(mw, mh);
            let rgba = decode(format, &b[o..o + sz], mw, mh)?;
            mips.push(Mip {
                width: mw as u32,
                height: mh as u32,
                rgba,
            });
        }
        Ok(Vtf {
            width,
            height,
            flags,
            frames,
            format,
            reflectivity,
            mips,
        })
    }
}

fn mip_dim(w: usize, h: usize, mip: usize) -> (usize, usize) {
    ((w >> mip).max(1), (h >> mip).max(1))
}

fn rgb565(v: u16) -> [u8; 3] {
    let r = ((v >> 11) & 31) as u32;
    let g = ((v >> 5) & 63) as u32;
    let b = (v & 31) as u32;
    [
        ((r * 255 + 15) / 31) as u8,
        ((g * 255 + 31) / 63) as u8,
        ((b * 255 + 15) / 31) as u8,
    ]
}

fn dxt_color_block(blk: &[u8], one_bit: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([blk[0], blk[1]]);
    let c1 = u16::from_le_bytes([blk[2], blk[3]]);
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [a[0], a[1], a[2], 255];
    pal[1] = [b[0], b[1], b[2], 255];
    if c0 > c1 || !one_bit {
        for i in 0..3 {
            pal[2][i] = ((2 * a[i] as u32 + b[i] as u32) / 3) as u8;
            pal[3][i] = ((a[i] as u32 + 2 * b[i] as u32) / 3) as u8;
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for i in 0..3 {
            pal[2][i] = ((a[i] as u32 + b[i] as u32) / 2) as u8;
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
    let mut out = [[0u8; 4]; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (2 * i)) & 3) as usize];
    }
    out
}

fn decode(fmt: ImageFormat, d: &[u8], w: usize, h: usize) -> Result<Vec<u8>> {
    use ImageFormat::*;
    let mut out = vec![255u8; w * h * 4];
    match fmt {
        Dxt1 | Dxt1OneBitAlpha | Dxt3 | Dxt5 => {
            let bw = w.div_ceil(4).max(1);
            let bh = h.div_ceil(4).max(1);
            let stride = if matches!(fmt, Dxt1 | Dxt1OneBitAlpha) {
                8
            } else {
                16
            };
            for by in 0..bh {
                for bx in 0..bw {
                    let blk = &d[(by * bw + bx) * stride..][..stride];
                    let (color, alpha): ([[u8; 4]; 16], Option<[u8; 16]>) = match fmt {
                        Dxt1 | Dxt1OneBitAlpha => (dxt_color_block(blk, true), None),
                        Dxt3 => {
                            let mut a = [0u8; 16];
                            for i in 0..16 {
                                let n = (blk[i / 2] >> (4 * (i % 2))) & 15;
                                a[i] = n * 17;
                            }
                            (dxt_color_block(&blk[8..], false), Some(a))
                        }
                        _ => {
                            let (a0, a1) = (blk[0] as u32, blk[1] as u32);
                            let mut pal = [0u8; 8];
                            pal[0] = a0 as u8;
                            pal[1] = a1 as u8;
                            if a0 > a1 {
                                for i in 1..7 {
                                    pal[i + 1] = (((7 - i as u32) * a0 + i as u32 * a1) / 7) as u8;
                                }
                            } else {
                                for i in 1..5 {
                                    pal[i + 1] = (((5 - i as u32) * a0 + i as u32 * a1) / 5) as u8;
                                }
                                pal[6] = 0;
                                pal[7] = 255;
                            }
                            let mut bits = 0u64;
                            for i in 0..6 {
                                bits |= (blk[2 + i] as u64) << (8 * i);
                            }
                            let mut a = [0u8; 16];
                            for (i, v) in a.iter_mut().enumerate() {
                                *v = pal[((bits >> (3 * i)) & 7) as usize];
                            }
                            (dxt_color_block(&blk[8..], false), Some(a))
                        }
                    };
                    for py in 0..4 {
                        for px in 0..4 {
                            let (x, y) = (bx * 4 + px, by * 4 + py);
                            if x < w && y < h {
                                let mut c = color[py * 4 + px];
                                if let Some(a) = alpha {
                                    c[3] = a[py * 4 + px];
                                }
                                out[(y * w + x) * 4..][..4].copy_from_slice(&c);
                            }
                        }
                    }
                }
            }
        }
        Rgba8888 => out.copy_from_slice(&d[..w * h * 4]),
        Abgr8888 => {
            for i in 0..w * h {
                let s = &d[i * 4..];
                out[i * 4..][..4].copy_from_slice(&[s[3], s[2], s[1], s[0]]);
            }
        }
        Argb8888 => {
            for i in 0..w * h {
                let s = &d[i * 4..];
                out[i * 4..][..4].copy_from_slice(&[s[1], s[2], s[3], s[0]]);
            }
        }
        Bgra8888 | Bgrx8888 | Uvwq8888 | Uvlx8888 => {
            for i in 0..w * h {
                let s = &d[i * 4..];
                let a = if fmt == Bgra8888 { s[3] } else { 255 };
                out[i * 4..][..4].copy_from_slice(&[s[2], s[1], s[0], a]);
            }
        }
        Rgb888 => {
            for i in 0..w * h {
                out[i * 4..][..3].copy_from_slice(&d[i * 3..][..3]);
            }
        }
        Bgr888 => {
            for i in 0..w * h {
                let s = &d[i * 3..];
                out[i * 4..][..3].copy_from_slice(&[s[2], s[1], s[0]]);
            }
        }
        Rgb565 | Bgr565 => {
            for i in 0..w * h {
                let v = u16::from_le_bytes([d[i * 2], d[i * 2 + 1]]);
                let c = rgb565(v);
                let c = if fmt == Bgr565 { [c[2], c[1], c[0]] } else { c };
                out[i * 4..][..3].copy_from_slice(&c);
            }
        }
        I8 => {
            for i in 0..w * h {
                out[i * 4..][..3].fill(d[i]);
            }
        }
        A8 => {
            for i in 0..w * h {
                out[i * 4..][..3].fill(255);
                out[i * 4 + 3] = d[i];
            }
        }
        Ia88 => {
            for i in 0..w * h {
                out[i * 4..][..3].fill(d[i * 2]);
                out[i * 4 + 3] = d[i * 2 + 1];
            }
        }
        Uv88 => {
            for i in 0..w * h {
                out[i * 4..][..4].copy_from_slice(&[d[i * 2], d[i * 2 + 1], 255, 255]);
            }
        }
        Bgra4444 => {
            for i in 0..w * h {
                let v = u16::from_le_bytes([d[i * 2], d[i * 2 + 1]]);
                let n = |s: u32| (((v >> s) & 15) as u8) * 17;
                out[i * 4..][..4].copy_from_slice(&[n(8), n(4), n(0), n(12)]);
            }
        }
        Bgra5551 | Bgrx5551 => {
            for i in 0..w * h {
                let v = u16::from_le_bytes([d[i * 2], d[i * 2 + 1]]);
                let n = |s: u32| ((((v >> s) & 31) as u32 * 255 + 15) / 31) as u8;
                let a = if fmt == Bgra5551 && v & 0x8000 == 0 {
                    0
                } else {
                    255
                };
                out[i * 4..][..4].copy_from_slice(&[n(10), n(5), n(0), a]);
            }
        }
        Rgba16161616F => {
            for i in 0..w * h {
                for c in 0..4 {
                    let h16 = u16::from_le_bytes([d[i * 8 + c * 2], d[i * 8 + c * 2 + 1]]);
                    let f = half_to_f32(h16).clamp(0.0, 1.0);
                    out[i * 4 + c] = (f.powf(1.0 / 2.2) * 255.0) as u8;
                }
            }
        }
        Rgba16161616 => {
            for i in 0..w * h {
                for c in 0..4 {
                    out[i * 4 + c] = d[i * 8 + c * 2 + 1];
                }
            }
        }
        Unknown => bail!("unknown VTF image format"),
    }
    Ok(out)
}

fn half_to_f32(h: u16) -> f32 {
    let s = ((h >> 15) & 1) as u32;
    let e = ((h >> 10) & 31) as i32;
    let m = (h & 1023) as u32;
    let v = if e == 0 {
        (m as f32) * 2f32.powi(-24)
    } else if e == 31 {
        if m == 0 {
            f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        (1.0 + m as f32 / 1024.0) * 2f32.powi(e - 15)
    };
    if s == 1 {
        -v
    } else {
        v
    }
}
