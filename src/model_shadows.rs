//! Dynamic model shadows: Source's render-to-texture shadows, aimed and
//! coloured by the map's `shadow_control`.
//!
//! Retail casts a blurred shadow texture from each dynamic model (players,
//! infected, physics/dynamic props) onto world brushes along the
//! `shadow_control` angles. The shadow modulates the surface toward the
//! `color` and fades out over `distance` units from the caster.
//!
//! Here one directional shadow map is rendered from those casters only (they
//! join `LAYER`; world and static geometry do not cast), and the world shader
//! (source_lighting.wgsl `model_shadow`) reads the occluder depth to apply
//! the colour and the distance fade. Not reproduced: per-caster directions
//! from nearby lights (`enableshadowsfromlocallights`), the shadow texture's
//! blur kernel, and shadows on static props.
use bevy::{camera::visibility::RenderLayers, light::CascadeShadowConfigBuilder, prelude::*};
use std::collections::BTreeMap;

/// Render layer of shadow-casting dynamic models.
pub const LAYER: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowControl {
    /// Direction the shadows are cast (from the light toward the ground).
    pub direction: Vec3,
    /// Shadow colour, linear.
    pub color: Vec3,
    /// Maximum cast distance, metres.
    pub distance: f32,
    pub enabled: bool,
}
impl Default for ShadowControl {
    /// Source defaults without a shadow_control: straight down, colour
    /// 64 64 64, 75 units.
    fn default() -> Self {
        Self {
            direction: Vec3::NEG_Y,
            color: Vec3::splat(srgb_to_linear(64. / 255.)),
            distance: 75. * crate::collision::UNIT,
            enabled: true,
        }
    }
}
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
pub fn read(entities: &[BTreeMap<String, String>]) -> ShadowControl {
    let Some(e) = entities
        .iter()
        .find(|e| e.get("classname").is_some_and(|c| c == "shadow_control"))
    else {
        return ShadowControl::default();
    };
    let numbers = |key: &str| -> Vec<f32> {
        e.get(key)
            .map(|v| v.split_whitespace().filter_map(|x| x.parse::<f32>().ok()).collect())
            .unwrap_or_default()
    };
    let mut control = ShadowControl::default();
    let angles = numbers("angles");
    if angles.len() >= 2 {
        // Source AngleVectors forward: pitch down positive.
        let (pitch, yaw) = (angles[0].to_radians(), angles[1].to_radians());
        let source = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), -pitch.sin());
        control.direction = Vec3::new(source.x, source.z, -source.y).normalize_or(Vec3::NEG_Y);
    }
    let color = numbers("color");
    if color.len() >= 3 {
        control.color = Vec3::new(color[0], color[1], color[2])
            .map(|c| srgb_to_linear((c / 255.).clamp(0., 1.)));
    }
    if let Some(d) = numbers("distance").first() {
        control.distance = d.max(0.) * crate::collision::UNIT;
    }
    control.enabled = e.get("disableallshadows").is_none_or(|v| v.trim() != "1");
    control
}
/// Source measures the cast distance beyond the caster's own bounds, but a
/// shadow map only stores the caster's light-facing surface (a standing
/// survivor's head with a straight-down light). A typical model height is
/// added so the fade starts at roughly the caster's far side; approximate.
const CASTER_EXTENT: f32 = 72. * crate::collision::UNIT;
/// Lighting-table globals [3] (colour, enabled) and [4] (fade distance from
/// the stored occluder surface).
pub fn globals(control: &ShadowControl) -> [Vec4; 2] {
    [
        control.color.extend(if control.enabled { 1. } else { 0. }),
        Vec4::new(control.distance + CASTER_EXTENT, 0., 0., 0.),
    ]
}

#[derive(Component)]
struct ShadowSun;

pub fn install(app: &mut App) {
    // Phones: half-size shadow maps. The directional map (16 -> 4 MiB) and
    // the point-light cube map (24 -> 6 MiB) are fixed GPU costs on every map.
    let mobile = cfg!(any(target_os = "ios", target_os = "android"))
        || std::env::args().any(|a| a == "--profile-mobile-memory");
    app.insert_resource(bevy::light::DirectionalLightShadowMap { size: if mobile { 1024 } else { 2048 } })
        .add_systems(Update, (sun, casters));
    if mobile {
        app.insert_resource(bevy::light::PointLightShadowMap { size: 512 });
    }
}

fn sun(
    mut commands: Commands,
    map: Option<Res<crate::viewer::Map>>,
    graphics: Res<crate::graphics_options::Graphics>,
    mut suns: Query<(Entity, &mut DirectionalLight, &mut Transform), With<ShadowSun>>,
) {
    let Some(map) = map else {
        return;
    };
    let control = read(&map.0.world.entities);
    let enabled = control.enabled && graphics.shader >= 1;
    let up = if control.direction.cross(Vec3::Y).length_squared() < 1e-6 { Vec3::X } else { Vec3::Y };
    let transform = Transform::default().looking_to(control.direction, up);
    match suns.single_mut() {
        Ok((_, mut light, mut current)) => {
            if light.shadow_maps_enabled != enabled {
                light.shadow_maps_enabled = enabled;
            }
            if current.rotation != transform.rotation {
                current.rotation = transform.rotation;
            }
        }
        Err(_) => {
            commands.spawn((
                DirectionalLight {
                    // Contributes no light: the world is lightmapped and the
                    // shadow is applied as a modulation in the world shader.
                    illuminance: 0.,
                    color: Color::BLACK,
                    shadow_maps_enabled: enabled,
                    ..default()
                },
                // One 15 m cascade: Source limits rendered model shadows to
                // nearby casters too, and a second cascade re-rendered the
                // whole horde (Hotel horde p50 +3 ms).
                CascadeShadowConfigBuilder {
                    num_cascades: 1,
                    minimum_distance: 0.1,
                    first_cascade_far_bound: 15.,
                    maximum_distance: 15.,
                    overlap_proportion: 0.2,
                }
                .build(),
                transform,
                RenderLayers::layer(LAYER),
                ShadowSun,
            ));
        }
    }
}

/// Most model meshes casting at once. Source caps rendered model shadows
/// as well; without a cap every horde member, corpse and gore piece was
/// re-rendered into the shadow map (Hotel horde p50 +2-3 ms).
const MAX_CASTERS: usize = 40;
/// Dynamic models (per-draw lit: players, infected, dynamic/physics props)
/// in the world view join the caster layer, nearest first, re-chosen four
/// times a second. The viewmodel never casts.
fn casters(
    mut commands: Commands,
    time: Res<Time>,
    mut clock: Local<f32>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    draws: Query<(Entity, &GlobalTransform, Option<&RenderLayers>), With<crate::viewer::DynamicModelLighting>>,
) {
    *clock -= time.delta_secs();
    if *clock > 0. {
        return;
    }
    *clock = 0.25;
    let Ok(eye) = camera.single().map(|t| t.translation()) else {
        return;
    };
    let world = RenderLayers::from_layers(&[0, 1]);
    let caster = RenderLayers::layer(LAYER);
    let viewmodel = RenderLayers::layer(2);
    let mut candidates: Vec<(f32, Entity)> = draws
        .iter()
        .filter(|(_, _, layers)| {
            let layers = layers.cloned().unwrap_or_default();
            layers.intersects(&world) && !layers.intersects(&viewmodel)
        })
        .map(|(entity, transform, _)| (transform.translation().distance_squared(eye), entity))
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (rank, (distance, entity)) in candidates.into_iter().enumerate() {
        let Ok((_, _, layers)) = draws.get(entity) else { continue };
        let layers = layers.cloned().unwrap_or_default();
        let want = rank < MAX_CASTERS && distance < 15. * 15.;
        let has = layers.intersects(&caster);
        if want && !has {
            commands.entity(entity).insert(layers.with(LAYER));
        } else if !want && has {
            commands.entity(entity).insert(layers.without(LAYER));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entity(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
    #[test]
    fn reads_retail_shadow_control_keyvalues() {
        // c1m2_streets shadow_control (Hammer 1838293).
        let c = read(&[entity(&[
            ("classname", "shadow_control"),
            ("angles", "80 30 0"),
            ("color", "26 27 28"),
            ("distance", "100"),
            ("disableallshadows", "0"),
        ])]);
        assert!(c.enabled);
        assert!((c.distance - 100. * crate::collision::UNIT).abs() < 1e-6);
        // Pitch 80 points almost straight down; yaw 30 tilts toward +X/+Y.
        assert!(c.direction.y < -0.98);
        assert!(c.direction.x > 0. && c.direction.z < 0.);
        assert!((c.color.x - srgb_to_linear(26. / 255.)).abs() < 1e-6);
        let off = read(&[entity(&[("classname", "shadow_control"), ("disableallshadows", "1")])]);
        assert!(!off.enabled);
        assert_eq!(read(&[]), ShadowControl::default());
    }
}
