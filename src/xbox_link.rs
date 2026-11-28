//! Xbox 360 System Link: play in a real Xbox 360 L4D2 game.
//!
//! The vendor crate `l4d2r-xbox-link` runs the 360 session (XNet over Npcap,
//! lobby join, connect, sign-on, entity decoding) on its own thread. This
//! module bridges it into the existing multiplayer session as a client
//! `Link`: the Xbox host looks like a rewrite host, so the lobby, chapter
//! loading and remote survivors (`net_play`) work unchanged.
//!
//! Setup: `runtime/xbox-link.cfg` (private, not in the repository):
//!   xex = <path to the user's own L4D2 default.xex>
//!   platform_source = <emulator source containing key19[]>
//!   local_address = <PC IPv4 on the console's network>
//!   sendtables = <sendtables-360.txt recovered from the user's console>
//!   maps = <folder with the 360 maps, *.360.bsp>
//!   avatar = mechanic            (optional: gambler, producer, coach, mechanic)
//!
//! Console: `xbox_find`, `xbox_join <n>`, `xbox_status`, `xbox_leave`.

use bevy::prelude::*;
use l4d2r_xbox_link::client::{Command, Config, Phase, XboxClient};
use l4d2r_xbox_link::state::{survivor_name, weapon_name, weapon_slot, GameState, TEAM_SURVIVORS};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

static CLIENT: Mutex<Option<XboxClient>> = Mutex::new(None);
/// Hosting a System Link game for consoles (xbox_host).
static HOST: Mutex<Option<l4d2r_xbox_link::hostrun::XboxHost>> = Mutex::new(None);
/// Chapter map stems per campaign (co-op and Survival lists), to turn the
/// host's map name into lobby settings.
static CHAPTERS: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());
static SURVIVAL_CHAPTERS: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());
/// (map stem, campaign name, chapter name, chapter picture) for JOIN A GAME rows.
static CHAPTER_ART: Mutex<Vec<(String, String, String, String)>> = Mutex::new(Vec::new());

fn remember_art(campaigns: &[crate::campaign_settings::Campaign]) {
    let mut art = CHAPTER_ART.lock().unwrap();
    for c in campaigns {
        for ch in &c.chapters {
            let stem = ch.map.file_stem().unwrap_or_default().to_string_lossy().to_string();
            if !art.iter().any(|a| a.0 == stem) {
                art.push((stem, c.name.clone(), ch.name.clone(), ch.image.clone()));
            }
        }
    }
}

/// Catalog names and picture of a 360 lobby's campaign code and chapter ("L4D2C2", 1).
fn lobby_art(code: &str, chapter: i64) -> Option<(String, String, String)> {
    let n: u32 = code.to_ascii_uppercase().strip_prefix("L4D2C")?.parse().ok()?;
    let prefix = format!("c{n}m{}_", chapter.max(1));
    let art = CHAPTER_ART.lock().unwrap();
    art.iter().find(|a| a.0.to_ascii_lowercase().starts_with(&prefix)).map(|a| (a.1.clone(), a.2.clone(), a.3.clone()))
}

/// Appends a line to runtime/xbox-link.log (and stdout).
pub(crate) fn log(line: &str) {
    use std::io::Write;
    println!("Xbox: {line}");
    let path = config_path().with_file_name("xbox-link.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.);
        let _ = writeln!(f, "{t:.2} {line}");
    }
}

fn config_path() -> std::path::PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    // From the repository root, or started from runtime\ itself (double-click):
    // the config sits in runtime\ next to the exe.
    for base in [std::env::current_dir().ok(), exe_dir].into_iter().flatten() {
        for p in [base.join("runtime").join("xbox-link.cfg"), base.join("xbox-link.cfg")] {
            if p.exists() {
                return p;
            }
        }
    }
    std::path::PathBuf::from("runtime/xbox-link.cfg")
}

fn platform_key(source: &str) -> Result<[u8; 16], String> {
    let text = std::fs::read_to_string(source).map_err(|e| format!("{source}: {e}"))?;
    let at = text.find("key19[]").ok_or("key19[] not found in platform_source")?;
    let body = &text[at..];
    let (open, close) = (body.find('{').ok_or("bad key19")?, body.find('}').ok_or("bad key19")?);
    let key: Vec<u8> = body[open + 1..close]
        .split(',')
        .filter_map(|t| u8::from_str_radix(t.trim().trim_start_matches("0x").trim_start_matches("0X"), 16).ok())
        .collect();
    key.try_into().map_err(|_| "key19 must be 16 bytes".into())
}

fn load_config() -> Result<Config, String> {
    let path = config_path();
    let text = std::fs::read_to_string(&path).map_err(|_| format!("Missing {} (see src/xbox_link.rs for the format).", path.display()))?;
    let mut get = std::collections::HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            get.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
        }
    }
    let need = |k: &str| get.get(k).cloned().ok_or(format!("{} is missing '{k}'", path.display()));
    Ok(Config {
        xex: need("xex")?.into(),
        platform_key: platform_key(&need("platform_source")?)?,
        local_address: need("local_address")?,
        sendtables: need("sendtables")?.into(),
        maps: need("maps")?.into(),
        name: crate::game_chat::local_name(),
        // No avatar: the 360 lobby shows us as "?" and the host assigns one.
        avatar: get.get("avatar").cloned().unwrap_or_default(),
        xuid: Some(stable_xuid(&get)),
    })
}

/// xbox_host: host a System Link game for consoles. Needs `host_recording`
/// in runtime/xbox-link.cfg: a private relay recording of a 360 host's
/// sign-on on the map to host (its map is the one hosted).
fn start_hosting() -> String {
    use l4d2r_xbox_link::host::GameSettings;
    use l4d2r_xbox_link::hostrun::{HostSetup, XboxHost};
    let config = match load_config() {
        Ok(c) => c,
        Err(e) => return e,
    };
    let text = std::fs::read_to_string(config_path()).unwrap_or_default();
    let Some(recording) = text.lines().find_map(|l| l.trim().strip_prefix("host_recording").map(|r| r.trim_start_matches([' ', '=']).trim().trim_matches('"').to_string())) else {
        return format!("{} needs host_recording = <relay.jsonl> (a 360 host's sign-on on the map to host).", config_path().display());
    };
    // One System Link role at a time (both use the same adapter).
    *CLIENT.lock().unwrap() = None;
    let settings = GameSettings { mode: "coop".into(), difficulty: "normal".into(), campaign: "L4D2C1".into(), chapter: 1,
        state: "lobby".into(), players: 1, slots: 4 };
    match XboxHost::start(HostSetup { config, template_recording: recording.into(), settings, console_slot: 2 }) {
        Ok(h) => {
            *HOST.lock().unwrap() = Some(h);
            log("hosting started");
            "Hosting a System Link game: consoles can find it now; run xbox_host_start to begin.".into()
        }
        Err(e) => e,
    }
}

/// The weapon a survivor has out (rewrite name): its primary in slot 1, else the pistol.
fn held_weapon(m: &crate::game_state::Match) -> String {
    let class = match (&m.primary, m.slot) {
        (Some(p), 1) => p.class.clone(),
        _ => m.pistol.class.clone(),
    };
    if class.starts_with("weapon_") { class } else { format!("weapon_{class}") }
}

/// While hosting: our survivor (slot 1) and bots (slots 3, 4) go to the consoles.
fn feed_host(
    body: Option<Single<&crate::game::Body>>,
    state: Option<Res<crate::game_state::Match>>,
    squad: Option<Res<crate::game_state::Squad>>,
    mut last: Local<Option<Instant>>,
) {
    use l4d2r_xbox_link::hostrun::{Feed, HostCommand};
    let guard = HOST.lock().unwrap();
    let Some(host) = guard.as_ref() else { return };
    if last.is_some_and(|t| t.elapsed().as_secs_f32() < 1.0 / 30.0) {
        return;
    }
    *last = Some(Instant::now());
    let to_source = |p: Vec3| [p.x / 0.0254, -p.z / 0.0254, p.y / 0.0254];
    let mut feed = Vec::new();
    if let Some(body) = body.as_deref() {
        let (pitch, yaw) = source_angles(body.yaw, body.pitch);
        let health = state.as_deref().map_or(100, |m| m.health.round() as i64);
        let weapon = state.as_deref().map(held_weapon);
        feed.push(Feed { slot: 1, origin: to_source(body.feet), pitch, yaw, health, weapon });
    }
    if let Some(squad) = squad.as_deref() {
        for (i, c) in squad.0.iter().take(2).enumerate() {
            let yaw = (-c.aim.z).atan2(c.aim.x).to_degrees();
            feed.push(Feed { slot: 3 + i as u32, origin: to_source(c.position), pitch: 0.0, yaw, health: c.state.health.round() as i64,
                weapon: Some(held_weapon(&c.state)) });
        }
    }
    host.send(HostCommand::Feed(feed));
}

/// In (or connecting to) a game hosted by an Xbox, from the lobby on.
pub(crate) fn in_xbox_lobby() -> bool {
    CLIENT.lock().unwrap().as_ref().is_some_and(|c| {
        matches!(c.shared().phase, Phase::InLobby | Phase::Connecting | Phase::SigningOn | Phase::InGame)
    })
}

/// The client's status notes (lobby messages, connection steps) into the log.
fn forward_notes(mut seen: Local<usize>) {
    let guard = CLIENT.lock().unwrap();
    let Some(c) = guard.as_ref() else {
        *seen = 0;
        return;
    };
    let g = c.shared();
    if g.messages.len() < *seen {
        *seen = 0;
    }
    let new: Vec<String> = g.messages[*seen..].to_vec();
    *seen = g.messages.len();
    drop(g);
    drop(guard);
    for line in new.iter().filter(|l| !l.starts_with("snapshot tick")) {
        log(&format!("client: {line}"));
    }
}

/// Our encoded voice to the Xbox host.
pub(crate) fn send_voice(bytes: Vec<u8>) {
    if let Some(c) = CLIENT.lock().unwrap().as_ref() {
        c.send(Command::Voice(bytes));
    }
}

/// Starts the Speex codec while in System Link and plays the voice that arrives.
fn voice_io(mut tried: Local<bool>) {
    if !in_system_link() {
        return;
    }
    if !*tried {
        *tried = true;
        let text = std::fs::read_to_string(config_path()).unwrap_or_default();
        let dll = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("voice_dll").map(|r| r.trim_start_matches([' ', '=']).trim().trim_matches('"').to_string()))
            .unwrap_or_else(|| "C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2/bin/vaudio_speex.dll".into());
        match crate::voice_link::ensure_codec(std::path::Path::new(&dll)) {
            Ok(()) => log("voice: Speex codec ready (hold K to talk)"),
            Err(e) => log(&format!("voice: no Speex codec ({e})")),
        }
    }
    let incoming = {
        let guard = CLIENT.lock().unwrap();
        let Some(c) = guard.as_ref() else { return };
        let v = std::mem::take(&mut c.shared().voice_in);
        v
    };
    for (_, bytes) in incoming {
        if let Some(pcm) = crate::voice_link::decode(&bytes) {
            crate::voice_link::play(&pcm);
        }
    }
}

/// True while in a game hosted by a real Xbox (it decides all damage).
pub(crate) fn joined_xbox() -> bool {
    CLIENT.lock().unwrap().as_ref().is_some_and(|c| c.shared().phase == Phase::InGame)
}

/// True while joined to, or hosting, a System Link game.
pub(crate) fn in_system_link() -> bool {
    HOST.lock().unwrap().is_some()
        || CLIENT.lock().unwrap().as_ref().is_some_and(|c| c.shared().phase == Phase::InGame)
}

/// Retail-style text for a 360 vote issue token.
fn vote_text(issue: &str, param: &str) -> String {
    let t = issue.to_ascii_lowercase();
    let p = param.trim();
    if t.contains("kick") {
        format!("Kick {p}?")
    } else if t.contains("restart") {
        "Restart the campaign?".into()
    } else if t.contains("lobby") {
        "Return to the lobby?".into()
    } else if t.contains("difficulty") {
        format!("Change difficulty to {p}?")
    } else if t.contains("mission") || t.contains("campaign") {
        format!("Change campaign to {}?", campaign_title(p))
    } else if t.contains("chapter") {
        format!("Change chapter to {p}?")
    } else if t.contains("alltalk") || t.contains("all_talk") {
        "Change all talk?".into()
    } else {
        let s = issue.trim_start_matches('#').replace('_', " ");
        if p.is_empty() { format!("{s}?") } else { format!("{s} {p}?") }
    }
}

#[derive(Component)]
struct VotePanel;

/// Votes called on the Xbox: shown top left, answered with F1 (yes) / F2 (no).
fn vote_panel(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut fonts: ResMut<Assets<Font>>,
    mut font: Local<Option<Handle<Font>>>,
    mut panel: Query<(&mut Text, &mut Node), With<VotePanel>>,
) {
    let vote = {
        let guard = CLIENT.lock().unwrap();
        let Some(c) = guard.as_ref() else { return };
        if keys.just_pressed(KeyCode::F1) || keys.just_pressed(KeyCode::F2) {
            let open = c.shared().vote.as_ref().is_some_and(|v| v.result.is_none() && !v.voted);
            if open {
                c.send(Command::Vote(keys.just_pressed(KeyCode::F1)));
            }
        }
        let v = c.shared().vote.clone();
        v
    };
    let text = vote.as_ref().and_then(|v| {
        if v.decided.is_some_and(|t| t.elapsed().as_secs_f32() > 4.0) {
            return None;
        }
        let head = format!("VOTE: {}", vote_text(&v.issue, &v.param));
        Some(match v.result {
            Some(true) => format!("{head}
Vote passed."),
            Some(false) => format!("{head}
Vote failed."),
            None if v.voted => format!("{head}
Yes {}   No {}", v.yes, v.no),
            None => format!("{head}
F1: Yes ({})   F2: No ({})", v.yes, v.no),
        })
    });
    if panel.is_empty() {
        let handle = font.get_or_insert_with(|| crate::game_chat::chat_font(&mut fonts)).clone();
        commands.spawn((
            VotePanel,
            Text::new(""),
            TextFont { font: bevy::text::FontSource::Handle(handle), font_size: 18.0.into(), ..default() },
            TextColor(Color::srgb(1.0, 0.95, 0.8)),
            Node { position_type: PositionType::Absolute, left: Val::Px(24.0), top: Val::Px(120.0), display: Display::None, ..default() },
            GlobalZIndex(50),
        ));
        return;
    }
    for (mut t, mut node) in &mut panel {
        match &text {
            Some(s) => {
                if t.0 != *s {
                    t.0 = s.clone();
                }
                node.display = Display::Flex;
            }
            None => node.display = Display::None,
        }
    }
}

/// Online play (System Link or a rewrite session) keeps running under the pause menu.
fn update_keep_running() {
    crate::game_menu::set_keep_running(in_system_link() || crate::net_flow::online());
}

/// Consoles already handed to the session as guests.
static ANNOUNCED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Consoles that finished loading into our hosted game and are not yet
/// session members (multiplayer.rs adds each as a guest link).
pub(crate) fn take_new_consoles() -> Vec<ConsoleBridge> {
    let guard = HOST.lock().unwrap();
    let Some(host) = guard.as_ref() else {
        ANNOUNCED.lock().unwrap().clear();
        return Vec::new();
    };
    let players = host.shared().players.clone();
    let mut announced = ANNOUNCED.lock().unwrap();
    let fresh: Vec<_> = players.into_iter().filter(|p| p.in_game && !announced.contains(&p.xuid)).collect();
    announced.extend(fresh.iter().map(|p| p.xuid));
    fresh.into_iter().map(|p| ConsoleBridge { xuid: p.xuid, name: p.name, hello: false, last: Instant::now() }).collect()
}

/// One console as a guest of our session: says hello as Ellis, then sends
/// its survivor's position (from its usercmds, hostrun.rs) as "p" samples,
/// so the rewrite draws it like any other remote player.
pub struct ConsoleBridge {
    xuid: u64,
    name: String,
    hello: bool,
    last: Instant,
}

impl ConsoleBridge {
    pub fn poll(&mut self) -> Option<Vec<Vec<u8>>> {
        let guard = HOST.lock().unwrap();
        let host = guard.as_ref()?;
        let g = host.shared();
        let Some(p) = g.players.iter().find(|p| p.xuid == self.xuid) else {
            ANNOUNCED.lock().unwrap().retain(|x| *x != self.xuid);
            return None;
        };
        let mut out = Vec::new();
        if !self.hello {
            self.hello = true;
            out.push(json!({"t": "hello", "v": crate::multiplayer::PROTOCOL, "name": self.name, "survivor": "Ellis", "steam": 0}));
        }
        if self.last.elapsed().as_secs_f32() >= 0.05 {
            self.last = Instant::now();
            let m = json!({
                "k": "p",
                "s": "Ellis",
                "p": crate::binary::source_position(p.origin),
                "a": source_aim(p.pitch, p.yaw).to_array(),
                "h": 100,
                "t": 0.0,
                "i": false,
                "d": false,
                "w": 2,
                "g1": Value::Null,
                "g2": "weapon_pistol",
                "f": 0,
            });
            out.push(json!({"t": "g", "m": m}));
        }
        Some(out.into_iter().map(|v| serde_json::to_vec(&v).unwrap_or_default()).collect())
    }
}

/// While hosting: keeps each console survivor out of walls and on the floor
/// (the host thread moves it by its usercmds; the map's collision lives here).
fn collide_consoles(
    collision: Option<Res<crate::collision::Collision>>,
    mut accepted: Local<std::collections::HashMap<u64, Vec3>>,
) {
    use l4d2r_xbox_link::hostrun::HostCommand;
    let guard = HOST.lock().unwrap();
    let (Some(host), Some(collision)) = (guard.as_ref(), collision) else { return };
    let players = host.shared().players.clone();
    let up = Vec3::Y;
    let hull = Vec3::new(0.35, 0.3, 0.35);
    for p in players.iter().filter(|p| p.in_game) {
        let desired = Vec3::from_array(crate::binary::source_position(p.origin));
        let prev = *accepted.entry(p.xuid).or_insert(desired);
        // Walls: slide no further than the first hit at knee height.
        let start = prev + up * 0.65;
        let end = Vec3::new(desired.x, prev.y, desired.z) + up * 0.65;
        let t = collision.trace(start, end, hull);
        let mut pos = if t.start_solid { prev } else { prev + (end - start) * (t.fraction * 0.98) };
        pos.y = prev.y;
        // Floor: step up to 0.45 m, drop until ground.
        let down = collision.trace(pos + up * 0.45, pos - up * 1.0, Vec3::ZERO);
        if down.fraction < 1.0 && !down.start_solid {
            pos.y = pos.y + 0.45 - 1.45 * down.fraction;
        } else {
            pos.y -= 0.3;
        }
        accepted.insert(p.xuid, pos);
        if pos.distance(desired) > 0.01 {
            host.send(HostCommand::Place(p.xuid, [pos.x / 0.0254, -pos.z / 0.0254, pos.y / 0.0254]));
        }
    }
    accepted.retain(|x, _| players.iter().any(|p| p.xuid == *x));
}

/// While hosting: our prop doors (state and swing) go to the consoles.
fn feed_host_doors(logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>, mut last: Local<Option<Instant>>) {
    use l4d2r_game_core::entities::{DoorState, Kind};
    use l4d2r_xbox_link::host::DoorFeed;
    let guard = HOST.lock().unwrap();
    let (Some(host), Some(logic)) = (guard.as_ref(), logic) else { return };
    if last.is_some_and(|t| t.elapsed().as_secs_f32() < 0.2) {
        return;
    }
    *last = Some(Instant::now());
    let list: Vec<DoorFeed> = logic
        .world
        .ents
        .iter()
        .filter(|e| e.alive && e.me.classname.starts_with("prop_door"))
        .filter_map(|e| {
            let Kind::Door(d) = &e.kind else { return None };
            let state = match d.state {
                DoorState::Closed => 0,
                DoorState::Opening => 1,
                DoorState::Open => 2,
                DoorState::Closing => 3,
            };
            let yaw = if d.rotating { e.me.angles.y + d.dist * d.frac } else { e.me.angles.y };
            Some(DoorFeed { origin: [e.me.origin.x, e.me.origin.y, e.me.origin.z], yaw, state })
        })
        .collect();
    host.send(l4d2r_xbox_link::hostrun::HostCommand::Doors(list));
}

/// While hosting: the rewrite's living infected near our survivor go to the consoles.
fn feed_host_horde(
    enemies: Query<(Entity, &Transform, &crate::game::Enemy)>,
    body: Option<Single<&crate::game::Body>>,
    mut last: Local<Option<Instant>>,
) {
    use l4d2r_xbox_link::host::InfectedFeed;
    use l4d2r_xbox_link::hostrun::HostCommand;
    let guard = HOST.lock().unwrap();
    let Some(host) = guard.as_ref() else { return };
    if last.is_some_and(|t| t.elapsed().as_secs_f32() < 1.0 / 15.0) {
        return;
    }
    *last = Some(Instant::now());
    let near = body.as_deref().map(|b| b.feet).unwrap_or(Vec3::ZERO);
    let mut list: Vec<(f32, InfectedFeed)> = enemies
        .iter()
        .filter(|(_, _, e)| e.health > 0.0 && (e.model.contains("/infected/common") || e.model.contains("witch")))
        .map(|(entity, t, e)| {
            let p = t.translation;
            let yaw = t.rotation.to_euler(EulerRot::YXZ).0.to_degrees();
            (p.distance(near), InfectedFeed {
                id: entity.to_bits(),
                model: e.model.clone(),
                origin: [p.x / 0.0254, -p.z / 0.0254, p.y / 0.0254],
                yaw,
                witch: e.model.contains("witch"),
            })
        })
        .collect();
    list.sort_by(|a, b| a.0.total_cmp(&b.0));
    list.truncate(80);
    host.send(HostCommand::Infected(list.into_iter().map(|(_, f)| f).collect()));
}

/// Our xuid from runtime/xbox-link.cfg, made once and saved there, so a
/// restarted game is the same player to the 360 lobby.
fn stable_xuid(get: &std::collections::HashMap<String, String>) -> u64 {
    if let Some(x) = get.get("xuid").and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok()) {
        return x;
    }
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    let x = 0xE030_0000_0000_0000u64 | (seed ^ (seed >> 29)) & 0xFFFF_FFFF;
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(config_path()) {
        let _ = writeln!(f, "xuid = {x:016x}");
    }
    x
}

/// Leaving the game: tell the Xbox we left (XNet teardown), so the 360 lobby
/// does not keep a ghost of us.
fn leave_on_exit(mut exits: MessageReader<AppExit>) {
    if exits.read().next().is_none() {
        return;
    }
    if let Some(c) = CLIENT.lock().unwrap().take() {
        c.send(Command::Leave);
        drop(c);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    *HOST.lock().unwrap() = None;
}

fn with_client<T>(f: impl FnOnce(&XboxClient) -> T) -> Result<T, String> {
    let mut guard = CLIENT.lock().unwrap();
    if guard.is_none() {
        *guard = Some(XboxClient::start(load_config()?)?);
    }
    Ok(f(guard.as_ref().unwrap()))
}

/// Console commands. Returns None for other commands.
pub fn command(command: &str) -> Option<String> {
    let mut words = command.split_whitespace();
    let verb = words.next()?.to_ascii_lowercase();
    match verb.as_str() {
        "xbox_find" => Some(match with_client(|c| {
            c.send(Command::Search);
            lobby_list(c)
        }) {
            Ok(list) => format!("Searching for Xbox 360 System Link games...\n{list}"),
            Err(e) => e,
        }),
        "xbox_join" => {
            let n: usize = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
            Some(match with_client(|c| {
                let count = c.shared().lobbies.len();
                if n >= count {
                    return Err(format!("No System Link game #{n} (found {count}; run xbox_find first)."));
                }
                c.send(Command::Join(n));
                Ok(())
            }) {
                Ok(Ok(())) => {
                    crate::multiplayer::join_xbox();
                    format!("Joining Xbox game #{n}...")
                }
                Ok(Err(e)) | Err(e) => e,
            })
        }
        "xbox_join_id" => {
            let id: u64 = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
            Some(match with_client(|c| {
                let index = c.shared().lobbies.iter().position(|l| u64::from_be_bytes(l.session_id) == id);
                if let Some(i) = index {
                    c.send(Command::Join(i));
                }
                index
            }) {
                Ok(Some(_)) => {
                    crate::multiplayer::join_xbox();
                    "Joining the Xbox System Link game...".into()
                }
                Ok(None) => "That Xbox game is no longer listed.".into(),
                Err(e) => e,
            })
        }
        "xbox_host" => Some(start_hosting()),
        "xbox_host_start" => Some(match HOST.lock().unwrap().as_ref() {
            Some(h) => {
                h.send(l4d2r_xbox_link::hostrun::HostCommand::Start);
                "Starting: consoles in the lobby are connecting.".into()
            }
            None => "Not hosting (run xbox_host first).".into(),
        }),
        "xbox_host_stop" => {
            *HOST.lock().unwrap() = None;
            Some("Stopped hosting.".into())
        }
        "xbox_host_status" => Some(match HOST.lock().unwrap().as_ref() {
            Some(h) => {
                let g = h.shared();
                let mut t = format!("Hosting: running {}, started {}", g.running, g.started);
                for p in &g.players {
                    t += &format!("
  {} (slot {}) {} at ({:.0}, {:.0}, {:.0})", p.name, p.slot,
                        if p.in_game { "in game" } else { "in lobby" }, p.origin[0], p.origin[1], p.origin[2]);
                }
                if let Some(e) = &g.error {
                    t += &format!("
  error: {e}");
                }
                t
            }
            None => "Not hosting.".into(),
        }),
        "xbox_status" => Some(match CLIENT.lock().unwrap().as_ref() {
            None => "Xbox System Link is not running (xbox_find).".into(),
            Some(c) => {
                let g = c.shared();
                let mut out = format!("Xbox System Link: {:?}", g.phase);
                if let Some(m) = &g.map {
                    out += &format!(" on {m}");
                }
                if let Some(e) = &g.error {
                    out += &format!("\nError: {e}");
                }
                if let Some(gs) = &g.state {
                    for p in &gs.players {
                        out += &format!("\n  {:<16} {:<9} hp {:>3} at ({:.0}, {:.0}, {:.0})", p.name, p.class, p.health,
                            p.origin[0], p.origin[1], p.origin[2]);
                    }
                }
                for m in g.messages.iter().rev().take(5).rev() {
                    out += &format!("\n  {m}");
                }
                out
            }
        }),
        "xbox_leave" => {
            if let Some(c) = CLIENT.lock().unwrap().as_ref() {
                c.send(Command::Leave);
            }
            Some("Left the Xbox game.".into())
        }
        _ => None,
    }
}

fn lobby_list(c: &XboxClient) -> String {
    let g = c.shared();
    if g.lobbies.is_empty() {
        return "No games found yet (run xbox_find again in a moment).".into();
    }
    g.lobbies
        .iter()
        .enumerate()
        .map(|(i, l)| format!("  #{i}: {} - {} {} {} ch{} ({}) {}/{}", l.host, l.mode, l.difficulty, l.campaign, l.chapter,
            l.state, l.players, l.slots))
        .collect::<Vec<_>>()
        .join("\n")
}

/// JOIN A GAME: keep searching for System Link games while the page is open
/// (only when System Link is set up; see the module docs).
pub(crate) fn browser_search() {
    if CLIENT.lock().unwrap().is_none() && !config_path().exists() {
        return;
    }
    match with_client(|c| c.send(Command::Search)) {
        Ok(()) => {}
        Err(e) => {
            static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                log(&format!("System Link unavailable: {e}"));
            }
        }
    }
}

/// Retail campaign title of a 360 mission code ("L4D2C1").
fn campaign_title(code: &str) -> String {
    let n: Option<u32> = code.to_ascii_uppercase().strip_prefix("L4D2C").and_then(|n| n.parse().ok());
    let title = match n {
        Some(1) => "Dead Center",
        Some(2) => "Dark Carnival",
        Some(3) => "Swamp Fever",
        Some(4) => "Hard Rain",
        Some(5) => "The Parish",
        Some(6) => "The Passing",
        Some(7) => "The Sacrifice",
        Some(8) => "No Mercy",
        Some(9) => "Crash Course",
        Some(10) => "Death Toll",
        Some(11) => "Dead Air",
        Some(12) => "Blood Harvest",
        Some(13) => "Cold Stream",
        _ => "",
    };
    if title.is_empty() { code.to_string() } else { title.to_string() }
}

/// JOIN A GAME rows for the System Link games found (id = session id).
pub(crate) fn browser_games() -> Vec<(u64, crate::game_menu::browser::Advert)> {
    let guard = CLIENT.lock().unwrap();
    let Some(c) = guard.as_ref() else { return Vec::new() };
    let g = c.shared();
    g.lobbies
        .iter()
        .map(|l| {
            let id = u64::from_be_bytes(l.session_id);
            let mut difficulty = l.difficulty.clone();
            if let Some(first) = difficulty.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            let mode = if l.mode == "survival" { "Survival" } else { "Campaign" };
            let art = lobby_art(&l.campaign, l.chapter as i64);
            (id, crate::game_menu::browser::Advert {
                id,
                name: l.host.clone(),
                campaign: art.as_ref().map(|a| a.0.clone()).unwrap_or_else(|| campaign_title(&l.campaign)),
                chapter: art.as_ref().map(|a| a.1.clone()).unwrap_or_else(|| format!("Chapter {}", l.chapter)),
                image: art.map(|a| a.2).unwrap_or_default(),
                difficulty,
                mode: mode.into(),
                players: l.players.max(0) as usize,
                max: l.slots.max(0) as usize,
                playing: l.state == "game",
                port: 0,
                access: 0,
            })
        })
        .collect()
}

/// Survivor name for a 360 lobby avatar.
fn avatar_survivor(avatar: &str) -> &'static str {
    match avatar.to_ascii_lowercase().as_str() {
        "gambler" => "Nick",
        "producer" => "Rochelle",
        "coach" => "Coach",
        "mechanic" => "Ellis",
        "namvet" => "Bill",
        "teengirl" => "Zoey",
        "biker" => "Francis",
        "manager" => "Louis",
        _ => "",
    }
}

fn difficulty_index(d: &str) -> usize {
    match d {
        "easy" => 0,
        "hard" => 2,
        "impossible" => 3,
        _ => 1,
    }
}

/// (campaign, chapter) of a map stem in the co-op or Survival list.
/// The 360's own map order per mission (co-op, Survival), read from the 360
/// game's mission files: a lobby's chapter number counts within it (Dead
/// Center Survival is only c1m4_atrium, so "L4D2C1" chapter 1 is Mall Atrium).
fn mission_map(code: &str, survival: bool, index: usize) -> Option<&'static str> {
    let list: &[&str] = match (code.to_ascii_uppercase().as_str(), survival) {
        ("L4D2C1", false) => &["c1m1_hotel", "c1m2_streets", "c1m3_mall", "c1m4_atrium"],
        ("L4D2C1", true) => &["c1m4_atrium"],
        ("L4D2C2", false) => &["c2m1_highway", "c2m2_fairgrounds", "c2m3_coaster", "c2m4_barns", "c2m5_concert"],
        ("L4D2C2", true) => &["c2m1_highway", "c2m4_barns", "c2m5_concert"],
        ("L4D2C3", false) => &["c3m1_plankcountry", "c3m2_swamp", "c3m3_shantytown", "c3m4_plantation"],
        ("L4D2C3", true) => &["c3m1_plankcountry", "c3m4_plantation"],
        ("L4D2C4", false) => &["c4m1_milltown_a", "c4m2_sugarmill_a", "c4m3_sugarmill_b", "c4m4_milltown_b", "c4m5_milltown_escape"],
        ("L4D2C4", true) => &["c4m1_milltown_a", "c4m2_sugarmill_a"],
        ("L4D2C5", false) => &["c5m1_waterfront", "c5m2_park", "c5m3_cemetery", "c5m4_quarter", "c5m5_bridge"],
        ("L4D2C5", true) => &["c5m2_park", "c5m5_bridge"],
        _ => return None,
    };
    list.get(index).copied()
}

/// (campaign, chapter) of a 360 lobby's campaign code and chapter number in
/// the co-op or Survival list: via the 360's own mission order, else the
/// c<campaign>m<chapter>_ map name.
fn chapter_of_lobby(code: &str, chapter: i64, survival: bool) -> Option<(usize, usize)> {
    let index = usize::try_from(chapter.max(1) - 1).ok()?;
    if let Some(map) = mission_map(code, survival, index) {
        if let Some(found) = chapter_of(map, survival) {
            return Some(found);
        }
    }
    let n: u32 = code.to_ascii_uppercase().strip_prefix("L4D2C")?.parse().ok()?;
    let prefix = format!("c{n}m{}_", chapter.max(1));
    let chapters = if survival { SURVIVAL_CHAPTERS.lock().unwrap() } else { CHAPTERS.lock().unwrap() };
    chapters.iter().enumerate().find_map(|(c, list)| {
        list.iter().position(|s| s.to_ascii_lowercase().starts_with(&prefix)).map(|ch| (c, ch))
    })
}

fn chapter_of(map: &str, survival: bool) -> Option<(usize, usize)> {
    let chapters = if survival { SURVIVAL_CHAPTERS.lock().unwrap() } else { CHAPTERS.lock().unwrap() };
    chapters.iter().enumerate().find_map(|(c, list)| {
        list.iter().position(|s| s.eq_ignore_ascii_case(map)).map(|ch| (c, ch))
    })
}

/// Our own member id in the bridged session (the host is 0).
const ME: u32 = 1;

/// The multiplayer client link to an Xbox host.
pub struct Bridge {
    welcomed: bool,
    started: bool,
    left: bool,
    /// Lobby members / settings last sent (re-sent when the lobby changes).
    sent_members: Value,
    sent_settings: Value,
    last_serial: u64,
    last_sample: Instant,
    outbox: VecDeque<Vec<u8>>,
    horde: Horde,
    /// The Xbox map we last told the game about.
    map: Option<String>,
    /// When the map became known (the start waits briefly for our character).
    map_seen: Option<Instant>,
    /// The game announces its current survivor once as it joins: not a pick.
    survivor_announced: bool,
    /// Our survivor pick in the lobby, shown at once (the host confirms later).
    our_pick: Option<String>,
    /// When we made it: a pick the host has not confirmed after a few seconds
    /// was refused, so the lobby goes back to what the host has.
    our_pick_at: Option<Instant>,
}

/// Our survivor in the Xbox lobby as the host has it ("" if none yet).
pub(crate) fn host_survivor() -> &'static str {
    CLIENT.lock().unwrap().as_ref().map_or("", |c| {
        let g = c.shared();
        g.members.iter().find(|m| m.xuid == g.xuid).map_or("", |m| avatar_survivor(&m.avatar))
    })
}

/// Another member of the Xbox lobby already has this survivor.
pub(crate) fn survivor_taken(name: &str) -> bool {
    CLIENT.lock().unwrap().as_ref().is_some_and(|c| {
        let g = c.shared();
        g.members.iter().any(|m| m.xuid != g.xuid && avatar_survivor(&m.avatar).eq_ignore_ascii_case(name))
    })
}

/// Our survivor as the Xbox assigned it, for the game to load as
/// (multiplayer.rs Event::Start): a different local survivor would leave one
/// Xbox bot undriven and one local bot running its own AI.
static OUR_SURVIVOR: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn assigned_survivor() -> Option<String> {
    OUR_SURVIVOR.lock().unwrap().clone()
}

/// Sequence activity names per model (filled from the animation library by
/// `copy_activities`), so specials and Witches play the 360's activity.
static ACTIVITIES: Mutex<Option<std::collections::HashMap<String, Vec<String>>>> = Mutex::new(None);

fn activity_of(model: &str, sequence: i64) -> Option<String> {
    let guard = ACTIVITIES.lock().unwrap();
    let name = guard.as_ref()?.get(model)?.get(usize::try_from(sequence).ok()?)?;
    (!name.is_empty()).then(|| name.clone())
}

fn copy_activities(animation: Option<Res<crate::l4d2r_animation::AnimationLibrary>>, mut last: Local<Option<Instant>>) {
    let Some(animation) = animation else { return };
    if last.is_some_and(|t| t.elapsed().as_secs_f32() < 1.0) {
        return;
    }
    *last = Some(Instant::now());
    let models: Vec<String> = {
        let guard = CLIENT.lock().unwrap();
        let Some(client) = guard.as_ref() else { return };
        let g = client.shared();
        let Some(s) = g.state.as_ref() else { return };
        s.players.iter().filter(|p| p.team == 3).filter_map(|p| p.model.clone())
            .chain(s.infected.iter().filter(|i| i.class == "Witch").filter_map(|i| i.model.clone()))
            .collect()
    };
    let mut guard = ACTIVITIES.lock().unwrap();
    let map = guard.get_or_insert_with(Default::default);
    for m in models {
        if !map.contains_key(&m) {
            if let Some(list) = animation.sequence_activities(&m) {
                map.insert(m, list);
            }
        }
    }
}

/// The Xbox's infected, as rewrite horde snapshots (net_infected "inf").
#[derive(Default)]
struct Horde {
    /// Xbox entity -> tracked actor.
    tracked: std::collections::HashMap<u32, Tracked>,
    next_id: u32,
    serial: u64,
    last: Option<Instant>,
}

struct Tracked {
    id: u32,
    model: String,
    kind: crate::net_infected::Kind,
    state: crate::net_infected::Infected,
    at: Instant,
}

impl Horde {
    /// One snapshot from the Xbox state (None when it is not time yet).
    fn snapshot(&mut self, gs: &GameState) -> Option<Value> {
        use crate::net_infected::{Death, Infected, Kind, Spawn};
        if self.last.is_some_and(|t| t.elapsed().as_secs_f64() < 1.0 / 15.0) {
            return None;
        }
        let now = Instant::now();
        self.last = Some(now);
        self.serial += 1;
        // Commons, Witches and living special infected (team 3 players).
        let mut actors: Vec<(u32, String, Kind, [f32; 3], f32, f32, bool, i64)> = Vec::new();
        for i in &gs.infected {
            let Some(model) = i.model.clone() else { continue };
            let kind = if i.class == "Witch" { Kind::Witch } else { Kind::Common };
            let health = if kind == Kind::Witch { 1000.0 } else { 50.0 };
            actors.push((i.entity, model, kind, i.origin, i.yaw, health, i.burning, i.sequence));
        }
        for p in gs.players.iter().filter(|p| p.team == 3 && p.alive && !p.ghost) {
            let Some(model) = p.model.clone() else { continue };
            actors.push((p.entity, model, Kind::Special, p.origin, p.yaw, p.health.max(1) as f32, false, p.sequence));
        }
        let mut spawns = Vec::new();
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (entity, model, kind, origin, yaw, health, burning, sequence) in actors {
            seen.insert(entity);
            let position = Vec3::from_array(crate::binary::source_position(origin));
            // A new actor (or a reused entity slot) gets a new id.
            let fresh = self.tracked.get(&entity).is_none_or(|t| t.model != model);
            if fresh {
                self.next_id += 1;
                let state = Infected {
                    id: self.next_id, position, yaw: yaw.to_radians(), health, speed: 0.0, reaction: 0, attack: 0.0,
                    attack_serial: 0, reaction_serial: 0, vertical: 0.0, burning, activity: None, gesture: None,
                    death: None, tongue: None,
                };
                self.tracked.insert(entity, Tracked { id: self.next_id, model: model.clone(), kind, state, at: now });
            }
            let t = self.tracked.get_mut(&entity).unwrap();
            let dt = now.duration_since(t.at).as_secs_f32();
            if dt > 0.0 && !fresh {
                let flat = (position - t.state.position) * Vec3::new(1.0, 0.0, 1.0);
                let speed = flat.length() / dt;
                t.state.speed = t.state.speed * 0.5 + speed.min(12.0) * 0.5;
            }
            t.at = now;
            t.state.position = position;
            t.state.yaw = yaw.to_radians();
            t.state.health = health;
            t.state.burning = burning;
            if kind != Kind::Common {
                t.state.activity = activity_of(&t.model, sequence).or_else(|| t.state.activity.clone());
            }
            if fresh || self.serial % 30 == 0 {
                spawns.push(Spawn { id: t.id, model: t.model.clone(), kind: t.kind, look: None });
            }
            out.push(t.state.clone());
        }
        // Gone from the Xbox: killed (commons become client ragdolls there).
        let gone: Vec<u32> = self.tracked.keys().copied().filter(|e| !seen.contains(e)).collect();
        for e in gone {
            let t = self.tracked.remove(&e).unwrap();
            let mut state = t.state;
            state.health = 0.0;
            state.death = Some(Death { cause: 0, head: false, running: state.speed > 2.0, incoming: Vec3::ZERO });
            out.push(state);
        }
        Some(crate::net_infected::snapshot_json(self.serial, &spawns, &out))
    }
}

impl Bridge {
    pub fn new() -> Bridge {
        Bridge { welcomed: false, started: false, left: false, sent_members: Value::Null, sent_settings: Value::Null, last_serial: 0, last_sample: Instant::now(), outbox: VecDeque::new(), horde: Horde::default(), map: None, map_seen: None, survivor_announced: false, our_pick: None, our_pick_at: None }
    }

    fn push(&mut self, v: Value) {
        self.outbox.push_back(serde_json::to_vec(&v).unwrap_or_default());
    }

    /// Messages we send to the "host" (hello, chat, our samples).
    pub fn queue(&mut self, payload: &[u8]) {
        let Ok(m) = serde_json::from_slice::<Value>(payload) else { return };
        // Our survivor pick in the lobby goes to the Xbox as Game::Avatar
        // (not the announcement as we join, nor anything once the game runs).
        let picked = m.get("t").and_then(Value::as_str) == Some("survivor")
            && std::mem::replace(&mut self.survivor_announced, true)
            && !self.started
            && CLIENT.lock().unwrap().as_ref().is_some_and(|c| c.shared().phase == Phase::InLobby);
        let pick = m.get("survivor").and_then(Value::as_str).unwrap_or("");
        // The 360 lets each survivor be taken once: a taken one is not asked for.
        if picked && survivor_taken(pick) {
            log(&format!("survivor pick {pick}: another player has it"));
        } else if picked {
            self.our_pick = Some(pick.to_string());
            self.our_pick_at = Some(Instant::now());
            let avatar = match m.get("survivor").and_then(Value::as_str).unwrap_or("") {
                "Nick" => "Gambler",
                "Rochelle" => "Producer",
                "Coach" => "Coach",
                "Ellis" => "Mechanic",
                "Bill" => "NamVet",
                "Zoey" => "TeenGirl",
                "Francis" => "Biker",
                "Louis" => "Manager",
                _ => "",
            };
            if !avatar.is_empty() {
                if let Some(c) = CLIENT.lock().unwrap().as_ref() {
                    c.send(Command::Avatar(avatar.to_string()));
                }
            }
        }
        // Our chat goes to the Xbox as the retail say / say_team.
        if m.get("t").and_then(Value::as_str) == Some("chat") {
            let text = m.get("text").and_then(Value::as_str).unwrap_or("").to_string();
            let team = m.get("scope").and_then(Value::as_str) == Some("team");
            if !text.trim().is_empty() {
                if let Some(c) = CLIENT.lock().unwrap().as_ref() {
                    c.send(Command::Say(team, text));
                }
            }
        }
    }

    /// Messages "from the host", generated from the Xbox session.
    pub fn poll(&mut self) -> Option<Vec<Vec<u8>>> {
        let guard = CLIENT.lock().unwrap();
        let Some(client) = guard.as_ref() else { return None };
        let mut g = client.shared();
        if matches!(g.phase, Phase::Failed) || (self.welcomed && g.phase == Phase::Idle) {
            if !self.left {
                self.left = true;
                let reason = g.error.clone().unwrap_or_else(|| "the Xbox host closed the game".into());
                drop(g);
                drop(guard);
                self.push(json!({"t": "refused", "reason": reason}));
                return Some(self.outbox.drain(..).collect());
            }
            return None;
        }
        let our_xuid = g.xuid;
        if self.our_pick_at.is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(6)) {
            self.our_pick_at = None;
            if let Some(pick) = self.our_pick.take() {
                let have = g.members.iter().find(|m| m.xuid == our_xuid).map_or("", |m| avatar_survivor(&m.avatar));
                if have != pick {
                    log(&format!("survivor pick {pick}: the host kept us as {}", if have.is_empty() { "unassigned" } else { have }));
                }
            }
        }
        let difficulty = g.joined.as_ref().map(|l| difficulty_index(&l.difficulty)).unwrap_or(1);
        let survival = g.joined.as_ref().is_some_and(|l| l.mode == "survival");
        // Members as the lobby has them: a survivor only once the player picked
        // one (unpicked shows as "?", like the 360 lobby).
        let mut members = Vec::new();
        let mut next = 2;
        for m in &g.members {
            let id = if m.host { 0 } else if m.xuid == our_xuid { ME } else { next += 1; next - 1 };
            // Ours: the pick we just made, until the host's lobby update agrees.
            let survivor = match (&self.our_pick, avatar_survivor(&m.avatar)) {
                (Some(pick), lobby) if id == ME && lobby != pick.as_str() => pick.clone(),
                (_, lobby) => lobby.to_string(),
            };
            members.push(json!({"id": id, "name": m.name, "survivor": survivor, "steam": 0}));
        }
        if !g.members.iter().any(|m| m.xuid == our_xuid) {
            members.push(json!({"id": ME, "name": crate::game_chat::local_name(), "survivor": "", "steam": 0}));
        }
        let members = Value::Array(members);
        // The map: the running map once known, else the lobby's campaign + chapter.
        let place = g
            .map
            .as_deref()
            .and_then(|m| chapter_of(m, survival))
            .or_else(|| g.joined.as_ref().and_then(|l| chapter_of_lobby(&l.campaign, l.chapter, survival)))
            .unwrap_or((0, 0));
        let settings = json!({"campaign": place.0, "chapter": place.1, "difficulty": difficulty, "sv": survival});
        if !self.welcomed && matches!(g.phase, Phase::InLobby | Phase::Connecting | Phase::SigningOn | Phase::InGame) {
            self.welcomed = true;
            self.sent_members = members.clone();
            self.sent_settings = settings.clone();
            let welcome = json!({"t": "welcome", "id": ME, "members": members, "settings": settings});
            log(&format!("welcome: {} members, survival {survival}, place {place:?}", g.members.len()));
            drop(g);
            drop(guard);
            self.push(welcome);
            return Some(self.outbox.drain(..).collect());
        }
        if self.welcomed && !self.started {
            if members != self.sent_members {
                self.sent_members = members.clone();
                self.push(json!({"t": "members", "members": members}));
            }
            if settings != self.sent_settings {
                self.sent_settings = settings.clone();
                log(&format!("lobby settings: {settings}"));
                self.push(json!({"t": "settings", "settings": settings}));
            }
        }
        // Chat from the Xbox (the host's say, other players).
        let lines = std::mem::take(&mut g.chat_in);
        if !lines.is_empty() {
            let me = crate::game_chat::local_name();
            let mut next = 2;
            let ids: Vec<(String, u32)> = g
                .members
                .iter()
                .map(|m| (m.name.clone(), if m.host { 0 } else if m.xuid == our_xuid { ME } else { next += 1; next - 1 }))
                .collect();
            for (name, text, team) in lines {
                if name == me {
                    continue; // our own line, already shown when sent
                }
                let from = ids.iter().find(|(n, _)| *n == name).map(|(_, id)| *id).unwrap_or(0);
                let shown = if name.is_empty() { "Xbox".to_string() } else { name };
                self.push(json!({"t": "chat", "scope": if team { "team" } else { "all" }, "name": shown, "text": text, "from": from}));
            }
        }
        if !self.started {
            // Our character on the Xbox (once we are a survivor there).
            let ours = g.state.as_ref().and_then(|gs| {
                gs.players.iter().find(|p| p.xuid == our_xuid && p.team == TEAM_SURVIVORS && p.character >= 0)
                    .map(|p| survivor_name(p.character).to_string())
            });
            if g.map.is_some() && self.map_seen.is_none() {
                self.map_seen = Some(Instant::now());
            }
            let waited = self.map_seen.is_some_and(|t| t.elapsed().as_secs_f32() > 15.0);
            if let Some(name) = ours.clone().filter(|n| n != "?") {
                *OUR_SURVIVOR.lock().unwrap() = Some(name);
            }
            let ready = ours.as_deref().is_some_and(|n| n != "?") || waited;
            if let (Some(map), Phase::SigningOn | Phase::InGame, true) = (g.map.clone(), g.phase, ready) {
                if let Some((campaign, chapter)) = chapter_of(&map, survival) {
                    self.started = true;
                    log(&format!("our survivor on the Xbox: {:?}", assigned_survivor()));
                    self.map = Some(map.clone());
                    log(&format!("start: {map} (campaign {campaign}, chapter {chapter}, survival {survival})"));
                    let start = json!({"t": "start", "settings": {"campaign": campaign, "chapter": chapter, "difficulty": difficulty, "sv": survival}});
                    let follow = json!({"t": "g", "from": 0, "m": {"k": "map", "m": map}});
                    drop(g);
                    drop(guard);
                    self.push(start);
                    self.push(follow);
                    return Some(self.outbox.drain(..).collect());
                } else if self.last_serial == 0 {
                    self.last_serial = 1;
                    log(&format!("the host is on {map}, which is not in this game's {} list", if survival { "Survival" } else { "campaign" }));
                }
            }
        }
        // The host changed chapter: the game follows (net_play::follow_map).
        if let (true, Some(map)) = (self.started, g.map.clone()) {
            if self.map.as_deref() != Some(map.as_str()) {
                self.map = Some(map.clone());
                self.horde = Horde::default();
                log(&format!("map change: {map}"));
                self.push(json!({"t": "g", "from": 0, "m": {"k": "map", "m": map}}));
            }
        }
        if self.started && g.state_serial != self.last_serial && self.last_sample.elapsed().as_secs_f32() >= 0.05 {
            self.last_serial = g.state_serial;
            self.last_sample = Instant::now();
            if let Some(gs) = g.state.clone() {
                let ours = g.our_entity;
                // Lobby members with the session ids the welcome gave them (host 0,
                // us ME, others 2, 3, ...), matched to 360 players by xuid or name.
                let mut next = 2;
                let shots = g.shots.clone();
                let humans: Vec<Human> = g
                    .members
                    .iter()
                    .map(|m| {
                        let id = if m.host { 0 } else if m.xuid == our_xuid { ME } else { next += 1; next - 1 };
                        Human { xuid: m.xuid, name: m.name.clone(), id }
                    })
                    .collect();
                drop(g);
                drop(guard);
                for msg in samples(&gs, ours, our_xuid, &humans, &shots) {
                    self.push(msg);
                }
                if let Some(m) = self.horde.snapshot(&gs) {
                    self.push(json!({"t": "g", "from": 0, "m": m}));
                }
            }
        }
        Some(self.outbox.drain(..).collect())
    }
}

impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}

fn source_aim(pitch: f32, yaw: f32) -> Vec3 {
    let (p, y) = (pitch.to_radians(), yaw.to_radians());
    let f = [p.cos() * y.cos(), p.cos() * y.sin(), -p.sin()];
    Vec3::from_array(crate::binary::source_normal(f))
}

/// Rewrite "p" samples for every Xbox survivor except ours.
/// A lobby member and its session id.
struct Human {
    xuid: u64,
    name: String,
    id: u32,
}

fn samples(gs: &GameState, ours: Option<u32>, our_xuid: u64, humans: &[Human],
           shots: &std::collections::HashMap<i32, u64>) -> Vec<Value> {
    let mut out = Vec::new();
    for p in gs.players.iter().filter(|p| p.team == TEAM_SURVIVORS) {
        if Some(p.entity) == ours || (p.xuid != 0 && p.xuid == our_xuid) {
            continue;
        }
        let survivor = survivor_name(p.character);
        if survivor == "?" {
            continue;
        }
        let names: Vec<String> = p.weapons.iter().map(|c| weapon_name(c)).collect();
        let primary = names.iter().find(|n| weapon_slot(n) == 0).cloned();
        let pistol = names.iter().find(|n| weapon_slot(n) == 1).cloned().unwrap_or_else(|| "weapon_pistol".into());
        let active = p.active_weapon.as_deref().map(weapon_name);
        let slot = active.as_deref().map(|n| weapon_slot(n) + 1).unwrap_or(2);
        let pinned = p.pinned_by.and_then(|e| gs.players.iter().find(|q| q.entity == e)).map(|q| q.class.clone());
        let mut m = json!({
            "k": "p",
            "s": survivor,
            "p": crate::binary::source_position(p.origin),
            "a": source_aim(p.pitch, p.yaw).to_array(),
            "h": p.health,
            "t": p.temp_health,
            "i": p.incapacitated,
            "d": !p.alive,
            "w": slot,
            "g1": primary,
            "g2": pistol,
            "f": shots.get(&p.user_id).copied().unwrap_or(0),
            "pn": pinned,
            "lg": if p.hanging { json!([0.0, p.health]) } else { Value::Null },
            "c": p.ducked,
        });
        // The userinfo xuid can differ from the lobby's (online vs offline xuid),
        // so a name match also counts; 360 bots are named after their survivor.
        let human = humans.iter().find(|h| (h.xuid != 0 && h.xuid == p.xuid) || (!h.name.is_empty() && h.name == p.name));
        let from = match human {
            Some(h) => h.id,
            None => {
                m["b"] = json!(true);
                0
            }
        };
        out.push(json!({"t": "g", "from": from, "m": m}));
    }
    out
}

/// Keeps the catalog's chapter list available to the bridge.
fn copy_chapters(catalog: Option<Res<crate::campaign_settings::Catalog>>) {
    let Some(catalog) = catalog else { return };
    if !catalog.is_changed() {
        return;
    }
    let list: Vec<Vec<String>> = catalog
        .campaigns
        .iter()
        .map(|c| c.chapters.iter().map(|ch| ch.map.file_stem().unwrap_or_default().to_string_lossy().to_string()).collect())
        .collect();
    *CHAPTERS.lock().unwrap() = list;
    remember_art(&catalog.campaigns);
}

/// Loads the Survival map list once (the catalog only holds one mode at a time).
fn copy_survival(source: Option<Res<crate::game_ui::HudSource>>, mut done: Local<bool>) {
    let Some(source) = source else { return };
    if *done {
        return;
    }
    *done = true;
    let survival = crate::campaign_settings::Catalog::survival(&source.0);
    remember_art(&survival);
    let list: Vec<Vec<String>> = survival
        .iter()
        .map(|c| c.chapters.iter().map(|ch| ch.map.file_stem().unwrap_or_default().to_string_lossy().to_string()).collect())
        .collect();
    *SURVIVAL_CHAPTERS.lock().unwrap() = list;
}

/// --xbox-join: find System Link games at start-up and join the first one
/// (unattended testing).
fn auto_join(time: Res<Time<Real>>, mut stage: Local<u8>, mut at: Local<f64>, mut hosted: Local<u8>) {
    // --xbox-host: host a System Link game at start-up (unattended testing).
    if *hosted < 2 && std::env::args().any(|a| a == "--xbox-host") {
        let now = time.elapsed_secs_f64();
        if *hosted == 0 && now > 5.0 {
            *hosted = 1;
            log(&command("xbox_host").unwrap_or_default());
        } else if *hosted == 1 && now > 15.0 {
            *hosted = 2;
            log(&command("xbox_host_status").unwrap_or_default());
        }
    }
    if *stage == 2 || !std::env::args().any(|a| a == "--xbox-join") {
        return;
    }
    let now = time.elapsed_secs_f64();
    if *stage == 0 && now > 5.0 {
        log(&command("xbox_find").unwrap_or_default());
        *stage = 1;
        *at = now;
    } else if *stage == 1 && now - *at > 3.0 {
        *at = now;
        let found = CLIENT.lock().unwrap().as_ref().map(|c| c.shared().lobbies.len()).unwrap_or(0);
        if found > 0 {
            log(&command("xbox_join 0").unwrap_or_default());
            *stage = 2;
        } else {
            log("auto-join: no games yet");
        }
    }
}

/// Source view angles (degrees) of a Bevy-space yaw/pitch.
fn source_angles(yaw: f32, pitch: f32) -> (f32, f32) {
    let aim = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.) * Vec3::NEG_Z;
    // Bevy (x, y up, z) to Source (x, -z, y up).
    let (sx, sy, sz) = (aim.x, -aim.z, aim.y);
    let p = -sz.clamp(-1., 1.).asin().to_degrees();
    let y = sy.atan2(sx).to_degrees();
    (p, y)
}

/// Sends our player's input to the Xbox as user commands, and pulls our
/// survivor back to the server's position when they drift apart.
fn send_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    frontend: Res<crate::game_menu::Frontend>,
    mut body: Option<Single<&mut crate::game::Body>>,
    mut last_log: Local<f64>,
    time: Res<Time<Real>>,
    state: Option<Res<crate::game_state::Match>>,
    mut last_slot: Local<u8>,
    mut history: Local<VecDeque<(f64, Vec3)>>,
) {
    let guard = CLIENT.lock().unwrap();
    let Some(client) = guard.as_ref() else { return };
    let (phase, server_pos, me) = {
        let g = client.shared();
        let me = g.our_entity.and_then(|e| g.state.as_ref()?.players.iter().find(|p| p.entity == e).cloned());
        // Our exact origin (localdata): the cell origin's in-cell offset is not
        // sent to us, so it sits on a 32-unit grid (rubberbanding, 0.8 m too low).
        let pos = me.as_ref().filter(|p| p.alive && p.team == TEAM_SURVIVORS).map(|p| p.precise_origin.unwrap_or(p.origin));
        let roster: Vec<String> = g.state.as_ref().map(|s| s.players.iter().map(|p| format!("{}:{}:t{}:{}:hp{}:({:.0},{:.0},{:.0})", p.entity, p.name, p.team, if p.alive { "alive" } else { "dead" }, p.health, p.origin[0], p.origin[1], p.origin[2]) + &format!(":idle{}:obs{}->{:?}", p.idle_human, p.observer_mode, p.observer_target)).collect()).unwrap_or_default();
        (g.phase, pos, me.map(|m| (m, roster)))
    };
    if phase != Phase::InGame {
        return;
    }
    let Some(body) = body.as_deref_mut() else { return };
    let (pitch, yaw) = source_angles(body.yaw, body.pitch);
    // In a menu (pause, scoreboard...): stand still on the Xbox. The client
    // repeats our last command, so a held key would otherwise keep going.
    if frontend.page != crate::game_menu::Page::Game {
        client.send(Command::Input(l4d2r_xbox_link::game::UserCmd {
            command_number: 0,
            tick_count: 0,
            view: [pitch, yaw, 0.0],
            forward: 0.0,
            side: 0.0,
            up: 0.0,
            buttons: 0,
            weapon_select: 0,
            weapon_subtype: 0,
        }));
        return;
    }
    let axis = |pos: KeyCode, neg: KeyCode| (keys.pressed(pos) as i32 - keys.pressed(neg) as i32) as f32 * 450.0;
    let mut forward = axis(KeyCode::KeyW, KeyCode::KeyS);
    // --xbox-walk-test: walk forward 3 s out of every 10 (unattended testing).
    let walk_test = std::env::args().any(|a| a == "--xbox-walk-test");
    if walk_test && time.elapsed_secs_f64() % 10.0 < 3.0 {
        forward = 450.0;
    }
    let side = axis(KeyCode::KeyD, KeyCode::KeyA);
    let mut buttons = 0u32;
    let mut set = |on: bool, bit: u32| {
        if on {
            buttons |= bit
        }
    };
    set(mouse.pressed(MouseButton::Left), 1 << 0); // IN_ATTACK
    set(keys.pressed(KeyCode::Space), 1 << 1); // IN_JUMP
    set(keys.pressed(KeyCode::ControlLeft), 1 << 2); // IN_DUCK
    set(forward > 0.0, 1 << 3); // IN_FORWARD
    set(forward < 0.0, 1 << 4); // IN_BACK
    set(keys.pressed(KeyCode::KeyE), 1 << 5); // IN_USE
    set(side < 0.0, 1 << 9); // IN_MOVELEFT
    set(side > 0.0, 1 << 10); // IN_MOVERIGHT
    set(mouse.pressed(MouseButton::Right), 1 << 11); // IN_ATTACK2 (shove)
    set(keys.pressed(KeyCode::KeyR), 1 << 13); // IN_RELOAD
    // Idle/late-joined spectators take over their survivor bot by pressing
    // Attack (retail "press to take over"); pulse it until we are back in.
    if me.as_ref().is_some_and(|(p, _)| p.team == 1) {
        // Attack, then Jump, then Use (the 360 prompt's button is unconfirmed).
        let t = time.elapsed_secs_f64() % 3.0;
        set(t < 0.25, 1 << 0);
        set((1.0..1.25).contains(&t), 1 << 1);
        set((2.0..2.25).contains(&t), 1 << 5);
    }
    client.send(Command::Input(l4d2r_xbox_link::game::UserCmd {
        command_number: 0,
        tick_count: 0,
        view: [pitch, yaw, 0.0],
        forward,
        side,
        up: 0.0,
        buttons,
        weapon_select: 0,
        weapon_subtype: 0,
    }));
    // Weapon switch: the local slot changed; select the Xbox weapon entity for it.
    if let (Some(state), Some((p, _))) = (state.as_deref(), me.as_ref()) {
        if state.slot != *last_slot {
            *last_slot = state.slot;
            let pick = p.weapon_ids.iter().find(|(_, c)| weapon_slot(&weapon_name(c)) + 1 == state.slot);
            if let Some((entity, class)) = pick {
                client.send(Command::SelectWeapon(*entity));
                log(&format!("weapon switch: slot {} -> {class} (entity {entity})", state.slot));
            }
        }
    }
    {
        static LAST_US: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);
        let now = time.elapsed_secs_f64();
        let mut last = LAST_US.lock().unwrap();
        if now - *last > 2.0 {
            *last = now;
            match &me {
                Some((p, roster)) => log(&format!("us: entity {} team {} alive {} hp {} class {} at ({:.0}, {:.0}, {:.0}); players {}", p.entity, p.team,
                    p.alive, p.health, p.class, p.origin[0], p.origin[1], p.origin[2], roster.join(" "))),
                None => log("us: not in the host's player list yet"),
            }
        }
    }
    // Reconcile: the Xbox owns our survivor's position. Its position is a
    // round trip old, so compare it with where we were recently (not where we
    // are now), and correct only real disagreement.
    let now_t = time.elapsed_secs_f64();
    history.push_back((now_t, body.feet));
    while history.front().is_some_and(|(t, _)| now_t - t > 1.0) {
        history.pop_front();
    }
    if let Some(origin) = server_pos {
        let server = Vec3::from_array(crate::binary::source_position(origin));
        let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
        // The past position of ours closest to the server's.
        let (past, error) = history
            .iter()
            .map(|(_, p)| (*p, flat(*p - server).length()))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or((body.feet, flat(body.feet - server).length()));
        let vertical = (past.y - server.y).abs();
        let moving = forward != 0.0 || side != 0.0;
        if error > 2.0 || vertical > 2.0 {
            // Real disagreement (blocked, teleported, pushed): shift by the
            // difference so movement since then is kept; height only when far off.
            let mut shift = flat(server - past);
            if vertical > 2.0 {
                shift.y = server.y - past.y;
                body.velocity = Vec3::ZERO;
            }
            body.feet += shift;
            history.clear();
        } else if error > 0.05 && !moving {
            // Standing still: ease sideways onto the server position.
            let k = (time.delta_secs() * 4.0).min(1.0);
            let target = Vec3::new(server.x, body.feet.y, server.z);
            body.feet = body.feet.lerp(target, k);
        }
        let now = time.elapsed_secs_f64();
        if now - *last_log > if walk_test { 1.0 } else { 5.0 } {
            *last_log = now;
            log(&format!("server origin ({:.0}, {:.0}, {:.0})", origin[0], origin[1], origin[2]));

            log(&format!("input: view ({pitch:.0}, {yaw:.0}) move ({forward:.0}, {side:.0}) buttons {buttons:#x}; position error {error:.2} m"));
        }
    }
}

/// Doors: the Xbox decides whether each door is open; ours follows. 360 entity
/// indices differ from the BSP's, so doors are matched by position.
fn sync_doors(
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    collision: Option<Res<crate::collision::Collision>>,
    mut matched: Local<std::collections::HashMap<u32, Option<usize>>>,
    mut generation: Local<(u64, Option<String>)>,
    mut last: Local<Option<Instant>>,
) {
    use l4d2r_game_core::entities::{DoorState, Kind};
    let (Some(logic), Some(collision)) = (logic.as_mut(), collision) else { return };
    if last.is_some_and(|t| t.elapsed().as_secs_f32() < 0.2) {
        return;
    }
    *last = Some(Instant::now());
    let (doors, map) = {
        let guard = CLIENT.lock().unwrap();
        let Some(client) = guard.as_ref() else { return };
        let g = client.shared();
        if g.phase != Phase::InGame {
            return;
        }
        (g.state.as_ref().map(|s| s.doors.clone()).unwrap_or_default(), g.map.clone())
    };
    if generation.0 != logic.generation || generation.1 != map {
        *generation = (logic.generation, map);
        matched.clear();
    }
    for door in doors {
        let index = *matched.entry(door.entity).or_insert_with(|| {
            let at = l4d2r_math::Vec3::new(door.origin[0], door.origin[1], door.origin[2]);
            logic
                .world
                .ents
                .iter()
                .enumerate()
                .filter(|(_, e)| e.alive && matches!(e.kind, Kind::Door(_)) && e.me.classname.starts_with("prop_door"))
                .map(|(i, e)| (i, (e.me.origin - at).length()))
                .filter(|(_, d)| *d < 24.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        });
        let Some(index) = index else { continue };
        let Kind::Door(d) = &logic.world.ents[index].kind else { continue };
        let xbox_open = matches!(door.state, 1 | 2 | 4);
        // Only settled doors: a moving one finishes first.
        let ours = match d.state {
            DoorState::Closed => false,
            DoorState::Open => true,
            _ => continue,
        };
        if ours != xbox_open {
            log(&format!("door {} ({}): Xbox {}", index, door.class, if xbox_open { "opened" } else { "closed" }));
            logic.remote_use(index, None, &collision);
        }
    }
}

pub fn install(app: &mut App) {
    crate::voice_link::install(app);
    app.add_systems(Update, (sync_doors, copy_activities, feed_host, feed_host_horde, feed_host_doors, collide_consoles, update_keep_running, vote_panel, voice_io, leave_on_exit, forward_notes));
    app.add_systems(Update, (copy_chapters, copy_survival, auto_join, send_input));
}
