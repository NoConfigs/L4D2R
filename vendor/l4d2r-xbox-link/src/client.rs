//! Background Xbox 360 System Link client for the game: finds lobbies, joins
//! one, follows the host into the game, completes sign-on and keeps a decoded
//! `GameState` current. The game polls `Shared` and sends `Command`s; all
//! networking runs on the client's own thread.

use crate::bits::BitWriter;
use crate::entities::{dictionary_from_bsp, Schema, StringEntry, StringTables, World};
use crate::game::{self, Msg, Signon, UserCmd};
use crate::matchmaking::{lan_search, parse_game_details, request_join_data, Kv, Lobby, Message, Player};
use crate::netchan::{Channel, Incoming};
use crate::session::{Association, State};
use crate::state::{game_state, GameState};
use crate::sys::random_array;
use crate::xnet::{broadcast_keys, decode_broadcast, encode_broadcast, ethernet_frame, parse_frame, xex_lan_key, Broadcast, Packet,
    BROADCAST_MAC};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Config {
    /// The user's own L4D2 default.xex (title LAN key).
    pub xex: PathBuf,
    /// 16-byte platform key (from the emulator reference source).
    pub platform_key: [u8; 16],
    /// IPv4 address of the PC adapter on the console's network.
    pub local_address: String,
    /// SendTables recovered from the user's console (export_sendtables_text.py).
    pub sendtables: PathBuf,
    /// Folder with the 360 maps (`<map>.360.bsp`) for string table dictionaries.
    pub maps: PathBuf,
    pub name: String,
    /// Requested survivor avatar ("mechanic", "gambler", "producer", "coach").
    pub avatar: String,
    /// Our offline xuid. Keep it the same across launches: a new one each time
    /// makes the 360 lobby count a restarted game as a second player.
    pub xuid: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LobbyInfo {
    pub host: String,
    pub mode: String,
    pub difficulty: String,
    pub campaign: String,
    pub chapter: i64,
    pub state: String,
    pub players: i64,
    pub slots: i64,
    pub session_id: [u8; 8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Searching,
    Joining,
    InLobby,
    Connecting,
    SigningOn,
    InGame,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub name: String,
    pub xuid: u64,
    pub avatar: String,
    pub host: bool,
}

#[derive(Debug, Clone)]
pub struct Shared {
    pub phase: Phase,
    pub lobbies: Vec<LobbyInfo>,
    pub members: Vec<Member>,
    pub map: Option<String>,
    pub state: Option<GameState>,
    /// Our player entity in the host's game.
    pub our_entity: Option<u32>,
    pub xuid: u64,
    pub messages: Vec<String>,
    pub error: Option<String>,
    /// Bumped whenever `state` changes.
    pub state_serial: u64,
    /// The lobby we joined (mode, difficulty, map at join time).
    pub joined: Option<LobbyInfo>,
    /// The vote in progress on the host (or just decided), from its game events.
    pub vote: Option<Vote>,
    /// Voice received (speaker xuid, Speex bytes), for the game to decode and play.
    pub voice_in: Vec<(u64, Vec<u8>)>,
    /// Chat lines from the host (name, text, team only).
    pub chat_in: Vec<(String, String, bool)>,
    /// Shots fired per userinfo user id (weapon_fire events), for animation.
    pub shots: std::collections::HashMap<i32, u64>,
}

/// A call vote on the 360 host (vote_started / vote_changed / vote_passed / vote_failed).
#[derive(Debug, Clone, PartialEq)]
pub struct Vote {
    /// Localization token ("#L4D_vote_kick_player") and its parameter.
    pub issue: String,
    pub param: String,
    pub yes: i64,
    pub no: i64,
    pub potential: i64,
    /// Some(true) passed, Some(false) failed.
    pub result: Option<bool>,
    /// We already voted.
    pub voted: bool,
    /// When it was decided (the panel then fades).
    pub decided: Option<std::time::Instant>,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            phase: Phase::Idle,
            lobbies: Vec::new(),
            members: Vec::new(),
            map: None,
            state: None,
            our_entity: None,
            xuid: 0,
            messages: Vec::new(),
            error: None,
            state_serial: 0,
            joined: None,
            vote: None,
            voice_in: Vec::new(),
            chat_in: Vec::new(),
            shots: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    Search,
    Join(usize),
    Leave,
    /// Our player's input for the next commands.
    Input(UserCmd),
    /// Switch to this weapon entity (sent once, in the next command).
    SelectWeapon(u32),
    /// Answer the vote in progress (the retail client's "Vote Yes" / "Vote No").
    Vote(bool),
    /// Speex-encoded voice to send (clc_VoiceData).
    Voice(Vec<u8>),
    /// Say something in chat (team only when true).
    Say(bool, String),
    /// Pick our survivor in the lobby (360 avatar name, "Mechanic").
    Avatar(String),
    Stop,
}

pub struct XboxClient {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Command>,
}

impl XboxClient {
    #[cfg(windows)]
    pub fn start(config: Config) -> Result<XboxClient, String> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = channel();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("xbox-link".into())
            .spawn(move || {
                if let Err(e) = run(config, s.clone(), rx) {
                    let mut g = s.lock().unwrap();
                    g.phase = Phase::Failed;
                    g.error = Some(e);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(XboxClient { shared, tx })
    }
    #[cfg(not(windows))]
    pub fn start(_config: Config) -> Result<XboxClient, String> {
        Err("Xbox System Link needs Windows + Npcap".into())
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }
    pub fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap()
    }
}

impl Drop for XboxClient {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
    }
}

#[cfg(windows)]
pub(crate) fn adapter(local_ip: &str) -> Result<(String, [u8; 6]), String> {
    let script = format!(
        "$c = Get-NetIPAddress -AddressFamily IPv4 -IPAddress '{local_ip}' | Select-Object -First 1; \
         $a = Get-NetAdapter -InterfaceIndex $c.InterfaceIndex; $a.InterfaceGuid + ' ' + $a.MacAddress"
    );
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-Command", &script]);
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(format!("no network adapter owns {local_ip}"));
    }
    let mac: Vec<u8> = parts[1].split('-').filter_map(|h| u8::from_str_radix(h, 16).ok()).collect();
    Ok((format!(r"\Device\NPF_{}", parts[0]), mac.try_into().map_err(|_| "bad MAC")?))
}

/// Chat from svc_UserMessage: SayText2 (4: entity, chat flag, format token,
/// then params: name, text) or SayText (3: entity, text, chat flag). Returns
/// (name, text, team only).
fn chat_message(data: &[u8], bits: usize) -> Option<(String, String, bool)> {
    let mut r = crate::bits::BitReader::range(data, 0, bits);
    let kind = r.u(8)?;
    let len = r.u(11)? as usize;
    let end = r.pos + len;
    let mut r = crate::bits::BitReader::range(data, r.pos, end);
    match kind {
        4 => {
            r.u(8)?;
            r.u(8)?;
            let token = r.string(256)?;
            let name = r.string(256).unwrap_or_default();
            let text = r.string(256).unwrap_or_default();
            if text.trim().is_empty() {
                return None;
            }
            let team = token.to_ascii_lowercase().contains("team");
            Some((name, text, team))
        }
        3 => {
            r.u(8)?;
            let text = r.string(256)?;
            (!text.trim().is_empty()).then(|| (String::new(), text.trim().to_string(), false))
        }
        _ => None,
    }
}

/// Vote game events -> Shared::vote.
fn vote_event(s: &Arc<Mutex<Shared>>, name: &str, values: &[(String, crate::svc::EventValue)]) {
    let get = |k: &str| values.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let text = |k: &str| get(k).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let int = |k: &str| get(k).and_then(|v| v.as_int()).unwrap_or(0);
    let mut g = s.lock().unwrap();
    match name {
        "vote_started" => {
            g.vote = Some(Vote { issue: text("issue"), param: text("param1"), yes: 0, no: 0, potential: 0, result: None,
                voted: false, decided: None });
        }
        "vote_changed" => {
            if let Some(v) = g.vote.as_mut() {
                v.yes = int("yesVotes");
                v.no = int("noVotes");
                v.potential = int("potentialVotes");
            }
        }
        "vote_passed" | "vote_failed" => {
            if let Some(v) = g.vote.as_mut() {
                v.result = Some(name == "vote_passed");
                v.decided = Some(std::time::Instant::now());
            }
        }
        "vote_ended" => {
            if let Some(v) = g.vote.as_mut() {
                v.decided.get_or_insert_with(std::time::Instant::now);
            }
        }
        _ => {}
    }
}

fn note(s: &Arc<Mutex<Shared>>, text: impl Into<String>) {
    let text = text.into();
    println!("Xbox link: {text}");
    let mut g = s.lock().unwrap();
    g.messages.push(text);
    if g.messages.len() > 200 {
        g.messages.remove(0);
    }
}

fn lobby_info(l: &Lobby) -> LobbyInfo {
    LobbyInfo {
        host: l.host_name.clone(),
        mode: l.setting_str(&["game", "mode"]).unwrap_or("").into(),
        difficulty: l.setting_str(&["game", "difficulty"]).unwrap_or("").into(),
        campaign: l.setting_str(&["game", "campaign"]).unwrap_or("").into(),
        chapter: l.setting_int(&["game", "chapter"]).unwrap_or(0),
        state: l.setting_str(&["game", "state"]).unwrap_or("").into(),
        players: l.setting_int(&["Members", "numPlayers"]).unwrap_or(0),
        slots: l.setting_int(&["Members", "numSlots"]).unwrap_or(0),
        session_id: l.session_id,
    }
}

/// Members from a ReplyJoinData / settings tree (Members/machineN/player0).
fn members_from(settings: &Kv) -> Vec<Member> {
    let mut out = Vec::new();
    for m in 0..8 {
        let key = format!("machine{m}");
        let Some(machine) = settings.get(&["Members", &key]) else { continue };
        for p in 0..4 {
            let pk = format!("player{p}");
            let Some(player) = machine.get(&[&pk]) else { continue };
            out.push(Member {
                name: player.str(&["name"]).unwrap_or("").to_string(),
                xuid: player.u64(&["xuid"]).unwrap_or(0),
                avatar: player.str(&["game", "Avatar"]).unwrap_or("").to_string(),
                host: m == 0,
            });
        }
    }
    out
}

#[cfg(windows)]
fn run(cfg: Config, s: Arc<Mutex<Shared>>, rx: Receiver<Command>) -> Result<(), String> {
    use crate::sys::Pcap;
    let xex = std::fs::read(&cfg.xex).map_err(|e| format!("{}: {e}", cfg.xex.display()))?;
    let keys = broadcast_keys(&xex_lan_key(&xex).map_err(|e| format!("{e:?}"))?, &cfg.platform_key);
    let schema_text = std::fs::read_to_string(&cfg.sendtables).map_err(|e| format!("{}: {e}", cfg.sendtables.display()))?;
    let _schema = Schema::from_text(&schema_text).map_err(|e| format!("{e:?}"))?;
    let (device, mac) = adapter(&cfg.local_address)?;
    let mut pcap = Pcap::open(&device, 5)?;
    let xuid = cfg.xuid.unwrap_or_else(|| 0xE030_0000_0000_0000u64 | u32::from_le_bytes(random_array()) as u64);
    s.lock().unwrap().xuid = xuid;
    note(&s, format!("System Link ready on {}", cfg.local_address));

    let mut found: Vec<(Lobby, Instant)> = Vec::new();
    let mut searching = false;
    let mut next_search = Instant::now();
    let mut world: Option<World> = None;
    let mut session: Option<Joined> = None;

    struct Joined {
        assoc: Association,
        lobby: Lobby,
        lobby_chan: Channel,
        game_chan: Channel,
        signon: Signon,
        tables: StringTables,
        cvars: Vec<(String, String)>,
        cookie: u64,
        tick: u32,
        command: u32,
        history: Vec<UserCmd>,
        input: UserCmd,
        select: Option<u32>,
        /// A survivor pick waiting for the host's answer: (avatar, form tried, when).
        avatar_try: Option<(String, usize, Instant)>,
        /// Voice waiting for the next packet.
        voice_out: Vec<Vec<u8>>,
        /// The host's game event list (svc_GameEventList at sign-on).
        events: Vec<crate::svc::EventType>,
        connect_sent: Instant,
        next_beat: Instant,
        /// When we last asked to leave spectators ("jointeam 2") and how often.
        join_team_at: Option<Instant>,
        join_team_tries: u32,
        /// Idle on a survivor bot (the AFK timer handed it over): tap Attack,
        /// which the server answers by giving the bot back.
        idle_on_bot: bool,
    }

    loop {
        // Commands.
        loop {
            match rx.try_recv() {
                Ok(Command::Search) => {
                    searching = true;
                    next_search = Instant::now();
                    s.lock().unwrap().phase = Phase::Searching;
                }
                Ok(Command::Join(i)) => {
                    let Some((lobby, _)) = found.get(i).cloned() else { continue };
                    searching = false;
                    let (assoc, request) = Association::start(&keys, &lobby, mac);
                    pcap.send(&request)?;
                    note(&s, format!("Joining {}'s System Link lobby...", lobby.host_name));
                    {
                        let mut g = s.lock().unwrap();
                        g.phase = Phase::Joining;
                        g.joined = Some(lobby_info(&lobby));
                    }
                    session = Some(Joined {
                        assoc,
                        lobby,
                        lobby_chan: Channel::lobby(),
                        game_chan: Channel::game(),
                        signon: Signon::default(),
                        tables: StringTables::default(),
                        cvars: Vec::new(),
                        cookie: 0,
                        tick: 0,
                        command: 0,
                        history: Vec::new(),
                        input: UserCmd::default(),
                        select: None,
                        events: Vec::new(),
                        voice_out: Vec::new(),
                        avatar_try: None,
                        connect_sent: Instant::now(),
                        next_beat: Instant::now(),
                        join_team_at: None,
                        idle_on_bot: false,
                        join_team_tries: 0,
                    });
                }
                Ok(Command::Leave) => {
                    if let Some(mut j) = session.take() {
                        if let Some(t) = j.assoc.teardown() {
                            let _ = pcap.send(&t);
                        }
                    }
                    world = None;
                    let mut g = s.lock().unwrap();
                    g.phase = Phase::Idle;
                    g.state = None;
                    g.map = None;
                    g.members.clear();
                }
                Ok(Command::Input(cmd)) => {
                    if let Some(j) = session.as_mut() {
                        j.input = cmd;
                    }
                }
                Ok(Command::SelectWeapon(entity)) => {
                    if let Some(j) = session.as_mut() {
                        j.select = Some(entity);
                    }
                }
                Ok(Command::Avatar(avatar)) => {
                    if let Some(j) = session.as_mut() {
                        let mut w = BitWriter::new();
                        game::write_lobby_kv(&mut w, &game::game_avatar_variant(xuid, &avatar, 0));
                        j.lobby_chan.queue_reliable(&w);
                        note(&s, format!("asked the host for {avatar}"));
                        j.avatar_try = Some((avatar, 0, Instant::now()));
                    }
                }
                Ok(Command::Say(team, text)) => {
                    if let Some(j) = session.as_mut() {
                        let clean: String = text.chars().filter(|c| *c != '"' && !c.is_control()).take(127).collect();
                        let mut w = BitWriter::new();
                        game::write_string_cmd(&mut w, &format!("{} \"{clean}\"", if team { "say_team" } else { "say" }));
                        j.game_chan.queue_reliable(&w);
                    }
                }
                Ok(Command::Voice(bytes)) => {
                    if let Some(j) = session.as_mut() {
                        j.voice_out.push(bytes);
                        if j.voice_out.len() > 25 {
                            j.voice_out.remove(0);
                        }
                    }
                }
                Ok(Command::Vote(yes)) => {
                    if let Some(j) = session.as_mut() {
                        let mut w = BitWriter::new();
                        game::write_string_cmd(&mut w, if yes { "Vote Yes" } else { "Vote No" });
                        j.game_chan.queue_reliable(&w);
                        if let Some(v) = s.lock().unwrap().vote.as_mut() {
                            v.voted = true;
                        }
                    }
                }
                Ok(Command::Stop) | Err(TryRecvError::Disconnected) => {
                    if let Some(mut j) = session.take() {
                        if let Some(t) = j.assoc.teardown() {
                            let _ = pcap.send(&t);
                        }
                    }
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        // Discovery.
        if searching && Instant::now() >= next_search {
            next_search = Instant::now() + Duration::from_millis(1500);
            let search = encode_broadcast(&keys, &Broadcast {
                sequence: u16::from_le_bytes(random_array()),
                source_port: 27026,
                destination_port: 27026,
                body: lan_search().encode_discovery(),
                metadata: [0xd4, 0x08, 0x41, 0x45, 0x03, 0x06, 0, 0, 0, 0x97, 0x44, 0x20],
            });
            pcap.send(&ethernet_frame(&search, mac, BROADCAST_MAC))?;
            found.retain(|(_, at)| at.elapsed() < Duration::from_secs(10));
            s.lock().unwrap().lobbies = found.iter().map(|(l, _)| lobby_info(l)).collect();
        }

        // Timers for the joined session.
        if let Some(j) = session.as_mut() {
            // No answer to a survivor pick: try the next form of the request.
            if let Some((avatar, form, at)) = j.avatar_try.clone() {
                if at.elapsed() > Duration::from_millis(1500) {
                    if form + 1 < 4 {
                        let mut w = BitWriter::new();
                        game::write_lobby_kv(&mut w, &game::game_avatar_variant(xuid, &avatar, form + 1));
                        j.lobby_chan.queue_reliable(&w);
                        note(&s, format!("no answer for {avatar}; trying request form {} (lobby channel: {} acked, {} resent, waiting {})",
                            form + 1, j.lobby_chan.reliable_acked, j.lobby_chan.reliable_resent, j.lobby_chan.reliable_outstanding()));
                        j.avatar_try = Some((avatar, form + 1, Instant::now()));
                    } else {
                        note(&s, format!("the host never answered the {avatar} pick"));
                        j.avatar_try = None;
                    }
                }
            }
            if j.assoc.state == State::Established && Instant::now() >= j.next_beat {
                j.next_beat = Instant::now() + Duration::from_millis(33);
                let f = j.assoc.vdp(27027, 27027, &j.lobby_chan.heartbeat());
                pcap.send(&f)?;
                let phase = s.lock().unwrap().phase;
                if phase == Phase::Connecting && j.connect_sent.elapsed() > Duration::from_secs(1) {
                    pcap.send(&j.assoc.vdp(27005, 27015, &game::connect_query()))?;
                    j.connect_sent = Instant::now();
                }
                if matches!(phase, Phase::SigningOn | Phase::InGame) {
                    let mut unrel = BitWriter::new();
                    if j.signon.state == game::SIGNON_FULL {
                        j.command += 1;
                        let weapon_select = j.select.take().unwrap_or(0);
                        let mut cmd = UserCmd { command_number: j.command, tick_count: j.signon.last_tick, weapon_select, ..j.input };
                        if j.idle_on_bot && j.command % 16 < 4 {
                            cmd.buttons |= 1; // IN_ATTACK, pressed and released
                        }
                        j.history.push(cmd);
                        if j.history.len() > 3 {
                            j.history.remove(0);
                        }
                        game::write_move(&mut unrel, &j.history, 1);
                        game::write_tick(&mut unrel, j.signon.snapshot_tick, 3333, 300);
                        if !j.voice_out.is_empty() {
                            let bytes: Vec<u8> = j.voice_out.drain(..).flatten().collect();
                            game::write_voice(&mut unrel, xuid, &bytes);
                        }
                    }
                    let f = j.assoc.vdp(27005, 27015, &j.game_chan.send(&unrel));
                    pcap.send(&f)?;
                }
            }
        }

        let Some(frame) = pcap.recv()? else { continue };

        // Broadcasts (discovery replies).
        if let Some(f) = parse_frame(&frame) {
            if f.destination == BROADCAST_MAC && f.source != mac {
                if let Ok(b) = decode_broadcast(&keys, f.payload) {
                    if let Ok(msg) = Message::decode_discovery(&b.body) {
                        if let Some(l) = parse_game_details(&msg) {
                            found.retain(|(o, _)| o.session_id != l.session_id);
                            found.push((l, Instant::now()));
                            if searching {
                                s.lock().unwrap().lobbies = found.iter().map(|(l, _)| lobby_info(l)).collect();
                            }
                        }
                    }
                }
                continue;
            }
        }

        let Some(j) = session.as_mut() else { continue };
        let was = j.assoc.state;
        let (out, packets) = j.assoc.receive(&frame);
        for o in out {
            pcap.send(&o)?;
        }
        if was == State::Exchanging && j.assoc.state == State::Established {
            let player = Player { xuid, name: cfg.name.clone(), xnaddr: j.assoc.local_xnaddr(), dlcmask: 0 };
            let f = j.assoc.vdp(27027, 27027, &request_join_data(&player, &j.lobby.session_id).encode());
            pcap.send(&f)?;
        }
        if j.assoc.state == State::Closed {
            note(&s, "The Xbox host closed the connection.");
            session = None;
            world = None;
            let mut g = s.lock().unwrap();
            g.phase = Phase::Idle;
            g.state = None;
            continue;
        }
        for p in packets {
            let Packet::Data { source_port, destination_port, encrypted, .. } = p else { continue };
            match (source_port, destination_port) {
                (27027, 27027) => {
                    let Incoming::Packet(r) = j.lobby_chan.receive(&encrypted) else { continue };
                    for blob in &r.reliable {
                        for m in game::walk(blob, 0, blob.len() * 8, true).0 {
                            let Msg::KeyValues(Kv::Section(c)) = m else { continue };
                            let Some((name, body)) = c.first() else { continue };
                            match name.as_str() {
                                "SysSession::ReplyJoinData" => {
                                    let members = body.get(&["Settings"]).map(members_from).unwrap_or_default();
                                    let list: Vec<String> = members.iter().map(|m| format!("{} {:016X} '{}'", m.name, m.xuid, m.avatar)).collect();
                                    note(&s, format!("host members: {} (we are {xuid:016X})", list.join(", ")));
                                    let mut g = s.lock().unwrap();
                                    g.phase = Phase::InLobby;
                                    g.members = members;
                                    drop(g);
                                    note(&s, format!("Joined {}'s lobby.", j.lobby.host_name));
                                    // A native 360 client says "not ready" once it is in the lobby,
                                    // and the host answers with OnPlayerUpdated: it also shows the
                                    // host acts on our lobby commands.
                                    if body.str(&["Settings", "game", "state"]) != Some("game") {
                                        let mut w = BitWriter::new();
                                        game::write_lobby_kv(&mut w, &game::game_ready_command(xuid, false));
                                        j.lobby_chan.queue_reliable(&w);
                                        note(&s, "told the host we are not ready (it should answer)".to_string());
                                    }
                                }
                                "SysSession::OnPlayerUpdated" => {
                                    // A member picked a survivor (or readied).
                                    let who = body.u64(&["xuid"]).unwrap_or(0);
                                    note(&s, format!("player update {who:016X}: avatar {:?} ready {:?}", body.str(&["game", "Avatar"]), body.str(&["game", "ready"])));
                                    if let Some(avatar) = body.str(&["game", "Avatar"]) {
                                        if who == xuid {
                                            if let Some((_, form, _)) = j.avatar_try.take() {
                                                note(&s, format!("the host gave us {avatar} (request form {form})"));
                                            }
                                        }
                                        let mut g = s.lock().unwrap();
                                        if let Some(m) = g.members.iter_mut().find(|m| m.xuid == who) {
                                            m.avatar = avatar.to_string();
                                        }
                                    }
                                }
                                "SysSession::OnPlayerRemoved" => {
                                    let who = body.u64(&["xuid"]).unwrap_or(0);
                                    s.lock().unwrap().members.retain(|m| m.xuid != who);
                                }
                                "SysSession::OnUpdate" => {
                                    // The host changed the game settings in the lobby.
                                    let mut g = s.lock().unwrap();
                                    if let Some(l) = g.joined.as_mut() {
                                        if let Some(v) = body.str(&["update", "game", "mode"]) { l.mode = v.to_string(); }
                                        if let Some(v) = body.str(&["update", "game", "difficulty"]) { l.difficulty = v.to_string(); }
                                        if let Some(v) = body.str(&["update", "game", "campaign"]) { l.campaign = v.to_string(); }
                                        if let Some(v) = body.int(&["update", "game", "chapter"]) { l.chapter = v; }
                                    }
                                }
                                _ => {}
                            }
                            // Joining a game already running: the START reservation went out
                            // before we joined; it is the session nonce (relay capture).
                            let running = name == "SysSession::ReplyJoinData"
                                && body.str(&["Settings", "game", "state"]) == Some("game");
                            note(&s, format!("lobby message {name}{}", if running { " (game in progress)" } else { "" }));
                            let reservation = body.u64(&["update", "server", "reservationid"])
                                .or_else(|| body.u64(&["Settings", "server", "reservationid"]))
                                .or_else(|| running.then(|| body.u64(&["Settings", "System", "nonce"])).flatten());
                            if let Some(id) = reservation {
                                if s.lock().unwrap().phase == Phase::InLobby {
                                    j.cookie = id;
                                    let mut w = BitWriter::new();
                                    game::write_lobby_kv(&mut w, &game::game_ready_command(xuid, false));
                                    j.lobby_chan.queue_reliable(&w);
                                    pcap.send(&j.assoc.vdp(27027, 27027, &j.lobby_chan.send(&BitWriter::new())))?;
                                    pcap.send(&j.assoc.vdp(27005, 27015, &game::connect_query()))?;
                                    j.connect_sent = Instant::now();
                                    s.lock().unwrap().phase = Phase::Connecting;
                                    note(&s, "The host started the game; connecting...");
                                }
                            }
                        }
                    }
                }
                (27015, 27005) => match j.game_chan.receive(&encrypted) {
                    Incoming::Connectionless(d) => {
                        let phase = s.lock().unwrap().phase;
                        if let Some(ch) = game::parse_challenge(&d) {
                            if phase == Phase::Connecting {
                                let session_cookie = if j.cookie != 0 { j.cookie } else { ch.cookie };
                                j.cvars = game::client_cvars(&cfg.name, xuid, session_cookie);
                                pcap.send(&j.assoc.vdp(27005, 27015, &game::connect_packet(&ch, &cfg.name, &j.cvars)))?;
                            }
                        } else if game::is_accept(&d) && phase == Phase::Connecting {
                            s.lock().unwrap().phase = Phase::SigningOn;
                            // Load as the survivor the lobby gave us (the server reads avatarinfo).
                            let lobby_avatar = {
                                let g = s.lock().unwrap();
                                g.members.iter().find(|m| m.xuid == xuid).map(|m| m.avatar.clone()).unwrap_or_default()
                            };
                            let avatar = if lobby_avatar.is_empty() { cfg.avatar.clone() } else { lobby_avatar };
                            note(&s, format!("joining the game as {}", if avatar.is_empty() { "the host's choice" } else { &avatar }));
                            let w = j.signon.connected(&j.cvars, xuid, &avatar);
                            j.game_chan.queue_reliable(&w);
                            pcap.send(&j.assoc.vdp(27005, 27015, &j.game_chan.send(&BitWriter::new())))?;
                            world = Some(World::new(Schema::from_text(&schema_text).map_err(|e| format!("{e:?}"))?));
                        } else if let Some(why) = game::rejection(&d) {
                            note(&s, format!("The Xbox refused the connection: {why}"));
                        }
                    }
                    Incoming::Packet(r) => {
                        // Game events (votes): need the raw event list and event bits.
                        {
                            let mut parts: Vec<(&[u8], usize, usize)> = r.reliable.iter().map(|b| (b.as_slice(), 0, b.len() * 8)).collect();
                            parts.push((&r.payload, r.unreliable.0, r.unreliable.1));
                            for (d, st, en) in parts {
                                for m in crate::svc::split(d, st, en).unwrap_or_default() {
                                    match m {
                                        crate::svc::Svc::Raw { kind: 30, data, bits } => {
                                            if let Some(l) = crate::svc::parse_event_list(&data, bits) {
                                                j.events = l;
                                            }
                                        }
                                        crate::svc::Svc::Raw { kind: 25, data, bits } => {
                                            if let Some((name, values)) = crate::svc::parse_event(&j.events, &data, bits) {
                                                if name == "weapon_fire" {
                                                    if let Some(id) = values.iter().find(|(k, _)| k == "userid").and_then(|(_, v)| v.as_int()) {
                                                        *s.lock().unwrap().shots.entry(id as i32).or_insert(0) += 1;
                                                    }
                                                }
                                                vote_event(&s, &name, &values);
                                            }
                                        }
                                        crate::svc::Svc::Raw { kind: 23, data, bits } => {
                                            if let Some(line) = chat_message(&data, bits) {
                                                let mut g = s.lock().unwrap();
                                                g.chat_in.push(line);
                                                if g.chat_in.len() > 50 {
                                                    g.chat_in.remove(0);
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        let mut msgs = Vec::new();
                        for blob in &r.reliable {
                            let (m, stop) = game::walk(blob, 0, blob.len() * 8, false);
                            msgs.extend(m);
                            if let Some(at) = stop {
                                msgs.extend(game::find_signon_states(blob, at));
                            }
                        }
                        msgs.extend(game::walk(&r.payload, r.unreliable.0, r.unreliable.1, false).0);
                        if let Some(w) = world.as_mut() {
                            let mut changed = false;
                            for m in &msgs {
                                match m {
                                    Msg::Tick { tick } => j.tick = *tick,
                                    Msg::VoiceData { xuid: who, data, .. } => {
                                        let mut g = s.lock().unwrap();
                                        g.voice_in.push((*who, data.clone()));
                                        if g.voice_in.len() > 100 {
                                            g.voice_in.remove(0);
                                        }
                                    }
                                    // svc_ServerInfo comes before the string tables: load the
                                    // map's dictionary here (modelprecache etc. are
                                    // dictionary-encoded), and reset on a chapter change.
                                    Msg::ServerInfo(info) if !info.map.is_empty() => {
                                        let map = &info.map;
                                        let old = s.lock().unwrap().map.clone();
                                        if old.as_deref().is_some_and(|m| m != map) {
                                            note(&s, format!("The host changed map to {map}"));
                                            if let Ok(schema) = Schema::from_text(&schema_text) {
                                                *w = World::new(schema);
                                            }
                                            j.join_team_at = None;
                                            j.join_team_tries = 0;
                                        }
                                        j.tables = StringTables::default();
                                        let bsp = cfg.maps.join(format!("{map}.360.bsp"));
                                        j.tables.dictionary = std::fs::read(&bsp).ok().and_then(|b| dictionary_from_bsp(&b));
                                        if j.tables.dictionary.is_none() {
                                            note(&s, format!("no string table dictionary for {map} ({})", bsp.display()));
                                        }
                                        s.lock().unwrap().map = Some(map.clone());
                                    }
                                    Msg::SignonState { map, .. } if !map.is_empty() => {
                                        let mut g = s.lock().unwrap();
                                        if g.map.is_none() {
                                            g.map = Some(map.clone());
                                        }
                                    }
                                    Msg::CreateStringTable { name, max_entries, entries, user_data_fixed, user_data_bits, compressed, data, bits } => {
                                        let t = j.tables.create(name, *max_entries, *entries, user_data_fixed.then_some(*user_data_bits),
                                            *compressed, data, *bits);
                                        if name == "instancebaseline" {
                                            let e: Vec<StringEntry> = j.tables.tables[t].entries.iter().enumerate()
                                                .map(|(k, (n, d))| StringEntry { index: k as u32, name: n.clone(), data: d.clone() }).collect();
                                            w.set_baselines(&e);
                                        }
                                    }
                                    Msg::UpdateStringTable { table, changed: n, data, bits } => {
                                        let _ = j.tables.update(*table, *n, data, *bits);
                                        // Classes that appear later (bots, specials) get their
                                        // instance baselines here.
                                        if let Some(t) = j.tables.tables.get(*table as usize).filter(|t| t.name == "instancebaseline") {
                                            let e: Vec<StringEntry> = t.entries.iter().enumerate()
                                                .map(|(k, (n, d))| StringEntry { index: k as u32, name: n.clone(), data: d.clone() }).collect();
                                            w.set_baselines(&e);
                                        }
                                    }
                                    Msg::PacketEntities { delta_from, updated, data, bits, baseline, update_baseline } => {
                                        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                                        if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 150 == 0 {
                                            note(&s, format!("snapshot tick {} delta_from {:?} we acked {} cmd {}", j.tick, delta_from, j.signon.snapshot_tick, j.command));
                                        }
                                        if w.apply_with(data, *bits, *updated, *delta_from, j.tick, *baseline, *update_baseline).is_ok() {
                                            changed = true;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            if changed {
                                let gs = game_state(w, &j.tables);
                                let ours = gs.players.iter().find(|p| p.xuid == xuid).map(|p| p.entity);
                                // Joined in progress: we start as a spectator; take over a survivor bot.
                                let me = gs.players.iter().find(|p| p.xuid == xuid).map(|p| p.entity as i64);
                                let idle_on_bot = me.is_some_and(|e| gs.players.iter().any(|p| p.idle_human == e));
                                let spectating = gs.players.iter().any(|p| p.xuid == xuid && p.team == 1);
                                if idle_on_bot != j.idle_on_bot {
                                    j.idle_on_bot = idle_on_bot;
                                    note(&s, if idle_on_bot { "idle: the host gave our survivor to a bot; pressing Attack to take it back".to_string() } else { "idle: back in control".to_string() });
                                }
                                let due = j.join_team_at.is_none_or(|t| t.elapsed() > Duration::from_secs(5));
                                if spectating && j.signon.state == game::SIGNON_FULL && due && j.join_team_tries < 24 {
                                    j.join_team_at = Some(Instant::now());
                                    j.join_team_tries += 1;
                                    // Retail clients send "joingame" once fully connected (the server then
                                    // hands them a survivor bot); fall back to the team commands.
                                    // Idle on a bot: the retail client turns Attack into "spec_next",
                                    // which the server answers by handing the bot back.
                                    let cmd = match j.join_team_tries {
                                        _ if idle_on_bot => ["spec_next", "spec_prev", "spec_mode"][(j.join_team_tries as usize - 1) % 3],
                                        1 | 2 => "joingame",
                                        n if n % 2 == 1 => "jointeam survivor",
                                        _ => "jointeam 2",
                                    };
                                    let mut w2 = BitWriter::new();
                                    game::write_string_cmd(&mut w2, cmd);
                                    j.game_chan.queue_reliable(&w2);
                                    note(&s, format!("Spectating: asked to join the survivors ({cmd})"));
                                }
                                let mut g = s.lock().unwrap();
                                g.state = Some(gs);
                                g.our_entity = ours.or(g.our_entity);
                                g.state_serial += 1;
                                if j.signon.state == game::SIGNON_FULL {
                                    g.phase = Phase::InGame;
                                }
                            }
                        }
                        let (rel, _) = j.signon.handle(&msgs);
                        if rel.bits > 0 {
                            j.game_chan.queue_reliable(&rel);
                        }
                    }
                    Incoming::Invalid => {}
                },
                _ => {}
            }
        }
    }
}
