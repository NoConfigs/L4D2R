//! Native MDL animation sampling and weighted VVD skinning. Owned data stays local.
use super::ModelMesh;
use crate::{binary::*, source_assets::SourceAssets};
use bevy::math::{Mat4, Quat, Vec3};
use std::{collections::HashMap, io};
#[derive(Clone)]
struct Bone {
    name: String,
    parent: i32,
    pos: Vec3,
    rot: Quat,
    inverse: Mat4,
}
#[derive(Clone)]
pub struct Pose {
    pub transforms: HashMap<String, Mat4>,
    pub camera: Vec3,
}
fn matrix(b: &[u8], at: usize) -> io::Result<Mat4> {
    range(b, at, 48)?;
    Ok(Mat4::from_cols_array(&[
        f32_at(b, at)?,
        f32_at(b, at + 16)?,
        f32_at(b, at + 32)?,
        0.,
        f32_at(b, at + 4)?,
        f32_at(b, at + 20)?,
        f32_at(b, at + 36)?,
        0.,
        f32_at(b, at + 8)?,
        f32_at(b, at + 24)?,
        f32_at(b, at + 40)?,
        0.,
        f32_at(b, at + 12)?,
        f32_at(b, at + 28)?,
        f32_at(b, at + 44)?,
        1.,
    ]))
}
fn bones(b: &[u8]) -> io::Result<Vec<Bone>> {
    let n = count(b, 156, 1024)?;
    let start = count(b, 160, b.len())?;
    (0..n)
        .map(|i| {
            let at = start + i * 216;
            range(b, at, 216)?;
            let parent = u32_at(b, at + 4)? as i32;
            if parent < -1 || parent >= i as i32 {
                return Err(invalid("Invalid studio bone hierarchy"));
            }
            let rot = Quat::from_xyzw(
                f32_at(b, at + 44)?,
                f32_at(b, at + 48)?,
                f32_at(b, at + 52)?,
                f32_at(b, at + 56)?,
            );
            if !rot.is_finite() || rot.length_squared() < 0.01 {
                return Err(invalid("Invalid studio bone quaternion"));
            }
            Ok(Bone {
                name: cstring(b, relative(b, at, 0)?)?.to_lowercase(),
                parent,
                pos: Vec3::from_array(vec3(b, at + 32)?),
                rot: rot.normalize(),
                inverse: matrix(b, at + 96)?,
            })
        })
        .collect()
}
#[cfg(test)]
fn quat48(b: &[u8], at: usize) -> io::Result<Quat> {
    let x = (u16_at(b, at)? as f32 - 32768.) / 32768.;
    let y = (u16_at(b, at + 2)? as f32 - 32768.) / 32768.;
    let zbits = u16_at(b, at + 4)?;
    let z = ((zbits & 32767) as f32 - 16384.) / 16384.;
    let w = (1. - x * x - y * y - z * z).max(0.).sqrt() * if zbits & 32768 != 0 { -1. } else { 1. };
    Ok(Quat::from_xyzw(x, y, z, w).normalize())
}
#[cfg(test)]
fn quat64(b: &[u8], at: usize) -> io::Result<Quat> {
    let v = u64::from_le_bytes(range(b, at, 8)?.try_into().unwrap());
    let a = |shift: u32| (((v >> shift) & 0x1fffffu64) as f32 - 1048576.) / 1048576.5;
    let (x, y, z) = (a(0), a(21), a(42));
    let w = (1. - x * x - y * y - z * z).max(0.).sqrt() * if v >> 63 != 0 { -1. } else { 1. };
    Ok(Quat::from_xyzw(x, y, z, w).normalize())
}
#[cfg(test)]
fn quat48_sorted(b: &[u8], at: usize) -> io::Result<Quat> {
    let packed = [u16_at(b, at)?, u16_at(b, at + 2)?, u16_at(b, at + 4)?];
    let offset = ((packed[0] >> 15) * 2 + (packed[1] >> 15)) as usize;
    let mut q = [0.; 4];
    for i in 0..3 {
        q[(offset + i) % 4] = ((packed[i] & 32767) as f32 - 16384.) / 23168.;
    }
    q[(offset + 3) % 4] = (1. - q.iter().map(|v| v * v).sum::<f32>()).max(0.).sqrt()
        * if packed[2] & 32768 != 0 { -1. } else { 1. };
    Ok(Quat::from_array(q).normalize())
}
#[cfg(test)]
fn rle(b: &[u8], ptr: usize, axis: usize, mut frame: usize) -> io::Result<f32> {
    let offset = u16_at(b, ptr + axis * 2)? as i16;
    if offset <= 0 {
        return Ok(0.);
    }
    let mut at = ptr + offset as usize;
    for _ in 0..10000 {
        let span = range(b, at, 2)?;
        let valid = span[0] as usize;
        let total = span[1] as usize;
        if total == 0 || valid > total {
            return Err(invalid("Invalid animation run"));
        }
        if frame < total {
            return Ok(if valid == 0 {
                0.
            } else {
                u16_at(b, at + 2 * (1 + frame.min(valid - 1)))? as i16 as f32
            });
        }
        frame -= total;
        at += 2 * (valid + 1);
    }
    Err(invalid("Animation run overflow"))
}
/// l4d2r supports fractional frames, included sequences and frame-animation channels.
pub fn sample_l4d2r(
    assets: &mut SourceAssets,
    path: &str,
    preferences: &[&str],
    time: f32,
) -> io::Result<Pose> {
    let assets = std::cell::RefCell::new(assets);
    let sk = l4d2_formats::anim::Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok())
        .map_err(|e| invalid(e.to_string()))?;
    let seq = preferences
        .iter()
        .find_map(|name| sk.find_sequence(name))
        .or_else(|| {
            sk.sequences().iter().position(|s| {
                s.activity_name.starts_with("ACT_VM_IDLE") && !s.activity_name.contains("LAYER")
            })
        })
        .unwrap_or(0);
    let pose = sk.pose_sequence_params(seq, time, true, &[]);
    let mut camera = Vec3::ZERO;
    let transforms = sk
        .bones
        .iter()
        .zip(pose)
        .map(|(b, m)| {
            let m = Mat4::from_cols_array(&m.to_cols_array());
            if b.name.to_ascii_lowercase().ends_with(".camera") {
                camera = m.transform_point3(Vec3::ZERO);
            }
            (b.name.to_ascii_lowercase(), m)
        })
        .collect();
    Ok(Pose { transforms, camera })
}
pub fn apply(
    assets: &mut SourceAssets,
    path: &str,
    draws: &mut [ModelMesh],
    pose: &Pose,
    _view: bool,
) -> io::Result<()> {
    let mdl = assets.read(path)?;
    let bones = bones(&mdl)?;
    let mut globals: Vec<Mat4> = vec![];
    let mut palette = vec![];
    for b in bones {
        let local = Mat4::from_rotation_translation(b.rot, b.pos);
        let global = pose.transforms.get(&b.name).copied().unwrap_or_else(|| {
            if b.parent < 0 {
                local
            } else {
                globals[b.parent as usize] * local
            }
        });
        globals.push(global);
        palette.push(global * b.inverse);
    }
    // Viewmodel animations are already authored relative to the player's eye.
    // The animated Camera bone drives effects; subtracting it moves the gun away.
    let origin = Vec3::ZERO;
    for draw in draws {
        for i in 0..draw.positions.len() {
            let p = draw.positions[i];
            let p = Vec3::new(p[0], -p[2], p[1]) / 0.0254;
            let n = draw.normals[i];
            let n = Vec3::new(n[0], -n[2], n[1]);
            let (indices, weights) = draw.weights[i];
            let mut position = Vec3::ZERO;
            let mut normal = Vec3::ZERO;
            let mut sum = 0.;
            for j in 0..3 {
                if weights[j] <= 0. {
                    continue;
                }
                let m = palette
                    .get(indices[j] as usize)
                    .ok_or_else(|| invalid("VVD bone outside palette"))?;
                position += m.transform_point3(p) * weights[j];
                normal += m.transform_vector3(n) * weights[j];
                sum += weights[j];
            }
            if sum > 0. {
                position = position / sum - origin;
                normal = normal.normalize_or_zero();
            } else {
                position = p - origin;
                normal = n;
            }
            if !position.is_finite() || !normal.is_finite() {
                return Err(invalid("Nonfinite posed vertex"));
            }
            draw.positions[i] = source_position(position.to_array());
            draw.normals[i] = source_normal(normal.to_array());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compressed_quaternion_and_repeat_run() {
        let b = [0, 128, 0, 128, 0, 64];
        assert!(quat48(&b, 0).unwrap().dot(Quat::IDENTITY) > 0.999);
        let data = [6, 0, 0, 0, 0, 0, 2, 5, 10, 0, 20, 0];
        assert_eq!(rle(&data, 0, 0, 0).unwrap(), 10.);
        assert_eq!(rle(&data, 0, 0, 4).unwrap(), 20.);
        assert!(rle(&data, 0, 0, 5).is_err());
        assert!(quat64(&[0; 3], 0).is_err());
        assert!(
            quat48_sorted(&[0, 64, 0, 64, 0, 64], 0)
                .unwrap()
                .dot(Quat::IDENTITY)
                > 0.999
        );
    }
}
