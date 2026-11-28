//! Client -> server messages, read by a host (hosting System Link games).
//! Layouts follow the 360 engine's writers (message-writers-360.txt) and our
//! own client writers in game.rs, which match the native client.
use crate::bits::BitReader;
use crate::game::{id, UserCmd};

#[derive(Debug, Clone, PartialEq)]
pub enum Clc {
    /// net_Tick: the newest snapshot tick the client holds (deltas start there).
    Tick { tick: u32, frametime: u16, deviation: u16 },
    StringCmd(String),
    SetConVar(Vec<(String, String)>),
    SignonState { state: u8, spawn_count: u32 },
    ClientInfo { server_count: u32, send_table_crc: u32, hltv: bool, friends_id: u32, friends_name: String },
    /// Commands oldest first; the last `new` are new.
    Move { new: u8, cmds: Vec<UserCmd> },
    VoiceData { bits: usize },
    BaselineAck { tick: u32, baseline: bool },
    ListenEvents,
    RespondCvarValue,
    LoadingProgress(u8),
    CmdKeyValues(Vec<u8>),
    Nop,
}

fn cstr(r: &mut BitReader) -> Option<String> {
    let mut out = Vec::new();
    loop {
        let b = r.u(8)? as u8;
        if b == 0 {
            return Some(String::from_utf8_lossy(&out).into_owned());
        }
        out.push(b);
    }
}

/// Inverse of game::write_usercmd.
fn read_usercmd(r: &mut BitReader, from: &UserCmd) -> Option<UserCmd> {
    let mut c = *from;
    let int = |r: &mut BitReader, fallback: u32| -> Option<u32> { if r.bit()? { r.u32(32) } else { Some(fallback) } };
    let float = |r: &mut BitReader, fallback: f32| -> Option<f32> {
        if r.bit()? { Some(f32::from_bits(r.u32(32)?)) } else { Some(fallback) }
    };
    c.command_number = int(r, from.command_number.wrapping_add(1))?;
    c.tick_count = int(r, from.tick_count.wrapping_add(1))?;
    for i in 0..3 {
        c.view[i] = float(r, from.view[i])?;
    }
    c.forward = float(r, from.forward)?;
    c.side = float(r, from.side)?;
    c.up = float(r, from.up)?;
    c.buttons = int(r, from.buttons)?;
    if r.bit()? {
        r.u(8)?; // impulse
    }
    if r.bit()? {
        c.weapon_select = r.u32(11)?;
        if r.bit()? {
            c.weapon_subtype = r.u32(6)?;
        }
    } else {
        // Weapon select is a one-shot field: unchanged means "no switch" only
        // when the previous command had none.
        c.weapon_select = from.weapon_select;
    }
    if r.bit()? {
        r.u(16)?; // mouse dx
    }
    if r.bit()? {
        r.u(16)?; // mouse dy
    }
    Some(c)
}

/// Read one client message after its 6-bit type.
pub fn read(r: &mut BitReader, kind: u64) -> Option<Clc> {
    Some(match kind {
        id::NOP => Clc::Nop,
        id::TICK => Clc::Tick { tick: r.u32(32)?, frametime: r.u(16)? as u16, deviation: r.u(16)? as u16 },
        id::STRING_CMD => Clc::StringCmd(cstr(r)?),
        id::SET_CONVAR => {
            let n = r.u(8)?;
            Clc::SetConVar((0..n).map(|_| Some((cstr(r)?, cstr(r)?))).collect::<Option<_>>()?)
        }
        id::SIGNON_STATE => {
            let state = r.u(8)? as u8;
            let spawn_count = r.u32(32)?;
            r.u(32)?; // players
            let n = r.u(32)? as usize;
            r.skip(n * 8)?;
            let m = r.u(32)? as usize;
            r.skip(m * 8)?;
            Clc::SignonState { state, spawn_count }
        }
        id::CLC_CLIENT_INFO => {
            let server_count = r.u32(32)?;
            let send_table_crc = r.u32(32)?;
            let hltv = r.bit()?;
            let friends_id = r.u32(32)?;
            let friends_name = cstr(r)?;
            for _ in 0..4 {
                if r.bit()? {
                    r.u(32)?;
                }
            }
            Clc::ClientInfo { server_count, send_table_crc, hltv, friends_id, friends_name }
        }
        id::CLC_MOVE => {
            let new = r.u(4)? as u8;
            let backup = r.u(3)? as usize;
            let len = r.u(16)? as usize;
            let end = r.pos + len;
            let mut cmds = Vec::new();
            let mut prev = UserCmd::default();
            for _ in 0..(new as usize + backup) {
                let c = read_usercmd(r, &prev)?;
                // A command's weapon select applies to it alone.
                prev = UserCmd { weapon_select: 0, weapon_subtype: 0, ..c };
                cmds.push(c);
            }
            if r.pos > end {
                return None;
            }
            r.skip(end - r.pos)?;
            Clc::Move { new, cmds }
        }
        10 => {
            // clc_VoiceData: 16-bit bit length, the speaker's xuid, then the bits.
            let len = r.u(16)? as usize;
            r.u(64)?;
            r.skip(len)?;
            Clc::VoiceData { bits: len }
        }
        id::CLC_BASELINE_ACK => Clc::BaselineAck { tick: r.u32(32)?, baseline: r.bit()? },
        id::CLC_LISTEN_EVENTS => {
            r.skip(512)?; // the event mask (a trailing set bit after it is block padding)
            Clc::ListenEvents
        }
        13 => {
            r.u(32)?;
            r.u(4)?;
            cstr(r)?;
            cstr(r)?;
            Clc::RespondCvarValue
        }
        15 => Clc::LoadingProgress(r.u(8)? as u8),
        id::CLC_CMD_KEY_VALUES => {
            let len = r.u(32)? as usize;
            Clc::CmdKeyValues(r.bytes(len)?)
        }
        _ => return None,
    })
}

/// Split a client block; None if a message is not understood.
pub fn split(data: &[u8], start: usize, end: usize) -> Option<Vec<Clc>> {
    let mut r = BitReader::range(data, start, end);
    let mut out = Vec::new();
    while r.left() >= 6 {
        let kind = r.u(6)?;
        out.push(read(&mut r, kind)?);
    }
    Some(out)
}
