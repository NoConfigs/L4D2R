//! Native iPhone first-run installer bridge. No second Bevy event loop.
use l4d2r_steam_installer::Event;
use std::{
    ffi::{CStr, CString, c_char},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
static GUARD: Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>> = Mutex::new(None);
unsafe extern "C" {
    fn l4d2r_installer_run();
    fn l4d2r_game_files_show(pack: i32);
    fn l4d2r_installer_event(kind: i32, text: *const c_char, done: u64, total: u64);
    fn l4d2r_login_read() -> *mut c_char;
    fn l4d2r_login_save(value: *const c_char) -> bool;
    fn l4d2r_login_delete();
    fn l4d2r_secret_free(value: *mut c_char);
}
pub fn show() {
    unsafe { l4d2r_installer_run() }
}
pub fn show_files(pack: u8) {
    unsafe { l4d2r_game_files_show(pack.into()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn l4d2r_pack_complete(pack: i32) -> bool {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .is_some_and(|home| {
            home.join(format!(
                "Documents/Left 4 Dead 2/.l4d2r-pack-{}.json",
                pack.clamp(0, 16)
            ))
            .is_file()
        })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn l4d2r_install_guard(code: *const c_char) {
    if code.is_null() {
        return;
    }
    let code = unsafe { CStr::from_ptr(code) }
        .to_string_lossy()
        .into_owned();
    if let Some(sender) = GUARD.lock().unwrap().as_ref() {
        let _ = sender.send(code);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn l4d2r_install_login(
    pack: i32,
    account: *const c_char,
    password: *const c_char,
) {
    if account.is_null() || password.is_null() {
        return;
    }
    let (sender, codes) = tokio::sync::mpsc::unbounded_channel();
    *GUARD.lock().unwrap() = Some(sender);
    let credentials = l4d2r_steam_installer::Credentials {
        account: unsafe { CStr::from_ptr(account) }
            .to_string_lossy()
            .into_owned(),
        password: unsafe { CStr::from_ptr(password) }
            .to_string_lossy()
            .into_owned(),
        codes,
    };
    start(pack, Some(credentials));
}
fn send(kind: i32, text: &str, done: u64, total: u64) {
    if kind == 2 && done < total {
        static LAST: std::sync::OnceLock<Mutex<std::time::Instant>> = std::sync::OnceLock::new();
        let mut last = LAST
            .get_or_init(|| {
                Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1))
            })
            .lock()
            .unwrap();
        if last.elapsed() < std::time::Duration::from_millis(100) {
            return;
        }
        *last = std::time::Instant::now();
    }
    let text = CString::new(text.replace('\0', " ")).unwrap();
    unsafe { l4d2r_installer_event(kind, text.as_ptr(), done, total) }
}
#[unsafe(no_mangle)]
pub extern "C" fn l4d2r_install_cancel() {
    if let Some(cancel) = CANCEL.lock().unwrap().as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn l4d2r_install_forget() {
    if !RUNNING.load(Ordering::Acquire) {
        unsafe { l4d2r_login_delete() }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn l4d2r_install_start(pack: i32) {
    start(pack, None);
}
fn start(pack: i32, credentials: Option<l4d2r_steam_installer::Credentials>) {
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *CANCEL.lock().unwrap() = Some(cancel.clone());
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            let saved = unsafe {
                let ptr = l4d2r_login_read();
                if ptr.is_null() {
                    None
                } else {
                    let text = CStr::from_ptr(ptr).to_string_lossy().into_owned();
                    l4d2r_secret_free(ptr);
                    Some(text)
                }
            };
            let root = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .ok_or("The app storage folder is unavailable".to_owned())?
                .join("Documents/Left 4 Dead 2");
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|_| "Could not start the installer".to_owned())?;
            let pack = pack.clamp(0, 16) as u8;
            runtime.block_on(l4d2r_steam_installer::install_with_options(root, saved, cancel,
                l4d2r_steam_installer::Options {pack, credentials, qr:false}, |event| match event {
                Event::Status(s) => send(0, &s, 0, 0),
                Event::Qr(s) => send(1, &s, 0, 0),
                Event::GuardCode => send(6, "Approve the sign-in in Steam Guard, or enter your Steam Guard / email code.", 0, 0),
                Event::NeedCredentials => send(7, "Sign in with your Steam account to continue.", 0, 0),
                Event::SaveLogin(s) => {
                    let text = CString::new(s).unwrap();
                    if !unsafe { l4d2r_login_save(text.as_ptr()) } {
                        send(0, "Steam sign-in could not be saved. You may need to sign in again after reopening.", 0, 0);
                    }
                }
                Event::ForgetLogin => unsafe { l4d2r_login_delete() },
                Event::Progress { done, total, file } => send(2, &file, done, total),
                Event::Complete => send(3, "Download complete. These game files are ready.", 0, 0),
            }))
        });
        RUNNING.store(false, Ordering::Release);
        match result {
            Ok(Ok(())) => (),
            Ok(Err(message)) => send(4, &message, 0, 0),
            Err(_) => send(
                4,
                "The installer stopped unexpectedly. Tap Resume to try again.",
                0,
                0,
            ),
        }
    });
}
