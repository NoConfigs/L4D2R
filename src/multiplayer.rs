//! Multiplayer sessions, stage 1: a networked pre-game lobby and chat.
//!
//! One game hosts (creating a Public or Friends-Only lobby listens on the
//! Source default port 27015); others join with the console command
//! `connect <address>[:port]`, as in retail. The host is the lobby leader: it
//! owns the game settings, keeps the member list (name and survivor) and
//! relays chat. START THE GAME on the host sends every member the campaign,
//! chapter and difficulty to load.
//!
//! Gameplay is not shared yet (stage 2): after the start every player plays
//! the map locally with bots, but chat keeps working between them.
//!
//! Transport is Codex's `net` layer (framed TCP, polled per frame); messages
//! are JSON objects with a "t" type field. `SessionCore` holds all logic and
//! is driven without Bevy so it can be tested over localhost; the Bevy side
//! (`install`) connects it to the lobby, chat and console.
use crate::net::{self, NetError, Peer};
use serde_json::{json, Value};
use std::net::{SocketAddr, TcpListener};

/// Bumped when messages change; mismatched games refuse each other.
pub const PROTOCOL: u64 = 1;
/// Source's default game port.
pub const DEFAULT_PORT: u16 = 27015;
/// A lobby holds four survivors.
pub const MAX_MEMBERS: usize = 4;
/// A guest gives up when no welcome arrives in this time.
const JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub id: u32,
    pub name: String,
    /// Chosen survivor's name ("Nick"), if any.
    pub survivor: String,
    /// SteamID64 for the lobby avatar (PnlGamerPic); 0 when none.
    pub steam: u64,
}

/// Lobby settings chosen by the leader (indices into the campaign catalog).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Settings {
    pub campaign: usize,
    pub chapter: usize,
    /// Difficulty index (Easy, Normal, Advanced, Expert).
    pub difficulty: usize,
    /// A Survival map (campaign/chapter index the Survival map list).
    pub survival: bool,
}

/// What happened, for the game to act on.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Our connection or hosting state changed; the text is for the console
    /// and lobby ("Connected to ...", "Disconnected: ...").
    Status(String),
    /// The member list changed.
    Members,
    /// A chat line from someone: (scope, name, text). Scope is "lobby",
    /// "all" or "team".
    Chat(String, String, String),
    /// The leader changed the settings.
    Settings(Settings),
    /// The leader started the game with these settings.
    Start(Settings),
    /// A notice for the lobby chat ("Name joined.").
    Notice(String),
    /// A gameplay message from member `id` (net_play.rs).
    Game(u32, Value),
}

/// A connection to another game: direct TCP (LAN, phones, `connect
/// <address>`) or a Steam user reached through Steam's relays (steam.rs).
pub enum Link {
    Tcp(Peer),
    Steam(u64),
}
impl Link {
    fn queue(&mut self, payload: &[u8]) -> Result<(), NetError> {
        match self {
            Link::Tcp(p) => p.queue(payload),
            Link::Steam(id) => {
                if crate::steam::send(*id, payload) {
                    Ok(())
                } else {
                    Err(NetError::Closed)
                }
            }
        }
    }
    fn flush(&mut self) -> Result<(), NetError> {
        match self {
            Link::Tcp(p) => p.flush(),
            Link::Steam(_) => Ok(()),
        }
    }
    fn poll(&mut self) -> Result<Vec<Vec<u8>>, NetError> {
        match self {
            Link::Tcp(p) => p.poll(),
            Link::Steam(id) => crate::steam::take(*id).ok_or(NetError::Closed),
        }
    }
}
impl Drop for Link {
    fn drop(&mut self) {
        if let Link::Steam(id) = self {
            crate::steam::forget(*id);
        }
    }
}

struct Client {
    peer: Link,
    id: u32,
    /// Has sent a valid hello.
    ready: bool,
}

enum Role {
    Offline,
    Host { listener: TcpListener, clients: Vec<Client>, next_id: u32 },
    Client { peer: Link, welcomed: bool, since: std::time::Instant },
}

pub struct SessionCore {
    role: Role,
    pub members: Vec<Member>,
    /// Our id: 0 for the host.
    pub me: u32,
    pub settings: Settings,
    pub name: String,
    pub survivor: String,
    pub address: Option<SocketAddr>,
    /// Host: a game is running (`settings` is its current chapter), so a
    /// late joiner is sent straight into it.
    pub playing: bool,
}

impl SessionCore {
    pub fn new(name: &str, survivor: &str) -> Self {
        Self {
            role: Role::Offline,
            members: Vec::new(),
            me: 0,
            settings: Settings::default(),
            name: name.into(),
            survivor: survivor.into(),
            address: None,
            playing: false,
        }
    }
    pub fn is_host(&self) -> bool {
        matches!(self.role, Role::Host { .. })
    }
    pub fn is_client(&self) -> bool {
        matches!(self.role, Role::Client { .. })
    }
    pub fn online(&self) -> bool {
        !matches!(self.role, Role::Offline)
    }
    pub fn leader(&self) -> Option<&Member> {
        self.members.iter().find(|m| m.id == 0)
    }

    /// Starts hosting on `port` (0 picks one; used by tests).
    pub fn host(&mut self, port: u16) -> std::io::Result<SocketAddr> {
        self.close();
        // Another program may hold the port on loopback only (Apple's mobile
        // device service listens on 127.0.0.1:27015); local guests would
        // reach it instead of us, so treat the port as taken.
        if port != 0 {
            drop(TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))?);
        }
        let listener = net::listen(SocketAddr::from(([0, 0, 0, 0], port)))?;
        let addr = listener.local_addr()?;
        self.role = Role::Host { listener, clients: Vec::new(), next_id: 1 };
        crate::steam::set_accepting(true);
        self.me = 0;
        self.members = vec![Member {
            id: 0,
            name: self.name.clone(),
            survivor: self.survivor.clone(),
            steam: crate::steam::me().unwrap_or(0),
        }];
        self.address = Some(addr);
        Ok(addr)
    }

    /// Joins a host over an already-open connection (see `connect`).
    pub fn join(&mut self, mut peer: Link) {
        self.close();
        let hello = json!({
            "t": "hello",
            "v": PROTOCOL,
            "name": self.name,
            "survivor": self.survivor,
            "steam": crate::steam::me().unwrap_or(0),
        });
        let _ = peer.queue(hello.to_string().as_bytes());
        let _ = peer.flush();
        self.role = Role::Client { peer, welcomed: false, since: std::time::Instant::now() };
        self.members.clear();
    }

    /// Leaves the session (closes every connection). The others are told
    /// ("bye"), so a relayed Steam session ends at once instead of timing out.
    pub fn close(&mut self) {
        let bye = json!({"t": "bye"}).to_string();
        match &mut self.role {
            Role::Host { clients, .. } => {
                for c in clients.iter_mut() {
                    let _ = c.peer.queue(bye.as_bytes());
                    let _ = c.peer.flush();
                }
            }
            Role::Client { peer, .. } => {
                let _ = peer.queue(bye.as_bytes());
                let _ = peer.flush();
            }
            Role::Offline => {}
        }
        if self.is_host() {
            crate::steam::set_accepting(false);
        }
        self.role = Role::Offline;
        self.members.clear();
        self.address = None;
        self.me = 0;
    }

    fn members_json(&self) -> Value {
        Value::Array(
            self.members
                .iter()
                .map(|m| json!({"id": m.id, "name": m.name, "survivor": m.survivor, "steam": m.steam}))
                .collect(),
        )
    }
    fn settings_json(s: Settings) -> Value {
        json!({"campaign": s.campaign, "chapter": s.chapter, "difficulty": s.difficulty, "sv": s.survival})
    }
    fn parse_settings(v: &Value) -> Settings {
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
        Settings {
            campaign: n("campaign"),
            chapter: n("chapter"),
            difficulty: n("difficulty").min(3),
            survival: v.get("sv").and_then(Value::as_bool).unwrap_or(false),
        }
    }

    /// Host: sends `message` to every ready client.
    fn broadcast(&mut self, message: &Value) {
        if let Role::Host { clients, .. } = &mut self.role {
            let bytes = message.to_string();
            for c in clients.iter_mut().filter(|c| c.ready) {
                let _ = c.peer.queue(bytes.as_bytes());
            }
        }
    }
    /// Host: sends `message` to every ready client except `skip`.
    fn broadcast_except(&mut self, skip: u32, message: &Value) {
        if let Role::Host { clients, .. } = &mut self.role {
            let bytes = message.to_string();
            for c in clients.iter_mut().filter(|c| c.ready && c.id != skip) {
                let _ = c.peer.queue(bytes.as_bytes());
            }
        }
    }

    /// Sends a gameplay message to every other member (the host relays a
    /// guest's to the rest).
    pub fn game(&mut self, message: Value) {
        if self.is_host() {
            self.broadcast(&json!({"t": "g", "from": 0, "m": message}));
        } else {
            self.send_to_host(&json!({"t": "g", "m": message}));
        }
    }

    fn send_to_host(&mut self, message: &Value) {
        if let Role::Client { peer, .. } = &mut self.role {
            let _ = peer.queue(message.to_string().as_bytes());
        }
    }

    /// Sends a chat line ("lobby", "all" or "team"). The host relays it to
    /// everyone; our own copy is shown locally by the caller.
    pub fn chat(&mut self, scope: &str, text: &str) {
        let text: String = text.chars().take(127).collect();
        if self.is_host() {
            let line = json!({"t": "chat", "scope": scope, "name": self.name, "text": text, "from": 0});
            self.broadcast(&line);
        } else {
            self.send_to_host(&json!({"t": "chat", "scope": scope, "text": text}));
        }
    }

    /// Our survivor changed.
    pub fn set_survivor(&mut self, survivor: &str) {
        if self.survivor == survivor {
            return;
        }
        self.survivor = survivor.into();
        if self.is_host() {
            if let Some(m) = self.members.iter_mut().find(|m| m.id == 0) {
                m.survivor = survivor.into();
            }
            let members = json!({"t": "members", "members": self.members_json()});
            self.broadcast(&members);
        } else {
            self.send_to_host(&json!({"t": "survivor", "survivor": survivor}));
        }
    }

    /// Host: the lobby settings changed.
    pub fn set_settings(&mut self, settings: Settings) {
        if self.settings == settings {
            return;
        }
        self.settings = settings;
        if self.is_host() {
            self.broadcast(&json!({"t": "settings", "settings": Self::settings_json(settings)}));
        }
    }

    /// Host: START THE GAME for everyone.
    pub fn start(&mut self, settings: Settings) {
        self.settings = settings;
        if self.is_host() {
            self.broadcast(&json!({"t": "start", "settings": Self::settings_json(settings)}));
        }
    }

    /// Runs the session for one frame: accepts, reads, relays and sends.
    pub fn update(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        match &mut self.role {
            Role::Offline => {}
            Role::Host { .. } => self.update_host(&mut events),
            Role::Client { .. } => self.update_client(&mut events),
        }
        events
    }

    fn update_host(&mut self, events: &mut Vec<Event>) {
        let Role::Host { listener, clients, next_id } = &mut self.role else { return };
        while let Ok(Some(peer)) = net::accept(listener) {
            let id = *next_id;
            *next_id += 1;
            clients.push(Client { peer: Link::Tcp(peer), id, ready: false });
        }
        // Steam users who opened a session with us (relayed connections).
        for steam_id in crate::steam::take_new_peers() {
            if clients.iter().any(|c| matches!(c.peer, Link::Steam(s) if s == steam_id)) {
                continue;
            }
            let id = *next_id;
            *next_id += 1;
            clients.push(Client { peer: Link::Steam(steam_id), id, ready: false });
        }
        let mut incoming = Vec::new();
        let mut gone = Vec::new();
        for c in clients.iter_mut() {
            match c.peer.poll() {
                Ok(messages) => incoming.extend(messages.into_iter().map(|m| (c.id, m))),
                Err(_) => gone.push(c.id),
            }
        }
        for (id, bytes) in incoming {
            let Ok(message) = serde_json::from_slice::<Value>(&bytes) else { continue };
            self.host_message(id, &message, events);
        }
        for id in gone {
            self.drop_client(id, events);
        }
        if let Role::Host { clients, .. } = &mut self.role {
            let mut failed = Vec::new();
            for c in clients.iter_mut() {
                if c.peer.flush().is_err() {
                    failed.push(c.id);
                }
            }
            for id in failed {
                self.drop_client(id, events);
            }
        }
    }

    fn drop_client(&mut self, id: u32, events: &mut Vec<Event>) {
        let Role::Host { clients, .. } = &mut self.role else { return };
        let before = clients.len();
        clients.retain(|c| c.id != id);
        if clients.len() == before {
            return;
        }
        if let Some(i) = self.members.iter().position(|m| m.id == id) {
            let m = self.members.remove(i);
            let notice = format!("{} left.", m.name);
            self.broadcast(&json!({"t": "notice", "text": notice}));
            let members = json!({"t": "members", "members": self.members_json()});
            self.broadcast(&members);
            events.push(Event::Notice(notice));
            events.push(Event::Members);
        }
    }

    fn host_message(&mut self, id: u32, message: &Value, events: &mut Vec<Event>) {
        let kind = message.get("t").and_then(Value::as_str).unwrap_or("");
        let ready = matches!(&self.role, Role::Host { clients, .. } if clients.iter().any(|c| c.id == id && c.ready));
        match kind {
            "hello" if !ready => {
                let version = message.get("v").and_then(Value::as_u64).unwrap_or(0);
                let full = self.members.len() >= MAX_MEMBERS;
                let refuse = if version != PROTOCOL {
                    Some(format!("Version mismatch (host {PROTOCOL}, you {version})."))
                } else if full {
                    Some("The lobby is full.".to_string())
                } else {
                    None
                };
                if let Role::Host { clients, .. } = &mut self.role {
                    if let Some(c) = clients.iter_mut().find(|c| c.id == id) {
                        if let Some(reason) = refuse {
                            let _ = c.peer.queue(json!({"t": "refused", "reason": reason}).to_string().as_bytes());
                            let _ = c.peer.flush();
                            clients.retain(|c| c.id != id);
                            return;
                        }
                        c.ready = true;
                    }
                }
                let name: String = message
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Player")
                    .chars()
                    .take(32)
                    .collect();
                let survivor = message.get("survivor").and_then(Value::as_str).unwrap_or("").to_string();
                let steam = message.get("steam").and_then(Value::as_u64).unwrap_or(0);
                self.members.push(Member { id, name: name.clone(), survivor, steam });
                let welcome = json!({
                    "t": "welcome",
                    "id": id,
                    "members": self.members_json(),
                    "settings": Self::settings_json(self.settings),
                });
                // Joining a game already under way: straight into its chapter.
                let start = self.playing.then(|| json!({"t": "start", "settings": Self::settings_json(self.settings)}));
                if let Role::Host { clients, .. } = &mut self.role {
                    if let Some(c) = clients.iter_mut().find(|c| c.id == id) {
                        let _ = c.peer.queue(welcome.to_string().as_bytes());
                        if let Some(start) = &start {
                            let _ = c.peer.queue(start.to_string().as_bytes());
                        }
                    }
                }
                let notice = format!("{name} joined.");
                self.broadcast(&json!({"t": "notice", "text": notice}));
                let members = json!({"t": "members", "members": self.members_json()});
                self.broadcast(&members);
                events.push(Event::Notice(notice));
                events.push(Event::Members);
            }
            "chat" if ready => {
                let scope = message.get("scope").and_then(Value::as_str).unwrap_or("all").to_string();
                let text: String = message.get("text").and_then(Value::as_str).unwrap_or("").chars().take(127).collect();
                let Some(name) = self.members.iter().find(|m| m.id == id).map(|m| m.name.clone()) else { return };
                if text.trim().is_empty() {
                    return;
                }
                self.broadcast(&json!({"t": "chat", "scope": scope, "name": name, "text": text, "from": id}));
                events.push(Event::Chat(scope, name, text));
            }
            "bye" => {
                let mut gone = Vec::new();
                self.drop_client(id, &mut gone);
                events.extend(gone);
            }
            "g" if ready => {
                let m = message.get("m").cloned().unwrap_or(Value::Null);
                self.broadcast_except(id, &json!({"t": "g", "from": id, "m": m}));
                events.push(Event::Game(id, m));
            }
            "survivor" if ready => {
                let survivor = message.get("survivor").and_then(Value::as_str).unwrap_or("").to_string();
                if let Some(m) = self.members.iter_mut().find(|m| m.id == id) {
                    m.survivor = survivor;
                }
                let members = json!({"t": "members", "members": self.members_json()});
                self.broadcast(&members);
                events.push(Event::Members);
            }
            _ => {}
        }
    }

    fn update_client(&mut self, events: &mut Vec<Event>) {
        let Role::Client { peer, welcomed, since } = &mut self.role else { return };
        if !*welcomed && since.elapsed() > JOIN_TIMEOUT {
            self.close();
            events.push(Event::Status("Disconnected: no lobby answered at that address.".into()));
            return;
        }
        let result = peer.poll();
        let flushed = peer.flush();
        let messages = match result {
            Ok(m) => m,
            Err(e) => {
                let reason = match e {
                    NetError::Closed => "the host closed the lobby".to_string(),
                    NetError::Io(e) => e.to_string(),
                    NetError::Frame(e) => format!("{e:?}"),
                };
                self.close();
                events.push(Event::Status(format!("Disconnected: {reason}.")));
                return;
            }
        };
        if flushed.is_err() {
            self.close();
            events.push(Event::Status("Disconnected: connection lost.".into()));
            return;
        }
        for bytes in messages {
            let Ok(message) = serde_json::from_slice::<Value>(&bytes) else { continue };
            self.client_message(&message, events);
            if !self.online() {
                return;
            }
        }
    }

    fn client_message(&mut self, message: &Value, events: &mut Vec<Event>) {
        let kind = message.get("t").and_then(Value::as_str).unwrap_or("");
        let text = |k: &str| message.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match kind {
            "welcome" => {
                self.me = message.get("id").and_then(Value::as_u64).unwrap_or(0) as u32;
                if let Role::Client { welcomed, .. } = &mut self.role {
                    *welcomed = true;
                }
                self.read_members(message.get("members"));
                let settings = Self::parse_settings(message.get("settings").unwrap_or(&Value::Null));
                self.settings = settings;
                let host = self.leader().map(|m| m.name.clone()).unwrap_or_default();
                events.push(Event::Status(format!("Joined {host}'s lobby.")));
                events.push(Event::Settings(settings));
                events.push(Event::Members);
            }
            "bye" => {
                self.close();
                events.push(Event::Status("Disconnected: the host closed the lobby.".into()));
            }
            "refused" => {
                let reason = text("reason");
                self.close();
                events.push(Event::Status(format!("Refused: {reason}")));
            }
            "members" => {
                self.read_members(message.get("members"));
                events.push(Event::Members);
            }
            "notice" => events.push(Event::Notice(text("text"))),
            "chat" => {
                // Our own lines come back from the host; they were shown when sent.
                if message.get("from").and_then(Value::as_u64) == Some(self.me as u64) {
                    return;
                }
                events.push(Event::Chat(text("scope"), text("name"), text("text")));
            }
            "settings" => {
                let s = Self::parse_settings(message.get("settings").unwrap_or(&Value::Null));
                self.settings = s;
                events.push(Event::Settings(s));
            }
            "start" => {
                let s = Self::parse_settings(message.get("settings").unwrap_or(&Value::Null));
                self.settings = s;
                events.push(Event::Start(s));
            }
            "g" => {
                let from = message.get("from").and_then(Value::as_u64).unwrap_or(0) as u32;
                events.push(Event::Game(from, message.get("m").cloned().unwrap_or(Value::Null)));
            }
            _ => {}
        }
    }

    fn read_members(&mut self, value: Option<&Value>) {
        let Some(Value::Array(list)) = value else { return };
        self.members = list
            .iter()
            .map(|m| Member {
                id: m.get("id").and_then(Value::as_u64).unwrap_or(0) as u32,
                name: m.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                survivor: m.get("survivor").and_then(Value::as_str).unwrap_or("").to_string(),
                steam: m.get("steam").and_then(Value::as_u64).unwrap_or(0),
            })
            .collect();
    }
}

/// Parses "host", "host:port", "1.2.3.4", "1.2.3.4:27016"; the default port
/// is 27015. Host names are resolved.
pub fn parse_address(text: &str) -> Option<SocketAddr> {
    use std::net::ToSocketAddrs;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let with_port = if text.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        text.to_string()
    } else {
        format!("{text}:{DEFAULT_PORT}")
    };
    with_port.to_socket_addrs().ok()?.next()
}

/// This machine's LAN address (for "tell others to connect to ..."). A UDP
/// socket "connected" to a public address picks the outgoing interface; no
/// packet is sent.
pub fn local_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Runs both sessions until `done` holds or two seconds pass.
    fn pump(a: &mut SessionCore, b: &mut SessionCore, done: impl Fn(&[Event], &[Event]) -> bool) -> (Vec<Event>, Vec<Event>) {
        let deadline = Instant::now() + Duration::from_secs(2);
        let (mut ea, mut eb) = (Vec::new(), Vec::new());
        while Instant::now() < deadline {
            ea.extend(a.update());
            eb.extend(b.update());
            if done(&ea, &eb) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        (ea, eb)
    }

    #[test]
    fn joining_a_game_in_progress_starts_it_at_once() {
        let mut host = SessionCore::new("Host", "Nick");
        let addr = host.host(0).unwrap();
        host.playing = true;
        host.settings = Settings { campaign: 1, chapter: 2, difficulty: 3, survival: true };
        let mut client = SessionCore::new("Late", "Ellis");
        client.join(Link::Tcp(Peer::connect(SocketAddr::from(([127, 0, 0, 1], addr.port()))).unwrap()));
        let (_, events) = pump(&mut host, &mut client, |_, b| b.iter().any(|e| matches!(e, Event::Start(_))));
        assert!(
            events.iter().any(|e| matches!(e, Event::Start(s) if *s == Settings { campaign: 1, chapter: 2, difficulty: 3, survival: true })),
            "{events:?}"
        );
    }

    #[test]
    fn game_messages_reach_the_other_side() {
        let (mut host, mut client) = joined("Host", "Guest");
        client.game(json!({"k": "p", "x": 1}));
        let (events, _) = pump(&mut host, &mut client, |a, _| a.iter().any(|e| matches!(e, Event::Game(..))));
        assert!(events.iter().any(|e| matches!(e, Event::Game(id, m) if *id == 1 && m["x"] == 1)), "{events:?}");
        host.game(json!({"k": "p", "x": 2}));
        let (_, events) = pump(&mut host, &mut client, |_, b| b.iter().any(|e| matches!(e, Event::Game(..))));
        assert!(events.iter().any(|e| matches!(e, Event::Game(0, m) if m["x"] == 2)), "{events:?}");
    }

    fn joined(host_name: &str, client_name: &str) -> (SessionCore, SessionCore) {
        let mut host = SessionCore::new(host_name, "Nick");
        let addr = host.host(0).unwrap();
        let mut client = SessionCore::new(client_name, "Ellis");
        let peer = Peer::connect(SocketAddr::from(([127, 0, 0, 1], addr.port()))).unwrap();
        client.join(Link::Tcp(peer));
        let (_, events) = pump(&mut host, &mut client, |_, b| b.iter().any(|e| matches!(e, Event::Status(s) if s.starts_with("Joined"))));
        assert!(events.iter().any(|e| matches!(e, Event::Status(s) if s == &format!("Joined {host_name}'s lobby."))), "{events:?}");
        (host, client)
    }

    #[test]
    fn a_client_joins_sees_the_lobby_and_chats_both_ways() {
        let (mut host, mut client) = joined("Host", "Guest");
        assert_eq!(client.me, 1);
        assert_eq!(host.members.len(), 2);
        assert_eq!(client.members, host.members);
        assert_eq!(client.leader().unwrap().name, "Host");
        // Client -> host (relayed back to the client, which drops its own copy).
        client.chat("lobby", "hello");
        let (he, ce) = pump(&mut host, &mut client, |a, _| a.iter().any(|e| matches!(e, Event::Chat(..))));
        assert!(he.contains(&Event::Chat("lobby".into(), "Guest".into(), "hello".into())), "{he:?}");
        assert!(!ce.iter().any(|e| matches!(e, Event::Chat(..))));
        // Host -> client.
        host.chat("all", "go go go");
        let (_, ce) = pump(&mut host, &mut client, |_, b| b.iter().any(|e| matches!(e, Event::Chat(..))));
        assert!(ce.contains(&Event::Chat("all".into(), "Host".into(), "go go go".into())), "{ce:?}");
    }

    #[test]
    fn characters_settings_and_the_start_reach_every_member() {
        let (mut host, mut client) = joined("Host", "Guest");
        client.set_survivor("Coach");
        pump(&mut host, &mut client, |_, _| false);
        assert_eq!(host.members.iter().find(|m| m.id == 1).unwrap().survivor, "Coach");
        assert_eq!(client.members, host.members);
        let settings = Settings { campaign: 2, chapter: 1, difficulty: 3, survival: false };
        host.set_settings(settings);
        let (_, ce) = pump(&mut host, &mut client, |_, b| b.iter().any(|e| matches!(e, Event::Settings(_))));
        assert!(ce.contains(&Event::Settings(settings)));
        host.start(settings);
        let (_, ce) = pump(&mut host, &mut client, |_, b| b.iter().any(|e| matches!(e, Event::Start(_))));
        assert!(ce.contains(&Event::Start(settings)));
    }

    #[test]
    fn leaving_and_closing_are_reported() {
        let (mut host, mut client) = joined("Host", "Guest");
        client.close();
        let (he, _) = pump(&mut host, &mut client, |a, _| a.iter().any(|e| matches!(e, Event::Notice(_))));
        assert!(he.contains(&Event::Notice("Guest left.".into())), "{he:?}");
        assert_eq!(host.members.len(), 1);
        let (mut host, mut client) = joined("Host", "Guest");
        host.close();
        let (_, ce) = pump(&mut host, &mut client, |_, b| b.iter().any(|e| matches!(e, Event::Status(_))));
        assert!(ce.iter().any(|e| matches!(e, Event::Status(s) if s.starts_with("Disconnected"))), "{ce:?}");
        assert!(!client.online());
    }

    #[test]
    fn a_full_lobby_or_another_version_is_refused() {
        let mut host = SessionCore::new("Host", "Nick");
        let addr = host.host(0).unwrap();
        let target = SocketAddr::from(([127, 0, 0, 1], addr.port()));
        let mut guests = Vec::new();
        for i in 0..3 {
            let mut g = SessionCore::new(&format!("G{i}"), "");
            g.join(Link::Tcp(Peer::connect(target).unwrap()));
            guests.push(g);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while host.members.len() < 4 && Instant::now() < deadline {
            host.update();
            for g in &mut guests {
                g.update();
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(host.members.len(), 4);
        let mut late = SessionCore::new("Late", "");
        late.join(Link::Tcp(Peer::connect(target).unwrap()));
        let (_, le) = pump(&mut host, &mut late, |_, b| b.iter().any(|e| matches!(e, Event::Status(_))));
        assert!(le.contains(&Event::Status("Refused: The lobby is full.".into())), "{le:?}");
    }

    #[test]
    fn addresses_default_to_the_source_port() {
        assert_eq!(parse_address("127.0.0.1"), Some(SocketAddr::from(([127, 0, 0, 1], 27015))));
        assert_eq!(parse_address("127.0.0.1:27016"), Some(SocketAddr::from(([127, 0, 0, 1], 27016))));
        assert_eq!(parse_address(""), None);
    }
}

// ---------------------------------------------------------------------------
// Game side.

use bevy::prelude::*;
use std::sync::mpsc::Receiver;
use std::sync::Mutex;

/// Console requests (`connect`, `disconnect`) for the session system.
static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());
/// Last status line, for the `status` console command.
static STATUS: Mutex<String> = Mutex::new(String::new());

enum Request {
    Connect(String),
    Disconnect,
    Say(crate::game_chat::Mode, String),
    /// Join a Steam lobby (`connect_lobby <id>`, `+connect_lobby`).
    JoinLobby(u64),
}

/// Our Steam lobby while hosting or joined (0: none), for INVITE FRIENDS.
static STEAM_LOBBY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// INVITE FRIENDS: the Steam overlay's invite dialog for our lobby. False
/// without Steam or a lobby (the menu shows its FRIENDS page instead).
pub fn steam_invite() -> bool {
    let lobby = STEAM_LOBBY.load(std::sync::atomic::Ordering::Relaxed);
    if lobby == 0 {
        return false;
    }
    crate::steam::invite(lobby);
    true
}

/// The multiplayer session and an in-progress connect.
#[derive(Resource)]
pub struct Session {
    pub core: SessionCore,
    /// Gameplay messages received this frame, for net_play.rs.
    pub inbox: Vec<(u32, Value)>,
    /// Our Steam lobby and the access it was last given (0 public, 1
    /// friends, 2 private).
    steam_lobby: Option<u64>,
    steam_access: u8,
    connecting: Option<(String, Mutex<Receiver<std::io::Result<Peer>>>)>,
}
impl Default for Session {
    fn default() -> Self {
        Self { core: SessionCore::new(&crate::game_chat::local_name(), ""), inbox: Vec::new(), steam_lobby: None, steam_access: 0, connecting: None }
    }
}

fn set_status(text: &str) {
    println!("Multiplayer: {text}");
    *STATUS.lock().unwrap() = text.to_string();
}

/// Console commands: `connect <address>[:port]`, `disconnect`, `status`.
pub fn command(command: &str) -> Option<String> {
    let mut words = command.split_whitespace();
    let verb = words.next()?.to_ascii_lowercase();
    match verb.as_str() {
        "connect" => {
            let Some(address) = words.next() else {
                return Some("Usage: connect <address>[:port]  (default port 27015)".into());
            };
            REQUESTS.lock().unwrap().push(Request::Connect(address.to_string()));
            Some(format!("Connecting to {address}..."))
        }
        "connect_lobby" => {
            let Some(id) = words.next().and_then(|w| w.parse::<u64>().ok()) else {
                return Some("Usage: connect_lobby <Steam lobby id>".into());
            };
            if !crate::steam::available() {
                return Some("Steam is not running.".into());
            }
            REQUESTS.lock().unwrap().push(Request::JoinLobby(id));
            Some("Joining the Steam lobby...".into())
        }
        "disconnect" => {
            REQUESTS.lock().unwrap().push(Request::Disconnect);
            Some("Disconnecting.".into())
        }
        "say" | "say_team" => {
            let text = command.trim_start()[verb.len()..].trim().trim_matches('"').to_string();
            if text.is_empty() {
                return Some(format!("Usage: {verb} <text>"));
            }
            let mode = if verb == "say" { crate::game_chat::Mode::All } else { crate::game_chat::Mode::Team };
            REQUESTS.lock().unwrap().push(Request::Say(mode, text));
            Some(String::new())
        }
        "status" => {
            let status = STATUS.lock().unwrap().clone();
            Some(if status.is_empty() { "Not connected.".into() } else { status })
        }
        _ => None,
    }
}

fn settings_of(sel: &crate::campaign_settings::Selection) -> Settings {
    use crate::campaign_settings::Difficulty;
    Settings {
        campaign: sel.campaign,
        chapter: sel.chapter,
        difficulty: Difficulty::ALL.iter().position(|d| *d == sel.difficulty).unwrap_or(1),
        survival: crate::survival::active(),
    }
}

fn apply_settings(
    s: Settings,
    sel: &mut crate::campaign_settings::Selection,
    catalog: &crate::campaign_settings::Catalog,
) {
    use crate::campaign_settings::{Difficulty, Survivor};
    if catalog.campaigns.is_empty() {
        return;
    }
    let campaign = s.campaign.min(catalog.campaigns.len() - 1);
    sel.campaign = campaign;
    sel.chapter = s.chapter.min(catalog.campaigns[campaign].chapters.len().saturating_sub(1));
    sel.difficulty = Difficulty::ALL[s.difficulty.min(3)];
    let roster = Survivor::roster(catalog.campaigns[campaign].original);
    if !roster.contains(&sel.survivor) {
        sel.survivor = roster[0];
    }
}

fn notice(lobby: &mut crate::game_menu::lobby::Lobby, text: &str, now: f64) {
    lobby.chat.push(crate::game_chat::ChatLine {
        prefix: String::new(),
        name: String::new(),
        text: text.into(),
        team: crate::game_chat::Team::Survivor,
        notice: true,
        at: now,
    });
}

/// Runs the session each frame and keeps the lobby and chat in step with it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    mut session: ResMut<Session>,
    mut lobby: ResMut<crate::game_menu::lobby::Lobby>,
    mut chat: ResMut<crate::game_chat::ChatLog>,
    mut frontend: ResMut<crate::game_menu::Frontend>,
    mut sel: Option<ResMut<crate::campaign_settings::Selection>>,
    catalog: Option<Res<crate::campaign_settings::Catalog>>,
    current: Option<Res<crate::campaign_settings::SessionSettings>>,
    time: Res<Time<Real>>,
    // Steam's callbacks are pumped here; keep them on one (the main) thread.
    _main: bevy::ecs::system::NonSendMarker,
    mut launched: Local<bool>,
    mut tested: Local<bool>,
    mut advertised: Local<(u64, String)>,
) {
    use crate::game_chat::{compose, Mode, Team};
    use crate::game_menu::{lobby::Access, Page};
    let now = time.elapsed_secs_f64();
    let session = &mut *session;
    // Console requests.
    // `+connect <address>` on the command line joins at startup (retail).
    if !*launched {
        *launched = true;
        let args: Vec<String> = std::env::args().collect();
        if let Some(w) = args.windows(2).find(|w| w[0] == "+connect") {
            REQUESTS.lock().unwrap().push(Request::Connect(w[1].clone()));
        }
        // Steam passes this when a friend's invite or "Join Game" starts us.
        if let Some(id) = args.windows(2).find(|w| w[0] == "+connect_lobby").and_then(|w| w[1].parse().ok()) {
            REQUESTS.lock().unwrap().push(Request::JoinLobby(id));
        }
    }
    let requests: Vec<Request> = std::mem::take(&mut *REQUESTS.lock().unwrap());
    for request in requests {
        match request {
            Request::Connect(address) => {
                let Some(addr) = parse_address(&address) else {
                    set_status(&format!("Could not resolve {address}."));
                    continue;
                };
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(Peer::connect(addr));
                });
                session.connecting = Some((address, Mutex::new(rx)));
            }
            Request::JoinLobby(id) => {
                set_status("Joining the Steam lobby...");
                crate::steam::join_lobby(id);
            }
            Request::Say(mode, text) => {
                let me = crate::game_chat::local_name();
                chat.history.push(compose(mode, Team::Survivor, false, &me, &text, now));
                chat.outgoing.push((mode, text));
            }
            Request::Disconnect => {
                session.core.close();
                session.connecting = None;
                set_status("Disconnected.");
                if lobby.client {
                    crate::game_menu::lobby::leave_quiet(&mut lobby);
                    if frontend.page == Page::Lobby {
                        frontend.page = Page::Home;
                    }
                }
            }
        }
    }
    // Steam: invites, "Join Game", our lobby being created or entered.
    crate::steam::pump();
    let access_code = match lobby.access {
        Access::Public => 0u8,
        Access::Friends => 1,
        Access::Private => 2,
    };
    for event in crate::steam::events() {
        use crate::steam::SteamEvent;
        match event {
            SteamEvent::LobbyCreated(id) if session.core.is_host() => {
                session.steam_lobby = Some(id);
                session.steam_access = access_code;
                STEAM_LOBBY.store(id, std::sync::atomic::Ordering::Relaxed);
                if let Some(me) = crate::steam::me() {
                    crate::steam::set_lobby_data(id, "host", &me.to_string());
                }
                crate::steam::set_lobby_data(id, "name", &crate::game_chat::local_name());
                crate::steam::set_presence(Some(id));
                println!("Steam: lobby {id} created");
                notice(&mut lobby, "Steam lobby ready: invite friends with INVITE FRIENDS.", now);
            }
            SteamEvent::LobbyCreated(id) => crate::steam::leave_lobby(id),
            SteamEvent::LobbyFailed(e) => notice(&mut lobby, &format!("Could not create a Steam lobby: {e}"), now),
            SteamEvent::JoinRequested(id) => {
                set_status("Joining the Steam lobby...");
                crate::steam::join_lobby(id);
            }
            SteamEvent::LobbyEntered(id, owner) => {
                if Some(owner) == crate::steam::me() {
                    continue;
                }
                if let Some(sel) = sel.as_ref() {
                    session.core.survivor = sel.survivor.name().to_string();
                }
                session.core.name = crate::game_chat::local_name();
                session.core.join(Link::Steam(owner));
                session.steam_lobby = Some(id);
                STEAM_LOBBY.store(id, std::sync::atomic::Ordering::Relaxed);
                crate::steam::set_presence(Some(id));
                set_status("Connecting through Steam, joining the lobby...");
            }
            SteamEvent::LobbyJoinFailed => set_status("Could not join the Steam lobby (it may be full or closed)."),
        }
    }
    // A finished connect: join the host (its welcome opens the lobby).
    let finished = session.connecting.as_ref().and_then(|(address, rx)| rx.lock().unwrap().try_recv().ok().map(|r| (address.clone(), r)));
    if let Some((address, result)) = finished {
        session.connecting = None;
        match result {
            Ok(peer) => {
                if let Some(sel) = sel.as_ref() {
                    session.core.survivor = sel.survivor.name().to_string();
                }
                session.core.name = crate::game_chat::local_name();
                session.core.join(Link::Tcp(peer));
                set_status(&format!("Connected to {address}, joining the lobby..."));
            }
            Err(e) => set_status(&format!("Could not connect to {address}: {e}")),
        }
    }
    // Lobby signals from the menu.
    // With Steam every lobby hosts (Private is invite-only on Steam);
    // without it a Private lobby takes no one in.
    let steam = crate::steam::available();
    if std::mem::take(&mut lobby.created) && !session.core.online() && (steam || lobby.access != Access::Private) {
        if let Some(sel) = sel.as_ref() {
            session.core.survivor = sel.survivor.name().to_string();
            session.core.settings = settings_of(sel);
        }
        session.core.name = crate::game_chat::local_name();
        let hosted = (0..4).find_map(|i| session.core.host(DEFAULT_PORT + i).ok());
        match hosted {
            Some(addr) => {
                let ip = local_ip().map_or("this computer".to_string(), |ip| ip.to_string());
                lobby.address = if addr.port() == DEFAULT_PORT { ip } else { format!("{ip}:{}", addr.port()) };
                let text = format!("Join with: connect {}", lobby.address);
                set_status(&text);
                notice(&mut lobby, &text, now);
                if steam {
                    crate::steam::create_lobby(access_code);
                }
            }
            None => notice(&mut lobby, "Could not open a port for others to join.", now),
        }
    }
    if std::mem::take(&mut lobby.left) && session.core.online() {
        session.core.close();
        set_status("Left the lobby.");
    }
    // A Private lobby takes no one in.
    if !steam && session.core.is_host() && lobby.joined && lobby.access == Access::Private {
        session.core.close();
        lobby.address.clear();
        lobby.members.clear();
        notice(&mut lobby, "Private Game: hosting stopped.", now);
    }
    if session.core.online() {
        if let Some(sel) = sel.as_ref() {
            session.core.set_survivor(sel.survivor.name());
            if session.core.is_host() {
                // In a game: the chapter being played (late joiners go
                // there); in the lobby: the chosen one.
                let playing = frontend.started.then(|| current.as_ref().zip(catalog.as_ref())).flatten().and_then(|(c, cat)| {
                    let (campaign, chapter) = cat.campaigns.iter().enumerate().find_map(|(i, camp)| {
                        camp.chapters.iter().position(|ch| ch.map == c.map).map(|j| (i, j))
                    })?;
                    Some(Settings { campaign, chapter, ..settings_of(sel) })
                });
                session.core.playing = playing.is_some();
                session.core.set_settings(playing.unwrap_or_else(|| settings_of(sel)));
            }
        }
        if std::mem::take(&mut lobby.started) && session.core.is_host() {
            if let Some(sel) = sel.as_ref() {
                session.core.start(settings_of(sel));
            }
        }
        for text in std::mem::take(&mut lobby.outgoing) {
            session.core.chat("lobby", &text);
        }
        for (mode, text) in std::mem::take(&mut chat.outgoing) {
            session.core.chat(if mode == Mode::Team { "team" } else { "all" }, &text);
        }
    } else {
        lobby.outgoing.clear();
        chat.outgoing.clear();
        lobby.started = false;
    }
    for event in session.core.update() {
        match event {
            Event::Status(text) => {
                set_status(&text);
                if text.starts_with("Joined") {
                    lobby.joined = true;
                    lobby.client = true;
                    lobby.chat = Default::default();
                    frontend.page = Page::Lobby;
                }
                if !session.core.online() {
                    chat.history.push(crate::game_chat::ChatLine {
                        prefix: String::new(),
                        name: String::new(),
                        text: text.clone(),
                        team: Team::Survivor,
                        notice: true,
                        at: now,
                    });
                    if lobby.client {
                        crate::game_menu::lobby::leave_quiet(&mut lobby);
                        if frontend.page == Page::Lobby {
                            frontend.page = Page::Home;
                        }
                    }
                }
            }
            Event::Members => {}
            Event::Game(from, m) => session.inbox.push((from, m)),
            Event::Chat(scope, name, text) => {
                if scope == "lobby" {
                    lobby.chat.push(compose(Mode::All, Team::Survivor, false, &name, &text, now));
                } else {
                    let mode = if scope == "team" { Mode::Team } else { Mode::All };
                    chat.history.push(compose(mode, Team::Survivor, false, &name, &text, now));
                }
            }
            Event::Notice(text) => notice(&mut lobby, &text, now),
            Event::Settings(s) => {
                // The leader's mode: the Survival map list is swapped in
                // (survival::menu_mode) before the menu reads it.
                frontend.mode = crate::survival::lobby_mode(frontend.mode, s.survival);
                if let (Some(sel), Some(catalog)) = (sel.as_mut(), catalog.as_ref()) {
                    apply_settings(s, sel, catalog);
                }
            }
            Event::Start(s) => {
                frontend.mode = crate::survival::lobby_mode(frontend.mode, s.survival);
                if let (Some(sel), Some(catalog)) = (sel.as_mut(), catalog.as_ref()) {
                    apply_settings(s, sel, catalog);
                }
                lobby.pending_start = true;
            }
        }
    }
    // Test switches: `--capture-lobby-autostart` (host starts once someone
    // joins; `--capture-lobby-solo` starts at once), `--capture-lobby-say <text>` (a guest says a line on joining).
    let solo = std::env::args().any(|a| a == "--capture-lobby-solo");
    if !*tested && solo && lobby.joined && frontend.page == Page::Lobby {
        *tested = true;
        lobby.pending_start = true;
    }
    if !*tested && session.core.online() && session.core.members.len() >= 2 {
        *tested = true;
        let args: Vec<String> = std::env::args().collect();
        if session.core.is_host() && args.iter().any(|a| a == "--capture-lobby-autostart") {
            lobby.pending_start = true;
        }
        if let Some(w) = args.windows(2).find(|w| w[0] == "--capture-lobby-say") {
            session.core.chat("lobby", &w[1]);
            let me = crate::game_chat::local_name();
            lobby.chat.push(compose(Mode::All, Team::Survivor, false, &me, &w[1], now));
        }
    }
    // Steam lobby: follow the access setting; leave it when the session ends.
    if let Some(id) = session.steam_lobby {
        if !session.core.online() {
            crate::steam::leave_lobby(id);
            crate::steam::set_presence(None);
            session.steam_lobby = None;
            STEAM_LOBBY.store(0, std::sync::atomic::Ordering::Relaxed);
        } else if session.core.is_host() && lobby.joined && session.steam_access != access_code {
            session.steam_access = access_code;
            crate::steam::set_lobby_access(id, access_code);
        }
    }
    // JOIN A GAME (game_browser.rs): while hosting a Public or Friends-Only
    // game, answer LAN queries and describe the game in the Steam lobby.
    let advert = match (session.core.is_host() && lobby.access != Access::Private, sel.as_ref(), catalog.as_ref()) {
        (true, Some(sel), Some(catalog)) => {
            let st = session.core.settings;
            let campaign = catalog.campaigns.get(st.campaign);
            let chapter = campaign.and_then(|c| c.chapters.get(st.chapter));
            Some(crate::game_menu::browser::Advert {
                id: crate::game_menu::browser::host_id(),
                name: crate::game_chat::local_name(),
                campaign: campaign.map(|c| c.name.clone()).unwrap_or_default(),
                chapter: chapter.map(|c| c.name.clone()).unwrap_or_default(),
                image: chapter.map(|c| c.image.clone()).unwrap_or_default(),
                difficulty: sel.difficulty.name().to_string(),
                mode: if st.survival { "Survival" } else { "Campaign" }.to_string(),
                players: session.core.members.len(),
                max: MAX_MEMBERS,
                playing: session.core.playing,
                port: session.core.address.map_or(DEFAULT_PORT, |a| a.port()),
                access: access_code,
            })
        }
        _ => None,
    };
    if let (Some(advert), Some(id)) = (advert.as_ref(), session.steam_lobby) {
        let json = advert.to_json();
        if advertised.0 != id || advertised.1 != json {
            crate::steam::advertise(id, &json);
            *advertised = (id, json);
        }
    }
    *crate::game_menu::browser::ADVERT.lock().unwrap() = advert;
    // Members for the lobby screen, leader first.
    if session.core.online() {
        let mut members: Vec<&Member> = session.core.members.iter().collect();
        members.sort_by_key(|m| m.id);
        lobby.members = members.iter().map(|m| (m.name.clone(), m.survivor.clone(), m.steam)).collect();
        lobby.leader = session.core.leader().map(|m| m.name.clone()).unwrap_or_default();
        lobby.client = session.core.is_client();
        // Each player is a different survivor: whoever joined later moves
        // to the first free one.
        if let (Some(sel), Some(catalog)) = (sel.as_mut(), catalog.as_ref()) {
            let me = session.core.me;
            let clash = session.core.members.iter().any(|m| m.id < me && m.survivor == sel.survivor.name());
            if clash && sel.campaign < catalog.campaigns.len() {
                let roster = crate::campaign_settings::Survivor::roster(catalog.campaigns[sel.campaign].original);
                let taken = |s: &crate::campaign_settings::Survivor| {
                    session.core.members.iter().any(|m| m.id != me && m.survivor == s.name())
                };
                if let Some(free) = roster.iter().find(|s| !taken(s)) {
                    sel.survivor = *free;
                }
            }
        }
    } else if !lobby.members.is_empty() {
        lobby.members.clear();
        lobby.leader.clear();
        lobby.address.clear();
    }
}

pub fn install(app: &mut App) {
    // Start Steam now (main thread), so the Steam name is ready for the menus.
    crate::steam::available();
    app.init_resource::<Session>()
        .add_systems(Update, run.before(crate::game_menu::update).before(crate::game_menu::lobby::presentation))
        .add_systems(Update, crate::game_menu::lobby::avatars.after(run));
    crate::game_menu::browser::install(app);
}
