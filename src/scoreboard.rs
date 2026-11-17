//! Hold-Tab player board, laid out in the native 480-line PC VGUI space.
//! Local character portraits replace Steam avatars; networking is not simulated.
use crate::{
    campaign_settings::{Catalog, SessionSettings, Survivor},
    game_state::{Match, Squad},
    source_assets::SourceAssets,
    special_infected::SpecialClass,
};
use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Component)]
struct Board;
#[derive(Component)]
struct SurvivorRow(usize);
#[derive(Component)]
struct InfectedRow(usize);
#[derive(Component)]
struct Fill(usize);
#[derive(Component)]
struct Item {
    row: usize,
    slot: usize,
}
#[derive(Component)]
enum Caption {
    Name(usize),
    Status(usize),
    InfectedName(usize),
    Score(usize),
}
#[derive(Resource)]
struct Artwork {
    images: HashMap<String, Handle<Image>>,
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: vh(x / 4.8),
        top: vh(y / 4.8),
        width: vh(w / 4.8),
        height: vh(h / 4.8),
        ..default()
    }
}
fn text(
    commands: &mut Commands,
    parent: Entity,
    font: &Handle<Font>,
    value: &str,
    node: Node,
    size: f32,
) -> Entity {
    commands
        .spawn((
            node,
            Text::new(value),
            TextFont {
                font: bevy::text::FontSource::Handle(font.clone()),
                font_size: FontSize::Vh(size / 4.8),
                ..default()
            },
            TextColor(Color::WHITE),
            ChildOf(parent),
        ))
        .id()
}
fn art(commands: &mut Commands, parent: Entity, handle: Handle<Image>, node: Node) -> Entity {
    commands
        .spawn((
            node,
            ImageNode::new(handle).with_mode(bevy::ui::widget::NodeImageMode::Stretch),
            ChildOf(parent),
        ))
        .id()
}
fn roster(session: &SessionSettings) -> Vec<Survivor> {
    let order = if session.original {
        Survivor::roster(true)
    } else {
        [
            Survivor::Nick,
            Survivor::Rochelle,
            Survivor::Coach,
            Survivor::Ellis,
        ]
    };
    std::iter::once(session.survivor)
        .chain(order.into_iter().filter(|s| *s != session.survivor))
        .collect()
}
fn health_fraction(state: Option<&Match>) -> f32 {
    state.map_or(0., |s| {
        if s.dead {
            0.
        } else if s.incap {
            (s.incap_health / 300.).clamp(0., 1.)
        } else {
            ((s.health + s.temporary) / 100.).clamp(0., 1.)
        }
    })
}
fn show(held: bool, focused: bool, playing: bool, console: bool, cinematic: bool) -> bool {
    held && focused && playing && !console && !cinematic
}
pub fn install(app: &mut App) {
    app.add_systems(Startup, setup.before(crate::game_ui::target_camera))
        .add_systems(Update, update.after(crate::game_menu::update));
}
pub(crate) fn setup(
    mut commands: Commands,
    source: Res<crate::game_ui::HudSource>,
    session: Res<SessionSettings>,
    catalog: Res<Catalog>,
    mut images: ResMut<Assets<Image>>,
    mut fonts: ResMut<Assets<Font>>,
) {
    let Ok(mut assets) = SourceAssets::new(&source.0, vec![]) else {
        return;
    };
    let font = assets
        .read("resource/stubble-bold.vfont")
        .and_then(|b| crate::vfont::decode(&b))
        .map(|b| crate::game_ui::shared_font(&mut *fonts, b))
        .unwrap_or_default();
    let mut handles = HashMap::new();
    for key in [
        "scoreboardbg",
        "hud/healthbar_withglow_white",
        "hud/icon_first_aid_kit",
        "hud/icon_painpills",
        "hud/icon_molotov",
        "hud/icon_pipebomb",
        "hud/icon_boomer_bile",
        "hud/icon_adrenaline",
    ] {
        let path = format!("materials/vgui/{key}.vtf");
        if assets.read(&path).is_ok() {
            handles.insert(
                key.into(),
                crate::game_menu::image(&mut assets, &mut images, &path),
            );
        }
    }
    for s in Survivor::roster(session.original) {
        let key = format!("s_panel_{}", s.character());
        handles.insert(
            key.clone(),
            crate::game_menu::image(
                &mut assets,
                &mut images,
                &format!("materials/vgui/{key}.vtf"),
            ),
        );
    }
    for role in SpecialClass::ALL {
        let path = format!("materials/vgui/hud/zombieteamimage_{}.vtf", role.name());
        if assets.read(&path).is_ok() {
            handles.insert(
                format!("infected_{}", role.name()),
                crate::game_menu::image(&mut assets, &mut images, &path),
            );
        }
    }
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: percent(100.),
                height: percent(100.),
                display: Display::None,
                ..default()
            },
            GlobalZIndex(200),
            Board,
        ))
        .id();
    let panel = commands
        .spawn((
            rect(0., 0., 340., 480.),
            BackgroundColor(Color::srgba(0., 0., 0., 0.90)),
            ChildOf(root),
        ))
        .id();
    if let Some(bg) = handles.get("scoreboardbg") {
        art(&mut commands, panel, bg.clone(), rect(0., 0., 340., 480.));
    }
    let campaign = catalog.campaigns.iter().find(|c| {
        c.chapters
            .iter()
            .any(|chapter| chapter.map.file_stem() == session.map.file_stem())
    });
    let title = campaign.map_or_else(
        || {
            session
                .map
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .replace('_', " ")
        },
        |c| c.name.clone(),
    );
    text(
        &mut commands,
        panel,
        &font,
        &format!("{title}, {}", session.difficulty.name()),
        rect(20., 13., 310., 24.),
        18.,
    );
    if let Some(campaign) = campaign {
        let current = campaign
            .chapters
            .iter()
            .position(|c| c.map.file_stem() == session.map.file_stem())
            .unwrap_or(0);
        for (i, chapter) in campaign.chapters.iter().take(5).enumerate() {
            let path = format!(
                "materials/vgui/{}.vtf",
                chapter
                    .image
                    .trim_start_matches("vgui/")
                    .trim_end_matches(".vtf")
            );
            let image = crate::game_menu::image(&mut assets, &mut images, &path);
            let id = art(
                &mut commands,
                panel,
                image.clone(),
                rect(20. + i as f32 * 62., 90., 60., 30.),
            );
            if i < current {
                commands.entity(id).insert(ImageNode {
                    color: Color::srgb(0.4, 0.4, 0.4),
                    ..ImageNode::new(image).with_mode(bevy::ui::widget::NodeImageMode::Stretch)
                });
            }
            let caption = if i == current {
                Some("Now Playing")
            } else if i + 1 == campaign.chapters.len() {
                Some("Rescue")
            } else {
                None
            };
            if let Some(caption) = caption {
                // Bitmap-like arrow avoids a missing Unicode glyph when the
                // installed Stubble font has no triangle character.
                for line in 0..5 {
                    let width = 1. + line as f32 * 2.;
                    commands.spawn((
                        rect(
                            50. + i as f32 * 62. - width / 2.,
                            116. + line as f32,
                            width,
                            1.,
                        ),
                        BackgroundColor(Color::srgb_u8(192, 192, 192)),
                        ChildOf(panel),
                    ));
                }
                let id = text(
                    &mut commands,
                    panel,
                    &font,
                    caption,
                    rect(10. + i as f32 * 62., 120., 80., 13.),
                    10.,
                );
                commands
                    .entity(id)
                    .insert(TextLayout::default().with_justify(Justify::Center));
            }
        }
    }
    for (i, survivor) in roster(&session).into_iter().enumerate() {
        let row = commands
            .spawn((
                rect(20., 160. + i as f32 * 30., 300., 28.),
                BackgroundColor(Color::srgb_u8(129, 114, 89)),
                SurvivorRow(i),
                ChildOf(panel),
            ))
            .id();
        art(
            &mut commands,
            row,
            handles[&format!("s_panel_{}", survivor.character())].clone(),
            rect(2., 2., 24., 24.),
        );
        let id = text(
            &mut commands,
            row,
            &font,
            survivor.name(),
            rect(30., 4., 220., 16.),
            12.,
        );
        commands.entity(id).insert(Caption::Name(i));
        let status = text(
            &mut commands,
            row,
            &font,
            "",
            rect(175., 12., 75., 14.),
            11.,
        );
        commands.entity(status).insert(Caption::Status(i));
        commands.spawn((
            rect(30., 19., 96., 6.),
            BackgroundColor(Color::srgba(1., 1., 1., 0.1)),
            ChildOf(row),
        ));
        let clip = commands
            .spawn((
                Node {
                    overflow: Overflow::clip(),
                    ..rect(30., 19., 96., 7.)
                },
                Fill(i),
                ChildOf(row),
            ))
            .id();
        if let Some(bar) = handles.get("hud/healthbar_withglow_white") {
            let image = art(&mut commands, clip, bar.clone(), rect(0., 0., 96., 7.));
            commands.entity(image).insert(ImageNode {
                color: Color::srgb_u8(192, 192, 192),
                ..ImageNode::new(bar.clone()).with_mode(bevy::ui::widget::NodeImageMode::Stretch)
            });
        }
        for slot in 0..3 {
            commands.spawn((
                rect(130. + slot as f32 * 14., 14., 12., 12.),
                ImageNode::default(),
                Item { row: i, slot },
                ChildOf(row),
            ));
        }
        if i == 0 {
            signal(&mut commands, row, 255., 6., true);
            text(
                &mut commands,
                row,
                &font,
                "0",
                rect(274., 6., 25., 16.),
                12.,
            );
        }
    }
    for i in 0..4 {
        let row = commands
            .spawn((
                rect(20., 290. + i as f32 * 20., 300., 18.),
                BackgroundColor(Color::srgb_u8(35, 35, 35)),
                InfectedRow(i),
                ChildOf(panel),
            ))
            .id();
        let id = text(
            &mut commands,
            row,
            &font,
            "—",
            rect(20., 3., 132., 15.),
            12.,
        );
        commands.entity(id).insert(Caption::InfectedName(i));
        let score = text(
            &mut commands,
            row,
            &font,
            "—",
            rect(150., 0., 78., 18.),
            18.,
        );
        commands.entity(score).insert(Caption::Score(i));
        commands.spawn((
            rect(2., 1., 16., 16.),
            ImageNode::default(),
            Item { row: i, slot: 3 },
            ChildOf(row),
        ));
        let signal = signal(&mut commands, row, 255., 1., false);
        commands.entity(signal).insert(Item { row: i, slot: 4 });
    }
    commands.insert_resource(Artwork { images: handles });
}
fn signal(commands: &mut Commands, parent: Entity, x: f32, y: f32, background: bool) -> Entity {
    let root = commands
        .spawn((
            rect(x, y, 16., 16.),
            BackgroundColor(if background {
                Color::srgb_u8(28, 31, 24)
            } else {
                Color::NONE
            }),
            ChildOf(parent),
        ))
        .id();
    for i in 0..3 {
        commands.spawn((
            rect(
                3. + i as f32 * 3.,
                11. - i as f32 * 3.,
                2.,
                4. + i as f32 * 3.,
            ),
            BackgroundColor(Color::srgb_u8(71, 130, 48)),
            ChildOf(root),
        ));
    }
    root
}
#[allow(clippy::too_many_arguments)]
fn update(
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    state: Res<Match>,
    squad: Res<Squad>,
    session: Res<SessionSettings>,
    frontend: Res<crate::game_menu::Frontend>,
    console: Res<crate::game_console::Console>,
    cinematic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    artwork: Option<Res<Artwork>>,
    enemies: Query<&crate::game::Enemy>,
    mut nodes: Query<
        (
            &mut Node,
            Option<&Board>,
            Option<&SurvivorRow>,
            Option<&Fill>,
            Option<&Item>,
            Option<&InfectedRow>,
            Option<&mut ImageNode>,
            Option<&mut BackgroundColor>,
        ),
        Or<(
            With<Board>,
            With<SurvivorRow>,
            With<Fill>,
            With<Item>,
            With<InfectedRow>,
        )>,
    >,
    mut labels: Query<(&Caption, &mut Text)>,
) {
    static CAPTURE: std::sync::OnceLock<(bool, bool)> = std::sync::OnceLock::new();
    let (capture, capture_infected) = *CAPTURE.get_or_init(|| {
        let infected = std::env::args().any(|a| a == "--capture-scoreboard-infected");
        (
            infected || std::env::args().any(|a| a == "--capture-scoreboard"),
            infected,
        )
    });
    let visible = show(
        keys.pressed(KeyCode::Tab) || capture,
        capture || windows.iter().any(|w| w.focused),
        frontend.playing() && !state.paused,
        console.open,
        cinematic.as_ref().is_some_and(|c| c.active),
    );
    // The closed board does no roster scans, string building or asset work.
    if !visible {
        for (mut node, root, ..) in &mut nodes {
            if root.is_some() && node.display != Display::None {
                node.display = Display::None;
            }
        }
        return;
    }
    let roles = roster(&session);
    let survivors: Vec<_> = roles
        .iter()
        .map(|role| {
            if *role == session.survivor && !state.infected {
                Some(&*state)
            } else {
                squad
                    .0
                    .iter()
                    .find(|m| {
                        m.model
                            .ends_with(&format!("survivor_{}.mdl", role.character()))
                    })
                    .map(|m| &m.state)
            }
        })
        .collect();
    let teams = state.infected || matches!(frontend.mode, 3 | 4 | 6 | 7) || capture_infected;
    let mut infected = vec![];
    if state.infected {
        infected.push(state.special_class);
    }
    if teams {
        for e in &enemies {
            if e.health > 0. {
                if let Some(role) = SpecialClass::ALL
                    .into_iter()
                    .find(|role| role.model() == e.model)
                {
                    if infected.len() < 4 {
                        infected.push(role);
                    }
                }
            }
        }
    }
    if capture_infected {
        infected = vec![
            SpecialClass::Hunter,
            SpecialClass::Smoker,
            SpecialClass::Boomer,
            SpecialClass::Spitter,
        ];
    }
    for (mut node, root, row, fill, item, infected_row, image, color) in &mut nodes {
        if root.is_some() {
            let display = if visible {
                Display::Flex
            } else {
                Display::None
            };
            if node.display != display {
                node.display = display;
            }
            continue;
        }
        if !visible {
            continue;
        }
        if let Some(row) = row {
            if let Some(mut color) = color {
                let selected = row.0 == 0 && !state.infected;
                let next = if selected {
                    Color::srgb_u8(143, 50, 19)
                } else {
                    Color::srgb_u8(129, 114, 89)
                };
                if color.0 != next {
                    color.0 = next;
                }
            }
        }
        if let Some(fill) = fill {
            let width = vh(96. * health_fraction(survivors[fill.0]) / 4.8);
            if node.width != width {
                node.width = width;
            }
        }
        if let Some(item) = item {
            if item.slot >= 3 {
                let handle = infected.get(item.row).and_then(|role| {
                    artwork
                        .as_ref()
                        .and_then(|a| a.images.get(&format!("infected_{}", role.name())))
                });
                let display = if (item.slot == 4 && item.row < infected.len()) || handle.is_some() {
                    Display::Flex
                } else {
                    Display::None
                };
                if node.display != display {
                    node.display = display;
                }
                if let (Some(handle), Some(mut image)) = (handle, image) {
                    if image.image != *handle {
                        image.image = handle.clone();
                    }
                }
                continue;
            }
            // Dead survivors carry nothing.
            let key = survivors[item.row].filter(|s| !s.dead).and_then(|s| match item.slot {
                0 => s.throwable.as_deref().map(|name| {
                    if name.contains("molotov") {
                        "hud/icon_molotov"
                    } else if name.contains("vomit") {
                        "hud/icon_boomer_bile"
                    } else {
                        "hud/icon_pipebomb"
                    }
                }),
                1 => s.medkit.then_some("hud/icon_first_aid_kit"),
                _ => {
                    if s.adrenaline {
                        Some("hud/icon_adrenaline")
                    } else {
                        s.pills.then_some("hud/icon_painpills")
                    }
                }
            });
            let handle = key.and_then(|key| artwork.as_ref().and_then(|a| a.images.get(key)));
            let display = if handle.is_some() {
                Display::Flex
            } else {
                Display::None
            };
            if node.display != display {
                node.display = display;
            }
            if let (Some(handle), Some(mut image)) = (handle, image) {
                if image.image != *handle {
                    image.image = handle.clone();
                }
                let crop = Some(Rect::from_corners(Vec2::ZERO, Vec2::splat(64.)));
                if image.rect != crop {
                    image.rect = crop;
                    image.image_mode = bevy::ui::widget::NodeImageMode::Stretch;
                }
            }
        }
        if infected_row.is_some() {
            let display = if teams { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
        }
    }
    if !visible {
        return;
    }
    for (caption, mut text) in &mut labels {
        let next = match *caption {
            // Online: the players' own names (bots keep the survivor's).
            Caption::Name(i) if crate::net_flow::online() => {
                if roles[i] == session.survivor && !state.infected {
                    crate::game_chat::local_name()
                } else {
                    crate::net_play::player_of(roles[i].name()).unwrap_or_else(|| roles[i].name().to_owned())
                }
            }
            Caption::Name(i) => roles[i].name().to_owned(),
            Caption::Status(i) => survivors[i]
                .map_or("—", |s| {
                    if s.dead {
                        "DEAD"
                    } else if s.incap {
                        "DOWN"
                    } else {
                        ""
                    }
                })
                .to_owned(),
            Caption::InfectedName(i) => infected.get(i).map_or("—".into(), |role| {
                let mut name = role.name().to_owned();
                name[..1].make_ascii_uppercase();
                name
            }),
            Caption::Score(i) => if i < infected.len() { "0" } else { "—" }.into(),
        };
        if text.0 != next {
            text.0 = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hold_tab_never_overlays_console_pause_cinematic_or_lost_focus() {
        assert!(show(true, true, true, false, false));
        assert!(!show(false, true, true, false, false));
        assert!(!show(true, false, true, false, false));
        assert!(!show(true, true, false, false, false));
        assert!(!show(true, true, true, true, false));
        assert!(!show(true, true, true, false, true));
    }
    #[test]
    fn health_clips_overheal_temporary_incap_and_dead_values() {
        let mut s = Match::new(&crate::game_state::tests_weapon());
        s.health = 90.;
        s.temporary = 40.;
        assert_eq!(health_fraction(Some(&s)), 1.);
        s.incap = true;
        s.incap_health = 150.;
        assert_eq!(health_fraction(Some(&s)), 0.5);
        s.dead = true;
        assert_eq!(health_fraction(Some(&s)), 0.);
        assert_eq!(health_fraction(None), 0.);
    }
}
