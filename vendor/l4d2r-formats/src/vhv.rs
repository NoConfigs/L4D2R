//! VHV: baked per-vertex lighting for one static-prop instance (`sp_hdr_<instance>.vhv` in the map pak).
//! Layout (v2, verified on shipped maps): 32-byte header {version, checksum, flags, vertex_size(4), vertex_count, mesh_count, pad[2]},
//! then `mesh_count` 28-byte mesh headers of 7 u32 where [0] = LOD, [3] = vertex count, [4] = absolute byte offset of the vertex data
//! (RGBA, alpha always 255; small linear-light values).

use anyhow::{bail, Result};

#[derive(Debug, Clone)]
pub struct Vhv {
    pub vertex_count: u32,
    /// (lod, vertex_count, absolute byte offset)
    pub meshes: Vec<(u32, u32, u32)>,
    bytes: Vec<u8>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

impl Vhv {
    pub fn parse(b: &[u8]) -> Result<Vhv> {
        if b.len() < 32 || u32_at(b, 0) != 2 {
            bail!("unsupported vhv");
        }
        let (vertex_size, vertex_count, mesh_count) = (u32_at(b, 12), u32_at(b, 16), u32_at(b, 20));
        if vertex_size != 4 {
            bail!("unsupported vhv vertex size {vertex_size}");
        }
        let mut meshes = vec![];
        for i in 0..mesh_count as usize {
            let o = 32 + i * 28;
            if o + 28 > b.len() {
                bail!("vhv mesh header out of range");
            }
            meshes.push((u32_at(b, o), u32_at(b, o + 12), u32_at(b, o + 16)));
        }
        Ok(Vhv {
            vertex_count,
            meshes,
            bytes: b.to_vec(),
        })
    }

    /// RGBA records of the LOD 0 meshes, concatenated in mesh order.
    pub fn lod0_colors(&self) -> Vec<[u8; 4]> {
        let mut out = vec![];
        for (lod, count, offset) in &self.meshes {
            if *lod != 0 {
                continue;
            }
            for v in 0..*count as usize {
                let o = *offset as usize + v * 4;
                if o + 4 <= self.bytes.len() {
                    out.push([
                        self.bytes[o],
                        self.bytes[o + 1],
                        self.bytes[o + 2],
                        self.bytes[o + 3],
                    ]);
                }
            }
        }
        out
    }
}
