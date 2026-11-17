//! Installed mission metadata and local single-player launch settings.
use crate::source_assets::SourceAssets;
use bevy::prelude::*;
use l4d2_formats::kv::Kv;
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Survivor {
    Ellis,
    Rochelle,
    Coach,
    Nick,
    Bill,
    Zoey,
    Francis,
    Louis,
}
impl Survivor {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ellis => "Ellis",
            Self::Rochelle => "Rochelle",
            Self::Coach => "Coach",
            Self::Nick => "Nick",
            Self::Bill => "Bill",
            Self::Zoey => "Zoey",
            Self::Francis => "Francis",
            Self::Louis => "Louis",
        }
    }
    pub fn character(self) -> &'static str {
        match self {
            Self::Ellis => "mechanic",
            Self::Rochelle => "producer",
            Self::Coach => "coach",
            Self::Nick => "gambler",
            Self::Bill => "namvet",
            Self::Zoey => "teenangst",
            Self::Francis => "biker",
            Self::Louis => "manager",
        }
    }
    pub fn arms(self) -> String {
        let suffix = match self {
            Self::Bill => "bill",
            Self::Zoey => "zoey",
            Self::Francis => "francis",
            Self::Louis => "louis",
            _ => self.character(),
        };
        format!(
            "models/weapons/arms/v_arms_{suffix}{}.mdl",
            if matches!(self, Self::Bill | Self::Zoey | Self::Francis | Self::Louis) {
                ""
            } else {
                "_new"
            }
        )
    }
    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Ellis,
            Self::Rochelle,
            Self::Coach,
            Self::Nick,
            Self::Bill,
            Self::Zoey,
            Self::Francis,
            Self::Louis,
        ]
        .into_iter()
        .find(|x| x.name().eq_ignore_ascii_case(s))
    }
    pub fn roster(original: bool) -> [Self; 4] {
        if original {
            [Self::Bill, Self::Zoey, Self::Francis, Self::Louis]
        } else {
            [Self::Ellis, Self::Rochelle, Self::Coach, Self::Nick]
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    Normal,
    Advanced,
    Expert,
}
impl Difficulty {
    pub const ALL: [Self; 4] = [Self::Easy, Self::Normal, Self::Advanced, Self::Expert];
    pub fn name(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Normal => "Normal",
            Self::Advanced => "Advanced",
            Self::Expert => "Expert",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|x| x.name().eq_ignore_ascii_case(s))
    }
    // Common-infected standing claw damage; complete difficulty rules remain unfinished.
    pub fn claw(self) -> f32 {
        match self {
            Self::Easy => 1.,
            Self::Normal => 2.,
            Self::Advanced => 5.,
            Self::Expert => 20.,
        }
    }
}
#[derive(Resource, Clone, Debug)]
pub struct SessionSettings {
    pub map: PathBuf,
    pub survivor: Survivor,
    pub difficulty: Difficulty,
    pub original: bool,
}
#[derive(Clone)]
pub struct Chapter {
    pub name: String,
    pub map: PathBuf,
    pub image: String,
}
#[derive(Clone)]
pub struct Campaign {
    pub name: String,
    pub chapters: Vec<Chapter>,
    pub original: bool,
}
#[derive(Resource)]
pub struct Catalog {
    pub campaigns: Vec<Campaign>,
}
#[derive(Resource)]
pub struct Selection {
    pub campaign: usize,
    pub chapter: usize,
    pub survivor: Survivor,
    pub difficulty: Difficulty,
    pub open: Option<usize>,
    pub error: String,
}
impl Catalog {
    pub fn load(root: &Path) -> io::Result<Self> {
        let campaigns = Self::load_mode(root, "coop")?;
        if campaigns.is_empty() {
            return Err(io::Error::other("No installed cooperative campaigns found"));
        }
        Ok(Self { campaigns })
    }
    /// The Survival maps of every installed campaign (missions/campaignN.txt
    /// "modes" "survival"), grouped by campaign like the co-op chapters.
    pub fn survival(root: &Path) -> Vec<Campaign> {
        Self::load_mode(root, "survival").unwrap_or_default()
    }
    fn load_mode(root: &Path, mode: &str) -> io::Result<Vec<Campaign>> {
        let mut assets = SourceAssets::new(root, vec![])?;
        let mut localized = std::collections::HashMap::new();
        for path in [
            "resource/l4d360ui_english.txt",
            "resource/l4d360ui_tu_english.txt",
        ] {
            let Ok(bytes) = assets.read(path) else {
                continue;
            };
            let text = if bytes.starts_with(&[255, 254]) {
                String::from_utf16_lossy(
                    &bytes[2..]
                        .chunks_exact(2)
                        .map(|b| u16::from_le_bytes([b[0], b[1]]))
                        .collect::<Vec<_>>(),
                )
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            };
            let language = Kv::parse(&text);
            if let Some(tokens) = language.get("lang").and_then(|v| v.get("Tokens")) {
                for token in tokens.children() {
                    if let Some(value) = token.as_str() {
                        localized.insert(token.key.to_ascii_lowercase(), value.to_owned());
                    }
                }
            }
        }
        let localize = |s: &str| {
            localized
                .get(&s.trim_start_matches('#').to_ascii_lowercase())
                .cloned()
                .unwrap_or_else(|| s.to_owned())
        };
        let mut campaigns = vec![];
        for index in 1..=14 {
            let Ok(bytes) = assets.read(&format!("missions/campaign{index}.txt")) else {
                continue;
            };
            let kv = Kv::parse(&String::from_utf8_lossy(&bytes));
            let Some(mission) = kv.get("mission") else {
                continue;
            };
            let Some(coop) = mission.get("modes").and_then(|v| v.get(mode)) else {
                continue;
            };
            let mut chapters = vec![];
            for entry in coop.children() {
                let Some(map) = entry.str("Map") else {
                    continue;
                };
                if let Some(path) = find_map(root, map).or_else(|| {
                    cfg!(target_os = "ios").then(|| root.join("left4dead2/maps").join(format!("{map}.bsp")))
                }) {
                    chapters.push(Chapter {
                        name: localize(entry.str("DisplayName").unwrap_or(map)),
                        map: path,
                        image: entry.str("Image").unwrap_or("maps/any").into(),
                    });
                }
            }
            if !chapters.is_empty() {
                campaigns.push(Campaign {
                    name: localize(mission.str("DisplayTitle").unwrap_or("Campaign")),
                    chapters,
                    original: mission.i32("survivor_set") == Some(1),
                });
            }
        }
        Ok(campaigns)
    }
    pub fn selection(&self, current: &SessionSettings) -> Selection {
        let (campaign, chapter) = self
            .campaigns
            .iter()
            .enumerate()
            .find_map(|(i, c)| {
                c.chapters
                    .iter()
                    .position(|ch| ch.map == current.map)
                    .map(|j| (i, j))
            })
            .unwrap_or((0, 0));
        Selection {
            campaign,
            chapter,
            survivor: current.survivor,
            difficulty: current.difficulty,
            open: None,
            error: String::new(),
        }
    }
}
pub fn find_map(root: &Path, name: &str) -> Option<PathBuf> {
    [
        "update",
        "left4dead2_dlc3",
        "left4dead2_dlc2",
        "left4dead2_dlc1",
        "left4dead2",
    ]
    .into_iter()
    .map(|m| root.join(m).join("maps").join(format!("{name}.bsp")))
    .find(|p| p.is_file())
}
pub fn from_args(path: &Path) -> SessionSettings {
    let args: Vec<_> = std::env::args().collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    SessionSettings {
        map: path.into(),
        survivor: value("--survivor")
            .and_then(Survivor::parse)
            .unwrap_or(Survivor::Ellis),
        difficulty: value("--difficulty")
            .and_then(Difficulty::parse)
            .unwrap_or(Difficulty::Normal),
        original: false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roster_matches_survivor_assets() {
        for original in [false, true] {
            let roster = Survivor::roster(original);
            assert_eq!(roster.len(), 4);
            for s in roster {
                assert_eq!(Survivor::parse(s.name()), Some(s));
                assert!(s.arms().ends_with(".mdl"));
            }
        }
    }
    #[test]
    fn selection_tracks_map_and_settings() {
        let map = PathBuf::from("test.bsp");
        let c = Catalog {
            campaigns: vec![Campaign {
                name: "Test".into(),
                chapters: vec![Chapter {
                    name: "Chapter".into(),
                    map: map.clone(),
                    image: "maps/any".into(),
                }],
                original: false,
            }],
        };
        let s = c.selection(&SessionSettings {
            map,
            survivor: Survivor::Coach,
            difficulty: Difficulty::Expert,
            original: false,
        });
        assert_eq!((s.campaign, s.chapter), (0, 0));
        assert_eq!(s.survivor, Survivor::Coach);
        assert_eq!(s.difficulty, Difficulty::Expert);
    }
}
