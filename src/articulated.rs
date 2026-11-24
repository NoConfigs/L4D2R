//! Local rigid joint solver. Uses owned frames/limits, not the native IVP kernel.
use crate::{collision::Collision, rigid_contact};
use bevy::prelude::*;
#[cfg(test)]
#[path="articulated_event_trial.rs"]
mod event_trial;
#[cfg(test)]
pub(crate) use event_trial::PersistentTrial;
/// The solver owns a full physical endpoint. The public body fields hold
/// the sampled pose consumed by rendering; never feed that sample back into
/// the following constraint step. Native free-core readbacks verify this
/// distinction (ivp-world-1022-clock-audit.json).
#[derive(Clone, Copy)]
pub(crate) struct PoseCache {
    position: Vec3,
    rotation: Quat,
    published_position: Vec3,
    published_rotation: Quat,
    core_angular: Option<Vec3>,
    angular_world: Vec3,
}
#[derive(Clone)]
pub struct Rigid {
    pub bone: usize,
    pub position: Vec3,
    pub rotation: Quat,
    pub velocity: Vec3,
    pub angular: Vec3,
    pub center: Vec3,
    pub inverse_mass: f32,
    pub inverse_inertia: Mat3,
    pub vertices: Vec<Vec3>,
    pub hulls: Vec<rigid_contact::Hull>,
    pub damping: f32,
    pub angular_damping: f32,
    pub surface: crate::physics_data::Surface,
    // Per-hull-vertex persistent world-contact normals. Revalidated against
    // current collision every step; never retain a stale support plane.
    pub(crate) world_contacts: Vec<Vec3>,
    pub(crate) pose_cache: Option<PoseCache>,
}
#[derive(Clone)]
pub struct Joint {
    pub a: usize,
    pub b: usize,
    pub anchor_a: Vec3,
    pub anchor_b: Vec3,
    /// Joint axes in parent's local renderer basis.
    pub frame: Quat,
    pub min: Vec3,
    pub max: Vec3,
    pub friction: Vec3,
    pub(crate) native: Option<crate::ivp_joint::Frames>,
    pub(crate) hinge_target: f32,
}
#[derive(Clone)]
pub struct Articulation {
    pub bodies: Vec<Rigid>,
    pub joints: Vec<Joint>,
    pub collision_pairs: Vec<(usize, usize)>,
    /// Initial intrusions wait for geometric separation before contact is
    /// enabled. None means the group's initial geometry has not been checked.
    pub(crate) initial_intrusions: Option<Vec<(usize, usize)>>,
    pub(crate) pending_penetrations: Vec<(usize, usize)>,
    pub friction: f32,
    pub elasticity: f32,
}
fn inverse(body: &Rigid) -> Mat3 {
    let r = Mat3::from_quat(body.rotation);
    r * body.inverse_inertia * r.transpose()
}
fn integrate_rotation(body:&mut Rigid,dt:f32, core_angular:Option<Vec3>) -> Option<Vec3> {
    let inv=Vec3::new(body.inverse_inertia.x_axis.x,body.inverse_inertia.y_axis.y,body.inverse_inertia.z_axis.z);
    if inv.is_finite() && inv.min_element()>0. && body.inverse_inertia==Mat3::from_diagonal(inv) {
        // Owned compact PHY moments use the object's core axes. Unknown full
        // hull tensors do not establish that native frame and keep their path.
        let differences=Vec3::new(inv.y-inv.z,inv.z-inv.x,inv.x-inv.y);
        let square=(inv.x*inv.x+inv.y*inv.y)+inv.z*inv.z;
        let spread=(differences.x*differences.x+differences.y*differences.y)+differences.z*differences.z;
        let gyro=square*f32::from_bits(0x3c23d70b)<=spread; // owned 100f06b0
        let basis=Vec3::new(1.,-1.,-1.);
        let local=core_angular.unwrap_or_else(|| (body.rotation.conjugate()*body.angular)*basis);
        let step=crate::ivp_motion::angular_step(local,inv.recip(),inv,dt,gyro);
        let q=body.rotation;
        let end=crate::ivp_motion::rotation_end([q.x,-q.y,-q.z,q.w],step.delta);
        body.rotation=Quat::from_xyzw(end[0],-end[1],-end[2],end[3]);
        body.angular=body.rotation*(step.angular*basis);
        Some(step.angular)
    } else {
        body.rotation=(Quat::from_scaled_axis(body.angular*dt)*body.rotation).normalize();
        None
    }
}
fn cross(v: Vec3) -> Mat3 {
    Mat3::from_cols(v.cross(Vec3::X), v.cross(Vec3::Y), v.cross(Vec3::Z))
}
fn rotate(body: &mut Rigid, delta: Vec3) {
    if delta.is_finite() {
        body.rotation =
            (Quat::from_scaled_axis(delta.clamp_length_max(0.25)) * body.rotation).normalize();
    }
}
fn angular_energy(body: &Rigid) -> f32 {
    if body.angular == Vec3::ZERO || body.inverse_inertia.determinant().abs() < 1e-12 {
        return 0.;
    }
    let local = body.rotation.conjugate() * body.angular;
    0.5 * local.dot(body.inverse_inertia.inverse() * local)
}
fn preserve_projection_energy(body: &mut Rigid, before: f32) {
    let after = angular_energy(body);
    if before.is_finite() && after.is_finite() && after > before + 1e-6 {
        body.angular *= (before.max(0.) / after).sqrt();
    }
}
fn kinetic_energy(body: &Rigid) -> f32 {
    angular_energy(body)
        + if body.inverse_mass > 0. {
            body.velocity.length_squared() * 0.5 / body.inverse_mass
        } else {
            0.
        }
}
fn cap_projection_energy(bodies: &mut [Rigid], budget: f32) {
    let energy = bodies.iter().map(kinetic_energy).sum::<f32>();
    if budget.is_finite() && energy.is_finite() && energy > budget + 1e-6 {
        let scale = (budget.max(0.) / energy).sqrt();
        for body in bodies {
            body.velocity *= scale;
            body.angular *= scale;
        }
    }
}
fn pair(bodies: &mut [Rigid], a: usize, b: usize) -> (&mut Rigid, &mut Rigid) {
    assert_ne!(a, b);
    if a < b {
        let (left, right) = bodies.split_at_mut(b);
        (&mut left[a], &mut right[0])
    } else {
        let (left, right) = bodies.split_at_mut(a);
        (&mut right[0], &mut left[b])
    }
}
fn anchor(joint: &Joint, a: &mut Rigid, b: &mut Rigid) {
    let ra = a.rotation * joint.anchor_a;
    let rb = b.rotation * joint.anchor_b;
    let error = b.position + rb - a.position - ra;
    // Native ball-socket drift is corrected by next-step velocity bias,
    // not by translating the presentation pose after each integration.
    // Keep the existing large-separation recovery while the complete coupled
    // contact/manifold kernel remains under reconstruction.
    if error.length_squared() < 0.001f32.powi(2) { return; }
    let ia = inverse(a);
    let ib = inverse(b);
    let k = Mat3::IDENTITY * (a.inverse_mass + b.inverse_mass)
        - cross(ra) * ia * cross(ra)
        - cross(rb) * ib * cross(rb);
    if k.determinant().abs() < 1e-12 {
        return;
    }
    let impulse = k.inverse() * error * 0.8;
    a.position += impulse * a.inverse_mass;
    b.position -= impulse * b.inverse_mass;
    rotate(a, ia * ra.cross(impulse));
    rotate(b, -ib * rb.cross(impulse));
}
pub fn angles(joint: &Joint, a: &Rigid, b: &Rigid) -> Vec3 {
    let conversion = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let relative = conversion.conjugate()
        * joint.frame.conjugate()
        * a.rotation.conjugate()
        * b.rotation
        * conversion;
    // Source creates the joint with the child as reference and parent as
    // attached (ragdoll_shared.cpp). Limits therefore describe the attached
    // joint frame relative to the child, not child relative to parent.
    let (x, y, z) = relative.conjugate().normalize().to_euler(EulerRot::XYZ);
    Vec3::new(x, y, z)
}
/// IVP joint limits act predictively, like speculative contacts: before a
/// step moves the bodies, the relative angular velocity on each limited axis
/// is clamped so the angle reaches the limit, never past it, by the end of
/// the step. Owned vphysics.dll (tools/native_ivp_probe.cpp --joint):
/// 42.858 deg at 158.9 deg/s -> 126.6 deg/s (=(45-42.858)*60 -> 128.5),
/// reaching 44.973; 18.775 deg toward a 20 deg limit -> 72.2 (73.5).
fn predict_limits(j: &Joint, a: &mut Rigid, b: &mut Rigid, dt: f32) {
    if dt <= 0. {
        return;
    }
    if let Some(frames) = &j.native {
        let ia = inverse(a); let ib = inverse(b);
        let rows = frames.rows(a,b);
        // Owned 100b9290 solves twist, plane, then cone.
        for axis in [0, 2, 1] {
            let (coordinate, n, factor) = rows[axis];
            let k = n.dot((ia + ib) * n);
            if k <= 1e-8 || factor <= 1e-8 { continue; }
            let relative = (a.angular - b.angular).dot(n);
            let low = (frames.min[axis] - coordinate) / (dt * factor);
            let high = (frames.max[axis] - coordinate) / (dt * factor);
            let target = relative.clamp(low.min(high), low.max(high));
            let impulse = n * ((relative - target) / k);
            a.angular -= ia * impulse;
            b.angular += ib * impulse;
        }
        return;
    }
    let value = angles(j, a, b);
    let frame = b.rotation * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let ia = inverse(a);
    let ib = inverse(b);
    for axis in 0..3 {
        let locked = j.max[axis] - j.min[axis] <= 1e-5;
        let mut basis = Vec3::ZERO;
        basis[axis] = 1.;
        let n = frame * basis;
        let k = n.dot((ia + ib) * n);
        if k <= 1e-8 {
            continue;
        }
        // The angle grows as the relative angular speed on n falls (see
        // joint_velocity's limit test), so value' ~= -relative.
        let relative = (b.angular - a.angular).dot(n);
        // Owned 100ba490 / 100b9290 use projected swing coordinates,
        // not an Euler-angle boundary. In the Source frame, pitch uses
        // the sine plane and yaw the cosine cone. Compound-axis Jacobians
        // and native axis-selection permutations remain a separate gap.
        let target = if locked {
            (value[axis] - j.min[axis]) / dt
        } else if axis == 1 {
            let cosine = value[axis].cos();
            if cosine.abs() > 1e-5 {
                let coordinate = value[axis].sin();
                let low = (coordinate - j.max[axis].sin()) / (dt * cosine);
                let high = (coordinate - j.min[axis].sin()) / (dt * cosine);
                relative.clamp(low.min(high), low.max(high))
            } else { relative }
        } else if axis == 2 {
            let center = (j.min[axis] + j.max[axis]) * 0.5;
            let theta = value[axis] - center;
            let sine = theta.sin();
            if sine.abs() > 1e-5 {
                let coordinate = theta.cos();
                let low = (((j.max[axis] - j.min[axis]) * 0.5).cos() - coordinate) / (dt * sine);
                let high = (1. - coordinate) / (dt * sine);
                relative.clamp(low.min(high), low.max(high))
            } else { relative }
        } else {
            relative.clamp((value[axis] - j.max[axis]) / dt, (value[axis] - j.min[axis]) / dt)
        };
        if target != relative {
            let impulse = n * ((relative - target) / k);
            a.angular += ia * impulse;
            b.angular -= ib * impulse;
        }
    }
}
/// Ball-socket velocity constraint: equal anchor velocities on both bodies.
/// Solved before integration as well, as IVP does; with it the first step of
/// the owned-DLL spinning pair is exact (62.727 / -54.545 deg/s).
fn anchor_velocity(j: &Joint, a: &mut Rigid, b: &mut Rigid, dt: f32) {
    let ra = a.rotation * j.anchor_a;
    let rb = b.rotation * j.anchor_b;
    let ia = inverse(a);
    let ib = inverse(b);
    let k = Mat3::IDENTITY * (a.inverse_mass + b.inverse_mass)
        - cross(ra) * ia * cross(ra)
        - cross(rb) * ib * cross(rb);
    if k.determinant().abs() > 1e-12 {
        let relative = b.velocity + b.angular.cross(rb) - a.velocity - a.angular.cross(ra);
        // Owned 100b0880 / 100ba210: stiffness=tau=1 for stock
        // ragdolls (10088e20); error * inverse timestep is part of the RHS.
        let error = b.position + rb - a.position - ra;
        let bias = if dt > 0. { error / dt } else { Vec3::ZERO };
        let impulse = k.inverse() * (relative + bias);
        rigid_contact::apply(&mut a.velocity, &mut a.angular, a.inverse_mass, ia, ra, impulse);
        rigid_contact::apply(&mut b.velocity, &mut b.angular, b.inverse_mass, ib, rb, -impulse);
    }
}
fn joint_velocity(j: &Joint, a: &mut Rigid, b: &mut Rigid, dt: f32, motor_impulse: &mut Vec3) {
    let ia = inverse(a);
    let ib = inverse(b);
    if let Some(frames) = &j.native {
        let rows = frames.rows(a,b);
        for axis in [0, 2, 1] {
            let (coordinate, n, factor) = rows[axis];
            let k = n.dot((ia + ib) * n);
            if k <= 1e-8 || factor <= 1e-8 { continue; }
            let locked = frames.max[axis] - frames.min[axis] <= 1e-5;
            if locked && dt > 0. {
                let relative = (a.angular - b.angular).dot(n);
                let target = (frames.min[axis] - coordinate) / (dt * factor);
                let impulse = n * ((relative - target) / k);
                a.angular -= ia * impulse;
                b.angular += ib * impulse;
            }
            let friction = j.friction[frames.source_axes[axis]].abs();
            if !locked && friction > 0. && b.inverse_mass > 0. && dt > 0. {
                let bound = friction / b.inverse_mass * dt;
                let relative = (a.angular - b.angular).dot(n);
                let next = (motor_impulse[axis] + relative / k).clamp(-bound,bound);
                let impulse = n * (next - motor_impulse[axis]);
                motor_impulse[axis] = next;
                a.angular -= ia * impulse;
                b.angular += ib * impulse;
            }
        }
        return;
    }
    let value = angles(j, a, b);
    let conversion = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let frame = b.rotation * conversion;
    for axis in 0..3 {
        let mut basis = Vec3::ZERO;
        basis[axis] = 1.;
        let n = frame * basis;
        let relative = (b.angular - a.angular).dot(n);
        let unlocked = j.max[axis] - j.min[axis] > 1e-5;
        // Unlocked limits are enforced predictively before integration
        // (predict_limits), as IVP does; clamping here too stopped the joint
        // one step earlier than native.
        let limited = !unlocked;
        let k = n.dot((ia + ib) * n);
        if k > 1e-8 {
            if limited {
                // Locked native rows also carry angular position error;
                // cancelling velocity alone cannot repair a contact-displaced
                // locked joint after pose projection is removed.
                let target = if dt > 0. { (value[axis] - j.min[axis]) / dt } else { 0. };
                let impulse = n * ((relative - target) / k);
                a.angular += ia * impulse;
                b.angular -= ib * impulse;
            }
            if unlocked && j.friction[axis] != 0. && b.inverse_mass > 0. && dt > 0. {
                // Owned IVP constructor10088e20: abs(reference_mass*raw).
                // Reference is child b. Solver100b9290 multiplies by dt.
                // Clamp the accumulated motor impulse, not each iteration.
                let bound = j.friction[axis].abs() / b.inverse_mass * dt;
                let speed = (b.angular - a.angular).dot(n);
                let next = (motor_impulse[axis] + speed / k).clamp(-bound, bound);
                let impulse = n * (next - motor_impulse[axis]);
                motor_impulse[axis] = next;
                a.angular += ia * impulse;
                b.angular -= ib * impulse;
            }
        }
    }
}
struct ContactGeometry {
    lo: Vec3,
    hi: Vec3,
    hulls: Option<Vec<rigid_contact::Hull>>,
}
struct ContactAudit {
    corrections: Vec<usize>,
    depth: Vec<f32>,
    iterations: usize,
}
// Opt-in test diagnostics never run in the playable build.
#[cfg(test)]
thread_local! {
    pub(crate) static WORLD_IMPACT_AUDIT: std::cell::RefCell<Option<Vec<serde_json::Value>>> = const { std::cell::RefCell::new(None) };
}
struct WorldContact {
    body: usize,
    arm: Vec3,
    normal: Vec3,
    target: f32,
    impact: bool,
    impulse: f32,
    friction: Vec3,
    coefficient: f32,
}
fn world_velocity(b: &mut Rigid, contact: &mut WorldContact) {
    let inertia = inverse(b);
    let arm = contact.arm;
    let n = contact.normal;
    let velocity = b.velocity + b.angular.cross(arm);
    let k = rigid_contact::effective_mass(b.inverse_mass, inertia, arm, n).max(1e-6);
    let next = (contact.impulse + (contact.target - velocity.dot(n)) / k).max(0.);
    rigid_contact::apply(
        &mut b.velocity,
        &mut b.angular,
        b.inverse_mass,
        inertia,
        arm,
        n * (next - contact.impulse),
    );
    contact.impulse = next;
    let velocity = b.velocity + b.angular.cross(arm);
    let tangent = velocity - n * velocity.dot(n);
    let length = tangent.length();
    let mut next = contact.friction;
    if length > 1e-6 {
        let t = tangent / length;
        let k = rigid_contact::effective_mass(b.inverse_mass, inertia, arm, t).max(1e-6);
        next -= t * (length / k);
    }
    next = next.clamp_length_max(contact.coefficient * contact.impulse);
    rigid_contact::apply(
        &mut b.velocity,
        &mut b.angular,
        b.inverse_mass,
        inertia,
        arm,
        next - contact.friction,
    );
    contact.friction = next;
}
/// Half-extent of the swept box each hull vertex uses for world contacts.
/// IVP keeps a 0.25-inch (6.35 mm) collision distance: the owned
/// vphysics.dll reflects every impact with the surface 0.2499 Source units
/// away. With the projection's 0.5 mm push and the trace epsilon this box
/// leaves the same gap.
pub(crate) const CONTACT_HALF: f32 = 0.00485;
// Owned vphysics globals 1018dca4 / 1018dcb4 after initialization.
// 100ff6f0 / 1011cc90 use gain 1 inside the target gap, 20 outside.
const FRICTION_GAP: f32 = 0.0126949195;
const FRICTION_DISTANCE: f32 = 0.0285635684;
fn friction_gap_velocity(gap: f32) -> f32 {
    let error = FRICTION_GAP - gap;
    error * if error < 0. { 20. } else { 1. }
}
impl Articulation {
    fn initial_pair_waiting(&self,a:usize,b:usize) -> bool {
        self.initial_intrusions.as_ref().is_some_and(|pairs|pairs.contains(&(a,b))||pairs.contains(&(b,a)))
    }
    fn update_initial_intrusions(&mut self) {
        // Owned IVP crossing-box reference: a STATIC pair created touching/
        // overlapping has no contact response even while moving deeper in.
        // After it separates, subsequent entry collides normally. Never apply
        // this to two dynamic bodies: native runs penetration rescue for those.
        let intruding=|&(a,b):&(usize,usize)|
            (self.bodies[a].inverse_mass==0. || self.bodies[b].inverse_mass==0.) &&
            Self::self_contact(&self.bodies[a],&self.bodies[b]).is_some_and(|(_,depth,_)|depth>=0.);
        if let Some(pairs)=&mut self.initial_intrusions {pairs.retain(intruding);}
        else {self.initial_intrusions=Some(self.collision_pairs.iter().copied().filter(intruding).collect());}
    }
    pub fn recover_initial_pose(&mut self, collision: &Collision) {
        let clear = |offset: Vec3| {
            self.bodies.iter().all(|b| {
                b.vertices.iter().all(|v| {
                    let point = b.position + b.rotation * (*v - b.center) + offset;
                    !collision
                        .corpse_trace(point, point, Vec3::splat(CONTACT_HALF))
                        .start_solid
                })
            })
        };
        if clear(Vec3::ZERO) {
            return;
        }
        let mut recovery = None;
        'search: for step in 1..=18 {
            for direction in [
                Vec3::Y,
                Vec3::X,
                Vec3::NEG_X,
                Vec3::Z,
                Vec3::NEG_Z,
                Vec3::NEG_Y,
            ] {
                let offset = direction * (step as f32 * 0.02);
                if clear(offset) {
                    recovery = Some(offset);
                    break 'search;
                }
            }
        }
        if let Some(offset) = recovery {
            for body in &mut self.bodies {
                body.position += offset;
            }
        }
    }
    fn contact_geometry(b: &Rigid) -> ContactGeometry {
        let t = Transform::from_translation(b.position - b.rotation * b.center)
            .with_rotation(b.rotation);
        let (lo, hi) = b.vertices.iter().map(|v| t.transform_point(*v)).fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), v| (lo.min(v), hi.max(v)),
        );
        ContactGeometry {
            lo,
            hi,
            hulls: None,
        }
    }
    fn geometry_contact(a: &ContactGeometry, b: &ContactGeometry) -> Option<(Vec3, f32, Vec3)> {
        if !(a.hi + Vec3::splat(0.003)).cmpge(b.lo).all()
            || !(b.hi + Vec3::splat(0.003)).cmpge(a.lo).all()
        {
            return None;
        }
        a.hulls
            .as_ref()?
            .iter()
            .flat_map(|ha| {
                b.hulls
                    .as_ref()
                    .unwrap()
                    .iter()
                    .filter_map(move |hb| rigid_contact::convex_contact(ha, hb))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }
    fn self_contact(a: &Rigid, b: &Rigid) -> Option<(Vec3, f32, Vec3)> {
        let mut ga = Self::contact_geometry(a);
        let mut gb = Self::contact_geometry(b);
        if !Self::bounds_overlap(&ga, &gb) {
            return None;
        }
        Self::fill_hulls(&mut ga, a);
        Self::fill_hulls(&mut gb, b);
        Self::geometry_contact(&ga, &gb)
    }
    fn bounds_overlap(a: &ContactGeometry, b: &ContactGeometry) -> bool {
        (a.hi + Vec3::splat(0.003)).cmpge(b.lo).all()
            && (b.hi + Vec3::splat(0.003)).cmpge(a.lo).all()
    }
    fn fill_hulls(geometry: &mut ContactGeometry, b: &Rigid) {
        if geometry.hulls.is_none() {
            let t = Transform::from_translation(b.position - b.rotation * b.center)
                .with_rotation(b.rotation);
            geometry.hulls = Some(b.hulls.iter().map(|h| h.transformed(&t)).collect());
        }
    }
    fn self_positions(&mut self, mut audit: Option<&mut ContactAudit>) {
        // Same-ragdoll intrusions are sent to the native constraint group's
        // deferred rescue queue, not repeatedly projected apart here.
        if !self.joints.is_empty() { return; }
        let mut geometry = (0..self.bodies.len())
            .map(|_| None)
            .collect::<Vec<Option<ContactGeometry>>>();
        for (pair_index, &(a, b)) in self.collision_pairs.iter().enumerate() {
            if self.initial_pair_waiting(a,b) { continue; }
            if geometry[a].is_none() {
                geometry[a] = Some(Self::contact_geometry(&self.bodies[a]));
            }
            if geometry[b].is_none() {
                geometry[b] = Some(Self::contact_geometry(&self.bodies[b]));
            }
            if !Self::bounds_overlap(geometry[a].as_ref().unwrap(), geometry[b].as_ref().unwrap()) {
                continue;
            }
            Self::fill_hulls(geometry[a].as_mut().unwrap(), &self.bodies[a]);
            Self::fill_hulls(geometry[b].as_mut().unwrap(), &self.bodies[b]);
            let Some((n, depth, point)) = Self::geometry_contact(
                geometry[a].as_ref().unwrap(),
                geometry[b].as_ref().unwrap(),
            ) else {
                continue;
            };
            if depth <= 0.0005 {
                continue;
            }
            if let Some(stats) = audit.as_deref_mut() {
                stats.corrections[pair_index] += 1;
                stats.depth[pair_index] = stats.depth[pair_index].max(depth);
            }
            let (ai, bi) = (a, b);
            let (a, b) = pair(&mut self.bodies, a, b);
            let ra = point - a.position;
            let rb = point - b.position;
            let ia = inverse(a);
            let ib = inverse(b);
            let k = rigid_contact::effective_mass(a.inverse_mass, ia, ra, n)
                + rigid_contact::effective_mass(b.inverse_mass, ib, rb, n);
            if k < 1e-8 {
                continue;
            }
            let impulse = n * ((depth - 0.0005) * 0.8 / k);
            a.position -= impulse * a.inverse_mass;
            b.position += impulse * b.inverse_mass;
            rotate(a, -ia * ra.cross(impulse));
            rotate(b, ib * rb.cross(impulse));
            // Only the corrected bodies invalidate their geometry for later pairs.
            geometry[ai] = None;
            geometry[bi] = None;
        }
    }
    fn self_velocities(&mut self, contacts: &[(usize, usize, Vec3, Vec3)]) {
        for &(a, b, n, point) in contacts {
            let (a, b) = pair(&mut self.bodies, a, b);
            let ra = point - a.position;
            let rb = point - b.position;
            let ia = inverse(a);
            let ib = inverse(b);
            let v = b.velocity + b.angular.cross(rb) - a.velocity - a.angular.cross(ra);
            let inward = v.dot(n);
            if inward >= 0. {
                continue;
            }
            let k = rigid_contact::effective_mass(a.inverse_mass, ia, ra, n)
                + rigid_contact::effective_mass(b.inverse_mass, ib, rb, n);
            let magnitude = -inward / k.max(1e-6);
            rigid_contact::apply(
                &mut a.velocity,
                &mut a.angular,
                a.inverse_mass,
                ia,
                ra,
                -n * magnitude,
            );
            rigid_contact::apply(
                &mut b.velocity,
                &mut b.angular,
                b.inverse_mass,
                ib,
                rb,
                n * magnitude,
            );
        }
    }
    fn limit_world_impacts(&mut self,contacts:&[WorldContact],dt:f32) {
        // Only a newly applied impact invokes native10100030/101000b0.
        // Resting friction, free motion and queued async group rescue are
        // separate native paths and must not be continuously damped here.
        for contact in contacts.iter().filter(|c|c.impact && c.impulse>0.) {
            let body=&mut self.bodies[contact.body];
            if body.inverse_mass>0. {
                crate::ivp_impact::limit_render_impact(&mut body.velocity,&mut body.angular,dt);
            }
        }
    }
    pub fn anchor_error(&self) -> f32 {
        self.joints
            .iter()
            .map(|j| {
                let a = &self.bodies[j.a];
                let b = &self.bodies[j.b];
                (a.position + a.rotation * j.anchor_a)
                    .distance(b.position + b.rotation * j.anchor_b)
            })
            .fold(0., f32::max)
    }
    pub fn supported(&self, collision: &Collision) -> bool {
        self.bodies.iter().any(|b| {
            b.vertices.iter().any(|v| {
                let point = b.position + b.rotation * (*v - b.center);
                let hit =
                    collision.corpse_trace(point, point - Vec3::Y * 0.02, Vec3::splat(CONTACT_HALF));
                !hit.start_solid && hit.fraction < 1. && hit.normal.y > 0.6
            })
        })
    }
    pub fn step(&mut self, dt: f32, collision: &Collision) -> (bool, f32) {
        static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        self.step_with_cache(
            dt,
            collision,
            *CACHE.get_or_init(|| !std::env::args().any(|a| a == "--uncached-ragdoll-recovery")),
        )
    }
    pub(crate) fn step_with_cache(
        &mut self,
        dt: f32,
        collision: &Collision,
        reuse_overlap: bool,
    ) -> (bool, f32) {
        self.step_acceleration(dt, collision, reuse_overlap, Vec3::NEG_Y * 800. * crate::collision::UNIT)
    }
    pub(crate) fn step_acceleration(&mut self, dt: f32, collision: &Collision, reuse_overlap: bool, acceleration: Vec3) -> (bool, f32) {
        let _timing = crate::performance::scope("ragdoll::step");
        // Fresh embedded-vertex recovery searches allowed this step. A corpse
        // spawned inside c4m3's saferoom geometry ran hundreds a frame for
        // ~100 frames (~200 ms each); the rest wait for later steps.
        let mut recovery_budget = 64u32;
        let mut core_angular=vec![None;self.bodies.len()];
        if dt > 0. {
            for (index,b) in self.bodies.iter_mut().enumerate() {
                if let Some(cache) = b.pose_cache.take() {
                    // An authored relocation or external pose change resets
                    // prediction instead of restoring a stale physical pose.
                    if b.position == cache.published_position && b.rotation == cache.published_rotation {
                        b.position = cache.position;
                        b.rotation = cache.rotation;
                        if b.angular==cache.angular_world {core_angular[index]=cache.core_angular;}
                    }
                }
            }
        }

        // Native100af4d0 applies queued rescue AFTER its joint solves through
        // async accumulators: API velocity changes this tick, pose next tick.
        // Capture its PSI frame now, and publish velocity only after integration.
        let mut group_members=Vec::new();
        for joint in &self.joints {for body in [joint.a,joint.b] {if !group_members.contains(&body) {group_members.push(body);}}}
        let group_mass=group_members.iter().rev().map(|&i|&self.bodies[i]).filter(|b|b.inverse_mass>0.).map(|b|b.inverse_mass.recip()).sum::<f32>();
        let rescue=self.pending_penetrations.drain(..).filter(|(a,b)|group_members.contains(a)&&group_members.contains(b)).take(4).map(|(a,b)| {
            let delta=self.bodies[b].position-self.bodies[a].position;
            let direction=if delta.length()<=0.01 {Vec3::X} else {delta.normalize()};
            let impulse=direction*(2.*9.83*dt*group_mass);
            (a,b,impulse,self.bodies[a].rotation*Vec3::X*(-dt),self.bodies[b].rotation*Vec3::X*dt)
        }).collect::<Vec<_>>();
        static AUDIT_CONTACTS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let mut contact_audit = AUDIT_CONTACTS.get_or_init(|| std::env::args().any(|a| a=="--audit-ragdoll-contacts"))
            .then(|| ContactAudit { corrections:vec![0;self.collision_pairs.len()],depth:vec![0.;self.collision_pairs.len()],iterations:0 });
        for joint in &mut self.joints {
            if joint.native.is_none() {
                joint.native = crate::ivp_joint::Frames::prepare(joint, &self.bodies[joint.a], &self.bodies[joint.b]);
            }
        }
        self.update_initial_intrusions();
        // The independent-body friction solve is validated against native IVP.
        // Coupled manifold/joint impulses require a separate unfinished solve.
        let independent = self.joints.is_empty() && self.collision_pairs.is_empty();
        let previous = self
            .bodies
            .iter()
            .map(|b| (b.position, b.rotation))
            .collect::<Vec<_>>();
        for (index,b) in self.bodies.iter_mut().enumerate() {
            b.world_contacts.resize(b.vertices.len(), Vec3::ZERO);
            // Native PSI preparation commits external impulses before damping;
            // 100eabe0 then runs its damping controller before gravity.
            // Native movement-state/deactivation classification remains separate.
            if let Some(local)=&mut core_angular[index] {
                crate::ivp_damping::apply(&mut b.velocity,local,dt,
                    b.damping,Vec3::splat(b.angular_damping),1);
                b.angular=b.rotation*(*local*Vec3::new(1.,-1.,-1.));
            } else {
                crate::ivp_damping::apply(&mut b.velocity,&mut b.angular,dt,
                    b.damping,Vec3::splat(b.angular_damping),1);
            }
            b.velocity += acceleration * dt;
        }
        let angular_before_constraints=self.bodies.iter().map(|b|(b.rotation,b.angular)).collect::<Vec<_>>();
        // Persistent contacts counter gravity before integration. Current
        // geometry is re-queried so a removed/moving support is never retained.
        let mut supported = false;
        let mut resting = Vec::new();
        for (index, b) in self.bodies.iter_mut().enumerate() {
            for (vertex, v) in b.vertices.iter().enumerate() {
                let direction = b.world_contacts[vertex];
                if direction == Vec3::ZERO { continue; }
                let arm = b.rotation * (*v - b.center);
                let point = b.position + arm;
                let (hit, surface) = collision.corpse_contact(point, point - direction * FRICTION_DISTANCE,
                    Vec3::splat(CONTACT_HALF));
                if hit.start_solid || hit.fraction >= 1. || hit.normal.dot(direction) < 0.99 {
                    b.world_contacts[vertex] = Vec3::ZERO; continue;
                }
                let n = hit.normal;
                let plane = hit.plane_distance.unwrap_or_else(||
                    (point - direction * FRICTION_DISTANCE * hit.fraction).dot(n) - collision.trace_epsilon());
                let gap = point.dot(n) - plane + n.abs().element_sum() * CONTACT_HALF;
                if gap > FRICTION_DISTANCE { b.world_contacts[vertex] = Vec3::ZERO; continue; }
                supported |= n.y > 0.6;
                let coefficient = surface.map_or(self.friction, |environment| b.surface.contact_with(environment).0);
                resting.push(WorldContact { body: index, arm, normal: n, target: friction_gap_velocity(gap),
                    impact: false, impulse: 0., friction: Vec3::ZERO, coefficient });
            }
        }
        // Native100af4d0: prepare+initial solve forward, then two pairs of
        // reverse/forward sweeps. Preparing every hinge up front would capture
        // velocities before the preceding joint's initial solve.
        // Contact/joint coupling remains local; retain its existing contact
        // work per batch while correcting the actual constraint traversal.
        let mut hinges=(0..self.joints.len()).map(|_|None).collect::<Vec<Option<crate::ivp_hinge::Solver>>>();
        if self.joints.is_empty(){
            for _ in 0..if independent{8}else{5}{for contact in &mut resting{world_velocity(&mut self.bodies[contact.body],contact);}}
        }else{
            // Runtime currently uses the native default group. Additional
            // iterations require scaled constraint math before exposing them.
            for batch in crate::ivp_joint_group::Group::new(0).batches(){
                for slot in 0..self.joints.len(){
                    let index=if batch.reverse{self.joints.len()-1-slot}else{slot};
                    let j=&mut self.joints[index];let(a,b)=pair(&mut self.bodies,j.a,j.b);
                    if batch.prepare{hinges[index]=crate::ivp_hinge::Solver::prepare(j,a,b,dt);}
                    if let Some(hinge)=&hinges[index]{
                        if batch.prepare{hinge.limit(a,b,&mut j.hinge_target);}hinge.solve(a,b);
                    }else{predict_limits(j,a,b,dt);anchor_velocity(j,a,b,dt);}
                }
                for contact in &mut resting{world_velocity(&mut self.bodies[contact.body],contact);}
            }
        }
        for (index,b) in self.bodies.iter_mut().enumerate() {
            // Constraints advance the physical endpoint by a full step.
            // Sampling is separate below; a gravity-only pose correction
            // corrupts the equilibrium of continuing jointed contacts.
            b.position += b.velocity * dt;
            // Native keeps angular velocity in core coordinates. Retain that
            // state only while constraints/external impulses leave it untouched.
            let local=if angular_before_constraints[index]==(b.rotation,b.angular) {core_angular[index]} else {None};
            core_angular[index]=integrate_rotation(b,dt,local);
        }
        // Split positional corrections do no physical work. Reorienting an
        // anisotropic inertia tensor with unchanged world angular velocity can
        // otherwise add kinetic energy on every floor/joint projection.
        let projection_energy = self.bodies.iter().map(angular_energy).collect::<Vec<_>>();
        let integrated = self
            .bodies
            .iter()
            .map(|b| (b.position, b.rotation, b.velocity, b.angular))
            .collect::<Vec<_>>();
        let kinetic_budget = self.bodies.iter().map(kinetic_energy).sum::<f32>();
        // A wider clear sweep certifies a neighbourhood around the endpoint.
        // Reuse only clear space, never a hit plane across edges or moving steps.
        let mut clear_sweeps = self
            .bodies
            .iter()
            .map(|b| vec![(false, None::<(Vec3, Vec3)>); b.vertices.len()])
            .collect::<Vec<_>>();
        let mut iteration_start = vec![(Vec3::ZERO, Quat::IDENTITY); self.bodies.len()];
        let mut overlaps = self
            .bodies
            .iter()
            .map(|b| vec![false; b.vertices.len()])
            .collect::<Vec<_>>();
        let mut contact_patches = self
            .bodies
            .iter()
            .map(|b| vec![None::<crate::collision::ContactPatch>; b.vertices.len()])
            .collect::<Vec<_>>();
        let mut shared_patches =
            vec![Vec::<crate::collision::ContactPatch>::new(); self.bodies.len()];
        let mut contact_attempts = self
            .bodies
            .iter()
            .map(|b| vec![0u8; b.vertices.len()])
            .collect::<Vec<_>>();
        let mut recoveries = self
            .bodies
            .iter()
            .map(|b| vec![None::<(Vec3, Option<Vec3>)>; b.vertices.len()])
            .collect::<Vec<_>>();
        let mut clear_bodies = vec![None::<(Vec3, Vec3)>; self.bodies.len()];
        let mut body_attempts = vec![false; self.bodies.len()];
        // Bodies whose projection included a world-contact correction this step.
        let mut world_touch = vec![false; self.bodies.len()];
        let mut contact_normals = self
            .bodies
            .iter()
            .map(|b| vec![Vec3::ZERO; b.vertices.len()])
            .collect::<Vec<_>>();
        // Interleave anchored translations, physical angle limits and contacts.
        // No final presentation-only clamp can detach the skinned limbs.
        for iteration in 0..48 {
            if let Some(audit) = &mut contact_audit { audit.iterations=iteration+1; }
            for (start, b) in iteration_start.iter_mut().zip(&self.bodies) {
                *start = (b.position, b.rotation);
            }
            for j in &self.joints {
                let (a, b) = pair(&mut self.bodies, j.a, j.b);
                // Native angular limits act through velocity impulses; an
                // additional pose clamp erases their nonlinear response.
                anchor(j, a, b);
            }
            {
                let _timing = crate::performance::scope("ragdoll::self_contacts");
                self.self_positions(contact_audit.as_mut());
            }
            let _world_timing = crate::performance::scope("ragdoll::world_contacts");
            for (index, b) in self.bodies.iter_mut().enumerate() {
                if reuse_overlap && collision.supports_clear_certificates() {
                    let certified = clear_bodies[index].is_some_and(|(lo, hi)| {
                        b.vertices.iter().all(|v| {
                            let p = b.position + b.rotation * (*v - b.center);
                            p.cmpge(lo).all() && p.cmple(hi).all()
                        })
                    });
                    if !certified && !body_attempts[index] {
                        body_attempts[index] = true;
                        let (lo, hi) = b
                            .vertices
                            .iter()
                            .flat_map(|v| {
                                let local = *v - b.center;
                                [
                                    previous[index].0 + previous[index].1 * local,
                                    b.position + b.rotation * local,
                                ]
                            })
                            .fold(
                                (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                                |(lo, hi), p| (lo.min(p), hi.max(p)),
                            );
                        if lo.is_finite() && hi.is_finite() {
                            let lo = lo - Vec3::splat(0.024);
                            let hi = hi + Vec3::splat(0.024);
                            let center = (lo + hi) * 0.5;
                            let half = (hi - lo) * 0.5
                                + Vec3::splat(CONTACT_HALF + collision.trace_epsilon() + 0.00005);
                            let hit = collision.corpse_trace(center, center, half);
                            if !hit.start_solid && hit.fraction >= 1. {
                                clear_bodies[index] = Some((lo, hi));
                            }
                        }
                    }
                    if certified || (iteration == 0 && clear_bodies[index].is_some()) {
                        #[cfg(test)]
                        for v in &b.vertices {
                            let local = *v - b.center;
                            let hit = collision.corpse_trace(
                                previous[index].0 + previous[index].1 * local,
                                b.position + b.rotation * local,
                                Vec3::splat(CONTACT_HALF),
                            );
                            assert!(
                                !hit.start_solid && hit.fraction >= 1.,
                                "whole-body clearance changed a contact"
                            );
                        }
                        continue;
                    }
                }
                for vertex in 0..b.vertices.len() {
                    let local = b.vertices[vertex] - b.center;
                    let start = previous[index].0 + previous[index].1 * local;
                    let point = b.position + b.rotation * local;
                    let cache = &mut clear_sweeps[index][vertex];
                    if let Some((clear, margin)) = cache.1.filter(|_| reuse_overlap) {
                        if (point - clear).abs().cmple(margin).all() {
                            #[cfg(test)]
                            {
                                let reference =
                                    collision.corpse_trace(start, point, Vec3::splat(CONTACT_HALF));
                                assert!(
                                    !reference.start_solid && reference.fraction >= 1.,
                                    "certified clear sweep changed contact start={start:?} center={clear:?} margin={margin:?} end={point:?} result={reference:?}"
                                );
                            }
                            continue;
                        }
                        *cache = (false, None);
                    }
                    let half = Vec3::splat(CONTACT_HALF);
                    // The sweep start and collision snapshot are fixed for this
                    // whole solve. A confirmed start-solid result cannot change
                    // when later iterations adjust only the endpoint.
                    let cached_contact = if reuse_overlap {
                        contact_patches[index][vertex]
                            .and_then(|patch| patch.trace(start, point))
                            .or_else(|| {
                                if iteration > 1 {
                                    return None;
                                }
                                shared_patches[index].iter().find_map(|patch| {
                                    let hit = patch.trace(start, point)?;
                                    contact_patches[index][vertex] = Some(*patch);
                                    Some(hit)
                                })
                            })
                    } else {
                        None
                    };
                    #[cfg(test)]
                    if let Some(hit) = cached_contact {
                        let reference = collision.corpse_trace(start, point, half);
                        assert_eq!(
                            (hit.fraction, hit.normal, hit.start_solid),
                            (reference.fraction, reference.normal, reference.start_solid),
                            "floor cache start={start:?} end={point:?}"
                        );
                    }
                    let hit = if let Some(hit) = cached_contact {
                        hit
                    } else if reuse_overlap && overlaps[index][vertex] {
                        crate::collision::Trace {
                            fraction: 0.,
                            normal: Vec3::ZERO,
                            start_solid: true,
                            plane_distance: None,
                        }
                    } else {
                        collision.corpse_trace(start, point, half)
                    };
                    overlaps[index][vertex] |= hit.start_solid;
                    if reuse_overlap
                        && contact_attempts[index][vertex] < 2
                        && cached_contact.is_none()
                        && !hit.start_solid
                        && hit.fraction < 1.
                        && hit.normal.abs().element_sum() == 1.
                    {
                        contact_attempts[index][vertex] += 1;
                        if let Some(patch) = collision
                            .certify_contact_patch(start, point, half, 0.08)
                            .or_else(|| collision.certify_contact_patch(start, point, half, 0.024))
                        {
                            contact_patches[index][vertex] = Some(patch);
                            if shared_patches[index].len() < 8 {
                                shared_patches[index].push(patch);
                            }
                        }
                    }
                    if reuse_overlap
                        && collision.supports_clear_certificates()
                        && !hit.start_solid
                        && hit.fraction >= 1.
                        && !cache.0
                    {
                        cache.0 = true;
                        // Thin-axis certificates retain wide tangential reuse
                        // for clear vertices sliding very close to a surface.
                        for bits in [0, 2, 1, 4, 3, 6, 5, 7] {
                            let mut margin = Vec3::splat(0.024);
                            for axis in 0..3 {
                                if bits & (1 << axis) != 0 {
                                    margin[axis] = 0.00025;
                                }
                            }
                            // Include the trace kernel's plane epsilon, not
                            // only the physical hull, in the clear certificate.
                            let size =
                                margin + half + Vec3::splat(collision.trace_epsilon() + 0.00005);
                            let clearance = collision.corpse_trace(start, point, size);
                            if !clearance.start_solid && clearance.fraction >= 1. {
                                cache.1 = Some((point, margin));
                                break;
                            }
                        }
                    }
                    let correction = if hit.start_solid {
                        let _recovery_timing = crate::performance::scope("ragdoll::recovery");
                        let cached = recoveries[index][vertex];
                        let free = if reuse_overlap && cached.is_some_and(|(p, _)| p == point) {
                            cached.unwrap().1
                        } else if recovery_budget > 0 {
                            recovery_budget -= 1;
                            let free = collision.recover_corpse_position(point, half, 0.35);
                            recoveries[index][vertex] = Some((point, free));
                            free
                        } else {
                            None
                        };
                        free.map(|free| free - point)
                    } else if hit.fraction < 1. {
                        supported |= hit.normal.y > 0.6;
                        if hit.normal.dot(Vec3::Y) < 0.99 {
                            contact_normals[index][vertex] = hit.normal;
                        }
                        let surface = start + (point - start) * hit.fraction;
                        // A contact constrains motion along its normal. Rewinding
                        // the entire sweep also undid tangential joint corrections,
                        // making floor contacts fight the joint solver indefinitely.
                        let depth = hit
                            .plane_distance
                            .map_or((surface - point).dot(hit.normal), |d| {
                                d - point.dot(hit.normal)
                            })
                            + 0.0005;
                        (depth > 0.).then_some(hit.normal * depth)
                    } else {
                        None
                    };
                    if let Some(delta) = correction {
                        let depth = delta.length();
                        if depth < 1e-7 {
                            continue;
                        }
                        world_touch[index] = true;
                        let n = delta / depth;
                        let arm = b.rotation * local;
                        let inertia = inverse(b);
                        let k = rigid_contact::effective_mass(b.inverse_mass, inertia, arm, n)
                            .max(1e-6);
                        let impulse = n * (depth / k);
                        b.position += impulse * b.inverse_mass;
                        let turn = inertia * arm.cross(impulse);
                        rotate(b, turn);
                    }
                }
            }
            // Free-falling and already converged bodies do not need 24 full
            // collision passes. Contact or joint movement keeps the full cap.
            let settled_iteration = self.bodies.iter().zip(&iteration_start).all(|(b, (p, q))| {
                b.vertices.iter().all(|v| {
                    let local = *v - b.center;
                    (*p + *q * local).distance_squared(b.position + b.rotation * local)
                        < 0.0005f32.powi(2)
                })
            });
            if iteration >= 3 && settled_iteration && self.anchor_error() < 0.002 {
                break;
            }
        }
        // Reconcile projected motion with velocity, but never turn depenetration
        // or joint alignment into extra kinetic energy. Contacts then solve
        // together with joints, using physical pre-impact speeds for restitution.
        for (index, (b, energy)) in self.bodies.iter_mut().zip(projection_energy).enumerate() {
            preserve_projection_energy(b, energy);
            // Joint drift is corrected by position only (IVP keeps the
            // velocity-level solution); world contacts still turn their
            // projection into velocity for push-out and bounce.
            if dt > 0. && world_touch[index] {
                b.velocity += (b.position - integrated[index].0) / dt;
                let mut turn = b.rotation * integrated[index].1.conjugate();
                if turn.w < 0. {
                    turn = -turn;
                }
                b.angular += turn.normalize().to_scaled_axis() / dt;
            }
        }
        cap_projection_energy(&mut self.bodies, kinetic_budget);
        let mut world_contacts = Vec::new();
        for (index, b) in self.bodies.iter_mut().enumerate() {
            for (vertex, v) in b.vertices.iter().enumerate() {
                let arm = b.rotation * (*v - b.center);
                let point = b.position + arm;
                let old_normal = std::mem::replace(&mut b.world_contacts[vertex], Vec3::ZERO);
                let directions = [Vec3::Y, contact_normals[index][vertex], old_normal];
                for (i, direction) in directions.into_iter().enumerate() {
                    if directions[..i].contains(&direction) { continue; }
                    let continuing = old_normal.dot(direction) > 0.99;
                    let reach = if continuing { FRICTION_DISTANCE } else { 0.003 };
                    if direction == Vec3::ZERO {
                        continue;
                    }
                    let (hit, surface) = collision.corpse_contact(
                        point,
                        point - direction * reach,
                        Vec3::splat(CONTACT_HALF),
                    );
                    if hit.start_solid || hit.fraction >= 1. {
                        continue;
                    }
                    let n = hit.normal;
                    let velocity = integrated[index].2 + integrated[index].3.cross(arm);
                    let inward = velocity.dot(n);
                    let (friction, elasticity) = surface.map_or((self.friction, self.elasticity),
                        |environment| b.surface.contact_with(environment));
                    let plane = hit.plane_distance.unwrap_or_else(||
                        (point - direction * reach * hit.fraction).dot(n) - collision.trace_epsilon());
                    let gap = point.dot(n) - plane + n.abs().element_sum() * CONTACT_HALF;
                    if continuing && gap > FRICTION_DISTANCE { continue; }
                    b.world_contacts[vertex] = n; supported |= n.y > 0.6;
                    // Existing friction contacts counter gravity and correct
                    // their gap; restitution belongs to a new impact only.
                    let bounce = if !continuing && inward < -0.1 {
                        crate::physics_data::restitution(elasticity, -inward)
                    } else {
                        0.
                    };
                    world_contacts.push(WorldContact {
                        body: index,
                        arm,
                        normal: n,
                        target: if continuing { friction_gap_velocity(gap) } else { -bounce * inward.min(0.) },
                        impact: !continuing,
                        impulse: 0.,
                        friction: Vec3::ZERO,
                        coefficient: friction,
                    });
                }
            }
        }
        #[cfg(test)]
        WORLD_IMPACT_AUDIT.with(|audit| {
            if let Some(events)=audit.borrow_mut().as_mut() {
                for contact in &world_contacts {
                    let i=contact.body;let b=&self.bodies[i];
                    events.push(serde_json::json!({"body":i,"arm":contact.arm.to_array(),"normal":contact.normal.to_array(),
                        "target":contact.target,"impact":contact.impact,"friction":contact.coefficient,
                        "previous_position":previous[i].0.to_array(),"previous_rotation":previous[i].1.to_array(),
                        "integrated_position":integrated[i].0.to_array(),"integrated_rotation":integrated[i].1.to_array(),
                        "integrated_velocity":integrated[i].2.to_array(),"integrated_angular":integrated[i].3.to_array(),
                        "projected_position":b.position.to_array(),"projected_rotation":b.rotation.to_array(),
                        "projected_velocity":b.velocity.to_array(),"projected_angular":b.angular.to_array()}));
                }
            }
        });
        // Velocity iterations never change positions/rotations. Their contact
        // geometry is identical, so calculate it once rather than six times.
        let mut queued = Vec::new();
        let contacts = self
            .collision_pairs
            .iter()
            .filter_map(|&(a, b)| {
                if self.initial_pair_waiting(a,b) {return None;}
                let (n,depth,point)=Self::self_contact(&self.bodies[a], &self.bodies[b])?;
                if !self.joints.is_empty() && depth>0. {
                    if group_members.contains(&a) && group_members.contains(&b) && queued.len()<4 && !queued.contains(&(a,b)) && !queued.contains(&(b,a)) {queued.push((a,b));}
                    return None;
                }
                Some((a,b,n,point))
            })
            .collect::<Vec<_>>();
        self.pending_penetrations=queued;
        let mut motor_impulses = self.joints.iter().any(|j| j.friction != Vec3::ZERO)
            .then(|| vec![Vec3::ZERO; self.joints.len()]);
        let mut no_motor = Vec3::ZERO;
        // Free joints keep the exact pre-integration anchor solution; when
        // contact impulses act this step they must travel through the joints
        // in the same solve.
        let coupled = !world_contacts.is_empty() || !contacts.is_empty();
        let hinges=if coupled {self.joints.iter().map(|j|crate::ivp_hinge::Solver::prepare(j,&self.bodies[j.a],&self.bodies[j.b],dt)).collect::<Vec<_>>()} else {hinges};
        for _ in 0..16 {
            for (index, j) in self.joints.iter().enumerate() {
                let motor = match motor_impulses.as_mut() {
                    Some(motors) => &mut motors[index], None => &mut no_motor,
                };
                let (a, b) = pair(&mut self.bodies, j.a, j.b);
                if let Some(hinge)=&hinges[index] {
                    if coupled {hinge.solve(a,b);}
                    continue;
                }
                if coupled {
                    anchor_velocity(j, a, b, dt);
                }
                // Native angular constraints already ran in the five
                // pre-integration passes. With no contacts or motors there
                // is no later impulse to propagate: solving locked rows on
                // the newly displayed pose changes the next step's motion.
                if coupled || j.native.is_none() || j.friction != Vec3::ZERO {
                    joint_velocity(j, a, b, dt, motor);
                }
            }
            self.self_velocities(&contacts);
            for contact in &mut world_contacts {
                world_velocity(&mut self.bodies[contact.body], contact);
            }
        }
        cap_projection_energy(&mut self.bodies, kinetic_budget);
        // Time of impact: native IVP reflects a bouncing body at the moment of
        // contact and keeps moving it with the rebound speed for the rest of
        // the step (owned vphysics.dll drop: 5.84 -> contact -> 5.33 within
        // one 1/60 s step). The projection above already undid the inward
        // travel past contact; the share of the step it undid is the time left
        // after impact. Only bouncing contacts move; resting ones have no
        // outward speed.
        if dt > 0. {
            let mut travelled = false;
            for index in 0..self.bodies.len() {
                let Some(contact) = world_contacts
                    .iter()
                    .filter(|c| c.body == index && c.impact && c.target > 0.)
                    .max_by(|a, b| a.target.total_cmp(&b.target))
                else {
                    continue;
                };
                let n = contact.normal;
                let inward = -(integrated[index].2 + integrated[index].3.cross(contact.arm)).dot(n) * dt;
                if inward <= 1e-6 {
                    continue;
                }
                let b = &mut self.bodies[index];
                let after = ((b.position - integrated[index].0).dot(n) / inward).clamp(0., 1.);
                let lift = n * b.velocity.dot(n).max(0.) * after * dt;
                travelled |= lift != Vec3::ZERO;
                b.position += lift;
            }
            // Jointed partners move with the rebounding body (their solved
            // velocities agree at the anchor); close the joints it opened.
            if travelled {
                for _ in 0..8 {
                    for j in &self.joints {
                        let (a, b) = pair(&mut self.bodies, j.a, j.b);
                        anchor(j, a, b);
                    }
                }
            }
        }
        self.limit_world_impacts(&world_contacts,dt);
        // Rest is determined by actual body motion after contact projection,
        // not residual constraint velocity that may never move a supported body.
        let speed = self
            .bodies
            .iter()
            .zip(&previous)
            .map(|(b, (p, q))| {
                let linear = (b.position - *p).length_squared() / (dt * dt);
                let angular = b.rotation.angle_between(*q).powi(2) / (dt * dt);
                linear + angular * 0.01
            })
            .fold(0., f32::max);
        if let Some(audit) = contact_audit {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("local-research/parity/ivp-contact-1021-iterations.tsv") {
                static STEP: std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
                let step=STEP.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
                let _=writeln!(file,"queue\t{step}\t{}\t{}\t{group_mass}",self.pending_penetrations.len(),rescue.len());
                let mut pairs=(0..self.collision_pairs.len()).collect::<Vec<_>>();
                pairs.sort_by_key(|&i| std::cmp::Reverse(audit.corrections[i]));
                for &index in pairs.iter().take(5) {
                    let (a,b)=self.collision_pairs[index];
                    let _=writeln!(file,"{step}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",self.bodies.len(),audit.iterations,self.anchor_error(),speed,a,b,self.bodies[a].bone,self.bodies[b].bone,audit.corrections[index]);
                    let _=writeln!(file,"depth\t{step}\t{a}\t{b}\t{}",audit.depth[index]);
                }
            }
        }
        if dt > 0. {
            let basis = Vec3::new(1., -1., -1.);
            let native_quat = |q: Quat| [q.x, -q.y, -q.z, q.w];
            for (index, b) in self.bodies.iter_mut().enumerate() {
                let position = b.position;
                let rotation = b.rotation;
                // Free motion retains the true prediction anchor. Contact
                // projection currently owns its endpoint; exact event-time
                // cache restamping remains part of the unfinished CCD port.
                let projected = position != integrated[index].0;
                let start = if world_touch[index] || projected { position - b.velocity * dt }
                    else { previous[index].0 };
                let prediction = crate::ivp_motion::Prediction {
                    start: start * basis,
                    velocity: b.velocity * basis,
                    rotation_start: native_quat(previous[index].1),
                    rotation_end: native_quat(rotation),
                    time: 0., inverse_dt: dt.recip(),
                };
                let sample = prediction.sample(dt as f64 * 0.99,
                    crate::ivp_motion::ObjectFrame { shift: None, rotation: None });
                b.position = sample.position * basis;
                let convert = Mat3::from_diagonal(basis);
                b.rotation = Quat::from_mat3(&(convert * sample.rotation * convert)).normalize();
                b.pose_cache = Some(PoseCache { position, rotation,
                    published_position: b.position, published_rotation: b.rotation,
                    core_angular: if (rotation,b.angular)==(integrated[index].1,integrated[index].3) {core_angular[index]} else {None},
                    angular_world:b.angular });
            }
        }
        let mut rescue_activity=0.0f32;
        for (a,b,impulse,spin_a,spin_b) in rescue {
            let (a,b)=pair(&mut self.bodies,a,b);
            if a.inverse_mass>0. {a.velocity-=impulse*a.inverse_mass;a.angular+=spin_a;rescue_activity=rescue_activity.max(a.velocity.length_squared());}
            if b.inverse_mass>0. {b.velocity+=impulse*b.inverse_mass;b.angular+=spin_b;rescue_activity=rescue_activity.max(b.velocity.length_squared());}
        }
        (supported, speed.max(rescue_activity))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn angular_cache_is_invalidated_by_external_impulse_or_pose() {
        let empty=Collision::default();
        for relocate in [false,true] {
            let mut rig=drop_body();
            rig.bodies[0].inverse_inertia=Mat3::from_diagonal(Vec3::new(2.,3.,4.));
            rig.bodies[0].angular=Vec3::new(1.,-2.,3.);
            rig.step_acceleration(1./60.,&empty,true,Vec3::ZERO);
            assert!(rig.bodies[0].pose_cache.unwrap().core_angular.is_some());
            let mut reference=rig.clone();
            // A stale core value must never survive an external edit. The
            // otherwise identical reference retains its original cache.
            rig.bodies[0].pose_cache.as_mut().unwrap().core_angular=Some(Vec3::splat(500.));
            for value in [&mut rig,&mut reference] {
                if relocate {value.bodies[0].rotation=Quat::from_rotation_x(0.7);}
                else {value.bodies[0].angular+=Vec3::new(0.2,-0.4,0.8);}
                value.step_acceleration(1./60.,&empty,true,Vec3::ZERO);
            }
            assert_eq!(rig.bodies[0].rotation,reference.bodies[0].rotation);
            assert_eq!(rig.bodies[0].angular,reference.bodies[0].angular);
        }
    }
    fn drop_body() -> Articulation {
        let unit = crate::collision::UNIT;
        let radius = 4. * unit;
        let mut b = rigid(Vec3::Y * 96. * unit, 0.1);
        b.vertices = vec![Vec3::Y * radius, Vec3::NEG_Y * radius];
        b.inverse_inertia = Mat3::IDENTITY / (0.4 * 10. * radius * radius);
        Articulation { initial_intrusions: None, pending_penetrations: Vec::new(), bodies: vec![b], joints: vec![], collision_pairs: vec![], friction: 0.64, elasticity: 0.05 }
    }
    fn native_spinning_pair(masses: [f32; 2], rate: f32) -> Articulation {
        let unit = crate::collision::UNIT;
        let radius = 4. * unit;
        let mut bodies = Vec::new();
        for i in 0..2 {
            let mut b = rigid(Vec3::new(if i == 0 { -8.*unit } else { 8.*unit }, 96.*unit, 0.), masses[i].recip());
            b.vertices = vec![Vec3::ZERO];
            b.inverse_inertia = Mat3::IDENTITY / (0.4 * masses[i] * radius * radius);
            if i == 0 { b.angular = Vec3::NEG_Z * rate.to_radians(); }
            bodies.push(b);
        }
        Articulation { initial_intrusions: None, pending_penetrations: Vec::new(), bodies, joints: vec![Joint { native: None, hinge_target:0., a:0,b:1,anchor_a:Vec3::X*8.*unit,anchor_b:Vec3::NEG_X*8.*unit,
            frame:Quat::IDENTITY,min:Vec3::splat(-std::f32::consts::FRAC_PI_4),max:Vec3::splat(std::f32::consts::FRAC_PI_4),friction:Vec3::ZERO }],
            collision_pairs:vec![],friction:0.64,elasticity:0.05 }
    }
    #[test]
    fn group_rescue_matches_native_private_group_impulse_and_async_pose_timing() {
        // Owned100af450/100af4d0, queued once through the working private
        // ragdoll group in the isolated DLL fixture. Gravity controller is0;
        // native rescue still uses IVP environment gravity9.83, not sv_gravity.
        let mut rig=native_spinning_pair([10.,5.],0.);
        let mut orphan=rig.bodies[0].clone();orphan.inverse_mass=0.00001;
        rig.bodies.push(orphan); // Not registered in the native joint group.
        rig.pending_penetrations.push((0,1));
        let initial=rig.bodies.iter().map(|b|b.position).collect::<Vec<_>>();
        let (_,activity)=rig.step_acceleration(1./60.,&Collision::default(),true,Vec3::ZERO);
        for (i,expected) in [-19.3503933,38.7007866].into_iter().enumerate() {
            assert!((rig.bodies[i].velocity.x/crate::collision::UNIT-expected).abs()<0.00002);
            assert_eq!(rig.bodies[i].position,initial[i]);
            assert!(rig.bodies[i].rotation.angle_between(Quat::IDENTITY)<1e-6);
        }
        assert!((rig.bodies[0].angular.x.to_degrees()+0.954929709).abs()<0.00001);
        assert!((rig.bodies[1].angular.x.to_degrees()-0.954929709).abs()<0.00001);
        assert!(activity>0.1);
        rig.step_acceleration(1./60.,&Collision::default(),true,Vec3::ZERO);
        assert!(rig.bodies.iter().all(|b|b.velocity.length()<1e-6));
    }
    #[test]
    fn ragdoll_penetration_queue_is_bounded_and_deduplicates_reverse_pairs() {
        let mut rig=crossing_box_pair(0.,0.);
        let mut extra=rig.bodies[0].clone();extra.position=Vec3::ZERO;
        rig.bodies=vec![extra.clone(),extra.clone(),extra.clone(),extra];
        let j=native_spinning_pair([10.,5.],0.).joints.remove(0);
        rig.joints=(1..4).map(|b|Joint{a:0,b,anchor_a:Vec3::ZERO,anchor_b:Vec3::ZERO,..j.clone()}).collect();
        rig.collision_pairs=vec![(0,1),(1,0),(0,2),(0,3),(1,2),(1,3),(2,3)];
        rig.step_acceleration(1./60.,&Collision::default(),true,Vec3::ZERO);
        assert_eq!(rig.pending_penetrations,vec![(0,1),(0,2),(0,3),(1,2)]);
        assert!(rig.bodies.iter().all(|b|b.position==Vec3::ZERO));
    }
    fn crossing_box_pair(height: f32, velocity: f32) -> Articulation {
        let unit=crate::collision::UNIT;
        let mut bodies=vec![rigid(Vec3::Y*height*unit,0.1),rigid(Vec3::X*12.*unit,0.)];
        for (i,half) in [Vec3::new(16.,2.,2.),Vec3::new(2.,2.,16.)].into_iter().enumerate() {
            let vertices=[-1.,1.].into_iter().flat_map(|x|[-1.,1.].into_iter().flat_map(move |y|[-1.,1.].into_iter().map(move |z|Vec3::new(x,y,z)*half*unit))).collect::<Vec<_>>();
            bodies[i].vertices=vertices.clone();
            bodies[i].hulls=vec![rigid_contact::Hull {vertices,normals:vec![Vec3::X,Vec3::Y,Vec3::Z],edges:vec![Vec3::X,Vec3::Y,Vec3::Z]}];
        }
        // Owned native Poly box GetInertia values are kg*mÃƒâ€šÃ‚Â², not inchesÃƒâ€šÃ‚Â².
        bodies[0].inverse_inertia=Mat3::from_diagonal(Vec3::new(0.0778766349,0.550603747,0.550603747).recip());
        bodies[0].velocity=Vec3::Y*velocity*unit;
        Articulation {initial_intrusions:None, pending_penetrations: Vec::new(),bodies,joints:vec![],collision_pairs:vec![(0,1)],friction:0.,elasticity:0.}
    }
    #[test]
    fn native_initial_intrusions_do_not_invent_pushout_impulses() {
        for velocity in [0.,-0.1,-1.,-20.,1.,20.] {
            let mut rig=crossing_box_pair(3.8,velocity);
            for _ in 0..60 {rig.step_acceleration(1./60.,&Collision::default(),true,Vec3::ZERO);}
            let body=&rig.bodies[0];
            assert!((body.velocity.y/crate::collision::UNIT-velocity).abs()<1e-5);
            assert!(body.angular.length()<1e-6);
            assert_eq!(rig.bodies[1].position,Vec3::X*12.*crate::collision::UNIT);
        }
    }
    #[test]
    fn native_initial_intrusions_rearm_after_separation_not_on_later_penetration() {
        let mut rig=crossing_box_pair(3.8,20.);
        for _ in 0..30 {rig.step_acceleration(1./60.,&Collision::default(),true,Vec3::ZERO);}
        assert!(rig.initial_intrusions.as_ref().unwrap().is_empty());
        rig.bodies[0].velocity=Vec3::NEG_Y*20.*crate::collision::UNIT;
        for _ in 0..30 {rig.step_acceleration(1./60.,&Collision::default(),true,Vec3::ZERO);}
        // The owned reference also produces an angular/linear impact on
        // re-entry (step60). This is lifecycle coverage, not a trajectory gate.
        assert!(rig.bodies[0].angular.length()>0.01);
        assert!(rig.bodies[0].velocity.y > -19.*crate::collision::UNIT);
        assert!(rig.initial_intrusions.as_ref().unwrap().is_empty());
    }
    #[test]
    fn initially_separated_pairs_do_not_disable_contact_when_they_overlap_later() {
        let mut rig=crossing_box_pair(16.,-60.);
        for _ in 0..13 {rig.step_acceleration(1./60.,&Collision::default(),true,Vec3::ZERO);}
        assert!(rig.initial_intrusions.as_ref().unwrap().is_empty());
        assert!(rig.bodies[0].angular.length()>0.01);
    }
    #[test]
    fn overlapping_dynamic_pairs_are_not_given_the_static_intrusion_exemption() {
        let mut rig=crossing_box_pair(3.8,-20.);
        rig.bodies[1].inverse_mass=0.1;
        rig.bodies[1].inverse_inertia=rig.bodies[0].inverse_inertia;
        rig.step_acceleration(1./60.,&Collision::default(),true,Vec3::ZERO);
        assert!(rig.initial_intrusions.as_ref().unwrap().is_empty());
        // Exact native dynamic penetration rescue is still a separate gap.
    }
    #[test]
    fn prepared_joint_frames_match_owned_native_default_and_asymmetric_readbacks() {
        let mut rig = native_spinning_pair([10.,5.],90.);
        let native = crate::ivp_joint::Frames::prepare(&rig.joints[0],&rig.bodies[0],&rig.bodies[1]).unwrap();
        assert_eq!(native.source_axes,[2,0,1]);
        let basis = Mat3::from_diagonal(Vec3::new(1.,-1.,-1.));
        let expected = Mat3::from_cols(Vec3::Y,Vec3::Z,Vec3::X);
        assert!((basis * native.reference - expected).to_cols_array().iter().all(|v| v.abs()<1e-6));
        rig.joints[0].min=Vec3::new(-10.,-30.,-50.)*std::f32::consts::PI/180.;
        rig.joints[0].max=Vec3::new(20.,40.,60.)*std::f32::consts::PI/180.;
        let native = crate::ivp_joint::Frames::prepare(&rig.joints[0],&rig.bodies[0],&rig.bodies[1]).unwrap();
        assert_eq!(native.source_axes,[2,1,0]);
        // Actual native 100bd600/100ba490 readbacks, IVP-local columns.
        let expected = Mat3::from_cols(Vec3::new(0.0871557221,0.99619472,0.),Vec3::new(0.99619472,-0.0871557221,0.),Vec3::Z);
        assert!((basis * native.reference - expected).to_cols_array().iter().all(|v| v.abs()<1e-6));
        assert!((native.min - Vec3::new(-1.04719758,0.819152057,-0.173648179)).length()<1e-6);
        assert!((native.max - Vec3::new(0.87266463,100.,0.342020124)).length()<1e-6);
    }
    #[test]
    fn compound_twist_plane_and_cone_match_owned_internal_solver_samples() {
        let rig=native_spinning_pair([10.,5.],90.);
        let basis=Mat3::from_diagonal(Vec3::new(1.,-1.,-1.));
        // Final step32 internal scratch vectors, asymmetric [60,90,30] spin.
        // These are native IVP vectors, not reconstructed presentation angles.
        let reference_x=Vec3::new(0.256459236,0.950978756,0.172823921);
        let attached_x=Vec3::new(-0.300458729,0.874339461,0.381123364);
        let reference_z=Vec3::new(0.157459527,-0.217521369,0.963270962);
        let attached_z=Vec3::new(-0.104339987,-0.427318633,0.898060083);
        let frames=crate::ivp_joint::Frames {
            reference:basis*Mat3::from_cols(reference_x,Vec3::ZERO,reference_z),
            attached:basis*Mat3::from_cols(attached_x,Vec3::ZERO,attached_z),
            min:Vec3::ZERO,max:Vec3::ONE,source_axes:[2,1,0],
        };
        let rows=frames.rows(&rig.bodies[0],&rig.bodies[1]);
        for (row,expected) in rows.iter().zip([0.269227475,0.820290089,0.129627466]) {
            assert!((row.0-expected).abs()<1e-6,"{} vs{expected}",row.0);
            assert!((row.1.length()-1.).abs()<1e-6);
        }
    }
    #[test]
    fn locked_joint_repairs_contact_angle_error_by_velocity_without_a_pose_clamp() {
        let a=rigid(Vec3::ZERO,0.);
        let mut b=rigid(Vec3::X,1.);b.rotation=Quat::from_rotation_x(0.25);
        let joint=Joint { native: None, hinge_target:0.,a:0,b:1,anchor_a:Vec3::X,anchor_b:Vec3::ZERO,frame:Quat::IDENTITY,
            min:Vec3::ZERO,max:Vec3::ZERO,friction:Vec3::ZERO};
        let mut rig=Articulation { initial_intrusions: None, pending_penetrations: Vec::new(),bodies:vec![a,b],joints:vec![joint],collision_pairs:vec![],friction:0.,elasticity:0.};
        let original=rig.bodies[1].rotation;
        rig.step_acceleration(0., &Collision::default(), true, Vec3::ZERO);
        assert_eq!(rig.bodies[1].rotation,original,"zero time must not clamp the pose");
        rig.step_acceleration(1./60., &Collision::default(), true, Vec3::ZERO);
        assert!(rig.bodies[1].pose_cache.unwrap().rotation.angle_between(Quat::IDENTITY)<1e-4);
        assert!((rig.bodies[1].rotation.angle_between(Quat::IDENTITY)-0.0025).abs()<0.0002);
        assert_eq!(rig.bodies[0].position,Vec3::ZERO);
        assert_eq!(rig.bodies[0].rotation,Quat::IDENTITY);
        assert!(rig.bodies[1].angular.is_finite());
    }
    #[test]
    fn native_joint_error_bias_matches_second_step_across_masses_and_spin_rates() {
        // Owned parameterised IVP probe, no contacts and before angular limits.
        // The previous no-bias solve missed the equal-mass/rate90 second step
        // by0.0746deg/s; it also translated the first-step centres artificially.
        for (masses, rate, expected) in [([10.,5.],90.,[62.504509,-54.3071594]),
            ([10.,5.],180.,[123.689194,-107.194855]),([10.,5.],360.,[237.231323,-203.474899]),
            ([10.,10.],90.,[48.9744797,-40.7926674]),([5.,20.],90.,[24.5401688,-16.3559227])] {
            let mut rig = native_spinning_pair(masses, rate);
            let collision = Collision::test_solid_floor();
            rig.step_acceleration(1./60., &collision, true, Vec3::ZERO);
            assert!((rig.bodies[0].position.x / crate::collision::UNIT + 8.).abs() < 1e-5);
            rig.step_acceleration(1./60., &collision, true, Vec3::ZERO);
            for (body, expected) in rig.bodies.iter().zip(expected) {
                assert!((-body.angular.z.to_degrees()-expected).abs()<0.02,"{masses:?}/{rate}: {} vs{expected}",-body.angular.z.to_degrees());
            }
        }
    }
    #[test]
    fn native_joint_swing_limit_preserves_recovered_prepare_and_four_solve_passes() {
        let mut rig = native_spinning_pair([10.,5.],90.);
        let collision = Collision::test_solid_floor();
        let samples = [(27,[44.2699966,-33.4049225]),(28,[28.8806839,-16.6880093]),
            (29,[7.83218336,6.10792685]),(30,[5.26897192,8.85425758])];
        for step in 1..=30 {
            rig.step_acceleration(1./60., &collision, true, Vec3::ZERO);
            if let Some((_,expected))=samples.iter().find(|s|s.0==step) {
                for (body,expected) in rig.bodies.iter().zip(expected) {
                    assert!((-body.angular.z.to_degrees()-expected).abs()<0.04,"step{step}: {} vs{expected}",-body.angular.z.to_degrees());
                }
            }
        }
    }
    #[test]
    fn sampled_pose_never_feeds_back_into_the_next_physical_step_or_teleport() {
        let mut b = rigid(Vec3::ZERO, 1.);
        b.velocity = Vec3::X;
        let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(),
            bodies: vec![b], joints: vec![], collision_pairs: vec![], friction:0., elasticity:0. };
        let dt=1./60.;
        for step in 1..=3 {
            rig.step_acceleration(dt,&Collision::default(),true,Vec3::ZERO);
            let b=&rig.bodies[0];
            assert!((b.pose_cache.unwrap().position.x-step as f32*dt).abs()<1e-7);
            assert!((b.position.x-(step as f32-0.01)*dt).abs()<1e-7);
        }
        rig.bodies[0].position=Vec3::X*10.;
        rig.step_acceleration(dt,&Collision::default(),true,Vec3::ZERO);
        assert!((rig.bodies[0].position.x-(10.+0.99*dt)).abs()<1e-6);
    }

    #[test]
    fn joined_bodies_keep_resting_floor_contacts_without_repeated_bounce() {
        let mut rig = native_spinning_pair([10., 5.], 0.);
        for b in &mut rig.bodies {
            b.vertices = vec![Vec3::Y, Vec3::NEG_Y].into_iter()
                .map(|v| v * (4. * crate::collision::UNIT)).collect();
        }
        let floor = Collision::test_solid_floor();
        for _ in 0..180 { rig.step(1./60., &floor); }
        let before = rig.bodies.iter().map(|b| b.position).collect::<Vec<_>>();
        for _ in 0..30 { rig.step(1./60., &floor); }
        for (b, p) in rig.bodies.iter().zip(before) {
            assert!(b.world_contacts.iter().any(|n| n.y > 0.99));
            assert!((b.position.y - p.y).abs() < 0.002, "{:?} vs {:?}", b.position, p);
            assert!(b.velocity.y.abs() < 0.02, "{:?}", b.velocity);
        }
    }

    #[test]
    fn independent_contact_matches_native_drop_bounce_and_rest_samples() {
        // Owned vphysics.dll contact fixture, fixed 60Hz, 4-inch sphere,
        // 96-inch drop. Native sleep explains the small final gap drift.
        let samples = [(1, 95.7799988, -13.333334), (28, 5.83994865, -373.333496),
            (29, 5.33487988, 86.4613419), (30, 6.555902, 73.1280136),
            (50, 4.44532633, 0.0553877354), (60, 4.45375347, 0.0468189791),
            (70, 4.46087694, 0.039576076), (90, 4.46279669, 0.), (240, 4.46279669, 0.)];
        let mut rig = drop_body();
        let floor = Collision::test_solid_floor();
        for step in 1..=240 {
            rig.step(1. / 60., &floor);
            if let Some((_, z, v)) = samples.iter().find(|s| s.0 == step) {
                let b = &rig.bodies[0];
                assert!((b.position.y / crate::collision::UNIT - z).abs() < 0.05, "step {step}: {} vs {z}", b.position.y / crate::collision::UNIT);
                assert!((b.velocity.y / crate::collision::UNIT - v).abs() < 0.1, "step {step}: {} vs {v}", b.velocity.y / crate::collision::UNIT);
            }
        }
        assert!(!rig.bodies[0].world_contacts.iter().all(|n| *n == Vec3::ZERO));
        assert!(friction_gap_velocity(FRICTION_GAP - 0.001) > 0.);
        assert!((friction_gap_velocity(FRICTION_GAP + 0.001) + 0.02).abs() < 1e-6);
    }
    #[test]
    fn world_impact_reduces_exceeded_native_limits_but_free_motion_is_not_damped() {
        let floor=Collision::test_solid_floor();
        let mut rig=drop_body();
        rig.bodies[0].position.y=8.*crate::collision::UNIT;
        rig.bodies[0].velocity=Vec3::new(203.2,-20.,0.);
        rig.step(1./60.,&floor);
        assert!(rig.bodies[0].velocity.length()<=50.2921);
        assert!(rig.bodies[0].angular.length()<=0.95199776*60.+0.0001);
        let mut free=drop_body();
        free.bodies[0].velocity=Vec3::X*203.2;
        free.step_acceleration(1./60.,&Collision::default(),true,Vec3::ZERO);
        assert_eq!(free.bodies[0].velocity,Vec3::X*203.2);
    }
    #[test]
    fn world_impact_limit_matches_owned_angular_callback_and_ignores_resting_contacts() {
        let mut rig=drop_body();
        let source=Vec3::new(9000.,1000.,-5000.)*(std::f32::consts::PI/180.);
        let angular=Vec3::new(source.x,source.z,-source.y);
        rig.bodies[0].angular=angular;
        let mut contact=WorldContact {body:0,arm:Vec3::ZERO,normal:Vec3::Y,target:0.,impact:false,impulse:1.,friction:Vec3::ZERO,coefficient:0.64};
        rig.limit_world_impacts(std::slice::from_ref(&contact),1./60.);
        assert_eq!(rig.bodies[0].angular,angular);
        contact.impact=true;
        rig.limit_world_impacts(std::slice::from_ref(&contact),1./60.);
        let expected=Vec3::new(2562.73022,-1423.73901,-284.747833)*(std::f32::consts::PI/180.);
        assert!(rig.bodies[0].angular.distance(expected)<0.00002);
        let limited=rig.bodies[0].angular;
        rig.limit_world_impacts(std::slice::from_ref(&contact),1./60.);
        assert_eq!(rig.bodies[0].angular,limited);
    }
    #[test]
    fn retained_contacts_release_removed_support_and_do_not_block_an_outward_launch() {
        let floor = Collision::test_solid_floor();
        let mut rig = drop_body();
        for _ in 0..120 { rig.step(1. / 60., &floor); }
        let before = rig.bodies[0].position.y;
        let mut launched = rig.clone();
        launched.bodies[0].velocity = Vec3::Y * 3.;
        launched.step(1. / 60., &floor);
        assert!(launched.bodies[0].position.y > before + 0.03);
        for _ in 0..4 { rig.step(1. / 60., &Collision::default()); }
        assert!(rig.bodies[0].position.y < before - 0.03);
        assert!(rig.bodies[0].world_contacts.iter().all(|n| *n == Vec3::ZERO));
    }
    #[test]
    fn joint_friction_uses_child_mass_dt_and_one_accumulated_budget() {
        let mut a = rigid(Vec3::ZERO, 0.1);
        let mut b = rigid(Vec3::ZERO, 0.2);
        a.inverse_inertia = Mat3::IDENTITY;
        b.inverse_inertia = Mat3::IDENTITY;
        a.angular = Vec3::X;
        let j = Joint { native: None, hinge_target:0., a:0,b:1,anchor_a:Vec3::ZERO,anchor_b:Vec3::ZERO,
            frame:Quat::IDENTITY,min:Vec3::splat(-3.),max:Vec3::splat(3.),friction:Vec3::new(-0.01,0.,0.) };
        let mut impulse = Vec3::ZERO;
        for _ in 0..16 { joint_velocity(&j, &mut a, &mut b, 1./60., &mut impulse); }
        let budget = 5. * 0.01 / 60.;
        assert!((impulse.x + budget).abs() < 1e-7);
        assert!((a.angular.x - (1.-budget)).abs() < 1e-7);
        assert!((b.angular.x - budget).abs() < 1e-7);
        assert_eq!(a.velocity, Vec3::ZERO);
        assert_eq!(b.velocity, Vec3::ZERO);
        let mut zero = j.clone(); zero.friction=Vec3::ZERO;
        let before=(a.angular,b.angular);
        joint_velocity(&zero, &mut a, &mut b, 1./60., &mut Vec3::ZERO);
        assert_eq!((a.angular,b.angular),before);
        // A hard limit remains uncapped even when friction is very small: the
        // predictive limit brings an over-limit joint back within one step.
        a.rotation = Quat::from_rotation_x(3.1);
        let value = angles(&j, &a, &b).x;
        predict_limits(&j, &mut a, &mut b, 1./60.);
        let n = b.rotation * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2) * Vec3::X;
        let next = value - (b.angular - a.angular).dot(n) / 60.;
        assert!(next >= j.min.x - 1e-4 && next <= j.max.x + 1e-4, "{value} -> {next}");
    }

    #[test]
    fn owned_zero_free_axis_setup_keeps_identity_and_native_cone_slack() {
        let rig = native_spinning_pair([10., 5.], 90.);
        let mut j = rig.joints[0].clone();
        j.min = Vec3::ZERO; j.max = Vec3::ZERO;
        let frames = crate::ivp_joint::Frames::prepare(&j, &rig.bodies[0], &rig.bodies[1]).unwrap();
        assert_eq!(frames.reference, Mat3::from_diagonal(Vec3::new(1., -1., -1.)));
        assert_eq!(frames.attached, frames.reference);
        assert_eq!(frames.source_axes, [0, 2, 1]);
        assert_eq!(frames.min, Vec3::new(0., 0.995004177, 0.));
        assert_eq!(frames.max, Vec3::new(0., 100., 0.));
        let mut a = rig.bodies[0].clone();
        a.rotation = Quat::from_rotation_z(0.05);
        let row = frames.rows(&a, &rig.bodies[1])[1];
        assert!(row.0 > frames.min.y && row.0 < 1.);
        assert_eq!(row.2, 0.1, "native small-angle Jacobian floor");
        assert!((row.1.length() - 1.).abs() < 1e-6);
        a.rotation = Quat::from_rotation_z(0.0001);
        assert_eq!(frames.rows(&a, &rig.bodies[1])[1].2, 0., "native degenerate cross product cutoff");
    }

    #[test]
    fn zero_free_joint_matches_owned_first_three_angular_steps() {
        let mut rig = native_spinning_pair([10., 5.], 90.);
        rig.joints[0].min = Vec3::ZERO;
        rig.joints[0].max = Vec3::ZERO;
        rig.bodies[0].angular = Vec3::new(60., 30., -90.) * std::f32::consts::PI / 180.;
        // World angular readbacks from the owned all-locked fixture. The
        // third PSI reaches the small-angle cone's predictive slowdown.
        for expected in [
            [Vec3::new(40.0000038,2.02308178,-62.7272758), Vec3::new(40.0000038,2.02181983,54.5454559)],
            [Vec3::new(39.9663162,2.94385791,-62.5148087), Vec3::new(40.0683098,0.904919207,54.2513313)],
            [Vec3::new(39.9600792,3.28192091,-50.6031075), Vec3::new(40.0942917,0.458834946,41.4551697)],
        ] {
            rig.step_acceleration(1./60., &Collision::default(), true, Vec3::ZERO);
            for (b, w) in rig.bodies.iter().zip(expected) {
                assert!((b.angular - w * std::f32::consts::PI / 180.)
                    .abs().max_element() < 0.01f32.to_radians(), "{:?} vs {:?}", b.angular, w);
            }
        }
    }

    #[test]
    #[ignore = "manual differential fixture against the owned 32-bit IVP helper"]
    fn export_native_ivp_comparison() {
        use std::fmt::Write;
        let unit = crate::collision::UNIT;
        let output = std::env::var("L4D2R_IVP_EXPORT").unwrap_or_else(|_|
            "local-research/parity/wound-native-1017/rewrite-ivp-fixtures-1018.csv".into());
        let spin_axis = std::env::var("L4D2R_IVP_JOINT_AXIS").unwrap_or_else(|_| "y".into());
        assert!(["x", "y", "z"].contains(&spin_axis.as_str()));
        let settings = |key: &str, defaults: Vec<f32>| {
            let values = std::env::var(key).map(|s| s.split(',').map(|v| v.parse::<f32>().unwrap()).collect::<Vec<_>>()).unwrap_or(defaults.clone());
            assert_eq!(values.len(), defaults.len());
            assert!(values.iter().all(|v| v.is_finite()));
            values
        };
        let spin = settings("L4D2R_IVP_JOINT_SPIN", "xyz".chars().map(|axis| if spin_axis.starts_with(axis) {90.} else {0.}).collect());
        let limits = settings("L4D2R_IVP_JOINT_LIMITS", vec![-45.,45.,-45.,45.,-45.,45.]);
        let torque=settings("L4D2R_IVP_JOINT_TORQUE",vec![0.;3]);
        let boxed = std::env::var_os("L4D2R_IVP_FIXTURE_BOX").is_some();
        assert!(limits.chunks_exact(2).all(|v| -180.<=v[0] && v[0]<=v[1] && v[1]<=180.));
        let mut report = String::from("case,spin_axis,step,body,x,y,z,qx,qy,qz,qw,vx,vy,vz,wx,wy,wz,anchor_error,spin_x,spin_y,spin_z,xmin,xmax,ymin,ymax,zmin,zmax,shape,ix,iy,iz,torque_x,torque_y,torque_z\n");
        for (case, joint, free) in [("contact-fixture", false, false), ("joint-fixture", true, false), ("joint-free", true, true)] {
            let radius = 4. * unit;
            let mut vertices = vec![Vec3::Y * radius, Vec3::NEG_Y * radius];
            for latitude in 1..8 {
                let theta = latitude as f32 * std::f32::consts::PI / 8.;
                for longitude in 0..16 {
                    let phi = longitude as f32 * std::f32::consts::TAU / 16.;
                    vertices.push(Vec3::new(theta.sin()*phi.cos(), theta.cos(), theta.sin()*phi.sin()) * radius);
                }
            }
            if boxed {
                vertices = [-1.,1.].into_iter().flat_map(|x| [-1.,1.].into_iter()
                    .flat_map(move |y| [-1.,1.].into_iter().map(move |z| Vec3::new(x,y,z)*radius))).collect();
            }
            let mut bodies = Vec::new();
            for i in 0..if joint { 2 } else { 1 } {
                let mass = if i == 0 { 10. } else { 5. };
                let mut b = rigid(Vec3::new(if joint { if i == 0 { -8.*unit } else { 8.*unit } } else { 0. }, 96.*unit, 0.), 1./mass);
                b.vertices = vertices.clone();
                // Fixture INPUT mass property, independently read through
                // GetInertia from the owned four-inch BBox/Compile object.
                // Do not replace production mass-property computation with it.
                let inertia = if boxed { 0.0486610122 * (mass / 10.) } else { 0.4 * mass * radius * radius };
                b.inverse_inertia = Mat3::IDENTITY / inertia;
                if joint && i == 0 {
                    b.angular = Vec3::new(spin[0], spin[2], -spin[1]) * std::f32::consts::PI / 180.;
                }
                bodies.push(b);
            }
            let joints = if joint { vec![Joint { native: None, hinge_target:0., a:0, b:1, anchor_a:Vec3::X*8.*unit,
                anchor_b:Vec3::NEG_X*8.*unit, frame:Quat::IDENTITY,
                min:Vec3::new(limits[0],limits[2],limits[4]) * std::f32::consts::PI / 180.,
                max:Vec3::new(limits[1],limits[3],limits[5]) * std::f32::consts::PI / 180., friction:Vec3::new(torque[0],torque[1],torque[2]) }] } else { Vec::new() };
            let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(), bodies, joints, collision_pairs:Vec::new(), friction:0.64, elasticity:0.05 };
            let collision = Collision::test_solid_floor();
            for step in 0..=240 {
                for (i,b) in rig.bodies.iter().enumerate() {
                    let p = Vec3::new(b.position.x,-b.position.z,b.position.y)/unit;
                    let v = Vec3::new(b.velocity.x,-b.velocity.z,b.velocity.y)/unit;
                    let w = Vec3::new(b.angular.x,-b.angular.z,b.angular.y)*180./std::f32::consts::PI;
                    let inertia = b.inverse_inertia.x_axis.x.recip();
                    write!(&mut report,"{case},{spin_axis},{step},{i},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                        p.x,p.y,p.z,b.rotation.x,b.rotation.y,b.rotation.z,b.rotation.w,v.x,v.y,v.z,w.x,w.y,w.z,rig.anchor_error(),
                        spin[0],spin[1],spin[2],limits[0],limits[1],limits[2],limits[3],limits[4],limits[5],
                        if boxed {"box"} else {"sphere"},inertia,inertia,inertia).unwrap();
                    writeln!(&mut report,",{},{},{}",torque[0],torque[1],torque[2]).unwrap();
                }
                if step < 240 {
                    rig.step_acceleration(1./60., &collision, true,
                        if free { Vec3::ZERO } else { Vec3::NEG_Y * 800. * unit });
                }
            }
        }
        std::fs::write(output, report).unwrap();
    }
    #[test]
    fn spinning_joint_pair_matches_native_impulse_and_predictive_limit() {
        // Owned vphysics.dll joint-free fixture: 10 kg and 5 kg 4-inch spheres
        // 16 inches apart, +/-45 deg ragdoll joint, body 0 spinning 90 deg/s.
        let unit = crate::collision::UNIT;
        let radius = 4. * unit;
        let mut bodies = Vec::new();
        for i in 0..2 {
            let mass = if i == 0 { 10. } else { 5. };
            let mut b = rigid(Vec3::new(if i == 0 { -8. * unit } else { 8. * unit }, 96. * unit, 0.), 1. / mass);
            b.vertices = vec![Vec3::ZERO];
            b.inverse_inertia = Mat3::IDENTITY / (0.4 * mass * radius * radius);
            if i == 0 { b.angular = Vec3::NEG_Z * std::f32::consts::FRAC_PI_2; }
            bodies.push(b);
        }
        let joints = vec![Joint { native: None, hinge_target:0., a: 0, b: 1, anchor_a: Vec3::X * 8. * unit, anchor_b: Vec3::NEG_X * 8. * unit,
            frame: Quat::IDENTITY, min: Vec3::splat(-std::f32::consts::FRAC_PI_4),
            max: Vec3::splat(std::f32::consts::FRAC_PI_4), friction: Vec3::ZERO }];
        let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(), bodies, joints, collision_pairs: Vec::new(), friction: 0.64, elasticity: 0.05 };
        let collision = Collision::test_solid_floor();
        let spin = |rig: &Articulation, i: usize| -rig.bodies[i].angular.z.to_degrees();
        for step in 1..=28 {
            rig.step_acceleration(1. / 60., &collision, true, Vec3::ZERO);
            if step == 1 {
                // Exact ball-socket impulse.
                assert!((spin(&rig, 0) - 62.727).abs() < 0.01, "{}", spin(&rig, 0));
                assert!((spin(&rig, 1) + 54.545).abs() < 0.01, "{}", spin(&rig, 1));
            }
        }
        // Predictive limit step (native 28.881 / -16.688 deg/s).
        assert!((spin(&rig, 0) - 28.881).abs() < 1., "{}", spin(&rig, 0));
        assert!((spin(&rig, 1) + 16.688).abs() < 1., "{}", spin(&rig, 1));
    }
    #[test]
    fn bouncing_sphere_travels_for_the_rest_of_the_impact_step() {
        // Owned vphysics.dll drop (floor e 0.2 x body e 0.25, 60 Hz): after the
        // impact step the sphere has already risen with the rebound speed.
        let unit = crate::collision::UNIT;
        let radius = 4. * unit;
        let mut vertices = vec![Vec3::Y * radius, Vec3::NEG_Y * radius];
        for latitude in 1..8 {
            let theta = latitude as f32 * std::f32::consts::PI / 8.;
            for longitude in 0..16 {
                let phi = longitude as f32 * std::f32::consts::TAU / 16.;
                vertices.push(Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()) * radius);
            }
        }
        let mut b = rigid(Vec3::new(0., 96. * unit, 0.), 0.1);
        b.vertices = vertices;
        b.inverse_inertia = Mat3::IDENTITY / (0.4 * 10. * radius * radius);
        let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(), bodies: vec![b], joints: Vec::new(), collision_pairs: Vec::new(), friction: 0.64, elasticity: 0.05 };
        let floor = Collision::test_solid_floor();
        for _ in 0..29 {
            rig.step(1. / 60., &floor);
        }
        let z = rig.bodies[0].position.y / unit;
        let vz = rig.bodies[0].velocity.y / unit;
        // Native step 29: z 5.3349, vz 86.461 (contact itself is near z 4.25).
        assert!((vz - 86.461).abs() < 0.1, "rebound speed {vz}");
        assert!(z > 5.0, "body must keep moving after impact, z {z}");
    }
    #[test]
    fn world_contact_uses_pair_material_for_bounce_and_sliding() {
        let a = crate::physics_data::Surface { friction: 0.25, elasticity: 0.2, density: 1. };
        let floor = crate::physics_data::Surface { friction: 0.8, elasticity: 0.5, density: 1. };
        let (coefficient, bounce) = a.contact_with(floor);
        let mut b = rigid(Vec3::Y, 1.);
        b.velocity = Vec3::new(1., -1., 0.);
        let mut contact = WorldContact { body: 0, arm: Vec3::ZERO, normal: Vec3::Y,
            target: bounce, impact: true, impulse: 0., friction: Vec3::ZERO, coefficient };
        for _ in 0..16 { world_velocity(&mut b, &mut contact); }
        assert!((b.velocity - Vec3::new(0.78, 0.1, 0.)).length() < 1e-5);
        assert!(b.velocity.length_squared() < 2.);
    }
    fn rigid(position: Vec3, inverse_mass: f32) -> Rigid {
        Rigid {
            bone: 0,
            position,
            rotation: Quat::IDENTITY,
            velocity: Vec3::ZERO,
            angular: Vec3::ZERO,
            center: Vec3::ZERO,
            inverse_mass,
            inverse_inertia: Mat3::IDENTITY * inverse_mass,
            hulls: Vec::new(),
            vertices: vec![Vec3::ZERO],
            damping: 0.,
            angular_damping: 0.,
            surface: crate::physics_data::Surface::default(),
            world_contacts: Vec::new(), pose_cache: None,
        }
    }
    #[test]
    fn initial_pose_recovery_moves_the_group_without_adding_velocity() {
        let mut a = rigid(Vec3::new(0., -0.05, 0.), 1.);
        a.velocity = Vec3::X * 2.;
        a.angular = Vec3::Y * 0.5;
        let b = rigid(Vec3::new(0.2, -0.03, 0.), 1.);
        let before = b.position - a.position;
        let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(),
            bodies: vec![a, b],
            joints: Vec::new(),
            collision_pairs: Vec::new(),
            friction: 0.8,
            elasticity: 0.2,
        };
        let collision = Collision::test_solid_floor();
        rig.recover_initial_pose(&collision);
        assert!((rig.bodies[1].position - rig.bodies[0].position - before).length() < 1e-6);
        assert_eq!(rig.bodies[0].velocity, Vec3::X * 2.);
        assert_eq!(rig.bodies[0].angular, Vec3::Y * 0.5);
        assert!(rig.bodies.iter().all(|b| {
            !collision
                .corpse_trace(b.position, b.position, Vec3::splat(0.0005))
                .start_solid
        }));
        rig.bodies[0].position = Vec3::NEG_Y * 2.;
        rig.recover_initial_pose(&collision);
        assert_eq!(rig.bodies[0].position, Vec3::NEG_Y * 2.);
    }
    #[test]
    fn side_contact_stops_inward_velocity_without_rewinding_fall() {
        let mut b = rigid(Vec3::new(20.005, -2., 0.), 1.);
        b.velocity = Vec3::NEG_X * 3.;
        let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(),
            bodies: vec![b],
            joints: Vec::new(),
            collision_pairs: Vec::new(),
            friction: 0.,
            elasticity: 0.,
        };
        rig.step(1. / 60., &Collision::test_solid_floor());
        assert!(rig.bodies[0].position.x >= 20.0005);
        assert!(rig.bodies[0].velocity.x >= -0.0001);
        assert!(rig.bodies[0].position.y < -2.);
        assert!(rig.bodies[0].velocity.y < 0.);
    }
    #[test]
    fn supported_joint_velocity_cannot_pull_ground_contact_back_into_floor() {
        // Start just outside the native 0.25-inch contact distance.
        let lower = rigid(Vec3::Y * 0.0115, 1.);
        let mut upper = rigid(Vec3::Y * 1.0115, 1.);
        upper.velocity = Vec3::NEG_Y * 3.;
        let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(),
            bodies: vec![lower, upper],
            joints: vec![Joint { native: None, hinge_target:0.,
                a: 0,
                b: 1,
                anchor_a: Vec3::Y,
                anchor_b: Vec3::ZERO,
                frame: Quat::IDENTITY,
                min: Vec3::splat(-3.),
                max: Vec3::splat(3.),
                friction: Vec3::ZERO,
            }],
            collision_pairs: Vec::new(),
            friction: 0.8,
            elasticity: 0.,
        };
        rig.step(1. / 60., &Collision::test_solid_floor());
        assert!(rig.bodies[0].velocity.y >= -0.0001);
        assert!(rig.bodies[1].velocity.y > -0.1);
        assert!(rig.anchor_error() < 0.002, "anchor error {} lower {:?} upper {:?}", rig.anchor_error(), rig.bodies[0].position, rig.bodies[1].position);
    }
    #[test]
    fn split_projection_does_not_add_anisotropic_rotation_energy() {
        let mut b = rigid(Vec3::ZERO, 1.);
        b.inverse_inertia = Mat3::from_diagonal(Vec3::new(1., 0.01, 1.));
        b.angular = Vec3::X;
        let before = angular_energy(&b);
        b.rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        assert!(angular_energy(&b) > before * 90.);
        preserve_projection_energy(&mut b, before);
        assert!(angular_energy(&b) <= before + 1e-6);
        assert!(b.angular.x > 0. && b.angular.is_finite());
    }
    #[test]
    fn floor_contact_preserves_tangential_motion() {
        let mut body = rigid(Vec3::Y * 0.005, 1.);
        body.velocity = Vec3::new(3., -1., 0.);
        let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(),
            bodies: vec![body],
            joints: Vec::new(),
            collision_pairs: Vec::new(),
            friction: 0.8,
            elasticity: 0.2,
        };
        rig.step(1. / 60., &Collision::test_solid_floor());
        assert!((rig.bodies[0].pose_cache.unwrap().position.x - 0.05).abs() < 1e-6);
        // The sampled pose precedes the full endpoint by at most the
        // incoming tangential travel in the remaining1% of the interval.
        assert!((0.0495-1e-6..=0.05+1e-6).contains(&rig.bodies[0].position.x),
            "{:?}", rig.bodies[0].position);
        assert!(rig.bodies[0].position.y >= 0.0005);
        assert!(rig.bodies[0].velocity.x <= 3.);
    }
    #[test]
    fn embedded_body_recovers_without_converting_displacement_to_launch_velocity() {
        let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(),
            bodies: vec![rigid(Vec3::new(0., -0.08, 0.), 1.)],
            joints: Vec::new(),
            collision_pairs: Vec::new(),
            friction: 0.8,
            elasticity: 0.2,
        };
        let collision = Collision::test_solid_floor();
        let mut peak = -0.08f32;
        for _ in 0..60 {
            rig.step(1. / 60., &collision);
            let b = &rig.bodies[0];
            peak = peak.max(b.position.y);
            assert!(
                b.velocity.y < 0.5,
                "recovery created upward velocity: {}",
                b.velocity.y
            );
        }
        assert!(peak < 0.03, "embedded body launched to {peak}");
        assert!(rig.bodies[0].position.y >= -0.002);
    }
    #[test]
    fn free_fall_retains_gravity_at_real_time_without_positional_velocity_feedback() {
        let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(),
            bodies: vec![rigid(Vec3::Y * 10., 1.)],
            joints: Vec::new(),
            collision_pairs: Vec::new(),
            friction: 0.8,
            elasticity: 0.2,
        };
        for _ in 0..30 {
            rig.step(1. / 60., &Collision::default());
        }
        assert!((rig.bodies[0].velocity.y + 800. * crate::collision::UNIT * 0.5).abs() < 0.001);
        assert!((rig.bodies[0].position.y - 7.3753).abs() < 0.002);
    }
    #[test]
    fn physical_anchor_and_asymmetric_limits_converge_without_moving_a_fixed_parent() {
        let mut a = rigid(Vec3::ZERO, 0.);
        let mut b = rigid(Vec3::new(1., 0.2, 0.), 1.);
        b.rotation = Quat::from_rotation_x(0.8);
        let j = Joint { native: None, hinge_target:0.,
            a: 0,
            b: 1,
            anchor_a: Vec3::X,
            anchor_b: Vec3::ZERO,
            frame: Quat::IDENTITY,
            min: Vec3::splat(-0.1),
            max: Vec3::splat(0.2),
            friction: Vec3::ZERO,
        };
        let mut rig = Articulation { initial_intrusions: None, pending_penetrations: Vec::new(), bodies:vec![a,b],joints:vec![j.clone()],collision_pairs:vec![],friction:0.,elasticity:0. };
        for _ in 0..30 { rig.step_acceleration(1./60., &Collision::default(), true, Vec3::ZERO); }
        let a=&rig.bodies[0]; let b=&rig.bodies[1];
        assert_eq!(a.position, Vec3::ZERO);
        assert_eq!(a.rotation, Quat::IDENTITY);
        assert!(b.position.distance(Vec3::X) < 0.00001);
        // Native limits are centered/permuted projected coordinates, not
        // independent XYZ Euler bounds. Their frames/bounds have separate
        // owned-DLL readback coverage above.
        let frames = rig.joints[0].native.as_ref().unwrap();
        for (axis,(value,_,_)) in frames.rows(a,b).into_iter().enumerate() {
            assert!(value >= frames.min[axis]-0.0001 && value <= frames.max[axis]+0.0001,"axis{axis}: {value}");
        }
    }
}
