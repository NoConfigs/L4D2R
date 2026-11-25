//! Live survivor HUD, using locally mounted L4D2 VGUI images.
use crate::{
    game_state::{Match, Rules, Squad},
    source_assets::SourceAssets,
    vtf,
};
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::{collections::HashMap, path::PathBuf};
#[derive(Resource)]
pub struct HudSource(pub PathBuf);
#[derive(Component)]
pub struct HudRoot;
#[derive(Component)]
pub(crate) struct SurvivorHud;
#[derive(Component)]
pub enum Label {
    Health,
    Ammo,
    Reserve,
    SlotAmmo(u8),
    Status,
    Progress,
    Team(usize),
    Instructor,
    HintBinding,
    Fuel,
}
#[derive(Component)]
pub struct HintIcon;
#[derive(Resource, Default)]
pub struct NativeHints {
    strings: HashMap<String, String>,
    icons: HashMap<String, Rect>,
}
impl NativeHints {
    fn binding(command: &str) -> &str {
        match command {
            "+use" => "E",
            "+reload" => "R",
            "+attack" => "MOUSE1",
            "+attack2" => "MOUSE2",
            "+jump" => "SPACE",
            "+duck" => "CTRL",
            _ => command,
        }
    }
    fn load(assets: &mut SourceAssets) -> Self {
        let mut result = Self::default();
        if let Ok(bytes) = assets.read("resource/left4dead2_english.txt") {
            let text = if bytes.starts_with(&[255, 254]) {
                String::from_utf16_lossy(
                    &bytes[2..]
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect::<Vec<_>>(),
                )
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            };
            let tokens = crate::kv::tokens(&text);
            for pair in tokens.windows(2) {
                if pair[0].starts_with("L4D_") {
                    result
                        .strings
                        .insert(pair[0].to_lowercase(), pair[1].clone());
                }
            }
        }
        if let Ok(bytes) = assets.read("scripts/mod_textures.txt") {
            let tokens = crate::kv::tokens(&String::from_utf8_lossy(&bytes));
            for i in 0..tokens.len().saturating_sub(2) {
                if !tokens[i].starts_with("icon_") || tokens[i + 1] != "{" {
                    continue;
                }
                let end = tokens[i + 2..]
                    .iter()
                    .position(|s| s == "}")
                    .map(|n| n + i + 2)
                    .unwrap_or(tokens.len());
                let block = &tokens[i + 2..end];
                if !block
                    .windows(2)
                    .any(|p| p[0] == "file" && p[1] == "vgui/hud/iconsheet")
                {
                    continue;
                }
                let number = |key: &str| {
                    block
                        .windows(2)
                        .find(|p| p[0] == key)
                        .and_then(|p| p[1].parse::<f32>().ok())
                        .unwrap_or(0.)
                };
                let origin = Vec2::new(number("x"), number("y"));
                result.icons.insert(
                    tokens[i].to_lowercase(),
                    Rect::from_corners(
                        origin,
                        origin + Vec2::new(number("width"), number("height")),
                    ),
                );
            }
        }
        result
    }
    pub fn caption(&self, token: &str) -> String {
        let key = token.trim_start_matches('#').to_lowercase();
        let mut text = self
            .strings
            .get(&key)
            .cloned()
            .unwrap_or_else(|| token.into());
        for (command, key) in [
            ("+use", "E"),
            ("+reload", "R"),
            ("+attack", "MOUSE1"),
            ("+attack2", "MOUSE2"),
            ("+jump", "SPACE"),
            ("+duck", "CTRL"),
        ] {
            text = text.replace(&format!("%{command}%"), key);
        }
        text.replace("\\n", "\n")
    }
}
#[derive(Component)]
/// Health bar segment of the player (`None`) or a teammate. The second
/// field marks the temporary-health (pills/adrenaline buffer) segment.
pub struct HealthFill(pub Option<usize>, pub bool);
#[derive(Component)]
pub struct DamageOverlay;
#[derive(Component)]
pub struct InventoryIcon(pub u8);
#[derive(Component)]
pub struct PrimaryIcon(pub u8);
#[derive(Component)]
pub struct WeaponPanel;
#[derive(Component)]
pub(crate) struct InventoryFrame(u8);
fn weapon_rows(s: &Match) -> Vec<u8> {
    if s.primary.is_some() { vec![2] } else { vec![] }
}
fn displayed_gun(s: &Match) -> &crate::game_state::Gun {
    s.primary.as_ref().unwrap_or(&s.pistol)
}
fn selected_color(selected: bool) -> Color {
    if selected {
        Color::srgb(142. / 255., 214. / 255., 57. / 255.)
    } else {
        Color::WHITE
    }
}
fn fit_icon(rect: Rect, width: f32, height: f32) -> (f32, f32) {
    let size = rect.size();
    let scale = (width / size.x.max(1.)).min(height / size.y.max(1.));
    (size.x * scale, size.y * scale)
}
#[derive(Component)]
pub struct Crosshair;
/// A teammate panel element whose look follows that survivor's state
/// (client C_TeammatePanel, FUN_102c9a70): incapacitated survivors switch to
/// TeammatePanel_Incap.res (the s_panel_<name>_incap artwork replaces the
/// background, the portrait hides, the bar and name move into its box); dead
/// survivors show hud/overlay_dead, the portrait tinted 255,0,0,200 and no
/// health bar, exactly as the local panel does.
#[derive(Component)]
pub(crate) struct TeamPart {
    pub index: usize,
    pub kind: TeamPartKind,
}
pub(crate) enum TeamPartKind {
    Background,
    IncapBackground,
    Head,
    DeadOverlay,
    /// Normal and incapacitated layouts.
    Moves(Node, Node),
    /// Hidden while dead.
    Bar(Node, Node),
}
/// The death banner's "The Survivors have been overwhelmed" line (online it
/// shows only once everyone is down; net_flow).
#[derive(Component)]
pub(crate) struct OverwhelmedText;
/// A teammate panel's name (the survivor; a remote player's own name in
/// multiplayer, set by net_play).
#[derive(Component)]
pub(crate) struct TeamName(pub usize);
/// A local-player panel element that switches to LocalPlayerPanel_Incap.res
/// while the player is incapacitated: the s_panel_<name>_incap artwork
/// replaces the background, the portrait hides, and the health bar, icon
/// and number move into the artwork's box.
#[derive(Component)]
pub(crate) struct LocalPart(pub TeamPartKind);
/// Local HealthIcon/HealthNumber, tinted by `health_colour`.
#[derive(Component)]
pub(crate) struct HealthTint;
/// Survivor health colour (client FUN_1026ec00 tier -> FUN_1026ebd0 table
/// 106e7efc), applied by the survivor panel (102c9a70) to HealthIcon and
/// HealthNumber: incapacitated -> tier 0; permanent health / max below 0.25
/// -> 1; below survivor_limp_health / 100 (0.40) -> 2; otherwise 3.
pub(crate) fn health_colour(state: &Match) -> Color {
    let ratio = (state.health / 100.).clamp(0., 1.);
    let [r, g, b] = if state.incap || ratio < 0.25 {
        [161, 25, 25]
    } else if ratio < 0.40 {
        [216, 146, 12]
    } else {
        [10, 177, 50]
    };
    Color::srgb_u8(r, g, b)
}
pub fn health_tint(s: Res<Match>, mut texts: Query<&mut TextColor, With<HealthTint>>) {
    let colour = TextColor(health_colour(&s));
    for mut text in &mut texts {
        text.set_if_neq(colour);
    }
}
/// A teammate health panel; its retail `left` in vh. The panels move
/// together by `Interface::team_offset` (TOUCH & HUD page).
#[derive(Component)]
pub(crate) struct TeamPanel(pub f32);
pub(crate) fn place_team_panels(
    interface: Option<Res<crate::game::ui_fit::Interface>>,
    mut panels: Query<(&TeamPanel, &mut Node)>,
) {
    let offset = interface.map_or(Vec2::ZERO, |i| i.team_offset) * 100.;
    for (panel, mut node) in &mut panels {
        let (left, bottom) = (vh(panel.0 + offset.x), vh(-5.5 / 4.8 - offset.y));
        if node.left != left || node.bottom != bottom {
            node.left = left;
            node.bottom = bottom;
        }
    }
}
#[derive(Component)]
pub(crate) enum DeathPart {
    /// Full-window layer: reaches past the HUD's safe-area inset so the
    /// black bar spans the whole screen on notched phones.
    Banner,
    /// Inside the banner layer: the safe area again, for its text.
    BannerContent,
    Overlay,
    HealthText,
    Portrait,
    Equipment,
}
#[derive(Resource)]
pub struct HudImages(pub HashMap<String, Handle<Image>>);
#[derive(Resource)]
pub struct HudFonts {
    _native: Handle<Font>,
    _fallback: Handle<Font>,
}
// Layout adapted from l4d2r's hud_l4d.rs; VGUI uses a 480-unit vertical canvas.
fn hud_rect(x: f32, y: f32, w: f32, h: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: vh(x / 4.8),
        top: vh(y / 4.8),
        width: vh(w / 4.8),
        height: vh(h / 4.8),
        ..default()
    }
}
// Native teammate backgrounds are 256x128, drawn at (-2,0), size 144x68.
// Keep portrait and bar bounds in that same texture coordinate system.
fn team_frame_rect(x: f32, y: f32, w: f32, h: f32) -> Node {
    hud_rect(
        -2. + x * 144. / 256.,
        y * 68. / 128.,
        w * 144. / 256.,
        h * 68. / 128.,
    )
}
fn team_panels(
    session: &crate::campaign_settings::SessionSettings,
) -> Vec<(usize, crate::campaign_settings::Survivor)> {
    use crate::campaign_settings::Survivor;
    let mut panels = Survivor::roster(session.original)
        .into_iter()
        .filter(|s| *s != session.survivor)
        .enumerate()
        .collect::<Vec<_>>();
    if !session.original {
        panels.sort_by_key(|(_, survivor)| match survivor {
            Survivor::Rochelle => 0,
            Survivor::Nick => 1,
            Survivor::Coach => 2,
            _ => 3,
        });
    }
    panels
}
/// One health-bar segment: the bar span (0..1) and its texture.
pub(crate) struct HealthSegment {
    pub span: (f32, f32),
    pub texture: &'static str,
}
/// Retail HealthPanel (new_material_style, client FUN_10296770): permanent
/// health draws the solid bar over 0..health; the pills/adrenaline buffer
/// draws the striped `_ticks` bar right after it, health..health+buffer.
/// Both take their colour from permanent health alone (FUN_1026ec00: red
/// below 25%, orange below survivor_limp_health 40%, else green); a
/// black-and-white survivor uses the white bars. Incapacitated: FUN_1026ec00
/// returns tier 0 and the incap branch draws the ticks set (+400), whose
/// slot 0 is "healthbar_image_low_ticks" (FUN_10296050): the striped red bar
/// over the bleed-out health (300 max).
pub(crate) fn health_segment(state: &Match, temporary: bool) -> HealthSegment {
    if state.dead {
        return HealthSegment {
            span: (0., 0.),
            texture: "hud/healthbar_withglow_red",
        };
    }
    if state.incap {
        let f = (state.incap_health / 300.).clamp(0., 1.);
        return HealthSegment {
            span: if temporary { (f, f) } else { (0., f) },
            texture: "hud/healthbar_ticks_withglow_red",
        };
    }
    let permanent = (state.health / 100.).clamp(0., 1.);
    let buffer = (state.temporary / 100.).clamp(0., 1. - permanent);
    let tier = if state.black_white {
        "white"
    } else if permanent < 0.25 {
        "red"
    } else if permanent < 0.4 {
        "orange"
    } else {
        "green"
    };
    if temporary {
        HealthSegment {
            span: (permanent, permanent + buffer),
            texture: match tier {
                "white" => "hud/healthbar_ticks_withglow_white",
                "red" => "hud/healthbar_ticks_withglow_red",
                "orange" => "hud/healthbar_ticks_withglow_orange",
                _ => "hud/healthbar_ticks_withglow_green",
            },
        }
    } else {
        HealthSegment {
            span: (0., permanent),
            texture: match tier {
                "white" => "hud/healthbar_withglow_white",
                "red" => "hud/healthbar_withglow_red",
                "orange" => "hud/healthbar_withglow_orange",
                _ => "hud/healthbar_withglow_green",
            },
        }
    }
}
fn hud_font(handle: &Handle<Font>, size: f32) -> TextFont {
    TextFont {
        font: bevy::text::FontSource::Handle(handle.clone()),
        font_size: FontSize::Vh(size / 4.8),
        ..default()
    }
}
fn ammo_font_size(digits: usize) -> f32 {
    match digits {
        0..=2 => 16.,
        3 => 12.,
        _ => 9.,
    }
}
pub fn setup(
    mut commands: Commands,
    source: Res<HudSource>,
    mut images: ResMut<Assets<Image>>,
    rules: Res<Rules>,
    session: Res<crate::campaign_settings::SessionSettings>,
    mut fonts: ResMut<Assets<Font>>,
) {
    let fallback = std::fs::read("C:/Windows/Fonts/arialbd.ttf")
        .ok()
        .map(|b| crate::game_ui::shared_font(&mut *fonts, b))
        .unwrap_or_default();
    let mut assets = SourceAssets::new(&source.0, vec![]).expect("Mounted HUD assets");
    let mut hints = NativeHints::load(&mut assets);
    let mut load_font = |name: &str| {
        assets
            .read(&format!("resource/{name}.vfont"))
            .and_then(|b| crate::vfont::decode(&b))
            .map(|b| crate::game_ui::shared_font(&mut *fonts, b))
            .unwrap_or_else(|_| fallback.clone())
    };
    let native = load_font("stubble-bold");
    let toolbox = load_font("toolbox");
    let mut handles = HashMap::new();
    for path in [
        "s_panel_coach",
        "s_panel_gambler",
        "s_panel_producer",
        "s_panel_mechanic",
        "s_panel_namvet",
        "s_panel_teenangst",
        "s_panel_biker",
        "s_panel_manager",
        "s_panel_coach_incap",
        "s_panel_gambler_incap",
        "s_panel_producer_incap",
        "s_panel_mechanic_incap",
        "s_panel_namvet_incap",
        "s_panel_teenangst_incap",
        "s_panel_biker_incap",
        "s_panel_manager_incap",
        "hud/healthbar_bg_1",
        "hud/healthbar_bg_2",
        "hud/healthbar_bg_3",
        "hud/healthbar_bg_player",
        "hud/healthbar_withglow_green",
        "hud/healthbar_withglow_orange",
        "hud/healthbar_withglow_red",
        "hud/healthbar_withglow_white",
        "hud/healthbar_ticks_withglow_green",
        "hud/healthbar_ticks_withglow_orange",
        "hud/healthbar_ticks_withglow_red",
        "hud/healthbar_ticks_withglow_white",
        "hud/scalablepanel_bgmidgrey_outlinegreen_glow",
        "hud/scalablepanel_bgblack50_outlinewhite",
        "hud/scalablepanel_bgblack_outlinegrey",
        "hud/icon_first_aid_kit",
        "hud/icon_painpills",
        "hud/icon_molotov",
        "hud/overlay_dead",
        "hud/iconsheet",
        "hud/progressbar_bg",
        "hud/progressbar_fill",
    ] {
        match assets
            .read(&format!("materials/vgui/{path}.vtf"))
            .and_then(|b| vtf::decode(&b, 1024))
        {
            Ok(t) => {
                handles.insert(
                    path.to_string(),
                    images.add(Image::new(
                        Extent3d {
                            width: t.width,
                            height: t.height,
                            depth_or_array_layers: 1,
                        },
                        TextureDimension::D2,
                        t.rgba,
                        TextureFormat::Rgba8UnormSrgb,
                        RenderAssetUsages::RENDER_WORLD,
                    )),
                );
            }
            Err(e) => eprintln!("HUD {path}: {e}"),
        }
    }
    for weapon in rules.weapons.values() {
        if let Ok(t) = assets
            .read(&weapon.icon)
            .and_then(|b| vtf::decode(&b, 1024))
        {
            handles.insert(
                weapon.class.clone(),
                images.add(Image::new(
                    Extent3d {
                        width: t.width,
                        height: t.height,
                        depth_or_array_layers: 1,
                    },
                    TextureDimension::D2,
                    t.rgba,
                    TextureFormat::Rgba8UnormSrgb,
                    RenderAssetUsages::RENDER_WORLD,
                )),
            );
        }
        if let Ok(bytes) = assets.read(&format!("scripts/{}.txt", weapon.class)) {
            let tokens = crate::kv::tokens(&String::from_utf8_lossy(&bytes));
            if let Some(at) = tokens
                .windows(2)
                .position(|p| p[0] == "ammo" && p[1] == "{")
            {
                let end = tokens[at + 2..]
                    .iter()
                    .position(|t| t == "}")
                    .map_or(tokens.len(), |i| at + 2 + i);
                let fields = &tokens[at + 2..end];
                let get = |key: &str| {
                    fields
                        .windows(2)
                        .find(|p| p[0] == key)
                        .map(|p| p[1].as_str())
                };
                let number = |key, default| {
                    get(key)
                        .and_then(|s| s.parse::<f32>().ok())
                        .unwrap_or(default)
                };
                if let Some(file) = get("file") {
                    if let Ok(t) = assets
                        .read(&format!("materials/{file}.vtf"))
                        .and_then(|b| vtf::decode(&b, 1024))
                    {
                        let key = format!("{}::ammo", weapon.class);
                        let origin = Vec2::new(number("x", 0.), number("y", 0.));
                        hints.icons.insert(
                            key.clone(),
                            Rect::from_corners(
                                origin,
                                origin + Vec2::new(number("width", 64.), number("height", 64.)),
                            ),
                        );
                        handles.insert(
                            key,
                            images.add(Image::new(
                                Extent3d {
                                    width: t.width,
                                    height: t.height,
                                    depth_or_array_layers: 1,
                                },
                                TextureDimension::D2,
                                t.rgba,
                                TextureFormat::Rgba8UnormSrgb,
                                RenderAssetUsages::RENDER_WORLD,
                            )),
                        );
                    }
                }
            }
        }
    }
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: percent(100.),
            height: percent(100.),
            ..default()
        },
        BackgroundColor(Color::NONE),
        DamageOverlay,
    ));
    // Use the native translucent panel artwork, rather than a solid rectangle.
    for (position, (index, survivor)) in team_panels(&session).into_iter().enumerate() {
        let name = survivor.name();
        let portrait = format!("s_panel_{}", survivor.character());
        let panel = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: vh((4. + 138. * position as f32) / 4.8),
                    bottom: vh(-5.5 / 4.8),
                    width: vh(144. / 4.8),
                    height: vh(68. / 4.8),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                SurvivorHud,
                TeamPanel((4. + 138. * position as f32) / 4.8),
            ))
            .id();
        if let Some(h) = handles.get(&format!("hud/healthbar_bg_{}", position + 1)) {
            commands.spawn((
                ImageNode {
                    color: Color::srgb(80. / 255., 76. / 255., 82. / 255.),
                    // VGUI stretches this 256x128 art into a 144x68 slot.
                    // Auto letterboxes it, invalidating the bar's inner bounds.
                    image_mode: bevy::ui::widget::NodeImageMode::Stretch,
                    ..ImageNode::new(h.clone())
                },
                hud_rect(-2., 0., 144., 68.),
                TeamPart { index, kind: TeamPartKind::Background },
                ChildOf(panel),
            ));
        }
        // TeammatePanel_Incap.res, offset into this panel's space (retail
        // BackgroundImage -3,9 sits at -2,0 here: +1, -9).
        if let Some(h) = handles.get(&format!("s_panel_{}_incap", survivor.character())) {
            commands.spawn((
                ImageNode {
                    image_mode: bevy::ui::widget::NodeImageMode::Stretch,
                    ..ImageNode::new(h.clone())
                },
                Node { display: Display::None, ..hud_rect(-1., -7., 160., 80.) },
                TeamPart { index, kind: TeamPartKind::IncapBackground },
                ChildOf(panel),
            ));
        }
        if let Some(h) = handles.get(&portrait) {
            commands.spawn((
                ImageNode::new(h.clone()),
                // Registered against the native portrait pixels: 78x78 at
                // 1080p, leaving the background's frame visible on all sides.
                hud_rect(3.5, 6., 34.666668, 34.666668),
                TeamPart { index, kind: TeamPartKind::Head },
                ChildOf(panel),
            ));
        }
        commands.spawn((
            Text::new(name),
            hud_font(&native, 11.5),
            TextColor(Color::srgb(193. / 255., 193. / 255., 193. / 255.)),
            hud_rect(13.5, 44., 123., 18.),
            TeamPart { index, kind: TeamPartKind::Moves(hud_rect(13.5, 44., 123., 18.), hud_rect(12., 47., 123., 18.)) },
            TeamName(index),
            ChildOf(panel),
        ));
        commands.spawn((
            Text::new(""),
            hud_font(&native, 11.),
            hud_rect(42., 8., 90., 17.),
            Label::Team(index),
            ChildOf(panel),
        ));
        for (slot, path, x) in [
            (6 + index as u8, "hud/icon_first_aid_kit", 91.5),
            (9 + index as u8, "hud/icon_painpills", 119.),
        ] {
            commands.spawn((
                ImageNode {
                    rect: Some(if slot < 9 {
                        Rect::from_corners(Vec2::new(64., 0.), Vec2::new(128., 64.))
                    } else {
                        Rect::from_corners(Vec2::ZERO, Vec2::splat(64.))
                    }),
                    ..ImageNode::new(handles[path].clone())
                },
                if slot < 9 {
                    hud_rect(x, 5.5, 28., 28.)
                } else {
                    hud_rect(x, 12.25, 14., 14.)
                },
                PrimaryIcon(slot),
                ChildOf(panel),
            ));
        }
        let bar = commands
            .spawn((
                Node {
                    overflow: Overflow::clip(),
                    // All three frame variants share this interior. Full
                    // health reaches the inner right/bottom border.
                    ..team_frame_rect(76., 55., 167., 23.)
                },
                TeamPart {
                    index,
                    kind: TeamPartKind::Bar(
                        Node { overflow: Overflow::clip(), ..team_frame_rect(76., 55., 167., 23.) },
                        // Retail Health 11,42,121x12 inside the incap box.
                        Node { overflow: Overflow::clip(), ..hud_rect(12., 33., 121., 12.) },
                    ),
                },
                ChildOf(panel),
            ))
            .id();
        for temporary in [false, true] {
            commands.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0.),
                    // The texture includes a glow tail. Offset its gradient, but
                    // clip at the frame interior so it cannot paint over the rim.
                    top: vh((2.25 * 68. / 128.) / 4.8),
                    min_width: px(0.),
                    min_height: px(0.),
                    width: percent(100.),
                    height: percent(100.),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                ImageNode::new(handles["hud/healthbar_withglow_green"].clone())
                    .with_mode(bevy::ui::widget::NodeImageMode::Stretch),
                HealthFill(Some(index), temporary),
                ChildOf(bar),
            ));
        }
        // TeammatePanel.res "Dead": overlay_dead at 0,16 256x128 (+1,-9 here).
        commands.spawn((
            ImageNode {
                color: Color::NONE,
                ..ImageNode::new(handles["hud/overlay_dead"].clone())
                    .with_mode(bevy::ui::widget::NodeImageMode::Stretch)
            },
            hud_rect(1., 7., 256., 128.),
            TeamPart { index, kind: TeamPartKind::DeadOverlay },
            ChildOf(panel),
        ));
    }
    kill_feed::spawn(
        &mut commands,
        handles.get("hud/scalablepanel_bgblack_outlinegrey").cloned(),
        &native,
    );
    if let Some(name) = kill_feed::persona(&source.0) {
        commands.insert_resource(kill_feed::PlayerName(name));
    }
    let player = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: vh(0.),
                bottom: vh(12. / 4.8),
                width: vh(160. / 4.8),
                height: vh(80. / 4.8),
                ..default()
            },
            BackgroundColor(Color::NONE),
            SurvivorHud,
        ))
        .id();
    commands.spawn((
        ImageNode {
            color: Color::srgb(80. / 255., 76. / 255., 82. / 255.),
            ..ImageNode::new(handles["hud/healthbar_bg_player"].clone())
        },
        // The 512px artwork includes transparent padding beyond its 364px frame.
        // Its painted slot aligns with x=44..155 only at this native aspect ratio.
        hud_rect(0., 25., 220., 55.),
        LocalPart(TeamPartKind::Background),
        ChildOf(player),
    ));
    // LocalPlayerPanel_Incap.res BackgroundImage: -2,16 150x75.
    if let Some(h) = handles.get(&format!("s_panel_{}_incap", session.survivor.character())) {
        commands.spawn((
            ImageNode::new(h.clone()).with_mode(bevy::ui::widget::NodeImageMode::Stretch),
            Node { display: Display::None, ..hud_rect(-2., 16., 150., 75.) },
            LocalPart(TeamPartKind::IncapBackground),
            ChildOf(player),
        ));
    }
    commands.spawn((
        ImageNode::new(handles[&format!("s_panel_{}", session.survivor.character())].clone()),
        hud_rect(7., 36., 36., 35.),
        ChildOf(player),
        DeathPart::Portrait,
        LocalPart(TeamPartKind::Head),
    ));
    // Icon/number keep the normal layout's font offsets (+3.6 / +4 over the
    // .res ypos) in the incap layout (HealthIcon 12,60; HealthNumber 23,60).
    commands.spawn((
        Text::new(","),
        hud_font(&toolbox, 15.5),
        TextColor(Color::srgb(57. / 255., 189. / 255., 78. / 255.)),
        hud_rect(46., 34.6, 15., 26.),
        HealthTint,
        ChildOf(player),
        DeathPart::HealthText,
        LocalPart(TeamPartKind::Moves(hud_rect(46., 34.6, 15., 26.), hud_rect(12., 63.6, 15., 26.))),
    ));
    commands.spawn((
        Text::new("100"),
        hud_font(&native, 14.5),
        TextColor(Color::srgb(57. / 255., 189. / 255., 78. / 255.)),
        hud_rect(56., 36., 92., 25.),
        Label::Health,
        HealthTint,
        ChildOf(player),
        DeathPart::HealthText,
        LocalPart(TeamPartKind::Moves(hud_rect(56., 36., 92., 25.), hud_rect(23., 64., 92., 25.))),
    ));
    let bar = commands
        .spawn((
            Node {
                overflow: Overflow::clip(),
                ..hud_rect(44., 55.8, 111., 14.9)
            },
            // LocalPlayerPanel_Incap.res Health: 11,54 113x12.
            LocalPart(TeamPartKind::Bar(
                Node { overflow: Overflow::clip(), ..hud_rect(44., 55.8, 111., 14.9) },
                Node { overflow: Overflow::clip(), ..hud_rect(11., 54., 113., 12.) },
            )),
            ChildOf(player),
        ))
        .id();
    for temporary in [false, true] {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                min_width: px(0.),
                min_height: px(0.),
                width: percent(100.),
                height: percent(100.),
                ..default()
            },
            BackgroundColor(Color::NONE),
            ImageNode::new(handles["hud/healthbar_withglow_green"].clone())
                .with_mode(bevy::ui::widget::NodeImageMode::Stretch),
            HealthFill(None, temporary),
            ChildOf(bar),
        ));
    }
    commands.spawn((
        ImageNode {
            color: Color::NONE,
            ..ImageNode::new(handles["hud/overlay_dead"].clone())
                .with_mode(bevy::ui::widget::NodeImageMode::Stretch)
        },
        hud_rect(0., 24., 292., 146.),
        DeathPart::Overlay,
        ChildOf(player),
    ));
    let layer = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(0.),
                top: px(0.),
                right: px(0.),
                bottom: px(0.),
                display: Display::None,
                ..default()
            },
            DeathPart::Banner,
            GlobalZIndex(20),
        ))
        .id();
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(0.),
            top: px(0.),
            width: percent(100.),
            height: vh(75. / 4.8),
            ..default()
        },
        BackgroundColor(Color::BLACK),
        ChildOf(layer),
    ));
    let banner = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(0.),
                top: px(0.),
                right: px(0.),
                bottom: px(0.),
                ..default()
            },
            DeathPart::BannerContent,
            ChildOf(layer),
        ))
        .id();
    commands.spawn((
        ImageNode {
            rect: hints.icons.get("icon_skull").copied(),
            ..ImageNode::new(handles["hud/iconsheet"].clone())
        },
        hud_rect(61., 34., 24., 24.),
        ChildOf(banner),
    ));
    commands.spawn((
        Text::new("YOU ARE DEAD"),
        hud_font(&native, 20.),
        hud_rect(89., 34., 400., 27.),
        ChildOf(banner),
    ));
    commands.spawn((
        Text::new("The Survivors have been overwhelmed"),
        hud_font(&native, 11.),
        TextColor(Color::srgb(0.65, 0., 0.)),
        hud_rect(100., 57., 500., 15.),
        OverwhelmedText,
        ChildOf(banner),
    ));
    let weapon = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: vh(7.8 / 4.8),
                top: vh(160. / 4.8),
                width: vh(53.4 / 4.8),
                height: vh(24.6 / 4.8),
                ..default()
            },
            WeaponPanel,
        ))
        .id();
    commands.spawn((
        ImageNode {
            image_mode: bevy::ui::widget::NodeImageMode::Sliced(bevy::sprite::TextureSlicer {
                border: bevy::sprite::BorderRect::all(16.),
                center_scale_mode: bevy::sprite::SliceScaleMode::Stretch,
                sides_scale_mode: bevy::sprite::SliceScaleMode::Stretch,
                max_corner_scale: 0.85,
            }),
            ..ImageNode::new(handles["hud/scalablepanel_bgmidgrey_outlinegreen_glow"].clone())
        },
        Node {
            width: percent(100.),
            height: percent(100.),
            ..hud_rect(-1., -1., 55.4, 26.6)
        },
        InventoryFrame(0),
        ChildOf(weapon),
    ));
    let selected = Color::srgb(142. / 255., 214. / 255., 57. / 255.);
    commands.spawn((
        Text::new("15"),
        hud_font(&native, 16.),
        TextColor(selected),
        TextLayout::new(Justify::Right, LineBreak::NoWrap),
        Node {
            overflow: Overflow::clip(),
            left: Val::Auto,
            right: vh(31.4 / 4.8),
            width: Val::Auto,
            max_width: vh(22. / 4.8),
            ..hud_rect(0., 1.5, 22., 18.)
        },
        Label::Ammo,
        ChildOf(weapon),
    ));
    commands.spawn((
        ImageNode {
            color: selected,
            ..ImageNode::new(handles["weapon_pistol"].clone())
                .with_mode(bevy::ui::widget::NodeImageMode::Stretch)
        },
        hud_rect(29., 4., 25., 16.),
        PrimaryIcon(0),
        ChildOf(weapon),
    ));
    commands.spawn((
        Text::new(""),
        hud_font(&native, 10.),
        TextColor(Color::srgb(93. / 255., 142. / 255., 32. / 255.)),
        TextLayout::new(Justify::Right, LineBreak::NoWrap),
        Node {
            overflow: Overflow::clip(),
            left: Val::Auto,
            right: vh(31.4 / 4.8),
            width: Val::Auto,
            max_width: vh(22. / 4.8),
            ..hud_rect(0., 14., 22., 12.)
        },
        Label::Reserve,
        ChildOf(weapon),
    ));
    commands.spawn((
        ImageNode {
            color: selected,
            ..ImageNode::new(handles["hud/iconsheet"].clone())
        },
        // The sprite has a narrow bullet in the centre of a 64px tile.
        // Its tile overlaps the number column; its opaque pixels sit to its right.
        hud_rect(20., 3., 22., 22.),
        PrimaryIcon(12),
        ChildOf(weapon),
    ));
    for (slot, path) in [
        (1, "weapon_pistol"),
        (2, "weapon_pistol"),
        (3, "hud/icon_molotov"),
        (4, "hud/icon_first_aid_kit"),
        (5, "hud/icon_painpills"),
    ] {
        let icon = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    right: vh(7.5 / 4.8),
                    top: vh(217.33 / 4.8),
                    width: vh(23.7 / 4.8),
                    height: vh(24. / 4.8),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                BorderColor::all(Color::NONE),
                InventoryIcon(slot),
            ))
            .id();
        commands.spawn((
            ImageNode {
                color: if slot <= 2 { Color::NONE } else { Color::WHITE },
                image_mode: bevy::ui::widget::NodeImageMode::Sliced(bevy::sprite::TextureSlicer {
                    border: bevy::sprite::BorderRect::all(8.),
                    center_scale_mode: bevy::sprite::SliceScaleMode::Stretch,
                    sides_scale_mode: bevy::sprite::SliceScaleMode::Stretch,
                    max_corner_scale: 0.7,
                }),
                ..ImageNode::new(handles["hud/scalablepanel_bgblack50_outlinewhite"].clone())
            },
            Node {
                width: percent(100.),
                height: percent(100.),
                ..hud_rect(0., 0., 23.7, 24.)
            },
            InventoryFrame(slot),
            ChildOf(icon),
        ));
        if slot <= 2 {
            commands.spawn((
                Text::default(),
                hud_font(&native, 18.),
                TextColor(Color::WHITE),
                TextLayout::new(Justify::Right, LineBreak::NoWrap),
                Node {
                    overflow: Overflow::clip(),
                    left: Val::Auto,
                    right: vh(29.4 / 4.8),
                    width: Val::Auto,
                    max_width: vh(22. / 4.8),
                    ..hud_rect(2., 3., 21., 21.)
                },
                Label::SlotAmmo(slot),
                ChildOf(icon),
            ));
        }
        commands.spawn((
            ImageNode {
                rect: Some(Rect::from_corners(Vec2::ZERO, Vec2::splat(64.))),
                ..ImageNode::new(handles[path].clone())
            },
            hud_rect(3., 2., 18., 20.),
            PrimaryIcon(slot),
            ChildOf(icon),
        ));
    }
    commands.spawn((
        Text::new("Click to play · Esc pauses"),
        hud_font(&native, 15.),
        Node {
            position_type: PositionType::Absolute,
            left: percent(28.),
            top: percent(35.),
            ..default()
        },
        Label::Status,
    ));
    commands.spawn((
        Text::new(""),
        hud_font(&native, 13.),
        Node {
            position_type: PositionType::Absolute,
            left: percent(40.),
            top: percent(65.),
            ..default()
        },
        Label::Progress,
    ));
    let hint = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: percent(32.),
                top: percent(57.),
                width: vh(320. / 4.8),
                height: vh(48. / 4.8),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();
    commands.spawn((
        ImageNode {
            rect: hints.icons.get("icon_key_generic").copied(),
            ..ImageNode::new(handles["hud/iconsheet"].clone())
        },
        hud_rect(0., 0., 32., 32.),
        HintIcon,
        ChildOf(hint),
    ));
    commands.spawn((
        Text::new(""),
        hud_font(&native, 14.),
        hud_rect(11., 6., 35., 24.),
        Label::HintBinding,
        TextColor(Color::BLACK),
        ChildOf(hint),
    ));
    commands.spawn((
        Text::new(""),
        hud_font(&native, 16.),
        hud_rect(38., 4., 280., 40.),
        Label::Instructor,
        ChildOf(hint),
    ));
    commands.insert_resource(hints);
    let center = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: percent(50.),
                top: percent(50.),
                width: px(1.),
                height: px(1.),
                ..default()
            },
            Crosshair,
            DeathPart::Equipment,
        ))
        .id();
    for r in [
        rect(-17., -1., 9., 2.),
        rect(8., -1., 9., 2.),
        rect(-1., -17., 2., 9.),
        rect(-1., 8., 2., 9.),
    ] {
        commands.spawn((
            r,
            BackgroundColor(Color::srgb(0.65, 0.9, 0.2)),
            ChildOf(center),
        ));
    }
    commands.spawn((
        Text::new(""), hud_font(&native,16.),
        Node { position_type: PositionType::Absolute, left: percent(42.), top: percent(8.), ..default() },
        Label::Fuel,
    ));
    crate::adrenaline::spawn_overlay(&mut commands, &mut images);
    crate::progress_bar::spawn(&mut commands, &handles, &native);
    commands.insert_resource(HudImages(handles));
    commands.insert_resource(HudFonts {
        _native: native,
        _fallback: fallback,
    });
}
pub fn target_camera(
    mut commands: Commands,
    camera: Single<Entity, With<crate::game::ViewCamera>>,
    roots: Query<
        (Entity, Option<&crate::l4d2r_cinematic::CinematicFade>),
        (With<Node>, Without<ChildOf>),
    >,
) {
    let container = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: percent(100.),
                height: percent(100.),
                ..default()
            },
            UiTargetCamera(*camera),
            HudRoot,
        ))
        .id();
    for (root, fade) in &roots {
        if fade.is_some() {
            commands
                .entity(root)
                .insert((UiTargetCamera(*camera), HudRoot));
        } else {
            commands.entity(root).insert(ChildOf(container));
        }
    }
}
fn rect(x: f32, y: f32, w: f32, h: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(x),
        top: px(y),
        width: px(w),
        height: px(h),
        ..default()
    }
}
// Avoid invalidating Bevy's layout/image sizing when the HUD value is unchanged.
const INVENTORY_GLOW_PAD: f32 = 8. * 0.85 / 1.5;
// Leave two native HUD units between the selected glow and its neighbour.
fn inventory_top(slot: u8) -> f32 {
    let secondary = 160. + 28. + INVENTORY_GLOW_PAD + 2.;
    if slot <= 2 {
        secondary
    } else {
        secondary + (24. + INVENTORY_GLOW_PAD + 2.) * (slot - 2) as f32
    }
}
pub fn update_frames(
    s: Res<Match>,
    handles: Res<HudImages>,
    windows: Query<&Window>,
    mut frames: Query<(&InventoryFrame, &mut ImageNode, &mut Node)>,
) {
    let scale = windows.iter().next().map_or(1., |w| w.height() / 720.);
    for (frame, mut image, mut node) in &mut frames {
        let slot = if frame.0 == 0 {
            if s.primary.is_some() { 1 } else { 2 }
        } else {
            frame.0
        };
        let selected = s.slot == slot;
        let path = if selected {
            "hud/scalablepanel_bgmidgrey_outlinegreen_glow"
        } else {
            "hud/scalablepanel_bgblack50_outlinewhite"
        };
        if let Some(handle) = handles.0.get(path) {
            set_field(&mut image, |v| &mut v.image, handle.clone());
        }
        set_field(
            &mut image,
            |v| &mut v.image_mode,
            bevy::ui::widget::NodeImageMode::Sliced(bevy::sprite::TextureSlicer {
                border: bevy::sprite::BorderRect::all(if selected { 16. } else { 8. }),
                center_scale_mode: bevy::sprite::SliceScaleMode::Stretch,
                sides_scale_mode: bevy::sprite::SliceScaleMode::Stretch,
                max_corner_scale: if selected { 0.85 } else { 0.25 } * scale,
            }),
        );
        // The green texture includes eight source pixels of outer glow.
        // Expand around the box instead of shrinking its border into it.
        let pad = if selected { INVENTORY_GLOW_PAD } else { 0. };
        let width = if frame.0 <= 2 { 53.4 } else { 23.7 };
        let height = if frame.0 == 0 {
            if s.primary.is_some() { 28. } else { 24.6 }
        } else {
            24.
        };
        set_field(&mut node, |v| &mut v.left, vh(-pad / 4.8));
        set_field(&mut node, |v| &mut v.top, vh(-pad / 4.8));
        set_field(&mut node, |v| &mut v.width, vh((width + 2. * pad) / 4.8));
        set_field(&mut node, |v| &mut v.height, vh((height + 2. * pad) / 4.8));
        set_field(
            &mut image,
            |v| &mut v.color,
            if selected {
                Color::WHITE
            } else {
                Color::srgba(1., 1., 1., 0.35)
            },
        );
    }
}
fn set_field<C, V: PartialEq>(
    component: &mut Mut<'_, C>,
    field: impl FnOnce(&mut C) -> &mut V,
    value: V,
) {
    component.reborrow().map_unchanged(field).set_if_neq(value);
}
pub fn update(
    s: Res<Match>,
    rules: Res<Rules>,
    squad: Res<Squad>,
    handles: Res<HudImages>,
    hints: Res<NativeHints>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    camera: Query<&Transform, With<crate::viewer::GameCamera>>,
    mut hint_icons: Query<
        &mut ImageNode,
        (With<HintIcon>, Without<PrimaryIcon>, Without<HealthFill>),
    >,
    mut primary: Query<
        (&PrimaryIcon, &mut ImageNode, &mut Node),
        (Without<HealthFill>, Without<HintIcon>),
    >,
    mut labels: Query<(
        &Label,
        &mut Text,
        Option<&mut TextFont>,
        Option<&mut TextColor>,
    )>,
    mut fills: Query<
        (
            &HealthFill,
            &mut Node,
            &mut BackgroundColor,
            Option<&mut ImageNode>,
        ),
        Without<PrimaryIcon>,
    >,
    mut overlays: Query<
        &mut BackgroundColor,
        (
            With<DamageOverlay>,
            Without<HealthFill>,
            Without<InventoryIcon>,
        ),
    >,
    mut icons: Query<
        (
            &InventoryIcon,
            &mut BackgroundColor,
            &mut BorderColor,
            &mut Node,
        ),
        (
            Without<HealthFill>,
            Without<DamageOverlay>,
            Without<PrimaryIcon>,
        ),
    >,
    mut weapon_panel: Query<
        &mut Node,
        (
            With<WeaponPanel>,
            Without<PrimaryIcon>,
            Without<InventoryIcon>,
            Without<HealthFill>,
        ),
    >,
    image_assets: Option<Res<Assets<Image>>>,
) {
    let _timing = crate::performance::scope("game_ui::update");
    let rows = weapon_rows(&s);
    let shown = displayed_gun(&s);
    let held = rules.weapons.get(&shown.class).unwrap_or(&rules.pistol);
    let panel_selected = s.slot == if s.primary.is_some() { 1 } else { 2 };
    let ammo_visible = !held.melee();
    let reserve_visible = ammo_visible && !held.infinite();
    for mut panel in &mut weapon_panel {
        set_field(
            &mut panel,
            |v| &mut v.top,
            vh(if s.primary.is_some() { 160. } else { 189. } / 4.8),
        );
        let display = if s.dead || s.infected {
            Display::None
        } else {
            Display::Flex
        };
        if panel.display != display {
            panel.display = display;
        }
        let width = vh(53.4 / 4.8);
        if panel.width != width {
            set_field(&mut panel, |v| &mut v.width, width);
        }
        set_field(
            &mut panel,
            |v| &mut v.height,
            vh(if reserve_visible { 28. } else { 24.6 } / 4.8),
        );
    }
    let mut lesson = None;
    // Hints give way to the progress bar (healing, defibrillating, reviving,
    // deploying), which sits in the same place on screen.
    let progress_bar = crate::progress_bar::content(&s, &squad, |_| String::new()).is_some();
    if !s.infected
        && !s.paused
        && !s.dead
        && !s.incap
        && !progress_bar
        && !logic.as_ref().is_some_and(|l| l.active)
    {
        if let Some((id, _)) = logic.as_ref().and_then(|l| l.hud_hint) {
            let e = &logic.as_ref().unwrap().world.ents[id];
            lesson = Some((
                hints.caption(
                    e.me.kv("hint_caption")
                        .or_else(|| e.me.kv("message"))
                        .unwrap_or(""),
                ),
                NativeHints::binding(e.me.kv("hint_binding").unwrap_or("")).to_owned(),
                match e.me.kv("hint_icon_onscreen").unwrap_or("icon_tip") {
                    "use_binding" => "icon_key_generic".into(),
                    icon => icon.to_lowercase(),
                },
            ));
        } else if s.slot == 4 && s.medkit && s.action <= 0. {
            // The instructor hint gives way to the progress bar while healing.
            lesson = Some((
                hints.caption("#L4D_Instructor_notify_heal_self_hold"),
                String::new(),
                "icon_mouseleft".into(),
            ));
        } else if rules
            .weapons
            .get(&s.gun().class)
            .is_some_and(|w| !w.melee() && s.gun().ammo as f32 <= w.clip as f32 * 0.4)
            && s.reload <= 0.
        {
            lesson = Some((
                hints.caption("#L4D_Instructor_explain_reload"),
                "R".into(),
                "icon_key_generic".into(),
            ));
        } else if s.health < 40. && s.medkit {
            lesson = Some((
                hints.caption("#L4D_Instructor_notify_heal_self"),
                "4".into(),
                "icon_key_generic".into(),
            ));
        } else if !s.hint.is_empty() {
            let item = s
                .hint
                .split("Pick up ")
                .nth(1)
                .unwrap_or("")
                .replace("weapon_melee_", "")
                .replace("weapon_", "")
                .replace('_', " ");
            lesson = Some((
                format!(
                    "{} {}",
                    hints.caption("#L4D_Instructor_explain_pickup_item"),
                    item
                ),
                "E".into(),
                "icon_key_generic".into(),
            ));
        } else if let (Some(logic), Some(camera)) = (logic.as_ref(), camera.iter().next()) {
            let eye = camera.translation;
            let direction = camera.forward().as_vec3();
            if logic.world.ents.iter().enumerate().any(|(id, e)| {
                matches!(e.kind, l4d2r_game_core::entities::Kind::Door(_))
                    && e.alive
                    && logic
                        .bounds(id)
                        .and_then(|(lo, hi)| crate::ray_box(eye, direction, lo, hi))
                        .is_some_and(|d| d < 2.44)
            }) {
                lesson = Some((
                    hints.caption("#L4D_Instructor_explain_use_door"),
                    "E".into(),
                    "icon_key_generic".into(),
                ));
            }
        }
    }
    for mut icon in &mut hint_icons {
        set_field(
            &mut icon,
            |v| &mut v.color,
            if lesson.is_some() {
                Color::WHITE
            } else {
                Color::NONE
            },
        );
        if let Some((_, _, name)) = &lesson {
            set_field(
                &mut icon,
                |v| &mut v.rect,
                hints
                    .icons
                    .get(name)
                    .or_else(|| hints.icons.get("icon_tip"))
                    .copied(),
            );
        }
    }
    for (label, mut text, font, text_color) in &mut labels {
        if let Some(mut color) = text_color {
            match label {
                Label::Ammo => color.set_if_neq(TextColor(selected_color(panel_selected))),
                Label::Reserve => color.set_if_neq(TextColor(if panel_selected {
                    Color::srgb(93. / 255., 142. / 255., 32. / 255.)
                } else {
                    Color::srgb(169. / 255., 169. / 255., 169. / 255.)
                })),
                Label::SlotAmmo(slot) => {
                    color.set_if_neq(TextColor(selected_color(s.slot == *slot)))
                }
                _ => false,
            };
        }
        let value = match label {
            Label::Fuel => if s.infected || s.dead { String::new() } else {
                logic.as_ref().and_then(|l|l.world.scavenge_progress()).map(|(n,total)| match crate::scavenge_rules::hud_text() {
                    Some(clock) => format!("{clock}   FUEL {n} / {total}"),
                    None => format!("FUEL {n} / {total}"),
                }).unwrap_or_default()
            },
            Label::Instructor => lesson.as_ref().map(|l| l.0.clone()).unwrap_or_default(),
            Label::HintBinding => lesson.as_ref().map(|l| l.1.clone()).unwrap_or_default(),
            Label::Health => {
                if s.incap {
                    format!("{:.0}", s.incap_health)
                } else {
                    format!("{:.0}", s.health + s.temporary)
                }
            }
            Label::Ammo => {
                if !ammo_visible {
                    String::new()
                } else {
                    shown.ammo.to_string()
                }
            }
            Label::Reserve => {
                let gun = shown;
                let weapon = rules.weapons.get(&gun.class).unwrap_or(&rules.pistol);
                if !reserve_visible || weapon.infinite() || weapon.melee() {
                    String::new()
                } else {
                    gun.reserve.to_string()
                }
            }
            Label::SlotAmmo(slot) => {
                let gun = if *slot == 1 {
                    s.primary.as_ref()
                } else {
                    Some(&s.pistol)
                };
                gun.filter(|g| !rules.weapons.get(&g.class).is_some_and(|w| w.melee()))
                    .map_or(String::new(), |g| g.ammo.to_string())
            }
            Label::Status => {
                if s.dead {
                    String::new()
                } else if s.paused {
                    // Touch builds pause from the menu button, not Esc.
                    if crate::game::ui_fit::touch_build() {
                        String::new()
                    } else {
                        "Click to play · Esc pauses".into()
                    }
                } else if s.black_white && !s.incap {
                    "BLACK AND WHITE - heal before the next incapacitation".into()
                } else if s.incap {
                    "INCAPACITATED · Wait for a teammate to revive you".into()
                } else {
                    if lesson.is_some() {
                        String::new()
                    } else {
                        s.hint.clone()
                    }
                }
            }
            Label::Progress => {
                // Timed item use shows the retail HudProgressBar
                // (progress_bar.rs); pills and adrenaline show nothing.
                if s.reload > 0. {
                    "RELOADING".into()
                } else {
                    String::new()
                }
            }
            // The panel artwork carries death and incapacitation
            // (team_presentation); retail draws no status words there.
            Label::Team(_) => String::new(),
        };
        if let Some(mut font) = font {
            let size = match label {
                Label::Ammo | Label::SlotAmmo(_) => Some(ammo_font_size(value.len())),
                Label::Reserve => Some(if value.len() > 3 { 8. } else { 10. }),
                _ => None,
            };
            if let Some(size) = size {
                set_field(&mut font, |v| &mut v.font_size, FontSize::Vh(size / 4.8));
            }
        }
        if text.0 != value {
            text.0 = value;
        }
    }
    for (fill, mut node, mut color, image) in &mut fills {
        let state = fill.0.map_or(&*s, |i| &squad.0[i].state);
        let segment = health_segment(state, fill.1);
        let (start, end) = segment.span;
        let left = percent(start * 100.);
        let width = percent((end - start).max(0.) * 100.);
        if node.left != left {
            set_field(&mut node, |v| &mut v.left, left);
        }
        if node.width != width {
            set_field(&mut node, |v| &mut v.width, width);
        }
        if let Some(mut image) = image {
            if let Some(handle) = handles.0.get(segment.texture) {
                if image.image != *handle {
                    image.image = handle.clone();
                }
            }
            // Retail draws the matching slice of the bar texture
            // (DrawTexturedSubRect u = start..end, v = 0..1), not a squashed
            // copy. The solid and ticks textures differ in size, so the
            // slice is taken from each texture's own dimensions.
            let size = image_assets
                .as_ref()
                .and_then(|a| a.get(&image.image))
                .map_or(Vec2::new(512., 64.), |i| i.size_f32());
            // The ticks (temporary health) textures stop at row 50 of 64,
            // where the solid bar's glow tail begins; sample only their drawn
            // rows so the buffer fills the same height as permanent health.
            let bottom = if fill.1 { size.y * 50. / 64. } else { size.y };
            let rect = Some(Rect::new(
                start * size.x,
                0.,
                end.max(start) * size.x,
                bottom,
            ));
            if image.rect != rect {
                image.rect = rect;
            }
            set_field(&mut color, |v| &mut v.0, Color::NONE);
        } else {
            set_field(&mut color, |v| &mut v.0, Color::NONE);
        }
    }
    for mut color in &mut overlays {
        set_field(
            &mut color,
            |v| &mut v.0,
            Color::srgba(0.7, 0., 0., s.damage_flash * 0.7),
        );
    }
    if let Some(gun) = &s.primary {
        if let Some(image) = handles.0.get(&gun.class) {
            for (icon, mut node, _) in &mut primary {
                if icon.0 == 1 {
                    set_field(&mut node, |v| &mut v.image, image.clone());
                    if let Some(weapon) = rules.weapons.get(&gun.class) {
                        set_field(&mut node, |v| &mut v.rect, Some(weapon.icon_rect));
                    }
                }
            }
        }
    }
    if let Some(weapon) = rules.weapons.get(&s.pistol.class) {
        if let Some(image) = handles.0.get(&s.pistol.class) {
            for (icon, mut node, _) in &mut primary {
                if icon.0 == 2 {
                    set_field(&mut node, |v| &mut v.image, image.clone());
                    set_field(&mut node, |v| &mut v.rect, Some(weapon.icon_rect));
                }
            }
        }
    }
    for (icon, mut node, mut layout) in &mut primary {
        // Dimensions below already fit the authored sprite. Do not let the
        // image's intrinsic size impose a new layout minimum after a pickup.
        set_field(
            &mut node,
            |v| &mut v.image_mode,
            bevy::ui::widget::NodeImageMode::Stretch,
        );
        if icon.0 == 12 {
            let key = format!("{}::ammo", shown.class);
            if let (Some(image), Some(rect)) = (handles.0.get(&key), hints.icons.get(&key)) {
                set_field(&mut node, |v| &mut v.image, image.clone());
                set_field(&mut node, |v| &mut v.rect, Some(*rect));
                set_field(
                    &mut node,
                    |v| &mut v.color,
                    if reserve_visible {
                        selected_color(panel_selected)
                    } else {
                        Color::NONE
                    },
                );
            } else {
                set_field(&mut node, |v| &mut v.color, Color::NONE);
            }
            continue;
        }
        if icon.0 >= 6 {
            let index = if icon.0 >= 9 { icon.0 - 9 } else { icon.0 - 6 } as usize;
            // TeammatePanel_Incap.res has no IconForSlot_* panels, and dead
            // survivors carry nothing.
            let owned = squad.0.get(index).is_some_and(|m| {
                !m.state.incap && !m.state.dead && if icon.0 >= 9 {
                    m.state.pills || m.state.adrenaline
                } else {
                    m.state.medkit || m.state.defibrillator
                }
            });
            if let Some(m) = squad.0.get(index) {
                let class = if icon.0 >= 9 {
                    if m.state.adrenaline {
                        "weapon_adrenaline"
                    } else {
                        "weapon_pain_pills"
                    }
                } else {
                    if m.state.defibrillator {
                        "weapon_defibrillator"
                    } else {
                        "weapon_first_aid_kit"
                    }
                };
                if let (Some(w), Some(image)) = (rules.weapons.get(class), handles.0.get(class)) {
                    set_field(&mut node, |v| &mut v.image, image.clone());
                    let medkit = class == "weapon_first_aid_kit";
                    let rect = if medkit {
                        Rect::from_corners(Vec2::new(64., 0.), Vec2::new(128., 64.))
                    } else {
                        w.icon_rect
                    };
                    set_field(&mut node, |v| &mut v.rect, Some(rect));
                    if icon.0 < 9 {
                        set_field(
                            &mut layout,
                            |v| &mut v.left,
                            vh(if medkit { 91.5 } else { 99. } / 4.8),
                        );
                        set_field(
                            &mut layout,
                            |v| &mut v.top,
                            vh(if medkit { 5.5 } else { 12.25 } / 4.8),
                        );
                        set_field(
                            &mut layout,
                            |v| &mut v.width,
                            vh(if medkit { 28. } else { 14. } / 4.8),
                        );
                        set_field(
                            &mut layout,
                            |v| &mut v.height,
                            vh(if medkit { 28. } else { 14. } / 4.8),
                        );
                    }
                }
            }
            set_field(
                &mut node,
                |v| &mut v.color,
                if owned { Color::WHITE } else { Color::NONE },
            );
            continue;
        }
        if icon.0 == 0 {
            if let (Some(weapon), Some(image)) =
                (rules.weapons.get(&shown.class), handles.0.get(&shown.class))
            {
                set_field(&mut node, |v| &mut v.image, image.clone());
                set_field(&mut node, |v| &mut v.rect, Some(weapon.icon_rect));
                set_field(&mut node, |v| &mut v.color, selected_color(panel_selected));
                let (w, h) = if reserve_visible {
                    (60., 20.)
                } else {
                    fit_icon(
                        weapon.icon_rect,
                        if ammo_visible { 24. } else { 43. },
                        if ammo_visible { 24. } else { 18. },
                    )
                };
                set_field(
                    &mut layout,
                    |v| &mut v.left,
                    vh(if reserve_visible {
                        53.4 - w
                    } else if ammo_visible {
                        29.
                    } else {
                        (53.4 - w) * 0.5
                    } / 4.8),
                );
                set_field(
                    &mut layout,
                    |v| &mut v.top,
                    vh(if reserve_visible {
                        -10.
                    } else {
                        (24.6 - h) * 0.5
                    } / 4.8),
                );
                set_field(&mut layout, |v| &mut v.width, vh(w / 4.8));
                set_field(&mut layout, |v| &mut v.height, vh(h / 4.8));
            }
            continue;
        }
        let owned = match icon.0 {
            1 => s.primary.is_some(),
            2 => true,
            3 => s.throwable.is_some() || s.throw_class.is_some(),
            4 => s.medkit || s.defibrillator || s.upgrade_pack.is_some(),
            5 => s.pills || s.adrenaline,
            _ => false,
        };
        if matches!(icon.0, 4 | 5) {
            let class = if icon.0 == 4 {
                if let Some(kind) = s.upgrade_pack {
                    kind.pack_class()
                } else if s.defibrillator {
                    "weapon_defibrillator"
                } else {
                    "weapon_first_aid_kit"
                }
            } else {
                if s.adrenaline {
                    "weapon_adrenaline"
                } else {
                    "weapon_pain_pills"
                }
            };
            if let (Some(weapon), Some(image)) = (rules.weapons.get(class), handles.0.get(class)) {
                set_field(&mut node, |v| &mut v.image, image.clone());
                set_field(&mut node, |v| &mut v.rect, Some(weapon.icon_rect));
            }
        }
        if icon.0 == 3 {
            if let Some(class) = s.throw_class.as_ref().or(s.throwable.as_ref()) {
                if let (Some(weapon), Some(image)) =
                    (rules.weapons.get(class), handles.0.get(class))
                {
                    set_field(&mut node, |v| &mut v.image, image.clone());
                    set_field(&mut node, |v| &mut v.rect, Some(weapon.icon_rect));
                }
            } else if let Some(image) = handles.0.get("hud/icon_molotov") {
                set_field(&mut node, |v| &mut v.image, image.clone());
                set_field(
                    &mut node,
                    |v| &mut v.rect,
                    Some(Rect::from_corners(Vec2::ZERO, Vec2::splat(64.))),
                );
            }
        }
        set_field(
            &mut node,
            |v| &mut v.color,
            if s.slot == icon.0 {
                selected_color(true)
            } else {
                if owned {
                    Color::WHITE
                } else {
                    Color::srgb(90. / 255., 90. / 255., 90. / 255.)
                }
            },
        );
        if let Some(rect) = node.rect {
            let melee = icon.0 == 2
                && rules
                    .weapons
                    .get(&s.pistol.class)
                    .is_some_and(|w| w.melee());
            let (w, h) = fit_icon(
                rect,
                if melee {
                    43.
                } else if icon.0 <= 2 {
                    27.
                } else {
                    18.
                },
                if icon.0 <= 2 { 24. } else { 20. },
            );
            let box_width = if icon.0 <= 2 { 53.4 } else { 23.7 };
            set_field(
                &mut layout,
                |v| &mut v.left,
                vh(if icon.0 <= 2 && !melee {
                    box_width - w
                } else {
                    (box_width - w) * 0.5
                } / 4.8),
            );
            set_field(&mut layout, |v| &mut v.top, vh((24. - h) * 0.5 / 4.8));
            set_field(&mut layout, |v| &mut v.width, vh(w / 4.8));
            set_field(&mut layout, |v| &mut v.height, vh(h / 4.8));
        }
    }
    for (icon, mut color, mut border, mut node) in &mut icons {
        let owned = match icon.0 {
            1 => s.primary.is_some(),
            2 => true,
            3 => s.throwable.is_some() || s.throw_class.is_some(),
            4 => s.medkit || s.defibrillator || s.upgrade_pack.is_some(),
            5 => s.pills || s.adrenaline,
            _ => false,
        };
        let display = if !s.dead
            && !s.infected
            && (!s.incap || icon.0 == 2)
            && (if icon.0 <= 2 {
                rows.contains(&icon.0)
            } else {
                owned || icon.0 >= 3
            }) {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
        let top = inventory_top(icon.0);
        set_field(&mut node, |v| &mut v.top, vh(top / 4.8));
        set_field(
            &mut node,
            |v| &mut v.width,
            vh(if icon.0 <= 2 { 53.4 } else { 23.7 } / 4.8),
        );
        set_field(&mut color, |v| &mut v.0, Color::NONE);
        set_field(&mut node, |v| &mut v.border, UiRect::ZERO);
        border.set_if_neq(BorderColor::all(Color::NONE));
    }
}
pub fn team_presentation(
    squad: Res<crate::game_state::Squad>,
    mut parts: Query<(&TeamPart, &mut Node, Option<&mut ImageNode>)>,
) {
    for (part, mut node, image) in &mut parts {
        let Some(member) = squad.0.get(part.index).map(|m| &m.state) else { continue };
        let (dead, incap) = (member.dead, member.incap && !member.dead);
        let show = |node: &mut Node, visible: bool| {
            let display = if visible { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
        };
        // A kicked bot has no teammate panel.
        if crate::game_state::kicked(part.index) {
            show(&mut node, false);
            continue;
        }
        match &part.kind {
            TeamPartKind::Background => show(&mut node, !incap),
            TeamPartKind::IncapBackground => show(&mut node, incap),
            TeamPartKind::Head => {
                show(&mut node, !incap);
                if let Some(mut image) = image {
                    let color = if dead { Color::srgba(1., 0., 0., 200. / 255.) } else { Color::WHITE };
                    if image.color != color {
                        image.color = color;
                    }
                }
            }
            TeamPartKind::DeadOverlay => {
                if let Some(mut image) = image {
                    let color = if dead { Color::WHITE } else { Color::NONE };
                    if image.color != color {
                        image.color = color;
                    }
                }
            }
            TeamPartKind::Moves(normal, incapped) | TeamPartKind::Bar(normal, incapped) => {
                let target = if incap { incapped } else { normal };
                let visible = !(dead && matches!(part.kind, TeamPartKind::Bar(..)));
                // Overflow stays as ui_fit::hud_clipping set it.
                let wanted = Node {
                    display: if visible { Display::Flex } else { Display::None },
                    overflow: node.overflow,
                    ..target.clone()
                };
                if (node.left, node.top, node.width, node.height, node.display)
                    != (wanted.left, wanted.top, wanted.width, wanted.height, wanted.display)
                {
                    *node = wanted;
                }
            }
        }
    }
}
/// Local panel layout for the incapacitated state (LocalPlayerPanel_Incap.res).
/// Visibility of the health text while dead stays with `death_presentation`.
pub fn local_presentation(s: Res<Match>, mut parts: Query<(&LocalPart, &mut Node)>) {
    let incap = s.incap && !s.dead && !s.infected;
    for (part, mut node) in &mut parts {
        let display = |visible: bool| if visible { Display::Flex } else { Display::None };
        let wanted = match &part.0 {
            TeamPartKind::Background => Node { display: display(!incap), ..node.clone() },
            TeamPartKind::IncapBackground => Node { display: display(incap), ..node.clone() },
            TeamPartKind::Head => Node { display: display(!incap), ..node.clone() },
            TeamPartKind::Moves(normal, incapped) | TeamPartKind::Bar(normal, incapped) => Node {
                display: node.display,
                overflow: node.overflow,
                ..if incap { incapped.clone() } else { normal.clone() }
            },
            TeamPartKind::DeadOverlay => continue,
        };
        if (node.left, node.top, node.width, node.height, node.display)
            != (wanted.left, wanted.top, wanted.width, wanted.height, wanted.display)
        {
            *node = wanted;
        }
    }
}
pub fn death_presentation(
    s: Res<Match>,
    safe: Option<Res<crate::game::ui_fit::SafeArea>>,
    mut parts: Query<(&DeathPart, Option<&mut Node>, Option<&mut ImageNode>)>,
) {
    let safe = safe.map_or_else(Default::default, |s| *s);
    for (part, node, image) in &mut parts {
        match part {
            DeathPart::Banner => {
                if let Some(mut node) = node {
                    let display = if s.dead && !s.infected {
                        Display::Flex
                    } else {
                        Display::None
                    };
                    // The HUD container is inset by the safe area; undo it.
                    let inset = (px(-safe.left), px(-safe.top), px(-safe.right), px(-safe.bottom));
                    if node.display != display || (node.left, node.top, node.right, node.bottom) != inset {
                        node.display = display;
                        (node.left, node.top, node.right, node.bottom) = inset;
                    }
                }
            }
            DeathPart::BannerContent => {
                if let Some(mut node) = node {
                    let inset = (px(safe.left), px(safe.top), px(safe.right), px(safe.bottom));
                    if (node.left, node.top, node.right, node.bottom) != inset {
                        (node.left, node.top, node.right, node.bottom) = inset;
                    }
                }
            }
            DeathPart::HealthText | DeathPart::Equipment => {
                if let Some(mut node) = node {
                    let display = if s.dead || s.infected {
                        Display::None
                    } else {
                        Display::Flex
                    };
                    if node.display != display {
                        node.display = display;
                    }
                }
            }
            DeathPart::Overlay => {
                if let Some(mut image) = image {
                    let color = if s.dead { Color::WHITE } else { Color::NONE };
                    if image.color != color {
                        image.color = color;
                    }
                }
            }
            DeathPart::Portrait => {
                if let Some(mut image) = image {
                    // Client FUN_102c9a70: a dead survivor's head draws 255,0,0,200.
                    let color = if s.dead {
                        Color::srgba(1., 0., 0., 200. / 255.)
                    } else {
                        Color::WHITE
                    };
                    if image.color != color {
                        image.color = color;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;
    #[test]
    fn health_number_uses_the_retail_tier_colours() {
        let pistol = crate::game_state::tests_weapon();
        let mut s = Match::new(&pistol);
        s.health = 100.;
        assert_eq!(health_colour(&s), Color::srgb_u8(10, 177, 50));
        s.health = 39.;
        assert_eq!(health_colour(&s), Color::srgb_u8(216, 146, 12));
        s.health = 24.;
        assert_eq!(health_colour(&s), Color::srgb_u8(161, 25, 25));
        s.health = 100.;
        s.incap = true;
        assert_eq!(health_colour(&s), Color::srgb_u8(161, 25, 25));
    }
    #[test]
    fn secondary_pickups_replace_crop_and_keep_melee_icons_wide() {
        let pistol = crate::game_state::tests_weapon();
        let mut axe = pistol.clone();
        axe.class = "weapon_melee_fireaxe".into();
        axe.icon_rect = Rect::new(0., 0., 128., 64.);
        let mut single = pistol.clone();
        single.icon_rect = Rect::new(64., 0., 128., 64.);
        let mut dual = single.clone();
        dual.class = "weapon_pistol_dual".into();
        dual.icon_rect = Rect::new(0., 0., 64., 64.);
        let mut images = Assets::<Image>::default();
        let handles = [single.clone(), axe.clone(), dual.clone()]
            .into_iter()
            .map(|w| (w.class, images.add(Image::default())))
            .collect();
        let mut app = App::new();
        app.insert_resource(Match::new(&single))
            .insert_resource(Rules {
                pistol: single.clone(),
                weapons: HashMap::from([
                    (single.class.clone(), single.clone()),
                    (axe.class.clone(), axe.clone()),
                    (dual.class.clone(), dual.clone()),
                ]),
            })
            .insert_resource(Squad(vec![]))
            .insert_resource(HudImages(handles))
            .init_resource::<NativeHints>()
            .add_systems(Update, update);
        let icon = app
            .world_mut()
            .spawn((PrimaryIcon(2), ImageNode::default(), Node::default()))
            .id();
        let ammo = app
            .world_mut()
            .spawn((Label::SlotAmmo(2), Text::default()))
            .id();
        for weapon in [&axe, &dual, &single, &axe] {
            app.world_mut().resource_mut::<Match>().pistol.class = weapon.class.clone();
            app.update();
            let image = app.world().get::<ImageNode>(icon).unwrap();
            assert_eq!(image.rect, Some(weapon.icon_rect));
            assert_eq!(
                image.image,
                app.world().resource::<HudImages>().0[&weapon.class]
            );
            assert_eq!(image.image_mode, bevy::ui::widget::NodeImageMode::Stretch);
            assert_eq!(
                app.world().get::<Node>(icon).unwrap().width,
                vh(if weapon.melee() { 43. } else { 24. } / 4.8)
            );
            assert_eq!(
                app.world().get::<Text>(ammo).unwrap().0.is_empty(),
                weapon.melee()
            );
        }
    }
    #[test]
    fn native_panel_order_keeps_health_and_equipment_bound_to_the_right_bot() {
        use crate::campaign_settings::{Difficulty, SessionSettings, Survivor};
        for player in Survivor::roster(false) {
            let session = SessionSettings {
                map: "maps/rewrite_lab.bsp".into(),
                survivor: player,
                difficulty: Difficulty::Normal,
                original: false,
            };
            let original = Survivor::roster(false)
                .into_iter()
                .filter(|s| *s != player)
                .collect::<Vec<_>>();
            let panels = team_panels(&session);
            assert_eq!(panels.len(), 3);
            for (index, survivor) in &panels {
                assert_eq!(original[*index], *survivor);
            }
            if player == Survivor::Ellis {
                assert_eq!(
                    panels,
                    vec![
                        (0, Survivor::Rochelle),
                        (2, Survivor::Nick),
                        (1, Survivor::Coach)
                    ]
                );
            }
        }
    }
    #[test]
    fn selected_frame_has_clearance_above_and_below_at_all_hud_scales() {
        // Primary can be taller when reserve ammo is displayed. Either neighbour
        // may be selected; the outer glow must not cross the other frame.
        for scale in [0.5, 1., 1.5, 2., 3.] {
            let mut previous_top = 160.;
            let mut previous_height = 28.;
            for slot in 2..=5 {
                let top = inventory_top(slot);
                assert!(
                    (top - INVENTORY_GLOW_PAD - previous_top - previous_height) * scale
                        >= 1.99 * scale
                );
                assert!(
                    (top - previous_top - previous_height - INVENTORY_GLOW_PAD) * scale
                        >= 1.99 * scale
                );
                previous_top = top;
                previous_height = 24.;
            }
        }
    }
    #[derive(Resource, Default)]
    struct Invalidations(usize, usize);
    #[test]
    fn inventory_selection_changes_one_existing_frame_without_adding_rows() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let mut images = Assets::<Image>::default();
        let green = images.add(Image::default());
        let white = images.add(Image::default());
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.primary = Some(state.pistol.clone());
        world.insert_resource(state);
        world.insert_resource(HudImages(HashMap::from([
            (
                "hud/scalablepanel_bgmidgrey_outlinegreen_glow".into(),
                green.clone(),
            ),
            (
                "hud/scalablepanel_bgblack50_outlinewhite".into(),
                white.clone(),
            ),
        ])));
        let frames = [0, 2, 3, 4, 5].map(|slot| {
            world
                .spawn((InventoryFrame(slot), ImageNode::default()))
                .id()
        });
        let count = world.entities().len();
        for slot in 1..=5 {
            world.resource_mut::<Match>().slot = slot;
            world.run_system_once(update_frames).unwrap();
            assert_eq!(world.entities().len(), count);
            for (index, entity) in frames.iter().enumerate() {
                let expected = if index as u8 + 1 == slot {
                    &green
                } else {
                    &white
                };
                assert_eq!(&world.get::<ImageNode>(*entity).unwrap().image, expected);
            }
        }
    }
    #[test]
    fn primary_secondary_switching_and_death_keep_slots_and_ammo_consistent() {
        let pistol = crate::game_state::tests_weapon();
        let mut rifle = pistol.clone();
        rifle.class = "weapon_rifle".into();
        rifle.clip = 50;
        let mut state = Match::new(&pistol);
        state.primary = Some(crate::game_state::Gun {
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
            class: rifle.class.clone(),
            ammo: 50,
            reserve: 360,
        });
        state.slot = 1;
        let mut app = App::new();
        app.insert_resource(state)
            .insert_resource(Rules {
                pistol: pistol.clone(),
                weapons: HashMap::from([
                    (pistol.class.clone(), pistol),
                    (rifle.class.clone(), rifle),
                ]),
            })
            .insert_resource(Squad(vec![]))
            .insert_resource(HudImages(HashMap::new()))
            .init_resource::<NativeHints>()
            .init_resource::<Invalidations>()
            .add_systems(Update, (update, death_presentation).chain())
            .add_systems(
                PostUpdate,
                |nodes: Query<Entity, Changed<Node>>,
                 images: Query<Entity, Changed<ImageNode>>,
                 mut counts: ResMut<Invalidations>| {
                    counts.0 = nodes.iter().count();
                    counts.1 = images.iter().count();
                },
            );
        app.world_mut()
            .spawn((PrimaryIcon(2), ImageNode::default(), Node::default()));
        let health_fill = app
            .world_mut()
            .spawn((
                HealthFill(None, false),
                Node::default(),
                BackgroundColor::default(),
            ))
            .id();
        let parent = app.world_mut().spawn((Node::default(), WeaponPanel)).id();
        let primary = app
            .world_mut()
            .spawn((
                Node::default(),
                InventoryIcon(1),
                BackgroundColor::default(),
                BorderColor::default(),
            ))
            .id();
        let secondary = app
            .world_mut()
            .spawn((
                Node::default(),
                InventoryIcon(2),
                BackgroundColor::default(),
                BorderColor::default(),
            ))
            .id();
        let items = (3..=5)
            .map(|slot| {
                app.world_mut()
                    .spawn((
                        Node::default(),
                        InventoryIcon(slot),
                        BackgroundColor::default(),
                        BorderColor::default(),
                    ))
                    .id()
            })
            .collect::<Vec<_>>();
        let ammo = app
            .world_mut()
            .spawn((Text::default(), TextFont::default(), Label::Ammo))
            .id();
        let reserve = app
            .world_mut()
            .spawn((Text::default(), Label::Reserve))
            .id();
        let secondary_ammo = app
            .world_mut()
            .spawn((Text::default(), Label::SlotAmmo(2)))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Node>(primary).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(secondary).unwrap().display,
            Display::Flex
        );
        assert_eq!(app.world().get::<Text>(ammo).unwrap().0, "50");
        assert_eq!(app.world().get::<Text>(reserve).unwrap().0, "360");
        for (count, size) in [(100, 12.), (150, 12.), (50, 16.), (8, 16.)] {
            app.world_mut()
                .resource_mut::<Match>()
                .primary
                .as_mut()
                .unwrap()
                .ammo = count;
            app.update();
            assert_eq!(app.world().get::<Text>(ammo).unwrap().0, count.to_string());
            assert_eq!(
                app.world().get::<TextFont>(ammo).unwrap().font_size,
                FontSize::Vh(size / 4.8)
            );
        }
        app.world_mut()
            .resource_mut::<Match>()
            .primary
            .as_mut()
            .unwrap()
            .ammo = 50;
        app.update();
        assert_eq!(app.world().get::<Text>(secondary_ammo).unwrap().0, "15");
        app.update();
        let counts = app.world().resource::<Invalidations>();
        assert_eq!(
            (counts.0, counts.1),
            (0, 0),
            "An unchanged HUD must not invalidate layout/image sizing"
        );
        app.world_mut().resource_mut::<Match>().health = 75.;
        app.update();
        assert_eq!(
            app.world().get::<Node>(health_fill).unwrap().width,
            percent(75.)
        );
        app.world_mut().resource_mut::<Match>().pistol.ammo = 8;
        app.update();
        assert_eq!(app.world().get::<Text>(secondary_ammo).unwrap().0, "8");
        assert_eq!(app.world().get::<Text>(ammo).unwrap().0, "50");
        app.world_mut().resource_mut::<Match>().slot = 2;
        app.update();
        assert_eq!(
            app.world().get::<Node>(primary).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(secondary).unwrap().display,
            Display::Flex
        );
        assert_eq!(app.world().get::<Text>(reserve).unwrap().0, "360");
        let item_positions = items
            .iter()
            .map(|e| app.world().get::<Node>(*e).unwrap().top)
            .collect::<Vec<_>>();
        app.world_mut().resource_mut::<Match>().slot = 4;
        app.update();
        assert_eq!(
            app.world().get::<Node>(parent).unwrap().display,
            Display::Flex,
            "Item selection must retain both weapon rows"
        );
        assert_eq!(app.world().get::<Text>(ammo).unwrap().0, "50");
        assert_eq!(
            app.world().get::<Node>(primary).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(secondary).unwrap().display,
            Display::Flex
        );
        for (i, entity) in items.iter().enumerate() {
            assert_eq!(
                app.world().get::<Node>(*entity).unwrap().top,
                item_positions[i],
                "Equipping an item must preserve its existing position"
            );
        }
        let entity_count = app.world().entities().len();
        for slot in [3, 4, 5] {
            {
                let mut state = app.world_mut().resource_mut::<Match>();
                state.slot = slot;
                state.throwable = Some("weapon_pipe_bomb".into());
                state.medkit = true;
                state.pills = true;
            }
            app.update();
            assert_eq!(app.world().entities().len(), entity_count);
            assert_eq!(
                app.world().get::<Node>(parent).unwrap().display,
                Display::Flex
            );
            for (i, entity) in items.iter().enumerate() {
                let node = app.world().get::<Node>(*entity).unwrap();
                assert_eq!(
                    node.top, item_positions[i],
                    "Item selection must not add or move rows"
                );
                assert_eq!(
                    *app.world().get::<BorderColor>(*entity).unwrap(),
                    BorderColor::all(Color::NONE)
                );
            }
        }
        app.world_mut().resource_mut::<Match>().slot = 1;
        app.update();
        assert_eq!(
            app.world().get::<Node>(parent).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(primary).unwrap().display,
            Display::None
        );
        assert_eq!(app.world().get::<Text>(ammo).unwrap().0, "50");
        assert_eq!(app.world().get::<Text>(reserve).unwrap().0, "360");
        // Retail preserves the empty primary space when only a pistol is owned.
        app.world_mut().resource_mut::<Match>().primary = None;
        app.world_mut().resource_mut::<Match>().slot = 2;
        app.update();
        assert_eq!(app.world().get::<Node>(parent).unwrap().top, vh(189. / 4.8));
        for (i, entity) in items.iter().enumerate() {
            assert_eq!(
                app.world().get::<Node>(*entity).unwrap().top,
                item_positions[i]
            );
        }
        app.world_mut().resource_mut::<Match>().dead = true;
        app.update();
        for entity in [parent, primary, secondary] {
            assert_eq!(
                app.world().get::<Node>(entity).unwrap().display,
                Display::None
            );
        }
    }
}
/// Retail CHudPZDamageRecord death line for special infected kills (client
/// event handler 102b6d70, AddRecord 102b6320, fade 102b5b00; layout
/// scripts/hudlayout.res "HudPZDamageRecord" and
/// resource/UI/HUD/PZDamageRecordPanel.res):
/// - a survivor (the player or a bot) killing a special infected writes
///   "#L4D_KILLED" ("%s1 killed %s2") into recordlabel0 (x 10, y 170 units,
///   Default font, 246 5 5), over the label4background
///   ScalablePanel_bgBlack_outlineGrey sized to the text;
/// - Tanks and Witches are left out (hud_deathnotice_threats 0), bots are
///   shown (hud_deathnotice_bots 1); a newer kill replaces the line;
/// - the line stays hud_dmgrecord_persisttime_deaths (4 s), then fades over
///   0.5 s (alpha 255 x (1 - 2 x overtime)).
/// Outside versus (no "playercontrolledzombies") retail plays no sound and
/// shows no kill count with it: Hint.LittleReward is versus only.
pub mod kill_feed {
    use bevy::prelude::*;
    use std::sync::Mutex;
    pub const PERSIST: f32 = 4.;
    pub const FADE: f32 = 0.5;
    const RED: Color = Color::srgb(246. / 255., 5. / 255., 5. / 255.);
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Killer {
        Player,
        Bot(usize),
    }
    /// Who last hurt each infected, so its death can name the killer.
    static HITS: Mutex<Vec<(Entity, Killer)>> = Mutex::new(Vec::new());
    static KILLS: Mutex<Vec<(Killer, &'static str)>> = Mutex::new(Vec::new());
    pub fn hit(entity: Entity, who: Killer) {
        if let Ok(mut hits) = HITS.lock() {
            if let Some(h) = hits.iter_mut().find(|h| h.0 == entity) {
                h.1 = who;
            } else {
                if hits.len() >= 256 {
                    hits.remove(0);
                }
                hits.push((entity, who));
            }
        }
    }
    /// The survivor that last hurt `entity` (forgotten afterwards).
    pub fn killer(entity: Entity) -> Option<Killer> {
        let mut hits = HITS.lock().ok()?;
        let at = hits.iter().position(|h| h.0 == entity)?;
        Some(hits.remove(at).1)
    }
    /// A special infected `victim` ("Hunter") was killed by `who`.
    pub fn killed(who: Killer, victim: &'static str) {
        if let Ok(mut kills) = KILLS.lock() {
            kills.push((who, victim));
        }
    }
    /// Alpha of a line expiring at `expires` (game seconds) at `now`.
    pub fn alpha(now: f32, expires: f32) -> f32 {
        if now <= expires {
            1.
        } else {
            (1. - (now - expires) / FADE).clamp(0., 1.)
        }
    }
    /// Survivor display name from a squad model ("survivor_producer" -> Rochelle).
    pub fn display_name(model: &str) -> &'static str {
        let character = model
            .trim_start_matches("models/survivors/survivor_")
            .trim_end_matches(".mdl");
        match character {
            "producer" => "Rochelle",
            "coach" => "Coach",
            "mechanic" => "Ellis",
            "gambler" => "Nick",
            "namvet" => "Bill",
            "teenangst" => "Zoey",
            "biker" => "Francis",
            "manager" => "Louis",
            _ => "Survivor",
        }
    }
    /// The local player's name: retail shows the Steam persona, read here
    /// from the most recent login in Steam's config/loginusers.vdf.
    pub fn persona(game_root: &std::path::Path) -> Option<String> {
        let steam = game_root.ancestors().nth(3)?;
        let text = std::fs::read_to_string(steam.join("config/loginusers.vdf")).ok()?;
        let mut first = None;
        let mut current: Option<String> = None;
        for line in text.lines() {
            let fields: Vec<&str> = line.split('"').filter(|f| !f.trim().is_empty()).collect();
            match fields.as_slice() {
                [key, value] if key.eq_ignore_ascii_case("PersonaName") => {
                    current = Some(value.to_string());
                    first.get_or_insert_with(|| value.to_string());
                }
                [key, "1"] if key.eq_ignore_ascii_case("MostRecent") => {
                    if let Some(name) = current.clone() {
                        return Some(name);
                    }
                }
                [_] if line.trim_start().starts_with('"') => current = None,
                _ => {}
            }
        }
        first
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn kill_line_holds_four_seconds_then_fades_in_half_a_second() {
            assert_eq!(alpha(3.9, 4.), 1.);
            assert!((alpha(4.25, 4.) - 0.5).abs() < 1e-5);
            assert_eq!(alpha(4.5, 4.), 0.);
        }
        #[test]
        fn the_last_survivor_to_hurt_an_infected_is_its_killer() {
            let entity = Entity::from_raw_u32(4242).unwrap();
            hit(entity, Killer::Bot(1));
            hit(entity, Killer::Player);
            assert_eq!(killer(entity), Some(Killer::Player));
            assert_eq!(killer(entity), None);
            assert_eq!(display_name("models/survivors/survivor_producer.mdl"), "Rochelle");
        }
    }
    #[derive(Component)]
    pub struct Line;
    #[derive(Component)]
    pub struct Background;
    #[derive(Resource)]
    pub struct PlayerName(pub String);
    pub(super) fn spawn(commands: &mut Commands, background: Option<Handle<Image>>, font: &Handle<Font>) {
        // PZDamageRecordPanel.res label4background: 16-pixel source corners
        // drawn 8 units.
        if let Some(image) = background {
            commands.spawn((
                ImageNode {
                    image_mode: bevy::ui::widget::NodeImageMode::Sliced(bevy::sprite::TextureSlicer {
                        border: bevy::sprite::BorderRect::all(16.),
                        center_scale_mode: bevy::sprite::SliceScaleMode::Stretch,
                        sides_scale_mode: bevy::sprite::SliceScaleMode::Stretch,
                        max_corner_scale: 0.75,
                    }),
                    ..ImageNode::new(image)
                },
                Node {
                    display: Display::None,
                    ..super::hud_rect(2.7, 170., 0., 15.)
                },
                Background,
            ));
        }
        commands.spawn((
            Text::new(""),
            // Default (Stubble bold, tall 12): the vfont's em runs 15% larger
            // than Source's cell height, measured against retail captures.
            super::hud_font(font, 10.5),
            TextColor(RED),
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                left: vh(10. / 4.8),
                top: vh(172.3 / 4.8),
                ..default()
            },
            Line,
        ));
    }
    #[allow(clippy::type_complexity)]
    pub fn update(
        time: Res<Time>,
        squad: Res<crate::game_state::Squad>,
        session: Option<Res<crate::campaign_settings::SessionSettings>>,
        name: Option<Res<PlayerName>>,
        windows: Query<&Window>,
        mut shown: Local<Option<f32>>,
        mut lines: Query<(&mut Text, &mut TextColor, &mut Node, &ComputedNode), With<Line>>,
        mut backgrounds: Query<(&mut ImageNode, &mut Node), (With<Background>, Without<Line>)>,
    ) {
        let now = time.elapsed_secs();
        let kills = KILLS.lock().map(|mut k| std::mem::take(&mut *k)).unwrap_or_default();
        let Ok((mut text, mut colour, mut node, computed)) = lines.single_mut() else {
            return;
        };
        if let Some((who, victim)) = kills.last() {
            let attacker = match who {
                Killer::Player => name.as_ref().map(|n| n.0.clone()).unwrap_or_else(|| {
                    session.as_ref().map_or("Survivor", |s| s.survivor.name()).to_string()
                }),
                Killer::Bot(i) => squad.0.get(*i).map_or("Survivor", |m| display_name(&m.model)).to_string(),
            };
            text.0 = format!("{attacker} killed {victim}");
            *shown = Some(now + PERSIST);
        }
        let alpha = shown.map_or(0., |expires| alpha(now, expires));
        if alpha <= 0. {
            *shown = None;
        }
        let display = if alpha > 0. { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
        if colour.0.alpha() != alpha {
            colour.0 = RED.with_alpha(alpha);
        }
        // The background spans the text plus 16/640 of the screen (AddRecord
        // 102b6320), measured 9.5 pixels either side at 720p.
        let unit = windows.iter().next().map_or(1., |w| w.height() / 720.);
        let width = computed.size().x * computed.inverse_scale_factor();
        for (mut image, mut bg) in &mut backgrounds {
            if bg.display != display {
                bg.display = display;
            }
            let w = px(width + 20.5 * unit);
            if bg.width != w {
                bg.width = w;
            }
            if image.color.alpha() != alpha {
                image.color = Color::WHITE.with_alpha(alpha);
            }
        }
    }
}

/// One font asset per distinct font file for the whole session. HUD, menu,
/// scoreboard and loading screen loaded their fonts again on every map, and
/// each new font got its own glyph atlases (512x512 textures with a CPU
/// copy) that were never freed: ~37 MiB per map change, part of the iPhone
/// Hotel -> menu -> motel memory kill.
pub(crate) fn shared_font(fonts: &mut Assets<Font>, bytes: Vec<u8>) -> Handle<Font> {
    use std::hash::{Hash, Hasher};
    static FONTS: std::sync::Mutex<Vec<(u64, usize, Handle<Font>)>> = std::sync::Mutex::new(Vec::new());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let key = (hasher.finish(), bytes.len());
    let mut cache = FONTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, _, handle)) = cache.iter().find(|(h, n, handle)| (*h, *n) == key && fonts.contains(handle.id())) {
        return handle.clone();
    }
    let handle = fonts.add(Font::from_bytes(bytes));
    cache.retain(|(h, n, _)| (*h, *n) != key);
    cache.push((key.0, key.1, handle.clone()));
    handle
}
