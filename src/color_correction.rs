//! Retail colour correction (color_correction entities and .raw lookups).
//!
//! Every campaign map grades its image with a 32x32x32 RGB lookup
//! (materials/correction/*.raw). The server (CBasePlayer::UpdateFXVolume)
//! gives each player one colour correction: the one named by the enabled
//! fog_volume containing the view point (ColorCorrectionName), else the
//! master (spawnflags 1) when the map has enabled fog volumes. On the
//! client (10271530) a change fades the old one out and the new one in
//! (EnableOnClient, 1006dbc0): the fade lasts fadeOut/fadeInDuration
//! scaled by the part of the weight still to travel, and the first one
//! appears at once. C_ColorCorrection::Update (1006dd20) then weights each
//! lookup by its current weight x (1 - distance falloff, used only when
//! min/maxfalloff are both set and differ). The manager (100db3c0) scales
//! non-exclusive weights by (1 - the exclusive weight); the material system
//! normalises a total above 1 and gives the rest to the unmodified image.
//! engine_post applies the blended lookup to the gamma-space frame after
//! local contrast (engine_post.rs).
use bevy::prelude::*;
use std::collections::HashMap;

const SIZE: usize = 32;

/// One colour correction's client-side fade (per player).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Fade {
    enabled: bool,
    weight: f32,
    from: f32,
    start: f32,
    duration: f32,
}
impl Fade {
    fn target(&self, max: f32) -> f32 {
        if self.enabled { max } else { 0. }
    }
    fn update(&mut self, max: f32, now: f32) {
        let t = if self.duration != 0. { ((now - self.start) / self.duration).clamp(0., 1.) } else { 1. };
        self.weight = (self.target(max) - self.from) * t + self.from;
    }
    /// EnableOnClient (client 1006dbc0).
    fn enable(&mut self, on: bool, instant: bool, max: f32, fade_in: f32, fade_out: f32, now: f32) {
        if self.enabled == on {
            return;
        }
        self.enabled = on;
        self.from = self.weight;
        let mut remaining = if max != 0. { self.weight / max } else { 1. };
        if on {
            remaining = 1. - remaining;
        }
        if instant {
            remaining = 0.;
        }
        self.duration = ((if on { fade_in } else { fade_out }) * remaining).max(0.);
        self.start = now;
        self.update(max, now);
    }
}

#[derive(Clone, Debug)]
struct Entry {
    id: usize,
    file: String,
    max: f32,
    fade_in: f32,
    fade_out: f32,
    min_falloff: f32,
    max_falloff: f32,
    origin: Vec3,
    master: bool,
    client_side: bool,
    exclusive: bool,
    enabled: bool,
}

#[derive(Resource, Default)]
pub(crate) struct ColorCorrection {
    map: usize,
    entries: Vec<Entry>,
    fades: HashMap<usize, Fade>,
    current: Option<usize>,
    /// Client lookups (10271200): infected, third strike, ghost weights.
    client: [f32; 3],
    /// Each lookup's 32^3 RGB bytes.
    raw: HashMap<String, Option<Vec<u8>>>,
    applied: Vec<(String, u16)>,
    assets: Option<crate::source_assets::SourceAssets>,
}

/// Weighted lookups: (file, weight), after exclusivity.
fn weights(list: &[(String, f32, bool)]) -> Vec<(String, f32)> {
    let exclusive = list.iter().find(|(_, w, e)| *e && *w != 0.).map_or(0., |(_, w, _)| *w);
    list.iter()
        .map(|(file, w, e)| (file.clone(), if *e { *w } else { (1. - exclusive) * w }))
        .filter(|(_, w)| *w > 0.)
        .collect()
}

/// The weighted lookups as one normalised 32^3 table and its total weight
/// (at most 1; the rest is the unmodified colour).
fn blend(parts: &[(&[u8], f32)]) -> (Vec<u8>, f32) {
    let total: f32 = parts.iter().map(|p| p.1).sum();
    let n = SIZE * SIZE * SIZE * 3;
    if total <= 0. {
        return (identity_table(), 0.);
    }
    let mut out = vec![0u8; n];
    for (i, o) in out.iter_mut().enumerate() {
        let v: f32 = parts.iter().map(|(raw, w)| raw[i] as f32 * w).sum::<f32>() / total;
        *o = v.round().clamp(0., 255.) as u8;
    }
    (out, total.min(1.))
}
fn identity_table() -> Vec<u8> {
    let mut raw = Vec::with_capacity(SIZE * SIZE * SIZE * 3);
    for b in 0..SIZE {
        for g in 0..SIZE {
            for r in 0..SIZE {
                raw.extend([r, g, b].map(|v| ((v * 255) as f32 / (SIZE - 1) as f32).round() as u8));
            }
        }
    }
    raw
}
/// A 32^3 RGBA8 volume (red fastest), filtered like retail's lookup.
fn lookup_image(rgb: Vec<u8>) -> Image {
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let data: Vec<u8> = rgb.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect();
    let mut image = Image::new(
        Extent3d { width: SIZE as u32, height: SIZE as u32, depth_or_array_layers: SIZE as u32 },
        TextureDimension::D3,
        data,
        TextureFormat::Rgba8Unorm,
        bevy::asset::RenderAssetUsages::default(),
    );
    image.sampler = bevy::image::ImageSampler::linear();
    image
}

fn read(world: &l4d2r_game_core::entities::EntityWorld) -> Vec<Entry> {
    world
        .ents
        .iter()
        .enumerate()
        .filter(|(_, e)| e.alive && e.me.classname == "color_correction")
        .map(|(id, e)| {
            let o = e.me.origin;
            Entry {
                id,
                file: e.me.kv("filename").unwrap_or("").replace('\\', "/").to_ascii_lowercase(),
                max: e.me.kv_f32("maxweight").unwrap_or(1.),
                fade_in: e.me.kv_f32("fadeInDuration").unwrap_or(0.),
                fade_out: e.me.kv_f32("fadeOutDuration").unwrap_or(0.),
                min_falloff: e.me.kv_f32("minfalloff").unwrap_or(0.),
                max_falloff: e.me.kv_f32("maxfalloff").unwrap_or(1000.),
                origin: Vec3::new(o.x, o.z, -o.y) * crate::collision::UNIT,
                master: e.me.spawnflags() & 1 != 0,
                client_side: e.me.spawnflags() & 2 != 0,
                exclusive: e.me.kv_i32("exclusive").unwrap_or(0) != 0,
                enabled: e.me.kv_i32("StartDisabled") != Some(1),
            }
        })
        .collect()
}

pub(crate) fn sync(
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    game: Option<Res<crate::game_state::Match>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    mut commands: Commands,
    lookup: Option<ResMut<crate::engine_post::CcLookup>>,
    mut images: ResMut<Assets<Image>>,
    hunter: Option<Res<crate::special_infected::Hunter>>,
    mut state: Local<ColorCorrection>,
    camera: Query<(&GlobalTransform, &crate::game::Body), With<crate::viewer::GameCamera>>,
) {
    let Some(logic) = logic else {
        return;
    };
    let Some(mut lookup) = lookup else {
        let image = images.add(lookup_image(identity_table()));
        commands.insert_resource(crate::engine_post::CcLookup { image, weight: 0. });
        return;
    };
    // L4D2R_NO_CC: leave the image ungraded (comparisons).
    if std::env::var_os("L4D2R_NO_CC").is_some() {
        lookup.weight = 0.;
        return;
    }
    let world = &logic.world;
    let map = std::sync::Arc::as_ptr(&world.bsp) as usize;
    if state.map != map {
        let assets = state.assets.take();
        let raw = std::mem::take(&mut state.raw);
        *state = ColorCorrection { map, entries: read(world), assets, raw, ..default() };
    }
    let now = world.time;
    let Ok((transform, body)) = camera.single() else {
        return;
    };
    let (incap, dead) = game.as_ref().map_or((false, false), |g| (g.incap, g.dead));
    let (point, _) = crate::native_fog::view_point(body, incap, world.cine.view_control.is_some() || dead, transform);
    // UpdateFXVolume: the first enabled fog volume containing the view point.
    let by_name = |name: &str| state.entries.iter().find(|e| world.ents[e.id].me.targetname.eq_ignore_ascii_case(name)).map(|e| e.id);
    let master = state.entries.iter().find(|e| e.master).map(|e| e.id);
    let inside = world.fog.volumes.iter().copied().find(|&id| crate::native_fog::Runtime::contains(world, id, point));
    let chosen = match inside {
        Some(volume) => world.ents[volume].me.kv("ColorCorrectionName").and_then(by_name).or(master),
        None if !world.fog.volumes.is_empty() => master,
        None => state.current,
    };
    if chosen != state.current {
        let previous = state.current;
        for entry in state.entries.clone() {
            let fade = state.fades.entry(entry.id).or_default();
            if Some(entry.id) == previous {
                fade.enable(false, false, entry.max, entry.fade_in, entry.fade_out, now);
            }
            if Some(entry.id) == chosen {
                fade.enable(true, previous.is_none(), entry.max, entry.fade_in, entry.fade_out, now);
            }
        }
        state.current = chosen;
    }
    let mut list = Vec::new();
    for entry in state.entries.clone() {
        let fade = state.fades.entry(entry.id).or_default();
        let (on, weight) = if entry.client_side {
            fade.update(entry.max, now);
            (fade.enabled, fade.weight)
        } else {
            (entry.enabled, if entry.enabled { entry.max } else { 0. })
        };
        if !on && weight == 0. {
            continue;
        }
        let mut falloff = 0.;
        if entry.min_falloff != -1. && entry.max_falloff != -1. && entry.min_falloff != entry.max_falloff {
            let d = point.distance(entry.origin) / crate::collision::UNIT;
            falloff = ((d - entry.min_falloff) / (entry.max_falloff - entry.min_falloff)).clamp(0., 1.);
        }
        list.push((entry.file.clone(), (1. - falloff) * weight, entry.exclusive));
    }
    // Client-side lookups (10271200, loaded by 10272410): each weight
    // steps 0.1 per frame toward 1 while its condition holds, else toward
    // 0. Third strike: a living survivor on the third strike; infected and
    // ghost: an infected player, alive or as a ghost.
    let ghost = hunter.as_ref().is_some_and(|h| h.ghost);
    let conditions = game.as_ref().map_or([false; 3], |g| {
        [
            g.infected && !g.dead && !ghost,
            !g.infected && !g.dead && g.black_white,
            g.infected && !g.dead && ghost,
        ]
    });
    const CLIENT: [&str; 3] = [
        "materials/correction/infected.raw",
        "materials/correction/thirdstrike.raw",
        "materials/correction/ghost.raw",
    ];
    for i in 0..3 {
        let step = if conditions[i] { 0.1 } else { -0.1 };
        state.client[i] = (state.client[i] + step).clamp(0., 1.);
        if state.client[i] > 0. {
            list.push((CLIENT[i].into(), state.client[i], false));
        }
    }
    let weighted = weights(&list);
    // Rebuild only when a weight moves by more than a step of 8-bit colour.
    let key: Vec<(String, u16)> = weighted.iter().map(|(f, w)| (f.clone(), (w * 1024.).round() as u16)).collect();
    if key == state.applied {
        return;
    }
    for (file, _) in &weighted {
        if !state.raw.contains_key(file) {
            if state.assets.is_none() {
                state.assets = source.as_ref().and_then(|s| crate::source_assets::SourceAssets::new(&s.0, vec![]).ok());
            }
            let raw = state.assets.as_mut().and_then(|a| a.read(file).ok()).filter(|r| r.len() >= SIZE * SIZE * SIZE * 3);
            if raw.is_none() {
                eprintln!("Colour correction lookup missing: {file}");
            }
            state.raw.insert(file.clone(), raw);
        }
    }
    let parts: Vec<(&[u8], f32)> = weighted
        .iter()
        .filter_map(|(f, w)| state.raw.get(f).and_then(|g| g.as_deref()).map(|g| (g, *w)))
        .collect();
    // engine_post blends the lookups with the unmodified colour (the
    // material system normalises a total above 1); the pass gets the
    // normalised blend and its weight.
    let (texture, weight) = blend(&parts);
    let _ = images.insert(lookup.image.id(), lookup_image(texture));
    lookup.weight = weight;
    if std::env::var_os("L4D2R_CC_LOG").is_some() {
        eprintln!("L4D2R_CC t={now:.2} {:?}", key);
    }
    state.applied = key;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Vec<u8> {
        let mut raw = Vec::new();
        for b in 0..SIZE {
            for g in 0..SIZE {
                for r in 0..SIZE {
                    raw.extend([r, g, b].map(|v| (v * 255 / (SIZE - 1)) as u8));
                }
            }
        }
        raw
    }

    #[test]
    fn enabling_fades_by_the_remaining_weight() {
        let mut f = Fade::default();
        f.enable(true, true, 1., 2., 2., 0.);
        assert_eq!(f.weight, 1.);
        f.enable(false, false, 1., 2., 2., 10.);
        f.update(1., 11.);
        assert!((f.weight - 0.5).abs() < 1e-6);
        // Re-enabled half way: fades back in over the remaining second.
        f.enable(true, false, 1., 2., 2., 11.);
        assert!((f.duration - 1.).abs() < 1e-6);
        f.update(1., 11.5);
        assert!((f.weight - 0.75).abs() < 1e-6);
    }

    #[test]
    fn exclusive_scales_the_others() {
        let w = weights(&[("a".into(), 1., false), ("b".into(), 0.5, true)]);
        assert_eq!(w, vec![("a".into(), 0.5), ("b".into(), 0.5)]);
    }

    #[test]
    fn blends_normalise_above_one_and_keep_the_rest_unmodified() {
        let a = vec![200u8; SIZE * SIZE * SIZE * 3];
        let b = vec![100u8; SIZE * SIZE * SIZE * 3];
        let (t, w) = blend(&[(&a, 0.5)]);
        assert_eq!((t[0], w), (200, 0.5));
        let (t, w) = blend(&[(&a, 1.), (&b, 1.)]);
        assert_eq!((t[0], w), (150, 1.));
        assert_eq!(blend(&[]).1, 0.);
    }
    #[test]
    fn retail_lookups_load() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for file in ["materials/correction/cc_c1_main.raw", "materials/correction/cc_checkpoint.raw", "materials/correction/off.raw", "materials/correction/thirdstrike.raw", "materials/correction/infected.raw", "materials/correction/ghost.raw"] {
            let raw = assets.read(file).unwrap();
            assert_eq!(raw.len(), SIZE * SIZE * SIZE * 3, "{file}");
        }
        // off.raw is the identity.
        let off = assets.read("materials/correction/off.raw").unwrap();
        let worst = off.iter().zip(identity()).map(|(a, b)| (*a as i32 - b as i32).abs()).max().unwrap();
        assert!(worst <= 2, "{worst}");
    }
}
