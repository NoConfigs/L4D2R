//! Low Violence, retail's German/`-lv` mode, as an option.
//!
//! Retail switches it on with the `-lv` launch option or the Steam region
//! (engine IsLowViolence), or when any of violence_hblood / violence_hgibs /
//! violence_ablood / violence_agibs is 0 (server 1020d300, client 101ad240).
//! Retail has no menu entry; the rewrite adds one under VIDEO > ADVANCED.
//!
//! What it changes in retail, and here:
//! - No wounds or dismemberment on commons: SetWound (server 104034b0) returns
//!   at once, so neither cutouts nor their gibs appear (infected_gore).
//! - No fatal-blast gibbing (10405d90): blasts ragdoll the body whole; with
//!   no wounds that is what the rewrite does too.
//! - No blood: the client blood sprays/drops (100f1940, 100f2980, 1022f380)
//!   and the headshot burst (server 10404c10) are skipped (bullet_impacts).
//! - No Boomer limb chunks (client 10224210 spawns exploded_boomer_* only
//!   without it).
//! - No lying-down commons: InfectedWander's lie check (10419320) fails, and
//!   InfectedLieDown (10417c80) ends with "Low-violence mode enabled".
//! Not reproduced: client effects whose call sites carry no identifying
//! strings (about twenty, e.g. 10313560 on the common's entity).
use std::sync::atomic::{AtomicBool, Ordering};

static ON: AtomicBool = AtomicBool::new(false);
static LOADED: std::sync::Once = std::sync::Once::new();
const FILE: &str = "runtime/game-options.ini";

/// True while Low Violence is on.
pub fn on() -> bool {
    LOADED.call_once(|| {
        let saved = std::fs::read_to_string(FILE).unwrap_or_default();
        let enabled = std::env::args().any(|a| a == "-lv" || a == "--low-violence")
            || saved.lines().any(|l| l.trim() == "low_violence=true");
        ON.store(enabled, Ordering::Relaxed);
    });
    ON.load(Ordering::Relaxed)
}

/// Flips the option and saves it.
pub fn toggle() -> bool {
    let next = !on();
    ON.store(next, Ordering::Relaxed);
    let _ = std::fs::create_dir_all("runtime");
    if let Err(error) = std::fs::write(FILE, format!("low_violence={next}\n")) {
        eprintln!("Could not save game options: {error}");
    }
    next
}

/// The menu label: "LOW VIOLENCE: ON/OFF".
pub fn label() -> String {
    format!("LOW VIOLENCE: {}", if on() { "ON" } else { "OFF" })
}

/// Marks the menu button's text so it follows the setting.
#[derive(bevy::prelude::Component)]
pub struct Label;

pub fn update_labels(mut labels: bevy::prelude::Query<&mut bevy::prelude::Text, bevy::prelude::With<Label>>) {
    let text = label();
    for mut label in &mut labels {
        if label.0 != text {
            label.0 = text.clone();
        }
    }
}
