//! File-level packs. Shared VPK archives stay intact; they serve many maps.
use std::path::Path;

pub const PACK_NAMES: [&str; 17] = [
    "Starter: shared assets + Dead Center",
    "Dead Center",
    "Dark Carnival",
    "Swamp Fever",
    "Hard Rain",
    "The Parish",
    "The Passing",
    "The Sacrifice",
    "No Mercy",
    "Crash Course",
    "Death Toll",
    "Dead Air",
    "Blood Harvest",
    "Cold Stream",
    "The Last Stand",
    "Extra survivor dialogue",
    "All game files",
];
pub fn campaign(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    let b = lower.as_bytes();
    for i in 0..b.len() {
        if b[i] != b'c' || (i > 0 && b[i - 1].is_ascii_alphanumeric()) {
            continue;
        }
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == i + 1 || j >= b.len() || (b[j] != b'm' && b[j] != b'.' && b[j] != b'_') {
            continue;
        }
        if let Ok(id @ 1..=14) = lower[i + 1..j].parse::<u8>() {
            return Some(id);
        }
    }
    None
}
pub fn group(name: &str) -> u8 {
    let lower = name.to_ascii_lowercase();
    // Every campaign depends on these archives, including update/DLC archives.
    if lower.ends_with(".vpk") {
        return 0;
    }
    if lower.contains("/maps/") {
        return campaign(&lower).unwrap_or(16);
    }
    // The five main-menu backgrounds (~48 MB) play on iOS via ios/bink.c;
    // the intro, legal and upsell videos stay optional.
    if lower.contains("/media/l4d2_background") && lower.ends_with(".bik") {
        return 0;
    }
    if lower.contains("/media/") || lower.ends_with(".bik") {
        return 16;
    }
    if lower.contains("/sound/") {
        if let Some(c) = campaign(&lower) {
            return c;
        }
        if lower.contains("/survivor/voice/") {
            let stem = Path::new(&lower)
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if ![
                "pain", "death", "fall", "incap", "hurt", "help", "revive", "heal", "ledge",
                "warn", "reload", "charger", "hunter", "smoker", "jockey", "spitter", "boomer",
            ]
            .iter()
            .any(|s| stem.starts_with(s))
            {
                return 15;
            }
        }
    }
    0
}
pub fn includes(pack: u8, name: &str) -> bool {
    let group = group(name);
    pack == 16 || group == 0 || group == 1 || (pack != 0 && pack == group)
}
pub fn priority(name: &str) -> (u8, String) {
    let name = name.to_ascii_lowercase();
    let priority = if name.ends_with("gameinfo.txt") || name.ends_with("_dir.vpk") {
        0
    } else if name.ends_with(".vpk") {
        1
    } else if name.contains("/maps/c1m") {
        2
    } else if !name.contains("/sound/") {
        3
    } else {
        4
    };
    (priority, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separates_campaigns_without_confusing_c1_and_c10() {
        assert!(includes(0, "update/pak01_005.vpk"));
        assert!(includes(0, "left4dead2/maps/c1m1_hotel.bsp"));
        assert!(!includes(0, "left4dead2/maps/c10m1_caves.bsp"));
        assert!(includes(10, "left4dead2/maps/c10m1_caves.bsp"));
        assert!(!includes(2, "left4dead2/maps/c3m1_plankcountry.bsp"));
        assert!(!includes(0, "left4dead2/media/intro.bik"));
        assert!(includes(0, "left4dead2/media/l4d2_background03.bik"));
        assert!(includes(7, "left4dead2/media/l4d2_background01.bik"));
        assert!(!includes(
            0,
            "left4dead2/sound/player/survivor/voice/coach/story01.wav"
        ));
        assert!(includes(
            0,
            "left4dead2/sound/player/survivor/voice/coach/pain01.wav"
        ));
        assert!(includes(
            15,
            "left4dead2/sound/player/survivor/voice/coach/story01.wav"
        ));
    }
}
