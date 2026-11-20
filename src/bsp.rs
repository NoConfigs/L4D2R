//! L4D2 BSP faces, material UVs, baked lightmaps and initial entity placements.
use crate::{binary, kv};
use std::{
    collections::{BTreeMap, HashMap},
    io,
};
pub fn load_file(path: &std::path::Path, root: &std::path::Path) -> io::Result<World> {
    parse(&load_bytes(path, root)?)
}
pub fn load_file_mode(path: &std::path::Path, root: &std::path::Path, mode: MapMode) -> io::Result<World> {
    parse(&load_bytes_for_mode(path, root, mode)?)
}
pub fn load_bytes(path: &std::path::Path, root: &std::path::Path) -> io::Result<Vec<u8>> {
    load_bytes_for_mode(path, root, MapMode::Campaign)
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)] // Non-campaign variants support mode-differential tests/loaders.
pub enum MapMode {
    #[default]
    Campaign,
    Survival,
    Scavenge,
    Versus,
}
impl MapMode {
    pub fn output(self) -> &'static str {
        match self {
            Self::Campaign => "OnCoop",
            Self::Survival => "OnSurvival",
            Self::Scavenge => "OnScavenge",
            Self::Versus => "OnVersus",
        }
    }
    fn patch_suffix(self) -> &'static str {
        match self {
            Self::Survival => "h",
            Self::Scavenge => "s",
            _ => "l",
        }
    }
}
pub fn load_bytes_for_mode(
    path: &std::path::Path,
    root: &std::path::Path,
    mode: MapMode,
) -> io::Result<Vec<u8>> {
    let mut bytes = std::fs::read(path)?;
    let name = path
        .file_stem()
        .ok_or_else(|| error("Map filename has no stem"))?
        .to_string_lossy();
    let mut mounts = vec![];
    for mount in [
        "update",
        "left4dead2_dlc3",
        "left4dead2_dlc2",
        "left4dead2_dlc1",
        "left4dead2",
    ] {
        mounts.push(root.join(mount).join("maps"));
    }
    if let Some(parent) = path.parent() {
        mounts.push(parent.to_path_buf());
    }
    for index in 0..64 {
        // L4D2 uses l for ordinary modes, h for holdout/survival, s for scavenge.
        let filename = format!("{name}_{}_{index}.lmp", mode.patch_suffix());
        if let Some(patch) = mounts
            .iter()
            .map(|m| m.join(&filename))
            .find(|p| p.is_file())
        {
            apply_overlay(&mut bytes, &std::fs::read(patch)?)?;
        }
    }
    Ok(bytes)
}
fn apply_overlay(bytes: &mut Vec<u8>, patch: &[u8]) -> io::Result<()> {
    binary::range(bytes, 0, 1036)?;
    binary::range(patch, 0, 20)?;
    let offset = word(patch, 0)? as usize;
    let id = word(patch, 4)? as usize;
    let version = word(patch, 8)?;
    if id >= 64 || offset < 20 {
        return Err(error("Invalid Source lump patch header"));
    }
    let len = word(patch, 12)? as usize;
    let data = binary::range(patch, offset, len)?;
    let at = u32::try_from(bytes.len()).map_err(|_| error("Patched map exceeds 4 GiB"))?;
    bytes.extend_from_slice(data);
    let header = 8 + id * 16;
    bytes[header..header + 4].copy_from_slice(&version.to_le_bytes());
    bytes[header + 4..header + 8].copy_from_slice(&at.to_le_bytes());
    bytes[header + 8..header + 12].copy_from_slice(&(len as u32).to_le_bytes());
    bytes[header + 12..header + 16].fill(0);
    // Retail/community lump patches do not necessarily share the BSP revision.
    Ok(())
}

pub struct World {
    pub positions: Vec<[f32; 3]>,
    pub spawn: [f32; 3],
    pub faces: usize,
    pub surfaces: Vec<Surface>,
    pub props: Vec<Prop>,
    pub brushes: Vec<Brush>,
    pub entities: Vec<BTreeMap<String, String>>,
    pub pak: Vec<u8>,
    /// env_cubemap sample origins (lump 42, Source units); the pakfile holds
    /// each as materials/maps/<map>/c<x>_<y>_<z>[.hdr].vtf.
    pub cubemaps: Vec<[i32; 3]>,
    pub lightmap: LightAtlas,
    pub sky: Option<Sky>,
    /// Entity-lump indices of entities placed in the 3D skybox area. Source
    /// draws those (e.g. c1m1_skybox_particles_*) in the sky pass, scaled.
    pub sky_entities: std::collections::HashSet<usize>,
    pub suns: Vec<Sunlight>,
    pub ambient_field: AmbientField,
    pub local_lights: Vec<LocalLight>,
    pub visibility: crate::visibility::Data,
    /// Faces lit by switchable/animated light styles (crate::light_styles).
    pub light_styles: Vec<StyledFace>,
    /// Detail sprites (game lump `dprp`): the grass tufts and weeds vbsp
    /// scatters over terrain from detail.vbsp (crate::detail_props).
    pub detail_sprites: Vec<DetailSprite>,
}
/// One detail sprite. Renderer space; `rect` is the quad in Source units
/// (left, top, right, bottom around the origin, already scaled), `uv` its
/// rectangle on the map's detail material, `light` its baked linear light.
#[derive(Clone, Debug)]
pub struct DetailSprite {
    pub origin: [f32; 3],
    /// Source yaw (degrees) for fixed-orientation sprites.
    pub yaw: f32,
    /// 0 fixed angles, 1 screen aligned, 2 turns about z toward the viewer.
    pub orientation: u8,
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub light: [f32; 3],
}
/// Detail sprites from the `dprp` game lump (version 4, 52-byte records).
/// Detail models (type 0) and shapes are not read.
fn detail_sprites(b: &[u8]) -> io::Result<Vec<DetailSprite>> {
    let game = lump(b, 35)?;
    if game.is_empty() {
        return Ok(vec![]);
    }
    for i in 0..binary::count(game, 0, 64)? {
        let header = record(&game[4..], i, 16)?;
        if word(header, 0)? != 0x64707270 || short(header, 4)? != 0 || short(header, 6)? != 4 {
            continue;
        }
        let data = binary::range(b, word(header, 8)? as usize, word(header, 12)? as usize)?;
        let models = binary::count(data, 0, 4096)?;
        let mut at = 4 + models * 128;
        let sprites = binary::count(data, at, 4096)?;
        at += 4;
        let dictionary: Vec<[f32; 8]> = (0..sprites)
            .map(|k| -> io::Result<[f32; 8]> {
                let r = record(&data[at..], k, 32)?;
                let mut v = [0.; 8];
                for (j, out) in v.iter_mut().enumerate() {
                    *out = binary::f32_at(r, j * 4)?;
                }
                Ok(v)
            })
            .collect::<io::Result<_>>()?;
        at += sprites * 32;
        let count = binary::count(data, at, 1_000_000)?;
        at += 4;
        let mut out = Vec::with_capacity(count);
        for k in 0..count {
            let r = record(&data[at..], k, 52)?;
            if r[44] != 1 {
                continue;
            }
            let Some(sprite) = dictionary.get(short(r, 24)? as usize) else { continue };
            let scale = binary::f32_at(r, 48)?;
            let scale = if scale.is_finite() && scale > 0. { scale } else { 1. };
            let exponent = r[31] as i8 as i32;
            let light = [r[28], r[29], r[30]].map(|c| c as f32 * 2f32.powi(exponent) / 255.);
            out.push(DetailSprite {
                origin: convert(binary::vec3(r, 0)?),
                yaw: binary::f32_at(r, 16)?,
                orientation: r[40],
                rect: [sprite[0] * scale, sprite[1] * scale, sprite[2] * scale, sprite[3] * scale],
                uv: [sprite[4], sprite[5], sprite[6], sprite[7]],
                light,
            });
        }
        return Ok(out);
    }
    Ok(vec![])
}
/// A lightmapped face with a non-default light style in any of its four
/// style slots (`dface_t::styles`). `maps[k]` is the flat lightmap of style
/// slot k (w*h linear RGB); the atlas tile is the sum of every slot scaled by
/// its live style value.
pub struct StyledFace {
    /// Interior atlas tile: x, y, w, h (bake_atlas adds a 1-texel border).
    pub tile: [usize; 4],
    pub styles: [u8; 4],
    pub maps: Vec<Vec<[f32; 3]>>,
}
pub struct LocalLight {
    pub origin: [f32; 3],
    pub intensity: [f32; 3],
    pub normal: [f32; 3],
    pub spot: bool,
    pub cone: [f32; 2],
    pub exponent: f32,
    pub attenuation: [f32; 3],
    pub radius: f32,
    /// dworldlight_t cluster (-1: none): model lighting only takes lights in
    /// the sample point's PVS, as the engine's light cache does.
    pub cluster: i32,
}
pub struct AmbientField {
    nodes: Vec<u8>,
    planes: Vec<u8>,
    head: i32,
    probes: Vec<(usize, [f32; 3], [[f32; 3]; 6])>,
    /// Each leaf's probe range in `probes` (pushed in leaf order): a sample
    /// scanned every probe in the map (c2m2's reveal: ~40 ms of samples).
    leaf_ranges: Vec<std::ops::Range<usize>>,
    /// Probes by 256-unit cell, for the nearest-probe fallback (a point in a
    /// leaf without probes scanned every probe in the map).
    grid: HashMap<[i32; 3], Vec<usize>>,
}
impl AmbientField {
    fn load(b: &[u8], hdr: bool) -> io::Result<Self> {
        let indices = lump(b, if hdr { 51 } else { 52 })?;
        let samples = lump(b, if hdr { 55 } else { 56 })?;
        let leaves = lump(b, 10)?;
        let mut probes = vec![];
        let mut leaf_ranges = vec![];
        for (leaf_id, index) in indices.chunks_exact(4).enumerate() {
            let start = probes.len();
            let leaf = record(leaves, leaf_id, 32)?;
            let first = short(index, 2)? as usize;
            for i in first..first + short(index, 0)? as usize {
                let sample = record(samples, i, 28)?;
                let mut origin = [0.; 3];
                for axis in 0..3 {
                    let lo = short(leaf, 8 + axis * 2)? as i16 as f32;
                    let hi = short(leaf, 14 + axis * 2)? as i16 as f32;
                    origin[axis] = lo + (hi - lo) * sample[24 + axis] as f32 / 255.;
                }
                let cube = std::array::from_fn(|face| {
                    let p = &sample[face * 4..face * 4 + 4];
                    let scale = 2f32.powi(p[3] as i8 as i32);
                    [p[0], p[1], p[2]].map(|v| v as f32 * scale)
                });
                probes.push((leaf_id, origin, cube));
            }
            leaf_ranges.push(start..probes.len());
        }
        Ok(Self {
            nodes: lump(b, 5)?.to_vec(),
            planes: lump(b, 1)?.to_vec(),
            head: lump(b, 14)?
                .get(36..40)
                .map(|v| i32::from_le_bytes(v.try_into().unwrap()))
                .unwrap_or(-1),
            grid: {
                let mut grid: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
                for (i, (_, o, _)) in probes.iter().enumerate() {
                    grid.entry(o.map(|v| (v / 256.).floor() as i32)).or_default().push(i);
                }
                grid
            },
            probes,
            leaf_ranges,
        })
    }
    pub fn sample(&self, position: [f32; 3]) -> [[f32; 3]; 6] {
        let p = [
            position[0] / 0.0254,
            -position[2] / 0.0254,
            position[1] / 0.0254,
        ];
        let mut node = self.head;
        for _ in 0..1024 {
            if node < 0 {
                break;
            }
            let Ok(n) = record(&self.nodes, node as usize, 32) else {
                break;
            };
            let Ok(plane) = record(&self.planes, word(n, 0).unwrap_or(0) as usize, 20) else {
                break;
            };
            let normal = binary::vec3(plane, 0).unwrap_or([0.; 3]);
            let distance = (0..3).map(|i| normal[i] * p[i]).sum::<f32>()
                - binary::f32_at(plane, 12).unwrap_or(0.);
            node = word(n, if distance > 0. { 4 } else { 8 }).unwrap_or(u32::MAX) as i32;
        }
        let leaf = (-1i64 - node as i64).max(0) as usize;
        let distance = |origin: &[f32; 3]| (0..3).map(|i| (origin[i] - p[i]).powi(2)).sum::<f32>();
        // Stay in the native leaf when possible, avoiding light leaking through walls.
        // A hinge/attachment inside solid geometry uses the closest valid probe.
        let nearest = self
            .leaf_ranges
            .get(leaf)
            .and_then(|range| self.probes.get(range.clone()))
            .unwrap_or(&[])
            .iter()
            .min_by(|a, b| distance(&a.1).total_cmp(&distance(&b.1)))
            .or_else(|| {
                // Expanding rings of grid cells; one more ring after the first
                // hit keeps the nearest exact within a cell.
                let c = p.map(|v| (v / 256.).floor() as i32);
                let mut best: Option<&(usize, [f32; 3], [[f32; 3]; 6])> = None;
                let mut found_at = None;
                for r in 0..16i32 {
                    if found_at.is_some_and(|f: i32| r > f + 1) {
                        break;
                    }
                    for x in -r..=r {
                        for y in -r..=r {
                            for z in -r..=r {
                                if x.abs().max(y.abs()).max(z.abs()) != r {
                                    continue;
                                }
                                if let Some(ids) = self.grid.get(&[c[0] + x, c[1] + y, c[2] + z]) {
                                    for &i in ids {
                                        let probe = &self.probes[i];
                                        if best.is_none_or(|b| distance(&probe.1) < distance(&b.1)) {
                                            best = Some(probe);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if best.is_some() && found_at.is_none() {
                        found_at = Some(r);
                    }
                }
                best.or_else(|| {
                    self.probes
                        .iter()
                        .min_by(|a, b| distance(&a.1).total_cmp(&distance(&b.1)))
                })
            });
        nearest.map(|p| p.2).unwrap_or([[0.5; 3]; 6])
    }
}
pub struct Sunlight {
    pub direction: [f32; 3],
    pub intensity: [f32; 3],
}
pub struct Sky {
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub scale: f32,
    pub area: usize,
}
pub struct Surface {
    pub clusters: Vec<u16>,
    pub sky: bool,
    pub model: usize,
    pub material: String,
    pub positions: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub light_uv: Vec<[f32; 2]>,
    /// Displacement vertex alpha / 255 (WorldVertexTransition blend);
    /// 0 for flat faces.
    pub blend: Vec<f32>,
    /// Bumped lightmaps: the atlas u distance from one basis lightmap to the
    /// next (flat, then bump bases 1-3 side by side); 0 without them.
    pub bump: Vec<f32>,
    /// The texdata size `uv` was divided by. The engine maps by the base
    /// texture's real size instead, which differs when vbsp sized texdata
    /// from a %tooltexture or the texture changed after compile.
    pub texdata_size: [f32; 2],
}
#[derive(Clone)]
pub struct Prop {
    pub entity_index: Option<usize>,
    pub clusters: Vec<u16>,
    pub static_index: Option<usize>,
    pub lighting_flags: u8,
    pub lighting_origin: Option<[f32; 3]>,
    pub ambient: [[f32; 3]; 6],
    pub sky: bool,
    pub model: String,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub skin: usize,
    pub body: usize,
    pub tint: [u8; 4],
    pub classname: String,
}
pub struct Brush {
    pub entity_index: usize,
    pub sky: bool,
    pub model: usize,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub classname: String,
}
pub struct LightAtlas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 4]>,
}
fn error(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn word(b: &[u8], offset: usize) -> io::Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(offset..offset + 4)
            .ok_or_else(|| error("Truncated BSP field"))?
            .try_into()
            .unwrap(),
    ))
}
fn short(b: &[u8], offset: usize) -> io::Result<u16> {
    Ok(u16::from_le_bytes(
        b.get(offset..offset + 2)
            .ok_or_else(|| error("Truncated BSP field"))?
            .try_into()
            .unwrap(),
    ))
}
pub(crate) fn lump(b: &[u8], index: usize) -> io::Result<&[u8]> {
    // L4D2's header orders fields as version, offset, length, fourCC.
    let h = 8 + index * 16;
    let start = word(b, h + 4)? as usize;
    let len = word(b, h + 8)? as usize;
    if word(b, h + 12)? != 0 {
        return Err(error("Compressed BSP lumps are not supported yet"));
    }
    b.get(
        start
            ..start
                .checked_add(len)
                .ok_or_else(|| error("BSP lump overflow"))?,
    )
    .ok_or_else(|| error("BSP lump outside file"))
}
/// (material, SURF_* flags) for every texinfo record.
pub(crate) fn texinfo_materials(b: &[u8]) -> io::Result<Vec<(String, u32)>> {
    let texinfo = lump(b, 6)?;
    (0..texinfo.len() / 72)
        .map(|i| {
            let info = record(texinfo, i, 72)?;
            Ok((texture(b, info)?.0, word(info, 64)?))
        })
        .collect()
}
fn record(b: &[u8], index: usize, size: usize) -> io::Result<&[u8]> {
    let start = index
        .checked_mul(size)
        .ok_or_else(|| error("BSP index overflow"))?;
    b.get(
        start
            ..start
                .checked_add(size)
                .ok_or_else(|| error("BSP record overflow"))?,
    )
    .ok_or_else(|| error("BSP index outside lump"))
}
fn point_leaf(b: &[u8], p: [f32; 3]) -> io::Result<usize> {
    let nodes = lump(b, 5)?;
    let planes = lump(b, 1)?;
    let leaves = lump(b, 10)?;
    if nodes.is_empty() || leaves.is_empty() {
        return Ok(0);
    }
    let mut node = word(record(lump(b, 14)?, 0, 48)?, 36)? as i32;
    for _ in 0..1024 {
        if node < 0 {
            return Ok((-1i64 - node as i64) as usize);
        }
        let n = record(nodes, node as usize, 32)?;
        let plane = record(planes, word(n, 0)? as usize, 20)?;
        let v = binary::vec3(plane, 0)?;
        let d = v[0] * p[0] + v[1] * p[1] + v[2] * p[2] - binary::f32_at(plane, 12)?;
        node = word(n, if d > 0. { 4 } else { 8 })? as i32;
    }
    Err(error("Cyclic or excessively deep BSP tree"))
}
/// Area of a point one unit off a displacement's base face centre.
fn displacement_area(
    b: &[u8],
    face: &[u8],
    surfedges: &[u8],
    edges: &[u8],
    vertices: &[u8],
) -> io::Result<usize> {
    let start = word(face, 4)? as usize;
    let len = short(face, 8)? as usize;
    let mut centre = [0.; 3];
    for i in 0..len {
        let signed = word(record(surfedges, start + i, 4)?, 0)? as i32;
        let edge = record(edges, signed.unsigned_abs() as usize, 4)?;
        let v = binary::vec3(record(vertices, short(edge, if signed < 0 { 2 } else { 0 })? as usize, 12)?, 0)?;
        for axis in 0..3 {
            centre[axis] += v[axis] / len.max(1) as f32;
        }
    }
    let planes = lump(b, 1)?;
    let normal = binary::vec3(record(planes, short(face, 0)? as usize, 20)?, 0)?;
    point_area(b, std::array::from_fn(|axis| centre[axis] + normal[axis]))
}
fn point_area(b: &[u8], p: [f32; 3]) -> io::Result<usize> {
    let leaves = lump(b, 10)?;
    if leaves.is_empty() {
        return Ok(0);
    }
    Ok(short(record(leaves, point_leaf(b, p)?, 32)?, 6)? as usize & 511)
}
fn ambient_cube(b: &[u8], origin: [f32; 3], hdr: bool) -> io::Result<[[f32; 3]; 6]> {
    let indices = lump(b, if hdr { 51 } else { 52 })?;
    let samples = lump(b, if hdr { 55 } else { 56 })?;
    if indices.is_empty() || samples.is_empty() {
        return Ok([[0.5; 3]; 6]);
    }
    let leaf_index = point_leaf(b, origin)?;
    let index = record(indices, leaf_index, 4)?;
    let count = short(index, 0)? as usize;
    if count == 0 {
        return Ok([[0.5; 3]; 6]);
    }
    let first = short(index, 2)? as usize;
    let leaf = record(lump(b, 10)?, leaf_index, 32)?;
    let mut nearest = None;
    let mut nearest_distance = f32::INFINITY;
    for i in first..first + count {
        let sample = record(samples, i, 28)?;
        let mut distance = 0.;
        for axis in 0..3 {
            let lo = short(leaf, 8 + axis * 2)? as i16 as f32;
            let hi = short(leaf, 14 + axis * 2)? as i16 as f32;
            let p = lo + (hi - lo) * sample[24 + axis] as f32 / 255.;
            distance += (p - origin[axis]).powi(2);
        }
        if distance < nearest_distance {
            nearest_distance = distance;
            nearest = Some(sample);
        }
    }
    let sample = nearest.unwrap();
    Ok(std::array::from_fn(|i| {
        let p = &sample[i * 4..i * 4 + 4];
        // Ambient cubes use ColorRGBExp32ToVector, which retains the 255 factor
        // that the lightmap-specific TexLightToLinear conversion divides out.
        let factor = 2f32.powi(p[3] as i8 as i32);
        [
            p[0] as f32 * factor,
            p[1] as f32 * factor,
            p[2] as f32 * factor,
        ]
    }))
}
fn convert(v: [f32; 3]) -> [f32; 3] {
    [v[0] * 0.0254, v[2] * 0.0254, -v[1] * 0.0254]
}
fn portal_cover_targets(
    entities: &[BTreeMap<String, String>],
) -> std::collections::HashSet<String> {
    entities
        .iter()
        .filter(|e| {
            e.get("classname")
                .is_some_and(|c| c == "func_areaportalwindow")
        })
        .filter_map(|e| e.get("target"))
        .map(|name| name.to_ascii_lowercase())
        .collect()
}
fn spawn(entities: &[u8]) -> Option<[f32; 3]> {
    let text = String::from_utf8_lossy(entities);
    for classname in ["info_survivor_position", "info_player_start"] {
        for block in text.split('}') {
            let tokens: Vec<_> = block.split('"').skip(1).step_by(2).collect();
            if !tokens
                .chunks_exact(2)
                .any(|p| p[0] == "classname" && p[1] == classname)
            {
                continue;
            }
            if let Some(pair) = tokens.chunks_exact(2).find(|p| p[0] == "origin") {
                let v: Vec<f32> = pair[1]
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if v.len() == 3 && v.iter().all(|v| v.is_finite()) {
                    let mut p = convert([v[0], v[1], v[2]]);
                    p[1] += 1.65;
                    return Some(p);
                }
            }
        }
    }
    None
}
fn texture(b: &[u8], info: &[u8]) -> io::Result<(String, [f32; 2])> {
    let data = lump(b, 2)?;
    if data.is_empty() {
        return Ok(("__missing".into(), [1., 1.]));
    }
    let index = word(info, 68)? as usize;
    let data = record(data, index, 32)?;
    let table = lump(b, 44)?;
    let name_index = word(data, 12)? as usize;
    let string_offset = word(record(table, name_index, 4)?, 0)? as usize;
    let name = binary::cstring(lump(b, 43)?, string_offset)?;
    let size = [word(data, 16)?.max(1) as f32, word(data, 20)?.max(1) as f32];
    Ok((name.replace('\\', "/").to_lowercase(), size))
}
fn project(info: &[u8], offset: usize, p: [f32; 3]) -> io::Result<[f32; 2]> {
    let mut uv = [0.; 2];
    for (axis, out) in uv.iter_mut().enumerate() {
        let o = offset + axis * 16;
        *out = binary::f32_at(info, o)? * p[0]
            + binary::f32_at(info, o + 4)? * p[1]
            + binary::f32_at(info, o + 8)? * p[2]
            + binary::f32_at(info, o + 12)?;
    }
    Ok(uv)
}
/// Styled faces for crate::light_styles. Each style slot's block is one flat
/// lightmap, or four (flat + three bump directions) on SURF_BUMPLIGHT faces;
/// only the flat one is used, as for style 0.
fn styled_faces(faces: &[u8], texinfo: &[u8], light: &[u8], tiles: &HashMap<usize, [usize; 4]>) -> Vec<StyledFace> {
    const SURF_BUMPLIGHT: u32 = 0x800;
    let mut out = vec![];
    for (index, face) in faces.chunks_exact(56).enumerate() {
        let styles = [face[16], face[17], face[18], face[19]];
        if styles.iter().all(|&s| s == 0 || s == 255) {
            continue;
        }
        let Some(&[x, y, w, h]) = tiles.get(&index) else { continue };
        let Ok(offset) = word(face, 20) else { continue };
        let tex = i16::from_le_bytes([face[10], face[11]]);
        let bump = tex >= 0
            && texinfo
                .get(tex as usize * 72 + 64..tex as usize * 72 + 68)
                .is_some_and(|f| u32::from_le_bytes(f.try_into().unwrap()) & SURF_BUMPLIGHT != 0);
        let per_style = w * h * if bump { 4 } else { 1 };
        let mut maps = vec![];
        for k in 0..4 {
            if styles[k] == 255 {
                break;
            }
            let start = offset as usize + k * per_style * 4;
            let Some(block) = light.get(start..start + w * h * 4) else { break };
            maps.push(
                block
                    .chunks_exact(4)
                    .map(|s| {
                        let factor = 2f32.powi(s[3] as i8 as i32) / 255.;
                        [s[0] as f32 * factor, s[1] as f32 * factor, s[2] as f32 * factor]
                    })
                    .collect(),
            );
        }
        out.push(StyledFace { tile: [x, y, w, h], styles, maps });
    }
    out
}
fn bake_atlas(faces: &[u8], light: &[u8]) -> io::Result<(LightAtlas, HashMap<usize, [usize; 4]>)> {
    bake_atlas_bumped(faces, light, &std::collections::HashSet::new())
}
/// The lightmap atlas. Faces in `bumped` (SURF_BUMPLIGHT with lightmap
/// data) carry four lightmaps, flat then the three bump bases; their tile
/// reserves four (w + 2)-wide slots side by side, the flat one first, so a
/// basis is the flat texel plus k slot widths.
fn bake_atlas_bumped(
    faces: &[u8],
    light: &[u8],
    bumped: &std::collections::HashSet<usize>,
) -> io::Result<(LightAtlas, HashMap<usize, [usize; 4]>)> {
    for width in [2048, 4096, 8192] {
        match bake_atlas_width(faces, light, bumped, width) {
            Err(e) if e.to_string().contains("exceeds") && width < 8192 => continue,
            result => return result,
        }
    }
    unreachable!()
}
fn bake_atlas_width(
    faces: &[u8],
    light: &[u8],
    bumped: &std::collections::HashSet<usize>,
    width: usize,
) -> io::Result<(LightAtlas, HashMap<usize, [usize; 4]>)> {
    let mut x = 2;
    let mut y = 0;
    let mut row = 2;
    let mut tiles = HashMap::new();
    for (index, face) in faces.chunks_exact(56).enumerate() {
        let offset = word(face, 20)? as i32;
        if offset < 0 || light.is_empty() {
            continue;
        }
        let w = word(face, 36)? as i32 + 1;
        let h = word(face, 40)? as i32 + 1;
        if w <= 0 || h <= 0 || w > 1024 || h > 1024 {
            return Err(error("Invalid BSP lightmap dimensions"));
        }
        let (w, h) = (w as usize, h as usize);
        let maps = if bumped.contains(&index) { 4 } else { 1 };
        binary::range(light, offset as usize, w * h * 4 * maps)?;
        let span = (w + 2) * maps;
        if x + span > width {
            x = 0;
            y += row;
            row = 0;
        }
        if y + h + 2 > 8192 {
            return Err(error("BSP lightmap atlas exceeds 8192 pixels"));
        }
        tiles.insert(index, [x + 1, y + 1, w, h]);
        x += span;
        row = row.max(h + 2);
    }
    let height = (y + row).max(2).next_power_of_two();
    let mut pixels = vec![[1., 1., 1., 1.]; width * height];
    for (&index, &[x, y, w, h]) in &tiles {
        let face = record(faces, index, 56)?;
        let offset = word(face, 20)? as usize;
        let maps = if bumped.contains(&index) { 4 } else { 1 };
        for map in 0..maps {
            let source = offset + map * w * h * 4;
            let shift = map * (w + 2);
            for py in 0..h + 2 {
                for px in 0..w + 2 {
                    let sx = px.saturating_sub(1).min(w - 1);
                    let sy = py.saturating_sub(1).min(h - 1);
                    let sample = binary::range(light, source + (sy * w + sx) * 4, 4)?;
                    let factor = 2f32.powi(sample[3] as i8 as i32) / 255.;
                    pixels[(y + py - 1) * width + x + px - 1 + shift] = [
                        sample[0] as f32 * factor,
                        sample[1] as f32 * factor,
                        sample[2] as f32 * factor,
                        1.,
                    ];
                }
            }
        }
    }
    Ok((
        LightAtlas {
            width,
            height,
            pixels,
        },
        tiles,
    ))
}
fn static_props(b: &[u8]) -> io::Result<Vec<Prop>> {
    let game = lump(b, 35)?;
    if game.is_empty() {
        return Ok(vec![]);
    }
    let mut result = vec![];
    for i in 0..binary::count(game, 0, 64)? {
        let header = record(&game[4..], i, 16)?;
        if word(header, 0)? != 0x73707270 {
            continue;
        }
        if short(header, 4)? != 0 {
            return Err(error("Compressed static-prop game lump"));
        }
        let version = short(header, 6)?;
        let stride = match version {
            4 => 56,
            5 => 60,
            6 => 64,
            7 | 8 => 68,
            9 => 72,
            _ => return Err(error(&format!("Unsupported static-prop version {version}"))),
        };
        let data = binary::range(b, word(header, 8)? as usize, word(header, 12)? as usize)?;
        let names = binary::count(data, 0, 65536)?;
        let mut models = vec![];
        for n in 0..names {
            let r = record(&data[4..], n, 128)?;
            models.push(
                String::from_utf8_lossy(&r[..r.iter().position(|c| *c == 0).unwrap_or(128)])
                    .replace('\\', "/")
                    .to_lowercase(),
            );
        }
        let leaves = 4 + names * 128;
        let offset = leaves + 4 + binary::count(data, leaves, 2_000_000)? * 2;
        let count = binary::count(data, offset, 100_000)?;
        binary::range(data, offset + 4, count * stride)?;
        for n in 0..count {
            let p = record(&data[offset + 4..], n, stride)?;
            let model = models
                .get(short(p, 24)? as usize)
                .ok_or_else(|| error("Static prop model index outside dictionary"))?
                .clone();
            let tint = if version >= 7 {
                binary::range(p, 64, 4)?.try_into().unwrap()
            } else {
                [255; 4]
            };
            result.push(Prop {
                entity_index: None,
                clusters: {
                    let leaf_data = lump(b, 10)?;
                    let first = short(p, 26)? as usize;
                    let count = short(p, 28)? as usize;
                    let mut clusters = vec![];
                    for index in first..first + count {
                        let leaf = short(data, leaves + 4 + index * 2)? as usize;
                        let cluster = short(record(leaf_data, leaf, 32)?, 4)? as i16;
                        if cluster >= 0 {
                            clusters.push(cluster as u16);
                        }
                    }
                    clusters.sort_unstable();
                    clusters.dedup();
                    clusters
                },
                static_index: Some(n),
                lighting_flags: p[31],
                lighting_origin: if p[31] & 2 != 0 {
                    Some(binary::vec3(p, 44)?)
                } else {
                    None
                },
                ambient: [[0.5; 3]; 6],
                sky: false,
                model,
                origin: binary::vec3(p, 0)?,
                angles: binary::vec3(p, 12)?,
                skin: word(p, 32)? as usize,
                body: 0,
                tint,
                classname: "prop_static".into(),
            });
        }
    }
    Ok(result)
}
fn displacement_grid(
    info: &[u8],
    verts: &[u8],
    polygon: &[[f32; 3]],
) -> io::Result<(usize, Vec<([f32; 3], [f32; 3], [f32; 2])>)> {
    Ok(displacement_grid_alpha(info, verts, polygon)?.0)
}
/// The grid plus each vertex's alpha (CDispVert::m_flAlpha, 0..255).
fn displacement_grid_alpha(
    info: &[u8],
    verts: &[u8],
    polygon: &[[f32; 3]],
) -> io::Result<((usize, Vec<([f32; 3], [f32; 3], [f32; 2])>), Vec<f32>)> {
    if polygon.len() != 4 {
        return Err(error("Displacement base is not a quad"));
    }
    let power = word(info, 20)?;
    if !(2..=4).contains(&power) {
        return Err(error("Invalid displacement power"));
    }
    let start = binary::vec3(info, 0)?;
    let distance = |p: &[f32; 3]| (0..3).map(|i| (p[i] - start[i]).powi(2)).sum::<f32>();
    let corner = (0..4)
        .min_by(|&a, &b| distance(&polygon[a]).total_cmp(&distance(&polygon[b])))
        .unwrap();
    let q: Vec<_> = (0..4).map(|i| polygon[(corner + i) % 4]).collect();
    let cells = 1usize << power;
    let first = word(info, 12)? as usize;
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
        std::array::from_fn::<_, 3, _>(|i| a[i] + (b[i] - a[i]) * t)
    };
    let mut grid = Vec::with_capacity((cells + 1).pow(2));
    let mut alphas = Vec::with_capacity((cells + 1).pow(2));
    for y in 0..=cells {
        for x in 0..=cells {
            let u = x as f32 / cells as f32;
            let v = y as f32 / cells as f32;
            let base = lerp(lerp(q[0], q[3], u), lerp(q[1], q[2], u), v);
            let vertex = record(verts, first + y * (cells + 1) + x, 20)?;
            let direction = binary::vec3(vertex, 0)?;
            let amount = binary::f32_at(vertex, 12)?;
            alphas.push((binary::f32_at(vertex, 16)? / 255.).clamp(0., 1.));
            grid.push((
                std::array::from_fn(|i| base[i] + direction[i] * amount),
                base,
                [u, v],
            ));
        }
    }
    Ok(((cells, grid), alphas))
}
fn oriented_triangle(
    mut triangle: [([f32; 3], [f32; 3], [f32; 2]); 3],
    normal: [f32; 3],
) -> [([f32; 3], [f32; 3], [f32; 2]); 3] {
    let a = triangle[0].0;
    let b = triangle[1].0;
    let c = triangle[2].0;
    let u: [f32; 3] = std::array::from_fn(|i| b[i] - a[i]);
    let v: [f32; 3] = std::array::from_fn(|i| c[i] - a[i]);
    let cross = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    if (0..3).map(|i| cross[i] * normal[i]).sum::<f32>() < 0. {
        triangle.swap(1, 2);
    }
    triangle
}
/// `Surface::texdata_size` of an overlay: its `uv` is already normalised
/// (overlay U/V ranges), so `viewer::world_uv` must not rescale it.
pub const OVERLAY_UV: [f32; 2] = [0., 0.];
/// Lift off the wall (Source units) so the overlay does not z-fight it.
const OVERLAY_LIFT: f32 = 0.1;
/// Compiled info_overlay quads (lump 45, doverlay_t: 352 bytes) as world
/// surfaces: posters, signs, graffiti and stains were not drawn at all. The
/// quad is origin + x U + y V (U packed in the UV points' z, V = normal x U),
/// textured over the U/V ranges and lit through its first target face's
/// lightmap projection, so it lights exactly like the wall under it.
/// Not reproduced: clipping to the target faces (a quad wrapping an outside
/// corner overhangs it) and overlays on displacements (skipped).
fn overlays(
    b: &[u8],
    faces: &[u8],
    texinfo: &[u8],
    tiles: &HashMap<usize, [usize; 4]>,
    atlas: [f32; 2],
    visibility: &crate::visibility::Data,
) -> io::Result<Vec<Surface>> {
    let data = lump(b, 45)?;
    // One draw per (material, visibility clusters), like world faces.
    let mut out: BTreeMap<(String, Vec<u16>), Surface> = BTreeMap::new();
    for o in data.chunks_exact(352) {
        let tex = short(o, 4)? as i16;
        let count = (short(o, 6)? & 0x3fff) as usize;
        if tex < 0 || count == 0 {
            continue;
        }
        let first = word(o, 8)? as usize;
        let Ok(face) = record(faces, first, 56) else { continue };
        let on_displacement = short(face, 12)? as i16 >= 0;
        let points: Vec<[f32; 3]> = (0..4).map(|i| binary::vec3(o, 280 + i * 12)).collect::<io::Result<_>>()?;
        let origin = binary::vec3(o, 328)?;
        let n = binary::vec3(o, 340)?;
        let u = [points[0][2], points[1][2], points[2][2]];
        let v = [n[1] * u[2] - n[2] * u[1], n[2] * u[0] - n[0] * u[2], n[0] * u[1] - n[1] * u[0]];
        if u.iter().chain(&v).chain(&n).any(|x| !x.is_finite()) || u == [0.; 3] {
            continue;
        }
        let corners: Vec<[f32; 3]> = points
            .iter()
            .map(|p| std::array::from_fn(|i| origin[i] + p[0] * u[i] + p[1] * v[i] + n[i] * OVERLAY_LIFT))
            .collect();
        let (fu, fv) = ([binary::f32_at(o, 264)?, binary::f32_at(o, 268)?], [binary::f32_at(o, 272)?, binary::f32_at(o, 276)?]);
        let tex_uv = [[fu[0], fv[0]], [fu[0], fv[1]], [fu[1], fv[1]], [fu[1], fv[0]]];
        let targets: Vec<usize> = (0..count.min(64))
            .map(|i| word(o, 8 + i * 4).map(|v| v as usize))
            .collect::<io::Result<_>>()?;
        // A corner takes the lightmap of the listed face whose luxel extent
        // holds it: the first listed face can be a thin strip (c1m3's Gibbs
        // posters list a 15x2-luxel brick strip first, and sampling outside
        // it read neighbouring atlas tiles, so the posters glowed). Failing
        // that, the first face with a lightmap, clamped into its own tile.
        let light_uv = |p: [f32; 3]| -> io::Result<[f32; 2]> {
            let mut fallback = None;
            for &id in &targets {
                let (Ok(f), Some(&[x, y, _, _])) = (record(faces, id, 56), tiles.get(&id)) else { continue };
                let Ok(info) = record(texinfo, short(f, 10)? as usize, 72) else { continue };
                let l = project(info, 32, p)?;
                let st = [l[0] - word(f, 28)? as i32 as f32, l[1] - word(f, 32)? as i32 as f32];
                let size = [word(f, 36)? as i32 as f32, word(f, 40)? as i32 as f32];
                let inside = (0..2).all(|i| st[i] >= -0.5 && st[i] <= size[i] + 0.5);
                let uv = |st: [f32; 2]| [(st[0] + x as f32 + 0.5) / atlas[0], (st[1] + y as f32 + 0.5) / atlas[1]];
                if inside {
                    return Ok(uv(st));
                }
                fallback.get_or_insert(uv([st[0].clamp(0., size[0]), st[1].clamp(0., size[1])]));
            }
            Ok(fallback.unwrap_or([0.5 / atlas[0], 0.5 / atlas[1]]))
        };
        let (material, _) = texture(b, record(texinfo, tex as usize, 72)?)?;
        let clusters = visibility.clusters_for_face(first);
        if on_displacement {
            // Terrain: an 8x8 grid over the quad dropped onto the target
            // displacements along the overlay normal, lit through the hit
            // triangle's displacement lightmap coordinates (c5m3 had 177 of
            // 326 overlays on displacements).
            let Some(draped) = displacement_overlay(b, faces, &targets, &corners, &tex_uv, n, tiles, atlas)? else {
                continue;
            };
            let surface = out.entry((material.clone(), clusters.clone())).or_insert_with(|| Surface {
                clusters,
                model: 0,
                sky: false,
                material,
                positions: vec![],
                uv: vec![],
                light_uv: vec![],
                blend: vec![],
                bump: vec![],
                texdata_size: OVERLAY_UV,
            });
            for (p, uv, luv) in draped {
                surface.positions.push(convert(p));
                surface.uv.push(uv);
                surface.light_uv.push(luv);
                surface.blend.push(0.);
            }
            continue;
        }
        let surface = out.entry((material.clone(), clusters.clone())).or_insert_with(|| Surface {
            clusters,
            model: 0,
            sky: false,
            material,
            positions: vec![],
            uv: vec![],
            light_uv: vec![],
            blend: vec![],
            bump: vec![],
            texdata_size: OVERLAY_UV,
        });
        for tri in [[0, 1, 2], [0, 2, 3]] {
            let triangle = oriented_triangle(tri.map(|i| (corners[i], corners[i], tex_uv[i])), n);
            for (p, _, uv) in triangle {
                surface.positions.push(convert(p));
                surface.uv.push(uv);
                surface.light_uv.push(light_uv(p)?);
                surface.blend.push(0.);
            }
        }
    }
    Ok(out.into_values().collect())
}
/// The triangles of an overlay draped over its target displacements:
/// (position, texture uv, lightmap uv) per vertex, three per triangle, or
/// None when no grid point lands on them.
#[allow(clippy::too_many_arguments)]
fn displacement_overlay(
    b: &[u8],
    faces: &[u8],
    targets: &[usize],
    corners: &[[f32; 3]],
    tex_uv: &[[f32; 2]; 4],
    n: [f32; 3],
    tiles: &HashMap<usize, [usize; 4]>,
    atlas: [f32; 2],
) -> io::Result<Option<Vec<([f32; 3], [f32; 2], [f32; 2])>>> {
    const STEPS: usize = 8;
    const REACH: f32 = 64.;
    let (vertices, edges, surfedges) = (lump(b, 3)?, lump(b, 12)?, lump(b, 13)?);
    // (corner positions, lightmap uv per corner) per displacement triangle.
    let mut triangles: Vec<([[f32; 3]; 3], [[f32; 2]; 3])> = vec![];
    for &id in targets {
        let Ok(face) = record(faces, id, 56) else { continue };
        let disp = short(face, 12)? as i16;
        let Some(&[x, y, w, h]) = tiles.get(&id) else { continue };
        if disp < 0 {
            continue;
        }
        let start = word(face, 4)? as usize;
        let len = short(face, 8)? as usize;
        let mut polygon = vec![];
        for i in 0..len {
            let signed = word(record(surfedges, start + i, 4)?, 0)? as i32;
            let edge = record(edges, signed.unsigned_abs() as usize, 4)?;
            let vertex = short(edge, if signed < 0 { 2 } else { 0 })? as usize;
            polygon.push(binary::vec3(record(vertices, vertex, 12)?, 0)?);
        }
        let Ok(((cells, grid), _)) =
            displacement_grid_alpha(record(lump(b, 26)?, disp as usize, 176)?, lump(b, 33)?, &polygon)
        else {
            continue;
        };
        let luv = |g: [f32; 2]| {
            [
                (g[0] * (w - 1) as f32 + x as f32 + 0.5) / atlas[0],
                (g[1] * (h - 1) as f32 + y as f32 + 0.5) / atlas[1],
            ]
        };
        for gy in 0..cells {
            for gx in 0..cells {
                let a = gy * (cells + 1) + gx;
                let d = a + cells + 1;
                for t in [[a, d, a + 1], [d, d + 1, a + 1]] {
                    triangles.push((t.map(|i| grid[i].0), t.map(|i| luv(grid[i].2))));
                }
            }
        }
    }
    if triangles.is_empty() {
        return Ok(None);
    }
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
    };
    let down = [-n[0], -n[1], -n[2]];
    // Drop a point onto the nearest displacement triangle along -normal.
    let drop = |p: [f32; 3]| -> Option<([f32; 3], [f32; 2])> {
        let origin = [p[0] + n[0] * REACH, p[1] + n[1] * REACH, p[2] + n[2] * REACH];
        let mut best: Option<(f32, [f32; 3], [f32; 2])> = None;
        for (tri, luv) in &triangles {
            let (e1, e2) = (sub(tri[1], tri[0]), sub(tri[2], tri[0]));
            let pv = cross(down, e2);
            let det = dot(e1, pv);
            if det.abs() < 1e-6 {
                continue;
            }
            let tv = sub(origin, tri[0]);
            let u = dot(tv, pv) / det;
            let qv = cross(tv, e1);
            let v = dot(down, qv) / det;
            if !(-1e-4..=1. + 1e-4).contains(&u) || v < -1e-4 || u + v > 1. + 1e-4 {
                continue;
            }
            let t = dot(e2, qv) / det;
            if !(0. ..=2. * REACH).contains(&t) || best.is_some_and(|(bt, _, _)| bt <= t) {
                continue;
            }
            let w0 = 1. - u - v;
            let hit = std::array::from_fn(|i| origin[i] + down[i] * t + n[i] * OVERLAY_LIFT);
            let l = [
                w0 * luv[0][0] + u * luv[1][0] + v * luv[2][0],
                w0 * luv[0][1] + u * luv[1][1] + v * luv[2][1],
            ];
            best = Some((t, hit, l));
        }
        best.map(|(_, p, l)| (p, l))
    };
    let lerp2 = |a: [f32; 2], b: [f32; 2], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let lerp3 = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t) };
    // Corners 0-1-2-3 are (U0,V0) (U0,V1) (U1,V1) (U1,V0): s runs 0->3, t 0->1.
    let point = |s: f32, t: f32| {
        let p = lerp3(lerp3(corners[0], corners[3], s), lerp3(corners[1], corners[2], s), t);
        let uv = lerp2(lerp2(tex_uv[0], tex_uv[3], s), lerp2(tex_uv[1], tex_uv[2], s), t);
        (drop(p), uv)
    };
    let mut grid = vec![];
    for j in 0..=STEPS {
        for i in 0..=STEPS {
            grid.push(point(i as f32 / STEPS as f32, j as f32 / STEPS as f32));
        }
    }
    let mut out = vec![];
    for j in 0..STEPS {
        for i in 0..STEPS {
            let a = j * (STEPS + 1) + i;
            for tri in [[a, a + STEPS + 1, a + 1], [a + STEPS + 1, a + STEPS + 2, a + 1]] {
                let vs: Vec<([f32; 3], [f32; 2], [f32; 2])> = tri
                    .iter()
                    .filter_map(|&k| grid[k].0.map(|(p, l)| (p, grid[k].1, l)))
                    .collect();
                if vs.len() != 3 {
                    continue;
                }
                let triangle = oriented_triangle([0, 1, 2].map(|k| (vs[k].0, vs[k].0, vs[k].1)), n);
                for (p, _, uv) in triangle {
                    let l = vs.iter().find(|v| v.0 == p).map_or(vs[0].2, |v| v.2);
                    out.push((p, uv, l));
                }
            }
        }
    }
    Ok((!out.is_empty()).then_some(out))
}
pub fn parse(b: &[u8]) -> io::Result<World> {
    if b.get(..4) != Some(b"VBSP") || word(b, 4)? != 21 || b.len() < 1036 {
        return Err(error("Expected an L4D2 version 21 BSP"));
    }
    let vertices = lump(b, 3)?;
    let edges = lump(b, 12)?;
    let surfedges = lump(b, 13)?;
    let texinfo = lump(b, 6)?;
    let hdr_faces = lump(b, 58)?;
    let hdr_light = lump(b, 53)?;
    let hdr = !hdr_faces.is_empty() && !hdr_light.is_empty();
    let faces = if hdr { hdr_faces } else { lump(b, 7)? };
    let light = if hdr { hdr_light } else { lump(b, 8)? };
    let light_index = if hdr { 54 } else { 15 };
    let lights = lump(b, light_index)?;
    let version = word(b, 8 + light_index * 16)?;
    let (stride, extra) = match version {
        0 => (88, 0),
        1 => (100, 12),
        _ => return Err(error("Unsupported world-light version")),
    };
    if lights.len() % stride != 0 {
        return Err(error("Misaligned world lights"));
    }
    let mut suns = vec![];
    let mut local_lights = vec![];
    for light in lights.chunks_exact(stride) {
        if word(light, 40 + extra)? == 3 {
            let n = binary::vec3(light, 24)?;
            suns.push(Sunlight {
                direction: binary::source_normal(n.map(|v| -v)),
                intensity: binary::vec3(light, 12)?,
            });
        }
    }
    // Brightest skylight first: callers take `suns.first()` as the map's sun.
    // c1m1 lists a dim near-vertical fill skylight (0.03) before its real
    // light_environment sun (1.2, pitch -14), so models lost their sun.
    let luminance = |s: &Sunlight| s.intensity[0] * 0.2125 + s.intensity[1] * 0.7154 + s.intensity[2] * 0.0721;
    suns.sort_by(|a, b| luminance(b).total_cmp(&luminance(a)));
    for light in lights.chunks_exact(stride) {
        let kind = word(light, 40 + extra)?;
        if matches!(kind, 1 | 2) && word(light, 44 + extra)? == 0 {
            local_lights.push(LocalLight {
                origin: binary::source_position(binary::vec3(light, 0)?),
                intensity: binary::vec3(light, 12)?,
                normal: binary::source_normal(binary::vec3(light, 24)?),
                spot: kind == 2,
                cone: [
                    binary::f32_at(light, 48 + extra)?,
                    binary::f32_at(light, 52 + extra)?,
                ],
                exponent: binary::f32_at(light, 56 + extra)?,
                radius: binary::f32_at(light, 60 + extra)?,
                attenuation: binary::vec3(light, 64 + extra)?,
                cluster: word(light, 36 + extra)? as i32,
            });
        }
    }
    let models = lump(b, 14)?;
    for (bytes, size) in [
        (vertices, 12),
        (edges, 4),
        (surfedges, 4),
        (texinfo, 72),
        (faces, 56),
        (models, 48),
    ] {
        if bytes.len() % size != 0 {
            return Err(error("Misaligned BSP records"));
        }
    }
    let entities = kv::entities(&String::from_utf8_lossy(lump(b, 0)?));
    let sky = entities
        .iter()
        .find(|e| e.get("classname").is_some_and(|c| c == "sky_camera"))
        .map(|e| {
            let origin = kv::vector(e.get("origin"));
            Ok::<_, io::Error>(Sky {
                origin,
                // Retail never rotates the 3D skybox: sky_camera's use_angles
                // only aims its fog direction (server CSkyCamera datadesc,
                // m_bUseAngles beside m_skyboxData.fog.*; there is no
                // m_skybox3d angle netprop). Hotel's "0 180 0" camera had
                // turned the skyline round behind the hallway windows.
                angles: [0.; 3],
                scale: e
                    .get("scale")
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite() && *v > 0. && *v <= 128.)
                    .unwrap_or(16.),
                area: point_area(b, origin)?,
            })
        })
        .transpose()?;
    let mut sky_faces = std::collections::HashSet::new();
    if let Some(sky) = &sky {
        let leaves = lump(b, 10)?;
        let leaf_faces = lump(b, 16)?;
        for leaf in leaves.chunks_exact(32) {
            if short(leaf, 6)? as usize & 511 != sky.area {
                continue;
            }
            let first = short(leaf, 20)? as usize;
            let n = short(leaf, 22)? as usize;
            for i in first..first + n {
                sky_faces.insert(short(record(leaf_faces, i, 2)?, 0)? as usize);
            }
        }
    }
    let glow_targets: std::collections::HashSet<String> =
        l4d2_formats::bsp::parse_entities(&String::from_utf8_lossy(lump(b, 0)?))
            .iter()
            .flat_map(|e| e.0.iter())
            .filter_map(|(key, value)| l4d2_formats::vmf::Connection::parse(key, value))
            .filter(|c| c.input.eq_ignore_ascii_case("StartGlowing"))
            .map(|c| c.target.to_ascii_lowercase())
            .collect();
    let mut props = static_props(b)?;
    let mut brushes = vec![];
    // Portal windows use linked func_brush covers as distant visibility fillers.
    // We render the full map without Source PVS, so those covers must stay hidden.
    let portal_covers = portal_cover_targets(&entities);
    for (entity_index, e) in entities.iter().enumerate() {
        let Some(model) = e.get("model") else {
            continue;
        };
        let class = e.get("classname").cloned().unwrap_or_default();
        if class == "func_brush"
            && e.get("targetname")
                .is_some_and(|n| portal_covers.contains(&n.to_ascii_lowercase()))
        {
            continue;
        }
        let disabled = e.get("rendermode").is_some_and(|s| s == "10")
            || (e.get("renderamt").is_some_and(|s| s == "0")
                && e.get("rendermode").is_some_and(|s| s != "0"));
        if disabled
            && !e
                .get("targetname")
                .is_some_and(|n| glow_targets.contains(&n.to_ascii_lowercase()))
        {
            continue;
        }
        if model.starts_with("models/") && class.starts_with("prop_") {
            let rgb = kv::vector(e.get("rendercolor"));
            let tint = if e.contains_key("rendercolor") {
                [
                    rgb[0].clamp(0., 255.) as u8,
                    rgb[1].clamp(0., 255.) as u8,
                    rgb[2].clamp(0., 255.) as u8,
                    255,
                ]
            } else {
                [255; 4]
            };
            props.push(Prop {
                entity_index: Some(entity_index),
                clusters: vec![],
                static_index: None,
                lighting_flags: 0,
                lighting_origin: None,
                ambient: [[0.5; 3]; 6],
                sky: false,
                model: model.to_lowercase(),
                origin: kv::vector(e.get("origin")),
                angles: kv::vector(e.get("angles")),
                skin: e.get("skin").and_then(|s| s.parse().ok()).unwrap_or(0),
                body: e.get("body").and_then(|s| s.parse().ok()).unwrap_or(0),
                tint,
                classname: class,
            });
        } else if let Some(id) = model
            .strip_prefix('*')
            .and_then(|s| s.parse::<usize>().ok())
        {
            if class.starts_with("func_")
                && !matches!(
                    class.as_str(),
                    "func_areaportal"
                        | "func_areaportalwindow"
                        | "func_nav_blocker"
                        | "func_nav_attribute_region"
                        | "func_precipitation"
                )
            {
                brushes.push(Brush {
                    entity_index,
                    sky: false,
                    model: id,
                    origin: kv::vector(e.get("origin")),
                    angles: kv::vector(e.get("angles")),
                    classname: class,
                });
            }
        }
    }
    for prop in &mut props {
        prop.ambient = ambient_cube(b, prop.origin, hdr)?;
        if let Some(sky) = &sky {
            prop.sky = point_area(b, prop.origin)? == sky.area;
        }
    }
    if let Some(sky) = &sky {
        for brush in &mut brushes {
            brush.sky = point_area(b, brush.origin)? == sky.area;
        }
    }
    let mut sky_entities = std::collections::HashSet::new();
    if let Some(sky) = &sky {
        for (index, e) in entities.iter().enumerate() {
            if e.get("classname").is_some_and(|c| c == "info_particle_system")
                && e.contains_key("origin")
                && point_area(b, kv::vector(e.get("origin")))? == sky.area
            {
                sky_entities.insert(index);
            }
        }
    }
    let bumped: std::collections::HashSet<usize> = faces
        .chunks_exact(56)
        .enumerate()
        .filter(|(_, f)| {
            word(f, 20).is_ok_and(|o| o as i32 >= 0)
                && short(f, 10)
                    .ok()
                    .filter(|t| *t as i16 >= 0)
                    .and_then(|t| record(texinfo, t as usize, 72).ok())
                    .and_then(|info| word(info, 64).ok())
                    .is_some_and(|flags| flags & 0x800 != 0)
        })
        .map(|(i, _)| i)
        .collect();
    let (lightmap, tiles) = bake_atlas_bumped(faces, light, &bumped)?;
    let light_styles = styled_faces(faces, texinfo, light, &tiles);
    let visibility = crate::visibility::Data::parse(b, faces.len() / 56)?;
    let mut groups: BTreeMap<(usize, String, bool, Vec<u16>), Surface> = BTreeMap::new();
    let mut positions = vec![];
    let mut rendered = 0;
    for (model_index, model) in models.chunks_exact(48).enumerate() {
        let first = word(model, 40)? as usize;
        let count = word(model, 44)? as usize;
        for index in first
            ..first
                .checked_add(count)
                .ok_or_else(|| error("BSP face count overflow"))?
        {
            let face = record(faces, index, 56)?;
            let tex = short(face, 10)? as i16;
            if tex < 0 {
                continue;
            }
            let info = record(texinfo, tex as usize, 72)?;
            if word(info, 64)? & (0x80 | 0x40 | 0x4 | 0x2) != 0 {
                continue;
            }
            let start = word(face, 4)? as i32;
            let len = short(face, 8)? as usize;
            if start < 0 {
                return Err(error("Negative surfedge start"));
            }
            let (material, size) = texture(b, info)?;
            // Displacements are never listed in leaf faces, so a 3D-skybox
            // displacement (c2m1's horizon ring and black sky floor) is found
            // by the area just off its base face instead.
            let is_sky = model_index == 0
                && (sky_faces.contains(&index)
                    || short(face, 12)? as i16 >= 0
                        && sky.as_ref().is_some_and(|sky| {
                            displacement_area(b, face, surfedges, edges, vertices)
                                .is_ok_and(|area| area == sky.area)
                        }));
            let clusters = if model_index == 0 && !is_sky {
                visibility.clusters_for_face(index)
            } else {
                vec![]
            };
            let group = groups
                .entry((model_index, material.clone(), is_sky, clusters.clone()))
                .or_insert_with(|| Surface {
                    clusters,
                    model: model_index,
                    sky: is_sky,
                    material,
                    positions: vec![],
                    uv: vec![],
                    light_uv: vec![],
                    blend: vec![],
                    bump: vec![],
                    texdata_size: size,
                });
            let mut polygon = vec![];
            for i in 0..len {
                let signed = word(record(surfedges, start as usize + i, 4)?, 0)? as i32;
                let edge = record(edges, signed.unsigned_abs() as usize, 4)?;
                let vertex = short(edge, if signed < 0 { 2 } else { 0 })? as usize;
                polygon.push(binary::vec3(record(vertices, vertex, 12)?, 0)?);
            }
            if polygon.len() < 3 {
                continue;
            }
            let planes = lump(b, 1)?;
            let normal = if planes.is_empty() {
                [0.; 3]
            } else {
                let n = binary::vec3(record(planes, short(face, 0)? as usize, 20)?, 0)?;
                // planenum already selects an oriented plane. `side` records
                // which side of the BSP node contains the face, not a normal flip.
                n
            };
            let mut triangles = vec![];
            // Blend per triangle corner, matched to the corner's grid point.
            let mut corner_blend: Vec<([f32; 3], f32)> = vec![];
            let disp = short(face, 12)? as i16;
            if disp >= 0 {
                let ((cells, grid), alphas) = displacement_grid_alpha(
                    record(lump(b, 26)?, disp as usize, 176)?,
                    lump(b, 33)?,
                    &polygon,
                )?;
                corner_blend = grid.iter().zip(&alphas).map(|(g, a)| (g.0, *a)).collect();
                for y in 0..cells {
                    for x in 0..cells {
                        let a = y * (cells + 1) + x;
                        let d = a + cells + 1;
                        let indices = if (x + y) % 2 == 0 {
                            [[a, d, a + 1], [d, d + 1, a + 1]]
                        } else {
                            [[a, d, d + 1], [a, d + 1, a + 1]]
                        };
                        for t in indices {
                            triangles.push(t.map(|i| grid[i]));
                        }
                    }
                }
            } else {
                for i in 1..polygon.len() - 1 {
                    triangles
                        .push([polygon[0], polygon[i], polygon[i + 1]].map(|p| (p, p, [0.; 2])));
                }
            }
            for triangle in triangles {
                // Displacements wind by the flat base grid: a cliff or an
                // overhang raised from a floor quad faces away from the base
                // plane, and re-orienting each displaced triangle flipped
                // those to back faces (c13m1's start cliff drew as a hole
                // onto the 3D skybox, with stray slabs).
                let oriented = if disp >= 0 {
                    oriented_triangle(triangle.map(|(_, base, uv)| (base, base, uv)), normal)
                        .map(|(base, _, uv)| {
                            *triangle.iter().find(|t| t.1 == base && t.2 == uv).expect("grid corner")
                        })
                } else {
                    oriented_triangle(triangle, normal)
                };
                for (source, base, grid_uv) in oriented {
                    let p = convert(source);
                    group.positions.push(p);
                    if model_index == 0 {
                        positions.push(p);
                    }
                    let uv = project(info, 0, base)?;
                    group.uv.push([uv[0] / size[0], uv[1] / size[1]]);
                    group.blend.push(
                        corner_blend
                            .iter()
                            .find(|(p, _)| *p == source)
                            .map_or(0., |(_, a)| *a),
                    );
                    let luv = if let Some(&[x, y, w, h]) = tiles.get(&index) {
                        // Displacement luxels follow its start-corner grid, which
                        // can be rotated relative to the flat face's texinfo axes.
                        let uv = if disp >= 0 {
                            [grid_uv[0] * (w - 1) as f32, grid_uv[1] * (h - 1) as f32]
                        } else {
                            let uv = project(info, 32, base)?;
                            [
                                uv[0] - word(face, 28)? as i32 as f32,
                                uv[1] - word(face, 32)? as i32 as f32,
                            ]
                        };
                        [
                            (uv[0] + x as f32 + 0.5) / lightmap.width as f32,
                            (uv[1] + y as f32 + 0.5) / lightmap.height as f32,
                        ]
                    } else {
                        [0.5 / lightmap.width as f32, 0.5 / lightmap.height as f32]
                    };
                    group.light_uv.push(luv);
                    group.bump.push(match (bumped.contains(&index), tiles.get(&index)) {
                        (true, Some(&[_, _, w, _])) => (w + 2) as f32 / lightmap.width as f32,
                        _ => 0.,
                    });
                }
            }
            if model_index == 0 {
                rendered += 1;
            }
        }
    }
    if positions.is_empty() {
        return Err(error("No supported static world faces"));
    }
    let spawn = spawn(lump(b, 0)?).unwrap_or(positions[0]);
    let mut surfaces: Vec<Surface> = groups.into_values().collect();
    surfaces.extend(overlays(b, faces, texinfo, &tiles, [lightmap.width as f32, lightmap.height as f32], &visibility)?);
    let detail_sprites = detail_sprites(b).unwrap_or_default();
    Ok(World {
        detail_sprites,
        positions,
        spawn,
        faces: rendered,
        surfaces,
        props,
        brushes,
        entities,
        pak: lump(b, 40)?.to_vec(),
        cubemaps: lump(b, 42)?
            .chunks_exact(16)
            .map(|c| std::array::from_fn(|i| i32::from_le_bytes(c[i * 4..i * 4 + 4].try_into().unwrap())))
            .collect(),
        lightmap,
        sky,
        sky_entities,
        suns,
        visibility,
        ambient_field: AmbientField::load(b, hdr)?,
        local_lights,
        light_styles,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    /// c1m1's two skybox smoke columns sit in the 3D skybox area and are
    /// drawn through the sky camera; ordinary map effects are not.
    #[test]
    fn hotel_skybox_particle_systems_are_tagged_for_the_sky_pass() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        if !path.exists() {
            return;
        }
        let world = load_file(&path, root).unwrap();
        let names: Vec<_> = world
            .sky_entities
            .iter()
            .filter_map(|&i| world.entities[i].get("effect_name").cloned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().all(|n| n.starts_with("c1m1_skybox_particles_")));
    }
    /// c2m2's start room: the Midnight Riders posters and safe-room graffiti
    /// are info_overlays, drawn as world surfaces on the wall.
    #[test]
    fn fairgrounds_overlays_become_wall_surfaces() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c2m2_fairgrounds.bsp");
        if !path.exists() {
            return;
        }
        let world = load_file(&path, root).unwrap();
        let posters: Vec<_> = world
            .surfaces
            .iter()
            .filter(|s| s.texdata_size == OVERLAY_UV && s.material == "signs/poster_midnight_riders_01")
            .collect();
        assert!(!posters.is_empty());
        let near = posters.iter().flat_map(|s| s.positions.iter()).any(|p| {
            let source = [p[0] / 0.0254, -p[2] / 0.0254, p[1] / 0.0254];
            (source[0] - 1740.).abs() < 2. && (source[1] - 2849.).abs() < 40. && (source[2] - 79.).abs() < 60.
        });
        assert!(near, "poster quad on the start-room wall");
        for s in &posters {
            assert!(s.light_uv.iter().flatten().all(|v| (0. ..=1.).contains(v)));
            assert!(s.uv.iter().flatten().all(|v| v.is_finite()));
        }
    }
    /// c1m1 has a dim fill skylight listed before its light_environment
    /// sun; the sun (pitch -14, yaw 30) must come first.
    #[test]
    fn hotel_sun_is_the_brightest_skylight() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        if !path.exists() {
            return;
        }
        let world = load_file(&path, root).unwrap();
        assert_eq!(world.suns.len(), 2);
        let sun = &world.suns[0];
        assert!(sun.intensity[0] > 1., "{:?}", sun.intensity);
        // Toward the sun: low (about 14 degrees up).
        assert!((sun.direction[1] - 14f32.to_radians().sin()).abs() < 0.02, "{:?}", sun.direction);
    }
    #[test]
    fn highway_displacements_stay_in_their_baked_lightmap_tiles() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c2m1_highway.bsp");
        if !path.exists() {
            return;
        }
        let world = load_file(&path, root).unwrap();
        let mut samples = 0;
        for s in &world.surfaces {
            if s.model != 0
                || !["grass", "blacktop", "concrete/highway_"]
                    .iter()
                    .any(|n| s.material.contains(n))
            {
                continue;
            }
            for uv in &s.light_uv {
                let x = (uv[0] * world.lightmap.width as f32).floor() as usize;
                let y = (uv[1] * world.lightmap.height as f32).floor() as usize;
                let color = world.lightmap.pixels[y * world.lightmap.width + x];
                assert_ne!(
                    color, [1.; 4],
                    "{} samples unused atlas at {uv:?}",
                    s.material
                );
                samples += 1;
            }
        }
        assert!(samples > 1000);
    }
    #[test]
    fn portal_covers_follow_explicit_window_targets() {
        let entities = kv::entities(
            r#"
            { "classname" "func_areaportalwindow" "target" "Cover01" }
            { "classname" "func_areaportal" "target" "real_door" }
            { "classname" "func_brush" "targetname" "cover01" }
        "#,
        );
        let targets = portal_cover_targets(&entities);
        assert!(targets.contains("cover01"));
        assert!(!targets.contains("real_door"));
        assert_eq!(targets.len(), 1);
    }
    #[test]
    fn displacement_grid_restores_deformed_quad_and_preserves_base_uv_positions() {
        let mut info = vec![0; 176];
        info[20..24].copy_from_slice(&2u32.to_le_bytes());
        let mut vertices = vec![0; 25 * 20];
        vertices[12 * 20 + 8..12 * 20 + 12].copy_from_slice(&1f32.to_le_bytes());
        vertices[12 * 20 + 12..12 * 20 + 16].copy_from_slice(&10f32.to_le_bytes());
        let (cells, grid) = displacement_grid(
            &info,
            &vertices,
            &[
                [0., 0., 0.],
                [0., 100., 0.],
                [100., 100., 0.],
                [100., 0., 0.],
            ],
        )
        .unwrap();
        assert_eq!(cells, 4);
        assert_eq!(grid[12], ([50., 50., 10.], [50., 50., 0.], [0.5, 0.5]));
        assert!(displacement_grid(&info, &vertices[..20], &[[0.; 3]; 4]).is_err());
    }
    #[test]
    fn displacement_winding_follows_the_base_grid() {
        // A floor quad raised into a wall leaning back past vertical: the
        // displaced triangle faces away from the base normal and must keep
        // the grid winding, not be flipped per triangle.
        let base = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]];
        let raised = [[0., 0., 0.], [1., 0., 0.], [0., -0.2, 1.]];
        let triangle: [([f32; 3], [f32; 3], [f32; 2]); 3] =
            std::array::from_fn(|k| (raised[k], base[k], [k as f32, 0.]));
        let by_base = oriented_triangle(triangle.map(|(_, b, uv)| (b, b, uv)), [0., 0., 1.]);
        assert_eq!(by_base.map(|t| t.2), triangle.map(|t| t.2), "base winding kept");
        let by_displaced = oriented_triangle(triangle, [0., 0., 1.]);
        assert_ne!(by_displaced.map(|t| t.2), triangle.map(|t| t.2), "per-triangle check flips it");
    }
    #[test]
    fn winding_matches_face_plane_without_disabling_culling() {
        let triangle = [[0., 0., 0.], [0., 1., 0.], [1., 0., 0.]].map(|p| (p, p, [0.; 2]));
        let out = oriented_triangle(triangle, [0., 0., 1.]);
        assert_eq!(out[1], triangle[2]);
        assert_eq!(out[2], triangle[1]);
    }
    #[test]
    fn entity_overlay_replaces_original_entities() {
        let mut b = fixture();
        let text = br#"{ "classname" "info_survivor_position" "origin" "100 200 300" }"#;
        let mut patch = vec![];
        for word in [20u32, 0, 0, text.len() as u32, 1234] {
            patch.extend(word.to_le_bytes());
        }
        patch.extend(text);
        apply_overlay(&mut b, &patch).unwrap();
        let world = parse(&b).unwrap();
        assert_eq!(world.spawn[0], 2.54);
        assert_eq!(world.entities.len(), 1);
    }
    #[test]
    fn ambient_rgb_exponent_retains_native_scale_and_uses_nearest_probe() {
        let mut b = fixture();
        let mut leaf = vec![0; 32];
        for axis in 0..3 {
            leaf[14 + axis * 2..16 + axis * 2].copy_from_slice(&255i16.to_le_bytes());
        }
        let mut samples = vec![0; 56];
        for face in 0..6 {
            samples[face * 4..face * 4 + 4].copy_from_slice(&[128, 64, 32, 248]);
            samples[28 + face * 4..32 + face * 4].copy_from_slice(&[128, 64, 32, 249]);
        }
        samples[52..55].fill(255);
        for (i, data) in [(10, leaf), (51, vec![2, 0, 0, 0]), (55, samples)] {
            let offset = b.len() as u32;
            b[12 + i * 16..16 + i * 16].copy_from_slice(&offset.to_le_bytes());
            b[16 + i * 16..20 + i * 16].copy_from_slice(&(data.len() as u32).to_le_bytes());
            b.extend(data);
        }
        assert_eq!(
            ambient_cube(&b, [0.; 3], true).unwrap(),
            [[0.5, 0.25, 0.125]; 6]
        );
        assert_eq!(
            ambient_cube(&b, [255.; 3], true).unwrap(),
            [[1., 0.5, 0.25]; 6]
        );
        let field = AmbientField::load(&b, true).unwrap();
        assert_eq!(field.sample([0.; 3]), [[0.5, 0.25, 0.125]; 6]);
        assert_eq!(field.sample(convert([255.; 3])), [[1., 0.5, 0.25]; 6]);
    }
    #[test]
    fn baked_rgb_exponent_and_gutter_are_preserved() {
        let face = vec![0; 56];
        let (atlas, tiles) = bake_atlas(&face, &[128, 64, 32, 255]).unwrap();
        let [x, y, w, h] = tiles[&0];
        assert_eq!((w, h), (1, 1));
        let p = atlas.pixels[y * atlas.width + x];
        assert!((p[0] - 64. / 255.).abs() < 0.00001);
        assert_eq!(atlas.pixels[(y - 1) * atlas.width + x - 1], p);
    }
    #[test]
    fn reads_v9_static_prop_skin_and_origin() {
        let mut b = fixture();
        let mut prop = vec![0; 72];
        for (i, v) in [1f32, 2., 3.].into_iter().enumerate() {
            prop[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        prop[32..36].copy_from_slice(&2u32.to_le_bytes());
        prop[31] = 0x42;
        for (i, v) in [10f32, 20., 30.].into_iter().enumerate() {
            prop[44 + i * 4..48 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        prop[64..68].fill(255);
        let mut data = vec![];
        data.extend(1u32.to_le_bytes());
        let mut name = [0; 128];
        name[..15].copy_from_slice(b"models/test.mdl");
        data.extend(name);
        data.extend(0u32.to_le_bytes());
        data.extend(1u32.to_le_bytes());
        data.extend(prop);
        let payload = b.len() + 20;
        let mut game = vec![];
        game.extend(1u32.to_le_bytes());
        game.extend(0x73707270u32.to_le_bytes());
        game.extend(0u16.to_le_bytes());
        game.extend(9u16.to_le_bytes());
        game.extend((payload as u32).to_le_bytes());
        game.extend((data.len() as u32).to_le_bytes());
        let offset = b.len() as u32;
        b[12 + 35 * 16..16 + 35 * 16].copy_from_slice(&offset.to_le_bytes());
        b[16 + 35 * 16..20 + 35 * 16].copy_from_slice(&(game.len() as u32).to_le_bytes());
        b.extend(game);
        b.extend(data);
        let world = parse(&b).unwrap();
        assert_eq!(world.props.len(), 1);
        assert_eq!(world.props[0].skin, 2);
        assert_eq!(world.props[0].origin, [1., 2., 3.]);
        assert_eq!(world.props[0].model, "models/test.mdl");
        assert_eq!(world.props[0].lighting_flags, 0x42);
        assert_eq!(world.props[0].lighting_origin, Some([10., 20., 30.]));
    }
    fn fixture() -> Vec<u8> {
        let mut lumps: [Vec<u8>; 64] = std::array::from_fn(|_| vec![]);
        for vertex in [
            [0., 0., 0.],
            [100., 0., 0.],
            [100., 100., 0.],
            [0., 100., 0.],
        ] {
            for value in vertex {
                lumps[3].extend(f32::to_le_bytes(value));
            }
        }
        lumps[6] = vec![0; 72];
        lumps[7] = vec![0; 56];
        lumps[7][8..10].copy_from_slice(&4u16.to_le_bytes());
        lumps[7][12..14].copy_from_slice(&(-1i16).to_le_bytes());
        for (a, b) in [(0u16, 0u16), (0, 1), (2, 1), (2, 3), (3, 0)] {
            lumps[12].extend(a.to_le_bytes());
            lumps[12].extend(b.to_le_bytes());
        }
        for edge in [1i32, -2, 3, 4] {
            lumps[13].extend(edge.to_le_bytes());
        }
        lumps[14] = vec![0; 48];
        lumps[14][44..48].copy_from_slice(&1u32.to_le_bytes());
        let mut bytes = vec![0; 1036];
        bytes[..4].copy_from_slice(b"VBSP");
        bytes[4..8].copy_from_slice(&21u32.to_le_bytes());
        for (i, lump) in lumps.into_iter().enumerate() {
            let offset = bytes.len() as u32;
            bytes[12 + i * 16..16 + i * 16].copy_from_slice(&offset.to_le_bytes());
            bytes[16 + i * 16..20 + i * 16].copy_from_slice(&(lump.len() as u32).to_le_bytes());
            bytes.extend(lump);
        }
        bytes
    }
    #[test]
    fn triangulates_signed_surfedges() {
        let world = parse(&fixture()).unwrap();
        assert_eq!(world.faces, 1);
        assert_eq!(world.positions.len(), 6);
        assert_eq!(world.positions[1], [2.54, 0., 0.]);
        assert_eq!(world.positions[2], [2.54, 0., -2.54]);
        assert_eq!(world.positions[5], [0., 0., -2.54]);
    }
    #[test]
    fn rejects_lump_outside_file() {
        let mut bytes = fixture();
        bytes[12 + 3 * 16..16 + 3 * 16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&bytes).is_err());
    }
    #[test]
    fn rejects_invalid_headers() {
        assert!(parse(&[]).is_err());
        assert!(parse(&[0; 1036]).is_err());
    }
    #[test]
    fn source_axes_and_scale() {
        assert_eq!(convert([100., 200., 300.]), [2.54, 7.62, -5.08]);
    }
    #[test]
    fn finds_survivor_spawn() {
        let p =
            spawn(br#"{ "classname" "info_survivor_position" "origin" "100 200 300" }"#).unwrap();
        assert!((p[1] - 9.27).abs() < 0.0001);
        assert_eq!(p[0], 2.54);
    }
}
