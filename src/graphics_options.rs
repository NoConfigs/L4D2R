//! Persistent video controls and their renderer bindings.
use bevy::{
    image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        render_resource::{AsBindGroup, TextureFormat},
        renderer::{RenderAdapter, RenderDevice},
        view::ColorGrading,
    },
    shader::ShaderRef,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug)]
pub(crate) enum OptionKey {
    Brightness,
    Aspect,
    Resolution,
    Display,
    Grain,
    Aa,
    Filtering,
    Shader,
    Effects,
    Detail,
}
#[derive(Resource, Clone, Debug, PartialEq)]
pub(crate) struct Graphics {
    pub brightness: f32,
    pub grain: f32,
    pub samples: u32,
    pub filtering: u16,
    pub shader: u8,
    pub effects: u8,
    pub detail: u8,
    pub aspect: usize,
    pub resolution: usize,
    pub display: usize,
}
const RESOLUTIONS: [&[(u32, u32)]; 3] = [
    &[
        (1280, 720),
        (1600, 900),
        (1920, 1080),
        (2560, 1440),
        (3840, 2160),
    ],
    &[(1024, 768), (1280, 960), (1600, 1200), (1920, 1440)],
    &[
        (1280, 800),
        (1440, 900),
        (1680, 1050),
        (1920, 1200),
        (2560, 1600),
    ],
];
impl Default for Graphics {
    fn default() -> Self {
        let mut settings = Self::parse(
            &std::fs::read_to_string("runtime/graphics-options.ini").unwrap_or_default(),
        );
        let args: Vec<_> = std::env::args().collect();
        for pair in args.windows(2) {
            if pair[0] == "--capture-msaa"
                && let Ok(value @ (1 | 2 | 4 | 8)) = pair[1].parse::<u32>()
            {
                settings.samples = value;
            }
            if pair[0] == "--capture-grain"
                && let Ok(value) = pair[1].parse::<f32>()
                && value.is_finite()
            {
                settings.grain = value.clamp(0., 1.);
            }
            if pair[0] == "--capture-detail"
                && let Ok(value @ 0..=2) = pair[1].parse::<u8>()
            {
                settings.detail = value;
            }
        }
        TEXTURE_DETAIL.store(settings.detail, std::sync::atomic::Ordering::Relaxed);
        settings
    }
}
/// Texture detail in effect for textures loaded from now on (0 Low .. 2 High).
static TEXTURE_DETAIL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);
/// Sharpest mip levels left out of each map texture at load, like retail
/// `mat_picmip` (Low 2, Medium 1, High 0). They used to be uploaded and
/// only hidden by the sampler: on Medium the never-shown top level was 3/4
/// of every texture's memory. Changes apply from the next map load.
pub(crate) fn texture_skip() -> u32 {
    2 - TEXTURE_DETAIL.load(std::sync::atomic::Ordering::Relaxed).min(2) as u32
}
impl Graphics {
    pub fn recommended() -> Self {
        Self::parse("")
    }
    fn parse(text: &str) -> Self {
        let value = |key: &str, fallback: f32| {
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{key}="))?.parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(fallback)
        };
        let aspect = (value("aspect", 0.) as usize).min(2);
        // Phones: iOS kills an app past 3 GB of shared CPU+GPU memory, and
        // MSAA targets at the native panel size cost hundreds of MB.
        let mobile = cfg!(any(target_os = "ios", target_os = "android"));
        let (samples, detail, effects) = if mobile { (1., 1., 1.) } else { (4., 2., 2.) };
        Self {
            brightness: value("brightness", 1.).clamp(0.5, 1.5),
            // mat_grain_scale_override (retail default 1.0).
            grain: value("grain", 1.).clamp(0., 1.),
            samples: match value("samples", samples) as u32 {
                n @ (1 | 2 | 4 | 8) => n,
                _ => 4,
            },
            filtering: match value("filtering", 8.) as u16 {
                n @ (0 | 1 | 2 | 4 | 8 | 16) => n,
                _ => 8,
            },
            shader: (value("shader", 2.) as u8).min(2),
            effects: (value("effects", effects) as u8).min(2),
            detail: (value("detail", detail) as u8).min(2),
            aspect,
            resolution: (value("resolution", 0.) as usize).min(RESOLUTIONS[aspect].len() - 1),
            display: (value("display", 0.) as usize).min(2),
        }
    }
    fn encode(&self) -> String {
        format!(
            "brightness={}\ngrain={}\nsamples={}\nfiltering={}\nshader={}\neffects={}\ndetail={}\naspect={}\nresolution={}\ndisplay={}\n",
            self.brightness,
            self.grain,
            self.samples,
            self.filtering,
            self.shader,
            self.effects,
            self.detail,
            self.aspect,
            self.resolution,
            self.display
        )
    }
    pub fn save(&self) {
        let _ = std::fs::create_dir_all("runtime");
        if let Err(e) = std::fs::write("runtime/graphics-options.ini", self.encode()) {
            eprintln!("Graphics options: {e}");
        }
    }
    pub fn size(&self) -> (u32, u32) {
        RESOLUTIONS[self.aspect][self.resolution]
    }
    pub fn cycle(&mut self, key: OptionKey) {
        match key {
            OptionKey::Brightness => {
                self.brightness = if self.brightness >= 1.49 {
                    0.5
                } else {
                    ((self.brightness + 0.1) * 10.).round() / 10.
                }
            }
            OptionKey::Grain => {
                self.grain = if self.grain >= 0.99 {
                    0.
                } else {
                    ((self.grain + 0.1) * 10.).round() / 10.
                }
            }
            OptionKey::Aspect => {
                self.aspect = (self.aspect + 1) % 3;
                self.resolution = self.resolution.min(RESOLUTIONS[self.aspect].len() - 1);
            }
            OptionKey::Resolution => {
                self.resolution = (self.resolution + 1) % RESOLUTIONS[self.aspect].len()
            }
            OptionKey::Display => self.display = (self.display + 1) % 3,
            OptionKey::Aa => {
                let values = [1, 2, 4, 8];
                self.samples =
                    values[(values.iter().position(|v| *v == self.samples).unwrap_or(0) + 1) % 4];
            }
            OptionKey::Filtering => {
                let values = [0, 1, 2, 4, 8, 16];
                self.filtering = values[(values
                    .iter()
                    .position(|v| *v == self.filtering)
                    .unwrap_or(0)
                    + 1)
                    % 6];
            }
            OptionKey::Shader => self.shader = (self.shader + 1) % 3,
            OptionKey::Effects => self.effects = (self.effects + 1) % 3,
            OptionKey::Detail => {
                self.detail = (self.detail + 1) % 3;
                TEXTURE_DETAIL.store(self.detail, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    pub fn label(&self, key: OptionKey, caps: Option<&Capabilities>) -> String {
        match key {
            OptionKey::Brightness => format!("{:.0}%", self.brightness * 100.),
            OptionKey::Grain => format!("{:.0}%", self.grain * 100.),
            OptionKey::Aspect => {
                ["Widescreen 16:9", "Normal 4:3", "Widescreen 16:10"][self.aspect].into()
            }
            OptionKey::Resolution => {
                let (w, h) = self.size();
                format!("{w} x {h}")
            }
            OptionKey::Display => {
                ["Windowed", "Windowed (No Border)", "Fullscreen"][self.display].into()
            }
            OptionKey::Aa => {
                let actual = caps.map_or(self.samples, |c| c.resolve(self.samples));
                if self.samples == 1 {
                    "None".into()
                } else if actual == self.samples {
                    format!("{}x MSAA", self.samples)
                } else {
                    format!("{}x requested ({}x supported)", self.samples, actual)
                }
            }
            OptionKey::Filtering => match self.filtering {
                0 => "Bilinear".into(),
                1 => "Trilinear".into(),
                n => format!("Anisotropic {n}x"),
            },
            OptionKey::Shader => ["Low", "Medium", "High"][self.shader as usize].into(),
            OptionKey::Effects => ["Low", "Medium", "High"][self.effects as usize].into(),
            OptionKey::Detail => format!(
                "{} (models next map)",
                ["Low", "Medium", "High"][self.detail as usize]
            ),
        }
    }
    pub fn effect_scale(&self) -> f32 {
        [0.35, 0.65, 1.][self.effects as usize]
    }
    pub fn model_lod(&self) -> usize {
        2 - self.detail as usize
    }
}
#[derive(Resource, Default)]
pub(crate) struct Capabilities(pub Vec<u32>);
impl Capabilities {
    fn resolve(&self, desired: u32) -> u32 {
        self.0
            .iter()
            .copied()
            .filter(|v| *v <= desired)
            .max()
            .unwrap_or(1)
    }
}
fn capabilities(mut commands: Commands, adapter: Res<RenderAdapter>, device: Res<RenderDevice>) {
    let hdr = adapter
        .get_texture_format_features(TextureFormat::Rgba16Float)
        .flags;
    let depth = adapter
        .get_texture_format_features(TextureFormat::Depth32Float)
        .flags;
    let specific = device.features().contains(
        bevy::render::render_resource::WgpuFeatures::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
    );
    let samples: Vec<u32> = [1, 2, 4, 8]
        .into_iter()
        .filter(|n| {
            (*n == 1 || *n == 4 || specific)
                && hdr.sample_count_supported(*n)
                && depth.sample_count_supported(*n)
        })
        .collect();
    println!("Supported MSAA sample counts: {samples:?}");
    // Phones: whether DXT textures can stay compressed (BC) decides most of
    // the texture memory; log it so phone logs show which path ran.
    let f = device.features();
    use bevy::render::render_resource::WgpuFeatures as F;
    println!(
        "GPU {:?}: texture compression BC={} ASTC={} ETC2={}",
        adapter.get_info().name,
        f.contains(F::TEXTURE_COMPRESSION_BC),
        f.contains(F::TEXTURE_COMPRESSION_ASTC),
        f.contains(F::TEXTURE_COMPRESSION_ETC2)
    );
    commands.insert_resource(Capabilities(samples));
}
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub(crate) struct Grain {
    #[uniform(0)]
    controls: Vec4,
}
impl UiMaterial for Grain {
    fn fragment_shader() -> ShaderRef {
        "shaders/film_grain.wgsl".into()
    }
}
#[derive(Component)]
struct GrainOverlay;
pub(crate) fn setup_grain(
    mut commands: Commands,
    mut materials: ResMut<Assets<Grain>>,
    camera: Single<Entity, With<crate::game::ViewCamera>>,
) {
    commands.spawn((
        MaterialNode(materials.add(Grain {
            controls: Vec4::ZERO,
        })),
        Node {
            width: percent(100.),
            height: percent(100.),
            position_type: PositionType::Absolute,
            ..default()
        },
        GlobalZIndex(-1),
        UiTargetCamera(*camera),
        Pickable::IGNORE,
        GrainOverlay,
    ));
}
fn apply(
    time: Res<Time>,
    settings: Res<Graphics>,
    caps: Res<Capabilities>,
    mut cameras: Query<
        (&mut Msaa, &mut ColorGrading),
        Or<(
            With<crate::viewer::GameCamera>,
            With<crate::game::ViewCamera>,
        )>,
    >,
    mut windows: Query<&mut Window>,
    monitors: Query<&bevy::window::Monitor>,
    mut window_options: Local<Option<(usize, usize, usize)>>,
    mut grain: ResMut<Assets<Grain>>,
    mut overlays: Query<(&MaterialNode<Grain>, &mut Node), With<GrainOverlay>>,
) {
    // Full-resolution multisampled HDR targets share iPhone application RAM.
    let samples = match if cfg!(target_os = "ios") { 1 } else { caps.resolve(settings.samples) } {
        8 => Msaa::Sample8,
        4 => Msaa::Sample4,
        2 => Msaa::Sample2,
        _ => Msaa::Off,
    };
    for (mut aa, mut color) in &mut cameras {
        aa.set_if_neq(samples);
        if color.global.exposure != settings.brightness.log2() {
            color.global.exposure = settings.brightness.log2();
        }
    }
    let desired_window = (settings.aspect, settings.resolution, settings.display);
    // iPhone window/drawable dimensions come from UIKit.
    // Desktop resolution presets must not overwrite the OS resize events.
    if !cfg!(target_os = "ios") && *window_options != Some(desired_window) {
        for mut window in &mut windows {
            let (w, h) = settings.size();
            window.resolution.set(w as f32, h as f32);
            window.mode = match settings.display {
                1 => bevy::window::WindowMode::BorderlessFullscreen(
                    bevy::window::MonitorSelection::Current,
                ),
                2 => bevy::window::WindowMode::Fullscreen(
                    bevy::window::MonitorSelection::Current,
                    monitors
                        .iter()
                        .flat_map(|m| m.video_modes.iter())
                        .filter(|m| m.physical_size.x == w && m.physical_size.y == h)
                        .max_by_key(|m| m.refresh_rate_millihertz)
                        .copied()
                        .map(bevy::window::VideoModeSelection::Specific)
                        .unwrap_or(bevy::window::VideoModeSelection::Current),
                ),
                _ => bevy::window::WindowMode::Windowed,
            };
        }
        *window_options = Some(desired_window);
    }
    for (handle, mut node) in &mut overlays {
        node.reborrow()
            .map_unchanged(|n| &mut n.display)
            // Film grain is drawn by engine_post (retail NOISE_ENABLE); the
            // option is its mat_grain_scale_override. This overlay stays off.
            .set_if_neq(Display::None);
        if false
            && let Some(mut material) = grain.get_mut(&handle.0)
        {
            material.controls = Vec4::new(settings.grain, time.elapsed_secs(), 0., 0.);
        }
    }
}
#[derive(Resource, Default)]
pub(crate) struct Originals(
    pub(crate) HashMap<AssetId<crate::viewer::SourceMaterial>, Option<Handle<Image>>>,
);
fn materials(
    settings: Res<Graphics>,
    mut events: MessageReader<AssetEvent<crate::viewer::SourceMaterial>>,
    mut originals: ResMut<Originals>,
    mut materials: ResMut<Assets<crate::viewer::SourceMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let mut ids = HashSet::new();
    for event in events.read() {
        match event {
            AssetEvent::Added { id } => {
                ids.insert(*id);
            }
            AssetEvent::Removed { id } => {
                originals.0.remove(id);
            }
            _ => {}
        }
    }
    if settings.is_changed() {
        ids.extend(materials.iter().map(|(id, _)| id));
    }
    let mut textures = HashSet::new();
    for id in ids {
        if let Some(mut material) = materials.get_mut(id) {
            let normal = originals
                .0
                .entry(id)
                .or_insert_with(|| material.base.normal_map_texture.clone())
                .clone();
            let desired = if settings.shader == 0 {
                None
            } else {
                normal.clone()
            };
            if material.base.normal_map_texture != desired {
                material.base.normal_map_texture = desired;
            }
            textures.extend(
                material
                    .base
                    .base_color_texture
                    .iter()
                    .chain(normal.iter())
                    .map(|h| h.id()),
            );
        }
    }
    for id in textures {
        // Read first: Assets::get_mut marks the image changed, and a changed
        // image is re-extracted and re-uploaded even when nothing differs.
        // Doing that for every map texture at load doubled peak RAM.
        if let Some(image) = images.get(id) {
            let desired = texture_sampler(&image.sampler, &settings);
            // GPU-only map textures have no CPU texels left after upload, so
            // re-extracting them would upload a blank texture. Their filtering
            // changes take effect on the next map load.
            if image.sampler != desired && image.data.is_some() {
                if let Some(mut image) = images.get_mut(id) {
                    image.sampler = desired;
                }
            }
        }
    }
}
/// The world-texture sampler for the current filtering and detail settings.
fn texture_sampler(current: &ImageSampler, settings: &Graphics) -> ImageSampler {
    let mut sampler = match current {
        ImageSampler::Descriptor(d) => d.clone(),
        _ => ImageSamplerDescriptor::linear(),
    };
    sampler.anisotropy_clamp = settings.filtering.max(1);
    sampler.mipmap_filter = if settings.filtering == 0 {
        ImageFilterMode::Nearest
    } else {
        ImageFilterMode::Linear
    };
    sampler.min_filter = ImageFilterMode::Linear;
    sampler.mag_filter = ImageFilterMode::Linear;
    // Detail is applied by leaving mips out at load (texture_skip), so the
    // sampler must not hide further levels on top of that.
    let _ = settings.detail;
    sampler.lod_min_clamp = 0.;
    ImageSampler::Descriptor(sampler)
}
/// Map textures reserved by `viewer::setup` and added a few at a time, so
/// their GPU upload staging is spread over the loading frames instead of
/// needing every texel of the map in RAM twice in one frame.
#[derive(Resource, Default)]
pub(crate) struct PendingTextures(pub(crate) std::collections::VecDeque<(Handle<Image>, Image)>);
fn add_pending_textures(
    settings: Res<Graphics>,
    mut pending: ResMut<PendingTextures>,
    mut images: ResMut<Assets<Image>>,
) {
    let mut bytes = 0;
    let budget = if cfg!(target_os = "ios") || std::env::args().any(|a| a == "--profile-mobile-memory") {
        8 << 20
    } else { 128 << 20 };
    while bytes < budget {
        let Some((handle, mut image)) = pending.0.pop_front() else {
            break;
        };
        bytes += image.data.as_ref().map_or(0, Vec::len);
        image.sampler = texture_sampler(&image.sampler, &settings);
        let _ = images.insert(handle.id(), image);
    }
}
pub(crate) fn install(app: &mut App) {
    app.init_resource::<Graphics>()
        .init_resource::<PendingTextures>()
        .add_systems(Update, add_pending_textures)
        .init_resource::<Capabilities>()
        .init_resource::<Originals>()
        .add_plugins(UiMaterialPlugin::<Grain>::default())
        .add_systems(Startup, capabilities)
        .add_systems(Update, (apply, materials));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_props_load_lower_detail_without_invalid_meshes() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let paths: Vec<_> = assets
            .matching_paths("models/props_vehicles/")
            .into_iter()
            .filter(|p| p.ends_with(".mdl"))
            .take(8)
            .collect();
        let mut reduced = 0;
        for path in paths {
            let high = crate::studio::load_lod(&mut assets, &path, 0, 0, 0).unwrap();
            let low = crate::studio::load_lod(&mut assets, &path, 0, 0, 2).unwrap();
            let count = |draws: &[crate::studio::ModelMesh]| {
                draws.iter().map(|d| d.positions.len()).sum::<usize>()
            };
            assert!(count(&low) <= count(&high), "{path}");
            if count(&low) < count(&high) {
                reduced += 1;
            }
            for draw in low {
                assert_eq!(draw.positions.len(), draw.normals.len());
                assert_eq!(draw.positions.len() % 3, 0);
            }
        }
        assert!(
            reduced > 0,
            "Expected an authored vehicle LOD to reduce geometry"
        );
    }
    #[test]
    fn options_roundtrip_and_cycles_stay_valid() {
        let mut s = Graphics::parse("");
        for _ in 0..25 {
            for key in [
                OptionKey::Aa,
                OptionKey::Filtering,
                OptionKey::Aspect,
                OptionKey::Resolution,
                OptionKey::Display,
                OptionKey::Grain,
                OptionKey::Brightness,
                OptionKey::Shader,
                OptionKey::Effects,
                OptionKey::Detail,
            ] {
                s.cycle(key);
            }
            assert_eq!(Graphics::parse(&s.encode()), s);
        }
    }
    #[test]
    fn unsupported_msaa_falls_back_without_losing_requested_value() {
        let c = Capabilities(vec![1, 4]);
        assert_eq!(c.resolve(8), 4);
        assert_eq!(c.resolve(2), 1);
        assert_eq!(Graphics::parse("samples=8").samples, 8);
    }
    #[test]
    fn low_texture_detail_preserves_no_mip_data_samplers() {
        let original = ImageSampler::Descriptor(ImageSamplerDescriptor {
            lod_max_clamp: 0.,
            ..ImageSamplerDescriptor::linear()
        });
        for detail in 0..=2 {
            let settings = Graphics::parse(&format!("detail={detail}"));
            let ImageSampler::Descriptor(sampler) = texture_sampler(&original, &settings) else { panic!() };
            assert_eq!(sampler.lod_min_clamp, 0.);
            assert_eq!(sampler.lod_max_clamp, 0.);
        }
    }
}
