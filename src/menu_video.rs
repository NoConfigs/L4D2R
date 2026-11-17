//! Stream the installed menu background video into Bevy.
//!
//! Retail picks one of five backgrounds per launch (engine FUN_100799c0):
//! `-background N` (clamped 1..5), else Plat_GetClockStart() % 5 + 1, and
//! plays media/l4d2_background%02d.bik (FUN_10079a90). Desktop decodes the
//! Bink with ffmpeg. iOS decodes the same .bik in-process with a Bink-only
//! FFmpeg (ios/bink.c); an H.264 copy (l4d2_background%02d.mp4, from
//! tools/ios-copy-game.py) is played with AVFoundation if the .bik is absent.
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::{
    io::{self, Read},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};
#[derive(Resource)]
pub struct Video {
    pub image: Handle<Image>,
    frames: Mutex<Receiver<Vec<u8>>>,
    process: Mutex<Option<Child>>,
    active: Arc<AtomicBool>,
}
impl Drop for Video {
    fn drop(&mut self) {
        self.active.store(true, Ordering::Relaxed);
        if let Ok(Some(child)) = self.process.get_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
/// This launch's background, 1..5 (engine FUN_100799c0).
pub fn background_index() -> u32 {
    static INDEX: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *INDEX.get_or_init(|| {
        let args: Vec<String> = std::env::args().collect();
        let forced = args
            .windows(2)
            .find(|p| p[0] == "-background" || p[0] == "--background")
            .and_then(|p| p[1].parse::<i32>().ok())
            .filter(|n| *n > 0);
        match forced {
            Some(n) => n.clamp(1, 5) as u32,
            None => {
                let clock = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                (clock % 5) as u32 + 1
            }
        }
    })
}
/// The matching still, console/background%02d%s (engine FUN_10079a30):
/// "_widescreen" unless width / height is below 1.5999.
pub fn background_still(width: f32, height: f32) -> String {
    let wide = height <= 0. || width / height >= 1.5999;
    format!(
        "materials/console/background{:02}{}.vtf",
        background_index(),
        if wide { "_widescreen" } else { "" }
    )
}
const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
pub fn start(root: &Path, images: &mut Assets<Image>) -> io::Result<Video> {
    let name = format!("left4dead2/media/l4d2_background{:02}", background_index());
    if cfg!(target_os = "ios") {
        let bink = root.join(format!("{name}.bik"));
        let path = if bink.is_file() { bink } else { root.join(format!("{name}.mp4")) };
        return start_native(&path, images);
    }
    let executable = Path::new(env!("CARGO_MANIFEST_DIR")).join(".tools/ffmpeg/ffmpeg.exe");
    let native = root.join(format!("{name}.bik"));
    // Clean source packages omit the local research tools. Use a user's
    // installed decoder when the workspace copy is unavailable.
    let mut command = Command::new(if executable.is_file() {
        executable
    } else {
        "ffmpeg".into()
    });
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-threads",
            "1",
            "-stream_loop",
            "-1",
            "-re",
            "-i",
        ])
        .arg(&native)
        .args([
            "-an",
            "-vf",
            "scale=1280:720",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut process = command.spawn()?;
    let mut stream = process.stdout.take().expect("Video pipe");
    println!(
        "Native menu video: {} (bounded live RGBA stream)",
        native.display()
    );
    Ok(spawn_worker(images, Some(process), move |pixels| stream.read_exact(pixels).is_ok()))
}
/// Runs `next` (fill one RGBA frame; false ends the stream) on a worker
/// thread while the menu is visible and hands frames to `update`.
fn spawn_worker(
    images: &mut Assets<Image>,
    process: Option<Child>,
    mut next: impl FnMut(&mut [u8]) -> bool + Send + 'static,
) -> Video {
    let (sender, frames) = mpsc::sync_channel(2);
    let active = Arc::new(AtomicBool::new(true));
    let worker_active = active.clone();
    std::thread::spawn(move || {
        loop {
            while !worker_active.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let mut pixels = vec![0; WIDTH * HEIGHT * 4];
            if !next(&mut pixels) {
                break;
            }
            match sender.try_send(pixels) {
                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => break,
            }
        }
    });
    let image = images.add(Image::new(
        Extent3d {
            width: 1280,
            height: 720,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0; 1280 * 720 * 4],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    ));
    Video {
        image,
        frames: Mutex::new(frames),
        process: Mutex::new(process),
        active,
    }
}
#[cfg(target_os = "ios")]
unsafe extern "C" {
    fn l4d2r_video_open(path: *const std::ffi::c_char, width: i32, height: i32) -> *mut std::ffi::c_void;
    fn l4d2r_video_frame(video: *mut std::ffi::c_void, rgba: *mut u8) -> i32;
    fn l4d2r_video_close(video: *mut std::ffi::c_void);
    fn l4d2r_bink_open(path: *const std::ffi::c_char, width: i32, height: i32) -> *mut std::ffi::c_void;
    fn l4d2r_bink_frame(video: *mut std::ffi::c_void, rgba: *mut u8) -> i32;
    fn l4d2r_bink_close(video: *mut std::ffi::c_void);
}
/// iOS: AVFoundation decodes the H.264 copy (ios/main.m), looping at the
/// video's 24 fps like the retail Bink.
#[cfg(target_os = "ios")]
fn start_native(path: &Path, images: &mut Assets<Image>) -> io::Result<Video> {
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} (download it with GAME FILES)", path.display()),
        ));
    }
    let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes()).map_err(io::Error::other)?;
    struct Reader(*mut std::ffi::c_void);
    unsafe impl Send for Reader {}
    // Retail .bik through the Bink-only FFmpeg; .mp4 through AVFoundation.
    type Open = unsafe extern "C" fn(*const std::ffi::c_char, i32, i32) -> *mut std::ffi::c_void;
    type Frame = unsafe extern "C" fn(*mut std::ffi::c_void, *mut u8) -> i32;
    type Close = unsafe extern "C" fn(*mut std::ffi::c_void);
    let bink = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bik"));
    let (open_fn, frame_fn, close_fn, decoder): (Open, Frame, Close, &str) = if bink {
        (l4d2r_bink_open, l4d2r_bink_frame, l4d2r_bink_close, "Bink")
    } else {
        (l4d2r_video_open, l4d2r_video_frame, l4d2r_video_close, "AVFoundation")
    };
    let open = move |p: &std::ffi::CString| Reader(unsafe { open_fn(p.as_ptr(), WIDTH as i32, HEIGHT as i32) });
    let mut reader = open(&c_path);
    if reader.0.is_null() {
        return Err(io::Error::other(format!("{decoder} could not open {}", path.display())));
    }
    println!("Native menu video: {} ({decoder})", path.display());
    let frame_time = std::time::Duration::from_secs_f64(1. / 24.);
    let mut next_due = std::time::Instant::now();
    Ok(spawn_worker(images, None, move |pixels| {
        let now = std::time::Instant::now();
        if next_due > now {
            std::thread::sleep(next_due - now);
        }
        next_due = next_due.max(now) + frame_time;
        for _ in 0..2 {
            if unsafe { frame_fn(reader.0, pixels.as_mut_ptr()) } != 0 {
                return true;
            }
            // End of the clip: reopen to loop, as -stream_loop does.
            unsafe { close_fn(reader.0) };
            reader = open(&c_path);
            if reader.0.is_null() {
                return false;
            }
        }
        false
    }))
}
#[cfg(not(target_os = "ios"))]
fn start_native(_: &Path, _: &mut Assets<Image>) -> io::Result<Video> {
    unreachable!("AVFoundation playback is iOS only")
}
pub fn update(
    mut commands: Commands,
    source: Res<crate::game_ui::HudSource>,
    video: Option<Res<Video>>,
    frontend: Res<crate::game_menu::Frontend>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(video) = video else {
        // One attempt per launch: a missing decoder or video keeps the
        // native still instead of retrying (and logging) every frame.
        static FAILED: AtomicBool = AtomicBool::new(false);
        if !FAILED.load(Ordering::Relaxed)
            && matches!(
                frontend.page,
                crate::game_menu::Page::Home | crate::game_menu::Page::Campaign
            )
        {
            match start(&source.0, &mut images) {
                Ok(video) => {
                    commands.insert_resource(video);
                }
                Err(e) => {
                    FAILED.store(true, Ordering::Relaxed);
                    eprintln!("Menu video startup: {e}; keeping the native still");
                }
            }
        }
        return;
    };
    let visible = !frontend.started
        || matches!(
            frontend.page,
            crate::game_menu::Page::Home | crate::game_menu::Page::Campaign
        );
    video.active.store(visible, Ordering::Relaxed);
    if !visible {
        return;
    }
    let Ok(receiver) = video.frames.lock() else {
        return;
    };
    let mut latest = None;
    while let Ok(frame) = receiver.try_recv() {
        latest = Some(frame);
    }
    if let Some(frame) = latest {
        if let Some(mut image) = images.get_mut(&video.image) {
            image.data = Some(frame);
        }
    }
}
