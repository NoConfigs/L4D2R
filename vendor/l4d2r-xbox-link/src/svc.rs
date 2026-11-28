//! Server -> client messages for hosting: every field the 360 client reads,
//! with a reader (for checking against captures) and a writer. Messages a
//! server only passes through (game event list, decals, sounds...) are kept
//! as raw bits.
use crate::bits::{BitReader, BitWriter};
use crate::game::id;

fn cstr(r: &mut BitReader) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let b = r.u(8)? as u8;
        if b == 0 {
            return Some(out);
        }
        out.push(b);
    }
}
fn put_cstr(w: &mut BitWriter, s: &[u8]) {
    w.bytes(s);
    w.u(8, 0);
}
fn bit_coord_skip(r: &mut BitReader) -> Option<()> {
    let int = r.bit()?;
    let fract = r.bit()?;
    if int || fract {
        r.bit()?;
        if int {
            r.u(14)?;
        }
        if fract {
            r.u(5)?;
        }
    }
    Some(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerInfo {
    pub protocol: u16,
    pub server_count: u32,
    pub hltv: bool,
    pub dedicated: bool,
    pub client_crc: u32,
    pub table_crc: u32,
    pub max_classes: u16,
    pub map_crc: u32,
    pub player_slot: u8,
    pub max_clients: u8,
    pub tick_interval: f32,
    pub os: u8,
    pub game_dir: Vec<u8>,
    pub map: Vec<u8>,
    pub sky: Vec<u8>,
    pub host_name: Vec<u8>,
    /// Two 360-only words after the host name.
    pub extra: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTable {
    pub name: Vec<u8>,
    pub max_entries: u32,
    pub entries: u32,
    pub flag360: bool,
    /// (user data size in bytes, in bits) when entries have fixed-size data.
    pub fixed: Option<(u32, u32)>,
    pub compressed: bool,
    pub flag_after: bool,
    pub data: Vec<u8>,
    pub bits: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Svc {
    Tick { tick: u32, frametime: u16, deviation: u16 },
    Print(Vec<u8>),
    SetConVar(Vec<(Vec<u8>, Vec<u8>)>),
    SignonState { state: u8, spawn_count: u32, players: u32, ids: Vec<u8>, map: Vec<u8> },
    ServerInfo(ServerInfo),
    CreateStringTable(CreateTable),
    ClassInfo { classes: u16, create_on_client: bool, list: Vec<(u32, Vec<u8>, Vec<u8>)> },
    UpdateStringTable { table: u8, changed: Option<u16>, data: Vec<u8>, bits: usize },
    SetView(u16),
    PacketEntities { max_entries: u16, delta_from: Option<u32>, baseline: bool, updated: u16, update_baseline: bool, data: Vec<u8>, bits: usize },
    /// Passed through unchanged (message type and its payload bits).
    Raw { kind: u8, data: Vec<u8>, bits: usize },
}

fn entry_bits(max: u32) -> usize {
    crate::game::string_table_entry_bits(max)
}
fn class_bits(n: u32) -> usize {
    if n == 0 { 1 } else { (31 - n.leading_zeros()) as usize + 1 }
}

/// Payload size of a pass-through message (as game::read_msg skips it).
fn skip_raw(r: &mut BitReader, kind: u64) -> Option<()> {
    use id::*;
    match kind {
        SVC_SEND_TABLE => {
            r.bit()?;
            let len = r.u(16)? as usize;
            r.skip(len)?;
        }
        SVC_VOICE_INIT => {
            cstr(r)?;
            r.u(8)?;
        }
        SVC_BSP_DECAL => {
            let f = [r.bit()?, r.bit()?, r.bit()?];
            for on in f {
                if on {
                    bit_coord_skip(r)?;
                }
            }
            r.u(9)?;
            if r.bit()? {
                r.u(22)?;
            }
            r.bit()?;
        }
        SVC_GAME_EVENT_LIST => {
            r.u(9)?;
            let len = r.u(20)? as usize;
            r.skip(len)?;
        }
        SVC_SET_PAUSE => {
            r.u(1)?;
        }
        SVC_VOICE_DATA => {
            r.u(16)?;
            let len = r.u(16)? as usize;
            r.u(64)?;
            r.u(4)?;
            r.skip(len)?;
        }
        SVC_SOUNDS => {
            let len = if r.bit()? {
                r.u(8)?
            } else {
                r.u(8)?;
                r.u(16)?
            } as usize;
            r.skip(len)?;
        }
        SVC_FIX_ANGLE => {
            r.u(49)?;
        }
        SVC_CROSSHAIR_ANGLE => {
            r.u(48)?;
        }
        SVC_USER_MESSAGE => {
            r.u(8)?;
            let len = r.u(11)? as usize;
            r.skip(len)?;
        }
        SVC_ENTITY_MESSAGE => {
            r.u(20)?;
            let len = r.u(11)? as usize;
            r.skip(len)?;
        }
        SVC_GAME_EVENT => {
            let len = r.u(11)? as usize;
            r.skip(len)?;
        }
        SVC_TEMP_ENTITIES => {
            r.u(8)?;
            let len = r.u(17)? as usize;
            r.skip(len)?;
        }
        SVC_CMD_KEY_VALUES => {
            let len = r.u(32)? as usize;
            r.skip(len * 8)?;
        }
        STRING_CMD => {
            cstr(r)?;
        }
        // svc_Prefetch (360 type 28): a 14-bit sound index.
        28 => {
            r.u(14)?;
        }
        _ => return None,
    }
    Some(())
}

/// Read one message after its 6-bit type.
pub fn read(r: &mut BitReader, kind: u64) -> Option<Svc> {
    use id::*;
    let snapshot = r.clone();
    Some(match kind {
        TICK => Svc::Tick { tick: r.u32(32)?, frametime: r.u(16)? as u16, deviation: r.u(16)? as u16 },
        SVC_PRINT => Svc::Print(cstr(r)?),
        SET_CONVAR => {
            let n = r.u(8)?;
            Svc::SetConVar((0..n).map(|_| Some((cstr(r)?, cstr(r)?))).collect::<Option<_>>()?)
        }
        SIGNON_STATE => {
            let state = r.u(8)? as u8;
            let spawn_count = r.u32(32)?;
            let players = r.u32(32)?;
            let n = r.u(32)? as usize;
            let ids = r.bytes(n)?;
            let m = r.u(32)? as usize;
            let map = r.bytes(m)?;
            Svc::SignonState { state, spawn_count, players, ids, map }
        }
        SVC_SERVER_INFO => Svc::ServerInfo(ServerInfo {
            protocol: r.u(16)? as u16,
            server_count: r.u32(32)?,
            hltv: r.bit()?,
            dedicated: r.bit()?,
            client_crc: r.u32(32)?,
            table_crc: r.u32(32)?,
            max_classes: r.u(16)? as u16,
            map_crc: r.u32(32)?,
            player_slot: r.u(8)? as u8,
            max_clients: r.u(8)? as u8,
            tick_interval: f32::from_bits(r.u32(32)?),
            os: r.u(8)? as u8,
            game_dir: cstr(r)?,
            map: cstr(r)?,
            sky: cstr(r)?,
            host_name: cstr(r)?,
            extra: r.u(64)?,
        }),
        SVC_CREATE_STRING_TABLE => {
            let name = cstr(r)?;
            let max_entries = r.u32(16)?;
            let entries = r.u32(entry_bits(max_entries))?;
            let bits = r.u(20)? as usize;
            let flag360 = r.bit()?;
            let fixed = if r.bit()? { Some((r.u32(12)?, r.u32(4)?)) } else { None };
            let compressed = r.bit()?;
            let flag_after = r.bit()?;
            let data = r.bit_vec(bits)?;
            Svc::CreateStringTable(CreateTable { name, max_entries, entries, flag360, fixed, compressed, flag_after, data, bits })
        }
        SVC_CLASS_INFO => {
            let classes = r.u(16)? as u16;
            let create_on_client = r.bit()?;
            let mut list = Vec::new();
            if !create_on_client {
                for _ in 0..classes {
                    list.push((r.u32(class_bits(classes as u32))?, cstr(r)?, cstr(r)?));
                }
            }
            Svc::ClassInfo { classes, create_on_client, list }
        }
        SVC_UPDATE_STRING_TABLE => {
            let table = r.u(5)? as u8;
            let changed = if r.bit()? { Some(r.u(16)? as u16) } else { None };
            let bits = r.u(20)? as usize;
            Svc::UpdateStringTable { table, changed, data: r.bit_vec(bits)?, bits }
        }
        SVC_SET_VIEW => Svc::SetView(r.u(11)? as u16),
        SVC_PACKET_ENTITIES => {
            let max_entries = r.u(11)? as u16;
            let delta_from = if r.bit()? { Some(r.u32(32)?) } else { None };
            let baseline = r.bit()?;
            let updated = r.u(11)? as u16;
            let bits = r.u(20)? as usize;
            let update_baseline = r.bit()?;
            Svc::PacketEntities { max_entries, delta_from, baseline, updated, update_baseline, data: r.bit_vec(bits)?, bits }
        }
        _ => {
            skip_raw(r, kind)?;
            let bits = r.pos - snapshot.pos;
            let mut back = snapshot;
            let data = back.bit_vec(bits)?;
            Svc::Raw { kind: kind as u8, data, bits }
        }
    })
}

fn append_bits(w: &mut BitWriter, data: &[u8], bits: usize) {
    let mut r = BitReader::range(data, 0, bits);
    while r.left() > 0 {
        let k = r.left().min(32);
        w.u(k, r.u(k).unwrap());
    }
}

/// Write one message with its 6-bit type.
pub fn write(w: &mut BitWriter, m: &Svc) {
    use id::*;
    match m {
        Svc::Tick { tick, frametime, deviation } => {
            w.u(6, TICK);
            w.u(32, *tick as u64);
            w.u(16, *frametime as u64);
            w.u(16, *deviation as u64);
        }
        Svc::Print(t) => {
            w.u(6, SVC_PRINT);
            put_cstr(w, t);
        }
        Svc::SetConVar(vars) => {
            w.u(6, SET_CONVAR);
            w.u(8, vars.len() as u64);
            for (k, v) in vars {
                put_cstr(w, k);
                put_cstr(w, v);
            }
        }
        Svc::SignonState { state, spawn_count, players, ids, map } => {
            w.u(6, SIGNON_STATE);
            w.u(8, *state as u64);
            w.u(32, *spawn_count as u64);
            w.u(32, *players as u64);
            w.u(32, ids.len() as u64);
            w.bytes(ids);
            w.u(32, map.len() as u64);
            w.bytes(map);
        }
        Svc::ServerInfo(i) => {
            w.u(6, SVC_SERVER_INFO);
            w.u(16, i.protocol as u64);
            w.u(32, i.server_count as u64);
            w.bit(i.hltv);
            w.bit(i.dedicated);
            w.u(32, i.client_crc as u64);
            w.u(32, i.table_crc as u64);
            w.u(16, i.max_classes as u64);
            w.u(32, i.map_crc as u64);
            w.u(8, i.player_slot as u64);
            w.u(8, i.max_clients as u64);
            w.u(32, i.tick_interval.to_bits() as u64);
            w.u(8, i.os as u64);
            put_cstr(w, &i.game_dir);
            put_cstr(w, &i.map);
            put_cstr(w, &i.sky);
            put_cstr(w, &i.host_name);
            w.u(64, i.extra);
        }
        Svc::CreateStringTable(t) => {
            w.u(6, SVC_CREATE_STRING_TABLE);
            put_cstr(w, &t.name);
            w.u(16, t.max_entries as u64);
            w.u(entry_bits(t.max_entries), t.entries as u64);
            w.u(20, t.bits as u64);
            w.bit(t.flag360);
            w.bit(t.fixed.is_some());
            if let Some((size, bits)) = t.fixed {
                w.u(12, size as u64);
                w.u(4, bits as u64);
            }
            w.bit(t.compressed);
            w.bit(t.flag_after);
            append_bits(w, &t.data, t.bits);
        }
        Svc::ClassInfo { classes, create_on_client, list } => {
            w.u(6, SVC_CLASS_INFO);
            w.u(16, *classes as u64);
            w.bit(*create_on_client);
            if !create_on_client {
                for (i, a, b) in list {
                    w.u(class_bits(*classes as u32), *i as u64);
                    put_cstr(w, a);
                    put_cstr(w, b);
                }
            }
        }
        Svc::UpdateStringTable { table, changed, data, bits } => {
            w.u(6, SVC_UPDATE_STRING_TABLE);
            w.u(5, *table as u64);
            w.bit(changed.is_some());
            if let Some(c) = changed {
                w.u(16, *c as u64);
            }
            w.u(20, *bits as u64);
            append_bits(w, data, *bits);
        }
        Svc::SetView(e) => {
            w.u(6, SVC_SET_VIEW);
            w.u(11, *e as u64);
        }
        Svc::PacketEntities { max_entries, delta_from, baseline, updated, update_baseline, data, bits } => {
            w.u(6, SVC_PACKET_ENTITIES);
            w.u(11, *max_entries as u64);
            w.bit(delta_from.is_some());
            if let Some(t) = delta_from {
                w.u(32, *t as u64);
            }
            w.bit(*baseline);
            w.u(11, *updated as u64);
            w.u(20, *bits as u64);
            w.bit(*update_baseline);
            append_bits(w, data, *bits);
        }
        Svc::Raw { kind, data, bits } => {
            w.u(6, *kind as u64);
            append_bits(w, data, *bits);
        }
    }
}

/// Split a message block into messages (None if a type is not understood).
pub fn split(data: &[u8], start: usize, end: usize) -> Option<Vec<Svc>> {
    let mut r = BitReader::range(data, start, end);
    let mut out = Vec::new();
    while r.left() >= 6 {
        let kind = r.u(6)?;
        if kind == id::NOP {
            out.push(Svc::Raw { kind: 0, data: Vec::new(), bits: 0 });
            continue;
        }
        out.push(read(&mut r, kind)?);
    }
    Some(out)
}

// ------------------------------------------------------------ game events ---

/// One event type from svc_GameEventList: id, name and its keys (type, name).
/// Key types: 1 string, 2 float, 3 long, 4 short, 5 byte, 6 bool, 7 uint64.
#[derive(Debug, Clone, PartialEq)]
pub struct EventType {
    pub id: u32,
    pub name: String,
    pub keys: Vec<(u8, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventValue {
    Str(String),
    Float(f32),
    Int(i64),
    Bool(bool),
}

impl EventValue {
    pub fn as_str(&self) -> Option<&str> {
        if let EventValue::Str(s) = self { Some(s) } else { None }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            EventValue::Int(v) => Some(*v),
            EventValue::Bool(b) => Some(*b as i64),
            EventValue::Float(f) => Some(*f as i64),
            EventValue::Str(_) => None,
        }
    }
}

fn str_lossy(r: &mut BitReader) -> Option<String> {
    cstr(r).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Parse a raw svc_GameEventList payload (the bits after the 6-bit type).
pub fn parse_event_list(data: &[u8], bits: usize) -> Option<Vec<EventType>> {
    let mut r = BitReader::range(data, 0, bits);
    let count = r.u(9)? as usize;
    let len = r.u(20)? as usize;
    let end = r.pos + len;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let id = r.u32(9)?;
        let name = str_lossy(&mut r)?;
        let mut keys = Vec::new();
        loop {
            let ty = r.u(3)? as u8;
            if ty == 0 {
                break;
            }
            keys.push((ty, str_lossy(&mut r)?));
        }
        out.push(EventType { id, name, keys });
    }
    (r.pos == end).then_some(out)
}

/// Decode a raw svc_GameEvent payload (bits after the 6-bit type) with the
/// event list. Returns the event name and its values.
pub fn parse_event(list: &[EventType], data: &[u8], bits: usize) -> Option<(String, Vec<(String, EventValue)>)> {
    let mut r = BitReader::range(data, 0, bits);
    let len = r.u(11)? as usize;
    let end = r.pos + len;
    let id = r.u32(9)?;
    let ty = list.iter().find(|e| e.id == id)?;
    let mut values = Vec::new();
    for (kind, key) in &ty.keys {
        let v = match kind {
            1 => EventValue::Str(str_lossy(&mut r)?),
            2 => EventValue::Float(f32::from_bits(r.u32(32)?)),
            3 => EventValue::Int(r.u32(32)? as i32 as i64),
            4 => EventValue::Int(r.u(16)? as u16 as i16 as i64),
            5 => EventValue::Int(r.u(8)? as i64),
            6 => EventValue::Bool(r.bit()?),
            7 => EventValue::Int(r.u(64)? as i64),
            _ => return None,
        };
        values.push((key.clone(), v));
    }
    (r.pos == end).then(|| (ty.name.clone(), values))
}
