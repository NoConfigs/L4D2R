//! Local gameplay console, with no shell execution or arbitrary file commands.
use crate::{
    collision::HEIGHT,
    game::Body,
    game_menu::{Frontend, Page},
    game_state::{Gun, Match, Rules},
};
use bevy::{
    input::{ButtonState, keyboard::KeyboardInput},
    prelude::*,
};
#[derive(Resource, Default)]
pub struct Console {
    pub open: bool,
    pub noclip: bool,
    input: String,
    history: Vec<String>,
    cursor: usize,
    lines: Vec<String>,
    pending_scripts: Vec<(bool, String)>,
    /// director_force_panic_event, applied by `run_scripts`.
    panic: bool,
    scroll: usize,
    visible_lines: usize,
    pub flashlight: bool,
}
#[derive(Component)]
struct Panel;
#[derive(Component)]
struct Content;
#[derive(Component)]
struct InputLine;
#[derive(Component)]
struct Submit;
#[derive(Component)]
struct Close;
fn layout_console(
    windows: Query<&Window>,
    mut panels: Query<&mut Node, With<Panel>>,
    mut state: ResMut<Console>,
) {
    let Some(window) = windows.iter().next() else {
        return;
    };
    let x = (window.width() * 0.425).min(window.width() - 1068.).max(0.);
    let y = (window.height() / 30.).min(window.height() - 792.).max(0.);
    state.visible_lines = ((window.height().min(792.) - 90.).max(12.) / 12.) as usize;
    for mut panel in &mut panels {
        if panel.left != px(x) {
            panel.left = px(x);
        }
        if panel.top != px(y) {
            panel.top = px(y);
        }
    }
}
#[derive(Component)]
struct Flashlight;
pub fn install(app: &mut App) {
    app.init_resource::<Console>()
        .add_systems(Startup, setup.after(crate::game_menu::setup))
        .add_systems(Update, console.before(crate::game_menu::update))
        .add_systems(Update, run_scripts.after(console))
        .add_systems(Update, paste.before(console))
        .add_systems(Update, layout_console.before(console))
        .add_systems(Update, lab_panel_visibility.after(console))
        .add_systems(
            PostUpdate,
            light.after(bevy::transform::TransformSystems::Propagate),
        );
    if std::env::args().any(|a| a == "--capture-flashlight") {
        app.world_mut().resource_mut::<Console>().flashlight = true;
    }
    if std::env::args().any(|a| a == "--capture-console") {
        let mut c = app.world_mut().resource_mut::<Console>();
        c.open = true;
        c.lines.push(
            "Commands: noclip, flashlight, give, heal, hurt, setpos, getpos, map, restart, clear"
                .into(),
        );
    }
}
fn paste(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<Console>,
    mut clipboard: ResMut<bevy::clipboard::Clipboard>,
) {
    if state.open
        && (keys.pressed(KeyCode::ControlLeft)
            || keys.pressed(KeyCode::ControlRight)
            || keys.just_pressed(KeyCode::ControlLeft)
            || keys.just_pressed(KeyCode::ControlRight))
        && keys.just_pressed(KeyCode::KeyV)
    {
        if let Some(Ok(text)) = clipboard.fetch_text().poll_result() {
            let room = 8192usize.saturating_sub(state.input.len());
            let mut inserted = String::new();
            for c in text.chars().filter(|c| !c.is_control()) {
                if inserted.len() + c.len_utf8() > room {
                    break;
                }
                inserted.push(c);
            }
            state.input.push_str(&inserted);
        }
    }
}
fn run_scripts(
    mut state: ResMut<Console>,
    mut director: Option<ResMut<crate::source_director::Director>>,
) {
    if std::mem::take(&mut state.panic) {
        if let Some(director) = director.as_mut() {
            director.command("ForcePanicEvent", "");
        }
    }
    for (file, code) in std::mem::take(&mut state.pending_scripts) {
        let output = director.as_mut().map_or_else(
            || "VScript VM is not initialized for this map.".into(),
            |vm| vm.console_script(&code, file),
        );
        state.lines.extend(output.lines().map(str::to_owned));
    }
}
pub(crate) fn rebuild(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world.resource_mut::<Console>().open = false;
    world.resource_mut::<Console>().noclip = false;
    world
        .run_system_once(setup)
        .expect("Console / flashlight UI");
}
fn lab_panel_visibility(
    state: Res<Console>,
    frontend: Res<Frontend>,
    mut panels: Query<&mut Node, With<crate::test_lab::Panel>>,
) {
    let display = if !state.open && frontend.playing() {
        Display::Flex
    } else {
        Display::None
    };
    for mut panel in &mut panels {
        if panel.display != display {
            panel.display = display;
        }
    }
}
fn setup(
    mut commands: Commands,
    camera: Single<Entity, With<crate::viewer::GameCamera>>,
    hud_camera: Single<Entity, With<crate::game::ViewCamera>>,
    mut fonts: ResMut<Assets<Font>>,
) {
    let camera = *camera;
    commands
        .entity(camera)
        .insert(bevy::camera::ShadowLodOrigin);
    let overlay = *hud_camera;
    let font = std::fs::read("C:/Windows/Fonts/cour.ttf")
        .ok()
        .map(|bytes| crate::game_ui::shared_font(&mut *fonts, bytes))
        .unwrap_or_default();
    let text_font = TextFont {
        font: bevy::text::FontSource::Handle(font),
        font_size: FontSize::Px(10.),
        ..default()
    };
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(816.),
                top: px(36.),
                width: px(1068.),
                max_width: percent(100),
                height: px(792.),
                max_height: percent(100),
                padding: UiRect::all(px(16.)),
                flex_direction: FlexDirection::Column,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgb_u8(118, 118, 118)),
            GlobalZIndex(150),
            UiTargetCamera(overlay),
            Panel,
        ))
        .id();
    commands.spawn((
        Text::new("Console"),
        text_font.clone(),
        TextColor(Color::WHITE),
        Node {
            height: px(22.),
            ..default()
        },
        ChildOf(root),
    ));
    let close = commands
        .spawn((
            Button,
            Close,
            Node {
                position_type: PositionType::Absolute,
                right: px(8.),
                top: px(4.),
                width: px(16.),
                height: px(20.),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    commands.spawn((
        Text::new("x"),
        text_font.clone(),
        TextColor(Color::srgb_u8(190, 190, 190)),
        ChildOf(close),
    ));
    let output = commands
        .spawn((
            Node {
                flex_grow: 1.,
                width: percent(100),
                overflow: Overflow::clip(),
                border: UiRect::all(px(1.)),
                padding: UiRect::all(px(2.)),
                ..default()
            },
            BorderColor::all(Color::srgb_u8(155, 155, 155)),
            BackgroundColor(Color::srgb_u8(83, 83, 83)),
            ChildOf(root),
        ))
        .id();
    commands.spawn((
        Text::new(""),
        text_font.clone(),
        TextColor(Color::srgb_u8(230, 230, 230)),
        Content,
        ChildOf(output),
    ));
    let row = commands
        .spawn((
            Node {
                width: percent(100),
                height: px(24.),
                margin: UiRect::top(px(8.)),
                column_gap: px(12.),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    let input = commands
        .spawn((
            Node {
                flex_grow: 1.,
                border: UiRect::all(px(1.)),
                overflow: Overflow::clip(),
                padding: UiRect::all(px(2.)),
                ..default()
            },
            BorderColor::all(Color::srgb_u8(155, 155, 155)),
            BackgroundColor(Color::srgb_u8(83, 83, 83)),
            ChildOf(row),
        ))
        .id();
    commands.spawn((
        Text::new(""),
        text_font.clone(),
        TextColor(Color::WHITE),
        Content,
        InputLine,
        ChildOf(input),
    ));
    let submit = commands
        .spawn((
            Button,
            Node {
                width: px(64.),
                border: UiRect::all(px(1.)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BorderColor::all(Color::srgb_u8(170, 170, 170)),
            BackgroundColor(Color::srgb_u8(118, 118, 118)),
            Submit,
            ChildOf(row),
        ))
        .id();
    commands.spawn((
        Text::new("Submit"),
        text_font,
        TextColor(Color::WHITE),
        ChildOf(submit),
    ));
    commands.spawn((
        SpotLight {
            intensity: 0.,
            range: 25.,
            radius: 0.02,
            inner_angle: crate::survivor_flashlight::INNER_ANGLE.to_radians(),
            outer_angle: crate::survivor_flashlight::OUTER_ANGLE.to_radians(),
            shadow_maps_enabled: false,
            shadow_depth_bias: 0.02,
            shadow_normal_bias: 0.05,
            ..default()
        },
        Transform::from_xyz(0., 0.08, -0.12),
        Visibility::Visible,
        bevy::camera::visibility::NoFrustumCulling,
        bevy::camera::visibility::RenderLayers::from_layers(&[0, 1]),
        Flashlight,
        ChildOf(camera),
    ));
}
fn light(
    console: Res<Console>,
    graphics: Res<crate::graphics_options::Graphics>,
    state: Res<Match>,
    frontend: Res<Frontend>,
    mut lights: Query<&mut SpotLight, With<Flashlight>>,
) {
    let value =
        if console.flashlight && !state.infected && !state.dead && frontend.page == Page::Game {
            crate::survivor_flashlight::LUMENS
        } else {
            0.
        };
    for mut light in &mut lights {
        if light.intensity != value {
            light.intensity = value;
        }
        let shadows = value > 0. && graphics.shader == 2;
        if light.shadow_maps_enabled != shadows {
            light.shadow_maps_enabled = shadows;
        }
    }
}
pub(crate) fn execute(
    text: &str,
    state: &mut Console,
    game: &mut Match,
    rules: &Rules,
    body: &mut Body,
    collision: &crate::collision::Collision,
    requests: &mut crate::map_transition::Requests,
    settings: &crate::campaign_settings::SessionSettings,
    root: &std::path::Path,
    frontend: &mut Frontend,
) -> String {
    let words = text.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() {
        return String::new();
    }
    match words[0].to_ascii_lowercase().as_str() {
        "team" => match words.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("infected") => {
                use crate::special_infected::SpecialClass;
                let role = match words.get(2) {
                    None => game.special_class,
                    Some(name) => match SpecialClass::parse(name) {
                        Some(role) => role,
                        None => return format!("Unknown infected role '{name}'. Roles: hunter, smoker, boomer, spitter, jockey, charger, tank"),
                    },
                };
                if !role.playable() { return format!("{} is not playable yet.", role.label()); }
                game.special_class = role;
                game.infected = true;
                format!("{} team survivor restores your survivor.", role.controls())
            }
            Some("survivor") => {game.infected=false; "Survivor team selected".into()},
            _ => "Usage: team infected [hunter|smoker|boomer|spitter|jockey|charger|tank] / team survivor".into(),
        },
        "help"=>"Commands: script <code>, script_execute <file>, team infected/survivor, sb_stop <0|1>, noclip, flashlight, give <weapon>, heal, hurt <amount>, setpos x y z, getpos, map <name>, restart, fov_desired <degrees>, cl_viewmodelfovsurvivor <degrees>, glow_settings, cl_glow_* <number>, clear, hideconsole, quit".into(),
        "noclip"=>{if state.noclip {match collision.supported_spawn_near(body.feet,HEIGHT){Ok(p)=>body.feet=p,Err(_)=>return "Cannot leave noclip inside solid geometry; fly into clear space first.".into()}}state.noclip=!state.noclip;body.velocity=Vec3::ZERO;format!("noclip {}",state.noclip)},
        "flashlight"=>{state.flashlight=!state.flashlight;format!("flashlight {}",state.flashlight)},
        "director_force_panic_event"=>{state.panic=true;"Panic event".into()},
        "sb_stop" => match words.get(1) {
            None => format!("\"sb_stop\" = \"{}\"", crate::game_state::bots_stopped() as u8),
            Some(value) => match value.to_ascii_lowercase().as_str() {
                "1" | "true" => {
                    crate::game_state::set_bots_stopped(true);
                    "sb_stop 1: survivor bots stopped".into()
                }
                "0" | "false" => {
                    crate::game_state::set_bots_stopped(false);
                    "sb_stop 0: survivor bots resumed".into()
                }
                _ => "Usage: sb_stop <0|1>".into(),
            },
        },
        "heal"=>{game.resuscitate();game.health=if game.infected {game.special_class.health()} else {100.};"Health restored".into()},
        "hurt"=>match words.get(1).and_then(|v|v.parse::<f32>().ok()).filter(|v|v.is_finite()&&*v>0.){Some(v)=>{game.damage(v);format!("Damage {v}")},None=>"Usage: hurt <positive amount>".into()},
        // setang pitch yaw (Source degrees): yaw 0 faces Source +x.
        "setang"=>{let values=words.iter().skip(1).map(|s|s.parse::<f32>()).collect::<Result<Vec<_>,_>>();match values {Ok(v) if v.len()>=2&&v.iter().all(|f|f.is_finite())=>{body.pitch=(-v[0]).to_radians().clamp(-1.55,1.55);body.yaw=(v[1]-90.).to_radians();"View angles set in Source degrees".into()},_=>"Usage: setang pitch yaw (Source degrees)".into()}},
        "getpos"=>{let p=body.feet;format!("setpos {:.1} {:.1} {:.1}",p.x/0.0254,-p.z/0.0254,p.y/0.0254)},
        "setpos"=>{let values=words.iter().skip(1).map(|s|s.parse::<f32>()).collect::<Result<Vec<_>,_>>();match values {Ok(v) if v.len()==3&&v.iter().all(|f|f.is_finite())=>{body.feet=Vec3::from_array(crate::binary::source_position([v[0],v[1],v[2]]));body.velocity=Vec3::ZERO;"Position set in Source units".into()},_=>"Usage: setpos x y z (Source units)".into()}},
        "give"=>{let Some(name)=words.get(1)else{return "Usage: give rifle / molotov / first_aid_kit".into()};let name=if name.starts_with("weapon_"){name.to_string()}else{format!("weapon_{name}")};
            if name=="weapon_first_aid_kit" {game.medkit=true;game.defibrillator=false;}else if name=="weapon_pain_pills"{game.pills=true;game.adrenaline=false;}else if name=="weapon_adrenaline"{game.adrenaline=true;game.pills=false;}else if name=="weapon_defibrillator"{game.defibrillator=true;game.medkit=false;}
            else if let Some(w)=rules.weapons.get(&name){if w.throwable(){game.throwable=Some(name.clone());game.slot=3;}else{let gun=Gun{
 laser: false, ammo_upgrade: None, upgraded_rounds: 0,class:name.clone(),ammo:w.clip,reserve:w.reserve_capacity()};if w.melee()||w.infinite(){if !game.pickup_secondary(gun){return "Already carrying dual pistols".into();}}else{game.primary=Some(gun);game.slot=1;}}game.reload=0.;game.reload_phase=0;}
            else{return format!("Unknown or unsupported item: {name}");}format!("Given {name}")},
        "restart"=>if crate::net_flow::request_restart(){frontend.restart=true;state.open=false;"Restart requested".into()}else{"Only the lobby leader can restart the chapter.".into()},
        "map"=>{let Some(name)=words.get(1)else{return "Usage: map c1m1_hotel / rewrite_lab".into()};if !name.chars().all(|c|c.is_ascii_alphanumeric()||c=='_'){return "Map names must contain only letters, numbers and underscores.".into();}
            let map=if *name=="rewrite_lab"{crate::test_lab::path()}else{root.join("left4dead2/maps").join(format!("{name}.bsp"))};if !map.is_file(){return "Map file not found".into();}
            let mut settings=settings.clone();settings.map=map;requests.0=Some((root.to_path_buf(),settings));state.open=false;"Loading map in existing window".into()},
        "clear"=>{state.lines.clear();state.scroll=0;String::new()},"showconsole"=>{state.open=true;String::new()},"toggleconsole"=>{state.open=!state.open;String::new()},"echo"=>text.split_once(char::is_whitespace).map_or("",|(_,s)|s).into(),"hideconsole"=>{state.open=false;String::new()},
        _=>format!("Unknown command: {} (type help)",words[0]),
    }
}
fn console(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut events: MessageReader<KeyboardInput>,
    mut state: ResMut<Console>,
    mut game: ResMut<Match>,
    rules: Res<Rules>,
    mut body: Single<&mut Body>,
    collision: Res<crate::collision::Collision>,
    mut requests: ResMut<crate::map_transition::Requests>,
    settings: Res<crate::campaign_settings::SessionSettings>,
    root: Res<crate::game_ui::HudSource>,
    mut frontend: ResMut<Frontend>,
    mut glow: ResMut<crate::glow_settings::GlowSettings>,
    mut display: ResMut<crate::performance::DisplaySettings>,
    mut panels: Query<&mut Node, With<Panel>>,
    mut labels: Query<(&mut Text, Option<&InputLine>), With<Content>>,
    mut actions: (
        MessageWriter<bevy::app::AppExit>,
        Query<
            (&Interaction, Option<&Close>),
            (Or<(With<Submit>, With<Close>)>, Changed<Interaction>),
        >,
        ResMut<crate::special_infected::witch::Requests>,
        Local<u32>,
    ),
) {
    let game_frames: &mut u32 = &mut actions.3;
    // `--capture-command "<frame> <command>"` (repeatable) runs a console
    // command once that many in-game frames after loading.
    let mut scripted = None;
    if std::env::var_os("L4D2R_COMMON_LOG").is_some() {
        static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
        let now = format!("page={} paused={} dead={} health={:.0} incap={}", frontend.page as u8, game.paused, game.dead, game.health, game.incap);
        let mut last = LAST.lock().unwrap();
        if *last != now {
            println!("L4D2RSTATE {now}");
            *last = now;
        }
    }
    if frontend.page == Page::Game {
        *game_frames += 1;
        let args: Vec<String> = std::env::args().collect();
        scripted = args
            .windows(2)
            .filter(|w| w[0] == "--capture-command")
            .find_map(|w| {
                let (frame, command) = w[1].split_once(' ')?;
                (frame.parse::<u32>().ok()? == *game_frames).then(|| command.to_string())
            });
    }
    if !state.open && frontend.page == Page::Game && keys.just_pressed(KeyCode::KeyF) {
        state.flashlight = !state.flashlight;
    }
    let mut toggled = false;
    if keys.just_pressed(KeyCode::Backquote) && frontend.page != Page::Loading {
        state.open = !state.open;
        game.paused = state.open;
        toggled = true;
    }
    if state.open && keys.just_pressed(KeyCode::Escape) {
        state.open = false;
        game.paused = false;
        toggled = true;
        keys.clear_just_pressed(KeyCode::Escape);
    }
    if state.open
        && actions
            .1
            .iter()
            .any(|(i, c)| *i == Interaction::Pressed && c.is_some())
    {
        state.open = false;
        game.paused = false;
        toggled = true;
    }
    let clicked = state.open
        && actions
            .1
            .iter()
            .any(|(i, c)| *i == Interaction::Pressed && c.is_none());
    let inputs = events
        .read()
        .filter(|e| e.state == ButtonState::Pressed)
        .map(|e| (e.key_code, e.text.as_ref().map(|s| s.to_string())))
        .chain(clicked.then_some((KeyCode::Enter, None)))
        .chain(scripted.is_some().then_some((KeyCode::Enter, None)));
    if let Some(command) = &scripted {
        println!("Capture command: {command}");
        state.input = command.clone();
    }
    for (key_code, text) in inputs {
        if (!state.open && scripted.is_none()) || toggled {
            continue;
        }
        match key_code {
            KeyCode::Enter => {
                let command = std::mem::take(&mut state.input);
                if command.trim() == "quit" {
                    actions.0.write(bevy::app::AppExit::Success);
                } else {
                    state.lines.push(format!("> {command}"));
                    let out = if let Some(out) = catalog_command(&command) {
                        out
                    } else if let Some((file, code)) = script_request(&command) {
                        if code.trim().is_empty() {
                            "Usage: script <Squirrel code> / script_execute <filename>".into()
                        } else {
                            state.pending_scripts.push((file, code.into()));
                            String::new()
                        }
                    } else if let Some(out) = display.command(&command) {
                        out
                    } else if let Some(out) = glow.command(&command) {
                        out
                    } else if let Some(out) =
                        crate::special_infected::witch::command(&command, &mut actions.2)
                    {
                        out
                    } else if let Some(out) = crate::special_infected::ai::command(&command) {
                        out
                    } else if let Some(out) = crate::multiplayer::command(&command) {
                        out
                    } else if let Some(out) = crate::survival::command(&command) {
                        out
                    } else {
                        execute(
                            &command,
                            &mut state,
                            &mut game,
                            &rules,
                            &mut body,
                            &collision,
                            &mut requests,
                            &settings,
                            &root.0,
                            &mut frontend,
                        )
                    };
                    if !state.open && frontend.page == Page::Game {
                        game.paused = false;
                    }
                    if !out.is_empty() {
                        state.lines.push(out);
                    }
                }
                state.scroll = 0;
                state.history.push(command);
                state.cursor = state.history.len();
            }
            KeyCode::PageUp => {
                state.scroll = (state.scroll + 20).min(state.lines.len().saturating_sub(1));
            }
            KeyCode::PageDown => {
                state.scroll = state.scroll.saturating_sub(20);
            }
            KeyCode::Tab => {
                let prefix = state.input.trim().to_ascii_lowercase();
                let matches = command_names()
                    .into_iter()
                    .filter(|s| s.starts_with(&prefix))
                    .collect::<Vec<_>>();
                if matches.len() == 1 {
                    state.input = format!("{} ", matches[0]);
                } else if !matches.is_empty() {
                    state.lines.push(matches.join("  "));
                }
            }
            KeyCode::Backspace => {
                state.input.pop();
            }
            KeyCode::ArrowUp => {
                state.cursor = state.cursor.saturating_sub(1);
                state.input = state.history.get(state.cursor).cloned().unwrap_or_default();
            }
            KeyCode::ArrowDown => {
                state.cursor = (state.cursor + 1).min(state.history.len());
                state.input = state.history.get(state.cursor).cloned().unwrap_or_default();
            }
            _ => {
                if let Some(text) = &text {
                    if keys.pressed(KeyCode::ControlLeft)
                        || keys.pressed(KeyCode::ControlRight)
                        || keys.just_pressed(KeyCode::ControlLeft)
                        || keys.just_pressed(KeyCode::ControlRight)
                    {
                        continue;
                    }
                    if state.input.len() < 8192 {
                        state
                            .input
                            .extend(text.chars().filter(|c| !c.is_control() && *c != '`'));
                    }
                }
            }
        }
    }
    if state.lines.len() > 2048 {
        let excess = state.lines.len() - 2048;
        state.lines.drain(..excess);
    }
    if state.open {
        game.paused = true;
    } else if toggled {
        game.paused = false;
    }
    for mut panel in &mut panels {
        panel.display = if state.open {
            Display::Flex
        } else {
            Display::None
        };
    }
    let output = state
        .lines
        .iter()
        .flat_map(|s| s.lines())
        .collect::<Vec<_>>();
    let end = output.len().saturating_sub(state.scroll);
    let text = output[end.saturating_sub(state.visible_lines.max(1))..end].join("\n");
    for (mut label, input) in &mut labels {
        let value = if input.is_some() {
            format!("{}|", state.input)
        } else {
            text.clone()
        };
        if label.0 != value {
            label.0 = value;
        }
    }
}

fn script_request(command: &str) -> Option<(bool, &str)> {
    let command = command.trim_start();
    let end = command.find(char::is_whitespace).unwrap_or(command.len());
    match command[..end].to_ascii_lowercase().as_str() {
        "script" => Some((false, command[end..].trim_start())),
        "script_execute" => Some((true, command[end..].trim_start())),
        _ => None,
    }
}
fn command_names() -> Vec<String> {
    let mut names = "team help noclip flashlight give heal hurt setpos getpos map restart clear hideconsole showconsole toggleconsole echo quit script script_execute director_force_panic_event find cvarlist fov fov_desired viewmodel_fov cl_viewmodelfovsurvivor glow_settings z_spawn witch_force_wander sb_stop"
        .split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    names.extend(
        crate::glow_settings::GlowSettings::default()
            .values
            .into_keys(),
    );
    names.sort();
    names.dedup();
    names
}
fn catalog_command(text: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    let name = words.next()?.to_ascii_lowercase();
    let filter = words.next().unwrap_or("").to_ascii_lowercase();
    match name.as_str() {
        "find" | "cvarlist" => Some(
            command_names()
                .into_iter()
                .filter(|n| n.contains(&filter))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        "help" if !filter.is_empty() => Some(match filter.as_str() {
            "script" => {
                "script <code> : Run the text as a script in the persistent root VScript scope."
                    .into()
            }
            "script_execute" => "script_execute <file> : Run a mounted vscript file.".into(),
            _ if command_names().contains(&filter) => {
                format!("{filter}: available in this rewrite.")
            }
            _ => format!("Unknown command \"{filter}\""),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_console_preserves_quotes_spaces_and_statement_separators() {
        assert_eq!(
            script_request(" script printl(\"hello world\"); x <- 5;"),
            Some((false, "printl(\"hello world\"); x <- 5;"))
        );
        assert_eq!(
            script_request("SCRIPT_EXECUTE example.nut"),
            Some((true, "example.nut"))
        );
        assert_eq!(script_request("script"), Some((false, "")));
        assert_eq!(script_request("scripted_mode"), None);
    }
}
