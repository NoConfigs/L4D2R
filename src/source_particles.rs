//! Owned PCF sprite effects. The supported operators are evaluated locally;
//! noise, collision, lighting and the complete Source particle VM remain incomplete.
use crate::{
    binary,
    source_assets::SourceAssets,
    source_pcf::{self, Element, SheetSequence},
};
use bevy::{
    asset::RenderAssetUsages,
    camera::primitives::Aabb,
    light::NotShadowCaster,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use l4d2r_game_core::{entities::Kind, random::UniformRandomStream};
use std::collections::{HashMap, HashSet};
#[derive(Clone)]
struct NoiseRemap {
    field: u32,
    output: [f32; 2],
    spatial: f32,
    time: f32,
    world: f32,
    offset: Vec3,
    time_offset: f32,
    absolute: bool,
    invert: bool,
    scale: bool,
}
#[derive(Clone)]
struct VelocityNoise {
    output: [Vec3; 2],
    spatial: f32,
    time: f32,
    offset: Vec3,
    time_offset: f32,
    absolute: Vec3,
    invert: Vec3,
    local: bool,
}
fn coherent_velocity(
    noise: &crate::source_noise::Noise,
    op: &VelocityNoise,
    position: Vec3,
    creation: f32,
) -> Vec3 {
    let coordinates =
        (position + op.offset) * op.spatial + Vec3::splat((creation + op.time_offset) * op.time);
    let offsets = [
        Vec3::ZERO,
        Vec3::new(100000.5, 300000.25, 9000000.75),
        Vec3::new(110000.25, 310000.75, 9100000.5),
    ];
    let values: [f32; 3] = std::array::from_fn(|i| {
        let mut sample = noise.sample((coordinates + offsets[i]).to_array());
        if op.absolute[i] != 0. {
            sample = sample.abs();
        }
        if op.invert[i] != 0. {
            sample = 1. - sample;
        }
        let scale = if op.absolute[i] != 0. { 1. } else { 0.5 };
        op.output[0][i] + (op.output[1][i] - op.output[0][i]) * (1. - scale + scale * sample)
    });
    Vec3::from_array(values)
}

#[derive(Clone)]
struct Renderer {
    screen_rotation: Option<f32>,
    orientation: u32,
    fps: f32,
    second_fps: f32,
    animation_fps: bool,
    animation_fit: bool,
    trail: f32,
    trail_min: f32,
    trail_fade: f32,
    rope: bool,
    /// Renderer visibility by camera distance to the proxy control point:
    /// (distance min, distance max, alpha scale min, alpha scale max), Source
    /// units. The distance is remapped min..max to 0..1 and the alpha scale
    /// lerped across it (fog_volume_1024_512: 1 -> 0 over 1512 -> 1712).
    visibility: Option<[f32; 4]>,
}
impl Renderer {
    fn parse(node: &Element) -> Option<Self> {
        let name = node.text("functionName").to_ascii_lowercase();
        if !matches!(
            name.as_str(),
            "render_animated_sprites"
                | "render_sprite_trail"
                | "render_rope"
                | "render_screen_velocity_rotate"
        ) {
            return None;
        }
        let trail = name == "render_sprite_trail";
        Some(Self {
            screen_rotation: (name == "render_screen_velocity_rotate")
                // C_OP_RenderScreenVelocityRotate's field default is "-90.0f"
                // (client.dll descriptor at 103c1096, string 1065a8cc).
                .then(|| node.number("forward_angle", -90.).to_radians()),
            orientation: node.number("orientation_type", 0.) as u32,
            fps: node
                .number("animation rate", if trail { 0.1 } else { 1. })
                .max(0.),
            second_fps: node.number("second sequence animation rate", 1.).max(0.),
            animation_fps: node.boolean("use animation rate as FPS"),
            animation_fit: node.boolean("animation_fit_lifetime"),
            trail: if trail {
                node.number("max length", 2000.).max(0.) * 0.0254
            } else {
                0.
            },
            trail_min: node.number("min length", 0.).max(0.) * 0.0254,
            trail_fade: node.number("length fade in time", 0.).max(0.),
            rope: name == "render_rope",
            visibility: {
                let (lo, hi) = (
                    node.number("Visibility input distance minimum", 0.),
                    node.number("Visibility input distance maximum", 0.),
                );
                (node.number("Visibility Proxy Input Control Point Number", -1.) >= 0. && (hi - lo).abs() > 0.001).then(|| [
                    lo,
                    hi,
                    node.number("Visibility Alpha Scale minimum", 0.),
                    node.number("Visibility Alpha Scale maximum", 1.),
                ])
            },
        })
    }
}
#[derive(Clone)]
struct Definition {
    minimum_rendered_frames: u8,
    child_delay: f32,
    parent: Option<usize>,
    parent_particles: Option<ParentParticles>,
    scale_emission_by_parent: bool,
    renderers: Vec<Renderer>,
    ground: Option<(f32, f32, bool)>,
    position_remap: Option<([f32; 2], [Vec3; 2], [f32; 2], bool)>,
    color_fade: Option<(Vec3, [f32; 2], bool)>,
    pre_age: Option<NoiseRemap>,
    cull: Option<([f32; 2], f32, f32)>,
    flip: f32,
    trail_time: ([f32; 2], f32),
    control_lock: Option<(bool, f32)>,
    /// Every Velocity Noise initializer adds its own noise (C_INIT_InitialVelocityNoise).
    velocity_noise: Vec<VelocityNoise>,
    noise: Vec<NoiseRemap>,
    material: String,
    rate: f32,
    burst: usize,
    point_scale: f32,
    duration: f32,
    start: f32,
    radius: [f32; 2],
    radius_exponent: f32,
    scale: [f32; 2],
    scale_bias: f32,
    scale_window: [f32; 2],
    scale_ease: bool,
    life: [f32; 2],
    alpha: [f32; 2],
    color: [Vec3; 2],
    distance: f32,
    bias: Vec3,
    /// Position Within Sphere Random's control point: (control_point_number,
    /// "randomly distribute to highest supplied Control Point", "randomly
    /// distribution growth time").
    sphere_points: Option<(usize, bool, f32)>,
    velocity: [Vec3; 2],
    gravity: Vec3,
    drag: f32,
    fade: [f32; 2],
    fade_random: [Option<([f32; 2], bool, f32)>; 2],
    fade_in_spline: bool,
    fade_out_style: Option<(bool, f32)>,
    sequences: [usize; 2],
    second_sequences: [usize; 2],
    lifetime_fps: f32,
    offset: [Vec3; 2],
    proportional_offset: bool,
    offset_local: bool,
    warp: Option<[Vec3; 2]>,
    velocity_local: bool,
    rotation: [f32; 2],
    rotation_exponent: f32,
    max: usize,
    /// PCF "maximum draw distance" in metres: Source does not draw a
    /// collection farther than this from the view (fog_volume_1024_512 1024u).
    max_draw_distance: f32,
    trail: f32,
    rope: bool,
    cull_plane: Option<f32>,
    path_steps: Option<f32>,
    distance_radius: Option<([f32; 2], [f32; 2])>,
    /// Operator "Remap Distance to Control Point to Scalar" on the radius
    /// (C_OP_DistanceToCP): every frame the radius becomes the remapped
    /// distance to control point 0, times the spawn radius when "output is
    /// scalar of initial random range"; with "only active within specified
    /// distance" particles outside the input range keep their radius.
    live_distance_radius: Option<([f32; 2], [f32; 2], bool, bool)>,
    radius_alpha: Option<([f32; 2], [f32; 2], bool)>,
    creation_remaps: Vec<(u32, [f32; 2], [f32; 2], [f32; 2], bool)>,
}
#[derive(Clone)]
struct ParentParticles {
    random: bool,
    inherited_velocity: f32,
    offset_after: bool,
    velocity_noise_after: bool,
}
fn parent_index(
    count: usize,
    random: bool,
    cursor: &mut usize,
    rng: &mut UniformRandomStream,
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let index = if random {
        rng.random_int(0, count as i32 - 1) as usize
    } else {
        *cursor % count
    };
    *cursor = index + 1;
    Some(index)
}
fn inherit_parent(
    previous: Vec3,
    current: Vec3,
    fraction: f32,
    velocity_scale: f32,
    dt: f32,
) -> (Vec3, Vec3) {
    let position = previous.lerp(current, fraction.clamp(0., 1.));
    let velocity = (position - previous) * velocity_scale / dt.max(0.000001);
    (position, velocity)
}
/// C_OP_ContinuousEmitter (client 1037f520): the rate is multiplied by
/// "scale emission to used control points" x the highest control point
/// index (collection +0xb0), or by the parent's particle count; a zero
/// product leaves the rate unscaled unless parent scaling is on.
fn emission_scale(
    point_scale: f32,
    parent_scaling: bool,
    highest_point: usize,
    parent_count: Option<usize>,
) -> f32 {
    if parent_scaling {
        point_scale * parent_count.unwrap_or(1) as f32
    } else if point_scale * highest_point as f32 != 0. {
        point_scale * highest_point as f32
    } else {
        1.
    }
}
/// The live radius from "Remap Distance to Control Point to Scalar", which
/// runs after the radius-scale operator in the authored definitions and so
/// replaces its result.
fn live_radius(d: &Definition, p: &Particle, origin: Vec3) -> Option<f32> {
    let (input, output, initial, active) = d.live_distance_radius?;
    let distance = p.position.distance(origin) / 0.0254;
    if active && (distance < input[0].min(input[1]) || distance > input[0].max(input[1])) {
        return None;
    }
    let value = remap(distance, input, output);
    Some(if initial { p.radius * value } else { value })
}
fn remap(value: f32, input: [f32; 2], output: [f32; 2]) -> f32 {
    let t = if (input[1] - input[0]).abs() < 1e-6 {
        0.
    } else {
        ((value - input[0]) / (input[1] - input[0])).clamp(0., 1.)
    };
    output[0] + (output[1] - output[0]) * t
}
fn sprite_phase(
    age: f32,
    life: f32,
    duration: f32,
    frames: usize,
    rate: f32,
    fps: bool,
    fit: bool,
) -> f32 {
    if fit {
        age / life.max(0.001) * duration
    } else if fps {
        age * rate * duration / frames.max(1) as f32
    } else {
        age * rate * duration
    }
}
fn emitter_offset(offset: Vec3, local: bool, matrix: l4d2r_math::Mat4) -> Vec3 {
    if local {
        Vec3::from_array(
            matrix
                .transform_vector3(l4d2r_math::Vec3::from_array(offset.to_array()))
                .to_array(),
        )
    } else {
        offset
    }
}
fn radius_scale(age: f32, range: [f32; 2], window: [f32; 2], bias: f32, ease: bool) -> f32 {
    let mut t = if window[1] > window[0] {
        ((age - window[0]) / (window[1] - window[0])).clamp(0., 1.)
    } else {
        1.
    };
    if ease {
        t = t * t * (3. - 2. * t);
    }
    // Source SDK mathlib Bias, without its mutable global cache.
    let biased = t.powf(bias.clamp(0.001, 0.999).ln() * -1.4427);
    range[0] + (range[1] - range[0]) * biased
}
fn definition(elements: &[Element], index: usize) -> Option<Definition> {
    let node = elements.get(index)?;
    if !node.references("renderers").is_empty() && node.text("material").is_empty() {
        return None;
    }
    let constant_color = match node.attributes.get("color") {
        Some(crate::source_pcf::Value::Vector(v)) if v.len() >= 4 => [v[0], v[1], v[2], v[3]],
        _ => [255.; 4],
    };
    let mut d = Definition {
        minimum_rendered_frames: node.number("minimum rendered frames",0.).clamp(0.,8.) as u8,
        child_delay: 0.,
        parent: None,
        parent_particles: None,
        scale_emission_by_parent: false,
        renderers: node
            .references("renderers")
            .iter()
            .filter_map(|i| elements.get(*i).and_then(Renderer::parse))
            .collect(),
        ground: None,
        position_remap: None,
        color_fade: None,
        pre_age: None,
        cull: None,
        flip: 0.,
        trail_time: ([0.1, 0.1], 1.),
        control_lock: None,
        velocity_noise: vec![],
        noise: vec![],
        material: node.text("material").into(),
        rate: 0.,
        burst: node.number("initial_particles", 0.).clamp(0., 8192.) as usize,
        point_scale: 0.,
        duration: 0.,
        start: 0.,
        radius: [node.number("radius", 5.); 2],
        radius_exponent: 1.,
        scale: [1., 1.],
        scale_bias: 0.5,
        scale_window: [0., 1.],
        scale_ease: false,
        life: [1., 1.],
        // The definition's constant "color" (RGBA bytes) is every particle's
        // starting tint and alpha unless an initializer sets them. Faint
        // always-on glows rely on it (emergency_light_glow is alpha 12).
        alpha: [constant_color[3]; 2],
        color: [Vec3::new(constant_color[0], constant_color[1], constant_color[2]); 2],
        distance: 0.,
        bias: Vec3::ONE,
        sphere_points: None,
        velocity: [Vec3::ZERO; 2],
        gravity: Vec3::ZERO,
        drag: 0.,
        fade: [0., 0.],
        fade_random: [None, None],
        fade_in_spline: false,
        fade_out_style: None,
        sequences: [node.number("sequence_number", 0.).max(0.) as usize; 2],
        second_sequences: [0, 0],
        lifetime_fps: 0.,
        offset: [Vec3::ZERO; 2],
        proportional_offset: false,
        offset_local: false,
        warp: None,
        velocity_local: false,
        rotation: [0., 0.],
        rotation_exponent: 1.,
        trail: 0.,
        rope: false,
        cull_plane: None,
        max: node.number("max_particles", 1000.).clamp(1., 8192.) as usize,
        max_draw_distance: node.number("maximum draw distance", 100000.).max(0.) * 0.0254,
        path_steps: None,
        distance_radius: None,
        live_distance_radius: None,
        radius_alpha: None,
        creation_remaps: vec![],
    };
    for key in ["emitters", "initializers", "operators"] {
        for i in node.references(key) {
            let Some(o) = elements.get(i) else {
                continue;
            };
            let pair =
                |a: &str, b: &str, default: f32| [o.number(a, default), o.number(b, default)];
            let vector = |name: &str, default: [f32; 3]| Vec3::from_array(o.vector(name, default));
            match o.text("functionName").to_ascii_lowercase().as_str() {
                "remap scalar to vector"
                    if o.number("input field", 8.) == 8.
                        && o.number("output field", 0.) == 0.
                        && !o.boolean("output is scalar of initial random range")
                        && o.number("control_point_number", 0.) == 0. =>
                {
                    d.position_remap = Some((
                        pair("input minimum", "input maximum", 0.),
                        [
                            vector("output minimum", [0.; 3]),
                            vector("output maximum", [1.; 3]),
                        ],
                        pair(
                            "emitter lifetime start time (seconds)",
                            "emitter lifetime end time (seconds)",
                            -1.,
                        ),
                        !o.attributes.contains_key("use local system")
                            || o.boolean("use local system"),
                    ));
                    if let Some((input, _, _, _)) = d.position_remap.as_mut() {
                        input[1] = o.number("input maximum", 1.);
                    }
                }
                "color fade" => {
                    d.color_fade = Some((
                        vector("color_fade", [255.; 3]) / 255.,
                        [
                            o.number("fade_start_time", 0.),
                            o.number("fade_end_time", 1.),
                        ],
                        !o.attributes.contains_key("ease_in_and_out")
                            || o.boolean("ease_in_and_out"),
                    ));
                }
                "rotation yaw flip random" => {
                    d.flip = o.number("Flip Percentage", 0.5).clamp(0., 1.)
                }
                "trail length random" => {
                    d.trail_time = (
                        pair("length_min", "length_max", 0.1),
                        o.number("length_random_exponent", 1.).max(0.001),
                    )
                }
                "cull random" => {
                    d.cull = Some((
                        [
                            o.number("Cull Start Time", 0.),
                            o.number("Cull End Time", 1.),
                        ],
                        o.number("Cull Time Exponent", 1.).max(0.001),
                        o.number("Cull Percentage", 0.5).clamp(0., 1.),
                    ))
                }
                "lifetime pre-age noise" => {
                    d.pre_age = Some(NoiseRemap {
                        field: 8,
                        output: [
                            o.number("start age minimum", 0.),
                            o.number("start age maximum", 1.),
                        ],
                        spatial: o.number("spatial noise coordinate scale", 1.),
                        time: o.number("time noise coordinate scale", 1.),
                        world: 0.,
                        offset: vector("spatial coordinate offset", [0.; 3]),
                        time_offset: o.number("time coordinate offset", 0.),
                        absolute: o.boolean("absolute value"),
                        invert: o.boolean("invert absolute value"),
                        scale: false,
                    })
                }
                "emit_instantaneously" => {
                    d.burst = o.number("num_to_emit", 1.).clamp(0., 128.) as usize;
                    d.start = o.number("emission_start_time", 0.);
                }
                "alpha fade and decay" => {
                    d.fade = [
                        o.number("end_fade_in_time", 0.),
                        1. - o.number("start_fade_out_time", 0.),
                    ];
                }
                "emit_continuously" => {
                    d.rate = o.number("emission_rate", 100.);
                    d.point_scale = o.number("scale emission to used control points", 0.);
                    d.scale_emission_by_parent =
                        o.boolean("use parent particles for emission scaling");
                    d.duration = o.number("emission_duration", 0.);
                    d.start = o.number("emission_start_time", 0.);
                }
                "position from parent particles" => {
                    d.parent_particles = Some(ParentParticles {
                        random: o.boolean("Random Parent Particle Distribution"),
                        inherited_velocity: o.number("Inherited Velocity Scale", 0.),
                        offset_after: false,
                        velocity_noise_after: false,
                    });
                }
                "radius random" => {
                    d.radius = pair("radius_min", "radius_max", 1.);
                    d.radius_exponent = o.number("radius_random_exponent", 1.);
                }
                "radius scale" => {
                    d.scale = pair("radius_start_scale", "radius_end_scale", 1.);
                    d.scale_bias = o.number("scale_bias", 0.5);
                    d.scale_window = [o.number("start_time", 0.), o.number("end_time", 1.)];
                    d.scale_ease = o.boolean("ease_in_and_out");
                }
                "lifetime random" => d.life = pair("lifetime_min", "lifetime_max", 1.),
                "alpha random" => d.alpha = pair("alpha_min", "alpha_max", 255.),
                "color random" => {
                    d.color = [vector("color1", [255.; 3]), vector("color2", [255.; 3])]
                }
                "position within sphere random" => {
                    d.distance = o.number("distance_max", 0.);
                    d.bias = vector("distance_bias", [1.; 3]);
                    d.sphere_points = Some((
                        o.number("control_point_number", 0.).max(0.) as usize,
                        o.boolean("randomly distribute to highest supplied Control Point"),
                        o.number("randomly distribution growth time", 0.).max(0.),
                    ));
                    d.velocity = [
                        vector("speed_in_local_coordinate_system_min", [0.; 3]),
                        vector("speed_in_local_coordinate_system_max", [0.; 3]),
                    ];
                    d.velocity_local = d.velocity.iter().any(|v| v.length_squared() > 0.);
                }
                "velocity noise" => {
                    if let Some(parent) = &mut d.parent_particles {
                        parent.velocity_noise_after = true;
                    }
                    d.velocity_noise.push(VelocityNoise {
                        output: [
                            vector("output minimum", [0.; 3]),
                            vector("output maximum", [1.; 3]),
                        ],
                        spatial: o.number("Spatial Noise Coordinate Scale", 0.01),
                        time: o.number("Time Noise Coordinate Scale", 1.),
                        offset: vector("Spatial Coordinate Offset", [0.; 3]),
                        time_offset: o.number("Time Coordinate Offset", 0.),
                        absolute: vector("Absolute Value", [0.; 3]),
                        invert: vector("Invert Abs Value", [0.; 3]),
                        local: o.boolean("Apply Velocity in Local Space (0/1)")
                            || o.boolean("Apply Velocity In Local Space (0/1)"),
                    });
                }
                "remap noise to scalar" => {
                    d.noise.push(NoiseRemap {
                        field: o.number("output field", 3.) as u32,
                        output: [
                            o.number("output minimum", 0.),
                            o.number("output maximum", 1.),
                        ],
                        spatial: o.number("spatial noise coordinate scale", 0.001),
                        time: o.number("time noise coordinate scale", 0.1),
                        world: o.number("world time noise coordinate scale", 0.),
                        offset: vector("spatial coordinate offset", [0.; 3]),
                        time_offset: o.number("time coordinate offset", 0.),
                        absolute: o.boolean("absolute value"),
                        invert: o.boolean("invert absolute value"),
                        scale: o.boolean("output is scalar of initial random range"),
                    });
                }
                "movement lock to control point"
                    if o.number("control_point_number", 0.) == 0.
                        && o.number("start_fadeout_min", 1.) >= 1. =>
                {
                    d.control_lock = Some((
                        o.boolean("lock rotation"),
                        o.number("distance fade range", 0.).max(0.) * 0.0254,
                    ));
                }
                "movement basic" => {
                    d.gravity = vector("gravity", [0.; 3]);
                    d.drag = o.number("drag", 0.).clamp(0., 1.);
                }
                "alpha fade in random" => {
                    d.fade_in_spline = true;
                    d.fade[0] = (o.number("fade in time min", 0.25)
                        + o.number("fade in time max", 0.25))
                        * 0.5;
                    d.fade_random[0] = Some((
                        pair("fade in time min", "fade in time max", 0.25),
                        !o.attributes.contains_key("proportional 0/1")
                            || o.boolean("proportional 0/1"),
                        o.number("fade in time exponent", 1.),
                    ));
                }
                "alpha fade out random" => {
                    d.fade_out_style = Some((
                        !o.attributes.contains_key("ease in and out")
                            || o.boolean("ease in and out"),
                        o.number("fade bias", 0.5),
                    ));
                    d.fade[1] = (o.number("fade out time min", 0.25)
                        + o.number("fade out time max", 0.25))
                        * 0.5;
                    d.fade_random[1] = Some((
                        pair("fade out time min", "fade out time max", 0.25),
                        !o.attributes.contains_key("proportional 0/1")
                            || o.boolean("proportional 0/1"),
                        o.number("fade out time exponent", 1.),
                    ));
                }
                "sequence two random" => {
                    d.second_sequences = [
                        o.number("sequence_min", 0.).max(0.) as usize,
                        o.number("sequence_max", 0.).max(0.) as usize,
                    ];
                }
                "sequence random" => {
                    d.sequences = [
                        o.number("sequence_min", 0.).max(0.) as usize,
                        o.number("sequence_max", 0.).max(0.) as usize,
                    ]
                }
                "cull when crossing plane" => {
                    d.cull_plane = Some(o.number("Cull plane offset", 0.) * 0.0254)
                }
                "position modify place on ground" => {
                    d.ground = Some((
                        o.number("max trace length", 1000.),
                        o.number("offset", 0.),
                        o.boolean("kill on no collision"),
                    ));
                }
                "lifetime from sequence" => {
                    d.lifetime_fps = o.number("Frames Per Second", 30.).max(0.01)
                }
                "position modify offset random" => {
                    if let Some(parent) = &mut d.parent_particles {
                        parent.offset_after = true;
                    }
                    d.offset = [vector("offset min", [0.; 3]), vector("offset max", [0.; 3])];
                    d.proportional_offset = o.boolean("offset proportional to radius 0/1");
                    d.offset_local = o.boolean("offset in local space 0/1");
                }
                "position modify warp random" => {
                    d.warp = Some([vector("warp min", [1.; 3]), vector("warp max", [1.; 3])]);
                }
                "rotation random" => {
                    let initial = o.number("rotation_initial", 0.);
                    d.rotation = [
                        initial + o.number("rotation_offset_min", 0.),
                        initial + o.number("rotation_offset_max", 360.),
                    ]
                    .map(f32::to_radians);
                    d.rotation_exponent = o.number("rotation_random_exponent", 1.).max(0.001);
                }
                "position along path sequential" => {
                    d.path_steps =
                        Some(o.number("particles to map from start to end", 100.).max(2.));
                }
                "remap initial distance to control point to scalar"
                    if o.number("output field", 3.) == 3. =>
                {
                    d.distance_radius = Some((
                        [
                            o.number("distance minimum", 0.),
                            o.number("distance maximum", 128.),
                        ],
                        [
                            o.number("output minimum", 0.),
                            o.number("output maximum", 1.),
                        ],
                    ));
                }
                "remap distance to control point to scalar"
                    if o.number("output field", 3.) == 3.
                        && o.number("control point", 0.) == 0. =>
                {
                    d.live_distance_radius = Some((
                        [
                            o.number("distance minimum", 0.),
                            o.number("distance maximum", 128.),
                        ],
                        [
                            o.number("output minimum", 0.),
                            o.number("output maximum", 1.),
                        ],
                        o.boolean("output is scalar of initial random range"),
                        o.boolean("only active within specified distance"),
                    ));
                }
                "remap initial scalar"
                    if o.number("input field", 0.) == 3. && o.number("output field", 0.) == 7. =>
                {
                    d.radius_alpha = Some((
                        [o.number("input minimum", 0.), o.number("input maximum", 1.)],
                        [
                            o.number("output minimum", 0.),
                            o.number("output maximum", 1.),
                        ],
                        o.boolean("output is scalar of initial random range"),
                    ));
                }
                "remap initial scalar" if o.number("input field", 8.) == 8. => {
                    d.creation_remaps.push((
                        o.number("output field", 3.) as u32,
                        [o.number("input minimum", 0.), o.number("input maximum", 1.)],
                        [
                            o.number("output minimum", 0.),
                            o.number("output maximum", 1.),
                        ],
                        [
                            o.number("emitter lifetime start time (seconds)", -1.),
                            o.number("emitter lifetime end time (seconds)", -1.),
                        ],
                        o.boolean("output is scalar of initial random range"),
                    ));
                }
                _ => {}
            }
        }
    }
    d.trail = d.renderers.iter().map(|r| r.trail).fold(0., f32::max);
    d.rope = d.renderers.iter().any(|r| r.rope);
    // Keep simulation-only/unsupported-renderer parents alive for their
    // children, without silently substituting billboard draws.
    Some(d)
}
// Reserve a bounded allowance for foreground feedback when map fire/smoke
// has reached the ordinary world-particle cap.
fn particle_budget(total:usize,foreground:bool)->usize {
    (8192usize+if foreground {512} else {0}).saturating_sub(total)
}
// Short native muzzle collections request at least one rendered frame. Keep
// simulation age unchanged, but allow their first samples before decay wins.
fn foreground_minimum_frames(authored:u8,foreground:bool)->u8 {
    if foreground {authored.max(1)} else {0}
}
fn minimum_frame_age(age:f32,life:f32,rendered:u8,minimum:u8)->f32 {
    // Do not retain an offscreen/stale muzzle until the player later turns
    // back toward it. The render dt is capped at 50ms above.
    if rendered<minimum && age<=life+0.05 {age.min(life*0.5)} else {age}
}
fn emission_overlap(local:f32,dt:f32,start:f32,duration:f32)->f32 {
    let end=if duration>0. {start+duration} else {f32::INFINITY};
    (local.min(end)-(local-dt.max(0.)).max(start)).max(0.)
}
fn flatten(
    elements: &[Element],
    index: usize,
    path: &mut HashSet<usize>,
    result: &mut Vec<Definition>,
) {
    flatten_parent(elements, index, path, result, None);
}
fn flatten_parent(
    elements: &[Element],
    index: usize,
    path: &mut HashSet<usize>,
    result: &mut Vec<Definition>,
    parent: Option<usize>,
) {
    if path.len() >= 16 || !path.insert(index) {
        return;
    }
    // L4D2R_PFX_SKIP=name,name: debug captures without those definitions.
    // L4D2R_PFX_SKIP=*: every definition.
    let skipped = std::env::var("L4D2R_PFX_SKIP").ok().is_some_and(|list| {
        list == "*" || elements.get(index).is_some_and(|e| list.split(',').any(|n| n == e.name))
    });
    let Some(mut d) = definition(elements, index).filter(|_| !skipped) else {
        path.remove(&index);
        return;
    };
    d.parent = parent;
    let collection = result.len();
    result.push(d);
    if let Some(node) = elements.get(index) {
        for i in node.references("children") {
            if let Some(child) = elements.get(i) {
                let n = child.number("child", -1.);
                if n >= 0. {
                    let first=result.len();
                    flatten_parent(elements, n as usize, path, result, Some(collection));
                    let delay=child.number("delay",0.).max(0.);
                    for definition in &mut result[first..] {definition.child_delay+=delay;}
                }
            }
        }
    }
    path.remove(&index);
}
struct Particle {
    rendered_frames: u8,
    previous_position: Vec3,
    normal: Vec3,
    kill_at: f32,
    flipped: bool,
    trail_time: f32,
    position: Vec3,
    velocity: Vec3,
    age: f32,
    life: f32,
    radius: f32,
    alpha: f32,
    color: Vec3,
    sequence: usize,
    second_sequence: usize,
    rotation: f32,
    fade: [f32; 2],
}
struct Batch {
    parent: Option<usize>,
    parent_cursor: usize,
    previous_control: Option<l4d2r_math::Mat4>,
    owner: usize,
    controls: Vec<usize>,
    definition: Definition,
    sheet: Vec<SheetSequence>,
    zoom: f32,
    sizes: Vec4,
    draws: Vec<ParticleDraw>,
    particles: Vec<Particle>,
    credit: f32,
    started: f32,
    emission_local: f32,
    burst_fired: bool,
    cull_origin: Option<Vec3>,
    clusters: Vec<u16>,
    /// Additive sprite: sky fog fades it to black rather than to the fog colour.
    additive: bool,
    /// Material $cropfactor (1, 1 when absent).
    crop: [f32; 2],
    /// Material takes vertex fog ($vertexfogamount); smoke1_nofog does not.
    fogged: bool,
}
struct ParticleDraw {
    renderer_index: usize,
    entity: Entity,
    mesh: Handle<Mesh>,
}
#[derive(Resource)]
pub struct Effects {
    noise: Option<crate::source_noise::Noise>,
    batches: Vec<Batch>,
    rng: UniformRandomStream,
    /// `MapLogic::generation` the batches were built for.
    generation: u64,
    /// The live fog controller's farz in metres (0: none). Source's far
    /// plane clips particles with the world: c7m1's distant fire-truck
    /// smoke (~4900u, farz 2500) drew as a dark blot in retail's haze.
    far: f32,
}
type MaterialCache = HashMap<
    String,
    (
        Handle<crate::particle_pull::ParticleMaterial>,
        Handle<crate::particle_sprite::SpriteMaterial>,
        Vec<SheetSequence>,
    ),
>;
/// Parsed PCF definitions and loaded particle materials, kept so a map
/// restart can rebuild the batches without re-reading every effect file.
#[derive(Resource, Clone)]
pub struct ParticleCache {
    definitions: HashMap<String, Vec<Definition>>,
    materials: MaterialCache,
}
/// A map change: drop the loaded particle materials (and so their
/// textures) once nothing uses them. The cache only ever grew, so every
/// map's particle textures stayed loaded for the session (the iPhone kept
/// Hotel's after exit to menu). Definitions stay: they are map-independent.
pub(crate) fn forget_materials(world: &mut World) {
    if let Some(mut cache) = world.get_resource_mut::<ParticleCache>() {
        cache.materials = MaterialCache::default();
    }
}
/// Batches are bound to entity indices (`Batch::owner`). A map restart
/// rebuilds the entity list - runtime effect pools come back in a different
/// order and map-spawned effects get new indices - so stale batches would
/// draw another effect's particles (zombie hits showing spitter goo) or none
/// at all (template fires). Rebuild them for the new list.
pub fn rebuild_after_restart(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    let Some(generation) = world
        .get_resource::<crate::l4d2r_cinematic::MapLogic>()
        .map(|l| l.generation)
    else {
        return;
    };
    let Some(effects) = world.get_resource::<Effects>() else {
        return;
    };
    if effects.generation == generation {
        return;
    }
    let draws: Vec<Entity> = effects
        .batches
        .iter()
        .flat_map(|b| b.draws.iter().map(|d| d.entity))
        .collect();
    for entity in draws {
        world.despawn(entity);
    }
    world.remove_resource::<Effects>();
    if let Err(error) = world.run_system_once(setup) {
        eprintln!("Particle rebuild after restart failed: {error}");
    }
}
impl Effects {
    pub(crate) fn owner_has_particles(&self, owner: usize) -> bool {
        self.batches
            .iter()
            .any(|b| b.owner == owner && !b.particles.is_empty())
    }
}

pub fn setup(
    mut commands: Commands,
    source: Res<crate::game_ui::HudSource>,
    logic: Res<crate::l4d2r_cinematic::MapLogic>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<crate::particle_pull::ParticleMaterial>>,
    mut sprite_materials: ResMut<Assets<crate::particle_sprite::SpriteMaterial>>,
    mut images: ResMut<Assets<Image>>,
    pull: Res<crate::particle_pull::ParticleVertices>,
    cached: Option<Res<ParticleCache>>,
    map: Option<Res<crate::viewer::Map>>,
) {
    let Ok(mut assets) = SourceAssets::new(&source.0, vec![]) else {
        return;
    };
    let cached = cached.map(|c| c.clone());
    let mut definitions = cached.as_ref().map(|c| c.definitions.clone()).unwrap_or_default();
    let mut water_report = String::new();
    let mut report = String::from("file\teffect\toperator\tstatus\n");
    let files = if cached.is_some() { vec![] } else { assets.matching_paths(".pcf") };
    for file in files {
        let parsed = assets.read(&file).and_then(|b| source_pcf::parse(&b));
        match parsed {
            Ok(elements) => {
                for (index, e) in elements
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.kind == "DmeParticleSystemDefinition")
                {
                    if e.name == "pipe_drips_h_2"
                        || e.name == "rain_stream_04"
                        || e.name.starts_with("blood_impact_infected_01")
                        || e.name.starts_with("blood_impact_red_01")
                    {
                        water_report.push_str(&format!("{e:#?}\n"));
                        for key in ["emitters", "initializers", "operators", "renderers"] {
                            for index in e.references(key) {
                                water_report.push_str(&format!("{:#?}\n", elements[index]));
                            }
                        }
                    }
                    let mut draw = vec![];
                    flatten(&elements, index, &mut HashSet::new(), &mut draw);
                    for key in ["emitters", "initializers", "operators", "renderers"] {
                        for i in e.references(key) {
                            if let Some(o) = elements.get(i) {
                                let name = o.text("functionName");
                                let status = match name.to_ascii_lowercase().as_str() {
                                    "emit_continuously"
                                    | "radius random"
                                    | "lifetime random"
                                    | "alpha random"
                                    | "color random"
                                    | "sequence random"
                                    | "movement basic"
                                    | "emit_instantaneously"
                                    | "alpha fade and decay"
                                    | "lifespan decay"
                                    | "lifetime from sequence"
                                    | "rotation random"
                                    | "position modify offset random" => "supported subset",
                                    "position along path sequential" => {
                                        "muzzle attachment path subset"
                                    }
                                    "remap initial distance to control point to scalar" => {
                                        "radius output subset"
                                    }
                                    "remap initial scalar" => "radius-to-alpha subset",
                                    "remap noise to scalar" => {
                                        "native noise lattice; scalar initialization subset"
                                    }
                                    "position modify warp random" => {
                                        "control-point-space warp subset"
                                    }
                                    "position from parent particles" => {
                                        "live parent selection, birth interpolation and velocity inheritance; partial initializer ordering"
                                    }
                                    "render_animated_sprites" => {
                                        "shared collection passes; native clocks/frame blending; visibility fade incomplete"
                                    }
                                    "radius scale" => "authored bias/window subset",
                                    "alpha fade in random" | "alpha fade out random" => {
                                        "per-particle fade range/proportional subset"
                                    }
                                    "position within sphere random" | "velocity noise" => {
                                        "approximate evaluation"
                                    }
                                    _ => "not evaluated",
                                };
                                report.push_str(&format!("{file}\t{}\t{name}\t{status}\n", e.name));
                            }
                        }
                    }
                    definitions.insert(e.name.to_ascii_lowercase(), draw);
                }
            }
            Err(e) => report.push_str(&format!("{file}\t\t\tdecode error: {e}\n")),
        }
    }
    let mut cache: MaterialCache = cached.as_ref().map(|c| c.materials.clone()).unwrap_or_default();
    let mut batches = vec![];
    let mut missing = 0;
    let mut sky_materials: HashMap<String, Handle<crate::particle_pull::ParticleMaterial>> = HashMap::new();
    for (id, e) in logic
        .world
        .ents
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, Kind::Particle { .. } | Kind::Fire { .. }))
    {
        let name =
            e.me.kv("effect_name")
                .unwrap_or("env_fire_medium")
                .to_ascii_lowercase();
        let Some(draw) = definitions.get(&name) else {
            missing += 1;
            report.push_str(&format!("map\t{name}\t\tdefinition unavailable\n"));
            continue;
        };
        let controls = (1..=63)
            .filter_map(|i| {
                e.me.kv(&format!("cpoint{i}"))
                    .and_then(|n| logic.world.find(n).first().copied())
            })
            .collect::<Vec<_>>();
        let mut collection_batches = HashMap::new();
        for (definition_index, d) in draw.iter().enumerate() {
            let parent = d.parent.and_then(|i| collection_batches.get(&i).copied());
            if d.parent.is_some() && parent.is_none() {
                report.push_str(&format!(
                    "map\t{name}\t{}\tparent collection unavailable\n",
                    d.material
                ));
                continue;
            }
            if d.renderers.is_empty() {
                collection_batches.insert(definition_index, batches.len());
                batches.push(Batch {
                    parent,
                    parent_cursor: 0,
                    previous_control: None,
                    owner: id,
                    controls: controls.clone(),
                    definition: d.clone(),
                    sheet: vec![],
                    zoom: 1.,
                    sizes: Vec4::ONE,
                    draws: vec![],
                    particles: vec![],
                    credit: 0.,
                    started: -1.,
                    emission_local: 0.,
                    burst_fired: false,
                    cull_origin: None,
                    clusters: vec![],
                    additive: false,
                    crop: [1., 1.],
                    fogged: false,
                });
                continue;
            }
            if d.renderers.iter().all(|r| r.rope) && controls.is_empty() {
                report.push_str(&format!(
                    "map\t{name}\t{}\trope endpoint unavailable; no billboard fallback\n",
                    d.material
                ));
                continue;
            }
            if !cache.contains_key(&d.material) {
                let result = (|| -> std::io::Result<_> {
                    let material = crate::material::load(&mut assets, &d.material)?;
                    let refraction =
                        crate::material::particle_refraction(&mut assets, &d.material)?;
                    let refraction_amount =
                        refraction.as_ref().map_or(0., |(_, amount, _)| *amount);
                    let bytes = assets.read(
                        material
                            .base
                            .as_deref()
                            .or_else(|| refraction.as_ref().map(|(path, _, _)| path.as_str()))
                            .ok_or_else(|| binary::invalid("Particle texture missing"))?,
                    )?;
                    let sheet = source_pcf::sheet(&bytes)?;
                    let texture = crate::vtf::decode(&bytes, 2048)?;
                    let image = Image::new(
                        Extent3d {
                            width: texture.width,
                            height: texture.height,
                            depth_or_array_layers: 1,
                        },
                        TextureDimension::D2,
                        texture.rgba,
                        // SpriteCard reads its base texture with the sRGB decode
                        // unless $EXTRACTGREENALPHA (retail stdshader_dx9
                        // 10033f60: EnableSRGBRead(0, true) when that flag is
                        // off). L4D2R_PFX_RAW: debug, texture and vertex colour
                        // as stored.
                        if refraction.is_some() || std::env::var_os("L4D2R_PFX_RAW").is_some() {
                            TextureFormat::Rgba8Unorm
                        } else {
                            TextureFormat::Rgba8UnormSrgb
                        },
                        // GPU-only: sprite sheets are never read back on the CPU.
                        RenderAssetUsages::RENDER_WORLD,
                    );
                    let texture = images.add(image);
                    let noise_path =
                        crate::material::particle_spitter_noise(&mut assets, &d.material)?;
                    let spitter_noise = if let Some(path) = &noise_path {
                        let decoded = crate::vtf::decode(&assets.read(path)?, 2048)?;
                        let mut image = Image::new(
                            Extent3d {
                                width: decoded.width,
                                height: decoded.height,
                                depth_or_array_layers: 1,
                            },
                            TextureDimension::D2,
                            decoded.rgba,
                            TextureFormat::Rgba8Unorm,
                            RenderAssetUsages::RENDER_WORLD,
                        );
                        image.sampler = bevy::image::ImageSampler::Descriptor(
                            bevy::image::ImageSamplerDescriptor {
                                address_mode_u: bevy::image::ImageAddressMode::Repeat,
                                address_mode_v: bevy::image::ImageAddressMode::Repeat,
                                ..bevy::image::ImageSamplerDescriptor::linear()
                            },
                        );
                        images.add(image)
                    } else {
                        texture.clone()
                    };
                    let (sprite_settings, add_self, dual, sizes) =
                        crate::material::sprite_settings(&mut assets, &d.material)?;
                    let sprite = sprite_materials.add(crate::particle_sprite::SpriteMaterial {
                        texture: texture.clone(),
                        settings: Vec4::from_array(sprite_settings),
                        add_self,
                        dual: Vec4::from_array(dual),
                        additive: material.alpha == 3,
                        sizes: Vec4::from_array(sizes),
                    });
                    let handle = materials.add(crate::particle_pull::ParticleMaterial {
                        base: StandardMaterial {
                            specular_transmission: if refraction.is_some() { 1. } else { 0. },
                            base_color_texture: Some(texture.clone()),
                            unlit: true,
                            cull_mode: None,
                            alpha_mode: if refraction.is_some() {
                                // Bevy only captures scene colour for the transmissive
                                // phase; transparent alpha modes bypass that phase.
                                AlphaMode::Opaque
                            } else if add_self != 0. {
                                AlphaMode::Premultiplied
                            } else if material.alpha == 3 {
                                AlphaMode::Add
                            } else {
                                AlphaMode::Blend
                            },
                            ..default()
                        },
                        extension: crate::particle_pull::ParticlePull {
                            mod2x: crate::material::particle_mod2x(&mut assets, &d.material).unwrap_or(false),
                            depth_blend: crate::material::particle_depth_blend(&mut assets, &d.material).unwrap_or(0.),
                            sky_fog: Vec4::ZERO,
                            spitter_noise,
                            vertices: pull.buffer.clone(),
                            texture,
                            settings: Vec4::from_array(sprite_settings),
                            // L4D2R_ADDSELF_TEX: debug variant (negative):
                            // the self-add term from the texture alone.
                            add_self: if add_self != 0. && std::env::var_os("L4D2R_ADDSELF_TEX").is_some() { -add_self.abs() } else { add_self },
                            dual: Vec4::from_array(dual),
                            effects: Vec4::new(
                                refraction_amount,
                                crate::material::particle_fog_mode(&mut assets, &d.material)?,
                                // z: refract tint; sprites: 2 = vertex colour
                                // used as linear (L4D2R_PFX_LINEAR_COLOR, debug).
                                if refraction.is_some() {
                                    f32::from(refraction.as_ref().is_some_and(|(_, _, tint)| *tint))
                                } else if std::env::var_os("L4D2R_PFX_RAW").is_some() {
                                    2.
                                } else {
                                    0.
                                },
                                f32::from(noise_path.is_some()),
                            ),
                        },
                    });
                    Ok((handle, sprite, sheet))
                })();
                match result {
                    Ok(v) => {
                        cache.insert(d.material.clone(), v);
                    }
                    Err(error) => {
                        report.push_str(&format!("material\t{}\t\t{error}\n", d.material));
                        continue;
                    }
                }
            }
            let (material, sprite, sheet) = cache[&d.material].clone();
            let fogged = materials.get(&material).is_some_and(|m| m.extension.effects.y != 0.);
            // 3D-skybox effects get their own copy without world distance
            // fog; update applies the sky_camera's fog to their vertices.
            let sky = map.as_ref().is_some_and(|m| m.0.world.sky_entities.contains(&id));
            let material = if sky {
                sky_materials
                    .entry(d.material.clone())
                    .or_insert_with(|| {
                        let mut copy = materials.get(&material).cloned().expect("particle material");
                        // Fogged sprites take the sky_camera fog in the
                        // shader (effects.y 3); others stay unfogged.
                        copy.extension.effects.y = if fogged { 3. } else { 0. };
                        // Source feathers ($depthblend) in the skybox view's
                        // own units; drawn scaled up into the world, the
                        // feather distance scales too (c5m1's skybox smoke
                        // stayed opaque against the sky buildings).
                        let sky_scale = map.as_ref().and_then(|m| m.0.world.sky.as_ref()).map_or(1., |s| s.scale.max(1.));
                        copy.extension.depth_blend *= sky_scale;
                        materials.add(copy)
                    })
                    .clone()
            } else {
                material
            };
            let additive = sprite_materials
                .get(&sprite)
                .is_some_and(|m| m.additive || m.add_self != 0.);
            let zoom = sprite_materials.get(&sprite).map_or(1., |m| m.dual.w);
            let sizes = sprite_materials.get(&sprite).unwrap().sizes;
            let muzzle = e.me.targetname.starts_with("__rewrite_muzzle_fp_");
            // First-person muzzle sprites keep their own rewritten mesh; world
            // batches draw shared capacity meshes (particle_pull).
            let mut particle_draws = Vec::new();
            for (renderer_index, renderer) in d.renderers.iter().enumerate() {
                // This native renderer is an ordered rotation modifier. It
                // writes particle rotation but submits no sprite geometry.
                if renderer.screen_rotation.is_some() {
                    continue;
                }
                if renderer.rope && controls.is_empty() {
                    continue;
                }
                let mesh = if muzzle {
                    meshes.add(empty_mesh())
                } else {
                    pull.meshes[0].clone()
                };
                let mut entity = commands.spawn((
                    Mesh3d(mesh.clone()),
                    Transform::default(),
                    Visibility::Hidden,
                    Aabb::default(),
                    NotShadowCaster,
                ));
                if muzzle {
                    entity.insert((
                        MeshMaterial3d(sprite.clone()),
                        bevy::camera::visibility::RenderLayers::layer(2),
                    ));
                } else {
                    entity.insert((
                        MeshMaterial3d(material.clone()),
                        bevy::mesh::MeshTag(0),
                        bevy::camera::visibility::NoAutoAabb,
                    ));
                }
                let entity = entity.id();
                particle_draws.push(ParticleDraw {
                    renderer_index,
                    entity,
                    mesh,
                });
            }
            collection_batches.insert(definition_index, batches.len());
            batches.push(Batch {
                parent,
                parent_cursor: 0,
                previous_control: None,
                owner: id,
                controls: controls.clone(),
                definition: d.clone(),
                sheet,
                zoom,
                sizes,
                draws: particle_draws,
                particles: vec![],
                credit: 0.,
                started: -1.,
                emission_local: 0.,
                burst_fired: false,
                cull_origin: None,
                clusters: vec![],
                additive,
                crop: crate::material::particle_crop(&mut assets, &d.material).unwrap_or([1., 1.]),
                fogged,
            });
        }
    }
    if cached.is_none() {
        let _ = std::fs::write("local-research/particle-operator-audit.tsv", report);
        let _ = std::fs::write(
            "local-research/water-particle-definitions.txt",
            water_report,
        );
    }
    println!(
        "Owned PCF effects: {} definitions, {} particle collections, {} renderer passes, {missing} unavailable map effects",
        definitions.len(),
        batches.len(),
        batches.iter().map(|b| b.draws.len()).sum::<usize>()
    );
    commands.insert_resource(Effects {
        noise: crate::source_noise::Noise::load(&source.0).ok(),
        batches,
        rng: UniformRandomStream::new(7),
        generation: logic.generation,
        far: 0.,
    });
    commands.insert_resource(ParticleCache { definitions, materials: cache });
    commands.insert_resource(SkyMaterials(sky_materials.into_values().collect()));
}
/// The 3D-skybox particle material copies (sync_sky_fog keeps their fog
/// colour on the sky meshes' exposure-scaled value).
#[derive(Resource, Default)]
pub struct SkyMaterials(Vec<Handle<crate::particle_pull::ParticleMaterial>>);
pub(crate) fn sync_sky_fog(
    sky: Option<Res<SkyMaterials>>,
    fog: Option<Res<crate::native_fog::Runtime>>,
    effects: Option<ResMut<Effects>>,
    mut materials: ResMut<Assets<crate::particle_pull::ParticleMaterial>>,
) {
    if let Some(mut effects) = effects {
        let far = fog.as_ref().and_then(|f| f.far()).unwrap_or(0.);
        if effects.far != far {
            effects.far = far;
        }
    }
    let (Some(sky), Some(color)) = (sky, fog.as_ref().and_then(|f| f.sky_color())) else {
        return;
    };
    for handle in &sky.0 {
        // Touch a material only for a visible change: get_mut re-uploads it.
        let stale = materials.get(handle).is_some_and(|m| {
            m.extension.effects.y == 3. && (m.extension.sky_fog - color).abs().max_element() > 0.002 * color.max_element().max(0.01)
        });
        if stale && let Some(mut m) = materials.get_mut(handle) {
            m.extension.sky_fog = color;
        }
    }
}
fn empty_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.; 3]; 6]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.; 2]; 6]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, vec![[0.; 2]; 6]);
    mesh.insert_attribute(crate::particle_sprite::FRAME_BLEND, vec![[0.; 3]; 6]);
    mesh.insert_attribute(crate::particle_sprite::SECOND_UV, vec![[0.; 4]; 6]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.; 4]; 6]);
    // Keep the draw layout identical to live indexed quads so the first
    // emission does not create a new render/preprocessing pipeline in play.
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 3, 4, 5]));
    mesh
}
fn between(rng: &mut UniformRandomStream, pair: [f32; 2]) -> f32 {
    rng.random_float(pair[0].min(pair[1]), pair[0].max(pair[1]))
}
fn point(logic: &crate::l4d2r_cinematic::MapLogic, id: usize) -> Vec3 {
    Vec3::from_array(binary::source_position(
        logic.world.world_matrix(id).w_axis.truncate().to_array(),
    ))
}
fn atlas_corner(rect: [f32; 4], corner: [f32; 2], scale: f32) -> [f32; 2] {
    let u = 0.5 + (corner[0] - 0.5) * scale;
    let v = 0.5 + (corner[1] - 0.5) * scale;
    [
        rect[0] + (rect[2] - rect[0]) * u,
        rect[1] + (rect[3] - rect[1]) * v,
    ]
}
fn sprite_extent(radius: f32, distance: f32, sizes: Vec4) -> (f32, f32) {
    let radius = radius.max(sizes.x * distance);
    let fade = if radius > sizes.z * distance {
        if radius >= sizes.w * distance {
            0.
        } else {
            1. - (radius - sizes.z * distance) / ((sizes.w - sizes.z) * distance).max(0.000001)
        }
    } else {
        1.
    };
    (radius.min(sizes.y * distance), fade)
}
fn alpha_envelope(
    age: f32,
    life: f32,
    fade: [f32; 2],
    spline_in: bool,
    out_style: Option<(bool, f32)>,
) -> f32 {
    let spline = |t: f32| t * t * (3. - 2. * t);
    if fade[1] > 0. && age > life - fade[1] {
        let t = ((life - age) / fade[1]).clamp(0., 1.);
        return out_style.map_or(t, |(ease, bias)| {
            if ease {
                spline(t)
            } else {
                1. - (1. - t).powf(
                    (if bias == 0. {
                        0.5_f32
                    } else {
                        bias.clamp(0.001, 0.999)
                    })
                    .ln()
                        * -1.4427,
                )
            }
        });
    }
    let t = if fade[0] > 0. {
        (age / fade[0]).clamp(0., 1.)
    } else {
        1.
    };
    if spline_in { spline(t) } else { t }
}
fn birth_age(interval: f32, index: usize, count: usize) -> f32 {
    interval * (1. - index as f32 / count.max(1) as f32)
}
fn sheet_uv(sheet: &[SheetSequence], sequence: usize, age: f32, fps: f32) -> [f32; 4] {
    sheet_frames(sheet, sequence, age * fps).0
}
fn sheet_frames(sheet: &[SheetSequence], sequence: usize, phase: f32) -> ([f32; 4], [f32; 4], f32) {
    let Some(sequence) = sheet
        .iter()
        .find(|s| s.id == sequence)
        .or_else(|| sheet.first())
    else {
        return ([0., 0., 1., 1.], [0., 0., 1., 1.], 0.);
    };
    let mut frame = phase.max(0.);
    if sequence.looping && sequence.duration > 0. {
        frame = frame.rem_euclid(sequence.duration);
    }
    for (i, (duration, uv)) in sequence.frames.iter().enumerate() {
        if frame < *duration {
            let next = sequence.frames.get(i + 1).or_else(|| {
                if sequence.looping {
                    sequence.frames.first()
                } else {
                    None
                }
            });
            return (
                *uv,
                next.map_or(*uv, |f| f.1),
                if *duration > 0. { frame / duration } else { 0. },
            );
        }
        frame -= *duration;
    }
    let last = sequence
        .frames
        .last()
        .map_or([0., 0., 1., 1.], |(_, uv)| *uv);
    (last, last, 0.)
}
// Live positions keep ropes and moving emitters conservative; padding covers
// every billboard rotation, authored scale endpoint and maximum trail length.
fn particle_bounds(particles: &[Particle], scale: [f32; 2], trail: f32) -> Option<Aabb> {
    if particles.is_empty() {
        return None;
    }
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for p in particles {
        let pad = p.radius.abs() * scale[0].abs().max(scale[1].abs()) * std::f32::consts::SQRT_2
            + trail.abs();
        if !p.position.is_finite() || !pad.is_finite() {
            return None;
        }
        lo = lo.min(p.position - Vec3::splat(pad));
        hi = hi.max(p.position + Vec3::splat(pad));
    }
    Some(Aabb::from_min_max(lo, hi))
}
pub fn update(
    collision: Res<crate::collision::Collision>,
    time: Res<Time>,
    graphics: Option<Res<crate::graphics_options::Graphics>>,
    state: Res<crate::game_state::Match>,
    frontend: Res<crate::game_menu::Frontend>,
    logic: Res<crate::l4d2r_cinematic::MapLogic>,
    camera: Single<(&Transform, &Projection), With<crate::viewer::GameCamera>>,
    map: Option<Res<crate::viewer::Map>>,
    effects: Option<ResMut<Effects>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut visibility: Query<&mut Visibility>,
    mut bounds: Query<&mut Aabb>,
    warmup: Option<Res<crate::render_warmup::Warmup>>,
    mut pull: ResMut<crate::particle_pull::ParticleVertices>,
    mut draws: Query<
        (&mut Mesh3d, &mut bevy::mesh::MeshTag, &mut Transform),
        Without<crate::viewer::GameCamera>,
    >,
    mut writes: ResMut<crate::gpu_writes::GpuWrites>,
) {
    // No scene while a phone map change is between release and apply.
    let Some(map) = map else { return };
    let _timing = crate::performance::scope("source_particles::update");
    let Some(mut effects) = effects else {
        return;
    };
    static HIDE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *HIDE.get_or_init(|| std::env::args().any(|a| a == "--profile-hide-particles")) {
        for batch in &effects.batches {
            for draw in &batch.draws {
                if let Ok(mut v) = visibility.get_mut(draw.entity) {
                    *v = Visibility::Hidden;
                }
            }
        }
        return;
    }
    if !frontend.playing() {
        for batch in &mut effects.batches {
            batch.particles.clear();
            batch.credit = 0.;
            batch.parent_cursor = 0;
            for draw in &batch.draws {
                if let Ok(mut v) = visibility.get_mut(draw.entity) {
                    *v = Visibility::Hidden;
                }
            }
        }
        return;
    }
    // Prepare active emitter buffers behind the loading overlay. Normal pause
    // still freezes effects; map logic and gameplay remain paused during warmup.
    // A dead infected player keeps watching its own death effects (Boomer blast).
    if (state.paused && warmup.is_none()) || (state.dead && !state.infected) {
        return;
    }
    let dt = time.delta_secs().min(0.05);
    // L4D2R_PFX_DEBUG=<targetname prefix, or *>: once a second, each batch of
    // those emitters: material, live count, mean life/radius, top height.
    if let Ok(prefix) = std::env::var("L4D2R_PFX_DEBUG") {
        static TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 60 == 0 {
            for batch in &effects.batches {
                let Some(owner) = logic.world.ents.get(batch.owner) else { continue };
                if !(prefix == "*" || owner.me.targetname.starts_with(&prefix)) || batch.particles.is_empty() { continue; }
                let n = batch.particles.len() as f32;
                let floor = batch.particles.iter().map(|p| p.position.y).fold(f32::MAX, f32::min);
                let top = batch.particles.iter().map(|p| p.position.y + p.radius).fold(f32::MIN, f32::max);
                let faded = |p: &Particle| batch.definition.color_fade.map_or(p.color, |(target, window, _)| p.color.lerp(target, remap(p.age / p.life, window, [0., 1.])));
                let color = batch.particles.iter().map(|p| faded(p)).sum::<Vec3>() / n * 255.;
                println!("L4D2RPFX color={:.0} {:.0} {:.0}", color.x, color.y, color.z);
                println!("L4D2RPFX t={:.1} {} {} {} n={} life={:.2} radius={:.1} age={:.2} span={:.0}u alpha={:.2}",
                    logic.world.time, owner.me.targetname, owner.me.kv("effect_name").unwrap_or(""), batch.definition.material, batch.particles.len(),
                    batch.particles.iter().map(|p| p.life).sum::<f32>() / n,
                    batch.particles.iter().map(|p| p.radius).sum::<f32>() / n / 0.0254,
                    batch.particles.iter().map(|p| p.age).sum::<f32>() / n,
                    (top - floor) / 0.0254,
                    batch.particles.iter().map(|p| p.alpha).sum::<f32>() / n);
            }
        }
    }
    let (camera, projection) = *camera;
    let frustum = projection.compute_frustum(&GlobalTransform::from(*camera));
    let now = logic.world.time;
    let camera_cluster = map.0.world.visibility.cluster_at(camera.translation);
    let camera_clusters = if std::env::args().any(|a| a == "--legacy-pvs-fallback") {
        vec![camera_cluster]
    } else {
        map.0.world.visibility.camera_clusters(camera.translation)
    };
    static PVS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let pvs = *PVS.get_or_init(|| !std::env::args().any(|a| a == "--no-particle-pvs"));
    // 3D skybox: Source draws sky-area effects from the sky_camera with the
    // sky's own fog. Simulate and billboard them from that camera in sky
    // space, then scale the vertices out like the sky geometry
    // (viewer::sky_transform).
    let sky_view = map.0.world.sky.as_ref().map(|sky| {
        let a = sky.angles;
        let rotation = (Quat::from_rotation_y(a[1].to_radians())
            * Quat::from_rotation_z(-a[0].to_radians())
            * Quat::from_rotation_x(a[2].to_radians()))
        .inverse();
        let origin = Vec3::from_array(crate::binary::source_position(sky.origin));
        let view = Transform {
            translation: origin + rotation.inverse() * camera.translation / sky.scale,
            rotation: rotation.inverse() * camera.rotation,
            scale: Vec3::ONE,
        };
        let frustum = projection.compute_frustum(&GlobalTransform::from(view));
        let [fog, color, _] = crate::native_fog::sky_globals(&map.0.world.entities);
        (origin, rotation, sky.scale, view, frustum, fog, color)
    });
    let world_camera = camera;
    let world_frustum = frustum;
    let effects = &mut *effects;
    let effects_far = effects.far;
    pull.clear();
    let mut total = effects
        .batches
        .iter()
        .map(|b| b.particles.len())
        .sum::<usize>();
    for batch_index in 0..effects.batches.len() {
        // Collections are flattened parent-first. Children read the parent's
        // current frame without cloning every live particle into a cache.
        let (ancestors, remaining) = effects.batches.split_at_mut(batch_index);
        let batch = &mut remaining[0];
        let parent = batch.parent.and_then(|i| ancestors.get(i));
        let Some(e) = logic.world.ents.get(batch.owner) else {
            batch.particles.clear();
            for draw in &batch.draws {
                if let Ok(mut v) = visibility.get_mut(draw.entity) {
                    *v = Visibility::Hidden;
                }
            }
            continue;
        };
        let active = e.alive
            && match e.kind {
                Kind::Particle { active, .. } | Kind::Fire { active, .. } => active,
                _ => false,
            };
        let started = match e.kind {
            Kind::Particle { started, .. } => started,
            _ => 0.,
        };
        if now < batch.started || (active && started != batch.started) {
            if now < batch.started || !e.me.targetname.starts_with("__rewrite_muzzle_fp_") {
                total = total.saturating_sub(batch.particles.len());
                batch.particles.clear();
                batch.previous_control = None;
            }
            batch.credit = 0.;
            batch.parent_cursor = 0;
            batch.started = started;
            batch.emission_local = -batch.definition.child_delay;
            batch.burst_fired = false;
        }
        if !active && batch.particles.is_empty() {
            continue;
        }
        let sky = sky_view
            .as_ref()
            .filter(|_| map.0.world.sky_entities.contains(&batch.owner));
        let (camera, frustum) = match sky {
            Some((_, _, _, view, frustum, _, _)) => (view, frustum),
            None => (world_camera, &world_frustum),
        };
        let origin = point(&logic, batch.owner);
        if !frontend.playing() || (sky.is_none() && origin.distance(camera.translation) > 65.) {
            total = total.saturating_sub(batch.particles.len());
            batch.particles.clear();
            batch.credit = 0.;
            for draw in &batch.draws {
                if let Ok(mut v) = visibility.get_mut(draw.entity) {
                    *v = Visibility::Hidden;
                }
            }
            continue;
        }
        let d = &batch.definition;
        // Viewmodel effects bypass world PVS; live attachment motion must not
        // trigger BSP cluster scans every rendered frame.
        if !e.me.targetname.starts_with("__rewrite_muzzle_fp_")
            && batch.cull_origin != Some(origin)
            && batch.controls.is_empty()
        {
            let life = if d.lifetime_fps > 0. {
                batch
                    .sheet
                    .iter()
                    .map(|s| s.duration / d.lifetime_fps)
                    .fold(0., f32::max)
            } else {
                d.life[0].max(d.life[1])
            };
            let mut radius = d
                .distance_radius
                .map_or(d.radius[0].max(d.radius[1]), |(_, out)| out[0].max(out[1]));
            for op in d.noise.iter().filter(|o| o.field == 3) {
                let output = op.output[0].max(op.output[1]);
                radius = radius.max(if op.scale { radius * output } else { output });
            }
            if let Some((_, out, initial, _)) = d.live_distance_radius {
                let output = out[0].max(out[1]);
                radius = radius.max(if initial { radius * output } else { output });
            }
            let extent = (radius * d.scale[0].max(d.scale[1])
                + d.distance
                    * d.bias.abs().max_element()
                    * d.warp
                        .map_or(1., |w| w[0].abs().max(w[1].abs()).max_element())
                + d.offset[0].length().max(d.offset[1].length())
                    * if d.proportional_offset { radius } else { 1. }
                + (d.velocity[0].length().max(d.velocity[1].length())
                    + d.velocity_noise
                        .iter()
                        .map(|n| n.output[0].length().max(n.output[1].length()))
                        .sum::<f32>())
                    * life
                + 0.5 * d.gravity.length() * life * life)
                * 0.0254
                + d.trail.min(
                    (d.velocity[0].length().max(d.velocity[1].length())
                        + d.velocity_noise
                            .iter()
                            .map(|n| n.output[0].length().max(n.output[1].length()))
                            .sum::<f32>())
                        * d.trail_time.0[0].max(d.trail_time.0[1])
                        * 0.0254,
                );
            let extent = extent
                + if d.path_steps.is_some() {
                    d.distance_radius.map_or(0., |(range, _)| range[1]) * 0.0254
                } else {
                    0.
                };
            batch.clusters = map
                .0
                .world
                .visibility
                .clusters_in_bounds(origin - Vec3::splat(extent), origin + Vec3::splat(extent));
            batch.cull_origin = Some(origin);
        }
        let show = e.me.targetname.starts_with("__rewrite_muzzle_fp_")
            || sky.is_some()
            || !pvs
            || camera_clusters.iter().any(|c| {
                map.0.world.visibility.visible(*c, &batch.clusters)
                    || (d.parent_particles.is_some()
                        && parent.is_some_and(|p| map.0.world.visibility.visible(*c, &p.clusters)))
            });
        let local = now - batch.started - d.child_delay;
        // Map time advances on fixed ticks, while this system runs per render.
        // Consume the entire unseen interval once, including a short emitter
        // that starts and ends between two rendered samples.
        let emission_dt=emission_overlap(local,(local-batch.emission_local).max(0.),d.start,d.duration);
        if active { batch.emission_local=batch.emission_local.max(local); }
        let minimum_frames=foreground_minimum_frames(d.minimum_rendered_frames,e.me.targetname.starts_with("__rewrite_muzzle_fp_"));
        let emitting = active && (emission_dt>0. || (!batch.burst_fired && local>=d.start && (d.duration<=0. || local-dt<d.start+d.duration)));
        let old_count = batch.particles.len();
        if emitting {
            let points = batch.controls.len();
            let quality = if e.me.targetname.starts_with("__rewrite_muzzle_fp_") {
                1.
            } else {
                graphics.as_ref().map_or(1., |g| g.effect_scale())
            };
            if !batch.burst_fired {
                batch.credit += d.burst as f32 * quality;
                batch.burst_fired = true;
            }
            batch.credit += emission_dt
                * d.rate
                * quality
                * emission_scale(
                    d.point_scale,
                    d.scale_emission_by_parent,
                    points,
                    parent.map(|b| b.particles.len()),
                );
            let count = (batch.credit as usize)
                .min(if d.burst > 0 {
                    128
                } else if e.me.targetname.starts_with("__rewrite_muzzle_fp_") {
                    d.max
                } else {
                    24
                })
                .min(d.max.saturating_sub(batch.particles.len()))
                .min(particle_budget(total,e.me.targetname.starts_with("__rewrite_muzzle_fp_")));
            batch.credit = (batch.credit - count as f32).min(24.);
            for particle_index in 0..count {
                let rng = &mut effects.rng;
                let highest = batch.controls.len();
                let cp = match d.sphere_points {
                    // InitCreateWithinSphere: one control point, or a random
                    // one up to the highest supplied, whose range grows from
                    // the first point over the growth time (the fire spreads).
                    Some((first, true, growth)) => {
                        let first = first.min(highest);
                        let top = if growth > 0. {
                            first.max(((local / growth).clamp(0., 1.) * highest as f32) as usize)
                        } else {
                            highest
                        };
                        rng.random_int(first as i32, top as i32) as usize
                    }
                    Some((first, false, _)) => first.min(highest),
                    None => rng.random_int(0, highest as i32) as usize,
                };
                let center = if cp == 0 {
                    origin
                } else {
                    point(&logic, batch.controls[cp - 1])
                };
                let mut offset = Vec3::new(
                    between(rng, [-1., 1.]),
                    between(rng, [-1., 1.]),
                    between(rng, [-1., 1.]),
                )
                .normalize_or_zero()
                    * between(rng, [0., d.distance])
                    * d.bias;
                if let Some(warp) = d.warp {
                    let scale = warp[0].lerp(warp[1], between(rng, [0., 1.]));
                    let matrix = logic.world.world_matrix(batch.owner);
                    let local = Vec3::from_array(
                        matrix
                            .inverse()
                            .transform_vector3(l4d2r_math::Vec3::from_array(offset.to_array()))
                            .to_array(),
                    );
                    offset = emitter_offset(local * scale, true, matrix);
                }
                let mut radius = d.radius[0]
                    + (d.radius[1] - d.radius[0])
                        * between(rng, [0., 1.]).powf(d.radius_exponent.max(0.01));
                let velocity = Vec3::new(
                    between(rng, [d.velocity[0].x, d.velocity[1].x]),
                    between(rng, [d.velocity[0].y, d.velocity[1].y]),
                    between(rng, [d.velocity[0].z, d.velocity[1].z]),
                );
                let authored_offset = d.offset[0].lerp(d.offset[1], between(rng, [0., 1.]))
                    * if d.proportional_offset { radius } else { 1. };
                let authored_offset = emitter_offset(
                    authored_offset,
                    d.offset_local,
                    logic.world.world_matrix(batch.owner),
                );
                let offset = offset + authored_offset;
                let mut center = center;
                if e.me.targetname.starts_with("__rewrite_muzzle_fp_") {
                    if let Some(steps) = d.path_steps {
                        let (pitch, yaw) = (e.me.angles.x.to_radians(), e.me.angles.y.to_radians());
                        let forward = Vec3::new(
                            pitch.cos() * yaw.cos(),
                            -pitch.sin(),
                            -pitch.cos() * yaw.sin(),
                        );
                        let length = d.distance_radius.map_or(0., |(range, _)| range[1]);
                        center += forward
                            * length
                            * 0.0254
                            * (particle_index as f32 / (steps - 1.)).clamp(0., 1.);
                    }
                }
                if let Some((input, output)) = d.distance_radius {
                    radius = remap(center.distance(origin) / 0.0254, input, output);
                }
                let mut alpha = between(rng, d.alpha) / 255.;
                if let Some((input, output, scale)) = d.radius_alpha {
                    let value = remap(radius, input, output);
                    alpha = if scale { alpha * value } else { value };
                }
                let sequence = rng.random_int(
                    d.sequences[0].min(d.sequences[1]) as i32,
                    d.sequences[0].max(d.sequences[1]) as i32,
                ) as usize;
                let mut life = if d.lifetime_fps > 0. {
                    batch
                        .sheet
                        .iter()
                        .find(|s| s.id == sequence)
                        .map_or(1., |s| s.duration / d.lifetime_fps)
                } else {
                    between(rng, d.life)
                };
                // Creation is inside the elapsed emission interval, never in
                // the future. Parent interpolation reads this same timestamp.
                let creation = (local.min(if d.duration>0. {d.start+d.duration} else {local}) - birth_age(emission_dt, particle_index, count)).max(d.start);
                for &(field, input, output, window, scale) in &d.creation_remaps {
                    if (window[0] >= 0. && creation < window[0])
                        || (window[1] >= 0. && creation > window[1])
                    {
                        continue;
                    }
                    let value = remap(creation, input, output);
                    let target = match field {
                        1 => &mut life,
                        3 => &mut radius,
                        7 => &mut alpha,
                        _ => continue,
                    };
                    *target = if scale { *target * value } else { value };
                }
                if let Some(noise) = &effects.noise {
                    for operator in &d.noise {
                        let position =
                            center + Vec3::from_array(binary::source_position(offset.to_array()));
                        let source_position =
                            Vec3::new(position.x, -position.z, position.y) / 0.0254;
                        let coordinates = source_position * operator.spatial
                            + operator.offset
                            + Vec3::splat(
                                (creation + operator.time_offset) * operator.time
                                    + now * operator.world,
                            );
                        let n = noise.sample(coordinates.to_array());
                        let fraction = if operator.invert {
                            1. - n.abs()
                        } else if operator.absolute {
                            n.abs()
                        } else {
                            (n + 1.) * 0.5
                        };
                        let value = operator.output[0]
                            + (operator.output[1] - operator.output[0]) * fraction;
                        let target = match operator.field {
                            1 => &mut life,
                            3 => &mut radius,
                            7 => &mut alpha,
                            _ => continue,
                        };
                        *target = if operator.scale {
                            *target * value
                        } else {
                            value
                        };
                    }
                }
                let mut velocity = emitter_offset(
                    velocity,
                    d.velocity_local,
                    logic.world.world_matrix(batch.owner),
                );
                let mut position =
                    center + Vec3::from_array(binary::source_position(offset.to_array()));
                if let Some((input, output, window, local_space)) = d.position_remap {
                    if (window[0] < 0. || creation >= window[0])
                        && (window[1] < 0. || creation < window[1])
                    {
                        let value = output[0].lerp(output[1], remap(creation, input, [0., 1.]));
                        let value = emitter_offset(
                            value,
                            local_space,
                            logic.world.world_matrix(batch.owner),
                        );
                        position =
                            origin + Vec3::from_array(binary::source_position(value.to_array()));
                        if let Some(warp) = d.warp {
                            let matrix = logic.world.world_matrix(batch.owner);
                            let value = Vec3::from_array(
                                matrix
                                    .inverse()
                                    .transform_vector3(l4d2r_math::Vec3::from_array(
                                        value.to_array(),
                                    ))
                                    .to_array(),
                            );
                            let scale = Vec3::new(
                                between(rng, [warp[0].x, warp[1].x]),
                                between(rng, [warp[0].y, warp[1].y]),
                                between(rng, [warp[0].z, warp[1].z]),
                            );
                            position = origin
                                + Vec3::from_array(binary::source_position(
                                    emitter_offset(value * scale, true, matrix).to_array(),
                                ));
                        }
                    }
                }
                if let Some(op) = &d.parent_particles {
                    if let Some(parent) = parent {
                        let Some(index) = parent_index(
                            parent.particles.len(),
                            op.random,
                            &mut batch.parent_cursor,
                            rng,
                        ) else {
                            continue;
                        };
                        let p = &parent.particles[index];
                        let fraction = if dt > 0. {
                            (creation - (local - dt)) / dt
                        } else {
                            1.
                        };
                        let (inherited_position, inherited_velocity) = inherit_parent(
                            p.previous_position,
                            p.position,
                            fraction,
                            op.inherited_velocity,
                            dt,
                        );
                        position = inherited_position;
                        velocity = Vec3::new(
                            inherited_velocity.x,
                            -inherited_velocity.z,
                            inherited_velocity.y,
                        ) / 0.0254;
                    } else {
                        // The native initializer uses the zero vector without
                        // a parent. An existing-but-empty parent kills births.
                        position = Vec3::ZERO;
                        velocity = Vec3::ZERO;
                    }
                    if op.offset_after {
                        position +=
                            Vec3::from_array(binary::source_position(authored_offset.to_array()));
                    }
                }
                if d.parent_particles
                    .as_ref()
                    .is_none_or(|p| p.velocity_noise_after)
                {
                    if let Some(noise) = &effects.noise {
                        let source_position =
                            Vec3::new(position.x, -position.z, position.y) / 0.0254;
                        for op in &d.velocity_noise {
                            velocity += emitter_offset(
                                coherent_velocity(noise, op, source_position, creation),
                                op.local,
                                logic.world.world_matrix(batch.owner),
                            );
                        }
                    }
                }
                // Births are distributed across the authored emission interval.
                // Advance only the interval remaining after this particle's birth,
                // never the entire preceding render-frame dt.
                let mut age = (local-creation).max(0.);
                if let (Some(noise), Some(op)) = (&effects.noise, &d.pre_age) {
                    let source_position = Vec3::new(position.x, -position.z, position.y) / 0.0254;
                    let coordinates = (source_position + op.offset) * op.spatial
                        + Vec3::splat((creation + op.time_offset) * op.time);
                    let mut n = noise.sample(coordinates.to_array());
                    if op.absolute {
                        n = n.abs();
                    }
                    if op.invert {
                        n = 1. - n;
                    }
                    let scale = if op.absolute { 1. } else { 0.5 };
                    age += (op.output[0]
                        + (op.output[1] - op.output[0]) * (1. - scale + scale * n))
                        .clamp(0., 1.)
                        * life;
                }
                let kill_at = d.cull.map_or(f32::INFINITY, |(range, exponent, percent)| {
                    if between(rng, [0., 1.]) <= percent {
                        life * (range[0]
                            + (range[1] - range[0]) * between(rng, [0., 1.]).powf(exponent))
                    } else {
                        f32::INFINITY
                    }
                });
                let mut normal = Vec3::Y;
                if let Some((length, offset, kill_on_miss)) = d.ground {
                    let hit = collision.physics_trace(
                        position,
                        position - Vec3::Y * (length.max(0.) * 0.0254),
                        Vec3::ZERO,
                    );
                    if hit.start_solid || hit.fraction >= 1. {
                        if kill_on_miss {
                            continue;
                        }
                    } else {
                        position += -Vec3::Y * (length.max(0.) * 0.0254) * hit.fraction
                            + hit.normal * (offset * 0.0254);
                        normal = hit.normal.normalize_or(Vec3::Y);
                    }
                }
                batch.particles.push(Particle {
                    rendered_frames: 0,
                    previous_position: position
                        - Vec3::from_array(binary::source_position(velocity.to_array())) * dt,
                    normal,
                    position,
                    kill_at,
                    flipped: d.flip > 0. && between(rng, [0., 1.]) < d.flip,
                    trail_time: d.trail_time.0[0]
                        + (d.trail_time.0[1] - d.trail_time.0[0])
                            * between(rng, [0., 1.]).powf(d.trail_time.1),
                    velocity: Vec3::from_array(binary::source_position(velocity.to_array())),
                    age,
                    life: life.max(0.001),
                    radius: radius * 0.0254,
                    alpha,
                    color: d.color[0].lerp(d.color[1], between(rng, [0., 1.])) / 255.,
                    sequence,
                    second_sequence: rng.random_int(
                        d.second_sequences[0].min(d.second_sequences[1]) as i32,
                        d.second_sequences[0].max(d.second_sequences[1]) as i32,
                    ) as usize,
                    rotation: d.rotation[0]
                        + (d.rotation[1] - d.rotation[0])
                            * between(rng, [0., 1.]).powf(d.rotation_exponent),
                    fade: std::array::from_fn(|i| {
                        d.fade_random[i].map_or(d.fade[i], |(range, proportional, exponent)| {
                            let value = range[0]
                                + (range[1] - range[0])
                                    * between(rng, [0., 1.]).powf(exponent.max(0.001));
                            value * if proportional { life } else { 1. }
                        })
                    }),
                });
                total += 1;
            }
        }
        let before = batch.particles.len();
        if emitting
            && before > old_count
            && e.me.targetname.starts_with("__rewrite_muzzle_fp_")
            && std::env::args().any(|a| a == "--capture-muzzle-debug")
        {
            let peak = batch
                .particles
                .iter()
                .map(|p| {
                    p.alpha
                        * alpha_envelope(minimum_frame_age(p.age,p.life,p.rendered_frames,minimum_frames), p.life, p.fade, d.fade_in_spline, d.fade_out_style)
                })
                .fold(0., f32::max);
            let peak_screen = batch
                .particles
                .iter()
                .map(|p| {
                    let radius = p.radius
                        * radius_scale(
                            minimum_frame_age(p.age,p.life,p.rendered_frames,minimum_frames) / p.life,
                            d.scale,
                            d.scale_window,
                            d.scale_bias,
                            d.scale_ease,
                        );
                    p.alpha
                        * alpha_envelope(minimum_frame_age(p.age,p.life,p.rendered_frames,minimum_frames), p.life, p.fade, d.fade_in_spline, d.fade_out_style)
                        * sprite_extent(
                            radius,
                            p.position.distance(camera.translation),
                            batch.sizes,
                        )
                        .1
                })
                .fold(0., f32::max);
            println!(
                "Muzzle particles: {} material={} new={} peak_alpha={peak:.4} screen_alpha={peak_screen:.4} local={local:.5}",
                e.me.targetname,
                d.material,
                before - old_count
            );
        }
        let control = logic.world.world_matrix(batch.owner);
        let delta = batch
            .previous_control
            .map(|previous| control * previous.inverse());
        for p in batch.particles.iter_mut().take(old_count) {
            p.previous_position = p.position;
            p.age += dt;
            p.velocity *= (1. - d.drag).powf(dt * 30.);
            p.velocity += Vec3::from_array(binary::source_position(d.gravity.to_array())) * dt;
            p.position += p.velocity * dt;
            if let (Some((rotation, range)), Some(delta)) = (d.control_lock, delta) {
                let shift = Vec3::from_array(binary::source_position(
                    control.w_axis.truncate().to_array(),
                ));
                let previous_origin = batch.previous_control.unwrap().w_axis.truncate();
                let translation = Vec3::from_array(binary::source_position(
                    (control.w_axis.truncate() - previous_origin).to_array(),
                ));
                let distance = (p.position + translation - shift).length();
                let fraction = if range > 0. {
                    (distance / range)
                        .clamp(0., 1.)
                        .powf(0.2_f32.ln() * -1.4427)
                } else {
                    0.
                };
                let strength = (1. - fraction) * (p.age / dt.max(0.001)).clamp(0., 1.);
                if rotation {
                    let source =
                        l4d2r_math::Vec3::new(p.position.x, -p.position.z, p.position.y) / 0.0254;
                    let moved = Vec3::from_array(binary::source_position(
                        delta.transform_point3(source).to_array(),
                    ));
                    p.position = p.position.lerp(moved, strength);
                    let velocity =
                        l4d2r_math::Vec3::new(p.velocity.x, -p.velocity.z, p.velocity.y) / 0.0254;
                    let turned = Vec3::from_array(binary::source_position(
                        delta.transform_vector3(velocity).to_array(),
                    ));
                    p.velocity = p.velocity.lerp(turned, strength);
                } else {
                    p.position += translation * strength;
                }
            }
        }
        batch.previous_control = Some(control);
        batch.particles.retain(|p| {
            minimum_frame_age(p.age,p.life,p.rendered_frames,minimum_frames) < p.life
                && minimum_frame_age(p.age,p.life,p.rendered_frames,minimum_frames) < p.kill_at
                && e.alive
                && d.cull_plane
                    .is_none_or(|offset| p.position.y >= origin.y + offset)
        });
        if d.rope {
            if let Some(end) = batch.controls.first().map(|id| point(&logic, *id)) {
                let count = batch.particles.len().saturating_sub(1).max(1) as f32;
                for (i, p) in batch.particles.iter_mut().enumerate() {
                    p.position = origin.lerp(end, i as f32 / count);
                }
            }
        }
        total = total.saturating_sub(before - batch.particles.len());
        static CULL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        // The far plane (world only; the 3D skybox has its own view).
        let reach = if sky.is_none() && effects_far > 0. {
            d.max_draw_distance.min(effects_far)
        } else {
            d.max_draw_distance
        };
        let within_draw_distance = reach >= 2500.
            || batch.particles.iter().any(|p| {
                p.position.distance_squared(camera.translation) <= (reach + p.radius).powi(2)
            });
        let visible_geometry = show
            && within_draw_distance
            && (!*CULL.get_or_init(|| !std::env::args().any(|a| a == "--no-particle-frustum"))
                || particle_bounds(
                    &batch.particles,
                    d.live_distance_radius.map_or(d.scale, |(_, out, _, _)| {
                        let grow = out[0].abs().max(out[1].abs()).max(d.scale[0].abs()).max(d.scale[1].abs());
                        [grow, grow]
                    }),
                    batch
                        .particles
                        .iter()
                        .map(|p| (p.velocity.length() * p.trail_time).min(d.trail))
                        .fold(0., f32::max),
                )
                .is_none_or(|bound| frustum.intersects_obb_identity(&bound)));
        static VIS_WORLD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let vis_world = *VIS_WORLD.get_or_init(|| std::env::var_os("L4D2R_PFX_VIS_WORLD").is_some());
        for (renderer_index, renderer) in d.renderers.iter().enumerate() {
            // Renderer visibility fade by the camera's distance to the
            // proxy control point (the system origin). 3D-skybox systems
            // measure in sky space (debug L4D2R_PFX_VIS_WORLD: from the
            // world camera instead).
            let visibility_alpha = renderer.visibility.map_or(1., |[lo, hi, alpha_lo, alpha_hi]| {
                let eye = if vis_world { world_camera.translation } else { camera.translation };
                let distance = origin.distance(eye) / 0.0254;
                let t = ((distance - lo) / (hi - lo)).clamp(0., 1.);
                alpha_lo + (alpha_hi - alpha_lo) * t
            });
            if let Some(forward_angle) = renderer.screen_rotation {
                // Keep authored renderer order: a modifier after a sprite
                // pass changes rotation for subsequent passes/the next frame.
                if visible_geometry {
                    for p in &mut batch.particles {
                        if let Some(rotation) = screen_velocity_rotation(
                            p.position - p.previous_position,
                            camera.rotation * Vec3::X,
                            camera.rotation * Vec3::Y,
                            forward_angle,
                        ) {
                            p.rotation = rotation;
                        }
                    }
                }
                continue;
            }
            let Some(draw) = batch
                .draws
                .iter()
                .find(|draw| draw.renderer_index == renderer_index)
            else {
                continue;
            };
            if let Ok(mut v) = visibility.get_mut(draw.entity) {
                let next = if batch.particles.is_empty() || !visible_geometry {
                    Visibility::Hidden
                } else {
                    Visibility::Inherited
                };
                if *v != next {
                    *v = next;
                }
            }
            // Bevy cannot upload zero-byte meshes into the slab allocator. Keep the last
            // nonempty mesh hidden until the emitter has live vertices again.
            if batch.particles.is_empty() || !visible_geometry {
                continue;
            }
            let mut positions = Vec::with_capacity(batch.particles.len() * 4);
            let mut uv = Vec::with_capacity(positions.capacity());
            let mut next_uv = Vec::with_capacity(positions.capacity());
            let mut second_uv = Vec::with_capacity(positions.capacity());
            let mut frame_blends = Vec::with_capacity(positions.capacity());
            let mut colors = Vec::with_capacity(positions.capacity());
            let mut indices = Vec::with_capacity(batch.particles.len() * 6);
            let camera_right = camera.rotation * Vec3::X;
            let camera_up = camera.rotation * Vec3::Y;
            for (index, p) in batch.particles.iter().enumerate() {
                let sample_age=minimum_frame_age(p.age,p.life,p.rendered_frames,minimum_frames);
                let scale = radius_scale(
                    sample_age / p.life,
                    d.scale,
                    d.scale_window,
                    d.scale_bias,
                    d.scale_ease,
                );
                let (radius, screen_fade) = if e.me.targetname.starts_with("__rewrite_muzzle_fp_") {
                    sprite_extent(
                        p.radius * scale,
                        p.position.distance(camera.translation),
                        batch.sizes,
                    )
                } else {
                    (live_radius(d, p, origin).unwrap_or(p.radius * scale), 1.)
                };
                let (base_right, base_up) = if matches!(renderer.orientation, 2 | 3) {
                    let normal = if renderer.orientation == 3 {
                        p.normal
                    } else {
                        Vec3::Y
                    };
                    let right = normal.any_orthonormal_vector();
                    (right, normal.cross(right))
                } else {
                    (camera_right, camera_up)
                };
                let (right, up) = billboard_axes(base_right, base_up, p.rotation, radius);
                // $cropfactor keeps the frame's proportions: the sprite
                // shrinks to the cropped part of the frame.
                let (mut right, up) = (right * batch.crop[0], up * batch.crop[1]);
                if p.flipped {
                    right = -right;
                }
                let corners = if renderer.rope {
                    let Some(next) = batch.particles.get(index + 1) else {
                        continue;
                    };
                    let side = (next.position - p.position)
                        .cross(camera.translation - p.position)
                        .normalize_or_zero()
                        * p.radius
                        * scale;
                    [
                        p.position - side,
                        p.position + side,
                        next.position + side,
                        next.position - side,
                    ]
                } else if renderer.trail > 0. {
                    let length = p.velocity.length()
                        * p.trail_time
                        * if renderer.trail_fade > 0. {
                            (sample_age / renderer.trail_fade).clamp(0., 1.)
                        } else {
                            1.
                        };
                    if length <= 0. {
                        continue;
                    }
                    let length =
                        length.clamp(renderer.trail_min.min(renderer.trail), renderer.trail);
                    let tail = p.position - p.velocity.normalize_or_zero() * length;
                    let side = (p.position - tail)
                        .cross(camera.translation - p.position)
                        .normalize_or_zero()
                        * (p.radius * scale).min(length)
                        * 0.5;
                    [
                        tail - side,
                        tail + side,
                        p.position + side,
                        p.position - side,
                    ]
                } else {
                    [
                        p.position - right - up,
                        p.position + right - up,
                        p.position + right + up,
                        p.position - right + up,
                    ]
                };
                let sequence = batch
                    .sheet
                    .iter()
                    .find(|s| s.id == p.sequence)
                    .or_else(|| batch.sheet.first());
                let duration = sequence.map_or(1., |s| s.duration);
                let frames = sequence.map_or(1, |s| s.frames.len());
                let phase = sprite_phase(
                    sample_age,
                    p.life,
                    duration,
                    frames,
                    renderer.fps,
                    renderer.animation_fps,
                    renderer.animation_fit,
                );
                let (atlas, next_atlas, blend) = sheet_frames(&batch.sheet, p.sequence, phase);
                let second = batch
                    .sheet
                    .iter()
                    .find(|s| s.id == p.second_sequence)
                    .or_else(|| batch.sheet.first());
                let phase2 = sprite_phase(
                    sample_age,
                    p.life,
                    second.map_or(1., |s| s.duration),
                    second.map_or(1, |s| s.frames.len()),
                    renderer.second_fps,
                    renderer.animation_fps,
                    renderer.animation_fit,
                );
                let (atlas2, next_atlas2, blend2) =
                    sheet_frames(&batch.sheet, p.second_sequence, phase2);
                let zoom_old = (1. + 1. / batch.zoom) * 0.5;
                let old_scale = zoom_old + (1. / batch.zoom - zoom_old) * blend2;
                let new_scale = 1. + (zoom_old - 1.) * blend2;
                let crop = |r: [f32; 4]| {
                    let (cx, cy) = (batch.crop[0], batch.crop[1]);
                    let (w, h) = (r[2] - r[0], r[3] - r[1]);
                    let (dx, dy) = (w * (1. - cx) * 0.5, h * (1. - cy) * 0.5);
                    [r[0] + dx, r[1] + dy, r[2] - dx, r[3] - dy]
                };
                let (atlas, next_atlas) = (crop(atlas), crop(next_atlas));
                let tex = [
                    [atlas[0], atlas[3]],
                    [atlas[2], atlas[3]],
                    [atlas[2], atlas[1]],
                    [atlas[0], atlas[1]],
                ];
                let next_tex = [
                    [next_atlas[0], next_atlas[3]],
                    [next_atlas[2], next_atlas[3]],
                    [next_atlas[2], next_atlas[1]],
                    [next_atlas[0], next_atlas[1]],
                ];
                let alpha = p.alpha
                    * alpha_envelope(sample_age, p.life, p.fade, d.fade_in_spline, d.fade_out_style);
                let color = d.color_fade.map_or(p.color, |(target, window, ease)| {
                    let mut t = remap(sample_age / p.life, window, [0., 1.]);
                    if ease {
                        t = t * t * (3. - 2. * t);
                    }
                    p.color.lerp(target, t)
                });
                let base = positions.len() as u32;
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
                for i in 0..4 {
                    positions.push(corners[i].to_array());
                    uv.push(tex[i]);
                    next_uv.push(next_tex[i]);
                    let corner = [[0., 1.], [1., 1.], [1., 0.], [0., 0.]][i];
                    let a = atlas_corner(atlas2, corner, old_scale);
                    let b = atlas_corner(next_atlas2, corner, new_scale);
                    second_uv.push([a[0], a[1], b[0], b[1]]);
                    frame_blends.push([blend, blend2, screen_fade]);
                    colors.push([color.x, color.y, color.z, alpha * visibility_alpha]);
                }
            }
            if positions.is_empty() {
                if let Ok(mut v) = visibility.get_mut(draw.entity) {
                    *v = Visibility::Hidden;
                }
                continue;
            }
            let mut sky_fog = Vec::new();
            if let Some((origin, rotation, scale, _, _, fog, _)) = sky {
                for position in positions.iter_mut() {
                    let world = *rotation * (Vec3::from_array(*position) - *origin) * *scale;
                    *position = world.to_array();
                    // sky_camera fog (enabled in w), squared like the world
                    // fog. The shader blends to the exposure-scaled colour
                    // after the texture: blending the vertex colour toward
                    // the raw colour left c7m1's sky smoke a dark blot
                    // inside retail's fully fogged horizon.
                    if fog.w != 0. && batch.fogged {
                        let ramp = ((world.distance(world_camera.translation) - fog.x)
                            / (fog.y - fog.x).max(1e-4))
                        .clamp(0., fog.z);
                        sky_fog.push(ramp * ramp);
                    }
                }
            }
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            for position in &positions {
                let position = Vec3::from_array(*position);
                lo = lo.min(position);
                hi = hi.max(position);
            }
            if let Ok(mut bound) = bounds.get_mut(draw.entity) {
                *bound = Aabb::from_min_max(lo, hi);
            }
            if let Ok((mut mesh, mut tag, mut transform)) = draws.get_mut(draw.entity) {
                match pull.push_fogged(
                    &positions,
                    &uv,
                    &colors,
                    Some((&next_uv, &frame_blends, &second_uv)),
                    &sky_fog,
                ) {
                    Some((capacity, packed)) => {
                        if mesh.0 != capacity {
                            mesh.0 = capacity;
                        }
                        tag.set_if_neq(bevy::mesh::MeshTag(packed));
                        // Sort/cull from the cloud centre, matching the AABB centre
                        // Bevy took from the old per-batch mesh.
                        let center = (lo + hi) * 0.5;
                        if transform.translation != center {
                            transform.translation = center;
                        }
                        if let Ok(mut bound) = bounds.get_mut(draw.entity) {
                            *bound = Aabb::from_min_max(lo - center, hi - center);
                        }
                    }
                    None => {
                        if let Ok(mut v) = visibility.get_mut(draw.entity) {
                            *v = Visibility::Hidden;
                        }
                    }
                }
                continue;
            }
            if let Some(mut mesh) = meshes.get_mut(&draw.mesh) {
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, next_uv);
                mesh.insert_attribute(crate::particle_sprite::FRAME_BLEND, frame_blends);
                mesh.insert_attribute(crate::particle_sprite::SECOND_UV, second_uv);
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
                mesh.insert_indices(Indices::U32(indices));
            }
        }
        if visible_geometry && !batch.draws.is_empty() {
            for p in &mut batch.particles {p.rendered_frames=p.rendered_frames.saturating_add(1);}
        }
    }
    pull.flush(&mut writes);
}
fn billboard_axes(right: Vec3, up: Vec3, angle: f32, radius: f32) -> (Vec3, Vec3) {
    let (s, c) = angle.sin_cos();
    ((right * c + up * s) * radius, (up * c - right * s) * radius)
}
// Retail client.dll C_OP_RenderScreenVelocityRotate::Render (103c8b40)
// writes atan2(view-space delta Y, delta X) + authored forward angle.
fn screen_velocity_rotation(delta: Vec3, right: Vec3, up: Vec3, forward: f32) -> Option<f32> {
    let x = delta.dot(right);
    let y = delta.dot(up);
    (x.is_finite() && y.is_finite()).then(|| y.atan2(x) + forward)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_tick_emission_consumes_short_windows_once_across_render_rates() {
        // A 25ms flash must not disappear when the next map tick is 33ms
        // away, or be emitted twice while map time stays on the same tick.
        for render_hz in [30.,60.,144.] {
            let mut previous=0.;let mut sum=0.;
            for i in 0..60 {
                let time=((i as f32/render_hz)*30.).floor()/30.;
                sum+=emission_overlap(time,(time-previous).max(0.),0.,0.025);
                previous=time;
            }
            assert!((sum-0.025).abs()<1e-6,"{render_hz}: {sum}");
        }
        assert_eq!(emission_overlap(1./30.,1./60.,0.,0.015),0.,"old render-dt window misses the complete flash");
        assert!((emission_overlap(1./30.,1./30.,0.,0.015)-0.015).abs()<1e-6);
    }
    #[test]
    fn short_foreground_effect_keeps_requested_sample_without_extending_simulation_life() {
        assert_eq!(foreground_minimum_frames(0,true),1,"short child effects must reach the foreground camera too");
        assert_eq!(foreground_minimum_frames(3,true),3);
        assert_eq!(foreground_minimum_frames(3,false),0,"world effects retain the prior decay policy");
        let life=0.015;
        let age=1./30.;
        let sample=minimum_frame_age(age,life,0,1);
        assert!(alpha_envelope(sample,life,[0.,0.01],false,None)>0.);
        assert_eq!(minimum_frame_age(age,life,1,1),age);
        assert_eq!(minimum_frame_age(0.2,life,0,1),0.2,"unseen flashes must expire rather than reappear much later");
        assert_eq!(minimum_frame_age(age,life,0,0),age,"ordinary effects retain their lifetime");
        assert_eq!(particle_budget(8192,false),0);
        assert_eq!(particle_budget(8192,true),512);
        assert_eq!(particle_budget(8704,true),0);
    }
    #[test]
    fn emission_intervals_conserve_authored_duration_at_boundaries() {
        for hz in [30.,60.,144.] {
            let dt=1./hz;let sum=(1..=(hz as usize*3)).map(|i|emission_overlap(i as f32*dt,dt,0.213,1.031)).sum::<f32>();
            assert!((sum-1.031).abs()<0.00002,"{hz}: {sum}");
        }
        assert!((emission_overlap(1.05,0.1,0.,1.)-0.05).abs()<1e-6);
        assert_eq!(emission_overlap(-0.1,0.1,0.,1.),0.);
    }
    #[test]
    fn nested_child_delays_are_relative_to_their_collection_and_keep_emitter_start() {
        use crate::source_pcf::Value;
        let node=|name:&str,attrs|Element{name:name.into(),kind:"DmeParticleSystemDefinition".into(),attributes:attrs};
        let elements=vec![
            node("root",std::collections::BTreeMap::from([("children".into(),Value::Array(vec![Value::Int(1)]))])),
            node("child link",std::collections::BTreeMap::from([("child".into(),Value::Int(2)),("delay".into(),Value::Float(0.5))])),
            node("child",std::collections::BTreeMap::from([("children".into(),Value::Array(vec![Value::Int(3)]))])),
            node("grandchild link",std::collections::BTreeMap::from([("child".into(),Value::Int(4)),("delay".into(),Value::Float(0.25))])),
            node("grandchild",std::collections::BTreeMap::new())];
        let mut d=vec![];flatten(&elements,0,&mut HashSet::new(),&mut d);
        assert_eq!(d.iter().map(|v|v.child_delay).collect::<Vec<_>>(),vec![0.,0.5,0.75]);
        assert_eq!(emission_overlap(0.7-d[2].child_delay,0.1,0.,0.),0.);
    }
    /// L4D2R_PCF_DUMP=<definition>: prints that definition tree (children,
    /// functions and their attributes) from whichever PCF holds it.
    #[test]
    #[ignore = "debug dump"]
    fn dump_definition_tree() {
        let Ok(name) = std::env::var("L4D2R_PCF_DUMP") else { return };
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");if !root.exists(){return;}
        let mut assets=SourceAssets::new(root,vec![]).unwrap();
        fn show(e: &[source_pcf::Element], i: usize, depth: usize, seen: &mut HashSet<usize>) {
            let pad = "  ".repeat(depth);
            let n = &e[i];
            println!("{pad}[{}] {}", n.kind, n.name);
            if !seen.insert(i) { return; }
            for (k, v) in &n.attributes {
                match v {
                    source_pcf::Value::Array(items) if items.iter().all(|x| matches!(x, source_pcf::Value::Reference(_) | source_pcf::Value::Int(_))) && k != "functionName" => {
                        println!("{pad}  {k}:");
                        for c in n.references(k) { if c < e.len() { show(e, c, depth + 2, seen); } }
                    }
                    _ => println!("{pad}  {k} = {v:?}"),
                }
            }
        }
        for path in assets.matching_paths(".pcf") {
            let Ok(bytes)=assets.read(&path) else {continue};
            let Ok(elements)=source_pcf::parse(&bytes) else {continue};
            if let Some(i) = elements.iter().position(|e| e.name == name && e.kind == "DmeParticleSystemDefinition") {
                println!("== {path}");
                show(&elements, i, 0, &mut HashSet::new());
                return;
            }
        }
        println!("{name}: not found");
    }
    #[test]
    fn audit_owned_child_delay_metadata() {
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");if !root.exists(){return;}
        let mut assets=SourceAssets::new(root,vec![]).unwrap();let mut delays=0;
        for path in ["particles/fire_01.pcf","particles/weapon_fx.pcf","particles/infected_fx.pcf"] {
            let Ok(bytes)=assets.read(path) else {continue};let elements=source_pcf::parse(&bytes).unwrap();
            for node in &elements {for child in node.references("children") {let c=&elements[child];let delay=c.number("delay",0.);if delay>0. {println!("Native child delay {path}: {} delay {delay}",node.name);delays+=1;}}}
        }
        println!("Owned delayed child links: {delays}");
    }
    #[test]
    fn native_parent_selection_wraps_survives_shrink_and_never_selects_empty() {
        let mut rng = UniformRandomStream::new(17);
        let mut cursor = 0;
        let selected: Vec<_> = (0..5)
            .map(|_| parent_index(3, false, &mut cursor, &mut rng).unwrap())
            .collect();
        assert_eq!(selected, [0, 1, 2, 0, 1]);
        assert_eq!(parent_index(1, false, &mut cursor, &mut rng), Some(0));
        assert_eq!(parent_index(0, false, &mut cursor, &mut rng), None);
        for _ in 0..64 {
            assert!(parent_index(3, true, &mut cursor, &mut rng).unwrap() < 3);
        }
    }
    #[test]
    fn native_parent_motion_is_interpolated_at_birth_with_scaled_previous_position() {
        let (position, velocity) =
            inherit_parent(Vec3::splat(10.), Vec3::splat(14.), 0.25, 2., 0.5);
        assert_eq!(position, Vec3::splat(11.));
        assert_eq!(velocity, Vec3::splat(4.));
        assert_eq!(
            inherit_parent(Vec3::ZERO, Vec3::X, 1., 0., 0.01).1,
            Vec3::ZERO
        );
        assert_eq!(inherit_parent(Vec3::ZERO, Vec3::X, 2., 1., 1.).0, Vec3::X);
    }
    #[test]
    fn emission_scaling_preserves_fractional_control_point_and_parent_multipliers() {
        assert_eq!(emission_scale(0.5, false, 4, None), 2.);
        assert_eq!(emission_scale(0., false, 4, None), 1.);
        // Only control point 0: the product is zero, so no scaling.
        assert_eq!(emission_scale(1., false, 0, None), 1.);
        assert_eq!(emission_scale(0.5, true, 4, Some(10)), 5.);
        assert_eq!(emission_scale(1., true, 4, Some(0)), 0.);
        assert_eq!(emission_scale(1., true, 4, None), 1.);
        assert_eq!(emission_scale(0., true, 4, Some(10)), 0.);
    }
    #[test]
    fn screen_velocity_rotation_uses_camera_plane_and_authored_forward_angle() {
        let forward = std::f32::consts::PI;
        assert_eq!(
            screen_velocity_rotation(Vec3::X, Vec3::X, Vec3::Y, forward),
            Some(forward)
        );
        assert!(
            (screen_velocity_rotation(Vec3::Y, Vec3::X, Vec3::Y, forward).unwrap() - forward * 1.5)
                .abs()
                < 1e-6
        );
        let turn = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        assert!(
            (screen_velocity_rotation(turn * Vec3::X, turn * Vec3::X, turn * Vec3::Y, forward)
                .unwrap()
                - forward)
                .abs()
                < 1e-6
        );
        assert_eq!(
            screen_velocity_rotation(Vec3::ZERO, Vec3::X, Vec3::Y, forward),
            Some(forward)
        );
        assert!(
            screen_velocity_rotation(Vec3::splat(f32::NAN), Vec3::X, Vec3::Y, forward).is_none()
        );
        // Forward/backward movement alone must not become a vertical streak.
        assert_eq!(
            screen_velocity_rotation(Vec3::Z, Vec3::X, Vec3::Y, 0.),
            Some(0.)
        );
    }
    #[test]
    fn multiple_renderer_clocks_do_not_overwrite_each_other_or_duplicate_emission() {
        use crate::source_pcf::Value;
        let node = |name: &str, attributes| Element {
            name: name.into(),
            kind: String::new(),
            attributes,
        };
        let root = node(
            "shared",
            std::collections::BTreeMap::from([
                ("material".into(), Value::Text("particle/test".into())),
                ("initial_particles".into(), Value::Int(8)),
                (
                    "renderers".into(),
                    Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
                ),
            ]),
        );
        let first = node(
            "first",
            std::collections::BTreeMap::from([
                (
                    "functionName".into(),
                    Value::Text("render_animated_sprites".into()),
                ),
                ("animation rate".into(), Value::Float(0.)),
            ]),
        );
        let second = node(
            "second",
            std::collections::BTreeMap::from([
                (
                    "functionName".into(),
                    Value::Text("render_animated_sprites".into()),
                ),
                ("orientation_type".into(), Value::Int(2)),
                ("animation rate".into(), Value::Float(30.)),
                ("use animation rate as FPS".into(), Value::Bool(true)),
            ]),
        );
        let unknown = node(
            "unknown",
            std::collections::BTreeMap::from([(
                "functionName".into(),
                Value::Text("render_models".into()),
            )]),
        );
        let elements = [root, first, second, unknown];
        let mut collections = vec![];
        flatten(&elements, 0, &mut HashSet::new(), &mut collections);
        assert_eq!(collections.len(), 1);
        let d = &collections[0];
        assert_eq!(d.burst, 8);
        assert_eq!(d.renderers.len(), 2);
        let a = &d.renderers[0];
        let b = &d.renderers[1];
        assert_eq!(a.orientation, 0);
        assert_eq!(b.orientation, 2);
        assert_eq!(
            sprite_phase(0.5, 2., 1., 20, a.fps, a.animation_fps, a.animation_fit),
            0.
        );
        assert_eq!(
            sprite_phase(0.5, 2., 1., 20, b.fps, b.animation_fps, b.animation_fit),
            0.75
        );
        let mut unsupported = elements[0].clone();
        unsupported
            .attributes
            .insert("renderers".into(), Value::Array(vec![Value::Int(3)]));
        assert!(
            definition(
                &[
                    unsupported,
                    elements[1].clone(),
                    elements[2].clone(),
                    elements[3].clone()
                ],
                0
            )
            .unwrap()
            .renderers
            .is_empty()
        );
    }
    #[test]
    fn native_acid_definitions_retain_ground_placement_and_authored_initial_particles() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let elements =
            source_pcf::parse(&assets.read("particles/spitter_fx.pcf").unwrap()).unwrap();
        let index = elements
            .iter()
            .position(|e| {
                e.kind == "DmeParticleSystemDefinition"
                    && e.name == "spitter_areaofdenial_base_refract"
            })
            .unwrap();
        let d = definition(&elements, index).unwrap();
        assert_eq!(d.ground, Some((48., 1.5, true)));
        assert_eq!(d.renderers[1].orientation, 2);
        assert_eq!(d.burst, 2);
        // Both authored passes consume this one collection, retaining their
        // own orientation and clock rather than the final pass winning.
        assert_eq!(d.renderers.len(), 2);
        assert_eq!(d.renderers[0].orientation, 0);
        assert_eq!(d.renderers[1].orientation, 2);
        assert!((d.renderers[1].fps - 0.14).abs() < 1e-6);
        let (normal, amount, tint) = crate::material::particle_refraction(&mut assets, &d.material)
            .unwrap()
            .unwrap();
        assert_eq!(normal, "materials/particle/warp_pool_01_normal.vtf");
        assert!((amount - 0.07625).abs() < 1e-6);
        assert!(tint);
        assert!(!crate::material::particle_fog_enabled(&mut assets, &d.material).unwrap());
        assert_eq!(crate::material::particle_fog_mode(&mut assets, &d.material).unwrap(), 0.);
        // The c1m1 rooftop smoke (SpriteCard, $vertexfogamount 1): vertex fog
        // only, which the world pass skips like retail's HDR path.
        assert_eq!(crate::material::particle_fog_mode(&mut assets, "particle/smoke1/smoke1.vmt").unwrap(), 2.);
        assert!(!assets.read(&normal).unwrap().is_empty());
        let noise = crate::material::particle_spitter_noise(&mut assets, &d.material)
            .unwrap()
            .unwrap();
        assert_eq!(noise, "materials/liquids/water_noise.vtf");
        let noise = crate::vtf::decode(&assets.read(&noise).unwrap(), 2048).unwrap();
        assert!(noise.width > 1 && noise.height > 1);
        let cloud = elements
            .iter()
            .position(|e| {
                e.name == "spitter_areaofdenial_cloud" && e.kind == "DmeParticleSystemDefinition"
            })
            .unwrap();
        let cloud = definition(&elements, cloud).unwrap();
        assert!(cloud.parent_particles.is_some());
        assert!(cloud.scale_emission_by_parent);
        assert_eq!(cloud.point_scale, 1.);
        let parent_init = cloud.parent_particles.as_ref().unwrap();
        assert!(!parent_init.offset_after);
        assert!(!parent_init.velocity_noise_after);
        assert_eq!(cloud.renderers.len(), 2);
        assert!(cloud.renderers[0].screen_rotation.is_none());
        assert_eq!(
            cloud.renderers[1].screen_rotation,
            Some(std::f32::consts::PI)
        );
        assert_eq!(
            cloud
                .renderers
                .iter()
                .filter(|r| r.screen_rotation.is_none())
                .count(),
            1
        );
        let root = elements
            .iter()
            .position(|e| {
                e.kind == "DmeParticleSystemDefinition" && e.name == "spitter_areaofdenial"
            })
            .unwrap();
        let mut hierarchy = vec![];
        flatten(&elements, root, &mut HashSet::new(), &mut hierarchy);
        assert!(hierarchy[0].renderers.is_empty());
        assert!(hierarchy[0].parent.is_none());
        assert!(
            hierarchy
                .iter()
                .enumerate()
                .skip(1)
                .all(|(i, d)| d.parent.is_some_and(|parent| parent < i))
        );
    }
    #[test]
    fn sprite_animation_distinguishes_cycles_fps_and_fit_to_lifetime() {
        assert_eq!(sprite_phase(0.5, 2., 20., 20, 1., false, false), 10.);
        assert_eq!(sprite_phase(0.5, 2., 20., 20, 30., true, false), 15.);
        assert_eq!(sprite_phase(0.5, 2., 20., 20, 30., true, true), 5.);
        assert_eq!(sprite_phase(0.5, 2., 1., 20, 30., true, false), 0.75);
    }
    #[test]
    fn authored_smoke_growth_bias_and_active_window_are_respected() {
        assert!((radius_scale(0.5, [1., 6.], [0., 1.], 0.4, false) - 3.).abs() < 0.0001);
        assert_eq!(radius_scale(0.1, [1., 6.], [0.2, 0.8], 0.4, false), 1.);
        assert_eq!(radius_scale(1., [1., 6.], [0.2, 0.8], 0.4, false), 6.);
    }
    #[test]
    fn native_local_muzzle_offset_turns_with_attachment_without_double_translation() {
        let matrix = l4d2r_math::Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2)
            * l4d2r_math::Mat4::from_translation(l4d2r_math::Vec3::splat(50.));
        let offset = Vec3::X * 8.;
        assert!((emitter_offset(offset, true, matrix) - Vec3::Y * 8.).length() < 1e-5);
        assert_eq!(emitter_offset(offset, false, matrix), offset);
    }
    #[test]
    fn hotel_window_smoke_grows_with_distance_from_its_emitter() {
        // fire_window_smoke_hotel_2 (c1m1 burning-floor smoke): "Remap
        // Distance to Control Point to Scalar" scales the spawn radius 0.25x
        // at the window up to 7x 2000 units away, which makes retail's tall
        // column over the hotel roof.
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let elements = source_pcf::parse(&assets.read("particles/fire_01l4d.pcf").unwrap()).unwrap();
        let index = elements
            .iter()
            .position(|e| e.name == "fire_window_smoke_hotel_2" && e.kind == "DmeParticleSystemDefinition")
            .unwrap();
        let d = definition(&elements, index).unwrap();
        assert_eq!(d.live_distance_radius, Some(([0., 2000.], [0.25, 7.], true, false)));
        let mut p = Particle {
            rendered_frames: 0,
            previous_position: Vec3::ZERO,
            normal: Vec3::Y,
            fade: [0., 0.],
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            age: 0.,
            life: 10.,
            radius: 80.,
            alpha: 1.,
            color: Vec3::ONE,
            sequence: 0,
            second_sequence: 0,
            kill_at: f32::INFINITY,
            flipped: false,
            trail_time: 0.,
            rotation: 0.,
        };
        assert!((live_radius(&d, &p, Vec3::ZERO).unwrap() - 20.).abs() < 1e-3);
        p.position = Vec3::Y * 1000. * 0.0254;
        assert!((live_radius(&d, &p, Vec3::ZERO).unwrap() - 80. * 3.625).abs() < 1e-2);
        p.position = Vec3::Y * 5000. * 0.0254;
        assert!((live_radius(&d, &p, Vec3::ZERO).unwrap() - 560.).abs() < 1e-2);
    }
    #[test]
    fn native_pistol_flash_path_radius_and_alpha_remaps_are_preserved() {
        assert_eq!(remap(0., [0., 11.], [8., 3.5]), 8.);
        assert_eq!(remap(11., [0., 11.], [8., 3.5]), 3.5);
        assert_eq!(remap(8., [8., 3.5], [1., 0.25]), 1.);
        assert_eq!(remap(3.5, [8., 3.5], [1., 0.25]), 0.25);
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let elements = source_pcf::parse(&assets.read("particles/weapon_fx.pcf").unwrap()).unwrap();
        let index = elements
            .iter()
            .position(|e| e.name == "weapon_muzzle_flash_pistol_main")
            .unwrap();
        let d = definition(&elements, index).unwrap();
        assert_eq!(d.path_steps, Some(8.));
        assert_eq!(d.distance_radius, Some(([0., 11.], [8., 3.5])));
        assert_eq!(d.radius_alpha, Some(([8., 3.5], [1., 0.25], true)));
        let core = elements
            .iter()
            .position(|e| {
                e.name == "weapon_muzzle_flash_pistol_core2"
                    && e.kind == "DmeParticleSystemDefinition"
            })
            .unwrap();
        let core = definition(&elements, core).unwrap();
        assert_eq!(core.minimum_rendered_frames,1);
        assert!(
            core.creation_remaps
                .iter()
                .any(|(field, _, out, _, _)| *field == 1 && *out == [0.025, 0.015])
        );
    }
    #[test]
    fn audit_native_muzzle_operators() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let elements = source_pcf::parse(&assets.read("particles/weapon_fx.pcf").unwrap()).unwrap();
        let mut report = String::new();
        let mut coverage = String::from("weapon\teffect\toperator\n");
        for class in [
            "pistol",
            "pistol_dual",
            "pistol_magnum",
            "smg",
            "smg_silenced",
            "smg_mp5",
            "rifle",
            "rifle_ak47",
            "rifle_desert",
            "rifle_sg552",
            "pumpshotgun",
            "shotgun_chrome",
            "autoshotgun",
            "shotgun_spas",
            "hunting_rifle",
            "sniper_military",
            "sniper_awp",
            "sniper_scout",
            "rifle_m60",
            "grenade_launcher",
        ] {
            let weapon =
                crate::game_state::Weapon::load(&mut assets, &format!("weapon_{class}")).unwrap();
            if let Some(index) = elements.iter().position(|e| {
                e.kind == "DmeParticleSystemDefinition" && e.name == weapon.muzzle_effect
            }) {
                let mut visited = vec![index];
                let mut seen = HashSet::new();
                while let Some(index) = visited.pop() {
                    if !seen.insert(index) {
                        continue;
                    }
                    let node = &elements[index];
                    for key in ["initializers", "operators", "emitters", "renderers"] {
                        for operator in node.references(key) {
                            coverage.push_str(&format!(
                                "{class}\t{}\t{}\n",
                                node.name,
                                elements[operator].text("functionName")
                            ));
                        }
                    }
                    for child in node.references("children") {
                        let index = elements[child].number("child", -1.);
                        if index >= 0. {
                            visited.push(index as usize);
                        }
                    }
                }
            } else {
                coverage.push_str(&format!(
                    "{class}\t{}\tdefinition not in weapon_fx.pcf\n",
                    weapon.muzzle_effect
                ));
            }
        }
        std::fs::write("local-research/parity/muzzle-weapon-coverage.tsv", coverage).unwrap();
        for name in [
            "particle/vistasmokev1/vistasmokev4_nearcull",
            "particle/smoke1/smoke1",
            "particle/particle_spark",
            "particle/fire_particle_4/fire_particle_4",
        ] {
            if let Ok(vmt) = assets.read(&crate::material::material_path(name)) {
                report.push_str(&format!(
                    "NATIVE MATERIAL {name}\n{}\n",
                    String::from_utf8_lossy(&vmt)
                ));
            }
        }
        if let Ok(vmt) = assets.read("materials/particle/particle_muzzleflash4.vmt") {
            report.push_str(&format!(
                "NATIVE MUZZLE MATERIAL\n{}\n",
                String::from_utf8_lossy(&vmt)
            ));
        }
        for e in elements
            .iter()
            .filter(|e| e.name.contains("muzzle_flash") && e.kind == "DmeParticleSystemDefinition")
        {
            report.push_str(&format!("{e:#?}\n"));
            for key in ["initializers", "operators", "renderers"] {
                for id in e.references(key) {
                    report.push_str(&format!("{:#?}\n", elements[id]));
                }
            }
        }
        std::fs::write("local-research/parity/native-muzzle-operators.txt", report).unwrap();
    }
    #[test]
    fn live_particle_bounds_include_rotated_billboards_trails_and_camera_edges() {
        let mut particles = vec![Particle {
            rendered_frames: 0,
            previous_position: Vec3::new(-1., 0., -5.),
            normal: Vec3::Y,
            fade: [0., 0.],
            position: Vec3::new(0., 0., -5.),
            velocity: Vec3::X,
            age: 1.,
            life: 5.,
            radius: 1.,
            alpha: 1.,
            color: Vec3::ONE,
            sequence: 0,
            second_sequence: 0,
            kill_at: f32::INFINITY,
            flipped: false,
            trail_time: 0.1,
            rotation: 0.7,
        }];
        let bound = particle_bounds(&particles, [0.5, 2.], 3.).unwrap();
        let pad = 2. * std::f32::consts::SQRT_2 + 3.;
        assert!((bound.half_extents.x - pad).abs() < 1e-5);
        let projection = Projection::Perspective(PerspectiveProjection {
            fov: 60f32.to_radians(),
            aspect_ratio: 1.,
            far: 100.,
            ..default()
        });
        let frustum = projection.compute_frustum(&GlobalTransform::IDENTITY);
        assert!(
            frustum.intersects_obb_identity(&particle_bounds(&particles, [1., 1.], 0.).unwrap())
        );
        particles[0].position = Vec3::new(20., 0., -5.);
        assert!(
            !frustum.intersects_obb_identity(&particle_bounds(&particles, [1., 1.], 0.).unwrap())
        );
        particles[0].position = Vec3::new(3.5, 0., -5.);
        assert!(
            frustum.intersects_obb_identity(&particle_bounds(&particles, [1., 1.], 0.).unwrap()),
            "Effects crossing a view edge must remain visible"
        );
        particles[0].position = Vec3::new(0., 0., -5.);
        assert!(
            frustum.intersects_obb_identity(&particle_bounds(&particles, [1., 1.], 0.).unwrap()),
            "Returning particles must reappear without stale bounds"
        );
        particles[0].position.x = f32::NAN;
        assert!(
            particle_bounds(&particles, [1., 1.], 0.).is_none(),
            "Invalid bounds must fail open"
        );
    }
    #[test]
    fn compact_billboard_basis_matches_the_authored_camera_rotation() {
        for camera in [
            Quat::IDENTITY,
            Quat::from_euler(EulerRot::YXZ, 1.2, -0.4, 0.3),
        ] {
            for angle in [-3., -1., 0., 0.7, 2.6] {
                let (right, up) = billboard_axes(camera * Vec3::X, camera * Vec3::Y, angle, 0.3);
                let q = camera * Quat::from_rotation_z(angle);
                assert!(right.distance(q * Vec3::X * 0.3) < 0.00001);
                assert!(up.distance(q * Vec3::Y * 0.3) < 0.00001);
            }
        }
    }
    #[test]
    fn authored_instant_emitter_is_bounded_and_not_a_continuous_fallback() {
        use crate::source_pcf::Value;
        let root = Element {
            name: "burst".into(),
            kind: "DmeParticleSystemDefinition".into(),
            attributes: std::collections::BTreeMap::from([
                ("material".into(), Value::Text("blood".into())),
                ("renderers".into(), Value::Array(vec![Value::Int(2)])),
                ("emitters".into(), Value::Array(vec![Value::Int(1)])),
            ]),
        };
        let emitter = Element {
            name: "emitter".into(),
            kind: String::new(),
            attributes: std::collections::BTreeMap::from([
                (
                    "functionName".into(),
                    Value::Text("emit_instantaneously".into()),
                ),
                ("num_to_emit".into(), Value::Int(8)),
            ]),
        };
        let renderer = Element {
            name: "renderer".into(),
            kind: String::new(),
            attributes: std::collections::BTreeMap::from([(
                "functionName".into(),
                Value::Text("render_animated_sprites".into()),
            )]),
        };
        let d = definition(&[root, emitter, renderer], 0).unwrap();
        assert_eq!(d.burst, 8);
        assert_eq!(d.rate, 0.);
    }
    #[test]
    fn owned_pipe_drips_use_small_trails_and_the_authored_cull_plane() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let nodes =
            source_pcf::parse(&assets.read("particles/environmental_fx.pcf").unwrap()).unwrap();
        let index = nodes
            .iter()
            .position(|n| n.name == "pipe_drips_h_2")
            .unwrap();
        let d = definition(&nodes, index).unwrap();
        assert_eq!(d.radius, [1.; 2]);
        assert!((d.trail - 12. * 0.0254).abs() < 1e-5);
        assert_eq!(d.cull_plane, Some(-0.0254));
    }
    #[test]
    fn installed_fire_definitions_and_glass_waves_decode() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let mut definitions = 0;
        let mut drawable = 0;
        for path in ["particles/fire_01.pcf", "particles/fire_01l4d.pcf"] {
            let nodes = source_pcf::parse(&assets.read(path).unwrap()).unwrap();
            for (id, node) in nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.kind == "DmeParticleSystemDefinition")
            {
                definitions += 1;
                let mut draws = vec![];
                flatten(&nodes, id, &mut HashSet::new(), &mut draws);
                for d in draws {
                    if d.renderers.is_empty() {
                        continue;
                    }
                    let material = match crate::material::load(&mut assets, &d.material) {
                        Ok(m) => m,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            eprintln!("Unused retail particle reference missing: {}", d.material);
                            continue;
                        }
                        Err(e) => panic!("{}: {e}", d.material),
                    };
                    if let Some(path) = material.base {
                        let bytes = assets.read(&path).unwrap();
                        crate::vtf::decode(&bytes, 512).unwrap();
                        let sheet = source_pcf::sheet(&bytes).unwrap();
                        for sequence in sheet {
                            for (_, uv) in sequence.frames {
                                assert!(uv.iter().all(|v| v.is_finite()));
                            }
                        }
                    }
                    drawable += 1;
                }
                assert!(!node.name.is_empty());
            }
        }
        assert!(definitions > 50 && drawable > 50);
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        let glass = &registry.definitions["breakable.glass"];
        assert_eq!(glass.waves.len(), 3);
        for path in &glass.waves {
            let wave = assets.read(path).unwrap();
            assert_eq!(&wave[..4], b"RIFF");
        }
        println!(
            "Native asset validation: {definitions} fire definitions, {drawable} child sprite emitters, three retail glass waves"
        );
    }
    #[test]
    fn sheet_uses_authored_frame_durations_and_clamping() {
        let sheet = vec![SheetSequence {
            id: 7,
            looping: false,
            duration: 3.,
            frames: vec![(1., [0., 0., 0.5, 1.]), (2., [0.5, 0., 1., 1.])],
        }];
        assert_eq!(sheet_uv(&sheet, 7, 0.5, 1.), [0., 0., 0.5, 1.]);
        assert_eq!(sheet_uv(&sheet, 7, 1.1, 1.), [0.5, 0., 1., 1.]);
        assert_eq!(sheet_uv(&sheet, 7, 100., 1.), [0.5, 0., 1., 1.]);
        let (current, next, blend) = sheet_frames(&sheet, 7, 0.5);
        assert_eq!(current, sheet[0].frames[0].1);
        assert_eq!(next, sheet[0].frames[1].1);
        assert_eq!(blend, 0.5);
        assert_eq!(sheet_frames(&sheet, 7, 100.).2, 0.);
        let mut looping = sheet;
        looping[0].looping = true;
        assert_eq!(sheet_frames(&looping, 7, 2.).1, looping[0].frames[0].1);
        assert_eq!(sheet_frames(&looping, 7, 3.5).2, 0.5);
    }
    #[test]
    fn secondary_sequence_uses_its_own_range_and_clock() {
        use crate::source_pcf::Value;
        let node = Element {
            name: "smoke".into(),
            kind: "DmeParticleSystemDefinition".into(),
            attributes: std::collections::BTreeMap::from([
                ("material".into(), Value::Text("smoke".into())),
                ("initializers".into(), Value::Array(vec![Value::Int(1)])),
                ("renderers".into(), Value::Array(vec![Value::Int(2)])),
            ]),
        };
        let seq = Element {
            name: "sequence".into(),
            kind: String::new(),
            attributes: std::collections::BTreeMap::from([
                (
                    "functionName".into(),
                    Value::Text("Sequence Two Random".into()),
                ),
                ("sequence_min".into(), Value::Int(5)),
                ("sequence_max".into(), Value::Int(12)),
            ]),
        };
        let render = Element {
            name: "sprites".into(),
            kind: String::new(),
            attributes: std::collections::BTreeMap::from([
                (
                    "functionName".into(),
                    Value::Text("render_animated_sprites".into()),
                ),
                ("animation rate".into(), Value::Float(1.)),
                ("second sequence animation rate".into(), Value::Float(0.15)),
            ]),
        };
        let d = definition(&[node, seq, render], 0).unwrap();
        assert_eq!(d.second_sequences, [5, 12]);
        assert_eq!(d.sequences, [0, 0]);
        assert_eq!(d.renderers[0].second_fps, 0.15);
        assert_eq!(
            sprite_phase(1., 2., 10., 10, d.renderers[0].second_fps, false, false),
            1.5
        );
    }
    #[test]
    fn secondary_zoom_remains_inside_its_atlas_rectangle() {
        let rect = [0.25, 0.5, 0.5, 0.75];
        for corner in [[0., 0.], [1., 0.], [1., 1.], [0., 1.]] {
            let uv = atlas_corner(rect, corner, 1. / 24.);
            assert!((rect[0]..=rect[2]).contains(&uv[0]));
            assert!((rect[1]..=rect[3]).contains(&uv[1]));
        }
        assert_eq!(atlas_corner(rect, [0., 0.], 1.), [rect[0], rect[1]]);
    }
    #[test]
    fn sprite_size_limits_and_near_fade_match_authored_ranges() {
        let sizes = Vec4::new(0., 20., 0.4, 0.7);
        assert_eq!(sprite_extent(0.4, 2., sizes), (0.4, 1.));
        assert!((sprite_extent(1., 2., sizes).1 - 2. / 3.).abs() < 1e-6);
        assert_eq!(sprite_extent(1.4, 2., sizes).1, 0.);
        assert_eq!(sprite_extent(0.5, 10., Vec4::new(0.1, 20., 10., 20.)).0, 1.);
        assert_eq!(sprite_extent(3., 2., Vec4::new(0., 0.85, 10., 20.)).0, 1.7);
    }
    #[test]
    fn default_random_fade_preserves_brief_muzzle_brightness() {
        use crate::source_pcf::Value;
        let root = Element {
            name: "flash".into(),
            kind: "DmeParticleSystemDefinition".into(),
            attributes: std::collections::BTreeMap::from([
                ("material".into(), Value::Text("flash".into())),
                ("operators".into(), Value::Array(vec![Value::Int(1)])),
                ("renderers".into(), Value::Array(vec![Value::Int(2)])),
            ]),
        };
        let mut fade = Element {
            name: "fade".into(),
            kind: String::new(),
            attributes: std::collections::BTreeMap::from([
                (
                    "functionName".into(),
                    Value::Text("Alpha Fade Out Random".into()),
                ),
                ("fade out time min".into(), Value::Float(0.3)),
                ("fade out time max".into(), Value::Float(0.3)),
            ]),
        };
        let render = Element {
            name: "render".into(),
            kind: String::new(),
            attributes: std::collections::BTreeMap::from([(
                "functionName".into(),
                Value::Text("render_animated_sprites".into()),
            )]),
        };
        let d = definition(&[root.clone(), fade.clone(), render.clone()], 0).unwrap();
        assert!(d.fade_random[1].unwrap().1);
        assert_eq!(
            alpha_envelope(0., 0.02, [0., 0.02 * 0.3], false, d.fade_out_style),
            1.
        );
        assert!(
            (alpha_envelope(0.017, 0.02, [0., 0.006], false, d.fade_out_style) - 0.5).abs() < 1e-5
        );
        fade.attributes
            .insert("proportional 0/1".into(), Value::Bool(false));
        assert!(
            !definition(&[root, fade, render], 0).unwrap().fade_random[1]
                .unwrap()
                .1
        );
    }
    #[test]
    fn birth_subframe_ages_show_brief_fading_particles_without_killing_them() {
        for index in 0..13 {
            let age = birth_age(0.0075, index, 13);
            assert!(age > 0. && age <= 0.0075 && age < 0.015);
            assert!(
                alpha_envelope(
                    age,
                    0.015,
                    [0.015 * 0.25, 0.015 * 0.3],
                    true,
                    Some((true, 0.5))
                ) > 0.
            );
        }
    }
    #[test]
    fn owned_velocity_noise_has_independent_repeatable_channels() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let noise = crate::source_noise::Noise::load(root).unwrap();
        let op = VelocityNoise {
            output: [Vec3::new(50., -40., -40.), Vec3::new(120., 40., 20.)],
            spatial: 2.,
            time: 1.,
            offset: Vec3::ZERO,
            time_offset: 0.,
            absolute: Vec3::ZERO,
            invert: Vec3::ZERO,
            local: true,
        };
        let a = coherent_velocity(&noise, &op, Vec3::new(554.4, 5636.4, 2849.8), 0.01);
        assert_eq!(
            a,
            coherent_velocity(&noise, &op, Vec3::new(554.4, 5636.4, 2849.8), 0.01)
        );
        for i in 0..3 {
            assert!((op.output[0][i]..=op.output[1][i]).contains(&a[i]));
        }
        let fraction = (a - op.output[0]) / (op.output[1] - op.output[0]);
        assert!(
            fraction.x != fraction.y || fraction.y != fraction.z,
            "Noise must not reduce to one correlated random scalar"
        );
    }
}
