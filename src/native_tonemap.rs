//! Map-controlled exposure, metered the way L4D2's client does it
//! (client.dll, mat_tonemap_algorithm 1, the default): the tonemap scale puts
//! the luminance that the brightest `percent_bright_pixels` of the centre
//! region (mat_exposure_center_region 0.9 x 0.85) exceed at
//! `percent_target` of white, clamped to the map's auto-exposure bounds.
//! There is no tone curve: the scaled colour is clipped. Bevy's histogram
//! does the metering, averaging a narrow band around that percentile.
//! Not modelled: the min-average-luminance floor and Source's adaptation curve.
use bevy::{
    asset::RenderAssetUsages,
    post_process::auto_exposure::{AutoExposure, AutoExposureCompensationCurve},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use l4d2r_game_core::entities::Tonemap;

/// The world camera, metered with Source's rule. Other auto-exposure cameras
/// (the first-person view model) keep the mean-to-middle-grey metering.
#[derive(Component)]
pub(crate) struct SourceMetering;

/// Bevy's `Exposure { ev100: 0 }` multiplies every shader by 1/1.2 before
/// metering; auto exposure must undo it for the map bounds to mean the same
/// final scale as Source's.
const CAMERA_EXPOSURE: f32 = 1. / 1.2;
/// EV distance below which Bevy's adaptation is exponential (it always is
/// within the map's 1..5 exposure range).
const ADAPT_RANGE: f32 = 10.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bounds {
    pub min: f32,
    pub max: f32,
    pub rate: f32,
    /// Fraction of white the bright-pixel percentile is scaled to.
    pub target: f32,
    /// Fraction of metered pixels allowed above the target.
    pub bright: f32,
    /// Source's minimum average (median) luminance, a fraction
    /// (mat_tonemap_min_avglum 3% unless the controller sets one).
    pub min_average: f32,
}
impl Bounds {
    pub(crate) fn from_map(map: Tonemap) -> Self {
        let valid = |v: Option<f32>, fallback| {
            v.filter(|v| v.is_finite() && *v > 0.)
                .unwrap_or(fallback)
                .clamp(1. / 256., 256.)
        };
        let percent = |v: f32, fallback: f32, lo: f32, hi: f32| {
            if v.is_finite() && v > 0. { v } else { fallback }.clamp(lo, hi) / 100.
        };
        let min = valid(map.auto_min, 0.5);
        let max = valid(map.auto_max, 2.).max(min);
        Self {
            min,
            max,
            rate: if map.rate.is_finite() && map.rate > 0. {
                map.rate
            } else {
                1.
            },
            target: percent(map.percent_target, 45., 1., 100.),
            bright: percent(map.percent_bright_pixels, 1., 0.1, 50.),
            min_average: percent(map.min_avg_lum, 3., 0.01, 100.),
        }
    }
    /// Histogram band Bevy averages: about the (1 - bright) percentile.
    /// Retail exposure adaptation (client 101c53c0, mat_tonemap_algorithm
    /// 1): each frame the scale moves k x frametime of the way to the target,
    /// k = 2 x rate brightening and up to mat_accelerate_adjust_exposure_down
    /// (3) times that darkening, capped at 1/64 per frame. That is
    /// exponential in linear scale; Bevy adapts exponentially in EV, so the
    /// per-EV speed is matched at a 2 EV change (2^2 - 1 = 3 / 0.75 of the
    /// scale gap): Bevy's step is delta x speed / ADAPT_RANGE.
    pub(crate) fn adapt_speeds(self) -> (f32, f32) {
        const FRAME_CAP: f32 = 60. / 64.;
        let up = (2. * self.rate).min(FRAME_CAP);
        let down = (3. * 2. * self.rate).min(FRAME_CAP);
        let per_ev = |k: f32, gap: f32| k * gap / (std::f32::consts::LN_2 * 2.);
        (per_ev(up, 3.) * ADAPT_RANGE, per_ev(down, 0.75) * ADAPT_RANGE)
    }
    /// The EV a new world view starts at: Source's tonemap scale is reset to
    /// 1 on level start and clamped to the map's bounds, where Bevy would
    /// start at EV 0 (scale 1/1.2, below c1m1's minimum of 1).
    pub(crate) fn initial_ev(self) -> f32 {
        (1f32.clamp(self.min, self.max) / CAMERA_EXPOSURE).log2()
    }
    pub(crate) fn filter(self) -> std::ops::RangeInclusive<f32> {
        let p = 1. - self.bright;
        (p - 0.01).max(0.)..=(p + 0.01).min(1.)
    }
    fn knots(self, gray: f32, scale: f32) -> Vec<Vec2> {
        // Curve compensation minus measured log luminance is the final EV.
        // Quantized knots satisfy the spline builder's exact continuity check.
        // Bevy bakes the curve into 256 u8 steps (rounded down) over its x and
        // y spans, so keep them close to the metered -8..8 EV histogram: a
        // +-32 EV span made each step a quarter stop and under-exposed.
        let quantize = |v: f32| (v * 256.).round() / 256.;
        let gray = quantize(gray.log2());
        let low = quantize((self.min * scale).log2());
        let high = quantize((self.max * scale).log2());
        let from = (gray - high - 1.).min(-8.).floor();
        let to = (gray - low + 1.).max(8.).ceil();
        if low == high {
            return vec![Vec2::new(from, from + low), Vec2::new(to, to + low)];
        }
        vec![
            Vec2::new(from, from + high),
            Vec2::new(gray - high, gray),
            Vec2::new(gray - low, gray),
            Vec2::new(to, to + low),
        ]
    }
    fn build(knots: Vec<Vec2>) -> AutoExposureCompensationCurve {
        AutoExposureCompensationCurve::from_curve(bevy::math::cubic_splines::LinearSpline::new(
            knots,
        ))
        .expect("Bounded native map exposure curve")
    }
    /// Source's rule, for the world camera.
    pub(crate) fn curve(self) -> AutoExposureCompensationCurve {
        Self::build(self.knots(self.target, 1. / CAMERA_EXPOSURE))
    }
    /// Mean luminance to middle grey, for the view-model camera.
    pub(crate) fn legacy_curve(self) -> AutoExposureCompensationCurve {
        Self::build(self.knots(0.18, 1.))
    }
}
/// Weights only the centre 90% x 85% of the screen, like
/// mat_exposure_center_region_x/y.
pub(crate) fn metering_mask() -> Image {
    let size = 200u32;
    let mut data = vec![0u8; (size * size) as usize];
    for y in 15..185 {
        for x in 10..190 {
            data[(y * size + x) as usize] = 255;
        }
    }
    Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    )
}
/// Source has no tone curve, but Bevy only applies auto exposure in its
/// tonemapping pass, which `Tonemapping::None` skips. The world camera uses
/// the TonyMcMapface LUT path instead, with that LUT replaced by a linear
/// ramp clipped at 1. Bevy indexes the LUT with x/(x+1), so each texel holds
/// min(e/(1-e), 1). Nothing else in the game uses TonyMcMapface.
pub(crate) const WORLD_TONEMAPPING: bevy::core_pipeline::tonemapping::Tonemapping =
    bevy::core_pipeline::tonemapping::Tonemapping::TonyMcMapface;
pub(crate) fn linear_clip_lut() -> Image {
    const DIMS: usize = 48;
    let ramp: Vec<f32> = (0..DIMS)
        .map(|i| {
            let e = i as f32 / (DIMS - 1) as f32;
            if e >= 0.5 { 1. } else { e / (1. - e) }
        })
        .collect();
    let mut data = Vec::with_capacity(DIMS * DIMS * DIMS * 8);
    for b in 0..DIMS {
        for g in 0..DIMS {
            for r in 0..DIMS {
                for v in [ramp[r], ramp[g], ramp[b], 1.] {
                    data.extend_from_slice(&half::f16::from_f32(v).to_le_bytes());
                }
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: DIMS as u32,
            height: DIMS as u32,
            depth_or_array_layers: DIMS as u32,
        },
        TextureDimension::D3,
        data,
        TextureFormat::Rgba16Float,
        // Bevy keeps its LUTs in the main world too.
        RenderAssetUsages::default(),
    );
    image.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        label: Some("Linear clip LUT".into()),
        address_mode_u: bevy::image::ImageAddressMode::ClampToEdge,
        address_mode_v: bevy::image::ImageAddressMode::ClampToEdge,
        address_mode_w: bevy::image::ImageAddressMode::ClampToEdge,
        mag_filter: bevy::image::ImageFilterMode::Linear,
        min_filter: bevy::image::ImageFilterMode::Linear,
        mipmap_filter: bevy::image::ImageFilterMode::Linear,
        ..default()
    });
    image
}
/// The world camera's adapted auto exposure (EV), read back from the GPU
/// each frame (vendor/bevy_post_process patch). Source blends fog after the
/// tonemap scale, so native_fog divides the fog colour by 2^EV.
#[derive(Resource, Clone)]
pub(crate) struct ExposureReadback(pub Handle<bevy::render::storage::ShaderBuffer>);
static EXPOSURE_EV: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// The last read exposure scale (2^EV of Bevy's auto exposure), 1 before
/// the first readback.
pub(crate) fn exposure_scale() -> f32 {
    let ev = f32::from_bits(EXPOSURE_EV.load(std::sync::atomic::Ordering::Relaxed));
    if ev.is_finite() { ev.exp2() } else { 1. }
}
/// Source's tonemap scale for the world camera (Bevy's exposure includes
/// the 1/1.2 camera exposure the curve compensates).
pub(crate) fn source_scale() -> f32 {
    exposure_scale() * CAMERA_EXPOSURE
}
/// The shader API's fog colour (shaderapidx9 10011150): linear fog colour x
/// tonemap scale, scaled down so its luminance (0.2125, 0.7154, 0.0721)
/// stays at most fog_max_brightness_after_tonemapping (1.0). Returns the
/// colour Bevy must use before its exposure is applied.
pub(crate) fn fog_before_exposure(linear: [f32; 3]) -> [f32; 3] {
    let scale = source_scale();
    let lum = (linear[0] * 0.2125 + linear[1] * 0.7154 + linear[2] * 0.0721) * scale;
    let cap = if lum > 1. { 1. / lum } else { 1. };
    // Bevy multiplies the fog by exposure_scale() after the main pass.
    let k = scale * cap / exposure_scale().max(1e-4);
    linear.map(|c| c * k)
}
pub(crate) fn install_exposure_readback(
    mut commands: Commands,
    mut buffers: ResMut<Assets<bevy::render::storage::ShaderBuffer>>,
) {
    let handle = buffers.add(bevy::render::storage::ShaderBuffer::new(
        &[0u8; 16],
        RenderAssetUsages::RENDER_WORLD,
    ));
    commands.insert_resource(ExposureReadback(handle.clone()));
    commands
        .spawn(bevy::render::gpu_readback::Readback::buffer(handle))
        .observe(|event: On<bevy::render::gpu_readback::ReadbackComplete>| {
            if event.data.len() >= 4 {
                let ev = f32::from_le_bytes([event.data[0], event.data[1], event.data[2], event.data[3]]);
                if ev.is_finite() {
                    // L4D2R_EV_LOG: debug, the adapted EV about once a second.
                    static LOGS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                    if std::env::var_os("L4D2R_EV_LOG").is_some() && LOGS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 60 == 0 {
                        let f = |i: usize| event.data.get(i * 4..i * 4 + 4).map_or(f32::NAN, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                        println!("L4D2R EV {ev:.3} scale {:.3} metered avg_log {:.3} median_log {:.3} target {:.3}", ev.exp2() * CAMERA_EXPOSURE, f(1), f(2), f(3));
                    }
                    EXPOSURE_EV.store(ev.to_bits(), std::sync::atomic::Ordering::Relaxed);
                }
            }
        });
}
/// The view model takes the world camera's adapted tonemap scale. Its own
/// auto exposure metered the gun alone to middle grey, so in a dark room
/// (c1m2's store) a nearly black retail pistol drew fully lit. Bevy's
/// `Exposure` multiplies by 1/(1.2 x 2^ev100), the world by 2^EV / 1.2.
pub(crate) fn share_world_exposure(mut views: Query<&mut bevy::camera::Exposure, With<crate::game::ViewCamera>>) {
    let ev100 = -exposure_scale().log2();
    for mut exposure in &mut views {
        if (exposure.ev100 - ev100).abs() > 1e-4 {
            exposure.ev100 = ev100;
        }
    }
}
pub(crate) fn install_linear_lut(
    luts: Option<Res<bevy::core_pipeline::tonemapping::TonemappingLuts>>,
    mut images: ResMut<Assets<Image>>,
) {
    if let Some(luts) = luts {
        let _ = images.insert(luts.tony_mc_mapface.id(), linear_clip_lut());
    }
}
pub(crate) fn sync(
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    mut curves: ResMut<Assets<AutoExposureCompensationCurve>>,
    mut cameras: Query<(Entity, &mut AutoExposure, Has<SourceMetering>)>,
    mut applied: Local<std::collections::HashMap<Entity, Bounds>>,
) {
    let Some(logic) = logic else {
        return;
    };
    let mut bounds = Bounds::from_map(logic.world.tonemap);
    // L4D2R_TONEMAP_SCALE=x: a fixed scale, like mat_force_tonemap_scale.
    if let Some(scale) = std::env::var("L4D2R_TONEMAP_SCALE").ok().and_then(|v| v.parse::<f32>().ok()).filter(|v| *v > 0.) {
        bounds.min = scale;
        bounds.max = scale;
    }
    if cameras
        .iter()
        .all(|(entity, ..)| applied.get(&entity) == Some(&bounds))
    {
        return;
    }
    let source_curves: std::collections::HashSet<_> = cameras
        .iter()
        .filter(|(.., source)| *source)
        .map(|(_, camera, _)| camera.compensation_curve.id())
        .collect();
    // Mutating AutoExposure resets Bevy's adapted state, so touch each camera
    // only when its bounds change.
    let mut updated = std::collections::HashSet::new();
    for (entity, mut camera, source) in &mut cameras {
        if applied.get(&entity) == Some(&bounds) {
            continue;
        }
        let curve = if source {
            bounds.curve()
        } else {
            bounds.legacy_curve()
        };
        if !source && source_curves.contains(&camera.compensation_curve.id()) {
            // The view model clones the world camera's settings at spawn.
            camera.compensation_curve = curves.add(curve);
        } else if updated.insert(camera.compensation_curve.id()) {
            if let Some(mut existing) = curves.get_mut(&camera.compensation_curve) {
                *existing = curve;
            }
        }
        if source {
            camera.filter = bounds.filter();
            // Median floor (client 101c6560): dark rooms with a bright
            // window (c8m1's rooftop) read ~2.4x darker than retail without it.
            camera.min_average_luminance = bounds.min_average;
            camera.max_ev = (bounds.max / CAMERA_EXPOSURE).log2();
        } else {
            camera.filter = AutoExposure::default().filter;
            camera.metering_mask = default();
        }
        // Bevy names the speeds after the scene: speed_brighten applies when
        // the scene gets brighter (the image darkens), retail's accelerated
        // direction.
        let (image_up, image_down) = bounds.adapt_speeds();
        camera.speed_brighten = image_down;
        camera.speed_darken = image_up;
        camera.exponential_transition_distance = ADAPT_RANGE;
        applied.insert(entity, bounds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exposure_adapts_at_the_retail_rate() {
        let bounds = Bounds { min: 1., max: 5., rate: 0.25, target: 0.45, bright: 0.01, min_average: 0.03 };
        let (up, down) = bounds.adapt_speeds();
        // 2 x 0.25 of the scale gap per second at a 2 EV change: 1.08 EV/s per EV.
        assert!((up / ADAPT_RANGE - 0.5 * 3. / (2. * std::f32::consts::LN_2)).abs() < 1e-4);
        // Darkening: 3 x faster, capped at 1/64 per 60 Hz frame.
        assert!((down / ADAPT_RANGE - 0.9375 * 0.75 / (2. * std::f32::consts::LN_2)).abs() < 1e-4);
    }
    fn gain(knots: &[Vec2], lum: f32) -> f32 {
        let pair = knots
            .windows(2)
            .find(|p| lum >= p[0].x && lum <= p[1].x)
            .unwrap();
        let t = (lum - pair[0].x) / (pair[1].x - pair[0].x);
        (pair[0].y + t * (pair[1].y - pair[0].y) - lum).exp2()
    }
    #[test]
    fn exposure_curves_clamp_to_each_maps_bounds() {
        for (min, max) in [(0.5, 2.), (1., 5.), (1., 6.), (2., 2.)] {
            let b = Bounds::from_map(Tonemap {
                auto_min: Some(min),
                auto_max: Some(max),
                ..default()
            });
            b.curve();
            b.legacy_curve();
            for lum in [-8., -4., 0., 4., 8.] {
                // The final scale (camera exposure x auto exposure) stays in
                // the map's bounds.
                let scale = gain(&b.knots(b.target, 1. / CAMERA_EXPOSURE), lum) * CAMERA_EXPOSURE;
                assert!(
                    scale >= min * 0.997 && scale <= max * 1.003,
                    "{min}..{max}: {scale}"
                );
                let legacy = gain(&b.knots(0.18, 1.), lum);
                assert!(
                    legacy >= min * 0.997 && legacy <= max * 1.003,
                    "{min}..{max}: {legacy}"
                );
            }
        }
    }
    /// c1m1's logic_auto sets 5% bright pixels; retail screenshots on the
    /// roof put that percentile at about 0.47 linear.
    #[test]
    fn hotel_meters_the_brightest_five_percent_to_45_percent() {
        let b = Bounds::from_map(Tonemap {
            auto_min: Some(1.),
            auto_max: Some(5.),
            percent_bright_pixels: 5.,
            ..default()
        });
        assert!((b.target - 0.45).abs() < 1e-6);
        let f = b.filter();
        assert!((f.start() - 0.94).abs() < 1e-4 && (f.end() - 0.96).abs() < 1e-4);
        // A metered percentile inside the bounds lands on the target.
        let measured = 0.2f32;
        let scaled = measured * gain(&b.knots(b.target, 1. / CAMERA_EXPOSURE), measured.log2());
        assert!((scaled - 0.45).abs() < 0.01, "{scaled}");
    }
    #[test]
    fn a_new_view_starts_at_source_scale_one_within_the_maps_bounds() {
        let at = |min: f32, max: f32| {
            let b = Bounds { min, max, rate: 1., target: 0.45, bright: 0.01, min_average: 0.03 };
            b.initial_ev().exp2() * CAMERA_EXPOSURE
        };
        assert!((at(1., 5.) - 1.).abs() < 1e-5);
        assert!((at(0.5, 2.) - 1.).abs() < 1e-5);
        assert!((at(1.5, 4.) - 1.5).abs() < 1e-5);
        assert!((at(0.25, 0.75) - 0.75).abs() < 1e-5);
    }
    #[test]
    fn invalid_controller_inputs_cannot_poison_the_renderer() {
        let b = Bounds::from_map(Tonemap {
            auto_min: Some(f32::NAN),
            auto_max: Some(-1.),
            rate: f32::INFINITY,
            percent_target: f32::NAN,
            percent_bright_pixels: -3.,
            ..default()
        });
        assert_eq!(
            b,
            Bounds {
                min: 0.5,
                max: 2.,
                rate: 1.,
                target: 0.45,
                bright: 0.01,
                min_average: 0.03,
            }
        );
        b.curve();
    }
}
