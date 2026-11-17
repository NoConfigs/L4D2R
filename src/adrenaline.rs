//! Adrenaline shot presentation for the survivor player (retail
//! adrenaline_duration 15 s; speed/heal values live in game.rs and
//! survivor_items.rs):
//! - client.dll FUN_1025d1c0 attaches the screen-space `screen_adrenaline`
//!   particle system (particles/screen_fx.pcf) while adrenaline is active:
//!   a ring of dark-red additive glow sprites (radius 75 at distance 125,
//!   colour 69-111/0/0, alpha 14) plus a child ring of red spray particles
//!   (colour 48-66/0/0, alpha 64-72, oscillating alpha) drifting outward,
//!   each fading in/out over 0.5 s. It is reproduced as a red edge vignette.
//! - client.dll FUN_1024a630 sets the `adrenalineLayer` sound-mixer layer
//!   from the networked `m_fNVAdrenaline` amount. scripts/soundmixers.txt
//!   raises the infected/special/Tank/Witch mix groups (volume 2-3) and
//!   solos them at 0.36, which pulls everything else down: the thin,
//!   infected-focused sound. Its per-group DSP column is not reproduced.
use bevy::{
    asset::RenderAssetUsages,
    audio::{AudioSink, AudioSinkPlayback, SpatialAudioSink, Volume},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::collections::HashMap;

/// Fade in/out time of both screen_adrenaline systems.
const FADE: f32 = 0.5;
/// Non-layer mix groups while the layer is fully active (solo 0.36).
const SOLO_DUCK: f32 = 0.64;

/// 0..1 strength of the adrenaline presentation (the networked amount).
#[derive(Resource, Default)]
pub struct Adrenaline {
    pub weight: f32,
    applied: f32,
    clock: f32,
}
#[derive(Component)]
pub struct AdrenalineOverlay;

/// Retail mix-group rules (path substring, in file order) and the
/// adrenalineLayer volume per group.
#[derive(Resource, Default)]
pub struct MixRules {
    groups: Vec<(String, String)>,
    layer: HashMap<String, f32>,
    /// MixGroups rules without a classname filter, in file order:
    /// (group, path substring, sndlvl min, sndlvl max).
    rules: Vec<(String, String, Option<f32>, Option<f32>)>,
    /// Default_Mix group volumes.
    mix: HashMap<String, f32>,
}
impl MixRules {
    /// Parses scripts/soundmixers.txt. Only groups matched by sound path
    /// alone are used (classname rules such as `localPlayer` need the
    /// emitting entity).
    pub fn parse(text: &str) -> Self {
        // Quoted fields of each line inside the named top-level block.
        fn rows<'a>(text: &'a str, name: &str) -> Vec<Vec<&'a str>> {
            let mut out = vec![];
            let mut depth = 0;
            let mut inside = false;
            let mut armed = false;
            for line in text.lines() {
                let line = line.split("//").next().unwrap_or("");
                let fields: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
                if !inside && fields.first() == Some(&name) {
                    armed = true;
                }
                for c in line.chars() {
                    match c {
                        '{' => {
                            depth += 1;
                            if armed && depth == 1 {
                                inside = true;
                                armed = false;
                            }
                        }
                        '}' => {
                            depth -= 1;
                            if depth == 0 && inside {
                                return out;
                            }
                        }
                        _ => {}
                    }
                }
                if inside && depth == 1 && !line.contains('{') && fields.len() >= 2 {
                    out.push(fields);
                }
            }
            out
        }
        // A block nested inside another (adrenalineLayer inside MixLayers):
        // parse it as a top-level block of the outer block's body.
        fn nested<'a>(text: &'a str, outer: &str, name: &str) -> Vec<Vec<&'a str>> {
            let Some(start) = text.find(&format!("\"{outer}\"")) else {
                return vec![];
            };
            let body = &text[start..];
            let open = body.find('{').map_or(body.len(), |i| i + 1);
            rows(&body[open..], name)
        }
        let mut rules = Self::default();
        // group, path substring, classname substring, channel, sndlvl
        // min/max, priority, ducked, causes ducking, duck percent, threshold.
        for row in rows(text, "MixGroups") {
            if row.len() >= 3 && row[2].is_empty() {
                let level = |i: usize| row.get(i).and_then(|v| v.trim().parse::<f32>().ok());
                rules.rules.push((row[0].to_string(), row[1].to_ascii_lowercase(), level(4), level(5)));
            }
            if row.len() >= 3 && row[2].is_empty() && !row[1].is_empty() {
                rules
                    .groups
                    .push((row[0].to_string(), row[1].to_ascii_lowercase()));
            }
        }
        // group, volume, level, dsp, solo, mute.
        for row in nested(text, "SoundMixers", "Default_Mix") {
            if let Some(volume) = row.get(1).and_then(|v| v.parse::<f32>().ok()) {
                rules.mix.insert(row[0].to_string(), volume.max(0.));
            }
        }
        for row in nested(text, "MixLayers", "adrenalineLayer") {
            if let Some(volume) = row.get(1).and_then(|v| v.parse::<f32>().ok()) {
                rules.layer.insert(row[0].to_string(), volume);
            }
        }
        rules
    }
    /// Default_Mix volume for a sound file (engine 1002fbd0): the sound's
    /// mix groups are its matching MixGroups rules in file order, and the
    /// first group the mixer defines sets the volume ("All" is last, 0.7).
    /// Rules filtered by the source's classname (localPlayer, Player) are
    /// not applied: sounds carry no source entity here.
    pub fn mix_volume(&self, path: &str, soundlevel: f32) -> f32 {
        let path = path.to_ascii_lowercase().replace('\\', "/");
        let path = path.strip_prefix("sound/").unwrap_or(&path);
        self.rules
            .iter()
            .filter(|(_, sub, lo, hi)| {
                !sub.starts_with('?')
                    && path.contains(sub.as_str())
                    && lo.is_none_or(|l| soundlevel >= l)
                    && hi.is_none_or(|h| soundlevel <= h)
            })
            .find_map(|(group, ..)| self.mix.get(group).copied())
            .unwrap_or(1.)
    }
    /// The adrenalineLayer volume for a sound file, or `None` when its mix
    /// group is not in the layer (then it is ducked by the solo).
    pub fn layer_volume(&self, path: &str) -> Option<f32> {
        let path = path.to_ascii_lowercase().replace('\\', "/");
        let group = self
            .groups
            .iter()
            .find(|(_, substring)| path.contains(substring.as_str()))?;
        self.layer.get(&group.0).copied()
    }
}
/// Gain of a sound under the layer at strength `w`.
pub fn mix_gain(layer_volume: Option<f32>, w: f32) -> f32 {
    match layer_volume {
        Some(v) => 1. + (v - 1.) * w,
        None => 1. - (1. - SOLO_DUCK) * w,
    }
}
/// Base volume and adrenaline-layer volume of a playing sound.
#[derive(Component)]
pub struct Mixed {
    pub base: f32,
    pub layer: Option<f32>,
}
/// An extra live gain on a mixed sound (the client's zombat2/zombat3 music
/// layer volumes, survivor_music::zombat_layers).
#[derive(Component, Clone, Copy, PartialEq)]
pub struct LayerGain(pub f32);

pub fn install(app: &mut App) {
    app.init_resource::<Adrenaline>()
        .add_systems(Startup, load_rules)
        .add_systems(Update, (fade, overlay, mix).chain());
}
fn load_rules(mut commands: Commands, source: Option<Res<crate::game_ui::HudSource>>) {
    let rules = source
        .and_then(|s| crate::source_assets::SourceAssets::new(&s.0, vec![]).ok())
        .and_then(|mut a| a.read("scripts/soundmixers.txt").ok())
        .map(|b| MixRules::parse(&String::from_utf8_lossy(&b)))
        .unwrap_or_default();
    println!(
        "Adrenaline mix: {} path groups, {} layer groups",
        rules.groups.len(),
        rules.layer.len()
    );
    commands.insert_resource(rules);
}
fn fade(time: Res<Time>, state: Option<Res<crate::game_state::Match>>, mut fx: ResMut<Adrenaline>) {
    let active = state
        .as_ref()
        .is_some_and(|s| !s.infected && !s.dead && s.adrenaline_time > 0.);
    let paused = state.as_ref().is_some_and(|s| s.paused);
    let dt = if paused { 0. } else { time.delta_secs() };
    let target = if active { 1. } else { 0. };
    fx.weight = if fx.weight < target {
        (fx.weight + dt / FADE).min(target)
    } else {
        (fx.weight - dt / FADE).max(target)
    };
    fx.clock += dt;
}
/// Red edge vignette texture for the overlay (built once per HUD).
fn vignette(images: &mut Assets<Image>) -> Handle<Image> {
    const SIZE: u32 = 256;
    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let mut seed = 0x2545_f491_u32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = (x as f32 + 0.5) / SIZE as f32 * 2. - 1.;
            let dy = (y as f32 + 0.5) / SIZE as f32 * 2. - 1.;
            let d = (dx * dx + dy * dy).sqrt();
            // Glow ring near the screen edge (radius 75 at distance 125).
            let ring = ((d - 0.62) / 0.55).clamp(0., 1.);
            let ring = ring * ring * (3. - 2. * ring);
            // Spray speckle: the child system's 200 small sprites.
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let speckle = (seed % 1000) as f32 / 1000.;
            let spray = (((d - 0.8) / 0.3).clamp(0., 1.)) * (0.6 + 0.4 * speckle);
            let alpha = (ring * 0.55 + spray * 0.25).clamp(0., 0.8);
            let red = 70. + 40. * speckle;
            data.extend_from_slice(&[red as u8, 0, 0, (alpha * 255.) as u8]);
        }
    }
    images.add(Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    ))
}
pub fn spawn_overlay(commands: &mut Commands, images: &mut Assets<Image>) {
    commands.spawn((
        ImageNode {
            color: Color::srgba(1., 1., 1., 0.),
            image_mode: bevy::ui::widget::NodeImageMode::Stretch,
            ..ImageNode::new(vignette(images))
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.),
            top: Val::Px(0.),
            width: Val::Percent(100.),
            height: Val::Percent(100.),
            ..default()
        },
        GlobalZIndex(-5),
        Visibility::Hidden,
        AdrenalineOverlay,
    ));
}
fn overlay(
    fx: Res<Adrenaline>,
    mut overlays: Query<(&mut ImageNode, &mut Visibility), With<AdrenalineOverlay>>,
) {
    // The spray system's alpha oscillates (Oscillate Scalar rate -2).
    let pulse = 0.85 + 0.15 * (fx.clock * 2.).sin();
    for (mut image, mut visibility) in &mut overlays {
        visibility.set_if_neq(if fx.weight > 0. {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        let alpha = fx.weight * pulse;
        if (image.color.alpha() - alpha).abs() > 1e-3 {
            image.color = Color::srgba(1., 1., 1., alpha);
        }
    }
}
fn mix(
    mut fx: ResMut<Adrenaline>,
    mut sinks: Query<(
        &Mixed,
        Option<Ref<LayerGain>>,
        Option<&mut AudioSink>,
        Option<&mut SpatialAudioSink>,
    )>,
) {
    let w = fx.weight;
    let changed = (w - fx.applied).abs() > 1e-4;
    fx.applied = w;
    // Every sink when the weight moves; otherwise only sounds that just
    // started playing under an active layer, or whose layer gain moved.
    for (mixed, gain, sink, spatial) in &mut sinks {
        let gain_changed = gain.as_ref().is_some_and(|g| g.is_changed());
        if !changed && w == 0. && !gain_changed && gain.is_none() {
            continue;
        }
        let volume = Volume::Linear(mixed.base * mix_gain(mixed.layer, w) * gain.as_ref().map_or(1., |g| g.0));
        if let Some(mut sink) = sink {
            if changed || gain_changed || sink.is_added() {
                sink.set_volume(volume);
            }
        }
        if let Some(mut sink) = spatial {
            if changed || gain_changed || sink.is_added() {
                sink.set_volume(volume);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn soundmixer_rules_put_infected_in_the_layer_and_duck_the_rest() {
        let text = r#"
"MixGroups"
{
	"localSurvivorGunFire"  "gunfire" "localPlayer" "" "" "" "75" "0" "0" "100" "40"
	"InfectedFeetRun"	"player/footsteps/infected/run" "" "" "" "" "25" "0" "0" "75" "40"
	"Weapons"	"weapons/" "" "" "" "" "50" "0" "0" "100" "40"
}
"MixLayers"
{
	"adrenalineLayer"
	{
	    "InfectedFeetRun"        "3" "1.0" "3.0" "0.36" "0.0"
	}
}
"#;
        let rules = MixRules::parse(text);
        assert_eq!(rules.groups.len(), 2, "classname rules are skipped");
        let mixed = MixRules::parse(r#"
"MixGroups"
{
	"localSurvivorGunFire"  "gunfire" "localPlayer" "" "" "" "75" "0" "0" "100" "40"
	"Explosions"  "explo" "" "" "120" "" "50" "0" "0" "100" "40"
	"Ambient"  "ambient" "" "" "" "" "45" "0" "0" "10" "40"
	"Undefined"  "weapons" "" "" "" "" "45" "0" "0" "10" "40"
	"All"  "" "" "" "" "" "50" "0" "0" "100" "40"
}
"SoundMixers"
{
	"Default_Mix"
	{
		"Ambient"   "0.55" "1.0" "1.0"
		"Explosions" "1.0" "1.0" "1.0"
		"All"       "0.7"  "1.0" "1.0"
	}
}
"#);
        assert_eq!(mixed.mix_volume("sound/ambient/fire/fire_big.wav", 75.), 0.55);
        assert_eq!(mixed.mix_volume("weapons/explode3.wav", 140.), 1.);
        assert_eq!(mixed.mix_volume("weapons/explode3.wav", 75.), 0.7, "below the Explosions sndlvl_min");
        assert_eq!(mixed.mix_volume("weapons/rifle/fire.wav", 75.), 0.7, "an undefined group falls through to All");
        assert_eq!(
            rules.layer_volume("player/footsteps/infected/run/concrete1.wav"),
            Some(3.)
        );
        assert_eq!(
            rules.layer_volume("weapons/rifle/gunfire/rifle_fire_1.wav"),
            None
        );
        assert_eq!(mix_gain(Some(3.), 1.), 3.);
        assert_eq!(mix_gain(Some(3.), 0.), 1.);
        assert!((mix_gain(None, 1.) - 0.64).abs() < 1e-6);
        assert_eq!(mix_gain(None, 0.), 1.);
    }
}
