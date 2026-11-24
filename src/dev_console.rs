//! Source developer console behaviour: the `developer` level, the top-left
//! notify area, `-dev` / `-console` / `+command` launch arguments, and the
//! `ent_text` / `ent_fire` entity commands.
//!
//! - `developer 1`: console text also prints at the top left for
//!   `con_notifytime` (8) seconds, as retail's notify area does.
//! - `developer 2`: also entity I/O traces, in retail's formats
//!   (CBaseEntityOutput::FireOutput / CEventQueue::ServiceEvents), from
//!   `l4d2r_game_core::entities::dev_msg`.
//! - `ent_text [name|classname]`: CBaseEntity::DrawDebugTextOverlays' text
//!   above each matching entity (no argument: the one under the crosshair).
//! - `ent_fire <target> <input> [parameter] [delay]`.
use bevy::prelude::*;
use l4d2r_game_core::entities::{DEVELOPER, DEV_LOG};
use std::sync::atomic::Ordering;

const NOTIFY_TIME: f32 = 8.;
const NOTIFY_LINES: usize = 8;

enum Request {
    Fire { target: String, input: String, param: String, delay: f32 },
    Text(Option<String>),
    /// ent_create / prop_*_create: classname, model, keyvalues.
    Create { class: String, model: Option<String>, keys: Vec<(String, String)> },
    KeyValue { target: String, pairs: Vec<(String, String)> },
    RemoveAll(String),
}
static REQUESTS: std::sync::Mutex<Vec<Request>> = std::sync::Mutex::new(Vec::new());
/// Entities created by console commands (presentation picks them up).
pub static SPAWNED: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
static STARTUP: std::sync::Mutex<Option<std::collections::VecDeque<String>>> = std::sync::Mutex::new(None);

/// Source launch arguments: `-dev` (developer 1), `+cmd args...` queued as
/// console commands in order. `-console` is applied by game_console.
fn startup_commands() -> std::collections::VecDeque<String> {
    let mut out = std::collections::VecDeque::new();
    let mut current: Option<String> = None;
    for arg in std::env::args().skip(1) {
        if let Some(cmd) = arg.strip_prefix('+').filter(|c| !c.is_empty()) {
            out.extend(current.take());
            current = Some(cmd.to_string());
        } else if arg.starts_with('-') && arg.len() > 1 && arg.parse::<f32>().is_err() {
            out.extend(current.take());
        } else if let Some(c) = current.as_mut() {
            c.push(' ');
            c.push_str(&arg);
        }
    }
    out.extend(current);
    if std::env::args().any(|a| a.eq_ignore_ascii_case("-dev")) {
        out.push_front("developer 1".into());
    }
    // Source runs autoexec.cfg at startup (retail testscript setups put
    // `alias d+ test_wait .05` there).
    let game = std::fs::read_to_string("runtime/game-dir.txt").ok().map(|s| s.trim().to_string());
    if game.is_some_and(|g| std::path::Path::new(&g).join("left4dead2/cfg/autoexec.cfg").is_file()) {
        out.push_front("exec autoexec".into());
    }
    out
}

/// The next `+command` from the launch arguments, one per frame.
pub fn next_startup_command() -> Option<String> {
    let mut queue = STARTUP.lock().ok()?;
    queue.get_or_insert_with(startup_commands).pop_front()
}

pub fn developer() -> u32 {
    DEVELOPER.load(Ordering::Relaxed)
}

/// Console commands handled here; None for anything else.
pub fn command(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let name = words.first()?.to_ascii_lowercase();
    match name.as_str() {
        "developer" => Some(match words.get(1) {
            None => format!("\"developer\" = \"{}\" ( def. \"0\" )\n - Set developer message level", developer()),
            Some(value) => match value.parse::<f32>() {
                Ok(v) => {
                    DEVELOPER.store(v.max(0.) as u32, Ordering::Relaxed);
                    String::new()
                }
                Err(_) => format!("\"developer\" = \"{}\" ( def. \"0\" )", developer()),
            },
        }),
        "ent_fire" => {
            // Quoted values stay one argument: addoutput "HammerId 9768024".
            let (_, rest) = crate::source_commands::name_and_rest(text);
            let a = crate::source_commands::args(rest);
            Some(match a.as_slice() {
                [target, input, more @ ..] => {
                    let param = more.first().cloned().unwrap_or_default();
                    let delay = more.get(1).and_then(|d| d.parse().ok()).unwrap_or(0.);
                    REQUESTS.lock().unwrap().push(Request::Fire { target: target.clone(), input: input.clone(), param, delay });
                    String::new()
                }
                _ => "Usage:\n   ent_fire <target> [action] [value] [delay]".into(),
            })
        }
        "ent_text" => {
            REQUESTS.lock().unwrap().push(Request::Text(words.get(1).map(|s| s.to_string())));
            Some(String::new())
        }
        "ent_create" | "prop_dynamic_create" | "prop_physics_create" => {
            let (_, rest) = crate::source_commands::name_and_rest(text);
            let a = crate::source_commands::args(rest);
            let Some(first) = a.first() else {
                return Some(format!("Usage: {name} <{}>", if name == "ent_create" { "entity name" } else { "model name" }));
            };
            let (class, model, keys) = if name == "ent_create" {
                // Retail ent_create ignores extra keyvalues; so does this,
                // so a script that works here works there.
                (first.to_ascii_lowercase(), None, vec![])
            } else {
                // Source prefixes models/ and appends .mdl.
                let path = first.trim().replace('\\', "/").to_ascii_lowercase();
                let path = path.strip_prefix("models/").unwrap_or(&path).trim_end_matches(".mdl").to_string();
                let model = format!("models/{path}.mdl");
                let class = if name == "prop_dynamic_create" { "prop_dynamic" } else { "prop_physics" };
                (class.to_string(), Some(model.clone()), vec![("model".to_string(), model), ("solid".to_string(), "6".to_string())])
            };
            REQUESTS.lock().unwrap().push(Request::Create { class, model, keys });
            Some(String::new())
        }
        "ent_keyvalue" => {
            let (_, rest) = crate::source_commands::name_and_rest(text);
            let a = crate::source_commands::args(rest);
            if a.len() < 3 {
                return Some("Format: ent_keyvalue <entity id> \"key1\"=\"value1\" \"key2\" \"value2\" ... \"keyN\" \"valueN\"".into());
            }
            let pairs = a[1..].chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].clone(), c[1].clone())).collect();
            REQUESTS.lock().unwrap().push(Request::KeyValue { target: a[0].clone(), pairs });
            Some(String::new())
        }
        "ent_remove_all" => {
            let (_, rest) = crate::source_commands::name_and_rest(text);
            match crate::source_commands::args(rest).first() {
                Some(t) => {
                    REQUESTS.lock().unwrap().push(Request::RemoveAll(t.clone()));
                    Some(String::new())
                }
                None => Some("Usage: ent_remove_all <entity name or class name>".into()),
            }
        }
        "say" => {
            let (_, rest) = crate::source_commands::name_and_rest(text);
            Some(format!("Console: {}", rest.trim().trim_matches('"')))
        }
        _ => None,
    }
}

#[derive(Resource, Default)]
pub struct DevState {
    /// (line, time printed) for the notify area.
    notify: Vec<(String, f32)>,
    /// Console lines already mirrored to the notify area.
    seen: usize,
    /// Entities with ent_text on.
    text: Vec<usize>,
}
#[derive(Component)]
struct Notify;
#[derive(Component)]
struct EntText(usize);

pub fn install(app: &mut App) {
    app.init_resource::<DevState>()
        .add_systems(Update, (requests, notify).after(crate::game_menu::update));
}

fn requests(
    mut state: ResMut<DevState>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    collision: Option<Res<crate::collision::Collision>>,
    source: Option<Res<crate::game_ui::HudSource>>,
) {
    let pending = std::mem::take(&mut *REQUESTS.lock().unwrap());
    let Some(logic) = logic.as_mut() else { return };
    crate::source_commands::GAME_TIME.store(logic.world.time.to_bits(), std::sync::atomic::Ordering::Relaxed);
    for request in pending {
        match request {
            Request::Fire { target, input, param, delay } => {
                logic.world.queue_input(&target, &input, &param, delay, None);
            }
            Request::Create { class, model, keys } => {
                // Retail prop_dynamic refuses breakable models.
                if let (Some(path), true, Some(source)) = (model.as_ref(), class == "prop_dynamic", source.as_ref()) {
                    if let Ok(mut assets) = crate::source_assets::SourceAssets::new(&source.0, vec![]) {
                        if crate::source_breakables::model_health(&mut assets, path) > 0. {
                            l4d2r_game_core::entities::dev_msg(0, format!("prop_dynamic has a health specified in model '{path}'. Use prop_physics or prop_dynamic instead."));
                            continue;
                        }
                    }
                }
                // Where the player is looking (Source traces the view).
                let Some(c) = camera.iter().next() else { continue };
                let (eye, forward) = (c.translation(), c.forward().as_vec3());
                let s = |v: Vec3| [v.x / crate::collision::UNIT, -v.z / crate::collision::UNIT, v.y / crate::collision::UNIT];
                let end = eye + forward * 200.;
                let (at, normal) = collision.as_ref()
                    .and_then(|col| col.source_ray_normal(s(eye), s(end), 0x1 | 0x2 | 0x8))
                    .map_or((s(end), [0., 0., 1.]), |(_, p, n)| (p, n));
                let id = logic.world.create_entity(&class, &keys, l4d2r_math::Vec3::from_array(at), l4d2r_math::Vec3::from_array(surface_angles(normal)), model);
                SPAWNED.lock().unwrap().push(id);
            }
            Request::KeyValue { target, pairs } => {
                // A number is a HammerId (retail testscripts), else a name
                // or classname.
                let mut ids = if target.chars().all(|c| c.is_ascii_digit()) { logic.world.find_hammer_id(&target) } else { vec![] };
                if ids.is_empty() {
                    ids = logic.world.find(&target);
                }
                if ids.is_empty() {
                    ids = logic.world.ents.iter().enumerate().filter(|(_, e)| e.alive && l4d2r_game_core::entities::EntityWorld::target_class(e).eq_ignore_ascii_case(&target)).map(|(i, _)| i).collect();
                }
                for id in ids {
                    for (k, v) in &pairs {
                        logic.world.set_keyvalue(id, k, v);
                    }
                }
            }
            Request::RemoveAll(target) => {
                let ids: Vec<usize> = logic.world.ents.iter().enumerate()
                    .filter(|(_, e)| e.alive && (l4d2r_game_core::entities::EntityWorld::target_class(e).eq_ignore_ascii_case(&target) || e.me.targetname.eq_ignore_ascii_case(&target)))
                    .map(|(i, _)| i).collect();
                for id in ids {
                    logic.world.input(id, "Kill", "", None, None);
                }
            }
            Request::Text(name) => {
                let ids: Vec<usize> = match name.as_deref() {
                    Some(n) if !n.eq_ignore_ascii_case("!picker") => {
                        let mut ids = logic.world.find(n);
                        if ids.is_empty() {
                            ids = logic.world.ents.iter().enumerate()
                                .filter(|(_, e)| e.alive && e.me.classname.eq_ignore_ascii_case(n))
                                .map(|(i, _)| i).collect();
                        }
                        ids
                    }
                    // The entity nearest the crosshair ray (Source traces
                    // for the entity under the crosshair).
                    _ => camera.iter().next().and_then(|c| {
                        let (eye, forward) = (c.translation(), c.forward().as_vec3());
                        logic.world.ents.iter().enumerate()
                            .filter(|(_, e)| e.alive && !e.me.classname.starts_with("info_") && !e.me.targetname.starts_with("__rewrite_"))
                            .filter_map(|(i, e)| {
                                let o = e.me.origin;
                                let p = Vec3::new(o.x, o.z, -o.y) * crate::collision::UNIT;
                                let to = p - eye;
                                let along = to.dot(forward);
                                (along > 0. && along < 40.).then(|| (i, (to - forward * along).length() / along))
                            })
                            .filter(|(_, off)| *off < 0.15)
                            .min_by(|a, b| a.1.total_cmp(&b.1))
                            .map(|(i, _)| vec![i])
                    }).unwrap_or_default(),
                };
                for id in ids {
                    // Toggle, as retail's ent_text does.
                    if let Some(i) = state.text.iter().position(|&t| t == id) {
                        state.text.remove(i);
                    } else {
                        state.text.push(id);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn notify(
    mut commands: Commands,
    mut state: ResMut<DevState>,
    mut console: ResMut<crate::game_console::Console>,
    time: Res<Time>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    cameras: Query<(&Camera, &GlobalTransform), With<crate::viewer::GameCamera>>,
    hud: Query<Entity, With<crate::game::ViewCamera>>,
    mut panel: Query<(Entity, &mut Text, &mut Node), (With<Notify>, Without<EntText>)>,
    mut labels: Query<(Entity, &EntText, &mut Text, &mut Node), Without<Notify>>,
) {
    let now = time.elapsed_secs();
    // DevMsg lines from map I/O join the console log.
    let dev: Vec<String> = std::mem::take(&mut *DEV_LOG.lock().unwrap());
    if std::env::args().any(|a| a == "--capture-game") {
        for line in &dev {
            println!("Capture dev: {line}");
        }
    }
    console.push_lines(dev);
    let lines = console.lines();
    if state.seen > lines.len() {
        state.seen = lines.len();
    }
    let fresh: Vec<String> = lines[state.seen..].to_vec();
    state.seen = lines.len();
    if developer() >= 1 {
        for line in fresh {
            for part in line.lines().filter(|l| !l.trim().is_empty()) {
                state.notify.push((part.to_string(), now));
            }
        }
    }
    state.notify.retain(|(_, at)| now - at < NOTIFY_TIME);
    let excess = state.notify.len().saturating_sub(NOTIFY_LINES);
    state.notify.drain(..excess);
    let Some(hud) = hud.iter().next() else { return };
    let text = state.notify.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>().join("\n");
    match panel.iter_mut().next() {
        Some((_, mut t, _)) => {
            if t.0 != text {
                t.0 = text;
            }
        }
        None => {
            commands.spawn((
                Text::new(text),
                TextFont { font_size: FontSize::Px(12.), ..default() },
                TextColor(Color::srgb(0.9, 0.9, 0.9)),
                Node { position_type: PositionType::Absolute, left: px(4.), top: px(2.), ..default() },
                GlobalZIndex(140),
                UiTargetCamera(hud),
                Notify,
            ));
        }
    }
    // ent_text overlays.
    let Some(logic) = logic else { return };
    state.text.retain(|&id| logic.world.ents.get(id).is_some_and(|e| e.alive));
    let Some((camera, view)) = cameras.iter().next() else { return };
    for (entity, label, mut text, mut node) in &mut labels {
        let Some(e) = state.text.contains(&label.0).then(|| &logic.world.ents[label.0]) else {
            commands.entity(entity).despawn();
            continue;
        };
        let o = e.me.origin;
        let world = Vec3::new(o.x, o.z, -o.y) * crate::collision::UNIT;
        match camera.world_to_viewport(view, world) {
            Ok(at) => {
                node.display = Display::Flex;
                node.left = px(at.x);
                node.top = px(at.y);
                let body = ent_text(label.0, e);
                if text.0 != body {
                    text.0 = body;
                }
            }
            Err(_) => node.display = Display::None,
        }
    }
    let shown: Vec<usize> = labels.iter().map(|(_, l, _, _)| l.0).collect();
    for &id in state.text.iter().filter(|id| !shown.contains(id)) {
        commands.spawn((
            Text::new(ent_text(id, &logic.world.ents[id])),
            TextFont { font_size: FontSize::Px(12.), ..default() },
            TextColor(Color::WHITE),
            Node { position_type: PositionType::Absolute, ..default() },
            GlobalZIndex(139),
            UiTargetCamera(hud),
            EntText(id),
        ));
    }
}

/// CBaseEntity::DrawDebugTextOverlays' OVERLAY_TEXT_BIT lines.
fn ent_text(index: usize, e: &l4d2r_game_core::entities::Ent) -> String {
    let name = if e.me.targetname.is_empty() { &e.me.classname } else { &e.me.targetname };
    let o = e.me.origin;
    let mut s = format!("({index}) Name: {name} ({})\nPosition: {:.1}, {:.1}, {:.1}", e.me.classname, o.x, o.y, o.z);
    if let Some(model) = e.me.kv("model").filter(|m| !m.is_empty()) {
        s.push_str(&format!("\nModel:{model}"));
    }
    if let Some(filter) = e.me.kv("damagefilter").filter(|m| !m.is_empty()) {
        s.push_str(&format!("\nDAMAGE FILTER:{filter}"));
    }
    s
}

/// Retail prop_*_create stands the model on the surface it hit: model up =
/// hit normal, yaw 0 whatever the view yaw (measured on c1m1: floor (0 0 0),
/// a wall facing -y (0 0 90), +y (0 0 -90), -x (-90 0 0)).
fn surface_angles(n: [f32; 3]) -> [f32; 3] {
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if !(len > 1e-6) {
        return [0.; 3];
    }
    let n = [n[0] / len, n[1] / len, n[2] / len];
    [n[0].atan2(n[2]).to_degrees(), 0., (-n[1]).clamp(-1., 1.).asin().to_degrees()]
}

#[cfg(test)]
mod surface_angle_tests {
    #[test]
    fn prop_create_angles_match_retail_surface_alignment() {
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01);
        assert!(close(super::surface_angles([0., 0., 1.]), [0., 0., 0.]));
        assert!(close(super::surface_angles([0., -1., 0.]), [0., 0., 90.]));
        assert!(close(super::surface_angles([0., 1., 0.]), [0., 0., -90.]));
        assert!(close(super::surface_angles([-1., 0., 0.]), [-90., 0., 0.]));
    }
}
