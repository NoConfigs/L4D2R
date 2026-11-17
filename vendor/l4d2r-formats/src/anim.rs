//! Skeletal animation for Source studio models (MDL v44-49 as shipped with L4D2).
//!
//! What is implemented (verified against real L4D2 data in `tests/anim_real.rs`):
//! * `Skeleton`: bones (+ `poseToBone`), sequences (`mstudioseqdesc_t`), animation descriptors
//!   (`mstudioanimdesc_t`), `$includemodel` sequence groups (survivors keep nearly all of their
//!   sequences in `anim_*.mdl` files), external `.ani` anim blocks.
//! * RLE / raw per-bone animation decode (`mstudioanim_t`: RAWPOS/RAWROT/ANIMPOS/ANIMROT/DELTA/RAWROT2,
//!   Quaternion48/64, Vector48), sectioned animations.
//! * Pose evaluation (bone-to-model matrices) and CPU skinning.
//!
//! Includes blend grids, weighted gestures, native sequence autolayers and frame animations.
//! Native IK chains and MDL/ANI error rules are decoded for runtime targeting.
//! Not implemented: bone controllers, procedural bones (jiggle / aim-at / quaternion
//! interpolation), zero-frame caches and extraction of root motion into entity movement.

use crate::mdl::{load_static, ModelBatch};
use anyhow::{anyhow, ensure, Result};
use glam::{Mat4, Quat, Vec3};
#[path = "studio_ik.rs"]
mod ik;
pub use ik::{solve_chain, IkChain, IkLock, IkRule};

// ---------------------------------------------------------------------------------------------
// byte reader (kept local; mdl.rs's reader is private)
// ---------------------------------------------------------------------------------------------

struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn bytes(&self, off: usize, n: usize) -> Result<&'a [u8]> {
        let end = off
            .checked_add(n)
            .ok_or_else(|| anyhow!("offset overflow"))?;
        self.0
            .get(off..end)
            .ok_or_else(|| anyhow!("read {}..{} out of bounds ({})", off, end, self.0.len()))
    }
    fn i32(&self, off: usize) -> Result<i32> {
        Ok(i32::from_le_bytes(self.bytes(off, 4)?.try_into().unwrap()))
    }
    fn u16(&self, off: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(off, 2)?.try_into().unwrap()))
    }
    fn i16(&self, off: usize) -> Result<i16> {
        Ok(self.u16(off)? as i16)
    }
    fn u8(&self, off: usize) -> Result<u8> {
        Ok(self.bytes(off, 1)?[0])
    }
    fn f32(&self, off: usize) -> Result<f32> {
        Ok(f32::from_le_bytes(self.bytes(off, 4)?.try_into().unwrap()))
    }
    fn vec3(&self, off: usize) -> Result<Vec3> {
        Ok(Vec3::new(
            self.f32(off)?,
            self.f32(off + 4)?,
            self.f32(off + 8)?,
        ))
    }
    fn cstr(&self, off: usize) -> Result<String> {
        let s = self
            .0
            .get(off..)
            .ok_or_else(|| anyhow!("string offset {} out of bounds", off))?;
        let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
        Ok(String::from_utf8_lossy(&s[..end]).into_owned())
    }
    fn rel(base: usize, rel: i32) -> Result<usize> {
        let v = base as i64 + rel as i64;
        ensure!(v >= 0, "negative offset");
        Ok(v as usize)
    }
}

// ---------------------------------------------------------------------------------------------
// flags
// ---------------------------------------------------------------------------------------------

pub const STUDIO_ANIM_RAWPOS: u8 = 0x01;
pub const STUDIO_ANIM_RAWROT: u8 = 0x02;
pub const STUDIO_ANIM_ANIMPOS: u8 = 0x04;
pub const STUDIO_ANIM_ANIMROT: u8 = 0x08;
pub const STUDIO_ANIM_DELTA: u8 = 0x10;
pub const STUDIO_ANIM_RAWROT2: u8 = 0x20;

/// `mstudioanimdesc_t::flags` / `mstudioseqdesc_t::flags` bits that matter here.
pub const STUDIO_LOOPING: i32 = 0x0001;
pub const STUDIO_DELTA: i32 = 0x0004;
/// Snaps to the new sequence without fading (gesture layers keep weight 1).
pub const STUDIO_SNAP: i32 = 0x0002;
pub const STUDIO_POST: i32 = 0x0010;
pub const STUDIO_LOCAL: i32 = 0x0200;
const AL_SPLINE: i32 = 0x0040;
const AL_XFADE: i32 = 0x0080;
const AL_NOBLEND: i32 = 0x0200;
const AL_LOCAL: i32 = 0x1000;
const AL_POSE: i32 = 0x4000;
pub const STUDIO_ALLZEROS: i32 = 0x0020; // anim desc
pub const STUDIO_FRAMEANIM: i32 = 0x0040; // anim desc (unsupported)
pub const STUDIO_SEQ_AUTOPLAY: i32 = 0x0008;
pub const STUDIO_SEQ_HIDDEN: i32 = 0x0040;

// ---------------------------------------------------------------------------------------------
// public data types
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    /// Parent bone index or -1.
    pub parent: i32,
    pub pos: Vec3,
    /// Source stores quaternions as (x, y, z, w).
    pub quat: Quat,
    /// Euler (radians), RadianEuler order = (roll X, pitch Y, yaw Z). Base for `ANIMROT` data.
    pub rot: Vec3,
    pub pos_scale: Vec3,
    pub rot_scale: Vec3,
    /// Inverse of the bind-pose bone-to-model matrix (`mstudiobone_t::poseToBone`).
    pub pose_to_bone: Mat4,
    pub flags: i32,
    pub proc_type: i32,
    /// STUDIO_PROC_QUATINTERP helper (shoulder, bicep, elbow, knee...).
    pub quat_interp: Option<QuatInterp>,
}

/// `mstudioquatinterpbone_t`: a helper bone posed from the local rotation
/// of a `control` bone by blending authored trigger poses.
#[derive(Clone, Debug)]
pub struct QuatInterp {
    pub control: usize,
    pub triggers: Vec<QuatTrigger>,
}
/// `mstudioquatinterpinfo_t`.
#[derive(Clone, Copy, Debug)]
pub struct QuatTrigger {
    pub inv_tolerance: f32,
    pub trigger: Quat,
    pub pos: Vec3,
    pub quat: Quat,
}

/// Source bone_setup.cpp DoQuatInterpBone: weight each trigger by how close
/// the control bone's parent-relative rotation is to it, and blend the
/// triggers' local pose. Returns the helper's parent-relative transform.
pub fn quat_interp_local(proc_: &QuatInterp, control_local: Quat) -> Option<Mat4> {
    let first = proc_.triggers.first()?;
    let weights: Vec<f32> = proc_
        .triggers
        .iter()
        .map(|t| {
            let dot = t.trigger.dot(control_local).abs().clamp(-1.0, 1.0);
            (1.0 - 2.0 * dot.acos() * t.inv_tolerance).max(0.0)
        })
        .collect();
    let total: f32 = weights.iter().sum();
    if total <= 0.001 {
        return Some(Mat4::from_rotation_translation(first.quat, first.pos));
    }
    let mut q = glam::Vec4::ZERO;
    let mut pos = Vec3::ZERO;
    for (t, w) in proc_.triggers.iter().zip(&weights) {
        if *w <= 0.0 {
            continue;
        }
        let s = w / total;
        // QuaternionAlign: flip into the same hemisphere as the running sum.
        let tq = glam::Vec4::from(t.quat);
        let tq = if tq.dot(q) < 0.0 { -tq } else { tq };
        q += tq * s;
        pos += t.pos * s;
    }
    let q = Quat::from_vec4(q);
    if q.length_squared() < 1e-12 {
        return None;
    }
    Some(Mat4::from_rotation_translation(q.normalize(), pos))
}

#[derive(Clone, Debug)]
pub struct AnimDesc {
    pub name: String,
    /// 0 = the main MDL, n>0 = n-th `$includemodel` (see `Skeleton::source_names`).
    pub source: usize,
    pub fps: f32,
    pub flags: i32,
    pub num_frames: i32,
    /// 0 = data lives in the MDL itself, >0 = block in the `.ani` file.
    pub anim_block: i32,
    pub anim_index: i32,
    pub section_index: i32,
    pub section_frames: i32,
    /// Byte offset of this descriptor inside its MDL (section/anim offsets are relative to it).
    base: usize,
}

impl AnimDesc {
    pub fn looping(&self) -> bool {
        self.flags & STUDIO_LOOPING != 0
    }
    pub fn duration(&self) -> f32 {
        if self.fps > 0.0 {
            (self.num_frames - 1).max(0) as f32 / self.fps
        } else {
            0.0
        }
    }
}

/// `mstudioevent_t` (80 bytes: cycle, event, type, options[64], szeventindex).
#[derive(Clone, Debug)]
pub struct SeqEvent {
    pub cycle: f32,
    pub event: i32,
    pub options: String,
}

/// Native `mstudioautolayer_t`, with source-local indices resolved on loading.
#[derive(Clone, Debug)]
pub struct AutoLayer {
    pub sequence: usize,
    pub pose: Option<String>,
    pub flags: i32,
    pub start: f32,
    pub peak: f32,
    pub tail: f32,
    pub end: f32,
}

impl AutoLayer {
    fn sample(&self, cycle: f32, parent_weight: f32, values: &[(&str, f32)]) -> Option<(f32, f32)> {
        if self.start == self.end {
            return Some((cycle, parent_weight));
        }
        let pose_driven = self.flags & AL_POSE != 0 && self.flags & AL_LOCAL == 0;
        let index = if pose_driven {
            self.pose
                .as_ref()
                .and_then(|name| values.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)))
                .map_or(0., |(_, value)| *value)
        } else {
            cycle
        };
        if !index.is_finite() || index < self.start || index >= self.end {
            return None;
        }
        let mut ramp = if index < self.peak && self.peak != self.start {
            (index - self.start) / (self.peak - self.start)
        } else if index > self.tail && self.end != self.tail {
            (self.end - index) / (self.end - self.tail)
        } else {
            1.
        }
        .clamp(0., 1.);
        if self.flags & AL_SPLINE != 0 {
            ramp = ramp * ramp * (3. - 2. * ramp);
        }
        let weight = if self.flags & AL_XFADE != 0 && index > self.tail {
            let denominator = 1. - parent_weight + ramp * parent_weight;
            if denominator > 0. {
                ramp * parent_weight / denominator
            } else {
                0.
            }
        } else if self.flags & AL_NOBLEND != 0 {
            ramp
        } else {
            ramp * parent_weight
        };
        let layer_cycle = if pose_driven {
            cycle
        } else {
            (cycle - self.start) / (self.end - self.start)
        };
        Some((layer_cycle, weight.clamp(0., 1.)))
    }
}

#[derive(Clone, Debug)]
pub struct Sequence {
    pub label: String,
    /// e.g. "ACT_IDLE" (empty if the sequence has none).
    pub activity_name: String,
    /// Raw `activity` int. Not stable across games/builds; prefer `activity_name`.
    pub activity: i32,
    pub activity_weight: i32,
    pub flags: i32,
    pub source: usize,
    pub num_blends: i32,
    /// `groupsize[2]`.
    pub group_size: [i32; 2],
    /// Indices into `Skeleton::anims()` for every blend (`group_size[0]*group_size[1]`).
    pub blend_anims: Vec<usize>,
    /// Per-bone weights (indexed by main-skeleton bone), empty if none.
    pub weight_list: Vec<f32>,
    pub local_entry_node: i32,
    pub local_exit_node: i32,
    pub node_flags: i32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub bbmin: Vec3,
    pub bbmax: Vec3,
    pub num_events: i32,
    /// `mstudioevent_t` list (cycle 0..1, numeric event id, options string).
    pub events: Vec<SeqEvent>,
    pub num_autolayers: i32,
    pub autolayers: Vec<AutoLayer>,
    pub ik_locks: Vec<IkLock>,
    /// Pose parameter driving each blend-grid axis (name, start, end), if any. A parameter value `v` maps to the
    /// grid axis as `(v - start) / (end - start)` (0..1).
    pub blend_params: [Option<(String, f32, f32)>; 2],
}

/// `mstudioposeparamdesc_t`
#[derive(Clone, Debug)]
pub struct PoseParam {
    pub name: String,
    pub flags: i32,
    pub start: f32,
    pub end: f32,
    pub loop_: f32,
}

impl Sequence {
    pub fn looping(&self) -> bool {
        self.flags & STUDIO_LOOPING != 0
    }
    pub fn is_delta(&self) -> bool {
        self.flags & STUDIO_DELTA != 0
    }
    pub fn snap(&self) -> bool {
        self.flags & STUDIO_SNAP != 0
    }
}

#[derive(Clone)]
struct SrcBone {
    name: String,
    quat: Quat,
    pos: Vec3,
    rot: Vec3,
    pos_scale: Vec3,
    rot_scale: Vec3,
    main: Option<usize>,
}

struct Source {
    name: String,
    /// Shared between every skeleton that includes the same file.
    mdl: std::sync::Arc<[u8]>,
    ani: Option<std::sync::Arc<[u8]>>,
    /// `(datastart, dataend)` of each `mstudioanimblock_t`.
    blocks: Vec<(usize, usize)>,
    /// This MDL's own bones (bases/scales for decoding) + index into the main skeleton.
    bones: Vec<SrcBone>,
    ani_name: String,
    chains: Vec<IkChain>,
}

/// One copy of each included animation MDL/.ani for the whole process.
/// Every model that includes e.g. anim_common.mdl used to keep its own copy
/// of the file bytes (gigabytes across the survivor, infected and view
/// models). The main model ("<main>") is not shared.
fn shared_bytes(name: &str, bytes: &[u8]) -> std::sync::Arc<[u8]> {
    use std::sync::{Arc, Mutex, OnceLock, Weak};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Weak<[u8]>>>> = OnceLock::new();
    if name.starts_with('<') {
        return Arc::from(bytes);
    }
    let key = name.to_ascii_lowercase();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = cache.get(&key).and_then(Weak::upgrade) {
        if *found == *bytes {
            return found;
        }
    }
    let new: Arc<[u8]> = Arc::from(bytes);
    cache.retain(|_, w| w.strong_count() > 0);
    cache.insert(key, Arc::downgrade(&new));
    new
}

/// `mstudioattachment_t`: a named point on a bone (cameras, muzzle flashes...).
#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub bone: usize,
    /// Attachment-to-bone transform.
    pub local: Mat4,
}

pub struct Skeleton {
    pub bones: Vec<Bone>,
    pub hitboxes: Vec<Hitbox>,
    pub attachments: Vec<Attachment>,
    pub ik_chains: Vec<IkChain>,
    pub ik_autoplay_locks: Vec<IkLock>,
    sources: Vec<Source>,
    sequences: Vec<Sequence>,
    anims: Vec<AnimDesc>,
    pose_params: Vec<PoseParam>,
}
#[derive(Clone, Debug)]
pub struct Hitbox {
    pub bone: usize,
    pub group: i32,
    pub min: Vec3,
    pub max: Vec3,
}
fn parse_hitboxes(m: &Rd, bone_count: usize) -> Result<Vec<Hitbox>> {
    let sets = m.i32(172)?;
    if sets == 0 {
        return Ok(vec![]);
    }
    ensure!((1..=128).contains(&sets), "invalid hitbox set count");
    let set = m.i32(176)?;
    ensure!(set > 0, "invalid hitbox set offset");
    let set = set as usize;
    let count = m.i32(set + 4)?;
    ensure!((0..=1024).contains(&count), "invalid hitbox count");
    let start = Rd::rel(set, m.i32(set + 8)?)?;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let at = start + i * 68;
        let bone = m.i32(at)?;
        let min = m.vec3(at + 8)?;
        let max = m.vec3(at + 20)?;
        ensure!(
            bone >= 0
                && (bone as usize) < bone_count
                && min.is_finite()
                && max.is_finite()
                && min.cmple(max).all(),
            "invalid studio hitbox"
        );
        out.push(Hitbox {
            bone: bone as usize,
            group: m.i32(at + 4)?,
            min,
            max,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// parsing
// ---------------------------------------------------------------------------------------------

fn check_header(mdl: &[u8]) -> Result<Rd<'_>> {
    ensure!(mdl.len() >= 408 && &mdl[0..4] == b"IDST", "bad MDL magic");
    let m = Rd(mdl);
    let version = m.i32(4)?;
    ensure!(
        (44..=49).contains(&version),
        "unsupported MDL version {}",
        version
    );
    Ok(m)
}

fn parse_attachments(m: &Rd) -> Vec<Attachment> {
    let (Ok(n), Ok(idx)) = (m.i32(240), m.i32(244)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..n.max(0) as usize {
        let b = idx as usize + i * 92;
        let (Ok(ni), Ok(bone)) = (m.i32(b), m.i32(b + 8)) else {
            break;
        };
        let Ok(name) = Rd::rel(b, ni).and_then(|o| m.cstr(o)) else {
            break;
        };
        let mut r = [0f32; 12];
        for (k, v) in r.iter_mut().enumerate() {
            *v = m.f32(b + 12 + k * 4).unwrap_or(0.0);
        }
        let local = Mat4::from_cols_array(&[
            r[0], r[4], r[8], 0.0, r[1], r[5], r[9], 0.0, r[2], r[6], r[10], 0.0, r[3], r[7],
            r[11], 1.0,
        ]);
        out.push(Attachment {
            name,
            bone: bone.max(0) as usize,
            local,
        });
    }
    out
}

/// STUDIO_PROC_QUATINTERP (2) data at `procindex` (relative to the bone).
fn parse_quat_interp(m: &Rd, b: usize, bones: usize) -> Result<Option<QuatInterp>> {
    if m.i32(b + 164)? != 2 {
        return Ok(None);
    }
    let at = Rd::rel(b, m.i32(b + 168)?)?;
    let control = m.i32(at)?;
    let count = m.i32(at + 4)?;
    if control < 0 || control as usize >= bones || !(1..=32).contains(&count) {
        return Ok(None);
    }
    let base = Rd::rel(at, m.i32(at + 8)?)?;
    let quat = |o: usize| -> Result<Quat> {
        Ok(Quat::from_xyzw(m.f32(o)?, m.f32(o + 4)?, m.f32(o + 8)?, m.f32(o + 12)?))
    };
    let mut triggers = Vec::with_capacity(count as usize);
    for t in 0..count as usize {
        let o = base + t * 48;
        let q = quat(o + 32)?;
        triggers.push(QuatTrigger {
            inv_tolerance: m.f32(o)?,
            trigger: quat(o + 4)?.normalize(),
            pos: m.vec3(o + 20)?,
            quat: if q.length_squared() > 1e-6 { q.normalize() } else { Quat::IDENTITY },
        });
    }
    Ok(Some(QuatInterp { control: control as usize, triggers }))
}

fn parse_bones(m: &Rd) -> Result<Vec<Bone>> {
    let n = m.i32(156)? as usize;
    let idx = m.i32(160)? as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let b = idx + i * 216;
        let name = m.cstr(Rd::rel(b, m.i32(b)?)?)?;
        let parent = m.i32(b + 4)?;
        let q = Quat::from_xyzw(
            m.f32(b + 44)?,
            m.f32(b + 48)?,
            m.f32(b + 52)?,
            m.f32(b + 56)?,
        );
        let mut r = [[0f32; 4]; 3];
        for (row, rv) in r.iter_mut().enumerate() {
            for (col, c) in rv.iter_mut().enumerate() {
                *c = m.f32(b + 96 + (row * 4 + col) * 4)?;
            }
        }
        let p2b = Mat4::from_cols_array(&[
            r[0][0], r[1][0], r[2][0], 0.0, r[0][1], r[1][1], r[2][1], 0.0, r[0][2], r[1][2],
            r[2][2], 0.0, r[0][3], r[1][3], r[2][3], 1.0,
        ]);
        out.push(Bone {
            name,
            parent,
            pos: m.vec3(b + 32)?,
            quat: if q.length_squared() > 1e-6 {
                q.normalize()
            } else {
                Quat::IDENTITY
            },
            rot: m.vec3(b + 60)?,
            pos_scale: m.vec3(b + 72)?,
            rot_scale: m.vec3(b + 84)?,
            pose_to_bone: p2b,
            flags: m.i32(b + 160)?,
            proc_type: m.i32(b + 164)?,
            quat_interp: parse_quat_interp(m, b, n).ok().flatten(),
        });
    }
    Ok(out)
}

/// Names of `$includemodel` MDL paths referenced by an MDL (lowercase, `/` separated).
pub fn include_model_names(mdl: &[u8]) -> Result<Vec<String>> {
    let m = check_header(mdl)?;
    let n = m.i32(336)? as usize;
    let idx = m.i32(340)? as usize;
    let mut v = Vec::new();
    for i in 0..n {
        let g = idx + i * 8;
        let name = m.cstr(Rd::rel(g, m.i32(g + 4)?)?)?;
        let name = name.to_ascii_lowercase().replace('\\', "/");
        if !name.is_empty() {
            v.push(name);
        }
    }
    Ok(v)
}

/// Name of the external anim-block file (`.ani`) an MDL refers to, if any.
pub fn anim_block_name(mdl: &[u8]) -> Result<Option<String>> {
    let m = check_header(mdl)?;
    if m.i32(352)? <= 0 {
        return Ok(None);
    }
    let off = m.i32(348)?;
    if off <= 0 {
        return Ok(None);
    }
    let s = m.cstr(off as usize)?;
    Ok(if s.is_empty() {
        None
    } else {
        Some(s.to_ascii_lowercase().replace('\\', "/"))
    })
}

impl Skeleton {
    /// Parse a single MDL (no `$includemodel` resolution). `ani` = bytes of its `.ani` file, if it uses anim blocks.
    pub fn parse(mdl: &[u8], ani: Option<&[u8]>) -> Result<Skeleton> {
        let m = check_header(mdl)?;
        let bones = parse_bones(&m)?;
        let hitboxes = parse_hitboxes(&m, bones.len())?;
        let attachments = parse_attachments(&m);
        let ik_chains = ik::parse_chains(&m, bones.len())?;
        let ik_autoplay_locks = ik::parse_locks(
            &m,
            m.i32(320)?,
            m.i32(324)? as usize,
            &ik_chains,
            &ik_chains,
        )?;
        let mut sk = Skeleton {
            bones,
            hitboxes,
            attachments,
            ik_chains,
            ik_autoplay_locks,
            sources: Vec::new(),
            sequences: Vec::new(),
            anims: Vec::new(),
            pose_params: Vec::new(),
        };
        sk.add_source("<main>", mdl, ani)?;
        Ok(sk)
    }

    /// Parse an MDL and recursively pull in its `$includemodel` MDLs (and every `.ani`) through `read`
    /// (path relative to the game root, e.g. "models/survivors/anim_coach.mdl"; return None if missing).
    pub fn load_with(mdl_path: &str, read: impl Fn(&str) -> Option<Vec<u8>>) -> Result<Skeleton> {
        let mdl = read(mdl_path).ok_or_else(|| anyhow!("cannot read {}", mdl_path))?;
        let ani = Self::read_ani(&mdl, mdl_path, &read);
        let mut sk = Skeleton::parse(&mdl, ani.as_deref())?;
        sk.sources[0].name = mdl_path.to_string();
        let mut seen = vec![mdl_path.to_ascii_lowercase()];
        let mut queue: Vec<String> = include_model_names(&mdl)?;
        queue.reverse();
        // depth-first in include order, like the engine's virtual model
        while let Some(p) = queue.pop() {
            if seen.contains(&p) || seen.len() > 64 {
                continue;
            }
            seen.push(p.clone());
            let Some(inc) = read(&p) else {
                log::warn!("anim: include model {} not found", p);
                continue;
            };
            let ani = Self::read_ani(&inc, &p, &read);
            if let Err(e) = sk.add_source(&p, &inc, ani.as_deref()) {
                log::warn!("anim: include model {} failed: {}", p, e);
                continue;
            }
            if let Ok(mut nested) = include_model_names(&inc) {
                nested.reverse();
                queue.extend(nested);
            }
        }
        Ok(sk)
    }

    fn read_ani(
        mdl: &[u8],
        mdl_path: &str,
        read: &impl Fn(&str) -> Option<Vec<u8>>,
    ) -> Option<Vec<u8>> {
        // Only fetch when the MDL actually has anim blocks.
        match anim_block_name(mdl) {
            Ok(Some(n)) => {
                read(&n).or_else(|| read(&format!("{}.ani", mdl_path.trim_end_matches(".mdl"))))
            }
            _ => None,
        }
    }

    /// Add a further MDL as an animation source (an `$includemodel`). Its bones are matched to this
    /// skeleton by name. Safe to call once per include, in include order.
    pub fn add_include(&mut self, name: &str, mdl: &[u8], ani: Option<&[u8]>) -> Result<()> {
        self.add_source(name, mdl, ani)
    }

    fn add_source(&mut self, name: &str, mdl: &[u8], ani: Option<&[u8]>) -> Result<()> {
        let m = check_header(mdl)?;
        let source = self.sources.len();
        let own_bones = if source == 0 {
            self.bones.clone()
        } else {
            parse_bones(&m)?
        };
        let bones: Vec<SrcBone> = own_bones
            .iter()
            .enumerate()
            .map(|(i, b)| SrcBone {
                name: b.name.clone(),
                quat: b.quat,
                pos: b.pos,
                rot: b.rot,
                pos_scale: b.pos_scale,
                rot_scale: b.rot_scale,
                main: if source == 0 {
                    Some(i)
                } else {
                    self.bones
                        .iter()
                        .position(|mb| mb.name.eq_ignore_ascii_case(&b.name))
                },
            })
            .collect();

        let local_chains = ik::parse_chains(&m, own_bones.len())?;
        // anim blocks
        let nblocks = m.i32(352)?.max(0) as usize;
        let bidx = m.i32(356)?.max(0) as usize;
        let mut blocks = Vec::new();
        for i in 0..nblocks {
            blocks.push((
                m.i32(bidx + i * 8)?.max(0) as usize,
                m.i32(bidx + i * 8 + 4)?.max(0) as usize,
            ));
        }
        let ani_name = anim_block_name(mdl)?.unwrap_or_default();

        // anim descs
        let nanim = m.i32(180)?.max(0) as usize;
        let aidx = m.i32(184)?.max(0) as usize;
        let anim_base = self.anims.len();
        for i in 0..nanim {
            let a = aidx + i * 100;
            self.anims.push(AnimDesc {
                name: m.cstr(Rd::rel(a, m.i32(a + 4)?)?)?,
                source,
                fps: m.f32(a + 8)?,
                flags: m.i32(a + 12)?,
                num_frames: m.i32(a + 16)?,
                anim_block: m.i32(a + 52)?,
                anim_index: m.i32(a + 56)?,
                section_index: m.i32(a + 80)?,
                section_frames: m.i32(a + 84)?,
                base: a,
            });
        }

        // pose parameters (of this MDL; sequences refer to them by index)
        let nposeparams = m.i32(300)?.max(0) as usize;
        let pp_idx = m.i32(304)?.max(0) as usize;
        ensure!(nposeparams <= 1024, "invalid pose parameter count");
        for i in 0..nposeparams {
            let pd = pp_idx + i * 20;
            let name = m.cstr(Rd::rel(pd, m.i32(pd)?)?)?;
            // Virtual-model includes can introduce movement/lean axes
            // absent from the root MDL. Preserve the main/first descriptor
            // when later includes share a parameter name.
            if self
                .pose_params
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            self.pose_params.push(PoseParam {
                name,
                flags: m.i32(pd + 4)?,
                start: m.f32(pd + 8)?,
                end: m.f32(pd + 12)?,
                loop_: m.f32(pd + 16)?,
            });
        }

        // sequences
        let nseq = m.i32(188)?.max(0) as usize;
        let sidx = m.i32(192)?.max(0) as usize;
        let sequence_base = self.sequences.len();
        for i in 0..nseq {
            let s = sidx + i * 212;
            let label = m.cstr(Rd::rel(s, m.i32(s + 4)?)?)?;
            let act_off = m.i32(s + 8)?;
            let activity_name = if act_off != 0 {
                m.cstr(Rd::rel(s, act_off)?)?
            } else {
                String::new()
            };
            let gs = [m.i32(s + 68)?.max(1), m.i32(s + 72)?.max(1)];
            let nblend_total = (gs[0] * gs[1]) as usize;
            let ai_idx = Rd::rel(s, m.i32(s + 60)?)?;
            let mut blend_anims = Vec::new();
            for k in 0..nblend_total.min(1024) {
                let a = m.i16(ai_idx + k * 2)?;
                blend_anims.push(if a >= 0 && (a as usize) < nanim {
                    anim_base + a as usize
                } else {
                    usize::MAX
                });
            }
            let wl_off = m.i32(s + 156)?;
            let mut weight_list = Vec::new();
            if wl_off != 0 {
                let wl = Rd::rel(s, wl_off)?;
                for b in 0..own_bones_len(&m)? {
                    let w = m.f32(wl + b * 4)?;
                    if let Some(mb) = bones.get(b).and_then(|x| x.main) {
                        if weight_list.len() <= mb {
                            weight_list.resize(self.bones.len(), 0.0);
                        }
                        weight_list[mb] = w;
                    }
                }
            }
            let mut blend_params: [Option<(String, f32, f32)>; 2] = [None, None];
            for ax in 0..2 {
                let pi = m.i32(s + 76 + ax * 4)?;
                if pi >= 0 && (pi as usize) < nposeparams {
                    let pd = pp_idx + pi as usize * 20;
                    let name = m.cstr(Rd::rel(pd, m.i32(pd)?)?)?;
                    blend_params[ax] =
                        Some((name, m.f32(s + 84 + ax * 4)?, m.f32(s + 92 + ax * 4)?));
                }
            }
            let mut events = Vec::new();
            let (nev, eidx) = (
                m.i32(s + 24)?.max(0) as usize,
                m.i32(s + 28)?.max(0) as usize,
            );
            for e in 0..nev.min(256) {
                let eo = s + eidx + e * 80;
                if let (Ok(cycle), Ok(event)) = (m.f32(eo), m.i32(eo + 4)) {
                    let mut options = m.cstr(eo + 12).unwrap_or_default();
                    // szeventindex (relative to the event struct) names the event for `event == 0` (v49 string events).
                    if let Ok(si) = m.i32(eo + 76) {
                        if si != 0 {
                            if let Some(n) = Rd::rel(eo, si).ok().and_then(|o| m.cstr(o).ok()) {
                                options = if options.is_empty() {
                                    n
                                } else {
                                    format!("{n} {options}")
                                };
                            }
                        }
                    }
                    events.push(SeqEvent {
                        cycle,
                        event,
                        options,
                    });
                }
            }
            let num_autolayers = m.i32(s + 148)?;
            ensure!(
                (0..=1024).contains(&num_autolayers),
                "invalid autolayer count"
            );
            let mut autolayers = Vec::new();
            if num_autolayers > 0 {
                let layer_base = Rd::rel(s, m.i32(s + 152)?)?;
                for k in 0..num_autolayers as usize {
                    let at = layer_base + k * 24;
                    let target = m.i16(at)?;
                    if target < 0 || target as usize >= nseq {
                        continue;
                    }
                    let pi = m.i16(at + 2)?;
                    let pose = if pi >= 0 && (pi as usize) < nposeparams {
                        let pd = pp_idx + pi as usize * 20;
                        Some(m.cstr(Rd::rel(pd, m.i32(pd)?)?)?)
                    } else {
                        None
                    };
                    autolayers.push(AutoLayer {
                        sequence: sequence_base + target as usize,
                        pose,
                        flags: m.i32(at + 4)?,
                        start: m.f32(at + 8)?,
                        peak: m.f32(at + 12)?,
                        tail: m.f32(at + 16)?,
                        end: m.f32(at + 20)?,
                    });
                }
            }
            self.sequences.push(Sequence {
                label,
                activity_name,
                activity: m.i32(s + 16)?,
                activity_weight: m.i32(s + 20)?,
                flags: m.i32(s + 12)?,
                source,
                num_blends: m.i32(s + 56)?,
                group_size: gs,
                blend_anims,
                weight_list,
                local_entry_node: m.i32(s + 112)?,
                local_exit_node: m.i32(s + 116)?,
                node_flags: m.i32(s + 120)?,
                fade_in: m.f32(s + 104)?,
                fade_out: m.f32(s + 108)?,
                bbmin: m.vec3(s + 32)?,
                bbmax: m.vec3(s + 44)?,
                num_events: m.i32(s + 24)?,
                events,
                num_autolayers,
                autolayers,
                ik_locks: ik::parse_locks(
                    &m,
                    m.i32(s + 164)?,
                    Rd::rel(s, m.i32(s + 168)?)?,
                    &local_chains,
                    &self.ik_chains,
                )?,
                blend_params,
            });
        }

        self.sources.push(Source {
            name: name.to_string(),
            mdl: shared_bytes(name, mdl),
            ani: ani.map(|a| shared_bytes(&format!("{name}#ani"), a)),
            blocks,
            bones,
            ani_name,
            chains: local_chains,
        });
        Ok(())
    }

    // -----------------------------------------------------------------------------------------
    // queries
    // -----------------------------------------------------------------------------------------

    /// Model-space transform of attachment `name` ("Attachment_1" style names, case-insensitive) for a bone-to-model `pose`.
    pub fn attachment_matrix(&self, name: &str, pose: &[Mat4]) -> Option<Mat4> {
        let a = self
            .attachments
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))?;
        Some(*pose.get(a.bone)? * a.local)
    }

    pub fn sequences(&self) -> &[Sequence] {
        &self.sequences
    }
    pub fn sequence_names(&self) -> Vec<&str> {
        self.sequences.iter().map(|s| s.label.as_str()).collect()
    }
    /// Pose parameters declared by the main MDL (e.g. `move_x`, `move_y`, `body_pitch`).
    pub fn pose_params(&self) -> &[PoseParam] {
        &self.pose_params
    }
    pub fn anims(&self) -> &[AnimDesc] {
        &self.anims
    }
    /// Names (paths) of the MDLs sequences come from; index 0 is the main model.
    pub fn source_names(&self) -> Vec<&str> {
        self.sources.iter().map(|s| s.name.as_str()).collect()
    }
    /// Name of the `.ani` file an animation source expects (if it has anim blocks), and whether it was supplied.
    pub fn ani_status(&self, source: usize) -> Option<(&str, bool)> {
        let s = self.sources.get(source)?;
        if s.blocks.is_empty() {
            None
        } else {
            Some((s.ani_name.as_str(), s.ani.is_some()))
        }
    }
    /// Bone names of an animation source MDL (index = bone id used by that source's animation data).
    pub fn source_bone_names(&self, source: usize) -> Vec<&str> {
        self.sources
            .get(source)
            .map(|s| s.bones.iter().map(|b| b.name.as_str()).collect())
            .unwrap_or_default()
    }
    /// Bind (pos, quat) of an animation source MDL's own bones.
    pub fn source_bone_bind(&self, source: usize) -> Vec<(Vec3, Quat)> {
        self.sources
            .get(source)
            .map(|s| s.bones.iter().map(|b| (b.pos, b.quat)).collect())
            .unwrap_or_default()
    }
    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.bones
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
    }

    /// Find a sequence by label or activity name (case-insensitive). Label matches win; among
    /// activity matches the one with the highest `activity_weight` (then lowest index) is returned.
    pub fn find_sequence(&self, name: &str) -> Option<usize> {
        if let Some(i) = self
            .sequences
            .iter()
            .position(|s| s.label.eq_ignore_ascii_case(name))
        {
            return Some(i);
        }
        let mut best: Option<usize> = None;
        for (i, s) in self.sequences.iter().enumerate() {
            if !s.activity_name.is_empty() && s.activity_name.eq_ignore_ascii_case(name) {
                if best.map_or(true, |b| {
                    s.activity_weight > self.sequences[b].activity_weight
                }) {
                    best = Some(i);
                }
            }
        }
        best
    }

    /// All sequence indices whose label or activity name contains `needle` (case-insensitive).
    pub fn find_sequences_containing(&self, needle: &str) -> Vec<usize> {
        let n = needle.to_ascii_lowercase();
        self.sequences
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.label.to_ascii_lowercase().contains(&n)
                    || s.activity_name.to_ascii_lowercase().contains(&n)
            })
            .map(|(i, _)| i)
            .collect()
    }

    // -----------------------------------------------------------------------------------------
    // evaluation
    // -----------------------------------------------------------------------------------------

    /// True if the anim is known to carry no per-frame data (a constant pose, e.g. the "Zero" blend of
    /// L4D2 locomotion grids). Only detectable for `STUDIO_FRAMEANIM` data; RLE anims report false.
    pub fn anim_is_static(&self, anim: usize) -> bool {
        let Some(a) = self.anims.get(anim) else {
            return true;
        };
        if a.num_frames <= 1 || a.flags & STUDIO_ALLZEROS != 0 {
            return true;
        }
        if a.flags & STUDIO_FRAMEANIM == 0 {
            return false;
        }
        match self.locate(a, 0) {
            Ok((buf, off, _)) => Rd(buf)
                .i32(off + 8)
                .map(|framelen| framelen == 0)
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// The anim desc used for sequence `seq`: the "center" blend of the blend grid (pose parameters
    /// are not evaluated). If the center is a static "zero" pose while other blends move (L4D2 locomotion
    /// grids put the standing "Zero" pose in the middle), the middle of the first row (e.g. "forward") is used,
    /// then the first non-static blend. Use `pose_sequence_blend` for real 2D blending.
    pub fn sequence_anim(&self, seq: usize) -> Option<usize> {
        let s = self.sequences.get(seq)?;
        let valid = |i: usize| s.blend_anims.get(i).copied().filter(|&a| a != usize::MAX);
        let (g0, g1) = (s.group_size[0] as usize, s.group_size[1] as usize);
        let center = g0 / 2 + (g1 / 2) * g0;
        let first_row_mid = g0 / 2;
        let candidates = [valid(center), valid(first_row_mid)];
        if s.blend_anims.len() > 1 {
            for c in candidates.iter().flatten() {
                if !self.anim_is_static(*c) {
                    return Some(*c);
                }
            }
            if let Some(a) = s
                .blend_anims
                .iter()
                .copied()
                .find(|&a| a != usize::MAX && !self.anim_is_static(a))
            {
                return Some(a);
            }
        }
        candidates[0].or_else(|| s.blend_anims.iter().copied().find(|&a| a != usize::MAX))
    }
    /// Authored root movement over one loop of animation `index` (Source
    /// model space: x forward, y left, z up), from its last movement block.
    pub fn anim_movement(&self, index: usize) -> Option<Vec3> {
        let a = self.anims.get(index)?;
        let m = Rd(&self.sources[a.source].mdl);
        let count = m.i32(a.base + 20).ok()?;
        let offset = m.i32(a.base + 24).ok()?;
        if count <= 0 || count > 4096 || offset <= 0 {
            return None;
        }
        let at = a.base + offset as usize + (count as usize - 1) * 44 + 32;
        Some(Vec3::new(m.f32(at).ok()?, m.f32(at + 4).ok()?, m.f32(at + 8).ok()?))
    }
    /// Source `Studio_AnimPosition`: the extracted root position (model
    /// space: x forward, y left, z up) and yaw in degrees at `cycle` (0..1)
    /// of animation `index`, from its piecewise movement blocks
    /// (mstudiomovement_t: endframe, motionflags, v0, v1, angle, vector,
    /// position). Animations without movement stay at the origin.
    pub fn anim_position(&self, index: usize, cycle: f32) -> (Vec3, f32) {
        let Some(a) = self.anims.get(index) else {
            return (Vec3::ZERO, 0.);
        };
        let m = Rd(&self.sources[a.source].mdl);
        let count = m.i32(a.base + 20).unwrap_or(0);
        let offset = m.i32(a.base + 24).unwrap_or(0);
        if count <= 0 || count > 4096 || offset <= 0 {
            return (Vec3::ZERO, 0.);
        }
        let frame = cycle.clamp(0., 1.) * (a.num_frames - 1).max(0) as f32;
        let mut previous = 0f32;
        let mut position = Vec3::ZERO;
        let mut yaw = 0f32;
        for i in 0..count as usize {
            let at = a.base + offset as usize + i * 44;
            let read = |o: usize| m.f32(at + o).unwrap_or(0.);
            let end = m.i32(at).unwrap_or(0) as f32;
            if end >= frame {
                let f = if end > previous { (frame - previous) / (end - previous) } else { 0. };
                let (v0, v1) = (read(8), read(12));
                let d = v0 * f + 0.5 * (v1 - v0) * f * f;
                position += d * Vec3::new(read(20), read(24), read(28));
                yaw = yaw * (1. - f) + read(16) * f;
                return (position, yaw);
            }
            previous = end;
            position = Vec3::new(read(32), read(36), read(40));
            yaw = read(16);
        }
        (position, yaw)
    }
    /// Authored root movement distance per second, in Source units.
    pub fn sequence_ground_speed(&self, seq: usize) -> f32 {
        let Some(index) = self.sequence_anim(seq) else {
            return 0.;
        };
        let a = &self.anims[index];
        if a.duration() <= 0. {
            return 0.;
        }
        let speed = self.anim_displacement(index).length() / a.duration();
        if speed.is_finite() {
            speed
        } else {
            0.
        }
    }
    /// Whole-clip root displacement of one animation (its last movement block).
    fn anim_displacement(&self, index: usize) -> Vec3 {
        let a = &self.anims[index];
        let m = Rd(&self.sources[a.source].mdl);
        let count = m.i32(a.base + 20).unwrap_or(0);
        let offset = m.i32(a.base + 24).unwrap_or(0);
        if count <= 0 || count > 4096 || offset <= 0 {
            return Vec3::ZERO;
        }
        let at = a.base + offset as usize + (count as usize - 1) * 44 + 32;
        Vec3::new(
            m.f32(at).unwrap_or(0.),
            m.f32(at + 4).unwrap_or(0.),
            m.f32(at + 8).unwrap_or(0.),
        )
    }
    /// Ground speed of a (possibly blended) sequence at these pose-parameter
    /// values, as Source's GetSequenceGroundSpeed: the bilinearly weighted
    /// root displacement (Studio_SeqMovement) times the weighted cycle rate
    /// (Studio_Duration).
    pub fn sequence_ground_speed_params(&self, seq: usize, values: &[(&str, f32)]) -> f32 {
        let Some(s) = self.sequences.get(seq) else {
            return 0.;
        };
        if s.blend_anims.len() <= 1 {
            return self.sequence_ground_speed(seq);
        }
        let params = self.blend_axes(seq, values);
        let (g0, g1) = (s.group_size[0].max(1) as usize, s.group_size[1].max(1) as usize);
        let axis = |p: f32, g: usize| -> (usize, usize, f32) {
            let x = p.clamp(0.0, 1.0) * (g - 1) as f32;
            let i = (x.floor() as usize).min(g - 1);
            (i, (i + 1).min(g - 1), x - i as f32)
        };
        let (c0, c1, fu) = axis(params[0], g0);
        let (r0, r1, fv) = axis(params[1], g1);
        let (mut displacement, mut rate) = (Vec3::ZERO, 0.);
        for (c, r, w) in [
            (c0, r0, (1.0 - fu) * (1.0 - fv)),
            (c1, r0, fu * (1.0 - fv)),
            (c0, r1, (1.0 - fu) * fv),
            (c1, r1, fu * fv),
        ] {
            let Some(&a) = s.blend_anims.get(c + r * g0) else { continue };
            if a == usize::MAX || w <= 0. {
                continue;
            }
            displacement += self.anim_displacement(a) * w;
            let d = self.anims[a].duration();
            if d > 0. {
                rate += w / d;
            }
        }
        let speed = displacement.length() * rate;
        if speed.is_finite() { speed } else { 0. }
    }

    /// Like `pose_sequence_blend`, but takes pose-parameter values by name (e.g. `&[("move_x", 0.0), ("move_y", 1.0)]`);
    /// values are mapped through the sequence's start/end range. Unlisted parameters default to the middle of their range.
    pub fn pose_sequence_params(
        &self,
        seq: usize,
        time: f32,
        looping: bool,
        values: &[(&str, f32)],
    ) -> Vec<Mat4> {
        let p = self.blend_axes(seq, values);
        self.layered_pose(seq, self.sequence_cycle(seq, time, looping), p, values)
    }

    fn blend_axes(&self, seq: usize, values: &[(&str, f32)]) -> [f32; 2] {
        let Some(s) = self.sequences.get(seq) else {
            return [0.5; 2];
        };
        let mut p = [0.5f32; 2];
        for ax in 0..2 {
            if let Some((name, start, end)) = &s.blend_params[ax] {
                if let Some((_, v)) = values.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
                    p[ax] = if (end - start).abs() > 1e-6 {
                        (v - start) / (end - start)
                    } else {
                        0.0
                    };
                }
            }
        }
        p
    }

    /// 2D blend over the sequence's blend grid. `params` are the two pose-parameter values normalized to
    /// `0..=1` across their range (axis 0 selects the column, axis 1 the row of `blend_anims`
    /// = `col + row * group_size[0]`). Cells are bilinearly weighted; all frames use the same cycle phase.
    pub fn pose_sequence_blend(
        &self,
        seq: usize,
        time: f32,
        looping: bool,
        params: [f32; 2],
    ) -> Vec<Mat4> {
        let values: Vec<(&str, f32)> = self
            .sequences
            .get(seq)
            .into_iter()
            .flat_map(|s| s.blend_params.iter().enumerate())
            .filter_map(|(ax, p)| {
                p.as_ref()
                    .map(|(n, start, end)| (n.as_str(), start + params[ax] * (end - start)))
            })
            .collect();
        self.layered_pose(
            seq,
            self.sequence_cycle(seq, time, looping),
            params,
            &values,
        )
    }

    fn sequence_cycle(&self, seq: usize, time: f32, looping: bool) -> f32 {
        let duration = self
            .sequence_anim(seq)
            .map_or(0., |a| self.anims[a].duration());
        if duration <= 0. || !time.is_finite() {
            0.
        } else if looping {
            (time / duration).rem_euclid(1.)
        } else {
            (time / duration).clamp(0., 1.)
        }
    }

    fn sample_blend(&self, seq: usize, phase: f32, params: [f32; 2]) -> Option<Vec<(Vec3, Quat)>> {
        let s = self.sequences.get(seq)?;
        let (g0, g1) = (
            s.group_size[0].max(1) as usize,
            s.group_size[1].max(1) as usize,
        );
        let axis = |p: f32, g: usize| -> (usize, usize, f32) {
            let x = p.clamp(0.0, 1.0) * (g - 1) as f32;
            let i = (x.floor() as usize).min(g - 1);
            (i, (i + 1).min(g - 1), x - i as f32)
        };
        let (c0, c1, fu) = axis(params[0], g0);
        let (r0, r1, fv) = axis(params[1], g1);
        let mut cells: Vec<(usize, f32)> = Vec::new();
        for (c, r, w) in [
            (c0, r0, (1.0 - fu) * (1.0 - fv)),
            (c1, r0, fu * (1.0 - fv)),
            (c0, r1, (1.0 - fu) * fv),
            (c1, r1, fu * fv),
        ] {
            if w <= 1e-4 {
                continue;
            }
            if let Some(&a) = s.blend_anims.get(c + r * g0) {
                if a != usize::MAX {
                    cells.push((a, w));
                }
            }
        }
        if cells.is_empty() {
            let a = self.sequence_anim(seq)?;
            return self
                .local_pose(a, phase * (self.anims[a].num_frames - 1).max(0) as f32)
                .ok();
        }
        let mut acc: Option<(Vec<(Vec3, Quat)>, f32)> = None;
        for (a, w) in cells {
            let n = self.anims[a].num_frames.max(1);
            let Ok(l) = self.local_pose(a, phase * (n - 1) as f32) else {
                continue;
            };
            acc = Some(match acc {
                None => (l, w),
                Some((cur, cw)) => {
                    let t = w / (cw + w);
                    (
                        cur.into_iter()
                            .zip(l)
                            .map(|((pa, qa), (pb, qb))| (pa.lerp(pb, t), qa.slerp(qb, t)))
                            .collect(),
                        cw + w,
                    )
                }
            });
        }
        acc.map(|(locals, _)| locals)
    }

    fn layered_pose(
        &self,
        seq: usize,
        cycle: f32,
        params: [f32; 2],
        values: &[(&str, f32)],
    ) -> Vec<Mat4> {
        let Some(mut locals) = self.sample_blend(seq, cycle, params) else {
            return self.bind_pose();
        };
        self.add_layers(&mut locals, seq, cycle, 1., values, &mut vec![seq], true);
        self.add_layers(&mut locals, seq, cycle, 1., values, &mut vec![seq], false);
        self.world_from_local(&locals)
    }

    // Evaluate native local layers before blending their parent; ordinary layers
    // accumulate afterwards. A path guard rejects cycles without suppressing
    // repeated, intentional layers on separate branches.
    fn add_layers(
        &self,
        locals: &mut [(Vec3, Quat)],
        seq: usize,
        cycle: f32,
        weight: f32,
        values: &[(&str, f32)],
        path: &mut Vec<usize>,
        local: bool,
    ) {
        let s = &self.sequences[seq];
        if local && s.flags & STUDIO_LOCAL == 0 {
            return;
        }
        for layer in &s.autolayers {
            if (layer.flags & AL_LOCAL != 0) != local {
                continue;
            }
            if let Some((phase, strength)) = layer.sample(cycle, weight, values) {
                self.accumulate_sequence(locals, layer.sequence, phase, strength, values, path);
            }
        }
    }

    fn accumulate_sequence(
        &self,
        locals: &mut [(Vec3, Quat)],
        seq: usize,
        cycle: f32,
        weight: f32,
        values: &[(&str, f32)],
        path: &mut Vec<usize>,
    ) {
        if weight <= 0. || path.len() >= 16 || path.contains(&seq) {
            return;
        }
        let Some(mut overlay) = self.sample_blend(seq, cycle, self.blend_axes(seq, values)) else {
            return;
        };
        path.push(seq);
        self.add_layers(&mut overlay, seq, cycle, 1., values, path, true);
        let s = &self.sequences[seq];
        let locked_pose = if s.ik_locks.is_empty() {
            None
        } else {
            Some(self.world_from_local(locals))
        };
        let delta = s.is_delta();
        for (i, ((position, rotation), (lp, lq))) in locals.iter_mut().zip(overlay).enumerate() {
            let strength = weight * s.weight_list.get(i).copied().unwrap_or(1.).clamp(0., 1.);
            if strength <= 0. {
                continue;
            }
            if delta {
                *position += (lp - self.bones[i].pos) * strength;
                let dq = Quat::IDENTITY.slerp(self.bones[i].quat.inverse() * lq, strength);
                *rotation = if s.flags & STUDIO_POST != 0 {
                    *rotation * dq
                } else {
                    dq * *rotation
                }
                .normalize();
            } else {
                *position = position.lerp(lp, strength);
                *rotation = rotation.slerp(lq, strength);
            }
        }
        self.add_layers(locals, seq, cycle, weight, values, path, false);
        if let Some(before) = locked_pose {
            let mut after = self.world_from_local(locals);
            self.apply_ik_locks(&before, &mut after, &s.ik_locks);
            for (i, bone) in self.bones.iter().enumerate() {
                let local = if bone.parent < 0 {
                    after[i]
                } else {
                    after[bone.parent as usize].inverse() * after[i]
                };
                let (_, q, p) = local.to_scale_rotation_translation();
                locals[i] = (p, q.normalize());
            }
        }
        path.pop();
    }

    /// Bone-to-model matrices for sequence `seq` at `time` seconds. `looping` wraps time around the cycle,
    /// otherwise it is clamped to the last frame. Bones without data keep their bind pose.
    pub fn pose_sequence(&self, seq: usize, time: f32, looping: bool) -> Vec<Mat4> {
        self.pose_sequence_params(seq, time, looping, &[])
    }

    /// Alias of `pose_sequence`.
    pub fn pose(&self, seq: usize, time: f32, looping: bool) -> Vec<Mat4> {
        self.pose_sequence(seq, time, looping)
    }

    /// Bone-to-model matrices for the bind pose (== inverse of every `pose_to_bone`).
    pub fn bind_pose(&self) -> Vec<Mat4> {
        let local: Vec<(Vec3, Quat)> = self.bones.iter().map(|b| (b.pos, b.quat)).collect();
        self.world_from_local(&local)
    }

    /// Pose for a specific anim desc (index into `anims()`).
    pub fn pose_anim(&self, anim: usize, time: f32, looping: bool) -> Vec<Mat4> {
        let Some(a) = self.anims.get(anim) else {
            return self.bind_pose();
        };
        let n = a.num_frames.max(1) as usize;
        let mut f = time * a.fps;
        if n <= 1 {
            f = 0.0;
        } else {
            let last = (n - 1) as f32;
            f = if looping {
                f.rem_euclid(last)
            } else {
                f.clamp(0.0, last)
            };
        }
        self.pose_anim_frame(anim, f)
    }

    /// Pose at a fractional frame (clamped to `0..=numframes-1`), interpolating between neighbours.
    pub fn pose_anim_frame(&self, anim: usize, frame: f32) -> Vec<Mat4> {
        match self.local_pose(anim, frame) {
            Ok(l) => self.world_from_local(&l),
            Err(e) => {
                log::warn!("anim: pose failed for anim {}: {}", anim, e);
                self.bind_pose()
            }
        }
    }

    /// Local (parent-relative) pos/quat of every bone. Errors are returned (unlike `pose_*`, which fall back to bind).
    pub fn local_pose(&self, anim: usize, frame: f32) -> Result<Vec<(Vec3, Quat)>> {
        let a = self
            .anims
            .get(anim)
            .ok_or_else(|| anyhow!("no anim {}", anim))?;
        let n = a.num_frames.max(1);
        let f = frame.clamp(0.0, (n - 1) as f32);
        let i0 = f.floor() as i32;
        let s = f - i0 as f32;
        let p0 = self.decode_frame(a, i0)?;
        if s < 1e-4 || i0 + 1 >= n {
            return Ok(p0);
        }
        let p1 = self.decode_frame(a, i0 + 1)?;
        Ok(p0
            .into_iter()
            .zip(p1)
            .map(|((pa, qa), (pb, qb))| (pa.lerp(pb, s), qa.slerp(qb, s)))
            .collect())
    }

    fn world_from_local(&self, local: &[(Vec3, Quat)]) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let (p, q) = local[i];
            let mut m = Mat4::from_rotation_translation(q, p);
            // Procedural helper bones follow their control bone's rotation.
            if let Some(proc_) = &b.quat_interp {
                let control = proc_.control;
                let control_parent = self.bones.get(control).map_or(-1, |c| c.parent);
                if control < i && control_parent >= 0 && (control_parent as usize) < i {
                    let rel = out[control_parent as usize].inverse() * out[control];
                    let (_, rotation, _) = rel.to_scale_rotation_translation();
                    if let Some(local) = quat_interp_local(proc_, rotation) {
                        m = local;
                    }
                }
            }
            out.push(if b.parent >= 0 && (b.parent as usize) < i {
                out[b.parent as usize] * m
            } else {
                m
            });
        }
        out
    }
    /// Evaluate a viewmodel's authored action layer over its idle base.
    /// The decoder represents delta animations relative to bind pose, so remove
    /// that bind contribution before adding the layer to the base animation.
    fn view_idle_sequence(&self) -> Option<usize> {
        self.find_sequence("ACT_VM_IDLE").or_else(|| {
            self.sequences.iter().position(|s| {
                s.activity_name.starts_with("ACT_VM_IDLE_") && !s.activity_name.contains("LAYER")
            })
        })
    }
    /// Studio_CPS: cycles per second of `seq` blended by pose parameters
    /// (a weighted average of each blend's fps / (frames - 1)).
    pub fn sequence_cycle_rate_params(&self, seq: usize, values: &[(&str, f32)]) -> f32 {
        let Some(s) = self.sequences.get(seq) else {
            return 0.;
        };
        let params = self.blend_axes(seq, values);
        let (g0, g1) = (s.group_size[0].max(1) as usize, s.group_size[1].max(1) as usize);
        let axis = |p: f32, g: usize| -> (usize, usize, f32) {
            let x = p.clamp(0.0, 1.0) * (g - 1) as f32;
            let i = (x.floor() as usize).min(g - 1);
            (i, (i + 1).min(g - 1), x - i as f32)
        };
        let (c0, c1, fu) = axis(params[0], g0);
        let (r0, r1, fv) = axis(params[1], g1);
        let (mut rate, mut total) = (0., 0.);
        for (c, r, w) in [
            (c0, r0, (1.0 - fu) * (1.0 - fv)),
            (c1, r0, fu * (1.0 - fv)),
            (c0, r1, (1.0 - fu) * fv),
            (c1, r1, fu * fv),
        ] {
            let Some(a) = s.blend_anims.get(c + r * g0).copied().filter(|&a| a != usize::MAX) else {
                continue;
            };
            let Some(anim) = self.anims.get(a) else { continue };
            if w > 0. && anim.num_frames > 1 {
                rate += w * anim.fps / (anim.num_frames - 1) as f32;
                total += w;
            }
        }
        if total > 0. { rate / total } else { 0. }
    }
    /// `pose_view_sequence` with the viewmodel's pose parameters. L4D2 keeps
    /// the idle as the base sequence under paired action layers; the idle
    /// blends a_idle_1 / a_run by `move_x` at its own `idle_cycle`, and its
    /// look layer reads `ver_aims`. Sequences without an idle base fall back
    /// to `pose_view_sequence`.
    pub fn pose_view_sequence_params(
        &self,
        seq: usize,
        time: f32,
        looping: bool,
        idle_cycle: f32,
        values: &[(&str, f32)],
    ) -> Vec<Mat4> {
        let Some(s) = self.sequences.get(seq) else {
            return self.bind_pose();
        };
        let idle = self.view_idle_sequence();
        let layer_only = s.flags & STUDIO_DELTA != 0 || s.label.ends_with("_layer");
        let action_layer = if layer_only {
            Some(seq)
        } else {
            self.find_sequence(&format!("{}_layer", s.label))
                .or_else(|| self.find_sequence(&format!("{}_LAYER", s.activity_name)))
        };
        let base_seq = if action_layer.is_some() { idle.unwrap_or(seq) } else { seq };
        if Some(base_seq) != idle {
            return self.pose_view_sequence(seq, time, looping);
        }
        let Some(mut locals) = self.sample_blend(base_seq, idle_cycle, self.blend_axes(base_seq, values)) else {
            return self.pose_view_sequence(seq, time, looping);
        };
        let mut path = vec![base_seq];
        self.add_layers(&mut locals, base_seq, idle_cycle, 1., values, &mut path, true);
        self.add_layers(&mut locals, base_seq, idle_cycle, 1., values, &mut path, false);
        if let Some(layer) = action_layer.filter(|_| seq != base_seq) {
            if !self.sequences[base_seq].autolayers.iter().any(|a| a.sequence == layer) {
                let cycle = self.sequence_cycle(seq, time, looping);
                self.accumulate_sequence(&mut locals, layer, cycle, 1., values, &mut path);
            }
        }
        self.world_from_local(&locals)
    }
    pub fn pose_view_sequence(&self, seq: usize, time: f32, looping: bool) -> Vec<Mat4> {
        let Some(s) = self.sequences.get(seq) else {
            return self.bind_pose();
        };
        let layer_only = s.flags & STUDIO_DELTA != 0 || s.label.ends_with("_layer");
        let action_layer = if layer_only {
            Some(seq)
        } else {
            self.find_sequence(&format!("{}_layer", s.label))
                .or_else(|| self.find_sequence(&format!("{}_LAYER", s.activity_name)))
        };
        // L4D2 keeps m_nSequence at idle and selects m_nLayerSequence for
        // paired viewmodel actions. The delta track already contains weapon
        // and arm motion; playing the full action beneath it doubles that motion.
        let base_seq = if action_layer.is_some() {
            self.view_idle_sequence().unwrap_or(seq)
        } else {
            seq
        };
        let Some(base) = self.sequence_anim(base_seq) else {
            return self.bind_pose();
        };
        let phase = |a: &AnimDesc| {
            let d = a.duration();
            if d <= 0. {
                0.
            } else if looping {
                (time / d).rem_euclid(1.)
            } else {
                (time / d).clamp(0., 1.)
            }
        };
        let a = &self.anims[base];
        let cycle = phase(
            self.sequence_anim(seq)
                .and_then(|i| self.anims.get(i))
                .unwrap_or(a),
        );
        let Ok(mut locals) = self.local_pose(base, cycle * (a.num_frames - 1).max(0) as f32) else {
            return self.pose_sequence(seq, time, looping);
        };
        let mut path = vec![base_seq];
        self.add_layers(&mut locals, base_seq, cycle, 1., &[], &mut path, true);
        self.add_layers(&mut locals, base_seq, cycle, 1., &[], &mut path, false);
        if let Some(layer) = action_layer {
            // Paired arm tracks are a viewmodel convention. An explicitly
            // declared native autolayer already contributes the same track.
            if !self.sequences[base_seq]
                .autolayers
                .iter()
                .any(|a| a.sequence == layer)
            {
                self.accumulate_sequence(&mut locals, layer, cycle, 1., &[], &mut path);
            }
        }
        self.world_from_local(&locals)
    }

    /// Layer a weighted, bind-relative gesture over native locomotion.
    pub fn pose_gesture(
        &self,
        base: usize,
        base_time: f32,
        gesture: usize,
        time: f32,
    ) -> Vec<Mat4> {
        let values = [
            ("move_x", 1.),
            ("move_y", 0.),
            ("body_pitch", 0.),
            ("body_yaw", 0.),
            ("lean_yaw", 0.),
        ];
        self.pose_gesture_params(base, base_time, gesture, time, &values)
    }
    pub fn pose_gesture_params(
        &self,
        base: usize,
        base_time: f32,
        gesture: usize,
        time: f32,
        values: &[(&str, f32)],
    ) -> Vec<Mat4> {
        self.pose_layers_params(base, base_time, true, &[(gesture, time, false)], values)
    }
    /// Compose authored weighted clips over a full base pose. Keep each layer's
    /// own cycle/loop policy; partial pin clips must not start from bind pose.
    pub fn pose_layers_params(
        &self,
        base: usize,
        base_time: f32,
        looping: bool,
        layers: &[(usize, f32, bool)],
        values: &[(&str, f32)],
    ) -> Vec<Mat4> {
        let weighted: Vec<_> = layers.iter().map(|&(s, t, l)| (s, t, l, 1.)).collect();
        self.pose_weighted_layers_params(base, base_time, looping, &weighted, values)
    }
    /// `pose_layers_params` with a weight per layer, applied in order
    /// (overlay gesture slots and their fades).
    pub fn pose_weighted_layers_params(
        &self,
        base: usize,
        base_time: f32,
        looping: bool,
        layers: &[(usize, f32, bool, f32)],
        values: &[(&str, f32)],
    ) -> Vec<Mat4> {
        let base_pose = self.pose_sequence_params(base, base_time, looping, values);
        let mut locals = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let local = if b.parent >= 0 {
                base_pose[b.parent as usize].inverse() * base_pose[i]
            } else {
                base_pose[i]
            };
            let (_, rotation, position) = local.to_scale_rotation_translation();
            locals.push((position, rotation));
        }
        for &(sequence, time, looping, weight) in layers {
            self.accumulate_sequence(
                &mut locals,
                sequence,
                self.sequence_cycle(sequence, time, looping),
                weight,
                values,
                &mut vec![],
            );
        }
        self.world_from_local(&locals)
    }
    /// Transfer authored survivor use motion onto the viewmodel's bind rig.
    /// Included world-model clips have different absolute parent offsets;
    /// apply their bind-relative motion and keep the viewmodel camera fixed.
    pub fn pose_retargeted_view_sequence(&self, seq: usize, time: f32, looping: bool) -> Vec<Mat4> {
        let Some(index) = self.sequence_anim(seq) else {
            return self.bind_pose();
        };
        let a = &self.anims[index];
        let activity = &self.sequences[seq].activity_name;
        let world_medical =
            activity.starts_with("ACT_TERROR_HEAL") || activity == "ACT_TERROR_USE_DEFIBRILLATOR";
        if !world_medical
            || a.source == 0
            || !self.sources[a.source]
                .name
                .to_ascii_lowercase()
                .starts_with("models/survivors/")
        {
            return self.pose_view_sequence(seq, time, looping);
        }
        let d = a.duration();
        let cycle = if d <= 0. {
            0.
        } else if looping {
            (time / d).rem_euclid(1.)
        } else {
            (time / d).clamp(0., 1.)
        };
        let Ok(mut locals) = self.local_pose(index, cycle * (a.num_frames - 1).max(0) as f32)
        else {
            return self.bind_pose();
        };
        for source in &self.sources[a.source].bones {
            let Some(main) = source.main else {
                continue;
            };
            let bind = &self.bones[main];
            let (position, rotation) = locals[main];
            locals[main] = (
                bind.pos + bind.quat * (source.quat.inverse() * (position - source.pos)),
                (bind.quat * source.quat.inverse() * rotation).normalize(),
            );
        }
        let mut pose = self.world_from_local(&locals);
        if let Some(camera) = self.bone_index("ValveBiped.Camera") {
            let idle = self.view_idle_sequence().map_or_else(
                || self.bind_pose(),
                |i| self.pose_view_sequence(i, 0., true),
            );
            // World use clips aim at a patient's body rather than the FPS
            // presentation point. Preserve the held-item attachment frame;
            // retain the authored motion of the arms relative to that frame.
            let anchor = self.bone_index("ValveBiped.weapon_bone").unwrap_or(camera);
            let correction = idle[anchor] * pose[anchor].inverse();
            for bone in &mut pose {
                *bone = correction * *bone;
            }
            pose[camera] = idle[camera];
        }
        pose
    }
    /// `pose[i] * poseToBone[i]`: takes a bind-pose model-space vertex to the posed model space.
    pub fn skin_matrices(&self, pose: &[Mat4]) -> Vec<Mat4> {
        self.bones
            .iter()
            .zip(pose)
            .map(|(b, m)| *m * b.pose_to_bone)
            .collect()
    }

    /// Find the chunk list for `frame` of `a`: (buffer, byte offset of first `mstudioanim_t`, frame within section).
    fn locate(&self, a: &AnimDesc, frame: i32) -> Result<(&[u8], usize, i32)> {
        let src = &self.sources[a.source];
        let nframes = a.num_frames.max(1);
        let mut frame = frame.clamp(0, nframes - 1);
        let (block, index) = if a.section_frames != 0 {
            let sec = if nframes > a.section_frames && frame == nframes - 1 {
                frame = 0;
                (nframes / a.section_frames + 1) as usize
            } else {
                let s = frame / a.section_frames;
                frame -= s * a.section_frames;
                s as usize
            };
            let m = Rd(&src.mdl);
            let so = a.base + a.section_index as usize + sec * 8;
            (m.i32(so)?, m.i32(so + 4)?)
        } else {
            (a.anim_block, a.anim_index)
        };
        if block == 0 {
            ensure!(index > 0, "anim '{}' has no data (index 0)", a.name);
            Ok((&*src.mdl, a.base + index as usize, frame))
        } else {
            let ani = src.ani.as_deref().ok_or_else(|| {
                anyhow!(
                    "{} needs external anim block file '{}' (block {})",
                    a.name,
                    src.ani_name,
                    block
                )
            })?;
            let &(ds, _) = src
                .blocks
                .get(block as usize)
                .ok_or_else(|| anyhow!("anim block {} out of range", block))?;
            Ok((ani, ds + index as usize, frame))
        }
    }

    /// Debug helper: raw bytes at the chunk list start (`len` bytes) for `frame`.
    pub fn raw_bytes(&self, anim: usize, frame: i32, len: usize) -> Result<Vec<u8>> {
        let a = self.anims.get(anim).ok_or_else(|| anyhow!("no anim"))?;
        let (buf, off, _) = self.locate(a, frame)?;
        Ok(Rd(buf).bytes(off, len.min(buf.len() - off))?.to_vec())
    }

    /// Debug helper: the raw `(bone, flags, nextoffset)` chunk headers used for `frame` of anim `anim`.
    pub fn chunk_list(&self, anim: usize, frame: i32) -> Result<Vec<(u8, u8, i16)>> {
        let a = self.anims.get(anim).ok_or_else(|| anyhow!("no anim"))?;
        let (buf, mut off, _) = self.locate(a, frame)?;
        let r = Rd(buf);
        let mut v = Vec::new();
        loop {
            let (b, f, n) = (r.u8(off)?, r.u8(off + 1)?, r.i16(off + 2)?);
            v.push((b, f, n));
            if n == 0 || v.len() > 4096 {
                break;
            }
            off = Rd::rel(off, n as i32)?;
        }
        Ok(v)
    }

    /// Decode the local transforms of all (main-skeleton) bones at an integer frame.
    fn decode_frame(&self, a: &AnimDesc, frame: i32) -> Result<Vec<(Vec3, Quat)>> {
        let mut out: Vec<(Vec3, Quat)> = self.bones.iter().map(|b| (b.pos, b.quat)).collect();
        if a.flags & STUDIO_ALLZEROS != 0 {
            return Ok(out);
        }
        let src = &self.sources[a.source];
        let (buf, start, frame) = self.locate(a, frame)?;
        let r = Rd(buf);
        if a.flags & STUDIO_FRAMEANIM != 0 {
            let delta = a.flags & STUDIO_DELTA != 0;
            decode_frameanim(&r, start, frame as usize, src, delta, &mut out)?;
            return Ok(out);
        }

        let mut off = start;
        let mut guard = 0;
        loop {
            guard += 1;
            ensure!(guard < 4096, "anim chunk list too long");
            let bone = r.u8(off)? as usize;
            let flags = r.u8(off + 1)?;
            let next = r.i16(off + 2)?;
            if let Some(&SrcBone {
                pos: bpos,
                rot: brot,
                pos_scale: pscale,
                rot_scale: rscale,
                main: Some(mb),
                ..
            }) = src.bones.get(bone)
            {
                let mut dp = off + 4; // data cursor
                                      // ---- rotation
                let mut q: Option<Quat> = None;
                let mut rot_vp: Option<usize> = None;
                if flags & STUDIO_ANIM_RAWROT != 0 {
                    q = Some(quat48(r.bytes(dp, 6)?));
                    dp += 6;
                } else if flags & STUDIO_ANIM_RAWROT2 != 0 {
                    q = Some(quat64(r.bytes(dp, 8)?));
                    dp += 8;
                } else if flags & STUDIO_ANIM_ANIMROT != 0 {
                    rot_vp = Some(dp);
                    dp += 6;
                }
                // ---- position
                let mut p: Option<Vec3> = None;
                let mut pos_vp: Option<usize> = None;
                if flags & STUDIO_ANIM_RAWPOS != 0 {
                    let b = r.bytes(dp, 6)?;
                    p = Some(Vec3::new(f16(&b[0..2]), f16(&b[2..4]), f16(&b[4..6])));
                } else if flags & STUDIO_ANIM_ANIMPOS != 0 {
                    pos_vp = Some(dp);
                }
                let delta = flags & STUDIO_ANIM_DELTA != 0;

                if let Some(vp) = rot_vp {
                    let mut e = Vec3::ZERO;
                    for j in 0..3 {
                        e[j] = extract_value(&r, vp, j, frame as usize)? * rscale[j];
                    }
                    if !delta {
                        e += brot;
                    }
                    q = Some(euler_to_quat(e));
                }
                if let Some(vp) = pos_vp {
                    let mut v = Vec3::ZERO;
                    for j in 0..3 {
                        v[j] = extract_value(&r, vp, j, frame as usize)? * pscale[j];
                    }
                    if !delta {
                        v += bpos;
                    }
                    p = Some(v);
                }
                let (mut cur_p, mut cur_q) = out[mb];
                if delta {
                    // additive: apply on top of the bind pose
                    if let Some(p) = p {
                        cur_p += p;
                    }
                    if let Some(q) = q {
                        cur_q = (cur_q * q).normalize();
                    }
                } else {
                    if let Some(p) = p {
                        cur_p = p;
                    }
                    if let Some(q) = q {
                        cur_q = q;
                    }
                }
                out[mb] = (cur_p, cur_q);
            }
            if next == 0 {
                break;
            }
            off = Rd::rel(off, next as i32)?;
        }
        Ok(out)
    }
}

/// `STUDIO_FRAMEANIM` data: `mstudioframeanim_t { constantoffset, frameoffset, framelength, unused[3] }`, followed by one
/// flag byte per bone of the source MDL. Constant data comes from `constantoffset`, per-frame data from
/// `frameoffset + frame * framelength`; both are walked in bone order (rot, then pos).
fn decode_frameanim(
    r: &Rd,
    base: usize,
    frame: usize,
    src: &Source,
    delta: bool,
    out: &mut [(Vec3, Quat)],
) -> Result<()> {
    // Observed in shipped L4D2 data: older-built files (common infected) use 0x02/0x08 for const/animated
    // rotation stored as Quaternion48; newer ones (survivors) use 0x40/0x80 with a "smallest three"
    // 48-bit quaternion (reverse-engineered, see `quat48s`). Position is 0x01 (const) / 0x04 (animated),
    // Vector48 (half floats), in both.
    const CONST_POS: u8 = 0x01;
    const CONST_ROT: u8 = 0x02;
    const ANIM_POS: u8 = 0x04;
    const ANIM_ROT: u8 = 0x08;
    // Verified in helicopter_rescue @c1m1_intro_chopper section 5: flags
    // 0x18 = Quaternion48 followed by a 12-byte float position.
    const ANIM_POS_FLOAT: u8 = 0x10;
    const CONST_POS_FLOAT: u8 = 0x20;
    const CONST_ROT_S: u8 = 0x40; // Quaternion48S
    const ANIM_ROT_S: u8 = 0x80; // Quaternion48S
    let const_off = r.i32(base)? as usize;
    let frame_off = r.i32(base + 4)? as usize;
    let frame_len = r.i32(base + 8)? as usize;
    let mut cp = base + const_off;
    let mut fp = base + frame_off + frame * frame_len;
    for (i, sb) in src.bones.iter().enumerate() {
        let fl = r.u8(base + 24 + i)?;
        let mut q: Option<Quat> = None;
        let mut p: Option<Vec3> = None;
        if fl & CONST_ROT != 0 {
            q = Some(quat48(r.bytes(cp, 6)?));
            cp += 6;
        }
        if fl & CONST_ROT_S != 0 {
            q = Some(quat48s(r.bytes(cp, 6)?));
            cp += 6;
        }
        if fl & CONST_POS != 0 {
            let b = r.bytes(cp, 6)?;
            p = Some(Vec3::new(f16(&b[0..2]), f16(&b[2..4]), f16(&b[4..6])));
            cp += 6;
        }
        if fl & CONST_POS_FLOAT != 0 {
            p = Some(r.vec3(cp)?);
            cp += 12;
        }
        if fl & ANIM_ROT != 0 {
            q = Some(quat48(r.bytes(fp, 6)?));
            fp += 6;
        }
        if fl & ANIM_ROT_S != 0 {
            q = Some(quat48s(r.bytes(fp, 6)?));
            fp += 6;
        }
        if fl & ANIM_POS != 0 {
            let b = r.bytes(fp, 6)?;
            p = Some(Vec3::new(f16(&b[0..2]), f16(&b[2..4]), f16(&b[4..6])));
            fp += 6;
        }
        if fl & ANIM_POS_FLOAT != 0 {
            p = Some(r.vec3(fp)?);
            fp += 12;
        }
        if let Some(mb) = sb.main {
            let (cur_p, cur_q) = &mut out[mb];
            if delta {
                if let Some(p) = p {
                    *cur_p += p;
                }
                if let Some(q) = q {
                    *cur_q = (*cur_q * q).normalize();
                }
            } else {
                if let Some(p) = p {
                    *cur_p = p;
                }
                if let Some(q) = q {
                    *cur_q = q;
                }
            }
        }
    }
    ensure!(
        fp == base + frame_off + (frame + 1) * frame_len,
        "frame animation stride mismatch"
    );
    Ok(())
}

fn own_bones_len(m: &Rd) -> Result<usize> {
    Ok(m.i32(156)?.max(0) as usize)
}

#[cfg(test)]
mod frame_animation_tests {
    use super::*;
    #[test]
    fn autolayer_envelopes_remap_cycles_and_preserve_pose_driven_phase() {
        let mut layer = AutoLayer {
            sequence: 0,
            pose: Some("body_pitch".into()),
            flags: 0,
            start: 0.2,
            peak: 0.4,
            tail: 0.6,
            end: 0.8,
        };
        assert!(layer.sample(0.1, 0.5, &[]).is_none());
        assert!(layer.sample(0.8, 0.5, &[]).is_none());
        let (cycle, weight) = layer.sample(0.3, 0.5, &[]).unwrap();
        assert!((cycle - 1. / 6.).abs() < 0.0001 && (weight - 0.25).abs() < 0.0001);
        layer.flags = AL_SPLINE;
        let (_, weight) = layer.sample(0.25, 1., &[]).unwrap();
        assert!((weight - 0.15625).abs() < 0.0001);
        layer.flags = AL_XFADE;
        let (_, weight) = layer.sample(0.7, 0.5, &[]).unwrap();
        assert!((weight - 1. / 3.).abs() < 0.0001);
        layer.flags = AL_NOBLEND;
        assert!((layer.sample(0.3, 0.1, &[]).unwrap().1 - 0.5).abs() < 0.0001);
        layer.flags = AL_POSE;
        let (cycle, weight) = layer.sample(0.1, 0.5, &[("body_pitch", 0.5)]).unwrap();
        assert_eq!((cycle, weight), (0.1, 0.5));
        layer.start = 0.;
        layer.end = 0.;
        assert_eq!(layer.sample(0.9, 0.2, &[]), Some((0.9, 0.2)));
    }
    #[test]
    fn float_position_tracks_do_not_shift_following_rotations() {
        let mut data = vec![0u8; 68];
        data[0..4].copy_from_slice(&28i32.to_le_bytes());
        data[4..8].copy_from_slice(&40i32.to_le_bytes());
        data[8..12].copy_from_slice(&24i32.to_le_bytes());
        data[24] = 0x18; // animated Quaternion48 + float Vector
        data[25] = 0x28; // constant float Vector + animated Quaternion48
        let position = |data: &mut [u8], at: usize, p: [f32; 3]| {
            for (i, v) in p.iter().enumerate() {
                data[at + i * 4..at + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        };
        let identity = [0, 128, 0, 128, 0, 64];
        position(&mut data, 28, [11., 22., 33.]);
        data[40..46].copy_from_slice(&identity);
        position(&mut data, 46, [100., 200., 300.]);
        data[58..64].copy_from_slice(&identity);
        let bones = (0..2)
            .map(|i| SrcBone {
                name: format!("bone{i}"),
                quat: Quat::IDENTITY,
                pos: Vec3::ZERO,
                rot: Vec3::ZERO,
                pos_scale: Vec3::ONE,
                rot_scale: Vec3::ONE,
                main: Some(i),
            })
            .collect();
        let src = Source {
            name: String::new(),
            mdl: std::sync::Arc::from([] as [u8; 0]),
            ani: None,
            blocks: vec![],
            bones,
            ani_name: String::new(),
            chains: vec![],
        };
        let mut pose = vec![(Vec3::ZERO, Quat::IDENTITY); 2];
        decode_frameanim(&Rd(&data), 0, 0, &src, false, &mut pose).unwrap();
        assert_eq!(pose[0].0, Vec3::new(100., 200., 300.));
        assert_eq!(pose[1].0, Vec3::new(11., 22., 33.));
        assert_eq!(pose[1].1, Quat::IDENTITY);
        data[8..12].copy_from_slice(&18i32.to_le_bytes());
        assert!(decode_frameanim(&Rd(&data), 0, 0, &src, false, &mut pose).is_err());
    }
}

/// Resolve one RLE track at `frame` (`mstudioanim_valueptr_t` at `vp`, component `j`).
fn extract_value(r: &Rd, vp: usize, j: usize, frame: usize) -> Result<f32> {
    let off = r.i16(vp + j * 2)?;
    if off <= 0 {
        return Ok(0.0);
    }
    let mut p = vp + off as usize;
    let mut k = frame;
    loop {
        let valid = r.u8(p)? as usize;
        let total = r.u8(p + 1)? as usize;
        if total == 0 {
            return Ok(0.0);
        }
        if total <= k {
            k -= total;
            p += (valid + 1) * 2;
            continue;
        }
        let idx = if valid > k { k } else { valid - 1 };
        return Ok(r.i16(p + 2 + idx * 2)? as f32);
    }
}

fn f16(b: &[u8]) -> f32 {
    let h = u16::from_le_bytes([b[0], b[1]]);
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if man == 0 {
            sign << 31
        } else {
            // subnormal
            let mut e = 127 - 15 + 1;
            let mut m = man;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            (sign << 31) | ((e as u32) << 23) | ((m & 0x3ff) << 13)
        }
    } else if exp == 31 {
        (sign << 31) | 0x7f80_0000 | (man << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (man << 13)
    };
    f32::from_bits(bits)
}

/// Source `Quaternion48` -> Quat (x,y,z + sign bit of w).
pub fn quat48(b: &[u8]) -> Quat {
    let s0 = u16::from_le_bytes([b[0], b[1]]) as i32;
    let s1 = u16::from_le_bytes([b[2], b[3]]) as i32;
    let s2 = u16::from_le_bytes([b[4], b[5]]) as i32;
    let x = (s0 - 32768) as f32 / 32768.0;
    let y = (s1 - 32768) as f32 / 32768.0;
    let z = ((s2 & 0x7fff) - 16384) as f32 / 16384.0;
    let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    if s2 & 0x8000 != 0 {
        w = -w;
    }
    Quat::from_xyzw(x, y, z, w)
}

/// 48-bit "smallest three" quaternion used by `STUDIO_FRAME` bone flags 0x40/0x80 in newer L4D2 files.
/// Three u16: low 15 bits are the three smaller components, `(v - 16384) / 23168` (23168 = 32768/sqrt 2);
/// the high bit of word 0/1 select which component was dropped (`idx = hi1 + 2*hi0`, dropped slot =
/// `(idx + 3) % 4` in x,y,z,w order), the high bit of word 2 is the sign of the dropped one. The three stored
/// components fill the slots cyclically after the dropped one. (Reverse-engineered from shipped data by
/// maximizing agreement with bind rotations; the dropped-slot mapping for rarely used indices has the
/// least evidence.)
pub fn quat48s(b: &[u8]) -> Quat {
    let s = [
        u16::from_le_bytes([b[0], b[1]]),
        u16::from_le_bytes([b[2], b[3]]),
        u16::from_le_bytes([b[4], b[5]]),
    ];
    let idx = ((s[1] >> 15) as usize) + 2 * ((s[0] >> 15) as usize);
    let slot = (idx + 3) % 4;
    let c = |v: u16| ((v & 0x7fff) as i32 - 16384) as f32 / 23168.0;
    let (a, b, cc) = (c(s[0]), c(s[1]), c(s[2]));
    let mut d = (1.0 - a * a - b * b - cc * cc).max(0.0).sqrt();
    if s[2] >> 15 != 0 {
        d = -d;
    }
    let mut q = [0f32; 4];
    q[slot] = d;
    q[(slot + 1) % 4] = a;
    q[(slot + 2) % 4] = b;
    q[(slot + 3) % 4] = cc;
    Quat::from_xyzw(q[0], q[1], q[2], q[3]).normalize()
}

/// Source `Quaternion64` -> Quat (21 bits per component + sign bit of w).
pub fn quat64(b: &[u8]) -> Quat {
    let v = u64::from_le_bytes(b[0..8].try_into().unwrap());
    let c = |sh: u32| (((v >> sh) & 0x1f_ffff) as i64 - 1_048_576) as f32 / 1_048_576.5;
    let (x, y, z) = (c(0), c(21), c(42));
    let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    if (v >> 63) & 1 != 0 {
        w = -w;
    }
    Quat::from_xyzw(x, y, z, w)
}

/// Source `AngleQuaternion(RadianEuler)`: x = roll, y = pitch, z = yaw.
pub fn euler_to_quat(e: Vec3) -> Quat {
    let (sr, cr) = (e.x * 0.5).sin_cos();
    let (sp, cp) = (e.y * 0.5).sin_cos();
    let (sy, cy) = (e.z * 0.5).sin_cos();
    Quat::from_xyzw(
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    )
}

// ---------------------------------------------------------------------------------------------
// skinned mesh
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkinnedVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// MDL bone indices (unused slots 0 with weight 0).
    pub bone_ids: [u8; 4],
    /// Normalized weights (sum 1), `weights[3]` is always 0 (Source allows 3 bones/vertex).
    pub weights: [f32; 4],
}

/// Same data as `mdl::ModelMesh` but with skinning attributes per vertex.
#[derive(Clone, Debug, Default)]
pub struct SkinnedMesh {
    pub materials: Vec<String>,
    pub material_dirs: Vec<String>,
    pub texture_names: Vec<String>,
    pub vertices: Vec<SkinnedVertex>,
    pub batches: Vec<ModelBatch>,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

impl SkinnedMesh {
    pub fn triangle_count(&self) -> usize {
        self.batches.iter().map(|b| b.indices.len() / 3).sum()
    }
}

/// Per-vertex (weights, bones, numbones) of the LOD-0 VVD vertex array, in the exact order `load_static` uses
/// (fixups applied).
fn vvd_skin(vvd: &[u8]) -> Result<Vec<([f32; 3], [u8; 3], u8)>> {
    let r = Rd(vvd);
    ensure!(vvd.len() >= 64 && &vvd[0..4] == b"IDSV", "bad VVD magic");
    ensure!(r.i32(4)? == 4, "unsupported VVD version");
    let n0 = r.i32(16)? as usize;
    let num_fixups = r.i32(48)? as usize;
    let fixup_start = r.i32(52)? as usize;
    let vert_start = r.i32(56)? as usize;
    let read = |i: usize| -> Result<([f32; 3], [u8; 3], u8)> {
        let o = vert_start + i * 48;
        Ok((
            [r.f32(o)?, r.f32(o + 4)?, r.f32(o + 8)?],
            [r.u8(o + 12)?, r.u8(o + 13)?, r.u8(o + 14)?],
            r.u8(o + 15)?,
        ))
    };
    let mut out = Vec::new();
    if num_fixups == 0 {
        for i in 0..n0 {
            out.push(read(i)?);
        }
    } else {
        for f in 0..num_fixups {
            let fo = fixup_start + f * 12;
            if r.i32(fo)? < 0 {
                continue;
            }
            let src = r.i32(fo + 4)? as usize;
            for i in 0..r.i32(fo + 8)? as usize {
                out.push(read(src + i)?);
            }
        }
    }
    Ok(out)
}

/// Load a model like `mdl::load_static` but keep bone weights/indices.
///
/// Bone indices come from the VVD `mstudiobone weight` struct (3 weights, 3 bone ids, numbones), which holds
/// *global* MDL bone indices. The VTX per-vertex `boneID`s are the same global ids for software-skinned
/// strip groups and hardware ids (translated through the strip's `boneStateChanges`) for hw-skinned ones;
/// since the CPU path uses the VVD ids directly, the VTX bone-remap tables are not needed.
pub fn load_skinned(mdl: &[u8], vvd: &[u8], vtx: &[u8]) -> Result<SkinnedMesh> {
    let m = load_static(mdl, vvd, vtx)?;
    let sk = vvd_skin(vvd)?;
    ensure!(
        sk.len() == m.vertices.len(),
        "VVD skin count {} != vertex count {}",
        sk.len(),
        m.vertices.len()
    );
    let nbones = Rd(mdl).i32(156)?.max(0) as usize;
    let mut vertices = Vec::with_capacity(sk.len());
    for (v, (w, b, nb)) in m.vertices.iter().zip(sk) {
        let nb = (nb as usize).clamp(1, 3);
        let mut weights = [0f32; 4];
        let mut ids = [0u8; 4];
        let mut sum = 0.0;
        for k in 0..nb {
            ensure!(
                (b[k] as usize) < nbones.max(1),
                "bone id {} >= {}",
                b[k],
                nbones
            );
            weights[k] = w[k].max(0.0);
            ids[k] = b[k];
            sum += weights[k];
        }
        if sum > 1e-6 {
            for w in weights.iter_mut() {
                *w /= sum;
            }
        } else {
            weights = [1.0, 0.0, 0.0, 0.0];
        }
        vertices.push(SkinnedVertex {
            pos: v.pos,
            normal: v.normal,
            uv: v.uv,
            bone_ids: ids,
            weights,
        });
    }
    Ok(SkinnedMesh {
        materials: m.materials,
        material_dirs: m.material_dirs,
        texture_names: m.texture_names,
        vertices,
        batches: m.batches,
        mins: m.mins,
        maxs: m.maxs,
    })
}

/// Linear-blend skin all vertices on the CPU. `skin[i] = pose[i] * poseToBone[i]` (see `Skeleton::skin_matrices`).
pub fn skin_cpu(mesh: &SkinnedMesh, skin: &[Mat4]) -> Vec<[f32; 3]> {
    mesh.vertices
        .iter()
        .map(|v| {
            let p = Vec3::from(v.pos);
            let mut acc = Vec3::ZERO;
            let mut wsum = 0.0;
            for k in 0..4 {
                let w = v.weights[k];
                if w <= 0.0 {
                    continue;
                }
                if let Some(m) = skin.get(v.bone_ids[k] as usize) {
                    acc += m.transform_point3(p) * w;
                    wsum += w;
                }
            }
            if wsum <= 0.0 {
                v.pos
            } else {
                (acc / wsum).to_array()
            }
        })
        .collect()
}

/// Same for normals (rotation part only, not renormalized by weights sum beyond normalize()).
pub fn skin_normals_cpu(mesh: &SkinnedMesh, skin: &[Mat4]) -> Vec<[f32; 3]> {
    mesh.vertices
        .iter()
        .map(|v| {
            let n = Vec3::from(v.normal);
            let mut acc = Vec3::ZERO;
            for k in 0..4 {
                if v.weights[k] > 0.0 {
                    if let Some(m) = skin.get(v.bone_ids[k] as usize) {
                        acc += m.transform_vector3(n) * v.weights[k];
                    }
                }
            }
            acc.normalize_or_zero().to_array()
        })
        .collect()
}
