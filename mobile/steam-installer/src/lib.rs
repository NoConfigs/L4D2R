//! First-party Steam content access, using the user's own Steam authorization.
//! No account tokens or Valve assets are included in the application bundle.
use sha1::{Digest, Sha1};
pub mod packs;
mod receipts;
pub struct Credentials {
    pub account: String,
    pub password: String,
    pub codes: tokio::sync::mpsc::UnboundedReceiver<String>,
}
pub struct Options {
    pub pack: u8,
    pub credentials: Option<Credentials>,
    pub qr: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            pack: 0,
            credentials: None,
            qr: true,
        }
    }
}
use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use steam_cdn::CDNClient;
use steam_vent::{Connection, ServerList, auth::SavedLogin};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub enum Event {
    Status(String),
    Qr(String),
    GuardCode,
    NeedCredentials,
    /// Private credential payload. Store in Keychain; never display or log.
    SaveLogin(String),
    ForgetLogin,
    Progress {
        done: u64,
        total: u64,
        file: String,
    },
    Complete,
}
type Result<T> = std::result::Result<T, String>;
const APP: u32 = 550;
const DEPOTS: [u32; 2] = [551, 552];
const INCOMPLETE: &str = ".l4d2r-install-incomplete";

pub fn needs_install(root: &Path) -> bool {
    (root.join(INCOMPLETE).exists() && !root.join(".l4d2r-pack-0.json").is_file())
        || !root.join("left4dead2/gameinfo.txt").is_file()
        || !root.join("left4dead2/pak01_dir.vpk").is_file()
}

/// Restrict Steam manifest paths to data consumed by the rewrite.
pub fn asset_path(name: &str) -> Option<PathBuf> {
    let name = name.replace('\\', "/");
    if name.is_empty()
        || name.contains(':')
        || name.contains('\0')
        || name.starts_with('/')
        || name
            .split('/')
            .any(|p| p == ".." || p == "." || p.is_empty())
    {
        return None;
    }
    let path = PathBuf::from(&name);
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let mut parts = name.split('/');
    if !matches!(
        parts.next()?,
        "update" | "left4dead2_dlc3" | "left4dead2_dlc2" | "left4dead2_dlc1" | "left4dead2" | "hl2"
    ) {
        return None;
    }
    if parts.clone().any(|s| s.eq_ignore_ascii_case("bin")) {
        return None;
    }
    let ext = path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(
        ext.as_str(),
        "exe" | "dll" | "so" | "dylib" | "pdb" | "bat" | "cmd" | "sh"
    ) {
        return None;
    }
    Some(path)
}

fn target(root: &Path, relative: &Path) -> Result<PathBuf> {
    let mut at = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err("Unsafe asset path".into());
        };
        at.push(part);
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(
                "An asset path contains a symbolic link. Remove the link before installing.".into(),
            );
        }
    }
    Ok(at)
}

async fn valid_file(path: &Path, size: u64, hash: &[u8]) -> bool {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return false;
    };
    if file.metadata().await.map_or(true, |m| m.len() != size) {
        return false;
    }
    // Steam uses an all-zero hash for intentional empty depot placeholders,
    // including update/pak01_005.vpk. It is not SHA-1(empty).
    if size == 0 && hash.iter().all(|b| *b == 0) {
        return true;
    }
    let mut digest = Sha1::new();
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        match file.read(&mut buf).await {
            Ok(0) => return digest.finalize().as_slice() == hash,
            Ok(n) => digest.update(&buf[..n]),
            Err(_) => return false,
        }
    }
}

async fn bounded<T, E>(
    future: impl std::future::Future<Output = std::result::Result<T, E>>,
    seconds: u64,
    message: &str,
) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(seconds), future)
        .await
        .map_err(|_| format!("{message} (timed out). Check your connection and retry."))?
        .map_err(|_| format!("{message}. Check your connection and retry."))
}

/// Cancellation drops in-flight network work. Completed chunks remain resumable.
pub async fn install(
    root: PathBuf,
    saved: Option<String>,
    cancel: Arc<AtomicBool>,
    event: impl FnMut(Event) + Send,
) -> Result<()> {
    install_with_options(root, saved, cancel, Options::default(), event).await
}
pub async fn install_with_options(
    root: PathBuf,
    saved: Option<String>,
    cancel: Arc<AtomicBool>,
    mut options: Options,
    mut event: impl FnMut(Event) + Send,
) -> Result<()> {
    options.pack = options.pack.min(16);
    if cancel.load(Ordering::Relaxed) {
        return Err("Paused. Tap Resume to continue.".into());
    }
    tokio::select! {
        result = install_inner(&root, saved, options, &mut event) => result,
        _ = async { while !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }} => Err("Paused. Tap Resume to continue; completed files and chunks are kept.".into())
    }
}

async fn install_inner(
    root: &Path,
    saved: Option<String>,
    mut options: Options,
    event: &mut (impl FnMut(Event) + Send),
) -> Result<()> {
    let bootstrap = needs_install(root);
    std::fs::create_dir_all(root).map_err(|_| "Cannot create the game data folder")?;
    let root = std::fs::canonicalize(root).map_err(|_| "Cannot access the game data folder")?;
    if bootstrap {
        std::fs::write(root.join(INCOMPLETE), b"Starter assets pending")
            .map_err(|_| "Cannot save install status")?;
    }
    std::fs::write(
        root.join(format!(".l4d2r-pending-{}", options.pack)),
        b"Steam installation in progress",
    )
    .map_err(|_| "Cannot write game data. Check free storage.")?;
    event(Event::Status("Connecting to Steam…".into()));
    let servers = bounded(ServerList::discover(), 45, "Could not reach Steam").await?;
    let saved = saved.and_then(|s| serde_json::from_str::<SavedLogin>(&s).ok());
    let mut connection = None;
    if let Some(login) = saved {
        event(Event::Status("Restoring your Steam sign-in…".into()));
        match tokio::time::timeout(Duration::from_secs(45), Connection::login_saved(&servers,&login)).await {
            Ok(Ok(c)) => connection = Some(c),
            Ok(Err(steam_vent::ConnectionError::LoginError(_))) => event(Event::ForgetLogin),
            _ => return Err("Could not reconnect to Steam. Your saved sign-in and downloads are kept; check the connection and tap Resume.".into()),
        }
    }
    let connection = match connection {
        Some(c) => c,
        None => {
            let (connection, saved) = if let Some(mut credentials) = options.credentials.take() {
                use steam_vent::auth::{ConfirmationAction, SteamGuardToken};
                event(Event::Status("Signing in to Steam…".into()));
                let auth_event = &mut *event;
                let codes = &mut credentials.codes;
                bounded(Connection::login_password_saved(&servers, &credentials.account, &credentials.password, |methods| async move {
                    if let Some(kind) = methods.iter().find_map(|m| m.token_type()) {
                        auth_event(Event::GuardCode);
                        match codes.recv().await {
                            Some(code) => ConfirmationAction::GuardToken(SteamGuardToken::from_code(code), kind),
                            None => ConfirmationAction::Abort,
                        }
                    } else if methods.iter().any(|m| m.action_required()) {
                        auth_event(Event::Status("Approve the sign-in request in the Steam app on this phone, then return here.".into()));
                        std::future::pending::<ConfirmationAction>().await
                    } else { ConfirmationAction::None }
                }), 300, "Steam sign-in failed. Check your account name, password and Steam Guard approval").await?
            } else if options.qr {
                event(Event::Status("Approve sign-in with Steam Guard.".into()));
                bounded(
                    Connection::login_qr(&servers, "L4D2 Rewrite", |url| {
                        if url.starts_with("https://s.team/q/") {
                            event(Event::Qr(url.to_owned()));
                        }
                    }),
                    300,
                    "Steam sign-in was not completed",
                )
                .await?
            } else {
                event(Event::NeedCredentials);
                return Err("Sign in with your Steam account to continue.".into());
            };
            event(Event::SaveLogin(
                serde_json::to_string(&saved).map_err(|_| "Cannot save the Steam sign-in")?,
            ));
            connection
        }
    };
    event(Event::Status(
        "Checking Steam access to Left 4 Dead 2…".into(),
    ));
    let cdn = bounded(
        CDNClient::discover(Arc::new(connection)),
        45,
        "Could not find Steam download servers",
    )
    .await?;
    let apps = bounded(
        cdn.get_depots(vec![APP]),
        45,
        "Could not read L4D2 content information",
    )
    .await?;
    let app = apps
        .iter()
        .find(|a| a.app_id == APP)
        .ok_or("Steam did not return Left 4 Dead 2")?;
    let mut plans = Vec::new();
    // Request authorization for BOTH depots before downloading any game data.
    let mut authorized = Vec::new();
    for depot_id in DEPOTS {
        let depot = app
            .depots
            .iter()
            .find(|d| d.depot_id == depot_id)
            .ok_or("A required L4D2 depot is unavailable")?;
        let gid = depot
            .manifests
            .iter()
            .find(|m| m.branch == "public")
            .and_then(|m| m.gid())
            .ok_or("Steam did not supply a public L4D2 manifest")?;
        let key = bounded(cdn.get_depot_decryption_key(APP, depot_id), 45, "Steam content authorization failed").await?
            .ok_or("Steam did not authorize the required L4D2 content. Sign in with an account that owns Left 4 Dead 2.")?;
        let code = bounded(
            cdn.get_manifest_request_code(APP, depot_id, gid),
            45,
            "Steam did not authorize the L4D2 manifest",
        )
        .await?;
        authorized.push((depot_id, gid, key, code));
    }
    event(Event::Status(
        "Steam authorized L4D2. Preparing the asset download…".into(),
    ));
    for (depot, gid, key, code) in authorized {
        let manifest = bounded(
            cdn.get_manifest(depot, gid, Some(code), Some(key)),
            90,
            "Could not download the Steam content manifest",
        )
        .await?;
        plans.push((depot, gid, key, manifest));
    }
    // Later depots override earlier ones, as in the installed game.
    let mut selected = HashSet::new();
    let mut files = Vec::new();
    for (depot, gid, key, manifest) in plans.iter().rev() {
        for file in manifest.files() {
            if !file.linktarget().is_empty() || file.flags() & 64 != 0 {
                continue;
            }
            let Some(relative) = asset_path(&file.full_path()) else {
                continue;
            };
            if !packs::includes(options.pack, &file.full_path()) {
                continue;
            }
            if selected.insert(relative.clone()) {
                files.push((*depot, *gid, *key, file, relative));
            }
        }
    }
    files.sort_by_key(|(_, _, _, _, path)| packs::priority(&path.to_string_lossy()));
    let total = files.iter().map(|(_, _, _, f, _)| f.size()).sum::<u64>();
    if total == 0 {
        return Err("Steam returned no usable game assets".into());
    }
    let mut required = 0u64;
    for (depot, gid, _, file, relative) in &files {
        let output = target(&root, relative)?;
        if std::fs::metadata(output).is_ok_and(|m| m.len() == file.size()) {
            continue;
        }
        let partial = root.join(format!(
            ".l4d2r-downloads/{depot}/{gid}/{:x}.part",
            Sha1::digest(relative.to_string_lossy().as_bytes())
        ));
        let have = std::fs::metadata(partial).map_or(0, |m| m.len().min(file.size()));
        required = required.saturating_add(file.size() - have);
    }
    let available =
        fs2::available_space(&root).map_err(|_| "Cannot check available iPhone storage")?;
    if available < required.saturating_add(256 * 1024 * 1024) {
        return Err(format!(
            "Not enough free storage. This download needs about {:.1} GB more; {:.1} GB is available. Free space and tap Resume.",
            required as f64 / 1e9,
            available as f64 / 1e9
        ));
    }
    let mut done = 0;
    let mut verified = receipts::Verified::load(&root);
    for (depot, gid, key, file, relative) in files {
        let output = target(&root, &relative)?;
        let name = relative.to_string_lossy().into_owned();
        event(Event::Progress {
            done,
            total,
            file: format!("Checking {name}"),
        });
        if verified.matches(&name, &output, file.size(), &file.sha_content())
            || valid_file(&output, file.size(), &file.sha_content()).await
        {
            verified.record(&name, &output, &file.sha_content());
            verified.flush(&root, false);
            done += file.size();
            continue;
        }
        let partial_dir = target(&root, Path::new(&format!(".l4d2r-downloads/{depot}/{gid}")))?;
        std::fs::create_dir_all(&partial_dir)
            .map_err(|_| "Cannot create download files. Check free storage.")?;
        let partial_name = format!("{:x}.part", Sha1::digest(name.as_bytes()));
        let partial = target(
            &root,
            &Path::new(&format!(".l4d2r-downloads/{depot}/{gid}")).join(partial_name),
        )?;
        let mut success = false;
        let mut last_error = String::new();
        for attempt in 0..3 {
            let length = tokio::fs::metadata(&partial).await.map_or(0, |m| m.len());
            let offset = file
                .chunks()
                .iter()
                .map(|c| c.offset() + u64::from(c.original_size()))
                .filter(|&end| end <= length && end <= file.size())
                .max()
                .unwrap_or(0);
            let mut stream = tokio::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&partial)
                .await
                .map_err(|_| "Cannot write download files. Check free storage.")?;
            stream
                .set_len(offset)
                .await
                .map_err(|_| "Cannot resume the download")?;
            stream
                .seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|_| "Cannot resume the download")?;
            let mut last = Instant::now() - Duration::from_secs(1);
            event(Event::Status(format!(
                "Downloading {name}{}",
                if attempt == 0 {
                    String::new()
                } else {
                    format!(" — retry {} (trying another Steam server)", attempt + 1)
                }
            )));
            let transfer = file.download_resume(key, &mut stream, offset, 16, |bytes| {
                if last.elapsed() >= Duration::from_millis(200) {
                    event(Event::Progress {
                        done: done + bytes,
                        total,
                        file: name.clone(),
                    });
                    last = Instant::now();
                }
            });
            let result = match tokio::time::timeout(Duration::from_secs(1800), transfer).await {
                Ok(result) => result.map_err(|error| safe_download_error(&error)),
                Err(_) => Err("Transfer timed out. Resume will keep verified chunks.".to_owned()),
            };
            drop(stream);
            if result.is_ok() && valid_file(&partial, file.size(), &file.sha_content()).await {
                success = true;
                break;
            }
            last_error = result.err().unwrap_or_else(|| {
                "File hash did not match Steam’s manifest; downloading again.".to_owned()
            });
            event(Event::Status(format!("{name}: {last_error}")));
            if last_error.starts_with("File hash") {
                let _ = tokio::fs::remove_file(&partial).await;
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
            }
        }
        if !success {
            return Err(format!(
                "Could not finish {name}: {last_error} Tap Resume; verified files and chunks are kept."
            ));
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).map_err(|_| "Cannot create an asset folder")?;
        }
        // A verified partial is promoted only after the whole-file hash matches.
        #[cfg(windows)]
        if output.exists() {
            std::fs::remove_file(&output).map_err(|_| "Cannot replace an old asset")?;
        }
        tokio::fs::rename(&partial, &output)
            .await
            .map_err(|_| "Cannot finish installing an asset")?;
        verified.record(&name, &output, &file.sha_content());
        verified.flush(&root, false);
        done += file.size();
        event(Event::Progress {
            done,
            total,
            file: name,
        });
    }
    if !root.join("left4dead2/gameinfo.txt").is_file()
        || !root.join("left4dead2/pak01_dir.vpk").is_file()
    {
        return Err("The downloaded assets are missing essential L4D2 files".into());
    }
    let receipt = serde_json::json!({"version":4,"pack":options.pack,"complete":true});
    for pack in if options.pack == 16 {
        (0..=16).collect::<Vec<_>>()
    } else {
        vec![0, 1, options.pack]
    } {
        let path = root.join(format!(".l4d2r-pack-{pack}.json"));
        std::fs::write(path.with_extension("tmp"), receipt.to_string())
            .map_err(|_| "Cannot save installation status")?;
        std::fs::rename(path.with_extension("tmp"), path)
            .map_err(|_| "Cannot finish installation status")?;
    }
    let _ = std::fs::remove_file(root.join(format!(".l4d2r-pending-{}", options.pack)));
    let _ = std::fs::remove_file(root.join(INCOMPLETE));
    verified.flush(&root, true);
    event(Event::Complete);
    Ok(())
}

fn safe_download_error(error: &steam_cdn::Error) -> String {
    use steam_cdn::Error;
    match error {
        Error::HttpStatus(status) => {
            format!("Steam download server returned HTTP {}.", status.as_u16())
        }
        Error::Io(error) if error.raw_os_error() == Some(28) => {
            "Not enough storage on this device.".into()
        }
        Error::Io(_) => "Could not write the downloaded asset. Check free storage.".into(),
        Error::Decompress(_) | Error::Unexpected(_) | Error::Eof(_) => {
            "Downloaded chunk failed verification or decompression.".into()
        }
        _ => "Connection to the Steam download server was interrupted.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_escape_paths_and_binaries_but_keeps_game_data() {
        for bad in [
            "../left4dead2/x",
            "/left4dead2/x",
            "C:/left4dead2/x",
            "left4dead2/../../x",
            "left4dead2//x",
            "left4dead2/./x",
            "left4dead2/bin/server.dll",
            "left4dead2/engine.EXE",
            "steam.exe",
            "left4dead2/x\0y",
        ] {
            assert!(asset_path(bad).is_none(), "{bad:?}");
        }
        for good in [
            "left4dead2/pak01_dir.vpk",
            "update/pak01_001.vpk",
            "hl2/resource/fonts/test.ttf",
            "left4dead2_dlc3/scripts/test.nut",
            "left4dead2\\maps\\c1m1_hotel.bsp",
        ] {
            assert!(asset_path(good).is_some(), "{good}");
        }
    }
    #[tokio::test]
    async fn matching_size_is_not_enough_to_skip_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("asset");
        std::fs::write(&p, b"good").unwrap();
        let hash = Sha1::digest(b"good");
        assert!(valid_file(&p, 4, &hash).await);
        std::fs::write(&p, b"evil").unwrap();
        assert!(!valid_file(&p, 4, &hash).await);
    }
    #[tokio::test]
    async fn steam_empty_placeholder_is_valid_but_zero_hash_cannot_bypass_nonempty_checks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pak01_005.vpk");
        std::fs::write(&path, b"").unwrap();
        assert!(valid_file(&path, 0, &[0; 20]).await);
        assert!(valid_file(&path, 0, &Sha1::digest(b"")).await);
        std::fs::write(&path, b"evil").unwrap();
        assert!(!valid_file(&path, 0, &[0; 20]).await);
        assert!(!valid_file(&path, 4, &[0; 20]).await);
    }
    #[test]
    fn partial_install_is_never_treated_as_playable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("left4dead2")).unwrap();
        for n in ["gameinfo.txt", "pak01_dir.vpk"] {
            std::fs::write(dir.path().join("left4dead2").join(n), b"x").unwrap();
        }
        assert!(!needs_install(dir.path()));
        std::fs::write(dir.path().join(INCOMPLETE), b"x").unwrap();
        assert!(needs_install(dir.path()));
    }
    #[tokio::test]
    async fn cancelled_install_does_not_connect_or_write_data() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("game");
        let result = install(root.clone(), None, Arc::new(AtomicBool::new(true)), |_| {
            panic!("No network events expected")
        })
        .await;
        assert!(result.unwrap_err().contains("Paused"));
        assert!(!root.exists());
    }
    #[cfg(unix)]
    #[test]
    fn refuses_existing_symlinks_inside_asset_paths() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("left4dead2")).unwrap();
        assert!(target(root.path(), Path::new("left4dead2/gameinfo.txt")).is_err());
    }
}
