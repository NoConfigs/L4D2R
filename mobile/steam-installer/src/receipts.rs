//! Avoid re-reading gigabytes of previously verified files after each resume.
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant, UNIX_EPOCH},
};
pub struct Verified {
    entries: HashMap<String, Value>,
    last: Instant,
}
impl Verified {
    pub fn load(root: &Path) -> Self {
        let path = root.join(".l4d2r-verified-v1.json");
        let entries = if std::fs::metadata(&path)
            .is_ok_and(|m| m.len() < 16 * 1024 * 1024 && !m.file_type().is_symlink())
        {
            std::fs::read(path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        Self {
            entries,
            last: Instant::now(),
        }
    }
    fn stamp(path: &Path, hash: &[u8]) -> Option<Value> {
        let m = std::fs::symlink_metadata(path).ok()?;
        if !m.is_file() || m.file_type().is_symlink() {
            return None;
        }
        Some(
            json!({"size":m.len(),"modified":m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos().to_string(),"sha1":hash}),
        )
    }
    pub fn matches(&self, name: &str, path: &Path, size: u64, hash: &[u8]) -> bool {
        self.entries.get(name).is_some_and(|saved| {
            saved["size"].as_u64() == Some(size) && Self::stamp(path, hash).as_ref() == Some(saved)
        })
    }
    pub fn record(&mut self, name: &str, path: &Path, hash: &[u8]) {
        if let Some(stamp) = Self::stamp(path, hash) {
            self.entries.insert(name.to_owned(), stamp);
        }
    }
    pub fn flush(&mut self, root: &Path, force: bool) {
        if !force && self.last.elapsed() < Duration::from_secs(2) {
            return;
        }
        let dest = root.join(".l4d2r-verified-v1.json");
        let temp = root.join(".l4d2r-verified-v1.tmp");
        if [temp.as_path(), dest.as_path()]
            .iter()
            .any(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
        {
            return;
        }
        if let Ok(data) = serde_json::to_vec(&self.entries) {
            if std::fs::write(&temp, data).is_ok() {
                #[cfg(windows)]
                {
                    let _ = std::fs::remove_file(&dest);
                }
                let _ = std::fs::rename(temp, dest);
            }
        }
        self.last = Instant::now();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipts_require_unchanged_metadata_and_manifest_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("asset");
        std::fs::write(&path, b"test").unwrap();
        let mut verified = Verified::load(dir.path());
        verified.record("asset", &path, b"steam hash");
        verified.flush(dir.path(), true);
        let verified = Verified::load(dir.path());
        assert!(verified.matches("asset", &path, 4, b"steam hash"));
        assert!(!verified.matches("asset", &path, 4, b"new manifest"));
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();
        assert!(!verified.matches("asset", &path, 4, b"steam hash"));
    }
}
