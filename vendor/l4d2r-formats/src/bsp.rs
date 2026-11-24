//! VBSP v20/v21 (Left 4 Dead / Left 4 Dead 2) loader.
//!
//! Lump directory entries in L4D2 are `{ version, fileofs, filelen, fourCC }` (verified against the
//! shipped maps), which differs from the older `{ fileofs, filelen, version, fourCC }` order.

use anyhow::{bail, Result};
use glam::Vec3;
use std::collections::HashMap;

pub const LUMP_ENTITIES: usize = 0;
pub const LUMP_PLANES: usize = 1;
pub const LUMP_TEXDATA: usize = 2;
pub const LUMP_VERTEXES: usize = 3;
pub const LUMP_VISIBILITY: usize = 4;
pub const LUMP_NODES: usize = 5;
pub const LUMP_TEXINFO: usize = 6;
pub const LUMP_FACES: usize = 7;
pub const LUMP_LIGHTING: usize = 8;
pub const LUMP_LEAFS: usize = 10;
pub const LUMP_EDGES: usize = 12;
pub const LUMP_SURFEDGES: usize = 13;
pub const LUMP_MODELS: usize = 14;
pub const LUMP_LEAFFACES: usize = 16;
pub const LUMP_LEAFBRUSHES: usize = 17;
pub const LUMP_BRUSHES: usize = 18;
pub const LUMP_BRUSHSIDES: usize = 19;
pub const LUMP_DISPINFO: usize = 26;
pub const LUMP_PHYSCOLLIDE: usize = 29;
pub const LUMP_DISPVERTS: usize = 33;
pub const LUMP_GAMELUMP: usize = 35;
pub const LUMP_PAKFILE: usize = 40;
pub const LUMP_TEXDATA_STRING_DATA: usize = 43;
pub const LUMP_TEXDATA_STRING_TABLE: usize = 44;
pub const LUMP_LIGHTING_HDR: usize = 53;
pub const LUMP_FACES_HDR: usize = 58;

pub const SURF_SKY2D: i32 = 0x2;
pub const SURF_SKY: i32 = 0x4;
pub const SURF_WARP: i32 = 0x8;
pub const SURF_TRANS: i32 = 0x10;
pub const SURF_NOPORTAL: i32 = 0x20;
pub const SURF_TRIGGER: i32 = 0x40;
pub const SURF_NODRAW: i32 = 0x80;
pub const SURF_HINT: i32 = 0x100;
pub const SURF_SKIP: i32 = 0x200;
pub const SURF_NOLIGHT: i32 = 0x400;

pub const CONTENTS_SOLID: i32 = 0x1;
pub const CONTENTS_WINDOW: i32 = 0x2;
pub const CONTENTS_GRATE: i32 = 0x8;
pub const CONTENTS_WATER: i32 = 0x20;
pub const CONTENTS_PLAYERCLIP: i32 = 0x10000;
pub const CONTENTS_MONSTERCLIP: i32 = 0x20000;
pub const CONTENTS_MOVEABLE: i32 = 0x4000;
pub const CONTENTS_MONSTER: i32 = 0x2000000;
/// MASK_PLAYERSOLID from the Source SDK: SOLID|MOVEABLE|PLAYERCLIP|WINDOW|MONSTER|GRATE
pub const MASK_PLAYERSOLID: i32 = CONTENTS_SOLID
    | CONTENTS_MOVEABLE
    | CONTENTS_PLAYERCLIP
    | CONTENTS_WINDOW
    | CONTENTS_MONSTER
    | CONTENTS_GRATE;

#[derive(Clone, Copy, Debug, Default)]
struct Lump {
    version: i32,
    ofs: usize,
    len: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub normal: Vec3,
    pub dist: f32,
    pub ty: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct TexInfo {
    pub tex_vecs: [[f32; 4]; 2],
    pub lm_vecs: [[f32; 4]; 2],
    pub flags: i32,
    pub texdata: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct TexData {
    pub reflectivity: Vec3,
    pub name_id: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Face {
    pub plane: u16,
    pub side: u8,
    pub first_edge: i32,
    pub num_edges: i16,
    pub texinfo: i16,
    pub dispinfo: i16,
    pub styles: [u8; 4],
    pub light_ofs: i32,
    pub lm_mins: [i32; 2],
    pub lm_size: [i32; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub plane: i32,
    pub children: [i32; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct Leaf {
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub contents: i32,
    pub cluster: i16,
    pub first_leaf_brush: u16,
    pub num_leaf_brushes: u16,
    pub first_leaf_face: u16,
    pub num_leaf_faces: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct Brush {
    pub first_side: i32,
    pub num_sides: i32,
    pub contents: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct BrushSide {
    pub plane: u16,
    pub texinfo: i16,
    pub dispinfo: i16,
    pub bevel: i16,
}

#[derive(Clone, Copy, Debug)]
pub struct Model {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub origin: Vec3,
    pub head_node: i32,
    pub first_face: i32,
    pub num_faces: i32,
}

#[derive(Clone, Debug)]
pub struct DispInfo {
    pub start_position: Vec3,
    pub vert_start: i32,
    pub power: i32,
    pub contents: i32,
    pub map_face: u16,
    /// Hammer collision flags from minTess when its DISP_INFO_FLAG_MAGIC
    /// bit is set (2 no physics, 4 no hull, 8 no ray collision), else 0.
    pub flags: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct StaticProp {
    pub origin: Vec3,
    pub angles: Vec3,
    pub model_idx: u16,
    pub solid: u8,
    pub skin: i32,
    pub fade_min: f32,
    pub fade_max: f32,
    pub scale: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Entity(pub Vec<(String, String)>);

impl Entity {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
    pub fn classname(&self) -> &str {
        self.get("classname").unwrap_or("")
    }
    pub fn vec3(&self, k: &str) -> Option<Vec3> {
        let mut it = self
            .get(k)?
            .split_whitespace()
            .map(|s| s.parse::<f32>().ok());
        Some(Vec3::new(it.next()??, it.next()??, it.next()??))
    }
    pub fn origin(&self) -> Vec3 {
        self.vec3("origin").unwrap_or(Vec3::ZERO)
    }
    pub fn angles(&self) -> Vec3 {
        self.vec3("angles").unwrap_or(Vec3::ZERO)
    }
    /// Hammer object id (`hammerid`), matching the VMF entity `id`.
    pub fn hammerid(&self) -> Option<i64> {
        self.get("hammerid")?.trim().parse().ok()
    }
    /// Output connections: values of the form `target,input,param,delay,times`, separated by 0x1B in current
    /// compiles and by commas in older ones (e.g. retail `tutorial_standards`).
    pub fn connections(&self) -> Vec<crate::vmf::Connection> {
        self.0
            .iter()
            .filter_map(|(k, v)| crate::vmf::Connection::parse(k, v))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct WorldVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub lm_uv: [f32; 2],
    /// Displacement vertex alpha / 255 (blend weight for WorldVertexTransition), 0 for ordinary faces.
    pub blend: f32,
}

#[derive(Debug, Default)]
pub struct MaterialBatch {
    pub material: String,
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
}

pub struct LightmapAtlas {
    pub width: u32,
    pub height: u32,
    /// RGBA float16 (little-endian bytes), linear HDR light.
    pub rgba: Vec<u8>,
}

/// Per-leaf ambient lighting cubes (+X,-X,+Y,-Y,+Z,-Z), linear HDR, used to light props and characters.
/// UNVERIFIED: ambient-cube exponents are ~2^-6 of the lightmap convention; fitted against the original (docs/COMPARISON.md).
const AMBIENT_EXP_BIAS: f32 = 255.0;

#[derive(Clone)]
pub struct AmbientField {
    planes: Vec<Plane>,
    nodes: Vec<Node>,
    leaf_bounds: Vec<(Vec3, Vec3)>,
    index: Vec<(u16, u16)>,
    samples: Vec<([[f32; 3]; 6], [u8; 3])>,
    head: i32,
}

impl AmbientField {
    fn leaf_at(&self, p: Vec3) -> usize {
        let mut n = self.head;
        while n >= 0 {
            let node = &self.nodes[n as usize];
            let pl = &self.planes[node.plane as usize];
            n = node.children[(pl.normal.dot(p) - pl.dist < 0.0) as usize];
        }
        (-1 - n) as usize
    }

    pub fn debug_info(&self, p: Vec3) -> (usize, (u16, u16), usize, usize) {
        let leaf = self.leaf_at(p);
        (
            leaf,
            self.index.get(leaf).copied().unwrap_or((0, 0)),
            self.index.len(),
            self.samples.len(),
        )
    }

    /// Ambient cube at a world position (nearest sample inside its leaf).
    pub fn cube_at(&self, p: Vec3) -> [[f32; 3]; 6] {
        let leaf = self.leaf_at(p).min(self.index.len().saturating_sub(1));
        let (count, first) = self.index.get(leaf).copied().unwrap_or((0, 0));
        let (mn, mx) = self
            .leaf_bounds
            .get(leaf)
            .copied()
            .unwrap_or((Vec3::ZERO, Vec3::ZERO));
        let mut best: Option<(f32, usize)> = None;
        for k in 0..count as usize {
            let Some((_, xyz)) = self.samples.get(first as usize + k) else {
                break;
            };
            let sp =
                mn + (mx - mn) * Vec3::new(xyz[0] as f32, xyz[1] as f32, xyz[2] as f32) / 255.0;
            let d = sp.distance_squared(p);
            if best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, first as usize + k));
            }
        }
        best.map(|(_, i)| self.samples[i].0)
            .unwrap_or([[0.05; 3]; 6])
    }
}

pub struct Bsp {
    data: Vec<u8>,
    lumps: [Lump; 64],
    pub version: i32,
    pub planes: Vec<Plane>,
    pub texinfo: Vec<TexInfo>,
    pub texdata: Vec<TexData>,
    pub texnames: Vec<String>,
    pub faces: Vec<Face>,
    pub models: Vec<Model>,
    pub nodes: Vec<Node>,
    pub leafs: Vec<Leaf>,
    pub leaf_brushes: Vec<u16>,
    pub brushes: Vec<Brush>,
    pub brush_sides: Vec<BrushSide>,
    pub vertexes: Vec<Vec3>,
    pub edges: Vec<[u16; 2]>,
    pub surfedges: Vec<i32>,
    pub dispinfo: Vec<DispInfo>,
    pub entities: Vec<Entity>,
    pub static_prop_names: Vec<String>,
    pub static_props: Vec<StaticProp>,
    pub hdr: bool,
}

/// Authored per-brush-model VPhysics payload; collision bytes remain in the BSP.
pub struct PhysicsModel<'a> {
    pub model_index:i32,
    pub solid_count:usize,
    pub collision:&'a [u8],
    pub keyvalues:&'a str,
}
impl PhysicsModel<'_> {
    pub fn geometry(&self)->Result<Vec<crate::phy::PhySolid>>{
        crate::phy::parse_collision(self.collision,self.solid_count)
    }
}

fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn v3_at(b: &[u8], o: usize) -> Vec3 {
    Vec3::new(f32_at(b, o), f32_at(b, o + 4), f32_at(b, o + 8))
}

impl Bsp {
    pub fn physics_models(&self)->Result<Vec<PhysicsModel<'_>>>{
        if self.lumps[LUMP_PHYSCOLLIDE].version!=0{bail!("unsupported physics lump version");}
        let bytes=self.lump(LUMP_PHYSCOLLIDE);let mut offset=0;let mut models=Vec::new();
        while offset<bytes.len(){
            let header=bytes.get(offset..offset+16).ok_or_else(||anyhow::anyhow!("truncated BSP physics header"))?;
            let model_index=i32_at(header,0);let collision=i32_at(header,4);let keys=i32_at(header,8);let solids=i32_at(header,12);offset+=16;
            if model_index==-1{
                if offset!=bytes.len(){bail!("data after BSP physics terminator");}return Ok(models);
            }
            if model_index<0||collision<0||keys<0||solids<0{bail!("negative BSP physics sizes");}
            let end=offset.checked_add(collision as usize).and_then(|p|p.checked_add(keys as usize)).ok_or_else(||anyhow::anyhow!("BSP physics size overflow"))?;
            let payload=bytes.get(offset..end).ok_or_else(||anyhow::anyhow!("truncated BSP physics payload"))?;
            let (collision,keys)=payload.split_at(collision as usize);
            let keys=&keys[..keys.iter().position(|&b|b==0).unwrap_or(keys.len())];
            models.push(PhysicsModel{model_index,solid_count:solids as usize,collision,keyvalues:std::str::from_utf8(keys)?});offset=end;
        }
        if !bytes.is_empty(){bail!("missing BSP physics terminator");}Ok(models)
    }
    /// Corner-ordered quad (corner 0 nearest `start_position`) and the (2^power+1)^2 displaced grid.
    pub fn displacement_grid(
        &self,
        di: &DispInfo,
        verts: &[Vec3],
    ) -> Option<([Vec3; 4], Vec<Vec3>)> {
        if verts.len() != 4 {
            return None;
        }
        let dispverts = self.lump(LUMP_DISPVERTS);
        let n = (1usize << di.power) + 1;
        let mut start = 0;
        let mut best = f32::MAX;
        for (i, v) in verts.iter().enumerate() {
            let d = v.distance_squared(di.start_position);
            if d < best {
                best = d;
                start = i;
            }
        }
        let c = [
            verts[start],
            verts[(start + 1) % 4],
            verts[(start + 2) % 4],
            verts[(start + 3) % 4],
        ];
        let (left_edge, right_edge) = (c[1] - c[0], c[2] - c[3]);
        let mut grid = vec![Vec3::ZERO; n * n];
        for y in 0..n {
            let fy = y as f32 / (n - 1) as f32;
            let start_e = c[0] + left_edge * fy;
            let end_e = c[3] + right_edge * fy;
            for x in 0..n {
                let fx = x as f32 / (n - 1) as f32;
                let base_pos = start_e + (end_e - start_e) * fx;
                let idx = di.vert_start as usize + y * n + x;
                if (idx + 1) * 20 > dispverts.len() {
                    return None;
                }
                let rec = &dispverts[idx * 20..idx * 20 + 20];
                grid[y * n + x] = base_pos + v3_at(rec, 0) * f32_at(rec, 12);
            }
        }
        Some((c, grid))
    }

    /// All displacement triangles `(contents, [a,b,c])` for collision.
    pub fn displacement_triangles(&self) -> Vec<(i32, [Vec3; 3])> {
        self.displacement_triangles_with_flags().into_iter().map(|(c, _, t)| (c, t)).collect()
    }

    /// As `displacement_triangles`, with each displacement's Hammer
    /// collision flags (`DispInfo::flags`).
    pub fn displacement_triangles_with_flags(&self) -> Vec<(i32, u32, [Vec3; 3])> {
        let mut out = Vec::new();
        for f in &self.faces {
            if f.dispinfo < 0 {
                continue;
            }
            let di = &self.dispinfo[f.dispinfo as usize];
            let verts = self.face_verts(f);
            let Some((_, grid)) = self.displacement_grid(di, &verts) else {
                continue;
            };
            let n = (1usize << di.power) + 1;
            // Orient every triangle so (b-a)x(c-a) agrees with the owning face's outward normal (front side).
            let pl = self.planes[f.plane as usize].normal;
            let face_n = if f.side != 0 { -pl } else { pl };
            let mut push = |a: Vec3, b: Vec3, c: Vec3| {
                if (b - a).cross(c - a).dot(face_n) >= 0.0 {
                    out.push((di.contents, di.flags, [a, b, c]));
                } else {
                    out.push((di.contents, di.flags, [a, c, b]));
                }
            };
            for y in 0..n - 1 {
                for x in 0..n - 1 {
                    let (a, b, c2, d) = (
                        grid[y * n + x],
                        grid[y * n + x + 1],
                        grid[(y + 1) * n + x],
                        grid[(y + 1) * n + x + 1],
                    );
                    if (x + y) % 2 == 0 {
                        push(a, c2, b);
                        push(b, c2, d);
                    } else {
                        push(a, c2, d);
                        push(a, d, b);
                    }
                }
            }
        }
        out
    }

    pub fn parse(data: Vec<u8>) -> Result<Bsp> {
        if data.len() < 8 + 64 * 16 || &data[0..4] != b"VBSP" {
            bail!("not a VBSP file");
        }
        let version = i32_at(&data, 4);
        if !(19..=21).contains(&version) {
            bail!("unsupported BSP version {version}");
        }
        let mut lumps = [Lump::default(); 64];
        for (i, l) in lumps.iter_mut().enumerate() {
            let o = 8 + i * 16;
            *l = Lump {
                version: i32_at(&data, o),
                ofs: i32_at(&data, o + 4) as usize,
                len: i32_at(&data, o + 8) as usize,
            };
            if l.ofs + l.len > data.len() {
                bail!("lump {i} out of bounds");
            }
            if i32_at(&data, o + 12) != 0 {
                bail!("lump {i} is LZMA-compressed (unsupported)");
            }
        }
        let mut bsp = Bsp {
            data,
            lumps,
            version,
            planes: vec![],
            texinfo: vec![],
            texdata: vec![],
            texnames: vec![],
            faces: vec![],
            models: vec![],
            nodes: vec![],
            leafs: vec![],
            leaf_brushes: vec![],
            brushes: vec![],
            brush_sides: vec![],
            vertexes: vec![],
            edges: vec![],
            surfedges: vec![],
            dispinfo: vec![],
            entities: vec![],
            static_prop_names: vec![],
            static_props: vec![],
            hdr: false,
        };
        bsp.load_all();
        Ok(bsp)
    }

    fn lump(&self, i: usize) -> &[u8] {
        let l = self.lumps[i];
        &self.data[l.ofs..l.ofs + l.len]
    }

    fn load_all(&mut self) {
        let b = self.lump(LUMP_PLANES).to_vec();
        self.planes = b
            .chunks_exact(20)
            .map(|c| Plane {
                normal: v3_at(c, 0),
                dist: f32_at(c, 12),
                ty: i32_at(c, 16),
            })
            .collect();
        let b = self.lump(LUMP_VERTEXES).to_vec();
        self.vertexes = b.chunks_exact(12).map(|c| v3_at(c, 0)).collect();
        let b = self.lump(LUMP_EDGES).to_vec();
        self.edges = b
            .chunks_exact(4)
            .map(|c| [u16_at(c, 0), u16_at(c, 2)])
            .collect();
        let b = self.lump(LUMP_SURFEDGES).to_vec();
        self.surfedges = b.chunks_exact(4).map(|c| i32_at(c, 0)).collect();
        let b = self.lump(LUMP_TEXINFO).to_vec();
        self.texinfo = b
            .chunks_exact(72)
            .map(|c| {
                let r = |o: usize| {
                    [
                        f32_at(c, o),
                        f32_at(c, o + 4),
                        f32_at(c, o + 8),
                        f32_at(c, o + 12),
                    ]
                };
                TexInfo {
                    tex_vecs: [r(0), r(16)],
                    lm_vecs: [r(32), r(48)],
                    flags: i32_at(c, 64),
                    texdata: i32_at(c, 68),
                }
            })
            .collect();
        let b = self.lump(LUMP_TEXDATA).to_vec();
        self.texdata = b
            .chunks_exact(32)
            .map(|c| TexData {
                reflectivity: v3_at(c, 0),
                name_id: i32_at(c, 12),
                width: i32_at(c, 16),
                height: i32_at(c, 20),
            })
            .collect();
        let table = self.lump(LUMP_TEXDATA_STRING_TABLE).to_vec();
        let strdata = self.lump(LUMP_TEXDATA_STRING_DATA).to_vec();
        self.texnames = table
            .chunks_exact(4)
            .map(|c| {
                let o = i32_at(c, 0) as usize;
                let end = strdata[o..]
                    .iter()
                    .position(|&x| x == 0)
                    .map(|n| o + n)
                    .unwrap_or(strdata.len());
                String::from_utf8_lossy(&strdata[o..end]).to_ascii_lowercase()
            })
            .collect();

        // Faces: prefer the HDR face lump when it carries data (L4D2 ships HDR-only lighting).
        let hdr_faces = self.lumps[LUMP_FACES_HDR].len > 0 && self.lumps[LUMP_LIGHTING_HDR].len > 0;
        self.hdr = hdr_faces;
        let fl = if hdr_faces {
            LUMP_FACES_HDR
        } else {
            LUMP_FACES
        };
        let b = self.lump(fl).to_vec();
        self.faces = b
            .chunks_exact(56)
            .map(|c| Face {
                plane: u16_at(c, 0),
                side: c[2],
                first_edge: i32_at(c, 4),
                num_edges: i16_at(c, 8),
                texinfo: i16_at(c, 10),
                dispinfo: i16_at(c, 12),
                styles: [c[16], c[17], c[18], c[19]],
                light_ofs: i32_at(c, 20),
                lm_mins: [i32_at(c, 28), i32_at(c, 32)],
                lm_size: [i32_at(c, 36), i32_at(c, 40)],
            })
            .collect();
        let b = self.lump(LUMP_MODELS).to_vec();
        self.models = b
            .chunks_exact(48)
            .map(|c| Model {
                mins: v3_at(c, 0),
                maxs: v3_at(c, 12),
                origin: v3_at(c, 24),
                head_node: i32_at(c, 36),
                first_face: i32_at(c, 40),
                num_faces: i32_at(c, 44),
            })
            .collect();
        let b = self.lump(LUMP_NODES).to_vec();
        self.nodes = b
            .chunks_exact(32)
            .map(|c| Node {
                plane: i32_at(c, 0),
                children: [i32_at(c, 4), i32_at(c, 8)],
            })
            .collect();
        // L4D2 leaf lump version 1 has no ambient cube (32-byte records vs 56 in version 0).
        let lv = self.lumps[LUMP_LEAFS];
        let stride = if lv.version == 0 { 56 } else { 32 };
        let b = self.lump(LUMP_LEAFS).to_vec();
        self.leafs = b
            .chunks_exact(stride)
            .map(|c| Leaf {
                mins: [i16_at(c, 8), i16_at(c, 10), i16_at(c, 12)],
                maxs: [i16_at(c, 14), i16_at(c, 16), i16_at(c, 18)],
                contents: i32_at(c, 0),
                cluster: i16_at(c, 4),
                first_leaf_face: u16_at(c, 20),
                num_leaf_faces: u16_at(c, 22),
                first_leaf_brush: u16_at(c, 24),
                num_leaf_brushes: u16_at(c, 26),
            })
            .collect();
        let b = self.lump(LUMP_LEAFBRUSHES).to_vec();
        self.leaf_brushes = b.chunks_exact(2).map(|c| u16_at(c, 0)).collect();
        let b = self.lump(LUMP_BRUSHES).to_vec();
        self.brushes = b
            .chunks_exact(12)
            .map(|c| Brush {
                first_side: i32_at(c, 0),
                num_sides: i32_at(c, 4),
                contents: i32_at(c, 8),
            })
            .collect();
        let b = self.lump(LUMP_BRUSHSIDES).to_vec();
        self.brush_sides = b
            .chunks_exact(8)
            .map(|c| BrushSide {
                plane: u16_at(c, 0),
                texinfo: i16_at(c, 2),
                dispinfo: i16_at(c, 4),
                bevel: i16_at(c, 6),
            })
            .collect();
        let b = self.lump(LUMP_DISPINFO).to_vec();
        self.dispinfo = b
            .chunks_exact(176)
            .map(|c| DispInfo {
                start_position: v3_at(c, 0),
                vert_start: i32_at(c, 12),
                power: i32_at(c, 20),
                contents: i32_at(c, 32),
                map_face: u16_at(c, 36),
                flags: {
                    let tess = i32_at(c, 24) as u32;
                    if tess & 0x8000_0000 != 0 { tess & 0xff } else { 0 }
                },
            })
            .collect();
        self.entities = parse_entities(&String::from_utf8_lossy(self.lump(LUMP_ENTITIES)));
        self.load_static_props();
    }

    fn load_static_props(&mut self) {
        let gl = self.lump(LUMP_GAMELUMP);
        if gl.len() < 4 {
            return;
        }
        let n = i32_at(gl, 0) as usize;
        for i in 0..n {
            let e = 4 + i * 16;
            if e + 16 > gl.len() {
                break;
            }
            // id is the FourCC 'sprp' stored little-endian => bytes "prps".
            if &gl[e..e + 4] != b"prps" {
                continue;
            }
            let ver = u16_at(gl, e + 6) as i32;
            let ofs = i32_at(gl, e + 8) as usize;
            let len = i32_at(gl, e + 12) as usize;
            if ofs + len > self.data.len() {
                continue;
            }
            let d = self.data[ofs..ofs + len].to_vec();
            let mut p = 0usize;
            let ndict = i32_at(&d, p) as usize;
            p += 4;
            for _ in 0..ndict {
                let raw = &d[p..p + 128];
                let end = raw.iter().position(|&c| c == 0).unwrap_or(128);
                self.static_prop_names
                    .push(String::from_utf8_lossy(&raw[..end]).into_owned());
                p += 128;
            }
            let nleaf = i32_at(&d, p) as usize;
            p += 4 + nleaf * 2;
            let nprops = i32_at(&d, p) as usize;
            p += 4;
            if nprops == 0 {
                return;
            }
            let stride = (d.len() - p) / nprops;
            log::debug!("sprp v{ver}: {nprops} props, stride {stride}");
            for k in 0..nprops {
                let c = &d[p + k * stride..p + (k + 1) * stride];
                let scale = if ver >= 11 && stride >= 72 {
                    f32_at(c, stride - 4)
                } else {
                    1.0
                };
                self.static_props.push(StaticProp {
                    origin: v3_at(c, 0),
                    angles: v3_at(c, 12),
                    model_idx: u16_at(c, 24),
                    solid: c[30],
                    skin: i32_at(c, 32),
                    fade_min: f32_at(c, 36),
                    fade_max: f32_at(c, 40),
                    scale: if scale > 0.0 { scale } else { 1.0 },
                });
            }
            return;
        }
    }

    /// Build the ambient-cube lookup (HDR lumps 51/55 when the map has HDR lighting, else 52/56).
    pub fn ambient_field(&self) -> AmbientField {
        let (li, ls) = if self.hdr { (51, 55) } else { (52, 56) };
        let index = self
            .lump(li)
            .chunks_exact(4)
            .map(|c| (u16_at(c, 0), u16_at(c, 2)))
            .collect();
        let conv = |c: &[u8]| -> [f32; 3] {
            let scale = 2f32.powi(c[3] as i8 as i32);
            [0, 1, 2].map(|i| (c[i] as f32 / 255.0) * scale * AMBIENT_EXP_BIAS)
        };
        let samples = self
            .lump(ls)
            .chunks_exact(28)
            .map(|c| {
                let cube = [0, 1, 2, 3, 4, 5].map(|k| conv(&c[k * 4..k * 4 + 4]));
                (cube, [c[24], c[25], c[26]])
            })
            .collect();
        AmbientField {
            planes: self.planes.clone(),
            nodes: self.nodes.clone(),
            leaf_bounds: self
                .leafs
                .iter()
                .map(|l| {
                    (
                        Vec3::new(l.mins[0] as f32, l.mins[1] as f32, l.mins[2] as f32),
                        Vec3::new(l.maxs[0] as f32, l.maxs[1] as f32, l.maxs[2] as f32),
                    )
                })
                .collect(),
            index,
            samples,
            head: self.models[0].head_node,
        }
    }

    /// Leaf index containing `p` (world model).
    pub fn leaf_at(&self, p: Vec3) -> usize {
        let mut n = self.models[0].head_node;
        while n >= 0 {
            let node = &self.nodes[n as usize];
            let pl = &self.planes[node.plane as usize];
            n = node.children[(pl.normal.dot(p) - pl.dist < 0.0) as usize];
        }
        (-1 - n) as usize
    }

    /// Union of the leaf bounds of the visibility cluster containing `p` (the 3D-skybox room around `sky_camera`).
    pub fn cluster_bounds_at(&self, p: Vec3) -> Option<(Vec3, Vec3)> {
        let cluster = self.leafs.get(self.leaf_at(p))?.cluster;
        if cluster < 0 {
            return None;
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for l in self.leafs.iter().filter(|l| l.cluster == cluster) {
            lo = lo.min(Vec3::new(
                l.mins[0] as f32,
                l.mins[1] as f32,
                l.mins[2] as f32,
            ));
            hi = hi.max(Vec3::new(
                l.maxs[0] as f32,
                l.maxs[1] as f32,
                l.maxs[2] as f32,
            ));
        }
        Some((lo - Vec3::splat(8.0), hi + Vec3::splat(8.0)))
    }

    pub fn world_bounds(&self) -> (Vec3, Vec3) {
        let m = &self.models[0];
        (m.mins, m.maxs)
    }

    /// Embedded PAK (zip) lump: maps ship their own VMT/VTF here.
    pub fn pakfile(&self) -> &[u8] {
        self.lump(LUMP_PAKFILE)
    }

    pub fn face_texname(&self, f: &Face) -> &str {
        let ti = self.texinfo[f.texinfo.max(0) as usize];
        let td = self.texdata[ti.texdata.max(0) as usize];
        &self.texnames[td.name_id as usize]
    }

    fn face_verts(&self, f: &Face) -> Vec<Vec3> {
        let mut out = Vec::with_capacity(f.num_edges as usize);
        for i in 0..f.num_edges as i32 {
            let se = self.surfedges[(f.first_edge + i) as usize];
            let e = self.edges[se.unsigned_abs() as usize];
            let v = if se >= 0 { e[0] } else { e[1] };
            out.push(self.vertexes[v as usize]);
        }
        out
    }

    /// Build per-material triangle batches for one brush model (0 = world) plus the lightmap atlas.
    pub fn build_mesh(&self, model: usize) -> (Vec<MaterialBatch>, LightmapAtlas) {
        let (mut v, atlas) = self.build_meshes(&[model]);
        (v.remove(0), atlas)
    }

    /// Several brush models sharing one lightmap atlas (world + brush entities). One batch list per model.
    pub fn build_meshes(&self, models: &[usize]) -> (Vec<Vec<MaterialBatch>>, LightmapAtlas) {
        let lighting: &[u8] = if self.hdr {
            self.lump(LUMP_LIGHTING_HDR)
        } else {
            self.lump(LUMP_LIGHTING)
        };
        let mut packer = AtlasPacker::new(2048);
        let mut results = Vec::with_capacity(models.len());
        for &model in models {
            let m = &self.models[model];
            let mut batches: HashMap<String, MaterialBatch> = HashMap::new();

            for fi in m.first_face..m.first_face + m.num_faces {
                let f = &self.faces[fi as usize];
                if f.texinfo < 0 || f.num_edges < 3 {
                    continue;
                }
                let ti = self.texinfo[f.texinfo as usize];
                if ti.flags
                    & (SURF_SKY2D | SURF_SKY | SURF_NODRAW | SURF_HINT | SURF_SKIP | SURF_TRIGGER)
                    != 0
                {
                    continue;
                }
                let td = self.texdata[ti.texdata as usize];
                let name = &self.texnames[td.name_id as usize];
                if name.starts_with("tools/") {
                    continue;
                }
                let plane = self.planes[f.plane as usize];
                let normal = if f.side != 0 {
                    -plane.normal
                } else {
                    plane.normal
                };
                let verts = self.face_verts(f);

                // Lightmap rect (style 0 only for now).
                let (lw, lh) = ((f.lm_size[0] + 1) as u32, (f.lm_size[1] + 1) as u32);
                let has_light =
                    f.light_ofs >= 0 && ti.flags & SURF_NOLIGHT == 0 && f.styles[0] != 255;
                let rect = if has_light
                    && (f.light_ofs as usize + (lw * lh * 4) as usize) <= lighting.len()
                {
                    let src = &lighting[f.light_ofs as usize..][..(lw * lh * 4) as usize];
                    packer.add(lw, lh, src)
                } else {
                    packer.add_solid(lw, lh)
                };
                let uvs = |p: Vec3| -> ([f32; 2], [f32; 2]) {
                    let s = p.dot(Vec3::from_slice(&ti.tex_vecs[0][..3])) + ti.tex_vecs[0][3];
                    let t = p.dot(Vec3::from_slice(&ti.tex_vecs[1][..3])) + ti.tex_vecs[1][3];
                    let ls = p.dot(Vec3::from_slice(&ti.lm_vecs[0][..3])) + ti.lm_vecs[0][3]
                        - f.lm_mins[0] as f32;
                    let lt = p.dot(Vec3::from_slice(&ti.lm_vecs[1][..3])) + ti.lm_vecs[1][3]
                        - f.lm_mins[1] as f32;
                    (
                        [s / td.width.max(1) as f32, t / td.height.max(1) as f32],
                        [ls + 0.5, lt + 0.5], // in luxels; remapped into atlas space below
                    )
                };

                let batch = batches
                    .entry(name.clone())
                    .or_insert_with(|| MaterialBatch {
                        material: name.clone(),
                        ..Default::default()
                    });
                let base = batch.vertices.len() as u32;

                if f.dispinfo >= 0 {
                    let di = &self.dispinfo[f.dispinfo as usize];
                    let power = di.power;
                    let n = (1usize << power) + 1;
                    if verts.len() != 4 {
                        continue;
                    }
                    let Some((c, grid)) = self.displacement_grid(di, &verts) else {
                        continue;
                    };
                    let (left_edge, right_edge) = (c[1] - c[0], c[2] - c[3]);
                    for y in 0..n {
                        for x in 0..n {
                            let p = grid[y * n + x];
                            // Texture coords come from the flat quad position, lightmap coords are parametric.
                            let fx = x as f32 / (n - 1) as f32;
                            let fy = y as f32 / (n - 1) as f32;
                            let flat = c[0]
                                + left_edge * fy
                                + ((c[3] + right_edge * fy) - (c[0] + left_edge * fy)) * fx;
                            let (uv, _) = uvs(flat);
                            let lm = [fx * (lw - 1) as f32 + 0.5, fy * (lh - 1) as f32 + 0.5];
                            let nrm = disp_normal(&grid, n, x, y);
                            let dv = self.lump(LUMP_DISPVERTS);
                            let ai = (di.vert_start as usize + y * n + x) * 20 + 16;
                            let blend = if ai + 4 <= dv.len() {
                                (f32_at(dv, ai) / 255.0).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            batch.vertices.push(WorldVertex {
                                pos: p.to_array(),
                                normal: nrm.to_array(),
                                uv,
                                lm_uv: lm,
                                blend,
                            });
                        }
                    }
                    for y in 0..n - 1 {
                        for x in 0..n - 1 {
                            let i0 = base + (y * n + x) as u32;
                            let (i1, i2, i3) = (i0 + 1, i0 + n as u32, i0 + n as u32 + 1);
                            if (x + y) % 2 == 0 {
                                batch.indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
                            } else {
                                batch.indices.extend_from_slice(&[i0, i2, i3, i0, i3, i1]);
                            }
                        }
                    }
                    // Flip winding if the grid orientation disagrees with the face normal.
                    let tail = batch.indices.len() - (n - 1) * (n - 1) * 6;
                    let (a, b2, c2) = (
                        batch.vertices[batch.indices[tail] as usize].pos,
                        batch.vertices[batch.indices[tail + 1] as usize].pos,
                        batch.vertices[batch.indices[tail + 2] as usize].pos,
                    );
                    let tn = (Vec3::from(b2) - Vec3::from(a)).cross(Vec3::from(c2) - Vec3::from(a));
                    // Front faces are clockwise, i.e. the right-handed triangle normal opposes the face normal.
                    if tn.dot(normal) > 0.0 {
                        for tri in batch.indices[tail..].chunks_exact_mut(3) {
                            tri.swap(1, 2);
                        }
                    }
                } else {
                    for p in &verts {
                        let (uv, lm) = uvs(*p);
                        batch.vertices.push(WorldVertex {
                            pos: p.to_array(),
                            normal: normal.to_array(),
                            uv,
                            lm_uv: lm,
                            blend: 0.0,
                        });
                    }
                    for i in 1..verts.len() as u32 - 1 {
                        batch
                            .indices
                            .extend_from_slice(&[base, base + i, base + i + 1]);
                    }
                }
                // Remap lightmap coordinates from luxels into atlas UV space.
                let aw = packer.size as f32;
                for v in &mut batch.vertices[base as usize..] {
                    v.lm_uv = [
                        (rect.0 as f32 + v.lm_uv[0].clamp(0.5, lw as f32 - 0.5)) / aw,
                        (rect.1 as f32 + v.lm_uv[1].clamp(0.5, lh as f32 - 0.5)) / aw,
                    ];
                }
            }
            let mut out: Vec<_> = batches.into_values().collect();
            out.sort_by(|a, b| a.material.cmp(&b.material));
            results.push(out);
        }
        (results, packer.finish())
    }
}

fn disp_normal(grid: &[Vec3], n: usize, x: usize, y: usize) -> Vec3 {
    let at = |xx: usize, yy: usize| grid[yy * n + xx];
    let xl = at(x.saturating_sub(1), y);
    let xr = at((x + 1).min(n - 1), y);
    let yl = at(x, y.saturating_sub(1));
    let yr = at(x, (y + 1).min(n - 1));
    let nrm = (xr - xl).cross(yr - yl);
    if nrm.length_squared() < 1e-8 {
        Vec3::Z
    } else {
        nrm.normalize()
    }
}

pub fn parse_entities(s: &str) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut cur: Option<Entity> = None;
    // The installed Last Stand entity patch uses bare CR line endings.
    for line in s.split(['\r', '\n']) {
        let line = line.trim().trim_end_matches('\0');
        if line == "{" {
            cur = Some(Entity::default());
        } else if line == "}" {
            if let Some(e) = cur.take() {
                out.push(e);
            }
        } else if let Some(e) = cur.as_mut() {
            // "key" "value"
            let mut it = line.split('"');
            let _ = it.next();
            if let (Some(k), Some(_), Some(v)) = (it.next(), it.next(), it.next()) {
                e.0.push((k.to_string(), v.to_string()));
            }
        }
    }
    out
}

fn f32_to_f16(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let man = b & 0x7f_ffff;
    if exp >= 31 {
        return sign | 0x7bff; // clamp to max finite
    }
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (man | 0x80_0000) >> (1 - exp);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    sign | ((exp as u16) << 10) | ((man + 0x1000) >> 13) as u16
}

struct AtlasPacker {
    size: u32,
    /// RGBA float16 texels (linear HDR light).
    pixels: Vec<u16>,
    x: u32,
    y: u32,
    row_h: u32,
}

impl AtlasPacker {
    fn new(size: u32) -> Self {
        let mut pixels = vec![0u16; (size * size * 4) as usize];
        let one = f32_to_f16(1.0);
        // (0,0) 2x2 is reserved as a white texel block for unlit faces.
        for y in 0..2 {
            for x in 0..2 {
                pixels[((y * size + x) * 4) as usize..][..4].copy_from_slice(&[one, one, one, one]);
            }
        }
        Self {
            size,
            pixels,
            x: 2,
            y: 0,
            row_h: 2,
        }
    }

    fn alloc(&mut self, w: u32, h: u32) -> (u32, u32) {
        if self.x + w + 1 > self.size {
            self.x = 0;
            self.y += self.row_h + 1;
            self.row_h = 0;
        }
        let pos = (self.x, self.y);
        if self.y + h + 1 > self.size {
            // Atlas full: fall back to the white block rather than corrupt memory.
            return (0, 0);
        }
        self.x += w + 1;
        self.row_h = self.row_h.max(h);
        pos
    }

    fn add_solid(&mut self, _w: u32, _h: u32) -> (u32, u32) {
        (0, 0)
    }

    /// Source lightmaps are ColorRGBExp32: rgb mantissa + signed exponent, light = c/255 * 2^exp (linear HDR; exposure and
    /// tone mapping happen in the post pass). Checked empirically: with this scale the auto-exposure the post pass picks
    /// lands inside the maps' declared range (1..5/6), without the /255 it saturates at the minimum.
    fn add(&mut self, w: u32, h: u32, src: &[u8]) -> (u32, u32) {
        let (px, py) = self.alloc(w, h);
        if (px, py) == (0, 0) {
            return (0, 0);
        }
        for y in 0..h {
            for x in 0..w {
                let s = &src[((y * w + x) * 4) as usize..][..4];
                let scale = 2f32.powi(s[3] as i8 as i32);
                let conv = |c: u8| f32_to_f16(((c as f32 / 255.0) * scale).min(60000.0));
                let o = (((py + y) * self.size + px + x) * 4) as usize;
                self.pixels[o..o + 4].copy_from_slice(&[
                    conv(s[0]),
                    conv(s[1]),
                    conv(s[2]),
                    f32_to_f16(1.0),
                ]);
            }
        }
        (px, py)
    }

    fn finish(self) -> LightmapAtlas {
        let mut bytes = Vec::with_capacity(self.pixels.len() * 2);
        for h in &self.pixels {
            bytes.extend_from_slice(&h.to_le_bytes());
        }
        LightmapAtlas {
            width: self.size,
            height: self.size,
            rgba: bytes,
        }
    }
}
