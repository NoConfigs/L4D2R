//! Authored env_sun sprites. These are a sky glow, not another world light:
//! light_environment is already baked into the map's lightmaps.
//! Native colour/size math: client.dll 100a0060 / 1009fe40 / 10107290.
//! Occlusion samples world, alpha-tested static props and animated scripted
//! models (the c1m1 intro helicopter) on the CPU rather than Source's GPU
//! pixel-visibility query; temporal/coverage parity is open. Translucent
//! materials do not occlude (they write no depth in Source).
use bevy::{
    camera::visibility::RenderLayers,
    light::NotShadowCaster,
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{AsBindGroup, CompareFunction, RenderPipelineDescriptor,
        SpecializedMeshPipelineError},
    mesh::MeshVertexBufferLayoutRef,
    shader::ShaderRef,
};
use std::collections::{BTreeMap, HashMap};
use crate::{collision::UNIT, scene::Scene};
use std::sync::Arc;

struct AlphaMask {
    width: usize, height: usize, pixels: Vec<u8>, clamp: [bool; 2],
}
impl AlphaMask {
    fn sample(&self, uv: Vec2) -> f32 {
        let coord = |v: f32, clamp: bool| if clamp { v.clamp(0., 1.) } else { v.rem_euclid(1.) };
        let x = coord(uv.x, self.clamp[0]) * self.width as f32 - 0.5;
        let y = coord(uv.y, self.clamp[1]) * self.height as f32 - 0.5;
        let at = |x: i32, y: i32| {
            let wrap = |v: i32, n: usize, clamp: bool| if clamp { v.clamp(0, n as i32 - 1) } else { v.rem_euclid(n as i32) } as usize;
            self.pixels[wrap(y, self.height, self.clamp[1]) * self.width + wrap(x, self.width, self.clamp[0])] as f32 / 255.
        };
        let (ix, iy) = (x.floor() as i32, y.floor() as i32);
        let (tx, ty) = (x - x.floor(), y - y.floor());
        let top = at(ix, iy) * (1. - tx) + at(ix + 1, iy) * tx;
        let bottom = at(ix, iy + 1) * (1. - tx) + at(ix + 1, iy + 1) * tx;
        top * (1. - ty) + bottom * ty
    }
}
struct OccluderDraw {
    positions: Vec<Vec3>, uv: Vec<Vec2>, mask: Option<Arc<AlphaMask>>, cutoff: f32,
}
struct OccluderModel { lo: Vec3, hi: Vec3, draws: Vec<OccluderDraw> }
#[derive(Resource, Default)]
pub(crate) struct SunOcclusion(Vec<(Mat4, Arc<OccluderModel>)>, std::collections::HashSet<String>);
/// Ray against a world-space triangle list (with its bounds), as the static
/// occluders test theirs.
fn triangles_blocked(lo: Vec3, hi: Vec3, positions: &[Vec3], origin: Vec3, direction: Vec3) -> bool {
    OccluderModel {
        lo,
        hi,
        draws: vec![],
    }
    .box_hit(origin, direction)
        && positions.chunks_exact(3).any(|t| triangle_hit(t, origin, direction).is_some())
}
/// Möller-Trumbore: (u, v) of a forward hit.
fn triangle_hit(triangle: &[Vec3], origin: Vec3, direction: Vec3) -> Option<(f32, f32)> {
    let e1 = triangle[1] - triangle[0];
    let e2 = triangle[2] - triangle[0];
    let p = direction.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-8 { return None; }
    let t = origin - triangle[0];
    let u = t.dot(p) / det;
    if !(0. ..=1.).contains(&u) { return None; }
    let q = t.cross(e1);
    let v = direction.dot(q) / det;
    if v < 0. || u + v > 1. || e2.dot(q) / det <= 0.001 { return None; }
    Some((u, v))
}
impl OccluderModel {
    fn box_hit(&self, origin: Vec3, direction: Vec3) -> bool {
        let (mut near, mut far) = (0.001_f32, f32::INFINITY);
        for axis in 0..3 {
            if direction[axis].abs() < 1e-8 {
                if origin[axis] < self.lo[axis] || origin[axis] > self.hi[axis] { return false; }
            } else {
                let a = (self.lo[axis] - origin[axis]) / direction[axis];
                let b = (self.hi[axis] - origin[axis]) / direction[axis];
                near = near.max(a.min(b)); far = far.min(a.max(b));
                if near > far { return false; }
            }
        }
        true
    }
    fn blocked(&self, origin: Vec3, direction: Vec3) -> bool {
        if !self.box_hit(origin, direction) { return false; }
        for draw in &self.draws {
            for (i, triangle) in draw.positions.chunks_exact(3).enumerate() {
                let Some((u, v)) = triangle_hit(triangle, origin, direction) else { continue };
                if let Some(mask) = &draw.mask {
                    let uv = &draw.uv[i * 3..i * 3 + 3];
                    if mask.sample(uv[0] * (1. - u - v) + uv[1] * u + uv[2] * v) < draw.cutoff { continue; }
                }
                return true;
            }
        }
        false
    }
}
impl SunOcclusion {
    pub(crate) fn prepare(scene: &Scene) -> Self {
        if !scene.world.entities.iter().any(|e| e.get("classname").is_some_and(|c| c == "env_sun")) { return Self::default(); }
        let translucent = scene.materials.iter().filter(|(_, m)| m.alpha >= 2).map(|(k, _)| k.clone()).collect();
        let mut models = HashMap::new();
        let mut masks = HashMap::<String, Arc<AlphaMask>>::new();
        let mut props = vec![];
        for prop in scene.world.props.iter().filter(|p| p.static_index.is_some() && !p.sky) {
            let key = (prop.model.clone(), prop.skin, prop.body);
            let model = models.entry(key.clone()).or_insert_with(|| {
                let mut model = OccluderModel { lo: Vec3::splat(f32::INFINITY), hi: Vec3::splat(f32::NEG_INFINITY), draws: vec![] };
                for draw in scene.models.get(&key).into_iter().flatten() {
                    let Some(material) = scene.materials.get(&draw.material) else { continue; };
                    if material.alpha >= 2 { continue; } // Blended/additive props do not write depth.
                    let mask = if material.alpha == 1 {
                        material.base.as_ref().and_then(|path| scene.textures.get(path).map(|texture| {
                            masks.entry(path.clone()).or_insert_with(|| Arc::new(AlphaMask {
                                width: texture.width as usize, height: texture.height as usize,
                                pixels: texture.rgba.chunks_exact(4).map(|p| p[3]).collect(), clamp: texture.clamp,
                            })).clone()
                        }))
                    } else { None };
                    let positions: Vec<_> = draw.positions.iter().map(|p| Vec3::from_array(*p)).collect();
                    for &p in &positions { model.lo = model.lo.min(p); model.hi = model.hi.max(p); }
                    model.draws.push(OccluderDraw { positions,
                        uv: draw.uv.iter().map(|uv| Vec2::from_array(crate::material::transform_uv(*uv, material.uv))).collect(),
                        mask, cutoff: material.cutoff });
                }
                Arc::new(model)
            });
            let transform = Transform {
                translation: Vec3::from_array(crate::binary::source_position(prop.origin)),
                rotation: Quat::from_rotation_y(prop.angles[1].to_radians())
                    * Quat::from_rotation_z(-prop.angles[0].to_radians())
                    * Quat::from_rotation_x(prop.angles[2].to_radians()), ..default()
            };
            props.push((transform.to_matrix().inverse(), model.clone()));
        }
        Self(props, translucent)
    }
    fn blocked(&self, origin: Vec3, direction: Vec3) -> bool {
        self.0.iter().any(|(inverse, model)|
            model.blocked(inverse.transform_point3(origin), inverse.transform_vector3(direction)))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Definition {
    direction: Vec3,
    colour: Vec3,
    size: f32,
    overlay_size: f32,
    overlay_colour: Vec3,
    hdr: f32,
    materials: [String; 2],
}
fn number(e: &BTreeMap<String, String>, key: &str, default: f32) -> f32 {
    e.get(key).and_then(|s| s.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(default)
}
pub(crate) fn read(e: &BTreeMap<String, String>) -> Option<Definition> {
    if e.get("classname")?.as_str() != "env_sun" { return None; }
    // Retail Streets reports (-.852868, -.492404, .173648) for
    // angles="0 30 0", pitch=-10. Origin is not the sun's position.
    let angles = crate::kv::vector(e.get("angles"));
    let pitch = number(e, "pitch", angles[0]).to_radians();
    let yaw = angles[1].to_radians();
    let source = Vec3::new(-pitch.cos() * yaw.cos(), -pitch.cos() * yaw.sin(), -pitch.sin());
    let colour = Vec3::from_array(crate::kv::vector(e.get("rendercolor"))).max(Vec3::ZERO);
    let colour = if colour.max_element() > 0. { colour / colour.max_element() } else { Vec3::ONE };
    let overlay = Vec3::from_array(crate::kv::vector(e.get("overlaycolor"))).max(Vec3::ZERO) / 255.;
    let size = number(e, "size", 16.).clamp(0., 255.);
    let overlay_size = number(e, "overlaysize", -1.);
    let material = |key: &str| e.get(key).filter(|s| !s.is_empty())
        .cloned().unwrap_or_else(|| "sprites/light_glow02_add_noz".into());
    Some(Definition {
        direction: Vec3::new(source.x, source.z, -source.y),
        colour, size,
        overlay_size: if overlay_size < 0. { size } else { overlay_size.clamp(0., 255.) },
        overlay_colour: if overlay == Vec3::ZERO { colour } else { overlay },
        hdr: number(e, "hdrcolorscale", 1.).max(0.),
        materials: [material("material"), material("overlaymaterial")],
    })
}
impl Definition {
    pub(crate) fn materials(&self) -> impl Iterator<Item = &String> { self.materials.iter() }
}
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub(crate) struct SunMaterial {
    #[texture(0)] #[sampler(1)] texture: Handle<Image>,
    // RGB includes native HDRColorScale; W is occlusion/facing visibility.
    #[uniform(2)] colour: Vec4,
}
impl Material for SunMaterial {
    fn fragment_shader() -> ShaderRef { "shaders/source_sun.wgsl".into() }
    fn alpha_mode(&self) -> AlphaMode { AlphaMode::Add }
    fn enable_prepass() -> bool { false }
    fn enable_shadows() -> bool { false }
    fn specialize(_: &MaterialPipeline, descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef, _: MaterialPipelineKey<Self>) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            // The authored material is $ignorez; visibility gates the whole
            // sprite so its halo can overlap a nearby silhouette.
            depth.depth_compare = Some(CompareFunction::Always);
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}
#[derive(Component)]
pub(crate) struct Sun {
    direction: Vec3,
    visibility: f32,
}
pub(crate) fn spawn(commands: &mut Commands, meshes: &mut Assets<Mesh>,
    materials: &mut Assets<SunMaterial>, scene: &Scene, diffuse: &HashMap<String, Handle<Image>>) {
    for definition in scene.world.entities.iter().filter_map(read) {
        // Render the main authored glow. The native secondary overlay uses
        // a separate sky/pixel-visibility query. Enabling that six-times
        // larger halo with our CPU visibility washed out the whole view in
        // the retail comparison; defer it until that query is reproduced.
        for index in 0..1 {
            let Some(texture) = scene.materials.get(&definition.materials[index])
                .and_then(|m| m.base.as_ref()).and_then(|t| diffuse.get(t)) else {
                warn!("Sun sprite unavailable: {}", definition.materials[index]);
                continue;
            };
            let overlay = index == 1;
            let size = if overlay { definition.overlay_size * 6. } else { definition.size };
            if size <= 0. { continue; }
            let colour = if overlay { definition.overlay_colour } else { definition.colour };
            commands.spawn((
                Mesh3d(meshes.add(Rectangle::new(size * 2. * UNIT, size * 2. * UNIT))),
                MeshMaterial3d(materials.add(SunMaterial { texture: texture.clone(),
                    colour: (colour * definition.hdr).extend(0.) })),
                Transform::default(), Visibility::Hidden, NotShadowCaster,
                bevy::camera::visibility::NoFrustumCulling,
                RenderLayers::layer(1),
                Sun { direction: definition.direction, visibility: 0. },
            ));
        }
        println!("Authored sun: direction={:?} size={} overlay={} HDR={}",
            definition.direction, definition.size, definition.overlay_size, definition.hdr);
    }
}
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn update(
    time: Res<Time>,
    shadows: Option<Res<crate::lighting::WorldShadow>>,
    props: Option<Res<SunOcclusion>>,
    animated: (
        Option<Res<crate::l4d2r_cinematic::MapLogic>>,
        Option<Res<crate::l4d2r_animation::AnimationLibrary>>,
        Query<(&crate::l4d2r_cinematic::ScriptedDraw, &GlobalTransform, &ViewVisibility)>,
    ),
    cameras: Query<(&Transform, &RenderLayers), (With<Camera3d>, Without<Sun>)>,
    mut suns: Query<(&mut Transform, &mut Sun, &MeshMaterial3d<SunMaterial>, &mut Visibility)>,
    mut materials: ResMut<Assets<SunMaterial>>,
) {
    let Some((camera, _)) = cameras.iter().find(|(_, layers)| layers.intersects(&RenderLayers::layer(1))) else { return; };
    let mut coverage = HashMap::<[u32; 3], f32>::new();
    // Animated scripted models (GPU-skinned), CPU-skinned on demand while a
    // sun faces the view: world-space triangles with bounds.
    let mut moving: Option<Vec<(Vec3, Vec3, Vec<Vec3>)>> = None;
    let (logic, library, scripted) = &animated;
    for (mut transform, mut sun, material, mut visible) in &mut suns {
        let facing = camera.forward().dot(sun.direction);
        if facing > 0. && moving.is_none() {
            let mut out = vec![];
            if let (Some(logic), Some(library)) = (logic, library) {
                for (draw, global, seen) in scripted {
                    let Some((name, start)) = logic.world.cine.anims.get(&draw.ent) else { continue };
                    if !seen.get() { continue; }
                    let Some((positions, material)) =
                        library.scripted_triangles(&draw.model, draw.draw, name, logic.world.time - start)
                    else { continue };
                    if props.as_ref().is_some_and(|p| p.1.contains(material)) { continue; }
                    let world: Vec<Vec3> = positions.iter().map(|p| global.transform_point(Vec3::from_array(*p))).collect();
                    let lo = world.iter().fold(Vec3::splat(f32::INFINITY), |a, p| a.min(*p));
                    let hi = world.iter().fold(Vec3::splat(f32::NEG_INFINITY), |a, p| a.max(*p));
                    out.push((lo, hi, world));
                }
            }
            moving = Some(out);
        }
        let target = if facing <= 0. { 0. } else { *coverage.entry(sun.direction.to_array().map(f32::to_bits)).or_insert_with(|| {
            let side = sun.direction.cross(Vec3::Y).normalize_or(Vec3::X);
            let up = side.cross(sun.direction).normalize_or_zero();
            let mut clear = 0;
            for y in -1..=1 { for x in -1..=1 {
                let ray = (sun.direction + (side * x as f32 + up * y as f32) * 0.01).normalize();
                if shadows.as_ref().is_some_and(|world| !world.blocked(camera.translation.to_array(), ray.to_array()))
                    && !props.as_ref().is_some_and(|props| props.blocked(camera.translation, ray))
                    && !moving.iter().flatten().any(|(lo, hi, t)| triangles_blocked(*lo, *hi, t, camera.translation, ray)) { clear += 1; }
            }}
            clear as f32 / 9.
        }) };
        // Smooth query changes instead of flashing when crossing a roof edge.
        let amount = (time.delta_secs() * 10.).min(1.);
        sun.visibility += (target - sun.visibility) * amount;
        let strength = sun.visibility;
        *visible = if strength > 0.001 { Visibility::Visible } else { Visibility::Hidden };
        if let Some(mut material) = materials.get_mut(&material.0) { material.colour.w = strength; }
        let up = if sun.direction.cross(Vec3::Y).length_squared() < 1e-6 { Vec3::X } else { Vec3::Y };
        // Native DrawSprite basis puts the directional sprite 100 units
        // from the eye, with its own plane perpendicular to the sun ray.
        *transform = Transform::from_translation(camera.translation + sun.direction * (100. * UNIT))
            .looking_to(sun.direction, up);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn foliage_alpha_holes_pass_sunlight_and_opaque_texels_block_it() {
        let mask = Arc::new(AlphaMask { width: 2, height: 1, pixels: vec![0, 255], clamp: [true; 2] });
        let make = |uv| OccluderModel {
            lo: Vec3::new(-1., -1., 1.), hi: Vec3::new(1., 1., 1.),
            draws: vec![OccluderDraw { positions: vec![Vec3::new(-1., -1., 1.), Vec3::new(1., -1., 1.), Vec3::new(0., 1., 1.)],
                uv: vec![Vec2::new(uv, 0.5); 3], mask: Some(mask.clone()), cutoff: 0.5 }],
        };
        assert!(!make(0.25).blocked(Vec3::ZERO, Vec3::Z));
        assert!(make(0.75).blocked(Vec3::ZERO, Vec3::Z));
        assert!(!make(0.75).blocked(Vec3::ZERO, Vec3::NEG_Z));
        let rotated = Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let props = SunOcclusion(vec![(rotated.inverse(), Arc::new(make(0.75)))], Default::default());
        assert!(props.blocked(Vec3::ZERO, Vec3::X));
    }
    #[test]
    fn streets_sun_matches_live_retail_direction_sizes_and_hdr_scale() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        let world = crate::bsp::parse(&std::fs::read(path).unwrap()).unwrap();
        let suns: Vec<_> = world.entities.iter().filter_map(read).collect();
        assert_eq!(suns.len(), 1);
        let sun = &suns[0];
        let actual_source = Vec3::new(sun.direction.x, -sun.direction.z, sun.direction.y);
        assert!(actual_source.distance(Vec3::new(-0.852868, -0.492404, 0.173648)) < 0.00001);
        assert_eq!((sun.size, sun.overlay_size, sun.hdr), (32., 32., 3.));
        assert!((sun.colour - Vec3::new(1., 199./236., 149./236.)).length() < 0.00001);
        assert_eq!(sun.overlay_colour, sun.colour);
        assert_eq!(sun.materials[0], "sprites/light_glow02_add_noz");
    }
    #[test]
    fn animated_model_triangles_block_the_sun_ray() {
        // A triangle 5 m along +X: rays through it are blocked, others pass.
        let t = vec![Vec3::new(5., -1., -1.), Vec3::new(5., 1., -1.), Vec3::new(5., 0., 1.)];
        let (lo, hi) = (Vec3::new(5., -1., -1.), Vec3::new(5., 1., 1.));
        assert!(triangles_blocked(lo, hi, &t, Vec3::ZERO, Vec3::X));
        assert!(!triangles_blocked(lo, hi, &t, Vec3::ZERO, Vec3::NEG_X));
        assert!(!triangles_blocked(lo, hi, &t, Vec3::ZERO, Vec3::new(1., 1., 0.).normalize()));
    }
    #[test]
    fn invalid_map_values_cannot_poison_sun_material() {
        let e = crate::kv::entities(r#"{"classname" "env_sun" "size" "nan" "hdrcolorscale" "inf"}"#).remove(0);
        let sun = read(&e).unwrap();
        assert!(sun.direction.is_finite());
        assert_eq!((sun.size, sun.overlay_size, sun.hdr), (16., 16., 1.));
    }
}
