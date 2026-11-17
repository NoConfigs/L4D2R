//! Native studio IK records and model-space two-segment constraints.
//! Layout reference: Valve's public Source SDK studio.h. Runtime contact
//! selection/latching is deliberately separate from animation sampling.
use super::*;

#[derive(Clone, Debug)]
pub struct IkChain {
    pub name: String,
    pub bones: [usize; 3],
    pub knee_direction: Vec3,
}
#[derive(Clone, Debug)]
pub struct IkLock {
    pub chain: usize,
    pub position_weight: f32,
    pub local_rotation_weight: f32,
    pub flags: i32,
}
pub(super) fn parse_locks(
    r: &Rd,
    count: i32,
    start: usize,
    local: &[IkChain],
    main: &[IkChain],
) -> Result<Vec<IkLock>> {
    ensure!((0..=128).contains(&count), "invalid IK lock count");
    let mut locks = Vec::new();
    for i in 0..count as usize {
        let at = start + i * 32;
        r.bytes(at, 32)?;
        let index = r.i32(at)?;
        ensure!(
            index >= 0 && (index as usize) < local.len(),
            "invalid IK lock chain"
        );
        let Some(chain) = main
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(&local[index as usize].name))
        else {
            continue;
        };
        let position_weight = r.f32(at + 4)?;
        let local_rotation_weight = r.f32(at + 8)?;
        ensure!(
            position_weight.is_finite()
                && local_rotation_weight.is_finite()
                && (0. ..=1.).contains(&position_weight)
                && (0. ..=1.).contains(&local_rotation_weight),
            "invalid IK lock weight"
        );
        locks.push(IkLock {
            chain,
            position_weight,
            local_rotation_weight,
            flags: r.i32(at + 12)?,
        });
    }
    Ok(locks)
}
#[derive(Clone, Debug)]
pub struct IkRule {
    pub index: i32,
    pub chain: usize,
    pub kind: i32,
    pub bone: Option<usize>,
    pub slot: i32,
    pub weight: f32,
    pub target: Mat4,
    pub height: f32,
    pub radius: f32,
    pub floor: f32,
    pub drop: f32,
    pub top: f32,
    pub attachment: String,
}
pub(super) fn parse_chains(r: &Rd, bone_count: usize) -> Result<Vec<IkChain>> {
    let n = r.i32(284)?;
    ensure!((0..=128).contains(&n), "invalid IK chain count");
    if n == 0 {
        return Ok(vec![]);
    }
    let start = r.i32(288)?;
    ensure!(start > 0, "invalid IK chain offset");
    let mut out = Vec::new();
    for i in 0..n as usize {
        let at = start as usize + i * 16;
        ensure!(r.i32(at + 8)? == 3, "unsupported IK chain length");
        let links = Rd::rel(at, r.i32(at + 12)?)?;
        let mut bones = [0; 3];
        for (j, b) in bones.iter_mut().enumerate() {
            let value = r.i32(links + j * 28)?;
            ensure!(
                value >= 0 && (value as usize) < bone_count,
                "invalid IK link bone"
            );
            *b = value as usize;
        }
        let knee_direction = r.vec3(links + 4)?;
        ensure!(knee_direction.is_finite(), "invalid IK knee direction");
        out.push(IkChain {
            name: r.cstr(Rd::rel(at, r.i32(at)?)?)?,
            bones,
            knee_direction,
        });
    }
    Ok(out)
}

fn envelope(cycle: f32, start: f32, peak: f32, tail: f32, end: f32) -> (f32, f32) {
    let c = if end > 1. && cycle < start {
        cycle + 1.
    } else {
        cycle
    };
    let w = if c < start || c >= end {
        0.
    } else if c < peak {
        (c - start) / (peak - start)
    } else if c < tail {
        1.
    } else {
        (end - c) / (end - tail)
    };
    let w = w.clamp(0., 1.);
    (w * w * (3. - 2. * w), c)
}
fn channel(r: &Rd, at: usize, frame: usize) -> Result<f32> {
    let mut p = at;
    let mut f = frame;
    for _ in 0..10000 {
        let valid = r.u8(p)? as usize;
        let total = r.u8(p + 1)? as usize;
        ensure!(total > 0 && valid <= total, "invalid IK error run");
        if f < total {
            return Ok(if valid == 0 {
                0.
            } else {
                r.i16(p + 2 * (1 + f.min(valid - 1)))? as f32
            });
        }
        f -= total;
        p += 2 * (1 + valid);
    }
    Err(anyhow!("IK error run overflow"))
}
fn error_pose(r: &Rd, rule: usize, frame: f32) -> Result<Option<Mat4>> {
    let raw = r.i32(rule + 72)?;
    let compressed = r.i32(rule + 60)?;
    let f = frame.max(0.);
    let i = f.floor() as usize;
    let fraction = f.fract();
    let sample = |index: usize| -> Result<(Vec3, Quat)> {
        if raw != 0 {
            let at = Rd::rel(rule, raw)? + index * 28;
            let q = Quat::from_xyzw(
                r.f32(at + 12)?,
                r.f32(at + 16)?,
                r.f32(at + 20)?,
                r.f32(at + 24)?,
            );
            ensure!(
                q.is_finite() && q.length_squared() > 0.01,
                "invalid IK error rotation"
            );
            return Ok((r.vec3(at)?, q.normalize()));
        }
        let at = Rd::rel(rule, compressed)?;
        let mut values = [0.; 6];
        for (axis, value) in values.iter_mut().enumerate() {
            let offset = r.i16(at + 24 + axis * 2)?;
            if offset > 0 {
                *value = channel(r, at + offset as usize, index)? * r.f32(at + axis * 4)?;
            }
        }
        let q = Quat::from_rotation_z(values[5])
            * Quat::from_rotation_y(values[4])
            * Quat::from_rotation_x(values[3]);
        Ok((Vec3::new(values[0], values[1], values[2]), q))
    };
    if raw == 0 && compressed == 0 {
        return Ok(None);
    }
    let (p, q) = sample(i)?;
    let (p, q) = if fraction > 0.0001 {
        let (p1, q1) = sample(i + 1)?;
        (p.lerp(p1, fraction), q.slerp(q1, fraction))
    } else {
        (p, q)
    };
    ensure!(p.is_finite() && q.is_finite(), "nonfinite IK error");
    Ok(Some(Mat4::from_rotation_translation(q, p)))
}

impl Skeleton {
    /// Preserve endpoints captured before a sequence layer changes the rig.
    pub fn apply_ik_locks(&self, before: &[Mat4], after: &mut [Mat4], locks: &[IkLock]) {
        if before.len() != self.bones.len() || after.len() != self.bones.len() {
            return;
        }
        for lock in locks {
            let Some(chain) = self.ik_chains.get(lock.chain) else {
                continue;
            };
            let end = chain.bones[2];
            let parent = chain.bones[1];
            let original_local = after[parent].inverse() * after[end];
            self.solve_ik(after, lock.chain, before[end], lock.position_weight);
            let (_, locked_q, _) = before[end].to_scale_rotation_translation();
            let (_, current_q, _) =
                (after[parent] * original_local).to_scale_rotation_translation();
            let old = after[end];
            after[end] = Mat4::from_rotation_translation(
                locked_q
                    .slerp(current_q, lock.local_rotation_weight)
                    .normalize(),
                old.w_axis.truncate(),
            );
            let delta = after[end] * old.inverse();
            for i in end + 1..after.len() {
                let mut p = self.bones[i].parent;
                while p > end as i32 {
                    p = self.bones[p as usize].parent;
                }
                if p == end as i32 {
                    after[i] = delta * after[i];
                }
            }
        }
    }
    /// Strict diagnostic sampling; unlike playback, reports malformed tracks.
    pub fn animation_ik_rules_checked(&self, anim: usize, cycle: f32) -> Result<Vec<IkRule>> {
        ensure!(
            anim < self.anims.len() && cycle.is_finite(),
            "invalid IK animation sample"
        );
        self.animation_ik(anim, cycle.clamp(0., 1.), 1.)
    }
    /// Match the paired idle/action-layer convention used by pose_view_sequence.
    pub fn ik_view_rules(&self, seq: usize, time: f32, looping: bool) -> Vec<IkRule> {
        let Some(s) = self.sequences.get(seq) else {
            return vec![];
        };
        let layer = if s.flags & STUDIO_DELTA != 0 || s.label.ends_with("_layer") {
            Some(seq)
        } else {
            self.find_sequence(&format!("{}_layer", s.label))
                .or_else(|| self.find_sequence(&format!("{}_LAYER", s.activity_name)))
        };
        let base = if layer.is_some() {
            self.view_idle_sequence().unwrap_or(seq)
        } else {
            seq
        };
        let cycle = self.sequence_cycle(seq, time, looping);
        let mut out = Vec::new();
        self.collect_ik(base, cycle, 1., &[], 0, &mut out);
        if let Some(layer) = layer {
            if !self.sequences[base]
                .autolayers
                .iter()
                .any(|a| a.sequence == layer)
            {
                self.collect_ik(layer, cycle, 1., &[], 0, &mut out);
            }
        }
        out
    }
    /// Weighted IK rules from blend-grid cells and authored autolayers.
    pub fn ik_rules(
        &self,
        seq: usize,
        time: f32,
        looping: bool,
        values: &[(&str, f32)],
    ) -> Vec<IkRule> {
        let mut out = Vec::new();
        self.collect_ik(
            seq,
            self.sequence_cycle(seq, time, looping),
            1.,
            values,
            0,
            &mut out,
        );
        out
    }
    fn collect_ik(
        &self,
        seq: usize,
        cycle: f32,
        weight: f32,
        values: &[(&str, f32)],
        depth: usize,
        out: &mut Vec<IkRule>,
    ) {
        if depth > 16 || weight <= 0.0001 {
            return;
        }
        let Some(s) = self.sequences.get(seq) else {
            return;
        };
        let p = self.blend_axes(seq, values);
        let axis = |p: f32, g: i32| {
            let g = g.max(1) as usize;
            let x = p.clamp(0., 1.) * (g - 1) as f32;
            let a = (x.floor() as usize).min(g - 1);
            (a, (a + 1).min(g - 1), x - a as f32)
        };
        let (x0, x1, xw) = axis(p[0], s.group_size[0]);
        let (y0, y1, yw) = axis(p[1], s.group_size[1]);
        let mut blended: Vec<IkRule> = Vec::new();
        for (x, wx) in [(x0, 1. - xw), (x1, xw)] {
            for (y, wy) in [(y0, 1. - yw), (y1, yw)] {
                let w = weight * wx * wy;
                if w <= 0.0001 {
                    continue;
                }
                if let Some(&index) = s.blend_anims.get(x + y * s.group_size[0].max(1) as usize) {
                    if let Ok(rules) = self.animation_ik(index, cycle, w) {
                        for rule in rules {
                            if let Some(old) = blended.iter_mut().find(|r| {
                                r.index == rule.index
                                    && r.chain == rule.chain
                                    && r.kind == rule.kind
                                    && r.bone == rule.bone
                                    && r.slot == rule.slot
                            }) {
                                let sum = old.weight + rule.weight;
                                let t = rule.weight / sum;
                                let (_, q0, p0) = old.target.to_scale_rotation_translation();
                                let (_, q1, p1) = rule.target.to_scale_rotation_translation();
                                old.target = Mat4::from_rotation_translation(
                                    q0.slerp(q1, t),
                                    p0.lerp(p1, t),
                                );
                                old.height += (rule.height - old.height) * t;
                                old.radius += (rule.radius - old.radius) * t;
                                old.floor += (rule.floor - old.floor) * t;
                                old.drop += (rule.drop - old.drop) * t;
                                old.top += (rule.top - old.top) * t;
                                old.weight = sum;
                            } else {
                                blended.push(rule);
                            }
                        }
                    }
                }
            }
        }
        out.extend(blended);
        for layer in &s.autolayers {
            if let Some((phase, w)) = layer.sample(cycle, weight, values) {
                self.collect_ik(layer.sequence, phase, w, values, depth + 1, out);
            }
        }
    }
    fn animation_ik(&self, anim: usize, cycle: f32, blend: f32) -> Result<Vec<IkRule>> {
        let a = &self.anims[anim];
        let src = &self.sources[a.source];
        let mdl = Rd(&src.mdl);
        let count = mdl.i32(a.base + 60)?;
        ensure!((0..=128).contains(&count), "invalid IK rule count");
        if count == 0 {
            return Ok(vec![]);
        }
        let index = mdl.i32(a.base + 64)?;
        let (r, start) = if index != 0 {
            (Rd(&src.mdl), Rd::rel(a.base, index)?)
        } else {
            let offset = mdl.i32(a.base + 68)?;
            // Descriptors may reserve rule slots without a rule block.
            // Native pIKRule returns null for this case.
            if offset == 0 {
                return Ok(vec![]);
            }
            let (begin, end) = *src
                .blocks
                .get(a.anim_block.max(0) as usize)
                .ok_or_else(|| anyhow!("missing IK block"))?;
            let ani = src.ani.as_ref().ok_or_else(|| anyhow!("missing IK ANI"))?;
            ensure!(
                offset > 0 && begin <= end && end <= ani.len(),
                "invalid IK block offset"
            );
            (Rd(&ani[..end]), begin + offset as usize)
        };
        let mut out = Vec::new();
        for i in 0..count as usize {
            let at = start + i * 152;
            r.bytes(at, 152)?;
            let local_chain = r.i32(at + 8)?;
            let Some(chain) = src
                .chains
                .get(local_chain.max(0) as usize)
                .filter(|_| local_chain >= 0)
            else {
                continue;
            };
            let Some(chain_index) = self
                .ik_chains
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(&chain.name))
            else {
                continue;
            };
            let kind = r.i32(at + 4)?;
            let start_cycle = r.f32(at + 76)?;
            let (w, c) = envelope(
                cycle,
                start_cycle,
                r.f32(at + 80)?,
                r.f32(at + 84)?,
                r.f32(at + 88)?,
            );
            if w * blend <= 0.0001 {
                continue;
            }
            let frame = (a.num_frames - 1).max(0) as f32 * (c - start_cycle);
            let target = error_pose(&r, at, frame)?;
            // Release/unlatch rules need no positional error track.
            if target.is_none() && !matches!(kind, 4 | 6) {
                continue;
            }
            let bone = r.i32(at + 12)?;
            let bone = if bone < 0 {
                None
            } else {
                let Some(main) = src.bones.get(bone as usize).and_then(|b| b.main) else {
                    continue;
                };
                Some(main)
            };
            let attachment_offset = r.i32(at + 120)?;
            out.push(IkRule {
                index: r.i32(at)?,
                chain: chain_index,
                kind,
                bone,
                slot: r.i32(at + 16)?,
                weight: w * blend,
                target: target.unwrap_or(Mat4::IDENTITY),
                height: r.f32(at + 20)?,
                radius: r.f32(at + 24)?,
                floor: r.f32(at + 28)?,
                drop: r.f32(at + 100)?,
                top: r.f32(at + 104)?,
                attachment: if attachment_offset == 0 {
                    String::new()
                } else {
                    r.cstr(Rd::rel(at, attachment_offset)?)?
                },
            });
        }
        Ok(out)
    }
    /// Apply explicit model-space targets, preserving unrelated branches.
    pub fn solve_ik(&self, pose: &mut [Mat4], chain: usize, target: Mat4, weight: f32) -> bool {
        let Some(c) = self.ik_chains.get(chain) else {
            return false;
        };
        solve_chain(&self.bones, pose, c, target, weight)
    }
}

/// Analytic two-segment solver. Target is clamped to reachable distances;
/// descendants follow their solved ancestors with original local transforms.
pub fn solve_chain(
    bones: &[Bone],
    pose: &mut [Mat4],
    chain: &IkChain,
    target: Mat4,
    weight: f32,
) -> bool {
    if pose.len() != bones.len() || !target.is_finite() || !weight.is_finite() || weight <= 0. {
        return false;
    }
    let [a, b, c] = chain.bones;
    if c >= pose.len()
        || a >= b
        || b >= c
        || bones[b].parent != a as i32
        || bones[c].parent != b as i32
    {
        return false;
    }
    let old = pose.to_vec();
    let point = |i: usize| old[i].w_axis.truncate();
    let (root, mid, tip) = (point(a), point(b), point(c));
    let (upper, lower) = (mid.distance(root), tip.distance(mid));
    if upper < 0.0001 || lower < 0.0001 {
        return false;
    }
    let w = weight.clamp(0., 1.);
    let delta = tip.lerp(target.w_axis.truncate(), w) - root;
    let mut direction = delta.normalize_or_zero();
    if direction == Vec3::ZERO {
        direction = (tip - root).normalize_or_zero();
    }
    if direction == Vec3::ZERO {
        direction = Vec3::X;
    }
    let distance = delta
        .length()
        .clamp((upper - lower).abs() + 0.00001, upper + lower - 0.00001);
    // Folded chains magnify cancellation in the triangle calculation.
    // Keep its scalar arithmetic in f64 while poses remain native f32.
    let (u, l, d) = (upper as f64, lower as f64, distance as f64);
    let along = (u * u - l * l + d * d) / (2. * d);
    let height = (u * u - along * along).max(0.).sqrt() as f32;
    let along = along as f32;
    let mut bend = mid - root - direction * (mid - root).dot(direction);
    // Reproject with cross products to avoid an axial residual dominating
    // the bend direction of an almost straight/folded authored limb.
    bend = direction.cross(bend.cross(direction));
    if bend.length_squared() < 1e-8 {
        bend = old[a].transform_vector3(chain.knee_direction);
        bend -= direction * bend.dot(direction);
    }
    if bend.length_squared() < 1e-8 {
        let axis = if direction.z.abs() < 0.9 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        bend = direction.cross(axis);
    }
    let solved_mid = root + direction * along + bend.normalize() * height;
    let solved_tip = root + direction * distance;
    let (_, qa, _) = old[a].to_scale_rotation_translation();
    let (_, qb, _) = old[b].to_scale_rotation_translation();
    let (_, qc, _) = old[c].to_scale_rotation_translation();
    let (_, qt, _) = target.to_scale_rotation_translation();
    let ra = Quat::from_rotation_arc((mid - root).normalize(), (solved_mid - root).normalize());
    let rb = Quat::from_rotation_arc(
        (tip - mid).normalize(),
        (solved_tip - solved_mid).normalize(),
    );
    pose[a] = Mat4::from_rotation_translation((ra * qa).normalize(), root);
    pose[b] = Mat4::from_rotation_translation((rb * qb).normalize(), solved_mid);
    pose[c] = Mat4::from_rotation_translation(qc.slerp(qt, w).normalize(), solved_tip);
    for i in a + 1..pose.len() {
        if i == b || i == c {
            continue;
        }
        let parent = bones[i].parent;
        if parent >= 0 && parent as usize >= a {
            let parent = parent as usize;
            pose[i] = pose[parent] * old[parent].inverse() * old[i];
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrapped_rule_envelopes_and_compressed_fractional_errors() {
        assert_eq!(envelope(0.05, 0.8, 0.9, 1.1, 1.2).0, 1.);
        assert_eq!(envelope(0.5, 0.8, 0.9, 1.1, 1.2).0, 0.);
        assert!((envelope(0.85, 0.8, 0.9, 1.1, 1.2).0 - 0.5).abs() < 1e-5);
        let mut bytes = vec![0u8; 192];
        bytes[60..64].copy_from_slice(&152i32.to_le_bytes());
        bytes[152..156].copy_from_slice(&0.5f32.to_le_bytes());
        bytes[176..178].copy_from_slice(&36i16.to_le_bytes());
        bytes[188..192].copy_from_slice(&[1, 2, 8, 0]);
        let target = error_pose(&Rd(&bytes), 0, 0.5).unwrap().unwrap();
        assert_eq!(target.w_axis.truncate(), Vec3::X * 4.);
        bytes[189] = 0;
        assert!(error_pose(&Rd(&bytes), 0, 0.).is_err());
    }
    #[test]
    fn solver_reaches_preserves_lengths_and_descendants_without_nan() {
        let bones: Vec<_> = (0..5)
            .map(|i| Bone {
                name: format!("{i}"),
                parent: match i {
                    0 | 4 => -1,
                    _ => i - 1,
                },
                pos: Vec3::X,
                quat: Quat::IDENTITY,
                rot: Vec3::ZERO,
                pos_scale: Vec3::ONE,
                rot_scale: Vec3::ONE,
                pose_to_bone: Mat4::IDENTITY,
                flags: 0,
                proc_type: 0,
                quat_interp: None,
            })
            .collect();
        let chain = IkChain {
            name: "arm".into(),
            bones: [0, 1, 2],
            knee_direction: Vec3::Y,
        };
        for target in [Vec3::new(1., 1., 0.), Vec3::X * 100., Vec3::ZERO] {
            let mut pose: Vec<_> = (0..5)
                .map(|i| Mat4::from_translation(Vec3::X * i as f32))
                .collect();
            let old = pose.clone();
            assert!(solve_chain(
                &bones,
                &mut pose,
                &chain,
                Mat4::from_translation(target),
                1.
            ));
            assert!(pose.iter().all(|m| m.is_finite()));
            assert!((pose[0].w_axis.distance(pose[1].w_axis) - 1.).abs() < 1e-4);
            assert!((pose[1].w_axis.distance(pose[2].w_axis) - 1.).abs() < 1e-4);
            assert!((pose[2].inverse() * pose[3]).abs_diff_eq(old[2].inverse() * old[3], 1e-4));
            assert_eq!(pose[4], old[4]);
            if target.length() > 0. && target.length() < 2. {
                assert!(pose[2].w_axis.truncate().distance(target) < 1e-4);
            }
        }
    }
}
