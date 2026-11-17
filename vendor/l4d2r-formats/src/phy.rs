//! `.phy` VPhysics collision models: the IVP compact-surface ledge tree, flattened into convex pieces.
//! Layout (verified on retail files, see tests): `phyheader {size=16, id, solidCount, checksum}`, per solid `{int size; 'VPHY' ver modelType
//! surfaceSize dragAxisAreas[3] axisMapSize}` then `IVP_Compact_Surface` (48 bytes, ledge-tree root offset at +0x1c... read below),
//! nodes (28 bytes: right offset, ledge offset, centre, radius, box sizes), ledges (16 bytes + triangles of 16 bytes) and points (16 bytes).
//! IVP -> Source: `x = ix/0.0254, y = iz/0.0254, z = -iy/0.0254` (checked against MDL hulls).

use anyhow::{bail, Result};
use glam::Vec3;

#[derive(Clone, Debug)]
pub struct Convex {
    /// Original outward-facing triangles; keep these before sweep-plane merging.
    pub triangles: Vec<[Vec3; 3]>,
    /// Authored packed headers, in triangle order. Keep material codes and all
    /// other feature bits intact when flattening the native ledge tree.
    pub triangle_headers: Vec<TriangleHeader>,
    /// Authored compact edge links and unconverted IVP vertices. Synthetic hulls
    /// have no native topology and must not invent native feature identities.
    pub topology: Option<std::sync::Arc<CompactTopology>>,
    /// Outward planes `(normal, dist)`: inside is `n.p <= dist`.
    pub planes: Vec<(Vec3, f32)>,
    /// Unique hull edges (point pairs) for sweep bevels.
    pub edges: Vec<(Vec3, Vec3)>,
    /// Edge bevel planes (`cross(edge, axis)`, outward) so axis-aligned hull sweeps slide over edges correctly.
    pub bevels: Vec<(Vec3, f32)>,
    pub mins: Vec3,
    pub maxs: Vec3,
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct CompactEdgeId {pub triangle:usize,pub edge:usize}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct CompactEdge(pub u32);
impl CompactEdge {
    pub fn vertex_index(self)->u16{self.0 as u16}
    /// Native100fd6f0: signed15-bit displacement in32-bit words; top flag is
    /// excluded before the arithmetic right shift.
    pub fn opposite_offset(self)->i32{(self.0.wrapping_shl(1) as i32)>>17}
}
#[derive(Clone,Debug)]
pub struct CompactTopology {
    pub edges:Vec<[CompactEdge;3]>,
    pub vertices_ivp:std::collections::BTreeMap<u16,[f32;3]>,
}
impl CompactTopology {
    pub fn edge(&self,id:CompactEdgeId)->Option<CompactEdge>{self.edges.get(id.triangle)?.get(id.edge).copied()}
    pub fn next(&self,id:CompactEdgeId)->Option<CompactEdgeId>{self.edge(id)?;Some(CompactEdgeId{triangle:id.triangle,edge:(id.edge+1)%3})}
    pub fn previous(&self,id:CompactEdgeId)->Option<CompactEdgeId>{self.edge(id)?;Some(CompactEdgeId{triangle:id.triangle,edge:(id.edge+2)%3})}
    pub fn opposite(&self,id:CompactEdgeId)->Option<CompactEdgeId>{
        let edge=self.edge(id)?;
        let word=(id.triangle as i64+1)*4+id.edge as i64+1+edge.opposite_offset() as i64;
        if word<5||word%4==0{return None;}
        let other=CompactEdgeId{triangle:(word/4-1) as usize,edge:(word%4-1) as usize};self.edge(other)?;Some(other)
    }
    pub fn vertex(&self,id:CompactEdgeId)->Option<[f32;3]>{self.vertices_ivp.get(&self.edge(id)?.vertex_index()).copied()}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TriangleHeader(pub u32);
impl TriangleHeader {
    /// Native ICollisionQuery10082bf0: high byte, excluding its top flag bit.
    pub fn material_index(self) -> u8 { ((self.0 >> 24) & 127) as u8 }
    /// Native10082c30 replaces only the seven material bits, including for
    /// negative/oversized input values. The remaining25 bits are preserved.
    pub fn set_material_index(&mut self, index:i32) {
        self.0=(self.0 & !0x7f000000) | (((index as u32)&127)<<24);
    }
}

#[derive(Clone, Debug, Default)]
pub struct PhySolid {
    pub convexes: Vec<Convex>,
    pub mass_properties: Option<PhyMassProperties>,
    pub bounds: Option<PhyBounds>,
    /// Original admission hierarchy, shared across physical instances.
    pub compact_tree: Option<std::sync::Arc<CompactTree>>,
}

/// Original node bounds and ordered children. Keys are byte offsets relative
/// to the compact surface, not renderer hull indices or native heap addresses.
#[derive(Clone,Debug)]
pub struct CompactTreeNode {
    pub key:usize,pub center_ivp:Vec3,pub radius:f32,pub box_sizes:[u8;3],
    pub children:Option<[usize;2]>,pub ledge:Option<usize>,pub flags:u8,
}
#[derive(Clone,Debug,Default)]
pub struct CompactTree {
    pub root:usize,pub nodes:Vec<CompactTreeNode>,
    /// Native ledge+4 backlink, resolved to its query-root node when present.
    pub ledge_nodes:std::collections::BTreeMap<usize,usize>,
    pub leaf_convexes:std::collections::BTreeMap<usize,usize>,
    pub ledge_headers:std::collections::BTreeMap<usize,[u8;16]>,
    /// Coarse ledges must not be added to the legacy leaf collision geometry.
    pub coarse_convexes:std::collections::BTreeMap<usize,std::sync::Arc<Convex>>,
}
impl PhySolid {
    pub fn compact_ledge(&self,key:usize)->Option<&Convex>{
        let tree=self.compact_tree.as_ref()?;
        if let Some(&index)=tree.leaf_convexes.get(&key){self.convexes.get(index)}else{tree.coarse_convexes.get(&key).map(|c|c.as_ref())}
    }
}

/// Authored IVP compact-surface values, before object mass/inertia scaling.
/// These use IVP axes and metres; they are not Source-space hull coordinates.
#[derive(Clone, Copy, Debug)]
pub struct PhyMassProperties {
    pub center_ivp: Vec3,
    pub unit_inertia_ivp: Vec3,
}

/// Original compact radius and byte-sized angular-deviation factor.
#[derive(Clone,Copy,Debug)]
pub struct PhyBounds {pub center_ivp:Vec3,pub radius:f32,pub deviation_factor:u8}

fn i32_at(b: &[u8], o: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(
        b.get(o..o + 4)
            .ok_or_else(|| anyhow::anyhow!("oob"))?
            .try_into()
            .unwrap(),
    ))
}
fn f32_at(b: &[u8], o: usize) -> Result<f32> {
    Ok(f32::from_bits(i32_at(b, o)? as u32))
}

fn ivp_to_hl(p: [f32; 3]) -> Vec3 {
    const S: f32 = 1.0 / 0.0254;
    Vec3::new(p[0] * S, p[2] * S, -p[1] * S)
}

pub fn parse_phy(d: &[u8]) -> Result<Vec<PhySolid>> {
    let header = i32_at(d, 0)? as usize;
    let count = i32_at(d, 8)? as usize;
    parse_solids(d,header,count)
}

/// BSP PHYSCOLLIDE records contain the same serialized solids without the
/// standalone16-byte PHY file header. Keep offsets relative to their payload.
pub fn parse_collision(d:&[u8],count:usize)->Result<Vec<PhySolid>> {
    parse_solids(d,0,count)
}

fn parse_solids(d:&[u8],mut off:usize,count:usize)->Result<Vec<PhySolid>> {
    if count>1024||off>d.len(){bail!("invalid PHY header/count");}
    let mut out = Vec::new();
    for _ in 0..count {
        let size = i32_at(d, off)? as usize;
        let base = off + 4;
        let end=base.checked_add(size).filter(|&end|end<=d.len()).ok_or_else(||anyhow::anyhow!("invalid PHY solid size"))?;
        let solid_data=&d[..end];
        if d.get(base..base + 4) != Some(b"VPHY") {
            bail!("missing VPHY id at {base:#x}");
        }
        // compactsurfaceheader (28 bytes) then IVP_Compact_Surface.
        let surf = base + 28;
        let root = i32_at(d, surf + 0x20)? as usize; // offset_ledgetree_root
        let center_ivp = Vec3::new(f32_at(d, surf)?, f32_at(d, surf + 4)?, f32_at(d, surf + 8)?);
        let unit_inertia_ivp = Vec3::new(f32_at(d, surf + 12)?, f32_at(d, surf + 16)?, f32_at(d, surf + 20)?);
        let mass_properties = (center_ivp.is_finite() && unit_inertia_ivp.is_finite()
            && unit_inertia_ivp.min_element() > 0.).then_some(PhyMassProperties { center_ivp, unit_inertia_ivp });
        let radius=f32_at(d,surf+24)?;
        let deviation_factor=*solid_data.get(surf+28).ok_or_else(||anyhow::anyhow!("missing compact deviation factor"))?;
        let bounds=(center_ivp.is_finite()&&radius.is_finite()&&radius>=0.).then_some(PhyBounds{center_ivp,radius,deviation_factor});
        let mut solid = PhySolid { convexes: Vec::new(), mass_properties, bounds,compact_tree:None };
        let mut tree=CompactTree::default();
        let mut raw_nodes=Vec::new();let mut node_indices=std::collections::BTreeMap::new();let mut ledge_targets=std::collections::BTreeMap::new();
        let mut stack = vec![surf.checked_add(root).ok_or_else(||anyhow::anyhow!("compact root overflow"))?];
        let mut guard = 0;
        while let Some(n) = stack.pop() {
            guard += 1;
            if guard > 200_000 {bail!("ledge tree too large");}
            if n<surf||n.checked_add(28).is_none_or(|v|v>end){bail!("invalid compact tree node");}
            let right = i32_at(d, n)?;let relative_ledge=i32_at(d,n+4)?;
            let ledge:usize=(n as i64+relative_ledge as i64).try_into().map_err(|_|anyhow::anyhow!("invalid compact ledge offset"))?;
            let key=if relative_ledge!=0||right==0{Some(ledge.checked_sub(surf).ok_or_else(||anyhow::anyhow!("ledge before compact surface"))?)}else{None};
            let children=if right==0{None}else{Some([n+28,(n as i64+right as i64).try_into().map_err(|_|anyhow::anyhow!("invalid compact right child"))?])};
            if !node_indices.contains_key(&n){
                node_indices.insert(n,raw_nodes.len());
                raw_nodes.push((n,children,CompactTreeNode{key:n-surf,center_ivp:Vec3::new(f32_at(d,n+8)?,f32_at(d,n+12)?,f32_at(d,n+16)?),radius:f32_at(d,n+20)?,box_sizes:solid_data[n+24..n+27].try_into().unwrap(),children:None,ledge:if relative_ledge!=0{key}else{None},flags:solid_data[n+27]}));
            }
            if let Some(key)=key{
                let header=solid_data.get(ledge..ledge+16).ok_or_else(||anyhow::anyhow!("missing compact ledge header"))?;tree.ledge_headers.insert(key,header.try_into().unwrap());
                let backlink=i32_at(d,ledge+4)?;if let Ok(target)=usize::try_from(ledge as i64+backlink as i64){ledge_targets.insert(key,target);}
                if right==0{tree.leaf_convexes.insert(key,solid.convexes.len());solid.convexes.push(read_ledge(solid_data,ledge)?);}
                else if !tree.coarse_convexes.contains_key(&key){tree.coarse_convexes.insert(key,std::sync::Arc::new(read_ledge(solid_data,ledge)?));}
            }
            if let Some([left,right])=children{stack.push(right);stack.push(left);}
        }
        for (_,children,mut node) in raw_nodes{
            node.children=children.map(|c|[node_indices[&c[0]],node_indices[&c[1]]]);tree.nodes.push(node);
        }
        for (key,target) in ledge_targets{if let Some(&node)=node_indices.get(&target){tree.ledge_nodes.insert(key,node);}}
        solid.compact_tree=Some(std::sync::Arc::new(tree));
        out.push(solid);
        off = end;
    }
    Ok(out)
}

fn read_ledge(d: &[u8], l: usize) -> Result<Convex> {
    let pts_base = (l as i64 + i32_at(d, l)? as i64) as usize;
    let ntri = i16::from_le_bytes(
        d.get(l + 12..l + 14)
            .ok_or_else(|| anyhow::anyhow!("oob"))?
            .try_into()
            .unwrap(),
    );
    if ntri<0{bail!("negative PHY triangle count");}
    let ntri=ntri as usize;
    let mut tris = Vec::with_capacity(ntri);
    let mut triangle_headers = Vec::with_capacity(ntri);
    let mut topology=CompactTopology{edges:Vec::with_capacity(ntri),vertices_ivp:Default::default()};
    let mut pts: std::collections::BTreeMap<u16, Vec3> = Default::default();
    for t in 0..ntri {
        let to = l + 16 + t * 16;
        triangle_headers.push(TriangleHeader(i32_at(d,to)? as u32));
        let mut idx = [0u16; 3];
        let mut compact_edges=[CompactEdge(0);3];
        for e in 0..3 {
            compact_edges[e]=CompactEdge(i32_at(d,to+4+e*4)? as u32);
            idx[e] = compact_edges[e].vertex_index();
            let po = pts_base + idx[e] as usize * 16;
            if !pts.contains_key(&idx[e]){
                let raw=[f32_at(d,po)?,f32_at(d,po+4)?,f32_at(d,po+8)?];
                topology.vertices_ivp.insert(idx[e],raw);pts.insert(idx[e],ivp_to_hl(raw));
            }
        }
        tris.push(idx);
        topology.edges.push(compact_edges);
    }
    let n = pts.len().max(1) as f32;
    let centroid = pts.values().fold(Vec3::ZERO, |a, p| a + *p) / n;
    let (mut mins, mut maxs) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in pts.values() {
        mins = mins.min(*p);
        maxs = maxs.max(*p);
    }
    let mut planes: Vec<(Vec3, f32)> = Vec::new();
    let mut edge_planes: std::collections::BTreeMap<(u16, u16), Vec<usize>> = Default::default();
    for t in &tris {
        let (a, b, c) = (pts[&t[0]], pts[&t[1]], pts[&t[2]]);
        let mut nrm = (b - a).cross(c - a);
        if nrm.length_squared() < 1e-9 {
            continue;
        }
        nrm = nrm.normalize();
        if nrm.dot(a - centroid) < 0.0 {
            nrm = -nrm;
        }
        let dist = nrm.dot(a);
        let pi = match planes
            .iter()
            .position(|(n2, d2)| n2.dot(nrm) > 0.9999 && (d2 - dist).abs() < 0.01)
        {
            Some(k) => k,
            None => {
                planes.push((nrm, dist));
                planes.len() - 1
            }
        };
        for (i, j) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let v = edge_planes.entry((i.min(j), i.max(j))).or_default();
            if !v.contains(&pi) {
                v.push(pi);
            }
        }
    }
    // Only true hull edges (between two different face planes) get bevels; triangulation diagonals inside a face would
    // otherwise cut holes into the solid.
    let edges: Vec<(Vec3, Vec3)> = edge_planes
        .into_iter()
        .filter(|(_, v)| v.len() >= 2)
        .map(|((i, j), _)| (pts[&i], pts[&j]))
        .collect();
    let mut bevels: Vec<(Vec3, f32)> = Vec::new();
    for (a, b) in &edges {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            let bn = (*b - *a).cross(axis);
            if bn.length_squared() < 1e-6 {
                continue;
            }
            let mut bn = bn.normalize();
            if bn.dot(*a - centroid) < 0.0 {
                bn = -bn;
            }
            let d = bn.dot(*a);
            // Skip bevels that duplicate a face plane or are already present.
            if planes
                .iter()
                .chain(bevels.iter())
                .any(|(n2, d2)| n2.dot(bn) > 0.999 && (d2 - d).abs() < 0.05)
            {
                continue;
            }
            bevels.push((bn, d));
        }
    }
    Ok(Convex {
        topology:Some(std::sync::Arc::new(topology)),
        triangle_headers,
        triangles: tris
            .iter()
            .map(|t| {
                let (a, b, c) = (pts[&t[0]], pts[&t[1]], pts[&t[2]]);
                if (b - a).cross(c - a).dot(a - centroid) < 0. {
                    [a, c, b]
                } else {
                    [a, b, c]
                }
            })
            .collect(),
        planes,
        edges,
        bevels,
        mins,
        maxs,
    })
}
