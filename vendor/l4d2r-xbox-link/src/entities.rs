//! Networked entity state for the Xbox 360 build of L4D2.
//!
//! - `Schema`: the server's SendTables and ServerClasses (recovered from the
//!   user's own console, loaded from a private text file; see
//!   tools/xbox360_net/export_sendtables_text.py), flattened per class the way
//!   the engine does (excludes, collapsible base classes, child tables first,
//!   then the priority sort).
//! - Prop decoding (ints, quantised / coord / cell-coord / normal floats,
//!   vectors, strings, arrays) and the engine's interleaved index/value delta
//!   format (`RecvTable_Decode`, engine 0x86BF8580 / 0x86BED1B8).
//! - String tables (`instancebaseline`, `userinfo`, ...).
//! - `World`: applies svc_PacketEntities (full and delta) on top of baselines.
//!
//! Validated against a captured native session: all 34 instance baselines,
//! the 270-entity full snapshot and the following delta snapshots decode to
//! the exact bit.

use crate::bits::{lzss_decompress, BitReader};
use std::collections::HashMap;

pub mod flags {
    pub const UNSIGNED: u32 = 1;
    pub const COORD: u32 = 2;
    pub const NOSCALE: u32 = 4;
    pub const NORMAL: u32 = 32;
    pub const EXCLUDE: u32 = 64;
    pub const INSIDEARRAY: u32 = 256;
    pub const COLLAPSIBLE: u32 = 1 << 11;
    pub const COORD_MP: u32 = 1 << 12;
    pub const COORD_MP_LOWPRECISION: u32 = 1 << 13;
    pub const COORD_MP_INTEGRAL: u32 = 1 << 14;
    pub const CELL_COORD: u32 = 1 << 15;
    pub const CELL_COORD_LOWPRECISION: u32 = 1 << 16;
    pub const CELL_COORD_INTEGRAL: u32 = 1 << 17;
    pub const CHANGES_OFTEN: u32 = 1 << 18;
}
use flags::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropType {
    Int,
    Float,
    Vector,
    VectorXY,
    String,
    Array,
    DataTable,
    Int64,
}

#[derive(Debug, Clone)]
pub struct Prop {
    pub name: String,
    pub ty: PropType,
    pub bits: u32,
    pub low: f32,
    pub high: f32,
    pub elements: u32,
    pub priority: u8,
    pub flags: u32,
    pub child: Option<usize>,
    pub exclude_dt: Option<String>,
    /// Array element prop (the preceding INSIDEARRAY prop).
    pub element: Option<Box<Prop>>,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub props: Vec<Prop>,
}

#[derive(Debug, Clone)]
pub struct ServerClass {
    pub id: u32,
    pub name: String,
    pub table: usize,
}

#[derive(Debug, Clone)]
pub struct Schema {
    pub tables: Vec<Table>,
    pub classes: Vec<ServerClass>,
    flat: HashMap<u32, Vec<Prop>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f32),
    Vector([f32; 3]),
    VectorXY([f32; 2]),
    String(String),
    Array(Vec<Value>),
    Int64(u64),
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_float(&self) -> Option<f32> {
        match self {
            Value::Float(v) => Some(*v),
            Value::Int(v) => Some(*v as f32),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityError(pub String);
type Result<T> = std::result::Result<T, EntityError>;
fn err<T>(m: impl Into<String>) -> Result<T> {
    Err(EntityError(m.into()))
}
fn need<T>(o: Option<T>, what: &str) -> Result<T> {
    o.ok_or_else(|| EntityError(format!("truncated {what}")))
}

impl Schema {
    /// Parse the text export (C / T / P records, tab separated).
    pub fn from_text(text: &str) -> Result<Schema> {
        let mut tables: Vec<Table> = Vec::new();
        let mut by_addr: HashMap<u64, usize> = HashMap::new();
        let mut class_rows = Vec::new();
        let mut child_addrs: Vec<Vec<u64>> = Vec::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.first() {
                Some(&"C") if f.len() >= 4 => class_rows.push((f[1].parse::<u32>().unwrap_or(0), f[2].to_string(),
                    f[3].parse::<u64>().unwrap_or(0))),
                Some(&"T") if f.len() >= 4 => {
                    by_addr.insert(f[1].parse().unwrap_or(0), tables.len());
                    tables.push(Table { name: f[2].to_string(), props: Vec::new() });
                    child_addrs.push(Vec::new());
                }
                Some(&"P") if f.len() >= 11 => {
                    let t = tables.last_mut().ok_or(EntityError("prop before table".into()))?;
                    let ty = match f[2] {
                        "Int" => PropType::Int,
                        "Float" => PropType::Float,
                        "Vector" => PropType::Vector,
                        "VectorXY" => PropType::VectorXY,
                        "String" => PropType::String,
                        "Array" => PropType::Array,
                        "DataTable" => PropType::DataTable,
                        "Int64" => PropType::Int64,
                        other => return err(format!("unknown prop type {other}")),
                    };
                    let element = if ty == PropType::Array { t.props.last().cloned().map(Box::new) } else { None };
                    t.props.push(Prop {
                        name: f[1].to_string(),
                        ty,
                        bits: f[3].parse().unwrap_or(0),
                        low: f[4].parse().unwrap_or(0.0),
                        high: f[5].parse().unwrap_or(0.0),
                        elements: f[6].parse().unwrap_or(0),
                        priority: f[7].parse().unwrap_or(128),
                        flags: f[8].parse().unwrap_or(0),
                        child: None,
                        exclude_dt: (f[10] != "-").then(|| f[10].to_string()),
                        element,
                    });
                    child_addrs.last_mut().unwrap().push(f[9].parse().unwrap_or(0));
                }
                _ => {}
            }
        }
        for (ti, addrs) in child_addrs.iter().enumerate() {
            for (pi, a) in addrs.iter().enumerate() {
                if *a != 0 {
                    tables[ti].props[pi].child = by_addr.get(a).copied();
                }
            }
        }
        let classes = class_rows
            .into_iter()
            .filter_map(|(id, name, addr)| Some(ServerClass { id, name, table: *by_addr.get(&addr)? }))
            .collect::<Vec<_>>();
        let mut schema = Schema { tables, classes, flat: HashMap::new() };
        let ids: Vec<u32> = schema.classes.iter().map(|c| c.id).collect();
        for id in ids {
            let f = schema.flatten(schema.class(id).unwrap().table);
            schema.flat.insert(id, f);
        }
        Ok(schema)
    }

    pub fn class(&self, id: u32) -> Option<&ServerClass> {
        self.classes.iter().find(|c| c.id == id)
    }
    pub fn class_bits(&self) -> usize {
        (32 - (self.classes.len() as u32).leading_zeros()) as usize
    }
    pub fn flat(&self, class: u32) -> Option<&[Prop]> {
        self.flat.get(&class).map(|v| v.as_slice())
    }

    fn excludes(&self, table: usize, out: &mut Vec<(String, String)>) {
        for p in &self.tables[table].props {
            if p.flags & EXCLUDE != 0 {
                out.push((p.exclude_dt.clone().unwrap_or_default(), p.name.clone()));
            } else if p.ty == PropType::DataTable {
                if let Some(c) = p.child {
                    self.excludes(c, out);
                }
            }
        }
    }

    /// SendTable_BuildHierarchy + priority sort.
    pub fn flatten(&self, table: usize) -> Vec<Prop> {
        let mut ex = Vec::new();
        self.excludes(table, &mut ex);
        let mut flat = Vec::new();
        self.build(table, &ex, &mut flat);
        let mut prios: Vec<u8> = flat.iter().map(|p| p.priority).collect();
        prios.push(64);
        prios.sort_unstable();
        prios.dedup();
        let mut start = 0;
        for prio in prios {
            let mut i = start;
            while i < flat.len() {
                let p = &flat[i];
                if p.priority == prio || (p.flags & CHANGES_OFTEN != 0 && prio == 64) {
                    flat.swap(start, i);
                    start += 1;
                }
                i += 1;
            }
        }
        flat
    }

    fn build(&self, table: usize, ex: &[(String, String)], flat: &mut Vec<Prop>) {
        let mut own = Vec::new();
        self.iterate(table, ex, flat, &mut own);
        flat.extend(own);
    }

    fn iterate(&self, table: usize, ex: &[(String, String)], flat: &mut Vec<Prop>, own: &mut Vec<Prop>) {
        let t = &self.tables[table];
        for p in &t.props {
            if p.flags & EXCLUDE != 0 || ex.iter().any(|(dt, n)| *dt == t.name && *n == p.name) {
                continue;
            }
            if p.ty == PropType::DataTable {
                let Some(c) = p.child else { continue };
                if p.flags & COLLAPSIBLE != 0 {
                    self.iterate(c, ex, flat, own);
                } else {
                    self.build(c, ex, flat);
                }
            } else if p.flags & INSIDEARRAY == 0 {
                own.push(p.clone());
            }
        }
    }
}

// ------------------------------------------------------------ decoding ---

fn sbits(r: &mut BitReader, n: u32) -> Option<i64> {
    let v = r.u(n as usize)?;
    Some(if n > 0 && v >> (n - 1) & 1 == 1 { v as i64 - (1i64 << n) } else { v as i64 })
}

fn bit_coord(r: &mut BitReader) -> Option<f32> {
    let int = r.bit()?;
    let fract = r.bit()?;
    if !int && !fract {
        return Some(0.0);
    }
    let sign = r.bit()?;
    let mut v = 0.0;
    if int {
        v += r.u(14)? as f32 + 1.0;
    }
    if fract {
        v += r.u(5)? as f32 / 32.0;
    }
    Some(if sign { -v } else { v })
}

fn coord_mp(r: &mut BitReader, integral: bool, low: bool) -> Option<f32> {
    let in_bounds = r.bit()?;
    let ibits = if in_bounds { 11 } else { 14 };
    let (v, sign) = if integral {
        if r.bit()? {
            let sign = r.bit()?;
            (r.u(ibits)? as f32 + 1.0, sign)
        } else {
            (0.0, false)
        }
    } else {
        let has_int = r.bit()?;
        let sign = r.bit()?;
        let i = if has_int { r.u(ibits)? as f32 + 1.0 } else { 0.0 };
        let f = r.u(if low { 3 } else { 5 })? as f32 * if low { 1.0 / 8.0 } else { 1.0 / 32.0 };
        (i + f, sign)
    };
    Some(if sign { -v } else { v })
}

fn cell_coord(r: &mut BitReader, bits: u32, integral: bool, low: bool) -> Option<f32> {
    let i = r.u(bits as usize)? as f32;
    if integral {
        return Some(i);
    }
    let f = r.u(if low { 3 } else { 5 })? as f32 * if low { 1.0 / 8.0 } else { 1.0 / 32.0 };
    Some(i + f)
}

fn bit_normal(r: &mut BitReader) -> Option<f32> {
    let sign = r.bit()?;
    let f = r.u(11)? as f32 / ((1 << 11) - 1) as f32;
    Some(if sign { -f } else { f })
}

fn decode_float(r: &mut BitReader, p: &Prop) -> Option<f32> {
    let fl = p.flags;
    if fl & COORD != 0 {
        bit_coord(r)
    } else if fl & COORD_MP != 0 {
        coord_mp(r, false, false)
    } else if fl & COORD_MP_LOWPRECISION != 0 {
        coord_mp(r, false, true)
    } else if fl & COORD_MP_INTEGRAL != 0 {
        coord_mp(r, true, false)
    } else if fl & NOSCALE != 0 {
        Some(f32::from_bits(r.u32(32)?))
    } else if fl & NORMAL != 0 {
        bit_normal(r)
    } else if fl & CELL_COORD != 0 {
        cell_coord(r, p.bits, false, false)
    } else if fl & CELL_COORD_LOWPRECISION != 0 {
        cell_coord(r, p.bits, false, true)
    } else if fl & CELL_COORD_INTEGRAL != 0 {
        cell_coord(r, p.bits, true, false)
    } else {
        let v = r.u(p.bits as usize)? as f32;
        Some(p.low + (p.high - p.low) * v / ((1u64 << p.bits) - 1) as f32)
    }
}

pub fn decode_value(r: &mut BitReader, p: &Prop) -> Option<Value> {
    Some(match p.ty {
        PropType::Int => {
            if p.flags & UNSIGNED != 0 {
                Value::Int(r.u(p.bits as usize)? as i64)
            } else {
                Value::Int(sbits(r, p.bits)?)
            }
        }
        PropType::Float => Value::Float(decode_float(r, p)?),
        PropType::Vector => {
            let x = decode_float(r, p)?;
            let y = decode_float(r, p)?;
            if p.flags & NORMAL != 0 {
                let sign = r.bit()?;
                let z2 = 1.0 - x * x - y * y;
                let z = if z2 > 0.0 { z2.sqrt() } else { 0.0 };
                Value::Vector([x, y, if sign { -z } else { z }])
            } else {
                Value::Vector([x, y, decode_float(r, p)?])
            }
        }
        PropType::VectorXY => Value::VectorXY([decode_float(r, p)?, decode_float(r, p)?]),
        PropType::String => {
            let n = r.u(9)? as usize;
            Value::String(String::from_utf8_lossy(&r.bytes(n)?).into_owned())
        }
        PropType::Array => {
            let el = p.element.as_ref()?;
            let count_bits = 32 - p.elements.leading_zeros();
            let n = r.u(count_bits as usize)? as usize;
            Value::Array((0..n).map(|_| decode_value(r, el)).collect::<Option<_>>()?)
        }
        PropType::Int64 => {
            let lo = r.u(32)?;
            let hi = r.u((p.bits.saturating_sub(32)) as usize)?;
            Value::Int64(hi << 32 | lo)
        }
        PropType::DataTable => return None,
    })
}

/// CDeltaBitsReader::ReadNextPropIndex (CS:GO-style "new scheme").
fn next_index(r: &mut BitReader, last: i64, new_way: bool) -> Option<i64> {
    if new_way && r.bit()? {
        return Some(last + 1);
    }
    let ret = if new_way && r.bit()? {
        r.u(3)?
    } else {
        let v = r.u(7)?;
        match v & 96 {
            32 => (v & !96) | (r.u(2)? << 5),
            64 => (v & !96) | (r.u(4)? << 5),
            96 => (v & !96) | (r.u(7)? << 5),
            _ => v,
        }
    };
    if ret == 0xFFF {
        return Some(-1);
    }
    Some(last + 1 + ret as i64)
}

/// Decode an interleaved index/value prop list into `out` (keyed by flat index).
pub fn read_props(r: &mut BitReader, flat: &[Prop], out: &mut HashMap<usize, Value>) -> Result<()> {
    let new_way = r.bit().ok_or(EntityError("truncated prop list".into()))?;
    let mut last = -1i64;
    loop {
        last = next_index(r, last, new_way).ok_or(EntityError("truncated prop index".into()))?;
        if last < 0 {
            return Ok(());
        }
        let p = flat.get(last as usize).ok_or(EntityError(format!("prop index {last} >= {}", flat.len())))?;
        let v = decode_value(r, p).ok_or(EntityError(format!("truncated value for {}", p.name)))?;
        out.insert(last as usize, v);
    }
}

/// UBitVar as the L4D2/CS:GO engine generation writes it.
pub fn read_ubitvar(r: &mut BitReader) -> Option<u32> {
    let v = r.u32(6)?;
    Some(match v & 48 {
        16 => (v & 15) | (r.u32(4)? << 4),
        32 => (v & 15) | (r.u32(8)? << 4),
        48 => (v & 15) | (r.u32(28)? << 4),
        _ => v,
    })
}

// -------------------------------------------------------- string tables ---

#[derive(Debug, Clone, PartialEq)]
pub struct StringEntry {
    pub index: u32,
    pub name: Option<String>,
    pub data: Option<Vec<u8>>,
}

/// Parse a CreateStringTable payload (decompressing when `compressed`).
/// Dictionary-encoded tables are reported as an error.
pub fn parse_string_table(max_entries: u32, entries: u32, fixed_bits: Option<u32>, compressed: bool, data: &[u8], bits: usize)
    -> Result<Vec<StringEntry>> {
    parse_string_table_dict(max_entries, entries, fixed_bits, compressed, data, bits, None)
}

/// As `parse_string_table`, resolving dictionary-encoded strings with the
/// map's `stringtable_dictionary_xbox.dct` (see `dictionary_from_bsp`).
pub fn parse_string_table_dict(max_entries: u32, entries: u32, fixed_bits: Option<u32>, compressed: bool, data: &[u8],
                               bits: usize, dict: Option<&[String]>) -> Result<Vec<StringEntry>> {
    if compressed {
        if data.len() < 8 {
            return err("short compressed table");
        }
        let unc = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        let comp = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let plain = lzss_decompress(data.get(8..8 + comp).ok_or(EntityError("bad compressed size".into()))?)
            .ok_or(EntityError("bad LZSS".into()))?;
        if plain.len() != unc {
            return err("LZSS size mismatch");
        }
        return parse_entries_dict(max_entries, entries, fixed_bits, &plain, plain.len() * 8, dict);
    }
    parse_entries_dict(max_entries, entries, fixed_bits, data, bits, dict)
}

/// The L4D2-era string table entry stream (create and update share it).
pub fn parse_entries(max_entries: u32, entries: u32, fixed_bits: Option<u32>, buf: &[u8], nbits: usize)
    -> Result<Vec<StringEntry>> {
    parse_entries_dict(max_entries, entries, fixed_bits, buf, nbits, None)
}

/// Entry stream with optional dictionary: a dictionary string is a flag bit,
/// then either "previous dictionary index + 1" (1 bit) or Q_log2(n)+1 index bits.
pub fn parse_entries_dict(max_entries: u32, entries: u32, fixed_bits: Option<u32>, buf: &[u8], nbits: usize,
                          dict: Option<&[String]>) -> Result<Vec<StringEntry>> {
    let mut r = BitReader::range(buf, 0, nbits);
    let use_dict = need(r.bit(), "string table")?;
    if use_dict && dict.is_none() {
        return err("dictionary-encoded string table (no dictionary loaded)");
    }
    let dict_bits = dict.map(|d| (32 - (d.len().max(1) as u32).leading_zeros()) as usize).unwrap_or(0);
    let mut last_dict: i64 = -1;
    let index_bits = (31 - max_entries.max(1).leading_zeros()) as usize;
    let mut out = Vec::new();
    let mut history: Vec<String> = Vec::new();
    let mut last: i64 = -1;
    for _ in 0..entries {
        let index = if need(r.bit(), "string table")? { (last + 1) as u32 } else { need(r.u32(index_bits), "string table")? };
        last = index as i64;
        let mut name = None;
        if need(r.bit(), "string table")? {
            let s = if use_dict && need(r.bit(), "string table")? {
                let di = if need(r.bit(), "string table")? { last_dict + 1 } else { need(r.u(dict_bits), "string table")? as i64 };
                last_dict = di;
                dict.unwrap().get(di as usize).cloned().ok_or(EntityError(format!("dictionary index {di} out of range")))?
            } else if need(r.bit(), "string table")? {
                let hi = need(r.u(5), "string table")? as usize;
                let n = need(r.u(5), "string table")? as usize;
                let base: String = history.get(hi).map(|h| h.chars().take(n).collect()).unwrap_or_default();
                base + &need(r.string(4096), "string table")?
            } else {
                need(r.string(4096), "string table")?
            };
            name = Some(s);
        }
        let mut data = None;
        if need(r.bit(), "string table")? {
            data = Some(match fixed_bits {
                Some(b) => need(r.bit_vec(b as usize), "string table")?,
                None => {
                    let n = need(r.u(14), "string table")? as usize;
                    need(r.bytes(n), "string table")?
                }
            });
        }
        history.push(name.clone().unwrap_or_default());
        if history.len() > 32 {
            history.remove(0);
        }
        out.push(StringEntry { index, name, data });
    }
    Ok(out)
}

/// A client-side string table: entry index -> (string, user data).
#[derive(Debug, Clone)]
pub struct StringTable {
    pub name: String,
    pub max_entries: u32,
    pub fixed_bits: Option<u32>,
    pub entries: Vec<(Option<String>, Option<Vec<u8>>)>,
    /// Set when the table could not be decoded (e.g. dictionary encoding).
    pub undecoded: Option<String>,
}

/// String tables in creation order (UpdateStringTable addresses them by id).
#[derive(Debug, Default, Clone)]
pub struct StringTables {
    pub tables: Vec<StringTable>,
    /// The map's string table dictionary (needed by model/sound/scene tables).
    pub dictionary: Option<Vec<String>>,
}

impl StringTables {
    fn merge(t: &mut StringTable, entries: Vec<StringEntry>) {
        for e in entries {
            let i = e.index as usize;
            if i >= t.entries.len() {
                t.entries.resize(i + 1, (None, None));
            }
            let slot = &mut t.entries[i];
            if e.name.is_some() {
                slot.0 = e.name;
            }
            if e.data.is_some() {
                slot.1 = e.data;
            }
        }
    }

    pub fn create(&mut self, name: &str, max_entries: u32, entries: u32, fixed_bits: Option<u32>, compressed: bool,
                  data: &[u8], bits: usize) -> usize {
        let mut t = StringTable { name: name.to_string(), max_entries, fixed_bits, entries: Vec::new(), undecoded: None };
        match parse_string_table_dict(max_entries, entries, fixed_bits, compressed, data, bits, self.dictionary.as_deref()) {
            Ok(e) => Self::merge(&mut t, e),
            Err(e) => t.undecoded = Some(e.0),
        }
        self.tables.push(t);
        self.tables.len() - 1
    }

    pub fn update(&mut self, table: u32, changed: u32, data: &[u8], bits: usize) -> Result<()> {
        let t = self.tables.get_mut(table as usize).ok_or(EntityError(format!("no string table {table}")))?;
        if t.undecoded.is_some() {
            return Ok(());
        }
        let e = parse_entries_dict(t.max_entries, changed, t.fixed_bits, data, bits, self.dictionary.as_deref())?;
        Self::merge(t, e);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&StringTable> {
        self.tables.iter().find(|t| t.name == name)
    }

    /// String of entry `index` in table `name`.
    pub fn string(&self, name: &str, index: usize) -> Option<&str> {
        self.get(name)?.entries.get(index)?.0.as_deref()
    }
}

/// Extract `stringtable_dictionary_xbox.dct` from a 360 map BSP's pak lump
/// (big-endian lump directory, lump 40 = stored zip).
pub fn dictionary_from_bsp(bsp: &[u8]) -> Option<Vec<String>> {
    if bsp.len() < 8 + 64 * 16 || &bsp[..4] != b"PSBV" {
        return None;
    }
    let lump = 8 + 40 * 16;
    let off = u32::from_be_bytes(bsp[lump..lump + 4].try_into().ok()?) as usize;
    let len = u32::from_be_bytes(bsp[lump + 4..lump + 8].try_into().ok()?) as usize;
    let pak = bsp.get(off..off + len)?;
    let name = b"stringtable_dictionary_xbox.dct";
    let mut at = 0;
    while let Some(p) = pak[at..].windows(4).position(|w| w == [0x50u8, 0x4b, 0x03, 0x04]) {
        let h = at + p;
        let hdr = pak.get(h..h + 30)?;
        let method = u16::from_le_bytes([hdr[8], hdr[9]]);
        let size = u32::from_le_bytes(hdr[18..22].try_into().ok()?) as usize;
        let nl = u16::from_le_bytes([hdr[26], hdr[27]]) as usize;
        let el = u16::from_le_bytes([hdr[28], hdr[29]]) as usize;
        if pak.get(h + 30..h + 30 + nl) == Some(&name[..]) && method == 0 {
            let data = pak.get(h + 30 + nl + el..h + 30 + nl + el + size)?;
            let mut out: Vec<String> = data.split(|&b| b == 0).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
            if out.last().is_some_and(|s| s.is_empty()) {
                out.pop();
            }
            return Some(out);
        }
        at = h + 4;
    }
    None
}

impl StringTables {
    pub fn with_dictionary(dictionary: Option<Vec<String>>) -> StringTables {
        StringTables { tables: Vec::new(), dictionary }
    }
}

/// player_info_t as the 360 stores it (big-endian): xuid, name[32], userID, guid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfo {
    pub xuid: u64,
    pub name: String,
    pub user_id: i32,
    pub guid: String,
}

impl PlayerInfo {
    pub fn parse(d: &[u8]) -> Option<PlayerInfo> {
        if d.len() < 77 {
            return None;
        }
        let cstr = |b: &[u8]| String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).into_owned();
        Some(PlayerInfo {
            xuid: u64::from_be_bytes(d[0..8].try_into().ok()?),
            name: cstr(&d[8..40]),
            user_id: i32::from_be_bytes(d[40..44].try_into().ok()?),
            guid: cstr(&d[44..77]),
        })
    }
}

// ---------------------------------------------------------------- world ---

#[derive(Debug, Clone)]
pub struct Entity {
    pub class: u32,
    pub serial: u32,
    pub props: HashMap<usize, Value>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UpdateStats {
    pub entered: u32,
    pub updated: u32,
    pub left: u32,
    pub deleted: u32,
}

pub struct World {
    pub schema: Schema,
    /// Instance baselines by class id (raw prop lists).
    pub baselines: HashMap<u32, Vec<u8>>,
    /// Current (latest) frame.
    pub entities: HashMap<u32, Entity>,
    pub tick: u32,
    /// Recent frames by server tick; deltas are relative to one of these.
    frames: std::collections::VecDeque<(u32, HashMap<u32, Entity>)>,
    /// Source's per-entity baselines (two sets, chosen by PacketEntities'
    /// baseline bit). Snapshots marked update_baseline store the entities that
    /// enter in them; a later delta that re-enters an entity sends only what
    /// changed since, so it must start from this state, not the class baseline.
    pub entity_baselines: [HashMap<u32, Entity>; 2],
}

const FRAME_HISTORY: usize = 64;

impl World {
    pub fn new(schema: Schema) -> World {
        World { schema, baselines: HashMap::new(), entities: HashMap::new(), tick: 0, frames: Default::default(), entity_baselines: Default::default() }
    }

    /// Load `instancebaseline` entries (names are class ids).
    pub fn set_baselines(&mut self, entries: &[StringEntry]) {
        for e in entries {
            if let (Some(n), Some(d)) = (&e.name, &e.data) {
                if let Ok(id) = n.parse() {
                    self.baselines.insert(id, d.clone());
                }
            }
        }
    }

    /// Apply one svc_PacketEntities payload for server `tick`, as a delta
    /// from the stored frame at `delta_from` (or from nothing).
    pub fn apply(&mut self, data: &[u8], bits: usize, updated: u32, delta_from: Option<u32>, tick: u32) -> Result<UpdateStats> {
        self.apply_with(data, bits, updated, delta_from, tick, false, false)
    }

    /// As `apply`, with the packet's baseline bit and update_baseline flag
    /// (Source's per-entity baselines; see `entity_baselines`).
    pub fn apply_with(&mut self, data: &[u8], bits: usize, updated: u32, delta_from: Option<u32>, tick: u32,
                      baseline: bool, update_baseline: bool) -> Result<UpdateStats> {
        let saved_baselines = update_baseline.then(|| self.entity_baselines.clone());
        let b = baseline as usize;
        if update_baseline {
            self.entity_baselines[1 - b] = self.entity_baselines[b].clone();
        }
        let r = self.apply_frame(data, bits, updated, delta_from, tick, b, update_baseline);
        if r.is_err() {
            if let Some(saved) = saved_baselines {
                self.entity_baselines = saved;
            }
        }
        r
    }

    fn apply_frame(&mut self, data: &[u8], bits: usize, updated: u32, delta_from: Option<u32>, tick: u32,
                   b: usize, update_baseline: bool) -> Result<UpdateStats> {
        let base = match delta_from {
            None => HashMap::new(),
            Some(t) => match self.frames.iter().find(|(ft, _)| *ft == t) {
                Some((_, f)) => f.clone(),
                None => return err(format!("no stored frame for delta tick {t}")),
            },
        };
        let saved = std::mem::replace(&mut self.entities, base);
        match self.apply_on_current(data, bits, updated, delta_from.is_some(), b, update_baseline) {
            Ok(stats) => {
                self.tick = tick;
                self.frames.retain(|(ft, _)| *ft != tick);
                self.frames.push_back((tick, self.entities.clone()));
                while self.frames.len() > FRAME_HISTORY {
                    self.frames.pop_front();
                }
                Ok(stats)
            }
            Err(e) => {
                self.entities = saved;
                Err(e)
            }
        }
    }

    fn apply_on_current(&mut self, data: &[u8], bits: usize, updated: u32, is_delta: bool, b: usize,
                        update_baseline: bool) -> Result<UpdateStats> {
        let mut r = BitReader::range(data, 0, bits);
        let mut stats = UpdateStats::default();
        let mut ent: i64 = -1;
        let class_bits = self.schema.class_bits();
        for _ in 0..updated {
            ent += 1 + need(read_ubitvar(&mut r), "entity header")? as i64;
            let id = ent as u32;
            if need(r.bit(), "entity header")? {
                if need(r.bit(), "entity header")? {
                    self.entities.remove(&id);
                    stats.deleted += 1;
                } else {
                    stats.left += 1;
                }
                continue;
            }
            if need(r.bit(), "entity header")? {
                let class = need(r.u32(class_bits), "entity header")?;
                let serial = need(r.u32(10), "entity header")?;
                let flat = self.schema.flat(class).ok_or(EntityError(format!("unknown class {class}")))?;
                // Start from this entity's own baseline when a delta re-enters it,
                // else from the class's instance baseline.
                let own = if is_delta { self.entity_baselines[b].get(&id).filter(|e| e.class == class) } else { None };
                let mut props = match own {
                    Some(e) => e.props.clone(),
                    None => {
                        let mut p = HashMap::new();
                        if let Some(base) = self.baselines.get(&class) {
                            read_props(&mut BitReader::new(base), flat, &mut p)?;
                        }
                        p
                    }
                };
                read_props(&mut r, flat, &mut props)?;
                let entity = Entity { class, serial, props };
                if update_baseline {
                    self.entity_baselines[1 - b].insert(id, entity.clone());
                }
                self.entities.insert(id, entity);
                stats.entered += 1;
            } else {
                let e = self.entities.get_mut(&id).ok_or(EntityError(format!("delta for unknown entity {id}")))?;
                let flat = self.schema.flat.get(&e.class).ok_or(EntityError("unknown class".into()))?;
                read_props(&mut r, flat, &mut e.props)?;
                stats.updated += 1;
            }
        }
        if is_delta {
            while r.left() > 0 && need(r.bit(), "entity header")? {
                let id = need(r.u32(11), "entity header")?;
                self.entities.remove(&id);
                stats.deleted += 1;
            }
        }
        if r.left() >= 8 {
            return err(format!("{} bits left after entity update", r.left()));
        }
        Ok(stats)
    }

    /// Value of the first flattened prop called `name` on entity `id`.
    pub fn prop(&self, id: u32, name: &str) -> Option<&Value> {
        let e = self.entities.get(&id)?;
        let flat = self.schema.flat(e.class)?;
        flat.iter().enumerate().filter(|(_, p)| p.name == name).find_map(|(i, _)| e.props.get(&i))
    }

    pub fn class_name(&self, id: u32) -> Option<&str> {
        let e = self.entities.get(&id)?;
        self.schema.class(e.class).map(|c| c.name.as_str())
    }

    /// World origin from cell coordinates (cell * 2^cellbits - 16384 + local),
    /// which the server keeps current for every entity. The full-precision
    /// (NOSCALE) origin only fills in when a cell component was never sent
    /// (it can be stale for non-local players).
    /// The exact (NOSCALE) origin, which the server sends only to the
    /// player who owns this entity (localdata). For our own player the cell
    /// origin's in-cell offset is not sent at all, so `origin` would be
    /// stuck on a 32-unit grid; this is the one to use there.
    pub fn precise_origin(&self, id: u32) -> Option<[f32; 3]> {
        let e = self.entities.get(&id)?;
        let flat = self.schema.flat(e.class)?;
        let mut v: [Option<f32>; 3] = [None; 3];
        for (i, p) in flat.iter().enumerate() {
            if p.flags & NOSCALE == 0 {
                continue;
            }
            match (p.name.as_str(), e.props.get(&i)) {
                ("m_vecOrigin", Some(Value::VectorXY([x, y]))) => { v[0] = Some(*x); v[1] = Some(*y); }
                ("m_vecOrigin", Some(Value::Vector(o))) => v = [Some(o[0]), Some(o[1]), Some(o[2])],
                ("m_vecOrigin[2]", Some(Value::Float(z))) => v[2] = Some(*z),
                _ => {}
            }
        }
        match v {
            [Some(x), Some(y), Some(z)] if x != 0.0 || y != 0.0 || z != 0.0 => Some([x, y, z]),
            _ => None,
        }
    }

    pub fn origin(&self, id: u32) -> Option<[f32; 3]> {
        let e = self.entities.get(&id)?;
        let flat = self.schema.flat(e.class)?;
        let mut precise: [Option<f32>; 3] = [None; 3];
        for (i, p) in flat.iter().enumerate() {
            if p.flags & NOSCALE == 0 {
                continue;
            }
            match (p.name.as_str(), e.props.get(&i)) {
                ("m_vecOrigin", Some(Value::VectorXY([x, y]))) => { precise[0] = Some(*x); precise[1] = Some(*y); }
                ("m_vecOrigin", Some(Value::Vector(v))) => { precise = [Some(v[0]), Some(v[1]), Some(v[2])]; }
                ("m_vecOrigin[2]", Some(Value::Float(z))) => precise[2] = Some(*z),
                _ => {}
            }
        }
        let has_cells = ["m_cellX", "m_cellY", "m_cellZ"].iter().all(|n| self.prop(id, n).is_some());
        if !has_cells {
            if let [Some(x), Some(y), Some(z)] = precise {
                return Some([x, y, z]);
            }
        }
        let cellbits = self.prop(id, "m_cellbits").and_then(|v| v.as_int()).unwrap_or(5) as i32;
        let cell = |n: &str| self.prop(id, n).and_then(|v| v.as_int()).unwrap_or(0) as f32;
        let width = (1i32 << cellbits) as f32;
        let mut local = [0.0f32; 3];
        for (i, p) in flat.iter().enumerate() {
            let is_cell = p.flags & (CELL_COORD | CELL_COORD_LOWPRECISION | CELL_COORD_INTEGRAL) != 0;
            if !is_cell {
                continue;
            }
            match (p.name.as_str(), e.props.get(&i)) {
                ("m_vecOrigin", Some(Value::VectorXY([x, y]))) => {
                    local[0] = *x;
                    local[1] = *y;
                }
                ("m_vecOrigin", Some(Value::Vector(v))) => local = *v,
                ("m_vecOrigin[2]", Some(Value::Float(z))) => local[2] = *z,
                _ => {}
            }
        }
        Some([
            cell("m_cellX") * width - 16384.0 + local[0],
            cell("m_cellY") * width - 16384.0 + local[1],
            cell("m_cellZ") * width - 16384.0 + local[2],
        ])
    }
}
