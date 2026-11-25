//! EDIT WORLD: an in-game map editor that saves retail testscripts.
//!
//! Only offered in single-player CAMPAIGN with `sv_cheats 1` (pause menu,
//! bottom right). Edit mode flies the camera (noclip) with bots stopped;
//! F5 switches between playing and editing. Objects are ordinary entities
//! made by the same code as `prop_dynamic_create` / `ent_create`, so a saved
//! script (`testscripts/`, retail convention: create, test_wait, AddOutput
//! HammerId, ent_keyvalue) rebuilds them in the rewrite and in retail.
//!
//! The interface (crate::edit_world_ui) works like Hammer or Roblox Studio:
//! the cursor stays free, holding RMB looks around (WASD flies), a browser
//! with tabs lists props and entities with a preview, a translucent ghost
//! shows where a pick lands, arrows move the selection by the grid (studs),
//! handles resize triggers, and double-clicking opens its properties.
//! Keys: arrows/PageUp/PageDown nudge by the grid, Q/E rotate 15 degrees,
//! Delete, Ctrl+D duplicate, F5 play/edit, F6 save, F7 load. Enter still
//! opens the inspector line (`apply_line`) for typed edits.
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;

const TURN: f32 = 15.;

/// What the next click in the world places.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Place {
    /// A prop_dynamic with this model.
    Model(String),
    /// A point or trigger entity of this class.
    Class(String),
}

#[derive(Resource, Default)]
pub struct Editor {
    /// Edit mode (false: playing the edited map, or not using the editor).
    pub editing: bool,
    /// Ever opened this map (F5 only works then).
    pub(crate) opened: bool,
    /// Entities this editor created or loaded.
    pub(crate) objects: Vec<usize>,
    pub(crate) selected: Option<usize>,
    pub(crate) models: Vec<String>,
    pub(crate) model: usize,
    pub(crate) search: Option<String>,
    /// The inspector line being typed (Enter opens, Enter applies).
    pub(crate) command: Option<String>,
    pub(crate) next_hammer: u32,
    pub(crate) status: String,
    pub(crate) last_file: Option<String>,
    /// Grid ("studs") in Source units; 0 means the default 8.
    pub(crate) grid: f32,
    pub(crate) placing: Option<Place>,
    /// This frame's left button (just pressed, held, just released) and
    /// whether the right button is held (camera look), captured before the
    /// editor hides them from the game.
    pub(crate) lmb: (bool, bool, bool),
    pub(crate) rmb_down: bool,
    /// Set by the UI while a text field has focus: keys belong to it.
    pub(crate) typing: bool,
}

impl Editor {
    pub(crate) fn grid(&self) -> f32 {
        if self.grid > 0. { self.grid } else { 8. }
    }
}

static EDITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LOOK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// While editing: Some(true) when RMB is held (the game captures the cursor
/// and turns the view), Some(false) for a free cursor; None when not editing.
pub fn edit_cursor() -> Option<bool> {
    EDITING
        .load(std::sync::atomic::Ordering::Relaxed)
        .then(|| LOOK.load(std::sync::atomic::Ordering::Relaxed))
}

/// Marker on the pause menu's EDIT WORLD button.
#[derive(Component)]
pub struct EditWorldButton;

static OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Pause menu's EDIT WORLD: open the editor on the current map.
pub fn request_open() {
    OPEN.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// A CAMPAIGN game with no other human: offline, or hosting a lobby
/// (a private game) that nobody else has joined. Never as a client.
pub fn solo(frontend: &crate::game_menu::Frontend, session: Option<&crate::multiplayer::Session>) -> bool {
    frontend.mode == 0
        && !session.is_some_and(|s| s.core.is_client() || (s.core.online() && s.core.members.iter().any(|m| m.id != s.core.me)))
}

fn cheats() -> bool {
    crate::source_commands::cvar("sv_cheats").is_some_and(|v| v.trim() != "0" && !v.trim().is_empty())
}

/// The EDIT WORLD gate: single-player CAMPAIGN with cheats on.
pub fn allowed(frontend: &crate::game_menu::Frontend, session: Option<&crate::multiplayer::Session>) -> bool {
    solo(frontend, session) && cheats()
}

pub fn install(app: &mut App) {
    app.init_resource::<Editor>()
        .add_systems(PreUpdate, input.after(bevy::ui::UiSystems::Focus))
        .add_systems(Update, button_visibility);
}

fn button_visibility(
    frontend: Res<crate::game_menu::Frontend>,
    session: Option<Res<crate::multiplayer::Session>>,
    mut buttons: Query<&mut Node, With<EditWorldButton>>,
) {
    let show = solo(&frontend, session.as_deref());
    for mut node in &mut buttons {
        let display = if show { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
}

pub(crate) fn set_mode(editor: &mut Editor, console: &mut crate::game_console::Console, editing: bool) {
    editor.editing = editing;
    EDITING.store(editing, std::sync::atomic::Ordering::Relaxed);
    if !editing {
        editor.placing = None;
        LOOK.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    console.noclip = editing;
    crate::source_commands::submit(if editing { "sb_stop 1" } else { "sb_stop 0" });
    // No infected spawn while editing; play mode runs the director again.
    crate::source_commands::submit(if editing { "director_stop" } else { "director_start" });
    editor.status = if editing { "Editing".into() } else { "Playing (F5 to edit)".into() };
}

pub(crate) fn source(v: Vec3) -> [f32; 3] {
    [v.x / crate::collision::UNIT, -v.z / crate::collision::UNIT, v.y / crate::collision::UNIT]
}

#[allow(clippy::too_many_arguments)]
fn input(
    mut editor: ResMut<Editor>,
    mut console: ResMut<crate::game_console::Console>,
    frontend: Res<crate::game_menu::Frontend>,
    session: Option<Res<crate::multiplayer::Session>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut typed: MessageReader<KeyboardInput>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    collision: Option<Res<crate::collision::Collision>>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    root: Option<Res<crate::game_ui::HudSource>>,
    settings: Option<Res<crate::campaign_settings::SessionSettings>>,
) {
    let Some(mut logic) = logic else { return };
    let map = settings.as_ref().and_then(|s| s.map.file_stem()).map_or("map".to_string(), |m| m.to_string_lossy().into_owned());
    let open = OPEN.swap(false, std::sync::atomic::Ordering::Relaxed);
    if open && !allowed(&frontend, session.as_deref()) {
        // The button shows in any solo campaign; editing needs cheats.
        console.push_lines(vec!["EDIT WORLD requires sv_cheats 1 (single-player campaign only).".into()]);
        console.open = true;
    }
    if open && allowed(&frontend, session.as_deref()) {
        editor.opened = true;
        if editor.models.is_empty() {
            if let Some(root) = root.as_ref() {
                if let Ok(assets) = crate::source_assets::SourceAssets::new(&root.0, vec![]) {
                    editor.models = assets.matching_paths(".mdl").into_iter().filter(|m| m.starts_with("models/props")).collect();
                }
            }
            editor.model = editor.models.iter().position(|m| m.contains("props_urban/fence_cover001_128")).unwrap_or(0);
        }
        set_mode(&mut editor, &mut console, true);
    }
    if !editor.opened || frontend.page != crate::game_menu::Page::Game || console.open {
        LOOK.store(false, std::sync::atomic::Ordering::Relaxed);
        return;
    }
    if !allowed(&frontend, session.as_deref()) {
        if editor.editing {
            set_mode(&mut editor, &mut console, false);
        }
        return;
    }
    // Objects a saved script rebuilt (F7, or test_startscript in the console)
    // carry the exporter's l4d2r_edit_* class tag: adopt them, so the next
    // save includes them and the next load replaces them.
    let adopted: Vec<usize> = logic
        .world
        .ents
        .iter()
        .enumerate()
        .filter(|(i, e)| {
            e.alive
                && e.me.kv("classname").is_some_and(|c| c.to_ascii_lowercase().starts_with("l4d2r_edit_"))
                && !editor.objects.contains(i)
        })
        .map(|(i, _)| i)
        .collect();
    editor.objects.extend(adopted);
    if keys.just_pressed(KeyCode::F5) {
        let editing = !editor.editing;
        set_mode(&mut editor, &mut console, editing);
    }
    if !editor.editing {
        return;
    }
    // Model search: "/" starts typing, Enter or Escape ends it.
    if let Some(search) = editor.search.as_mut() {
        let mut done = false;
        for e in typed.read().filter(|e| e.state == ButtonState::Pressed) {
            match e.key_code {
                KeyCode::Enter | KeyCode::Escape => done = true,
                KeyCode::Backspace => {
                    search.pop();
                }
                _ => {
                    if let Some(t) = &e.text {
                        search.extend(t.chars().filter(|c| !c.is_control()));
                    }
                }
            }
        }
        let filter = search.to_ascii_lowercase();
        if let Some(i) = editor.models.iter().position(|m| m.contains(&filter)) {
            editor.model = i;
        }
        if done {
            editor.search = None;
        }
        keys.reset_all();
        mouse.reset_all();
        return;
    }
    // Inspector line: Enter opens it, Enter applies, Escape cancels.
    if let Some(line) = editor.command.as_mut() {
        let mut done = None;
        for e in typed.read().filter(|e| e.state == ButtonState::Pressed) {
            match e.key_code {
                KeyCode::Enter => done = Some(true),
                KeyCode::Escape => done = Some(false),
                KeyCode::Backspace => {
                    line.pop();
                }
                _ => {
                    if let Some(t) = &e.text {
                        line.extend(t.chars().filter(|c| !c.is_control()));
                    }
                }
            }
        }
        if let Some(apply) = done {
            let line = editor.command.take().unwrap_or_default();
            if apply && !line.trim().is_empty() {
                let aim = camera.iter().next().map(|view| {
                    let (eye, forward) = (view.translation(), view.forward().as_vec3());
                    collision
                        .as_ref()
                        .and_then(|c| c.source_ray(source(eye), source(eye + forward * 200.), 0x1 | 0x2 | 0x8, true))
                        .map_or(source(eye + forward * 3.), |(_, p, _, _)| p)
                });
                let at = aim.map_or(l4d2r_math::Vec3::ZERO, l4d2r_math::Vec3::from_array);
                let status = apply_line(&mut editor, &mut logic, &line, at);
                editor.status = status;
            }
        }
        keys.reset_all();
        mouse.reset_all();
        return;
    }
    // Mouse: edit_world_ui handles clicks on panels and in the world; keep
    // this frame's buttons for it, then hide them from the game so editing
    // never fires a weapon.
    editor.lmb = (
        mouse.just_pressed(MouseButton::Left),
        mouse.pressed(MouseButton::Left),
        mouse.just_released(MouseButton::Left),
    );
    editor.rmb_down = mouse.pressed(MouseButton::Right);
    LOOK.store(editor.rmb_down, std::sync::atomic::Ordering::Relaxed);
    // The buttons stay as they are: `reset` would also clear "pressed"
    // until the next physical press, so a held RMB turned the view for one
    // frame and a held LMB could not drag the move arrows. The game's own
    // input (game::controls, carryables) ignores the mouse while editing.
    if editor.typing {
        // A UI text field owns the keyboard (edit_world_ui reads the typed
        // characters); WASD must not fly and shortcuts must not fire.
        keys.reset_all();
        return;
    }
    typed.clear();
    // (Enter used to open a typed inspector line. Its on-screen overlay is
    // gone with the panel UI, so the hidden line swallowed every key until
    // Enter/Escape: "typing works, then nothing types". The properties
    // panel replaces it.)
    if keys.just_pressed(KeyCode::Escape) && editor.placing.is_some() {
        editor.placing = None;
        editor.status = "Placing cancelled".into();
    }
    let Some(view) = camera.iter().next() else { return };
    let forward = view.forward().as_vec3();
    let flat = Vec3::new(forward.x, 0., forward.z).normalize_or(Vec3::NEG_Z);
    // Arrows nudge along the view's nearest world axis by the grid.
    let axis = |v: Vec3| {
        let s = source(v);
        if s[0].abs() > s[1].abs() { l4d2r_math::Vec3::new(s[0].signum(), 0., 0.) } else { l4d2r_math::Vec3::new(0., s[1].signum(), 0.) }
    };
    let grid = editor.grid();
    let ahead = axis(flat);
    let side = axis(Vec3::new(-flat.z, 0., flat.x));
    for (key, delta) in [
        (KeyCode::ArrowUp, ahead * grid),
        (KeyCode::ArrowDown, ahead * -grid),
        (KeyCode::ArrowRight, side * grid),
        (KeyCode::ArrowLeft, side * -grid),
        (KeyCode::PageUp, l4d2r_math::Vec3::new(0., 0., grid)),
        (KeyCode::PageDown, l4d2r_math::Vec3::new(0., 0., -grid)),
    ] {
        if keys.just_pressed(key) {
            nudge(&mut editor, &mut logic, delta);
        }
    }
    if keys.just_pressed(KeyCode::KeyQ) {
        rotate(&mut editor, &mut logic, TURN);
    }
    if keys.just_pressed(KeyCode::KeyE) {
        rotate(&mut editor, &mut logic, -TURN);
    }
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    if ctrl && keys.just_pressed(KeyCode::KeyD) {
        duplicate(&mut editor, &mut logic);
    }
    if keys.just_pressed(KeyCode::Delete) {
        delete(&mut editor, &mut logic);
    }
    if keys.just_pressed(KeyCode::F6) {
        if let Some(root) = root.as_ref() {
            editor.status = save(&mut editor, &logic, &root.0, &map);
        }
    }
    if keys.just_pressed(KeyCode::F7) {
        load_last(&mut editor, &mut logic);
    }
    let _ = collision;
}

fn alive(logic: &crate::l4d2r_cinematic::MapLogic, id: usize) -> bool {
    logic.world.ents.get(id).is_some_and(|e| e.alive)
}

/// Classes placed as box triggers (editable mins/maxs, resize handles).
pub(crate) fn is_trigger(class: &str) -> bool {
    class.starts_with("trigger_")
}

/// Places `what` at `at` (Source units); returns the status line.
pub(crate) fn place(
    editor: &mut Editor,
    logic: &mut crate::l4d2r_cinematic::MapLogic,
    what: &Place,
    at: l4d2r_math::Vec3,
    yaw: f32,
    breakable: bool,
) -> String {
    let hammer = next_hammer(editor, logic);
    let name = format!("edit_{hammer}");
    let mut keys = vec![("targetname".to_string(), name.clone()), ("hammerid".to_string(), hammer.to_string())];
    let id = match what {
        Place::Model(_) if breakable => {
            return "That model is breakable: retail prop_dynamic refuses it. Pick another.".into();
        }
        Place::Model(model) => {
            keys.push(("model".into(), model.clone()));
            keys.push(("solid".into(), "6".into()));
            logic.world.create_entity("prop_dynamic", &keys, at, l4d2r_math::Vec3::new(0., yaw, 0.), Some(model.clone()))
        }
        Place::Class(class) if is_trigger(class) => {
            // A 64 x 64 x 64 box standing on the floor; Source's mins/maxs
            // keyvalues (also how the exported script sizes it in retail).
            let (lo, hi) = (l4d2r_math::Vec3::new(-32., -32., 0.), l4d2r_math::Vec3::new(32., 32., 64.));
            keys.push(("mins".into(), "-32 -32 0".into()));
            keys.push(("maxs".into(), "32 32 64".into()));
            keys.push(("solid".into(), "2".into()));
            keys.push(("spawnflags".into(), "1".into()));
            let id = logic.world.create_entity(class, &keys, at, l4d2r_math::Vec3::ZERO, None);
            logic.prop_bounds.insert(id, (lo, hi));
            logic.triggers.push(id);
            id
        }
        Place::Class(class) => logic.world.create_entity(class, &keys, at, l4d2r_math::Vec3::ZERO, None),
    };
    crate::dev_console::SPAWNED.lock().unwrap().push(id);
    editor.objects.push(id);
    editor.selected = Some(id);
    format!("Placed {name}")
}

/// Moves the selection by `delta` (Source units).
pub(crate) fn nudge(editor: &mut Editor, logic: &mut crate::l4d2r_cinematic::MapLogic, delta: l4d2r_math::Vec3) {
    let Some(id) = editor.selected.filter(|&id| alive(logic, id)) else { return };
    let o = logic.world.ents[id].me.origin + delta;
    logic.world.set_keyvalue(id, "origin", &format!("{} {} {}", o.x, o.y, o.z));
}

/// Turns the selection about the vertical axis.
pub(crate) fn rotate(editor: &mut Editor, logic: &mut crate::l4d2r_cinematic::MapLogic, degrees: f32) {
    let Some(id) = editor.selected.filter(|&id| alive(logic, id)) else { return };
    let mut a = logic.world.ents[id].me.angles;
    a.y = (a.y + degrees).rem_euclid(360.);
    logic.world.set_keyvalue(id, "angles", &format!("{} {} {}", a.x, a.y, a.z));
}

pub(crate) fn duplicate(editor: &mut Editor, logic: &mut crate::l4d2r_cinematic::MapLogic) {
    let Some(id) = editor.selected.filter(|&id| alive(logic, id)) else { return };
    let e = logic.world.ents[id].clone();
    let hammer = next_hammer(editor, logic);
    let mut kv: Vec<(String, String)> = e.me.ent.0.iter().filter(|(k, _)| !matches!(k.to_ascii_lowercase().as_str(), "classname" | "hammerid" | "targetname")).cloned().collect();
    kv.push(("targetname".into(), format!("edit_{hammer}")));
    kv.push(("hammerid".into(), hammer.to_string()));
    let offset = l4d2r_math::Vec3::new(editor.grid() * 4., 0., 0.);
    let copy = logic.world.create_entity(&e.me.classname, &kv, e.me.origin + offset, e.me.angles, e.me.mdl.clone());
    logic.world.ents[copy].conns = e.conns.clone();
    if let Some(bounds) = logic.prop_bounds.get(&id).copied().filter(|_| e.me.mdl.is_none()) {
        logic.prop_bounds.insert(copy, bounds);
        if is_trigger(&e.me.classname) {
            logic.triggers.push(copy);
        }
    }
    crate::dev_console::SPAWNED.lock().unwrap().push(copy);
    editor.objects.push(copy);
    editor.selected = Some(copy);
    editor.status = format!("Duplicated as edit_{hammer}");
}

pub(crate) fn delete(editor: &mut Editor, logic: &mut crate::l4d2r_cinematic::MapLogic) {
    let Some(id) = editor.selected.filter(|&id| alive(logic, id)) else { return };
    logic.world.input(id, "Kill", "", None, None);
    editor.objects.retain(|&o| o != id);
    editor.selected = None;
    editor.status = "Deleted".into();
}

/// F7: rebuild from the last saved script, replacing this session's objects.
pub(crate) fn load_last(editor: &mut Editor, logic: &mut crate::l4d2r_cinematic::MapLogic) {
    match editor.last_file.clone() {
        Some(file) => {
            // The script rebuilds everything: remove this session's
            // objects first (the script's own header removes objects an
            // earlier run made), so loading never duplicates the build.
            for id in std::mem::take(&mut editor.objects) {
                if alive(logic, id) {
                    logic.world.input(id, "Kill", "", None, None);
                }
            }
            editor.selected = None;
            crate::source_commands::submit(&format!("test_startscript {file}"));
            editor.status = format!("Loading {file}");
        }
        None => editor.status = "Nothing saved yet (F6)".into(),
    }
}

/// Applies one inspector line to the selected object; returns the status.
///
/// - `key value...`: a keyvalue (`name` is short for targetname;
///   `classname` renames the class used for targeting, as retail's
///   ClassName keyvalue does).
/// - `OnOutput target,input[,param[,delay[,times]]]`: adds an output.
/// - `-key` / `-OnOutput`: removes that keyvalue / every such output.
/// - `give <item>` / `health`: OnTakeDamage runs `give <item>` through a
///   point_clientcommand named PD1 (created if missing), as bmap.txt does.
/// - `new <classname>`: a logic entity at the crosshair (selected).
pub fn apply_line(editor: &mut Editor, logic: &mut crate::l4d2r_cinematic::MapLogic, line: &str, at: l4d2r_math::Vec3) -> String {
    let line = line.trim();
    let (word, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(w, r)| (w, r.trim()));
    let lower = word.to_ascii_lowercase();
    if lower == "new" {
        if rest.is_empty() || rest.contains(char::is_whitespace) {
            return "Usage: new <classname> (e.g. new logic_relay)".into();
        }
        let hammer = next_hammer(editor, logic);
        let name = format!("edit_{hammer}");
        let kv = vec![("targetname".to_string(), name.clone()), ("hammerid".to_string(), hammer.to_string())];
        let id = logic.world.create_entity(&rest.to_ascii_lowercase(), &kv, at, l4d2r_math::Vec3::ZERO, None);
        crate::dev_console::SPAWNED.lock().unwrap().push(id);
        editor.objects.push(id);
        editor.selected = Some(id);
        return format!("Created {} {name}", rest.to_ascii_lowercase());
    }
    let Some(id) = editor.selected.filter(|&id| logic.world.ents.get(id).is_some_and(|e| e.alive)) else {
        return "Select an object first (RMB), or: new <classname>".into();
    };
    if lower == "give" || lower == "health" {
        let item = if lower == "health" { "health" } else { rest };
        if item.is_empty() || item.contains(char::is_whitespace) {
            return "Usage: give <item> (e.g. give rifle_ak47, give first_aid_kit)".into();
        }
        if logic.world.find("PD1").is_empty() {
            let hammer = next_hammer(editor, logic);
            let kv = vec![("targetname".to_string(), "PD1".to_string()), ("hammerid".to_string(), hammer.to_string())];
            let pd = logic.world.create_entity("point_clientcommand", &kv, l4d2r_math::Vec3::ZERO, l4d2r_math::Vec3::ZERO, None);
            crate::dev_console::SPAWNED.lock().unwrap().push(pd);
            editor.objects.push(pd);
        }
        logic.world.set_keyvalue(id, "OnTakeDamage", &format!("PD1,Command,give {item}"));
        return format!("OnTakeDamage -> PD1 give {item} (damage it to test, F5)");
    }
    if let Some(key) = word.strip_prefix('-') {
        let key_lower = key.to_ascii_lowercase();
        let e = &mut logic.world.ents[id];
        if key_lower.starts_with("on") {
            let before = e.conns.len();
            e.conns.retain(|c| !c.output.eq_ignore_ascii_case(key));
            return format!("Removed {} {key} output(s)", before - e.conns.len());
        }
        if matches!(key_lower.as_str(), "classname" | "hammerid" | "targetname" | "model" | "origin" | "angles") {
            return format!("{key} can't be removed (set a new value instead)");
        }
        let before = e.me.ent.0.len();
        e.me.ent.0.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
        return if e.me.ent.0.len() < before { format!("Removed {key}") } else { format!("No keyvalue {key}") };
    }
    if rest.is_empty() {
        return "Usage: <key> <value>, <OnOutput> <target,input,param>, -<key>, give <item>, new <class>".into();
    }
    let key = if lower == "name" { "targetname" } else { word };
    if key.to_ascii_lowercase().starts_with("on") && rest.split(',').count() < 2 {
        return format!("Usage: {key} <target>,<input>[,<param>[,<delay>]]");
    }
    if key.eq_ignore_ascii_case("hammerid") && rest.parse::<u32>().map_or(true, |n| n <= 1000) {
        return "HammerId must be a number above 1000".into();
    }
    logic.world.set_keyvalue(id, key, rest);
    format!("{key} = {rest}")
}

/// The lowest unused HammerId above 1000.
pub(crate) fn next_hammer(editor: &mut Editor, logic: &crate::l4d2r_cinematic::MapLogic) -> u32 {
    let used: std::collections::HashSet<u32> = logic.world.ents.iter().filter_map(|e| e.me.kv("hammerid")?.trim().parse().ok()).collect();
    let mut id = editor.next_hammer.max(1001);
    while used.contains(&id) {
        id += 1;
    }
    editor.next_hammer = id + 1;
    id
}

/// Writes `testscripts/edit_<map>.txt` in retail's convention.
pub(crate) fn save(editor: &mut Editor, logic: &crate::l4d2r_cinematic::MapLogic, root: &std::path::Path, map: &str) -> String {
    let text = export(logic, &editor.objects, map);
    let file = format!("edit_{map}.txt");
    let dir = root.join("left4dead2/testscripts");
    let dir = if dir.is_dir() { dir } else { std::path::PathBuf::from("testscripts") };
    let _ = std::fs::create_dir_all(&dir);
    match std::fs::write(dir.join(&file), text) {
        Ok(()) => {
            editor.last_file = Some(file.clone());
            format!("Saved {} (run: sv_cheats 1; test_startscript {file})", dir.join(&file).display())
        }
        Err(e) => format!("Save failed: {e}"),
    }
}

fn quote(s: &str) -> String {
    s.replace(['"', ';', '\r', '\n'], "")
}

/// The testscript for `objects`: props as prop_dynamic_create / test_wait /
/// AddOutput HammerId / ent_keyvalue (the user's bmap.txt and DeadForge
/// convention, existing props renamed first so the HammerId lands on the new
/// one only); other entities as `ent_create class key value ...`.
pub fn export(logic: &crate::l4d2r_cinematic::MapLogic, objects: &[usize], map: &str) -> String {
    let wait = "test_wait .05;";
    let mut out = vec![
        "sv_cheats 1;".to_string(),
        format!("// L4D2R EDIT WORLD - {map}"),
        "// Load the map, then run: sv_cheats 1; test_startscript <this file>".into(),
        "// Remove what an earlier run of this script made, so loading it again".into(),
        "// replaces the build instead of duplicating it.".into(),
        "ent_fire l4d2r_edit_* kill;".into(),
        wait.into(),
        "// Isolate existing props before assigning Hammer IDs.".into(),
        "ent_fire prop_dynamic addoutput \"classname l4d2r_existing_dynamic\";".into(),
        wait.into(),
        String::new(),
    ];
    let f = |v: l4d2r_math::Vec3| format!("{} {} {}", round(v.x), round(v.y), round(v.z));
    for &id in objects {
        let Some(e) = logic.world.ents.get(id).filter(|e| e.alive) else { continue };
        let hammer = e.me.kv("hammerid").unwrap_or("0").trim().to_string();
        // l4d2r_* class names are this exporter's own tags (an object rebuilt
        // from a saved script): export its real class again.
        let class = e
            .me
            .kv("classname")
            .filter(|c| !c.to_ascii_lowercase().starts_with("l4d2r_"))
            .unwrap_or(&e.me.classname)
            .to_string();
        let extra: Vec<String> = e
            .me
            .ent
            .0
            .iter()
            .filter(|(k, _)| !matches!(k.to_ascii_lowercase().as_str(), "classname" | "hammerid" | "targetname" | "origin" | "angles" | "model"))
            .map(|(k, v)| format!("\"{}\" \"{}\"", quote(k), quote(v)))
            .collect();
        out.push(format!("// {}", quote(&e.me.targetname)));
        if let Some(model) = e.me.mdl.as_ref().filter(|_| e.me.classname.starts_with("prop_dynamic") || class.starts_with("prop_dynamic") || e.me.mdl.is_some()) {
            let short = model.trim_start_matches("models/").trim_end_matches(".mdl");
            out.push(format!("prop_dynamic_create {short};"));
            out.push(wait.into());
            out.push(format!("ent_fire prop_dynamic addoutput \"HammerId {hammer}\";"));
            out.push(wait.into());
            out.push(format!(
                "ent_keyvalue {hammer} \"TargetName\" \"{}\" \"ClassName\" \"{}\" \"angles\" \"{}\" \"origin\" \"{}\";",
                quote(&e.me.targetname),
                quote(if class == "prop_dynamic" { "l4d2r_edit_prop" } else { &class }),
                f(e.me.angles),
                f(e.me.origin)
            ));
        } else {
            // Retail ent_create ignores keyvalues (checked 24 Nov: the
            // entity spawns unnamed), so logic entities follow the same
            // create / AddOutput HammerId / ent_keyvalue pattern as props,
            // after renaming that class's existing entities.
            // Named straight after creation with AddOutput, as the user's
            // bmap.txt does for PD1 (retail-proven); outputs use AddOutput's
            // "target:input:param:delay:times" form. The class rename comes
            // last so the AddOutputs above still reach the new entity.
            let class_name = quote(&e.me.classname);
            let name = quote(&e.me.targetname);
            out.push(format!("ent_fire {class_name} addoutput \"classname l4d2r_existing_{class_name}\";"));
            out.push(wait.into());
            out.push(format!("ent_create {class_name};"));
            out.push(wait.into());
            out.push(format!("ent_fire {class_name} addoutput \"TargetName {name}\";"));
            out.push(format!("ent_fire {class_name} addoutput \"HammerId {hammer}\";"));
            out.push(format!("ent_fire {class_name} addoutput \"origin {}\";", f(e.me.origin)));
            out.push(format!("ent_fire {class_name} addoutput \"angles {}\";", f(e.me.angles)));
            for (k, v) in e.me.ent.0.iter().filter(|(k, _)| !matches!(k.to_ascii_lowercase().as_str(), "classname" | "hammerid" | "targetname" | "origin" | "angles" | "model")) {
                out.push(format!("ent_fire {class_name} addoutput \"{} {}\";", quote(k), quote(v)));
            }
            for c in &e.conns {
                out.push(format!(
                    "ent_fire {class_name} addoutput \"{} {}:{}:{}:{}:{}\";",
                    quote(&c.output), quote(&c.target), quote(&c.input), quote(&c.param), c.delay, c.times
                ));
            }
            out.push(wait.into());
            out.push(format!("ent_fire {name} addoutput \"classname l4d2r_edit_{class_name}\";"));
            out.push(String::new());
            continue;
        }
        if !extra.is_empty() {
            out.push(format!("ent_keyvalue {hammer} {};", extra.join(" ")));
        }
        for c in &e.conns {
            let mut value = format!("{},{}", c.target, c.input);
            if !c.param.is_empty() || c.delay != 0. {
                value.push_str(&format!(",{}", c.param));
            }
            if c.delay != 0. {
                value.push_str(&format!(",{}", c.delay));
            }
            out.push(format!("ent_keyvalue {hammer} \"{}\" \"{}\";", quote(&c.output), quote(&value)));
        }
        out.push(String::new());
    }
    out.push(wait.into());
    out.push(format!("say {} objects loaded - {map};", objects.len()));
    out.join("\n") + "\n"
}

fn round(v: f32) -> f32 {
    (v * 1000.).round() / 1000.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_writes_retail_testscript_commands() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &root.join("left4dead2/maps/c2m4_barns.bsp"), false).unwrap();
        let kv = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        let crate_id = logic.world.create_entity(
            "prop_dynamic",
            &kv(&[("model", "models/props_urban/fence_cover001_128.mdl"), ("targetname", "crate1"), ("hammerid", "1001"), ("rendercolor", "0 255 0")]),
            l4d2r_math::Vec3::new(-4300., 2500., -250.),
            l4d2r_math::Vec3::new(0., 90., 0.),
            Some("models/props_urban/fence_cover001_128.mdl".into()),
        );
        logic.world.set_keyvalue(crate_id, "OnTakeDamage", "PD1,Command,give health");
        let pd = logic.world.create_entity("point_clientcommand", &kv(&[("targetname", "PD1"), ("hammerid", "1002")]), l4d2r_math::Vec3::ZERO, l4d2r_math::Vec3::ZERO, None);
        let text = export(&logic, &[crate_id, pd], "c2m4_barns");
        assert!(text.contains("prop_dynamic_create props_urban/fence_cover001_128;"), "{text}");
        // Re-running the script replaces its earlier objects.
        assert!(text.find("ent_fire l4d2r_edit_* kill;") < text.find("prop_dynamic_create"), "{text}");
        assert!(text.contains("ent_fire prop_dynamic addoutput \"HammerId 1001\";"), "{text}");
        assert!(text.contains("ent_keyvalue 1001 \"TargetName\" \"crate1\""), "{text}");
        assert!(text.contains("\"origin\" \"-4300 2500 -250\""), "{text}");
        assert!(text.contains("ent_keyvalue 1001 \"OnTakeDamage\" \"PD1,Command,give health\";"), "{text}");
        assert!(text.contains("\"rendercolor\" \"0 255 0\""), "{text}");
        assert!(text.contains("ent_create point_clientcommand;"), "{text}");
        assert!(text.contains("ent_fire point_clientcommand addoutput \"TargetName PD1\";"), "{text}");
        assert!(text.contains("ent_fire PD1 addoutput \"classname l4d2r_edit_point_clientcommand\";"), "{text}");
        // The inspector line builds the same objects.
        let mut editor = Editor { selected: Some(crate_id), objects: vec![crate_id], ..Default::default() };
        assert_eq!(apply_line(&mut editor, &mut logic, "name fence2", l4d2r_math::Vec3::ZERO), "targetname = fence2");
        assert_eq!(logic.world.ents[crate_id].me.targetname, "fence2");
        apply_line(&mut editor, &mut logic, "give rifle_ak47", l4d2r_math::Vec3::ZERO);
        assert!(logic.world.ents[crate_id].conns.iter().any(|c| c.output == "OnTakeDamage" && c.param == "give rifle_ak47"));
        assert!(apply_line(&mut editor, &mut logic, "-OnTakeDamage", l4d2r_math::Vec3::ZERO).starts_with("Removed 2"));
        assert!(logic.world.ents[crate_id].conns.is_empty());
        assert_eq!(apply_line(&mut editor, &mut logic, "hammerid 12", l4d2r_math::Vec3::ZERO), "HammerId must be a number above 1000");
        let relay = apply_line(&mut editor, &mut logic, "new logic_relay", l4d2r_math::Vec3::new(1., 2., 3.));
        assert!(relay.starts_with("Created logic_relay edit_"), "{relay}");
        assert_eq!(editor.objects.len(), 2);
        // Every line splits into well-formed commands (quotes balanced).
        for command in crate::source_commands::split(&text) {
            assert_eq!(command.matches('"').count() % 2, 0, "{command}");
        }
    }
}
