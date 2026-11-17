//! Front end built from the installed PC main-menu resources and native artwork.
use crate::{
    game_audio,
    game_state::Match,
    game_ui::{HudRoot, HudSource},
    source_assets::SourceAssets,
    vtf,
};
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
#[derive(Resource)]
pub struct Frontend {
    pub page: Page,
    pub mode: usize,
    pub restart: bool,
    pub started: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Campaign,
    Options,
    Extras,
    Game,
    Pause,
    Audio,
    Video,
    Vote,
    Difficulty,
    Advanced,
    Loading,
    /// ADD-ONS & MODS: Steam add-ons and local mods tabs.
    Addons,
    /// TOUCH & HUD: touch-control editor, presets and HUD size
    /// (game_touch.rs draws it over the menu background or the paused game).
    Touch,
    /// FRIENDS: the rewrite's friends list (src/friends).
    Friends,
    /// Pre-game lobby (retail gamelobby.res; game_lobby.rs).
    Lobby,
    /// JOIN A GAME: LAN and Steam games (retail foundgames.res;
    /// game_browser.rs).
    Browser,
}
impl Frontend {
    #[cfg(test)]
    pub fn playing_for_test() -> Self {
        Self { page: Page::Game, mode: 0, restart: false, started: true }
    }
    pub fn playing(&self) -> bool {
        self.page == Page::Game
    }
}
#[derive(Component)]
pub(crate) struct MenuRoot;
#[derive(Component)]
pub(crate) struct PageNode(Page);
impl PageNode {
    pub(crate) fn page(&self) -> Page {
        self.0
    }
}
#[derive(Component)]
pub(crate) struct Action(ButtonAction);
#[derive(Clone, Copy)]
pub(crate) enum ButtonAction {
    GameFiles,
    Mode(usize),
    Next,
    Previous,
    Campaign,
    Start,
    Resume,
    Options,
    Extras,
    Friends,
    Home,
    Quit,
    Back,
    Volume(f32),
    ShowFps,
    /// VIDEO > ADVANCED: retail's Low Violence (-lv) mode on or off.
    LowViolence,
    Vsync,
    Audio,
    Video,
    Vote,
    Difficulty,
    SetDifficulty(usize),
    Restart,
    NewCampaign,
    Advanced,
    Recommended,
    DisplayMode,
    Resolution,
    Graphics(crate::graphics_options::OptionKey),
    Select(usize),
    Choose(usize, usize),
    Addons,
    /// ADD-ONS & MODS tab: 0 Steam add-ons, 1 mods.
    AddonsTab(u8),
    /// Turns a listed mod on or off (index into MODS).
    ToggleMod(usize),
    /// Opens the TOUCH & HUD page.
    TouchHud,
    /// PLAY CAMPAIGN settings -> lobby (retail CREATE LOBBY).
    CreateLobby,
    /// Lobby EDIT GAME SETTINGS: back to the settings page.
    EditLobbySettings,
    /// Lobby drop-down: 0 CHOOSE CHARACTER, 1 PERMISSIONS.
    LobbyDrop(u8),
    /// Lobby PERMISSIONS entry (Public, Friends-Only, Private).
    LobbyAccess(u8),
    /// Lobby LEAVE LOBBY: back to the main menu.
    LeaveLobby,
    /// Lobby INVITE FRIENDS (opens FRIENDS; no Steam overlay).
    LobbyInvite,
    /// Lobby START THE GAME (same as START GAME).
    LobbyStart,
    /// Main menu JOIN A GAME.
    Browser,
    /// A game row in JOIN A GAME (a second press joins it).
    BrowserPick(usize),
    /// JOIN A GAME: JOIN GAME on the selected row.
    BrowserJoin,
}
#[path = "game_lobby.rs"]
pub(crate) mod lobby;
#[path = "game_browser.rs"]
pub(crate) mod browser;
/// Selected ADD-ONS & MODS tab (0 Steam add-ons, 1 mods).
static ADDONS_TAB: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);
/// Content of one ADD-ONS & MODS tab.
#[derive(Component)]
pub(crate) struct AddonsPanel(u8);
/// Underline under the selected ADD-ONS & MODS tab.
#[derive(Component)]
pub(crate) struct AddonsTabMark(u8);
/// Built-in mods: (name, description, controls).
const MODS: &[(&str, &str, &str)] = &[(
    "Skate 4 Dead",
    "Ride a skateboard in the campaign: ollies, flip tricks and a trick score.",
    "B board   W kick   S brake   hold/release SPACE ollie   Q/Z/X flips   C manual   A/D grind balance",
)];
/// "ENABLED"/"DISABLED" line of a listed mod.
#[derive(Component)]
pub(crate) struct ModStatus(usize);
#[derive(Component)]
pub(crate) struct VolumeLabel;
#[derive(Component)]
pub(crate) struct SelectedMode;
#[derive(Component)]
pub(crate) struct CarouselTile(usize);
#[derive(Component)]
pub(crate) struct ModeTitle;
#[derive(Component)]
pub(crate) struct ModeDescription;
#[derive(Component)]
pub(crate) struct OptionsFlyout;
#[derive(Component)]
pub(crate) enum Detail {
    Difficulty,
    Aspect,
    Resolution,
    Display,
    Graphics(crate::graphics_options::OptionKey),
    Vsync,
}
/// The map picker's title: PLAY CAMPAIGN, or PLAY SURVIVAL (survival.rs).
#[derive(Component)]
pub(crate) struct CampaignTitle;
#[derive(Resource)]
pub(crate) struct MenuArt {
    modes: Vec<Handle<Image>>,
    chapters: std::collections::HashMap<String, Handle<Image>>,
}
const MODES: [(&str, &str); 8] = [
    ("CAMPAIGN", "campaign"),
    ("MUTATION", "mutation"),
    ("REALISM", "realism"),
    ("VERSUS", "versus"),
    ("REALISM VS", "realismversus"),
    ("SURVIVAL", "survival"),
    ("SURVIVAL VS", "versussurvival"),
    ("SCAVENGE", "scavenge"),
];
pub(crate) fn image(
    assets: &mut SourceAssets,
    images: &mut Assets<Image>,
    path: &str,
) -> Handle<Image> {
    match assets.read(path).and_then(|b| vtf::decode(&b, 2048)) {
        Ok(t) => images.add(Image::new(
            Extent3d {
                width: t.width,
                height: t.height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            t.rgba,
            TextureFormat::Rgba8UnormSrgb,
            // GPU-only: nothing reads menu artwork texels after upload, and
            // on phones (shared CPU/GPU memory) a CPU copy counts twice.
            RenderAssetUsages::RENDER_WORLD,
        )),
        Err(e) => {
            eprintln!("Menu artwork {path}: {e}");
            Handle::default()
        }
    }
}
/// A page-wide background band. Phone pages are scaled and moved inside
/// the safe area (ui_fit), which left the band short of the screen edges
/// (a bare strip beside the notch); there it runs well past both sides.
fn band(y: f32, h: f32) -> Node {
    if crate::game::ui_fit::touch_build() {
        rect(-1280., y, 3840., h)
    } else {
        rect(0., y, 1280., h)
    }
}
fn rect(x: f32, y: f32, w: f32, h: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: vh(x / 7.2),
        top: vh(y / 7.2),
        width: vh(w / 7.2),
        height: vh(h / 7.2),
        ..default()
    }
}
fn button(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    font: &Handle<Font>,
    node: Node,
    action: ButtonAction,
) -> Entity {
    let root = commands
        .spawn((
            Button,
            node,
            BackgroundColor(Color::NONE),
            Action(action),
            ChildOf(parent),
        ))
        .id();
    let text = commands
        .spawn((
            Text::new(label),
            TextFont {
                font: bevy::text::FontSource::Handle(font.clone()),
                font_size: FontSize::Vh(label_size(action)                ),
                ..default()
            },
            TextColor(Color::WHITE),
            ChildOf(root),
        ))
        .id();
    match action {
        ButtonAction::ShowFps => {
            commands
                .entity(text)
                .insert(crate::performance::DisplayLabel(false));
        }
        ButtonAction::LowViolence => {
            commands.entity(text).insert(crate::low_violence::Label);
        }
        ButtonAction::Vsync => {}
        _ => {}
    }
    root
}
/// Font size (vh) `button` gives a label for `action`.
fn label_size(action: ButtonAction) -> f32 {
    if matches!(
        action,
        ButtonAction::Back
            | ButtonAction::CreateLobby
            | ButtonAction::EditLobbySettings
            | ButtonAction::LobbyDrop(_)
            | ButtonAction::LobbyAccess(_)
            | ButtonAction::LeaveLobby
            | ButtonAction::LobbyInvite
            | ButtonAction::LobbyStart
            | ButtonAction::BrowserJoin
            | ButtonAction::SetDifficulty(_)
            | ButtonAction::DisplayMode
            | ButtonAction::Resolution
            | ButtonAction::Advanced
    ) || (matches!(action, ButtonAction::Recommended) && !crate::game::ui_fit::touch_build())
    {
        19. / 7.2
    } else if matches!(
        action,
        ButtonAction::Video
            | ButtonAction::Audio
            | ButtonAction::TouchHud
            | ButtonAction::ToggleMod(_)
            | ButtonAction::Difficulty
            | ButtonAction::NewCampaign
            | ButtonAction::Restart
    ) {
        17. / 7.2
    } else {
        24. / 7.2
    }
}
/// Retail L4D360HybridButton look (gameui CL4D360HybridButton::Paint
/// 1005b700) for the main/in-game menu (MainMenuButton, style 1) and flyout
/// (FlyoutMenuButton, style 2) buttons:
/// - normal: text 125 grey;
/// - focused (hover): white text with the blurred font drawn over it in
///   white at alpha sin(4t) x 30 + 60;
/// - open (its flyout is showing): text 200 grey, blur alpha 30, over a
///   black fade (alpha 0 -> 255 across the button), the
///   HybridButton.BlotchColor (120 20 20) fade (alpha 0 -> 150 over the
///   first quarter of text width + 40 units, then -> 0) and 2-pixel
///   HybridButton.BorderColor (240 0 0) lines at the top and bottom fading
///   in (alpha 0 -> 255).
#[derive(Component, Clone, Copy, PartialEq)]
pub(crate) enum Hybrid {
    Main,
    Flyout,
}
/// One copy of a label in the ring that stands in for the retail blur font.
#[derive(Component)]
pub(crate) struct Glow;
/// The open-state blotch, sized from the label width.
#[derive(Component)]
pub(crate) struct Blotch;
/// The open-state black fade and border lines.
#[derive(Component)]
pub(crate) struct OpenPart;
/// Offsets (720p pixels) of the glow copies: rings at 1.5 and 3 pixels
/// approximate the scheme's "blur 3" fonts.
const GLOW_RING: [(f32, f32); 16] = [
    (1.5, 0.),
    (-1.5, 0.),
    (0., 1.5),
    (0., -1.5),
    (1.06, 1.06),
    (-1.06, 1.06),
    (1.06, -1.06),
    (-1.06, -1.06),
    (3., 0.),
    (-3., 0.),
    (0., 3.),
    (0., -3.),
    (2.12, 2.12),
    (-2.12, 2.12),
    (2.12, -2.12),
    (-2.12, -2.12),
];
/// Each glow copy carries this share of the blur font's alpha.
const GLOW_SHARE: f32 = 0.09;
const DISABLED_SHARE: f32 = 0.16;
const HOME_SHIFT: f32 = 13.5;
/// Flyout entries sit 21 pixels apart at 720p (retail OptionsFlyout), their
/// labels 3 pixels below the row top.
const FLYOUT_ROW: f32 = 21.;
const FLYOUT_TEXT: f32 = 3.;
const BORDER_RED: Color = Color::srgb(240. / 255., 0., 0.);
const BLOTCH_RED: Color = Color::srgb(120. / 255., 20. / 255., 20. / 255.);
fn fade(color: Color, stops: &[(f32, f32)]) -> BackgroundGradient {
    BackgroundGradient::from(LinearGradient {
        angle: LinearGradient::TO_RIGHT,
        stops: stops
            .iter()
            .map(|&(alpha, at)| ColorStop::percent(color.with_alpha(alpha), at))
            .collect(),
        // vgui interpolates vertex colours in gamma space.
        color_space: InterpolationColorSpace::Srgba,
    })
}
fn spawn_glow(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    font: &Handle<Font>,
    size: f32,
    color: Color,
) {
    for (dx, dy) in GLOW_RING {
        commands.spawn((
            Text::new(label),
            TextFont {
                font: bevy::text::FontSource::Handle(font.clone()),
                font_size: FontSize::Vh(size),
                ..default()
            },
            TextColor(color),
            Node {
                position_type: PositionType::Absolute,
                left: vh(dx / 7.2),
                top: vh(dy / 7.2),
                ..default()
            },
            Glow,
            Pickable::IGNORE,
            ChildOf(parent),
        ));
    }
}
/// A menu or flyout button with the retail hybrid button look.
fn hybrid_button(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    font: &Handle<Font>,
    node: Node,
    action: ButtonAction,
    style: Hybrid,
) -> Entity {
    let root = button(commands, parent, label, font, node, action);
    commands.entity(root).insert(style);
    if style == Hybrid::Main {
        let full = || Node {
            position_type: PositionType::Absolute,
            left: px(0.),
            top: px(0.),
            width: percent(100.),
            height: percent(100.),
            display: Display::None,
            ..default()
        };
        commands.spawn((
            full(),
            fade(Color::BLACK, &[(0., 0.), (1., 100.)]),
            ZIndex(-3),
            OpenPart,
            Pickable::IGNORE,
            ChildOf(root),
        ));
        commands.spawn((
            Node {
                width: px(0.),
                ..full()
            },
            fade(BLOTCH_RED, &[(0., 0.), (150. / 255., 25.), (0., 100.)]),
            ZIndex(-2),
            Blotch,
            Pickable::IGNORE,
            ChildOf(root),
        ));
        for top in [true, false] {
            commands.spawn((
                Node {
                    top: if top { px(0.) } else { auto() },
                    bottom: if top { auto() } else { px(0.) },
                    height: vh(2. / 7.2),
                    ..full()
                },
                fade(BORDER_RED, &[(0., 0.), (1., 100.)]),
                ZIndex(-1),
                OpenPart,
                Pickable::IGNORE,
                ChildOf(root),
            ));
        }
    }
    spawn_glow(commands, root, label, font, label_size(action), Color::NONE);
    root
}
/// A disabled flyout entry: the label at alpha 64 with the blur font drawn
/// over it in the disabled colour (88 97 104).
fn disabled_label(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    font: &Handle<Font>,
    node: Node,
) {
    let colour = Color::srgb_u8(88, 97, 104);
    let root = commands.spawn((node, ChildOf(parent))).id();
    commands.spawn((
        Text::new(label),
        TextFont {
            font: bevy::text::FontSource::Handle(font.clone()),
            font_size: FontSize::Vh(17. / 7.2),
            ..default()
        },
        TextColor(colour.with_alpha(64. / 255.)),
        ChildOf(root),
    ));
    // Drawn at full alpha, unlike the focus glow.
    spawn_glow(commands, root, label, font, 17. / 7.2, colour.with_alpha(DISABLED_SHARE));
}
/// How far the home OPTIONS button sits from where the OPTIONS flyout is
/// drawn for (150, 399, 270 wide): one row lower since JOIN A GAME; on
/// phones in the left column's third, taller row.
fn home_options_offset() -> (f32, f32) {
    if crate::game::ui_fit::touch_build() {
        (150. + 300. - 420., 369. + 42. * 2. - 399.)
    } else {
        (0., 30.)
    }
}
/// Retail flyout background (gameui 1002e040): black, solid over the left
/// half, fading 255 -> 220 -> 0 over the right half; 2-pixel border-colour
/// lines along the top and bottom fading out to the right, and a left edge
/// running from the owning button's bottom down.
fn flyout_panel(commands: &mut Commands, parent: Entity, node: Node, owner_tall: f32) -> Entity {
    let panel = commands.spawn((node, ChildOf(parent))).id();
    let full = Node {
        position_type: PositionType::Absolute,
        left: px(0.),
        top: px(0.),
        width: percent(100.),
        height: percent(100.),
        ..default()
    };
    commands.spawn((
        full.clone(),
        fade(Color::BLACK, &[(1., 0.), (1., 50.), (220. / 255., 75.), (0., 100.)]),
        ZIndex(-2),
        Pickable::IGNORE,
        ChildOf(panel),
    ));
    for top in [true, false] {
        commands.spawn((
            Node {
                top: if top { px(0.) } else { auto() },
                bottom: if top { auto() } else { px(0.) },
                height: vh(2. / 7.2),
                ..full.clone()
            },
            fade(BORDER_RED, &[(1., 0.), (0., 100.)]),
            ZIndex(-1),
            Pickable::IGNORE,
            ChildOf(panel),
        ));
    }
    commands.spawn((
        Node {
            top: vh((owner_tall - 2.) / 7.2),
            bottom: px(0.),
            height: auto(),
            width: vh(2. / 7.2),
            ..full
        },
        BackgroundColor(BORDER_RED),
        ZIndex(-1),
        Pickable::IGNORE,
        ChildOf(panel),
    ));
    panel
}
/// Paints the hybrid buttons each frame (see `Hybrid`).
pub fn hybrid_paint(
    frontend: Res<Frontend>,
    time: Res<Time>,
    windows: Query<&Window>,
    buttons: Query<(&Interaction, &Action, &Children), With<Hybrid>>,
    mut texts: Query<(&mut TextColor, &ComputedNode, Has<Glow>)>,
    mut open_parts: Query<&mut Node, (With<OpenPart>, Without<Blotch>)>,
    mut blotches: Query<&mut Node, (With<Blotch>, Without<OpenPart>)>,
) {
    if frontend.playing() && !frontend.is_changed() {
        return;
    }
    let unit = windows.iter().next().map_or(1., |w| w.height() / 720.);
    // OptionsFlyout.res InitialFocus "BtnVideo": Video holds the focus until
    // the mouse moves onto another entry.
    let flyout_hover = buttons.iter().any(|(i, a, _)| {
        matches!(i, Interaction::Hovered | Interaction::Pressed)
            && matches!(a.0, ButtonAction::Audio | ButtonAction::Video | ButtonAction::TouchHud)
    });
    for (interaction, action, children) in &buttons {
        let open = matches!(action.0, ButtonAction::Options) && frontend.page == Page::Options
            || matches!(action.0, ButtonAction::Vote) && frontend.page == Page::Vote;
        let focused = matches!(interaction, Interaction::Hovered | Interaction::Pressed)
            || matches!(action.0, ButtonAction::Video) && frontend.page == Page::Options && !flyout_hover;
        let (grey, blur) = if open {
            (200., 30.)
        } else if focused {
            (255., (time.elapsed_secs() * 4.).sin() * 30. + 60.)
        } else {
            (125., 0.)
        };
        let text_colour = Color::srgb_u8(grey as u8, grey as u8, grey as u8);
        let glow_colour = Color::WHITE.with_alpha(blur / 255. * GLOW_SHARE);
        let mut label_width = 0.;
        for child in children {
            if let Ok((mut colour, node, glow)) = texts.get_mut(*child) {
                let next = if glow { glow_colour } else { text_colour };
                if !glow {
                    label_width = node.size().x * node.inverse_scale_factor();
                }
                if colour.0 != next {
                    colour.0 = next;
                }
            }
        }
        let display = if open { Display::Flex } else { Display::None };
        for child in children {
            if let Ok(mut node) = open_parts.get_mut(*child) {
                if node.display != display {
                    node.display = display;
                }
            }
            if let Ok(mut node) = blotches.get_mut(*child) {
                let width = px(label_width + 60. * unit);
                if node.display != display || node.width != width {
                    node.display = display;
                    node.width = width;
                }
            }
        }
    }
}
/// ADD-ONS & MODS content: two tabs (Steam add-ons placeholder, mods list).
fn addons_page(commands: &mut Commands, panel: Entity, font: &Handle<Font>) {
    let text = |commands: &mut Commands, parent: Entity, value: &str, size: f32, color: Color, node: Node| {
        commands.spawn((
            Text::new(value),
            TextFont {
                font: bevy::text::FontSource::Handle(font.clone()),
                font_size: FontSize::Vh(size / 7.2),
                ..default()
            },
            TextColor(color),
            node,
            ChildOf(parent),
        ));
    };
    for (tab, label, x) in [(0u8, "STEAM ADD-ONS", 150.), (1, "MODS", 420.)] {
        button(commands, panel, label, font, rect(x, 136., 250., 32.), ButtonAction::AddonsTab(tab));
        commands.spawn((
            rect(x, 168., 250., 3.),
            BackgroundColor(Color::srgb(0.85, 0., 0.)),
            AddonsTabMark(tab),
            ChildOf(panel),
        ));
    }
    let grey = Color::srgb(0.7, 0.7, 0.7);
    // Steam add-ons (placeholder).
    let steam = commands
        .spawn((rect(0., 0., 1280., 463.), AddonsPanel(0), ChildOf(panel)))
        .id();
    text(commands, steam, "No Steam add-ons installed.", 21., Color::WHITE, rect(150., 200., 900., 30.));
    text(commands, steam, "Steam Workshop add-on support is coming later.", 17., grey, rect(150., 235., 900., 26.));
    // Mods.
    let mods = commands
        .spawn((rect(0., 0., 1280., 463.), AddonsPanel(1), ChildOf(panel)))
        .id();
    for (i, (name, description, controls)) in MODS.iter().enumerate() {
        let y = 195. + i as f32 * 95.;
        commands.spawn((
            Node { border: UiRect::all(px(1.)), ..rect(150., y, 980., 85.) },
            BackgroundColor(Color::srgba(0., 0., 0., 0.45)),
            BorderColor::all(Color::srgba(1., 1., 1., 0.25)),
            ChildOf(mods),
        ));
        text(commands, mods, name, 24., Color::WHITE, rect(165., y + 6., 600., 32.));
        text(commands, mods, description, 17., grey, rect(165., y + 36., 700., 24.));
        text(commands, mods, controls, 15., Color::srgb(0.55, 0.7, 0.9), rect(165., y + 58., 760., 22.));
        commands.spawn((
            Text::new(""),
            TextFont {
                font: bevy::text::FontSource::Handle(font.clone()),
                font_size: FontSize::Vh(17. / 7.2),
                ..default()
            },
            TextColor(Color::WHITE),
            rect(960., y + 8., 160., 24.),
            ModStatus(i),
            ChildOf(mods),
        ));
        button(commands, mods, "TURN ON / OFF", font, rect(940., y + 38., 180., 34.), ButtonAction::ToggleMod(i));
    }
}
/// Shows the selected ADD-ONS & MODS tab and its underline.
pub fn addons_tabs(
    mut panels: Query<(&AddonsPanel, &mut Node), Without<AddonsTabMark>>,
    mut marks: Query<(&AddonsTabMark, &mut Node), Without<AddonsPanel>>,
    mut status: Query<(&ModStatus, &mut Text, &mut TextColor)>,
    mods: Option<Res<crate::skate_mod::Mods>>,
) {
    let on = mods.as_ref().is_some_and(|m| crate::skate_mod::skate_enabled(m));
    for (_, mut text, mut color) in &mut status {
        let (label, tint) = if on { ("ENABLED", Color::srgb(0.4, 0.9, 0.4)) } else { ("DISABLED", Color::srgb(0.8, 0.3, 0.3)) };
        if text.0 != label {
            text.0 = label.into();
            color.0 = tint;
        }
    }
    static CAPTURE: std::sync::Once = std::sync::Once::new();
    CAPTURE.call_once(|| {
        if std::env::args().any(|a| a == "--capture-addons-steam") {
            ADDONS_TAB.store(0, std::sync::atomic::Ordering::Relaxed);
        }
    });
    let tab = ADDONS_TAB.load(std::sync::atomic::Ordering::Relaxed);
    for (panel, mut node) in &mut panels {
        let display = if panel.0 == tab { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    for (mark, mut node) in &mut marks {
        let display = if mark.0 == tab { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
}
pub fn setup(
    mut commands: Commands,
    source: Res<HudSource>,
    mut images: ResMut<Assets<Image>>,
    mut fonts: ResMut<Assets<Font>>,
    camera: Single<Entity, With<crate::game::ViewCamera>>,
    mut queue: ResMut<game_audio::Queue>,
    frontend: Res<Frontend>,
    catalog: Res<crate::campaign_settings::Catalog>,
    windows: Query<&Window>,
) {
    let mut assets = SourceAssets::new(&source.0, vec![]).expect("Menu mounts");
    let font = assets
        .read("resource/stubble-bold.vfont")
        .and_then(|b| crate::vfont::decode(&b))
        .map(|b| crate::game_ui::shared_font(&mut *fonts, b))
        .unwrap_or_default();
    // Without the video (no decoder, or iOS before tools/ios-copy-game.py
    // has copied the MP4s) the menu keeps its own still. The numbered
    // console/background%02d stills are retail's startup/loading screens.
    let _ = &windows;
    let mut background = image(
        &mut assets,
        &mut images,
        "materials/console/background_menu_widescreen.vtf",
    );
    if !frontend.playing() {
        match crate::menu_video::start(&source.0, &mut images) {
            Ok(video) => {
                background = video.image.clone();
                commands.insert_resource(video);
            }
            Err(e) => eprintln!("Menu video fallback to native still: {e}"),
        }
    }
    let modes = MODES
        .iter()
        .map(|(_, mode)| {
            image(
                &mut assets,
                &mut images,
                &format!("materials/vgui/menu_mode_{mode}.vtf"),
            )
        })
        .collect::<Vec<_>>();
    // Native mainmenu.res: pic offset x=100, 153-square main tile, 86-square subtiles,
    // labels at x=100, y=255..355. Source's 480-line UI space is scaled by 1.5.
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: percent(100.),
                height: percent(100.),
                left: vh((0.) / 7.2),
                top: vh((0.) / 7.2),
                min_width: vh((0.) / 7.2),
                min_height: vh((0.) / 7.2),
                ..default()
            },
            ImageNode::new(background).with_mode(bevy::ui::widget::NodeImageMode::Stretch),
            GlobalZIndex(100),
            UiTargetCamera(*camera),
            MenuRoot,
        ))
        .id();
    // Retail places the carousel, mode title and buttons 13.5 pixels lower
    // at 720p than this layout was drawn (mainmenu.res BtnOptions ypos 275 ->
    // 412.5); the logo below keeps its place.
    let home = commands
        .spawn((
            Node {
                width: percent(100.),
                height: percent(100.),
                top: vh(HOME_SHIFT / 7.2),
                ..default()
            },
            PageNode(Page::Home),
            ChildOf(root),
        ))
        .id();
    let logo = image(
        &mut assets,
        &mut images,
        "materials/vgui/left4dead2logo.vtf",
    );
    commands.spawn((
        ImageNode {
            color: Color::srgba(0.65, 0.65, 0.65, 0.17),
            ..ImageNode::new(logo)
        },
        rect(630., 18. - HOME_SHIFT, 570., 285.),
        ChildOf(home),
    ));
    let selected = commands
        .spawn((
            Button,
            rect(150., 38., 229.5, 229.5),
            UiTransform::from_rotation(Rot2::degrees(-3.)),
            ImageNode::new(modes[0].clone()),
            Action(ButtonAction::Campaign),
            SelectedMode,
            CarouselTile(0),
            ChildOf(home),
        ))
        .id();
    let frame = image(
        &mut assets,
        &mut images,
        "materials/vgui/menu_mode_border.vtf",
    );
    commands.spawn((
        ImageNode::new(frame),
        rect(0., 0., 229.5, 229.5),
        ChildOf(selected),
    ));
    for index in 1..6 {
        let mut bounds = rect(455. + (index - 1) as f32 * 137., 145., 129., 129.);
        bounds.overflow = Overflow::clip();
        let tile = commands
            .spawn((
                Button,
                bounds,
                ImageNode::new(modes[index].clone()),
                Action(ButtonAction::Mode(index)),
                CarouselTile(index),
                ChildOf(home),
            ))
            .id();
        commands.spawn((
            rect(0., 109., 129., 20.),
            BackgroundColor(Color::BLACK),
            ChildOf(tile),
        ));
        commands.spawn((
            Text::new(MODES[index].0),
            TextLayout::default().with_no_wrap(),
            TextFont {
                font: bevy::text::FontSource::Handle(font.clone()),
                font_size: FontSize::Vh((17.) / 7.2),
                ..default()
            },
            rect(5., 109., 124., 20.),
            ChildOf(tile),
        ));
    }
    for (x, direction, action) in [
        (113., "left", ButtonAction::Previous),
        (405., "right", ButtonAction::Next),
    ] {
        let arrow = image(
            &mut assets,
            &mut images,
            &format!("materials/vgui/arrow_{direction}.vtf"),
        );
        commands.spawn((
            Button,
            ImageNode::new(arrow),
            rect(x, 243., 23., 30.),
            BackgroundColor(Color::NONE),
            Action(action),
            ChildOf(home),
        ));
    }
    commands.spawn((
        rect(0., 282., 1280., 72.),
        BackgroundColor(Color::srgba(0., 0., 0., 0.3)),
        ChildOf(home),
    ));
    commands.spawn((
        Text::new("PLAY CAMPAIGN"),
        TextFont {
            font: bevy::text::FontSource::Handle(font.clone()),
            font_size: FontSize::Vh((27.) / 7.2),
            ..default()
        },
        rect(150., 290., 800., 37.),
        ModeTitle,
        ChildOf(home),
    ));
    commands.spawn((
        Text::new("Play through a co-operative campaign online."),
        TextFont {
            font: bevy::text::FontSource::Handle(font.clone()),
            font_size: FontSize::Vh((17.) / 7.2),
            ..default()
        },
        rect(150., 319., 950., 28.),
        ModeDescription,
        ChildOf(home),
    ));
    let mut home_buttons = vec![
        ("JOIN A GAME", ButtonAction::Browser),
        ("FRIENDS", ButtonAction::Friends),
        ("OPTIONS", ButtonAction::Options),
        ("ADD-ONS & MODS", ButtonAction::Addons),
        ("EXTRAS", ButtonAction::Extras),
        ("ACHIEVEMENTS", ButtonAction::Extras),
    ];
    if cfg!(target_os = "ios") {
        home_buttons.push(("GAME FILES", ButtonAction::GameFiles));
    }
    home_buttons.push(("QUIT", ButtonAction::Quit));
    for (i, (label, action)) in home_buttons.into_iter().enumerate() {
        // Phones: two columns of four with 40-unit rows, so every button
        // has room to tap and the list stays inside the screen (ui_fit's
        // home fit keeps y 47..595 on screen).
        let node = if crate::game::ui_fit::touch_build() {
            let (col, row) = (i / 4, i % 4);
            // The right column starts past the OPTIONS flyout (x 450..684).
            rect(150. + 570. * col as f32, 369. + 42. * row as f32, 300., 38.)
        } else {
            rect(150., 369. + 30. * i as f32, 270., 30.)
        };
        hybrid_button(&mut commands, home, label, &font, node, action, Hybrid::Main);
    }
    campaign_setup(&mut commands, root, &font, &mut assets, &mut images);
    let chat_font = crate::game_chat::chat_font(&mut fonts);
    lobby::setup(&mut commands, root, &font, &chat_font, &mut assets, &mut images);
    browser::setup(&mut commands, root, &font, &mut assets, &mut images);
    for page in [
        Page::Audio,
        Page::Video,
        Page::Extras,
        Page::Pause,
        Page::Options,
        Page::Vote,
        Page::Difficulty,
        Page::Advanced,
        Page::Addons,
        Page::Friends,
    ] {
        let panel = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: percent(100.),
                    height: percent(100.),
                    display: Display::None,
                    ..default()
                },
                PageNode(page),
                ChildOf(root),
            ))
            .id();
        if page == Page::Vote {
            // The flyout opens at the CALL A VOTE button's top right; the
            // button itself paints the open state (`Hybrid`).
            let flyout = flyout_panel(&mut commands, panel, rect(480., 275., 234., 130.), 35.);
            for (label, row, action) in [
                ("Change Difficulty", 1., ButtonAction::Difficulty),
                ("Start New Campaign", 2., ButtonAction::NewCampaign),
                ("Restart Campaign", 3., ButtonAction::Restart),
            ] {
                hybrid_button(
                    &mut commands,
                    flyout,
                    label,
                    &font,
                    rect(12., FLYOUT_TEXT + row * FLYOUT_ROW, 210., FLYOUT_ROW),
                    action,
                    Hybrid::Flyout,
                );
            }
            for (label, row) in [
                ("Return to Lobby", 0.),
                ("Kick Player", 4.),
                ("Change All Talk", 5.),
            ] {
                disabled_label(
                    &mut commands,
                    flyout,
                    label,
                    &font,
                    rect(12., FLYOUT_TEXT + row * FLYOUT_ROW, 210., FLYOUT_ROW),
                );
            }
            continue;
        }
        if page == Page::Difficulty {
            let dialog = commands
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: percent(26.6),
                        top: percent(34.3),
                        width: vh(600. / 7.2),
                        height: vh(227. / 7.2),
                        border: UiRect::all(px(1.)),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.16, 0.16, 0.16)),
                    BorderColor::all(Color::srgb(0.55, 0.55, 0.55)),
                    ChildOf(panel),
                ))
                .id();
            commands.spawn((
                Text::new("CHOOSE NEW DIFFICULTY LEVEL"),
                TextFont {
                    font: bevy::text::FontSource::Handle(font.clone()),
                    font_size: FontSize::Vh(28. / 7.2),
                    ..default()
                },
                TextColor(Color::srgb(0.8, 0.8, 0.8)),
                rect(14., 10., 575., 38.),
                ChildOf(dialog),
            ));
            commands.spawn((
                Text::new("Current Difficulty: Normal"),
                TextFont {
                    font: bevy::text::FontSource::Handle(font.clone()),
                    font_size: FontSize::Vh(17. / 7.2),
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.6, 0.6)),
                rect(21., 49., 500., 25.),
                Detail::Difficulty,
                ChildOf(dialog),
            ));
            for (i, difficulty) in crate::campaign_settings::Difficulty::ALL.iter().enumerate() {
                button(
                    &mut commands,
                    dialog,
                    difficulty.name(),
                    &font,
                    rect(21., 80. + i as f32 * 23., 400., 25.),
                    ButtonAction::SetDifficulty(i),
                );
            }
            button(
                &mut commands,
                dialog,
                "Cancel",
                &font,
                rect(21., 186., 400., 26.),
                ButtonAction::Back,
            );
            continue;
        }
        if page == Page::Options {
            // Retail OptionsFlyout.res, opened at the OPTIONS button's top
            // right (home: 150 + 270, 412.5; `presentation` moves it to the
            // in-game button). The button paints its own open state.
            // Its left edge runs down from the owner's bottom: phones'
            // home buttons are 38 tall.
            let phone = crate::game::ui_fit::touch_build();
            let owner = if phone { 38. } else { 30. };
            if phone {
                // Phones: only the entries that do something on a phone
                // (no Steam Input, Multiplayer or Steam Cloud), in rows tall
                // enough to tap.
                const ROW: f32 = 34.;
                let flyout = flyout_panel(&mut commands, panel, rect(420., 412.5, 234., 3. * ROW + 12.), owner);
                commands.entity(flyout).insert(OptionsFlyout);
                for (i, (label, action)) in [
                    ("Video", ButtonAction::Video),
                    ("Audio", ButtonAction::Audio),
                    ("Touch & HUD", ButtonAction::TouchHud),
                ]
                .into_iter()
                .enumerate()
                {
                    // Labels sit at the top of their button; each button
                    // reaches down to the next label.
                    let top = FLYOUT_TEXT + (ROW - FLYOUT_ROW) * 0.5 + i as f32 * ROW;
                    hybrid_button(&mut commands, flyout, label, &font, rect(12., top, 210., ROW), action, Hybrid::Flyout);
                }
                continue;
            }
            let flyout = flyout_panel(&mut commands, panel, rect(420., 412.5, 234., 130.), owner);
            commands.entity(flyout).insert(OptionsFlyout);
            hybrid_button(
                &mut commands,
                flyout,
                "Video",
                &font,
                rect(12., FLYOUT_TEXT, 210., FLYOUT_ROW),
                ButtonAction::Video,
                Hybrid::Flyout,
            );
            hybrid_button(
                &mut commands,
                flyout,
                "Audio",
                &font,
                rect(12., FLYOUT_TEXT + FLYOUT_ROW, 210., FLYOUT_ROW),
                ButtonAction::Audio,
                Hybrid::Flyout,
            );
            disabled_label(&mut commands, flyout, "Steam Input", &font, rect(12., FLYOUT_TEXT + 2. * FLYOUT_ROW, 210., FLYOUT_ROW));
            hybrid_button(
                &mut commands,
                flyout,
                "HUD Size",
                &font,
                rect(12., FLYOUT_TEXT + 3. * FLYOUT_ROW, 210., FLYOUT_ROW),
                ButtonAction::TouchHud,
                Hybrid::Flyout,
            );
            // Not built yet: drawn like retail's idle entries.
            for (label, row) in [("Multiplayer", 4.), ("Steam Cloud", 5.)] {
                commands.spawn((
                    Text::new(label),
                    TextFont {
                        font: bevy::text::FontSource::Handle(font.clone()),
                        font_size: FontSize::Vh(17. / 7.2),
                        ..default()
                    },
                    TextColor(Color::srgb_u8(125, 125, 125)),
                    rect(12., FLYOUT_TEXT + row * FLYOUT_ROW, 210., FLYOUT_ROW),
                    ChildOf(flyout),
                ));
            }
            continue;
        }
        if page == Page::Pause {
            commands.spawn((
                if crate::game::ui_fit::touch_build() {
                    band(190., 340.)
                } else {
                    Node { width: percent(100.), ..rect(0., 190., 1280., 340.) }
                },
                BackgroundColor(Color::srgba(0., 0., 0., 0.75)),
                ChildOf(panel),
            ));
            for (label, y, action) in [
                ("RETURN TO GAME", 201., ButtonAction::Resume),
                ("ACHIEVEMENTS", 386., ButtonAction::Extras),
                ("OPTIONS", 423., ButtonAction::Options),
                ("CALL A VOTE", 275., ButtonAction::Vote),
                ("EXIT TO MAIN MENU", 479., ButtonAction::Home),
            ] {
                hybrid_button(
                    &mut commands,
                    panel,
                    label,
                    &font,
                    rect(150., y, 330., 35.),
                    action,
                    Hybrid::Main,
                );
            }
            for (label, y) in [
                ("TAKE A BREAK", 238.),
                ("INVITE FRIENDS", 312.),
                ("LEADERBOARDS", 349.),
            ] {
                commands.spawn((
                    Text::new(label),
                    TextFont {
                        font: bevy::text::FontSource::Handle(font.clone()),
                        font_size: FontSize::Vh((24.) / 7.2),
                        ..default()
                    },
                    TextColor(Color::srgb(0.36, 0.39, 0.4)),
                    rect(150., y, 420., 35.),
                    ChildOf(panel),
                ));
            }
            continue;
        }
        commands.spawn((
            band(0., 85.),
            BackgroundColor(Color::srgba(0., 0., 0., 0.78)),
            ChildOf(panel),
        ));
        let title = match page {
            Page::Audio => "AUDIO",
            Page::Video => "VIDEO",
            Page::Advanced => "ADVANCED VIDEO",
            Page::Addons => "ADD-ONS & MODS",
            Page::Friends => "FRIENDS",
            _ => "EXTRAS",
        };
        commands.spawn((
            Text::new(title),
            TextFont {
                font: bevy::text::FontSource::Handle(font.clone()),
                font_size: FontSize::Vh((32.) / 7.2),
                ..default()
            },
            rect(130., 5., 1000., 45.),
            ChildOf(panel),
        ));
        commands.spawn((
            Text::new(if page == Page::Video || page == Page::Advanced {
                "Find your optimal settings based on your hardware and preferences."
            } else if page == Page::Audio {
                "Adjust your audio settings."
            } else if page == Page::Addons {
                "Steam add-ons and local mods."
            } else if page == Page::Friends {
                "Friends who play the rewrite and their status."
            } else {
                "Campaign mode is available locally."
            }),
            TextFont {
                font: bevy::text::FontSource::Handle(font.clone()),
                font_size: FontSize::Vh((17.) / 7.2),
                ..default()
            },
            rect(130., 48., 1000., 27.),
            ChildOf(panel),
        ));
        commands.spawn((
            band(128., 335.),
            BackgroundColor(Color::srgba(0., 0., 0., 0.68)),
            ChildOf(panel),
        ));
        if page == Page::Video || page == Page::Advanced {
            use crate::graphics_options::OptionKey as G;
            // Phones: no aspect ratio, resolution, display mode or film
            // grain (the window is the screen); the ADVANCED settings sit on
            // VIDEO itself. iPhone forces MSAA off and is always synced, so
            // anti-aliasing and vsync are left out there.
            let phone = crate::game::ui_fit::touch_build();
            let rows: Vec<(&str, G)> = if page == Page::Video && phone {
                let mut rows = vec![("BRIGHTNESS", G::Brightness)];
                if !cfg!(target_os = "ios") {
                    rows.push(("ANTI-ALIASING MODE", G::Aa));
                }
                rows.extend([
                    ("FILTERING MODE", G::Filtering),
                    ("SHADER DETAIL", G::Shader),
                    ("EFFECT DETAIL", G::Effects),
                    ("MODEL / TEXTURE DETAIL", G::Detail),
                ]);
                rows
            } else if page == Page::Video {
                vec![
                    ("BRIGHTNESS", G::Brightness),
                    ("ASPECT RATIO", G::Aspect),
                    ("RESOLUTION", G::Resolution),
                    ("DISPLAY MODE", G::Display),
                    ("FILM GRAIN AMOUNT", G::Grain),
                ]
            } else {
                vec![
                    ("ANTI-ALIASING MODE", G::Aa),
                    ("FILTERING MODE", G::Filtering),
                    ("SHADER DETAIL", G::Shader),
                    ("EFFECT DETAIL", G::Effects),
                    ("MODEL / TEXTURE DETAIL", G::Detail),
                ]
            };
            let count = rows.len();
            // Phones: larger values, centred on the 24-unit labels (text
            // hangs from its node's top, so a smaller value sat high).
            let value_size = if page == Page::Video && phone { 20. } else { 17. };
            let value_drop = if page == Page::Video && phone { (24. - value_size) * 0.5 } else { 0. };
            for (i, (label, key)) in rows.into_iter().enumerate() {
                let y = 140. + i as f32 * 35.;
                button(
                    &mut commands,
                    panel,
                    label,
                    &font,
                    Node {
                        left: percent(21.5),
                        width: percent(70.),
                        ..rect(290., y, 760., 30.)
                    },
                    ButtonAction::Graphics(key),
                );
                commands.spawn((
                    Text::default(),
                    TextFont {
                        font: bevy::text::FontSource::Handle(font.clone()),
                        font_size: FontSize::Vh(value_size / 7.2),
                        ..default()
                    },
                    TextColor(Color::srgb(0.7, 0.7, 0.7)),
                    Node {
                        // Phones: the values sit just past the longest label.
                        left: percent(if page == Page::Video && phone { 45. } else { 60. }),
                        width: percent(30.),
                        ..rect(765., y + value_drop, 380., 30.)
                    },
                    Detail::Graphics(key),
                    ChildOf(panel),
                ));
            }
            if page == Page::Video && phone {
                // Same column and text size as the option rows above.
                let row = |y: f32| Node { left: percent(21.5), width: percent(70.), ..rect(290., y, 760., 30.) };
                let y = 140. + count as f32 * 35.;
                button(&mut commands, panel, "SHOW FPS: OFF", &font, row(y), ButtonAction::ShowFps);
                button(&mut commands, panel, "USE RECOMMENDED SETTINGS", &font, row(y + 35.), ButtonAction::Recommended);
            } else if page == Page::Video {
                button(
                    &mut commands,
                    panel,
                    "ADVANCED SETTINGS",
                    &font,
                    rect(290., 320., 760., 30.),
                    ButtonAction::Advanced,
                );
                button(
                    &mut commands,
                    panel,
                    "USE RECOMMENDED SETTINGS",
                    &font,
                    rect(290., 355., 760., 30.),
                    ButtonAction::Recommended,
                );
            } else {
                button(
                    &mut commands,
                    panel,
                    "WAIT FOR VERTICAL SYNC",
                    &font,
                    Node {
                        left: percent(21.5),
                        width: percent(70.),
                        ..rect(290., 315., 760., 30.)
                    },
                    ButtonAction::Vsync,
                );
                commands.spawn((
                    Text::default(),
                    TextFont {
                        font: bevy::text::FontSource::Handle(font.clone()),
                        font_size: FontSize::Vh(17. / 7.2),
                        ..default()
                    },
                    TextColor(Color::srgb(0.7, 0.7, 0.7)),
                    Node {
                        left: percent(60.),
                        width: percent(30.),
                        ..rect(765., 315., 380., 30.)
                    },
                    Detail::Vsync,
                    ChildOf(panel),
                ));
                button(
                    &mut commands,
                    panel,
                    "SHOW FPS: OFF",
                    &font,
                    rect(290., 350., 760., 30.),
                    ButtonAction::ShowFps,
                );
                button(
                    &mut commands,
                    panel,
                    &crate::low_violence::label(),
                    &font,
                    rect(290., 385., 760., 30.),
                    ButtonAction::LowViolence,
                );
            }
        } else if page == Page::Addons {
            addons_page(&mut commands, panel, &font);
        } else if page == Page::Friends {
            // Placeholder until the friends book is connected to the menu.
            commands.spawn((
                Text::new("No friends yet."),
                TextFont {
                    font: bevy::text::FontSource::Handle(font.clone()),
                    font_size: FontSize::Vh((21.) / 7.2),
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.6, 0.6)),
                rect(335., 180., 600., 35.),
                ChildOf(panel),
            ));
        } else if page == Page::Audio {
            commands.spawn((
                Text::new("MASTER VOLUME"),
                TextFont {
                    font: bevy::text::FontSource::Handle(font.clone()),
                    font_size: FontSize::Vh((21.) / 7.2),
                    ..default()
                },
                rect(335., 180., 500., 35.),
                ChildOf(panel),
            ));
            button(
                &mut commands,
                panel,
                "-",
                &font,
                rect(335., 225., 50., 40.),
                ButtonAction::Volume(-0.1),
            );
            button(
                &mut commands,
                panel,
                "+",
                &font,
                rect(545., 225., 50., 40.),
                ButtonAction::Volume(0.1),
            );
            commands.spawn((
                Text::new("70%"),
                TextFont {
                    font: bevy::text::FontSource::Handle(font.clone()),
                    font_size: FontSize::Vh((24.) / 7.2),
                    ..default()
                },
                rect(415., 225., 120., 40.),
                VolumeLabel,
                ChildOf(panel),
            ));
        }
        button(
            &mut commands,
            panel,
            "DONE",
            &font,
            rect(
                335.,
                if page == Page::Video && crate::game::ui_fit::touch_build() {
                    // Below the phone's longer VIDEO list.
                    430.
                } else if page == Page::Advanced {
                    // Below LOW VIOLENCE.
                    432.
                } else if page == Page::Video {
                    410.
                } else {
                    370.
                },
                420.,
                35.,
            ),
            ButtonAction::Back,
        );
    }
    // Survival maps have their own pictures (survival.rs swaps the list in).
    let survival = crate::campaign_settings::Catalog::survival(&source.0);
    let chapters = catalog
        .campaigns
        .iter()
        .chain(survival.iter())
        .flat_map(|c| &c.chapters)
        .map(|ch| {
            (
                ch.image.clone(),
                image(
                    &mut assets,
                    &mut images,
                    &format!("materials/vgui/{}.vtf", ch.image),
                ),
            )
        })
        .collect();
    commands.insert_resource(MenuArt { modes, chapters });
    if !frontend.playing() {
        queue.0.push_back(game_audio::Request {
            name: "sound/music/l4d2/l4d2_c1.wav".into(),
            position: None,
            looping: true,
            gain: 1.,
            map_entity: None, soundlevel: None,
        });
    }
}
/// Native UI handles taps; a horizontal drag on the mode cards changes
/// selection once on release. Never turn a vertical drag or cancelled touch
/// into a menu action, and never feed these gestures into gameplay controls.
pub(crate) fn touch_carousel(
    touches: Res<bevy::input::touch::Touches>,
    windows: Query<&Window>,
    mut frontend: ResMut<Frontend>,
    mut queue: ResMut<game_audio::Queue>,
    mut gesture: Local<Option<(u64, Vec2, usize)>>,
    tiles: Query<(&ComputedNode, &UiGlobalTransform, &InheritedVisibility), With<CarouselTile>>,
) {
    if frontend.page != Page::Home {
        *gesture = None;
        return;
    }
    let Some(window) = windows.iter().next() else { return; };
    let height = window.height();
    if height <= 0. { return; }
    // Start on a card's on-screen rectangle (UiGlobalTransform includes the
    // phone scaling from ui_fit), not a fixed screen band.
    let scale = window.scale_factor();
    let on_card = |at: Vec2| {
        let mut any = false;
        for (node, transform, visible) in &tiles {
            if !visible.get() {
                continue;
            }
            any = true;
            if node.contains_point(*transform, at * scale) {
                return true;
            }
        }
        !any && at.y >= height * 0.04 && at.y < height * 0.43
    };
    // A dropped release (app switch, notification) must not block later swipes.
    if let Some((id, _, _)) = *gesture {
        if touches.get_pressed(id).is_none()
            && !touches.iter_just_released().any(|t| t.id() == id)
        {
            *gesture = None;
        }
    }
    for touch in touches.iter_just_pressed() {
        if gesture.is_none() && on_card(touch.position()) {
            *gesture = Some((touch.id(), touch.position(), frontend.mode));
        }
    }
    if let Some((id, start, mode)) = *gesture {
        if touches.iter_just_canceled().any(|t| t.id() == id) {
            *gesture = None;
        } else if let Some(touch) = touches.iter_just_released().find(|t| t.id() == id) {
            let delta = touch.position() - start;
            if let Some(next) = swipe_mode(mode, delta, height) {
                frontend.mode = next;
                queue.play("Menu.Scroll", None);
            }
            *gesture = None;
        }
    }
}

fn swipe_mode(mode: usize, delta: Vec2, height: f32) -> Option<usize> {
    if delta.x.abs() < height * 0.08 || delta.x.abs() <= delta.y.abs() * 1.5 {
        return None;
    }
    Some((mode + if delta.x < 0. { 1 } else { MODES.len() - 1 }) % MODES.len())
}

pub fn update(
    mut commands: Commands,
    mut frontend: ResMut<Frontend>,
    mut s: ResMut<Match>,
    input: (
        Res<ButtonInput<KeyCode>>,
        Option<Res<bevy::input::touch::Touches>>,
        Local<Option<(u64, ButtonAction, Vec2, Page)>>,
    ),
    mut buttons: Query<(&Interaction, &Action, &mut BackgroundColor, Has<Hybrid>, Has<CarouselTile>), Changed<Interaction>>,
    mut queue: ResMut<game_audio::Queue>,
    mut settings: ResMut<game_audio::Settings>,
    display_settings: (
        ResMut<crate::performance::DisplaySettings>,
        Option<ResMut<crate::graphics_options::Graphics>>,
    ),
    music: Query<Entity, With<game_audio::MenuMusic>>,
    mut exit: MessageWriter<AppExit>,
    mut selection: Option<ResMut<crate::campaign_settings::Selection>>,
    catalog: Option<Res<crate::campaign_settings::Catalog>>,
    mut session: Option<ResMut<crate::campaign_settings::SessionSettings>>,
    mut director: Option<ResMut<crate::source_director::Director>>,
    mut windows: Query<&mut Window>,
    runtime_source: (
        Option<Res<HudSource>>,
        Option<ResMut<crate::map_transition::Requests>>,
        Option<Res<crate::game_console::Console>>,
        Option<ResMut<lobby::Lobby>>,
        Option<Res<Time<Real>>>,
        Option<ResMut<browser::Browser>>,
    ),
) {
    let (source, mut requests, console, mut game_lobby, real_time, mut game_browser) = runtime_source;
    let now = real_time.as_ref().map_or(0., |t| t.elapsed_secs_f64());
    let (keys, touches, mut pending_tap) = input;
    let (mut display_settings, mut graphics) = display_settings;
    if frontend.page == Page::Loading {
        return;
    }
    if console.is_some_and(|c| c.open) {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        if frontend.page == Page::Campaign {
            if let Some(sel) = selection.as_mut() {
                if sel.open.take().is_some() {
                    queue.play("Menu.Cancel", None);
                    return;
                }
            }
            // Editing the lobby's settings: Escape returns to the lobby.
            if game_lobby.as_ref().is_some_and(|l| l.joined) {
                frontend.page = Page::Lobby;
                queue.play("Menu.Cancel", None);
                return;
            }
        }
        if frontend.page == Page::Lobby {
            if let Some(l) = game_lobby.as_mut() {
                if l.open.take().is_some() {
                    queue.play("Menu.Cancel", None);
                    return;
                }
                // Escape leaves the lobby, like LEAVE LOBBY.
                lobby::leave(l);
            }
        }
        frontend.page = if frontend.playing() {
            s.paused = true;
            Page::Pause
        } else if frontend.page == Page::Pause {
            Page::Game
        } else if frontend.started {
            Page::Pause
        } else {
            Page::Home
        };
        queue.play("Menu.Cancel", None);
    }
    let mut activated = Vec::new();
    if let (Some(touches), Some((id, action, start, page))) = (touches.as_deref(), *pending_tap) {
        if touches.iter_just_canceled().any(|t| t.id() == id) || frontend.page != page {
            *pending_tap = None;
        } else if let Some(touch) = touches.iter_just_released().find(|t| t.id() == id) {
            let height = windows.iter().next().map_or(720., |w| w.height());
            if touch.position().distance(start) < height * 0.08 {
                activated.push(action);
            }
            *pending_tap = None;
        }
    }
    for (interaction, action, mut color, hybrid, tile) in &mut buttons {
        // Mode pictures get no red hover fill: it showed as a red fringe
        // around the rotated, framed picture (retail has none).
        color.0 = if *interaction == Interaction::Hovered && !hybrid && !tile {
            Color::srgba(0.55, 0.05, 0.04, 0.75)
        } else {
            Color::NONE
        };
        if *interaction == Interaction::Hovered {
            queue.play("Menu.Scroll", None);
        }
        if *interaction != Interaction::Pressed {
            continue;
        }
        // Picking can report Pressed a frame after the finger lands, so use
        // any held touch (newest first), measured from where it started.
        if let Some(touch) = touches.as_deref().and_then(|t| {
            t.iter_just_pressed().next().or_else(|| t.iter().max_by_key(|t| t.id()))
        }) {
            // Wait for release: pressing a card must not open a campaign
            // before we can distinguish a tap from a carousel swipe.
            if pending_tap.is_none() {
                *pending_tap = Some((touch.id(), action.0, touch.start_position(), frontend.page));
            }
            continue;
        }
        if touches.as_deref().is_some_and(|t| t.iter().next().is_some()) {
            continue;
        }
        activated.push(action.0);
    }
    // The lobby leader started an online game: start it here too.
    let forced_start = game_lobby.as_mut().is_some_and(|l| std::mem::take(&mut l.pending_start));
    if forced_start {
        activated.push(ButtonAction::LobbyStart);
    }
    let lobby_client = game_lobby.as_ref().is_some_and(|l| l.client);
    for action in activated {
        // Only the Leader can start the game or change its settings
        // (L4D360UI_Lobby_StartMatchmacking_Disabled_Tip).
        if lobby_client
            && frontend.page == Page::Lobby
            && (matches!(action, ButtonAction::EditLobbySettings | ButtonAction::LobbyAccess(_) | ButtonAction::LobbyDrop(1))
                || matches!(action, ButtonAction::LobbyStart) && !forced_start)
        {
            queue.play("Menu.Deny", None);
            continue;
        }
        queue.play("Menu.Select", None);
        match action {
            ButtonAction::Mode(offset) => frontend.mode = (frontend.mode + offset) % MODES.len(),
            ButtonAction::Next => frontend.mode = (frontend.mode + 1) % MODES.len(),
            ButtonAction::Previous => {
                frontend.mode = (frontend.mode + MODES.len() - 1) % MODES.len()
            }
            ButtonAction::Campaign => {
                // Campaign and Survival pick a map on the campaign page
                // (survival.rs swaps in the Survival map list).
                if frontend.mode == 0 || frontend.mode == crate::survival::MODE_INDEX {
                    frontend.page = Page::Campaign;
                } else {
                    frontend.page = Page::Extras;
                }
            }
            ButtonAction::Options => {
                frontend.page = if frontend.page == Page::Options {
                    if frontend.started {
                        Page::Pause
                    } else {
                        Page::Home
                    }
                } else {
                    Page::Options
                }
            }
            ButtonAction::Vote => frontend.page = Page::Vote,
            ButtonAction::Difficulty => frontend.page = Page::Difficulty,
            ButtonAction::NewCampaign => frontend.page = Page::Campaign,
            ButtonAction::Restart => {
                frontend.page = Page::Game;
                // Online only the lobby leader restarts (everyone; net_flow).
                if crate::net_flow::request_restart() {
                    frontend.restart = true;
                } else {
                    queue.play("Menu.Deny", None);
                }
            }
            ButtonAction::SetDifficulty(index) => {
                let difficulty = crate::campaign_settings::Difficulty::ALL[index];
                if let Some(session) = session.as_mut() {
                    session.difficulty = difficulty;
                }
                if let Some(selection) = selection.as_mut() {
                    selection.difficulty = difficulty;
                }
                if let Some(director) = director.as_mut() {
                    director.set_difficulty(difficulty);
                }
                frontend.page = Page::Pause;
            }
            ButtonAction::Advanced => frontend.page = Page::Advanced,
            ButtonAction::Recommended => {
                display_settings.show_fps = false;
                display_settings.vsync = false;
                display_settings.save();
                if let Some(graphics) = graphics.as_mut() {
                    **graphics = crate::graphics_options::Graphics::recommended();
                    graphics.save();
                }
            }
            ButtonAction::Graphics(key) => {
                if let Some(settings) = graphics.as_mut() {
                    settings.cycle(key);
                    settings.save();
                }
            }
            ButtonAction::DisplayMode => {
                for mut window in &mut windows {
                    window.mode = if window.mode == bevy::window::WindowMode::Windowed {
                        bevy::window::WindowMode::BorderlessFullscreen(
                            bevy::window::MonitorSelection::Current,
                        )
                    } else {
                        bevy::window::WindowMode::Windowed
                    };
                }
            }
            ButtonAction::Resolution => {
                for mut window in &mut windows {
                    let sizes = [(1280., 720.), (1600., 900.), (1920., 1080.), (1024., 768.)];
                    let next = sizes
                        .iter()
                        .position(|s| {
                            (s.0 - window.width()).abs() < 1. && (s.1 - window.height()).abs() < 1.
                        })
                        .map_or(0, |i| (i + 1) % sizes.len());
                    window.resolution.set(sizes[next].0, sizes[next].1);
                }
            }
            ButtonAction::Audio => frontend.page = Page::Audio,
            ButtonAction::Video => frontend.page = Page::Video,
            ButtonAction::TouchHud => frontend.page = Page::Touch,
            ButtonAction::Extras => frontend.page = Page::Extras,
            ButtonAction::Friends => frontend.page = Page::Friends,
            ButtonAction::Addons => frontend.page = Page::Addons,
            ButtonAction::GameFiles => {
                #[cfg(target_os = "ios")]
                crate::ios_installer::show_files(0);
            }
            ButtonAction::AddonsTab(tab) => {
                ADDONS_TAB.store(tab, std::sync::atomic::Ordering::Relaxed);
            }
            ButtonAction::ToggleMod(_) => {
                crate::skate_mod::TOGGLE_SKATE.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            ButtonAction::Home => {
                frontend.page = Page::Home;
                frontend.started = false;
                if music.is_empty() {
                    queue.0.push_back(game_audio::Request {
                        name: "sound/music/l4d2/l4d2_c1.wav".into(),
                        position: None,
                        looping: true,
                        gain: 1.,
                        map_entity: None, soundlevel: None,
                    });
                }
            }
            ButtonAction::Resume => {
                frontend.page = Page::Game;
                s.paused = true;
            }
            ButtonAction::CreateLobby => {
                if let Some(l) = game_lobby.as_mut() {
                    lobby::open(l, now);
                }
                frontend.page = Page::Lobby;
            }
            ButtonAction::EditLobbySettings => {
                if let Some(l) = game_lobby.as_mut() {
                    l.open = None;
                }
                frontend.page = Page::Campaign;
            }
            ButtonAction::LobbyDrop(drop) => {
                if let Some(l) = game_lobby.as_mut() {
                    l.open = if l.open == Some(drop) { None } else { Some(drop) };
                }
            }
            ButtonAction::LobbyAccess(index) => {
                if let Some(l) = game_lobby.as_mut() {
                    l.access = [lobby::Access::Public, lobby::Access::Friends, lobby::Access::Private]
                        [(index as usize).min(2)];
                    l.open = None;
                }
            }
            // The Steam invite dialog when there is a Steam lobby.
            ButtonAction::Browser => frontend.page = Page::Browser,
            ButtonAction::BrowserPick(row) => {
                if let Some(b) = game_browser.as_mut() {
                    if b.selected == Some(row) {
                        browser::join(b);
                    } else if row < b.found.len() {
                        b.selected = Some(row);
                    }
                }
            }
            ButtonAction::BrowserJoin => {
                if let Some(b) = game_browser.as_mut() {
                    browser::join(b);
                }
            }
            ButtonAction::LobbyInvite => {
                if !crate::multiplayer::steam_invite() {
                    frontend.page = Page::Friends;
                }
            }
            ButtonAction::LeaveLobby => {
                if let Some(l) = game_lobby.as_mut() {
                    lobby::leave(l);
                }
                frontend.page = Page::Home;
            }
            ButtonAction::Back if frontend.page == Page::Campaign
                && game_lobby.as_ref().is_some_and(|l| l.joined) =>
            {
                frontend.page = Page::Lobby;
            }
            ButtonAction::Back => {
                frontend.page = if frontend.page == Page::Advanced {
                    Page::Video
                } else if frontend.started {
                    Page::Pause
                } else {
                    Page::Home
                };
            }
            ButtonAction::Quit => {
                exit.write(AppExit::Success);
            }
            ButtonAction::LowViolence => {
                crate::low_violence::toggle();
            }
            ButtonAction::ShowFps => {
                display_settings.show_fps = !display_settings.show_fps;
                display_settings.save();
            }
            ButtonAction::Vsync => {
                display_settings.vsync = !display_settings.vsync;
                display_settings.save();
            }
            ButtonAction::Volume(delta) => {
                settings.volume = (settings.volume + delta).clamp(0., 1.)
            }
            ButtonAction::Select(field) => {
                if let Some(sel) = selection.as_mut() {
                    sel.open = if sel.open == Some(field) {
                        None
                    } else {
                        Some(field)
                    };
                }
            }
            ButtonAction::Choose(field, index) => {
                if let (Some(sel), Some(catalog)) = (selection.as_mut(), catalog.as_ref()) {
                    choose_setting(sel, catalog, field, index);
                }
                if let Some(l) = game_lobby.as_mut() {
                    l.open = None;
                }
            }
            ButtonAction::Start | ButtonAction::LobbyStart => {
                // The game starts with the players in the lobby; it closes
                // (an online session stays up for chat).
                if let Some(l) = game_lobby.as_mut() {
                    lobby::start(l);
                }
                if let (Some(sel), Some(catalog), Some(_session), Some(source)) = (
                    selection.as_mut(),
                    catalog.as_ref(),
                    session.as_ref(),
                    source.as_ref(),
                ) {
                    let campaign = &catalog.campaigns[sel.campaign];
                    let map = &campaign.chapters[sel.chapter].map;
                    // Packs can be installed after this menu's catalog was built.
                    // Resolve the newly installed map's actual Source mount now.
                    #[cfg(target_os = "ios")]
                    let installed_map = map.file_stem().and_then(|n| n.to_str())
                        .and_then(|name| crate::campaign_settings::find_map(&source.0, name));
                    #[cfg(target_os = "ios")]
                    let map = installed_map.as_ref().unwrap_or(map);
                    #[cfg(target_os = "ios")]
                    if !map.is_file() || !map.with_extension("nav").is_file() {
                        let id = l4d2r_steam_installer::packs::campaign(&map.to_string_lossy()).unwrap_or(16);
                        crate::ios_installer::show_files(id);
                        continue;
                    }
                    {
                        if let Some(requests) = requests.as_mut() {
                            requests.0 = Some((
                                source.0.clone(),
                                crate::campaign_settings::SessionSettings {
                                    map: map.clone(),
                                    survivor: sel.survivor,
                                    difficulty: sel.difficulty,
                                    original: campaign.original,
                                },
                            ));
                            s.paused = true;
                            queue.play("Menu.StartGame", None);
                        }
                        continue;
                    }
                }

                frontend.page = Page::Game;
                frontend.restart = true;
                frontend.started = true;
                s.paused = true;
                queue.play("Menu.StartGame", None);
                for e in &music {
                    commands.entity(e).despawn();
                }
            }
        }
    }
}
pub fn presentation(
    frontend: Res<Frontend>,
    mut roots: Query<
        (&mut Node, &mut ImageNode),
        (
            With<MenuRoot>,
            Without<PageNode>,
            Without<HudRoot>,
            Without<SelectedMode>,
            Without<CarouselTile>,
        ),
    >,
    mut pages: Query<
        (&PageNode, &mut Node),
        (Without<MenuRoot>, Without<HudRoot>, Without<CarouselTile>),
    >,
    mut hud: Query<
        &mut Node,
        (
            With<HudRoot>,
            Without<MenuRoot>,
            Without<PageNode>,
            Without<CarouselTile>,
        ),
    >,
    art: Res<MenuArt>,
    mut selected: Query<
        (&CarouselTile, &mut ImageNode, &Children, &mut Node),
        (Without<MenuRoot>, Without<HudRoot>, Without<PageNode>),
    >,
    mut text: Query<&mut Text>,
    _windows: Query<&Window>,
    mut scale: ResMut<UiScale>,
    time: Res<Time>,
    mut motion: Local<(usize, f32)>,
    video: Option<Res<crate::menu_video::Video>>,
    mut flyouts: Query<&mut UiTransform, With<OptionsFlyout>>,
) {
    // Hidden carousel layout must not invalidate the whole UI during gameplay.
    if frontend.playing() && !frontend.is_changed() {
        return;
    }
    for mut transform in &mut flyouts {
        // In game the OPTIONS button is 330 wide at y 423 (home: 270, 412.5).
        *transform = UiTransform::from_translation(if frontend.started {
            Val2::new(vh(60. / 7.2), vh(10.5 / 7.2))
        } else {
            // The flyout is built for an OPTIONS button at (150, 399, 270
            // wide); move it to where the home list puts the button now.
            let (dx, dy) = home_options_offset();
            Val2::new(vh(dx / 7.2), vh(dy / 7.2))
        });
    }
    if motion.0 != frontend.mode {
        motion.1 = if (frontend.mode + MODES.len() - motion.0) % MODES.len() <= MODES.len() / 2 {
            138.
        } else {
            -138.
        };
        motion.0 = frontend.mode;
    }
    motion.1 *= (-18. * time.delta_secs()).exp();
    if motion.1.abs() < 0.01 {
        motion.1 = 0.;
    }
    if scale.0 != 1. {
        scale.0 = 1.;
    }
    for (mut node, mut image) in &mut roots {
        if let Some(video) = &video {
            image.image = video.image.clone();
        }
        image.color = Color::srgba(
            1.,
            1.,
            1.,
            // TOUCH & HUD previews the HUD over the map behind the menu.
            if frontend.page == Page::Touch
                || frontend.started && frontend.page != Page::Home && frontend.page != Page::Campaign
            {
                0.
            } else {
                1.
            },
        );
        let display = if frontend.playing() {
            Display::None
        } else {
            Display::Flex
        };
        if node.display != display {
            node.display = display;
        }
    }
    for (page, mut node) in &mut pages {
        let display = if frontend.page == page.0
            || (frontend.page == Page::Vote && page.0 == Page::Pause)
            || (frontend.page == Page::Options
                && page.0
                    == if frontend.started {
                        Page::Pause
                    } else {
                        Page::Home
                    }) {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    for mut node in &mut hud {
        let display = if frontend.playing() || frontend.page == Page::Touch {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    if frontend.playing() {
        return;
    }
    for (tile, mut image, children, mut node) in &mut selected {
        let left = vh((if tile.0 == 0 {
            150.
        } else {
            455. + (tile.0 - 1) as f32 * 137.
        } + motion.1)
            / 7.2);
        if node.left != left {
            node.left = left;
        }
        let mode = (frontend.mode + tile.0) % MODES.len();
        if image.image != art.modes[mode] {
            image.image = art.modes[mode].clone();
        }
        for child in children {
            if let Ok(mut label) = text.get_mut(*child) {
                if label.0 != MODES[mode].0 {
                    label.0 = MODES[mode].0.into();
                }
            }
        }
    }
}
pub fn volume_label(
    frontend: Res<Frontend>,
    buttons: Query<(&Interaction, &Children, &Action), (Without<Hybrid>, Without<lobby::LobbyStartButton>)>,
    mut colors: Query<&mut TextColor>,
    mut mode_labels: Query<
        (&mut Text, Option<&ModeTitle>),
        (
            Or<(With<ModeTitle>, With<ModeDescription>)>,
            Without<VolumeLabel>,
        ),
    >,
    settings: Res<game_audio::Settings>,
    mut labels: Query<
        &mut Text,
        (
            With<VolumeLabel>,
            Without<ModeTitle>,
            Without<ModeDescription>,
        ),
    >,
) {
    for (interaction, children, action) in &buttons {
        for child in children {
            if let Ok(mut color) = colors.get_mut(*child) {
                let next = if matches!(interaction, Interaction::Hovered | Interaction::Pressed)
                    || matches!(action.0, ButtonAction::Options) && frontend.page == Page::Options
                {
                    Color::WHITE
                } else {
                    Color::srgb(0.6, 0.6, 0.6)
                };
                if color.0 != next {
                    color.0 = next;
                }
            }
        }
    }
    for (mut text, title) in &mut mode_labels {
        let next = if title.is_some() {
            format!("PLAY {}", MODES[frontend.mode].0)
        } else {
            match frontend.mode {
                0 => "Play through a co-operative campaign online.",
                1 => "Play a custom game mode.",
                2 => "Play a campaign with greater realism.",
                3 | 4 | 7 => {
                    "Play as the Survivors or the Infected in a round-based competitive match."
                }
                _ => "Survive as long as you can against the Infected.",
            }
            .into()
        };
        if text.0 != next {
            text.0 = next;
        }
    }
    for mut label in &mut labels {
        let next = format!("{:.0}%", settings.volume * 100.);
        if label.0 != next {
            label.0 = next;
        }
    }
}

pub fn detail_labels(
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    windows: Query<&Window>,
    mut labels: Query<(&Detail, &mut Text)>,
    graphics: Option<Res<crate::graphics_options::Graphics>>,
    capabilities: Option<Res<crate::graphics_options::Capabilities>>,
    display: Res<crate::performance::DisplaySettings>,
) {
    let (w, h) = windows
        .iter()
        .next()
        .map(|w| (w.width() as u32, w.height() as u32))
        .unwrap_or_else(crate::game_loading::dimensions);
    for (detail, mut text) in &mut labels {
        let next = match detail {
            Detail::Graphics(key) => graphics
                .as_ref()
                .map_or(String::new(), |g| g.label(*key, capabilities.as_deref())),
            Detail::Vsync => {
                if display.vsync {
                    "Enabled".into()
                } else {
                    "Disabled".into()
                }
            }
            Detail::Difficulty => format!(
                "Current Difficulty: {}",
                session.as_ref().map_or("Normal", |s| s.difficulty.name())
            ),
            Detail::Aspect => {
                if w * 3 == h * 4 {
                    "Normal 4:3".into()
                } else if w * 10 == h * 16 {
                    "Widescreen 16:10".into()
                } else {
                    "Widescreen 16:9".into()
                }
            }
            Detail::Resolution => format!("{w} x {h}"),
            Detail::Display => {
                if windows
                    .iter()
                    .next()
                    .is_some_and(|w| w.mode != bevy::window::WindowMode::Windowed)
                {
                    "Windowed (No Border)".into()
                } else {
                    "Windowed".into()
                }
            }
        };
        if text.0 != next {
            text.0 = next;
        }
    }
}

#[derive(Component)]
pub(crate) struct CampaignLabel(usize);
#[derive(Component)]
pub(crate) struct CampaignPreview;
#[derive(Component)]
pub(crate) struct Flyout(usize);
#[derive(Component)]
pub(crate) struct ChoiceRow(usize, usize);
#[derive(Component)]
pub(crate) struct ChoiceLabel(usize, usize);
#[derive(Component)]
pub(crate) struct LaunchError;
fn campaign_setup(
    commands: &mut Commands,
    root: Entity,
    font: &Handle<Font>,
    assets: &mut SourceAssets,
    images: &mut Assets<Image>,
) {
    // PC gamesettings.res: c-250/115..215, chapter picture c52/125,
    // frame c45/113; convert the native 480-line space to 720 lines.
    let panel = commands
        .spawn((
            Node {
                width: percent(100.),
                height: percent(100.),
                display: Display::None,
                ..default()
            },
            PageNode(Page::Campaign),
            ChildOf(root),
        ))
        .id();
    commands.spawn((
        band(155., 330.),
        BackgroundColor(Color::srgba(0., 0., 0., 0.78)),
        ChildOf(panel),
    ));
    commands.spawn((
        Text::new("PLAY CAMPAIGN"),
        TextFont {
            font: bevy::text::FontSource::Handle(font.clone()),
            font_size: FontSize::Vh((36.) / 7.2),
            ..default()
        },
        rect(265., 98., 600., 55.),
        CampaignTitle,
        ChildOf(panel),
    ));
    commands.spawn((
        ImageNode::default(),
        rect(718., 187.5, 300., 150.),
        CampaignPreview,
        ChildOf(panel),
    ));
    let frame = image(assets, images, "materials/vgui/campaignframe.vtf");
    commands.spawn((
        ImageNode::new(frame),
        rect(707.5, 169.5, 369., 184.5),
        ChildOf(panel),
    ));
    let arrow = image(assets, images, "materials/vgui/icon_arrow_down.vtf");
    for field in 0..4 {
        let y = 172.5 + field as f32 * 30.;
        let button = commands
            .spawn((
                Button,
                rect(265., y, 420., 29.),
                Action(ButtonAction::Select(field)),
                BackgroundColor(Color::NONE),
                ChildOf(panel),
            ))
            .id();
        commands.spawn((
            Text::new(""),
            TextFont {
                font: bevy::text::FontSource::Handle(font.clone()),
                font_size: FontSize::Vh((22.) / 7.2),
                ..default()
            },
            CampaignLabel(field),
            ChildOf(button),
        ));
        commands.spawn((
            ImageNode::new(arrow.clone()),
            rect(398., 3., 18., 18.),
            ChildOf(button),
        ));
        let list = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: vh((265.) / 7.2),
                    top: vh((y + 29.) / 7.2),
                    width: vh((420.) / 7.2),
                    flex_direction: FlexDirection::Column,
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(Color::srgb(0.08, 0.08, 0.08)),
                GlobalZIndex(120),
                Flyout(field),
                ChildOf(panel),
            ))
            .id();
        for index in 0..if field == 0 {
            14
        } else if field == 1 {
            8
        } else {
            4
        } {
            let row = commands
                .spawn((
                    Button,
                    Node {
                        width: percent(100.),
                        height: vh((28.) / 7.2),
                        min_height: vh((28.) / 7.2),
                        padding: UiRect::horizontal(vh((8.) / 7.2)),
                        ..default()
                    },
                    Action(ButtonAction::Choose(field, index)),
                    ChoiceRow(field, index),
                    BackgroundColor(Color::NONE),
                    ChildOf(list),
                ))
                .id();
            commands.spawn((
                Text::new(""),
                TextFont {
                    font: bevy::text::FontSource::Handle(font.clone()),
                    font_size: FontSize::Vh((21.) / 7.2),
                    ..default()
                },
                ChoiceLabel(field, index),
                ChildOf(row),
            ));
        }
    }
    // Retail gamesettings_coopcreate: settings first, then the lobby.
    button(
        commands,
        panel,
        "CREATE LOBBY",
        font,
        rect(265., 292.5, 420., 29.),
        ButtonAction::CreateLobby,
    );
    button(
        commands,
        panel,
        "‹ BACK",
        font,
        rect(265., 322.5, 420., 29.),
        ButtonAction::Back,
    );
    commands.spawn((
        Text::new(""),
        TextFont {
            font: bevy::text::FontSource::Handle(font.clone()),
            font_size: FontSize::Vh((19.) / 7.2),
            ..default()
        },
        TextColor(Color::srgb(1., 0.35, 0.3)),
        rect(265., 405., 760., 70.),
        LaunchError,
        ChildOf(panel),
    ));
}
fn choose_setting(
    sel: &mut crate::campaign_settings::Selection,
    catalog: &crate::campaign_settings::Catalog,
    field: usize,
    index: usize,
) {
    use crate::campaign_settings::{Difficulty, Survivor};
    match field {
        0 if index < catalog.campaigns.len() => {
            sel.campaign = index;
            sel.chapter = 0;
            let roster = Survivor::roster(catalog.campaigns[index].original);
            if !roster.contains(&sel.survivor) {
                sel.survivor = roster[0];
            }
        }
        1 if index < catalog.campaigns[sel.campaign].chapters.len() => sel.chapter = index,
        2 if index < 4 => sel.difficulty = Difficulty::ALL[index],
        3 if index < 4 => {
            sel.survivor = Survivor::roster(catalog.campaigns[sel.campaign].original)[index]
        }
        _ => return,
    }
    sel.open = None;
    sel.error.clear();
}
pub fn campaign_presentation(
    frontend: Res<Frontend>,
    sel: Res<crate::campaign_settings::Selection>,
    catalog: Res<crate::campaign_settings::Catalog>,
    art: Res<MenuArt>,
    mut text: Query<(
        &mut Text,
        Option<&CampaignLabel>,
        Option<&ChoiceLabel>,
        Option<&LaunchError>,
    )>,
    mut flyouts: Query<(&Flyout, &mut Node), Without<ChoiceRow>>,
    mut rows: Query<(&ChoiceRow, &mut Node), Without<Flyout>>,
    mut previews: Query<&mut ImageNode, With<CampaignPreview>>,
) {
    use crate::campaign_settings::{Difficulty, Survivor};
    if frontend.page != Page::Campaign {
        return;
    }
    let c = &catalog.campaigns[sel.campaign];
    let ch = &c.chapters[sel.chapter];
    let options = [
        catalog
            .campaigns
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>(),
        c.chapters.iter().map(|ch| ch.name.clone()).collect(),
        Difficulty::ALL.iter().map(|d| d.name().into()).collect(),
        Survivor::roster(c.original)
            .iter()
            .map(|s| s.name().into())
            .collect(),
    ];
    for (mut text, label, choice, error) in &mut text {
        let value = if let Some(label) = label {
            match label.0 {
                0 => format!("Campaign: {}", c.name),
                1 if crate::survival::active() => format!("Map: {}", ch.name),
                1 => format!("Chapter: {}", ch.name),
                2 => format!("Difficulty: {}", sel.difficulty.name()),
                _ => format!("Character: {}", sel.survivor.name()),
            }
        } else if let Some(choice) = choice {
            options[choice.0].get(choice.1).cloned().unwrap_or_default()
        } else if error.is_some() {
            sel.error.clone()
        } else {
            continue;
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    for (field, mut node) in &mut flyouts {
        let display = if sel.open == Some(field.0) {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    for (row, mut node) in &mut rows {
        let display = if row.1 < options[row.0].len() {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    if let Some(handle) = art.chapters.get(&ch.image) {
        for mut image in &mut previews {
            if image.image != *handle {
                image.image = handle.clone();
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phone_menu_swipes_wrap_and_ignore_taps_and_vertical_drags() {
        for height in [393., 720., 1179.] {
            assert_eq!(swipe_mode(0, Vec2::new(-height * 0.2, 0.), height), Some(1));
            assert_eq!(swipe_mode(0, Vec2::new(height * 0.2, 0.), height), Some(MODES.len()-1));
            assert_eq!(swipe_mode(MODES.len()-1, Vec2::new(-height * 0.2, 0.), height), Some(0));
            assert_eq!(swipe_mode(0, Vec2::new(2., 1.), height), None);
            assert_eq!(swipe_mode(0, Vec2::new(height * 0.1, height * 0.3), height), None);
        }
    }

    #[test]
    fn phone_menu_tracks_one_finger_and_ignores_cancelled_or_gameplay_touches() {
        use bevy::input::touch::{TouchInput, TouchPhase};
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::input::InputPlugin))
            .insert_resource(Frontend { page: Page::Home, mode: 0, restart: false, started: false })
            .init_resource::<game_audio::Queue>()
            .add_systems(Update, touch_carousel);
        let window = app.world_mut().spawn(Window::default()).id();
        app.update();
        let height = app.world().get::<Window>(window).unwrap().height();
        let send = |app: &mut App, phase, x| {
            app.world_mut().write_message(TouchInput {
                phase, position: Vec2::new(height * x, height * 0.2),
                window, force: None, id: 1,
            });
            app.update();
        };
        send(&mut app, TouchPhase::Started, 0.9);
        send(&mut app, TouchPhase::Moved, 0.4);
        send(&mut app, TouchPhase::Ended, 0.4);
        assert_eq!(app.world().resource::<Frontend>().mode, 1);
        send(&mut app, TouchPhase::Started, 0.9);
        send(&mut app, TouchPhase::Canceled, 0.4);
        assert_eq!(app.world().resource::<Frontend>().mode, 1);
        app.world_mut().resource_mut::<Frontend>().page = Page::Game;
        send(&mut app, TouchPhase::Started, 0.9);
        send(&mut app, TouchPhase::Ended, 0.4);
        assert_eq!(app.world().resource::<Frontend>().mode, 1);
    }
    #[test]
    fn phone_menu_taps_activate_on_release_and_swipes_do_not_open_campaign() {
        use bevy::input::touch::{TouchInput, TouchPhase};
        let mut app = app();
        app.world_mut().resource_mut::<Frontend>().page = Page::Home;
        app.add_plugins(bevy::input::InputPlugin)
            .add_systems(Update, touch_carousel.before(update));
        let window = app.world_mut().spawn(Window::default()).id();
        let button = app.world_mut().spawn((Interaction::None,
            Action(ButtonAction::Options), BackgroundColor(Color::NONE))).id();
        app.update();
        let send = |app: &mut App, phase, x| {
            app.world_mut().write_message(TouchInput {
                phase, position: Vec2::new(x, 150.), window, force: None, id: 1,
            });
            app.update();
        };
        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Pressed;
        send(&mut app, TouchPhase::Started, 400.);
        assert_eq!(app.world().resource::<Frontend>().page as u8, Page::Home as u8);
        send(&mut app, TouchPhase::Ended, 402.);
        assert_eq!(app.world().resource::<Frontend>().page as u8, Page::Options as u8);
        app.world_mut().resource_mut::<Frontend>().page = Page::Home;
        app.world_mut().get_mut::<Action>(button).unwrap().0 = ButtonAction::Campaign;
        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::None;
        app.update();
        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Pressed;
        send(&mut app, TouchPhase::Started, 400.);
        send(&mut app, TouchPhase::Moved, 200.);
        send(&mut app, TouchPhase::Ended, 200.);
        assert_eq!(app.world().resource::<Frontend>().page as u8, Page::Home as u8);
        assert_eq!(app.world().resource::<Frontend>().mode, 1);
    }

    #[test]
    fn campaign_change_resets_chapter_and_uses_matching_survivors() {
        use crate::campaign_settings::*;
        let campaign = |original| crate::campaign_settings::Campaign {
            name: "Campaign".into(),
            chapters: vec![Chapter {
                name: "One".into(),
                map: "one.bsp".into(),
                image: String::new(),
            }],
            original,
        };
        let catalog = Catalog {
            campaigns: vec![campaign(false), campaign(true)],
        };
        let mut s = Selection {
            campaign: 0,
            chapter: 3,
            survivor: Survivor::Coach,
            difficulty: Difficulty::Expert,
            open: Some(0),
            error: String::new(),
        };
        choose_setting(&mut s, &catalog, 0, 1);
        assert_eq!((s.campaign, s.chapter), (1, 0));
        assert_eq!(s.survivor, Survivor::Bill);
        assert_eq!(s.difficulty, Difficulty::Expert);
        assert_eq!(s.open, None);
        choose_setting(&mut s, &catalog, 3, 1);
        assert_eq!(s.survivor, Survivor::Zoey);
    }
    fn app() -> App {
        let mut app = App::new();
        let weapon = crate::game_state::Weapon {
            penetration: Default::default(),
            pellets: 1,
            range_modifier: 1.,
            scatter: Vec2::ZERO,
            accuracy: Default::default(),
            muzzle_effect: String::new(),
            class: "weapon_pistol".into(),
            clip: 15,
            damage: 36.,
            cycle: 0.175,
            range: 63.5,
            speed: 6.35,
            punch: 2.,
            model: String::new(),
            view_model: String::new(),
            reload_times: [1.7, 0.5, 0.5],
            reload_empty_time: 0.,
            damage_flags: 2,
            decapitates: false,
            player_model: String::new(),
            muzzle_forward: [0., 0., 1.],
            fire_sound: "Pistol.Fire".into(),
            melee_hit_sound: String::new(),
            melee_world_sound: String::new(),
            melee_attacks: vec![],
            icon: String::new(),
            icon_rect: Rect::default(),
        };
        app.insert_resource(Frontend {
            page: Page::Campaign,
            mode: 0,
            restart: false,
            started: false,
        })
        .insert_resource(Match::new(&weapon))
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<game_audio::Queue>()
        .init_resource::<game_audio::Settings>()
        .insert_resource(crate::performance::DisplaySettings {
            show_fps: false,
            vsync: false,
            viewmodel_fov: 51.,
            camera_fov: 90.,
        })
        .add_message::<AppExit>()
        .add_systems(Update, update);
        app
    }
    #[test]
    fn hybrid_buttons_glow_when_focused_and_paint_red_lines_when_open() {
        let mut app = App::new();
        app.insert_resource(Frontend { page: Page::Home, mode: 0, restart: false, started: false })
            .init_resource::<Time>()
            .add_systems(Update, hybrid_paint);
        let text = app.world_mut().spawn((TextColor(Color::WHITE), ComputedNode::default())).id();
        let glow = app.world_mut().spawn((TextColor(Color::NONE), ComputedNode::default(), Glow)).id();
        let line = app.world_mut().spawn((Node { display: Display::None, ..default() }, OpenPart)).id();
        let button = app
            .world_mut()
            .spawn((Interaction::None, Action(ButtonAction::Options), Hybrid::Main))
            .add_children(&[text, glow, line])
            .id();
        app.update();
        assert_eq!(app.world().get::<TextColor>(text).unwrap().0, Color::srgb_u8(125, 125, 125));
        assert_eq!(app.world().get::<TextColor>(glow).unwrap().0.alpha(), 0.);
        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Hovered;
        app.update();
        assert_eq!(app.world().get::<TextColor>(text).unwrap().0, Color::srgb_u8(255, 255, 255));
        // sin(4t) x 30 + 60 at t = 0.
        let alpha = app.world().get::<TextColor>(glow).unwrap().0.alpha();
        assert!((alpha - 60. / 255. * GLOW_SHARE).abs() < 1e-4);
        assert_eq!(app.world().get::<Node>(line).unwrap().display, Display::None);
        app.world_mut().resource_mut::<Frontend>().page = Page::Options;
        app.update();
        assert_eq!(app.world().get::<TextColor>(text).unwrap().0, Color::srgb_u8(200, 200, 200));
        assert_eq!(app.world().get::<Node>(line).unwrap().display, Display::Flex);
    }
    #[test]
    fn hovered_mode_pictures_get_no_red_fill() {
        // A hovered menu button gets the dark-red fill; the framed mode
        // picture must not (it showed as a red fringe around the tile).
        let mut app = app();
        let tile = app
            .world_mut()
            .spawn((Interaction::Hovered, Action(ButtonAction::Campaign), BackgroundColor(Color::NONE), CarouselTile(0)))
            .id();
        let plain = app
            .world_mut()
            .spawn((Interaction::Hovered, Action(ButtonAction::Previous), BackgroundColor(Color::NONE)))
            .id();
        app.update();
        assert_eq!(app.world().get::<BackgroundColor>(tile).unwrap().0, Color::NONE);
        assert_ne!(app.world().get::<BackgroundColor>(plain).unwrap().0, Color::NONE);
    }
    #[test]
    fn start_exits_menu_requests_reset_and_stops_music() {
        let mut app = app();
        let music = app.world_mut().spawn(game_audio::MenuMusic).id();
        app.world_mut().spawn((
            Interaction::Pressed,
            Action(ButtonAction::Start),
            BackgroundColor(Color::NONE),
        ));
        app.update();
        let frontend = app.world().resource::<Frontend>();
        assert!(frontend.playing() && frontend.restart && frontend.started);
        assert!(app.world().get_entity(music).is_err());
        assert!(
            app.world()
                .resource::<game_audio::Queue>()
                .0
                .iter()
                .any(|s| s.name == "Menu.StartGame")
        );
    }
    #[test]
    fn audio_setting_is_bounded_and_escape_pauses_game() {
        let mut app = app();
        app.world_mut().resource_mut::<Frontend>().page = Page::Game;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.world_mut().spawn((
            Interaction::Pressed,
            Action(ButtonAction::Volume(5.)),
            BackgroundColor(Color::NONE),
        ));
        app.update();
        assert!(app.world().resource::<Frontend>().page == Page::Pause);
        assert_eq!(app.world().resource::<game_audio::Settings>().volume, 1.);
        assert!(app.world().resource::<Match>().paused);
    }
    #[test]
    fn carousel_advances_every_tile_and_keeps_the_leading_tile_selected() {
        let mut app = app();
        app.init_resource::<Time>().init_resource::<UiScale>();
        let mut images = Assets::<Image>::default();
        let modes = (0..8)
            .map(|_| images.add(Image::default()))
            .collect::<Vec<_>>();
        app.insert_resource(MenuArt {
            modes: modes.clone(),
            chapters: Default::default(),
        });
        app.add_systems(Update, presentation.after(update));
        let mut tiles = vec![];
        for i in 0..2 {
            let tile = app
                .world_mut()
                .spawn((CarouselTile(i), ImageNode::default(), Node::default()))
                .id();
            let label = app.world_mut().spawn((Text::new(""), ChildOf(tile))).id();
            tiles.push((tile, label));
        }
        let next = app
            .world_mut()
            .spawn((
                Interaction::Pressed,
                Action(ButtonAction::Next),
                BackgroundColor(Color::NONE),
            ))
            .id();
        app.update();
        assert_eq!(app.world().resource::<Frontend>().mode, 1);
        assert_eq!(app.world().get::<Text>(tiles[0].1).unwrap().0, "MUTATION");
        assert_eq!(app.world().get::<Text>(tiles[1].1).unwrap().0, "REALISM");
        assert_eq!(
            app.world().get::<ImageNode>(tiles[0].0).unwrap().image,
            modes[1]
        );
        app.world_mut().despawn(next);
        app.world_mut().spawn((
            Interaction::Pressed,
            Action(ButtonAction::Mode(2)),
            BackgroundColor(Color::NONE),
        ));
        app.update();
        assert_eq!(app.world().resource::<Frontend>().mode, 3);
        assert_eq!(app.world().get::<Text>(tiles[0].1).unwrap().0, "VERSUS");
    }
    #[test]
    fn options_back_preserves_the_paused_session() {
        let mut app = app();
        {
            let mut frontend = app.world_mut().resource_mut::<Frontend>();
            frontend.page = Page::Options;
            frontend.started = true;
        }
        app.world_mut().spawn((
            Interaction::Pressed,
            Action(ButtonAction::Back),
            BackgroundColor(Color::NONE),
        ));
        app.update();
        let frontend = app.world().resource::<Frontend>();
        assert!(frontend.page == Page::Pause && frontend.started && !frontend.restart);
    }
    #[test]
    fn difficulty_change_preserves_campaign_progress() {
        let mut app = app();
        app.insert_resource(crate::campaign_settings::SessionSettings {
            map: "hotel.bsp".into(),
            survivor: crate::campaign_settings::Survivor::Ellis,
            difficulty: crate::campaign_settings::Difficulty::Normal,
            original: false,
        });
        app.insert_resource(crate::source_director::Director::default());
        {
            let mut frontend = app.world_mut().resource_mut::<Frontend>();
            frontend.page = Page::Difficulty;
            frontend.started = true;
        }
        {
            let mut state = app.world_mut().resource_mut::<Match>();
            state.health = 34.;
            state.elapsed = 18.;
        }
        app.world_mut().spawn((
            Interaction::Pressed,
            Action(ButtonAction::SetDifficulty(3)),
            BackgroundColor(Color::NONE),
        ));
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::campaign_settings::SessionSettings>()
                .difficulty,
            crate::campaign_settings::Difficulty::Expert
        );
        let state = app.world().resource::<Match>();
        assert_eq!((state.health, state.elapsed), (34., 18.));
        let frontend = app.world().resource::<Frontend>();
        assert!(frontend.page == Page::Pause && !frontend.restart);
    }
    #[test]
    fn advanced_settings_done_returns_to_video() {
        let mut app = app();
        app.world_mut().resource_mut::<Frontend>().page = Page::Advanced;
        app.world_mut().spawn((
            Interaction::Pressed,
            Action(ButtonAction::Back),
            BackgroundColor(Color::NONE),
        ));
        app.update();
        assert!(app.world().resource::<Frontend>().page == Page::Video);
    }
}
