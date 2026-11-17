//! Static Source model loader (.mdl v44-49 + .vvd + .dx90.vtx), LOD 0, bind pose only.
//!
//! Layout notes (verified against L4D2 files, see `tests/mdl_real.rs`):
//! * MDL `mstudiomodel_t::vertexindex` is a *byte* offset into the VVD vertex array (48 bytes/vertex),
//!   `mstudiomesh_t::vertexoffset` is a vertex index relative to that model's first vertex.
//! * VTX strip-group vertices carry `origMeshVertID` (relative to the mesh), strip-group indices
//!   index the strip-group vertex table.
//! * VTX structs are `#pragma pack(1)`.

use anyhow::{anyhow, ensure, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ModelVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Clone, Debug, Default)]
pub struct ModelBatch {
    pub material_index: usize,
    /// Triangle list, indices into `ModelMesh::vertices`.
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct ModelMesh {
    /// For each LOD0 hardware (VTX) vertex, in VTX traversal order, the index into `vertices` (the order of baked static-prop lighting).
    pub vtx_map: Vec<u32>,
    /// Resolved material path (first `$cdmaterials` dir + texture name), lowercase, `/` separated, no extension.
    pub materials: Vec<String>,
    /// All `$cdmaterials` search dirs (lowercase, trailing `/`), for callers that want to probe alternatives.
    pub material_dirs: Vec<String>,
    /// Raw texture names from the MDL (lowercase).
    pub texture_names: Vec<String>,
    pub vertices: Vec<ModelVertex>,
    pub batches: Vec<ModelBatch>,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

impl ModelMesh {
    pub fn triangle_count(&self) -> usize {
        self.batches.iter().map(|b| b.indices.len() / 3).sum()
    }
}

struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn bytes(&self, off: usize, n: usize) -> Result<&'a [u8]> {
        let end = off
            .checked_add(n)
            .ok_or_else(|| anyhow!("offset overflow"))?;
        self.0
            .get(off..end)
            .ok_or_else(|| anyhow!("read {}..{} out of bounds ({})", off, end, self.0.len()))
    }
    fn i32(&self, off: usize) -> Result<i32> {
        Ok(i32::from_le_bytes(self.bytes(off, 4)?.try_into().unwrap()))
    }
    fn u16(&self, off: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(off, 2)?.try_into().unwrap()))
    }
    fn i16(&self, off: usize) -> Result<i16> {
        Ok(self.u16(off)? as i16)
    }
    fn u8(&self, off: usize) -> Result<u8> {
        Ok(self.bytes(off, 1)?[0])
    }
    fn f32(&self, off: usize) -> Result<f32> {
        Ok(f32::from_le_bytes(self.bytes(off, 4)?.try_into().unwrap()))
    }
    fn cstr(&self, off: usize) -> Result<String> {
        let s = self
            .0
            .get(off..)
            .ok_or_else(|| anyhow!("string offset {} out of bounds", off))?;
        let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
        Ok(String::from_utf8_lossy(&s[..end]).into_owned())
    }
    /// Offset helper: `base + rel` where rel is a (possibly negative) i32.
    fn rel(base: usize, rel: i32) -> Result<usize> {
        let v = base as i64 + rel as i64;
        ensure!(v >= 0, "negative offset");
        Ok(v as usize)
    }
}

fn norm_path(s: &str) -> String {
    s.to_ascii_lowercase().replace('\\', "/")
}

const VVD_VERTEX_SIZE: usize = 48;

/// Parse the VVD into a LOD-0 vertex array (fixups applied).
fn load_vvd(vvd: &[u8]) -> Result<Vec<ModelVertex>> {
    let r = Rd(vvd);
    ensure!(vvd.len() >= 64 && &vvd[0..4] == b"IDSV", "bad VVD magic");
    let version = r.i32(4)?;
    ensure!(version == 4, "unsupported VVD version {}", version);
    let num_lod_verts0 = r.i32(16)? as usize;
    let num_fixups = r.i32(48)? as usize;
    let fixup_start = r.i32(52)? as usize;
    let vert_start = r.i32(56)? as usize;

    let read_vert = |idx: usize| -> Result<ModelVertex> {
        let o = vert_start + idx * VVD_VERTEX_SIZE;
        Ok(ModelVertex {
            pos: [r.f32(o + 16)?, r.f32(o + 20)?, r.f32(o + 24)?],
            normal: [r.f32(o + 28)?, r.f32(o + 32)?, r.f32(o + 36)?],
            uv: [r.f32(o + 40)?, r.f32(o + 44)?],
        })
    };

    let mut out = Vec::new();
    if num_fixups == 0 {
        for i in 0..num_lod_verts0 {
            out.push(read_vert(i)?);
        }
    } else {
        for f in 0..num_fixups {
            let fo = fixup_start + f * 12;
            let lod = r.i32(fo)?;
            let src = r.i32(fo + 4)? as usize;
            let n = r.i32(fo + 8)? as usize;
            // A fixup applies to LOD 0 when its lod >= 0 (it is used for every LOD <= fixup.lod).
            if lod < 0 {
                continue;
            }
            for i in 0..n {
                out.push(read_vert(src + i)?);
            }
        }
    }
    Ok(out)
}

/// Load a static model at LOD 0, skin family 0, all body parts (first sub-model of each).
pub fn load_static(mdl: &[u8], vvd: &[u8], vtx: &[u8]) -> Result<ModelMesh> {
    let m = Rd(mdl);
    ensure!(mdl.len() >= 408 && &mdl[0..4] == b"IDST", "bad MDL magic");
    let version = m.i32(4)?;
    ensure!(
        (44..=49).contains(&version),
        "unsupported MDL version {}",
        version
    );

    let mut mesh = ModelMesh::default();
    mesh.mins = [m.f32(104)?, m.f32(108)?, m.f32(112)?];
    mesh.maxs = [m.f32(116)?, m.f32(120)?, m.f32(124)?];

    // Textures / cdmaterials / skin table.
    let num_tex = m.i32(204)? as usize;
    let tex_idx = m.i32(208)? as usize;
    let num_cd = m.i32(212)? as usize;
    let cd_idx = m.i32(216)? as usize;
    let num_skinref = m.i32(220)? as usize;
    let num_skinfam = m.i32(224)? as usize;
    let skin_idx = m.i32(228)? as usize;
    let num_bodyparts = m.i32(232)? as usize;
    let bodypart_idx = m.i32(236)? as usize;

    for i in 0..num_cd {
        let off = m.i32(cd_idx + i * 4)? as usize;
        let mut d = norm_path(&m.cstr(off)?);
        if !d.is_empty() && !d.ends_with('/') {
            d.push('/');
        }
        mesh.material_dirs.push(d);
    }
    for i in 0..num_tex {
        let base = tex_idx + i * 64;
        let name_off = Rd::rel(base, m.i32(base)?)?;
        mesh.texture_names.push(norm_path(&m.cstr(name_off)?));
    }
    let dir0 = mesh.material_dirs.first().cloned().unwrap_or_default();
    for t in &mesh.texture_names {
        // Names that already contain a path separator are rooted at materials/ by the engine too.
        mesh.materials.push(format!("{}{}", dir0, t));
    }
    // skin family 0: skinref -> texture index
    let skin0: Vec<usize> = if num_skinfam > 0 {
        (0..num_skinref)
            .map(|i| m.i16(skin_idx + i * 2).map(|v| v.max(0) as usize))
            .collect::<Result<_>>()?
    } else {
        (0..num_skinref).collect()
    };

    mesh.vertices = load_vvd(vvd)?;
    let nverts = mesh.vertices.len();

    // VTX header
    let v = Rd(vtx);
    ensure!(vtx.len() >= 36, "VTX too small");
    let vtx_ver = v.i32(0)?;
    ensure!(vtx_ver == 7, "unsupported VTX version {}", vtx_ver);
    let vtx_nlods = v.i32(20)?;
    ensure!(vtx_nlods >= 1, "VTX has no LODs");
    let vtx_nbp = v.i32(28)? as usize;
    let vtx_bp_off = v.i32(32)? as usize;
    ensure!(
        vtx_nbp == num_bodyparts,
        "VTX/MDL bodypart count mismatch ({} vs {})",
        vtx_nbp,
        num_bodyparts
    );

    // optimize.h packed sizes
    const BP_SZ: usize = 8;
    const MESH_SZ: usize = 9;
    const SG_SZ: usize = 25;
    const STRIP_SZ: usize = 27;
    const SVERT_SZ: usize = 9;

    for bp in 0..num_bodyparts {
        // MDL body part
        let mbp = bodypart_idx + bp * 16;
        let m_nmodels = m.i32(mbp + 4)? as usize;
        let m_model_idx = Rd::rel(mbp, m.i32(mbp + 12)?)?;
        if m_nmodels == 0 {
            continue;
        }
        // first model only
        let mmodel = m_model_idx;
        let m_nmeshes = m.i32(mmodel + 72)? as usize;
        let m_mesh_idx = Rd::rel(mmodel, m.i32(mmodel + 76)?)?;
        let m_numverts = m.i32(mmodel + 80)? as usize;
        let m_vert_byte = m.i32(mmodel + 84)? as usize;
        if m_numverts == 0 {
            continue;
        }
        let model_vstart = m_vert_byte / VVD_VERTEX_SIZE;

        // VTX body part -> model 0 -> lod 0
        let vbp = vtx_bp_off + bp * BP_SZ;
        let v_nmodels = v.i32(vbp)?;
        let v_model_off = Rd::rel(vbp, v.i32(vbp + 4)?)?;
        if v_nmodels < 1 {
            continue;
        }
        let vmodel = v_model_off;
        let v_nlods = v.i32(vmodel)?;
        if v_nlods < 1 {
            continue;
        }
        let vlod = Rd::rel(vmodel, v.i32(vmodel + 4)?)?;
        let v_nmeshes = v.i32(vlod)? as usize;
        let v_mesh_off = Rd::rel(vlod, v.i32(vlod + 4)?)?;
        ensure!(
            v_nmeshes == m_nmeshes,
            "VTX/MDL mesh count mismatch ({} vs {})",
            v_nmeshes,
            m_nmeshes
        );

        for mi in 0..m_nmeshes {
            let mm = m_mesh_idx + mi * 116;
            let mat_ref = m.i32(mm)? as usize;
            let mesh_vert_off = m.i32(mm + 12)? as usize;
            let material_index = skin0.get(mat_ref).copied().unwrap_or(mat_ref);

            let vm = v_mesh_off + mi * MESH_SZ;
            let nsg = v.i32(vm)? as usize;
            let sg_off = Rd::rel(vm, v.i32(vm + 4)?)?;
            let mut batch = ModelBatch {
                material_index,
                indices: Vec::new(),
            };
            for sgi in 0..nsg {
                let sg = sg_off + sgi * SG_SZ;
                let n_verts = v.i32(sg)? as usize;
                let vert_off = Rd::rel(sg, v.i32(sg + 4)?)?;
                let n_idx = v.i32(sg + 8)? as usize;
                let idx_off = Rd::rel(sg, v.i32(sg + 12)?)?;
                let n_strips = v.i32(sg + 16)? as usize;
                let strip_off = Rd::rel(sg, v.i32(sg + 20)?)?;

                // stripgroup vertex table -> global vertex index
                let mut table = Vec::with_capacity(n_verts);
                for i in 0..n_verts {
                    let orig = v.u16(vert_off + i * SVERT_SZ + 4)? as usize;
                    let g = model_vstart + mesh_vert_off + orig;
                    ensure!(g < nverts, "vertex {} out of range ({} verts)", g, nverts);
                    table.push(g as u32);
                }
                mesh.vtx_map.extend_from_slice(&table);
                for s in 0..n_strips {
                    let so = strip_off + s * STRIP_SZ;
                    let s_nidx = v.i32(so)? as usize;
                    let s_idx_off = v.i32(so + 4)? as usize;
                    let flags = v.u8(so + 18)?;
                    ensure!(
                        s_idx_off + s_nidx <= n_idx,
                        "strip index range out of bounds"
                    );
                    let mut ids = Vec::with_capacity(s_nidx);
                    for k in 0..s_nidx {
                        let li = v.u16(idx_off + (s_idx_off + k) * 2)? as usize;
                        ensure!(li < table.len(), "strip index {} >= {}", li, table.len());
                        ids.push(table[li]);
                    }
                    if flags & 2 != 0 {
                        // STRIP_IS_TRISTRIP
                        for k in 0..ids.len().saturating_sub(2) {
                            let (a, b, c) = if k % 2 == 0 {
                                (ids[k], ids[k + 1], ids[k + 2])
                            } else {
                                (ids[k + 1], ids[k], ids[k + 2])
                            };
                            if a != b && b != c && a != c {
                                batch.indices.extend([a, b, c]);
                            }
                        }
                    } else {
                        // STRIP_IS_TRILIST (flag 1). Some shipped models carry flags == 0 for what is
                        // still a plain triangle list (idx count divisible by 3), so treat that as a list too.
                        ensure!(
                            ids.len() % 3 == 0,
                            "trilist strip with {} indices (flags {:#x})",
                            ids.len(),
                            flags
                        );
                        batch.indices.extend_from_slice(&ids);
                    }
                }
            }
            if !batch.indices.is_empty() {
                mesh.batches.push(batch);
            }
        }
    }
    Ok(mesh)
}

/// Read a model's VTX. Later builds ship `<base>.dx90.vtx`; the 2009 retail VPK ships plain `<base>.vtx`.
/// UNVERIFIED: the real lookup order lives in studiorender/datacache; confirm in Ghidra before relying on it for mods.
pub fn read_vtx(base: &str, mut read: impl FnMut(&str) -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    read(&format!("{base}.dx90.vtx")).or_else(|| read(&format!("{base}.vtx")))
}

/// Collision/view bounds from the `studiohdr_t` header (no mesh loading): `(hull_min, hull_max, view_bbmin, view_bbmax)`.
pub fn read_header_bounds(mdl: &[u8]) -> Option<([f32; 3], [f32; 3], [f32; 3], [f32; 3])> {
    if mdl.len() < 152 || &mdl[0..4] != b"IDST" {
        return None;
    }
    let f = |o: usize| f32::from_le_bytes(mdl[o..o + 4].try_into().unwrap());
    let v = |o: usize| [f(o), f(o + 4), f(o + 8)];
    Some((v(104), v(116), v(128), v(140)))
}
