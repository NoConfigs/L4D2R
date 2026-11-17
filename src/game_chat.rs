//! In-game text chat, laid out from retail resource/ui/basechat.res
//! (CHudChat): Y opens "say" (everyone), U opens "say_team"; Enter sends,
//! Escape cancels. Lines use the retail formats from left4dead2_english.txt
//! (L4D_Chat_*), e.g. "(Survivor) Name :  message", and fade after
//! hud_saytext_time (12 s) unless the chat is open.
//!
//! There is no online multiplayer yet, so messages are local: `ChatLog`
//! keeps them, and a future network layer can append remote lines to it and
//! read `ChatLog::outgoing` to send ours.
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use std::collections::VecDeque;

/// hud_saytext_time: seconds a line stays up while the chat is closed.
const LINE_TIME: f64 = 12.;
/// Lines kept in memory (the history shows the newest that fit).
const KEEP: usize = 100;
/// Retail chat input limit (CBaseHudChatInputLine, 127 characters).
const MAX_INPUT: usize = 127;
/// Lines shown in the 75-unit history box (ChatFont, 12 units tall).
const SHOWN: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Team {
    Survivor,
    Infected,
    Spectator,
}
impl Team {
    /// Retail team colours for player names in chat.
    fn color(self) -> Color {
        match self {
            Team::Survivor => Color::srgb_u8(153, 204, 255),
            Team::Infected => Color::srgb_u8(255, 63, 63),
            Team::Spectator => Color::srgb_u8(204, 204, 204),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatLine {
    /// "(Survivor) ", "*DEAD* " ... before the name; empty for "say".
    pub prefix: String,
    pub name: String,
    pub text: String,
    pub team: Team,
    /// A notice ("Name joined.") drawn in one colour with no name.
    pub notice: bool,
    /// Real time the line arrived (seconds since startup).
    pub at: f64,
}
impl ChatLine {
    /// The retail line as plain text (L4D_Chat_* formats).
    pub fn plain(&self) -> String {
        if self.notice {
            self.text.clone()
        } else {
            format!("{}{} :  {}", self.prefix, self.name, self.text)
        }
    }
}

/// Messages shown in a chat box, newest last. Shared with the lobby.
#[derive(Default, Clone, Debug)]
pub struct ChatHistory {
    pub lines: VecDeque<ChatLine>,
    /// Bumped on every change so the UI rebuilds only when needed.
    pub serial: u64,
}
impl ChatHistory {
    pub fn push(&mut self, line: ChatLine) {
        self.lines.push_back(line);
        while self.lines.len() > KEEP {
            self.lines.pop_front();
        }
        self.serial += 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// "say": everyone.
    All,
    /// "say_team": your team only.
    Team,
}

/// The in-game chat: history, the open input line, and lines we sent (for a
/// network layer to deliver).
#[derive(Resource, Default)]
pub struct ChatLog {
    pub history: ChatHistory,
    pub input: Option<(Mode, String)>,
    pub outgoing: Vec<(Mode, String)>,
}

/// The local player's name: L4D2R_NAME (testing), else the Steam name
/// (steam.rs, desktop with Steam running), else the OS user name, else
/// "Player".
pub fn local_name() -> String {
    std::env::var("L4D2R_NAME")
        .ok()
        .or_else(crate::steam::name)
        .ok_or(())
        .or_else(|_| std::env::var("USERNAME"))
        .or_else(|_| std::env::var("USER"))
        .ok()
        .map(|n| n.trim().chars().take(32).collect::<String>())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Player".into())
}

/// Builds the line for a message, following L4D_Chat_* (left4dead2_english).
pub fn compose(mode: Mode, team: Team, dead: bool, name: &str, text: &str, at: f64) -> ChatLine {
    let prefix = match (mode, team, dead) {
        (Mode::All, Team::Spectator, _) => "*SPEC* ".to_string(),
        (Mode::All, _, true) => "*DEAD* ".to_string(),
        (Mode::All, _, false) => String::new(),
        (Mode::Team, Team::Spectator, _) => "(Spectator) ".to_string(),
        (Mode::Team, Team::Survivor, dead) => format!("{}(Survivor) ", if dead { "*DEAD*" } else { "" }),
        (Mode::Team, Team::Infected, dead) => format!("{}(Infected) ", if dead { "*DEAD*" } else { "" }),
    };
    ChatLine { prefix, name: name.into(), text: text.into(), team, notice: false, at }
}

/// The prompt shown before the input (L4D_Chat_Prompt_*).
pub fn prompt(mode: Mode, team: Team) -> &'static str {
    match (mode, team) {
        (Mode::All, _) => "Say :",
        (Mode::Team, Team::Survivor) => "Say (Survivor):",
        (Mode::Team, Team::Infected) => "Say (Infected):",
        (Mode::Team, Team::Spectator) => "Say (TEAM):",
    }
}

/// Applies typed keys to an input string. Returns Some(true) on Enter,
/// Some(false) on Escape, None otherwise.
pub fn edit(text: &mut String, key: &KeyboardInput) -> Option<bool> {
    if key.state != ButtonState::Pressed {
        return None;
    }
    // The iPhone keyboard's Return arrives as a "\n" character.
    if key.text.as_ref().is_some_and(|t| t.contains('\n') || t.contains('\r')) {
        return Some(true);
    }
    match &key.logical_key {
        Key::Enter => return Some(true),
        Key::Escape => return Some(false),
        Key::Backspace => {
            text.pop();
            return None;
        }
        _ => {}
    }
    if let Some(typed) = &key.text {
        for c in typed.chars().filter(|c| !c.is_control()) {
            if text.chars().count() < MAX_INPUT {
                text.push(c);
            }
        }
    }
    None
}

fn player_team(state: &crate::game_state::Match) -> Team {
    if state.infected {
        Team::Infected
    } else {
        Team::Survivor
    }
}

/// PreUpdate, after input and touch injection: open/close the chat, take
/// typed text, and while it is open hide every key and mouse button from
/// the game so typing never moves, shoots or opens menus.
#[allow(clippy::too_many_arguments)]
fn keys(
    mut log: ResMut<ChatLog>,
    mut events: MessageReader<KeyboardInput>,
    mut ime: MessageReader<bevy::window::Ime>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    frontend: Option<Res<crate::game_menu::Frontend>>,
    console: Option<Res<crate::game_console::Console>>,
    state: Option<Res<crate::game_state::Match>>,
    time: Res<Time<Real>>,
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
) {
    // The test laboratory keeps U for its uncommon-infected gallery.
    let lab = session.as_ref().is_some_and(|s| crate::test_lab::active(&s.map));
    let playing = frontend.as_ref().is_some_and(|f| f.playing())
        && !console.as_ref().is_some_and(|c| c.open);
    if !playing {
        events.clear();
        ime.clear();
        log.input = None;
        return;
    }
    // `--capture-chat`: a sent team message and the input open mid-typing.
    if log.history.lines.is_empty() && std::env::args().any(|a| a == "--capture-chat") {
        let now = time.elapsed_secs_f64();
        let me = local_name();
        log.history.push(compose(Mode::Team, Team::Survivor, false, &me, "Witch ahead, lights off", now));
        log.history.push(compose(Mode::All, Team::Survivor, false, &me, "nice", now));
        log.input = Some((Mode::Team, "on my way".into()));
    }
    if log.input.is_none() {
        let mode = if keys.just_pressed(KeyCode::KeyY) {
            Some(Mode::All)
        } else if keys.just_pressed(KeyCode::KeyU) && !lab {
            Some(Mode::Team)
        } else {
            None
        };
        // The key that opened the chat must not type into it.
        events.clear();
        ime.clear();
        if let Some(mode) = mode {
            log.input = Some((mode, String::new()));
            keys.reset_all();
            mouse.reset_all();
        }
        return;
    }
    let mut done = None;
    for key in events.read() {
        if let Some((_, text)) = log.input.as_mut() {
            if let Some(send) = edit(text, key) {
                done = Some(send);
                break;
            }
        }
    }
    for event in ime.read() {
        if let (bevy::window::Ime::Commit { value, .. }, Some((_, text))) = (event, log.input.as_mut()) {
            for c in value.chars().filter(|c| !c.is_control()) {
                if text.chars().count() < MAX_INPUT {
                    text.push(c);
                }
            }
        }
    }
    events.clear();
    keys.reset_all();
    mouse.reset_all();
    if let Some(send) = done {
        let (mode, text) = log.input.take().unwrap();
        let text = text.trim().to_string();
        if send && !text.is_empty() {
            let (team, dead) = state.as_ref().map_or((Team::Survivor, false), |s| (player_team(s), s.dead));
            let line = compose(mode, team, dead, &local_name(), &text, time.elapsed_secs_f64());
            println!("Chat: {}", line.plain());
            log.history.push(line);
            log.outgoing.push((mode, text));
        }
    }
}

#[derive(Component)]
struct ChatRoot;
/// Touch screens: the CHAT button (no Y/U keys there).
#[derive(Component)]
struct ChatButton;

/// The lobby's "Say:" line was tapped: keep the on-screen keyboard up
/// while the lobby is shown (game_lobby.rs).
static LOBBY_KEYBOARD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn lobby_keyboard(on: bool) {
    LOBBY_KEYBOARD.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn lobby_keyboard_up() -> bool {
    LOBBY_KEYBOARD.load(std::sync::atomic::Ordering::Relaxed)
}
/// The chat font, shared by each line's name and message spans (a span
/// does not inherit its parent's font).
#[derive(Resource, Clone)]
struct ChatFont(Handle<Font>);
#[derive(Component)]
struct ChatBackground;
#[derive(Component)]
struct ChatLineText(usize);
#[derive(Component)]
struct ChatInputText;

/// ChatFont "dropshadow": a dark copy one pixel down-right. Bevy's default
/// (4 px, translucent) read as a second, offset line on phones.
fn chat_shadow() -> TextShadow {
    TextShadow { offset: Vec2::splat(1.), color: Color::BLACK.with_alpha(0.9) }
}

/// Chat font: retail ChatFont is Tahoma bold 12 (of 480 lines).
pub(crate) fn chat_font(fonts: &mut Assets<Font>) -> Handle<Font> {
    for path in ["C:/Windows/Fonts/tahomabd.ttf", "C:/Windows/Fonts/arialbd.ttf"] {
        if let Ok(bytes) = std::fs::read(path) {
            return crate::game_ui::shared_font(&mut *fonts, bytes);
        }
    }
    Handle::default()
}

/// BaseChat.res on the 480-line canvas: HudChat at x 10, y r205 (275),
/// 280 x 120; history at +10/+17, 260 x 75; the input line below it.
fn chat_rect(x: f32, y: f32, w: f32, h: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: vh(x / 4.8),
        top: vh(y / 4.8),
        width: vh(w / 4.8),
        height: vh(h / 4.8),
        ..default()
    }
}

/// Builds the chat box on the HUD camera (again after a map change, which
/// deletes all UI).
fn spawn(
    mut commands: Commands,
    roots: Query<(), With<ChatRoot>>,
    camera: Option<Single<Entity, With<crate::game::ViewCamera>>>,
    mut fonts: ResMut<Assets<Font>>,
    mut font: Local<Option<Handle<Font>>>,
) {
    let Some(camera) = camera else { return };
    if !roots.is_empty() {
        return;
    }
    let font = font.get_or_insert_with(|| chat_font(&mut fonts)).clone();
    commands.insert_resource(ChatFont(font.clone()));
    let size = FontSize::Vh(12. / 4.8);
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: percent(100.),
                height: percent(100.),
                ..default()
            },
            Pickable::IGNORE,
            UiTargetCamera(*camera),
            GlobalZIndex(40),
            ChatRoot,
        ))
        .id();
    commands.spawn((
        chat_rect(10., 275., 280., 120.),
        BackgroundColor(Color::srgba(0., 0., 0., 0.55)),
        Visibility::Hidden,
        ChatBackground,
        Pickable::IGNORE,
        ChildOf(root),
    ));
    for i in 0..SHOWN {
        commands.spawn((
            Text::new(""),
            TextFont { font: bevy::text::FontSource::Handle(font.clone()), font_size: size, ..default() },
            TextColor(Color::srgb_u8(216, 216, 216)),
            chat_shadow(),
            TextLayout::linebreak(bevy::text::LineBreak::NoWrap),
            Node { overflow: Overflow::clip(), ..chat_rect(20., 292. + i as f32 * 12.5, 260., 13.) },
            ChatLineText(i),
            Pickable::IGNORE,
            ChildOf(root),
        ));
    }
    commands.spawn((
        Text::new(""),
        TextFont { font: bevy::text::FontSource::Handle(font.clone()), font_size: size, ..default() },
        TextColor(Color::WHITE),
        chat_shadow(),
        TextLayout::linebreak(bevy::text::LineBreak::NoWrap),
        Node { overflow: Overflow::clip(), ..chat_rect(20., 371., 260., 14.) },
        ChatInputText,
        Pickable::IGNORE,
        ChildOf(root),
    ));
    if crate::game::ui_fit::touch_build() {
        let button = commands
            .spawn((
                Button,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(12.),
                    top: px(10.),
                    width: vh(64. / 4.8),
                    height: vh(26. / 4.8),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border: UiRect::all(px(1.)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0., 0., 0., 0.45)),
                BorderColor::all(Color::srgba(1., 1., 1., 0.35)),
                ChatButton,
                // Its own UI root: the chat box's root moves up while typing.
                UiTargetCamera(*camera),
                GlobalZIndex(41),
            ))
            .id();
        commands.spawn((
            Text::new("CHAT"),
            TextFont { font: bevy::text::FontSource::Handle(font), font_size: FontSize::Vh(13. / 4.8), ..default() },
            TextColor(Color::srgb_u8(216, 216, 216)),
            Pickable::IGNORE,
            ChildOf(button),
        ));
    }
}

/// Touch screens: the CHAT button opens (or closes) the chat, the on-screen
/// keyboard follows the chat (and the lobby's "Say:" line), and while it is
/// up the chat box moves to the top of the screen, which the keyboard
/// leaves clear. The button keeps clear of the notch (safe area).
#[allow(clippy::type_complexity)]
fn touch_chat(
    mut log: ResMut<ChatLog>,
    frontend: Option<Res<crate::game_menu::Frontend>>,
    safe: Option<Res<crate::game::ui_fit::SafeArea>>,
    pressed: Query<&Interaction, (Changed<Interaction>, With<ChatButton>)>,
    mut button: Query<(&mut Node, &mut Visibility), (With<ChatButton>, Without<ChatRoot>)>,
    mut roots: Query<&mut Node, (With<ChatRoot>, Without<ChatButton>)>,
    mut windows: Query<&mut Window>,
    mut lifted: Local<bool>,
) {
    if !crate::game::ui_fit::touch_build() {
        return;
    }
    let playing = frontend.as_ref().is_some_and(|f| f.playing());
    let lobby = frontend.as_ref().is_some_and(|f| f.page == crate::game_menu::Page::Lobby);
    if !lobby {
        lobby_keyboard(false);
    }
    if playing && pressed.iter().any(|i| *i == Interaction::Pressed) {
        log.input = match log.input {
            Some(_) => None,
            None => Some((Mode::All, String::new())),
        };
    }
    let (left, top) = safe.as_ref().map_or((0., 0.), |s| (s.left, s.top));
    for (mut node, mut visibility) in &mut button {
        let l = px(left + 12.);
        let t = px(top + 10.);
        if node.left != l || node.top != t {
            node.left = l;
            node.top = t;
        }
        visibility.set_if_neq(if playing { Visibility::Inherited } else { Visibility::Hidden });
    }
    let open = playing && log.input.is_some();
    let keyboard = open || (lobby && LOBBY_KEYBOARD.load(std::sync::atomic::Ordering::Relaxed));
    for mut window in &mut windows {
        if window.ime_enabled != keyboard {
            window.ime_enabled = keyboard;
        }
    }
    // The box (y 275..395 of 480) moves up to y 40 while typing.
    if *lifted != open {
        *lifted = open;
        for mut node in &mut roots {
            node.top = if open { vh(-235. / 4.8) } else { px(0.) };
        }
    }
}

/// Fills the chat box: the newest lines (all of them while typing, else
/// only lines younger than hud_saytext_time) and the input line.
fn draw(
    log: Res<ChatLog>,
    time: Res<Time<Real>>,
    state: Option<Res<crate::game_state::Match>>,
    frontend: Option<Res<crate::game_menu::Frontend>>,
    mut commands: Commands,
    mut background: Query<&mut Visibility, With<ChatBackground>>,
    mut lines: Query<(Entity, &ChatLineText, &mut Text, &mut TextColor, Option<&Children>), Without<ChatInputText>>,
    mut input: Query<&mut Text, With<ChatInputText>>,
    mut shown: Local<(u64, bool, usize)>,
    font: Option<Res<ChatFont>>,
) {
    let Some(font) = font else { return };
    let span_font = TextFont {
        font: bevy::text::FontSource::Handle(font.0.clone()),
        font_size: FontSize::Vh(12. / 4.8),
        ..default()
    };
    let playing = frontend.as_ref().is_some_and(|f| f.playing());
    let open = playing && log.input.is_some();
    let now = time.elapsed_secs_f64();
    let visible: Vec<&ChatLine> = log
        .history
        .lines
        .iter()
        .rev()
        .filter(|l| playing && (open || now - l.at < LINE_TIME))
        .take(SHOWN)
        .collect();
    let key = (log.history.serial, open, visible.len());
    for mut v in &mut background {
        v.set_if_neq(if open { Visibility::Inherited } else { Visibility::Hidden });
    }
    let team = state.as_ref().map_or(Team::Survivor, |s| player_team(s));
    for mut text in &mut input {
        let value = match &log.input {
            Some((mode, typed)) if open => format!("{} {}_", prompt(*mode, team), typed),
            _ => String::new(),
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    if *shown == key {
        return;
    }
    *shown = key;
    for (entity, slot, mut text, mut color, children) in &mut lines {
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }
        // Newest line at the bottom slot.
        let index = SHOWN - 1 - slot.0;
        let Some(line) = visible.get(index) else {
            text.0.clear();
            continue;
        };
        if line.notice {
            text.0 = line.text.clone();
            color.0 = Color::srgb_u8(255, 176, 0);
            continue;
        }
        text.0 = line.prefix.clone();
        color.0 = Color::srgb_u8(216, 216, 216);
        commands.entity(entity).with_children(|parent| {
            parent.spawn((TextSpan::new(line.name.clone()), span_font.clone(), TextColor(line.team.color())));
            parent.spawn((
                TextSpan::new(format!(" :  {}", line.text)),
                span_font.clone(),
                TextColor(Color::srgb_u8(216, 216, 216)),
            ));
        });
    }
}

pub fn install(app: &mut App) {
    app.init_resource::<ChatLog>()
        .add_systems(PreUpdate, keys.after(crate::game::touch::inject))
        .add_systems(Update, (spawn, touch_chat, draw).chain());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lines_use_the_retail_chat_formats() {
        let say = compose(Mode::All, Team::Survivor, false, "Nick", "hi", 0.);
        assert_eq!(say.plain(), "Nick :  hi");
        let team = compose(Mode::Team, Team::Survivor, false, "Nick", "hi", 0.);
        assert_eq!(team.plain(), "(Survivor) Nick :  hi");
        let dead = compose(Mode::Team, Team::Infected, true, "Tank", "go", 0.);
        assert_eq!(dead.plain(), "*DEAD*(Infected) Tank :  go");
        assert_eq!(compose(Mode::All, Team::Survivor, true, "N", "x", 0.).plain(), "*DEAD* N :  x");
        assert_eq!(prompt(Mode::Team, Team::Survivor), "Say (Survivor):");
    }
    #[test]
    fn history_keeps_the_newest_lines() {
        let mut h = ChatHistory::default();
        for i in 0..150 {
            h.push(compose(Mode::All, Team::Survivor, false, "P", &i.to_string(), 0.));
        }
        assert_eq!(h.lines.len(), KEEP);
        assert_eq!(h.lines.back().unwrap().text, "149");
        assert_eq!(h.serial, 150);
    }
}
