//! EDIT WORLD interface, laid out like Hammer / Roblox Studio:
//!
//! - Free cursor; hold RMB to look around (WASD flies), handled with
//!   `world_editor::edit_cursor` in the game's input.
//! - Toolbar: play/edit, save, load, grid ("studs") size, move / resize
//!   tools, rotate, duplicate, delete.
//! - Browser (left): tabs for Props, Entities (triggers, logic, points) and
//!   Scene (what was placed), a search field, and a live 3D preview of the
//!   highlighted model.
//! - Picking a prop or entity shows a translucent ghost where it will land;
//!   a click places it (Esc stops placing).
//! - Click selects, double-click opens Properties (right): class, name,
//!   origin, angles, trigger mins/maxs, every keyvalue and every output,
//!   all editable in place, plus one-click "damage gives health/weapon".
//! - The selection shows axis arrows: drag one to move along it in grid
//!   steps. Resize tool: drag a trigger's face handles to change its box.
use crate::world_editor::{self, Editor, Place};
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::MouseWheel;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use bevy::window::PrimaryWindow;

const UNIT: f32 = 0.0254;
const ROWS: usize = 18;
const PREVIEW_LAYER: usize = 9;

const PANEL: Color = Color::srgba(0.07, 0.08, 0.09, 0.92);
const BUTTON: Color = Color::srgba(1., 1., 1., 0.07);
const BUTTON_HOVER: Color = Color::srgba(1., 1., 1., 0.16);
const BUTTON_ON: Color = Color::srgba(0.35, 0.55, 0.95, 0.55);
const TEXT: Color = Color::srgb(0.88, 0.89, 0.9);
const DIM: Color = Color::srgb(0.6, 0.62, 0.65);
const FIELD: Color = Color::srgba(0., 0., 0., 0.45);
const FIELD_FOCUS: Color = Color::srgba(0.2, 0.32, 0.6, 0.8);

/// Point, logic and trigger classes offered in the Entities tab.
const CLASSES: &[(&str, &str)] = &[
    ("trigger_multiple", "Trigger (fires every touch)"),
    ("trigger_once", "Trigger (fires once)"),
    ("logic_relay", "Relay"),
    ("logic_timer", "Timer"),
    ("logic_branch", "Branch"),
    ("logic_case", "Case"),
    ("logic_compare", "Compare"),
    ("math_counter", "Counter"),
    ("point_clientcommand", "Client command (give, say...)"),
    ("point_servercommand", "Server command"),
    ("info_target", "Target"),
    ("env_sprite", "Sprite"),
    ("env_shake", "Screen shake"),
    ("env_explosion", "Explosion"),
    ("point_hurt", "Hurt point"),
    ("ambient_generic", "Sound"),
    ("info_particle_system", "Particle system"),
];

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Tab {
    #[default]
    Props,
    Entities,
    Scene,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tool {
    #[default]
    Move,
    Resize,
}

#[derive(Clone, PartialEq, Debug)]
enum Field {
    Search,
    Grid,
    Class,
    Name,
    Origin,
    Angles,
    Mins,
    Maxs,
    KvKey(usize),
    KvVal(usize),
    NewKv,
    Out(usize, u8),
}

#[derive(Clone, Copy)]
enum Drag {
    Move { axis: usize, cursor: Vec2, dir: Vec2, px_per_unit: f32, start: l4d2r_math::Vec3 },
    Resize { axis: usize, max: bool, cursor: Vec2, dir: Vec2, px_per_unit: f32, lo: l4d2r_math::Vec3, hi: l4d2r_math::Vec3 },
}

#[derive(Resource, Default)]
struct UiState {
    tab: Tab,
    tool: Tool,
    filter: String,
    scroll: usize,
    props_open: bool,
    focus: Option<(Field, String)>,
    list_key: Option<(Tab, String, usize, usize, Option<usize>, Option<String>)>,
    props_key: Option<u64>,
    last_click: Option<(f64, usize)>,
    drag: Option<Drag>,
    /// Hovered gizmo handle: (axis 0..2, max side for resize handles).
    hover_handle: Option<(usize, bool)>,
    /// Where a click would place, Source units.
    hover_place: Option<l4d2r_math::Vec3>,
    place_breakable: bool,
    ghost_key: Option<Place>,
    ghost: Vec<Entity>,
    /// Model shown in the preview: (path, preview entities).
    preview_key: Option<String>,
    preview: Vec<Entity>,
    preview_image: Option<Handle<Image>>,
    preview_pivot: Option<Entity>,
    hovered_model: Option<String>,
    /// The editor hid the game HUD (restored when editing ends).
    hud_hidden: bool,
}

#[derive(Component)]
struct EditRoot;
/// Panels the cursor can be over (world clicks are ignored there).
#[derive(Component)]
struct Panel;
#[derive(Component)]
struct PropsPanel;
#[derive(Component)]
struct ListBox;
#[derive(Component)]
struct PropsBody;
#[derive(Component)]
struct PreviewNode;
#[derive(Component)]
struct StatusText;
#[derive(Component)]
struct ModeText;
#[derive(Component)]
struct GridText;
#[derive(Component)]
struct ListRow(usize);
#[derive(Component)]
struct FieldBtn(Field, String);
#[derive(Component)]
struct FieldText(Field);
#[derive(Component)]
struct Toggle(Act);
#[derive(Component)]
struct GhostPart;
#[derive(Component)]
struct PreviewPart;
#[derive(Component)]
struct PreviewPivot;
#[derive(Component)]
struct PreviewCamera;

#[derive(Component, Clone, Copy, PartialEq, Debug)]
enum Act {
    ToggleMode,
    Save,
    Load,
    GridDown,
    GridUp,
    ToolMove,
    ToolResize,
    RotateLeft,
    RotateRight,
    Duplicate,
    Delete,
    StopPlacing,
    Tab(Tab),
    CloseProps,
    AddOutput,
    DelKv(usize),
    DelOut(usize),
    GiveHealth,
    GiveWeapon,
}

pub fn install(app: &mut App) {
    app.add_systems(Startup, gizmo_style);
    app.init_resource::<UiState>().add_systems(
        Update,
        (setup, show, fps_below_toolbar, buttons, browser, properties, fields, world, draw, ghost, preview)
            .chain()
            .after(crate::game_menu::update),
    );
}

/// The editor is the only gizmo user: thicker lines drawn over the world,
/// so the selection box and arrows stay visible through props and walls.
fn gizmo_style(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<DefaultGizmoConfigGroup>();
    config.line.width = 3.;
    config.depth_bias = -1.;
}

fn px_node(left: f32, top: f32, width: f32, height: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(left),
        top: px(top),
        width: px(width),
        height: px(height),
        ..default()
    }
}

fn text(commands: &mut Commands, parent: Entity, s: &str, size: f32, color: Color) -> Entity {
    commands
        .spawn((Text::new(s), TextFont { font_size: FontSize::Px(size), ..default() }, TextColor(color), ChildOf(parent)))
        .id()
}

fn button(commands: &mut Commands, parent: Entity, label: &str, act: Act, width: f32) -> Entity {
    let b = commands
        .spawn((
            Button,
            act,
            Node {
                // Sized by the label (never wrapping); `width` is the least.
                // Fixed widths wrapped "Rotate -15" onto two lines in a
                // smaller window.
                min_width: px(width),
                height: px(24.),
                margin: UiRect::all(px(2.)),
                padding: UiRect::horizontal(px(6.)),
                flex_shrink: 0.,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(px(1.)),
                ..default()
            },
            BackgroundColor(BUTTON),
            BorderColor::all(Color::srgba(1., 1., 1., 0.15)),
            ChildOf(parent),
        ))
        .id();
    let t = text(commands, b, label, 13., TEXT);
    commands.entity(t).insert(TextLayout::default().with_no_wrap());
    b
}

fn field(commands: &mut Commands, parent: Entity, f: Field, value: &str, width: f32) -> Entity {
    let b = commands
        .spawn((
            Button,
            FieldBtn(f.clone(), value.to_string()),
            Node {
                width: px(width),
                min_height: px(22.),
                margin: UiRect::all(px(1.)),
                padding: UiRect::horizontal(px(4.)),
                align_items: AlignItems::Center,
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(FIELD),
            ChildOf(parent),
        ))
        .id();
    let (shown, color) = if value.is_empty() { ("(click to type)", DIM) } else { (value, TEXT) };
    commands.spawn((
        Text::new(shown),
        TextFont { font_size: FontSize::Px(12.), ..default() },
        TextLayout::default().with_no_wrap(),
        TextColor(color),
        FieldText(f),
        ChildOf(b),
    ));
    b
}

fn row(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, flex_wrap: FlexWrap::Wrap, ..default() },
            ChildOf(parent),
        ))
        .id()
}

fn setup(mut commands: Commands, roots: Query<(), With<EditRoot>>, camera: Query<Entity, With<crate::game::ViewCamera>>) {
    if !roots.is_empty() {
        return;
    }
    let Some(camera) = camera.iter().next() else { return };
    let root = commands
        .spawn((
            EditRoot,
            Node { position_type: PositionType::Absolute, width: percent(100.), height: percent(100.), display: Display::None, ..default() },
            GlobalZIndex(90),
            UiTargetCamera(camera),
        ))
        .id();
    // Toolbar.
    let bar = commands
        .spawn((
            Panel,
            RelativeCursorPosition::default(),
            Node {
                position_type: PositionType::Absolute,
                left: px(0.),
                top: px(0.),
                width: percent(100.),
                height: px(34.),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(px(6.)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(PANEL),
            ChildOf(root),
        ))
        .id();
    let mode = button(&mut commands, bar, "Play (F5)", Act::ToggleMode, 92.);
    commands.entity(mode).insert(ModeText);
    button(&mut commands, bar, "Save (F6)", Act::Save, 84.);
    button(&mut commands, bar, "Load (F7)", Act::Load, 84.);
    let studs = text(&mut commands, bar, "Studs", 13., DIM);
    commands.entity(studs).insert((Node { margin: UiRect::new(px(22.), px(10.), px(0.), px(0.)), flex_shrink: 0., ..default() }, TextLayout::default().with_no_wrap()));
    button(&mut commands, bar, "-", Act::GridDown, 24.);
    let grid = text(&mut commands, bar, "8", 13., TEXT);
    commands.entity(grid).insert((GridText, Node { margin: UiRect::horizontal(px(8.)), ..default() }));
    button(&mut commands, bar, "+", Act::GridUp, 24.);
    let tool = text(&mut commands, bar, "Tool", 13., DIM);
    commands.entity(tool).insert((Node { margin: UiRect::new(px(22.), px(10.), px(0.), px(0.)), flex_shrink: 0., ..default() }, TextLayout::default().with_no_wrap()));
    let m = button(&mut commands, bar, "Move", Act::ToolMove, 60.);
    commands.entity(m).insert(Toggle(Act::ToolMove));
    let r = button(&mut commands, bar, "Resize", Act::ToolResize, 60.);
    commands.entity(r).insert(Toggle(Act::ToolResize));
    button(&mut commands, bar, "Rotate -15", Act::RotateLeft, 96.);
    button(&mut commands, bar, "Rotate +15", Act::RotateRight, 96.);
    button(&mut commands, bar, "Duplicate", Act::Duplicate, 88.);
    button(&mut commands, bar, "Delete", Act::Delete, 62.);
    button(&mut commands, bar, "Stop placing", Act::StopPlacing, 104.);
    let status = text(&mut commands, bar, "", 13., DIM);
    commands.entity(status).insert((StatusText, Node { margin: UiRect::left(px(14.)), flex_shrink: 0., ..default() }, TextLayout::default().with_no_wrap()));
    // Browser.
    let browser = commands
        .spawn((
            Panel,
            RelativeCursorPosition::default(),
            Node {
                position_type: PositionType::Absolute,
                left: px(6.),
                top: px(40.),
                width: px(300.),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(6.)),
                ..default()
            },
            BackgroundColor(PANEL),
            ChildOf(root),
        ))
        .id();
    let tabs = row(&mut commands, browser);
    for (label, tab) in [("Props", Tab::Props), ("Entities", Tab::Entities), ("Scene", Tab::Scene)] {
        let b = button(&mut commands, tabs, label, Act::Tab(tab), 92.);
        commands.entity(b).insert(Toggle(Act::Tab(tab)));
    }
    let search = row(&mut commands, browser);
    text(&mut commands, search, "Search ", 12., DIM);
    field(&mut commands, search, Field::Search, "", 226.);
    commands.spawn((ListBox, Node { flex_direction: FlexDirection::Column, min_height: px(ROWS as f32 * 19.), ..default() }, ChildOf(browser)));
    text(&mut commands, browser, "Preview", 12., DIM);
    commands.spawn((
        PreviewNode,
        ImageNode::default(),
        Node { width: px(286.), height: px(200.), ..default() },
        BackgroundColor(Color::srgba(0., 0., 0., 0.5)),
        ChildOf(browser),
    ));
    text(&mut commands, browser, "Click a prop or entity, then click in the world to place it.\nRMB + WASD: fly.  Click: select.  Double-click: properties.", 11., DIM);
    // Properties.
    let props = commands
        .spawn((
            Panel,
            PropsPanel,
            RelativeCursorPosition::default(),
            Node {
                position_type: PositionType::Absolute,
                right: px(6.),
                top: px(40.),
                width: px(420.),
                max_height: percent(88.),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(6.)),
                overflow: Overflow::clip_y(),
                display: Display::None,
                ..default()
            },
            BackgroundColor(PANEL),
            ChildOf(root),
        ))
        .id();
    commands.spawn((PropsBody, Node { flex_direction: FlexDirection::Column, ..default() }, ChildOf(props)));
}

/// The FPS counter sits top right, where the toolbar's status text is: drop
/// it below the toolbar while editing.
/// Also hides the first-person gun and arms (layer 2 on the view-model
/// camera, which keeps drawing the UI) while editing, like Hammer / Studio.
fn fps_below_toolbar(
    editor: Res<Editor>,
    mut fps: Query<&mut Node, With<crate::performance::FpsOverlay>>,
    mut view: Query<&mut bevy::camera::visibility::RenderLayers, With<crate::game::ViewCamera>>,
) {
    let editing = editor.opened && editor.editing;
    let top = if editing { px(40.) } else { px(8.) };
    for mut node in &mut fps {
        if node.top != top {
            node.top = top;
        }
    }
    let layers = if editing {
        bevy::camera::visibility::RenderLayers::none()
    } else {
        bevy::camera::visibility::RenderLayers::layer(2)
    };
    for mut l in &mut view {
        if *l != layers {
            *l = layers.clone();
        }
    }
}

#[allow(clippy::type_complexity)]
fn show(
    editor: Res<Editor>,
    mut ui: ResMut<UiState>,
    frontend: Res<crate::game_menu::Frontend>,
    console: Res<crate::game_console::Console>,
    mut roots: Query<&mut Node, (With<EditRoot>, Without<PropsPanel>)>,
    mut props: Query<&mut Node, (With<PropsPanel>, Without<EditRoot>)>,
    mut status: Query<&mut Text, (With<StatusText>, Without<GridText>)>,
    mut grid: Query<&mut Text, (With<GridText>, Without<StatusText>)>,
    mut toggles: Query<(&Toggle, &Interaction, &mut BackgroundColor), Without<FieldBtn>>,
    mut mode: Query<&Children, With<ModeText>>,
    mut texts: Query<&mut Text, (Without<StatusText>, Without<GridText>)>,
    mut hud: Query<&mut Node, (With<crate::game_ui::HudRoot>, Without<EditRoot>, Without<PropsPanel>)>,
) {
    let visible = editor.opened && editor.editing && frontend.page == crate::game_menu::Page::Game && !console.open;
    if visible {
        // Like Hammer / Roblox Studio: no game HUD while editing.
        for mut node in &mut hud {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
        ui.hud_hidden = true;
    } else if ui.hud_hidden && frontend.page == crate::game_menu::Page::Game {
        for mut node in &mut hud {
            node.display = Display::Flex;
        }
        ui.hud_hidden = false;
    }
    for mut node in &mut roots {
        let d = if visible { Display::Flex } else { Display::None };
        if node.display != d {
            node.display = d;
        }
    }
    let open = ui.props_open && editor.selected.is_some();
    for mut node in &mut props {
        let d = if open { Display::Flex } else { Display::None };
        if node.display != d {
            node.display = d;
        }
    }
    if !visible {
        return;
    }
    let line = match &editor.placing {
        Some(Place::Model(m)) => format!("  Placing {} - click in the world, Esc to stop. {}", m.trim_start_matches("models/"), editor.status),
        Some(Place::Class(c)) => format!("  Placing {c} - click in the world, Esc to stop. {}", editor.status),
        None => format!("  {}", editor.status),
    };
    for mut t in &mut status {
        if t.0 != line {
            t.0 = line.clone();
        }
    }
    let g = format!(" {} ", editor.grid());
    for mut t in &mut grid {
        if t.0 != g {
            t.0 = g.clone();
        }
    }
    for children in &mut mode {
        for &c in children {
            if let Ok(mut t) = texts.get_mut(c) {
                let want = if editor.editing { "Play (F5)" } else { "Edit (F5)" };
                if t.0 != want {
                    t.0 = want.into();
                }
            }
        }
    }
    for (toggle, interaction, mut bg) in &mut toggles {
        let on = match toggle.0 {
            Act::ToolMove => ui.tool == Tool::Move,
            Act::ToolResize => ui.tool == Tool::Resize,
            Act::Tab(t) => ui.tab == t,
            _ => false,
        };
        bg.0 = if on {
            BUTTON_ON
        } else if *interaction == Interaction::Hovered {
            BUTTON_HOVER
        } else {
            BUTTON
        };
    }
}

fn give_output(logic: &mut crate::l4d2r_cinematic::MapLogic, editor: &mut Editor, id: usize, item: &str) {
    if logic.world.find("PD1").is_empty() {
        let hammer = world_editor::next_hammer(editor, logic);
        let kv = vec![("targetname".to_string(), "PD1".to_string()), ("hammerid".to_string(), hammer.to_string())];
        let pd = logic.world.create_entity("point_clientcommand", &kv, l4d2r_math::Vec3::ZERO, l4d2r_math::Vec3::ZERO, None);
        editor.objects.push(pd);
    }
    let output = if logic.world.ents[id].me.mdl.is_some() { "OnTakeDamage" } else { "OnTrigger" };
    logic.world.set_keyvalue(id, output, &format!("PD1,Command,give {item}"));
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn buttons(
    mut editor: ResMut<Editor>,
    mut ui: ResMut<UiState>,
    mut console: ResMut<crate::game_console::Console>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    root: Option<Res<crate::game_ui::HudSource>>,
    settings: Option<Res<crate::campaign_settings::SessionSettings>>,
    pressed: Query<(&Interaction, &Act), (Changed<Interaction>, Without<Toggle>)>,
    toggles: Query<(&Interaction, &Act), (Changed<Interaction>, With<Toggle>)>,
    mut plain: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<Act>, Without<Toggle>)>,
) {
    for (interaction, mut bg) in &mut plain {
        bg.0 = if *interaction == Interaction::None { BUTTON } else { BUTTON_HOVER };
    }
    let Some(mut logic) = logic else { return };
    let acts: Vec<Act> = pressed.iter().chain(toggles.iter()).filter(|(i, _)| **i == Interaction::Pressed).map(|(_, a)| *a).collect();
    for act in acts {
        let sel = editor.selected.filter(|&id| logic.world.ents.get(id).is_some_and(|e| e.alive));
        match act {
            Act::ToggleMode => {
                let editing = !editor.editing;
                world_editor::set_mode(&mut editor, &mut console, editing);
            }
            Act::Save => {
                if let Some(root) = root.as_ref() {
                    let map = settings.as_ref().and_then(|s| s.map.file_stem()).map_or("map".to_string(), |m| m.to_string_lossy().into_owned());
                    editor.status = world_editor::save(&mut editor, &logic, &root.0, &map);
                }
            }
            Act::Load => world_editor::load_last(&mut editor, &mut logic),
            Act::GridDown => editor.grid = (editor.grid() / 2.).max(1.),
            Act::GridUp => editor.grid = (editor.grid() * 2.).min(256.),
            Act::ToolMove => ui.tool = Tool::Move,
            Act::ToolResize => ui.tool = Tool::Resize,
            Act::RotateLeft => world_editor::rotate(&mut editor, &mut logic, -15.),
            Act::RotateRight => world_editor::rotate(&mut editor, &mut logic, 15.),
            Act::Duplicate => world_editor::duplicate(&mut editor, &mut logic),
            Act::Delete => world_editor::delete(&mut editor, &mut logic),
            Act::StopPlacing => editor.placing = None,
            Act::Tab(t) => {
                ui.tab = t;
                ui.scroll = 0;
            }
            Act::CloseProps => ui.props_open = false,
            Act::AddOutput => {
                if let Some(id) = sel {
                    let output = if logic.world.ents[id].me.mdl.is_some() { "OnTakeDamage" } else { "OnTrigger" };
                    logic.world.ents[id].conns.push(l4d2_formats::vmf::Connection {
                        output: output.into(),
                        target: "!activator".into(),
                        input: "Kill".into(),
                        param: String::new(),
                        delay: 0.,
                        times: -1,
                    });
                }
            }
            Act::DelKv(i) => {
                if let Some(id) = sel {
                    let keys = editable_keys(&logic.world.ents[id]);
                    if let Some(key) = keys.get(i).map(|(k, _)| k.clone()) {
                        logic.world.ents[id].me.ent.0.retain(|(k, _)| *k != key);
                    }
                }
            }
            Act::DelOut(i) => {
                if let Some(id) = sel {
                    if i < logic.world.ents[id].conns.len() {
                        logic.world.ents[id].conns.remove(i);
                    }
                }
            }
            Act::GiveHealth => {
                if let Some(id) = sel {
                    give_output(&mut logic, &mut editor, id, "health");
                    editor.status = "Damage it in play mode (F5) to get health".into();
                }
            }
            Act::GiveWeapon => {
                if let Some(id) = sel {
                    give_output(&mut logic, &mut editor, id, "rifle_ak47");
                    editor.status = "Edit the output's param to choose the weapon (give <name>)".into();
                }
            }
        }
    }
}

/// Keyvalues shown in the table (the named fields have their own rows).
fn editable_keys(e: &l4d2r_game_core::entities::Ent) -> Vec<(String, String)> {
    e.me.ent
        .0
        .iter()
        .filter(|(k, _)| {
            !matches!(
                k.to_ascii_lowercase().as_str(),
                "classname" | "targetname" | "origin" | "angles" | "hammerid" | "mins" | "maxs"
            ) && !k.starts_with("__")
        })
        .cloned()
        .collect()
}

/// Browser entries for the current tab and filter: (label, entry).
enum Entry {
    Model(String),
    Class(String),
    Object(usize),
}

fn entries(tab: Tab, filter: &str, editor: &Editor, logic: &crate::l4d2r_cinematic::MapLogic) -> Vec<(String, Entry)> {
    let f = filter.to_ascii_lowercase();
    match tab {
        Tab::Props => editor
            .models
            .iter()
            .filter(|m| f.is_empty() || m.contains(&f))
            .map(|m| (m.trim_start_matches("models/").trim_end_matches(".mdl").to_string(), Entry::Model(m.clone())))
            .collect(),
        Tab::Entities => CLASSES
            .iter()
            .filter(|(c, d)| f.is_empty() || c.contains(&f) || d.to_ascii_lowercase().contains(&f))
            .map(|(c, d)| (format!("{c}  -  {d}"), Entry::Class(c.to_string())))
            .collect(),
        Tab::Scene => editor
            .objects
            .iter()
            .filter(|&&id| logic.world.ents.get(id).is_some_and(|e| e.alive))
            .map(|&id| {
                let e = &logic.world.ents[id];
                let what = e.me.mdl.as_deref().map_or(e.me.classname.clone(), |m| m.trim_start_matches("models/").to_string());
                (format!("{}  ({what})", e.me.targetname), Entry::Object(id))
            })
            .filter(|(l, _)| f.is_empty() || l.to_ascii_lowercase().contains(&f))
            .collect(),
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn browser(
    mut commands: Commands,
    mut editor: ResMut<Editor>,
    mut ui: ResMut<UiState>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    root: Option<Res<crate::game_ui::HudSource>>,
    time: Res<Time<Real>>,
    lists: Query<Entity, With<ListBox>>,
    rows: Query<Entity, With<ListRow>>,
    clicked: Query<(&Interaction, &ListRow), Changed<Interaction>>,
    mut row_colors: Query<(&Interaction, &ListRow, &mut BackgroundColor)>,
    panels: Query<&RelativeCursorPosition, With<Panel>>,
    mut wheel: MessageReader<MouseWheel>,
) {
    let Some(logic) = logic else { return };
    if !editor.editing {
        wheel.clear();
        return;
    }
    let list = entries(ui.tab, &ui.filter, &editor, &logic);
    // Scroll with the wheel over the browser (the first panel under the cursor).
    let over = panels.iter().any(|p| p.cursor_over());
    for w in wheel.read() {
        if over {
            let step = if w.y > 0. { -3 } else { 3 };
            ui.scroll = (ui.scroll as i64 + step).clamp(0, list.len().saturating_sub(ROWS) as i64) as usize;
        }
    }
    // Clicks.
    let mut chosen = None;
    for (interaction, r) in &clicked {
        if *interaction == Interaction::Pressed {
            chosen = Some(r.0);
        }
    }
    if let Some(i) = chosen {
        match list.get(i).map(|(_, e)| e) {
            Some(Entry::Model(m)) => {
                ui.place_breakable = root
                    .as_ref()
                    .and_then(|r| crate::source_assets::SourceAssets::new(&r.0, vec![]).ok())
                    .is_some_and(|mut a| crate::source_breakables::model_health(&mut a, m) > 0.);
                editor.placing = Some(Place::Model(m.clone()));
                ui.hovered_model = Some(m.clone());
                editor.status = if ui.place_breakable { "Breakable model: retail prop_dynamic refuses it".into() } else { String::new() };
            }
            Some(Entry::Class(c)) => {
                editor.placing = Some(Place::Class(c.clone()));
                editor.status = String::new();
            }
            Some(Entry::Object(id)) => {
                let id = *id;
                let now = time.elapsed_secs_f64();
                if ui.last_click.is_some_and(|(t, last)| last == id && now - t < 0.4) {
                    ui.props_open = true;
                }
                ui.last_click = Some((now, id));
                editor.selected = Some(id);
                editor.placing = None;
                ui.hovered_model = logic.world.ents[id].me.mdl.clone();
            }
            None => {}
        }
    }
    let placing_label = editor.placing.as_ref().map(|p| format!("{p:?}"));
    let key = (ui.tab, ui.filter.clone(), ui.scroll, list.len(), editor.selected, placing_label);
    if ui.list_key.as_ref() != Some(&key) {
        ui.list_key = Some(key);
        for r in &rows {
            commands.entity(r).despawn();
        }
        if let Some(parent) = lists.iter().next() {
            for (i, (label, entry)) in list.iter().enumerate().skip(ui.scroll).take(ROWS) {
                let active = match entry {
                    Entry::Model(m) => editor.placing == Some(Place::Model(m.clone())),
                    Entry::Class(c) => editor.placing == Some(Place::Class(c.clone())),
                    Entry::Object(id) => editor.selected == Some(*id),
                };
                let b = commands
                    .spawn((
                        Button,
                        ListRow(i),
                        Node { height: px(19.), padding: UiRect::horizontal(px(4.)), align_items: AlignItems::Center, overflow: Overflow::clip(), ..default() },
                        BackgroundColor(if active { BUTTON_ON } else { Color::NONE }),
                        ChildOf(parent),
                    ))
                    .id();
                commands.spawn((
                    Text::new(label.clone()),
                    TextFont { font_size: FontSize::Px(12.), ..default() },
                    TextLayout::default().with_no_wrap(),
                    TextColor(TEXT),
                    ChildOf(b),
                ));
            }
            if list.len() > ROWS {
                let more = commands
                    .spawn((ListRow(usize::MAX), Node { height: px(16.), ..default() }, ChildOf(parent)))
                    .id();
                commands.spawn((
                    Text::new(format!("{}-{} of {} (scroll)", ui.scroll + 1, (ui.scroll + ROWS).min(list.len()), list.len())),
                    TextFont { font_size: FontSize::Px(11.), ..default() },
                    TextColor(DIM),
                    ChildOf(more),
                ));
            }
        }
    }
    // Hover: highlight the row and preview its model.
    for (interaction, r, mut bg) in &mut row_colors {
        if *interaction == Interaction::Hovered {
            bg.0 = BUTTON_HOVER;
            if let Some((_, Entry::Model(m))) = list.get(r.0) {
                ui.hovered_model = Some(m.clone());
            }
            if let Some((_, Entry::Object(id))) = list.get(r.0) {
                ui.hovered_model = logic.world.ents.get(*id).and_then(|e| e.me.mdl.clone());
            }
        } else if *interaction == Interaction::None && bg.0 == BUTTON_HOVER {
            bg.0 = Color::NONE;
        }
    }
}

fn hash_entity(e: &l4d2r_game_core::entities::Ent, bounds: Option<(l4d2r_math::Vec3, l4d2r_math::Vec3)>, id: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    for (k, v) in &e.me.ent.0 {
        k.hash(&mut h);
        v.hash(&mut h);
    }
    for c in &e.conns {
        (&c.output, &c.target, &c.input, &c.param, c.delay.to_bits()).hash(&mut h);
    }
    for v in [e.me.origin, e.me.angles] {
        v.to_array().map(f32::to_bits).hash(&mut h);
    }
    if let Some((lo, hi)) = bounds {
        lo.to_array().map(f32::to_bits).hash(&mut h);
        hi.to_array().map(f32::to_bits).hash(&mut h);
    }
    h.finish()
}

fn v3(v: l4d2r_math::Vec3) -> String {
    let r = |x: f32| (x * 100.).round() / 100.;
    format!("{} {} {}", r(v.x), r(v.y), r(v.z))
}

#[allow(clippy::too_many_arguments)]
fn properties(
    mut commands: Commands,
    editor: Res<Editor>,
    mut ui: ResMut<UiState>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    bodies: Query<(Entity, Option<&Children>), With<PropsBody>>,
) {
    let Some(logic) = logic else { return };
    let Some(id) = editor.selected.filter(|&id| logic.world.ents.get(id).is_some_and(|e| e.alive)) else {
        ui.props_key = None;
        return;
    };
    if !ui.props_open {
        return;
    }
    // Keep the panel while a field in it is being typed into.
    if ui.focus.as_ref().is_some_and(|(f, _)| !matches!(f, Field::Search | Field::Grid)) {
        return;
    }
    let e = &logic.world.ents[id];
    let bounds = logic.prop_bounds.get(&id).copied().filter(|_| e.me.mdl.is_none());
    let key = hash_entity(e, bounds, id);
    if ui.props_key == Some(key) {
        return;
    }
    ui.props_key = Some(key);
    let Some((body, children)) = bodies.iter().next() else { return };
    for &c in children.into_iter().flatten() {
        commands.entity(c).despawn();
    }
    let c = &mut commands;
    let header = row(c, body);
    let real = e.me.mdl.as_deref().map_or(e.me.classname.clone(), |m| format!("{} - {}", e.me.classname, m.trim_start_matches("models/")));
    text(c, header, &format!("Properties   {real}"), 14., TEXT);
    button(c, header, "Close", Act::CloseProps, 54.);
    for (label, f, value) in [
        ("Class", Field::Class, e.me.kv("classname").unwrap_or(&e.me.classname).to_string()),
        ("Name", Field::Name, e.me.targetname.clone()),
        ("Origin", Field::Origin, v3(e.me.origin)),
        ("Angles", Field::Angles, v3(e.me.angles)),
    ] {
        let r = row(c, body);
        let l = text(c, r, label, 12., DIM);
        c.entity(l).insert(Node { width: px(56.), ..default() });
        field(c, r, f, &value, 330.);
    }
    if let Some((lo, hi)) = bounds {
        for (label, f, value) in [("Mins", Field::Mins, v3(lo)), ("Maxs", Field::Maxs, v3(hi))] {
            let r = row(c, body);
            let l = text(c, r, label, 12., DIM);
            c.entity(l).insert(Node { width: px(56.), ..default() });
            field(c, r, f, &value, 330.);
        }
    }
    text(c, body, "\nKeyvalues (click to edit, Enter to apply)", 12., DIM);
    for (i, (k, v)) in editable_keys(e).iter().enumerate() {
        let r = row(c, body);
        field(c, r, Field::KvKey(i), k, 130.);
        field(c, r, Field::KvVal(i), v, 222.);
        button(c, r, "x", Act::DelKv(i), 22.);
    }
    let r = row(c, body);
    text(c, r, "Add:", 12., DIM);
    field(c, r, Field::NewKv, "key value", 330.);
    text(c, body, "\nOutputs: output > target > input (param) after delay", 12., DIM);
    for (i, conn) in e.conns.iter().enumerate() {
        let r = row(c, body);
        field(c, r, Field::Out(i, 0), &conn.output, 96.);
        field(c, r, Field::Out(i, 1), &conn.target, 70.);
        field(c, r, Field::Out(i, 2), &conn.input, 70.);
        field(c, r, Field::Out(i, 3), &conn.param, 92.);
        field(c, r, Field::Out(i, 4), &conn.delay.to_string(), 34.);
        button(c, r, "x", Act::DelOut(i), 22.);
    }
    let r = row(c, body);
    button(c, r, "+ Output", Act::AddOutput, 80.);
    button(c, r, "Damage/touch: give health", Act::GiveHealth, 170.);
    button(c, r, "give weapon", Act::GiveWeapon, 90.);
}

/// Applies a committed field.
fn commit(
    field: &Field,
    value: &str,
    editor: &mut Editor,
    ui: &mut UiState,
    logic: &mut crate::l4d2r_cinematic::MapLogic,
) {
    let value = value.trim();
    let vec3 = |s: &str| {
        let n: Vec<f32> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        (n.len() == 3 && n.iter().all(|v| v.is_finite())).then(|| l4d2r_math::Vec3::new(n[0], n[1], n[2]))
    };
    match field {
        Field::Search => {
            ui.filter = value.to_string();
            ui.scroll = 0;
            return;
        }
        Field::Grid => {
            if let Ok(g) = value.parse::<f32>() {
                editor.grid = g.clamp(0.25, 512.);
            }
            return;
        }
        _ => {}
    }
    let Some(id) = editor.selected.filter(|&id| logic.world.ents.get(id).is_some_and(|e| e.alive)) else { return };
    let set_bounds = |logic: &mut crate::l4d2r_cinematic::MapLogic, lo: l4d2r_math::Vec3, hi: l4d2r_math::Vec3| {
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        logic.prop_bounds.insert(id, (lo, hi));
        logic.world.set_keyvalue(id, "mins", &v3(lo));
        logic.world.set_keyvalue(id, "maxs", &v3(hi));
    };
    match field {
        Field::Class if !value.is_empty() => logic.world.set_keyvalue(id, "classname", value),
        Field::Name => logic.world.set_keyvalue(id, "targetname", value),
        Field::Origin => {
            if vec3(value).is_some() {
                logic.world.set_keyvalue(id, "origin", value);
            }
        }
        Field::Angles => {
            if vec3(value).is_some() {
                logic.world.set_keyvalue(id, "angles", value);
            }
        }
        Field::Mins | Field::Maxs => {
            if let (Some(v), Some((lo, hi))) = (vec3(value), logic.prop_bounds.get(&id).copied()) {
                if *field == Field::Mins { set_bounds(logic, v, hi) } else { set_bounds(logic, lo, v) }
            }
        }
        Field::KvKey(i) => {
            let keys = editable_keys(&logic.world.ents[id]);
            if let Some((old, v)) = keys.get(*i).cloned() {
                if !value.is_empty() && value != old {
                    logic.world.ents[id].me.ent.0.retain(|(k, _)| *k != old);
                    logic.world.set_keyvalue(id, value, &v);
                }
            }
        }
        Field::KvVal(i) => {
            let keys = editable_keys(&logic.world.ents[id]);
            if let Some((k, _)) = keys.get(*i).cloned() {
                logic.world.set_keyvalue(id, &k, value);
            }
        }
        Field::NewKv => {
            if let Some((k, v)) = value.split_once(char::is_whitespace) {
                if k != "key" {
                    logic.world.set_keyvalue(id, k, v.trim());
                }
            }
        }
        Field::Out(i, part) => {
            if let Some(conn) = logic.world.ents[id].conns.get_mut(*i) {
                match part {
                    0 => conn.output = value.to_string(),
                    1 => conn.target = value.to_string(),
                    2 => conn.input = value.to_string(),
                    3 => conn.param = value.to_string(),
                    _ => conn.delay = value.parse::<f32>().unwrap_or(conn.delay).max(0.),
                }
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn fields(
    mut editor: ResMut<Editor>,
    mut ui: ResMut<UiState>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    clicked: Query<(&Interaction, &FieldBtn), Changed<Interaction>>,
    mut boxes: Query<(&FieldBtn, &mut BackgroundColor)>,
    mut texts: Query<(&FieldText, &mut Text)>,
    mut typed: MessageReader<KeyboardInput>,
) {
    let Some(mut logic) = logic else { return };
    if !editor.editing {
        typed.clear();
        ui.focus = None;
        editor.typing = false;
        return;
    }
    // A click on a field focuses it (committing the previous one).
    let mut newly: Option<(Field, String)> = None;
    for (interaction, f) in &clicked {
        if *interaction == Interaction::Pressed {
            newly = Some((f.0.clone(), f.1.clone()));
        }
    }
    if let Some(next) = newly {
        if let Some((f, v)) = ui.focus.take() {
            if f != next.0 {
                commit(&f, &v, &mut editor, &mut ui, &mut logic);
            }
        }
        let value = if next.0 == Field::NewKv { String::new() } else { next.1 };
        ui.focus = Some((next.0, value));
    } else if editor.lmb.0 && ui.focus.is_some() {
        // Clicking anywhere else applies the edit.
        let (f, v) = ui.focus.take().unwrap();
        commit(&f, &v, &mut editor, &mut ui, &mut logic);
    }
    if let Some((f, mut v)) = ui.focus.take() {
        let mut done = None;
        for e in typed.read().filter(|e| e.state == ButtonState::Pressed) {
            match e.key_code {
                KeyCode::Enter | KeyCode::NumpadEnter => done = Some(true),
                KeyCode::Escape => done = Some(false),
                KeyCode::Backspace => {
                    v.pop();
                }
                _ => {
                    if let Some(t) = &e.text {
                        v.extend(t.chars().filter(|c| !c.is_control()));
                    }
                }
            }
        }
        if f == Field::Search {
            // Live filtering while typing.
            if ui.filter != v {
                ui.filter = v.clone();
                ui.scroll = 0;
            }
        }
        match done {
            Some(true) => commit(&f, &v, &mut editor, &mut ui, &mut logic),
            Some(false) => {}
            None => ui.focus = Some((f, v)),
        }
    } else {
        typed.clear();
    }
    editor.typing = ui.focus.is_some();
    // Show the focused field's text with a caret, others as stored.
    for (ft, mut t) in &mut texts {
        let want = match &ui.focus {
            Some((f, v)) if *f == ft.0 => format!("{v}|"),
            _ => continue,
        };
        if t.0 != want {
            t.0 = want;
        }
    }
    for (fb, mut bg) in &mut boxes {
        let focused = ui.focus.as_ref().is_some_and(|(f, _)| *f == fb.0);
        bg.0 = if focused { FIELD_FOCUS } else { FIELD };
    }
}

fn to_bevy(p: l4d2r_math::Vec3) -> Vec3 {
    Vec3::from_array(crate::binary::source_position(p.to_array()))
}

const AXES: [l4d2r_math::Vec3; 3] = [l4d2r_math::Vec3::X, l4d2r_math::Vec3::Y, l4d2r_math::Vec3::Z];

/// Ray / box (Bevy space) entry distance.
fn ray_box(origin: Vec3, dir: Vec3, lo: Vec3, hi: Vec3) -> Option<f32> {
    let inv = dir.recip();
    let t0 = (lo - origin) * inv;
    let t1 = (hi - origin) * inv;
    let near = t0.min(t1).max_element();
    let far = t0.max(t1).min_element();
    (far >= near.max(0.)).then_some(near.max(0.))
}

fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0., 1.);
    (a + ab * t).distance(p)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn world(
    mut editor: ResMut<Editor>,
    mut ui: ResMut<UiState>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    collision: Option<Res<crate::collision::Collision>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<crate::viewer::GameCamera>>,
    panels: Query<&RelativeCursorPosition, With<Panel>>,
    buttons: Query<&Interaction, With<Button>>,
    time: Res<Time<Real>>,
) {
    let Some(mut logic) = logic else { return };
    ui.hover_place = None;
    ui.hover_handle = None;
    if !editor.editing || editor.rmb_down {
        ui.drag = None;
        return;
    }
    let (Some(window), Some((cam, gt))) = (windows.iter().next(), camera.iter().next()) else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let over_ui = panels.iter().any(|p| p.cursor_over()) || buttons.iter().any(|i| *i != Interaction::None);
    let Ok(ray) = cam.viewport_to_world(gt, cursor) else { return };
    let (origin, dir) = (ray.origin, *ray.direction);
    let grid = editor.grid();
    let selected = editor.selected.filter(|&id| logic.world.ents.get(id).is_some_and(|e| e.alive));
    // Dragging a handle.
    if let Some(drag) = ui.drag {
        if !editor.lmb.1 {
            ui.drag = None;
        } else if let Some(id) = selected {
            let snap = |d: f32| (d / grid).round() * grid;
            match drag {
                Drag::Move { axis, cursor: c0, dir: d, px_per_unit, start } => {
                    let units = snap((cursor - c0).dot(d) / px_per_unit.max(1e-4));
                    let p = start + AXES[axis] * units;
                    logic.world.set_keyvalue(id, "origin", &format!("{} {} {}", p.x, p.y, p.z));
                }
                Drag::Resize { axis, max, cursor: c0, dir: d, px_per_unit, lo, hi } => {
                    let units = snap((cursor - c0).dot(d) / px_per_unit.max(1e-4));
                    let (mut lo, mut hi) = (lo, hi);
                    if max {
                        hi[axis] = (hi[axis] + units).max(lo[axis] + grid);
                    } else {
                        lo[axis] = (lo[axis] + units).min(hi[axis] - grid);
                    }
                    logic.prop_bounds.insert(id, (lo, hi));
                    logic.world.set_keyvalue(id, "mins", &v3(lo));
                    logic.world.set_keyvalue(id, "maxs", &v3(hi));
                }
            }
        }
        return;
    }
    if over_ui {
        return;
    }
    // Handles of the selection: move arrows, or a trigger's face handles.
    let project = |p: Vec3| cam.world_to_viewport(gt, p).ok();
    if let Some(id) = selected {
        let e = &logic.world.ents[id];
        let o = e.me.origin;
        let length_m = (gt.translation().distance(to_bevy(o)) * 0.15).clamp(0.4, 4.);
        let length_u = length_m / UNIT;
        let box_bounds = logic.prop_bounds.get(&id).copied().filter(|_| e.me.mdl.is_none());
        let mut best: Option<(f32, usize, bool, Vec2, Vec2, f32)> = None;
        if ui.tool == Tool::Resize && box_bounds.is_some() {
            let (lo, hi) = box_bounds.unwrap();
            let mid = (lo + hi) * 0.5;
            for axis in 0..3 {
                for max in [false, true] {
                    let mut p = mid;
                    p[axis] = if max { hi[axis] } else { lo[axis] };
                    let (Some(a), Some(b)) = (project(to_bevy(o + p)), project(to_bevy(o + p + AXES[axis] * length_u * 0.5))) else { continue };
                    let d = a.distance(cursor);
                    if d < 12. && best.is_none_or(|b| d < b.0) {
                        best = Some((d, axis, max, a, b, length_u * 0.5));
                    }
                }
            }
        } else {
            for axis in 0..3 {
                let (Some(a), Some(b)) = (project(to_bevy(o)), project(to_bevy(o + AXES[axis] * length_u))) else { continue };
                let d = segment_distance(cursor, a, b);
                if d < 9. && best.is_none_or(|b| d < b.0) {
                    best = Some((d, axis, true, a, b, length_u));
                }
            }
        }
        if let Some((_, axis, max, a, b, units)) = best {
            ui.hover_handle = Some((axis, max));
            if editor.lmb.0 {
                let dir2 = (b - a).normalize_or_zero();
                let px_per_unit = a.distance(b) / units;
                ui.drag = Some(if ui.tool == Tool::Resize && box_bounds.is_some() {
                    let (lo, hi) = box_bounds.unwrap();
                    Drag::Resize { axis, max, cursor, dir: dir2, px_per_unit, lo, hi }
                } else {
                    Drag::Move { axis, cursor, dir: dir2, px_per_unit, start: o }
                });
                return;
            }
        }
    }
    // Where a placement would land: the surface under the cursor, on the grid.
    if editor.placing.is_some() {
        let end = origin + dir * 150.;
        let hit = collision
            .as_ref()
            .and_then(|c| c.source_ray(world_editor::source(origin), world_editor::source(end), 0x1 | 0x2 | 0x8, true))
            .filter(|(f, _, _, _)| *f < 1.)
            .map(|(_, p, _, _)| p)
            .unwrap_or_else(|| world_editor::source(origin + dir * 6.));
        let snap = |v: f32| (v / grid).round() * grid;
        let at = l4d2r_math::Vec3::new(snap(hit[0]), snap(hit[1]), hit[2]);
        ui.hover_place = Some(at);
        if editor.lmb.0 {
            let what = editor.placing.clone().unwrap();
            let breakable = ui.place_breakable;
            let status = world_editor::place(&mut editor, &mut logic, &what, at, 0., breakable);
            editor.status = status;
        }
        return;
    }
    // Click: select what is under the cursor; double-click: properties.
    if editor.lmb.0 {
        let mut best: Option<(f32, usize)> = None;
        let eye = gt.translation();
        for (id, e) in logic.world.ents.iter().enumerate() {
            if !e.alive || (e.me.mdl.is_none() && !editor.objects.contains(&id)) {
                continue;
            }
            let Some((lo, hi)) = logic.bounds(id).or_else(|| {
                // Point entities: a 16-unit box at the origin.
                editor.objects.contains(&id).then(|| {
                    let c = to_bevy(e.me.origin);
                    (c - Vec3::splat(8. * UNIT), c + Vec3::splat(8. * UNIT))
                })
            }) else {
                continue;
            };
            if (lo + hi).distance(eye * 2.) > 200. {
                continue;
            }
            if let Some(t) = ray_box(origin, dir, lo, hi) {
                // Prefer the editor's own objects at equal depth.
                let t = if editor.objects.contains(&id) { t - 0.05 } else { t };
                if best.is_none_or(|b| t < b.0) {
                    best = Some((t, id));
                }
            }
        }
        let now = time.elapsed_secs_f64();
        match best {
            Some((_, id)) => {
                if ui.last_click.is_some_and(|(t, last)| last == id && now - t < 0.4) {
                    ui.props_open = true;
                }
                ui.last_click = Some((now, id));
                editor.selected = Some(id);
                ui.hovered_model = logic.world.ents[id].me.mdl.clone();
            }
            None => {
                editor.selected = None;
                ui.props_open = false;
            }
        }
    }
}

fn draw(editor: Res<Editor>, ui: Res<UiState>, logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>, camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>, mut gizmos: Gizmos) {
    let Some(logic) = logic else { return };
    if !editor.editing {
        return;
    }
    let Some(gt) = camera.iter().next() else { return };
    // Every editor trigger as an orange box; point entities as small boxes.
    for &id in &editor.objects {
        let Some(e) = logic.world.ents.get(id).filter(|e| e.alive) else { continue };
        if e.me.mdl.is_some() {
            continue;
        }
        let (lo, hi) = logic.prop_bounds.get(&id).copied().unwrap_or((l4d2r_math::Vec3::splat(-8.), l4d2r_math::Vec3::splat(8.)));
        let (a, b) = (to_bevy(e.me.origin + lo), to_bevy(e.me.origin + hi));
        let color = if world_editor::is_trigger(&e.me.classname) { Color::srgb(1., 0.55, 0.1) } else { Color::srgb(0.4, 0.9, 1.) };
        gizmos.cube(Transform::from_translation((a + b) * 0.5).with_scale((b - a).abs()), color);
    }
    let Some(id) = editor.selected.filter(|&id| logic.world.ents.get(id).is_some_and(|e| e.alive)) else { return };
    let e = &logic.world.ents[id];
    if let Some((lo, hi)) = logic.bounds(id) {
        // Bright (HDR, so exposure does not dim it) and doubled so it reads
        // against any background.
        let bright = Color::linear_rgb(4., 3.6, 0.6);
        gizmos.cube(Transform::from_translation((lo + hi) * 0.5).with_scale(hi - lo + Vec3::splat(0.02)), bright);
        gizmos.cube(Transform::from_translation((lo + hi) * 0.5).with_scale(hi - lo + Vec3::splat(0.06)), Color::linear_rgb(2., 1.8, 0.3));
    }
    let o = e.me.origin;
    let length_m = (gt.translation().distance(to_bevy(o)) * 0.15).clamp(0.4, 4.);
    let colors = [Color::srgb(1., 0.25, 0.25), Color::srgb(0.3, 1., 0.3), Color::srgb(0.35, 0.55, 1.)];
    let box_bounds = logic.prop_bounds.get(&id).copied().filter(|_| e.me.mdl.is_none());
    if ui.tool == Tool::Resize && box_bounds.is_some() {
        let (lo, hi) = box_bounds.unwrap();
        let mid = (lo + hi) * 0.5;
        for axis in 0..3 {
            for max in [false, true] {
                let mut p = mid;
                p[axis] = if max { hi[axis] } else { lo[axis] };
                let hot = ui.hover_handle == Some((axis, max));
                let c = if hot { Color::srgb(1., 1., 0.2) } else { colors[axis] };
                gizmos.sphere(Isometry3d::from_translation(to_bevy(o + p)), length_m * 0.06, c);
            }
        }
    } else {
        for axis in 0..3 {
            let hot = ui.hover_handle.is_some_and(|(a, _)| a == axis) || matches!(ui.drag, Some(Drag::Move { axis: a, .. }) if a == axis);
            let c = if hot { Color::srgb(1., 1., 0.2) } else { colors[axis] };
            gizmos.arrow(to_bevy(o), to_bevy(o + AXES[axis] * (length_m / UNIT)), c);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_model_parts(
    commands: &mut Commands,
    draws: &crate::runtime_props::Draws,
    source_materials: &Assets<crate::viewer::SourceMaterial>,
    materials: &mut Assets<StandardMaterial>,
    tint: Color,
    blend: bool,
    extra: impl Fn(&mut EntityCommands),
) -> Vec<Entity> {
    draws
        .iter()
        .map(|(mesh, material)| {
            let texture = source_materials.get(material).and_then(|m| m.base.base_color_texture.clone());
            let m = materials.add(StandardMaterial {
                base_color: tint,
                base_color_texture: texture,
                alpha_mode: if blend { AlphaMode::Blend } else { AlphaMode::Mask(0.5) },
                unlit: blend,
                cull_mode: None,
                ..default()
            });
            let mut e = commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(m), Transform::default(), Visibility::Hidden));
            extra(&mut e);
            e.id()
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn ghost(
    mut commands: Commands,
    editor: Res<Editor>,
    mut ui: ResMut<UiState>,
    mut runtime: ResMut<crate::runtime_props::RuntimeModels>,
    source: Option<Res<crate::game_ui::HudSource>>,
    lighting: Res<crate::viewer::LightingTable>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut source_materials: ResMut<Assets<crate::viewer::SourceMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut parts: Query<(&mut Transform, &mut Visibility), With<GhostPart>>,
) {
    let want = editor.placing.clone().filter(|_| editor.editing);
    if ui.ghost_key != want {
        for e in ui.ghost.drain(..) {
            commands.entity(e).try_despawn();
        }
        ui.ghost_key = want.clone();
        match &want {
            Some(Place::Model(path)) => {
                if let Some(Ok(mut assets)) = source.as_ref().map(|s| crate::source_assets::SourceAssets::new(&s.0, vec![])) {
                    if let Some(draws) = crate::runtime_props::draws_for(&mut runtime, &mut assets, path, &mut meshes, &mut source_materials, &mut images, &lighting) {
                        let tint = if ui.place_breakable { Color::srgba(1., 0.4, 0.4, 0.45) } else { Color::srgba(0.75, 0.9, 1., 0.45) };
                        ui.ghost = spawn_model_parts(&mut commands, &draws, &source_materials, &mut materials, tint, true, |e| {
                            e.insert(GhostPart);
                        });
                    }
                }
            }
            Some(Place::Class(class)) => {
                let size = if world_editor::is_trigger(class) { Vec3::new(64., 64., 64.) * UNIT } else { Vec3::splat(16. * UNIT) };
                let mesh = meshes.add(Cuboid::new(size.x, size.y, size.z));
                let m = materials.add(StandardMaterial { base_color: Color::srgba(1., 0.6, 0.15, 0.35), alpha_mode: AlphaMode::Blend, unlit: true, ..default() });
                let offset = if world_editor::is_trigger(class) { Vec3::Y * size.y * 0.5 } else { Vec3::ZERO };
                ui.ghost = vec![commands
                    .spawn((Mesh3d(mesh), MeshMaterial3d(m), Transform::from_translation(offset), Visibility::Hidden, GhostPart))
                    .id()];
            }
            None => {}
        }
    }
    let at = ui.hover_place;
    let is_box = matches!(&want, Some(Place::Class(c)) if world_editor::is_trigger(c));
    for (mut t, mut v) in &mut parts {
        match at {
            Some(p) => {
                let mut transform = crate::l4d2r_cinematic::transform(l4d2r_math::Mat4::from_translation(p));
                if is_box {
                    transform.translation.y += 32. * UNIT;
                }
                *t = transform;
                *v = Visibility::Visible;
            }
            None => *v = Visibility::Hidden,
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn preview(
    mut commands: Commands,
    editor: Res<Editor>,
    mut ui: ResMut<UiState>,
    mut runtime: ResMut<crate::runtime_props::RuntimeModels>,
    source: Option<Res<crate::game_ui::HudSource>>,
    lighting: Res<crate::viewer::LightingTable>,
    time: Res<Time<Real>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut source_materials: ResMut<Assets<crate::viewer::SourceMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut nodes: Query<&mut ImageNode, With<PreviewNode>>,
    mut pivots: Query<&mut Transform, (With<PreviewPivot>, Without<PreviewCamera>)>,
    mut cameras: Query<(&mut Transform, &mut Camera), (With<PreviewCamera>, Without<PreviewPivot>)>,
) {
    let layer = bevy::camera::visibility::RenderLayers::layer(PREVIEW_LAYER);
    let want = ui.hovered_model.clone().filter(|_| editor.editing);
    // The render target, camera, light and turning pivot are made once.
    if ui.preview_image.is_none() {
        let mut image = Image::new_target_texture(286, 200, bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb, None);
        image.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
        let handle = images.add(image);
        commands.spawn((
            Camera3d::default(),
            Camera { order: -40, clear_color: ClearColorConfig::Custom(Color::srgb(0.12, 0.13, 0.15)), is_active: false, ..default() },
            bevy::camera::RenderTarget::Image(handle.clone().into()),
            Transform::from_xyz(0., 0.5, 2.).looking_at(Vec3::ZERO, Vec3::Y),
            layer.clone(),
            AmbientLight { color: Color::WHITE, brightness: 600., affects_lightmapped_meshes: false },
            PreviewCamera,
        ));
        commands.spawn((
            DirectionalLight { illuminance: 6000., ..default() },
            Transform::from_xyz(1., 2., 1.5).looking_at(Vec3::ZERO, Vec3::Y),
            layer.clone(),
        ));
        ui.preview_pivot = Some(commands.spawn((PreviewPivot, Transform::default(), Visibility::Visible, layer.clone())).id());
        ui.preview_image = Some(handle);
    }
    for mut node in &mut nodes {
        if let Some(h) = &ui.preview_image {
            if node.image != *h {
                node.image = h.clone();
            }
        }
    }
    for (_, mut cam) in &mut cameras {
        let active = want.is_some();
        if cam.is_active != active {
            cam.is_active = active;
        }
    }
    if ui.preview_key != want {
        for e in ui.preview.drain(..) {
            commands.entity(e).try_despawn();
        }
        ui.preview_key = want.clone();
        if let (Some(path), Some(Ok(mut assets)), Some(pivot)) =
            (&want, source.as_ref().map(|s| crate::source_assets::SourceAssets::new(&s.0, vec![])), ui.preview_pivot)
        {
            if let Some(draws) = crate::runtime_props::draws_for(&mut runtime, &mut assets, path, &mut meshes, &mut source_materials, &mut images, &lighting) {
                // Frame the model by its mesh bounds, turned to Bevy axes the
                // way props are (Source Z up -> Bevy Y up).
                let turn = crate::l4d2r_cinematic::transform(l4d2r_math::Mat4::IDENTITY);
                let mut lo = Vec3::splat(f32::INFINITY);
                let mut hi = Vec3::splat(f32::NEG_INFINITY);
                for (mesh, _) in &draws {
                    if let Some(bevy::mesh::VertexAttributeValues::Float32x3(ps)) = meshes.get(mesh).and_then(|m| m.attribute(Mesh::ATTRIBUTE_POSITION)) {
                        for p in ps {
                            let p = turn.rotation * Vec3::from_array(*p);
                            lo = lo.min(p);
                            hi = hi.max(p);
                        }
                    }
                }
                let (center, radius) = if lo.is_finite() { ((lo + hi) * 0.5, (hi - lo).length() * 0.5) } else { (Vec3::ZERO, 1.) };
                let parts = spawn_model_parts(&mut commands, &draws, &source_materials, &mut materials, Color::WHITE, false, |e| {
                    e.insert((PreviewPart, layer.clone()));
                });
                for &p in &parts {
                    commands.entity(p).insert((
                        Visibility::Inherited,
                        Transform::from_rotation(turn.rotation).with_translation(-center),
                        ChildOf(pivot),
                    ));
                }
                for (mut t, _) in &mut cameras {
                    let d = radius.max(0.1) * 2.4;
                    *t = Transform::from_xyz(0., d * 0.35, d).looking_at(Vec3::ZERO, Vec3::Y);
                }
                ui.preview = parts;
            }
        }
    }
    // Turn the preview slowly.
    let spin = Quat::from_rotation_y(time.elapsed_secs() * 0.6);
    for mut t in &mut pivots {
        t.rotation = spin;
    }
}
