//! Native sound-script registry. All manifest definitions are available on demand.
use crate::{kv, source_assets::SourceAssets};
use std::{collections::HashMap, io};
#[derive(Clone, Debug)]
pub struct Definition {
    pub waves: Vec<String>,
    pub volume: f32,
    pub pitch: f32,
    pub volume_range: [f32;2],
    pub pitch_range: [f32;2],
    pub delay: f32,
    pub global: bool,
    /// Source sound level in dB (SNDLVL_NORM 75 when unset; 0 = none).
    pub soundlevel: f32,
    /// A `#` wave prefix: bypasses room DSP.
    pub dry: bool,
}
/// A soundscript `soundlevel` value: SNDLVL_<n>dB, a named level or a
/// number; a "a,b" range uses its midpoint.
pub fn soundlevel(value: &str) -> Option<f32> {
    let levels: Vec<f32> = value
        .split(',')
        .filter_map(|v| {
            let v = v.trim().to_ascii_uppercase();
            Some(match v.as_str() {
                "SNDLVL_NONE" => 0.,
                "SNDLVL_IDLE" | "SNDLVL_TALKING" => 60.,
                "SNDLVL_STATIC" => 66.,
                "SNDLVL_NORM" => 75.,
                "SNDLVL_GUNFIRE" => 140.,
                _ => v
                    .strip_prefix("SNDLVL_")
                    .map(|n| n.trim_end_matches("DB"))
                    .unwrap_or(&v)
                    .parse::<f32>()
                    .ok()?,
            })
        })
        .collect();
    (!levels.is_empty()).then(|| levels.iter().sum::<f32>() / levels.len() as f32)
}
/// A soundscript `attenuation` value as a sound level
/// (ATTN_TO_SNDLVL: 50 + 20 / attenuation, 0 for none).
pub fn attenuation_soundlevel(value: &str) -> Option<f32> {
    let a = match value.trim().to_ascii_uppercase().as_str() {
        "ATTN_NONE" => 0.,
        "ATTN_NORM" => 0.8,
        "ATTN_IDLE" => 2.,
        "ATTN_STATIC" => 1.25,
        "ATTN_RICOCHET" => 1.5,
        "ATTN_GUNFIRE" => 0.27,
        v => v.split(',').next()?.trim().parse().ok()?,
    };
    Some(if a > 0. { (50. + 20. / a as f32).trunc() } else { 0. })
}
/// engine.dll FastPow10 (1027bcb0): the bit-trick 10^x the mixer uses.
fn fast_pow10(x: f32) -> f32 {
    let x = x * 3.321_928;
    let f = x - x.floor();
    let bits = ((x + 127. - (f - f * f) * 0.339_71) * 8_388_608.) as i32;
    f32::from_bits(bits as u32)
}
/// Distance gain of a sound at `soundlevel` dB heard `distance` Source
/// units away: engine.dll SND_GetGainFromSoundLevel (101ed280) and
/// SND_GetGainFromMult (101ed0d0) with retail defaults snd_gain 1,
/// snd_refdb 60, snd_refdist 36, snd_gain_max 1, snd_gain_min 0.01 and
/// snd_foliage_db_loss 4 (an extra 4 dB per 1200 units).
pub fn distance_gain(soundlevel: f32, distance: f32) -> f32 {
    const REF_DB: f32 = 60.;
    const REF_DIST: f32 = 36.;
    const FOLIAGE_DB_LOSS: f32 = 4.;
    const GAIN_MIN: f32 = 0.01;
    let level = soundlevel as i32;
    if level == 0 {
        return 1.;
    }
    let dist_mult = (1000f32 / fast_pow10(level as f32 * 0.05)) / REF_DIST;
    if dist_mult == 0. {
        return 1.;
    }
    debug_assert_eq!(10f32.powf(REF_DB / 20.), 1000.);
    let relative = distance * dist_mult * fast_pow10(distance * (1. / 1200.) * FOLIAGE_DB_LOSS * 0.05);
    let mut gain = if relative > 0.1 { 1. / relative } else { 10. };
    if gain > 0.5 {
        let db = (0.301_03 * (1000. / (dist_mult * REF_DIST)).log2() * 20.) as i32 as f32;
        let mut power = 2.5;
        if db > 90. {
            power = 2.5 - (db - 90.) * 1.7 * 0.02;
        }
        let y = -1. / (0.5f32.powf(power) * -0.5);
        gain = 1. - 1. / (y * gain.powf(power));
    }
    if gain < GAIN_MIN {
        gain = (2. - GAIN_MIN * relative) * GAIN_MIN;
        if gain <= 0. {
            gain = 0.001;
        }
    }
    gain
}
pub struct Registry {
    pub definitions: HashMap<String, Definition>,
    pub files: usize,
}
fn number(value: Option<&str>, fallback: f32) -> f32 {
    let values: Vec<f32> = value
        .unwrap_or("")
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .filter(|n| n.is_finite())
        .collect();
    if values.is_empty() {
        fallback
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}
fn interval(value:Option<&str>,fallback:f32,limits:[f32;2])->[f32;2] {
    let values:Vec<f32>=value.unwrap_or("").split(',').filter_map(|s|s.trim().parse::<f32>().ok()).filter(|v|v.is_finite()).collect();
    let a=values.first().copied().unwrap_or(fallback).clamp(limits[0],limits[1]);
    let b=values.get(1).copied().unwrap_or(a).clamp(limits[0],limits[1]);[a.min(b),a.max(b)]
}
#[derive(Default)]
pub struct Chooser {
    rng: l4d2r_game_core::random::UniformRandomStream,
    bags: HashMap<String,Vec<usize>>,
}
impl Chooser {
    pub fn choose(&mut self,name:&str,definition:&Definition)->Option<(String,Definition)> {
        if definition.waves.is_empty() {return None;}
        let bag=self.bags.entry(name.into()).or_default();
        if bag.is_empty() {bag.extend(0..definition.waves.len());}
        let index=self.rng.random_int(0,bag.len() as i32-1) as usize;
        let wave=bag.swap_remove(index);
        let mut sampled=definition.clone();
        sampled.volume=self.rng.random_float(sampled.volume_range[0],sampled.volume_range[1]);
        sampled.pitch=self.rng.random_float(sampled.pitch_range[0],sampled.pitch_range[1]);
        Some((definition.waves[wave].clone(),sampled))
    }
}
pub fn wave_path(value: &str) -> io::Result<String> {
    // Source's leading characters select streaming/spatial playback flags.
    let wave =
        value.trim_start_matches(['*', '#', '@', '>', '<', '^', '(', ')', '}', '$', '?', '!']);
    let path = crate::source_assets::normalize(wave)?;
    Ok(if path.starts_with("sound/") {
        path
    } else {
        format!("sound/{path}")
    })
}
pub fn parse(text: &str) -> HashMap<String, Definition> {
    let t = kv::tokens(text);
    let mut i = 0;
    let mut result = HashMap::new();
    while i + 1 < t.len() {
        let name = t[i].to_lowercase();
        i += 1;
        if t[i] != "{" {
            i += 1;
            continue;
        }
        i += 1;
        let start = i;
        let mut depth = 1;
        while i < t.len() && depth > 0 {
            if t[i] == "{" {
                depth += 1;
            } else if t[i] == "}" {
                depth -= 1;
            }
            i += 1;
        }
        if depth != 0 {
            break;
        }
        let block = &t[start..i.saturating_sub(1)];
        let value = |key: &str| {
            block
                .windows(2)
                .find(|p| p[0].eq_ignore_ascii_case(key))
                .map(|p| p[1].as_str())
        };
        let waves = block
            .windows(2)
            .filter(|p| p[0].eq_ignore_ascii_case("wave"))
            .filter_map(|p| wave_path(&p[1]).ok())
            .collect::<Vec<_>>();
        if !waves.is_empty() {
            result.insert(
                name,
                Definition {
                    waves,
                    volume: number(value("volume"), 1.).clamp(0., 1.),
                    pitch: number(value("pitch"), 100.).clamp(25., 255.) / 100.,
                    volume_range: interval(value("volume"),1.,[0.,1.]),
                    pitch_range: interval(value("pitch"),100.,[25.,255.]).map(|v|v/100.),
                    delay: number(value("delay_msec"),0.).max(0.)/1000.,
                    global: value("soundlevel").is_some_and(|v|v.eq_ignore_ascii_case("SNDLVL_NONE") || v=="0") || value("attenuation").is_some_and(|v|v.eq_ignore_ascii_case("ATTN_NONE") || v=="0"),
                    soundlevel: value("soundlevel")
                        .and_then(soundlevel)
                        .or_else(|| value("attenuation").and_then(attenuation_soundlevel))
                        .unwrap_or(75.),
                    dry: block
                        .windows(2)
                        .filter(|p| p[0].eq_ignore_ascii_case("wave"))
                        .any(|p| p[1].trim_start_matches(|c: char| "*@><^()}$?!".contains(c)).starts_with('#')),
                },
            );
        }
    }
    result
}
impl Registry {
    pub fn load(assets: &mut SourceAssets) -> io::Result<Self> {
        let manifest = assets.read("scripts/game_sounds_manifest.txt")?;
        let t = kv::tokens(&String::from_utf8_lossy(&manifest));
        let mut definitions = HashMap::new();
        let mut files = 0;
        for p in t
            .windows(2)
            .filter(|p| p[0] == "precache_file" || p[0] == "preload_file")
        {
            match assets.read(&p[1]) {
                Ok(b) => {
                    definitions.extend(parse(&String::from_utf8_lossy(&b)));
                    files += 1;
                }
                Err(e) => eprintln!("Sound manifest {}: {e}", p[1]),
            }
        }
        Ok(Self { definitions, files })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sound_ranges_sample_and_wave_bags_do_not_cycle_in_lockstep() {
        let defs=parse(r#"Test {volume "0.2,0.9" pitch "90,110" delay_msec 250 soundlevel SNDLVL_NONE rndwave {wave a.wav wave b.wav wave c.wav}}"#);
        let d=&defs["test"];assert_eq!(d.volume_range,[0.2,0.9]);assert_eq!(d.pitch_range,[0.9,1.1]);assert!(d.global);assert_eq!(d.delay,0.25);
        let mut chooser=Chooser::default();let mut volumes=std::collections::HashSet::new();
        for _ in 0..10 {
            let mut paths=std::collections::HashSet::new();
            for _ in 0..3 {let (path,d)=chooser.choose("test",d).unwrap();paths.insert(path);assert!((0.2..=0.9).contains(&d.volume));assert!((0.9..=1.1).contains(&d.pitch));volumes.insert(d.volume.to_bits());}
            assert_eq!(paths.len(),3,"every authored variant must be used before recycling");
        }
        assert!(volumes.len()>20,"do not average authored ranges");
    }
    #[test]
    fn native_variants_flags_and_ranges() {
        let defs = parse(
            r#""Pistol.Fire" { "volume" "0.7,0.9" "pitch" "97,103" "rndwave" { "wave" ")weapons/a.wav" "wave" "*weapons/b.wav" } } "Menu.Select" { "wave" "UI/click.wav" }"#,
        );
        assert_eq!(
            defs["pistol.fire"].waves,
            ["sound/weapons/a.wav", "sound/weapons/b.wav"]
        );
        assert!((defs["pistol.fire"].volume - 0.8).abs() < 0.001);
        assert_eq!(defs["menu.select"].waves[0], "sound/ui/click.wav");
        assert!(wave_path("*../../bad.wav").is_err());
        assert_eq!(
            wave_path("(animation/jets/jet.wav").unwrap(),
            "sound/animation/jets/jet.wav"
        );
    }
    #[test]
    fn soundlevels_and_engine_distance_gain() {
        assert_eq!(soundlevel("SNDLVL_95dB"), Some(95.));
        assert_eq!(soundlevel("SNDLVL_NORM"), Some(75.));
        assert_eq!(soundlevel("SNDLVL_70dB,SNDLVL_80dB"), Some(75.));
        assert_eq!(attenuation_soundlevel("ATTN_NORM"), Some(75.));
        assert_eq!(attenuation_soundlevel("ATTN_STATIC"), Some(66.));
        let defs = parse(r#"A { wave a.wav attenuation ATTN_IDLE } B { wave b.wav }"#);
        assert_eq!(defs["a"].soundlevel, 60.);
        assert_eq!(defs["b"].soundlevel, 75.);
        // Full volume up close, monotonic falloff, louder sounds carry further.
        assert!((distance_gain(75., 10.) - 1.).abs() < 0.05);
        let mut last = 2.;
        for d in [36., 200., 600., 1200., 3000., 8000.] {
            let g = distance_gain(75., d);
            assert!(g < last, "{d} {g}");
            last = g;
        }
        assert!(distance_gain(140., 3000.) > distance_gain(75., 3000.) * 10.);
        assert_eq!(distance_gain(0., 5000.), 1.);
        // SNDLVL_NORM: 1/relative distance in the mid range, with the
        // L4D 4 dB/1200 foliage loss (relative = 1000/10^3.75/36 * d * 10^(d/6000)).
        let d = 1200.;
        let expected = 1. / (d * (1000. / 10f32.powf(3.75)) / 36. * 10f32.powf(d / 6000.));
        assert!((distance_gain(75., d) - expected).abs() / expected < 0.02, "{} {expected}", distance_gain(75., d));
    }
    #[test]
    fn truncated_sound_block_is_ignored() {
        assert!(parse("Broken { wave bad.wav").is_empty());
        assert!(parse("Broken {").is_empty());
    }
}
