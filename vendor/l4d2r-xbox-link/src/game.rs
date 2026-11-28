//! L4D2 (Xbox 360 build) game protocol over the NetChannel: net/svc/clc
//! messages, the connectionless connect handshake (q / A / k / B), lobby
//! (type-20 KeyValues) messages, and a client sign-on state machine that
//! mirrors a native 360 client (captured 2026-11-27).
//!
//! Message ids come from the 360 engine's own GetType() vtable entries.

use crate::bits::{BitReader, BitWriter};
use crate::matchmaking::{read_kv, section, write_kv, Kv};

pub mod id {
    pub const NOP: u64 = 0;
    pub const DISCONNECT: u64 = 1;
    pub const TICK: u64 = 4;
    pub const STRING_CMD: u64 = 5;
    pub const SET_CONVAR: u64 = 6;
    pub const SIGNON_STATE: u64 = 7;
    // server -> client
    pub const SVC_SERVER_INFO: u64 = 8;
    pub const SVC_SEND_TABLE: u64 = 9;
    pub const SVC_CLASS_INFO: u64 = 10;
    pub const SVC_SET_PAUSE: u64 = 11;
    pub const SVC_CREATE_STRING_TABLE: u64 = 12;
    pub const SVC_VOICE_INIT: u64 = 14;
    pub const SVC_BSP_DECAL: u64 = 21;
    pub const SVC_GAME_EVENT_LIST: u64 = 30;
    pub const SVC_UPDATE_STRING_TABLE: u64 = 13;
    pub const SVC_VOICE_DATA: u64 = 15;
    pub const SVC_PRINT: u64 = 16;
    pub const SVC_SOUNDS: u64 = 17;
    pub const SVC_SET_VIEW: u64 = 18;
    pub const SVC_FIX_ANGLE: u64 = 19;
    pub const SVC_CROSSHAIR_ANGLE: u64 = 20;
    pub const SVC_USER_MESSAGE: u64 = 23;
    pub const SVC_ENTITY_MESSAGE: u64 = 24;
    pub const SVC_GAME_EVENT: u64 = 25;
    pub const SVC_PACKET_ENTITIES: u64 = 26;
    pub const SVC_TEMP_ENTITIES: u64 = 27;
    pub const SVC_CMD_KEY_VALUES: u64 = 32;
    // client -> server
    pub const CLC_CLIENT_INFO: u64 = 8;
    pub const CLC_MOVE: u64 = 9;
    pub const CLC_BASELINE_ACK: u64 = 11;
    pub const CLC_LISTEN_EVENTS: u64 = 12;
    pub const CLC_CMD_KEY_VALUES: u64 = 17;
    /// Matchmaking KeyValues on the lobby channel (27027).
    pub const LOBBY_KEY_VALUES: u64 = 20;
}

pub const SIGNON_CONNECTED: u8 = 2;
pub const SIGNON_NEW: u8 = 3;
pub const SIGNON_PRESPAWN: u8 = 4;
pub const SIGNON_SPAWN: u8 = 5;
pub const SIGNON_FULL: u8 = 6;

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Tick { tick: u32 },
    StringCmd(String),
    SetConVar(Vec<(String, String)>),
    SignonState { state: u8, spawn_count: u32, map: String },
    Print(String),
    SetView(u32),
    /// Entity snapshot; `data`/`bits` hold the entity stream (see entities.rs).
    PacketEntities { delta_from: Option<u32>, baseline: bool, updated: u32, update_baseline: bool, data: Vec<u8>, bits: usize },
    /// Lobby-channel KeyValues (SysSession::...), or svc_CmdKeyValues.
    KeyValues(Kv),
    ServerInfo(ServerInfo),
    /// Name, max entries, entry count, user data (fixed size, bits) and the
    /// table data (bit length; data kept for later decoding).
    CreateStringTable { name: String, max_entries: u32, entries: u32, user_data_fixed: bool, user_data_bits: u32, compressed: bool, data: Vec<u8>, bits: usize },
    /// Class count; classes are built from the client's own tables when
    /// `create_on_client` (the 360 listen-server case).
    ClassInfo { classes: u32, create_on_client: bool, list: Vec<(u32, String, String)> },
    /// Changed entries for string table `table` (see entities::StringTables).
    UpdateStringTable { table: u32, changed: u32, data: Vec<u8>, bits: usize },
    /// svc_VoiceData: who is talking and their Speex bytes.
    VoiceData { client: u8, xuid: u64, data: Vec<u8>, bits: usize },
    /// Parsed and skipped (type, payload bits).
    Other(u64, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerInfo {
    pub protocol: u32,
    pub server_count: u32,
    pub dedicated: bool,
    pub max_classes: u32,
    pub player_slot: u32,
    pub max_clients: u32,
    pub tick_interval: f32,
    pub game_dir: String,
    pub map: String,
    pub sky: String,
    pub host_name: String,
}

fn floor_log2(n: u32) -> usize {
    if n == 0 { 0 } else { 31 - n.leading_zeros() as usize }
}

/// String-table entry-count width: Q_log2(max) + 1 (engine 0x86E1F1C0).
pub fn string_table_entry_bits(max: u32) -> usize {
    floor_log2(max) + 1
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

pub fn bit_vec3_coord(r: &mut BitReader) -> Option<[f32; 3]> {
    let flags = [r.bit()?, r.bit()?, r.bit()?];
    let mut v = [0.0; 3];
    for i in 0..3 {
        if flags[i] {
            v[i] = bit_coord(r)?;
        }
    }
    Some(v)
}

fn read_string(r: &mut BitReader) -> Option<String> {
    r.string(4096)
}

/// Read one server/lobby message whose type has already been read.
fn read_msg(r: &mut BitReader, kind: u64, lobby: bool) -> Option<Msg> {
    use id::*;
    if lobby {
        return match kind {
            LOBBY_KEY_VALUES => {
                r.u(32)?;
                let len = r.u(32)? as usize;
                let kv = read_kv(&r.bytes(len)?).ok()?;
                // Binary payload after the KeyValues (u32 size + bytes).
                if r.left() >= 32 {
                    let bin = r.u(32)? as usize;
                    r.skip(bin * 8)?;
                }
                Some(Msg::KeyValues(kv))
            }
            _ => None,
        };
    }
    Some(match kind {
        TICK => {
            let tick = r.u32(32)?;
            r.u(32)?;
            Msg::Tick { tick }
        }
        STRING_CMD => Msg::StringCmd(read_string(r)?),
        SET_CONVAR => {
            let n = r.u(8)?;
            Msg::SetConVar((0..n).map(|_| Some((read_string(r)?, read_string(r)?))).collect::<Option<_>>()?)
        }
        SIGNON_STATE => {
            let state = r.u(8)? as u8;
            let spawn_count = r.u32(32)?;
            let _players = r.u32(32)?;
            let ids = r.u(32)? as usize;
            r.skip(ids * 8)?;
            let maplen = r.u(32)? as usize;
            let map = String::from_utf8_lossy(&r.bytes(maplen)?).trim_end_matches('\0').to_string();
            Msg::SignonState { state, spawn_count, map }
        }
        SVC_SERVER_INFO => {
            let protocol = r.u32(16)?;
            let server_count = r.u32(32)?;
            r.bit()?; // HLTV
            let dedicated = r.bit()?;
            r.u(32)?; // client CRC
            r.u(32)?; // string table CRC
            let max_classes = r.u32(16)?;
            r.u(32)?; // map CRC
            let player_slot = r.u32(8)?;
            let max_clients = r.u32(8)?;
            let tick_interval = f32::from_bits(r.u32(32)?);
            r.u(8)?; // OS
            let game_dir = read_string(r)?;
            let map = read_string(r)?;
            let sky = read_string(r)?;
            let host_name = read_string(r)?;
            r.u(64)?; // two 360-specific words
            Msg::ServerInfo(ServerInfo { protocol, server_count, dedicated, max_classes, player_slot,
                max_clients, tick_interval, game_dir, map, sky, host_name })
        }
        SVC_SEND_TABLE => {
            r.bit()?;
            let len = r.u(16)? as usize;
            r.skip(len)?;
            Msg::Other(kind, len)
        }
        SVC_CLASS_INFO => {
            let classes = r.u32(16)?;
            let create_on_client = r.bit()?;
            let mut list = Vec::new();
            if !create_on_client {
                let bits = floor_log2(classes) + 1;
                for _ in 0..classes {
                    list.push((r.u32(bits)?, read_string(r)?, read_string(r)?));
                }
            }
            Msg::ClassInfo { classes, create_on_client, list }
        }
        SVC_CREATE_STRING_TABLE => {
            let name = read_string(r)?;
            let max_entries = r.u32(16)?;
            let entries = r.u32(string_table_entry_bits(max_entries))?;
            let bits = r.u(20)? as usize;
            r.bit()?; // 360-specific flag
            let user_data_fixed = r.bit()?;
            let mut user_data_bits = 0;
            if user_data_fixed {
                r.u(12)?;
                user_data_bits = r.u32(4)?;
            }
            let compressed = r.bit()?;
            r.bit()?;
            let data = r.bit_vec(bits)?;
            Msg::CreateStringTable { name, max_entries, entries, user_data_fixed, user_data_bits, compressed, data, bits }
        }
        SVC_VOICE_INIT => {
            read_string(r)?;
            r.u(8)?;
            Msg::Other(kind, 0)
        }
        SVC_BSP_DECAL => {
            bit_vec3_coord(r)?;
            r.u(9)?;
            if r.bit()? {
                r.u(22)?;
            }
            r.bit()?;
            Msg::Other(kind, 0)
        }
        SVC_GAME_EVENT_LIST => {
            r.u(9)?;
            let len = r.u(20)? as usize;
            r.skip(len)?;
            Msg::Other(kind, len)
        }
        SVC_SET_PAUSE => Msg::Other(kind, r.u(1).map(|_| 1)?),
        SVC_UPDATE_STRING_TABLE => {
            let table = r.u32(5)?;
            let changed = if r.bit()? { r.u32(16)? } else { 1 };
            let bits = r.u(20)? as usize;
            let data = r.bit_vec(bits)?;
            Msg::UpdateStringTable { table, changed, data, bits }
        }
        SVC_VOICE_DATA => {
            // 360 writer: client 8, proximity 8, bit length 16, xuid 64,
            // four split-screen flags, then the codec bits.
            let client = r.u(8)? as u8;
            r.u(8)?;
            let len = r.u(16)? as usize;
            let xuid = r.u(64)?;
            r.u(4)?;
            let data = r.bit_vec(len)?;
            Msg::VoiceData { client, xuid, data, bits: len }
        }
        SVC_PRINT => Msg::Print(read_string(r)?),
        SVC_SOUNDS => {
            let len = if r.bit()? { r.u(8)? } else { r.u(8)?; r.u(16)? } as usize;
            r.skip(len)?;
            Msg::Other(kind, len)
        }
        SVC_SET_VIEW => Msg::SetView(r.u32(11)?),
        SVC_FIX_ANGLE => {
            r.u(1 + 48)?;
            Msg::Other(kind, 49)
        }
        SVC_CROSSHAIR_ANGLE => {
            r.u(48)?;
            Msg::Other(kind, 48)
        }
        SVC_USER_MESSAGE => {
            r.u(8)?;
            let len = r.u(11)? as usize;
            r.skip(len)?;
            Msg::Other(kind, len)
        }
        SVC_ENTITY_MESSAGE => {
            r.u(20)?;
            let len = r.u(11)? as usize;
            r.skip(len)?;
            Msg::Other(kind, len)
        }
        SVC_GAME_EVENT => {
            let len = r.u(11)? as usize;
            r.skip(len)?;
            Msg::Other(kind, len)
        }
        SVC_PACKET_ENTITIES => {
            r.u(11)?;
            let delta_from = if r.bit()? { Some(r.u32(32)?) } else { None };
            let baseline = r.bit()?;
            let updated = r.u32(11)?;
            let len = r.u(20)? as usize;
            let update_baseline = r.bit()?;
            let data = r.bit_vec(len)?;
            Msg::PacketEntities { delta_from, baseline, updated, update_baseline, data, bits: len }
        }
        SVC_TEMP_ENTITIES => {
            r.u(8)?;
            let len = r.u(17)? as usize;
            r.skip(len)?;
            Msg::Other(kind, len)
        }
        SVC_CMD_KEY_VALUES => {
            let len = r.u(32)? as usize;
            Msg::KeyValues(read_kv(&r.bytes(len)?).ok()?)
        }
        _ => return None,
    })
}

/// Walk messages in `[start, end)`. Returns the messages and, if a message
/// type is not understood, the bit position where walking stopped.
pub fn walk(data: &[u8], start: usize, end: usize, lobby: bool) -> (Vec<Msg>, Option<usize>) {
    let mut r = BitReader::range(data, start, end);
    let mut out = Vec::new();
    while r.left() >= 6 {
        let at = r.pos;
        let kind = r.u(6).unwrap();
        if kind == id::NOP {
            continue;
        }
        match read_msg(&mut r, kind, lobby) {
            Some(m) => out.push(m),
            None => return (out, Some(at)),
        }
    }
    (out, None)
}

/// Locate host `SignonState` messages after `from` by their strict layout
/// (used inside sign-on blobs whose table messages are not decoded yet).
pub fn find_signon_states(data: &[u8], from: usize) -> Vec<Msg> {
    let total = data.len() * 8;
    let mut out = Vec::new();
    let mut p = from;
    while p + 46 + 96 <= total {
        let mut r = BitReader::range(data, p, total);
        if r.u(6) == Some(id::SIGNON_STATE) {
            if let Some(Msg::SignonState { state, spawn_count, map }) = read_msg(&mut r, id::SIGNON_STATE, false) {
                let plausible = (2..=6).contains(&state)
                    && spawn_count < 1_000_000
                    && !map.is_empty()
                    && map.len() < 128
                    && map.bytes().all(|b| b.is_ascii_graphic());
                if plausible {
                    out.push(Msg::SignonState { state, spawn_count, map });
                    p = r.pos;
                    continue;
                }
            }
        }
        p += 1;
    }
    out
}

// ------------------------------------------------------------ writers ---

pub fn write_set_convar(w: &mut BitWriter, vars: &[(String, String)]) {
    w.u(6, id::SET_CONVAR);
    w.u(8, vars.len() as u64);
    for (k, v) in vars {
        w.string(k);
        w.string(v);
    }
}

/// clc_VoiceData (360 writer: bit length 16, speaker xuid 64, then the bits).
pub fn write_voice(w: &mut BitWriter, xuid: u64, data: &[u8]) {
    w.u(6, 10);
    w.u(16, (data.len() * 8) as u64);
    w.u(64, xuid);
    w.bytes(data);
}

pub fn write_string_cmd(w: &mut BitWriter, cmd: &str) {
    w.u(6, id::STRING_CMD);
    w.string(cmd);
}

/// Client SignonState: player list and map are empty (96 zero bits).
pub fn write_signon_state(w: &mut BitWriter, state: u8, spawn_count: u32) {
    w.u(6, id::SIGNON_STATE);
    w.u(8, state as u64);
    w.u(32, spawn_count as u64);
    w.u(32, 0);
    w.u(32, 0);
    w.u(32, 0);
}

pub fn write_cmd_key_values(w: &mut BitWriter, kv: &Kv) {
    let bytes = write_kv(kv);
    w.u(6, id::CLC_CMD_KEY_VALUES);
    w.u(32, bytes.len() as u64);
    w.bytes(&bytes);
}

/// Send-table CRC the native 360 client reports for this build.
pub const SEND_TABLE_CRC: u32 = 0xff50_553d;
/// clc_ClientInfo as written by the 360 client (engine 0x86C4ED30): server
/// count, send-table CRC, HLTV bit, friends id, friends name, then four
/// custom-file slots (1 bit each, plus a 32-bit CRC when present).
pub fn write_client_info(w: &mut BitWriter, server_count: u32) {
    w.u(6, id::CLC_CLIENT_INFO);
    w.u(32, server_count as u64);
    w.u(32, SEND_TABLE_CRC as u64);
    w.bit(false); // not HLTV
    w.u(32, 0); // friends id
    w.string(""); // friends name
    w.u(4, 0); // no custom files
}

/// The game-event mask the native client listens with after prespawn.
pub const LISTEN_EVENTS_MASK: &str = "8b8a459781e0e303062f7ab80f6de3fb273627ff6eb8f9ffff53f00495ff7ffeffffff870160fcc5c35f803ef301000000000000000000000000000000000000";

pub fn write_listen_events(w: &mut BitWriter) {
    w.u(6, id::CLC_LISTEN_EVENTS);
    let mask: Vec<u8> = (0..LISTEN_EVENTS_MASK.len()).step_by(2)
        .map(|i| u8::from_str_radix(&LISTEN_EVENTS_MASK[i..i + 2], 16).unwrap()).collect();
    w.bytes(&mask);
    // The native client follows the 512-bit mask with one set bit.
    w.bit(true);
}

pub fn write_baseline_ack(w: &mut BitWriter, tick: u32, baseline: bool) {
    w.u(6, id::CLC_BASELINE_ACK);
    w.u(32, tick as u64);
    w.bit(baseline);
}

/// net_Tick: the latest snapshot tick we hold (the server deltas from it),
/// host frame time and its deviation (x 100000, as the native client sends).
pub fn write_tick(w: &mut BitWriter, tick: u32, frametime: u16, deviation: u16) {
    w.u(6, id::TICK);
    w.u(32, tick as u64);
    w.u(16, frametime as u64);
    w.u(16, deviation as u64);
}

/// One player command (Source CUserCmd fields carried by clc_Move).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UserCmd {
    pub command_number: u32,
    pub tick_count: u32,
    pub view: [f32; 3],
    pub forward: f32,
    pub side: f32,
    pub up: f32,
    pub buttons: u32,
    /// Entity index of the weapon to switch to (0 = no switch).
    pub weapon_select: u32,
    pub weapon_subtype: u32,
}

/// Source WriteUsercmd delta: 13 change flags (command, tick, 3 angles,
/// 3 moves, buttons, impulse, weapon select, mouse dx, mouse dy).
fn write_usercmd(w: &mut BitWriter, to: &UserCmd, from: &UserCmd) {
    let int_field = |w: &mut BitWriter, changed: bool, v: u32| {
        w.bit(changed);
        if changed {
            w.u(32, v as u64);
        }
    };
    int_field(w, to.command_number != from.command_number.wrapping_add(1), to.command_number);
    int_field(w, to.tick_count != from.tick_count.wrapping_add(1), to.tick_count);
    let floats = [(to.view[0], from.view[0]), (to.view[1], from.view[1]), (to.view[2], from.view[2]),
                  (to.forward, from.forward), (to.side, from.side), (to.up, from.up)];
    for (a, b) in floats {
        int_field(w, a != b, a.to_bits());
    }
    int_field(w, to.buttons != from.buttons, to.buttons);
    w.bit(false); // impulse
    // Weapon select (11 bits); its subtype follows only when it changed (360 WriteUsercmd).
    let select = to.weapon_select != from.weapon_select;
    w.bit(select);
    if select {
        w.u(11, to.weapon_select as u64);
        let sub = to.weapon_subtype != from.weapon_subtype;
        w.bit(sub);
        if sub {
            w.u(6, to.weapon_subtype as u64);
        }
    }
    w.u(2, 0); // mouse dx, mouse dy unchanged
}

/// clc_Move carrying `cmds` (oldest first; the last `new` are new commands).
pub fn write_move(w: &mut BitWriter, cmds: &[UserCmd], new: usize) {
    let mut data = BitWriter::new();
    let mut prev = UserCmd::default();
    for c in cmds {
        write_usercmd(&mut data, c, &prev);
        prev = *c;
    }
    w.u(6, id::CLC_MOVE);
    w.u(4, new as u64);
    w.u(3, (cmds.len() - new) as u64);
    w.u(16, data.bits as u64);
    w.append(&data);
}

/// Lobby-channel KeyValues message (type 20).
pub fn write_lobby_kv(w: &mut BitWriter, kv: &Kv) {
    let bytes = write_kv(kv);
    w.u(6, id::LOBBY_KEY_VALUES);
    w.u(32, 0);
    w.u(32, bytes.len() as u64);
    w.bytes(&bytes);
    // Binary payload size (none). A native 360 client always writes it: the
    // host reads it, so a message without it is cut short and dropped
    // (relay capture: 84-byte block for a 71-byte KeyValues).
    w.u(32, 0);
}

pub fn game_ready_command(xuid: u64, ready: bool) -> Kv {
    section(vec![(
        "SysSession::Command",
        section(vec![(
            "Game::Ready",
            section(vec![
                ("run", Kv::Str("host".into())),
                ("xuid", Kv::U64(xuid)),
                ("ready", Kv::Str(if ready { "yes" } else { "no" }.into())),
            ]),
        )]),
    )])
}

/// Pick a survivor in the lobby: GameUI's Game::Avatar {run "host", xuid,
/// avatar} with the lowercase name its character_<name> buttons carry
/// ("mechanic"); a capitalised value is ignored by the host.
pub fn game_avatar_command(xuid: u64, avatar: &str) -> Kv {
    game_avatar_variant(xuid, avatar, 0)
}

/// Game::Avatar forms to try until the host answers (key / value case):
/// 0 avatar+lower (GameUI's), 1 Avatar+lower, 2 avatar+Capital, 3 Avatar+Capital.
pub fn game_avatar_variant(xuid: u64, avatar: &str, variant: usize) -> Kv {
    let lower = avatar.to_ascii_lowercase();
    let capital: String = lower.chars().enumerate().map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c }).collect();
    let key = if variant % 2 == 0 { "avatar" } else { "Avatar" };
    let value = if variant < 2 { lower } else { capital };
    section(vec![(
        "SysSession::Command",
        section(vec![(
            "Game::Avatar",
            section(vec![("run", Kv::Str("host".into())), ("xuid", Kv::U64(xuid)), (key, Kv::Str(value))]),
        )]),
    )])
}

// --------------------------------------------------- connect handshake ---

pub fn connect_query() -> Vec<u8> {
    let mut v = vec![0xff; 4];
    v.extend_from_slice(b"qconnect0x00000000\0");
    v
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub challenge: u32,
    pub auth_mode: u32,
    pub echo: String,
    pub protocol: u32,
    pub password_required: bool,
    pub cookie: u64,
}

/// Parse an `A` challenge (payload after the 0xffffffff prefix).
pub fn parse_challenge(d: &[u8]) -> Option<Challenge> {
    if d.first()? != &b'A' {
        return None;
    }
    let mut p = 1usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = d.get(p..p + n)?;
        p += n;
        Some(s)
    };
    let challenge = u32::from_le_bytes(take(4)?.try_into().ok()?);
    let auth_mode = u32::from_le_bytes(take(4)?.try_into().ok()?);
    take(2 + 8 + 1)?;
    let rest = &d[p..];
    let nul = rest.iter().position(|&b| b == 0)?;
    let echo = String::from_utf8_lossy(&rest[..nul]).into_owned();
    let rest = &rest[nul + 1..];
    let protocol = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
    let rest = &rest[4..];
    let nul = rest.iter().position(|&b| b == 0)?;
    let rest = &rest[nul + 1..];
    let password_required = *rest.first()? == 1;
    let cookie = u64::from_le_bytes(rest.get(1..9)?.try_into().ok()?);
    Some(Challenge { challenge, auth_mode, echo, protocol, password_required, cookie })
}

/// Native default mode-2 authentication string.
pub const DEFAULT_AUTH: &str = "12345678901234567890123456789012";

/// The 26 user settings a native client announces (connect + SetConVar).
pub fn client_cvars(name: &str, xuid: u64, cookie: u64) -> Vec<(String, String)> {
    let id = format!("{:08X}:{:08X}:", (xuid >> 32) as u32, xuid as u32);
    let session = format!("${cookie:016x}");
    [
        ("tv_nochat", "0"), ("joy_autoaimdampen", "0.3"), ("name", name), ("cl_interp_ratio", "2"),
        ("cl_predict", "1"), ("cl_updaterate", "20"), ("networkid_force", &id), ("spec_allowroaming", "0"),
        ("cl_mouselook", "0"), ("cl_session", &session), ("cl_lagcompensation", "1"), ("closecaption", "0"),
        ("cl_language", "english"), ("spec_cinematic", "0"), ("cl_ideal_spec_mode", "5"),
        ("voice_spatialize", "0"), ("english", "1"), ("snd_musicvolume", "1"), ("cl_cmdrate", "30"),
        ("net_maxroutable", "1200"), ("rate", "20000"), ("cl_predictweapons", "1"), ("cl_autohelp", "1"),
        ("cl_interpolate", "1"), ("cl_interp", "0.1"), ("fov_desired", "90"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// `k` connect packet (with 0xffffffff prefix).
pub fn connect_packet(ch: &Challenge, name: &str, cvars: &[(String, String)]) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.u(8, b'k' as u64);
    w.u(32, ch.protocol as u64);
    w.u(32, ch.auth_mode as u64);
    w.u(32, ch.challenge as u64);
    w.string(name);
    w.string(""); // password
    w.u(8, 1); // one split-screen player
    w.u(6, 16); // clc_SplitPlayerConnect
    w.u(8, cvars.len() as u64);
    for (k, v) in cvars {
        w.string(k);
        w.string(v);
    }
    w.bit(false);
    w.u(64, ch.cookie);
    w.string(DEFAULT_AUTH);
    let mut out = vec![0xff; 4];
    out.extend_from_slice(&w.data);
    out
}

pub fn is_accept(d: &[u8]) -> bool {
    d.first() == Some(&b'B')
}
pub fn rejection(d: &[u8]) -> Option<String> {
    (d.first() == Some(&b'9')).then(|| String::from_utf8_lossy(&d[1..]).trim_end_matches('\0').to_string())
}

// ------------------------------------------------- sign-on client ---

/// Reacts to host messages like a native client; returns reliable and
/// unreliable messages to send.
#[derive(Debug, Default)]
pub struct Signon {
    pub state: u8,
    pub spawn_count: u32,
    pub map: String,
    pub last_tick: u32,
    pub prints: Vec<String>,
    pub snapshots: u64,
    /// Times the server sent us back to CONNECTED (Source "reconnect").
    pub resets: u32,
    /// Tick of the latest snapshot received (acknowledged with net_Tick).
    pub snapshot_tick: u32,
    /// Value sent in ClientInfo's first field on the latest attempt.
    pub client_info_count: u32,
    cvars: Vec<(String, String)>,
}

impl Signon {
    /// First reliable block after `B`.
    pub fn connected(&mut self, cvars: &[(String, String)], xuid: u64, avatar: &str) -> BitWriter {
        self.cvars = cvars.to_vec();
        let mut w = BitWriter::new();
        write_set_convar(&mut w, cvars);
        // Only when we chose a survivor; without one the host assigns ours.
        if !avatar.is_empty() {
            write_cmd_key_values(&mut w, &section(vec![(
                "avatarinfo",
                section(vec![
                    ("xuid", Kv::U64(xuid)),
                    ("team", Kv::Str("Survivor".into())),
                    ("Avatar", Kv::Str(avatar.into())),
                ]),
            )]));
        }
        write_signon_state(&mut w, SIGNON_CONNECTED, u32::MAX);
        self.state = SIGNON_CONNECTED;
        w
    }

    /// Handle host messages; returns (reliable, unreliable) replies.
    pub fn handle(&mut self, msgs: &[Msg]) -> (BitWriter, BitWriter) {
        let mut rel = BitWriter::new();
        let mut unrel = BitWriter::new();
        for m in msgs {
            match m {
                Msg::Print(t) => self.prints.push(t.clone()),
                Msg::Tick { tick } => self.last_tick = *tick,
                Msg::SignonState { state, spawn_count, map } => {
                    self.spawn_count = *spawn_count;
                    if !map.is_empty() {
                        self.map = map.clone();
                    }
                    match *state {
                        // Server reset us (Source Reconnect): restart sign-on.
                        SIGNON_CONNECTED if self.state > SIGNON_CONNECTED => {
                            self.resets += 1;
                            write_set_convar(&mut rel, &self.cvars);
                            write_signon_state(&mut rel, SIGNON_CONNECTED, u32::MAX);
                            self.state = SIGNON_CONNECTED;
                        }
                        SIGNON_NEW if self.state < SIGNON_NEW => {
                            // Live test (2026-11-27): 0..4 all rejected identically, so this
                            // really is the spawn count.
                            // Native block: ClientInfo then SignonState(NEW) with the
                            // current spawn count (a stale count makes the server reconnect us).
                            self.client_info_count = *spawn_count;
                            write_client_info(&mut rel, self.client_info_count);
                            write_signon_state(&mut rel, SIGNON_NEW, *spawn_count);
                            self.state = SIGNON_NEW;
                        }
                        SIGNON_PRESPAWN | SIGNON_SPAWN if self.state < *state => {
                            write_signon_state(&mut rel, *state, *spawn_count);
                            write_listen_events(&mut rel);
                            self.state = *state;
                        }
                        _ => {}
                    }
                }
                Msg::PacketEntities { baseline, update_baseline, .. } => {
                    self.snapshots += 1;
                    self.snapshot_tick = self.last_tick;
                    if *update_baseline {
                        write_baseline_ack(&mut rel, self.last_tick, *baseline);
                        if self.state == SIGNON_SPAWN {
                            write_signon_state(&mut rel, SIGNON_FULL, self.spawn_count);
                            write_listen_events(&mut rel);
                            self.state = SIGNON_FULL;
                        }
                    }
                }
                _ => {}
            }
        }
        let _ = &mut unrel;
        (rel, unrel)
    }
}
