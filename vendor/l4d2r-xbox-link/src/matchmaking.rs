//! Xbox 360 L4D2 matchmaking messages: binary KeyValues (big-endian ints on
//! the 360), the connectionless envelope, session-info nibble text, and the
//! LanSearch / GameDetailsPlayer / RequestJoinData / ReplyJoinData messages.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kv {
    Section(Vec<(String, Kv)>),
    Str(String),
    Int(i32),
    Ptr(u32),
    U64(u64),
}

impl Kv {
    pub fn get(&self, path: &[&str]) -> Option<&Kv> {
        let mut node = self;
        for name in path {
            let Kv::Section(children) = node else { return None };
            node = &children.iter().find(|(n, _)| n.eq_ignore_ascii_case(name))?.1;
        }
        Some(node)
    }
    pub fn str(&self, path: &[&str]) -> Option<&str> {
        match self.get(path)? {
            Kv::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn int(&self, path: &[&str]) -> Option<i64> {
        match self.get(path)? {
            Kv::Int(v) => Some(*v as i64),
            Kv::Ptr(v) => Some(*v as i64),
            Kv::U64(v) => Some(*v as i64),
            _ => None,
        }
    }
    pub fn u64(&self, path: &[&str]) -> Option<u64> {
        match self.get(path)? {
            Kv::U64(v) => Some(*v),
            _ => None,
        }
    }
}

pub fn section(children: Vec<(&str, Kv)>) -> Kv {
    Kv::Section(children.into_iter().map(|(n, v)| (n.to_string(), v)).collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvError(pub &'static str);

/// Parse a binary KeyValues stream: a node list terminated by 0x0B.
pub fn read_kv(data: &[u8]) -> Result<Kv, KvError> {
    let mut pos = 0usize;
    let mut count = 0usize;
    let root = read_section(data, &mut pos, 0, &mut count)?;
    if pos != data.len() {
        return Err(KvError("trailing KeyValues bytes"));
    }
    Ok(Kv::Section(root))
}

fn take<'a>(data: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], KvError> {
    let out = data.get(*pos..*pos + n).ok_or(KvError("truncated KeyValues"))?;
    *pos += n;
    Ok(out)
}

fn read_string(data: &[u8], pos: &mut usize) -> Result<String, KvError> {
    let rest = &data[*pos..];
    let end = rest.iter().position(|&b| b == 0).ok_or(KvError("unterminated string"))?;
    if end >= 1024 {
        return Err(KvError("oversized string"));
    }
    let s = String::from_utf8_lossy(&rest[..end]).into_owned();
    *pos += end + 1;
    Ok(s)
}

fn read_section(data: &[u8], pos: &mut usize, depth: usize, count: &mut usize) -> Result<Vec<(String, Kv)>, KvError> {
    if depth > 16 {
        return Err(KvError("KeyValues nested too deep"));
    }
    let mut nodes = Vec::new();
    loop {
        let kind = take(data, pos, 1)?[0];
        if kind == 11 {
            return Ok(nodes);
        }
        *count += 1;
        if *count > 512 {
            return Err(KvError("too many KeyValues nodes"));
        }
        let name = read_string(data, pos)?;
        let value = match kind {
            0 => Kv::Section(read_section(data, pos, depth + 1, count)?),
            1 => Kv::Str(read_string(data, pos)?),
            2 => Kv::Int(i32::from_be_bytes(take(data, pos, 4)?.try_into().unwrap())),
            4 => Kv::Ptr(u32::from_be_bytes(take(data, pos, 4)?.try_into().unwrap())),
            7 => Kv::U64(u64::from_be_bytes(take(data, pos, 8)?.try_into().unwrap())),
            _ => return Err(KvError("unsupported KeyValues type")),
        };
        nodes.push((name, value));
    }
}

pub fn write_kv(root: &Kv) -> Vec<u8> {
    let mut out = Vec::new();
    if let Kv::Section(children) = root {
        write_section(children, &mut out);
    }
    out
}

fn write_section(children: &[(String, Kv)], out: &mut Vec<u8>) {
    for (name, value) in children {
        let kind = match value {
            Kv::Section(_) => 0,
            Kv::Str(_) => 1,
            Kv::Int(_) => 2,
            Kv::Ptr(_) => 4,
            Kv::U64(_) => 7,
        };
        out.push(kind);
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        match value {
            Kv::Section(c) => write_section(c, out),
            Kv::Str(s) => {
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Kv::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
            Kv::Ptr(v) => out.extend_from_slice(&v.to_be_bytes()),
            Kv::U64(v) => out.extend_from_slice(&v.to_be_bytes()),
        }
    }
    out.push(11);
}

/// Connectionless matchmaking message:
/// `ffffffff, u32 0, u32 engine (LE), u32 kv_size (LE), kv, u32 binary_size (LE), binary`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub engine: u32,
    pub kv: Kv,
    pub binary: Vec<u8>,
}

impl Message {
    pub fn new(kv: Kv) -> Self {
        Message { engine: 0, kv, binary: vec![] }
    }
    /// Name of the top-level node ("LanSearch", "SysSession::RequestJoinData", ...).
    pub fn name(&self) -> Option<&str> {
        match &self.kv {
            Kv::Section(c) => c.first().map(|(n, _)| n.as_str()),
            _ => None,
        }
    }
    pub fn body(&self) -> Option<&Kv> {
        match &self.kv {
            Kv::Section(c) => c.first().map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn encode(&self) -> Vec<u8> {
        let kv = write_kv(&self.kv);
        let mut out = vec![0xff; 4];
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&self.engine.to_le_bytes());
        out.extend_from_slice(&(kv.len() as u32).to_le_bytes());
        out.extend_from_slice(&kv);
        out.extend_from_slice(&(self.binary.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.binary);
        out
    }
    pub fn decode(data: &[u8]) -> Result<Self, KvError> {
        if data.len() < 20 || data[..4] != [0xff; 4] {
            return Err(KvError("not a connectionless matchmaking message"));
        }
        let engine = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let size = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;
        let kv = read_kv(data.get(16..16 + size).ok_or(KvError("truncated KeyValues block"))?)?;
        let bsize_at = 16 + size;
        let bsize = u32::from_le_bytes(
            data.get(bsize_at..bsize_at + 4).ok_or(KvError("missing binary size"))?.try_into().unwrap(),
        ) as usize;
        let binary = data.get(bsize_at + 4..bsize_at + 4 + bsize).ok_or(KvError("truncated binary"))?.to_vec();
        Ok(Message { engine, kv, binary })
    }
    /// Discovery broadcasts prefix the message with a big-endian u16 length.
    pub fn encode_discovery(&self) -> Vec<u8> {
        let inner = self.encode();
        let mut out = (inner.len() as u16).to_be_bytes().to_vec();
        out.extend_from_slice(&inner);
        out
    }
    pub fn decode_discovery(body: &[u8]) -> Result<Self, KvError> {
        if body.len() < 2 || u16::from_be_bytes([body[0], body[1]]) as usize != body.len() - 2 {
            return Err(KvError("bad discovery length prefix"));
        }
        Self::decode(&body[2..])
    }
}

/// Session info / XNADDR text: each byte is two chars `0x30 + nibble`,
/// low nibble first.
pub fn decode_nibbles(text: &str) -> Option<Vec<u8>> {
    let b = text.as_bytes();
    if b.len() % 2 != 0 || b.iter().any(|&c| !(0x30..=0x3f).contains(&c)) {
        return None;
    }
    Some(b.chunks(2).map(|p| (p[0] - 0x30) | ((p[1] - 0x30) << 4)).collect())
}

pub fn encode_nibbles(bytes: &[u8]) -> String {
    bytes.iter().flat_map(|b| [(0x30 + (b & 15)) as char, (0x30 + (b >> 4)) as char]).collect()
}

pub fn lan_search() -> Message {
    Message::new(section(vec![("LanSearch", Kv::Section(vec![]))]))
}

/// A System Link lobby advertised by a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lobby {
    pub host_name: String,
    pub host_xuid: u64,
    pub session_id: [u8; 8],
    pub host_xnaddr: [u8; 36],
    pub session_key: [u8; 16],
    /// Parsed settings blob (`Settings{System, game, Options, Members ...}`), if present.
    pub settings: Option<Kv>,
}

impl Lobby {
    pub fn host_mac(&self) -> [u8; 6] {
        self.host_xnaddr[10..16].try_into().unwrap()
    }
    pub fn setting_str(&self, path: &[&str]) -> Option<&str> {
        self.settings.as_ref()?.str(&[&["Settings"], path].concat())
    }
    pub fn setting_int(&self, path: &[&str]) -> Option<i64> {
        self.settings.as_ref()?.int(&[&["Settings"], path].concat())
    }
}

pub fn parse_game_details(msg: &Message) -> Option<Lobby> {
    if msg.name()? != "GameDetailsPlayer" {
        return None;
    }
    let body = msg.body()?;
    let session = decode_nibbles(body.str(&["Options", "sessioninfo"])?)?;
    if session.len() != 60 {
        return None;
    }
    // Settings blob: 8 zero bytes, KeyValues, 4 zero bytes.
    let settings = (msg.binary.len() >= 12).then(|| read_kv(&msg.binary[8..msg.binary.len() - 4]).ok()).flatten();
    Some(Lobby {
        host_name: body.str(&["player", "name"]).unwrap_or("").to_string(),
        host_xuid: body.u64(&["player", "xuid"]).unwrap_or(0),
        session_id: session[..8].try_into().unwrap(),
        host_xnaddr: session[8..44].try_into().unwrap(),
        session_key: session[44..60].try_into().unwrap(),
        settings,
    })
}

/// What a System Link host advertises in its `GameDetailsPlayer` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advert {
    pub host_name: String,
    pub host_xuid: u64,
    pub host_xuid_online: u64,
    pub session_id: [u8; 8],
    pub host_xnaddr: [u8; 36],
    pub session_key: [u8; 16],
    /// `Settings{System, Options, Members, game ...}`.
    pub settings: Kv,
}

/// Size of the settings blob a 360 host sends (the `Binary.size` field).
pub const SETTINGS_BLOB: usize = 256;
/// The 360 host's `Binary.ptr` (a console heap address; the reader ignores it).
const SETTINGS_PTR: u32 = 0xdc16db00;

/// A host's `GameDetailsPlayer` reply to `LanSearch` (same layout as a 360 host).
pub fn game_details(a: &Advert) -> Message {
    let mut info = a.session_id.to_vec();
    info.extend_from_slice(&a.host_xnaddr);
    info.extend_from_slice(&a.session_key);
    let mut binary = vec![0u8; 8];
    binary.extend_from_slice(&write_kv(&a.settings));
    binary.extend_from_slice(&[0; 4]);
    if binary.len() < SETTINGS_BLOB {
        binary.resize(SETTINGS_BLOB, 0);
    }
    let size = binary.len() as i32;
    let kv = section(vec![(
        "GameDetailsPlayer",
        section(vec![
            ("Options", section(vec![("sessioninfo", Kv::Str(encode_nibbles(&info)))])),
            (
                "player",
                section(vec![
                    ("name", Kv::Str(a.host_name.clone())),
                    ("xuid", Kv::U64(a.host_xuid)),
                    ("xuidOnline", Kv::U64(a.host_xuid_online)),
                ]),
            ),
            ("Binary", section(vec![("ptr", Kv::Ptr(SETTINGS_PTR)), ("size", Kv::Int(size))])),
        ]),
    )]);
    Message { engine: 0, kv, binary }
}

/// Our identity in the lobby.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub xuid: u64,
    pub name: String,
    pub xnaddr: [u8; 36],
    pub dlcmask: u64,
}

/// Engine build string both the base game and the final title update report.
pub const TUVER: &str = "20120724";

/// `SysSession::RequestJoinData`, laid out exactly as a native 360 client sends it.
pub fn request_join_data(player: &Player, session_id: &[u8; 8]) -> Message {
    let machine = section(vec![
        ("id", Kv::U64(player.xuid)),
        ("numPlayers", Kv::Int(1)),
        ("dlcmask", Kv::U64(player.dlcmask)),
        ("tuver", Kv::Str(TUVER.into())),
        ("ping", Kv::Int(0)),
        ("player0", section(vec![("xuid", Kv::U64(player.xuid)), ("name", Kv::Str(player.name.clone()))])),
        ("xnaddr", Kv::Str(encode_nibbles(&player.xnaddr))),
    ]);
    let members = section(vec![
        ("numMachines", Kv::Int(1)),
        ("numPlayers", Kv::Int(1)),
        ("numSlots", Kv::Int(1)),
        ("machine0", machine),
    ]);
    Message::new(section(vec![(
        "SysSession::RequestJoinData",
        section(vec![
            ("id", Kv::U64(player.xuid)),
            ("Settings", section(vec![("Members", members)])),
            ("sessionid", Kv::U64(u64::from_be_bytes(*session_id))),
        ]),
    )]))
}
