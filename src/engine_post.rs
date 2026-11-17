//! Retail L4D2 screen post-processing (engine_post): local contrast, colour
//! correction and the top vignette, on the gamma-space frame.
//!
//! Chain (client DoEnginePostProcessing 101c9310, Generate8BitBloomTexture
//! 101c7d60): the frame is downsampled to quarter size (downsample_nohdr,
//! alpha = luminance), blurred by the 13-tap kernel 4 in X then Y
//! (blurfilter), and engine_post combines it. L4D2 sets the bloom amount to
//! 0 (101c57c0), so the blurred image only feeds local contrast. Shader
//! maths: assets/shaders/engine_post.wgsl.
//!
//! Parameters (client 102727b0 / 10271640 / 10271780 / 1026faf0): the
//! player's postprocess_controller (from the fog volume's PostProcessName,
//! else the master: spawnflags 1, else the first) is blended to over
//! max(fadetime, 0.001) s from the values shown. Before a player exists the
//! values are scripts/postprocess.txt "default". Controller key defaults
//! (server 10194bc0): contrast 0, edge 0, vignette 1..2, blur 0, fade to
//! black 0, grain 1, top vignette 1, fadetime 2.
//! Player states (102727b0): Hunter pounce / Smoker tongue (fading to
//! their _incap presets as incap health drains), the incap_1..3 cycle,
//! vomited ("it"), adrenaline, pills (2 s), healed (3 s) and the third
//! strike, with the blurred vignette. The view fade is blended here, before
//! the HUD, like retail's g_vViewFadeColor. Not reproduced yet: high stress,
//! on fire, defibrillator and modulate-mode fades.
use bevy::{
    core_pipeline::{schedule::Core3d, tonemapping::tonemapping, Core3dSystems, FullscreenShader},
    prelude::*,
    render::{
        extract_component::{ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin, UniformComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{
            binding_types::{sampler, texture_2d, texture_3d, uniform_buffer},
            BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId, ColorTargetState,
            ColorWrites, Extent3d, FragmentState, Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor,
            RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType,
            TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView,
        },
        renderer::{RenderContext, RenderDevice, ViewQuery},
        texture::{CachedTexture, GpuImage, TextureCache},
        view::{ExtractedView, ViewTarget},
        Render, RenderApp, RenderStartup, RenderSystems,
    },
};
use std::collections::HashMap;

/// The blended colour-correction lookup (color_correction.rs).
#[derive(Resource, Clone, ExtractResource)]
pub struct CcLookup {
    pub image: Handle<Image>,
    pub weight: f32,
}
/// materials/dev/vignette.vtf and dev/noise_post (engine_post.vmt).
#[derive(Resource, Clone, ExtractResource)]
pub struct VignetteTexture(pub Handle<Image>, pub Handle<Image>);

/// The engine_post constants of the world camera.
#[derive(Component, Clone, Copy, Default, ShaderType, ExtractComponent)]
pub struct EnginePost {
    /// Centre local contrast, edge local contrast, vignette start, end.
    pub contrast: Vec4,
    /// Top vignette scale, fade to black, colour correction weight,
    /// vignette texture present (1).
    pub extra: Vec4,
    /// Film grain: strength (0 = off), shader time mod 1000.
    pub grain: Vec4,
    /// x: blurred vignette scale (BLURRED_VIGNETTE_ENABLE when above 0).
    pub blur: Vec4,
    /// The view fade (g_vViewFadeColor): gamma colour and alpha.
    pub fade: Vec4,
}

/// The eight controller values: local contrast, edge local contrast,
/// vignette start, vignette end, vignette blur, fade to black, grain, top
/// vignette.
pub type Values = [f32; 8];
const CONTROLLER_DEFAULTS: Values = [0., 0., 1., 2., 0., 0., 1., 1.];
const KEYS: [&str; 8] = [
    "localcontraststrength",
    "localcontrastedgestrength",
    "vignettestart",
    "vignetteend",
    "vignetteblurstrength",
    "fadetoblackstrength",
    "grainstrength",
    "topvignettestrength",
];
/// postprocess.txt "default" (the six preset values; grain and top
/// vignette are not part of presets).
const DEFAULT_PRESET: Values = [0.5, 0., 0.8, 1.1, 0., 0., 0., 0.];

/// Values of a postprocess_controller entity and its fade time.
pub fn controller(e: &l4d2r_game_core::entities::Ent) -> (Values, f32) {
    let mut v = CONTROLLER_DEFAULTS;
    for (i, key) in KEYS.iter().enumerate() {
        if let Some(x) = e.me.kv_f32(key) {
            v[i] = x;
        }
    }
    (v, e.me.kv_f32("fadetime").unwrap_or(2.))
}

/// The client's blend toward the active state (10271780 / 1026faf0).
#[derive(Clone, Debug, PartialEq)]
pub struct Blend {
    pub from: Values,
    pub target: Values,
    pub current: Values,
    pub end: f32,
    pub duration: f32,
    pub source: Option<usize>,
}
impl Default for Blend {
    fn default() -> Self {
        Self { from: DEFAULT_PRESET, target: DEFAULT_PRESET, current: DEFAULT_PRESET, end: 0., duration: 1., source: None }
    }
}
impl Blend {
    /// Follows controller `source` with `values`; a new source starts a
    /// blend from what is shown over max(fade, 0.001) seconds.
    pub fn follow(&mut self, source: usize, values: Values, fade: f32, now: f32) {
        if self.source != Some(source) {
            self.source = Some(source);
            self.duration = fade.max(0.001);
            self.end = now + self.duration;
            self.from = self.current;
        }
        self.target = values;
    }
    /// A player-state preset (10271640): a new state blends from what is
    /// shown; presets set the first six values, grain and top vignette keep
    /// their last ones.
    pub fn preset(&mut self, state: usize, six: [f32; 6], blend: f32, now: f32) {
        let key = STATE_KEY + state;
        if self.source != Some(key) {
            self.source = Some(key);
            self.duration = blend.max(0.001);
            self.end = now + self.duration;
            self.from = self.current;
        }
        self.target[..6].copy_from_slice(&six);
    }
    pub fn update(&mut self, now: f32) {
        let left = if self.end > 0. { ((self.end - now) / self.duration).clamp(0., 1.) } else { 0. };
        let t = 1. - left;
        for i in 0..8 {
            self.current[i] = (self.target[i] - self.from[i]) * t + self.from[i];
        }
    }
}

const STATE_KEY: usize = 1 << 40;
/// scripts/postprocess.txt, in the client's preset order (102727b0 indexes
/// them 0..16): default, pills_1, pills_2, healed, it, pounced,
/// pounced_incap, choked, choked_incap, 3rd_strike, incap_1..3,
/// high_stress, on_fire, adrenaline, defibrillator.
const PRESETS: [[f32; 6]; 17] = [
    [0.5, 0., 0.8, 1.1, 0., 0.],
    [2.5, 0., 0.8, 1.1, 0., 0.],
    [-0.4, 0., 0.8, 1.1, 0., 0.],
    [-0.4, 0., 0.8, 1.1, 0., 0.],
    [-0.7, -0.7, 0.8, 1.1, 0., 0.],
    [1., 0., 0.7, 1., 1., 0.],
    [1., 0., 0.7, 1., 1., 1.],
    [1., 0., 0.7, 1., 1., 0.],
    [1., 0., 0.7, 1., 1., 1.],
    [1., -1., 0.6, 1., 0., 0.],
    [1., -1., 0.6, 1., 0., 0.],
    [0., -1., 0.2, 0.6, 0., 0.],
    [0., -1., 0.2, 0.6, 0., 1.],
    [0.6, 0.3, 0.8, 1.1, 0., 0.],
    [1.25, -0.65, 0.4, 0.85, 0., 0.],
    [1.8, -1., 0.6, 0.95, 0., 0.],
    [-0.8, -0.8, 0.26, 0.95, 1., -3.],
];
fn lerp6(a: [f32; 6], b: [f32; 6], t: f32) -> [f32; 6] {
    std::array::from_fn(|i| (b[i] - a[i]) * t + a[i])
}
/// The player state the client shows (102727b0), as (preset state,
/// values, blend seconds), or None for the map controller. `health` is the
/// smoothed incap health. Survivor incap health is 300 (survivor_incap_health).
/// `stress` is m_clientIntensity at 100 (the Director's survivor intensity
/// at its maximum).
///
/// `defibbed`: a teammate is defibrillating the dead player
/// (m_iCurrentUseAction 5 aimed at them), checked right after the Smoker.
/// Not reachable yet: on_fire (14, an attached flame entity, after high
/// stress), since survivors do not ignite.
pub fn player_state(s: &crate::game_state::Match, health: f32, base: Values, stress: bool, defibbed: bool) -> Option<(usize, [f32; 6], f32)> {
    // Pills / healed / high stress add their contrast and edge to the
    // map controller's (FUN_1026f240), clamped.
    let relative = |preset: [f32; 6], hi: f32| {
        let mut v = preset;
        v[0] = (v[0] + base[0]).clamp(-0.3, hi);
        v[1] = (v[1] + base[1]).clamp(-0.3, hi);
        v
    };
    use crate::special_infected::SpecialClass;
    let fraction = health / 300.;
    let incap = s.incap && !s.dead;
    match s.pinned_by.filter(|_| s.pinned) {
        Some(SpecialClass::Hunter) => {
            let t = if incap { (1. - fraction * 2.).clamp(0., 1.) } else { 0. };
            return Some((5, lerp6(PRESETS[5], PRESETS[6], t), 0.5));
        }
        Some(SpecialClass::Smoker) => {
            if !incap {
                return Some((7, PRESETS[7], 0.75));
            }
            let t = (1. - fraction * 4.).clamp(0., 1.);
            return Some((5, lerp6(PRESETS[7], PRESETS[8], t), 0.5));
        }
        _ => {}
    }
    if defibbed {
        return Some((16, PRESETS[16], 0.75));
    }
    if incap {
        // incap_1 -> incap_2 down to a quarter of the incap health, then
        // incap_2 -> incap_3.
        let values = if fraction <= 0.25 {
            let f = (fraction * 4.).clamp(0., 1.);
            lerp6(PRESETS[11], PRESETS[12], 1. - f)
        } else {
            let f = ((fraction - 0.25) * (4. / 3.)).clamp(0., 1.);
            lerp6(PRESETS[10], PRESETS[11], 1. - f)
        };
        return Some((10, values, 0.5));
    }
    if s.dead {
        return None;
    }
    // Vomited ("it", 1002efd0 / 10270480): default -> it by the fraction
    // of the IT timer left, less cl_glow_it_timer_ratio_reduction (0.08)
    // in its second half.
    if s.vomited > 0. {
        let mut f = (s.vomited / crate::special_infected::boomer::IT_DURATION).clamp(0., 1.);
        if f < 0.5 {
            f = (f - 0.08).max(0.);
        }
        return Some((4, lerp6(PRESETS[0], PRESETS[4], f), 0.25));
    }
    if s.adrenaline_time > 0. {
        return Some((15, PRESETS[15], 0.75));
    }
    // Pills: pills_1 until 0.8 s of the 2 s timer remain, then pills_2.
    if s.post_pills > 0. {
        return Some(if s.post_pills <= 0.8 {
            (2, relative(PRESETS[2], 3.), 1.)
        } else {
            (1, relative(PRESETS[1], 3.), 0.2)
        });
    }
    if s.post_healed > 0. {
        return Some((3, relative(PRESETS[3], 3.), 0.5));
    }
    if s.black_white {
        return Some((9, PRESETS[9], 0.3));
    }
    // high_stress: relative like pills, clamped to -0.3..1.5, 2 s blend.
    if stress {
        return Some((13, relative(PRESETS[13], 1.5), 2.));
    }
    None
}

#[derive(Default)]
pub(crate) struct State {
    map: usize,
    blend: Blend,
    controllers: Vec<usize>,
    master: Option<usize>,
    /// Incap health smoothed downward by 5% per frame (102727b0 +0x120).
    health: f32,
    logged: Option<usize>,
}

/// Picks the player's controller, blends, and writes the camera constants.
pub(crate) fn sync(
    mut commands: Commands,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    game: Option<Res<crate::game_state::Match>>,
    lookup: Option<Res<CcLookup>>,
    vignette: Option<Res<VignetteTexture>>,
    mut state: Local<State>,
    time: Res<Time<Real>>,
    graphics: Option<Res<crate::graphics_options::Graphics>>,
    director: Option<Res<crate::source_director::Director>>,
    squad: Option<Res<crate::game_state::Squad>>,
    cameras: Query<(Entity, &GlobalTransform, &crate::game::Body), With<crate::viewer::GameCamera>>,
) {
    let Some(logic) = logic else { return };
    let Ok((entity, transform, body)) = cameras.single() else { return };
    if std::env::var_os("L4D2R_NO_POST").is_some() {
        commands.entity(entity).remove::<EnginePost>();
        return;
    }
    let world = &logic.world;
    let map = std::sync::Arc::as_ptr(&world.bsp) as usize;
    if state.map != map {
        let controllers: Vec<usize> = world
            .ents
            .iter()
            .enumerate()
            .filter(|(_, e)| e.alive && e.me.classname == "postprocess_controller")
            .map(|(i, _)| i)
            .collect();
        // 10194a30: the first controller, replaced by one with spawnflags 1.
        let master = controllers
            .iter()
            .copied()
            .find(|&i| world.ents[i].me.spawnflags() & 1 != 0)
            .or(controllers.first().copied());
        *state = State { map, controllers, master, ..default() };
    }
    let now = world.time;
    let (incap, dead) = game.as_ref().map_or((false, false), |g| (g.incap, g.dead));
    let (point, _) = crate::native_fog::view_point(body, incap, world.cine.view_control.is_some() || dead, transform);
    let by_name = |name: &str| {
        state.controllers.iter().copied().find(|&i| world.ents[i].me.targetname.eq_ignore_ascii_case(name))
    };
    let inside = world.fog.volumes.iter().copied().find(|&id| crate::native_fog::Runtime::contains(world, id, point));
    let chosen = inside
        .and_then(|v| world.ents[v].me.kv("PostProcessName"))
        .and_then(by_name)
        .or(state.master);
    let shown = game.as_ref().and_then(|g| {
        let raw = if g.incap { g.incap_health } else { g.health };
        state.health = if g.incap && raw <= state.health { (raw - state.health) * 0.05 + state.health } else { raw };
        let base = chosen.map_or(DEFAULT_PRESET, |id| controller(&world.ents[id]).0);
        let stress = !g.infected && director.as_ref().is_some_and(|d| d.survivor_intensity(0) >= 1.);
        let defibbed = g.dead && squad.as_ref().is_some_and(|q| q.0.iter().any(|m| m.state.medical_kind == 6 && m.state.action > 0.));
        player_state(g, state.health, base, stress, defibbed)
    });
    match shown {
        Some((preset, values, blend)) => {
            // L4D2R_POST_LOG: debug, player post-process state changes.
            if std::env::var_os("L4D2R_POST_LOG").is_some() && state.logged != Some(preset) {
                println!("Post state {preset} at {now:.2}: {values:?}");
                state.logged = Some(preset);
            }
            state.blend.preset(preset, values, blend, now)
        }
        None => {
            state.logged = None;
            if let Some(id) = chosen {
                let (values, fade) = controller(&world.ents[id]);
                state.blend.follow(id, values, fade, now);
            }
        }
    }
    state.blend.update(now);
    let v = state.blend.current;
    // NOISE_ENABLE: mat_grain_enable 1 and $noiseScale (the grain value)
    // above 0; mat_grain_scale_override -1 leaves it. Time: the shader
    // API's real time wrapped at 1000 s (stdshader 1000ca40).
    let clock = time.elapsed_secs_wrapped();
    let clock = clock - (clock * 0.001).floor() * 1000.;
    let post = EnginePost {
        contrast: Vec4::new(v[0], v[1], v[2], v[3]),
        extra: Vec4::new(
            v[7],
            v[5],
            lookup.as_ref().map_or(0., |l| l.weight),
            if vignette.is_some() && v[7] > 0. { 1. } else { 0. },
        ),
        // $noiseScale x mat_grain_scale_override (the Film Grain option).
        grain: Vec4::new(v[6].max(0.) * graphics.as_ref().map_or(1., |g| g.grain), clock, 0., 0.),
        blur: Vec4::new(v[4].max(0.), 0., 0., 0.),
        fade: world.cine.fade.map_or(Vec4::ZERO, |f| {
            Vec4::new(f.color[0], f.color[1], f.color[2], f.alpha_at(world.time))
        }),
    };
    commands.entity(entity).insert(post);
}

/// Loads dev/vignette once.
fn load_vignette(
    mut commands: Commands,
    source: Option<Res<crate::game_ui::HudSource>>,
    mut images: ResMut<Assets<Image>>,
    mut tried: Local<bool>,
) {
    if *tried {
        return;
    }
    let Some(source) = source else { return };
    *tried = true;
    let load = |path: &str, repeat: bool| -> std::io::Result<Image> {
        let mut assets = crate::source_assets::SourceAssets::new(&source.0, vec![])?;
        let t = crate::vtf::decode(&assets.read(path)?, 1024)?;
        let mut image = Image::new(
            Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
            TextureDimension::D2,
            t.rgba,
            TextureFormat::Rgba8Unorm,
            bevy::asset::RenderAssetUsages::default(),
        );
        let address = if repeat { bevy::image::ImageAddressMode::Repeat } else { bevy::image::ImageAddressMode::ClampToEdge };
        image.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
            address_mode_u: address,
            address_mode_v: address,
            mag_filter: bevy::image::ImageFilterMode::Linear,
            min_filter: bevy::image::ImageFilterMode::Linear,
            ..default()
        });
        Ok(image)
    };
    match (load("materials/dev/vignette.vtf", false), load("materials/dev/noise_post.vtf", true)) {
        (Ok(vignette), Ok(noise)) => {
            commands.insert_resource(VignetteTexture(images.add(vignette), images.add(noise)));
        }
        (a, b) => eprintln!("engine_post textures: {:?} {:?}", a.err(), b.err()),
    }
}

pub struct EnginePostPlugin;
impl Plugin for EnginePostPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ExtractComponentPlugin::<EnginePost>::default(),
            UniformComponentPlugin::<EnginePost>::default(),
            ExtractResourcePlugin::<CcLookup>::default(),
            ExtractResourcePlugin::<VignetteTexture>::default(),
        ))
        .add_systems(Update, load_vignette)
        .add_systems(PostUpdate, sync.after(crate::color_correction::sync));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
            .add_systems(Core3d, engine_post_pass.in_set(Core3dSystems::PostProcess).after(tonemapping));
    }
}

const SMALL_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

#[derive(Resource)]
struct Pipeline {
    simple: BindGroupLayoutDescriptor,
    full: BindGroupLayoutDescriptor,
    sampler: Sampler,
    repeat: Sampler,
    downsample: CachedRenderPipelineId,
    blur_x: CachedRenderPipelineId,
    blur_y: CachedRenderPipelineId,
    shader: Handle<bevy::shader::Shader>,
    vertex: bevy::render::render_resource::VertexState,
    combine: HashMap<TextureFormat, CachedRenderPipelineId>,
}

fn init_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    asset_server: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    pipeline_cache: Res<PipelineCache>,
) {
    let simple = BindGroupLayoutDescriptor::new(
        "engine_post_simple",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture_2d(TextureSampleType::Float { filterable: true }), sampler(SamplerBindingType::Filtering)),
        ),
    );
    let full = BindGroupLayoutDescriptor::new(
        "engine_post_full",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
                uniform_buffer::<EnginePost>(true),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let sampler = render_device.create_sampler(&SamplerDescriptor {
        mag_filter: bevy::render::render_resource::FilterMode::Linear,
        min_filter: bevy::render::render_resource::FilterMode::Linear,
        ..default()
    });
    let repeat = render_device.create_sampler(&SamplerDescriptor {
        address_mode_u: bevy::render::render_resource::AddressMode::Repeat,
        address_mode_v: bevy::render::render_resource::AddressMode::Repeat,
        mag_filter: bevy::render::render_resource::FilterMode::Linear,
        min_filter: bevy::render::render_resource::FilterMode::Linear,
        ..default()
    });
    let shader: Handle<bevy::shader::Shader> = asset_server.load("shaders/engine_post.wgsl");
    let vertex = fullscreen.to_vertex_state();
    let small = |entry: &'static str| {
        pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(format!("engine_post_{entry}").into()),
            layout: vec![simple.clone()],
            vertex: vertex.clone(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState { format: SMALL_FORMAT, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        })
    };
    let downsample = small("downsample");
    let blur_x = small("blur_x");
    let blur_y = small("blur_y");
    commands.insert_resource(Pipeline {
        simple,
        full,
        sampler,
        repeat,
        downsample,
        blur_x,
        blur_y,
        shader,
        vertex,
        combine: HashMap::new(),
    });
}

#[derive(Component)]
struct Prepared {
    a: CachedTexture,
    b: CachedTexture,
    combine: CachedRenderPipelineId,
}

fn prepare(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipeline: ResMut<Pipeline>,
    mut texture_cache: ResMut<TextureCache>,
    render_device: Res<RenderDevice>,
    views: Query<(Entity, &ExtractedView, Option<&EnginePost>)>,
) {
    for (entity, view, post) in &views {
        if post.is_none() {
            commands.entity(entity).remove::<Prepared>();
            continue;
        }
        let format = view.target_format;
        let Pipeline { full, shader, vertex, combine, .. } = &mut *pipeline;
        let combine = *combine.entry(format).or_insert_with(|| {
            pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("engine_post_combine".into()),
                layout: vec![full.clone()],
                vertex: vertex.clone(),
                fragment: Some(FragmentState {
                    shader: shader.clone(),
                    entry_point: Some("combine".into()),
                    targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                    ..default()
                }),
                ..default()
            })
        });
        // _rt_SmallFB0/1: a quarter of the frame (integer division).
        let size = view.viewport.zw();
        let small = Extent3d { width: (size.x / 4).max(1), height: (size.y / 4).max(1), depth_or_array_layers: 1 };
        let descriptor = |label: &'static str| TextureDescriptor {
            label: Some(label),
            size: small,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: SMALL_FORMAT,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        let a = texture_cache.get(&render_device, descriptor("engine_post_small_0"));
        let b = texture_cache.get(&render_device, descriptor("engine_post_small_1"));
        commands.entity(entity).insert(Prepared { a, b, combine });
    }
}

fn engine_post_pass(
    view: ViewQuery<(&ViewTarget, &DynamicUniformIndex<EnginePost>, &Prepared)>,
    pipeline: Res<Pipeline>,
    pipeline_cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<EnginePost>>,
    lookup: Option<Res<CcLookup>>,
    vignette: Option<Res<VignetteTexture>>,
    images: Res<RenderAssets<GpuImage>>,
    render_device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (view_target, index, prepared) = view.into_inner();
    let (Some(down), Some(blur_x), Some(blur_y), Some(combine), Some(binding)) = (
        pipeline_cache.get_render_pipeline(pipeline.downsample),
        pipeline_cache.get_render_pipeline(pipeline.blur_x),
        pipeline_cache.get_render_pipeline(pipeline.blur_y),
        pipeline_cache.get_render_pipeline(prepared.combine),
        uniforms.uniforms().binding(),
    ) else {
        return;
    };
    let (Some(lookup), Some(vignette), Some(noise)) = (
        lookup.as_ref().and_then(|l| images.get(&l.image)),
        vignette.as_ref().and_then(|v| images.get(&v.0)),
        vignette.as_ref().and_then(|v| images.get(&v.1)),
    ) else {
        return;
    };
    let simple = pipeline_cache.get_bind_group_layout(&pipeline.simple);
    let run = |label: &'static str, render: &bevy::render::render_resource::RenderPipeline, input: &TextureView, output: &TextureView, ctx: &mut RenderContext| {
        let group = render_device.create_bind_group(label, &simple, &BindGroupEntries::sequential((input, &pipeline.sampler)));
        let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(render);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    };
    run("engine_post_downsample", down, view_target.main_texture_view(), &prepared.a.default_view, &mut ctx);
    run("engine_post_blur_x", blur_x, &prepared.a.default_view, &prepared.b.default_view, &mut ctx);
    run("engine_post_blur_y", blur_y, &prepared.b.default_view, &prepared.a.default_view, &mut ctx);
    let post = view_target.post_process_write();
    let group = render_device.create_bind_group(
        "engine_post_combine",
        &pipeline_cache.get_bind_group_layout(&pipeline.full),
        &BindGroupEntries::sequential((
            post.source,
            &pipeline.sampler,
            &prepared.a.default_view,
            &vignette.texture_view,
            &lookup.texture_view,
            binding,
            &noise.texture_view,
            &pipeline.repeat,
        )),
    );
    let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some("engine_post_combine"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: post.destination,
            depth_slice: None,
            resolve_target: None,
            ops: Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(combine);
    pass.set_bind_group(0, &group, &[index.index()]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controller_changes_blend_from_the_shown_values() {
        let mut b = Blend::default();
        let exterior = [-0.2, -0.7, 0.8, 3., 0., 0., 0.5, 0.1];
        let interior = [0., 0., 1., 2., 0., 0., 1., 1.];
        b.follow(1, exterior, 2., 0.);
        b.update(1.);
        assert!((b.current[0] - (0.5 + (-0.2 - 0.5) * 0.5)).abs() < 1e-6);
        b.update(2.);
        assert!(b.current.iter().zip(exterior).all(|(a, e)| (a - e).abs() < 1e-6));
        b.follow(2, interior, 2., 10.);
        b.update(11.);
        assert!((b.current[3] - 2.5).abs() < 1e-6);
        b.update(12.);
        assert!(b.current.iter().zip(interior).all(|(a, e)| (a - e).abs() < 1e-6));
    }
    #[test]
    fn player_states_follow_the_client_order() {
        use crate::special_infected::SpecialClass;
        let mut s = crate::game_state::Match::new(&crate::game_state::tests_weapon());
        assert!(player_state(&s, 100., DEFAULT_PRESET, false, false).is_none());
        // High stress is relative to the controller (0.5 + 0.6, 0 + 0.3)
        // and loses to black-and-white.
        let stress = player_state(&s, 100., DEFAULT_PRESET, true, false).unwrap();
        assert_eq!(stress.0, 13);
        assert!((stress.1[0] - 1.1).abs() < 1e-6 && (stress.1[1] - 0.3).abs() < 1e-6 && stress.2 == 2.);
        let mut strong = DEFAULT_PRESET;
        strong[0] = 1.4;
        assert_eq!(player_state(&s, 100., strong, true, false).unwrap().1[0], 1.5);
        s.black_white = true;
        assert_eq!(player_state(&s, 100., DEFAULT_PRESET, true, false).unwrap().0, 9);
        s.black_white = false;
        s.dead = true;
        assert!(player_state(&s, 100., DEFAULT_PRESET, false, false).is_none());
        assert_eq!(player_state(&s, 100., DEFAULT_PRESET, false, true).unwrap(), (16, PRESETS[16], 0.75));
        s.dead = false;
        s.post_pills = 2.;
        let pills = player_state(&s, 100., DEFAULT_PRESET, false, false).unwrap();
        assert_eq!((pills.0, pills.1[0], pills.2), (1, 3., 0.2));
        s.post_pills = 0.5;
        assert_eq!(player_state(&s, 100., DEFAULT_PRESET, false, false).unwrap().0, 2);
        s.post_pills = 0.;
        s.vomited = crate::special_infected::boomer::IT_DURATION;
        assert!(player_state(&s, 100., DEFAULT_PRESET, false, false).unwrap().1.iter().zip(PRESETS[4]).all(|(a, b)| (a - b).abs() < 1e-5));
        s.vomited = 0.;
        s.black_white = true;
        assert_eq!(player_state(&s, 100., DEFAULT_PRESET, false, false).unwrap().0, 9);
        s.adrenaline_time = 5.;
        assert_eq!(player_state(&s, 100., DEFAULT_PRESET, false, false).unwrap().0, 15);
        s.incap = true;
        let full = player_state(&s, 300., DEFAULT_PRESET, false, false).unwrap();
        assert_eq!((full.0, full.1), (10, PRESETS[10]));
        assert_eq!(player_state(&s, 75., DEFAULT_PRESET, false, false).unwrap().1, PRESETS[11]);
        assert_eq!(player_state(&s, 0., DEFAULT_PRESET, false, false).unwrap().1, PRESETS[12]);
        s.pinned = true;
        s.pinned_by = Some(SpecialClass::Hunter);
        assert_eq!(player_state(&s, 0., DEFAULT_PRESET, false, false).unwrap().1, PRESETS[6]);
        s.incap = false;
        assert_eq!(player_state(&s, 100., DEFAULT_PRESET, false, false).unwrap(), (5, PRESETS[5], 0.5));
        s.pinned_by = Some(SpecialClass::Smoker);
        assert_eq!(player_state(&s, 100., DEFAULT_PRESET, false, false).unwrap(), (7, PRESETS[7], 0.75));
    }
}
