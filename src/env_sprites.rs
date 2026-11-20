//! Map `env_sprite` glows (lamp halos, signal lights).
//!
//! 631 env_sprites across 26 retail maps, nearly all render mode 9 (world
//! space glow), were not drawn at all. Each is a camera-facing additive quad
//! of its sprite material's base texture, sized texture width x `scale`
//! units (CSprite), tinted by rendercolor x renderamt x HDRColorScale.
//! Retail world glows skip the depth test and fade with a pixel-visibility
//! query; here the quad is pulled toward the camera so the lamp it sits in
//! does not clip it, and a sight trace from the eye fades it out when a wall
//! is in the way. Not reproduced: animated frames, sprite toggling inputs
//! (ShowSprite/HideSprite), screen-space glow sizing (render mode 3).
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::{NoFrustumCulling, RenderLayers},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::collections::HashMap;

const UNIT: f32 = crate::collision::UNIT;
/// Seconds for a glow to fade fully in or out as its sight line changes.
const FADE: f32 = 0.1;

#[derive(Component)]
struct SpriteDraw;

struct Glow {
    entity: Entity,
    material: Handle<StandardMaterial>,
    position: Vec3,
    color: Vec3,
    /// GlowProxySize (metres): how far toward the eye the sight line starts.
    proxy: f32,
    /// 0..1, eased toward `target`.
    visible: f32,
    /// The last sight-line result.
    target: f32,
}

#[derive(Default)]
struct State {
    map: usize,
    glows: Vec<Glow>,
    /// Round-robin position of the sight-line checks.
    cursor: usize,
}
/// Sight-line checks per frame: a long brush trace costs ~0.4 ms on big
/// maps, and c2m1's 150 glows traced every frame cost ~60 ms.
const TRACES_PER_FRAME: usize = 12;

/// Calibrated against live retail's `r_drawsprites 0` difference on c2m2's
/// Whispering Oaks marquee (19 Nov): every letter's bulbs glow, with small
/// halos, and the start-room lamp glow above the ceiling stays hidden.
/// `L4D2R_NO_SPRITES=1` turns them off.
pub fn install(app: &mut App) {
    if std::env::var_os("L4D2R_NO_SPRITES").is_none() {
        app.add_systems(Update, update);
    }
}

/// Overall glow brightness fitted to live retail (c2m2 marquee, 19 Nov,
/// `r_drawsprites 0` difference): the unscaled glows added ~4x retail's
/// light per bulb. Stands in for StandardGlowBlend's partial pixel
/// visibility and distance fade, which are not reproduced exactly.
const GLOW_GAIN: f32 = 0.25;
/// The enabled sprites of a map: (origin, model, colour, scale, proxy size).
pub(crate) fn sprites(entities: &[std::collections::BTreeMap<String, String>]) -> Vec<(Vec3, String, Vec3, f32, f32)> {
    entities
        .iter()
        .filter(|e| e.get("classname").is_some_and(|c| c == "env_sprite"))
        .filter_map(|e| {
            let flags: u32 = e.get("spawnflags").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
            // CSprite::Spawn: a named sprite without "Start on" spawns off.
            let named = e.get("targetname").is_some_and(|n| !n.is_empty());
            if named && flags & 1 == 0 {
                return None;
            }
            let model = e.get("model")?.replace('\\', "/").to_ascii_lowercase();
            let numbers = |key: &str, default: &[f32]| -> Vec<f32> {
                let v: Vec<f32> = e
                    .get(key)
                    .map(|s| s.split_whitespace().filter_map(|x| x.parse().ok()).collect())
                    .unwrap_or_default();
                if v.len() >= default.len() { v } else { default.to_vec() }
            };
            let origin = numbers("origin", &[0., 0., 0.]);
            let rgb = numbers("rendercolor", &[255., 255., 255.]);
            let amount = numbers("renderamt", &[255.])[0].clamp(0., 255.) / 255.;
            let hdr = numbers("HDRColorScale", &[0.7])[0].max(0.);
            let scale = numbers("scale", &[1.])[0].max(0.);
            let proxy = numbers("GlowProxySize", &[2.])[0].clamp(0., 64.) * UNIT;
            if amount <= 0. || scale <= 0. {
                return None;
            }
            let color = Color::srgb(rgb[0] / 255., rgb[1] / 255., rgb[2] / 255.).to_linear();
            Some((
                Vec3::from_array(crate::binary::source_position([origin[0], origin[1], origin[2]])),
                model,
                Vec3::new(color.red, color.green, color.blue) * amount * hdr * GLOW_GAIN,
                scale,
                proxy,
            ))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    map: Option<Res<crate::viewer::Map>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    collision: Option<Res<crate::collision::Collision>>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut transforms: Query<&mut Transform, With<SpriteDraw>>,
    mut state: Local<State>,
) {
    let _timing = crate::performance::scope("env_sprites::update");
    let Some(map) = map else { return };
    let world = &map.0.world;
    let id = world.entities.as_ptr() as usize ^ world.entities.len();
    if state.map != id {
        state.map = id;
        for glow in state.glows.drain(..) {
            commands.entity(glow.entity).try_despawn();
        }
        let Some(source) = source.as_deref() else { return };
        let Ok(mut assets) = crate::source_assets::SourceAssets::new(&source.0, vec![]) else { return };
        // Texture per sprite material: (image, width in texels).
        let mut textures: HashMap<String, Option<(Handle<Image>, f32)>> = HashMap::new();
        let quad = meshes.add(Rectangle::new(1., 1.));
        for (position, model, color, scale, proxy) in sprites(&world.entities) {
            let texture = textures
                .entry(model.clone())
                .or_insert_with(|| {
                    let name = model.trim_end_matches(".vmt").trim_end_matches(".spr");
                    let material = crate::material::load(&mut assets, name).ok()?;
                    let t = crate::vtf::decode(&assets.read(material.base.as_ref()?).ok()?, 512).ok()?;
                    let width = t.width as f32;
                    let image = images.add(Image::new(
                        Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
                        TextureDimension::D2,
                        t.rgba,
                        TextureFormat::Rgba8UnormSrgb,
                        RenderAssetUsages::default(),
                    ));
                    Some((image, width))
                })
                .clone();
            let Some((image, width)) = texture else { continue };
            let size = width * scale * UNIT;
            let material = materials.add(StandardMaterial {
                base_color: Color::linear_rgb(0., 0., 0.),
                base_color_texture: Some(image),
                unlit: true,
                alpha_mode: AlphaMode::Add,
                cull_mode: None,
                // Additive glows fade to black in fog, not toward its colour.
                fog_enabled: false,
                ..default()
            });
            let entity = commands
                .spawn((
                    SpriteDraw,
                    Mesh3d(quad.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::from_translation(position).with_scale(Vec3::splat(size)),
                    NoFrustumCulling,
                    RenderLayers::layer(1),
                    bevy::light::NotShadowCaster,
                ))
                .id();
            state.glows.push(Glow { entity, material, position, color, proxy, visible: 0., target: 0. });
        }
        if !state.glows.is_empty() {
            println!("Map sprites: {}", state.glows.len());
        }
        return;
    }
    if state.glows.is_empty() {
        return;
    }
    let Some((eye, forward)) = camera.iter().next().map(|c| (c.translation(), c.forward().as_vec3())) else { return };
    let step = (time.delta_secs() / FADE).min(1.);
    let count = state.glows.len();
    let first = state.cursor % count;
    state.cursor = (first + TRACES_PER_FRAME) % count;
    for (index, glow) in state.glows.iter_mut().enumerate() {
        let to_eye = eye - glow.position;
        let distance = to_eye.length();
        let due = (index + count - first) % count < TRACES_PER_FRAME;
        // Behind the camera: not drawn, nothing to trace.
        if forward.dot(-to_eye) <= 0. {
            glow.target = 0.;
        } else if due {
            glow.target = match collision.as_deref() {
            // A short way out of the lamp model so its own brush does not
            // block the line.
            // Retail tests a GlowProxySize quad around the sprite: start the
            // line that far toward the eye, so a bulb set into its sign
            // (c2m2's OAKS letters) is not hidden by the sign itself.
            Some(c) if distance > glow.proxy + 0.05 => {
                let start = glow.position + to_eye / distance * (glow.proxy + 0.05);
                // World brushes only, solid/opaque contents: retail's depth
                // test sees glows through chain-link (c2m2's OAKS letters
                // stand behind fence002) and windows, and lamp props.
                let source = |p: Vec3| [p.x / UNIT, -p.z / UNIT, p.y / UNIT];
                match c.source_ray(source(eye), source(start), 0x1 | 0x80, false) {
                    Some((fraction, _, _, solid)) if fraction < 0.99 || solid => 0.,
                    _ => 1.,
                }
            }
            _ => 1.,
            };
        }
        glow.visible += (glow.target - glow.visible).clamp(-step, step);
        if let Ok(mut transform) = transforms.get_mut(glow.entity) {
            let half = transform.scale.x * 0.5;
            // Face the camera, pulled toward it so nearby geometry does not
            // cut the glow (retail draws world glows without a depth test).
            let pull = half.min(distance * 0.5);
            let at = if distance > 1e-3 { glow.position + to_eye / distance * pull } else { glow.position };
            let looking = Transform::from_translation(at).looking_at(eye, Vec3::Y);
            transform.translation = at;
            transform.rotation = looking.rotation;
        }
        // `get_mut` marks the asset changed (a re-upload) even when nothing
        // differs; with ~150 glows that cost c2m1 ~50 ms a frame.
        let c = glow.color * glow.visible;
        let color = Color::linear_rgb(c.x, c.y, c.z);
        if materials.get(&glow.material).is_some_and(|m| m.base_color != color) {
            if let Some(mut material) = materials.get_mut(&glow.material) {
                material.base_color = color;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entity(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
    #[test]
    fn sprites_follow_start_on_and_render_settings() {
        let list = sprites(&[
            entity(&[("classname", "env_sprite"), ("origin", "100 0 0"), ("model", "sprites/glow06.vmt"),
                ("rendermode", "9"), ("rendercolor", "255 255 255"), ("renderamt", "255"), ("scale", "0.5")]),
            // Named without "Start on": spawns off.
            entity(&[("classname", "env_sprite"), ("targetname", "lamp"), ("spawnflags", "0"), ("model", "sprites/glow06.vmt")]),
            entity(&[("classname", "env_sprite"), ("targetname", "lamp2"), ("spawnflags", "1"), ("model", "sprites/glow06.vmt"),
                ("renderamt", "0")]),
            entity(&[("classname", "env_sprite"), ("targetname", "lamp3"), ("spawnflags", "1"), ("model", "Sprites\\Glow06.vmt"),
                ("renderamt", "127"), ("HDRColorScale", "1")]),
        ]);
        assert_eq!(list.len(), 2);
        assert!((list[0].0.x - 100. * UNIT).abs() < 1e-5);
        assert!((list[0].2.x - 0.7 * GLOW_GAIN).abs() < 1e-5, "default HDRColorScale 0.7");
        assert!((list[0].4 - 2. * UNIT).abs() < 1e-6, "default GlowProxySize 2");
        assert_eq!(list[1].1, "sprites/glow06.vmt");
        assert!((list[1].2.x - 127. / 255. * GLOW_GAIN).abs() < 1e-5);
    }
}

/// L4D2R_TRANSFORM_DEBUG: once a second, how many entities changed their
/// Transform this frame, grouped by name prefix (transform propagation cost).
pub fn install_transform_debug(app: &mut App) {
    if std::env::var_os("L4D2R_TRANSFORM_DEBUG").is_some() {
        app.add_systems(PostUpdate, transform_debug);
    }
}
fn transform_debug(
    changed: Query<(Option<&Name>, Option<&ChildOf>, Has<Mesh3d>, Has<Children>), Changed<Transform>>,
    names: Query<&Name>,
    mut tick: Local<u32>,
) {
    *tick += 1;
    if *tick % 60 != 0 {
        return;
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (name, parent, mesh, children) in &changed {
        let label = name
            .map(|n| n.as_str().to_string())
            .or_else(|| parent.and_then(|p| names.get(p.parent()).ok()).map(|n| format!("child of {}", n.as_str())))
            .unwrap_or_else(|| format!("<unnamed mesh={mesh} children={children} parented={}>", parent.is_some()));
        let key: String = label.split(['/', '#', ' ']).take(2).collect::<Vec<_>>().join(" ");
        *counts.entry(key).or_default() += 1;
    }
    let mut top: Vec<_> = counts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    println!("L4D2R changed transforms: {}: {:?}", top.iter().map(|t| t.1).sum::<usize>(), &top[..top.len().min(8)]);
}
