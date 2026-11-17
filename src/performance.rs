//! Opt-in frame-time evidence; disk output happens at shutdown, outside gameplay.
use bevy::prelude::*;
/// iPhone, or the PC stand-in `--profile-mobile-memory`.
pub(crate) fn mobile_memory_profile() -> bool {
    cfg!(target_os = "ios") || std::env::args().any(|a| a == "--profile-mobile-memory")
}
/// The render device, for GPU allocation reports in memory checkpoints.
pub(crate) static RENDER_DEVICE: std::sync::OnceLock<bevy::render::renderer::RenderDevice> = std::sync::OnceLock::new();
pub(crate) fn keep_render_device(device: Option<Res<bevy::render::renderer::RenderDevice>>) {
    if let Some(device) = device {
        let _ = RENDER_DEVICE.set(device.clone());
    }
}
/// PC `--profile-mobile-memory`: process commit (CPU side) and GPU
/// allocations, which together approximate an iPhone's physical footprint.
#[cfg(windows)]
fn pc_memory() -> (u64, Option<u64>) {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_nonpaged: usize,
        quota_nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    let gpu = RENDER_DEVICE
        .get()
        .and_then(|d| d.wgpu_device().generate_allocator_report())
        .map(|r| r.total_allocated_bytes);
    (c.pagefile as u64, gpu)
}
/// Return freed memory to the OS. Map data is built on a short-lived loading
/// thread; when that thread exits mimalloc abandons its pages, and freeing
/// the old map later leaves them committed until a forced collection, which
/// iOS counts as live (the campaign-change memory kill).
pub(crate) fn release_freed_memory(stage: &str) {
    collect_all_threads();
    memory_checkpoint(stage);
    crate::heap_audit::dump(stage);
    if std::env::var_os("L4D2R_MI_STATS").is_some() {
        unsafe extern "C" fn out(msg: *const std::ffi::c_char, _arg: *mut std::ffi::c_void) {
            let text = unsafe { std::ffi::CStr::from_ptr(msg) }.to_string_lossy();
            eprint!("MI STATS {text}");
        }
        unsafe extern "C" {
            fn mi_stats_print_out(out: unsafe extern "C" fn(*const std::ffi::c_char, *mut std::ffi::c_void), arg: *mut std::ffi::c_void);
        }
        eprintln!("MI STATS {stage}:");
        unsafe { mi_stats_print_out(out, std::ptr::null_mut()) };
    }
}
/// Bytes mimalloc has committed from the OS: beside the live heap, this
/// separates allocator retention/fragmentation from memory outside the heap.
pub(crate) fn allocator_committed() -> usize {
    unsafe extern "C" {
        fn mi_process_info(
            elapsed: *mut usize, user: *mut usize, system: *mut usize, rss: *mut usize,
            peak_rss: *mut usize, commit: *mut usize, peak_commit: *mut usize, faults: *mut usize,
        );
    }
    let mut v = [0usize; 8];
    unsafe {
        mi_process_info(&mut v[0], &mut v[1], &mut v[2], &mut v[3], &mut v[4], &mut v[5], &mut v[6], &mut v[7]);
    }
    v[5]
}
unsafe extern "C" {
    fn mi_collect(force: bool);
}
/// Set when every thread should hand its freed pages back; the render
/// thread checks it in `collect_render_thread`.
static COLLECT_RENDER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// mimalloc keeps each thread's freed pages with that thread until the
/// thread itself collects. Bevy's task-pool and render threads live for the
/// whole session, so a released map's memory stayed committed there: after
/// Hotel -> exit to menu the PC process kept ~870 MiB while live allocations
/// were only ~84 MiB above a fresh menu (the iPhone motel-load kill).
pub(crate) fn collect_all_threads() {
    use bevy::tasks::{AsyncComputeTaskPool, ComputeTaskPool, IoTaskPool};
    let pools: [Option<&bevy::tasks::TaskPool>; 3] = [
        ComputeTaskPool::try_get().map(|p| &**p),
        AsyncComputeTaskPool::try_get().map(|p| &**p),
        IoTaskPool::try_get().map(|p| &**p),
    ];
    for pool in pools.into_iter().flatten() {
        let threads = pool.thread_num().max(1);
        // Each task holds its thread briefly, so the batch lands on every
        // worker of the pool.
        pool.scope(|scope| {
            for _ in 0..threads * 2 {
                scope.spawn(async {
                    unsafe { mi_collect(true) };
                    std::thread::sleep(std::time::Duration::from_millis(2));
                });
            }
        });
    }
    COLLECT_RENDER.store(true, std::sync::atomic::Ordering::Relaxed);
    crate::map_transition::collect_loader();
    unsafe { mi_collect(true) };
}
/// Render-world half of `collect_all_threads`.
pub(crate) fn collect_render_thread() {
    if COLLECT_RENDER.swap(false, std::sync::atomic::Ordering::Relaxed) {
        unsafe { mi_collect(true) };
    }
}
/// Registers the render-thread collection.
pub(crate) fn install_memory_release(app: &mut App) {
    if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
        render.add_systems(bevy::render::Render, collect_render_thread);
    }
}
/// `--capture-shots <dir> <frame>,<frame>...`: save window screenshots at
/// those frames (counted from startup), e.g. to check a map after a switch.
pub(crate) fn frame_shots(mut commands: Commands, mut frame: Local<u32>) {
    *frame += 1;
    static SHOTS: std::sync::OnceLock<Option<(String, Vec<u32>)>> = std::sync::OnceLock::new();
    let Some((dir, frames)) = SHOTS.get_or_init(|| {
        let args: Vec<String> = std::env::args().collect();
        let i = args.iter().position(|a| a == "--capture-shots")?;
        let frames = args.get(i + 2)?.split(',').filter_map(|f| f.parse().ok()).collect();
        Some((args.get(i + 1)?.clone(), frames))
    }) else {
        return;
    };
    if frames.contains(&*frame) {
        let path = format!("{dir}/shot_{}.png", *frame);
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(path));
    }
}
/// `L4D2R_GPU_AUDIT=1`: the largest live GPU allocations grouped by label.
pub(crate) fn gpu_audit(stage: &str) {
    if std::env::var_os("L4D2R_GPU_AUDIT").is_none() {
        return;
    }
    let Some(report) = RENDER_DEVICE.get().and_then(|d| d.wgpu_device().generate_allocator_report()) else {
        eprintln!("GPU AUDIT {stage}: no report");
        return;
    };
    let mut groups: std::collections::HashMap<String, (u64, usize)> = Default::default();
    for a in &report.allocations {
        let key: String = a.name.chars().filter(|c| !c.is_ascii_digit()).take(60).collect();
        let e = groups.entry(key).or_default();
        e.0 += a.size;
        e.1 += 1;
    }
    let mut rows: Vec<_> = groups.into_iter().collect();
    rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    eprintln!("GPU AUDIT {stage}: {:.1} MiB in {} allocations, {:.1} MiB reserved", report.total_allocated_bytes as f64 / 1048576., report.allocations.len(), report.total_reserved_bytes as f64 / 1048576.);
    for (name, (size, count)) in rows.iter().take(15) {
        eprintln!("GPU AUDIT {stage}:   {:8.1} MiB x{count:5}  {name}", *size as f64 / 1048576.);
    }
    let mut sizes: Vec<(u64, &str)> = report.allocations.iter().map(|a| (a.size, a.name.as_str())).collect();
    sizes.sort_by(|a, b| b.0.cmp(&a.0));
    let largest: Vec<String> = sizes.iter().take(12).map(|(s, n)| format!("{:.1}M {}", *s as f64 / 1048576., n.chars().take(24).collect::<String>())).collect();
    eprintln!("GPU AUDIT {stage}: largest {}", largest.join(", "));
    let mut textures: Vec<u64> = report.allocations.iter().filter(|a| a.name.contains("Unlabeled texture")).map(|a| a.size).collect();
    textures.sort_unstable_by(|a, b| b.cmp(a));
    let bucket = |lo: u64, hi: u64| textures.iter().filter(|s| **s >= lo && **s < hi).fold((0usize, 0u64), |(n, t), s| (n + 1, t + s));
    for (lo, hi, label) in [(16u64 << 20, u64::MAX, ">=16M"), (4 << 20, 16 << 20, "4-16M"), (1 << 20, 4 << 20, "1-4M"), (0, 1 << 20, "<1M")] {
        let (n, t) = bucket(lo, hi);
        eprintln!("GPU AUDIT {stage}: textures {label}: {n} totalling {:.1} MiB", t as f64 / 1048576.);
    }
}
pub(crate) fn memory_checkpoint(stage: &str) {
    #[cfg(windows)]
    if mobile_memory_profile() {
        let (cpu, gpu) = pc_memory();
        let mib = |b: u64| b as f64 / 1048576.;
        match gpu {
            Some(gpu) => eprintln!("PC memory: {stage}: {:.1} MiB CPU + {:.1} MiB GPU = {:.1} MiB; live heap {:.1} MiB; allocator committed {:.1} MiB", mib(cpu), mib(gpu), mib(cpu + gpu), crate::live_heap_bytes() as f64 / 1048576., allocator_committed() as f64 / 1048576.),
            None => eprintln!("PC memory: {stage}: {:.1} MiB CPU (GPU report unavailable)", mib(cpu)),
        }
    }
    #[cfg(target_os = "ios")]
    {
        unsafe extern "C" {
            fn l4d2r_memory_bytes() -> u64;
            fn l4d2r_metal_bytes() -> u64;
            fn mi_collect(force: bool);
        }
        // Map/atlas milestones are outside play: return retired allocator
        // pages now instead of carrying them into the next upload peak.
        unsafe { mi_collect(true); }
        let bytes = unsafe { l4d2r_memory_bytes() };
        let metal = unsafe { l4d2r_metal_bytes() };
        eprintln!(
            "iOS memory: {stage}: {:.1} MiB physical footprint ({:.1} MiB Metal, live heap {:.1} MiB, allocator committed {:.1} MiB)",
            bytes as f64 / 1048576.,
            metal as f64 / 1048576.,
            crate::live_heap_bytes() as f64 / 1048576.,
            allocator_committed() as f64 / 1048576.
        );
    }
    #[cfg(not(target_os = "ios"))]
    let _ = stage;
}
/// iPhone: logs the memory footprint during play every 20 s, and every 5 s
/// while iOS leaves under 300 MiB before it would close the app, so a log
/// kept from a session that was closed for memory shows how close it came.
pub(crate) fn play_memory(
    time: Res<Time<Real>>,
    mut next: Local<f32>,
    mut peak: Local<u64>,
    images: Res<Assets<Image>>,
    meshes: Res<Assets<Mesh>>,
    entities: Query<Entity>,
) {
    // PC phone profile: the same timeline with asset counts, to find what
    // grows during long play.
    #[cfg(windows)]
    if mobile_memory_profile() {
        let now = time.elapsed_secs();
        if now >= *next {
            *next = now + 10.;
            let (cpu, gpu) = pc_memory();
            let mib = |b: u64| b as f64 / 1048576.;
            eprintln!(
                "PC memory (play {now:.0} s): {:.1} MiB CPU + {:.1} MiB GPU; live heap {:.1} MiB; {} images, {} meshes, {} entities",
                mib(cpu),
                mib(gpu.unwrap_or(0)),
                crate::live_heap_bytes() as f64 / 1048576.,
                images.len(),
                meshes.len(),
                entities.iter().len()
            );
        }
    }
    #[cfg(not(windows))]
    let _ = (&images, &meshes, &entities);
    #[cfg(target_os = "ios")]
    {
        unsafe extern "C" {
            fn l4d2r_memory_bytes() -> u64;
            fn l4d2r_metal_bytes() -> u64;
            fn os_proc_available_memory() -> usize;
        }
        let now = time.elapsed_secs();
        if now < *next {
            return;
        }
        let bytes = unsafe { l4d2r_memory_bytes() };
        let left = unsafe { os_proc_available_memory() } as u64;
        *peak = (*peak).max(bytes);
        let mib = |b: u64| b as f64 / 1048576.;
        let metal = unsafe { l4d2r_metal_bytes() };
        eprintln!(
            "iOS memory (play {now:.0} s): {:.1} MiB physical footprint ({:.1} MiB Metal), peak {:.1} MiB, {:.1} MiB left before iOS closes the app",
            mib(bytes),
            mib(metal),
            mib(*peak),
            mib(left)
        );
        *next = now + if left < 300 << 20 { 5. } else { 20. };
    }
    #[cfg(not(target_os = "ios"))]
    let _ = (time, &mut next, &mut peak);
}
#[derive(Resource)]
pub struct DisplaySettings {
    pub show_fps: bool,
    pub vsync: bool,
    pub viewmodel_fov: f32,
    pub camera_fov: f32,
}
impl Default for DisplaySettings {
    fn default() -> Self {
        let saved = std::fs::read_to_string("runtime/display-options.ini").unwrap_or_default();
        Self {
            show_fps: saved.lines().any(|l| l == "show_fps=true")
                || std::env::args().any(|a| a == "--show-fps"),
            vsync: saved.lines().any(|l| l == "vsync=true"),
            camera_fov: saved
                .lines()
                .find_map(|l| l.strip_prefix("camera_fov=")?.parse::<f32>().ok())
                .filter(|f| f.is_finite() && (1. ..179.).contains(f))
                .unwrap_or(90.),
            viewmodel_fov: std::env::args()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|a| a[0] == "--capture-viewmodel-fov")
                .and_then(|a| a[1].parse::<f32>().ok())
                .or_else(|| {
                    saved
                        .lines()
                        .find_map(|l| l.strip_prefix("viewmodel_fov=")?.parse::<f32>().ok())
                })
                .filter(|f| f.is_finite() && (1. ..179.).contains(f))
                .unwrap_or(51.),
        }
    }
}
impl DisplaySettings {
    pub fn save(&self) {
        let _ = std::fs::create_dir_all("runtime");
        if let Err(error) = std::fs::write(
            "runtime/display-options.ini",
            format!(
                "show_fps={}\nvsync={}\nviewmodel_fov={}\ncamera_fov={}\n",
                self.show_fps, self.vsync, self.viewmodel_fov, self.camera_fov
            ),
        ) {
            eprintln!("Could not save display options: {error}");
        }
    }
    pub fn command(&mut self, command: &str) -> Option<String> {
        let mut words = command.split_whitespace();
        let name = words.next()?.to_ascii_lowercase();
        let (target, label) = match name.as_str() {
            "fov" | "fov_desired" => (&mut self.camera_fov, "fov_desired"),
            "viewmodel_fov" | "cl_viewmodelfovsurvivor" => {
                (&mut self.viewmodel_fov, "cl_viewmodelfovsurvivor")
            }
            _ => return None,
        };
        if let Some(value) = words.next() {
            if words.next().is_some() {
                return Some(format!("Usage: {label} <degrees>"));
            }
            let Ok(fov) = value.parse::<f32>() else {
                return Some("Expected a field of view in degrees".into());
            };
            if !fov.is_finite() || !(1. ..179.).contains(&fov) {
                return Some("Field of view must be between 1 and 179 degrees".into());
            }
            *target = fov;
            self.save();
        }
        let value = if label == "fov_desired" {
            self.camera_fov
        } else {
            self.viewmodel_fov
        };
        Some(format!("{label} {value}"))
    }
}
pub fn viewmodel_vertical_fov(horizontal: f32) -> f32 {
    2. * ((horizontal.to_radians() * 0.5).tan() / (4. / 3.)).atan()
}
#[derive(Component)]
pub struct DisplayLabel(pub bool);
#[derive(Component)]
pub struct FpsOverlay;
#[derive(Resource, Default)]
pub struct FpsSample {
    elapsed: f64,
    frames: usize,
}
pub fn setup(mut commands: Commands, camera: Single<Entity, With<crate::game::ViewCamera>>) {
    commands.init_resource::<FpsSample>();
    commands.spawn((
        Text::new("FPS: --"),
        TextFont {
            font_size: FontSize::Px(18.),
            ..default()
        },
        TextColor(Color::srgb(0.75, 1., 0.45)),
        Node {
            position_type: PositionType::Absolute,
            top: px(8.),
            right: px(12.),
            ..default()
        },
        GlobalZIndex(1000),
        UiTargetCamera(*camera),
        FpsOverlay,
    ));
}
pub fn display(
    time: Res<Time<Real>>,
    settings: Res<DisplaySettings>,
    cinematic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    mut sample: ResMut<FpsSample>,
    mut overlay: Query<(&mut Text, &mut Node), With<FpsOverlay>>,
    mut labels: Query<(&DisplayLabel, &mut Text), Without<FpsOverlay>>,
    mut windows: Query<&mut Window>,
    mut cameras: Query<&mut Projection, With<crate::game::ViewCamera>>,
    mut world_cameras: Query<
        &mut Projection,
        (
            With<crate::viewer::GameCamera>,
            Without<crate::game::ViewCamera>,
        ),
    >,
) {
    let world_fov = viewmodel_vertical_fov(settings.camera_fov);
    for mut camera in world_cameras
        .iter_mut()
        .filter(|_| !cinematic.as_ref().is_some_and(|c| c.active))
    {
        if let Projection::Perspective(p) = &mut *camera {
            p.fov = world_fov;
        }
    }
    let fov = viewmodel_vertical_fov(settings.viewmodel_fov);
    for mut camera in &mut cameras {
        if let Projection::Perspective(p) = &mut *camera {
            if p.fov != fov {
                p.fov = fov;
            }
        }
    }
    for mut window in &mut windows {
        let mode = if settings.vsync {
            bevy::window::PresentMode::AutoVsync
        } else {
            bevy::window::PresentMode::AutoNoVsync
        };
        if window.present_mode != mode {
            window.present_mode = mode;
        }
    }
    for (kind, mut label) in &mut labels {
        let (name, enabled) = if kind.0 {
            ("VSYNC", settings.vsync)
        } else {
            ("SHOW FPS", settings.show_fps)
        };
        let next = format!("{name}: {}", if enabled { "ON" } else { "OFF" });
        if label.0 != next {
            label.0 = next;
        }
    }
    sample.elapsed += time.delta_secs_f64();
    sample.frames += 1;
    let measurement = if sample.elapsed >= 0.25 {
        let fps = sample.frames as f64 / sample.elapsed;
        let text = format!(
            "FPS: {fps:.0}  ({:.1} ms)",
            sample.elapsed * 1000. / sample.frames as f64
        );
        sample.elapsed = 0.;
        sample.frames = 0;
        Some(text)
    } else {
        None
    };
    for (mut text, mut node) in &mut overlay {
        let display = if settings.show_fps {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
        if let Some(next) = &measurement {
            if text.0 != *next {
                text.0.clone_from(next);
            }
        }
    }
}
#[derive(Resource, Default)]
pub struct FrameProfile {
    frames: Vec<f64>,
    warmup: usize,
    detail: String,
    previous_cpu: String,
}
#[derive(Resource, Clone, Default)]
pub struct PipelineStatus(pub std::sync::Arc<std::sync::Mutex<(usize, usize)>>);
pub fn install_profile(app: &mut App) {
    // GPU statistics/readback have measurable driver cost. Keep ordinary FPS
    // comparisons on the same render path as play, with GPU details opt-in.
    if std::env::args().any(|a| a == "--profile-render") {
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);
    }
    app.init_resource::<StageClocks>()
        .add_systems(First, |mut clocks: ResMut<StageClocks>| {
            clocks.0.insert("engine::main", scope("engine::main"));
        })
        .add_systems(
            Last,
            (|mut clocks: ResMut<StageClocks>| {
                clocks.0.remove("engine::main");
            })
            .before(record),
        )
        .add_systems(
            PostUpdate,
            (|mut clocks: ResMut<StageClocks>| {
                clocks
                    .0
                    .insert("engine::ui_layout", scope("engine::ui_layout"));
            })
            .after(bevy::ui::UiSystems::Content)
            .before(bevy::ui::UiSystems::Layout),
        )
        .add_systems(
            PostUpdate,
            (|mut clocks: ResMut<StageClocks>| {
                clocks.0.remove("engine::ui_layout");
            })
            .after(bevy::ui::UiSystems::Layout)
            .before(bevy::ui::UiSystems::PostLayout),
        );
    // Transform propagation and visibility scale with entity counts (every
    // GPU-skinned draw owns per-bone joint entities).
    use bevy::camera::visibility::VisibilitySystems;
    use bevy::transform::TransformSystems;
    app.add_systems(
        PostUpdate,
        (|mut clocks: ResMut<StageClocks>| {
            clocks
                .0
                .insert("engine::transforms", scope("engine::transforms"));
        })
        .before(TransformSystems::Propagate),
    )
    .add_systems(
        PostUpdate,
        (|mut clocks: ResMut<StageClocks>| {
            clocks.0.remove("engine::transforms");
        })
        .after(TransformSystems::Propagate),
    )
    .add_systems(
        PostUpdate,
        (|mut clocks: ResMut<StageClocks>| {
            clocks
                .0
                .insert("engine::visibility", scope("engine::visibility"));
        })
        .after(VisibilitySystems::UpdateFrusta)
        .before(VisibilitySystems::CheckVisibility),
    )
    .add_systems(
        PostUpdate,
        (|mut clocks: ResMut<StageClocks>| {
            clocks.0.remove("engine::visibility");
        })
        .after(VisibilitySystems::CheckVisibility),
    );
    if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
        use bevy::render::RenderSystems;
        render.init_resource::<StageClocks>();
        for (name, previous, set, next) in [
            (
                "engine::prepare_assets",
                RenderSystems::ExtractCommands,
                RenderSystems::PrepareAssets,
                RenderSystems::PrepareMeshes,
            ),
            (
                "engine::prepare_meshes",
                RenderSystems::PrepareAssets,
                RenderSystems::PrepareMeshes,
                RenderSystems::CreateViews,
            ),
            (
                "engine::specialize",
                RenderSystems::CreateViews,
                RenderSystems::Specialize,
                RenderSystems::PrepareViews,
            ),
            (
                "engine::queue",
                RenderSystems::PrepareViews,
                RenderSystems::Queue,
                RenderSystems::PhaseSort,
            ),
            (
                "engine::prepare",
                RenderSystems::PhaseSort,
                RenderSystems::Prepare,
                RenderSystems::Render,
            ),
            (
                "engine::render",
                RenderSystems::Prepare,
                RenderSystems::Render,
                RenderSystems::Cleanup,
            ),
        ] {
            add_stage_clock(render, name, previous, set, next);
        }
        // Finer split of Prepare (lights/shadows, batching, phase buffers,
        // bind groups) so render-thread growth can be attributed.
        for (name, previous, set, next) in [
            (
                "engine::prepare_resources",
                RenderSystems::PhaseSort,
                RenderSystems::PrepareResources,
                RenderSystems::PrepareResourcesBatchPhases,
            ),
            (
                "engine::prepare_batch",
                RenderSystems::PrepareResources,
                RenderSystems::PrepareResourcesBatchPhases,
                RenderSystems::PrepareResourcesWritePhaseBuffers,
            ),
            (
                "engine::prepare_phase_buffers",
                RenderSystems::PrepareResourcesBatchPhases,
                RenderSystems::PrepareResourcesWritePhaseBuffers,
                RenderSystems::PrepareResourcesFlush,
            ),
            (
                "engine::prepare_bind_groups",
                RenderSystems::PrepareResourcesFlush,
                RenderSystems::PrepareBindGroups,
                RenderSystems::Render,
            ),
        ] {
            add_stage_clock(render, name, previous, set, next);
        }
    }
}
fn add_stage_clock(
    render: &mut bevy::app::SubApp,
    name: &'static str,
    previous: bevy::render::RenderSystems,
    set: bevy::render::RenderSystems,
    next: bevy::render::RenderSystems,
) {
    use bevy::render::Render;
    render.add_systems(
        Render,
        (move |mut clocks: ResMut<StageClocks>| {
            clocks.0.insert(name, scope(name));
        })
        .after(previous)
        .before(set.clone()),
    );
    render.add_systems(
        Render,
        (move |mut clocks: ResMut<StageClocks>| {
            clocks.0.remove(name);
            flush_thread_metrics();
        })
        .after(set)
        .before(next),
    );
}
#[derive(Resource, Default)]
struct StageClocks(std::collections::BTreeMap<&'static str, Scope>);
/// Frames recorded by `record`; trace windows are keyed on it.
static PROFILE_FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg_attr(not(feature = "profile-trace"), allow(dead_code))]
const TRACE_WINDOW: u64 = 300;
#[cfg(feature = "profile-trace")]
static TRACE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, std::collections::BTreeMap<u64, f64>>>,
> = std::sync::OnceLock::new();
/// `profile-trace` builds: aggregates every Bevy system span (bevy_ecs
/// `trace`) into milliseconds per 300-frame window, written at exit to
/// local-research/trace-systems.tsv.
#[cfg(feature = "profile-trace")]
pub fn trace_layer(_: &mut App) -> Option<bevy::log::BoxedLayer> {
    use bevy::log::tracing::{Subscriber, field, span};
    use bevy::log::tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};
    struct Name(String);
    struct Entered(std::time::Instant);
    struct Visitor(Option<String>);
    impl field::Visit for Visitor {
        fn record_str(&mut self, f: &field::Field, value: &str) {
            if f.name() == "name" {
                self.0 = Some(value.to_owned());
            }
        }
        fn record_debug(&mut self, f: &field::Field, value: &dyn std::fmt::Debug) {
            if f.name() == "name" {
                self.0 = Some(format!("{value:?}"));
            }
        }
    }
    struct Aggregate;
    impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Aggregate {
        fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
            let mut visitor = Visitor(None);
            attrs.record(&mut visitor);
            if let Some(span) = ctx.span(id) {
                let label = attrs.metadata().name();
                let name = visitor
                    .0
                    .map_or_else(|| label.to_owned(), |n| format!("{label}:{n}"));
                span.extensions_mut().insert(Name(name));
            }
        }
        fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
            if let Some(span) = ctx.span(id) {
                span.extensions_mut()
                    .replace(Entered(std::time::Instant::now()));
            }
        }
        fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
            let Some(span) = ctx.span(id) else {
                return;
            };
            let extensions = span.extensions();
            let (Some(name), Some(entered)) =
                (extensions.get::<Name>(), extensions.get::<Entered>())
            else {
                return;
            };
            let ms = entered.0.elapsed().as_secs_f64() * 1000.;
            let window = PROFILE_FRAME.load(std::sync::atomic::Ordering::Relaxed) / TRACE_WINDOW;
            let mut trace = TRACE.get_or_init(Default::default).lock().unwrap();
            *trace
                .entry(name.0.clone())
                .or_default()
                .entry(window)
                .or_default() += ms;
        }
    }
    Some(Box::new(Aggregate))
}
#[cfg(feature = "profile-trace")]
fn write_trace() {
    let Some(trace) = TRACE.get() else {
        return;
    };
    let trace = trace.lock().unwrap();
    let windows = trace
        .values()
        .flat_map(|w| w.keys().copied())
        .max()
        .unwrap_or(0);
    let mut text = String::from("system");
    for w in 0..=windows {
        text += &format!("\tw{}", w * TRACE_WINDOW);
    }
    text.push('\n');
    let mut rows = trace.iter().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.1.values()
            .sum::<f64>()
            .total_cmp(&a.1.values().sum::<f64>())
    });
    for (name, per) in rows {
        text += name;
        for w in 0..=windows {
            // Milliseconds per frame within the window.
            text += &format!(
                "\t{:.3}",
                per.get(&w).copied().unwrap_or(0.) / TRACE_WINDOW as f64
            );
        }
        text.push('\n');
    }
    let _ = std::fs::write("local-research/trace-systems.tsv", text);
}
/// Opt-in (`--profile-counts`) scene-size log, printed every 120 frames, to
/// correlate render-thread growth with what accumulates in the world.
pub fn install_counts(app: &mut App) {
    if std::env::args().any(|a| a == "--profile-counts") {
        app.add_systems(Last, log_counts);
    }
}
#[allow(clippy::type_complexity)]
fn log_counts(
    mut frame: Local<u64>,
    entities: Query<Entity>,
    draws: Query<&ViewVisibility, With<Mesh3d>>,
    skins: Query<&ViewVisibility, With<bevy::mesh::skinning::SkinnedMesh>>,
    enemies: Query<&crate::game::Enemy>,
    lightmaps: Query<&bevy::pbr::Lightmap>,
    meshes: Res<Assets<Mesh>>,
    source: Res<Assets<crate::viewer::SourceMaterial>>,
    standard: Res<Assets<StandardMaterial>>,
    state: Option<Res<crate::game_state::Match>>,
    views: (Query<(&Camera, Option<&Msaa>)>, Query<&Window>),
    images: Res<Assets<Image>>,
    materials: Query<
        (
            &ViewVisibility,
            &MeshMaterial3d<crate::viewer::SourceMaterial>,
            Option<&crate::viewer::DynamicModelLighting>,
        ),
        With<Mesh3d>,
    >,
    mut mesh_events: (
        MessageReader<AssetEvent<Mesh>>,
        Local<[usize; 3]>,
        Local<std::collections::HashMap<usize, usize>>,
    ),
) {
    *frame += 1;
    // Mesh asset churn re-runs Bevy's mesh allocator; bucket modified meshes
    // by vertex count to identify the system rewriting them.
    for event in mesh_events.0.read() {
        match event {
            AssetEvent::Added { .. } => mesh_events.1[0] += 1,
            AssetEvent::Modified { id } => {
                mesh_events.1[1] += 1;
                let vertices = meshes.get(*id).map_or(0, Mesh::count_vertices);
                *mesh_events.2.entry(vertices).or_default() += 1;
            }
            AssetEvent::Removed { .. } => mesh_events.1[2] += 1,
            _ => {}
        }
    }
    if *frame % 120 != 0 {
        return;
    }
    let mut top = mesh_events.2.drain().collect::<Vec<_>>();
    top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    top.truncate(6);
    println!(
        "Counts mesh_events/120f added={} modified={} removed={} modified_by_vertices={top:?}",
        mesh_events.1[0], mesh_events.1[1], mesh_events.1[2]
    );
    *mesh_events.1 = [0; 3];
    let (cameras, windows) = views;
    // Batching upper bound: Bevy can only merge draws sharing a material.
    let visible_source = materials.iter().filter(|(v, _, _)| v.get());
    let (mut lit, mut lit_materials, mut all_materials) = (
        0,
        std::collections::HashSet::new(),
        std::collections::HashSet::new(),
    );
    let mut total = 0;
    for (_, material, dynamic) in visible_source {
        total += 1;
        all_materials.insert(material.0.id());
        if dynamic.is_some() {
            lit += 1;
            lit_materials.insert(material.0.id());
        }
    }
    println!(
        "Counts source_draws={total} distinct_materials={} dynamic_lit_draws={lit} dynamic_lit_materials={}",
        all_materials.len(),
        lit_materials.len()
    );
    println!(
        "Counts images={} image_mb={:.1}",
        images.len(),
        images
            .iter()
            .map(|(_, i)| i.data.as_ref().map_or(0, Vec::len))
            .sum::<usize>() as f64
            / 1_048_576.
    );
    println!(
        "Counts mesh_mb={:.1}",
        meshes
            .iter()
            .map(|(_, m)| {
                m.get_vertex_buffer_size()
                    + m.get_index_buffer_bytes().map_or(0, <[u8]>::len)
            })
            .sum::<usize>() as f64
            / 1_048_576.
    );
    for window in &windows {
        println!(
            "Counts window {}x{} scale={} mode={:?} present={:?}",
            window.physical_width(),
            window.physical_height(),
            window.scale_factor(),
            window.mode,
            window.present_mode
        );
    }
    for (camera, msaa) in &cameras {
        println!(
            "Counts camera order={} active={} msaa={:?} target={:?}",
            camera.order,
            camera.is_active,
            msaa,
            camera.physical_target_size()
        );
    }
    let visible = |v: &ViewVisibility| v.get();
    println!(
        "Counts t={:.1} lightmapped={} lightmap_images={} entities={} draws={} visible_draws={} skins={} visible_skins={} enemies_alive={} corpses={} meshes={} source_materials={} standard_materials={}",
        state.map_or(0., |s| s.elapsed),
        lightmaps.iter().count(),
        lightmaps
            .iter()
            .map(|l| l.image.id())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        entities.iter().count(),
        draws.iter().count(),
        draws.iter().filter(|v| visible(v)).count(),
        skins.iter().count(),
        skins.iter().filter(|v| visible(v)).count(),
        enemies.iter().filter(|e| e.health > 0.).count(),
        enemies.iter().filter(|e| e.health <= 0.).count(),
        meshes.len(),
        source.len(),
        standard.len(),
    );
}
/// Source-unit goal for the opt-in benchmark autopilot (`--profile-walk "x y z"`).
fn walk_goal() -> Option<Vec3> {
    static GOAL: std::sync::OnceLock<Option<Vec3>> = std::sync::OnceLock::new();
    *GOAL.get_or_init(|| {
        let args = std::env::args().collect::<Vec<_>>();
        let value = args.windows(2).find(|a| a[0] == "--profile-walk")?[1].clone();
        let v = value
            .split_whitespace()
            .map(str::parse::<f32>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        (v.len() == 3).then(|| Vec3::from_array(crate::binary::source_position([v[0], v[1], v[2]])))
    })
}
/// Opt-in benchmark autopilot. It follows the bots' NAV route to the goal and
/// shoots the nearest visible common infected, so frame profiles cover moving
/// through new areas and combat rather than a static camera. With
/// `--profile-panic` it also requests one Director panic event after 3 s.
/// Normal play never installs it.
pub fn install_autopilot(app: &mut App) {
    if walk_goal().is_some() {
        app.add_systems(FixedUpdate, autopilot.before(crate::game::move_player));
    }
}
#[allow(clippy::too_many_arguments)]
fn autopilot(
    time: Res<Time<Fixed>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut input: ResMut<crate::game::Input>,
    mut body: Single<&mut crate::game::Body>,
    mut state: ResMut<crate::game_state::Match>,
    collision: Res<crate::collision::Collision>,
    mut navigation: Option<ResMut<crate::l4d2r_navigation::Navigation>>,
    mut director: Option<ResMut<crate::source_director::Director>>,
    enemies: Query<(&Transform, &crate::game::Enemy)>,
    mut panic_sent: Local<bool>,
    mut last_target: Local<Option<Vec3>>,
) {
    let Some(goal) = walk_goal() else {
        return;
    };
    if state.paused {
        return;
    }
    let dt = time.delta_secs();
    if !*panic_sent && state.elapsed > 3. && std::env::args().any(|a| a == "--profile-panic") {
        if let Some(director) = director.as_mut() {
            director.command("panicevent", "");
            println!("Autopilot: requested Director panic event");
        }
        *panic_sent = true;
    }
    // Keep the benchmark firing without ammunition pickups along the route.
    let gun = state.gun_mut();
    gun.reserve = gun.reserve.max(500);
    let eye = body.feet + Vec3::Y * 1.6;
    let target = enemies
        .iter()
        .filter(|(_, e)| e.health > 0.)
        .map(|(t, _)| t.translation + Vec3::Y * 1.2)
        .filter(|p| p.distance(eye) < 15.)
        .min_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye)))
        .filter(|p| collision.trace(eye, *p, Vec3::ZERO).fraction >= 0.99);
    let route = navigation
        .as_mut()
        .map_or(goal - body.feet, |nav| nav.toward(97, body.feet, goal, dt));
    let arrived = (goal - body.feet).length() < 2.;
    let look = target.map_or(route, |p| p - eye);
    if look.length_squared() > 1e-6 {
        body.yaw = (-look.x).atan2(-look.z);
        body.pitch = if target.is_some() {
            (look.y / look.length()).clamp(-1., 1.).asin()
        } else {
            0.
        };
    }
    // Opt-in death-physics QA: keep the rendered corpse in view after firing.
    // Normal benchmark routes retain their existing camera and timing behavior.
    static WATCH_CORPSES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *WATCH_CORPSES.get_or_init(|| std::env::args().any(|a| a == "--profile-watch-corpses")) {
        if let Some(point) = target { *last_target = Some(point); }
        if let Some(point) = last_target.filter(|_| target.is_none()).map(|p| p - Vec3::Y * 0.95) {
            let look = point - eye;
            if look.length_squared() > 1e-6 {
                body.yaw = (-look.x).atan2(-look.z);
                body.pitch = (look.y / look.length()).clamp(-1., 1.).asin();
            }
        }
    }
    input.fire_held = target.is_some();
    if target.is_some() {
        input.fire = true;
    }
    // Stand and shoot when a target is visible; otherwise walk the route.
    if arrived || target.is_some() {
        keys.release(KeyCode::KeyW);
    } else {
        keys.press(KeyCode::KeyW);
    }
}
pub fn install_pipeline_status(app: &mut App) {
    use bevy::render::{Render, RenderApp, render_resource::PipelineCache};
    let status = PipelineStatus::default();
    app.insert_resource(status.clone());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.insert_resource(status).add_systems(
            Render,
            |cache: Res<PipelineCache>, status: Res<PipelineStatus>| {
                *status.0.lock().unwrap() =
                    (cache.pipelines().count(), cache.waiting_pipelines().count());
            },
        );
    }
}
static FRAME_CPU: std::sync::OnceLock<
    std::sync::Mutex<std::collections::BTreeMap<&'static str, f64>>,
> = std::sync::OnceLock::new();
pub fn record(
    profile: Option<ResMut<FrameProfile>>,
    mut previous: Local<Option<std::time::Instant>>,
    pipelines: Option<Res<PipelineStatus>>,
    diagnostics: Option<Res<bevy::diagnostic::DiagnosticsStore>>,
    state: Option<Res<crate::game_state::Match>>,
    warming: Option<Res<crate::render_warmup::Warmup>>,
    loading_frame: Option<Res<crate::render_warmup::LoadingFrame>>,
) {
    let Some(mut profile) = profile else {
        return;
    };
    flush_thread_metrics();
    let now = std::time::Instant::now();
    let delta = previous
        .replace(now)
        .map_or(0., |last| now.duration_since(last).as_secs_f64() * 1000.);
    profile.warmup += 1;
    PROFILE_FRAME.store(profile.warmup as u64, std::sync::atomic::Ordering::Relaxed);
    let loading = warming.is_some() || loading_frame.is_some_and(|f| f.0);
    let cpu = FRAME_CPU
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .clone();
    FRAME_CPU.get().unwrap().lock().unwrap().clear();
    let mut cpu = cpu.into_iter().collect::<Vec<_>>();
    cpu.sort_by(|a, b| b.1.total_cmp(&a.1));
    let cpu = cpu
        .iter()
        .map(|(n, v)| format!("{n}:{v:.3}"))
        .collect::<Vec<_>>()
        .join(";");
    let (total, waiting) = pipelines.as_ref().map_or((0, 0), |s| *s.0.lock().unwrap());
    let render = diagnostics.as_ref().map_or_else(String::new, |d| {
        d.iter()
            .filter(|d| d.path().as_str().starts_with("render/"))
            .filter_map(|d| d.value().map(|v| format!("{}:{v:.3}", d.path().as_str())))
            .collect::<Vec<_>>()
            .join(";")
    });
    if profile.detail.is_empty() {
        profile.detail =
            "frame,ms,game_time,pipelines,waiting,loading,cpu_current,cpu_previous,render\n".into();
    }
    let row = format!(
        "{},{delta:.3},{:.3},{total},{waiting},{},{cpu},{},{}\n",
        profile.warmup,
        state.as_ref().map_or(0., |s| s.elapsed),
        loading,
        profile.previous_cpu,
        render
    );
    profile.detail.push_str(&row);
    if delta > 35. {
        println!(
            "Slow frame {}: {delta:.2}ms pipelines={total} waiting={waiting} cpu=[{cpu}] previous=[{}]",
            profile.warmup, profile.previous_cpu
        );
    }
    profile.previous_cpu = cpu;
    if !loading && profile.warmup > 30 && profile.frames.len() < 36000 {
        profile.frames.push(delta);
    }
}
impl Drop for FrameProfile {
    fn drop(&mut self) {
        #[cfg(feature = "profile-trace")]
        write_trace();
        if self.frames.is_empty() {
            return;
        }
        let mut sorted = self.frames.clone();
        sorted.sort_by(f64::total_cmp);
        let percentile = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize];
        let report = format!(
            "frames={} p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms over50ms={}\n",
            sorted.len(),
            percentile(0.5),
            percentile(0.95),
            percentile(0.99),
            sorted[sorted.len() - 1],
            sorted.iter().filter(|v| **v > 50.).count()
        );
        println!("Frame profile: {report}");
        if let Some(cpu) = CPU.get() {
            let cpu = cpu.lock().unwrap();
            let mut text = String::from("system\tcalls\tmean_ms\tmax_ms\ttotal_ms\n");
            for (name, (calls, total, maximum)) in cpu.iter() {
                text += &format!(
                    "{name}\t{calls}\t{:.3}\t{maximum:.3}\t{total:.3}\n",
                    total / *calls as f64
                );
            }
            let _ = std::fs::write("local-research/cpu-profile.tsv", text);
        }
        let _ = std::fs::create_dir_all("local-research");
        let _ = std::fs::write("local-research/frame-profile.txt", report);
        let mut csv = String::from("frame,milliseconds\n");
        for (i, ms) in self.frames.iter().enumerate() {
            csv += &format!("{i},{ms:.3}\n");
        }
        let _ = std::fs::write("local-research/frame-profile.csv", csv);
        let _ = std::fs::write("local-research/frame-detail.csv", &self.detail);
    }
}

// Opt-in CPU timings; normal play only checks the cached flag.
static CPU: std::sync::OnceLock<
    std::sync::Mutex<std::collections::BTreeMap<&'static str, (u64, f64, f64)>>,
> = std::sync::OnceLock::new();
thread_local! {
    static THREAD_CPU: std::cell::RefCell<std::collections::BTreeMap<&'static str, (u64, f64, f64)>> = Default::default();
}
/// Publish worker timings once per batch/frame, rather than taking two shared
/// mutexes for every contact iteration and embedded-vertex recovery.
pub(crate) fn flush_thread_metrics() {
    let entries = THREAD_CPU.with(|local| {
        local
            .borrow_mut()
            .iter_mut()
            .filter_map(|(name, value)| {
                if value.0 == 0 {
                    None
                } else {
                    let entry = (*name, *value);
                    *value = (0, 0., 0.);
                    Some(entry)
                }
            })
            .collect::<Vec<_>>()
    });
    if entries.is_empty() {
        return;
    }
    let mut cpu = CPU.get_or_init(Default::default).lock().unwrap();
    let mut frame = FRAME_CPU.get_or_init(Default::default).lock().unwrap();
    for (name, value) in entries {
        let total = cpu.entry(name).or_default();
        total.0 += value.0;
        total.1 += value.1;
        total.2 = total.2.max(value.2);
        *frame.entry(name).or_default() += value.1;
    }
}
pub struct Scope {
    name: &'static str,
    start: Option<std::time::Instant>,
}
pub fn scope(name: &'static str) -> Scope {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    Scope {
        name,
        start: ENABLED
            .get_or_init(|| {
                std::env::args().any(|a| a == "--profile-frames")
                    && !std::env::args().any(|a| a == "--profile-frame-times-only")
            })
            .then(std::time::Instant::now),
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            let ms = start.elapsed().as_secs_f64() * 1000.;
            THREAD_CPU.with(|local| {
                let mut local = local.borrow_mut();
                let entry = local.entry(self.name).or_default();
                entry.0 += 1;
                entry.1 += ms;
                entry.2 = entry.2.max(ms);
            });
        }
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;
    #[test]
    fn viewmodel_projection_tracks_the_native_reference_fov() {
        for horizontal in [51f32, 90.] {
            let vertical = viewmodel_vertical_fov(horizontal);
            let recovered = 2. * ((vertical * 0.5).tan() * (4. / 3.)).atan();
            assert!((recovered.to_degrees() - horizontal).abs() < 0.0001);
        }
        let mut settings = DisplaySettings {
            show_fps: false,
            vsync: false,
            viewmodel_fov: 51.,
            camera_fov: 90.,
        };
        assert_eq!(
            settings.command("cl_viewmodelfovsurvivor").unwrap(),
            "cl_viewmodelfovsurvivor 51"
        );
        for value in ["NaN", "inf", "0", "180"] {
            settings.command(&format!("cl_viewmodelfovsurvivor {value}"));
            assert_eq!(settings.viewmodel_fov, 51.);
            settings.command(&format!("fov_desired {value}"));
            assert_eq!(settings.camera_fov, 90.);
        }
        assert_eq!(settings.command("FOV").unwrap(), "fov_desired 90");
        assert_eq!(
            settings.command("viewmodel_fov").unwrap(),
            "cl_viewmodelfovsurvivor 51"
        );
        settings.command("fov 100 120");
        assert_eq!(settings.camera_fov, 90.);
    }
    #[test]
    fn overlay_and_present_mode_follow_the_option_without_resetting_the_session() {
        let mut app = App::new();
        let mut clock = Time::<Real>::default();
        clock.advance_by(std::time::Duration::from_millis(20));
        app.insert_resource(clock)
            .insert_resource(DisplaySettings {
                show_fps: true,
                vsync: false,
                viewmodel_fov: 51.,
                camera_fov: 90.,
            })
            .init_resource::<FpsSample>()
            .add_systems(Update, display);
        let overlay = app
            .world_mut()
            .spawn((Text::new("FPS: --"), Node::default(), FpsOverlay))
            .id();
        let window = app.world_mut().spawn(Window::default()).id();
        let camera = app
            .world_mut()
            .spawn((
                crate::game::ViewCamera,
                Projection::Perspective(PerspectiveProjection::default()),
            ))
            .id();
        let world_camera = app
            .world_mut()
            .spawn((
                crate::viewer::GameCamera,
                Projection::Perspective(PerspectiveProjection::default()),
            ))
            .id();
        let option = app
            .world_mut()
            .spawn((DisplayLabel(false), Text::new("SHOW FPS: OFF")))
            .id();
        for _ in 0..15 {
            app.update();
        }
        assert!(app.world().get::<Text>(overlay).unwrap().0.contains("50"));
        assert_eq!(app.world().get::<Text>(option).unwrap().0, "SHOW FPS: ON");
        assert_eq!(
            app.world().get::<Window>(window).unwrap().present_mode,
            bevy::window::PresentMode::AutoNoVsync
        );
        let mut settings = app.world_mut().resource_mut::<DisplaySettings>();
        settings.show_fps = false;
        settings.vsync = true;
        settings.viewmodel_fov = 90.;
        settings.camera_fov = 110.;
        app.update();
        let Projection::Perspective(world_projection) =
            app.world().get::<Projection>(world_camera).unwrap()
        else {
            panic!("world projection");
        };
        assert!((world_projection.fov - viewmodel_vertical_fov(110.)).abs() < 1e-6);
        let Projection::Perspective(p) = app.world().get::<Projection>(camera).unwrap() else {
            panic!("viewmodel projection");
        };
        assert!((p.fov - viewmodel_vertical_fov(90.)).abs() < 1e-6);
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Window>(window).unwrap().present_mode,
            bevy::window::PresentMode::AutoVsync
        );
    }
}

/// After each map's render warmup, reports the CPU-side bytes Bevy still
/// holds for meshes and images (assets keep a main-world copy unless they
/// are RENDER_WORLD-only). Runs on iOS or with `--profile-mobile-memory`.
pub fn asset_memory_report(
    warmup: Option<Res<crate::render_warmup::Warmup>>,
    meshes: Res<Assets<Mesh>>,
    images: Res<Assets<Image>>,
    mut settle: Local<Option<u32>>,
    named: Query<(&Name, &Mesh3d)>,
) {
    if !(cfg!(target_os = "ios") || std::env::args().any(|a| a == "--profile-mobile-memory")) {
        return;
    }
    // Count frames from the end of warmup; static CPU copies are released
    // a couple of frames after it.
    if warmup.is_some() {
        *settle = Some(0);
        return;
    }
    let Some(frames) = settle.as_mut() else {
        return;
    };
    *frames += 1;
    if *frames < 10 {
        return;
    }
    *settle = None;
    let (mut mesh_count, mut mesh_bytes, mut kept, mut largest) = (0usize, 0usize, 0usize, Vec::new());
    let mut kinds: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for (id, mesh) in meshes.iter() {
        mesh_count += 1;
        let mut bytes = 0;
        if let Ok(attributes) = mesh.try_attributes() {
            bytes += attributes.map(|(_, v)| v.get_bytes().len()).sum::<usize>();
        }
        if let Ok(Some(indices)) = mesh.try_indices_option() {
            bytes += indices.len() * match indices {
                bevy::mesh::Indices::U16(_) => 2,
                bevy::mesh::Indices::U32(_) => 4,
            };
        }
        if bytes > 0 {
            kept += 1;
            let kind = if mesh.contains_attribute(Mesh::ATTRIBUTE_JOINT_INDEX) {
                "skinned"
            } else if mesh.contains_attribute(Mesh::ATTRIBUTE_UV_1) {
                "uv1"
            } else if mesh.contains_attribute(Mesh::ATTRIBUTE_COLOR) {
                "color"
            } else {
                "plain"
            };
            let kind = format!("{kind} {:?}", mesh.asset_usage);
            let e = kinds.entry(kind).or_insert((0usize, 0usize));
            e.0 += 1;
            e.1 += bytes;
        }
        mesh_bytes += bytes;
        largest.push((bytes, id));
    }
    largest.sort_by(|a, b| b.0.cmp(&a.0));
    if std::env::args().any(|a| a == "--profile-mobile-memory") {
        let mut users: std::collections::HashMap<bevy::asset::AssetId<Mesh>, (usize, String)> = Default::default();
        for (name, mesh) in &named {
            let e = users.entry(mesh.0.id()).or_insert((0, name.to_string()));
            e.0 += 1;
        }
        let mut by_user: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
        for (bytes, id) in &largest {
            if *bytes == 0 { continue; }
            let label = users.get(id).map_or("(no entity)".to_string(), |(_, n)| n.split(" / ").next().unwrap_or(n).to_string());
            let e = by_user.entry(label).or_default();
            e.0 += 1;
            e.1 += bytes;
        }
        let mut v: Vec<_> = by_user.into_iter().collect();
        v.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        for (label, (n, b)) in v.iter().take(12) {
            println!("Asset mesh owner: {label}: {n} meshes, {:.1} MiB", *b as f64 / 1048576.);
        }
    }
    let (mut image_count, mut image_bytes, mut image_kept) = (0usize, 0usize, 0usize);
    let mut big = Vec::new();
    let mut by_shape: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for (_, image) in images.iter() {
        image_count += 1;
        if let Some(data) = &image.data {
            image_kept += 1;
            image_bytes += data.len();
            let d = image.texture_descriptor.size;
            let shape = format!("{}x{}x{} {:?} mips{} {:?}", d.width, d.height, d.depth_or_array_layers, image.texture_descriptor.format, image.texture_descriptor.mip_level_count, image.asset_usage);
            let e = by_shape.entry(shape.clone()).or_default();
            e.0 += 1;
            e.1 += data.len();
            big.push((data.len(), shape));
        }
    }
    let mut shapes: Vec<_> = by_shape.into_iter().collect();
    shapes.sort_by(|a, b| b.1.1.cmp(&a.1.1));
    for (shape, (n, bytes)) in shapes.iter().take(15) {
        println!("Asset image group: {n} x {shape} = {:.1} MiB", *bytes as f64 / 1048576.);
    }
    let mib = |b: usize| b as f64 / 1048576.;
    let line = format!(
        "Asset memory: meshes {mesh_count} ({kept} with CPU data, {:.1} MiB); images {image_count} ({image_kept} with CPU data, {:.1} MiB); largest meshes {:?} MiB",
        mib(mesh_bytes),
        mib(image_bytes),
        largest.iter().take(5).map(|(b, _)| (mib(*b) * 10.).round() / 10.).collect::<Vec<_>>()
    );
    for (kind, (n, b)) in &kinds {
        println!("Asset mesh group: {kind}: {n} meshes, {:.1} MiB", *b as f64 / 1048576.);
    }
    println!("{line}");
    memory_checkpoint("Asset memory report");
    gpu_audit("asset report");
}
/// `L4D2R_GPU_AUDIT=1`: which material types still hold each loaded image
/// (a released map's textures must not stay referenced).
pub(crate) fn image_audit(world: &World, stage: &str) {
    use std::collections::HashMap;
    if std::env::var_os("L4D2R_GPU_AUDIT").is_none() {
        return;
    }
    let mut owner: HashMap<AssetId<Image>, &'static str> = HashMap::new();
    let mut mark = |h: Option<&Handle<Image>>, name: &'static str| {
        if let Some(h) = h {
            owner.entry(h.id()).or_insert(name);
        }
    };
    let base = |m: &StandardMaterial| [m.base_color_texture.clone(), m.emissive_texture.clone(), m.normal_map_texture.clone()];
    if let Some(a) = world.get_resource::<Assets<crate::viewer::SourceMaterial>>() {
        for (_, m) in a.iter() {
            for h in base(&m.base).iter() {
                mark(h.as_ref(), "source");
            }
        }
    }
    if let Some(a) = world.get_resource::<Assets<crate::particle_pull::ParticleMaterial>>() {
        for (_, m) in a.iter() {
            mark(Some(&m.extension.texture), "particle");
            for h in base(&m.base).iter() { mark(h.as_ref(), "particle"); }
        }
    }
    if let Some(a) = world.get_resource::<Assets<crate::particle_sprite::SpriteMaterial>>() {
        for (_, m) in a.iter() { mark(Some(&m.texture), "sprite"); }
    }
    if let Some(a) = world.get_resource::<Assets<crate::source_water::WaterMaterial>>() {
        for (_, m) in a.iter() {
            for h in [&m.extension.normal, &m.extension.flow, &m.extension.noise, &m.extension.sky] { mark(h.as_ref(), "water"); }
        }
    }
    if let Some(a) = world.get_resource::<Assets<StandardMaterial>>() {
        for (_, m) in a.iter() { for h in base(m).iter() { mark(h.as_ref(), "standard"); } }
    }
    let mut groups: HashMap<(&str, u32, u32), usize> = HashMap::new();
    let mut detail: HashMap<String, usize> = HashMap::new();
    if let Some(images) = world.get_resource::<Assets<Image>>() {
        for (id, image) in images.iter() {
            let s = image.texture_descriptor.size;
            let who = owner.get(&id).copied().unwrap_or("other");
            *groups.entry((who, s.width, s.height)).or_default() += 1;
            if who == "other" && s.width == 512 && s.height == 512 {
                let d = &image.texture_descriptor;
                *detail
                    .entry(format!(
                        "{:?} mips{} layers{} cpu={} usage={:?} id={}",
                        d.format,
                        d.mip_level_count,
                        s.depth_or_array_layers,
                        image.data.is_some(),
                        image.asset_usage,
                        if matches!(id, AssetId::Uuid { .. }) { "uuid" } else { "index" }
                    ))
                    .or_default() += 1;
            }
        }
    }
    for (k, n) in &detail {
        eprintln!("IMAGE AUDIT {stage}: other 512x512 x{n}: {k}");
    }
    let mut rows: Vec<_> = groups.into_iter().filter(|(_, n)| *n >= 3).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    for ((owner, w, h), n) in rows.iter().take(14) {
        eprintln!("IMAGE AUDIT {stage}: {n:4} x {w}x{h} held by {owner}");
    }
}
