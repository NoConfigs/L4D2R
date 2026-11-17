//! One application/window, asynchronous CPU map preparation, then atomic scene replacement.
use crate::{
    campaign_settings::SessionSettings,
    game_menu::{Frontend, Page},
    game_state::Match,
};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
#[derive(Resource, Default)]
pub struct Requests(
    pub Option<(PathBuf, SessionSettings)>,
    pub Option<crate::campaign_flow::Carry>,
);
#[derive(Resource, Default)]
struct Probe {
    baseline: Option<Entity>,
    loads: usize,
    ticks: usize,
    report: String,
    captured_loading: bool,
}
/// Map preparation runs on one loader thread kept for the session. A fresh
/// thread per map exited with its allocator pages still holding parts of the
/// prepared map; once the map was freed those abandoned pages were not
/// handed back (PC phone profile: ~400 MiB held but unused after Hotel ->
/// lab, part of the iPhone Hotel -> menu -> motel kill). The loader thread
/// owns its pages, and collects them after each job and on request.
pub(crate) struct LoaderTask {
    done: Arc<std::sync::atomic::AtomicBool>,
    panicked: Arc<std::sync::atomic::AtomicBool>,
}
type LoaderJob = Box<dyn FnOnce() + Send>;
fn loader() -> &'static Mutex<std::sync::mpsc::Sender<LoaderJob>> {
    static LOADER: std::sync::OnceLock<Mutex<std::sync::mpsc::Sender<LoaderJob>>> = std::sync::OnceLock::new();
    LOADER.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel::<LoaderJob>();
        std::thread::Builder::new()
            .name("map loader".into())
            .spawn(move || {
                for job in receive {
                    job();
                }
            })
            .expect("map loader thread");
        Mutex::new(send)
    })
}
impl LoaderTask {
    fn spawn(job: impl FnOnce() + Send + 'static) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        let done = Arc::new(AtomicBool::new(false));
        let panicked = Arc::new(AtomicBool::new(false));
        let (d, p) = (done.clone(), panicked.clone());
        let _ = loader().lock().unwrap().send(Box::new(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                p.store(true, Ordering::Release);
            }
            collect_here();
            d.store(true, Ordering::Release);
        }));
        Self { done, panicked }
    }
    fn is_finished(&self) -> bool {
        self.done.load(std::sync::atomic::Ordering::Acquire)
    }
    fn join(self) -> Result<(), ()> {
        while !self.is_finished() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if self.panicked.load(std::sync::atomic::Ordering::Acquire) { Err(()) } else { Ok(()) }
    }
}
fn collect_here() {
    unsafe extern "C" {
        fn mi_collect(force: bool);
    }
    unsafe { mi_collect(true) };
}
/// Has the loader thread hand back pages freed by others (the released
/// map) before the next map is prepared; waits for it.
pub(crate) fn collect_loader() {
    let (send, receive) = std::sync::mpsc::channel::<()>();
    if loader().lock().unwrap().send(Box::new(move || { collect_here(); let _ = send.send(()); })).is_ok() {
        let _ = receive.recv_timeout(std::time::Duration::from_secs(2));
    }
}
#[derive(Resource, Default)]
struct Transition {
    task: Option<LoaderTask>,
    result: Option<Arc<Mutex<Option<std::io::Result<crate::game::Prepared>>>>>,
    root: PathBuf,
    cancelled: bool,
    previous: Option<Page>,
    /// Mobile: frames left to wait after releasing the outgoing map, and the
    /// page shown before the change. The render world frees GPU buffers a
    /// frame or two after the assets go; preparing the next map at once
    /// stacked both maps in memory (phone log: 2993 MiB at preparation
    /// begin, killed past 3072 MiB).
    release_wait: Option<(u8, Page)>,
}
const RELEASE_FRAMES: u8 = 8;
const AUDIT_RELEASE_FRAMES: u8 = 120;
/// Leaving a game for the menu unloads the map like retail: the tiny lab map
/// is loaded behind the menu (as on a plain start) and the menu page stays up.
#[derive(Resource, Default)]
struct MenuReturn {
    /// Menu page to show once the lab map is in place.
    pending: Option<Page>,
    /// Whether a game was being played last frame.
    was_started: bool,
}
#[derive(Component)]
struct Overlay;
#[derive(Component)]
struct Status;
#[derive(Component)]
struct LoadingSpinner;
#[derive(Component)]
struct Fill;
pub fn install(app: &mut App) {
    app.init_resource::<Requests>()
        .init_resource::<Transition>()
        .init_resource::<Probe>()
        .init_resource::<MenuReturn>()
        .add_systems(Update, (unload_on_menu, poll).chain().before(crate::game_menu::update));
}
pub(crate) fn overlay(world: &mut World, root: &std::path::Path, map: &std::path::Path) {
    let mut assets =
        crate::source_assets::SourceAssets::new(root, vec![]).expect("Installed loading artwork");
    let name = crate::game_loading::poster(&map.file_stem().unwrap_or_default().to_string_lossy());
    let wide = format!("materials/vgui/loadingscreen_{name}_widescreen.vtf");
    let path = if assets.read(&wide).is_ok() {
        wide
    } else {
        format!("materials/vgui/loadingscreen_{name}.vtf")
    };
    let image = crate::game_menu::image(
        &mut assets,
        &mut world.resource_mut::<Assets<Image>>(),
        &path,
    );
    let target = world
        .query_filtered::<&bevy::camera::RenderTarget, With<crate::viewer::GameCamera>>()
        .iter(world)
        .next()
        .cloned();
    let camera = world
        .spawn((
            Camera2d,
            Msaa::Off,
            Camera {
                order: 20,
                clear_color: bevy::camera::ClearColorConfig::None,
                ..default()
            },
            RenderLayers::layer(31),
            Overlay,
        ))
        .id();
    if let Some(target) = target {
        world.entity_mut(camera).insert(target);
    }
    let font = assets
        .read("resource/stubble-bold.vfont")
        .and_then(|b| crate::vfont::decode(&b))
        .map(|b| {
            crate::game_ui::shared_font(&mut world.resource_mut::<Assets<Font>>(), b)
        })
        .unwrap_or_default();
    let (bg, fill, spinner) = {
        let mut images = world.resource_mut::<Assets<Image>>();
        (
            crate::game_menu::image(&mut assets, &mut images, "materials/vgui/loadingbar_bg.vtf"),
            crate::game_menu::image(&mut assets, &mut images, "materials/vgui/loadingbar.vtf"),
            crate::game_menu::image(&mut assets, &mut images, "materials/engine/l4d_spinner.vtf"),
        )
    };
    let map = map.file_stem().unwrap_or_default().to_string_lossy();
    let root = world
        .spawn((
            ImageNode::new(image).with_mode(bevy::ui::widget::NodeImageMode::Stretch),
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
            UiTargetCamera(camera),
            Overlay,
        ))
        .id();
    let strip = world
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: px(0),
                width: percent(100),
                height: vh(60. / 4.8),
                ..default()
            },
            BackgroundColor(Color::srgba(0., 0., 0., 0.7)),
            ChildOf(root),
        ))
        .id();
    let text_font = |size: f32| TextFont {
        font: bevy::text::FontSource::Handle(font.clone()),
        font_size: FontSize::Vh(size / 4.8),
        ..default()
    };
    world.spawn((
        Text::new("Joining a Campaign game."),
        text_font(14.),
        TextColor(Color::srgb(0.8, 0.8, 0.8)),
        crate::game_loading::rect(22., 6., 500., 20.),
        ChildOf(strip),
    ));
    let original = map.starts_with("c7")
        || map.starts_with("c14")
        || ["c8", "c9", "c10", "c11", "c12"]
            .iter()
            .any(|c| map.starts_with(c));
    let roster = if original {
        "Bill, Zoey, Francis, Louis"
    } else {
        "Coach, Ellis, Nick, Rochelle"
    };
    world.spawn((
        Text::new(format!("Starring: {roster}")),
        text_font(11.),
        TextColor(Color::srgb(0.8, 0.8, 0.8)),
        crate::game_loading::rect(22., 25., 500., 18.),
        ChildOf(strip),
    ));
    world.spawn((
        Text::new("Loading"),
        text_font(13.),
        TextColor(Color::srgb(0.8, 0.8, 0.8)),
        Node {
            position_type: PositionType::Absolute,
            right: vh(22. / 4.8),
            bottom: vh(35. / 4.8),
            ..default()
        },
        Status,
        ChildOf(root),
    ));
    let bar = world
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: vh(23. / 4.8),
                bottom: vh(12. / 4.8),
                width: vh(135. / 4.8),
                height: vh(33. / 4.8),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    world.spawn((
        ImageNode::new(bg),
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            ..default()
        },
        ChildOf(bar),
    ));
    let clip = world
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: percent(55),
                height: percent(100),
                overflow: Overflow::clip(),
                ..default()
            },
            Fill,
            ChildOf(bar),
        ))
        .id();
    world.spawn((
        ImageNode::new(fill).with_mode(bevy::ui::widget::NodeImageMode::Stretch),
        Node {
            flex_shrink: 0.,
            width: vh(135. / 4.8),
            height: vh(33. / 4.8),
            ..default()
        },
        ChildOf(clip),
    ));
    world.spawn((
        ImageNode::new(spinner),
        Node {
            position_type: PositionType::Absolute,
            right: vh(60. / 4.8),
            top: vh(45. / 4.8),
            width: vh(45. / 4.8),
            height: vh(45. / 4.8),
            ..default()
        },
        LoadingSpinner,
        UiTransform::default(),
        ChildOf(root),
    ));
}
pub(crate) fn remove_overlay(world: &mut World) {
    let ids = world
        .query_filtered::<Entity, With<Overlay>>()
        .iter(world)
        .collect::<Vec<_>>();
    for id in ids {
        if world.get_entity(id).is_ok() {
            world.despawn(id);
        }
    }
}
fn unload_on_menu(
    mut frontend: ResMut<Frontend>,
    mut menu: ResMut<MenuReturn>,
    mut requests: ResMut<Requests>,
    settings: Option<Res<SessionSettings>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    mut played: Local<u32>,
    audio: Query<(), (With<AudioPlayer>, Without<crate::game_audio::MenuMusic>)>,
) {
    // `--capture-exit-menu <frame>`: EXIT TO MAIN MENU after that many
    // gameplay frames, logging how many map sounds remain afterwards.
    if let Some(at) = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--capture-exit-menu")
        .and_then(|w| w[1].parse::<u32>().ok())
    {
        if frontend.playing() {
            *played += 1;
            if *played == at {
                println!("Capture: EXIT TO MAIN MENU ({} sounds playing)", audio.iter().count());
                frontend.page = Page::Home;
                frontend.started = false;
            }
        } else if *played >= at && *played < at + 120 {
            *played += 1;
            if *played % 30 == 0 {
                println!("Capture: menu frame {}: {} map sounds playing", *played - at, audio.iter().count());
            }
        }
    }
    let left = menu.was_started && !frontend.started;
    menu.was_started = frontend.started;
    if !left || !matches!(frontend.page, Page::Home | Page::Campaign) || requests.0.is_some() {
        return;
    }
    let (Some(settings), Some(source)) = (settings, source) else {
        return;
    };
    if crate::test_lab::active(&settings.map) {
        return;
    }
    let mut lab = settings.clone();
    lab.map = crate::test_lab::path();
    requests.0 = Some((source.0.clone(), lab));
    requests.1 = None;
    menu.pending = Some(frontend.page);
    println!("Menu: unloading {} (lab map behind the menu)", settings.map.display());
}
fn poll(world: &mut World) {
    // Opt-in native-window launch-path benchmark; no gameplay quality changes.
    let profile_route = std::env::args().any(|a| a == "--profile-lab-route");
    let profile_direct = std::env::args().any(|a| a == "--profile-lab-direct");
    if (profile_route || profile_direct)
        && world.resource::<Transition>().task.is_none()
        && world.resource::<Requests>().0.is_none()
        && !world.contains_resource::<crate::render_warmup::Warmup>()
    {
        world.resource_mut::<Probe>().ticks += 1;
        let ticks = world.resource::<Probe>().ticks;
        let loads = world.resource::<Probe>().loads;
        if profile_route && loads == 0 && ticks > 60 {
            let root = world.resource::<crate::game_ui::HudSource>().0.clone();
            let mut settings = world.resource::<SessionSettings>().clone();
            // `--profile-route-map <bsp>` measures a menu start of a real map.
            settings.map = std::env::args()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|a| a[0] == "--profile-route-map")
                .map_or_else(crate::test_lab::path, |a| std::path::PathBuf::from(&a[1]));
            world.resource_mut::<Requests>().0 = Some((root, settings));
            world.resource_mut::<Probe>().ticks = 0;
        } else if (profile_direct || loads > 0)
            && ticks
                > std::env::args()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .find(|a| a[0] == "--profile-ticks")
                    .and_then(|a| a[1].parse::<usize>().ok())
                    .unwrap_or(600)
            && !std::env::args().any(|a| a == "--profile-hold")
        {
            let map = world.resource::<SessionSettings>().map.clone();
            let meshes = world.query::<&Mesh3d>().iter(world).count();
            let cameras = world.query::<&Camera>().iter(world).count();
            let enemies = world.query::<&crate::game::Enemy>().iter(world).count();
            println!(
                "Lab launch benchmark: map={} meshes={meshes} cameras={cameras} enemies={enemies}",
                map.display()
            );
            if std::env::args().any(|a| a == "--touch") {
                let roots = world.query_filtered::<Entity, With<crate::game::touch::TouchRoot>>().iter(world).count();
                let buttons = world.query_filtered::<Entity, With<crate::game::touch::TouchButton>>().iter(world).count();
                assert_eq!((roots, buttons), (1, 8), "Map reload must restore touch controls");
                println!("Touch reload check: {roots} overlay, {buttons} buttons");
            }
            if let Some(path) = std::env::args().collect::<Vec<_>>().windows(2)
                .find(|a| a[0] == "--profile-route-capture").map(|a| a[1].clone()) {
                world.resource_mut::<Probe>().ticks = 0;
                world.spawn(bevy::render::view::screenshot::Screenshot::primary_window())
                    .observe(bevy::render::view::screenshot::save_to_disk(path))
                    .observe(|_: On<bevy::render::view::screenshot::ScreenshotCaptured>,
                        mut exit: MessageWriter<bevy::app::AppExit>| { exit.write(bevy::app::AppExit::Success); });
                return;
            }
            world.write_message(bevy::app::AppExit::Success);
            return;
        }
    }
    if std::env::args().any(|a| a == "--verify-window-reload") {
        let windows = world
            .query_filtered::<Entity, With<Window>>()
            .iter(world)
            .collect::<Vec<_>>();
        assert_eq!(windows.len(), 1, "Reload must preserve exactly one window");
        let id = windows[0];
        let loads = world.resource::<Probe>().loads;
        if let Some(baseline) = world.resource::<Probe>().baseline {
            if baseline != id {
                panic!("Map change replaced the window entity");
            }
        } else {
            world.resource_mut::<Probe>().baseline = Some(id);
            if std::env::args().any(|a| a == "--verify-campaign-carry") {
                let mut state = world.resource_mut::<Match>();
                state.health = 37.;
                state.medkit = true;
                state.pills = true;
            }
        }
        world.resource_mut::<Probe>().ticks += 1;
        if world.resource::<Transition>().task.is_none()
            && world.resource::<Requests>().0.is_none()
            && !world.contains_resource::<crate::render_warmup::Warmup>()
            && world.resource::<Probe>().ticks > 30
        {
            crate::survivor_glow::verify_scene(world);
            world.resource::<crate::ladders::Ladders>()
                .verify_scene(&world.resource::<crate::l4d2r_cinematic::MapLogic>().world);
            // `L4D2R_RELOADS=N` runs N map loads (lab, Hotel, lab, ...).
            let limit = std::env::var("L4D2R_RELOADS").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
            if loads >= limit {
                let report = world.resource::<Probe>().report.clone();
                let _ = std::fs::write("local-research/same-window-verification.tsv", report);
                world.write_message(bevy::app::AppExit::Success);
                return;
            }
            let root = world.resource::<crate::game_ui::HudSource>().0.clone();
            let mut settings = world.resource::<SessionSettings>().clone();
            settings.map = if loads == 1 {
                root.join("left4dead2/maps/c1m1_hotel.bsp")
            } else {
                crate::test_lab::path()
            };
            world.resource_mut::<Requests>().0 = Some((root, settings));
            world.resource_mut::<Probe>().ticks = 0;
        }
    }
    if world.resource::<Transition>().task.is_none() {
        let mobile = cfg!(any(target_os = "ios", target_os = "android"))
            || std::env::args().any(|a| a == "--profile-mobile-memory");
        if mobile && world.resource::<Requests>().0.is_some() {
            match world.resource::<Transition>().release_wait {
                None => {
                    let previous = world.resource::<Frontend>().page;
                    world.resource_mut::<Match>().paused = true;
                    crate::performance::gpu_audit("before release");
                    // Exit to menu keeps the menu on screen while the small
                    // laboratory map loads behind it; only map loads clear.
                    if world.resource::<MenuReturn>().pending.is_none() {
                        crate::game::clear_outgoing_map(world);
                    } else {
                        crate::game::clear_outgoing_world(world);
                    }
                    crate::viewer::release_mobile_map_assets(world);
                    if world.resource::<MenuReturn>().pending.is_none() {
                        let (root, map) = {
                            let request = world.resource::<Requests>().0.as_ref().unwrap();
                            (request.0.clone(), request.1.map.clone())
                        };
                        world.resource_mut::<Frontend>().page = Page::Loading;
                        overlay(world, &root, &map);
                    }
                    crate::game_loading::reset_progress();
                    world.resource_mut::<Transition>().release_wait = Some((if std::env::var_os("L4D2R_ENTITY_AUDIT").is_some() { AUDIT_RELEASE_FRAMES } else { RELEASE_FRAMES }, previous));
                    return;
                }
                Some((frames, previous)) if frames > 0 => {
                    if std::env::var_os("L4D2R_ENTITY_AUDIT").is_some() && frames % 10 == 0 {
                        crate::performance::memory_checkpoint(&format!("AUDIT release wait {frames}"));
                    }
                    if frames == 1 {
                        crate::performance::gpu_audit("release wait end");
                    }
                    world.resource_mut::<Transition>().release_wait = Some((frames - 1, previous));
                    return;
                }
                Some(_) => {
                    crate::performance::release_freed_memory("Outgoing map released");
                    crate::performance::gpu_audit("after release");
                    crate::performance::image_audit(world, "after release");
                }
            }
        }
        let released = world.resource_mut::<Transition>().release_wait.take();
        let request = world.resource_mut::<Requests>().0.take();
        if let Some((root, settings)) = request {
            let mut carry = world.resource_mut::<Requests>().1.take();
            if carry.is_none() && std::env::args().any(|a| a == "--verify-campaign-carry") {
                carry = Some(crate::campaign_flow::Carry {
                    player: world.resource::<Match>().clone(),
                    bots: world
                        .resource::<crate::game_state::Squad>()
                        .0
                        .iter()
                        .map(|m| m.state.clone())
                        .collect(),
                });
            }
            let previous = match released {
                // Mobile: released, paused and overlaid a few frames ago.
                Some((_, previous)) => previous,
                None => {
                    let previous = world.resource::<Frontend>().page;
                    world.resource_mut::<Match>().paused = true;
                    // Exit to menu: the menu shows at once and the old map
                    // closes now (its sounds stop); the lab map then loads
                    // silently behind the menu.
                    if world.resource::<MenuReturn>().pending.is_none() {
                        world.resource_mut::<Frontend>().page = Page::Loading;
                        overlay(world, &root, &settings.map);
                    } else {
                        crate::game::clear_outgoing_world(world);
                    }
                    crate::game_loading::reset_progress();
                    previous
                }
            };
            let result = Arc::new(Mutex::new(None));
            let out = result.clone();
            let task_root = root.clone();
            let task = LoaderTask::spawn(move || {
                crate::performance::memory_checkpoint("Map preparation begin");
                let path = settings.map.clone();
                let value = crate::game::prepare(&task_root, &path, None, false, Some(settings))
                    .and_then(|p| {
                        let mut prepared =
                            p.ok_or_else(|| std::io::Error::other("No playable map prepared"))?;
                        if let Some(carry) = carry {
                            crate::game::carry_prepared(&mut prepared, carry);
                        }
                        Ok(prepared)
                    });
                crate::performance::memory_checkpoint("Map preparation ready");
                *out.lock().unwrap() = Some(value);
            });
            *world.resource_mut::<Transition>() = Transition {
                task: Some(task),
                result: Some(result),
                root,
                cancelled: false,
                previous: Some(previous),
                release_wait: None,
            };
        }
        return;
    }
    if world
        .resource::<ButtonInput<KeyCode>>()
        .just_pressed(KeyCode::Escape)
        && world.resource::<MenuReturn>().pending.is_none()
    {
        world.resource_mut::<Transition>().cancelled = true;
    }
    let progress = crate::game_loading::progress();
    if std::env::args().any(|a| a == "--verify-window-reload")
        && progress >= 0.25
        && !world.resource::<Probe>().captured_loading
    {
        world.resource_mut::<Probe>().captured_loading = true;
        world
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(
                "local-research/same-window-loading.png",
            ));
    }
    let dt = world.resource::<Time>().delta_secs();
    for mut transform in world
        .query_filtered::<&mut UiTransform, With<LoadingSpinner>>()
        .iter_mut(world)
    {
        transform.rotation *= Rot2::radians(dt * 3.);
    }

    for mut node in world
        .query_filtered::<&mut Node, With<Fill>>()
        .iter_mut(world)
    {
        node.width = percent(progress * 100.);
    }
    if !world
        .resource::<Transition>()
        .task
        .as_ref()
        .unwrap()
        .is_finished()
    {
        return;
    }
    let (value, root, cancelled, previous) = {
        let mut t = world.resource_mut::<Transition>();
        let task = t.task.take().unwrap();
        let joined = task.join();
        let value = t
            .result
            .take()
            .unwrap()
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| {
                Err(std::io::Error::other(if joined.is_err() {
                    "Map worker failed"
                } else {
                    "No map result"
                }))
            });
        (
            value,
            t.root.clone(),
            t.cancelled,
            t.previous.unwrap_or(Page::Home),
        )
    };
    if cancelled {
        remove_overlay(world);
        world.resource_mut::<Frontend>().page = previous;
        world.resource_mut::<MenuReturn>().pending = None;
        return;
    }
    match value {
        Ok(prepared) => {
            crate::performance::memory_checkpoint("Map apply begin");
            // A silent menu load keeps whichever menu page is open now.
            let menu_page = world.resource::<Frontend>().page;
            crate::game::apply_prepared(world, prepared, &root);
            crate::performance::release_freed_memory("Map apply complete");
            crate::performance::gpu_audit("after apply");
            if std::env::var_os("L4D2R_ENTITY_AUDIT").is_some() {
                let mut rows: Vec<(usize, String)> = world
                    .archetypes()
                    .iter()
                    .filter(|a| !a.is_empty())
                    .map(|a| {
                        let names: Vec<String> = a
                            .components().iter()
                            .filter_map(|c| world.components().get_name(*c))
                            .map(|n| n.to_string().rsplit("::").next().unwrap_or_default().to_string())
                            .collect();
                        (a.len() as usize, names.join(","))
                    })
                    .collect();
                rows.sort_by(|a, b| b.0.cmp(&a.0));
                println!("ENTITY AUDIT: {} entities", world.entities().len());
                for (n, names) in rows.iter().take(25) {
                    println!("ENTITY AUDIT {n}: {}", &names[..names.len().min(300)]);
                }
            }
            if let Some(exit_page) = world.resource_mut::<MenuReturn>().pending.take() {
                let page = if matches!(menu_page, Page::Game | Page::Loading) { exit_page } else { menu_page };
                // Back to the menu, not into the lab: no game is running.
                *world.resource_mut::<Frontend>() = Frontend { page, mode: 0, restart: false, started: false };
                world.resource_mut::<Match>().paused = true;
                world.resource_mut::<MenuReturn>().was_started = false;
                // Render warmup still runs behind the menu (it keeps the game
                // paused off the Game page); only its loading artwork goes.
                remove_overlay(world);
            }
            if std::env::args().any(|a| a == "--verify-campaign-carry") {
                let state = world.resource::<Match>();
                assert_eq!(state.health, 37.);
                assert!(
                    state.medkit && state.pills,
                    "Chapter inventory was not carried"
                );
            }
            let id = world
                .query_filtered::<Entity, With<Window>>()
                .iter(world)
                .next();
            let map = world
                .resource::<SessionSettings>()
                .map
                .display()
                .to_string();
            let mut probe = world.resource_mut::<Probe>();
            probe.loads += 1;
            probe.ticks = 0;
            probe.report += &format!("{}\t{:?}\t{map}\n", std::process::id(), id);
            let _ = std::fs::write("local-research/same-window-verification.tsv", &probe.report);
            println!("Same-window check: loads={}, window={id:?}", probe.loads);
            println!("Map loaded in existing window / process");
        }
        Err(error) => {
            remove_overlay(world);
            world.resource_mut::<Frontend>().page = Page::Campaign;
            world
                .resource_mut::<crate::campaign_settings::Selection>()
                .error = format!("Map loading failed: {error}");
        }
    }
}
