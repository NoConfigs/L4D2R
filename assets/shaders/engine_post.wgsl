// Retail L4D2 engine_post (engine_post.rs), from the owned shaders
// (hl2/shaders/fxc): downsample_nohdr_ps20b, blurfilter_ps20b (kernel 4)
// and engine_post_ps20b (local contrast + colour correction + vignette).
// Everything is computed on the gamma-space frame, as in retail.
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;

const LUMA: vec3<f32> = vec3<f32>(0.299, 0.587, 0.114);

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3<f32>(0.0031308));
}
fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(pow((x + 0.055) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045));
}

// downsample_nohdr_ps20b, FLOAT_BACK_BUFFER: four bilinear taps covering the
// 4x4 source block, each converted to gamma; rgb = avg^2.2 * (r+g+b) (the
// shader's fixed params 1,1,1,2.2), alpha = luminance.
@fragment
fn downsample(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(source));
    let quarter = floor(in.uv * size / 4.0);
    let base = quarter * 4.0;
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        let o = vec2<f32>(f32(i % 2) * 2.0 + 1.0, f32(i / 2) * 2.0 + 1.0);
        sum += srgb_encode(textureSampleLevel(source, linear_sampler, (base + o) / size, 0.0).rgb);
    }
    let avg = sum * 0.25;
    let bloom = pow(avg, vec3<f32>(2.2)) * dot(avg, vec3<f32>(1.0));
    return vec4<f32>(bloom, dot(avg, LUMA));
}

// blurfilter_ps20b kernel 4: 13 taps (offsets in texels), each saturated.
const OFFSETS: array<f32, 6> = array<f32, 6>(1.3366, 3.4295, 5.4264, 7.4359, 9.4436, 11.4401);
const WEIGHTS: array<f32, 6> = array<f32, 6>(0.2185, 0.0821, 0.0461, 0.0262, 0.0162, 0.0102);
fn blur(uv: vec2<f32>, axis: vec2<f32>) -> vec4<f32> {
    let texel = axis / vec2<f32>(textureDimensions(source));
    var c = clamp(textureSampleLevel(source, linear_sampler, uv, 0.0), vec4<f32>(0.0), vec4<f32>(1.0)) * 0.2013;
    for (var i = 0; i < 6; i++) {
        let d = texel * OFFSETS[i];
        let a = clamp(textureSampleLevel(source, linear_sampler, uv + d, 0.0), vec4<f32>(0.0), vec4<f32>(1.0));
        let b = clamp(textureSampleLevel(source, linear_sampler, uv - d, 0.0), vec4<f32>(0.0), vec4<f32>(1.0));
        c += (a + b) * WEIGHTS[i];
    }
    return c;
}
@fragment
fn blur_x(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return blur(in.uv, vec2<f32>(1.0, 0.0));
}
@fragment
fn blur_y(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return blur(in.uv, vec2<f32>(0.0, 1.0));
}

struct EnginePost {
    // centre local contrast, edge local contrast, vignette start, vignette end
    contrast: vec4<f32>,
    // top vignette scale, fade to black, colour correction weight, vignette on
    extra: vec4<f32>,
    // grain strength, time
    grain: vec4<f32>,
    // blurred vignette scale
    blur: vec4<f32>,
    // view fade colour (gamma) and alpha
    fade: vec4<f32>,
}
@group(0) @binding(2) var blurred: texture_2d<f32>;
@group(0) @binding(3) var vignette: texture_2d<f32>;
@group(0) @binding(4) var lookup: texture_3d<f32>;
@group(0) @binding(5) var<uniform> post: EnginePost;
@group(0) @binding(6) var noise: texture_2d<f32>;
@group(0) @binding(7) var repeat_sampler: sampler;

// engine_post_ps20b with LOCAL_CONTRAST_ENABLE, COL_CORRECT_NUM_LOOKUPS 1
// and VIGNETTE_ENABLE (bloom amount is 0 in L4D2: 101c57c0).
@fragment
fn combine(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let src = textureSampleLevel(source, linear_sampler, in.uv, 0.0);
    var col = srgb_encode(src.rgb);
    // Local contrast, faded from the centre value to the edge value between
    // the vignette radii (smoothstep), never below -1 when negative.
    let p = in.uv * 2.0 - 1.0;
    let v = clamp((length(p) - post.contrast.z) / (post.contrast.w - post.contrast.z), 0.0, 1.0);
    let s = v * v * (3.0 - 2.0 * v);
    let lc = mix(post.contrast.x, post.contrast.y, s);
    let floor_value = select(-1.0, 0.0, lc >= 0.0);
    let local = textureSampleLevel(blurred, linear_sampler, in.uv, 0.0).a;
    col = col + max(lc * (dot(col, LUMA) - local), floor_value);
    // BLURRED_VIGNETTE_ENABLE: toward the blurred grey luminance by the
    // same radial ramp x $blurredVignetteScale.
    if (post.blur.x > 0.0) {
        col = mix(col, vec3<f32>(local), s * post.blur.x);
    }
    // View fade (g_vViewFadeColor, alpha blend; bloom amount is 0).
    col = mix(col, post.fade.rgb, post.fade.a);
    // Colour correction: weight x lookup + (1 - weight) x colour.
    if (post.extra.z > 0.0) {
        let graded = textureSampleLevel(lookup, linear_sampler, col * 0.96875 + 0.015625, 0.0).rgb;
        col = graded * post.extra.z + col * (1.0 - post.extra.z);
    }
    // Top vignette (dev/vignette, red channel).
    if (post.extra.w > 0.0) {
        let t = textureSampleLevel(vignette, linear_sampler, in.uv * 0.96875 + 0.015625, 0.0).r;
        col = col * (post.extra.x * (clamp(t * 0.55 + 0.46, 0.0, 1.0) - 1.0) + 1.0);
    }
    // Film grain (NOISE_ENABLE, frames at least 720 high): two dependent
    // noise_post lookups, green * 0.3 - 0.15, strongest in the dark.
    if (post.grain.x > 0.0 && textureDimensions(source).y >= 720u) {
        let t = post.grain.y;
        let a = textureSampleLevel(noise, repeat_sampler, in.uv * 10.0 + t, 0.0);
        let b = textureSampleLevel(noise, repeat_sampler, vec2<f32>(in.uv.y * 20.0 - t, in.uv.x * 20.0 - t), 0.0);
        let n = textureSampleLevel(noise, repeat_sampler, vec2<f32>(a.y, b.y), 0.0).y;
        let g = (n * 0.3 - 0.15) * post.grain.x;
        let l = clamp(dot(col, vec3<f32>(0.2125, 0.7154, 0.0721)), 0.0, 1.0);
        let m = clamp(pow(1.0 - l, 12.0), 0.0, 1.0) * 0.8 + 0.2;
        col = col + g * m;
    }
    col = col - post.extra.y * col;
    return vec4<f32>(srgb_decode(col), src.a);
}
