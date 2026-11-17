//! Room DSP: Source `dsp_room` presets (scripts/dsp_presets.txt) and the L4D
//! automatic room system (ADSP) that `"dsp" "1"` soundscapes select.
//!
//! Recovered from engine.dll:
//! * room 1 is ADSP (10026c60 / 100150b0); dsp_automatic is 0, so without
//!   ADSP room 1 would be silent pass-through (10026c70).
//! * the probe (10012a80 / 10012770): 20 rays from the listener (table in
//!   1000df00), surface `audioreflectivity`, sky detection, and for the
//!   rising rays the ceiling height above the floor under each hit.
//! * the analysis (1000e270): width/length from the better horizontal axis
//!   pair, height from the rising rays, wall/ceiling/floor reflectivity and
//!   the outdoor flag (any rising ray reaching sky).
//! * the classification (10026e70) into room, duct, hall, tunnel, street,
//!   alley, courtyard, open space/wall/street/courtyard, the size buckets
//!   (100229e0), and the preset synthesis (10027160 / 10022f70): the class'
//!   adsp_*_min preset and the next one are blended per parameter by the
//!   size bucket / 5, with room-dimension values for negative parameters.
//! * mixing: the preset's mix range by source distance over
//!   dsp_dist_min/max (0..1440), reduced by `mixdrop` for sounds quieter than
//!   `db_min`, wet level dsp_volume 0.8.
//!
//! Not recovered: the engine's processor implementations. The processors
//! below (feedback combs, allpass diffusors, filtered delays, RBJ filters)
//! follow the documented parameter meanings, not the engine's exact code;
//! modulators (LFO, EFO, ENV, CRS, PTC) pass audio through. The effect is
//! applied per sound as it starts (Bevy has no master bus), so a sound keeps
//! the room it started in; looping sounds are left dry.
use bevy::audio::{AudioSource, ChannelCount, Decodable, Sample, SampleRate, Source};
use bevy::prelude::*;
use std::{collections::HashMap, sync::Arc};

pub const DLY: u8 = 1;
pub const RVA: u8 = 2;
pub const FLT: u8 = 3;
pub const CRS: u8 = 4;
pub const PTC: u8 = 5;
pub const ENV: u8 = 6;
pub const LFO: u8 = 7;
pub const EFO: u8 = 8;
pub const MDY: u8 = 9;
pub const DFR: u8 = 10;
pub const AMP: u8 = 11;
/// dsp_dist_min / dsp_dist_max / dsp_volume.
const DIST_MIN: f32 = 0.;
const DIST_MAX: f32 = 1440.;
const DSP_VOLUME: f32 = 0.8;

#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub kind: u8,
    pub p: [f32; 16],
}
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub mix: [f32; 2],
    pub db_min: f32,
    pub mixdrop: f32,
    pub procs: Vec<Proc>,
}

fn kind(name: &str) -> Option<u8> {
    Some(match name {
        "DLY" => DLY,
        "RVA" => RVA,
        "FLT" => FLT,
        "CRS" => CRS,
        "PTC" => PTC,
        "ENV" => ENV,
        "LFO" => LFO,
        "EFO" => EFO,
        "MDY" => MDY,
        "DFR" => DFR,
        "AMP" => AMP,
        _ => return None,
    })
}
/// A preset parameter: a number or one of the file's named constants.
fn param(token: &str) -> f32 {
    token.parse().unwrap_or(match token {
        "HP" | "MED" | "ALLPASS" | "TRI" | "EXP" => 1.,
        "BP" | "HI" | "LOWPASS" | "SQR" => 2.,
        "VHI" | "DLINEAR" | "SAW" => 3.,
        "FLINEAR" | "RND" => 4.,
        "LOWPASS_4TAP" | "LOG_IN" => 5.,
        "PLAIN_4TAP" | "LOG_OUT" => 6.,
        "LIN_IN" => 7.,
        "LIN_OUT" => 8.,
        _ => 0., // LP, LO, PLAIN, SIN, LIN
    })
}
/// scripts/dsp_presets.txt by preset number.
pub fn parse_presets(text: &str) -> HashMap<usize, Preset> {
    let clean: String = text.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    let spaced = clean.replace('{', " { ").replace('}', " } ");
    let t: Vec<&str> = spaced.split_whitespace().collect();
    let mut out = HashMap::new();
    let mut i = 0;
    while i < t.len() {
        if t[i] != "{" {
            i += 1;
            continue;
        }
        let Some(number) = t.get(i + 1).and_then(|n| n.parse::<usize>().ok()) else {
            i += 1;
            continue;
        };
        i += 3; // "{", number, configuration type
        let mut header = vec![];
        while i < t.len() && t[i] != "{" && t[i] != "}" {
            header.push(param(t[i]));
            i += 1;
        }
        let mut procs = vec![];
        while i < t.len() && t[i] == "{" {
            i += 1;
            let name = t.get(i).copied().unwrap_or("");
            i += 1;
            let mut p = [0.; 16];
            let mut k = 0;
            while i < t.len() && t[i] != "}" {
                if k < 16 {
                    p[k] = param(t[i]);
                }
                k += 1;
                i += 1;
            }
            i += 1;
            if let Some(kind) = kind(name) {
                procs.push(Proc { kind, p });
            }
        }
        if i < t.len() && t[i] == "}" {
            i += 1;
        }
        let h = |k: usize, d: f32| header.get(k).copied().unwrap_or(d);
        out.insert(number, Preset { mix: [h(0, 0.2), h(1, 0.7)], db_min: h(4, 80.), mixdrop: h(5, 0.5), procs });
    }
    out
}

/// ADSP ray offsets (1000df00), Source units.
pub const RAYS: [[f32; 3]; 20] = [
    [4800., 0., 0.],
    [-4800., 0., 0.],
    [0., 4800., 0.],
    [0., -4800., 0.],
    [-2496., 2304., 0.],
    [2496., -2304., 0.],
    [2496., 2304., 0.],
    [-2496., -2304., 0.],
    [2496., 2304., 1248.],
    [2496., 2304., 2496.],
    [1248., 1152., 2496.],
    [-2496., -2304., 1248.],
    [-2496., -2304., 2496.],
    [-1248., -1152., 2496.],
    [-2496., 0., 2496.],
    [0., 1248., 2496.],
    [0., -2496., 2496.],
    [1248., 0., 2496.],
    [0., 0., 4800.],
    [0., 0., -4800.],
];
/// One ray hit: fraction, end point, sky surface, surface reflectivity.
pub struct Hit {
    pub fraction: f32,
    pub end: [f32; 3],
    pub sky: bool,
    pub reflectivity: f32,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rays {
    pub dist: [i32; 20],
    pub refl: [f32; 20],
    pub sky: [f32; 20],
}
/// adsp_low_ceiling / adsp_door_height / adsp_wall_height.
const LOW_CEILING: i32 = 108;
const DOOR_HEIGHT: i32 = 112;
const WALL_HEIGHT: i32 = 128;
/// The ADSP probe (100123b0 / 1000eb50 / 10012a80 / 10012770).
/// `ray(start, end, props)` returns the first hit, or None when clear;
/// `props` false is the world-only filter with CONTENTS_SOLID, true adds
/// props and entities with mask 0x4003. None when the listener is more than
/// 72 units above the floor (the engine skips the measurement).
pub fn probe(listener: [f32; 3], mut ray: impl FnMut([f32; 3], [f32; 3], bool) -> Option<Hit>) -> Option<Rays> {
    let add = |a: [f32; 3], b: [f32; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    // Horizontal rays run at wall height (129 over the floor) under a solid
    // ceiling: above railings and furniture, below lintels.
    let down = ray(listener, add(listener, [0., 0., -4800.]), true);
    if down.as_ref().is_some_and(|d| d.fraction * 4800. > 72.) {
        return None;
    }
    let mut level = listener;
    if let Some(up) = ray(listener, add(listener, [0., 0., 4800.]), true).filter(|u| !u.sky) {
        let below = down.as_ref().map_or(1., |d| d.fraction);
        let height = (up.fraction * 4800.) as i32 + (below * 4800.) as i32;
        let floor = down.as_ref().map_or(listener[2], |d| d.end[2].min(listener[2]));
        level[2] = if height > LOW_CEILING && height > WALL_HEIGHT {
            (WALL_HEIGHT + 1) as f32 + floor
        } else if height > LOW_CEILING && height > DOOR_HEIGHT {
            (DOOR_HEIGHT + 1) as f32 + floor
        } else {
            (height - 1) as f32 + floor
        };
    }
    let mut out = Rays::default();
    for (i, offset) in RAYS.iter().enumerate() {
        let origin = if i < 8 { level } else { listener };
        let end = add(origin, *offset);
        let mut hit = ray(origin, end, false);
        // A clear or sky-bound horizontal ray is re-traced against props.
        if i < 8 && hit.as_ref().is_none_or(|h| h.sky) {
            hit = ray(origin, end, true);
        }
        let mut dist = 4800.;
        match hit {
            // Clear: a rising ray that climbs over 2400 units saw open sky.
            None => {
                if offset[2] > 2400. {
                    out.sky[i] = 0.1;
                }
            }
            Some(hit) if hit.sky => {
                out.sky[i] = 0.1;
                if i < 8 {
                    dist = 1.;
                }
            }
            Some(hit) => {
                out.refl[i] = hit.reflectivity.clamp(0., 1.);
                let length = (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt();
                dist = hit.fraction * length;
                // Rising rays measure the ceiling height over the floor below.
                if (8..19).contains(&i) {
                    let top = if origin[2] <= hit.end[2] { hit.end } else { origin };
                    if let Some(floor) = ray(top, [top[0], top[1], top[2] - 4800.], true) {
                        dist = floor.fraction * 4800.;
                    }
                }
            }
        }
        out.dist[i] = dist as i32;
    }
    Some(out)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Room {
    pub width: i32,
    pub length: i32,
    pub height: i32,
    /// Reflectivity of the four walls of the chosen axis pair.
    pub walls: [f32; 4],
    pub reflectivity: f32,
    pub diffusion: f32,
    pub outside: bool,
}
/// 1000e270.
pub fn analyse(r: &Rays) -> Room {
    let axis: [i32; 4] = std::array::from_fn(|k| r.dist[2 * k] + r.dist[2 * k + 1]);
    let mut a = axis[0] * axis[1];
    if a == 0 {
        a = 1;
    }
    let b = if axis[2] * axis[3] != 0 { axis[2] * axis[3] } else { 1 };
    let ratio = if a as f32 <= b as f32 { a as f32 / b as f32 } else { b as f32 / a as f32 };
    let pair = if 1. - ratio <= 0.25 {
        // Similar areas: the axis pair holding the longest single axis.
        let mut best = 0;
        for k in 1..4 {
            if axis[k] > axis[best] {
                best = k;
            }
        }
        if best < 2 { 0 } else { 2 }
    } else if a as f32 <= b as f32 {
        2
    } else {
        0
    };
    let width = axis[pair].min(axis[pair + 1]);
    let length = axis[pair].max(axis[pair + 1]);
    let height = r.dist[8..19].iter().copied().max().unwrap_or(0).max(0);
    let walls: [f32; 4] = std::array::from_fn(|k| r.refl[pair * 2 + k]);
    // Ceiling: the last rising ray's reflectivity, 0 if any rising ray had none.
    let mut ceiling = 0.;
    for i in 8..19 {
        if r.refl[i] == 0. {
            ceiling = 0.;
            break;
        }
        ceiling = r.refl[i];
    }
    let reflectivity = (walls.iter().sum::<f32>() + ceiling + r.refl[19]) / 6.;
    Room {
        width,
        length,
        height,
        walls,
        reflectivity,
        diffusion: 0.,
        outside: r.sky[8..19].iter().any(|s| *s > 0.),
    }
}

fn bucket(v: f32, thresholds: &[f32]) -> usize {
    thresholds.iter().filter(|t| v > **t).count()
}
/// Class (10026e70) and size buckets (100229e0): (class, size, length,
/// width, height, reflectivity, diffusion buckets).
pub fn classify(room: &Room) -> (usize, [usize; 6]) {
    let (mut w, mut l, mut h) = (room.width as f32, room.length as f32, room.height as f32);
    let class = if room.outside {
        let open = room.walls.iter().filter(|r| **r == 0.).count();
        if open > 0 {
            11 - open
        } else if l <= 2.5 * w {
            6
        } else if w <= 144. {
            5
        } else {
            4
        }
    } else {
        // A vertical shaft is classified (and sized) on its height.
        if h > l * 3. {
            std::mem::swap(&mut l, &mut h);
        }
        if l > 4. * w && w <= 48. {
            1
        } else if l > 2.5 * w && w > 48. && w <= 96. {
            2
        } else if l > 4. * w && w > 96. {
            3
        } else {
            0
        }
    };
    let by_length = bucket(l, &[120., 240., 480., 1200.]);
    let length = bucket(l, &[144., 288., 576., 1152.]);
    let width = bucket(w, &[72., 144., 288., 576.]);
    let height = bucket(h, &[48., 128., 216., 384.]);
    let reflect = bucket(room.reflectivity, &[0.04, 0.5, 0.8]);
    let diffuse = bucket(room.diffusion, &[0.01, 0.1, 0.3]);
    let size = match class {
        6 | 10 => by_length,
        4 | 5 => width,
        _ => (length + width) / 2,
    };
    (class, [size, length, width, height, reflect, diffuse])
}
/// adsp_room_min .. adsp_opencourtyard_min defaults by class.
const CLASS_PRESET: [usize; 11] = [102, 106, 110, 114, 118, 122, 126, 130, 130, 118, 126];
fn nth(p: &Preset, kind: u8, n: usize) -> Option<usize> {
    p.procs.iter().enumerate().filter(|(_, x)| x.kind == kind).nth(n).map(|(i, _)| i)
}
/// 10022cc0: out.p[k] = a + (b - a) * f (f = num / den, squared when
/// `sq`) for the n-th processor of `kind` present in all three presets.
#[allow(clippy::too_many_arguments)]
fn lerp(out: &mut Preset, a: &Preset, b: &Preset, kind: u8, n: usize, k: usize, num: usize, den: f32, sq: bool) {
    let (Some(i), Some(ia), Some(ib)) = (nth(out, kind, n), nth(a, kind, n), nth(b, kind, n)) else { return };
    let mut f = num as f32 / den;
    if sq {
        f *= f;
    }
    let (x, y) = (a.procs[ia].p[k], b.procs[ib].p[k]);
    out.procs[i].p[k] = x + (y - x) * f;
}
/// 10022e30: a squared blend, or the room-sized `value` when either preset
/// leaves the parameter negative (automatic).
#[allow(clippy::too_many_arguments)]
fn auto(out: &mut Preset, a: &Preset, b: &Preset, kind: u8, n: usize, k: usize, num: usize, value: f32) {
    let (Some(i), Some(ia), Some(ib)) = (nth(out, kind, n), nth(a, kind, n), nth(b, kind, n)) else { return };
    if a.procs[ia].p[k] >= 0. && b.procs[ib].p[k] >= 0. {
        lerp(out, a, b, kind, n, k, num, 5., true);
    } else {
        out.procs[i].p[k] = value;
    }
}
/// The ADSP preset for a measured room (10027160 / 10022f70).
pub fn synthesize(presets: &HashMap<usize, Preset>, room: &Room) -> Option<Preset> {
    let (class, [size, length, width, height, reflect, diffuse]) = classify(room);
    let base = CLASS_PRESET[class] + if diffuse > 1 { 2 } else { 0 };
    let (a, b) = (presets.get(&base)?, presets.get(&(base + 1))?);
    let mut out = if size < 2 { a.clone() } else { b.clone() };
    let feet = |v: i32| (v as f32 / 12.).clamp(6., 500.);
    let echo = |v: i32| (v as f32 * 2. / 12.).clamp(14., 500.);
    let o = &mut out;
    for n in 0..2 {
        for k in 0..4 {
            lerp(o, a, b, DFR, n, k, size, 5., false);
        }
        for (k, sq) in [(3, false), (1, true), (0, true), (4, false), (2, false), (7, false), (8, false)] {
            lerp(o, a, b, RVA, n, k, size, 5., sq);
        }
        auto(o, a, b, RVA, n, 9, width, feet(room.width));
        auto(o, a, b, RVA, n, 10, length, feet(room.length));
        auto(o, a, b, RVA, n, 11, height, feet(room.height));
        for (k, bucket) in [(12, width), (13, length), (14, height)] {
            lerp(o, a, b, RVA, n, k, bucket, 5., false);
        }
        // Cutoff: brighter (toward the min preset) with reflective surfaces.
        lerp(o, b, a, RVA, n, 5, reflect, 4., false);
        auto(o, a, b, DLY, n, 1, length, echo(room.length));
        lerp(o, a, b, DLY, n, 2, length, 5., false);
        lerp(o, a, b, DLY, n, 3, length, 5., false);
        auto(o, a, b, DLY, n, 8, length, echo(room.width));
        auto(o, a, b, DLY, n, 9, length, echo(room.length));
        auto(o, a, b, DLY, n, 10, length, echo(room.length));
        lerp(o, b, a, DLY, n, 5, length, 5., false);
        lerp(o, b, a, DLY, n, 6, length, 5., false);
        for k in 0..13 {
            lerp(o, a, b, MDY, n, k, length, 5., false);
        }
        for (kind, count) in [(AMP, 8), (FLT, 5), (PTC, 3), (CRS, 4), (ENV, 9), (EFO, 4), (LFO, 4)] {
            for k in 0..count {
                lerp(o, a, b, kind, n, k, size, 5., false);
            }
        }
    }
    Some(out)
}
/// Wet mix for a sound `distance` units away at `soundlevel` dB.
pub fn mix(preset: &Preset, distance: f32, soundlevel: f32) -> f32 {
    let f = ((distance - DIST_MIN) / (DIST_MAX - DIST_MIN)).clamp(0., 1.);
    let mut mix = preset.mix[0] + (preset.mix[1] - preset.mix[0]) * f;
    if soundlevel < preset.db_min {
        mix *= 1. - preset.mixdrop;
    }
    mix.clamp(0., 1.)
}

// ---- processors ----
//
// Fixed-point ports of the engine.dll DSP processors (PBITS 12, PMAX 4096,
// 44.1 kHz). Processor table 100267f0: DLY 1001eeb0, RVA 10020050 (params
// 10025640 / 10025370, alloc 10024f60), FLT 1001e840 (design 1001e400 /
// 1001e4c0), DFR 10020260 (params 100259c0), MDY 1001f8b0 (alloc
// 1001f7c0). Samples run as 16-bit-range integers like the paint buffer.
// Not ported: 4-tap delays (no preset uses them; the one RVA with `ftaps`
// falls back to its untapped type), AMP distortion/modulation, the MDY
// processor's own modulation (RVA comb modulation is ported), and the
// modulators (LFO, EFO, ENV, CRS, PTC), which pass audio through.
const PMAX: i32 = 4096;
const SAMPLE_SCALE: f32 = 32768.;
fn mul(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b) >> 12
}
/// Delay line with Source's decrementing write pointer: `tap(t)` is the
/// sample written `t` samples ago (t <= D).
#[derive(Clone)]
struct Line {
    buf: Vec<i32>,
    p: usize,
}
impl Line {
    fn new(d: usize) -> Self {
        Self { buf: vec![0; d + 1], p: 0 }
    }
    fn d(&self) -> usize {
        self.buf.len() - 1
    }
    fn tap(&self, t: usize) -> i32 {
        self.buf[(self.p + t.min(self.d())) % self.buf.len()]
    }
    fn push(&mut self, v: i32) {
        self.buf[self.p] = v;
        self.p = if self.p == 0 { self.buf.len() - 1 } else { self.p - 1 };
    }
}
/// First-order section (1001da60): w = x - a1*w1; y = b0*w + b1*w1.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Iir {
    a1: i32,
    b0: i32,
    b1: i32,
    w: [i32; 2],
}
impl Iir {
    /// 3 dB first-order design (1001e400): Oc = tan(Wc / 2).
    fn design(cutoff: f32, hp: bool) -> Self {
        let wc = cutoff as f64 * 6.283185307179586 * 2.2675736961451248e-05;
        let oc = (wc * 0.5).tan();
        let c = (1. - oc) / (1. + oc);
        let g = if hp { (1. + c) * 0.5 } else { (1. - c) * 0.5 };
        let v = (g * 4096.) as i32;
        Self { a1: (c * -4096.) as i32, b0: v, b1: if hp { -v } else { v }, w: [0; 2] }
    }
    /// The b coefficients scaled as FUN_1001e340 does.
    fn scaled(self, gain: f32) -> Self {
        Self { b0: (self.b0 as f32 * gain) as i32, b1: (self.b1 as f32 * gain) as i32, ..self }
    }
    fn run(&mut self, x: i32) -> i32 {
        if self.w == [0, 0] && x == 0 {
            return 0;
        }
        let w1 = self.w[1];
        let w = x.wrapping_sub(mul(self.a1, w1));
        self.w = [w, w];
        self.b1.wrapping_mul(w1).wrapping_add(self.b0.wrapping_mul(w)) >> 12
    }
}
/// FLT sections (1001e4c0): type 0 LP / 1 HP; `qwidth` > 0 makes a band
/// pass (HP at cutoff, LP at cutoff + qwidth). Quality 0..3 adds 0..3
/// unscaled sections; only the first section carries the gain.
fn filter_stages(kind: f32, cutoff: f32, qwidth: f32, quality: f32, gain: f32) -> Vec<Iir> {
    let band = qwidth > 0.;
    let (first, rest) = if band {
        (Iir::design(cutoff, true), Iir::design(cutoff + qwidth, false))
    } else {
        let d = Iir::design(cutoff, kind == 1.);
        (d, d)
    };
    let mut cascade = (quality as i32).clamp(0, 3);
    if band && cascade < 2 {
        cascade = 1;
    }
    let mut stages = vec![first.scaled(gain)];
    match cascade {
        1 => stages.push(rest),
        2 => stages.extend([rest, rest]),
        3 => stages.extend([rest, rest, first]),
        _ => {}
    }
    stages
}
/// Delay types (DLY_PLAIN .. DLY_FLINEAR); the 4-tap types are not ported.
const PLAIN: u8 = 0;
const ALLPASS: u8 = 1;
const LOWPASS: u8 = 2;
const LINEAR: u8 = 3;
const FLINEAR: u8 = 4;
/// Linear ramp generator (1001f5b0 / 1001f670), 20-bit fractional steps.
#[derive(Clone, Default)]
struct Ramp {
    start: i32,
    value: i32,
    total: i32,
    step: i32,
    acc: i32,
    pos: i32,
    dir: i32,
    done: bool,
    finished: bool,
}
impl Ramp {
    fn new(seconds: f32, start: i32, end: i32) -> Self {
        let delta = end - start;
        let samples = ((seconds * 44100.) as i32).max(1);
        let mut f = (delta as f32).abs() / samples as f32;
        if f as i32 > 0xfff {
            f = (f - (f as i32) as f32) + 4095.;
        }
        Self {
            start,
            value: start,
            total: delta.abs(),
            step: (f * 1048576.) as i32,
            acc: 0,
            pos: 0,
            dir: if delta >= 0 { 1 } else { -1 },
            done: false,
            finished: false,
        }
    }
    fn next(&mut self) -> i32 {
        if self.finished {
            return self.value;
        }
        let old = self.pos;
        let mut at = old;
        if !self.done {
            let u = self.acc.wrapping_add(self.step);
            self.pos = (u >> 20) + old;
            self.acc = u & 0xfffff;
            at = self.pos;
            if self.step == 0 || at < 0 || self.total <= at {
                self.done = true;
                at = old;
            }
        }
        if self.done {
            self.finished = true;
        }
        if self.dir < 1 {
            at = -at;
        }
        self.value = self.start + at;
        self.value
    }
}
/// Random delay-length modulation of one comb (MDY 1001f8b0, set up by
/// 10024f60): every `interval` samples a new length in [(1 - depth) D, D]
/// is chosen and the read point crossfades to it over `ramp_seconds`.
/// Retail draws from the engine RandomInt; this uses its own generator.
#[derive(Clone)]
struct Modulation {
    interval: i32,
    counter: i32,
    depth: f32,
    ramp_seconds: f32,
    ramp: Ramp,
    active: bool,
    seed: u32,
}
#[derive(Clone)]
struct Dly {
    kind: u8,
    line: Line,
    a: i32,
    b: i32,
    filter: Iir,
    /// Read taps and the crossfade between them (+0xc, +0x10, +0x14).
    t0: usize,
    t1: usize,
    frac: i32,
    modulation: Option<Box<Modulation>>,
}
impl Dly {
    /// DLY_Alloc (10024b00), including the output-gain normalisation of a
    /// feedback line (1001ee00): gain x clamp(4 (1 - fb), 0.25, 1).
    fn new(d: usize, a: i32, b: i32, kind: u8, filter: Option<Iir>) -> Self {
        let filter = filter.map(|f| f.scaled(a as f32 * (1. / 4096.))).unwrap_or_default();
        let (a, b) = if matches!(kind, LINEAR | FLINEAR) {
            (0, PMAX)
        } else {
            let a = a.min(0xfff);
            let fb = (a as f32 * (1. / 4096.)).min(0.999);
            let b0 = if b == 0 { PMAX } else { b };
            let g = (((1. - fb as f64) * 4096.) as i32) as f32 * 4.;
            let g = if g > 4096. { 4096. } else if g < 1024. { 1024. } else { g };
            (a, (g as f64 * (b0 as f64 * (1. / 4096.))) as i32)
        };
        Self { kind, line: Line::new(d), a, b, filter, t0: d, t1: d, frac: 0, modulation: None }
    }
    fn read(&self) -> i32 {
        let d0 = self.line.tap(self.t0);
        if self.modulation.as_ref().is_some_and(|m| m.active) {
            let d1 = self.line.tap(self.t1);
            mul(d1.wrapping_sub(d0), self.frac).wrapping_add(d0)
        } else {
            d0
        }
    }
    fn step(&mut self, x: i32) -> i32 {
        let d = self.read();
        match self.kind {
            ALLPASS => {
                let w = mul(d, self.a).wrapping_add(x);
                self.line.push(w);
                mul((self.a.wrapping_mul(w).wrapping_neg() >> 12).wrapping_add(d), self.b)
            }
            LOWPASS => {
                let v = self.filter.run(d).wrapping_add(x);
                self.line.push(v);
                mul(v, self.b)
            }
            LINEAR => {
                self.line.push(x);
                d
            }
            FLINEAR => {
                let f = self.filter.run(d);
                self.line.push(x);
                mul(f, self.b)
            }
            _ => {
                let y = mul(d, self.a).wrapping_add(x);
                self.line.push(y);
                mul(y, self.b)
            }
        }
    }
    fn next(&mut self, x: i32) -> i32 {
        let Some(mut m) = self.modulation.take() else {
            return self.step(x);
        };
        let y = self.step(x);
        if m.active {
            self.frac = m.ramp.next();
            if m.ramp.finished {
                self.t0 = self.t1.min(self.line.d());
                m.active = false;
            }
        }
        if m.interval != 0 {
            let was = m.counter;
            m.counter -= 1;
            if was == 0 {
                m.counter = m.interval;
                let d = self.line.d() as i32;
                let lo = ((1. - m.depth) * d as f32) as i32;
                m.seed = m.seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let pick = lo + ((m.seed >> 8) % ((d - lo).max(0) as u32 + 1)) as i32;
                m.active = true;
                m.ramp = Ramp::new(m.ramp_seconds, 0, PMAX);
                self.t1 = pick.min(d) as usize;
                self.frac = 0;
            }
        }
        self.modulation = Some(m);
        y
    }
    fn tail_seconds(&self) -> f32 {
        let fb = (self.a as f32 / 4096.).abs().clamp(0.01, 0.97);
        self.line.d() as f32 / 44100. * (0.001f32.ln() / fb.ln())
    }
}
enum Stage {
    /// Series allpass diffusor (DFR).
    Diffuse(Vec<Dly>),
    /// Comb bank (RVA): outputs summed, then an optional filter.
    Reverb { combs: Vec<Dly>, filter: Vec<Iir> },
    /// Single delay line (DLY / MDY).
    Echo(Dly),
    Filter(Vec<Iir>),
    Gain(i32),
}
/// DFR delay lengths in ms (10423ab0); the feedback scales (10423af0) are 1.
const DFR_MS: [f32; 16] = [13., 19., 26., 21., 32., 36., 38., 16., 24., 28., 41., 35., 10., 46., 50., 27.];
fn diffusor(v: &[f32; 16]) -> Stage {
    let size = v[0];
    let n = v[1].clamp(1., 8.) as usize;
    let gain = if v[3] == 0. { 1. } else { v[3] };
    let lines = (0..n)
        .map(|i| {
            let d = (((DFR_MS[i] * size) as i32) * 44100 / 1000).max(1) as usize;
            let a = ((v[2] as f64 * 4096.).min(4091.904)) as i32;
            Dly::new(d, a, (gain * 4096.) as i32, ALLPASS, None)
        })
        .collect();
    Stage::Diffuse(lines)
}
/// RVA comb lengths, feedback and gain, from the size range (10025640) or
/// the room dimensions (10025370), then the comb bank (10024f60).
fn reverb(v: &[f32; 16]) -> Stage {
    let (size_max, size_min, feedback, gain, cutoff, parallel) = (v[0], v[1], v[3], v[4], v[5], v[6]);
    let (fmoddly, fmodrate) = (v[7], v[8]);
    let mut n = v[2].clamp(1., 12.) as i32;
    let dims = [v[9].abs(), v[10].abs(), v[11].abs()];
    let mut d = vec![];
    let mut a = vec![];
    let bi;
    if dims.iter().all(|x| *x as i32 == 0) {
        let step = (size_max - size_min) / n as f32;
        bi = (gain * 4096. / n as f32) as i32;
        for i in 0..n as usize {
            let ms = ((i as f32 * step) as i32) as f32 + size_min;
            d.push((ms * 44100. * 0.001) as i32);
            let f = if i == 0 {
                feedback as f64 * 4096.
            } else {
                (feedback as f64).powf(d[i] as f64 / d[0] as f64) * 4096.
            };
            a.push(f.min(4091.904) as i32);
        }
    } else {
        n = match n {
            ..=2 => 3,
            4 | 5 => 6,
            7 | 8 => 9,
            10.. => 12,
            _ => n,
        };
        let mut dim = dims;
        let mut fb = [v[12], v[13], v[14]];
        let swap = |dim: &mut [f32; 3], fb: &mut [f32; 3], i: usize, j: usize| {
            if dim[i] < dim[j] {
                dim.swap(i, j);
                fb.swap(i, j);
            }
        };
        swap(&mut dim, &mut fb, 1, 2);
        swap(&mut dim, &mut fb, 0, 1);
        swap(&mut dim, &mut fb, 1, 2);
        if fb == [0.; 3] {
            let f = feedback as f64;
            fb = [
                f.powf(dim[0] as i32 as f64 / dim[2] as i32 as f64) as f32,
                f.powf(dim[1] as i32 as f64 / dim[2] as i32 as f64) as f32,
                feedback,
            ];
        }
        let spread = ((n / 3) * 7) as f32;
        let gap = dim[1] - dim[2];
        if spread >= gap {
            dim[1] += spread - gap;
        }
        if spread >= dim[0] - dim[1] {
            dim[0] += spread - gap;
        }
        bi = (gain * 4096. / n as f32) as i32;
        for i in 0..n {
            let k = (i % 3) as usize;
            let negative = fb[k] < 0. && i < 3;
            let mut len = dim[k] as i32;
            if fb[k] < 0. && !negative {
                len = (dim[k] * 0.25) as i32;
                if len < 8 {
                    len = 7;
                }
            }
            let mut extra = 0.;
            if i >= 3 {
                let q = (i / 3) as f64;
                let m = q * 7.;
                let v = len as f64 * 0.18 * q;
                extra = if m > v { m } else { v } as f32;
            }
            d.push(((extra as i32 + len) * 44100) / 1000);
            let mut ai = ((fb[k].abs() as f64 * 4096.).min(4091.904)) as i32;
            if negative {
                ai = -ai;
            }
            a.push(ai);
        }
    }
    let filter = (cutoff != 0.).then(|| Iir::design(cutoff, false));
    let parallel = parallel as i32 != 0;
    let combs = (0..n as usize)
        .map(|i| {
            // Filtered combs only in a parallel bank; a negative feedback
            // makes a feed-forward filtered line (DLY_FLINEAR).
            let kind = match (filter.is_some() && parallel, a[i] < 0) {
                (true, true) => FLINEAR,
                (true, false) => LOWPASS,
                _ => PLAIN,
            };
            let f = if kind == PLAIN { None } else { filter };
            let mut line = Dly::new(d[i].max(1) as usize, a[i].abs(), bi, kind, f);
            if fmoddly > 0. {
                let seconds = line.line.d() as f32 * (1. / 44100.);
                let depth = (fmoddly * 0.001 / seconds).clamp(0.01, 0.99);
                let interval = seconds * fmodrate;
                let ramp = (interval * 0.5).min(0.02);
                line.modulation = Some(Box::new(Modulation {
                    interval: (interval * 44100.) as i32,
                    counter: (interval * 44100.) as i32,
                    depth,
                    ramp_seconds: ramp,
                    ramp: Ramp::default(),
                    active: false,
                    seed: 0x9e37_79b9 ^ i as u32,
                }));
            }
            line
        })
        .collect();
    // A serial bank filters its summed output twice (10024f60 + 0xac).
    let post = match (filter, parallel) {
        (Some(f), false) => vec![f, f],
        _ => vec![],
    };
    Stage::Reverb { combs, filter: post }
}
/// DLY / MDY processor (10024cb0); the filter is FLT p[4..8] at unit gain.
fn echo(v: &[f32; 16]) -> Stage {
    let d = (v[1].abs() * 44100. * 0.001) as usize;
    let a = (v[2] * 4096.) as i32;
    let b = (v[3] * 4096.) as i32;
    let kind = match v[0] as i32 {
        1 => ALLPASS,
        2 | 5 => LOWPASS,
        3 => LINEAR,
        4 => FLINEAR,
        _ => PLAIN,
    };
    let filter = matches!(kind, LOWPASS | FLINEAR).then(|| filter_stages(v[4], v[5], v[6], v[7], 1.)[0]);
    Stage::Echo(Dly::new(d.max(1), a, b, kind, filter))
}
impl Stage {
    fn build(p: &Proc, _rate: f32) -> Option<Self> {
        let v = &p.p;
        Some(match p.kind {
            DFR => diffusor(v),
            RVA => reverb(v),
            DLY | MDY => echo(v),
            FLT => Stage::Filter(filter_stages(v[0], v[1], v[2], v[3], if v[4] == 0. { 1. } else { v[4] })),
            AMP => Stage::Gain((v[0] * 4096.) as i32),
            _ => return None,
        })
    }
    fn run(&mut self, x: i32) -> i32 {
        match self {
            Stage::Diffuse(lines) => lines.iter_mut().fold(x, |x, line| line.next(x)),
            Stage::Reverb { combs, filter } => {
                let sum = combs.iter_mut().fold(0i32, |s, c| s.wrapping_add(c.next(x)));
                filter.iter_mut().fold(sum, |v, f| f.run(v))
            }
            Stage::Echo(line) => line.next(x),
            Stage::Filter(stages) => stages.iter_mut().fold(x, |v, f| f.run(v)),
            Stage::Gain(g) => mul(x, *g),
        }
    }
    /// Seconds for the stage's tail to decay by 60 dB.
    fn tail(&self, _rate: f32) -> f32 {
        match self {
            Stage::Diffuse(lines) => lines.iter().map(Dly::tail_seconds).sum(),
            Stage::Reverb { combs, .. } => combs.iter().map(Dly::tail_seconds).fold(0., f32::max),
            Stage::Echo(line) => line.tail_seconds(),
            _ => 0.,
        }
    }
}
/// Runs one float sample through a processor chain in the integer domain.
fn run_chain(chain: &mut [Stage], x: f32) -> f32 {
    let xi = (x * SAMPLE_SCALE) as i32;
    chain.iter_mut().fold(xi, |v, s| s.run(v)) as f32 / SAMPLE_SCALE
}

/// Shared room bus. Retail Source mixes every sound's room send into one
/// room paint buffer and runs the dsp_room preset over it once; running a
/// whole preset per voice kept each sound alive for its 4 s reverb tail, so
/// a horde fight piled up 50-60 live voices and starved the audio thread.
/// Voices write their send into this ring `BUS_LATENCY` frames ahead of the
/// bus read clock; the bus plays the reverb of the summed send.
const BUS_RATE: u32 = 44_100;
/// Must exceed one bus chunk read ahead plus one voice flush (2 x
/// BUS_CHUNK); 768 frames (17 ms) keeps the room tail starting with the
/// sound instead of 46 ms behind it.
const BUS_LATENCY: u64 = 768;
const BUS_FRAMES: usize = 1 << 16;
const BUS_CHUNK: usize = 256;
pub struct BusShared {
    ring: std::sync::Mutex<Vec<f32>>,
    clock: std::sync::atomic::AtomicU64,
    preset: std::sync::Mutex<Option<Arc<Preset>>>,
}
impl BusShared {
    fn new() -> Self {
        Self {
            ring: std::sync::Mutex::new(vec![0.; BUS_FRAMES]),
            clock: std::sync::atomic::AtomicU64::new(0),
            preset: std::sync::Mutex::new(None),
        }
    }
    pub fn set_preset(&self, preset: Option<Arc<Preset>>) {
        if let Ok(mut p) = self.preset.lock() {
            *p = preset;
        }
    }
    /// Adds (bus frame, send) pairs; frames the bus already played are dropped.
    fn add(&self, samples: &[(u64, f32)]) {
        let clock = self.clock.load(std::sync::atomic::Ordering::Acquire);
        if let Ok(mut ring) = self.ring.lock() {
            for &(frame, v) in samples {
                if frame >= clock && frame < clock + BUS_FRAMES as u64 {
                    ring[frame as usize % BUS_FRAMES] += v;
                }
            }
        }
    }
}
#[derive(Resource, Clone)]
pub struct Bus(pub Arc<BusShared>);
impl Default for Bus {
    fn default() -> Self {
        Self(Arc::new(BusShared::new()))
    }
}
/// The bus as a playable source (one long-running stereo voice).
#[derive(Asset, TypePath, Clone)]
pub struct RoomBus(pub Arc<BusShared>);
pub struct BusDecoder {
    shared: Arc<BusShared>,
    preset: Option<Arc<Preset>>,
    chains: Vec<Vec<Stage>>,
    chunk: Vec<f32>,
    at: usize,
    channel: usize,
    wet: f32,
    /// Frames of silence left before the reverb tail has died away.
    tail: u64,
    quiet: u64,
}
impl Decodable for RoomBus {
    type Decoder = BusDecoder;
    fn decoder(&self) -> BusDecoder {
        BusDecoder {
            shared: self.0.clone(),
            preset: None,
            chains: vec![],
            chunk: vec![0.; BUS_CHUNK],
            at: BUS_CHUNK,
            channel: 0,
            wet: 0.,
            tail: 0,
            quiet: 0,
        }
    }
}
impl BusDecoder {
    fn refill(&mut self) {
        let clock = self.shared.clock.load(std::sync::atomic::Ordering::Acquire);
        if let Ok(mut ring) = self.shared.ring.lock() {
            for (k, v) in self.chunk.iter_mut().enumerate() {
                let slot = &mut ring[(clock as usize + k) % BUS_FRAMES];
                *v = *slot;
                *slot = 0.;
            }
        }
        self.shared
            .clock
            .store(clock + BUS_CHUNK as u64, std::sync::atomic::Ordering::Release);
        let preset = self.shared.preset.lock().ok().and_then(|p| p.clone());
        let same = match (&preset, &self.preset) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            let hz = BUS_RATE as f32;
            self.chains = match &preset {
                Some(p) => vec![p.procs.iter().filter_map(|q| Stage::build(q, hz)).collect()],
                None => vec![],
            };
            self.tail = (self.chains.first().map_or(0., |c| c.iter().map(|s| s.tail(hz)).sum::<f32>()).min(4.) * hz) as u64;
            self.preset = preset;
        }
        if self.chunk.iter().any(|v| *v != 0.) {
            self.quiet = 0;
        } else {
            self.quiet += BUS_CHUNK as u64;
        }
        self.at = 0;
    }
}
impl Iterator for BusDecoder {
    type Item = Sample;
    fn next(&mut self) -> Option<Sample> {
        if self.channel == 0 {
            if self.at >= BUS_CHUNK {
                self.refill();
            }
            let x = self.chunk[self.at];
            self.at += 1;
            self.wet = 0.;
            if !self.chains.is_empty() && self.quiet <= self.tail {
                // Retail runs the room preset once on the mono room buffer
                // and writes the result to every channel (10023880).
                let left = run_chain(&mut self.chains[0], x);
                let right = left;
                self.channel = 1;
                self.wet = right;
                return Some(left * DSP_VOLUME);
            }
            self.channel = 1;
            return Some(0.);
        }
        self.channel = 0;
        Some(self.wet * DSP_VOLUME)
    }
}
impl Source for BusDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        ChannelCount::new(2).unwrap()
    }
    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(BUS_RATE).unwrap()
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}
/// A sound that plays its dry part and sends `mix` x `send` of itself to
/// the room bus. It ends with the sound: the reverb tail lives in the bus.
#[derive(Asset, TypePath, Clone)]
pub struct RoomAudio {
    pub source: AudioSource,
    pub mix: f32,
    /// Gain of the send (the voice's volume when it started).
    pub send: f32,
    pub bus: Arc<BusShared>,
}
pub struct RoomDecoder {
    inner: <AudioSource as Decodable>::Decoder,
    channels: ChannelCount,
    rate: SampleRate,
    channel: usize,
    mix: f32,
    send: f32,
    bus: Arc<BusShared>,
    start: Option<u64>,
    frame: u64,
    mono: f32,
    pending: Vec<(u64, f32)>,
}
impl Decodable for RoomAudio {
    type Decoder = RoomDecoder;
    fn decoder(&self) -> RoomDecoder {
        let inner = self.source.decoder();
        RoomDecoder {
            channels: inner.channels(),
            rate: inner.sample_rate(),
            inner,
            channel: 0,
            mix: self.mix,
            send: self.send,
            bus: self.bus.clone(),
            start: None,
            frame: 0,
            mono: 0.,
            pending: Vec::with_capacity(BUS_CHUNK * 2),
        }
    }
}
impl RoomDecoder {
    fn flush(&mut self) {
        if !self.pending.is_empty() {
            self.bus.add(&self.pending);
            self.pending.clear();
        }
    }
}
impl Iterator for RoomDecoder {
    type Item = Sample;
    fn next(&mut self) -> Option<Sample> {
        let Some(x) = self.inner.next() else {
            self.flush();
            return None;
        };
        let start = *self.start.get_or_insert_with(|| {
            self.bus.clock.load(std::sync::atomic::Ordering::Acquire) + BUS_LATENCY
        });
        let channels = self.channels.get() as usize;
        self.mono += x / channels as f32;
        self.channel += 1;
        if self.channel >= channels {
            self.channel = 0;
            // Bus frames this source frame covers (sample-and-hold up).
            let rate = self.rate.get() as u64;
            let a = self.frame * BUS_RATE as u64 / rate;
            let b = (self.frame + 1) * BUS_RATE as u64 / rate;
            let v = self.mono * self.mix * self.send;
            if v != 0. {
                for f in a..b {
                    self.pending.push((start + f, v));
                }
            }
            self.mono = 0.;
            self.frame += 1;
            if self.pending.len() >= BUS_CHUNK {
                self.flush();
            }
        }
        Some(x * (1. - self.mix))
    }
}
impl Source for RoomDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        self.channels
    }
    fn sample_rate(&self) -> SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}
#[derive(Component)]
struct BusPlayer;
/// Keeps one bus voice playing and gives it the current room preset.
fn bus_sync(
    mut commands: Commands,
    bus: Res<Bus>,
    current: Res<Current>,
    players: Query<(), With<BusPlayer>>,
    mut buses: ResMut<Assets<RoomBus>>,
    mut handle: Local<Option<Handle<RoomBus>>>,
) {
    if current.is_changed() {
        bus.0.set_preset(current.preset.clone());
    }
    if players.is_empty() {
        let h = handle.get_or_insert_with(|| buses.add(RoomBus(bus.0.clone()))).clone();
        commands.spawn((AudioPlayer(h), PlaybackSettings::ONCE, BusPlayer));
    }
}

/// The room effect new sounds start with.
#[derive(Resource, Default)]
pub struct Current {
    pub preset: Option<Arc<Preset>>,
    pub label: String,
}
/// The active soundscape's `dsp` value (None keeps the previous room).
#[derive(Resource, Default)]
pub struct SoundscapeDsp(pub Option<i32>);

struct Surfaces {
    /// (sky, audio reflectivity) per texinfo.
    by_texinfo: Vec<(bool, f32)>,
}
#[derive(Default)]
struct State {
    map: Option<std::path::PathBuf>,
    presets: HashMap<usize, Preset>,
    surfaces: Option<Arc<Surfaces>>,
    loading: Option<std::sync::mpsc::Receiver<Surfaces>>,
    room: i32,
    probed_at: Option<[f32; 3]>,
    measured: Option<Room>,
    next: f32,
}

pub fn install(app: &mut App) {
    use bevy::audio::AddAudioSource;
    app.add_audio_source::<RoomAudio>()
        .add_audio_source::<RoomBus>()
        .init_resource::<Current>()
        .init_resource::<Bus>()
        .init_resource::<SoundscapeDsp>()
        .add_systems(Update, (update, bus_sync).chain());
}

fn load_surfaces(root: std::path::PathBuf, map: std::path::PathBuf) -> Option<Surfaces> {
    let mut assets = crate::source_assets::SourceAssets::new(&root, vec![]).ok()?;
    let bytes = crate::bsp::load_bytes(&map, &root).ok()?;
    let infos = crate::bsp::texinfo_materials(&bytes).ok()?;
    // surfaceproperties: name { key value ... } with `base` inheritance.
    let mut props: HashMap<String, HashMap<String, String>> = HashMap::new();
    if let Ok(b) = assets.read("scripts/surfaceproperties_manifest.txt") {
        let t = crate::kv::tokens(&String::from_utf8_lossy(&b));
        for p in t.windows(2).filter(|p| p[0] == "file") {
            if let Ok(b) = assets.read(&p[1]) {
                let t = crate::kv::tokens(&String::from_utf8_lossy(&b));
                let mut i = 0;
                while i + 1 < t.len() {
                    let name = t[i].to_lowercase();
                    if t[i + 1] != "{" {
                        i += 1;
                        continue;
                    }
                    i += 2;
                    let mut values = HashMap::new();
                    while i + 1 < t.len() && t[i] != "}" {
                        values.insert(t[i].to_lowercase(), t[i + 1].clone());
                        i += 2;
                    }
                    i += 1;
                    props.insert(name, values);
                }
            }
        }
    }
    let reflectivity = |name: &str| {
        let mut name = name.to_string();
        for _ in 0..16 {
            let Some(p) = props.get(&name) else { break };
            if let Some(v) = p.get("audioreflectivity").and_then(|v| v.parse().ok()) {
                return v;
            }
            name = p.get("base").cloned().unwrap_or_else(|| "default".into()).to_lowercase();
        }
        0.5
    };
    let mut cache = HashMap::new();
    let by_texinfo = infos
        .iter()
        .map(|(material, flags)| {
            let sky = flags & (0x2 | 0x4) != 0;
            let refl = *cache
                .entry(material.to_lowercase())
                .or_insert_with(|| reflectivity(&crate::material::surface_property(&mut assets, material)));
            (sky, refl)
        })
        .collect();
    Some(Surfaces { by_texinfo })
}

#[allow(clippy::too_many_arguments)]
fn update(
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    collision: Option<Res<crate::collision::Collision>>,
    scape: Res<SoundscapeDsp>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    time: Res<Time>,
    mut current: ResMut<Current>,
    mut state: Local<State>,
) {
    let _timing = crate::performance::scope("room_dsp::update");
    let (Some(session), Some(source)) = (session, source) else { return };
    if state.map.as_ref() != Some(&session.map) {
        state.map = Some(session.map.clone());
        if state.presets.is_empty() {
            if let Ok(mut assets) = crate::source_assets::SourceAssets::new(&source.0, vec![]) {
                if let Ok(b) = assets.read("scripts/dsp_presets.txt") {
                    state.presets = parse_presets(&String::from_utf8_lossy(&b));
                }
            }
        }
        state.surfaces = None;
        state.measured = None;
        state.probed_at = None;
        state.room = 0;
        current.preset = None;
        current.label.clear();
        let (tx, rx) = std::sync::mpsc::channel();
        let (root, map) = (source.0.clone(), session.map.clone());
        std::thread::spawn(move || {
            if let Some(s) = load_surfaces(root, map) {
                let _ = tx.send(s);
            }
        });
        state.loading = Some(rx);
    }
    if let Some(rx) = &state.loading {
        if let Ok(s) = rx.try_recv() {
            state.surfaces = Some(Arc::new(s));
            state.loading = None;
        }
    }
    if let Some(room) = scape.0 {
        if room != state.room {
            state.room = room;
            state.measured = None;
            state.probed_at = None;
            current.preset = (room != 1)
                .then(|| state.presets.get(&(room.max(0) as usize)).filter(|p| !p.procs.is_empty()).cloned().map(Arc::new))
                .flatten();
            current.label = format!("room {room}");
        }
    }
    if state.room != 1 {
        return;
    }
    // ADSP: re-measure once the listener has moved 48 units across the floor.
    let now = time.elapsed_secs();
    let (Some(collision), Some(surfaces), Some(listener)) = (collision, state.surfaces.clone(), camera.iter().next()) else { return };
    let p = listener.translation() / 0.0254;
    let origin = [p.x, -p.z, p.y];
    if now < state.next
        || state.probed_at.is_some_and(|q| ((q[0] - origin[0]).powi(2) + (q[1] - origin[1]).powi(2)).sqrt() <= 48.)
    {
        return;
    }
    state.next = now + 0.25;
    state.probed_at = Some(origin);
    let Some(rays) = probe(origin, |a, b, props| {
        let (fraction, end, texinfo, solid) = collision.source_ray(a, b, if props { 0x4003 } else { 0x1 }, props)?;
        if fraction >= 1. && !solid {
            return None;
        }
        let (sky, reflectivity) = texinfo.and_then(|t| surfaces.by_texinfo.get(t).copied()).unwrap_or((false, 0.5));
        Some(Hit { fraction, end, sky, reflectivity })
    }) else {
        return;
    };
    let room = analyse(&rays);
    // 1000f040: keep the current room unless it changed by 40% or more.
    if let Some(old) = &state.measured {
        let change = |a: i32, b: i32| if b == 0 { 1. } else { let r = a as f32 / b as f32; 1. - if r > 1. { 1. / r } else { r } };
        if old.outside == room.outside && change(room.width, old.width) < 0.4 && change(room.length, old.length) < 0.4 {
            return;
        }
    }
    let (class, sizes) = classify(&room);
    if let Some(preset) = synthesize(&state.presets, &room) {
        const NAMES: [&str; 11] = ["room", "duct", "hall", "tunnel", "street", "alley", "courtyard", "open space", "open wall", "open street", "open courtyard"];
        current.label = format!("ADSP {} {}x{}x{} size {}", NAMES[class], room.width, room.length, room.height, sizes[0]);
        if std::env::args().any(|a| a == "--audit-audio") {
            println!("Room DSP: {}", current.label);
        }
        current.preset = Some(Arc::new(preset));
    }
    state.measured = Some(room);
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = "
// comment { 99 }
{ 102 LINEAR 0.3 0.8 0.0 0.0 80 0.5
    // RVA size min
    { RVA 80.0 30.0 2 0.78 1.1 6000 1 0 0 0 0 0 0 0 0 0 }
}
{ 103 LINEAR 0.32 0.8 0.0 0.0 80 0.5
    { DFR 1.0 3 0.15 }
    { RVA 240.0 50.0 10 0.97 2.4 1800 1 0 0 0 0 0 0 0 0 0 }
}
{ 20 LINEAR 0.2 0.7 0.0 0.0 80 0.5 { DFR 1.0 2 0.15 } { DLY LOWPASS 300.0 0.5 0.84 LP 2000 0 LO 0 0 0 } }
";
    /// A mono 16-bit PCM wave of `samples` full-scale samples.
    fn wave(samples: usize, rate: u32) -> AudioSource {
        let data: Vec<u8> = (0..samples).flat_map(|_| 16000i16.to_le_bytes()).collect();
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend((36 + data.len() as u32).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(rate.to_le_bytes());
        b.extend((rate * 2).to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend((data.len() as u32).to_le_bytes());
        b.extend(data);
        AudioSource { bytes: b.into() }
    }
    #[test]
    fn voices_end_with_their_sound_and_the_shared_bus_plays_the_reverb() {
        let presets = parse_presets(SAMPLE);
        let bus = Bus::default();
        bus.0.set_preset(Some(Arc::new(presets[&20].clone())));
        let mut player = RoomBus(bus.0.clone()).decoder();
        // Prime the bus clock, as it runs before any voice starts.
        for _ in 0..BUS_CHUNK * 2 {
            player.next();
        }
        let voice = RoomAudio { source: wave(1000, 22_050), mix: 0.5, send: 1., bus: bus.0.clone() };
        // No per-voice tail: exactly the source's samples, at half dry level.
        let out: Vec<f32> = voice.decoder().collect();
        assert_eq!(out.len(), 1000);
        assert!((out[0] - 16000. / 32768. * 0.5).abs() < 1e-3);
        // The bus carries the send and its echo well past the voice's end.
        let tail: Vec<f32> = (0..44_100 * 2).map(|_| player.next().unwrap()).collect();
        let energy = |s: &[f32]| s.iter().map(|v| v * v).sum::<f32>();
        assert!(energy(&tail[..(BUS_LATENCY as usize + 4000) * 2]) > 1.);
        assert!(energy(&tail[40_000..]) > 0., "the echo outlives the voice");
    }
    #[test]
    fn a_quiet_bus_stops_processing_after_its_tail() {
        let presets = parse_presets(SAMPLE);
        let bus = Bus::default();
        bus.0.set_preset(Some(Arc::new(presets[&103].clone())));
        let mut player = RoomBus(bus.0.clone()).decoder();
        let out: Vec<f32> = (0..44_100 * 2 * 6).map(|_| player.next().unwrap()).collect();
        assert!(out.iter().all(|v| *v == 0.));
        assert!(player.quiet > player.tail);
    }
    #[test]
    fn presets_parse_with_named_constants() {
        let p = parse_presets(SAMPLE);
        assert_eq!(p[&102].mix, [0.3, 0.8]);
        assert_eq!(p[&102].procs[0].kind, RVA);
        assert_eq!(p[&103].procs.len(), 2);
        assert_eq!(p[&20].procs[1].kind, DLY);
        assert_eq!(p[&20].procs[1].p[0], 2.); // LOWPASS
        assert_eq!(p[&20].procs[1].p[1], 300.);
        assert!(!p.contains_key(&99));
    }
    fn box_room(w: f32, l: f32, h: f32) -> Rays {
        // A closed axis-aligned room centred on the listener, 64 units up.
        probe([0., 0., 64.], |a, b, _| {
            let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let mut t = 1f32;
            for (k, half) in [(0, l / 2.), (1, w / 2.)] {
                if d[k] != 0. {
                    t = t.min(((half * d[k].signum()) - a[k]) / d[k]);
                }
            }
            if d[2] > 0. {
                t = t.min((h - a[2]) / d[2]);
            } else if d[2] < 0. {
                t = t.min((0. - a[2]) / d[2]);
            }
            (t < 1.).then(|| Hit { fraction: t, end: [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t], sky: false, reflectivity: 0.6 })
        })
        .unwrap()
    }
    #[test]
    fn adsp_measures_and_classifies_rooms_halls_and_ducts() {
        // In a rectangle the diagonal pair spans the larger area, so the
        // engine sizes the room on the diagonals (2 x 294 units here).
        let room = analyse(&box_room(400., 500., 128.));
        assert_eq!((room.width, room.length), (588, 588));
        assert_eq!(room.height, 128);
        assert!(!room.outside);
        assert_eq!(classify(&room).0, 0);
        assert_eq!(classify(&analyse(&box_room(80., 400., 128.))).0, 2, "hall");
        assert_eq!(classify(&analyse(&box_room(40., 400., 40.))).0, 1, "duct");
        assert_eq!(classify(&analyse(&box_room(150., 1200., 200.))).0, 3, "tunnel");
        // Open sky above: outside, all walls closed and short -> courtyard.
        let mut open = box_room(400., 500., 128.);
        open.sky[18] = 0.1;
        assert_eq!(classify(&analyse(&open)).0, 6);
    }
    #[test]
    fn adsp_blends_min_and_max_presets_by_room_size() {
        let p = parse_presets(SAMPLE);
        let small = synthesize(&p, &analyse(&box_room(60., 80., 100.))).unwrap();
        let big = synthesize(&p, &analyse(&box_room(1200., 1500., 400.))).unwrap();
        let rva = |x: &Preset| x.procs.iter().find(|p| p.kind == RVA).unwrap().p;
        assert_eq!(rva(&small)[0], 80.);
        assert!(rva(&big)[0] > 150., "{}", rva(&big)[0]);
        assert!(rva(&big)[3] > rva(&small)[3]);
    }
    /// Numbers worked by hand from the engine.dll code paths.
    #[test]
    fn processors_match_engine_arithmetic() {
        // 1001e400: 3 kHz lowpass, Oc = tan(pi * 3000 / 44100).
        let oc = (std::f64::consts::PI * 3000. / 44100.).tan();
        let c = (1. - oc) / (1. + oc);
        let lp = Iir::design(3000., false);
        assert_eq!(lp.a1, (c * -4096.) as i32);
        assert_eq!((lp.b0, lp.b1), (((1. - c) * 0.5 * 4096.) as i32, ((1. - c) * 0.5 * 4096.) as i32));
        let hp = Iir::design(3000., true);
        assert_eq!(hp.b1, -hp.b0);
        // 1001ee00: feedback 0.9 -> output gain 4 * trunc(4096 * 0.1) / 4096.
        let line = Dly::new(100, (0.9 * 4096.) as i32, 4096, PLAIN, None);
        assert_eq!(line.b, 4 * ((1. - (3686. / 4096.) as f64) * 4096.) as i32);
        // Feedback 0.5 or less keeps the full gain.
        assert_eq!(Dly::new(100, 2048, 4096, PLAIN, None).b, 4096);
        // 100259c0: DFR delays are table ms x size at 44.1 kHz.
        let Stage::Diffuse(lines) = diffusor(&[1.0, 3., 0.15, 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.]) else {
            panic!()
        };
        assert_eq!(lines.iter().map(|l| l.line.d()).collect::<Vec<_>>(), vec![573, 837, 1146]);
        // A plain comb returns b * (x + a * delayed): the input itself
        // comes through on the first sample (1001eeb0 default case).
        let mut comb = Dly::new(10, 2048, 4096, PLAIN, None);
        assert_eq!(comb.next(1000), 1000);
        // 10025640: four combs at 30 + trunc(i * 12.5) ms, gain 1.1 / 4 each.
        let Stage::Reverb { combs, filter } = reverb(&[80., 30., 4., 0.85, 1.1, 4000., 1., 0., 0., 0., 0., 0., 0., 0., 0., 0.]) else {
            panic!()
        };
        assert!(filter.is_empty(), "a parallel bank filters inside each comb");
        assert_eq!(combs.iter().map(|c| c.line.d()).collect::<Vec<_>>(), vec![1323, 1852, 2425, 2954]);
        assert!(combs.iter().all(|c| c.kind == LOWPASS));
        assert_eq!(combs[0].a, (0.85f64 * 4096.) as i32);
        assert_eq!(combs[1].a, (0.85f64.powf(1852. / 1323.) * 4096.) as i32);
    }
    #[test]
    fn reverb_chain_adds_a_decaying_tail() {
        let p = parse_presets(SAMPLE);
        let mut chain: Vec<Stage> = p[&103].procs.iter().filter_map(|x| Stage::build(x, 22050.)).collect();
        let mut out = vec![];
        for n in 0..22050 {
            let x = if n == 0 { 1. } else { 0. };
            out.push(run_chain(&mut chain, x));
        }
        let energy = |r: std::ops::Range<usize>| out[r].iter().map(|v| v * v).sum::<f32>();
        assert!(energy(1000..5000) > 1e-4);
        assert!(energy(1000..5000) > energy(15000..22050));
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 10.));
    }
    /// ADSP measurements at positions recorded in retail with adsp_debug 2
    /// (local-research/parity/2026-10-22-retail-adsp-c1m1.log; retail prints
    /// feet). The rooftop retail reading was a reused nearby node.
    #[test]
    fn c1m1_adsp_positions() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        if !path.is_file() {
            return;
        }
        // (feet, class, length, width, height, reflectivity) from retail.
        let retail: [([f32; 3], Option<(usize, i32, i32, i32, f32)>); 3] = [
            ([923., 5843., 2702.], None),
            ([2130.7, 5529.7, 1192.], Some((3, 186 * 12, 27 * 12, 15 * 12, 0.135))),
            ([-670.6, 4760., 1192.], Some((0, 96 * 12, 89 * 12, 23 * 12, 0.03))),
        ];
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, false).unwrap();
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let surfaces = load_surfaces(root.to_path_buf(), path.clone()).unwrap();
        let eye: f32 = std::env::var("ADSP_EYE").ok().and_then(|v| v.parse().ok()).unwrap_or(62.);
        for feet in [[515.6, 5552.4, 2848.], [923., 5843., 2702.], [2059., 6217., 2507.], [2130.7, 5529.7, 1192.], [-670.6, 4760., 1192.]] {
            let origin = [feet[0], feet[1], feet[2] + eye];
            let rays = probe(origin, |a, b, props| {
                let (fraction, end, texinfo, solid) = collision.source_ray(a, b, if props { 0x4003 } else { 0x1 }, props)?;
                if fraction >= 1. && !solid {
                    return None;
                }
                let (sky, reflectivity) = texinfo.and_then(|t| surfaces.by_texinfo.get(t).copied()).unwrap_or((false, 0.5));
                Some(Hit { fraction, end, sky, reflectivity })
            });
            let expected = retail.iter().find(|(p, _)| *p == feet).map(|(_, e)| *e);
            let Some(rays) = rays else {
                println!("ADSP {feet:?}: skipped (floor more than 72 below)");
                assert!(expected.is_none_or(|e| e.is_none()), "{feet:?}");
                continue;
            };
            let room = analyse(&rays);
            let (class, sizes) = classify(&room);
            if let Some(Some((c, l, w, h, r))) = expected {
                let near = |a: i32, b: i32, tol: f32| ((a - b).abs() as f32) <= b as f32 * tol;
                assert_eq!(class, c, "{feet:?}");
                assert!(near(room.length, l, 0.06) && near(room.width, w, 0.06), "{feet:?} {room:?}");
                assert!(near(room.height, h, 0.15), "{feet:?} {room:?}");
                assert!((room.reflectivity - r).abs() < 0.02, "{feet:?} {room:?}");
            }
            println!("ADSP {feet:?}: size:({},{}) height:({}) refl {:.4} sky {} class {class} sizes {sizes:?} dist {:?} refl {:?}", room.length, room.width, room.height, room.reflectivity, room.outside as u8, rays.dist, rays.refl);
        }
    }
    #[test]
    fn retail_dsp_presets_parse() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let Ok(mut assets) = crate::source_assets::SourceAssets::new(root, vec![]) else { return };
        let Ok(b) = assets.read("scripts/dsp_presets.txt") else { return };
        let p = parse_presets(&String::from_utf8_lossy(&b));
        assert!(p.len() >= 140, "{}", p.len());
        for base in CLASS_PRESET {
            assert!(p.contains_key(&base) && p.contains_key(&(base + 1)), "{base}");
        }
        assert!(p[&1].procs.is_empty(), "room 1 is the ADSP placeholder");
    }
}
