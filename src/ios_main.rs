//! iOS library root (ios/Cargo.toml, a staticlib linked by ios/build-ipa.sh).
//!
//! The whole desktop program is compiled in unchanged; ios/main.m calls
//! `main_rs`. The app's Documents folder is shared with the Files app and
//! Finder/iTunes file sharing (Info.plist UIFileSharingEnabled), and the game
//! reads its Left 4 Dead 2 data from `Documents/Left 4 Dead 2`. The shaders
//! and the laboratory map ship inside the app bundle.
#![allow(dead_code, unused_imports)]
include!("main.rs");
mod ios_installer;

// Bevy's iOS logger writes to Apple's unified log, not stderr. Without
// this additional layer, the Files log omits GPU/shader validation errors.
fn ios_file_log(_: &mut bevy::prelude::App) -> Option<bevy::log::BoxedLayer> {
    Some(Box::new(
        bevy::log::tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .without_time()
            .with_writer(std::io::stderr),
    ))
}

fn ios_frame_status(
    windows: bevy::prelude::Query<&bevy::prelude::Window>,
    cameras: bevy::prelude::Query<(
        bevy::prelude::Entity,
        &bevy::prelude::Camera,
        bevy::prelude::Has<bevy::camera::Hdr>,
        Option<&bevy::prelude::Name>,
    )>,
    ui: bevy::prelude::Query<&bevy::prelude::UiTargetCamera>,
    mut frame: bevy::prelude::Local<u32>,
) {
    *frame += 1;
    if !matches!(*frame, 1 | 60 | 300) {
        return;
    }
    for window in &windows {
        eprintln!(
            "iOS frame {}: window={}x{} scale={} visible={}",
            *frame,
            window.physical_width(),
            window.physical_height(),
            window.scale_factor(),
            window.visible
        );
    }
    for (entity, camera, hdr, name) in &cameras {
        eprintln!(
            "iOS camera {entity:?} {name:?}: active={} HDR={hdr} viewport={:?} UI-targets={}",
            camera.is_active,
            camera.physical_viewport_size(),
            ui.iter().filter(|target| target.0 == entity).count()
        );
    }
}

/// What to copy, shown in the Files app until the data is there.
#[cfg(target_os = "ios")]
const HOW_TO: &str = "Copy these folders from your PC's Left 4 Dead 2 install into this \
\"Left 4 Dead 2\" folder, then start the app again:\n\
  update\n  left4dead2_dlc3\n  left4dead2_dlc2\n  left4dead2_dlc1\n  left4dead2\n  hl2\n";

#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn main_rs() {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let documents = home.join("Documents");
    let game = documents.join("Left 4 Dead 2");
    let _ = std::fs::create_dir_all(&game);
    // Relative paths (options files, local-research/) live in Documents.
    let _ = std::env::set_current_dir(&documents);
    log_to_documents(&documents);
    eprintln!("L4D2 Rewrite starting; game data: {}", game.display());
    if l4d2r_steam_installer::needs_install(&game) {
        ios_installer::show();
        return;
    }
    if !game.join("left4dead2").is_dir() {
        let _ = std::fs::write(game.join("COPY YOUR GAME FILES HERE.txt"), HOW_TO);
        eprintln!(
            "No game data: Documents/Left 4 Dead 2/left4dead2 is missing.
{HOW_TO}"
        );
        startup_error(&format!(
            "L4D2 Rewrite — game files needed\n\n{HOW_TO}\nIn Files, open On My iPhone → L4D2 Rewrite → Left 4 Dead 2. Copy the folders themselves, not another enclosing Left 4 Dead 2 folder.\n\nClose and reopen the app after copying. Startup details are saved in l4d2r-log.txt next to the game folder."
        ));
        return;
    }
    let map = test_lab::path();
    if !map.is_file() {
        startup_error(
            "L4D2 Rewrite — installation incomplete\n\nThe bundled test map could not be unpacked. Reinstall the latest IPA. Your game files belong in Documents/Left 4 Dead 2.\n\nSee l4d2r-log.txt in Files for startup details.",
        );
        return;
    }
    eprintln!("Loading map: {}", map.display());
    match game::run(&game, &map, None, false) {
        Ok(()) => eprintln!("Game exited."),
        Err(e) => {
            eprintln!("Game initialization failed: {e}");
            startup_error(&format!(
                "L4D2 Rewrite — unable to load\n\n{e}\n\nCheck that all game folders were copied completely, including the VPK files.\n\nOpen l4d2r-log.txt in Files → On My iPhone → L4D2 Rewrite for the startup log."
            ));
        }
    }
}

#[cfg(target_os = "ios")]
fn startup_error(message: &str) {
    unsafe extern "C" {
        fn l4d2r_startup_error(message: *const std::ffi::c_char);
    }
    eprintln!("{message}");
    let message = std::ffi::CString::new(message.replace('\0', " ")).unwrap();
    unsafe { l4d2r_startup_error(message.as_ptr()) };
}

/// stdout/stderr and panics go to Documents/l4d2r-log.txt (readable in the
/// Files app), since a sideloaded app has no visible console.
#[cfg(target_os = "ios")]
fn log_to_documents(documents: &std::path::Path) {
    use std::os::fd::IntoRawFd;
    unsafe extern "C" {
        fn dup2(from: i32, to: i32) -> i32;
    }
    // Keep the last session's log: after iOS closes the app, the next launch
    // would otherwise overwrite the only record of what happened.
    let _ = std::fs::rename(documents.join("l4d2r-log.txt"), documents.join("l4d2r-log-previous.txt"));
    if let Ok(file) = std::fs::File::create(documents.join("l4d2r-log.txt")) {
        let fd = file.into_raw_fd();
        unsafe {
            dup2(fd, 1);
            dup2(fd, 2);
        }
    }
    std::panic::set_hook(Box::new(|info| {
        eprintln!(
            "PANIC: {info}
{}",
            std::backtrace::Backtrace::force_capture()
        );
    }));
}
