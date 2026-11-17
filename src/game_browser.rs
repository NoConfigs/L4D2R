//! JOIN A GAME: games to join on the local network and on Steam, laid out
//! from retail resource/ui/l4d360ui/foundgames.res and foundgamelistitem.res
//! (PC entries) with the retail strings ("ALL JOINABLE GAMES FOUND",
//! "Searching for Games...", "No Games Found", "JOIN GAME", "CREATE LOBBY").
//!
//! Finding games:
//! - LAN: a hosting game answers UDP queries on port 27031 (`beacon`). The
//!   browser broadcasts a query, and also asks every address of its /24
//!   subnet directly: iOS needs a special entitlement to broadcast, but
//!   plain unicast works (with the local-network permission).
//! - Steam: a hosting game tags its Steam lobby (lobby data "game" =
//!   "l4d2r" plus the advert) and the browser lists tagged public lobbies
//!   worldwide, plus the lobbies of friends playing the rewrite (which also
//!   finds friends-only lobbies).
//!
//! Joining runs the console's `connect <ip:port>` or `connect_lobby <id>`;
//! the lobby opens when the host welcomes us.
//!
//! The 480-line retail coordinates convert to the 1280 x 720 menu canvas as
//! in game_lobby.rs; the FoundGames frame is 425 lines tall, centred.
use super::*;
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Mutex;

/// LAN discovery port (hosts on one computer take the next free ones).
pub const LAN_PORT: u16 = 27031;
const LAN_PORTS: u16 = 4;
const QUERY: &[u8] = b"L4D2R_QUERY 1";
/// Queries go out this often while the page is open.
const REFRESH: f64 = 3.;
/// A game not heard from for this long is dropped from the list.
const EXPIRE: f64 = 10.;
/// List rows: 10 retail rows; 8 taller ones on phones (easier to tap).
fn rows() -> usize {
    if crate::game::ui_fit::touch_build() { 8 } else { 10 }
}
fn row_tall() -> f32 {
    if crate::game::ui_fit::touch_build() { 31. } else { 25. }
}
/// Button height: retail 15; phones 24.
fn button_tall() -> f32 {
    if crate::game::ui_fit::touch_build() { 24. } else { 15. }
}

/// What a hosting game tells the browser.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Advert {
    /// Random per run: the same game seen over LAN and Steam shows once.
    pub id: u64,
    pub name: String,
    pub campaign: String,
    pub chapter: String,
    /// The chapter picture (Chapter::image).
    pub image: String,
    pub difficulty: String,
    /// "Campaign" or "Survival".
    pub mode: String,
    pub players: usize,
    pub max: usize,
    /// A game is running (joiners go straight in).
    pub playing: bool,
    /// TCP port for `connect`.
    pub port: u16,
    /// 0 public, 1 friends only.
    pub access: u8,
}
impl Advert {
    pub fn to_json(&self) -> String {
        json!({
            "t": "l4d2r", "v": crate::multiplayer::PROTOCOL, "id": self.id.to_string(),
            "name": self.name, "campaign": self.campaign, "chapter": self.chapter, "image": self.image,
            "diff": self.difficulty, "mode": self.mode, "n": self.players, "max": self.max,
            "playing": self.playing, "port": self.port, "access": self.access,
        })
        .to_string()
    }
    pub fn parse(text: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(text).ok()?;
        if v.get("t")?.as_str()? != "l4d2r" || v.get("v")?.as_u64()? != crate::multiplayer::PROTOCOL {
            return None;
        }
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Some(Self {
            id: s("id").parse().ok()?,
            name: s("name"),
            campaign: s("campaign"),
            chapter: s("chapter"),
            image: s("image"),
            difficulty: s("diff"),
            mode: s("mode"),
            players: n("n") as usize,
            max: n("max") as usize,
            playing: v.get("playing").and_then(Value::as_bool).unwrap_or(false),
            port: n("port") as u16,
            access: n("access") as u8,
        })
    }
}

/// Where a found game is joined.
#[derive(Clone, Debug, PartialEq)]
pub enum Where {
    Lan(SocketAddr),
    Steam(u64),
}

#[derive(Clone, Debug)]
pub struct Found {
    pub advert: Advert,
    pub at: Where,
    /// A Steam friend is in it.
    pub friend: bool,
    seen: f64,
}
impl Found {
    fn command(&self) -> String {
        match &self.at {
            Where::Lan(addr) => format!("connect {addr}"),
            Where::Steam(id) => format!("connect_lobby {id}"),
        }
    }
    /// The access line: retail L4D360UI_FoundGames_Public / _FriendsOnly,
    /// "LAN Game" for local ones.
    fn access(&self) -> &'static str {
        match self.at {
            Where::Lan(_) => "LAN Game",
            Where::Steam(_) if self.friend || self.advert.access == 1 => "Friend's Game",
            Where::Steam(_) => "Public Game",
        }
    }
    fn full(&self) -> bool {
        self.advert.max > 0 && self.advert.players >= self.advert.max
    }
}

/// The advert of the game we host (multiplayer.rs keeps it current; None
/// when not hosting or Private).
pub static ADVERT: Mutex<Option<Advert>> = Mutex::new(None);

/// This run's advert id.
pub fn host_id() -> u64 {
    static ID: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        (t ^ ((std::process::id() as u64) << 32)).max(1)
    })
}

/// Adds or refreshes a found game. The same game (advert id) is kept once,
/// preferring a LAN address (direct) over Steam, and a LAN address other
/// than loopback.
fn merge(list: &mut Vec<Found>, found: Found) {
    if found.advert.id == host_id() {
        return;
    }
    let rank = |f: &Found| match &f.at {
        Where::Lan(a) if !a.ip().is_loopback() => 0,
        Where::Lan(_) => 1,
        Where::Steam(_) => 2,
    };
    if let Some(old) = list.iter_mut().find(|f| f.advert.id == found.advert.id) {
        let friend = old.friend || found.friend;
        if rank(&found) <= rank(old) {
            *old = found;
        } else {
            old.seen = found.seen;
            old.advert = found.advert;
        }
        old.friend = friend;
        return;
    }
    println!("Browser: found {}'s game ({:?})", found.advert.name, found.at);
    list.push(found);
}

/// The addresses a LAN query goes to: broadcast, loopback, and every
/// address of our /24 (all on the first discovery port; broadcast and
/// loopback also on the others).
fn query_targets(local: Option<Ipv4Addr>) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for port in LAN_PORT..LAN_PORT + LAN_PORTS {
        out.push(SocketAddr::from((Ipv4Addr::BROADCAST, port)));
        out.push(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
        if let Some(ip) = local {
            let [a, b, c, _] = ip.octets();
            out.push(SocketAddr::from((Ipv4Addr::new(a, b, c, 255), port)));
        }
    }
    if let Some(ip) = local {
        let [a, b, c, _] = ip.octets();
        for d in 1..=254u8 {
            out.push(SocketAddr::from((Ipv4Addr::new(a, b, c, d), LAN_PORT)));
        }
    }
    out
}

/// The browser's state while JOIN A GAME is open.
#[derive(Resource, Default)]
pub(crate) struct Browser {
    pub found: Vec<Found>,
    pub selected: Option<usize>,
    socket: Option<UdpSocket>,
    last_query: Option<f64>,
    opened: f64,
    /// JOIN GAME was used: show the connection status line.
    joining: bool,
}

/// Host side: answers LAN queries with our advert while hosting.
fn beacon(mut socket: Local<Option<UdpSocket>>) {
    let advert = ADVERT.lock().unwrap().clone();
    let Some(advert) = advert else {
        *socket = None;
        return;
    };
    if socket.is_none() {
        *socket = (LAN_PORT..LAN_PORT + LAN_PORTS).find_map(|port| {
            let s = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], port))).ok()?;
            s.set_nonblocking(true).ok()?;
            Some(s)
        });
        if socket.is_none() {
            return;
        }
    }
    let Some(s) = socket.as_ref() else { return };
    let mut buf = [0u8; 256];
    let reply = advert.to_json();
    while let Ok((n, from)) = s.recv_from(&mut buf) {
        if &buf[..n] == QUERY {
            let _ = s.send_to(reply.as_bytes(), from);
        }
    }
}

/// Browser side: queries the LAN and Steam while the page is open and
/// collects the answers.
fn search(frontend: Option<Res<Frontend>>, mut browser: ResMut<Browser>, time: Res<Time<Real>>) {
    let now = time.elapsed_secs_f64();
    if frontend.is_none_or(|f| f.page != Page::Browser) {
        if browser.socket.is_some() || browser.last_query.is_some() {
            *browser = Browser::default();
        }
        return;
    }
    let browser = &mut *browser;
    if browser.socket.is_none() {
        browser.opened = now;
        browser.socket = UdpSocket::bind("0.0.0.0:0").ok().filter(|s| s.set_nonblocking(true).is_ok());
        if let Some(s) = &browser.socket {
            let _ = s.set_broadcast(true);
        }
    }
    if browser.last_query.is_none_or(|t| now - t >= REFRESH) {
        browser.last_query = Some(now);
        if let Some(s) = &browser.socket {
            let local = match crate::multiplayer::local_ip() {
                Some(IpAddr::V4(ip)) => Some(ip),
                _ => None,
            };
            for to in query_targets(local) {
                let _ = s.send_to(QUERY, to);
            }
        }
        crate::steam::request_lobbies();
    }
    if let Some(s) = &browser.socket {
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = s.recv_from(&mut buf) {
            let Some(advert) = std::str::from_utf8(&buf[..n]).ok().and_then(Advert::parse) else { continue };
            let at = Where::Lan(SocketAddr::new(from.ip(), advert.port));
            merge(&mut browser.found, Found { advert, at, friend: false, seen: now });
        }
    }
    for (lobby, text, friend) in crate::steam::found_lobbies() {
        if let Some(advert) = Advert::parse(&text) {
            merge(&mut browser.found, Found { advert, at: Where::Steam(lobby), friend, seen: now });
        }
    }
    let selected = browser.selected.and_then(|i| browser.found.get(i)).map(|f| f.advert.id);
    browser.found.retain(|f| now - f.seen < EXPIRE);
    // Friends first, then games with room, then by name.
    browser.found.sort_by_key(|f| (!f.friend, f.full(), f.advert.name.to_lowercase()));
    browser.selected = selected
        .and_then(|id| browser.found.iter().position(|f| f.advert.id == id))
        .or(if browser.found.is_empty() { None } else { Some(0) });
    browser.found.truncate(rows());
    // `--capture-browser-join`: join the first game found.
    if !browser.joining && !browser.found.is_empty() && std::env::args().any(|a| a == "--capture-browser-join") {
        join(browser);
    }
}

/// JOIN GAME on the selected row.
pub(crate) fn join(browser: &mut Browser) {
    let Some(found) = browser.selected.and_then(|i| browser.found.get(i)) else { return };
    if found.full() {
        return;
    }
    let command = found.command();
    browser.joining = true;
    if let Some(reply) = crate::multiplayer::command(&command) {
        println!("Browser: {command}: {reply}");
    }
}

#[derive(Component)]
pub(crate) struct BrowserRow(usize);
/// The red selection bar of a row (FoundGameListItem selected_color).
#[derive(Component)]
pub(crate) struct BrowserRowMark(usize);
/// Row columns: 0 name, 1 difficulty, 2 players, 3 where.
#[derive(Component)]
pub(crate) struct BrowserCell(usize, u8);
/// Fields: 0 count/searching line, 1 "No Games Found", 2.. the details.
#[derive(Component)]
pub(crate) struct BrowserText(u8);
#[derive(Component)]
pub(crate) struct BrowserPoster;
/// Parts shown only with a game selected (details, JOIN GAME).
#[derive(Component)]
pub(crate) struct BrowserDetail;

const COUNT: u8 = 0;
const NONE_FOUND: u8 = 1;
const DETAILS: u8 = 2; // ..= 8
const STATUS: u8 = 9;

/// Frame top on the 480-line screen: (480 - 425) / 2.
const FRAME_Y: f32 = 27.5;
fn r(x: f32, y: f32, w: f32, h: f32) -> Node {
    rect(640. + 1.5 * x, 1.5 * (y + FRAME_Y), 1.5 * w, 1.5 * h)
}
fn small(font: &Handle<Font>, size: f32) -> TextFont {
    TextFont { font: bevy::text::FontSource::Handle(font.clone()), font_size: FontSize::Vh(size / 7.2), ..default() }
}
/// l4d360uischeme TextYellow.
const TEXT_YELLOW: Color = Color::srgb(110. / 255., 110. / 255., 84. / 255.);
const LIST_X: f32 = -320.;
const LIST_Y: f32 = 110.;

pub(super) fn setup(
    commands: &mut Commands,
    root: Entity,
    font: &Handle<Font>,
    assets: &mut SourceAssets,
    images: &mut Assets<Image>,
) {
    let panel = commands
        .spawn((
            Node { width: percent(100.), height: percent(100.), display: Display::None, ..default() },
            PageNode(Page::Browser),
            ChildOf(root),
        ))
        .id();
    // ImgBackground (L4DMenuBackground), as in the lobby.
    commands.spawn((
        rect(0., 0., 1280., 720.),
        BackgroundGradient::from(LinearGradient {
            angle: LinearGradient::TO_BOTTOM,
            stops: [(0.85, 0.), (0.85, 5.), (0.55, 9.), (0.55, 30.), (0.88, 46.), (0.97, 70.), (0.97, 100.)]
                .iter()
                .map(|&(alpha, at)| ColorStop::percent(Color::BLACK.with_alpha(alpha), at))
                .collect(),
            color_space: InterpolationColorSpace::Srgba,
        }),
        ChildOf(panel),
    ));
    commands.spawn((Text::new("ALL JOINABLE GAMES FOUND"), small(font, 36.), rect(129., 60., 900., 40.), ChildOf(panel)));
    // L4D360UI_FoundGames_Description.
    commands.spawn((
        Text::new("Select an active game to begin playing with a friend."),
        small(font, 19.),
        TextColor(Color::srgb(0.7, 0.7, 0.7)),
        rect(129., 100., 900., 24.),
        ChildOf(panel),
    ));
    // Searching / game count, over the list.
    commands.spawn((
        Text::new(""),
        small(font, 19.),
        TextColor(TEXT_YELLOW),
        r(LIST_X, 94., 380., 14.),
        BrowserText(COUNT),
        ChildOf(panel),
    ));
    // GplGames: the list (bgcolor 32 32 32), between the two dividers.
    commands.spawn((r(LIST_X, LIST_Y, 380., 247.), BackgroundColor(Color::srgb_u8(32, 32, 32)), ChildOf(panel)));
    let divider = image(assets, images, "materials/vgui/divider_gradient.vtf");
    for y in [LIST_Y, 357.] {
        commands.spawn((ImageNode::new(divider.clone()), r(LIST_X, y, 380., 2.), ChildOf(panel)));
    }
    commands.spawn((
        Text::new("No Games Found"),
        small(font, 21.),
        TextColor(Color::srgb(0.6, 0.6, 0.6)),
        Node { justify_content: JustifyContent::Center, ..r(LIST_X, 220., 380., 20.) },
        TextLayout::justify(Justify::Center),
        BrowserText(NONE_FOUND),
        ChildOf(panel),
    ));
    let row_tall = row_tall();
    for row in 0..rows() {
        let button = commands
            .spawn((
                Button,
                r(LIST_X + 2., LIST_Y + 3. + row_tall * row as f32, 376., row_tall - 1.),
                BackgroundColor(Color::NONE),
                Action(ButtonAction::BrowserPick(row)),
                BrowserRow(row),
                ChildOf(panel),
            ))
            .id();
        commands.spawn((
            Node { position_type: PositionType::Absolute, width: percent(100.), height: percent(100.), ..default() },
            BackgroundColor(Color::NONE),
            ZIndex(-1),
            Pickable::IGNORE,
            BrowserRowMark(row),
            ChildOf(button),
        ));
        // LblGamerTag x 41, LblDifficulty x 175, LblNumPlayers x 300,
        // LblPing x 340 (here: LAN or Steam).
        for (column, x, w) in [(0u8, 6., 165.), (1, 175., 120.), (2, 300., 40.), (3, 336., 40.)] {
            commands.spawn((
                Text::new(""),
                small(font, 18.),
                TextColor(Color::WHITE),
                Node { position_type: PositionType::Absolute, left: vh(1.5 * x / 7.2), top: vh(1.5 * 4. / 7.2), width: vh(1.5 * w / 7.2), overflow: Overflow::clip(), ..default() },
                Pickable::IGNORE,
                BrowserCell(row, column),
                ChildOf(button),
            ));
        }
    }
    // Details of the selected game (x c+90): picture in its frame, then
    // the yellow lines (LblCampaign 235 .. LblNumPlayers 325).
    // The picture inside campaignframe.vtf (the lobby's 125 x 65 in 158 x 79,
    // scaled to this 196 x 98 frame).
    commands.spawn((ImageNode::default(), r(92.5, 140.5, 155., 80.5), BrowserPoster, BrowserDetail, ChildOf(panel)));
    let frame = image(assets, images, "materials/vgui/campaignframe.vtf");
    commands.spawn((ImageNode::new(frame), r(85., 132., 196., 98.), BrowserDetail, ChildOf(panel)));
    for line in 0..7u8 {
        commands.spawn((
            Text::new(""),
            small(font, 19.),
            TextColor(TEXT_YELLOW),
            r(90., 235. + 15. * line as f32, 230., 14.),
            BrowserText(DETAILS + line),
            BrowserDetail,
            ChildOf(panel),
        ));
    }
    // BtnJoinSelected with IconForwardArrow.
    let arrow = image(assets, images, "materials/vgui/icon_button_arrow_right.vtf");
    let tall = button_tall();
    commands.spawn((ImageNode::new(arrow), r(75., 345. + (tall - 15.) / 2., 15., 15.), BrowserDetail, ChildOf(panel)));
    let join = hybrid_button(commands, panel, "JOIN GAME", font, r(90., 345., 200., tall), ButtonAction::BrowserJoin, Hybrid::Flyout);
    commands.entity(join).insert(BrowserDetail);
    // Connecting / failure line under JOIN GAME.
    commands.spawn((
        Text::new(""),
        small(font, 17.),
        TextColor(Color::srgb(0.75, 0.75, 0.75)),
        r(90., 347. + tall, 260., 30.),
        BrowserText(STATUS),
        ChildOf(panel),
    ));
    // DrpCreateGame and BtnCancel with their arrows.
    let back_arrow = image(assets, images, "materials/vgui/icon_button_arrow_left.vtf");
    if crate::game::ui_fit::touch_build() {
        // Phones: BACK and CREATE LOBBY side by side under the list, far
        // enough apart to tap without hitting the other.
        commands.spawn((ImageNode::new(back_arrow), r(-315., 368. + 4.5, 15., 15.), ChildOf(panel)));
        hybrid_button(commands, panel, "BACK", font, r(-298., 368., 120., tall), ButtonAction::Back, Hybrid::Flyout);
        hybrid_button(commands, panel, "CREATE LOBBY", font, r(-120., 368., 180., tall), ButtonAction::Campaign, Hybrid::Flyout);
    } else {
        commands.spawn((ImageNode::new(back_arrow), r(-265., 390., 15., 15.), ChildOf(panel)));
        hybrid_button(commands, panel, "CREATE LOBBY", font, r(-250., 372., 290., 15.), ButtonAction::Campaign, Hybrid::Flyout);
        hybrid_button(commands, panel, "BACK", font, r(-250., 390., 180., 15.), ButtonAction::Back, Hybrid::Flyout);
    }
}

/// Fills the list and the details from `Browser`.
#[allow(clippy::type_complexity)]
pub(crate) fn presentation(
    frontend: Option<Res<Frontend>>,
    browser: Res<Browser>,
    art: Option<Res<MenuArt>>,
    time: Res<Time<Real>>,
    mut texts: Query<(&mut Text, Option<&BrowserText>, Option<&BrowserCell>, &mut TextColor)>,
    mut rows: Query<(&BrowserRow, &mut Node), (Without<BrowserDetail>, Without<BrowserText>)>,
    mut marks: Query<(&BrowserRowMark, &mut BackgroundColor)>,
    mut detail_nodes: Query<&mut Node, (With<BrowserDetail>, Without<BrowserRow>)>,
    mut poster: Query<&mut ImageNode, With<BrowserPoster>>,
) {
    let (Some(frontend), Some(art)) = (frontend, art) else { return };
    if frontend.page != Page::Browser {
        return;
    }
    let now = time.elapsed_secs_f64();
    let selected = browser.selected.and_then(|i| browser.found.get(i));
    let count = browser.found.len();
    // Retail: "Searching for Games..." until something answers, then
    // L4D360UI_FoundGames_GameCount ("%s1 Game(s)").
    let count_line = if count == 0 {
        if now - browser.opened < 4. { "Searching for Games...".to_string() } else { String::new() }
    } else if count == 1 {
        "1 Game".to_string()
    } else {
        format!("{count} Games")
    };
    let details: [String; 7] = selected.map_or_else(Default::default, |f| {
        let a = &f.advert;
        [
            a.campaign.clone(),
            a.chapter.clone(),
            format!("Lobby Leader: {}", a.name),
            if a.playing { "Game in Progress".into() } else { "In Lobby".into() },
            format!("Access: {}", f.access()),
            format!("Difficulty: {} ({})", a.difficulty, a.mode),
            format!("{} of {} Players", a.players, a.max),
        ]
    });
    let status = if browser.joining {
        crate::multiplayer::command("status").unwrap_or_default()
    } else if selected.is_some_and(Found::full) {
        "Game has no empty player slots.".to_string()
    } else {
        String::new()
    };
    for (mut text, field, cell, mut color) in &mut texts {
        let value = if let Some(field) = field {
            match field.0 {
                COUNT => count_line.clone(),
                NONE_FOUND => String::new(),
                STATUS => status.clone(),
                n => details.get((n - DETAILS) as usize).cloned().unwrap_or_default(),
            }
        } else if let Some(cell) = cell {
            let Some(f) = browser.found.get(cell.0) else {
                if !text.0.is_empty() {
                    text.0.clear();
                }
                continue;
            };
            let full = f.full();
            let grey = if full { Color::srgb(0.45, 0.45, 0.45) } else { Color::WHITE };
            if color.0 != grey {
                color.0 = grey;
            }
            match cell.1 {
                0 => f.advert.name.clone(),
                1 => f.advert.difficulty.clone(),
                2 => format!("{}/{}", f.advert.players, f.advert.max),
                _ => match f.at {
                    Where::Lan(_) => "LAN".into(),
                    Where::Steam(_) => "Steam".into(),
                },
            }
        } else {
            continue;
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    // "No Games Found" once the first search had time to answer.
    for (mut text, field, _, _) in &mut texts {
        if field.is_some_and(|f| f.0 == NONE_FOUND) {
            let value = if count == 0 && now - browser.opened >= 4. { "No Games Found" } else { "" };
            if text.0 != value {
                text.0 = value.into();
            }
        }
    }
    for (row, mut node) in &mut rows {
        let display = if row.0 < count { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    for (mark, mut bg) in &mut marks {
        // FoundGameListItem selected_color 168 26 26.
        let colour = if browser.selected == Some(mark.0) { Color::srgb_u8(168, 26, 26) } else { Color::NONE };
        if bg.0 != colour {
            bg.0 = colour;
        }
    }
    let display = if selected.is_some() { Display::Flex } else { Display::None };
    for mut node in &mut detail_nodes {
        if node.display != display {
            node.display = display;
        }
    }
    if let Some(handle) = selected.and_then(|f| art.chapters.get(&f.advert.image)) {
        for mut image in &mut poster {
            if image.image != *handle {
                image.image = handle.clone();
            }
        }
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Browser>()
        .add_systems(Update, (beacon, search).chain().after(crate::multiplayer::run))
        .add_systems(Update, presentation.after(search).after(crate::game_menu::update));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advert(id: u64) -> Advert {
        Advert {
            id,
            name: "Host".into(),
            campaign: "Dead Center".into(),
            chapter: "1: Hotel".into(),
            image: "maps/c1m1_hotel".into(),
            difficulty: "Normal".into(),
            mode: "Campaign".into(),
            players: 1,
            max: 4,
            playing: false,
            port: 27015,
            access: 0,
        }
    }

    #[test]
    fn adverts_round_trip_and_reject_other_protocols() {
        let a = advert(42);
        assert_eq!(Advert::parse(&a.to_json()), Some(a.clone()));
        let other = a.to_json().replace(&format!("\"v\":{}", crate::multiplayer::PROTOCOL), "\"v\":999");
        assert_eq!(Advert::parse(&other), None);
        assert_eq!(Advert::parse("hello"), None);
    }

    #[test]
    fn a_game_seen_twice_is_listed_once_preferring_the_lan_address() {
        let mut list = Vec::new();
        let steam = Found { advert: advert(7), at: Where::Steam(99), friend: true, seen: 0. };
        let lan = Found { advert: advert(7), at: Where::Lan("192.168.1.5:27015".parse().unwrap()), friend: false, seen: 1. };
        let loopback = Found { advert: advert(7), at: Where::Lan("127.0.0.1:27015".parse().unwrap()), friend: false, seen: 2. };
        merge(&mut list, steam);
        merge(&mut list, lan.clone());
        merge(&mut list, loopback);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].at, lan.at);
        assert!(list[0].friend, "still marked as a friend's game");
        assert_eq!(list[0].seen, 2.);
        // Our own game is never listed.
        merge(&mut list, Found { advert: advert(host_id()), at: Where::Steam(1), friend: false, seen: 0. });
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn lan_queries_reach_broadcast_loopback_and_the_whole_subnet() {
        let targets = query_targets(Some(Ipv4Addr::new(192, 168, 1, 20)));
        assert!(targets.contains(&"255.255.255.255:27031".parse().unwrap()));
        assert!(targets.contains(&"127.0.0.1:27034".parse().unwrap()));
        assert!(targets.contains(&"192.168.1.255:27031".parse().unwrap()));
        assert!(targets.contains(&"192.168.1.1:27031".parse().unwrap()));
        assert!(targets.contains(&"192.168.1.254:27031".parse().unwrap()));
        assert!(!targets.contains(&"192.168.1.0:27031".parse().unwrap()));
    }

    #[test]
    fn the_beacon_answers_a_query_with_the_advert() {
        let host = UdpSocket::bind("127.0.0.1:0").unwrap();
        let guest = UdpSocket::bind("127.0.0.1:0").unwrap();
        guest.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        guest.send_to(QUERY, host.local_addr().unwrap()).unwrap();
        let mut buf = [0u8; 512];
        let (n, from) = host.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], QUERY);
        host.send_to(advert(5).to_json().as_bytes(), from).unwrap();
        let (n, _) = guest.recv_from(&mut buf).unwrap();
        assert_eq!(Advert::parse(std::str::from_utf8(&buf[..n]).unwrap()).map(|a| a.id), Some(5));
    }
}
