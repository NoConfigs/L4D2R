//! Hosting a System Link game for 360 consoles: a background thread running
//! `HostCore` over Npcap at 30 Hz. The game feeds survivor state in and reads
//! the consoles' players out (see the game's xbox_link.rs).
use crate::client::Config;
use crate::game::UserCmd;
use crate::host::{GameSettings, HostMember, HostWorld, LobbyHost};
use crate::hostcore::HostCore;
use crate::matchmaking::Advert;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One survivor slot driven by the game (Source units and degrees).
#[derive(Debug, Clone, PartialEq)]
pub struct Feed {
    pub slot: u32,
    pub origin: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub health: i64,
    /// Held weapon (rewrite name, "weapon_smg").
    pub weapon: Option<String>,
}

/// A console player in our game.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsolePlayer {
    pub xuid: u64,
    pub name: String,
    pub slot: u32,
    pub origin: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub in_game: bool,
}

#[derive(Debug, Clone, Default)]
pub struct HostShared {
    pub running: bool,
    pub started: bool,
    pub players: Vec<ConsolePlayer>,
    pub messages: Vec<String>,
    pub error: Option<String>,
}

pub enum HostCommand {
    /// Survivors the game controls this tick.
    Feed(Vec<Feed>),
    /// The game's infected this tick (replaces the previous list).
    Infected(Vec<crate::host::InfectedFeed>),
    /// The game's collision-corrected position for a console's survivor.
    Place(u64, [f32; 3]),
    /// The game's prop doors (state and swing angle).
    Doors(Vec<crate::host::DoorFeed>),
    /// START: consoles in the lobby connect to the game.
    Start,
    Stop,
}

pub struct HostSetup {
    pub config: Config,
    /// Private relay recording of a 360 host's sign-on on the same map.
    pub template_recording: std::path::PathBuf,
    pub settings: GameSettings,
    /// Slot the first console gets (1-4) and its character (0 Nick .. 3 Ellis).
    pub console_slot: u32,
}

pub struct XboxHost {
    shared: Arc<Mutex<HostShared>>,
    tx: Sender<HostCommand>,
}

impl XboxHost {
    pub fn start(setup: HostSetup) -> Result<XboxHost, String> {
        let shared = Arc::new(Mutex::new(HostShared::default()));
        let (tx, rx) = channel();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("xbox-host".into())
            .spawn(move || {
                if let Err(e) = run(setup, s.clone(), rx) {
                    let mut g = s.lock().unwrap();
                    g.running = false;
                    g.error = Some(e);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(XboxHost { shared, tx })
    }
    pub fn send(&self, c: HostCommand) {
        let _ = self.tx.send(c);
    }
    pub fn shared(&self) -> std::sync::MutexGuard<'_, HostShared> {
        self.shared.lock().unwrap()
    }
}

impl Drop for XboxHost {
    fn drop(&mut self) {
        let _ = self.tx.send(HostCommand::Stop);
    }
}

/// Simple movement for a console survivor from its usercmds (no collision;
/// the rewrite does not simulate console players yet).
pub fn integrate(origin: &mut [f32; 3], cmd: &UserCmd, dt: f32) {
    let yaw = cmd.view[1].to_radians();
    let (f, s) = (cmd.forward / 450.0, cmd.side / 450.0);
    let speed = 220.0 * dt;
    origin[0] += (yaw.cos() * f + yaw.sin() * s) * speed;
    origin[1] += (yaw.sin() * f - yaw.cos() * s) * speed;
}

#[cfg(windows)]
fn run(setup: HostSetup, s: Arc<Mutex<HostShared>>, rx: Receiver<HostCommand>) -> Result<(), String> {
    use crate::sys::{random_array, Pcap};
    use crate::xnet::{broadcast_keys, xex_lan_key};
    let cfg = &setup.config;
    let xex = std::fs::read(&cfg.xex).map_err(|e| format!("{}: {e}", cfg.xex.display()))?;
    let keys = broadcast_keys(&xex_lan_key(&xex).map_err(|e| format!("{e:?}"))?, &cfg.platform_key);
    let schema_text = std::fs::read_to_string(&cfg.sendtables).map_err(|e| format!("{}: {e}", cfg.sendtables.display()))?;
    let recording = std::fs::read_to_string(&setup.template_recording)
        .map_err(|e| format!("{}: {e}", setup.template_recording.display()))?;
    let mut template = crate::host::template_from_relay(&recording).ok_or("the recording has no complete sign-on")?;
    template.set_host_name(&cfg.name);
    let map = template
        .stages
        .iter()
        .flatten()
        .find_map(|m| match m {
            crate::svc::Svc::ServerInfo(i) => Some(String::from_utf8_lossy(&i.map).to_string()),
            _ => None,
        })
        .unwrap_or_default();
    let bsp = std::fs::read(cfg.maps.join(format!("{map}.360.bsp"))).ok();
    let dict = bsp.as_deref().and_then(crate::entities::dictionary_from_bsp);
    let mut world = HostWorld::from_template(&schema_text, &template, dict).ok_or("no world in the recording")?;
    for (slot, character) in [(1u32, 2i64), (2, 3), (3, 0), (4, 1)] {
        world.ensure_survivor(slot, character);
    }
    // Doors and props outside the sign-on snapshot's view.
    let absorbed = world.absorb_recording(&recording);
    s.lock().unwrap().messages.push(format!("{absorbed} map entities added from the recording"));
    let (device, mac) = crate::client::adapter(&cfg.local_address)?;
    let mut pcap = Pcap::open(&device, 5)?;
    let xuid = 0xE000_0000_0000_0000 | (u64::from_le_bytes(random_array()) & 0xFFFF_FFFF);
    let mut xnaddr = [0u8; 36];
    let ip: Vec<u8> = cfg.local_address.split('.').filter_map(|p| p.parse().ok()).collect();
    if ip.len() == 4 {
        xnaddr[..4].copy_from_slice(&ip);
    }
    xnaddr[10..16].copy_from_slice(&mac);
    let nonce = u64::from_le_bytes(random_array());
    let lobby = LobbyHost::new(
        setup.settings.clone(),
        HostMember { xuid, name: cfg.name.clone(), avatar: cfg.avatar.clone(), dlcmask: 0, xnaddr },
        nonce,
    );
    let advert = Advert {
        host_name: cfg.name.clone(),
        host_xuid: xuid,
        host_xuid_online: 0,
        session_id: random_array(),
        host_xnaddr: xnaddr,
        session_key: random_array(),
        settings: setup.settings.kv(),
    };
    let mut core = HostCore::new(keys, mac, advert, lobby, template, world);
    {
        let mut g = s.lock().unwrap();
        g.running = true;
        g.messages.push(format!("Hosting {map} for System Link ({} on {})", cfg.name, cfg.local_address));
    }
    let mut next_tick = Instant::now();
    let mut console: Vec<ConsolePlayer> = Vec::new();
    let mut horde = crate::host::HordeSync::default();
    let mut doors = crate::host::DoorSync::default();
    let mut names_sent: Vec<(u32, u64, String)> = Vec::new();
    let mut in_game_count = 0;
    loop {
        loop {
            match rx.try_recv() {
                Ok(HostCommand::Feed(list)) => {
                    for f in list {
                        if console.iter().any(|c| c.slot == f.slot) {
                            continue; // a console drives this slot
                        }
                        core.world.set_survivor(f.slot, f.origin, f.pitch, f.yaw, f.health);
                        if let Some(w) = &f.weapon {
                            core.world.set_weapon(f.slot, w);
                        }
                    }
                }
                Ok(HostCommand::Infected(list)) => horde.apply(&mut core.world, &list),
                Ok(HostCommand::Doors(list)) => doors.apply(&mut core.world, &list),
                Ok(HostCommand::Place(who, origin)) => {
                    if let Some(c) = console.iter_mut().find(|c| c.xuid == who) {
                        c.origin = origin;
                        core.world.set_survivor(c.slot, c.origin, c.pitch, c.yaw, 100);
                    }
                }
                Ok(HostCommand::Start) => {
                    for f in core.start() {
                        pcap.send(&f)?;
                    }
                    s.lock().unwrap().started = true;
                }
                Ok(HostCommand::Stop) | Err(TryRecvError::Disconnected) => return Ok(()),
                Err(TryRecvError::Empty) => break,
            }
        }
        if Instant::now() >= next_tick {
            next_tick += Duration::from_millis(33);
            // Console players: give each a survivor slot and move it by its commands.
            for p in core.peers.values() {
                if p.xuid != 0 && !console.iter().any(|c| c.xuid == p.xuid) {
                    let slot = setup.console_slot + console.len() as u32;
                    let origin = core.world.world.origin(slot).unwrap_or_default();
                    console.push(ConsolePlayer { xuid: p.xuid, name: p.name.clone(), slot, origin, yaw: 0.0, pitch: 0.0, in_game: false });
                }
            }
            for (who, cmds) in core.take_cmds() {
                let Some(c) = console.iter_mut().find(|c| c.xuid == who) else { continue };
                c.in_game = true;
                for cmd in &cmds {
                    integrate(&mut c.origin, cmd, 1.0 / 30.0);
                    c.pitch = cmd.view[0];
                    c.yaw = cmd.view[1];
                }
                core.world.set_survivor(c.slot, c.origin, c.pitch, c.yaw, 100);
            }
            // Names: us on slot 1, each console on its slot; resent when a console finishes loading.
            let mut names = vec![(1u32, xuid, cfg.name.clone())];
            names.extend(console.iter().map(|c| (c.slot, c.xuid, c.name.clone())));
            let loaded = core.peers.values().filter(|p| p.loop_.is_some()).count();
            if loaded > 0 && (names != names_sent || loaded != in_game_count) {
                core.send_names(&names);
                names_sent = names;
                in_game_count = loaded;
            }
            for f in core.tick() {
                pcap.send(&f)?;
            }
            s.lock().unwrap().players = console.clone();
        }
        while let Some(frame) = pcap.recv()? {
            for f in core.on_frame(&frame) {
                pcap.send(&f)?;
            }
            if Instant::now() >= next_tick {
                break;
            }
        }
    }
}

#[cfg(not(windows))]
fn run(_setup: HostSetup, _s: Arc<Mutex<HostShared>>, _rx: Receiver<HostCommand>) -> Result<(), String> {
    Err("hosting System Link games needs Windows + Npcap".into())
}
