//! Pre-game lobby, laid out from retail resource/ui/l4d360ui/gamelobby.res
//! (PC entries) with the retail strings from l4d360ui_english.txt: title
//! ("CAMPAIGN LOBBY"), leader line, campaign picture in its frame, five
//! yellow summary lines, the button column (EDIT GAME SETTINGS, CHOOSE
//! CHARACTER, INVITE FRIENDS, PERMISSIONS, LEAVE LOBBY, START THE GAME),
//! the four player slots and the lobby chat ("Say:").
//!
//! There is no online multiplayer yet: the lobby holds the local player and
//! three open slots ("Player Slot Available"), and chat is local. The
//! `Lobby` resource is the place a network layer can add remote players and
//! chat lines later.
//!
//! The 480-line retail coordinates convert to this menu's 1280 x 720 canvas
//! as x' = 640 + 1.5 * (x - c), y' = 1.5 * (y + 20) (the frame is centred).
use super::*;
use crate::game_chat::{ChatHistory, ChatLine, Team};
use bevy::input::keyboard::KeyboardInput;

/// L4D360UI_Access_*: who may join.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Access {
    #[default]
    Public,
    Friends,
    Private,
}
impl Access {
    const ALL: [Access; 3] = [Access::Public, Access::Friends, Access::Private];
    fn label(self) -> &'static str {
        match self {
            Access::Public => "Public Game",
            Access::Friends => "Friends-Only Game",
            Access::Private => "Private Game",
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct Lobby {
    pub access: Access,
    pub chat: ChatHistory,
    pub input: String,
    /// Open drop-down: 0 character, 1 permissions.
    pub open: Option<u8>,
    /// The local player has "joined" (the joined notice was posted).
    pub joined: bool,
    /// Signals for multiplayer.rs: the lobby was just created, left, or
    /// started by us (the leader).
    pub created: bool,
    pub left: bool,
    pub started: bool,
    /// Set by multiplayer.rs when the leader starts: run START GAME here.
    pub pending_start: bool,
    /// Lobby chat lines we typed, for multiplayer.rs to send.
    pub outgoing: Vec<String>,
    /// Filled by multiplayer.rs while online: (name, survivor, SteamID64 or
    /// 0) per member, leader first.
    pub members: Vec<(String, String, u64)>,
    pub leader: String,
    /// We joined someone else's lobby (only the leader starts and edits).
    pub client: bool,
    /// "connect" address to share while hosting.
    pub address: String,
}

#[derive(Component)]
pub(crate) struct LobbyText(u8);
#[derive(Component)]
pub(crate) struct LobbyPoster;
#[derive(Component)]
pub(crate) struct LobbySlot(usize);
#[derive(Component)]
pub(crate) struct LobbySlotPortrait(usize);
#[derive(Component)]
pub(crate) struct LobbyChatLine(usize);
/// Speaker and avatar of a player row (hidden on open slots).
#[derive(Component)]
pub(crate) struct LobbySlotPart(usize);
/// The Steam avatar picture inside a player row's avatar square
/// (PnlGamerPic); transparent until the avatar is loaded.
#[derive(Component)]
pub(crate) struct LobbySlotAvatar(usize);
#[derive(Component)]
pub(crate) struct LobbyChatInput;
#[derive(Component)]
pub(crate) struct LobbyDrop(u8);
#[derive(Component)]
pub(crate) struct LobbyAccessRow(usize);

/// Text fields filled by `presentation`.
const TITLE: u8 = 0;
const LEADER: u8 = 1;
const SUMMARY: u8 = 2; // ..= 6
const SLOTS: usize = 4;
const CHAT_LINES: usize = 5;

/// The GameLobby frame is 440 lines tall, centred on the 480-line screen,
/// so every control sits 20 lines below its .res ypos (checked against a
/// retail 1920 x 1080 capture).
const FRAME_Y: f32 = 20.;

/// Retail 480-line PC coordinates centred on c, inside the frame.
fn r(x: f32, y: f32, w: f32, h: f32) -> Node {
    rect(640. + 1.5 * x, 1.5 * (y + FRAME_Y), 1.5 * w, 1.5 * h)
}

fn small(font: &Handle<Font>, size: f32) -> TextFont {
    TextFont {
        font: bevy::text::FontSource::Handle(font.clone()),
        font_size: FontSize::Vh(size / 7.2),
        ..default()
    }
}

/// One 720-line pixel (vgui draws 1-pixel lines at any resolution; this
/// scales with the window like the rest of the menu).
fn line_px(n: f32) -> Val {
    vh(n / 7.2)
}

/// l4d360uischeme TextYellow.
const TEXT_YELLOW: Color = Color::srgb(110. / 255., 110. / 255., 84. / 255.);
/// Player names and the ready box (retail capture: grey ~140).
const SLOT_GREY: Color = Color::srgb(145. / 255., 145. / 255., 145. / 255.);
/// Chat box outline and divider (retail capture: grey ~60).
const CHAT_LINE: Color = Color::srgb(64. / 255., 64. / 255., 64. / 255.);
/// Player row: SurvivorListItem is 28 tall; rows are 32 apart. The first
/// starts at y 110 (GplSurvivors y 100 plus its inset), x c+38.
const ROW_X: f32 = 38.;
const ROW_Y: f32 = 110.;
const ROW_STEP: f32 = 32.;

/// A chat scroll-bar button (chatscheme: 12 wide, ScrollBarGrey with the
/// raised ScrollBarButtonBorder and a ScrollBarHilight arrow).
fn scroll_button(commands: &mut Commands, parent: Entity, node: Node, up: bool) {
    let button = commands
        .spawn((
            Node {
                border: UiRect::all(line_px(1.)),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..node
            },
            BackgroundColor(Color::srgba(51. / 255., 51. / 255., 51. / 255., 192. / 255.)),
            BorderColor {
                top: Color::srgb(110. / 255., 110. / 255., 110. / 255.),
                left: Color::srgb(110. / 255., 110. / 255., 110. / 255.),
                bottom: Color::srgb(38. / 255., 38. / 255., 38. / 255.),
                right: Color::srgb(38. / 255., 38. / 255., 38. / 255.),
            },
            ChildOf(parent),
        ))
        .id();
    // The arrow: four rows narrowing to a point.
    let rows: [f32; 4] = if up { [1., 3., 5., 7.] } else { [7., 5., 3., 1.] };
    for w in rows {
        commands.spawn((
            Node { width: line_px(w), height: line_px(1.), ..default() },
            BackgroundColor(Color::srgb(110. / 255., 110. / 255., 110. / 255.)),
            ChildOf(button),
        ));
    }
}

pub(super) fn setup(
    commands: &mut Commands,
    root: Entity,
    font: &Handle<Font>,
    chat_font: &Handle<Font>,
    assets: &mut SourceAssets,
    images: &mut Assets<Image>,
) {
    let panel = commands
        .spawn((
            Node {
                width: percent(100.),
                height: percent(100.),
                display: Display::None,
                ..default()
            },
            PageNode(Page::Lobby),
            ChildOf(root),
        ))
        .id();
    // ImgBackground (L4DMenuBackground) darkens the menu scene with no hard
    // edges. Brightness down a retail capture: a dark strip under the
    // title, the scene dimmed to about half, then near-black from below
    // the summary down.
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
    // Frame title at the top left (retail capture: x 86, caps 46-62 lines).
    commands.spawn((Text::new("CAMPAIGN LOBBY"), small(font, 36.), rect(129., 60., 600., 40.), LobbyText(TITLE), ChildOf(panel)));
    // LblLeaderLine: 32 tall, text centred vertically.
    commands.spawn((
        Text::new(""),
        small(font, 20.),
        TextColor(Color::WHITE),
        r(-240., 108., 275., 16.),
        LobbyText(LEADER),
        ChildOf(panel),
    ));
    commands.spawn((ImageNode::default(), r(-244., 132., 125., 65.), LobbyPoster, ChildOf(panel)));
    let frame = image(assets, images, "materials/vgui/campaignframe.vtf");
    commands.spawn((ImageNode::new(frame), r(-250., 125., 158., 79.), ChildOf(panel)));
    for line in 0..5u8 {
        commands.spawn((
            Text::new(""),
            small(font, 19.),
            TextColor(TEXT_YELLOW),
            r(-105., 128. + 15. * line as f32, 240., 16.),
            LobbyText(SUMMARY + line),
            ChildOf(panel),
        ));
    }
    // Button column: icons at c-255, buttons at c-240, 180 x 15.
    let rows: [(&str, &str, f32, ButtonAction); 6] = [
        ("icon_button_settings", "EDIT GAME SETTINGS", 206., ButtonAction::EditLobbySettings),
        ("icon_button_character", "CHOOSE CHARACTER", 226., ButtonAction::LobbyDrop(0)),
        ("icon_button_friends", "INVITE FRIENDS", 246., ButtonAction::LobbyInvite),
        ("icon_button_permissions", "PERMISSIONS", 266., ButtonAction::LobbyDrop(1)),
        // L4D360UI_Lobby_LeaveLobby
        ("icon_button_arrow_left", "LEAVE THIS LOBBY", 286., ButtonAction::LeaveLobby),
        ("icon_button_arrow_right", "START THE GAME", 306., ButtonAction::LobbyStart),
    ];
    for (icon, label, y, action) in rows {
        let icon = image(assets, images, &format!("materials/vgui/{icon}.vtf"));
        commands.spawn((ImageNode::new(icon), r(-255., y, 15., 15.), ChildOf(panel)));
        if matches!(action, ButtonAction::LobbyStart) {
            // BtnStartGame fgcolor_override 255 12 12 (PC). Not a Hybrid
            // button: its hover painting would replace the red.
            let button = commands
                .spawn((Button, r(-240., y, 180., 15.), BackgroundColor(Color::NONE), Action(action), LobbyStartButton, ChildOf(panel)))
                .id();
            commands.spawn((Text::new(label), small(font, 19.), TextColor(Color::srgb_u8(255, 12, 12)), ChildOf(button)));
        } else {
            hybrid_button(commands, panel, label, font, r(-240., y, 180., 15.), action, Hybrid::Flyout);
        }
    }
    // CHOOSE CHARACTER and PERMISSIONS drop-downs (FlyoutMenu), opened to
    // the right of their buttons.
    for (drop, y, count) in [(0u8, 226., 4usize), (1u8, 266., 3usize)] {
        let list = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    display: Display::None,
                    ..r(-55., y, 150., 15. * count as f32)
                },
                BackgroundColor(Color::srgb(0.03, 0.03, 0.03)),
                GlobalZIndex(120),
                LobbyDrop(drop),
                ChildOf(panel),
            ))
            .id();
        for index in 0..count {
            let action = if drop == 0 { ButtonAction::Choose(3, index) } else { ButtonAction::LobbyAccess(index as u8) };
            let row = commands
                .spawn((
                    Button,
                    Node {
                        width: percent(100.),
                        height: vh(22.5 / 7.2),
                        padding: UiRect::horizontal(vh(8. / 7.2)),
                        ..default()
                    },
                    Action(action),
                    BackgroundColor(Color::NONE),
                    LobbyAccessRow(drop as usize * 10 + index),
                    ChildOf(list),
                ))
                .id();
            commands.spawn((Text::new(""), small(font, 19.), ChoiceText(drop, index), ChildOf(row)));
        }
    }
    // Player slots (SurvivorListItem.res): ImgReady (2, 8), the voice status
    // (20, 10), ImgPlayerPortrait (40, 0) 28 x 28, PnlGamerPic (72, 11)
    // 16 x 16, and the name drop-down (91, 12) with its ">" indicator.
    let voice = image(assets, images, "materials/vgui/icon_button_voice.vtf");
    for slot in 0..SLOTS {
        let y = ROW_Y + slot as f32 * ROW_STEP;
        // Ready box: an empty grey square (nobody readies in a campaign lobby).
        commands.spawn((
            Node { border: UiRect::all(line_px(1.5)), ..r(ROW_X + 1.5, y + 8., 8., 8.) },
            BorderColor::all(SLOT_GREY),
            ChildOf(panel),
        ));
        commands.spawn((
            ImageNode::new(voice.clone()).with_color(Color::srgb(150. / 255., 150. / 255., 150. / 255.)),
            r(ROW_X + 18., y + 6., 14., 14.),
            LobbySlotPart(slot),
            ChildOf(panel),
        ));
        commands.spawn((ImageNode::default(), r(ROW_X + 40., y, 28., 28.), LobbySlotPortrait(slot), ChildOf(panel)));
        // The avatar square (PnlGamerPic): grey until `avatars` puts the
        // player's Steam picture in it.
        let square = commands
            .spawn((
                Node { border: UiRect::all(line_px(1.)), ..r(ROW_X + 72., y + 6., 16., 16.) },
                BackgroundColor(Color::srgb(0.17, 0.17, 0.17)),
                BorderColor::all(Color::srgb(0.35, 0.35, 0.35)),
                LobbySlotPart(slot),
                ChildOf(panel),
            ))
            .id();
        commands.spawn((
            ImageNode { color: Color::NONE, ..default() },
            Node { width: percent(100.), height: percent(100.), ..default() },
            LobbySlotAvatar(slot),
            ChildOf(square),
        ));
        commands.spawn((
            Text::new(""),
            small(font, 19.),
            TextColor(SLOT_GREY),
            TextLayout::linebreak(bevy::text::LineBreak::NoWrap),
            r(ROW_X + 91., y + 5., 180., 15.),
            LobbySlot(slot),
            ChildOf(panel),
        ));
    }
    // Lobby chat. ChatBackground c-240/326 261 x 70; LobbyChatHistory
    // c-240/327 260 x 57 (black RichText, ChatFont, south-west, chatscheme
    // 12-wide scroll bar); ChatInputLine c-240/385 260 x 10 (0 0 0 200)
    // with the "Say:" prompt. Retail draws a grey 1-pixel outline round the
    // whole box and a divider above the input line.
    commands.spawn((
        Node { border: UiRect::all(line_px(1.)), ..r(-240., 326., 261., 70.) },
        BackgroundColor(Color::BLACK),
        BorderColor::all(CHAT_LINE),
        ChildOf(panel),
    ));
    let history = commands
        .spawn((
            Node {
                overflow: Overflow::clip(),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::FlexEnd,
                padding: UiRect::left(line_px(4.)),
                ..r(-240., 327., 247., 57.)
            },
            ChildOf(panel),
        ))
        .id();
    for line in 0..CHAT_LINES {
        commands.spawn((
            Text::new(""),
            TextFont {
                font: bevy::text::FontSource::Handle(chat_font.clone()),
                font_size: FontSize::Vh(13. / 7.2),
                ..default()
            },
            TextColor(Color::srgb_u8(216, 216, 216)),
            TextLayout::linebreak(bevy::text::LineBreak::NoWrap),
            TextShadow { offset: Vec2::splat(1.), color: Color::BLACK },
            LobbyChatLine(line),
            ChildOf(history),
        ));
    }
    scroll_button(commands, panel, r(8., 327., 12., 12.), true);
    scroll_button(commands, panel, r(8., 372., 12., 12.), false);
    commands.spawn((
        Node { border: UiRect::top(line_px(1.)), ..r(-240., 384., 261., 11.) },
        BackgroundColor(Color::srgba(0., 0., 0., 200. / 255.)),
        BorderColor::all(CHAT_LINE),
        ChildOf(panel),
    ));
    commands.spawn((
        Text::new("Say:"),
        TextFont {
            font: bevy::text::FontSource::Handle(chat_font.clone()),
            font_size: FontSize::Vh(10. / 7.2),
            ..default()
        },
        TextColor(Color::WHITE),
        TextLayout::linebreak(bevy::text::LineBreak::NoWrap),
        r(-239., 386.5, 258., 9.),
        LobbyChatInput,
        // Touch screens: tapping the line brings up the keyboard.
        Button,
        ChildOf(panel),
    ));
    // Voice chat (BtnVoiceButton): no voice yet, shown disabled.
    commands.spawn((ImageNode::new(voice), r(-255., 401., 15., 15.), ChildOf(panel)));
    disabled_label(commands, panel, "START VOICE CHAT", font, r(-240., 401., 180., 15.));
    // Touch screens: the line being typed, above the on-screen keyboard.
    let bar = commands
        .spawn((
            Node {
                display: Display::None,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(line_px(10.)),
                border: UiRect::all(line_px(1.)),
                ..rect(240., 24., 800., 44.)
            },
            BackgroundColor(Color::srgba(0., 0., 0., 0.85)),
            BorderColor::all(CHAT_LINE),
            GlobalZIndex(130),
            LobbyTypingBar,
            ChildOf(panel),
        ))
        .id();
    commands.spawn((
        Text::new("Say: _"),
        TextFont {
            font: bevy::text::FontSource::Handle(chat_font.clone()),
            font_size: FontSize::Vh(20. / 7.2),
            ..default()
        },
        TextColor(Color::WHITE),
        TextLayout::linebreak(bevy::text::LineBreak::NoWrap),
        LobbyTypingText,
        ChildOf(bar),
    ));
}

#[derive(Component)]
pub(crate) struct LobbyStartButton;
#[derive(Component)]
pub(crate) struct ChoiceText(u8, usize);

/// Posts "Name joined." when the lobby is created (L4D360UI_Lobby_Player_Joined).
pub(super) fn open(lobby: &mut Lobby, now: f64) {
    lobby.open = None;
    if !lobby.joined {
        lobby.joined = true;
        lobby.created = true;
        lobby.chat.push(ChatLine {
            prefix: String::new(),
            name: String::new(),
            text: format!("{} joined.", crate::game_chat::local_name()),
            team: Team::Survivor,
            notice: true,
            at: now,
        });
        // Phones have no console: joining a friend goes through this line.
        if crate::game::ui_fit::touch_build() {
            lobby.chat.push(ChatLine {
                prefix: String::new(),
                name: String::new(),
                text: "Join a friend: tap Say:, type connect <IP>".into(),
                team: Team::Survivor,
                notice: true,
                at: now,
            });
        }
    }
}

/// Leaving the lobby empties it (L4D360UI_Lobby_LeaveLobby_Tip: back to the
/// main menu).
pub(super) fn leave(lobby: &mut Lobby) {
    *lobby = Lobby { access: lobby.access, left: true, ..default() };
}

/// Empties the lobby without a "left" signal (the session already ended).
pub(crate) fn leave_quiet(lobby: &mut Lobby) {
    *lobby = Lobby { access: lobby.access, ..default() };
}

/// The game starts from the lobby: the lobby screen closes, but an online
/// session stays up (chat keeps working in game).
pub(super) fn start(lobby: &mut Lobby) {
    let was_leader = lobby.joined && !lobby.client;
    *lobby = Lobby { access: lobby.access, started: was_leader, ..default() };
}

/// Touch screens: tapping the lobby's "Say:" line brings up the keyboard.
pub(crate) fn chat_tap(taps: Query<&Interaction, (Changed<Interaction>, With<LobbyChatInput>)>) {
    if taps.iter().any(|i| *i == Interaction::Pressed) {
        crate::game_chat::lobby_keyboard(true);
    }
}

/// Lobby chat typing while the lobby is shown: Enter posts "Name :  text".
pub(crate) fn chat_keys(
    frontend: Res<Frontend>,
    mut lobby: ResMut<Lobby>,
    mut events: MessageReader<KeyboardInput>,
    mut ime: MessageReader<bevy::window::Ime>,
    time: Res<Time<Real>>,
) {
    if frontend.page != Page::Lobby {
        events.clear();
        ime.clear();
        return;
    }
    // `--capture-lobby`: a lobby with its joined notice and a chat line.
    if !lobby.joined && std::env::args().any(|a| a == "--capture-lobby") {
        let now = time.elapsed_secs_f64();
        open(&mut lobby, now);
        let me = crate::game_chat::local_name();
        lobby.chat.push(crate::game_chat::compose(crate::game_chat::Mode::All, Team::Survivor, false, &me, "ready when you are", now));
    }
    let mut send = false;
    for key in events.read() {
        if crate::game_chat::edit(&mut lobby.input, key) == Some(true) {
            send = true;
        }
    }
    for event in ime.read() {
        if let bevy::window::Ime::Commit { value, .. } = event {
            lobby.input.push_str(value);
        }
    }
    if send {
        let text = std::mem::take(&mut lobby.input).trim().to_string();
        // Phones have no console: "connect <address>" (and "disconnect",
        // "status") typed here run as console commands.
        let verb = text.trim_start_matches('/').split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        if matches!(verb.as_str(), "connect" | "disconnect" | "status" | "connect_lobby") {
            let reply = crate::multiplayer::command(text.trim_start_matches('/')).unwrap_or_default();
            if !reply.is_empty() {
                lobby.chat.push(ChatLine {
                    prefix: String::new(),
                    name: String::new(),
                    text: reply,
                    team: Team::Survivor,
                    notice: true,
                    at: time.elapsed_secs_f64(),
                });
            }
        } else if !text.is_empty() {
            lobby.outgoing.push(text.clone());
            let line = crate::game_chat::compose(
                crate::game_chat::Mode::All,
                Team::Survivor,
                false,
                &crate::game_chat::local_name(),
                &text,
                time.elapsed_secs_f64(),
            );
            lobby.chat.push(line);
        }
        // Phones: Return sends and puts the keyboard away.
        crate::game_chat::lobby_keyboard(false);
    }
}

/// Touch screens: the on-screen keyboard covers the lobby's "Say:" line, so
/// while it is up the line being typed shows in a bar at the top.
#[derive(Component)]
pub(crate) struct LobbyTypingBar;
#[derive(Component)]
pub(crate) struct LobbyTypingText;
pub(crate) fn typing_bar(
    frontend: Res<Frontend>,
    lobby: Res<Lobby>,
    mut bar: Query<&mut Node, With<LobbyTypingBar>>,
    mut text: Query<&mut Text, With<LobbyTypingText>>,
) {
    // `--capture-lobby-typing`: shown as if the keyboard were up.
    let forced = std::env::args().any(|a| a == "--capture-lobby-typing");
    let show = frontend.page == Page::Lobby
        && crate::game::ui_fit::touch_build()
        && (crate::game_chat::lobby_keyboard_up() || forced);
    for mut node in &mut bar {
        let display = if show { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    if show {
        let value = format!("Say: {}_", lobby.input);
        for mut t in &mut text {
            if t.0 != value {
                t.0 = value.clone();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn presentation(
    frontend: Res<Frontend>,
    lobby: Res<Lobby>,
    sel: Res<crate::campaign_settings::Selection>,
    catalog: Res<crate::campaign_settings::Catalog>,
    art: Res<MenuArt>,
    mut texts: Query<(
        &mut Text,
        Option<&LobbyText>,
        Option<&LobbySlot>,
        Option<&LobbyChatLine>,
        Option<&LobbyChatInput>,
        Option<&ChoiceText>,
    )>,
    mut poster: Query<&mut ImageNode, (With<LobbyPoster>, Without<LobbySlotPortrait>)>,
    mut portraits: Query<(&LobbySlotPortrait, &mut ImageNode, &mut Node), (Without<LobbyPoster>, Without<LobbySlotPart>)>,
    mut drops: Query<(&LobbyDrop, &mut Node), (Without<LobbySlotPortrait>, Without<LobbySlotPart>)>,
    mut parts: Query<(&LobbySlotPart, &mut Node), (Without<LobbySlotPortrait>, Without<LobbyDrop>)>,
    start: Query<&Children, With<LobbyStartButton>>,
    mut colors: Query<&mut TextColor, Without<Glow>>,
    mut assets: Local<Option<SourceAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut portrait_cache: Local<std::collections::HashMap<String, Handle<Image>>>,
    source: Option<Res<crate::game_ui::HudSource>>,
) {
    use crate::campaign_settings::Survivor;
    if frontend.page != Page::Lobby {
        return;
    }
    let c = &catalog.campaigns[sel.campaign];
    let ch = &c.chapters[sel.chapter];
    let me = crate::game_chat::local_name();
    let roster = Survivor::roster(c.original);
    // Retail order (LobbyDetailsSummary): access, "Campaign - <difficulty>"
    // (L4D360UI_Difficulty_*_coop), campaign, chapter ("1: Hotel"), server
    // type (L4D360UI_Lobby_ServerType_Listen).
    let survival = frontend.mode == crate::survival::MODE_INDEX;
    let xbox = crate::xbox_link::in_xbox_lobby();
    let summary = [
        if xbox { "System Link Game".to_string() } else { lobby.access.label().to_string() },
        if survival { "Survival".to_string() } else { format!("Campaign - {}", sel.difficulty.name()) },
        c.name.clone(),
        ch.name.clone(),
        if xbox { String::new() } else { "Hosting Local Server".to_string() },
    ];
    let leader = if lobby.leader.is_empty() { me.clone() } else { lobby.leader.clone() };
    let shown: Vec<&ChatLine> = lobby.chat.lines.iter().rev().take(CHAT_LINES).collect();
    for (mut text, field, slot, chat, input, choice) in &mut texts {
        let value = if let Some(field) = field {
            match field.0 {
                TITLE => if survival { "SURVIVAL LOBBY" } else { "CAMPAIGN LOBBY" }.to_string(),
                LEADER => format!("Lobby Leader: {leader}"),
                n => summary.get((n - SUMMARY) as usize).cloned().unwrap_or_default(),
            }
        } else if let Some(slot) = slot {
            // The name with the drop-down's ">" (ShowDropDownIndicator);
            // L4D360UI_Lobby_EmptySlotAd on open slots.
            if !lobby.members.is_empty() {
                match lobby.members.get(slot.0) {
                    Some((name, _, _)) => format!("{name} >"),
                    None => "Player Slot Available".to_string(),
                }
            } else if slot.0 == 0 {
                format!("{me} >")
            } else {
                "Player Slot Available".to_string()
            }
        } else if let Some(chat) = chat {
            // Newest line at the bottom.
            shown.get(CHAT_LINES - 1 - chat.0).map(|l| l.plain()).unwrap_or_default()
        } else if input.is_some() {
            format!("Say: {}_", lobby.input)
        } else if let Some(choice) = choice {
            if choice.0 == 0 {
                roster.get(choice.1).map(|s| s.name().to_string()).unwrap_or_default()
            } else {
                Access::ALL[choice.1].label().to_string()
            }
        } else {
            continue;
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    if let Some(handle) = art.chapters.get(&ch.image) {
        for mut image in &mut poster {
            if image.image != *handle {
                image.image = handle.clone();
            }
        }
    }
    // Each player's survivor portrait (the HUD panel art). Offline the
    // lobby holds just us.
    let survivors: Vec<Option<Survivor>> = if lobby.members.is_empty() {
        vec![Some(sel.survivor)]
    } else {
        lobby.members.iter().map(|(_, s, _)| Survivor::parse(s)).collect()
    };
    for (slot, mut image, mut node) in &mut portraits {
        let survivor = survivors.get(slot.0).copied().flatten();
        let display = if survivor.is_some() { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
        let Some(survivor) = survivor else { continue };
        let key = format!("s_panel_{}", survivor.character());
        if !portrait_cache.contains_key(&key) {
            if let Some(source) = &source {
                if assets.is_none() {
                    *assets = SourceAssets::new(&source.0, vec![]).ok();
                }
                if let Some(assets) = assets.as_mut() {
                    let handle = super::image(assets, &mut images, &format!("materials/vgui/{key}.vtf"));
                    portrait_cache.insert(key.clone(), handle);
                }
            }
        }
        if let Some(handle) = portrait_cache.get(&key) {
            if image.image != *handle {
                image.image = handle.clone();
            }
        }
    }
    let filled = survivors.len();
    for (part, mut node) in &mut parts {
        let display = if part.0 < filled { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    for (drop, mut node) in &mut drops {
        let display = if lobby.open == Some(drop.0) { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    let _ = (&start, &mut colors);
}

/// Steam avatars already turned into images, and when missing ones were
/// last asked for.
#[derive(Default)]
pub(crate) struct AvatarCache {
    images: std::collections::HashMap<u64, Handle<Image>>,
    last_try: Option<std::time::Instant>,
}

/// Shows each player's Steam avatar in their row (retail PnlGamerPic).
/// Steam may still be downloading an avatar; missing ones are retried about
/// once a second. Without Steam or an avatar the grey square stays.
pub(crate) fn avatars(
    frontend: Res<Frontend>,
    lobby: Res<Lobby>,
    mut slots: Query<(&LobbySlotAvatar, &mut ImageNode), (Without<LobbyPoster>, Without<LobbySlotPortrait>)>,
    mut images: ResMut<Assets<Image>>,
    mut cache: Local<AvatarCache>,
) {
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    if frontend.page != Page::Lobby {
        return;
    }
    // Offline the lobby holds just us.
    let ids: Vec<u64> = if lobby.members.is_empty() {
        vec![crate::steam::me().unwrap_or(0)]
    } else {
        lobby.members.iter().map(|m| m.2).collect()
    };
    let missing = ids.iter().any(|&id| id != 0 && !cache.images.contains_key(&id));
    let due = cache.last_try.is_none_or(|t| t.elapsed().as_secs_f32() >= 1.);
    if missing && due {
        cache.last_try = Some(std::time::Instant::now());
        for &id in &ids {
            if id == 0 || cache.images.contains_key(&id) {
                continue;
            }
            let Some((w, h, rgba)) = crate::steam::avatar(id) else { continue };
            if rgba.len() != (w * h * 4) as usize {
                continue;
            }
            let image = Image::new(
                Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                TextureDimension::D2,
                rgba,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            );
            cache.images.insert(id, images.add(image));
        }
    }
    for (slot, mut node) in &mut slots {
        let handle = ids.get(slot.0).and_then(|id| cache.images.get(id));
        match handle {
            Some(handle) => {
                if node.image != *handle {
                    node.image = handle.clone();
                }
                if node.color != Color::WHITE {
                    node.color = Color::WHITE;
                }
            }
            None => {
                if node.color != Color::NONE {
                    node.color = Color::NONE;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_the_lobby_posts_one_joined_notice_and_leaving_resets_it() {
        let mut lobby = Lobby::default();
        open(&mut lobby, 1.);
        open(&mut lobby, 2.);
        assert_eq!(lobby.chat.lines.len(), 1);
        assert!(lobby.chat.lines[0].text.ends_with(" joined."));
        lobby.access = Access::Friends;
        lobby.input = "hi".into();
        leave(&mut lobby);
        assert!(lobby.chat.lines.is_empty() && lobby.input.is_empty() && !lobby.joined && lobby.left);
        assert_eq!(lobby.access, Access::Friends, "the access choice is kept");
    }
}
