//! Observed glow controls from the owned L4D2 installation. Context policies
//! are implemented separately; retaining a value does not implement that mode.
use bevy::prelude::*;
use std::collections::BTreeMap;
#[derive(Resource, Clone)]
pub struct GlowSettings {
    pub values: BTreeMap<String, f32>,
}
impl Default for GlowSettings {
    fn default() -> Self {
        let mut values = BTreeMap::new();
        for line in include_str!("../assets/glow-settings.tsv")
            .lines()
            .filter(|s| !s.starts_with('#'))
        {
            if let Some((name, value)) = line.split_once('\t') {
                if let Ok(value) = value.parse::<f32>() {
                    values.insert(name.into(), value);
                }
            }
        }
        // Native accessibility selector also controls the infected health
        // and bile glow palettes (2 enables the alternate palette).
        values.insert("cl_colorblind".into(), 0.);
        Self { values }
    }
}
impl GlowSettings {
    pub fn get(&self, name: &str, fallback: f32) -> f32 {
        self.values.get(name).copied().unwrap_or(fallback)
    }
    pub fn color(&self, prefix: &str) -> Color {
        Color::srgb(
            self.get(&format!("{prefix}_r"), 0.3).clamp(0., 1.),
            self.get(&format!("{prefix}_g"), 0.4).clamp(0., 1.),
            self.get(&format!("{prefix}_b"), 1.).clamp(0., 1.),
        )
    }
    pub fn command(&mut self, text: &str) -> Option<String> {
        let args = text.split_whitespace().collect::<Vec<_>>();
        let name = *args.first()?;
        if name == "glow_settings" {
            return Some(
                self.values
                    .iter()
                    .map(|(k, v)| format!("{k} {v}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        let current = *self.values.get(name)?;
        if args.len() == 1 {
            return Some(format!("{name} = {current}"));
        }
        let value = args[1].parse::<f32>().ok().filter(|v| v.is_finite());
        let Some(value) = value else {
            return Some("Glow value must be a finite number".into());
        };
        if args.len() != 2 {
            return Some(format!("Usage: {name} <number>"));
        }
        self.values.insert(name.into(), value);
        Some(format!("{name} = {value}"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovered_controls_are_queryable_editable_and_finite() {
        let mut settings = GlowSettings::default();
        assert!(settings.values.len() >= 80);
        assert_eq!(settings.color("cl_glow_item"), Color::srgb(0.7, 0.7, 1.));
        assert_eq!(settings.get("cl_glow_los_fade_in_time", 0.), 0.5);
        assert!(settings.command("cl_glow_brightness 2").is_some());
        assert_eq!(settings.get("cl_glow_brightness", 0.), 2.);
        settings.command("cl_glow_brightness NaN");
        assert_eq!(settings.get("cl_glow_brightness", 0.), 2.);
        assert!(settings.command("unrelated_command 1").is_none());
    }
}
