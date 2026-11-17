//! Steam: the player's Steam name, Steam lobbies (invites, "Join Game") and
//! connections relayed through Steam's servers (no IP addresses or port
//! forwarding). Desktop only: on phones every call is a no-op and
//! multiplayer uses direct connections.
//!
//! The rewrite has no Steam App ID of its own, so it runs as Valve's public
//! test app 480 (Spacewar) unless L4D2R_STEAM_APPID says otherwise; friends
//! see "Spacewar" as the game being played. Steam must be running.
//!
//! Messages go over ISteamNetworkingMessages (reliable, channel 0). The
//! session code addresses peers by 64-bit Steam ID (`multiplayer::Link`).

/// Things that happened on Steam, for multiplayer.rs.
#[derive(Clone, Debug, PartialEq)]
pub enum SteamEvent {
    /// Our lobby was created (id).
    LobbyCreated(u64),
    /// Creating the lobby failed.
    LobbyFailed(String),
    /// We entered a lobby (id, owner Steam ID).
    LobbyEntered(u64, u64),
    /// Entering a lobby failed.
    LobbyJoinFailed,
    /// The player accepted an invite or chose "Join Game" (lobby id).
    JoinRequested(u64),
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod imp {
    use super::SteamEvent;
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::sync::{Arc, Mutex, OnceLock};
    use steamworks::networking_types::{NetworkingIdentity, SendFlags};
    use steamworks::{Client, LobbyId, LobbyType, SteamId};

    #[derive(Default)]
    struct Shared {
        events: Vec<SteamEvent>,
        /// Users who opened a session with us since the last call.
        new_peers: Vec<u64>,
        failed: HashSet<u64>,
        /// Accept incoming sessions (we are hosting).
        accepting: bool,
        /// The last public lobby search (`request_lobbies`).
        listed: Vec<u64>,
        searching: bool,
    }

    struct Steam {
        client: Client,
        me: u64,
        name: String,
        shared: Arc<Mutex<Shared>>,
        inbox: Mutex<HashMap<u64, VecDeque<Vec<u8>>>>,
        /// Friends' lobbies whose data was requested.
        asked: Mutex<HashSet<u64>>,
        _callbacks: Vec<steamworks::CallbackHandle>,
    }

    static STEAM: OnceLock<Option<Steam>> = OnceLock::new();

    fn app_id() -> u32 {
        std::env::var("L4D2R_STEAM_APPID").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(480)
    }

    fn steam() -> Option<&'static Steam> {
        STEAM
            .get_or_init(|| {
                if cfg!(test) || std::env::var_os("L4D2R_NO_STEAM").is_some() {
                    return None;
                }
                let client = match Client::init_app(app_id()) {
                    Ok(c) => c,
                    Err(e) => {
                        println!("Steam: not available ({e}); multiplayer uses direct connections only");
                        return None;
                    }
                };
                let shared = Arc::new(Mutex::new(Shared::default()));
                let messages = client.networking_messages();
                let s = shared.clone();
                messages.session_request_callback(move |request| {
                    let id = request.remote().steam_id().map(|i| i.raw());
                    let mut s = s.lock().unwrap();
                    match id {
                        Some(id) if s.accepting => {
                            if request.accept() {
                                s.failed.remove(&id);
                                s.new_peers.push(id);
                            }
                        }
                        _ => request.reject(),
                    }
                });
                let s = shared.clone();
                messages.session_failed_callback(move |failed| {
                    if let Some(id) = failed.identity_remote().and_then(|i| i.steam_id()) {
                        s.lock().unwrap().failed.insert(id.raw());
                    }
                });
                let s = shared.clone();
                let join = client.register_callback(move |r: steamworks::GameLobbyJoinRequested| {
                    s.lock().unwrap().events.push(SteamEvent::JoinRequested(r.lobby_steam_id.raw()));
                });
                let me = client.user().steam_id().raw();
                let name = client.friends().name();
                println!("Steam: signed in as {name} (app {})", app_id());
                Some(Steam {
                    client,
                    me,
                    name,
                    shared,
                    inbox: Mutex::new(HashMap::new()),
                    asked: Mutex::new(HashSet::new()),
                    _callbacks: vec![join],
                })
            })
            .as_ref()
    }

    pub fn available() -> bool {
        steam().is_some()
    }
    pub fn name() -> Option<String> {
        steam().map(|s| s.name.clone())
    }
    pub fn me() -> Option<u64> {
        steam().map(|s| s.me)
    }

    /// A user's Steam avatar as (width, height, RGBA). Medium (64x64) first,
    /// then small (32x32). None while Steam still downloads it: this asks
    /// Steam to fetch the user's info, so a later call can succeed.
    pub fn avatar(steam_id: u64) -> Option<(u32, u32, Vec<u8>)> {
        let s = steam()?;
        if steam_id == 0 {
            return None;
        }
        let friends = s.client.friends();
        let friend = friends.get_friend(SteamId::from_raw(steam_id));
        if let Some(rgba) = friend.medium_avatar() {
            return Some((64, 64, rgba));
        }
        if let Some(rgba) = friend.small_avatar() {
            return Some((32, 32, rgba));
        }
        if steam_id != s.me {
            friends.request_user_information(SteamId::from_raw(steam_id), false);
        }
        None
    }

    /// Runs Steam callbacks and collects arrived messages. Once per frame.
    pub fn pump() {
        let Some(s) = steam() else { return };
        s.client.run_callbacks();
        let messages = s.client.networking_messages();
        let mut inbox = s.inbox.lock().unwrap();
        loop {
            let batch = messages.receive_messages_on_channel(0, 64);
            if batch.is_empty() {
                break;
            }
            for m in batch {
                if let Some(id) = m.identity_peer().steam_id() {
                    inbox.entry(id.raw()).or_default().push_back(m.data().to_vec());
                }
            }
        }
    }

    pub fn events() -> Vec<SteamEvent> {
        let Some(s) = steam() else { return Vec::new() };
        let events = std::mem::take(&mut s.shared.lock().unwrap().events);
        events
            .into_iter()
            .map(|e| match e {
                SteamEvent::LobbyEntered(id, 0) => {
                    SteamEvent::LobbyEntered(id, s.client.matchmaking().lobby_owner(LobbyId::from_raw(id)).raw())
                }
                e => e,
            })
            .collect()
    }

    pub fn set_accepting(on: bool) {
        if let Some(s) = steam() {
            s.shared.lock().unwrap().accepting = on;
        }
    }
    pub fn take_new_peers() -> Vec<u64> {
        steam().map_or_else(Vec::new, |s| std::mem::take(&mut s.shared.lock().unwrap().new_peers))
    }

    pub fn send(user: u64, data: &[u8]) -> bool {
        let Some(s) = steam() else { return false };
        let to = NetworkingIdentity::new_steam_id(SteamId::from_raw(user));
        s.client
            .networking_messages()
            .send_message_to_user(to, SendFlags::RELIABLE_NO_NAGLE | SendFlags::AUTO_RESTART_BROKEN_SESSION, data, 0)
            .is_ok()
    }
    /// Messages from `user` since the last call; None once the session failed.
    pub fn take(user: u64) -> Option<Vec<Vec<u8>>> {
        let s = steam()?;
        if s.shared.lock().unwrap().failed.contains(&user) {
            return None;
        }
        Some(s.inbox.lock().unwrap().remove(&user).map(Vec::from).unwrap_or_default())
    }
    pub fn forget(user: u64) {
        if let Some(s) = steam() {
            s.inbox.lock().unwrap().remove(&user);
            s.shared.lock().unwrap().failed.remove(&user);
        }
    }

    fn lobby_type(access: u8) -> LobbyType {
        match access {
            0 => LobbyType::Public,
            1 => LobbyType::FriendsOnly,
            _ => LobbyType::Private,
        }
    }
    /// Creates a lobby (access: 0 public, 1 friends only, 2 private).
    pub fn create_lobby(access: u8) {
        let Some(s) = steam() else { return };
        let shared = s.shared.clone();
        s.client.matchmaking().create_lobby(lobby_type(access), crate::multiplayer::MAX_MEMBERS as u32, move |r| {
            let event = match r {
                Ok(id) => SteamEvent::LobbyCreated(id.raw()),
                Err(e) => SteamEvent::LobbyFailed(e.to_string()),
            };
            shared.lock().unwrap().events.push(event);
        });
    }
    pub fn set_lobby_access(lobby: u64, access: u8) {
        let Some(_) = steam() else { return };
        let ty = match access {
            0 => steamworks::sys::ELobbyType::k_ELobbyTypePublic,
            1 => steamworks::sys::ELobbyType::k_ELobbyTypeFriendsOnly,
            _ => steamworks::sys::ELobbyType::k_ELobbyTypePrivate,
        };
        unsafe {
            steamworks::sys::SteamAPI_ISteamMatchmaking_SetLobbyType(steamworks::sys::SteamAPI_SteamMatchmaking_v009(), lobby, ty);
        }
    }
    pub fn set_lobby_data(lobby: u64, key: &str, value: &str) {
        if let Some(s) = steam() {
            s.client.matchmaking().set_lobby_data(LobbyId::from_raw(lobby), key, value);
        }
    }
    pub fn join_lobby(lobby: u64) {
        let Some(s) = steam() else { return };
        let shared = s.shared.clone();
        s.client.matchmaking().join_lobby(LobbyId::from_raw(lobby), move |r| {
            // The owner is filled in by `events` (on the main thread).
            let event = match r {
                Ok(id) => SteamEvent::LobbyEntered(id.raw(), 0),
                Err(()) => SteamEvent::LobbyJoinFailed,
            };
            shared.lock().unwrap().events.push(event);
        });
    }
    pub fn leave_lobby(lobby: u64) {
        if let Some(s) = steam() {
            s.client.matchmaking().leave_lobby(LobbyId::from_raw(lobby));
        }
    }
    /// Marks our lobby as a rewrite game and stores its browser advert
    /// (game_browser.rs) as lobby data.
    pub fn advertise(lobby: u64, advert: &str) {
        set_lobby_data(lobby, "game", super::GAME_KEY);
        set_lobby_data(lobby, "l4d2r", advert);
    }
    /// Starts a search for public rewrite lobbies (worldwide); the result
    /// shows in `found_lobbies` when it arrives.
    pub fn request_lobbies() {
        let Some(s) = steam() else { return };
        {
            let mut shared = s.shared.lock().unwrap();
            if shared.searching {
                return;
            }
            shared.searching = true;
        }
        let mm = s.client.matchmaking();
        mm.add_request_lobby_list_string_filter(steamworks::StringFilter(
            steamworks::LobbyKey::new("game"),
            super::GAME_KEY,
            steamworks::StringFilterKind::Equal,
        ));
        mm.set_request_lobby_list_distance_filter(steamworks::DistanceFilter::Worldwide);
        let shared = s.shared.clone();
        mm.request_lobby_list(move |r| {
            let mut shared = shared.lock().unwrap();
            shared.searching = false;
            if let Ok(ids) = r {
                shared.listed = ids.into_iter().map(|l| l.raw()).collect();
            }
        });
    }
    /// Rewrite lobbies we can see: (lobby, advert, a friend is in it). The
    /// public search results, then the lobbies friends playing the rewrite
    /// are in (friends-only lobbies are not listed publicly). A friend's
    /// lobby data is asked for once and shows up a moment later.
    pub fn found_lobbies() -> Vec<(u64, String, bool)> {
        let Some(s) = steam() else { return Vec::new() };
        let mm = s.client.matchmaking();
        let mut out: Vec<(u64, String, bool)> = Vec::new();
        let mut add = |id: u64, friend: bool, out: &mut Vec<(u64, String, bool)>| {
            if let Some(i) = out.iter().position(|(l, _, _)| *l == id) {
                out[i].2 |= friend;
                return;
            }
            let lobby = LobbyId::from_raw(id);
            if mm.lobby_data(lobby, "game").as_deref() != Some(super::GAME_KEY) {
                return;
            }
            if let Some(advert) = mm.lobby_data(lobby, "l4d2r").filter(|a| !a.is_empty()) {
                out.push((id, advert, friend));
            }
        };
        let listed = s.shared.lock().unwrap().listed.clone();
        for id in listed {
            add(id, false, &mut out);
        }
        let app = app_id();
        for friend in s.client.friends().get_friends(steamworks::FriendFlags::IMMEDIATE) {
            let Some(game) = friend.game_played() else { continue };
            let id = game.lobby.raw();
            if game.game.app_id().0 != app || id == 0 {
                continue;
            }
            if s.asked.lock().unwrap().insert(id) {
                unsafe {
                    steamworks::sys::SteamAPI_ISteamMatchmaking_RequestLobbyData(steamworks::sys::SteamAPI_SteamMatchmaking_v009(), id);
                }
            }
            add(id, true, &mut out);
        }
        out
    }
    /// Steam overlay: invite friends to the lobby.
    pub fn invite(lobby: u64) {
        if let Some(s) = steam() {
            s.client.friends().activate_invite_dialog(LobbyId::from_raw(lobby));
        }
    }
    /// "Join Game" in the Steam friends list launches/joins with this.
    pub fn set_presence(lobby: Option<u64>) {
        if let Some(s) = steam() {
            let connect = lobby.map(|l| format!("+connect_lobby {l}"));
            s.client.friends().set_rich_presence("connect", connect.as_deref());
            s.client.friends().set_rich_presence("status", lobby.map(|_| "In a lobby"));
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "android"))]
mod imp {
    use super::SteamEvent;
    pub fn available() -> bool {
        false
    }
    pub fn name() -> Option<String> {
        None
    }
    pub fn me() -> Option<u64> {
        None
    }
    pub fn avatar(_: u64) -> Option<(u32, u32, Vec<u8>)> {
        None
    }
    pub fn pump() {}
    pub fn events() -> Vec<SteamEvent> {
        Vec::new()
    }
    pub fn set_accepting(_: bool) {}
    pub fn take_new_peers() -> Vec<u64> {
        Vec::new()
    }
    pub fn send(_: u64, _: &[u8]) -> bool {
        false
    }
    pub fn take(_: u64) -> Option<Vec<Vec<u8>>> {
        None
    }
    pub fn forget(_: u64) {}
    pub fn create_lobby(_: u8) {}
    pub fn set_lobby_access(_: u64, _: u8) {}
    pub fn set_lobby_data(_: u64, _: &str, _: &str) {}
    pub fn advertise(_: u64, _: &str) {}
    pub fn request_lobbies() {}
    pub fn found_lobbies() -> Vec<(u64, String, bool)> {
        Vec::new()
    }
    pub fn join_lobby(_: u64) {}
    pub fn leave_lobby(_: u64) {}
    pub fn invite(_: u64) {}
    pub fn set_presence(_: Option<u64>) {}
}

/// Lobby data "game" of rewrite lobbies (app 480 is shared by many games).
pub const GAME_KEY: &str = "l4d2r";

pub use imp::*;
