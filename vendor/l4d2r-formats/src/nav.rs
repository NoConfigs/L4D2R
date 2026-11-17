//! Navigation mesh (.nav) parser for Left 4 Dead 2 (version 16, sub-version 14).
//!
//! Layout recovered by decompiling server.dll (CNavMesh::Load, CNavArea::Load,
//! TerrorNavArea::Load, CNavLadder::Load); see docs/nav-format.md.
//! Parsing is strict: the whole file must be consumed exactly.

use anyhow::{bail, Result};

const MAGIC: u32 = 0xFEEDFACE;
pub const NUM_DIRECTIONS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct HidingSpot {
    pub id: u32,
    pub pos: [f32; 3],
    pub flags: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EncounterSpot {
    pub order_id: u32,
    /// Parametric position along the path, 0..1 (stored as u8/255).
    pub t: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpotEncounter {
    pub from_id: u32,
    pub from_dir: u8,
    pub to_id: u32,
    pub to_dir: u8,
    pub spots: Vec<EncounterSpot>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VisibleArea {
    /// Index into `NavMesh::areas` (not an area id; the game packs it into 14 bits).
    pub index: u32,
    pub attributes: u8,
}

#[derive(Clone, Debug)]
pub struct NavArea {
    pub id: u32,
    /// NAV_MESH_* attribute flags (CROUCH, JUMP, ..., TerrorNavArea bits).
    pub flags: u32,
    /// North-west corner (min x, min y in Source; z at that corner).
    pub nw: [f32; 3],
    /// South-east corner.
    pub se: [f32; 3],
    pub ne_z: f32,
    pub sw_z: f32,
    /// Connections in each direction: [North, East, South, West]
    pub connections: [Vec<u32>; NUM_DIRECTIONS],
    pub hiding_spots: Vec<HidingSpot>,
    pub encounters: Vec<SpotEncounter>,
    /// 1-based index into `NavMesh::place_names`; 0 = unnamed.
    pub place_id: u16,
    /// Ladders connected [up, down] (ladder ids).
    pub ladders: [Vec<u32>; 2],
    pub earliest_occupy_time: [f32; 2],
    pub light_intensity: [f32; 4],
    /// TerrorNavArea spawn attributes (SPAWN_* bit mask: EMPTY, STOP_SCAN, BATTLEFIELD, FINALE, ...).
    pub spawn_attributes: u32,
    /// TerrorNavArea u16 field (sub-version > 7), index into the script place table.
    pub script_place: u16,
    pub visible_areas: Vec<VisibleArea>,
    pub inherit_visibility_from: u32,
}

#[derive(Clone, Debug)]
pub struct NavLadder {
    pub id: u32,
    pub width: f32,
    pub top: [f32; 3],
    pub bottom: [f32; 3],
    pub length: f32,
    pub dir: u32,
    pub top_forward: u32,
    pub top_left: u32,
    pub top_right: u32,
    pub top_behind: u32,
    pub bottom_area: u32,
}

#[derive(Debug)]
pub struct NavMesh {
    pub version: u32,
    pub subversion: u32,
    pub save_bsp_size: u32,
    pub is_analyzed: bool,
    pub place_names: Vec<String>,
    pub has_unnamed_areas: bool,
    /// Map name string stored in the TerrorNavMesh header (sub-version >= 12).
    pub map_name: String,
    /// Second string table (sub-version >= 12), usually empty.
    pub script_places: Vec<String>,
    pub areas: Vec<NavArea>,
    pub ladders: Vec<NavLadder>,
    /// Trailing float written after the ladders (sub-version > 6).
    pub trailing_f32: f32,
}

impl NavArea {
    pub fn min_xy(&self) -> [f32; 2] {
        [self.nw[0].min(self.se[0]), self.nw[1].min(self.se[1])]
    }
    pub fn max_xy(&self) -> [f32; 2] {
        [self.nw[0].max(self.se[0]), self.nw[1].max(self.se[1])]
    }
    pub fn center(&self) -> [f32; 3] {
        [
            (self.nw[0] + self.se[0]) * 0.5,
            (self.nw[1] + self.se[1]) * 0.5,
            (self.nw[2] + self.se[2] + self.ne_z + self.sw_z) * 0.25,
        ]
    }
    /// Bilinear-ish plane height at (x, y), from the four corner heights.
    pub fn z_at(&self, x: f32, y: f32) -> f32 {
        let [x0, y0] = [self.nw[0], self.nw[1]];
        let [x1, y1] = [self.se[0], self.se[1]];
        let dx = x1 - x0;
        let dy = y1 - y0;
        let u = if dx.abs() > 1e-6 {
            ((x - x0) / dx).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let v = if dy.abs() > 1e-6 {
            ((y - y0) / dy).clamp(0.0, 1.0)
        } else {
            0.0
        };
        // corners: nw (u0,v0), ne (u1,v0), sw (u0,v1), se (u1,v1)
        let north = self.nw[2] + (self.ne_z - self.nw[2]) * u;
        let south = self.sw_z + (self.se[2] - self.sw_z) * u;
        north + (south - north) * v
    }
    pub fn contains_xy(&self, x: f32, y: f32) -> bool {
        let [a, b] = self.min_xy();
        let [c, d] = self.max_xy();
        x >= a && x <= c && y >= b && y <= d
    }
    pub fn is_connected_to(&self, id: u32) -> bool {
        self.connections.iter().any(|d| d.contains(&id))
    }
}

impl NavMesh {
    /// Index of the area under `pos` whose surface is closest in height (within 120 units).
    pub fn area_at(&self, pos: [f32; 3]) -> Option<usize> {
        let [x, y, z] = pos;
        let mut best: Option<(usize, f32)> = None;
        for (i, a) in self.areas.iter().enumerate() {
            if !a.contains_xy(x, y) {
                continue;
            }
            let dz = (a.z_at(x, y) - z).abs();
            if dz <= 120.0 && best.map_or(true, |(_, b)| dz < b) {
                best = Some((i, dz));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Center of the area at index `idx`.
    pub fn center(&self, idx: usize) -> Option<[f32; 3]> {
        self.areas.get(idx).map(|a| a.center())
    }

    /// Kept for compatibility with the old API.
    pub fn areas_center(&self, idx: usize) -> Option<[f32; 3]> {
        self.center(idx)
    }

    pub fn area_index_by_id(&self, id: u32) -> Option<usize> {
        self.areas.iter().position(|a| a.id == id)
    }

    pub fn place_name(&self, area: &NavArea) -> Option<&str> {
        if area.place_id == 0 {
            None
        } else {
            self.place_names
                .get(area.place_id as usize - 1)
                .map(|s| s.as_str())
        }
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self
            .pos
            .checked_add(n)
            .map_or(true, |e| e > self.data.len())
        {
            bail!(
                "unexpected EOF: need {n} bytes at 0x{:x} (len 0x{:x})",
                self.pos,
                self.data.len()
            );
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    fn cstring(&mut self) -> Result<String> {
        let start = self.pos;
        while self.u8()? != 0 {}
        Ok(String::from_utf8_lossy(&self.data[start..self.pos - 1]).into_owned())
    }
    /// u16 count, then per entry u16 length (incl. NUL) + bytes.
    fn string_table(&mut self) -> Result<Vec<String>> {
        let count = self.u16()? as usize;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let len = self.u16()? as usize;
            let b = self.take(len)?;
            let b = b.strip_suffix(&[0]).unwrap_or(b);
            out.push(String::from_utf8_lossy(b).into_owned());
        }
        Ok(out)
    }
}

/// Parse an L4D2 (.nav version 16, subversion up to 14) file; the whole buffer must be consumed.
pub fn parse_nav(data: &[u8]) -> Result<NavMesh> {
    let mut r = Reader { data, pos: 0 };
    if r.u32()? != MAGIC {
        bail!("bad nav magic");
    }
    let version = r.u32()?;
    if version != 15 && version != 16 {
        bail!("unsupported nav version {version} (15..=16 supported)");
    }
    let subversion = r.u32()?;
    if subversion > 14 || subversion < 12 {
        bail!("unsupported nav subversion {subversion}");
    }
    let save_bsp_size = r.u32()?;
    let is_analyzed = r.u8()? != 0;
    let place_names = r.string_table()?;
    let has_unnamed_areas = r.u8()? != 0;
    // TerrorNavMesh pre-area data (subversion >= 12)
    let map_name = r.cstring()?;
    let script_places = r.string_table()?;

    let area_count = r.u32()? as usize;
    let mut areas = Vec::with_capacity(area_count.min(1 << 20));
    for i in 0..area_count {
        areas.push(read_area(&mut r, subversion).map_err(|e| anyhow::anyhow!("area #{i}: {e}"))?);
    }

    let ladder_count = r.u32()? as usize;
    let mut ladders = Vec::with_capacity(ladder_count.min(1 << 16));
    for _ in 0..ladder_count {
        ladders.push(NavLadder {
            id: r.u32()?,
            width: r.f32()?,
            top: r.vec3()?,
            bottom: r.vec3()?,
            length: r.f32()?,
            dir: r.u32()?,
            top_forward: r.u32()?,
            top_left: r.u32()?,
            top_right: r.u32()?,
            top_behind: r.u32()?,
            bottom_area: r.u32()?,
        });
    }
    let trailing_f32 = r.f32()?;
    if r.pos != data.len() {
        bail!(
            "nav parse ended at 0x{:x}, file is 0x{:x} bytes",
            r.pos,
            data.len()
        );
    }
    Ok(NavMesh {
        version,
        subversion,
        save_bsp_size,
        is_analyzed,
        place_names,
        has_unnamed_areas,
        map_name,
        script_places,
        areas,
        ladders,
        trailing_f32,
    })
}

fn read_area(r: &mut Reader, _sub: u32) -> Result<NavArea> {
    // CNavArea::Load (version 16)
    let id = r.u32()?;
    let flags = r.u32()?;
    let nw = r.vec3()?;
    let se = r.vec3()?;
    let ne_z = r.f32()?;
    let sw_z = r.f32()?;
    let mut connections: [Vec<u32>; NUM_DIRECTIONS] = Default::default();
    for c in connections.iter_mut() {
        let n = r.u32()? as usize;
        for _ in 0..n {
            c.push(r.u32()?);
        }
    }
    let n = r.u8()? as usize;
    let mut hiding_spots = Vec::with_capacity(n);
    for _ in 0..n {
        hiding_spots.push(HidingSpot {
            id: r.u32()?,
            pos: r.vec3()?,
            flags: r.u8()?,
        });
    }
    // (approach areas only exist for version < 15)
    let n = r.u32()? as usize;
    let mut encounters = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        let from_id = r.u32()?;
        let from_dir = r.u8()?;
        let to_id = r.u32()?;
        let to_dir = r.u8()?;
        let ns = r.u8()? as usize;
        let mut spots = Vec::with_capacity(ns);
        for _ in 0..ns {
            spots.push(EncounterSpot {
                order_id: r.u32()?,
                t: r.u8()? as f32 / 255.0,
            });
        }
        encounters.push(SpotEncounter {
            from_id,
            from_dir,
            to_id,
            to_dir,
            spots,
        });
    }
    let place_id = r.u16()?;
    let mut ladders: [Vec<u32>; 2] = Default::default();
    for l in ladders.iter_mut() {
        let n = r.u32()? as usize;
        for _ in 0..n {
            l.push(r.u32()?);
        }
    }
    let earliest_occupy_time = [r.f32()?, r.f32()?];
    let light_intensity = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
    // TerrorNavArea::Load (subversion 12..14)
    let spawn_attributes = r.u32()?;
    let script_place = r.u16()?;
    let n = r.u32()? as usize;
    let mut visible_areas = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        visible_areas.push(VisibleArea {
            index: r.u32()?,
            attributes: r.u8()?,
        });
    }
    let inherit_visibility_from = r.u32()?;
    Ok(NavArea {
        id,
        flags,
        nw,
        se,
        ne_z,
        sw_z,
        connections,
        hiding_spots,
        encounters,
        place_id,
        ladders,
        earliest_occupy_time,
        light_intensity,
        spawn_attributes,
        script_place,
        visible_areas,
        inherit_visibility_from,
    })
}
