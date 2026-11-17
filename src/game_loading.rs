//! Loading-screen capture diagnostics and shared native loading artwork helpers.
use bevy::prelude::*;
use std::{
    io,
    path::PathBuf,
    process::{Child, Command},
};

#[derive(Resource)]
struct Loading {
    root: PathBuf,
    map: PathBuf,
    args: Vec<String>,
    status: PathBuf,
    child: Option<Child>,
    capture: Option<String>,
    headless: bool,
    progress: f32,
}
impl Drop for Loading {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.status);
    }
}
#[derive(Component)]
struct Fill;
#[derive(Component)]
struct Spinner;
#[derive(Component)]
struct Caption;
#[derive(Resource)]
struct CaptureTarget(Handle<Image>);

fn argument(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
pub fn dimensions() -> (u32, u32) {
    let args = std::env::args().collect::<Vec<_>>();
    argument(&args, "--capture-size")
        .and_then(|s| {
            let v = s
                .split_whitespace()
                .filter_map(|s| s.parse::<u32>().ok())
                .collect::<Vec<_>>();
            (v.len() == 2 && v[0] >= 640 && v[1] >= 480).then(|| (v[0], v[1]))
        })
        .unwrap_or_else(|| crate::graphics_options::Graphics::default().size())
}
pub(crate) fn rect(x: f32, y: f32, w: f32, h: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: vh(x / 4.8),
        top: vh(y / 4.8),
        width: vh(w / 4.8),
        height: vh(h / 4.8),
        ..default()
    }
}
pub fn run(args: &[String]) -> io::Result<()> {
    let root =
        PathBuf::from(argument(args, "--game-dir").unwrap_or_else(|| {
            r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2".into()
        }));
    let map = args
        .get(2)
        .filter(|a| a.ends_with(".bsp"))
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("left4dead2/maps/c1m1_hotel.bsp"));
    let capture = (args[1] == "--capture-loading").then(|| {
        argument(args, "--capture-output")
            .unwrap_or_else(|| "local-research/loading-preview.png".into())
    });
    let status = std::env::current_dir()?
        .join("runtime")
        .join(format!("loading-{}.state", std::process::id()));
    std::fs::create_dir_all(status.parent().unwrap())?;
    let mut app = App::new();
    let headless = capture.is_some() || args[1] == "--verify-loading";
    let plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: if headless {
            None
        } else {
            Some(Window {
                title: "L4D2 RUST".into(),
                resolution: dimensions().into(),
                ..default()
            })
        },
        exit_condition: if headless {
            bevy::window::ExitCondition::DontExit
        } else {
            bevy::window::ExitCondition::OnPrimaryClosed
        },
        ..default()
    });
    if headless {
        app.add_plugins(plugins.disable::<bevy::winit::WinitPlugin>())
            .add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
                std::time::Duration::from_secs_f64(1. / 60.),
            ));
    } else {
        app.add_plugins(plugins);
    }
    app.insert_resource(Loading {
        root,
        map,
        args: args.to_vec(),
        status,
        child: None,
        capture,
        headless,
        progress: 0.05,
    })
    .add_systems(Startup, setup)
    .add_systems(Update, tick)
    .run();
    Ok(())
}
pub(crate) fn poster(map: &str) -> &'static str {
    let campaign = map.split('m').next().unwrap_or("c1");
    match campaign {
        "c1" => "campaign1",
        "c2" => "darkcarnival",
        "c3" => "swampfever",
        "c4" => "campaign4",
        "c5" => "theparish",
        "c6" => "thepassing",
        "c7" => "the_sacrifice",
        "c8" => "nomercy",
        "c9" => "crashcourse",
        "c10" => "deathtoll",
        "c11" => "deadair",
        "c12" => "bloodharvest",
        "c13" => "coldstream",
        "c14" => "laststand",
        _ => "default",
    }
}
fn setup(
    mut commands: Commands,
    mut loading: ResMut<Loading>,
    mut images: ResMut<Assets<Image>>,
    mut fonts: ResMut<Assets<Font>>,
) {
    let camera = commands.spawn(Camera2d).id();
    if loading.headless {
        let (w, h) = dimensions();
        let target = images.add(Image::new_target_texture(
            w,
            h,
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
            None,
        ));
        commands
            .entity(camera)
            .insert(bevy::camera::RenderTarget::Image(target.clone().into()));
        commands.insert_resource(CaptureTarget(target));
    }
    let mut assets = crate::source_assets::SourceAssets::new(&loading.root, vec![])
        .expect("Loading artwork mount");
    let font = assets
        .read("resource/stubble-bold.vfont")
        .and_then(|b| crate::vfont::decode(&b))
        .map(|b| crate::game_ui::shared_font(&mut *fonts, b))
        .unwrap_or_default();
    let map = loading
        .map
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let name = poster(&map);
    let wide = format!("materials/vgui/loadingscreen_{name}_widescreen.vtf");
    let path = if assets.read(&wide).is_ok() {
        wide
    } else {
        format!("materials/vgui/loadingscreen_{name}.vtf")
    };
    let background = crate::game_menu::image(&mut assets, &mut images, &path);
    let root = commands
        .spawn((
            ImageNode::new(background).with_mode(bevy::ui::widget::NodeImageMode::Stretch),
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
            UiTargetCamera(camera),
        ))
        .id();
    let strip = commands
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
    commands.spawn((
        Text::new("Joining a Campaign game."),
        text_font(14.),
        TextColor(Color::srgb(0.8, 0.8, 0.8)),
        rect(22., 6., 500., 20.),
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
    commands.spawn((
        Text::new(format!("Starring: {roster}")),
        text_font(11.),
        TextColor(Color::srgb(0.8, 0.8, 0.8)),
        rect(22., 25., 500., 18.),
        ChildOf(strip),
    ));
    commands.spawn((
        Text::new("Loading"),
        text_font(13.),
        TextColor(Color::srgb(0.8, 0.8, 0.8)),
        Node {
            position_type: PositionType::Absolute,
            right: vh(22. / 4.8),
            bottom: vh(35. / 4.8),
            ..default()
        },
        Caption,
        ChildOf(root),
    ));
    let bar = commands
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
    let bg = crate::game_menu::image(&mut assets, &mut images, "materials/vgui/loadingbar_bg.vtf");
    let fill = crate::game_menu::image(&mut assets, &mut images, "materials/vgui/loadingbar.vtf");
    commands.spawn((
        ImageNode::new(bg),
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            ..default()
        },
        ChildOf(bar),
    ));
    let clip = commands
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
    commands.spawn((
        ImageNode::new(fill).with_mode(bevy::ui::widget::NodeImageMode::Stretch),
        Node {
            flex_shrink: 0.,
            width: vh(135. / 4.8),
            height: vh(33. / 4.8),
            ..default()
        },
        ChildOf(clip),
    ));
    let spinner =
        crate::game_menu::image(&mut assets, &mut images, "materials/engine/l4d_spinner.vtf");
    commands.spawn((
        ImageNode::new(spinner),
        Node {
            position_type: PositionType::Absolute,
            right: vh(60. / 4.8),
            top: vh(45. / 4.8),
            width: vh(45. / 4.8),
            height: vh(45. / 4.8),
            ..default()
        },
        Spinner,
        ChildOf(root),
    ));
    if loading.capture.is_none() {
        let mut child = Command::new(std::env::current_exe().expect("Game executable"));
        child
            .arg(if loading.headless {
                "--capture-game"
            } else {
                "--game"
            })
            .arg(&loading.map)
            .args(loading.args.iter().skip(3))
            .arg("--load-status")
            .arg(&loading.status);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            child.creation_flags(0x08000000);
        }
        match child.spawn() {
            Ok(child) => loading.child = Some(child),
            Err(e) => eprintln!("Game launch failed: {e}"),
        }
    }
}
fn tick(
    mut commands: Commands,
    mut loading: ResMut<Loading>,
    mut fills: Query<&mut Node, With<Fill>>,
    mut captions: Query<&mut Text, With<Caption>>,
    target: Option<Res<CaptureTarget>>,
    mut frames: Local<u32>,
    time: Res<Time>,
    mut spinners: Query<&mut UiTransform, With<Spinner>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut exit: MessageWriter<AppExit>,
) {
    *frames += 1;
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
        return;
    }
    for mut spinner in &mut spinners {
        *spinner = UiTransform::from_rotation(Rot2::degrees(time.elapsed_secs() * 90.));
    }
    if let Some(path) = loading.capture.clone() {
        loading.progress = 0.55;
        if *frames == 90 {
            if let Some(target) = target {
                commands
                    .spawn(bevy::render::view::screenshot::Screenshot::image(
                        target.0.clone(),
                    ))
                    .observe(bevy::render::view::screenshot::save_to_disk(path))
                    .observe(
                        |_: On<bevy::render::view::screenshot::ScreenshotCaptured>,
                         mut exit: MessageWriter<AppExit>| {
                            exit.write(AppExit::Success);
                        },
                    );
            }
        }
    } else {
        let state = std::fs::read_to_string(&loading.status).unwrap_or_default();
        if state == "ready" {
            println!("Loading handoff: game ready");
            loading.child.take();
            exit.write(AppExit::Success);
        } else if let Ok(progress) = state.parse::<f32>() {
            loading.progress = progress.clamp(0., 1.);
        }
        if loading
            .child
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_some())
            || loading.child.is_none() && state != "ready"
        {
            for mut text in &mut captions {
                text.0 = "Map loading failed".into();
            }
            if loading.headless {
                exit.write(AppExit::error());
            }
        }
    }
    for mut fill in &mut fills {
        fill.width = percent(loading.progress * 100.);
    }
}
pub fn milestone(progress: f32) {
    PROGRESS.store(
        (progress * 1000.) as u32,
        std::sync::atomic::Ordering::Relaxed,
    );
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(path) = argument(&args, "--load-status") {
        let _ = std::fs::write(path, progress.to_string());
    }
}
static PROGRESS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub(crate) fn reset_progress() {
    PROGRESS.store(0, std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn progress() -> f32 {
    PROGRESS.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.
}
pub fn ready(world: &mut World) {
    let was_warming = world.contains_resource::<crate::render_warmup::Warmup>();
    crate::render_warmup::ready(world);
    if world.contains_resource::<crate::render_warmup::Warmup>() {
        return;
    }
    if was_warming {
        for mut window in world.query::<&mut Window>().iter_mut(world) {
            window.visible = true;
            window.focused = true;
        }
        let args = std::env::args().collect::<Vec<_>>();
        if let Some(path) = argument(&args, "--load-status") {
            let _ = std::fs::write(path, "ready");
        }
    }
}
pub fn visible(windows: Query<&Window>, warm: Option<Res<crate::render_warmup::Warmup>>) -> bool {
    warm.is_none() && windows.iter().all(|w| w.visible)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn campaign_posters_match_map_prefixes() {
        assert_eq!(poster("c1m1_hotel"), "campaign1");
        assert_eq!(poster("c10m1_caves"), "deathtoll");
        assert_eq!(poster("c14m1_junkyard"), "laststand");
    }
    #[test]
    fn hidden_game_window_holds_simulation_until_handoff() {
        #[derive(Resource, Default)]
        struct Ticks(u32);
        fn advance(mut ticks: ResMut<Ticks>) {
            ticks.0 += 1;
        }
        let mut app = App::new();
        app.init_resource::<Ticks>()
            .add_systems(Update, advance.run_if(visible));
        let window = app
            .world_mut()
            .spawn(Window {
                visible: false,
                ..default()
            })
            .id();
        app.update();
        assert_eq!(app.world().resource::<Ticks>().0, 0);
        app.world_mut().get_mut::<Window>(window).unwrap().visible = true;
        app.update();
        assert_eq!(app.world().resource::<Ticks>().0, 1);
    }
}
