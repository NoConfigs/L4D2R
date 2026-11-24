//! EDIT WORLD: an in-game map editor that saves retail testscripts.
//!
//! Only offered in single-player CAMPAIGN with `sv_cheats 1` (pause menu,
//! bottom right). Edit mode flies the camera (noclip) with bots stopped;
//! F5 switches between playing and editing. Objects are ordinary entities
//! made by the same code as `prop_dynamic_create` / `ent_create`, so a saved
//! script (`testscripts/`, retail convention: create, test_wait, AddOutput
//! HammerId, ent_keyvalue) rebuilds them in the rewrite and in retail.
//!
//! Keys while editing: LMB place, RMB select, [ ] previous/next model,
//! / search models, arrows and PageUp/PageDown move 8 units, Q/E rotate 15
//! degrees, Delete remove, Ctrl+D duplicate, F5 play/edit, F6 save,
//! F7 load the last saved file.
use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;

const SNAP: f32 = 8.;
const TURN: f32 = 15.;

#[derive(Resource, Default)]
pub struct Editor {
    /// Edit mode (false: playing the edited map, or not using the editor).
    pub editing: bool,
    /// Ever opened this map (F5 only works then).
    opened: bool,
    /// Entities this editor created or loaded.
    objects: Vec<usize>,
    selected: Option<usize>,
    models: Vec<String>,
    model: usize,
    search: Option<String>,
    next_hammer: u32,
    status: String,
    last_file: Option<String>,
}

/// Marker on the pause menu's EDIT WORLD button.
#[derive(Component)]
pub struct EditWorldButton;
#[derive(Component)]
struct Overlay;

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
        .add_systems(PreUpdate, input.after(bevy::input::InputSystems))
        .add_systems(Update, (button_visibility, overlay));
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

fn set_mode(editor: &mut Editor, console: &mut crate::game_console::Console, editing: bool) {
    editor.editing = editing;
    console.noclip = editing;
    crate::source_commands::submit(if editing { "sb_stop 1" } else { "sb_stop 0" });
    editor.status = if editing { "Editing".into() } else { "Playing (F5 to edit)".into() };
}

fn source(v: Vec3) -> [f32; 3] {
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
        return;
    }
    if !allowed(&frontend, session.as_deref()) {
        if editor.editing {
            set_mode(&mut editor, &mut console, false);
        }
        return;
    }
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
    typed.clear();
    if keys.just_pressed(KeyCode::Slash) {
        editor.search = Some(String::new());
    }
    let count = editor.models.len().max(1);
    if keys.just_pressed(KeyCode::BracketRight) {
        editor.model = (editor.model + 1) % count;
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        editor.model = (editor.model + count - 1) % count;
    }
    let Some(view) = camera.iter().next() else { return };
    let (eye, forward) = (view.translation(), view.forward().as_vec3());
    let left = mouse.just_pressed(MouseButton::Left);
    let right = mouse.just_pressed(MouseButton::Right);
    // The editor owns the mouse: no shots while editing.
    mouse.reset(MouseButton::Left);
    mouse.reset(MouseButton::Right);
    // Retail prop_dynamic refuses breakable models, so the editor does too.
    let breakable = |model: &str| {
        root.as_ref()
            .and_then(|r| crate::source_assets::SourceAssets::new(&r.0, vec![]).ok())
            .is_some_and(|mut a| crate::source_breakables::model_health(&mut a, model) > 0.)
    };
    if left && editor.models.get(editor.model).is_some_and(|m| breakable(m)) {
        editor.status = "That model is breakable: retail prop_dynamic refuses it. Pick another.".into();
    } else if left {
        if let Some(model) = editor.models.get(editor.model).cloned() {
            let end = eye + forward * 200.;
            let at = collision
                .as_ref()
                .and_then(|c| c.source_ray(source(eye), source(end), 0x1 | 0x2 | 0x8, true))
                .map_or(source(eye + forward * 3.), |(_, p, _, _)| p);
            let snap = |v: f32| (v / SNAP).round() * SNAP;
            let at = l4d2r_math::Vec3::new(snap(at[0]), snap(at[1]), at[2]);
            let yaw = ((-forward.z).atan2(forward.x).to_degrees() / TURN).round() * TURN + 180.;
            let hammer = next_hammer(&mut editor, &logic);
            let name = format!("edit_{hammer}");
            let keys = vec![
                ("model".to_string(), model.clone()),
                ("solid".to_string(), "6".to_string()),
                ("targetname".to_string(), name.clone()),
                ("hammerid".to_string(), hammer.to_string()),
            ];
            let id = logic.world.create_entity("prop_dynamic", &keys, at, l4d2r_math::Vec3::new(0., yaw, 0.), Some(model));
            crate::dev_console::SPAWNED.lock().unwrap().push(id);
            editor.objects.push(id);
            editor.selected = Some(id);
            editor.status = format!("Placed {name}");
        }
    }
    if right {
        editor.selected = pick(&logic, eye, forward);
    }
    let Some(id) = editor.selected.filter(|&id| logic.world.ents.get(id).is_some_and(|e| e.alive)) else { return };
    let mut origin = logic.world.ents[id].me.origin;
    let mut angles = logic.world.ents[id].me.angles;
    let flat = Vec3::new(forward.x, 0., forward.z).normalize_or(Vec3::NEG_Z);
    // Arrows move along the view's nearest world axis, like DeadForge.
    let axis = |v: Vec3| {
        let s = source(v);
        if s[0].abs() > s[1].abs() { l4d2r_math::Vec3::new(s[0].signum(), 0., 0.) } else { l4d2r_math::Vec3::new(0., s[1].signum(), 0.) }
    };
    let ahead = axis(flat);
    let side = axis(Vec3::new(-flat.z, 0., flat.x));
    let mut moved = false;
    for (key, delta) in [
        (KeyCode::ArrowUp, ahead * SNAP),
        (KeyCode::ArrowDown, ahead * -SNAP),
        (KeyCode::ArrowRight, side * SNAP),
        (KeyCode::ArrowLeft, side * -SNAP),
        (KeyCode::PageUp, l4d2r_math::Vec3::new(0., 0., SNAP)),
        (KeyCode::PageDown, l4d2r_math::Vec3::new(0., 0., -SNAP)),
    ] {
        if keys.just_pressed(key) {
            origin += delta;
            moved = true;
        }
    }
    if keys.just_pressed(KeyCode::KeyQ) {
        angles.y = (angles.y + TURN).rem_euclid(360.);
        moved = true;
    }
    if keys.just_pressed(KeyCode::KeyE) {
        angles.y = (angles.y - TURN).rem_euclid(360.);
        moved = true;
    }
    if moved {
        let f = |v: l4d2r_math::Vec3| format!("{} {} {}", v.x, v.y, v.z);
        logic.world.set_keyvalue(id, "origin", &f(origin));
        logic.world.set_keyvalue(id, "angles", &f(angles));
    }
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    if ctrl && keys.just_pressed(KeyCode::KeyD) {
        let e = logic.world.ents[id].clone();
        let hammer = next_hammer(&mut editor, &logic);
        let mut kv: Vec<(String, String)> = e.me.ent.0.iter().filter(|(k, _)| !matches!(k.to_ascii_lowercase().as_str(), "classname" | "hammerid" | "targetname")).cloned().collect();
        kv.push(("targetname".into(), format!("edit_{hammer}")));
        kv.push(("hammerid".into(), hammer.to_string()));
        let copy = logic.world.create_entity(&e.me.classname, &kv, origin + l4d2r_math::Vec3::new(SNAP * 4., 0., 0.), angles, e.me.mdl.clone());
        logic.world.ents[copy].conns = e.conns.clone();
        crate::dev_console::SPAWNED.lock().unwrap().push(copy);
        editor.objects.push(copy);
        editor.selected = Some(copy);
        editor.status = format!("Duplicated as edit_{hammer}");
    }
    if keys.just_pressed(KeyCode::Delete) {
        logic.world.input(id, "Kill", "", None, None);
        editor.selected = None;
        editor.status = "Deleted".into();
    }
    if keys.just_pressed(KeyCode::F6) {
        if let Some(root) = root.as_ref() {
            editor.status = save(&mut editor, &logic, &root.0, &map);
        }
    }
    if keys.just_pressed(KeyCode::F7) {
        match editor.last_file.clone() {
            Some(file) => {
                crate::source_commands::submit(&format!("test_startscript {file}"));
                editor.status = format!("Loading {file}");
            }
            None => editor.status = "Nothing saved yet (F6)".into(),
        }
    }
}

/// The lowest unused HammerId above 1000.
fn next_hammer(editor: &mut Editor, logic: &crate::l4d2r_cinematic::MapLogic) -> u32 {
    let used: std::collections::HashSet<u32> = logic.world.ents.iter().filter_map(|e| e.me.kv("hammerid")?.trim().parse().ok()).collect();
    let mut id = editor.next_hammer.max(1001);
    while used.contains(&id) {
        id += 1;
    }
    editor.next_hammer = id + 1;
    id
}

/// The entity nearest the crosshair ray (any alive entity with a model).
fn pick(logic: &crate::l4d2r_cinematic::MapLogic, eye: Vec3, forward: Vec3) -> Option<usize> {
    logic
        .world
        .ents
        .iter()
        .enumerate()
        .filter(|(_, e)| e.alive && (e.me.mdl.is_some() || e.me.brush.is_some()))
        .filter_map(|(i, e)| {
            let o = e.me.origin;
            let p = Vec3::new(o.x, o.z, -o.y) * crate::collision::UNIT;
            let to = p - eye;
            let along = to.dot(forward);
            (along > 0. && along < 60.).then(|| (i, (to - forward * along).length() / along))
        })
        .filter(|(_, off)| *off < 0.12)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Writes `testscripts/edit_<map>.txt` in retail's convention.
fn save(editor: &mut Editor, logic: &crate::l4d2r_cinematic::MapLogic, root: &std::path::Path, map: &str) -> String {
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
        "// Isolate existing props before assigning Hammer IDs.".into(),
        "ent_fire prop_dynamic addoutput \"classname l4d2r_existing_dynamic\";".into(),
        wait.into(),
        String::new(),
    ];
    let f = |v: l4d2r_math::Vec3| format!("{} {} {}", round(v.x), round(v.y), round(v.z));
    for &id in objects {
        let Some(e) = logic.world.ents.get(id).filter(|e| e.alive) else { continue };
        let hammer = e.me.kv("hammerid").unwrap_or("0").trim().to_string();
        let class = e.me.kv("classname").unwrap_or(&e.me.classname).to_string();
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

fn overlay(
    mut commands: Commands,
    editor: Res<Editor>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    hud: Query<Entity, With<crate::game::ViewCamera>>,
    mut panel: Query<(&mut Text, &mut Node), With<Overlay>>,
) {
    let show = editor.opened && editor.editing;
    let text = if show {
        let model = editor.models.get(editor.model).map_or("(no models)", |m| m.as_str());
        let search = editor.search.as_ref().map_or(String::new(), |s| format!("\nSearch: {s}_"));
        let selected = match (editor.selected, logic.as_ref()) {
            (Some(id), Some(logic)) if logic.world.ents.get(id).is_some_and(|e| e.alive) => {
                let e = &logic.world.ents[id];
                let o = e.me.origin;
                let a = e.me.angles;
                let outputs: String = e.conns.iter().map(|c| format!("\n  {} -> {},{},{}", c.output, c.target, c.input, c.param)).collect();
                format!(
                    "\nSelected ({id}) {} [{}] HammerId {}\n  origin {:.0} {:.0} {:.0}  angles {:.0} {:.0} {:.0}{outputs}",
                    e.me.targetname,
                    e.me.classname,
                    e.me.kv("hammerid").unwrap_or("-"),
                    o.x, o.y, o.z, a.x, a.y, a.z
                )
            }
            _ => "\nNothing selected (RMB)".into(),
        };
        format!(
            "EDIT WORLD   {}\nModel [ ]: {model}{search}{selected}\nLMB place  RMB select  arrows/PgUp/PgDn move  Q/E rotate  Del  Ctrl+D  F5 play  F6 save  F7 load",
            editor.status
        )
    } else {
        String::new()
    };
    match panel.iter_mut().next() {
        Some((mut t, mut node)) => {
            if t.0 != text {
                t.0 = text;
            }
            node.display = if show { Display::Flex } else { Display::None };
        }
        None => {
            let Some(hud) = hud.iter().next() else { return };
            commands.spawn((
                Text::new(text),
                TextFont { font_size: FontSize::Px(13.), ..default() },
                TextColor(Color::srgb(1., 0.92, 0.7)),
                BackgroundColor(Color::srgba(0., 0., 0., 0.55)),
                Node { position_type: PositionType::Absolute, left: px(8.), bottom: px(120.), padding: UiRect::all(px(6.)), ..default() },
                GlobalZIndex(145),
                UiTargetCamera(hud),
                Overlay,
            ));
        }
    }
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
        assert!(text.contains("ent_fire prop_dynamic addoutput \"HammerId 1001\";"), "{text}");
        assert!(text.contains("ent_keyvalue 1001 \"TargetName\" \"crate1\""), "{text}");
        assert!(text.contains("\"origin\" \"-4300 2500 -250\""), "{text}");
        assert!(text.contains("ent_keyvalue 1001 \"OnTakeDamage\" \"PD1,Command,give health\";"), "{text}");
        assert!(text.contains("\"rendercolor\" \"0 255 0\""), "{text}");
        assert!(text.contains("ent_create point_clientcommand;"), "{text}");
        assert!(text.contains("ent_fire point_clientcommand addoutput \"TargetName PD1\";"), "{text}");
        assert!(text.contains("ent_fire PD1 addoutput \"classname l4d2r_edit_point_clientcommand\";"), "{text}");
        // Every line splits into well-formed commands (quotes balanced).
        for command in crate::source_commands::split(&text) {
            assert_eq!(command.matches('"').count() % 2, 0, "{command}");
        }
    }
}
