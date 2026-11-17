//! Android library root (android/Cargo.toml, built by tools/build-apk.ps1).
//!
//! The whole desktop program is compiled in unchanged; `android_main` is the
//! NativeActivity entry. On first launch the game data bundled in the APK
//! (`apk-assets/l4d2r-data`, listed in its `manifest.txt`) is copied to the
//! app's storage, because the game reads its files with std::fs. Then the
//! game starts with that copy as its Left 4 Dead 2 folder, on the main menu.
#![allow(dead_code, unused_imports)]
include!("main.rs");

#[cfg(target_os = "android")]
mod android {
    use bevy::android::android_activity::AndroidApp;
    use std::{
        ffi::CString,
        io::{Read, Write},
        path::{Path, PathBuf},
    };

    /// Bundled-data folder inside the APK's assets.
    const DATA: &str = "l4d2r-data";

    fn log(message: &str) {
        // NativeActivity has no stdout; Bevy's log goes to logcat.
        bevy::log::info!("{message}");
    }

    /// Copy every file listed in `l4d2r-data/manifest.txt` out of the APK
    /// into `root`, unless a previous launch finished the same manifest.
    pub fn extract(app: &AndroidApp, root: &Path) -> std::io::Result<()> {
        let assets = app.asset_manager();
        let manifest_name = CString::new(format!("{DATA}/manifest.txt")).unwrap();
        let Some(mut manifest) = assets.open(&manifest_name) else {
            log("No bundled game data in this APK");
            return Ok(());
        };
        let mut listing = String::new();
        manifest.read_to_string(&mut listing)?;
        let done = root.join(".extracted");
        if std::fs::read_to_string(&done).is_ok_and(|d| d == listing) {
            return Ok(());
        }
        let files: Vec<&str> = listing.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut buffer = vec![0; 4 << 20];
        for (i, name) in files.iter().enumerate() {
            let target = root.join(name);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let path = CString::new(format!("{DATA}/{name}")).unwrap();
            let Some(mut asset) = assets.open(&path) else {
                log(&format!("Bundled file missing from the APK: {name}"));
                continue;
            };
            let length = asset.length() as u64;
            if std::fs::metadata(&target).is_ok_and(|m| m.len() == length) {
                continue;
            }
            let partial = target.with_extension("partial");
            let mut out = std::io::BufWriter::new(std::fs::File::create(&partial)?);
            loop {
                let n = asset.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buffer[..n])?;
            }
            out.flush()?;
            drop(out);
            std::fs::rename(&partial, &target)?;
            log(&format!("Unpacked game data {}/{}: {name}", i + 1, files.len()));
        }
        std::fs::write(done, listing)?;
        Ok(())
    }

    pub fn storage(app: &AndroidApp) -> PathBuf {
        // App-specific external storage has room for the game data; internal
        // storage is the fallback.
        app.external_data_path()
            .or_else(|| app.internal_data_path())
            .unwrap_or_else(|| PathBuf::from("/data/local/tmp/l4d2r"))
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: bevy::android::android_activity::AndroidApp) {
    let _ = bevy::android::ANDROID_APP.set(app.clone());
    let root = android::storage(&app);
    let _ = std::fs::create_dir_all(&root);
    // Relative paths (maps/, local-research/, options files) live here.
    let _ = std::env::set_current_dir(&root);
    // Unpack on a worker while this thread keeps answering the activity, so
    // a long first-launch copy does not trip "app not responding".
    let worker = {
        let app = app.clone();
        let root = root.clone();
        std::thread::spawn(move || android::extract(&app, &root))
    };
    while !worker.is_finished() {
        app.poll_events(Some(std::time::Duration::from_millis(100)), |_| {});
    }
    if let Ok(Err(e)) = worker.join() {
        bevy::log::error!("Unpacking the game data failed: {e}");
    }
    let game = root.join("Left 4 Dead 2");
    if let Err(e) = game::run(&game, &test_lab::path(), None, false) {
        bevy::log::error!("Game initialization failed: {e}");
    }
}
