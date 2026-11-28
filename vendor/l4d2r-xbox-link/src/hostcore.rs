//! A System Link host as a pure state machine: Ethernet frames in, frames
//! out. Combines discovery (host.rs answer_search), the association
//! responder (session.rs), the lobby host, the game connect handshake,
//! sign-on and the per-tick game loop. Transport (Npcap) and the game world
//! feed live outside (see the game's xbox_link.rs).
use crate::bits::BitWriter;
use crate::game::UserCmd;
use crate::host::{
    accept_packet, answer_search, challenge_packet, parse_connect, parse_query, HostWorld, LobbyHost, ServerGame,
    ServerSignon, SignonTemplate,
};
use crate::matchmaking::{game_details, Advert, Kv, Message};
use crate::netchan::{Channel, Incoming};
use crate::session::{Association, State};
use crate::xnet::{parse_frame, Keys, Packet};
use std::collections::HashMap;

const LOBBY_PORT: u16 = 27027;
const GAME_SERVER_PORT: u16 = 27015;
const GAME_CLIENT_PORT: u16 = 27005;

/// One connected console.
pub struct Peer {
    pub assoc: Association,
    lobby: Channel,
    game: Channel,
    pub xuid: u64,
    pub name: String,
    challenge: u32,
    pub signon: Option<ServerSignon>,
    pub loop_: Option<ServerGame>,
    /// The player entity this console controls.
    pub entity: u32,
    pub cmds: Vec<UserCmd>,
}

pub struct HostCore {
    keys: Keys,
    pub mac: [u8; 6],
    pub advert: Advert,
    pub lobby: LobbyHost,
    template: SignonTemplate,
    pub world: HostWorld,
    pub peers: HashMap<[u8; 6], Peer>,
    pub started: bool,
    sequence: u16,
    next_challenge: u32,
}

impl HostCore {
    pub fn new(keys: Keys, mac: [u8; 6], advert: Advert, lobby: LobbyHost, template: SignonTemplate, world: HostWorld) -> Self {
        HostCore { keys, mac, advert, lobby, template, world, peers: HashMap::new(), started: false, sequence: 1, next_challenge: 0x0abd_f563 }
    }

    fn refresh_advert(&mut self) {
        self.lobby.settings.players = 1 + self.lobby.machines.len() as i32;
        self.advert.settings = self.lobby.settings.kv();
    }

    /// The GameDetails reply as a discovery message (also for tests).
    pub fn details(&self) -> Message {
        game_details(&self.advert)
    }

    fn lobby_kv(peer: &mut Peer, kv: &Kv) {
        let mut w = BitWriter::new();
        crate::game::write_lobby_kv(&mut w, kv);
        peer.lobby.queue_reliable(&w);
    }

    /// Feed one received Ethernet frame; returns frames to send.
    pub fn on_frame(&mut self, frame: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        // Discovery.
        self.refresh_advert();
        if let Some(reply) = answer_search(&self.keys, frame, self.mac, &self.advert, self.sequence) {
            self.sequence = self.sequence.wrapping_add(1);
            return vec![reply];
        }
        let Some(f) = parse_frame(frame) else { return out };
        if f.destination != self.mac {
            return out;
        }
        let source = f.source;
        // A new association.
        if !self.peers.contains_key(&source) {
            if let Some((assoc, answer)) = Association::respond(
                &self.keys, &self.advert.session_id, &self.advert.session_key, &self.advert.host_xnaddr, frame)
            {
                self.peers.insert(source, Peer {
                    assoc,
                    lobby: Channel::lobby(),
                    game: Channel::game(),
                    xuid: 0,
                    name: String::new(),
                    challenge: 0,
                    signon: None,
                    loop_: None,
                    entity: 0,
                    cmds: Vec::new(),
                });
                out.push(answer);
            }
            return out;
        }
        let peer = self.peers.get_mut(&source).unwrap();
        let (frames, packets) = peer.assoc.receive(frame);
        out.extend(frames);
        for p in packets {
            let Packet::Data { source_port, destination_port, encrypted, .. } = p else { continue };
            match (source_port, destination_port) {
                (LOBBY_PORT, LOBBY_PORT) => match peer.lobby.receive(&encrypted) {
                    Incoming::Connectionless(d) => {
                        // The channel strips the 0xffffffff prefix; Message::decode wants it.
                        let mut whole = vec![0xff; 4];
                        whole.extend_from_slice(&d);
                        let Ok(msg) = Message::decode(&whole) else { continue };
                        if let Some(reply) = self.lobby.join(&msg) {
                            let body = msg.body();
                            peer.xuid = body.and_then(|b| b.u64(&["id"])).unwrap_or(0);
                            peer.name = body
                                .and_then(|b| b.str(&["Settings", "Members", "machine0", "player0", "name"]))
                                .unwrap_or("")
                                .to_string();
                            Self::lobby_kv(peer, &reply);
                            out.push(peer.assoc.vdp(LOBBY_PORT, LOBBY_PORT, &peer.lobby.send(&BitWriter::new())));
                        }
                    }
                    Incoming::Packet(r) => {
                        for blob in &r.reliable {
                            for m in crate::game::walk(blob, 0, blob.len() * 8, true).0 {
                                if let crate::game::Msg::KeyValues(kv) = m {
                                    if let Some(update) = self.lobby.command(&kv) {
                                        Self::lobby_kv(peer, &update);
                                    }
                                }
                            }
                        }
                    }
                    Incoming::Invalid => {}
                },
                (GAME_CLIENT_PORT, GAME_SERVER_PORT) => match peer.game.receive(&encrypted) {
                    Incoming::Connectionless(d) => {
                        let body = &d[..];
                        if let Some(echo) = parse_query(body) {
                            peer.challenge = self.next_challenge;
                            self.next_challenge = self.next_challenge.wrapping_mul(1_103_515_245).wrapping_add(12345);
                            let a = challenge_packet(peer.challenge, &echo, self.lobby.nonce);
                            out.push(peer.assoc.vdp(GAME_SERVER_PORT, GAME_CLIENT_PORT, &a));
                        } else if let Some(c) = parse_connect(body) {
                            if c.challenge == peer.challenge {
                                peer.signon = Some(ServerSignon::new(self.template.clone()));
                                out.push(peer.assoc.vdp(GAME_SERVER_PORT, GAME_CLIENT_PORT, &accept_packet()));
                            }
                        }
                    }
                    Incoming::Packet(r) => {
                        let mut send = Vec::new();
                        for blob in &r.reliable {
                            if let Some(s) = peer.signon.as_mut() {
                                send.extend(s.handle(blob));
                                if s.full && peer.loop_.is_none() {
                                    peer.loop_ = Some(ServerGame::new(10_000, 2047));
                                }
                            }
                        }
                        for w in send {
                            peer.game.queue_reliable(&w);
                        }
                        if let Some(l) = peer.loop_.as_mut() {
                            if let Some(msgs) = crate::clc::split(&r.payload, r.unreliable.0, r.unreliable.1) {
                                peer.cmds.extend(l.on_client(&msgs));
                            }
                        }
                                        if peer.game.reliable_outstanding() {
                            for part in peer.game.send_split(&BitWriter::new()) {
                                out.push(peer.assoc.vdp(GAME_SERVER_PORT, GAME_CLIENT_PORT, &part));
                            }
                        }
                    }
                    Incoming::Invalid => {}
                },
                _ => {}
            }
        }
        out
    }

    /// START: every lobby member gets the reservation and connects.
    pub fn start(&mut self) -> Vec<Vec<u8>> {
        self.started = true;
        let start = self.lobby.start();
        let mut out = Vec::new();
        for peer in self.peers.values_mut() {
            Self::lobby_kv(peer, &start);
            out.push(peer.assoc.vdp(LOBBY_PORT, LOBBY_PORT, &peer.lobby.send(&BitWriter::new())));
        }
        out
    }

    /// One server tick: snapshots to signed-on consoles, heartbeats otherwise.
    pub fn tick(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for peer in self.peers.values_mut() {
            if peer.assoc.state != State::Established {
                continue;
            }
            out.push(peer.assoc.vdp(LOBBY_PORT, LOBBY_PORT, &peer.lobby.heartbeat()));
            let mut unreliable = BitWriter::new();
            if let Some(l) = peer.loop_.as_mut() {
                if let Some(msgs) = l.snapshot(&self.world.world.schema, &self.world.world.entities) {
                    for m in &msgs {
                        crate::svc::write(&mut unreliable, m);
                    }
                }
            }
            if peer.signon.is_some() {
                for part in peer.game.send_split(&unreliable) {
                    out.push(peer.assoc.vdp(GAME_SERVER_PORT, GAME_CLIENT_PORT, &part));
                }
            }
        }
        out
    }

    /// Player names (slot, xuid, name) to every signed-on console.
    pub fn send_names(&mut self, players: &[(u32, u64, String)]) {
        let list: Vec<(u32, u64, &str)> = players.iter().map(|(s, x, n)| (*s, *x, n.as_str())).collect();
        let Some(update) = self.world.set_names(&list) else { return };
        let mut w = BitWriter::new();
        crate::svc::write(&mut w, &update);
        for peer in self.peers.values_mut() {
            if peer.loop_.is_some() {
                peer.game.queue_reliable(&w);
            }
        }
    }

    /// Usercmds received since the last call, per console xuid.
    pub fn take_cmds(&mut self) -> Vec<(u64, Vec<UserCmd>)> {
        self.peers.values_mut().filter(|p| !p.cmds.is_empty()).map(|p| (p.xuid, std::mem::take(&mut p.cmds))).collect()
    }
}
