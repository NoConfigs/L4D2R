//! Opt-in, small live test. Reads a local prototype login; never prints it.
//! cargo run --example verify_steam -- <private-login-json> <scratch-output>
use sha1::{Digest, Sha1};
use std::{path::PathBuf, sync::Arc};
use steam_cdn::CDNClient;
use steam_vent::{Connection, ServerList, auth::SavedLogin};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    assert!(
        args.len() >= 3,
        "Provide login JSON and scratch output file"
    );
    let wanted = args
        .get(3)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "left4dead2/gameinfo.txt".into());
    let mut sizes = std::collections::HashMap::<String, u64>::new();
    let saved: SavedLogin = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let servers = ServerList::discover().await?;
    let connection = Connection::login_saved(&servers, &saved).await?;
    let cdn = CDNClient::discover(Arc::new(connection)).await?;
    let apps = cdn.get_depots(vec![550]).await?;
    for depot in [551, 552] {
        let d = apps
            .iter()
            .find(|a| a.app_id == 550)
            .unwrap()
            .depots
            .iter()
            .find(|d| d.depot_id == depot)
            .unwrap();
        let gid = d
            .manifests
            .iter()
            .find(|m| m.branch == "public")
            .and_then(|m| m.gid())
            .unwrap();
        let key = cdn
            .get_depot_decryption_key(550, depot)
            .await?
            .ok_or("Steam denied required depot")?;
        let code = cdn.get_manifest_request_code(550, depot, gid).await?;
        println!("Steam authorized depot {depot}");
        {
            let manifest = cdn.get_manifest(depot, gid, Some(code), Some(key)).await?;
            for f in manifest.files() {
                if f.flags() & 64 == 0
                    && l4d2r_steam_installer::asset_path(&f.full_path()).is_some()
                {
                    sizes.insert(f.full_path(), f.size());
                }
            }
            let Some(f) = manifest.files().iter().find(|f| f.full_path() == wanted) else {
                continue;
            };
            assert!(f.size() < 256 * 1024 * 1024);
            let output = PathBuf::from(&args[2]);
            let mut file = tokio::fs::File::create(&output).await?;
            let started = std::time::Instant::now();
            f.download_resume(key, &mut file, 0, 8, |_| {}).await?;
            drop(file);
            let bytes = std::fs::read(&output)?;
            assert_eq!(bytes.len() as u64, f.size());
            if f.size() == 0 && f.sha_content().iter().all(|b| *b == 0) {
                println!("Steam empty placeholder verified (zero hash)");
            } else {
                assert_eq!(Sha1::digest(&bytes).as_slice(), f.sha_content().as_slice());
            }
            println!(
                "Downloaded {} bytes; chunk and file SHA-1 verified",
                bytes.len()
            );
            println!(
                "Transfer took {:.2} seconds",
                started.elapsed().as_secs_f64()
            );
            if f.chunks().len() > 2 {
                use tokio::io::AsyncSeekExt;
                let offset = f.chunks().iter().map(|c| c.offset()).max().unwrap();
                let mut file = tokio::fs::OpenOptions::new()
                    .write(true)
                    .open(&output)
                    .await?;
                file.set_len(offset).await?;
                file.seek(std::io::SeekFrom::Start(offset)).await?;
                f.download_resume(key, &mut file, offset, 8, |_| {}).await?;
                drop(file);
                assert_eq!(
                    Sha1::digest(std::fs::read(&output)?).as_slice(),
                    f.sha_content().as_slice()
                );
                println!("Interrupted-file resume at {offset} verified");
            }
        }
    }
    for pack in [0, 1, 2, 15, 16] {
        let bytes: u64 = sizes
            .iter()
            .filter(|(n, _)| l4d2r_steam_installer::packs::includes(pack, n))
            .map(|(_, s)| *s)
            .sum();
        println!("Pack {pack}: {bytes} bytes");
    }
    Ok(())
}
