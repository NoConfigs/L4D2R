use crate::{binary::*, vpk};
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File},
    io::{self, Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};
struct Location {
    directory: PathBuf,
    data_start: u64,
    entry: vpk::Entry,
}
struct AssetIndex {
    mounts: Vec<PathBuf>,
    entries: HashMap<String, Location>,
}
#[derive(PartialEq, Eq)]
struct ArchiveStamp {
    path: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
}
#[derive(PartialEq, Eq)]
struct IndexSnapshot {
    root: PathBuf,
    mounts: Vec<PathBuf>,
    archives: Vec<ArchiveStamp>,
}
// Share metadata, never decoded textures or a map's embedded ZIP. A small LRU
// also keeps repeated chapter loads from re-parsing every installed VPK tree.
type IndexCache = VecDeque<(IndexSnapshot, Arc<AssetIndex>)>;
static INDEX_CACHE: OnceLock<Mutex<IndexCache>> = OnceLock::new();
const CACHED_INSTALLATIONS: usize = 4;
pub struct SourceAssets {
    index: Arc<AssetIndex>,
    pak: Option<zip::ZipArchive<Cursor<Vec<u8>>>>,
}
pub fn normalize(path: &str) -> io::Result<String> {
    let p = path.replace('\\', "/").to_lowercase();
    if p.starts_with('/') || p.contains(':') || p.split('/').any(|s| s == "..") {
        return Err(invalid("Unsafe virtual asset path"));
    }
    Ok(p.trim_start_matches("./").to_owned())
}
impl SourceAssets {
    #[allow(dead_code)] // Read-only asset_probe inventory command.
    pub fn matching_paths(&self, filter: &str) -> Vec<String> {
        let mut paths = self
            .index.entries
            .keys()
            .filter(|p| p.contains(filter))
            .cloned()
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }
    /// Lower-case paths inside the map's pakfile.
    pub fn pak_names(&self) -> Vec<String> {
        self.pak.as_ref().map_or_else(Vec::new, |p| p.file_names().map(|n| n.to_ascii_lowercase().replace('\\', "/")).collect())
    }
    pub fn new(root: &Path, pak: Vec<u8>) -> io::Result<Self> {
        let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let mut snapshot = IndexSnapshot { root: root.clone(), mounts: vec![], archives: vec![] };
        for name in ["update", "left4dead2_dlc3", "left4dead2_dlc2", "left4dead2_dlc1", "left4dead2", "hl2"] {
            let mount = root.join(name);
            if !mount.is_dir() { continue; }
            let mut directories: Vec<_> = fs::read_dir(&mount)?
                .filter_map(Result::ok).map(|e| e.path())
                .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with("_dir.vpk")))
                .collect();
            directories.sort();
            snapshot.mounts.push(mount);
            for path in directories {
                let metadata = fs::metadata(&path)?;
                snapshot.archives.push(ArchiveStamp { path, length: metadata.len(), modified: metadata.modified().ok() });
            }
        }
        let cache = INDEX_CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
        let cached = {
            let mut cache = cache.lock().map_err(|_| io::Error::other("Asset index cache lock poisoned"))?;
            cache.iter().position(|(key, _)| *key == snapshot).map(|at| {
                let entry = cache.remove(at).unwrap();
                let index = Arc::clone(&entry.1);
                cache.push_back(entry);
                index
            })
        };
        let index = if let Some(index) = cached { index } else {
            let mut entries = HashMap::new();
            for stamp in &snapshot.archives {
                let bytes = fs::read(&stamp.path)?;
                let header = if u32_at(&bytes, 4)? == 1 { 12 } else { 28 };
                let data_start = header + u32_at(&bytes, 8)? as u64;
                for entry in vpk::parse(&bytes)? {
                    entries.entry(normalize(&entry.path)?).or_insert_with(|| Location {
                        directory: stamp.path.clone(), data_start, entry,
                    });
                }
            }
            let index = Arc::new(AssetIndex { mounts: snapshot.mounts.clone(), entries });
            let mut cache = cache.lock().map_err(|_| io::Error::other("Asset index cache lock poisoned"))?;
            // Another map worker may have prepared the same snapshot while we
            // parsed it. Reuse its allocation instead of retaining a duplicate.
            if let Some(at) = cache.iter().position(|(key, _)| *key == snapshot) {
                let entry = cache.remove(at).unwrap();
                let shared = Arc::clone(&entry.1); cache.push_back(entry); shared
            } else {
                cache.push_back((snapshot, Arc::clone(&index)));
                while cache.len() > CACHED_INSTALLATIONS { cache.pop_front(); }
                index
            }
        };
        let mut result = Self { index, pak: None };
        if !pak.is_empty() {
            result.pak =
                Some(zip::ZipArchive::new(Cursor::new(pak)).map_err(|e| invalid(e.to_string()))?);
        }
        Ok(result)
    }
    pub fn exists(&mut self, path: &str) -> bool {
        let Ok(path) = normalize(path) else {
            return false;
        };
        self.pak.as_mut().is_some_and(|p| p.by_name(&path).is_ok())
            || self.index.mounts.iter().any(|m| m.join(&path).is_file())
            || self.index.entries.contains_key(&path)
    }
    pub fn read(&mut self, path: &str) -> io::Result<Vec<u8>> {
        let path = normalize(path)?;
        if let Some(pak) = self.pak.as_mut() {
            if let Ok(mut file) = pak.by_name(&path) {
                if file.size() > 128 * 1024 * 1024 {
                    return Err(invalid("Oversized embedded asset"));
                }
                let mut bytes = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut bytes)?;
                return Ok(bytes);
            }
        }
        // Loose files override archives within their own mount, not later mounts.
        for mount in &self.index.mounts {
            let loose = mount.join(&path);
            if loose.is_file() {
                if fs::metadata(&loose)?.len() > 128 * 1024 * 1024 {
                    return Err(invalid("Oversized loose asset"));
                }
                return fs::read(loose);
            }
            if let Some(location) = self.index.entries.get(&path) {
                if location.directory.parent() == Some(mount.as_path()) {
                    return read_location(location);
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Missing owned asset: {path}"),
        ))
    }
}
fn read_location(location: &Location) -> io::Result<Vec<u8>> {
    let e = &location.entry;
    if e.length as usize + e.preload.len() > 128 * 1024 * 1024 {
        return Err(invalid("Oversized VPK asset"));
    }
    let (path, offset) = if e.archive == 0x7fff {
        (
            location.directory.clone(),
            location.data_start + e.offset as u64,
        )
    } else {
        let stem = location.directory.file_name().unwrap().to_string_lossy();
        (
            location.directory.with_file_name(format!(
                "{}_{:03}.vpk",
                stem.trim_end_matches("_dir.vpk"),
                e.archive
            )),
            e.offset as u64,
        )
    };
    let mut file = File::open(path)?;
    if offset + e.length as u64 > file.metadata()?.len() {
        return Err(invalid("VPK payload outside archive"));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = e.preload.clone();
    bytes.resize(bytes.len() + e.length as usize, 0);
    file.read_exact(&mut bytes[e.preload.len()..])?;
    if crc32fast::hash(&bytes) != e.crc {
        return Err(invalid(format!("VPK CRC mismatch: {}", e.path)));
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    // These tests assert allocation identity in the shared four-installation
    // LRU. Unrelated concurrent fixtures can legitimately evict that identity.
    static FIXTURE_LOCK: Mutex<()> = Mutex::new(());
    fn owned_fixture() -> PathBuf {
        // Windows clock resolution can give parallel tests the same timestamp.
        // A process-local sequence prevents their live loose overrides from
        // leaking into another test's archive/CRC checks.
        static FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let fixture_id = FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "asset-test-{}-{}-{}",
                std::process::id(),
                fixture_id,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let mount = root.join("left4dead2");
        fs::create_dir_all(&mount).unwrap();
        let mut tree = b"txt\0scripts\0test\0".to_vec();
        tree.extend(crc32fast::hash(b"hello").to_le_bytes());
        tree.extend(2u16.to_le_bytes());
        tree.extend(0x7fffu16.to_le_bytes());
        tree.extend(0u32.to_le_bytes());
        tree.extend(3u32.to_le_bytes());
        tree.extend(0xffffu16.to_le_bytes());
        tree.extend(b"he\0\0\0");
        let mut b = vec![];
        b.extend(0x55aa1234u32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend((tree.len() as u32).to_le_bytes());
        b.extend(tree);
        b.extend(b"llo");
        fs::write(mount.join("pak01_dir.vpk"), b).unwrap();
        root
    }
    fn cleanup(root: PathBuf) {
        let mount = root.join("left4dead2");
        let loose = mount.join("scripts/test.txt");
        if loose.exists() {
            fs::remove_file(loose).unwrap();
            fs::remove_dir(mount.join("scripts")).unwrap();
        }
        fs::remove_file(mount.join("pak01_dir.vpk")).unwrap();
        fs::remove_dir(mount).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn joins_preload_and_embedded_payload_and_checks_crc() {
        let _fixture_guard = FIXTURE_LOCK.lock().unwrap();
        let root = owned_fixture();
        let mut assets = SourceAssets::new(&root, vec![]).unwrap();
        assert_eq!(assets.read("scripts/test.txt").unwrap(), b"hello");
        let path = root.join("left4dead2/pak01_dir.vpk");
        let mut b = fs::read(&path).unwrap();
        *b.last_mut().unwrap() = b'X';
        fs::write(path, b).unwrap();
        assert!(assets.read("scripts/test.txt").is_err());
        cleanup(root);
    }
    #[test]
    fn embedded_map_assets_override_loose_and_vpk_files() {
        use std::io::Write;
        let _fixture_guard = FIXTURE_LOCK.lock().unwrap();
        let root = owned_fixture();
        let scripts = root.join("left4dead2/scripts");
        fs::create_dir(&scripts).unwrap();
        fs::write(scripts.join("test.txt"), b"loose").unwrap();
        let mut loose = SourceAssets::new(&root, vec![]).unwrap();
        assert_eq!(loose.read("scripts/test.txt").unwrap(), b"loose");
        let mut zip = zip::ZipWriter::new(Cursor::new(vec![]));
        zip.start_file(
            "scripts/test.txt",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(b"map embedded").unwrap();
        let mut assets = SourceAssets::new(&root, zip.finish().unwrap().into_inner()).unwrap();
        assert_eq!(assets.read("scripts/test.txt").unwrap(), b"map embedded");
        cleanup(root);
    }
    #[test]
    fn rejects_traversal_and_normalizes_source_paths() {
        assert!(normalize("../secret").is_err());
        assert!(normalize("C:\\secret").is_err());
        assert_eq!(
            normalize("Materials\\Brick.VMT").unwrap(),
            "materials/brick.vmt"
        );
    }
    #[test]
    fn readers_share_archive_index_but_still_read_live_loose_overrides() {
        let _fixture_guard = FIXTURE_LOCK.lock().unwrap();
        let root = owned_fixture();
        let mut first = SourceAssets::new(&root, vec![]).unwrap();
        let mut second = SourceAssets::new(&root, vec![]).unwrap();
        assert!(Arc::ptr_eq(&first.index, &second.index));
        assert_eq!(first.read("scripts/test.txt").unwrap(), b"hello");
        fs::create_dir(root.join("left4dead2/scripts")).unwrap();
        fs::write(root.join("left4dead2/scripts/test.txt"), b"loose override").unwrap();
        assert_eq!(first.read("scripts/test.txt").unwrap(), b"loose override");
        fs::write(root.join("left4dead2/scripts/test.txt"), b"edited while readers live").unwrap();
        assert_eq!(second.read("scripts/test.txt").unwrap(), b"edited while readers live");
        cleanup(root);
    }
    #[test]
    fn shared_index_invalidates_when_archive_metadata_or_mounts_change() {
        let _fixture_guard = FIXTURE_LOCK.lock().unwrap();
        let root = owned_fixture();
        let first = SourceAssets::new(&root, vec![]).unwrap();
        let directory = root.join("left4dead2/pak01_dir.vpk");
        let mut bytes = fs::read(&directory).unwrap();
        bytes.push(0); // Valid extra data, with a deterministically different length.
        fs::write(&directory, &bytes).unwrap();
        let mut changed = SourceAssets::new(&root, vec![]).unwrap();
        assert!(!Arc::ptr_eq(&first.index, &changed.index));
        assert_eq!(changed.read("scripts/test.txt").unwrap(), b"hello");
        let update = root.join("update");
        fs::create_dir(&update).unwrap();
        let mut higher = fs::read(&directory).unwrap();
        let crc = 12 + b"txt\0scripts\0test\0".len();
        higher[crc..crc + 4].copy_from_slice(&crc32fast::hash(b"heNEW").to_le_bytes());
        let payload = higher.len() - 4;
        higher[payload..payload + 3].copy_from_slice(b"NEW");
        fs::write(update.join("pak01_dir.vpk"), higher).unwrap();
        let mut added = SourceAssets::new(&root, vec![]).unwrap();
        assert_eq!(added.read("scripts/test.txt").unwrap(), b"heNEW", "higher-priority added mount must override cached base");
        fs::remove_file(update.join("pak01_dir.vpk")).unwrap();
        fs::remove_dir(update).unwrap();
        let mut removed = SourceAssets::new(&root, vec![]).unwrap();
        assert_eq!(removed.read("scripts/test.txt").unwrap(), b"hello");
        assert!(INDEX_CACHE.get().unwrap().lock().unwrap().len() <= CACHED_INSTALLATIONS);
        cleanup(root);
    }
    #[test]
    fn shared_index_keeps_each_maps_embedded_assets_isolated() {
        use std::io::Write;
        let _fixture_guard = FIXTURE_LOCK.lock().unwrap();
        let root = owned_fixture();
        let embedded = |text: &[u8]| {
            let mut zip = zip::ZipWriter::new(Cursor::new(vec![]));
            zip.start_file("scripts/test.txt", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(text).unwrap();
            zip.finish().unwrap().into_inner()
        };
        let mut first = SourceAssets::new(&root, embedded(b"first map")).unwrap();
        let mut second = SourceAssets::new(&root, embedded(b"second map")).unwrap();
        assert!(Arc::ptr_eq(&first.index, &second.index));
        assert_eq!(first.read("scripts/test.txt").unwrap(), b"first map");
        assert_eq!(second.read("scripts/test.txt").unwrap(), b"second map");
        cleanup(root);
    }
}
