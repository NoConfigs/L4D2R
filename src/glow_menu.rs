//! OPTIONS > GLOWS: edit every retail glow colour (`cl_glow_*` and
//! `cl_witch_glow_*`, 0..1 floats like retail) with live RGB sliders and a
//! preview that shows what a survivor or an infected player sees. Changes
//! apply to the running game at once (crate::survivor_glow reads
//! GlowSettings every frame) and persist in runtime/glows.cfg.
//!
//! Presets are retail .cfg files in runtime/glow-presets/ (`cl_glow_item_r
//! "0.70"` lines), so a preset can be copied into retail's cfg folder as is
//! and any glow cfg dropped into that folder shows up as a preset. EXPORT TO
//! RETAIL writes left4dead2/cfg/l4d2r_glows.cfg and adds `exec l4d2r_glows`
//! to autoexec.cfg; IMPORT FROM RETAIL turns every retail cfg that sets glow
//! colours into a preset.
use crate::glow_settings::GlowSettings;
use bevy::{input::mouse::MouseButton, prelude::*, ui::RelativeCursorPosition};

const SAVE_FILE: &str = "runtime/glows.cfg";
const PRESET_DIR: &str = "runtime/glow-presets";
const RETAIL_FILE: &str = "l4d2r_glows";

/// Every editable colour: (cvar prefix, label).
pub const COLOURS: &[(&str, &str)] = &[
    ("cl_glow_survivor", "Survivor teammate"),
    ("cl_glow_survivor_hurt", "Survivor teammate hurt"),
    ("cl_glow_survivor_vomit", "Survivor vomited on"),
    ("cl_glow_item", "Item"),
    ("cl_glow_item_far", "Item far away"),
    ("cl_glow_thirdstrike_item", "Item (last life)"),
    ("cl_glow_infected", "Infected teammate"),
    ("cl_glow_ghost_infected", "Ghost infected"),
    ("cl_glow_infected_vomit", "Infected vomited on"),
    ("cl_glow_ability", "Ability / pinned"),
    ("cl_witch_glow_idle", "Witch idle"),
    ("cl_witch_glow_angry", "Witch angry"),
    ("cl_glow_survivor_health_high", "Survivor health high"),
    ("cl_glow_survivor_health_med", "Survivor health medium"),
    ("cl_glow_survivor_health_low", "Survivor health low"),
    ("cl_glow_survivor_health_crit", "Survivor health critical"),
    ("cl_glow_survivor_health_high_colorblind", "Health high (colourblind)"),
    ("cl_glow_survivor_health_med_colorblind", "Health medium (colourblind)"),
    ("cl_glow_survivor_health_low_colorblind", "Health low (colourblind)"),
    ("cl_glow_survivor_health_crit_colorblind", "Health critical (colourblind)"),
    ("cl_glow_thirdstrike_item_colorblind", "Last life item (colourblind)"),
    ("cl_glow_ability_colorblind", "Ability (colourblind)"),
];

/// The cvars a glow preset holds: every colour channel.
fn preset_keys() -> Vec<String> {
    COLOURS.iter().flat_map(|(p, _)| ["r", "g", "b"].map(|c| format!("{p}_{c}"))).collect()
}

/// A retail cfg of the current colours.
pub fn to_cfg(settings: &GlowSettings) -> String {
    let mut out = String::from("// Glow colours (L4D2 Rewrite OPTIONS > GLOWS). Works in retail: exec this file.\n");
    for key in preset_keys() {
        out.push_str(&format!("{key} \"{}\"\n", trim(settings.get(&key, 0.))));
    }
    out
}

fn trim(v: f32) -> String {
    let s = format!("{:.3}", v.clamp(0., 1.));
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { "0".into() } else { s.to_string() }
}

/// The glow colour values a cfg sets (other commands are ignored).
pub fn parse_cfg(text: &str) -> Vec<(String, f32)> {
    let keys = preset_keys();
    let mut out = vec![];
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("");
        for command in line.split(';') {
            let mut words = command.split_whitespace().map(|w| w.trim_matches('"'));
            let (Some(name), Some(value)) = (words.next(), words.next()) else { continue };
            let name = name.to_ascii_lowercase();
            if !keys.contains(&name) {
                continue;
            }
            if let Ok(v) = value.parse::<f32>() {
                if v.is_finite() {
                    out.push((name, v.clamp(0., 1.)));
                }
            }
        }
    }
    out
}

fn load_saved(settings: &mut GlowSettings) {
    if let Ok(text) = std::fs::read_to_string(SAVE_FILE) {
        for (k, v) in parse_cfg(&text) {
            settings.values.insert(k, v);
        }
    }
}

#[derive(Resource)]
struct State {
    selected: usize,
    infected_view: bool,
    dirty_presets: bool,
    status: String,
    save_at: Option<f64>,
    dragging: Option<usize>,
}
impl Default for State {
    fn default() -> Self {
        Self { selected: 0, infected_view: false, dirty_presets: true, status: String::new(), save_at: None, dragging: None }
    }
}

#[derive(Component)]
struct Root;
#[derive(Component)]
struct Category(usize);
#[derive(Component)]
struct Chip(usize);
#[derive(Component)]
struct CategoryLabel(usize);
#[derive(Component)]
struct Track(usize);
#[derive(Component)]
struct Fill(usize);
#[derive(Component)]
struct Value(usize);
#[derive(Component)]
struct Step(usize, i32);
#[derive(Component)]
struct Swatch;
#[derive(Component)]
struct SelectedName;
#[derive(Component)]
struct Status;
#[derive(Component)]
struct PresetList;
#[derive(Component)]
struct PresetRow;
#[derive(Component, Clone, Copy, PartialEq)]
enum Act {
    Done,
    ResetOne,
    ResetAll,
    SavePreset,
    Import,
    Export,
    OpenFolder,
    View(bool),
    Load(usize),
    Delete(usize),
}
/// A preview outline: which colour it shows in which view (None: hidden).
#[derive(Component)]
struct Glow {
    survivor: Option<&'static str>,
    infected: Option<&'static str>,
}
#[derive(Component)]
struct GlowLabel {
    survivor: &'static str,
    infected: &'static str,
}

pub fn install(app: &mut App) {
    let mut settings = GlowSettings::default();
    load_saved(&mut settings);
    app.insert_resource(settings)
        .init_resource::<State>()
        .add_systems(Update, (setup, visibility, interact, sliders, paint, presets, persist).chain());
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Node {
    Node { position_type: PositionType::Absolute, left: vh(x / 7.2), top: vh(y / 7.2), width: vh(w / 7.2), height: vh(h / 7.2), ..default() }
}
fn text(commands: &mut Commands, parent: Entity, font: &Handle<Font>, s: &str, size: f32, node: Node) -> Entity {
    commands
        .spawn((
            Text::new(s),
            TextFont { font: bevy::text::FontSource::Handle(font.clone()), font_size: FontSize::Vh(size / 7.2), ..default() },
            TextColor(Color::srgb_u8(200, 200, 200)),
            node,
            ChildOf(parent),
        ))
        .id()
}
fn button(commands: &mut Commands, parent: Entity, font: &Handle<Font>, label: &str, node: Node, act: Act) -> Entity {
    let b = commands
        .spawn((Button, act, node, BackgroundColor(Color::srgba(1., 1., 1., 0.06)), BorderColor::all(Color::srgba(1., 1., 1., 0.25)), ChildOf(parent)))
        .id();
    commands.entity(b).entry::<Node>().and_modify(|mut n| {
        n.border = UiRect::all(Val::Px(1.));
        n.justify_content = JustifyContent::Center;
        n.align_items = AlignItems::Center;
    });
    commands.spawn((
        Text::new(label),
        TextFont { font: bevy::text::FontSource::Handle(font.clone()), font_size: FontSize::Vh(15. / 7.2), ..default() },
        TextColor(Color::srgb_u8(210, 210, 210)),
        ChildOf(b),
    ));
    b
}

fn menu_font(source: &crate::game_ui::HudSource, fonts: &mut Assets<Font>) -> Handle<Font> {
    crate::source_assets::SourceAssets::new(&source.0, vec![])
        .ok()
        .and_then(|mut a| a.read("resource/stubble-bold.vfont").ok())
        .and_then(|b| crate::vfont::decode(&b).ok())
        .map(|b| crate::game_ui::shared_font(fonts, b))
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    roots: Query<(), With<Root>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    mut fonts: ResMut<Assets<Font>>,
    camera: Query<Entity, With<crate::game::ViewCamera>>,
) {
    if !roots.is_empty() {
        return;
    }
    let Some(source) = source else { return };
    // The menu's camera, as game_menu's root.
    let Some(camera) = camera.iter().next() else { return };
    let font = menu_font(&source, &mut fonts);
    let root = commands
        .spawn((
            Root,
            Node { position_type: PositionType::Absolute, width: percent(100.), height: percent(100.), display: Display::None, ..default() },
            GlobalZIndex(60),
            UiTargetCamera(camera),
        ))
        .id();
    commands.spawn((Node { width: percent(100.), ..rect(0., 0., 1280., 85.) }, BackgroundColor(Color::srgba(0., 0., 0., 0.78)), ChildOf(root)));
    let title = text(&mut commands, root, &font, "GLOWS", 32., rect(130., 5., 1000., 45.));
    commands.entity(title).insert(TextColor(Color::WHITE));
    text(&mut commands, root, &font, "Customise glow outline colours. Changes apply live and save automatically.", 17., rect(130., 48., 1000., 27.));
    commands.spawn((Node { width: percent(100.), ..rect(0., 100., 1280., 560.) }, BackgroundColor(Color::srgba(0., 0., 0., 0.72)), ChildOf(root)));

    // Colour list.
    for (i, (_, label)) in COLOURS.iter().enumerate() {
        let row = commands
            .spawn((Button, Category(i), rect(40., 108. + i as f32 * 24.5, 330., 23.), BackgroundColor(Color::NONE), ChildOf(root)))
            .id();
        commands.spawn((Chip(i), rect(4., 4., 15., 15.), BackgroundColor(Color::WHITE), BorderColor::all(Color::srgba(0., 0., 0., 0.8)), ChildOf(row)));
        let t = text(&mut commands, row, &font, label, 15., rect(28., 2., 300., 20.));
        commands.entity(t).insert(CategoryLabel(i));
    }

    // Editor.
    let name = text(&mut commands, root, &font, "", 22., rect(400., 112., 380., 30.));
    commands.entity(name).insert((SelectedName, TextColor(Color::WHITE)));
    commands.spawn((Swatch, rect(400., 150., 370., 44.), BackgroundColor(Color::WHITE), BorderColor::all(Color::srgba(1., 1., 1., 0.5)), ChildOf(root)));
    for (c, label) in ["R", "G", "B"].iter().enumerate() {
        let y = 212. + c as f32 * 44.;
        text(&mut commands, root, &font, label, 18., rect(400., y, 20., 24.));
        let step = button(&mut commands, root, &font, "<", rect(424., y, 24., 24.), Act::Done);
        commands.entity(step).remove::<Act>().insert(Step(c, -1));
        let track = commands
            .spawn((
                Button,
                Track(c),
                RelativeCursorPosition::default(),
                rect(454., y + 4., 230., 16.),
                BackgroundColor(Color::srgba(1., 1., 1., 0.12)),
                BorderColor::all(Color::srgba(1., 1., 1., 0.3)),
                ChildOf(root),
            ))
            .id();
        let tint = [Color::srgb(0.85, 0.2, 0.2), Color::srgb(0.2, 0.8, 0.25), Color::srgb(0.25, 0.4, 0.95)][c];
        commands.spawn((Fill(c), Node { position_type: PositionType::Absolute, left: px(0.), top: px(0.), height: percent(100.), width: percent(50.), ..default() }, BackgroundColor(tint), ChildOf(track)));
        let step = button(&mut commands, root, &font, ">", rect(690., y, 24., 24.), Act::Done);
        commands.entity(step).remove::<Act>().insert(Step(c, 1));
        let v = text(&mut commands, root, &font, "", 18., rect(722., y, 60., 24.));
        commands.entity(v).insert(Value(c));
    }
    button(&mut commands, root, &font, "RESET COLOUR", rect(400., 346., 180., 30.), Act::ResetOne);
    button(&mut commands, root, &font, "RESET ALL (RETAIL)", rect(590., 346., 180., 30.), Act::ResetAll);

    text(&mut commands, root, &font, "PRESETS", 20., rect(400., 392., 300., 26.));
    button(&mut commands, root, &font, "SAVE AS PRESET", rect(400., 420., 180., 30.), Act::SavePreset);
    button(&mut commands, root, &font, "OPEN FOLDER", rect(590., 420., 180., 30.), Act::OpenFolder);
    button(&mut commands, root, &font, "IMPORT FROM RETAIL", rect(400., 456., 180., 30.), Act::Import);
    button(&mut commands, root, &font, "EXPORT TO RETAIL", rect(590., 456., 180., 30.), Act::Export);
    commands.spawn((PresetList, rect(400., 494., 370., 160.), ChildOf(root)));

    // Preview.
    let preview = commands
        .spawn((rect(800., 108., 450., 546.), BackgroundColor(Color::srgb(0.05, 0.055, 0.06)), BorderColor::all(Color::srgba(1., 1., 1., 0.2)), ChildOf(root)))
        .id();
    commands.entity(preview).entry::<Node>().and_modify(|mut n| n.border = UiRect::all(Val::Px(1.)));
    text(&mut commands, preview, &font, "PREVIEW - VIEW AS:", 15., rect(14., 10., 200., 22.));
    button(&mut commands, preview, &font, "SURVIVOR", rect(190., 8., 110., 26.), Act::View(false));
    button(&mut commands, preview, &font, "INFECTED", rect(310., 8., 110., 26.), Act::View(true));
    // Three rows of four outlines; each slot names what it shows per view.
    let slots: [[(Option<&'static str>, &'static str, Option<&'static str>, &'static str, u8); 4]; 3] = [
        [
            (Some("cl_glow_survivor"), "Teammate", Some("cl_glow_survivor_health_high"), "Health high", 0),
            (Some("cl_glow_survivor_hurt"), "Hurt teammate", Some("cl_glow_survivor_health_med"), "Health medium", 0),
            (Some("cl_glow_survivor_vomit"), "Vomited on", Some("cl_glow_survivor_health_low"), "Health low", 0),
            (Some("cl_glow_ability"), "Pinned", Some("cl_glow_survivor_health_crit"), "Critical", 0),
        ],
        [
            (Some("cl_glow_item"), "Pistol", Some("cl_glow_survivor_vomit"), "Vomited on", 1),
            (Some("cl_glow_item"), "Medkit", Some("cl_glow_infected"), "Teammate", 2),
            (Some("cl_glow_item_far"), "Item far away", Some("cl_glow_ghost_infected"), "Ghost", 3),
            (Some("cl_glow_thirdstrike_item"), "Last life item", Some("cl_glow_ability"), "Ability", 4),
        ],
        [
            (Some("cl_glow_infected_vomit"), "Vomited infected", None, "", 5),
            (Some("cl_witch_glow_idle"), "Witch idle", None, "", 5),
            (Some("cl_witch_glow_angry"), "Witch angry", None, "", 5),
            (None, "", None, "", 0),
        ],
    ];
    for (row, slots) in slots.iter().enumerate() {
        for (col, &(survivor, s_label, infected, i_label, shape)) in slots.iter().enumerate() {
            if survivor.is_none() && infected.is_none() {
                continue;
            }
            let (x, y) = (14. + col as f32 * 108., 50. + row as f32 * 165.);
            let slot = commands.spawn((rect(x, y, 100., 158.), ChildOf(preview))).id();
            figure(&mut commands, slot, shape, Glow { survivor, infected });
            let t = text(&mut commands, slot, &font, s_label, 13., rect(0., 136., 100., 20.));
            commands.entity(t).insert(GlowLabel { survivor: s_label, infected: i_label }).entry::<Node>().and_modify(|mut n| n.justify_content = JustifyContent::Center);
        }
    }
    let status = text(&mut commands, root, &font, "", 15., rect(130., 668., 1000., 22.));
    commands.entity(status).insert(Status);
    button(&mut commands, root, &font, "DONE", rect(40., 664., 80., 30.), Act::Done);
}

/// Silhouette parts for a preview slot: 0 a standing person (in the
/// infected view a survivor too), 1 pistol, 2 medkit, 3 small far item,
/// 4 pill bottle, 5 a crouching person (witch / infected).
fn figure(commands: &mut Commands, slot: Entity, shape: u8, glow: Glow) {
    let parts: Vec<(f32, f32, f32, f32, f32)> = match shape {
        0 => vec![(38., 8., 24., 24., 12.), (30., 36., 40., 56., 8.), (32., 94., 14., 38., 5.), (54., 94., 14., 38., 5.)],
        1 => vec![(20., 60., 60., 16., 3.), (24., 74., 14., 26., 3.)],
        2 => vec![(25., 54., 50., 40., 6.)],
        3 => vec![(40., 80., 22., 14., 3.)],
        4 => vec![(42., 54., 18., 36., 5.)],
        _ => vec![(34., 40., 22., 22., 11.), (22., 64., 52., 40., 14.), (18., 104., 18., 26., 6.), (60., 104., 18., 26., 6.)],
    };
    let mut first = Some(glow);
    for (x, y, w, h, radius) in parts {
        let mut e = commands.spawn((
            Node { border_radius: BorderRadius::all(px(radius)), ..rect(x, y, w, h) },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.02, 0.9)),
            Outline { width: px(2.5), offset: px(0.), color: Color::WHITE },
            BoxShadow::new(Color::WHITE, px(0.), px(0.), px(2.), px(14.)),
            ChildOf(slot),
        ));
        if let Some(g) = first.take() {
            e.insert(g);
        } else {
            e.insert(Shared);
        }
    }
}
/// A silhouette part painted like its slot's first part.
#[derive(Component)]
struct Shared;

fn visibility(frontend: Res<crate::game_menu::Frontend>, mut roots: Query<&mut Node, With<Root>>) {
    let display = if frontend.page == crate::game_menu::Page::Glow { Display::Flex } else { Display::None };
    for mut node in &mut roots {
        if node.display != display {
            node.display = display;
        }
    }
}

fn colour(settings: &GlowSettings, prefix: &str) -> [f32; 3] {
    ["r", "g", "b"].map(|c| settings.get(&format!("{prefix}_{c}"), 0.).clamp(0., 1.))
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn interact(
    mut frontend: ResMut<crate::game_menu::Frontend>,
    mut state: ResMut<State>,
    mut settings: ResMut<GlowSettings>,
    categories: Query<(&Interaction, &Category), Changed<Interaction>>,
    acts: Query<(&Interaction, &Act), Changed<Interaction>>,
    steps: Query<(&Interaction, &Step), Changed<Interaction>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    game: Option<Res<crate::game_state::Match>>,
    mut was_open: Local<bool>,
) {
    if frontend.page != crate::game_menu::Page::Glow {
        *was_open = false;
        return;
    }
    // Opening the page shows the player's own team's view first.
    if !*was_open {
        *was_open = true;
        state.infected_view = game.as_ref().is_some_and(|g| g.infected);
        state.dirty_presets = true;
    }
    for (i, c) in &categories {
        if *i == Interaction::Pressed {
            state.selected = c.0;
        }
    }
    let prefix = COLOURS[state.selected].0;
    for (i, s) in &steps {
        if *i == Interaction::Pressed {
            let key = format!("{prefix}_{}", ["r", "g", "b"][s.0]);
            let v = ((settings.get(&key, 0.) * 255.).round() + s.1 as f32).clamp(0., 255.) / 255.;
            settings.values.insert(key, v);
        }
    }
    for (i, act) in &acts {
        if *i != Interaction::Pressed {
            continue;
        }
        match *act {
            Act::Done => {
                frontend.page = crate::game_menu::Page::Options;
            }
            Act::View(infected) => state.infected_view = infected,
            Act::ResetOne => {
                let defaults = GlowSettings::default();
                for c in ["r", "g", "b"] {
                    let key = format!("{prefix}_{c}");
                    settings.values.insert(key.clone(), defaults.get(&key, 0.));
                }
                state.status = format!("{} reset to retail.", COLOURS[state.selected].1);
            }
            Act::ResetAll => {
                let defaults = GlowSettings::default();
                for key in preset_keys() {
                    settings.values.insert(key.clone(), defaults.get(&key, 0.));
                }
                state.status = "All glow colours reset to retail.".into();
            }
            Act::SavePreset => {
                let _ = std::fs::create_dir_all(PRESET_DIR);
                let n = (1..).find(|n| !std::path::Path::new(&format!("{PRESET_DIR}/preset_{n}.cfg")).exists()).unwrap_or(1);
                let path = format!("{PRESET_DIR}/preset_{n}.cfg");
                state.status = match std::fs::write(&path, to_cfg(&settings)) {
                    Ok(()) => format!("Saved preset_{n} ({path})."),
                    Err(e) => format!("Could not save preset: {e}"),
                };
                state.dirty_presets = true;
            }
            Act::OpenFolder => {
                let _ = std::fs::create_dir_all(PRESET_DIR);
                let path = std::fs::canonicalize(PRESET_DIR).unwrap_or_else(|_| PRESET_DIR.into());
                #[cfg(windows)]
                let _ = std::process::Command::new("explorer").arg(&path).spawn();
                state.status = format!("Presets folder: {}. Drop glow .cfg files here to use them.", path.display());
            }
            Act::Export => {
                state.status = match source.as_ref().map(|s| export_to_retail(&s.0, &settings)) {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => format!("Export failed: {e}"),
                    None => "Export failed: game folder unknown.".into(),
                };
            }
            Act::Import => {
                state.status = match source.as_ref().map(|s| import_from_retail(&s.0)) {
                    Some(Ok(0)) => "No retail cfg sets glow colours (looked in left4dead2/cfg).".into(),
                    Some(Ok(n)) => format!("Imported {n} retail cfg(s) as presets."),
                    Some(Err(e)) => format!("Import failed: {e}"),
                    None => "Import failed: game folder unknown.".into(),
                };
                state.dirty_presets = true;
            }
            Act::Load(i) => {
                if let Some(path) = preset_files().get(i) {
                    let values = std::fs::read_to_string(path).map(|t| parse_cfg(&t)).unwrap_or_default();
                    for (k, v) in &values {
                        settings.values.insert(k.clone(), *v);
                    }
                    state.status = format!("Loaded {} ({} values).", stem(path), values.len());
                }
            }
            Act::Delete(i) => {
                if let Some(path) = preset_files().get(i) {
                    state.status = match std::fs::remove_file(path) {
                        Ok(()) => format!("Deleted {}.", stem(path)),
                        Err(e) => format!("Could not delete: {e}"),
                    };
                    state.dirty_presets = true;
                }
            }
        }
    }
}

fn stem(path: &std::path::Path) -> String {
    path.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned())
}

fn preset_files() -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(PRESET_DIR)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cfg"))).collect())
        .unwrap_or_default();
    files.sort();
    files
}

/// Writes left4dead2/cfg/l4d2r_glows.cfg and makes autoexec.cfg exec it.
fn export_to_retail(game: &std::path::Path, settings: &GlowSettings) -> std::io::Result<String> {
    let cfg = game.join("left4dead2").join("cfg");
    std::fs::create_dir_all(&cfg)?;
    std::fs::write(cfg.join(format!("{RETAIL_FILE}.cfg")), to_cfg(settings))?;
    let autoexec = cfg.join("autoexec.cfg");
    let current = std::fs::read_to_string(&autoexec).unwrap_or_default();
    let line = format!("exec {RETAIL_FILE}");
    let added = !current.lines().any(|l| l.trim().trim_end_matches(';').eq_ignore_ascii_case(&line));
    if added {
        let mut text = current;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&line);
        text.push('\n');
        std::fs::write(&autoexec, text)?;
    }
    Ok(format!(
        "Exported to retail cfg/{RETAIL_FILE}.cfg{}. In retail they load at startup, or type: exec {RETAIL_FILE}",
        if added { " and added it to autoexec.cfg" } else { "" }
    ))
}

/// Every retail cfg that sets glow colours becomes a preset.
fn import_from_retail(game: &std::path::Path) -> std::io::Result<usize> {
    let cfg = game.join("left4dead2").join("cfg");
    std::fs::create_dir_all(PRESET_DIR)?;
    let mut n = 0;
    for entry in std::fs::read_dir(cfg)?.flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cfg")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let values = parse_cfg(&text);
        if values.is_empty() {
            continue;
        }
        // Start from retail defaults so a partial cfg is a full preset.
        let mut preset = GlowSettings::default();
        for (k, v) in values {
            preset.values.insert(k, v);
        }
        std::fs::write(format!("{PRESET_DIR}/retail_{}.cfg", stem(&path)), to_cfg(&preset))?;
        n += 1;
    }
    Ok(n)
}

#[allow(clippy::type_complexity)]
fn sliders(
    frontend: Res<crate::game_menu::Frontend>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut state: ResMut<State>,
    mut settings: ResMut<GlowSettings>,
    tracks: Query<(&Track, &Interaction, &RelativeCursorPosition)>,
) {
    if frontend.page != crate::game_menu::Page::Glow || !mouse.pressed(MouseButton::Left) {
        state.dragging = None;
        return;
    }
    if mouse.just_pressed(MouseButton::Left) {
        state.dragging = tracks.iter().find(|(_, i, _)| **i == Interaction::Pressed).map(|(t, _, _)| t.0);
    }
    let Some(channel) = state.dragging else { return };
    let Some((_, _, cursor)) = tracks.iter().find(|(t, _, _)| t.0 == channel) else { return };
    let Some(n) = cursor.normalized else { return };
    // normalized runs -0.5..0.5 across the node.
    let v = ((n.x + 0.5).clamp(0., 1.) * 255.).round() / 255.;
    let key = format!("{}_{}", COLOURS[state.selected].0, ["r", "g", "b"][channel]);
    if (settings.get(&key, -1.) - v).abs() > 1e-4 {
        settings.values.insert(key, v);
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn paint(
    frontend: Res<crate::game_menu::Frontend>,
    settings: Res<GlowSettings>,
    state: Res<State>,
    mut chips: Query<(&Chip, &mut BackgroundColor), (Without<Swatch>, Without<Fill>, Without<Category>)>,
    mut rows: Query<(&Category, &Interaction, &mut BackgroundColor), (Without<Chip>, Without<Swatch>, Without<Fill>)>,
    mut swatch: Query<&mut BackgroundColor, (With<Swatch>, Without<Chip>, Without<Fill>, Without<Category>)>,
    mut fills: Query<(&Fill, &mut Node)>,
    mut texts: ParamSet<(
        Query<(&Value, &mut Text)>,
        Query<&mut Text, With<SelectedName>>,
        Query<&mut Text, With<Status>>,
        Query<(&GlowLabel, &mut Text)>,
    )>,
    mut glows: Query<(&Glow, &mut Outline, &mut BoxShadow, &mut Visibility, &ChildOf), Without<Shared>>,
    mut shared: Query<(&ChildOf, &mut Outline, &mut BoxShadow, &mut Visibility), With<Shared>>,
    mut view_buttons: Query<(&Act, &mut BackgroundColor), (Without<Chip>, Without<Swatch>, Without<Fill>, Without<Category>)>,
) {
    if frontend.page != crate::game_menu::Page::Glow {
        return;
    }
    let view = state.infected_view;
    if !settings.is_changed() && !state.is_changed() && !frontend.is_changed() && rows.iter().all(|(_, i, _)| *i == Interaction::None) {
        return;
    }
    let to_color = |c: [f32; 3]| Color::srgb(c[0], c[1], c[2]);
    for (chip, mut bg) in &mut chips {
        bg.0 = to_color(colour(&settings, COLOURS[chip.0].0));
    }
    for (cat, interaction, mut bg) in &mut rows {
        bg.0 = if cat.0 == state.selected {
            Color::srgba(1., 1., 1., 0.16)
        } else if *interaction != Interaction::None {
            Color::srgba(1., 1., 1., 0.07)
        } else {
            Color::NONE
        };
    }
    let (prefix, label) = COLOURS[state.selected];
    let c = colour(&settings, prefix);
    for mut bg in &mut swatch {
        bg.0 = to_color(c);
    }
    for (fill, mut node) in &mut fills {
        node.width = percent(c[fill.0] * 100.);
    }
    for (value, mut t) in &mut texts.p0() {
        t.0 = format!("{}", (c[value.0] * 255.).round() as i32);
    }
    for mut t in &mut texts.p1() {
        t.0 = format!("{label}  #{:02X}{:02X}{:02X}", (c[0] * 255.).round() as u8, (c[1] * 255.).round() as u8, (c[2] * 255.).round() as u8);
    }
    for mut t in &mut texts.p2() {
        t.0 = state.status.clone();
    }
    for (l, mut t) in &mut texts.p3() {
        t.0 = (if view { l.infected } else { l.survivor }).to_string();
    }
    let brightness = settings.get("cl_glow_brightness", 1.).clamp(0.2, 3.);
    let mut slot_paint = std::collections::HashMap::new();
    for (glow, mut outline, mut shadow, mut vis, parent) in &mut glows {
        let key = if view { glow.infected } else { glow.survivor };
        let paint = key.map(|k| {
            let c = colour(&settings, k).map(|v| (v * brightness).min(1.));
            to_color(c)
        });
        slot_paint.insert(parent.parent(), paint);
        apply(paint, &mut outline, &mut shadow, &mut vis);
    }
    for (parent, mut outline, mut shadow, mut vis) in &mut shared {
        let paint = slot_paint.get(&parent.parent()).copied().flatten();
        apply(paint, &mut outline, &mut shadow, &mut vis);
    }
    for (act, mut bg) in &mut view_buttons {
        if let Act::View(infected) = act {
            bg.0 = if *infected == view { Color::srgba(1., 1., 1., 0.22) } else { Color::srgba(1., 1., 1., 0.06) };
        }
    }
}

fn apply(paint: Option<Color>, outline: &mut Outline, shadow: &mut BoxShadow, vis: &mut Visibility) {
    match paint {
        Some(c) => {
            *vis = Visibility::Inherited;
            outline.color = c;
            if let Some(s) = shadow.0.first_mut() {
                s.color = c.with_alpha(0.85);
            }
        }
        None => *vis = Visibility::Hidden,
    }
}

fn presets(
    mut commands: Commands,
    mut state: ResMut<State>,
    list: Query<Entity, With<PresetList>>,
    rows: Query<Entity, With<PresetRow>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    mut fonts: ResMut<Assets<Font>>,
    mut font: Local<Option<Handle<Font>>>,
) {
    if !state.dirty_presets {
        return;
    }
    let Ok(list) = list.single() else { return };
    let Some(source) = source else { return };
    let font = font.get_or_insert_with(|| menu_font(&source, &mut fonts)).clone();
    state.dirty_presets = false;
    for row in &rows {
        commands.entity(row).despawn();
    }
    let files = preset_files();
    if files.is_empty() {
        let t = text(&mut commands, list, &font, "No presets yet.", 14., rect(0., 4., 360., 20.));
        commands.entity(t).insert(PresetRow);
    }
    for (i, path) in files.iter().enumerate().take(6) {
        let y = i as f32 * 26.;
        let load = button(&mut commands, list, &font, &stem(path), rect(0., y, 300., 24.), Act::Load(i));
        commands.entity(load).insert(PresetRow);
        let del = button(&mut commands, list, &font, "X", rect(306., y, 40., 24.), Act::Delete(i));
        commands.entity(del).insert(PresetRow);
    }
    if files.len() > 6 {
        let t = text(&mut commands, list, &font, &format!("+{} more in the folder", files.len() - 6), 13., rect(0., 158., 360., 18.));
        commands.entity(t).insert(PresetRow);
    }
}

/// Saves the colours half a second after the last change (console
/// `cl_glow_*` edits included).
fn persist(time: Res<Time<Real>>, settings: Res<GlowSettings>, mut state: ResMut<State>) {
    let now = time.elapsed_secs_f64();
    if settings.is_changed() && !settings.is_added() {
        state.save_at = Some(now + 0.5);
    }
    if state.save_at.is_some_and(|t| now >= t) {
        state.save_at = None;
        let _ = std::fs::create_dir_all("runtime");
        if let Err(e) = std::fs::write(SAVE_FILE, to_cfg(&settings)) {
            eprintln!("Glows: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn glow_cfg_round_trips_in_retail_format() {
        let mut s = GlowSettings::default();
        s.values.insert("cl_glow_item_r".into(), 0.2);
        let cfg = to_cfg(&s);
        assert!(cfg.contains("cl_glow_item_r \"0.2\""), "{cfg}");
        assert!(cfg.contains("cl_witch_glow_angry_r \"1\""), "{cfg}");
        let back = parse_cfg(&cfg);
        assert_eq!(back.len(), COLOURS.len() * 3);
        assert!(back.contains(&("cl_glow_item_r".to_string(), 0.2)));
        // Retail syntax: quotes optional, ; separated, comments, other cvars ignored.
        let v = parse_cfg("sv_cheats 1; cl_glow_survivor_r 0.5 // x\nCL_GLOW_ITEM_FAR_G \"2\"\ncl_glow_brightness 3");
        assert_eq!(v, vec![("cl_glow_survivor_r".to_string(), 0.5), ("cl_glow_item_far_g".to_string(), 1.)]);
    }
    #[test]
    fn every_listed_colour_has_retail_defaults() {
        let s = GlowSettings::default();
        for key in preset_keys() {
            assert!(s.values.contains_key(&key), "{key}");
        }
    }
}
