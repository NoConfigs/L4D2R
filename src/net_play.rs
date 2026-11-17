//! Multiplayer stage 2a: other players' survivors in your game.
//!
//! Every game sends its own player's state 20 times a second (position,
//! view, health, downed/dead, held weapon) over the session's game channel
//! (`SessionCore::game`; the host relays). Each receiver finds the squad bot
//! playing that survivor, switches its AI off (`game_state::set_remote`) and
//! moves it along the received states 100 ms behind, so the motion is
//! smooth. Whoever plays a survivor owns its state.
//!
//! The host also sends its survivor bots, so guests draw the same bots
//! instead of running their own. The infected are shared by
//! `net_infected` (host authoritative); this file forwards the damage, pins
//! and revives the host's game applies to a remote player's survivor.
use crate::campaign_settings::Survivor;
use crate::game_state::{Gun, Squad};
use crate::multiplayer::Session;
use bevy::prelude::*;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};

/// Sends per second.
const RATE: f64 = 20.;
/// Remote players are drawn this far in the past, between two received
/// states.
const DELAY: f64 = 0.1;
/// A player we stop hearing from goes back to its bot after this long.
const TIMEOUT: f64 = 3.;
/// Track keys for the host's survivor bots (plus the squad index); member
/// ids stay below this.
const BOT_KEY: u32 = 1000;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sample {
    pub position: Vec3,
    pub aim: Vec3,
    pub health: f32,
    pub temporary: f32,
    pub incap: bool,
    pub dead: bool,
    pub slot: u8,
    pub primary: Option<String>,
    pub pistol: String,
    /// Shots fired so far (Match::shot_serial).
    pub shots: u64,
    /// The special holding them (Match::pinned_by while pinned).
    pub pinned_by: Option<crate::special_infected::SpecialClass>,
    /// Hanging from a ledge: (seconds since the grab, hang health). The
    /// aim then points at the wall, so others draw the hang facing it.
    pub hang: Option<(f32, f32)>,
}

impl Sample {
    pub(crate) fn to_json(&self, survivor: &str) -> Value {
        let r = |v: f32| (v * 1000.).round() / 1000.;
        json!({
            "k": "p",
            "s": survivor,
            "p": [r(self.position.x), r(self.position.y), r(self.position.z)],
            "a": [r(self.aim.x), r(self.aim.y), r(self.aim.z)],
            "h": r(self.health),
            "t": r(self.temporary),
            "i": self.incap,
            "d": self.dead,
            "w": self.slot,
            "g1": self.primary,
            "g2": self.pistol,
            "f": self.shots,
            "pn": self.pinned_by.map(|c| c.name()),
            "lg": self.hang.map(|(t, h)| [r(t), r(h)]),
        })
    }
    pub(crate) fn from_json(m: &Value) -> Option<(String, Self)> {
        if m.get("k")?.as_str()? != "p" {
            return None;
        }
        let v3 = |k: &str| -> Option<Vec3> {
            let a = m.get(k)?.as_array()?;
            Some(Vec3::new(a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32))
        };
        let f = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.) as f32;
        let b = |k: &str| m.get(k).and_then(Value::as_bool).unwrap_or(false);
        let position = v3("p")?;
        if !position.is_finite() {
            return None;
        }
        Some((
            m.get("s")?.as_str()?.to_string(),
            Self {
                position,
                aim: v3("a").filter(|a| a.is_finite()).unwrap_or(Vec3::X),
                health: f("h"),
                temporary: f("t"),
                incap: b("i"),
                dead: b("d"),
                slot: m.get("w").and_then(Value::as_u64).unwrap_or(1).min(8) as u8,
                primary: m.get("g1").and_then(Value::as_str).map(str::to_string),
                pistol: m.get("g2").and_then(Value::as_str).unwrap_or("weapon_pistol").to_string(),
                shots: m.get("f").and_then(Value::as_u64).unwrap_or(0),
                pinned_by: m.get("pn").and_then(Value::as_str).and_then(crate::special_infected::SpecialClass::parse),
                hang: m.get("lg").and_then(Value::as_array).and_then(|a| {
                    let t = a.first()?.as_f64()? as f32;
                    let h = a.get(1)?.as_f64()? as f32;
                    (t.is_finite() && h.is_finite()).then_some((t.max(0.), h.max(0.)))
                }),
            },
        ))
    }

    /// The state between two samples (positions and aims blend; the rest
    /// switches at the later one).
    fn blend(&self, next: &Self, t: f32) -> Self {
        let aim = self.aim.lerp(next.aim, t);
        Self {
            position: self.position.lerp(next.position, t),
            aim: aim.try_normalize().unwrap_or(next.aim),
            ..next.clone()
        }
    }
}

/// Received states for one remote player, oldest first.
#[derive(Default)]
struct Track {
    survivor: String,
    samples: VecDeque<(f64, Sample)>,
}

impl Track {
    /// The state to draw at `at` (receive-time clock).
    fn sample(&self, at: f64) -> Option<Sample> {
        let (first_t, first) = self.samples.front()?;
        if at <= *first_t {
            return Some(first.clone());
        }
        for pair in self.samples.iter().collect::<Vec<_>>().windows(2) {
            let ((t0, a), (t1, b)) = (pair[0], pair[1]);
            if at >= *t0 && at <= *t1 {
                let t = if t1 > t0 { ((at - t0) / (t1 - t0)) as f32 } else { 1. };
                return Some(a.blend(b, t));
            }
        }
        self.samples.back().map(|(_, s)| s.clone())
    }
}

/// Whether we are a guest in a running online game (the host leads chapter
/// changes; campaign_flow skips its own on guests).
static GUEST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn guest() -> bool {
    GUEST.load(std::sync::atomic::Ordering::Relaxed)
}
/// Whether we host a running online game with at least one guest.
static HOST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn host() -> bool {
    HOST.load(std::sync::atomic::Ordering::Relaxed)
}

/// What the host last wrote into a remote player's survivor, to tell local
/// damage, pins and revives apart from the owner's updates.
#[derive(Clone, Copy, Default)]
struct Applied {
    serial: u64,
    health: f32,
    temporary: f32,
    incap: bool,
    incap_health: f32,
    pinned: bool,
    vomited: f32,
}

/// Damage taken by a survivor since `before`, in `Match::damage` units
/// (health plus temporary health while standing, then incap health).
fn damage_since(before: &Applied, now: &crate::game_state::Match) -> f32 {
    let standing = before.health + before.temporary;
    let amount = if before.incap {
        before.incap_health - now.incap_health
    } else if now.incap || now.dead {
        standing + (300. - now.incap_health).max(0.)
    } else {
        standing - (now.health + now.temporary)
    };
    amount.max(0.)
}

#[derive(Resource, Default)]
pub(crate) struct NetPlay {
    /// The chapter change we announced (host) or followed (guest).
    announced: Option<std::path::PathBuf>,
    tracks: HashMap<u32, Track>,
    /// Shots already sounded per remote player.
    heard: HashMap<u32, u64>,
    last_send: f64,
    /// Received gameplay messages of other kinds (net_infected reads them).
    pub(crate) other: Vec<(u32, Value)>,
    /// Messages queued by fixed-tick systems; `exchange` sends them.
    pub(crate) outgoing: Vec<Value>,
    /// Game-flow messages (map uses, mission lost, restart, finale won;
    /// net_flow reads them).
    pub(crate) flow: Vec<(u32, Value)>,
    host: bool,
    /// Host: per squad index, what `apply` last wrote.
    applied: HashMap<usize, Applied>,
    /// Host: a revive sent for this squad index; the survivor stays up
    /// locally until the owner's update agrees.
    revived: HashMap<usize, f64>,
    /// Host: when revive progress was last sent per squad index.
    progress_sent: HashMap<usize, f64>,
}

/// Guest: our revive progress in the host's game (seconds) and when it
/// arrived; it lapses if no update follows.
static REVIVE_PROGRESS: std::sync::Mutex<Option<(f32, std::time::Instant)>> = std::sync::Mutex::new(None);
pub(crate) fn set_revive_progress(seconds: f32) {
    *REVIVE_PROGRESS.lock().unwrap() = Some((seconds, std::time::Instant::now()));
}
/// Our revive progress in the host's game, if being revived right now.
pub(crate) fn revive_progress() -> f32 {
    match *REVIVE_PROGRESS.lock().unwrap() {
        Some((seconds, at)) if guest() && at.elapsed().as_secs_f32() < 0.4 => seconds,
        _ => 0.,
    }
}

impl NetPlay {
    /// Squad index of the survivor member `id` plays.
    pub(crate) fn member_index(&self, squad: &Squad, id: u32) -> Option<usize> {
        squad_index(squad, &self.tracks.get(&id)?.survivor)
    }
}

/// Squad index of the bot playing `survivor` (by its model).
fn squad_index(squad: &Squad, survivor: &str) -> Option<usize> {
    let character = Survivor::parse(survivor)?.character();
    let model = format!("survivor_{character}.mdl");
    squad.0.iter().position(|m| m.model.ends_with(&model))
}

fn in_game(frontend: &crate::game_menu::Frontend, session: &Session) -> bool {
    frontend.page == crate::game_menu::Page::Game && session.core.online()
}

/// Chapter changes: the host announces the map it is changing to; guests
/// load the same one, carrying their own player and bots like a normal
/// change.
#[allow(clippy::too_many_arguments)]
fn follow_map(
    net: &mut NetPlay,
    session: &mut Session,
    inbox: &[(u32, Value)],
    requests: Option<&mut crate::map_transition::Requests>,
    state: Option<&mut crate::game_state::Match>,
    squad: Option<&Squad>,
    settings: Option<&crate::campaign_settings::SessionSettings>,
    catalog: Option<&crate::campaign_settings::Catalog>,
    source: Option<&crate::game_ui::HudSource>,
) {
    let Some(requests) = requests else { return };
    if session.core.is_host() {
        match &requests.0 {
            Some((_, next)) if net.announced.as_ref() != Some(&next.map) => {
                net.announced = Some(next.map.clone());
                let stem = next.map.file_stem().unwrap_or_default().to_string_lossy().to_string();
                println!("Net play: chapter change to {stem}, guests follow");
                session.core.game(json!({"k": "map", "m": stem}));
            }
            None => net.announced = None,
            _ => {}
        }
        return;
    }
    let wanted = inbox.iter().rev().find_map(|(_, m)| {
        (m.get("k")?.as_str()? == "map").then(|| m.get("m")?.as_str().map(str::to_string)).flatten()
    });
    let (Some(stem), Some(state), Some(squad), Some(settings), Some(catalog), Some(source)) =
        (wanted, state, squad, settings, catalog, source)
    else {
        return;
    };
    let Some(path) = catalog
        .campaigns
        .iter()
        .flat_map(|c| &c.chapters)
        .find(|ch| ch.map.file_stem().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&stem)))
        .map(|ch| ch.map.clone())
    else {
        println!("Net play: the host changed to {stem}, which is not installed here");
        return;
    };
    if requests.0.is_some() || settings.map == path {
        return;
    }
    println!("Net play: following the host to {stem}");
    let mut next = settings.clone();
    next.map = path.clone();
    requests.1 = Some(crate::campaign_flow::Carry {
        player: state.clone(),
        bots: squad.0.iter().map(|m| m.state.clone()).collect(),
    });
    requests.0 = Some((source.0.clone(), next));
    state.paused = true;
    net.announced = Some(path);
}

/// Update: sends our state and stores what arrived.
#[allow(clippy::too_many_arguments)]
pub(crate) fn exchange(
    mut net: ResMut<NetPlay>,
    mut session: ResMut<Session>,
    frontend: Res<crate::game_menu::Frontend>,
    time: Res<Time<Real>>,
    mut state: Option<ResMut<crate::game_state::Match>>,
    body: Option<Single<&crate::game::Body>>,
    settings: Option<Res<crate::campaign_settings::SessionSettings>>,
    world: (
        Option<ResMut<crate::map_transition::Requests>>,
        Option<Res<Squad>>,
        Option<Res<crate::campaign_settings::Catalog>>,
        Option<Res<crate::game_ui::HudSource>>,
    ),
) {
    let inbox = std::mem::take(&mut session.inbox);
    let playing = in_game(&frontend, &session);
    GUEST.store(playing && session.core.is_client(), std::sync::atomic::Ordering::Relaxed);
    net.host = playing && session.core.is_host();
    HOST.store(net.host && session.core.members.len() > 1, std::sync::atomic::Ordering::Relaxed);
    if !playing {
        net.tracks.clear();
        net.announced = None;
        net.other.clear();
        net.outgoing.clear();
        net.flow.clear();
        net.applied.clear();
        return;
    }
    for message in std::mem::take(&mut net.outgoing) {
        session.core.game(message);
    }
    let (mut requests, squad, catalog, source) = world;
    follow_map(
        &mut net,
        &mut session,
        &inbox,
        requests.as_deref_mut(),
        state.as_deref_mut(),
        squad.as_deref(),
        settings.as_deref(),
        catalog.as_deref(),
        source.as_deref(),
    );
    let now = time.elapsed_secs_f64();
    for (from, message) in inbox {
        let kind = message.get("k").and_then(Value::as_str).unwrap_or("");
        if matches!(kind, "use" | "lost" | "restart" | "won" | "spit" | "acid" | "rocks" | "gore" | "blood" | "surv") {
            net.flow.push((from, message));
            continue;
        }
        if kind != "p" && kind != "map" {
            net.other.push((from, message));
            continue;
        }
        let Some((survivor, sample)) = Sample::from_json(&message) else { continue };
        // The host's survivor bots: one track per bot.
        let key = if message.get("b").and_then(Value::as_bool).unwrap_or(false) {
            let Some(index) = squad.as_deref().and_then(|q| squad_index(q, &survivor)) else { continue };
            if from != 0 {
                continue;
            }
            BOT_KEY + index as u32
        } else {
            from
        };
        let track = net.tracks.entry(key).or_default();
        track.survivor = survivor;
        track.samples.push_back((now, sample));
        while track.samples.len() > 2 && track.samples[1].0 < now - DELAY - 0.5 {
            track.samples.pop_front();
        }
    }
    net.tracks.retain(|_, t| t.samples.back().is_some_and(|(at, _)| now - at < TIMEOUT));
    if now - net.last_send < 1. / RATE {
        return;
    }
    let (Some(state), Some(body), Some(settings)) = (state, body, settings) else { return };
    if state.infected {
        return;
    }
    net.last_send = now;
    let mut aim = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    // Hanging: face the wall (others draw the hang along the aim).
    if let Some(wall) = state.ledge.and_then(|h| ((h.stand - h.hang) * Vec3::new(1., 0., 1.)).try_normalize()) {
        aim = wall;
    }
    let sample = Sample {
        position: body.feet,
        aim,
        health: state.health,
        temporary: state.temporary,
        incap: state.incap,
        dead: state.dead,
        slot: state.slot,
        primary: state.primary.as_ref().map(|g| g.class.clone()),
        pistol: state.pistol.class.clone(),
        shots: state.shot_serial,
        pinned_by: if state.pinned { state.pinned_by } else { None },
        hang: state.ledge.map(|h| ((state.elapsed - h.grabbed).max(0.), state.incap_health)),
    };
    if std::env::var_os("L4D2R_NET_DEBUG").is_some() && (now * 20.) as u64 % 40 == 0 {
        println!("Net play: sending {} at {:.2?}", settings.survivor.name(), sample.position);
    }
    session.core.game(sample.to_json(settings.survivor.name()));
    // The host's bots, so every game shows (and fights with) the same ones.
    if net.host {
        if let Some(squad) = squad.as_deref() {
            for (index, m) in squad.0.iter().enumerate() {
                if crate::game_state::remote(index) {
                    continue;
                }
                let Some(name) = bot_survivor(&m.model) else { continue };
                let s = &m.state;
                let mut message = Sample {
                    position: m.position,
                    aim: m.aim,
                    health: s.health,
                    temporary: s.temporary,
                    incap: s.incap,
                    dead: s.dead,
                    slot: s.slot,
                    primary: s.primary.as_ref().map(|g| g.class.clone()),
                    pistol: s.pistol.class.clone(),
                    shots: s.shot_serial,
                    pinned_by: if s.pinned { s.pinned_by } else { None },
                    hang: s.ledge.map(|h| ((s.elapsed - h.grabbed).max(0.), s.incap_health)),
                }
                .to_json(name);
                message["b"] = json!(true);
                session.core.game(message);
            }
        }
    }
}

/// Survivor name of a squad bot's model (".../survivor_gambler.mdl" is Nick).
fn bot_survivor(model: &str) -> Option<&'static str> {
    ["Nick", "Rochelle", "Coach", "Ellis", "Bill", "Zoey", "Louis", "Francis"].into_iter().find(|name| {
        Survivor::parse(name).is_some_and(|s| model.ends_with(&format!("survivor_{}.mdl", s.character())))
    })
}

fn gun(class: &str, old: Option<&Gun>) -> Gun {
    match old {
        Some(g) if g.class == class => g.clone(),
        _ => Gun {
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
            class: class.to_string(),
            ammo: 1,
            reserve: 0,
        },
    }
}

/// Fixed tick: drives the remote players' squad members.
fn apply(
    mut net: ResMut<NetPlay>,
    squad: Option<ResMut<Squad>>,
    time: Res<Time<Real>>,
    rules: Option<Res<crate::game_state::Rules>>,
    mut sounds: Option<ResMut<crate::game_audio::Queue>>,
) {
    let Some(mut squad) = squad else {
        crate::game_state::set_remote(0);
        return;
    };
    let at = time.elapsed_secs_f64() - DELAY;
    let mut mask = 0u8;
    let net = &mut *net;
    net.heard.retain(|id, _| net.tracks.contains_key(id));
    let now = time.elapsed_secs_f64();
    net.revived.retain(|_, until| *until > now);
    for (id, track) in &net.tracks {
        let Some(index) = squad_index(&squad, &track.survivor) else { continue };
        let Some(s) = track.sample(at) else { continue };
        mask |= 1 << index.min(7);
        let member = &mut squad.0[index];
        // Host: what our game did to their survivor since the last write
        // goes to its owner (infected damage, pins, revives).
        let owned = net.host && *id < BOT_KEY;
        let pinned = owned && member.state.pinned;
        if owned {
            let m = &member.state;
            if let Some(before) = net.applied.get(&index) {
                if m.damage_serial != before.serial {
                    let amount = damage_since(before, m);
                    if amount > 0. {
                        net.outgoing.push(json!({"k": "dmg", "s": track.survivor, "a": (amount * 100.).round() / 100.}));
                    }
                } else if before.incap && !m.incap && !m.dead && !net.revived.contains_key(&index) {
                    net.outgoing.push(json!({"k": "rev", "s": track.survivor}));
                    net.revived.insert(index, now + 2.);
                }
                // Being revived in our game: the progress, so the owner sees
                // their own revive (the ledge climb) while it happens.
                if m.incap && !m.dead && member.revive_time > 0.
                    && net.progress_sent.get(&index).is_none_or(|t| now - t >= 0.1)
                {
                    net.outgoing.push(json!({"k": "rvp", "s": track.survivor, "t": (member.revive_time * 100.).round() / 100.}));
                    net.progress_sent.insert(index, now);
                }
                if pinned {
                    let p = member.position;
                    net.outgoing.push(json!({
                        "k": "pin", "s": track.survivor,
                        "by": m.pinned_by.map(|c| c.name()),
                        "f": m.pin_allows_fire,
                        "p": [(p.x * 100.).round() / 100., (p.y * 100.).round() / 100., (p.z * 100.).round() / 100.],
                    }));
                } else if before.pinned {
                    net.outgoing.push(json!({"k": "pin", "s": track.survivor, "by": null}));
                }
                // Boomer bile (or a bile jar) on them in our game: their bile
                // screen and "it" time.
                if m.vomited > before.vomited + 0.5 {
                    net.outgoing.push(json!({"k": "vom", "s": track.survivor, "t": (m.vomited * 10.).round() / 10.}));
                }
            }
        }
        // A pinned survivor is moved by its attacker in the host's game.
        if !pinned {
            member.position = s.position;
        }
        member.aim = s.aim;
        member.vertical = 0.;
        let revived = net.revived.contains_key(&index);
        let m = &mut member.state;
        m.health = s.health;
        m.temporary = s.temporary;
        if m.incap != s.incap && !(revived && s.incap) {
            m.incap = s.incap;
            if s.incap {
                m.incap_health = 300.;
            }
        }
        m.dead = s.dead;
        // Hanging from a ledge in the owner's game: hang health and the time
        // since the grab drive the hang animation here. Not while our own
        // save of them waits for the owner to agree.
        match s.hang {
            Some((since, health)) if !revived && !s.dead => {
                m.incap = true;
                m.incap_health = health;
                let grabbed = m.elapsed - since;
                match m.ledge.as_mut() {
                    Some(h) => {
                        h.hang = s.position;
                        h.grabbed = grabbed;
                    }
                    None => {
                        m.ledge = Some(crate::ledge_hang::Hang {
                            hang: s.position,
                            stand: s.position + s.aim * Vec3::new(1., 0., 1.),
                            health: s.health,
                            temporary: s.temporary,
                            stage: 0,
                            grabbed,
                        })
                    }
                }
            }
            _ => m.ledge = None,
        }
        // Someone else's pin (the host's player and bots on a guest, a
        // guest's own on others): drawn with its pinned pose. On the host a
        // guest's pin is its own game's (sent above).
        if !owned {
            if m.pinned_by != s.pinned_by && std::env::var_os("L4D2R_NET_DEBUG").is_some() {
                println!("Net play: {} pinned by {:?} (None: released)", track.survivor, s.pinned_by);
            }
            m.pinned = s.pinned_by.is_some();
            m.pinned_by = s.pinned_by;
        }
        m.slot = s.slot;
        m.primary = s.primary.as_deref().map(|class| gun(class, m.primary.as_ref()));
        m.pistol = gun(&s.pistol, Some(&m.pistol));
        // New shots: the weapon's fire sound from them (one per update at
        // most; automatic fire still sounds continuous at 20 updates/s).
        let heard = net.heard.entry(*id).or_insert(s.shots);
        if s.shots > *heard {
            m.shot_serial = s.shots;
            if let (Some(rules), Some(sounds)) = (rules.as_ref(), sounds.as_mut()) {
                let weapon = rules.weapons.get(&m.gun().class).unwrap_or(&rules.pistol);
                sounds.play(&weapon.fire_sound, Some(s.position + Vec3::Y * crate::collision::EYE));
            }
        }
        *heard = s.shots.max(*heard);
        if owned {
            net.applied.insert(index, Applied {
                serial: m.damage_serial,
                health: m.health,
                temporary: m.temporary,
                incap: m.incap,
                incap_health: m.incap_health,
                pinned: m.pinned,
                vomited: m.vomited,
            });
        }
        if std::env::var_os("L4D2R_NET_DEBUG").is_some() && (time.elapsed_secs() * 30.) as u32 % 60 == 0 {
            println!("Net play: {} (member {id}) drawn at {:.2?}", track.survivor, s.position);
        }
    }
    let before = (0..8).fold(0u8, |m, i| m | (crate::game_state::remote(i) as u8) << i);
    if before != mask {
        for (index, member) in squad.0.iter().enumerate().take(8) {
            if (before ^ mask) & (1 << index) != 0 {
                let who = member.model.rsplit('/').next().unwrap_or("");
                println!("Net play: {who} is now {}", if mask & (1 << index) != 0 { "played by a remote player" } else { "a bot again" });
            }
        }
    }
    crate::game_state::set_remote(mask);
}

/// Team panels show a remote player's own name instead of the survivor's.
fn team_names(
    net: Res<NetPlay>,
    session: Res<Session>,
    squad: Option<Res<Squad>>,
    mut names: Query<(&crate::game_ui::TeamName, &mut Text)>,
) {
    let Some(squad) = squad else { return };
    // Survivor -> player name, for the scoreboard too.
    let players: HashMap<String, String> = net
        .tracks
        .iter()
        .filter(|(id, _)| **id < BOT_KEY)
        .filter_map(|(id, t)| Some((t.survivor.clone(), session.core.members.iter().find(|m| m.id == *id)?.name.clone())))
        .collect();
    *PLAYERS.lock().unwrap() = Some(players);
    for (slot, mut text) in &mut names {
        let Some(member) = squad.0.get(slot.0) else { continue };
        let player = net.tracks.iter().filter(|(id, _)| **id < BOT_KEY).find_map(|(id, t)| {
            (squad_index(&squad, &t.survivor) == Some(slot.0))
                .then(|| session.core.members.iter().find(|m| m.id == *id).map(|m| m.name.clone()))
                .flatten()
        });
        let name = player.unwrap_or_else(|| crate::game_ui::kill_feed::display_name(&member.model).to_string());
        if text.0 != name {
            text.0 = name;
        }
    }
}

/// Remote players by the survivor they play ("Rochelle" -> name).
static PLAYERS: std::sync::Mutex<Option<HashMap<String, String>>> = std::sync::Mutex::new(None);
/// The name of the remote player playing `survivor`, if any.
pub(crate) fn player_of(survivor: &str) -> Option<String> {
    PLAYERS.lock().unwrap().as_ref()?.get(survivor).cloned()
}

pub fn install(app: &mut App) {
    app.init_resource::<NetPlay>()
        .add_systems(Update, (exchange.after(crate::multiplayer::run), team_names))
        .add_systems(FixedUpdate, apply);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overhead_and_hud_names_follow_player_takeover_rename_and_disconnect() {
        use bevy::ecs::system::RunSystemOnce;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let pistol = crate::game_state::Weapon::load(&mut assets, "weapon_pistol").unwrap();
        let mut world = World::new();
        world.insert_resource(Squad(vec![crate::game_state::Companion {
            state: crate::game_state::Match::new(&pistol), position: Vec3::ZERO,
            spawn: Vec3::ZERO, model: "models/survivors/survivor_coach.mdl".into(),
            vertical: 0., cooldown: 0., revive_time: 0., aim: Vec3::Z,
        }]));
        world.init_resource::<NetPlay>();
        world.init_resource::<Session>();
        let hud = world.spawn((crate::game_ui::TeamName(0), Text::default())).id();
        let overhead = world.spawn((crate::game_ui::TeamName(0), Text::default())).id();
        world.run_system_once(team_names).unwrap();
        assert_eq!(world.get::<Text>(overhead).unwrap().0, "Coach");
        world.resource_mut::<Session>().core.members.push(crate::multiplayer::Member {
            id: 7, name: "Steam Friend 🎮".into(), survivor: "Coach".into(), steam: 76561190000000007,
        });
        world.resource_mut::<NetPlay>().tracks.insert(7, Track { survivor: "Coach".into(), ..default() });
        for expected in ["Steam Friend 🎮", "Renamed Friend"] {
            world.resource_mut::<Session>().core.members[0].name = expected.into();
            world.run_system_once(team_names).unwrap();
            assert_eq!(world.get::<Text>(hud).unwrap().0, expected);
            assert_eq!(world.get::<Text>(overhead).unwrap().0, expected);
        }
        world.resource_mut::<Session>().core.members.clear();
        world.resource_mut::<NetPlay>().tracks.clear();
        world.run_system_once(team_names).unwrap();
        assert_eq!(world.get::<Text>(overhead).unwrap().0, "Coach");
    }

    fn sample(x: f32) -> Sample {
        Sample {
            position: Vec3::new(x, 0., 0.),
            aim: Vec3::X,
            health: 80.,
            temporary: 5.,
            incap: false,
            dead: false,
            slot: 1,
            primary: Some("weapon_smg".into()),
            pistol: "weapon_pistol".into(),
            shots: 3,
            pinned_by: Some(crate::special_infected::SpecialClass::Hunter),
            hang: Some((1.5, 210.)),
        }
    }

    #[test]
    fn state_round_trips_through_json() {
        let s = sample(1.25);
        let (name, back) = Sample::from_json(&s.to_json("Rochelle")).unwrap();
        assert_eq!(name, "Rochelle");
        assert_eq!(back, s);
    }

    #[test]
    fn remote_players_are_drawn_between_received_states() {
        let mut track = Track::default();
        track.samples.push_back((1.0, sample(0.)));
        track.samples.push_back((1.1, sample(10.)));
        assert_eq!(track.sample(0.5).unwrap().position.x, 0.);
        assert!((track.sample(1.05).unwrap().position.x - 5.).abs() < 1e-3);
        assert_eq!(track.sample(2.0).unwrap().position.x, 10.);
    }
}
