//! Survivor bile screen effect: the native `boomer_vomit_screeneffect`
//! particle system (particles/boomer_fx.pcf, "screen space effect" +
//! "view model effect") drawn as screen sprites, like the Mudman splatter.
//!
//! Native definition: 3 sprites at once, then 3/s from 0.15 s for 11 s with
//! the emitter fading over 8–11 s; lifetime 3–4 s; alpha 180–230, scaled
//! from 1 down to 0.4 over 7–11 s; radius 100–150, scaled 1 to 2 over
//! 8–11 s; position box x ±120, z -80..-40 at depth 180–200, drifting away
//! at 100–200 units/s; 0.75 s alpha fade-out; random rotation and spin.
//! The child system boomer_vomit_screeneffect_b adds 12 splats at once:
//! lifetime 5-6 s, alpha 225-255, radius 70-100 grown from 0.2 to 1.5 of it
//! in the first 2.5% of life, sliding down (gravity 22, noise -20..3).
//!
//! As retail, the sprites are not drawn on screen but into an offscreen
//! refraction buffer (the texture is a refraction map), which
//! vomit_post.rs uses to refract and tint the frame (engine_post).
use crate::{game_state::Match, source_assets::SourceAssets};
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use l4d2r_game_core::random::UniformRandomStream;

#[derive(Resource)]
struct BileAssets {
    image: Handle<Image>,
    sequences: Vec<crate::source_pcf::SheetSequence>,
    size: Vec2,
    rng: UniformRandomStream,
}
#[derive(Resource, Default)]
struct BileEmitter {
    /// Seconds since the current splash began (`None`: idle).
    clock: Option<f32>,
    emitted: f32,
    previous: f32,
}
/// The UI camera that draws the sprites into the vomit buffer.
#[derive(Component)]
struct BufferCamera;
#[derive(Component)]
struct BileSprite {
    /// Child system (boomer_vomit_screeneffect_b) splat.
    splat: bool,
    age: f32,
    life: f32,
    /// Emitter clock at birth (drives the initial-scalar remaps).
    born: f32,
    x: f32,
    z: f32,
    depth: f32,
    speed: f32,
    radius: f32,
    alpha: f32,
    angle: f32,
    spin: f32,
}

const DURATION: f32 = 11.;
const RATE: f32 = 3.;
const START: f32 = 0.15;
const FADE_OUT: f32 = 0.75;

pub fn install(app: &mut App) {
    app.init_resource::<BileEmitter>()
        .add_plugins(crate::vomit_post::VomitPostPlugin)
        .add_systems(Update, (setup, buffer, update, post).chain());
}

fn setup(
    mut commands: Commands,
    source: Res<crate::game_ui::HudSource>,
    existing: Option<Res<BileAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut tried: Local<bool>,
) {
    if existing.is_some() || *tried {
        return;
    }
    *tried = true;
    let load = || -> std::io::Result<(crate::vtf::Texture, Vec<crate::source_pcf::SheetSequence>)> {
        let mut assets = SourceAssets::new(&source.0, vec![])?;
        let material = crate::material::load(&mut assets, "particle/screenspaceboomervomit.vmt")?;
        let data = assets.read(
            &material
                .base
                .ok_or_else(|| std::io::Error::other("Missing bile screen texture"))?,
        )?;
        Ok((crate::vtf::decode(&data, 2048)?, crate::source_pcf::sheet(&data)?))
    };
    match load() {
        Ok((t, sequences)) => {
            // The raw refraction map, unconverted (linear), for the buffer.
            let size = Vec2::new(t.width as f32, t.height as f32);
            let image = images.add(Image::new(
                Extent3d {
                    width: t.width,
                    height: t.height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                t.rgba,
                TextureFormat::Rgba8Unorm,
                RenderAssetUsages::default(),
            ));
            commands.insert_resource(BileAssets {
                image,
                sequences,
                size,
                rng: UniformRandomStream::new(5021),
            });
        }
        Err(e) => eprintln!("Native bile screen load: {e}"),
    }
}

/// engine_post.vmt vomit colours. The sprite texture is a normal-map style
/// refraction source (RG offsets around 0.5, blue high); retail renders the
/// sprites into a buffer, refracts the frame by them ($vomitRefractScale
/// 0.22) and blends these colours in. Without that post pass the sprites are
/// drawn directly in the blended colour, keyed by the texture's blue and
/// alpha; the screen refraction is not reproduced.
const VOMIT_COLOR_1: [f32; 3] = [0.675, 0.735, 0.0];
const VOMIT_COLOR_2: [f32; 3] = [0.599, 0.646, 0.0];
#[allow(dead_code)]
fn colorize(rgba: &mut [u8]) {
    for px in rgba.chunks_mut(4) {
        let t = (px[2] as f32 / 255.).clamp(0., 1.);
        for c in 0..3 {
            let v = VOMIT_COLOR_2[c] + (VOMIT_COLOR_1[c] - VOMIT_COLOR_2[c]) * t;
            px[c] = (v * 255.).round() as u8;
        }
    }
}
/// Linear remap of `value` from `in_min..in_max` onto `out_min..out_max`,
/// clamped (PCF "Remap Initial Scalar").
fn remap(value: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> f32 {
    let t = ((value - in_min) / (in_max - in_min)).clamp(0., 1.);
    out_min + (out_max - out_min) * t
}
/// Emitter rate scale: "operator start/end fadeout" 8 -> 11 s.
fn rate_scale(clock: f32) -> f32 {
    1. - remap(clock, 8., 11., 0., 1.)
}
fn sprite_alpha(s: &BileSprite) -> f32 {
    let fade_in = (s.age / 0.1).clamp(0., 1.);
    let left = s.life - s.age;
    let fade_out = if left < FADE_OUT {
        (left / FADE_OUT).clamp(0., 1.).powf(0.65)
    } else {
        1.
    };
    s.alpha * fade_in * fade_out
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    time: Res<Time>,
    mut state: ResMut<Match>,
    mut emitter: ResMut<BileEmitter>,
    mut fixture: Local<bool>,
    mut assets: Option<ResMut<BileAssets>>,
    cameras: Query<Entity, With<BufferCamera>>,
    mut sprites: Query<(Entity, &mut BileSprite, &mut ImageNode, &mut Node, &mut UiTransform)>,
) {
    let Some(ref mut assets) = assets else {
        return;
    };
    let dt = if state.paused { 0. } else { time.delta_secs() };
    // Opt-in render fixture: the survivor player is biled at 1 s.
    if !*fixture && !state.paused && state.elapsed > 1. && std::env::args().any(|a| a == "--capture-bile") {
        *fixture = true;
        state.vomited = crate::special_infected::boomer::IT_DURATION;
    }
    // A fresh coat (the timer jumped up) restarts the splash.
    if !state.infected && !state.dead && state.vomited > emitter.previous + 0.5 {
        emitter.clock = Some(0.);
        emitter.emitted = 0.;
    }
    emitter.previous = state.vomited;
    if state.infected || state.dead {
        emitter.clock = None;
        for (id, ..) in &sprites {
            commands.entity(id).despawn();
        }
        return;
    }
    for (id, mut s, mut image, mut node, mut transform) in &mut sprites {
        s.age += dt;
        if s.age >= s.life {
            commands.entity(id).despawn();
            continue;
        }
        // Placement is approximate: the native view-model space axes are not
        // confirmed, so the box/velocity are used as screen units (480 = the
        // screen height, as the Mudman effect) with the drift sliding the
        // splash down the screen.
        if s.splat {
            // Gravity 22 down the screen with the noise drift; the radius
            // grows 0.2 -> 1.5 over the first 2.5% of life (bias 0.75).
            s.speed += 22. * dt;
            s.z -= s.speed * dt;
        } else {
            s.z -= s.speed * 0.3 * dt;
        }
        s.angle += s.spin * dt;
        let radius = if s.splat {
            let t = (s.age / (s.life * 0.025)).clamp(0., 1.);
            s.radius * (0.2 + 1.3 * t.powf(0.75))
        } else {
            s.radius
        };
        let x = s.x;
        let y = -s.z;
        node.width = vh(radius * 2. / 4.8);
        node.height = vh(radius * 2. / 4.8);
        node.margin = UiRect {
            left: vh((x - radius) / 4.8),
            top: vh((y - radius) / 4.8),
            ..default()
        };
        *transform = UiTransform::from_rotation(Rot2::radians(s.angle));
        image.color = image.color.with_alpha(sprite_alpha(&s));
        if let Some(seq) = assets.sequences.first() {
            let mut clock = (s.age / s.life * seq.duration).min(seq.duration);
            let mut uv = seq.frames.last().map(|f| f.1);
            for (duration, frame) in &seq.frames {
                if clock < *duration {
                    uv = Some(*frame);
                    break;
                }
                clock -= *duration;
            }
            if let Some(uv) = uv {
                image.rect = Some(Rect::new(
                    uv[0] * assets.size.x,
                    uv[1] * assets.size.y,
                    uv[2] * assets.size.x,
                    uv[3] * assets.size.y,
                ));
            }
        }
    }
    let Some(clock) = emitter.clock else {
        return;
    };
    let next = clock + dt;
    emitter.clock = (next < DURATION).then_some(next);
    let Some(camera) = cameras.iter().next() else {
        return;
    };
    // emit_instantaneously 3, then emit_continuously.
    let mut births = if clock == 0. { 3 } else { 0 };
    let assets = &mut **assets;
    // boomer_vomit_screeneffect_b: 12 splats at once.
    if clock == 0. {
        for _ in 0..12 {
            let rng = &mut assets.rng;
            let sprite = BileSprite {
                splat: true,
                age: 0.,
                life: rng.random_float(5., 6.),
                born: 0.,
                x: rng.random_float(-110., 110.),
                z: rng.random_float(-25., 90.),
                depth: rng.random_float(40., 80.),
                speed: rng.random_float(-3., 20.),
                radius: rng.random_float(70., 100.),
                alpha: rng.random_float(225., 255.) / 255.,
                angle: rng.random_float(0., std::f32::consts::TAU),
                spin: 0.,
            };
            let mut image = ImageNode::new(assets.image.clone());
            image.color = Color::WHITE.with_alpha(0.);
            image.flip_x = rng.random_int(0, 1) == 1;
            commands.spawn((
                Node { position_type: PositionType::Absolute, left: percent(50.), top: percent(50.), ..default() },
                image,
                UiTargetCamera(camera),
                sprite,
            ));
        }
    }
    if next > START {
        emitter.emitted += RATE * rate_scale(next) * dt;
        while emitter.emitted >= 1. {
            emitter.emitted -= 1.;
            births += 1;
        }
    }
    for _ in 0..births {
        let rng = &mut assets.rng;
        let born = next;
        let life = rng.random_float(3., 4.);
        let alpha = rng.random_float(180., 230.) / 255. * remap(born, 7., 11., 1., 0.4);
        let radius = rng.random_float(100., 150.) * remap(born, 8., 11., 1., 2.);
        let sprite = BileSprite {
            splat: false,
            age: 0.,
            life,
            born,
            x: rng.random_float(-120., 120.),
            z: rng.random_float(-40., 100.),
            depth: rng.random_float(180., 200.),
            speed: rng.random_float(100., 200.),
            radius,
            alpha,
            angle: rng.random_float(0., std::f32::consts::TAU),
            spin: rng.random_float(-1., 1.),
        };
        let flip = rng.random_int(0, 1) == 1;
        let mut image = ImageNode::new(assets.image.clone());
        image.color = Color::WHITE.with_alpha(0.);
        if flip {
            image.flip_x = true; // Rotation Yaw Flip Random
        }
        let _ = sprite.born;
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: percent(50.),
                top: percent(50.),
                ..default()
            },
            image,
            UiTargetCamera(camera),
            sprite,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emitter_fades_and_remaps_follow_the_native_definition() {
        assert_eq!(rate_scale(5.), 1.);
        assert!((rate_scale(9.5) - 0.5).abs() < 1e-6);
        assert_eq!(rate_scale(11.), 0.);
        assert_eq!(remap(3., 7., 11., 1., 0.4), 1.);
        assert!((remap(11., 7., 11., 1., 0.4) - 0.4).abs() < 1e-6);
    }
    #[test]
    fn owned_bile_screen_material_has_a_texture() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let material = crate::material::load(&mut assets, "particle/screenspaceboomervomit.vmt").unwrap();
        let data = assets.read(&material.base.unwrap()).unwrap();
        let texture = crate::vtf::decode(&data, 2048).unwrap();
        assert!(texture.width > 0 && texture.height > 0);
    }
}

/// The offscreen vomit buffer and its camera: half the window, the window
/// aspect, cleared to "no refraction" (RG 0.5, A 0). Rebuilt after map
/// changes and window resizes.
fn buffer(
    mut commands: Commands,
    windows: Query<&Window>,
    cameras: Query<Entity, With<BufferCamera>>,
    mut images: ResMut<Assets<Image>>,
    buffer: Option<Res<crate::vomit_post::VomitBuffer>>,
    mut size: Local<UVec2>,
) {
    let Some(window) = windows.iter().next() else { return };
    let wanted = UVec2::new((window.physical_width() / 2).max(64), (window.physical_height() / 2).max(64));
    if *size == wanted && buffer.is_some() && !cameras.is_empty() {
        return;
    }
    for camera in &cameras {
        commands.entity(camera).despawn();
    }
    *size = wanted;
    let mut image = Image::new_target_texture(wanted.x, wanted.y, TextureFormat::Rgba8Unorm, None);
    image.asset_usage = RenderAssetUsages::RENDER_WORLD;
    let handle = images.add(image);
    commands.spawn((
        Camera2d,
        Camera {
            order: -20,
            clear_color: ClearColorConfig::Custom(Color::linear_rgba(0.5, 0.5, 0., 0.)),
            ..default()
        },
        bevy::camera::RenderTarget::Image(handle.clone().into()),
        Msaa::Off,
        BufferCamera,
    ));
    commands.insert_resource(crate::vomit_post::VomitBuffer(handle));
}

/// The refraction pass on the world camera while sprites are alive.
fn post(
    mut commands: Commands,
    sprites: Query<(), With<BileSprite>>,
    views: Query<(Entity, Has<crate::vomit_post::VomitPost>), With<crate::viewer::GameCamera>>,
) {
    let on = !sprites.is_empty();
    for (camera, has) in &views {
        if on && !has {
            commands.entity(camera).insert(crate::vomit_post::VomitPost::retail());
        } else if !on && has {
            commands.entity(camera).remove::<crate::vomit_post::VomitPost>();
        }
    }
}
