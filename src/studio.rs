//! Initial-pose Source MDL/VVD/VTX mesh loading; animation is handled separately.
use crate::{binary::*, source_assets::SourceAssets};
use std::io;
#[path = "studio_pose.rs"]
pub mod pose;
#[derive(Clone)]
pub struct ModelMesh {
    pub checksum: u32,
    pub lod: usize,
    pub lighting_indices: Vec<(usize, usize)>,
    pub material: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub weights: Vec<([u8; 3], [f32; 3])>,
}
fn table(b: &[u8], base: usize, field: usize, index: usize, stride: usize) -> io::Result<usize> {
    let offset = relative(b, base, field)?;
    let at = offset
        .checked_add(
            index
                .checked_mul(stride)
                .ok_or_else(|| invalid("Model table overflow"))?,
        )
        .ok_or_else(|| invalid("Model table overflow"))?;
    range(b, at, stride)?;
    Ok(at)
}
fn triangles(indices: &[usize], flags: u8) -> io::Result<Vec<[usize; 3]>> {
    if flags & 1 != 0 {
        if indices.len() % 3 != 0 {
            return Err(invalid("Incomplete VTX triangle list"));
        }
        Ok(indices
            .chunks_exact(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect())
    } else if flags & 2 != 0 {
        Ok(indices
            .windows(3)
            .enumerate()
            .filter_map(|(i, v)| {
                if v[0] == v[1] || v[0] == v[2] || v[1] == v[2] {
                    None
                } else {
                    Some(if i % 2 == 0 {
                        [v[0], v[1], v[2]]
                    } else {
                        [v[1], v[0], v[2]]
                    })
                }
            })
            .collect())
    } else if indices.len() % 3 == 0 {
        // No strip flag: Source draws anything that is not a tristrip as a
        // list (c8m1's models/props_doors/doordm01_01 has flags 0; the map
        // failed to load).
        Ok(indices.chunks_exact(3).map(|v| [v[0], v[1], v[2]]).collect())
    } else {
        Err(invalid("Unknown VTX strip topology"))
    }
}
pub fn load(
    assets: &mut SourceAssets,
    path: &str,
    skin: usize,
    body: usize,
) -> io::Result<Vec<ModelMesh>> {
    load_lod(assets, path, skin, body, 0)
}
/// Rigid animated props still need their authored sequence-zero/closed pose.
/// Reload bind vertices before skinning so repeated preparation cannot compound it.
pub fn load_posed_prop(assets: &mut SourceAssets, path: &str) -> io::Result<Vec<ModelMesh>> {
    let mut draws=load(assets,path,0,0)?;
    if !draws.is_empty() {
        let pose=pose::sample_l4d2r(assets,path,&["closed","idle"],0.)?;
        pose::apply(assets,path,&mut draws,&pose,false)?;
    }
    Ok(draws)
}
pub fn load_lod(
    assets: &mut SourceAssets,
    path: &str,
    skin: usize,
    body: usize,
    lod: usize,
) -> io::Result<Vec<ModelMesh>> {
    let mdl = assets.read(path)?;
    let base = path.trim_end_matches(".mdl");
    let vvd = assets.read(&format!("{base}.vvd"))?;
    let vtx = assets.read(&format!("{base}.dx90.vtx"))?;
    parse(assets, &mdl, &vvd, &vtx, skin, body, lod).map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))
}
/// Muzzle direction in the same bind-space coordinates as the loaded VVD mesh.
pub fn muzzle_forward(assets: &mut SourceAssets, path: &str) -> io::Result<[f32; 3]> {
    let mdl = assets.read(path)?;
    parse_muzzle(&mdl)
}
fn parse_muzzle(mdl: &[u8]) -> io::Result<[f32; 3]> {
    if range(&mdl, 0, 4)? != b"IDST" || !matches!(u32_at(&mdl, 4)?, 48 | 49) {
        return Err(invalid("Unsupported muzzle model"));
    }
    let bones = count(&mdl, 156, 1024)?;
    let bone_base = count(&mdl, 160, mdl.len())?;
    let attachments = count(&mdl, 240, 4096)?;
    let start = count(&mdl, 244, mdl.len())?;
    for i in 0..attachments {
        let at = start + i * 92;
        range(&mdl, at, 92)?;
        let name = cstring(&mdl, relative(&mdl, at, 0)?)?;
        if !name.to_lowercase().contains("muzzle") {
            continue;
        }
        let bone = count(&mdl, at + 8, bones.saturating_sub(1))?;
        if bones == 0 {
            return Err(invalid("Muzzle attachment has no bone"));
        }
        let pose = bone_base + bone * 216 + 96;
        range(&mdl, pose, 48)?;
        // poseToBone maps bind-space to bone-local space. Its rigid rotation's
        // transpose brings the attachment's X axis back to mesh bind space.
        let mut source = [0.; 3];
        for (axis, value) in source.iter_mut().enumerate() {
            for row in 0..3 {
                *value +=
                    f32_at(&mdl, pose + (row * 4 + axis) * 4)? * f32_at(&mdl, at + 12 + row * 16)?;
            }
        }
        let forward = source_normal(source);
        let length = forward.iter().map(|v| v * v).sum::<f32>().sqrt();
        if length < 0.0001 {
            return Err(invalid("Degenerate muzzle axis"));
        }
        return Ok(forward.map(|v| v / length));
    }
    Err(invalid("No native muzzle attachment"))
}
fn parse(
    assets: &mut SourceAssets,
    mdl: &[u8],
    vvd: &[u8],
    vtx: &[u8],
    skin: usize,
    body: usize,
    requested_lod: usize,
) -> io::Result<Vec<ModelMesh>> {
    if range(mdl, 0, 4)? != b"IDST"
        // 44-47: older studio headers whose body part/model/mesh/texture
        // tables match 48 (c13m1's militiawindow02_breakable_chunk_01 is 44).
        || !matches!(u32_at(mdl, 4)?, 44..=49)
        || range(vvd, 0, 4)? != b"IDSV"
        || u32_at(vvd, 4)? != 4
        || u32_at(vtx, 0)? != 7
    {
        return Err(invalid("Unsupported Source studio model version"));
    }
    let checksum = u32_at(mdl, 8)?;
    if checksum != u32_at(vvd, 8)? || checksum != u32_at(vtx, 16)? {
        return Err(invalid("MDL/VVD/VTX checksum mismatch"));
    }
    let vertex_count = count(vvd, 16, 2_000_000)?;
    let vertex_base = count(vvd, 56, vvd.len())?;
    let fixups = count(vvd, 48, 100_000)?;
    let mut vertex_map = vec![];
    if fixups == 0 {
        vertex_map.extend(0..vertex_count);
    } else {
        let offset = count(vvd, 52, vvd.len())?;
        for i in 0..fixups {
            let r = range(vvd, offset + i * 12, 12)?;
            let start = count(r, 4, 2_000_000)?;
            let n = count(r, 8, 2_000_000)?;
            if vertex_map.len() + n > vertex_count {
                return Err(invalid("VVD fixup exceeds LOD0 vertex count"));
            }
            range(vvd, vertex_base + start * 48, n * 48)?;
            vertex_map.extend(start..start + n);
        }
        if vertex_map.len() != vertex_count {
            return Err(invalid("Incomplete VVD LOD0 fixups"));
        }
    }
    let tex_count = count(mdl, 204, 4096)?;
    let tex_offset = count(mdl, 208, mdl.len())?;
    let mut textures = vec![];
    for i in 0..tex_count {
        let at = tex_offset + i * 64;
        range(mdl, at, 64)?;
        textures.push(cstring(mdl, relative(mdl, at, 0)?)?);
    }
    let dir_count = count(mdl, 212, 1024)?;
    let dir_offset = count(mdl, 216, mdl.len())?;
    let mut dirs = vec![];
    for i in 0..dir_count {
        dirs.push(cstring(mdl, count(mdl, dir_offset + i * 4, mdl.len())?)?);
    }
    if dirs.is_empty() {
        dirs.push(String::new());
    }
    let refs = count(mdl, 220, 4096)?;
    let families = count(mdl, 224, 4096)?;
    let skins = count(mdl, 228, mdl.len())?;
    let selected = if skin < families { skin } else { 0 };
    let bodies = count(mdl, 232, 256)?;
    let body_base = count(mdl, 236, mdl.len())?;
    let vtx_bodies = count(vtx, 28, 256)?;
    let vtx_body_base = count(vtx, 32, vtx.len())?;
    if vtx_bodies != bodies {
        return Err(invalid("MDL/VTX bodypart count mismatch"));
    }
    let mut output = vec![];
    let mut lighting_mesh = 0;
    for i in 0..bodies {
        let bp = body_base + i * 16;
        range(mdl, bp, 16)?;
        let model_count = count(mdl, bp + 4, 256)?;
        if model_count == 0 {
            continue;
        }
        let divisor = count(mdl, bp + 8, 1_000_000)?;
        let selected_model = if divisor == 0 {
            0
        } else {
            body / divisor % model_count
        };
        let model = table(mdl, bp, 12, selected_model, 148)?;
        let vbp = vtx_body_base + i * 8;
        range(vtx, vbp, 8)?;
        if count(vtx, vbp, 256)? != model_count {
            return Err(invalid("MDL/VTX model count mismatch"));
        }
        let vm = table(vtx, vbp, 4, selected_model, 8)?;
        if count(vtx, vm, 8)? == 0 {
            continue;
        }
        let lod_count = count(vtx, vm, 8)?;
        let selected_lod = requested_lod.min(lod_count.saturating_sub(1));
        let lod = table(
            vtx,
            vm,
            4,
            requested_lod.min(lod_count.saturating_sub(1)),
            12,
        )?;
        let meshes = count(mdl, model + 72, 4096)?;
        if meshes != count(vtx, lod, 4096)? {
            return Err(invalid("MDL/VTX mesh count mismatch"));
        }
        let model_vertex = count(mdl, model + 84, 100_000_000)?;
        if model_vertex % 48 != 0 {
            return Err(invalid("Unaligned MDL vertex offset"));
        }
        for mesh_index in 0..meshes {
            let mesh = table(mdl, model, 76, mesh_index, 116)?;
            let slot = count(mdl, mesh, 4096)?;
            let local_count = count(mdl, mesh + 8, 2_000_000)?;
            let local_base = count(mdl, mesh + 12, 2_000_000)?;
            let texture = if refs > 0 && families > 0 {
                if slot >= refs {
                    return Err(invalid("Mesh skin slot outside skin table"));
                }
                u16_at(mdl, skins + (selected * refs + slot) * 2)? as usize
            } else {
                slot
            };
            let texture = textures
                .get(texture)
                .ok_or_else(|| invalid("MDL texture index outside material table"))?;
            let mut material = format!("{}{}", dirs[0], texture).replace('\\', "/");
            for dir in &dirs {
                let candidate = format!("{dir}{texture}").replace('\\', "/");
                if assets.exists(&format!("materials/{candidate}.vmt")) {
                    material = candidate;
                    break;
                }
            }
            let mut draw = ModelMesh {
                checksum,
                lod: selected_lod,
                lighting_indices: vec![],
                material: material.to_lowercase(),
                positions: vec![],
                normals: vec![],
                uv: vec![],
                weights: vec![],
            };
            let vmesh = table(vtx, lod, 4, mesh_index, 9)?;
            let groups = count(vtx, vmesh, 65536)?;
            for group_index in 0..groups {
                let group = table(vtx, vmesh, 4, group_index, 25)?;
                let verts = count(vtx, group, 2_000_000)?;
                let idx_count = count(vtx, group + 8, 6_000_000)?;
                let vertex_offset = relative(vtx, group, 4)?;
                range(vtx, vertex_offset, verts * 9)?;
                let index_offset = relative(vtx, group, 12)?;
                range(vtx, index_offset, idx_count * 2)?;
                for strip_index in 0..count(vtx, group + 16, 65536)? {
                    let strip = table(vtx, group, 20, strip_index, 27)?;
                    let n = count(vtx, strip, 6_000_000)?;
                    let first = count(vtx, strip + 4, idx_count)?;
                    if first + n > idx_count {
                        return Err(invalid("VTX strip outside index buffer"));
                    }
                    let mut indices = vec![];
                    for k in first..first + n {
                        let index = u16_at(vtx, index_offset + k * 2)? as usize;
                        if index >= verts {
                            return Err(invalid("VTX index outside vertex buffer"));
                        }
                        let original = u16_at(vtx, vertex_offset + index * 9 + 4)? as usize;
                        if original >= local_count {
                            return Err(invalid("VTX vertex outside MDL mesh"));
                        }
                        let global = model_vertex / 48 + local_base + original;
                        let _ = vertex_map
                            .get(global)
                            .ok_or_else(|| invalid("VTX vertex outside VVD fixup map"))?;
                        indices.push(index);
                    }
                    for triangle in triangles(&indices, range(vtx, strip + 18, 1)?[0])? {
                        for index in triangle {
                            draw.lighting_indices.push((lighting_mesh, index));
                            let original = u16_at(vtx, vertex_offset + index * 9 + 4)? as usize;
                            let global = model_vertex / 48 + local_base + original;
                            let index = vertex_map[global];
                            let vertex = range(vvd, vertex_base + index * 48, 48)?;
                            draw.positions.push(source_position(vec3(vertex, 16)?));
                            draw.normals.push(source_normal(vec3(vertex, 28)?));
                            // Eye meshes use a generated iris projection, rather than authored face UVs.
                            // MDL eyeball layout: Valve's public studio.h, mstudioeyeball_t (172 bytes).
                            let uv = if u32_at(mdl, mesh + 24)? == 1 {
                                let eye_index = count(mdl, mesh + 28, 256)?;
                                let eye_count = count(mdl, model + 100, 256)?;
                                if eye_index < eye_count {
                                    let eye = table(mdl, model, 104, eye_index, 172)?;
                                    let bone = count(mdl, eye + 4, 1024)?;
                                    let bone_base = count(mdl, 160, mdl.len())? + bone * 216 + 96;
                                    range(mdl, bone_base, 48)?;
                                    let source = bevy::math::Vec3::from_array(vec3(vertex, 16)?);
                                    let local =
                                        bevy::math::Vec3::from_array(std::array::from_fn(|row| {
                                            let at = bone_base + row * 16;
                                            f32_at(mdl, at).unwrap() * source.x
                                                + f32_at(mdl, at + 4).unwrap() * source.y
                                                + f32_at(mdl, at + 8).unwrap() * source.z
                                                + f32_at(mdl, at + 12).unwrap()
                                        }));
                                    let center = bevy::math::Vec3::from_array(vec3(mdl, eye + 8)?);
                                    let up = bevy::math::Vec3::from_array(vec3(mdl, eye + 28)?)
                                        .normalize_or_zero();
                                    let forward =
                                        bevy::math::Vec3::from_array(vec3(mdl, eye + 40)?)
                                            .normalize_or_zero();
                                    let right = forward.cross(up).normalize_or_zero();
                                    let scale = f32_at(mdl, eye + 60)?;
                                    [
                                        (0.5 + (local - center).dot(right) * scale).clamp(0., 1.),
                                        (0.5 - (local - center).dot(up) * scale).clamp(0., 1.),
                                    ]
                                } else {
                                    [f32_at(vertex, 40)?, f32_at(vertex, 44)?]
                                }
                            } else {
                                [f32_at(vertex, 40)?, f32_at(vertex, 44)?]
                            };
                            draw.uv.push(uv);
                            let weights = vec3(vertex, 0)?;
                            let bones: [u8; 3] = vertex[12..15].try_into().unwrap();
                            if vertex[15] > 3 {
                                return Err(invalid("Invalid VVD bone influence count"));
                            }
                            draw.weights.push((bones, weights));
                        }
                    }
                }
                lighting_mesh += 1;
            }
            if !draw.positions.is_empty() {
                output.push(draw);
            }
        }
    }
    if output.is_empty() {
        return Err(invalid("Studio model has no drawable LOD0 geometry"));
    }
    // VTX winding must agree with the exported vertex normals for Bevy culling.
    for draw in &mut output {
        for first in (0..draw.positions.len()).step_by(3) {
            let p = |i| bevy::math::Vec3::from_array(draw.positions[first + i]);
            let n = (0..3)
                .map(|i| bevy::math::Vec3::from_array(draw.normals[first + i]))
                .sum::<bevy::math::Vec3>();
            if (p(1) - p(0)).cross(p(2) - p(0)).dot(n) < 0. {
                draw.positions.swap(first + 1, first + 2);
                draw.normals.swap(first + 1, first + 2);
                draw.uv.swap(first + 1, first + 2);
                draw.weights.swap(first + 1, first + 2);
                draw.lighting_indices.swap(first + 1, first + 2);
            }
        }
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn muzzle_attachment_uses_bone_bind_rotation_and_checks_bounds() {
        let mut b = vec![0; 700];
        b[..4].copy_from_slice(b"IDST");
        for (at, value) in [
            (4, 48u32),
            (156, 1),
            (160, 256),
            (240, 1),
            (244, 512),
            (512, 92),
        ] {
            b[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        b[604..617].copy_from_slice(b"muzzle_flash\0");
        let put =
            |b: &mut Vec<u8>, at: usize, v: f32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 352, 1.);
        put(&mut b, 352 + 16 + 8, 1.);
        put(&mut b, 352 + 32 + 4, -1.);
        put(&mut b, 512 + 12 + 32, 1.);
        let forward = parse_muzzle(&b).unwrap();
        assert!(forward[2] > 0.999);
        assert!(forward[0].abs() < 0.001 && forward[1].abs() < 0.001);
        b[520..524].copy_from_slice(&99u32.to_le_bytes());
        assert!(parse_muzzle(&b).is_err());
        assert!(parse_muzzle(&b[..245]).is_err());
    }
    #[test]
    fn strip_winding_and_degenerates() {
        assert_eq!(
            triangles(&[0, 1, 2, 3, 3, 4], 2).unwrap(),
            vec![[0, 1, 2], [2, 1, 3]]
        );
    }
    #[test]
    fn incomplete_lists_rejected() {
        assert!(triangles(&[0, 1], 1).is_err());
    }
}
#[cfg(test)]
mod cabinet_pose_tests {
    /// c8m1 and c13m1 failed to load on these two (flags-0 strips, a v44 MDL).
    #[test]
    fn dlc_models_with_unflagged_strips_and_old_headers_load() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for path in [
            "models/props_doors/doordm01_01.mdl",
            "models/props/cs_militia/militiawindow02_breakable_chunk_01.mdl",
        ] {
            let meshes = super::load(&mut assets, path, 0, 0).unwrap();
            let triangles: usize = meshes.iter().map(|m| m.positions.len() / 3).sum();
            assert!(triangles > 0, "{path}");
            for m in &meshes {
                assert!(m.positions.iter().flatten().all(|v| v.is_finite() && v.abs() < 1e4), "{path}");
            }
        }
    }
    #[test]
    fn medical_cabinet_hangs_flat_on_the_wall_in_its_idle_pose() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let path = "models/props_interiors/medicalcabinet02.mdl";
        let extent = |draws: &[super::ModelMesh]| {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in draws.iter().flat_map(|d| &d.positions) {
                for i in 0..3 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
            std::array::from_fn::<f32, 3, _>(|i| ((hi[i] - lo[i]) / 0.0254).round())
        };
        let bind = super::load_lod(&mut assets, path, 0, 0, 0).unwrap();
        let posed = super::load_posed_prop(&mut assets, path).unwrap();
        // Bevy axes (forward, up, side): the idle pose hangs the cabinet 11
        // units deep off the wall, 36 wide; the bind pose stands it 36 deep.
        assert_eq!(extent(&bind), [36., 29., 11.]);
        assert_eq!(extent(&posed), [11., 29., 36.]);
    }
}
