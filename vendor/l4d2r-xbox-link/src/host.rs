//! Hosting System Link games for 360 clients (reverse direction).
//!
//! Step 1 (discovery): answer `LanSearch` broadcasts with a `GameDetailsPlayer`
//! laid out exactly like a 360 host's (checked byte for byte against a capture
//! in tests/local_capture.rs). The association responder, lobby host and game
//! server are later steps (PLAN.md).
use crate::matchmaking::{game_details, section, Advert, Kv, Message};
use crate::xnet::{decode_broadcast, encode_broadcast, ethernet_frame, parse_frame, Broadcast, Keys, BROADCAST_MAC};

/// What the game is (the 360's `Settings` blob).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameSettings {
    /// "coop", "survival", ...
    pub mode: String,
    /// "easy", "normal", "hard", "impossible".
    pub difficulty: String,
    /// 360 mission code ("L4D2C1").
    pub campaign: String,
    pub chapter: i32,
    /// "lobby" or "game".
    pub state: String,
    pub players: i32,
    pub slots: i32,
}

impl GameSettings {
    /// The settings tree in a 360 host's key order.
    pub fn kv(&self) -> Kv {
        section(vec![(
            "Settings",
            section(vec![
                (
                    "System",
                    section(vec![
                        ("netflag", Kv::Str(String::new())),
                        ("network", Kv::Str("lan".into())),
                        ("access", Kv::Str("public".into())),
                    ]),
                ),
                ("Options", section(vec![("server", Kv::Str("listen".into()))])),
                ("Members", section(vec![("numPlayers", Kv::Int(self.players)), ("numSlots", Kv::Int(self.slots))])),
                (
                    "game",
                    section(vec![
                        ("MissionInfo", section(vec![("Version", Kv::Str("1".into()))])),
                        ("mode", Kv::Str(self.mode.clone())),
                        ("difficulty", Kv::Str(self.difficulty.clone())),
                        ("campaign", Kv::Str(self.campaign.clone())),
                        ("chapter", Kv::Int(self.chapter)),
                        ("state", Kv::Str(self.state.clone())),
                        ("maxrounds", Kv::Int(3)),
                    ]),
                ),
            ]),
        )])
    }
}

/// If `frame` is another console's LanSearch broadcast, the Ethernet frame of
/// our GameDetailsPlayer reply (broadcast, same ports and title metadata).
pub fn answer_search(keys: &Keys, frame: &[u8], our_mac: [u8; 6], advert: &Advert, sequence: u16) -> Option<Vec<u8>> {
    let f = parse_frame(frame)?;
    if f.destination != BROADCAST_MAC || f.source == our_mac {
        return None;
    }
    let search = decode_broadcast(keys, f.payload).ok()?;
    let msg = Message::decode_discovery(&search.body).ok()?;
    if msg.name()? != "LanSearch" {
        return None;
    }
    let reply = Broadcast {
        sequence,
        source_port: search.destination_port,
        destination_port: search.source_port,
        body: game_details(advert).encode_discovery(),
        metadata: search.metadata,
    };
    Some(ethernet_frame(&encode_broadcast(keys, &reply), our_mac, BROADCAST_MAC))
}

/// The host's own lobby member (machine0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostMember {
    pub xuid: u64,
    pub name: String,
    /// Survivor avatar ("coach", "mechanic", ...).
    pub avatar: String,
    pub dlcmask: u64,
    pub xnaddr: [u8; 36],
}

/// Step 3: the lobby side of a System Link host (VDP 27027 KeyValues), laid
/// out like a 360 host's messages (relay capture 2026-11-27).
#[derive(Debug, Clone)]
pub struct LobbyHost {
    pub settings: GameSettings,
    pub host: HostMember,
    /// Joined machines' blocks, kept as their RequestJoinData sent them.
    pub machines: Vec<Kv>,
    /// Session nonce; it is also the game reservation id sent at START.
    pub nonce: u64,
}

fn kv_str(s: &str) -> Kv {
    Kv::Str(s.to_string())
}

impl LobbyHost {
    pub fn new(settings: GameSettings, host: HostMember, nonce: u64) -> Self {
        LobbyHost { settings, host, machines: Vec::new(), nonce }
    }

    fn members(&self) -> Kv {
        let h = &self.host;
        let mut list = vec![
            ("numMachines".to_string(), Kv::Int(1 + self.machines.len() as i32)),
            ("numPlayers".to_string(), Kv::Int(1 + self.machines.len() as i32)),
            ("numSlots".to_string(), Kv::Int(self.settings.slots)),
            (
                "machine0".to_string(),
                section(vec![
                    ("id", Kv::U64(h.xuid)),
                    ("numPlayers", Kv::Int(1)),
                    ("dlcmask", Kv::U64(h.dlcmask)),
                    ("tuver", kv_str("20120724")),
                    ("ping", Kv::Int(0)),
                    (
                        "player0",
                        section(vec![
                            ("xuid", Kv::U64(h.xuid)),
                            ("name", kv_str(&h.name)),
                            ("game", section(vec![("Avatar", kv_str(&h.avatar))])),
                        ]),
                    ),
                    ("xnaddr", Kv::Str(crate::matchmaking::encode_nibbles(&h.xnaddr))),
                ]),
            ),
        ];
        for (i, m) in self.machines.iter().enumerate() {
            list.push((format!("machine{}", i + 1), m.clone()));
        }
        Kv::Section(list)
    }

    /// The full `Settings` tree sent in ReplyJoinData.
    pub fn settings_kv(&self) -> Kv {
        let s = &self.settings;
        section(vec![
            (
                "System",
                section(vec![("network", kv_str("lan")), ("access", kv_str("public")), ("nonce", Kv::U64(self.nonce))]),
            ),
            (
                "game",
                section(vec![
                    ("mode", kv_str(&s.mode)),
                    ("difficulty", kv_str(&s.difficulty)),
                    ("campaign", kv_str(&s.campaign)),
                    ("chapter", Kv::Int(s.chapter)),
                    ("state", kv_str(&s.state)),
                    ("maxrounds", Kv::Int(3)),
                    (
                        "MissionInfo",
                        section(vec![("Version", kv_str("1")), ("InfectedOnly", Kv::Int(0)), ("SurvivorSet", Kv::Int(2))]),
                    ),
                    ("dlcrequired", Kv::U64(0)),
                ]),
            ),
            ("Options", section(vec![("server", kv_str("listen"))])),
            ("Members", self.members()),
        ])
    }

    /// A RequestJoinData (connectionless) -> our ReplyJoinData, adding the
    /// joiner. None when it is not a join request or the game is full.
    pub fn join(&mut self, request: &Message) -> Option<Kv> {
        if request.name()? != "SysSession::RequestJoinData" {
            return None;
        }
        let body = request.body()?;
        let id = body.u64(&["id"])?;
        let machine = body.get(&["Settings", "Members", "machine0"])?.clone();
        let known = self.machines.iter().any(|m| m.u64(&["id"]) == Some(id));
        if !known {
            if 1 + self.machines.len() as i32 >= self.settings.slots {
                return None;
            }
            self.machines.push(machine);
        }
        self.settings.players = 1 + self.machines.len() as i32;
        Some(section(vec![(
            "SysSession::ReplyJoinData",
            section(vec![("id", Kv::U64(id)), ("Settings", self.settings_kv())]),
        )]))
    }

    /// OnPlayerUpdated for a member (ready / avatar changes).
    pub fn player_updated(&self, xuid: u64, name: &str, game: Vec<(&str, Kv)>) -> Kv {
        section(vec![(
            "SysSession::OnPlayerUpdated",
            section(vec![("game", section(game)), ("xuid", Kv::U64(xuid)), ("name", kv_str(name))]),
        )])
    }

    /// A member's `SysSession::Command` (Game::Ready / Game::Avatar) -> the
    /// OnPlayerUpdated every machine should see.
    pub fn command(&mut self, kv: &Kv) -> Option<Kv> {
        let Kv::Section(c) = kv.get(&["SysSession::Command"])? else { return None };
        let (what, body) = c.first()?;
        let xuid = body.u64(&["xuid"])?;
        let name = self
            .machines
            .iter()
            .find(|m| m.u64(&["id"]) == Some(xuid))
            .and_then(|m| m.str(&["player0", "name"]))
            .unwrap_or("")
            .to_string();
        match what.as_str() {
            "Game::Ready" => Some(self.player_updated(xuid, &name, vec![("ready", kv_str(body.str(&["ready"])?))])),
            "Game::Avatar" => {
                let avatar = body.str(&["Avatar"])?.to_ascii_lowercase();
                Some(self.player_updated(xuid, &name, vec![("Avatar", Kv::Str(avatar))]))
            }
            _ => None,
        }
    }

    /// START: lock the lobby and hand out the game reservation.
    pub fn start(&self) -> Kv {
        section(vec![(
            "SysSession::OnUpdate",
            section(vec![(
                "update",
                section(vec![
                    ("System", section(vec![("lock", kv_str(""))])),
                    ("server", section(vec![("server", kv_str("listen")), ("reservationid", Kv::U64(self.nonce))])),
                ]),
            )]),
        )])
    }

    /// The game is running on this campaign/chapter.
    pub fn in_game(&mut self) -> Kv {
        self.settings.state = "game".into();
        let s = &self.settings;
        section(vec![(
            "SysSession::OnUpdate",
            section(vec![(
                "update",
                section(vec![(
                    "game",
                    section(vec![
                        ("state", kv_str("game")),
                        ("campaign", kv_str(&s.campaign)),
                        ("chapter", Kv::Int(s.chapter)),
                        ("MissionInfo", section(vec![("Version", kv_str("1")), ("InfectedOnly", Kv::Int(0))])),
                    ]),
                )]),
            )]),
        )])
    }
}

// ------------------------------------------------ step 4: game server ---

/// A client's `k` connect packet (payload after 0xffffffff).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    pub protocol: u32,
    pub auth_mode: u32,
    pub challenge: u32,
    pub name: String,
    pub cvars: Vec<(String, String)>,
    pub cookie: u64,
}

/// Server `A` challenge reply to `q connect0x00000000` (360 layout).
pub fn challenge_packet(challenge: u32, echo: &str, cookie: u64) -> Vec<u8> {
    let mut v = vec![0xff; 4];
    v.push(b'A');
    v.extend_from_slice(&challenge.to_le_bytes());
    v.extend_from_slice(&2u32.to_le_bytes()); // auth mode
    v.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    v.extend_from_slice(echo.as_bytes());
    v.push(0);
    v.extend_from_slice(&0u32.to_le_bytes()); // protocol
    v.push(0); // empty string
    v.push(0); // no password
    v.extend_from_slice(&cookie.to_le_bytes());
    v
}

/// Server `B` accept (360 layout).
pub fn accept_packet() -> Vec<u8> {
    let mut v = vec![0xff; 4];
    v.extend_from_slice(b"B00000000000000\0");
    v
}

/// The echo string of a `q` query ("connect0x00000000").
pub fn parse_query(d: &[u8]) -> Option<String> {
    let rest = d.strip_prefix(b"q")?;
    let nul = rest.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&rest[..nul]).into_owned())
}

/// Parse a `k` connect packet (inverse of game::connect_packet).
pub fn parse_connect(d: &[u8]) -> Option<Connect> {
    let mut r = crate::bits::BitReader::new(d);
    if r.u(8)? != b'k' as u64 {
        return None;
    }
    let protocol = r.u32(32)?;
    let auth_mode = r.u32(32)?;
    let challenge = r.u32(32)?;
    let name = r.string(256)?;
    let _password = r.string(256)?;
    let _players = r.u(8)?;
    if r.u(6)? != 16 {
        return None; // clc_SplitPlayerConnect
    }
    let n = r.u(8)? as usize;
    let mut cvars = Vec::with_capacity(n);
    for _ in 0..n {
        cvars.push((r.string(256)?, r.string(256)?));
    }
    r.bit()?;
    let cookie = r.u(64)?;
    Some(Connect { protocol, auth_mode, challenge, name, cvars, cookie })
}

// ------------------------------------------------------- sign-on server ---

/// Client SignonState messages in a block: (state, spawn count). A client's
/// SignonState carries no player list or map (96 zero bits), which makes the
/// pattern distinctive enough to find without parsing every clc message.
pub fn client_signon_states(data: &[u8]) -> Vec<(u8, u32)> {
    use crate::bits::BitReader;
    let total = data.len() * 8;
    let mut out = Vec::new();
    let mut p = 0;
    while p + 6 + 8 + 32 + 96 <= total {
        let mut r = BitReader::range(data, p, total);
        let hit = (|| {
            if r.u(6)? != crate::game::id::SIGNON_STATE {
                return None;
            }
            let state = r.u(8)? as u8;
            let spawn = r.u32(32)?;
            let rest = (r.u(32)?, r.u(32)?, r.u(32)?);
            ((2..=6).contains(&state) && rest == (0, 0, 0)).then_some((state, spawn))
        })();
        if let Some(h) = hit {
            out.push(h);
            p += 6 + 8 + 32 + 96;
        } else {
            p += 1;
        }
    }
    out
}

/// A sign-on recorded from a 360 host on the same map, split at each
/// server SignonState: [..=NEW(3)], [..=PRESPAWN(4)], [..=SPAWN(5)],
/// [.. first full snapshot]. Kept private (it is game content).
#[derive(Debug, Clone, Default)]
pub struct SignonTemplate {
    pub stages: [Vec<crate::svc::Svc>; 4],
}

impl SignonTemplate {
    /// From the host's reliable blocks in order (as a client received them).
    pub fn from_blocks(blocks: &[Vec<u8>]) -> Option<Self> {
        use crate::svc::{split, Svc};
        let mut t = SignonTemplate::default();
        let mut stage = 0;
        for b in blocks {
            let Some(msgs) = split(b, 0, b.len() * 8) else { continue };
            for m in msgs {
                if stage > 3 {
                    break;
                }
                let full_snapshot = matches!(m, Svc::PacketEntities { delta_from: None, .. });
                let signon = matches!(m, Svc::SignonState { .. });
                if !matches!(m, Svc::Raw { kind: 0, .. }) {
                    t.stages[stage].push(m);
                }
                if signon && stage < 3 {
                    stage += 1;
                } else if full_snapshot && stage == 3 {
                    stage = 4;
                }
            }
        }
        (stage == 4).then_some(t)
    }

    /// Replace the host name shown to the client (ServerInfo and the Print banner stay otherwise as recorded).
    pub fn set_host_name(&mut self, name: &str) {
        for m in self.stages.iter_mut().flatten() {
            if let crate::svc::Svc::ServerInfo(i) = m {
                i.host_name = name.as_bytes().to_vec();
            }
        }
    }
}

/// Drives one client through sign-on: each client SignonState releases the
/// next template stage.
#[derive(Debug, Clone)]
pub struct ServerSignon {
    pub template: SignonTemplate,
    /// Stages sent so far (0..=4).
    pub sent: usize,
    pub full: bool,
}

impl ServerSignon {
    pub fn new(template: SignonTemplate) -> Self {
        ServerSignon { template, sent: 0, full: false }
    }

    /// A reliable block from the client -> reliable blocks to send back.
    pub fn handle(&mut self, block: &[u8]) -> Vec<crate::bits::BitWriter> {
        let mut out = Vec::new();
        for (state, _) in client_signon_states(block) {
            let want = match state {
                crate::game::SIGNON_CONNECTED => 1,
                crate::game::SIGNON_NEW => 2,
                crate::game::SIGNON_PRESPAWN => 3,
                crate::game::SIGNON_SPAWN => 4,
                crate::game::SIGNON_FULL => {
                    self.full = true;
                    continue;
                }
                _ => continue,
            };
            while self.sent < want {
                let mut w = crate::bits::BitWriter::new();
                for m in &self.template.stages[self.sent] {
                    crate::svc::write(&mut w, m);
                }
                out.push(w);
                self.sent += 1;
            }
        }
        out
    }
}

// ------------------------------------------------------ server game loop ---

/// Per-client game loop of a host: snapshots as deltas from the frame the
/// client last acknowledged (net_Tick), and the client's new usercmds.
#[derive(Debug, Clone)]
pub struct ServerGame {
    pub tick: u32,
    /// Recent frames we sent, by tick.
    frames: std::collections::VecDeque<(u32, std::collections::HashMap<u32, crate::entities::Entity>)>,
    /// Newest snapshot tick the client holds.
    pub acked: Option<u32>,
    /// Newest usercmd number we have run.
    pub last_command: u32,
    /// PacketEntities max-entries field (as the 360 host sends it).
    pub max_entries: u16,
}

const SERVER_FRAMES: usize = 64;

impl ServerGame {
    pub fn new(start_tick: u32, max_entries: u16) -> Self {
        ServerGame { tick: start_tick, frames: Default::default(), acked: None, last_command: 0, max_entries }
    }

    /// Client messages -> its new usercmds (oldest first); notes its ack.
    pub fn on_client(&mut self, msgs: &[crate::clc::Clc]) -> Vec<crate::game::UserCmd> {
        use crate::clc::Clc;
        let mut out = Vec::new();
        for m in msgs {
            match m {
                Clc::Tick { tick, .. } => {
                    if self.frames.iter().any(|(t, _)| t == tick) {
                        self.acked = Some(*tick);
                    }
                }
                Clc::Move { cmds, .. } => {
                    for c in cmds {
                        if c.command_number > self.last_command {
                            self.last_command = c.command_number;
                            out.push(*c);
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// One server tick: Tick + PacketEntities for the current world.
    pub fn snapshot(
        &mut self,
        schema: &crate::entities::Schema,
        world: &std::collections::HashMap<u32, crate::entities::Entity>,
    ) -> Option<Vec<crate::svc::Svc>> {
        use crate::svc::Svc;
        self.tick += 1;
        let base = self.acked.and_then(|t| self.frames.iter().find(|(ft, _)| *ft == t)).cloned();
        let (data, bits, updated, delta_from) = match &base {
            Some((t, from)) => {
                let (d, b, n) = crate::encode::delta_update(schema, from, world)?;
                (d, b, n, Some(*t))
            }
            None => {
                let list: Vec<(u32, &crate::entities::Entity)> = world.iter().map(|(i, e)| (*i, e)).collect();
                let (d, b, n) = crate::encode::full_update(schema, &list)?;
                (d, b, n, None)
            }
        };
        self.frames.push_back((self.tick, world.clone()));
        while self.frames.len() > SERVER_FRAMES {
            self.frames.pop_front();
        }
        Some(vec![
            Svc::Tick { tick: self.tick, frametime: 3333, deviation: 0 },
            Svc::PacketEntities {
                max_entries: self.max_entries,
                delta_from,
                baseline: false,
                updated: updated as u16,
                update_baseline: false,
                data,
                bits,
            },
        ])
    }
}

// ------------------------------------------------------------ host world ---

use crate::entities::{Value, World};

/// Set `name` on entity `id` (every flat prop with that name). False if absent.
pub fn set_prop(w: &mut World, id: u32, name: &str, v: Value) -> bool {
    let Some(e) = w.entities.get_mut(&id) else { return false };
    let Some(flat) = w.schema.flat(e.class) else { return false };
    let mut any = false;
    for (i, p) in flat.iter().enumerate() {
        if p.name == name {
            e.props.insert(i, v.clone());
            any = true;
        }
    }
    any
}

/// Set an entity's origin in both 360 encodings: the exact (NOSCALE) origin
/// and the cell origin (cell * 2^cellbits - 16384 + local).
pub fn set_origin(w: &mut World, id: u32, pos: [f32; 3]) -> bool {
    use crate::entities::flags::*;
    let cellbits = w.prop(id, "m_cellbits").and_then(|v| v.as_int()).unwrap_or(5) as i32;
    let width = (1i32 << cellbits) as f32;
    let cell = pos.map(|c| ((c + 16384.0) / width).floor());
    let local = [0, 1, 2].map(|i| pos[i] - (cell[i] * width - 16384.0));
    let Some(e) = w.entities.get_mut(&id) else { return false };
    let Some(flat) = w.schema.flat(e.class) else { return false };
    let has_cells = flat.iter().any(|p| p.name == "m_cellX");
    let mut any = false;
    for (i, p) in flat.iter().enumerate() {
        let cellular = p.flags & (CELL_COORD | CELL_COORD_LOWPRECISION | CELL_COORD_INTEGRAL) != 0;
        let src = if cellular && has_cells { local } else { pos };
        let v = match (p.name.as_str(), p.ty) {
            ("m_vecOrigin", crate::entities::PropType::VectorXY) => Value::VectorXY([src[0], src[1]]),
            ("m_vecOrigin", crate::entities::PropType::Vector) => Value::Vector(src),
            ("m_vecOrigin[2]", _) => Value::Float(src[2]),
            ("m_cellX", _) if has_cells => Value::Int(cell[0] as i64),
            ("m_cellY", _) if has_cells => Value::Int(cell[1] as i64),
            ("m_cellZ", _) if has_cells => Value::Int(cell[2] as i64),
            _ => continue,
        };
        e.props.insert(i, v);
        any = true;
    }
    any
}

/// The world a host serves: the recorded map world (from the sign-on
/// template's string tables and first full snapshot) that the game then
/// updates (survivors, infected, doors).
pub struct HostWorld {
    pub world: World,
    pub tables: crate::entities::StringTables,
}

impl HostWorld {
    pub fn from_template(schema_text: &str, t: &SignonTemplate, dictionary: Option<Vec<String>>) -> Option<Self> {
        use crate::svc::Svc;
        let mut world = World::new(crate::entities::Schema::from_text(schema_text).ok()?);
        let mut tables = crate::entities::StringTables::with_dictionary(dictionary);
        let mut tick = 0;
        for m in t.stages.iter().flatten() {
            match m {
                Svc::Tick { tick: tk, .. } => tick = *tk,
                Svc::CreateStringTable(c) => {
                    let name = String::from_utf8_lossy(&c.name).to_string();
                    let i = tables.create(&name, c.max_entries, c.entries, c.fixed.map(|f| f.1), c.compressed, &c.data, c.bits);
                    if name == "instancebaseline" {
                        let e: Vec<crate::entities::StringEntry> = tables.tables[i].entries.iter().enumerate()
                            .map(|(k, (n, d))| crate::entities::StringEntry { index: k as u32, name: n.clone(), data: d.clone() })
                            .collect();
                        world.set_baselines(&e);
                    }
                }
                Svc::UpdateStringTable { table, changed, data, bits } => {
                    let _ = tables.update(*table as u32, changed.unwrap_or(1) as u32, data, *bits);
                }
                Svc::PacketEntities { delta_from: None, updated, data, bits, .. } => {
                    world.apply(data, *bits, *updated as u32, None, tick).ok()?;
                }
                _ => {}
            }
        }
        (!world.entities.is_empty()).then_some(HostWorld { world, tables })
    }

    /// Make player slot `id` a survivor (L4D2 character 0 Nick, 1 Rochelle,
    /// 2 Coach, 3 Ellis), creating the entity from another player slot if the
    /// recorded world has none there (bots). False without a player to copy
    /// or a precached survivor model.
    pub fn ensure_survivor(&mut self, id: u32, character: i64) -> bool {
        let model = ["gambler", "producer", "coach", "mechanic"].get(character as usize).copied();
        let path = model.map(|m| format!("models/survivors/survivor_{m}.mdl"));
        let index = path.and_then(|p| {
            self.tables.get("modelprecache")?.entries.iter().position(|(n, _)| n.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&p)))
        });
        let Some(index) = index else { return false };
        if !self.world.entities.contains_key(&id) {
            let Some(template) = self.world.entities.iter()
                .find(|(i, _)| self.world.class_name(**i) == Some("CTerrorPlayer"))
                .map(|(_, e)| e.clone()) else { return false };
            self.world.entities.insert(id, crate::entities::Entity { serial: (id * 37) & 1023, ..template });
        }
        set_prop(&mut self.world, id, "m_iTeamNum", Value::Int(2));
        set_prop(&mut self.world, id, "m_survivorCharacter", Value::Int(character));
        set_prop(&mut self.world, id, "m_nModelIndex", Value::Int(index as i64));
        set_prop(&mut self.world, id, "m_iMaxHealth", Value::Int(100));
        set_prop(&mut self.world, id, "m_iHealth", Value::Int(100));
        set_prop(&mut self.world, id, "m_lifeState", Value::Int(0));
        true
    }

    /// Create entity `id` of server class `class` from its instance baseline.
    pub fn spawn(&mut self, id: u32, class: &str, serial: u32) -> bool {
        let Some(cid) = self.world.schema.classes.iter().position(|c| c.name == class) else { return false };
        let cid = cid as u32;
        let Some(flat) = self.world.schema.flat(cid) else { return false };
        let mut props = std::collections::HashMap::new();
        if let Some(b) = self.world.baselines.get(&cid) {
            if crate::entities::read_props(&mut crate::bits::BitReader::new(b), flat, &mut props).is_err() {
                return false;
            }
        }
        self.world.entities.insert(id, crate::entities::Entity { class: cid, serial: serial & 1023, props });
        true
    }

    /// Model precache index of `path` ("models/infected/common_male01.mdl").
    pub fn model_index(&self, path: &str) -> Option<usize> {
        self.tables.get("modelprecache")?.entries.iter().position(|(n, _)| n.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(path)))
    }

    /// A common infected (or Witch with class "Witch") at `origin` facing `yaw`.
    pub fn spawn_infected(&mut self, id: u32, class: &str, model: &str, origin: [f32; 3], yaw: f32) -> bool {
        let Some(index) = self.model_index(model) else { return false };
        if !self.spawn(id, class, id) {
            return false;
        }
        set_prop(&mut self.world, id, "m_nModelIndex", Value::Int(index as i64));
        set_origin(&mut self.world, id, origin);
        set_prop(&mut self.world, id, "m_angRotation[1]", Value::Float(yaw));
        true
    }

    /// Survivor player entities (CTerrorPlayer on team 2), lowest first.
    pub fn survivors(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.world.entities.keys().copied()
            .filter(|&id| self.world.class_name(id) == Some("CTerrorPlayer")
                && self.world.prop(id, "m_iTeamNum").and_then(|v| v.as_int()) == Some(2))
            .collect();
        v.sort_unstable();
        v
    }

    /// Drive one survivor from the game: position, view and health.
    pub fn set_survivor(&mut self, id: u32, origin: [f32; 3], pitch: f32, yaw: f32, health: i64) {
        set_origin(&mut self.world, id, origin);
        set_prop(&mut self.world, id, "m_angEyeAngles[0]", Value::Float(pitch));
        set_prop(&mut self.world, id, "m_angEyeAngles[1]", Value::Float(yaw));
        set_prop(&mut self.world, id, "m_iHealth", Value::Int(health));
        set_prop(&mut self.world, id, "m_lifeState", Value::Int(if health > 0 { 0 } else { 2 }));
    }
}

/// Sign-on template from a private relay recording (relay.jsonl lines with
/// "datagram", "direction", "sport" and "encrypted_hex"): the host's reliable
/// game-channel blocks in order.
pub fn template_from_relay(text: &str) -> Option<SignonTemplate> {
    let field = |l: &str, k: &str| -> Option<String> {
        let at = l.find(k)? + k.len();
        let rest = l[at..].trim_start_matches([' ', ':', '"']);
        Some(rest[..rest.find(['"', ','])?].to_string())
    };
    let unhex = |s: &str| -> Option<Vec<u8>> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
    };
    let mut chan = crate::netchan::Channel::game();
    let mut blocks = Vec::new();
    for l in text.lines().filter(|l| l.contains("\"datagram\"")) {
        let (Some(dir), Some(sport), Some(hex)) = (field(l, "\"direction\""), field(l, "\"sport\""), field(l, "\"encrypted_hex\"")) else { continue };
        if dir != "xbox->edge" || sport != "27015" {
            continue;
        }
        let Some(data) = unhex(&hex) else { continue };
        if data.starts_with(&[0xff; 4]) {
            continue;
        }
        if let crate::netchan::Incoming::Packet(r) = chan.receive(&data) {
            blocks.extend(r.reliable);
        }
        if blocks.len() > 400 {
            break;
        }
    }
    SignonTemplate::from_blocks(&blocks)
}

/// One infected the game wants the consoles to see (Source units, degrees).
#[derive(Debug, Clone, PartialEq)]
pub struct InfectedFeed {
    /// The game's own stable id for this infected.
    pub id: u64,
    /// Model path ("models/infected/common_male01.mdl").
    pub model: String,
    pub origin: [f32; 3],
    pub yaw: f32,
    pub witch: bool,
}

/// Keeps the host world's infected in step with the game's: new ones spawn,
/// existing ones move, missing ones are removed (the consoles then delete them).
#[derive(Debug, Default)]
pub struct HordeSync {
    /// Game id -> entity index.
    pub map: std::collections::HashMap<u64, u32>,
}

/// First entity index used for infected (above players, the map's entities stay below).
const INFECTED_FIRST: u32 = 1200;
const INFECTED_LAST: u32 = 2040;

impl HordeSync {
    pub fn apply(&mut self, hw: &mut HostWorld, list: &[InfectedFeed]) {
        let wanted: std::collections::HashSet<u64> = list.iter().map(|f| f.id).collect();
        self.map.retain(|id, e| {
            if wanted.contains(id) {
                true
            } else {
                hw.world.entities.remove(e);
                false
            }
        });
        for f in list {
            let entity = match self.map.get(&f.id) {
                Some(e) => *e,
                None => {
                    let Some(free) = (INFECTED_FIRST..=INFECTED_LAST).find(|i| !hw.world.entities.contains_key(i)) else { continue };
                    let class = if f.witch { "Witch" } else { "Infected" };
                    if !hw.spawn_infected(free, class, &f.model, f.origin, f.yaw) {
                        continue;
                    }
                    self.map.insert(f.id, free);
                    free
                }
            };
            set_origin(&mut hw.world, entity, f.origin);
            set_prop(&mut hw.world, entity, "m_angRotation[1]", Value::Float(f.yaw.rem_euclid(360.0)));
        }
    }
}

/// String-table entries (no dictionary, no substrings): the inverse of
/// entities::parse_entries_dict for what a host needs to send.
pub fn write_entries(max_entries: u32, entries: &[(u32, Option<&str>, Option<&[u8]>)]) -> crate::bits::BitWriter {
    let mut w = crate::bits::BitWriter::new();
    w.bit(false); // no dictionary
    let index_bits = (31 - max_entries.max(1).leading_zeros()) as usize;
    let mut last: i64 = -1;
    for (index, name, data) in entries {
        if *index as i64 == last + 1 {
            w.bit(true);
        } else {
            w.bit(false);
            w.u(index_bits, *index as u64);
        }
        last = *index as i64;
        w.bit(name.is_some());
        if let Some(n) = name {
            w.bit(false); // not a substring of an earlier entry
            w.string(n);
        }
        w.bit(data.is_some());
        if let Some(d) = data {
            w.u(14, d.len() as u64);
            w.bytes(d);
        }
    }
    w
}

impl HostWorld {
    /// Player names for slots: updates our `userinfo` table and returns the
    /// UpdateStringTable to send (the recorded entries keep their other fields).
    pub fn set_names(&mut self, players: &[(u32, u64, &str)]) -> Option<crate::svc::Svc> {
        let table = self.tables.tables.iter().position(|t| t.name == "userinfo")?;
        let max = self.tables.tables[table].max_entries;
        let sample = self.tables.tables[table].entries.iter().find_map(|(_, d)| d.clone().filter(|d| d.len() >= 77))?;
        let mut entries: Vec<(u32, String, Vec<u8>)> = Vec::new();
        for &(slot, xuid, name) in players {
            let index = slot.checked_sub(1)?;
            let t = &self.tables.tables[table];
            let mut data = t.entries.get(index as usize).and_then(|e| e.1.clone()).filter(|d| d.len() >= 77).unwrap_or_else(|| sample.clone());
            data[0..8].copy_from_slice(&xuid.to_be_bytes());
            let mut n = [0u8; 32];
            let b = name.as_bytes();
            n[..b.len().min(31)].copy_from_slice(&b[..b.len().min(31)]);
            data[8..40].copy_from_slice(&n);
            data[40..44].copy_from_slice(&(slot as i32 + 1).to_be_bytes());
            let key = t.entries.get(index as usize).and_then(|e| e.0.clone()).unwrap_or_else(|| index.to_string());
            entries.push((index, key, data));
        }
        entries.sort_by_key(|e| e.0);
        let list: Vec<(u32, Option<&str>, Option<&[u8]>)> =
            entries.iter().map(|(i, k, d)| (*i, Some(k.as_str()), Some(d.as_slice()))).collect();
        let w = write_entries(max, &list);
        let _ = self.tables.update(table as u32, entries.len() as u32, &w.data, w.bits);
        Some(crate::svc::Svc::UpdateStringTable {
            table: table as u8,
            changed: (entries.len() != 1).then_some(entries.len() as u16),
            data: w.data.clone(),
            bits: w.bits,
        })
    }
}

/// One door as the game has it (Source units / degrees; state as Source
/// DoorState: 0 closed, 1 opening, 2 open, 3 closing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DoorFeed {
    pub origin: [f32; 3],
    pub yaw: f32,
    pub state: i64,
}

/// Matches the game's doors to the host world's 360 prop doors by position
/// (once) and keeps their state and swing angle in step.
#[derive(Debug, Default)]
pub struct DoorSync {
    /// Game door origin (rounded) -> 360 entity (None: no door there).
    pub map: std::collections::HashMap<[i32; 3], Option<u32>>,
}

impl DoorSync {
    pub fn apply(&mut self, hw: &mut HostWorld, list: &[DoorFeed]) {
        for f in list {
            let key = f.origin.map(|c| c.round() as i32);
            let entity = *self.map.entry(key).or_insert_with(|| {
                hw.world
                    .entities
                    .keys()
                    .copied()
                    .filter(|&id| hw.world.class_name(id).is_some_and(|c| c.contains("Door")) && hw.world.prop(id, "m_eDoorState").is_some())
                    .filter_map(|id| {
                        let o = hw.world.origin(id)?;
                        let d = ((o[0] - f.origin[0]).powi(2) + (o[1] - f.origin[1]).powi(2) + (o[2] - f.origin[2]).powi(2)).sqrt();
                        (d < 24.0).then_some((id, d))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(id, _)| id)
            });
            let Some(entity) = entity else { continue };
            set_prop(&mut hw.world, entity, "m_eDoorState", Value::Int(f.state));
            let rot = match hw.world.prop(entity, "m_angRotation") {
                Some(Value::Vector(v)) => [v[0], f.yaw.rem_euclid(360.0), v[2]],
                _ => [0.0, f.yaw.rem_euclid(360.0), 0.0],
            };
            set_prop(&mut hw.world, entity, "m_angRotation", Value::Vector(rot));
        }
    }
}

impl HostWorld {
    /// Merge every map entity a recording ever shows (snapshots only carry
    /// what is near the player, so the sign-on snapshot misses most doors and
    /// props). Players, infected and carried weapons are left out.
    pub fn absorb_recording(&mut self, text: &str) -> usize {
        let field = |l: &str, k: &str| -> Option<String> {
            let at = l.find(k)? + k.len();
            let rest = l[at..].trim_start_matches([' ', ':', '"']);
            Some(rest[..rest.find(['"', ','])?].to_string())
        };
        let unhex = |s: &str| -> Option<Vec<u8>> {
            (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
        };
        let mut replay = World::new(self.world.schema.clone());
        replay.baselines = self.world.baselines.clone();
        let mut chan = crate::netchan::Channel::game();
        let (mut tick, mut full) = (0u32, false);
        let mut seen: std::collections::HashMap<u32, crate::entities::Entity> = Default::default();
        for l in text.lines().filter(|l| l.contains("\"datagram\"")) {
            let (Some(dir), Some(sport), Some(hex)) = (field(l, "\"direction\""), field(l, "\"sport\""), field(l, "\"encrypted_hex\"")) else { continue };
            if dir != "xbox->edge" || sport != "27015" {
                continue;
            }
            let Some(data) = unhex(&hex) else { continue };
            let crate::netchan::Incoming::Packet(r) = chan.receive(&data) else { continue };
            let mut msgs = Vec::new();
            for b in &r.reliable {
                msgs.extend(crate::game::walk(b, 0, b.len() * 8, false).0);
            }
            msgs.extend(crate::game::walk(&r.payload, r.unreliable.0, r.unreliable.1, false).0);
            for m in msgs {
                match m {
                    crate::game::Msg::Tick { tick: t } => tick = t,
                    crate::game::Msg::PacketEntities { delta_from, updated, data, bits, .. } => {
                        if delta_from.is_some() && !full {
                            continue;
                        }
                        if replay.apply(&data, bits, updated, delta_from, tick).is_ok() {
                            full = true;
                            for (id, e) in &replay.entities {
                                seen.insert(*id, e.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut added = 0;
        for (id, e) in seen {
            if self.world.entities.contains_key(&id) {
                continue;
            }
            let class = self.world.schema.classes.get(e.class as usize).map(|c| c.name.as_str()).unwrap_or("");
            let skip = matches!(class, "CTerrorPlayer" | "Infected" | "Witch" | "SurvivorBot")
                || class.starts_with("CTerrorPlayer");
            if skip {
                continue;
            }
            self.world.entities.insert(id, e);
            if crate::host::owned(&self.world, id) {
                self.world.entities.remove(&id);
                continue;
            }
            added += 1;
        }
        added
    }
}

/// True if a weapon-like entity is held by someone (m_hOwner set).
pub fn owned(w: &World, id: u32) -> bool {
    w.prop(id, "m_hOwner").and_then(|v| v.as_int()).and_then(crate::state::handle_index).is_some_and(|o| o != 0)
}

/// Entity indexes reserved for survivors' held weapons (one per slot).
const WEAPON_FIRST: u32 = 1100;

impl HostWorld {
    /// Server class of a rewrite weapon name ("weapon_smg" -> "CSubMachinegun").
    pub fn weapon_class(&self, weapon: &str) -> Option<String> {
        self.world
            .schema
            .classes
            .iter()
            .map(|c| c.name.as_str())
            .find(|c| crate::state::weapon_name(c) == weapon && self.world.schema.classes.iter().any(|x| x.name == *c))
            .map(str::to_string)
    }

    /// Survivor `slot` holds `weapon` (rewrite name): one owned weapon entity
    /// per slot, re-created when the weapon class changes.
    pub fn set_weapon(&mut self, slot: u32, weapon: &str) -> bool {
        let Some(class) = self.weapon_class(weapon) else { return false };
        let id = WEAPON_FIRST + slot;
        let player_serial = self.world.entities.get(&slot).map(|e| e.serial).unwrap_or(0);
        let same = self.world.class_name(id) == Some(class.as_str());
        if !same {
            self.world.entities.remove(&id);
            let serial = self.world.entities.get(&id).map(|e| e.serial + 1).unwrap_or(id * 7);
            if !self.spawn(id, &class, serial) {
                return false;
            }
        }
        let weapon_serial = self.world.entities[&id].serial as i64;
        let owner = slot as i64 | (player_serial as i64) << 11;
        set_prop(&mut self.world, id, "m_hOwner", Value::Int(owner));
        set_prop(&mut self.world, id, "m_hOwnerEntity", Value::Int(owner));
        let held = id as i64 | weapon_serial << 11;
        set_prop(&mut self.world, slot, "m_hActiveWeapon", Value::Int(held));
        if let Some(origin) = self.world.origin(slot) {
            set_origin(&mut self.world, id, origin);
        }
        true
    }
}
