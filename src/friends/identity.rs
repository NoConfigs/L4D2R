//! Stable local player identity.
//!
//! The ID is generated once and saved to a file, so the same player keeps the
//! same ID across launches. It is an opaque 128-bit value written as 32 hex
//! digits. Generation uses the standard library's randomly seeded hashers, which
//! is enough to avoid collisions between friends but is not cryptographic.
//! Replace it with a proper UUID crate once one is added to Cargo.toml.

use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayerId(pub u128);

impl PlayerId {
    pub fn generate() -> Self {
        let a = RandomState::new().build_hasher().finish();
        let b = RandomState::new().build_hasher().finish();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        PlayerId((((a as u128) << 64) | b as u128) ^ nanos)
    }

    pub fn to_hex(self) -> String {
        format!("{:032x}", self.0)
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        if text.len() != 32 {
            return None;
        }
        u128::from_str_radix(text, 16).ok().map(PlayerId)
    }

    /// Reads the ID at `path`, creating and saving a new one if the file does
    /// not exist. A file that exists but does not hold a valid ID is an error,
    /// not a reason to silently replace the player's identity.
    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Self::from_hex(text.trim()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "player id file is corrupt")
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let id = Self::generate();
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                }
                fs::write(path, id.to_hex())?;
                Ok(id)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PlayerId;

    #[test]
    fn hex_round_trips() {
        let id = PlayerId(0x0123_4567_89ab_cdef_0011_2233_4455_6677);
        assert_eq!(PlayerId::from_hex(&id.to_hex()), Some(id));
    }

    #[test]
    fn hex_rejects_wrong_length_and_non_hex() {
        assert_eq!(PlayerId::from_hex("abc"), None);
        assert_eq!(PlayerId::from_hex(&"z".repeat(32)), None);
    }

    #[test]
    fn generated_ids_differ() {
        assert_ne!(PlayerId::generate(), PlayerId::generate());
    }
}
