//! Bounded articulated corpse solver. Native poses seed the skeleton; this is
//! a local position-constraint solver, not an IVP/VPhysics implementation.
use crate::collision::Collision;
use bevy::prelude::*;
use l4d2_formats::anim::Skeleton;
use l4d2r_math::Mat4 as XMat;
struct Point {
    bone: usize,
    position: Vec3,
    velocity: Vec3,
    radius: f32,
    inverse_mass: f32,
}
struct AngularLimit {
    parent: usize,
    child: usize,
    reference: Quat,
    min: Vec3,
    max: Vec3,
    friction: Vec3,
}
struct Link {
    a: usize,
    b: usize,
    min: f32,
    max: f32,
}
/// A body resting on the world is put to sleep after this long awake even if
/// its contacts still jitter (Source's ragdoll_sleepaftertime idea). Each
/// awake corpse costs about 1 ms per 60 Hz step, so a few that never quite
/// settled used to eat most of the frame during hordes.
const SLEEP_AFTER: f32 = 4.;
const MAX_SUBSTEPS: u32 = 2;
// Recovered from the running retail IVP environment, not the unused Phys2
// defaults: settings+8 = 50.8 m/s, settings+10 = .95199776 rad/step.
const FRAGMENT_LINEAR_LIMIT: f32 = 50.8;
const FRAGMENT_ANGULAR_LIMIT: f32 = 0.95199776 * 60.;

fn gib_random_axis(rng: &mut l4d2r_game_core::random::UniformRandomStream, low: f32, high: f32) -> f32 {
    // client 1018be40 is RandomInt(0,32767), not RandomFloat.
    rng.random_int(0, 32767) as f32 * (1. / 32767.) * (high - low) + low
}

fn gib_random_vector(rng: &mut l4d2r_game_core::random::UniformRandomStream, low: f32, high: f32) -> Vec3 {
    let x = gib_random_axis(rng, low, high);
    let y = gib_random_axis(rng, low, high);
    let z = gib_random_axis(rng, low, high);
    // Native Source axes -> renderer axes, for force and axial torque alike.
    Vec3::new(x, z, -y)
}
pub struct Body {
    rigid: Option<crate::articulated::Articulation>,
    initial_velocity: Vec3,
    points: Vec<Point>,
    links: Vec<Link>,
    map: Vec<Option<usize>>,
    parents: Vec<i32>,
    original: Vec<Mat4>,
    local: Vec<Mat4>,
    children: Vec<Option<usize>>,
    accumulator: f32,
    support_clock: f32,
    quiet: f32,
    /// Seconds simulated since the body last woke (see SLEEP_AFTER).
    awake: f32,
    rest_pose: Option<Vec<(Vec3, Quat)>>,
    sleeping: bool,
    initial_pose_checked: bool,
    angular_limits: Vec<AngularLimit>,
    surface: crate::physics_data::Surface,
}
fn basis() -> Mat4 {
    Mat4::from_cols(
        Vec4::new(0.0254, 0., 0., 0.),
        Vec4::new(0., 0., -0.0254, 0.),
        Vec4::new(0., 0.0254, 0., 0.),
        Vec4::W,
    )
}
impl Body {
    pub fn new(sk: &Skeleton, pose: &[XMat], root: &Transform, velocity: Vec3) -> Self {
        let c = basis();
        let inverse = c.inverse();
        let actor = root.to_matrix();
        let original = pose
            .iter()
            .map(|p| actor * c * Mat4::from_cols_array(&p.to_cols_array()) * inverse)
            .collect::<Vec<_>>();
        let parents = sk.bones.iter().map(|b| b.parent).collect::<Vec<_>>();
        let center = original.iter().map(|m| m.w_axis.truncate()).sum::<Vec3>()
            / original.len().max(1) as f32;
        let local = original
            .iter()
            .enumerate()
            .map(|(i, m)| {
                if parents[i] >= 0 {
                    original[parents[i] as usize].inverse() * *m
                } else {
                    *m
                }
            })
            .collect();
        let mut map = vec![None; sk.bones.len()];
        let mut points = vec![];
        for (i, b) in sk.bones.iter().enumerate() {
            let name = b.name.to_ascii_lowercase();
            if name.contains("finger")
                || name.contains("weapon")
                || name.contains("attach")
                || name.contains("eye")
            {
                continue;
            }
            map[i] = Some(points.len());
            let radius = if name.contains("pelvis") || name.contains("spine") {
                0.10
            } else if name.contains("head") {
                0.085
            } else {
                0.045
            };
            points.push(Point {
                bone: i,
                position: original[i].w_axis.truncate(),
                // A perfectly balanced, collinear skeleton otherwise has no
                // torque and can remain upright under distance constraints.
                velocity: velocity
                    + Vec3::new(0.65, 0., 0.45).cross(original[i].w_axis.truncate() - center),
                radius,
                inverse_mass: 1.,
            });
        }
        let mut links = vec![];
        let mut children = vec![None; sk.bones.len()];
        for p in &points {
            let mut parent = parents[p.bone];
            while parent >= 0 && map[parent as usize].is_none() {
                parent = parents[parent as usize];
            }
            if parent >= 0 {
                let a = map[parent as usize].unwrap();
                let b = map[p.bone].unwrap();
                let length = points[a].position.distance(points[b].position);
                if length > 0.001 {
                    links.push(Link {
                        a,
                        b,
                        min: length,
                        max: length,
                    });
                }
                // Prefer the spine over a leg as the pelvis' orientation axis.
                if children[parent as usize].is_none()
                    || sk.bones[p.bone].name.to_ascii_lowercase().contains("spine")
                {
                    children[parent as usize] = Some(p.bone);
                }
                let mut grand = parents[parent as usize];
                while grand >= 0 && map[grand as usize].is_none() {
                    grand = parents[grand as usize];
                }
                if grand >= 0 {
                    let a = map[grand as usize].unwrap();
                    let length = points[a].position.distance(points[b].position);
                    if length > 0.001 {
                        links.push(Link {
                            a,
                            b,
                            min: length * 0.25,
                            max: length * 1.05,
                        });
                    }
                }
            }
        }
        Self {
            rigid: None,
            initial_velocity: velocity,
            points,
            links,
            map,
            parents,
            original,
            local,
            children,
            accumulator: 0.,
            support_clock: 0.,
            quiet: 0.,
            awake: 0.,
            rest_pose: None,
            sleeping: false,
            initial_pose_checked: false,
            angular_limits: Vec::new(),
            surface: default(),
        }
    }
    pub fn with_profile(
        self,
        sk: &Skeleton,
        profile: &crate::physics_data::Profile,
        surfaces: &crate::physics_data::Surfaces,
    ) -> Self {
        self.with_native_profile(sk, profile, surfaces, 3)
    }
    /// Detached limbs can have just one or two physical bones.
    pub fn with_fragment_profile(
        self,
        sk: &Skeleton,
        profile: &crate::physics_data::Profile,
        surfaces: &crate::physics_data::Surfaces,
    ) -> Self {
        self.with_native_profile(sk, profile, surfaces, 1)
    }
    fn with_native_profile(
        mut self,
        sk: &Skeleton,
        profile: &crate::physics_data::Profile,
        surfaces: &crate::physics_data::Surfaces,
        minimum_bodies: usize,
    ) -> Self {
        let matches = profile
            .solids
            .iter()
            .filter_map(|solid| {
                sk.bones
                    .iter()
                    .position(|b| b.name.eq_ignore_ascii_case(&solid.name))
                    .map(|bone| (bone, solid))
            })
            .collect::<std::collections::HashMap<_, _>>();
        if matches.len() < minimum_bodies || matches.keys().any(|b| self.map[*b].is_none()) {
            return self;
        }
        self.points.retain(|p| matches.contains_key(&p.bone));
        self.map.fill(None);
        for (i, p) in self.points.iter_mut().enumerate() {
            self.map[p.bone] = Some(i);
            p.inverse_mass = 1. / matches[&p.bone].mass;
        }
        self.links.clear();
        self.children.fill(None);
        let by_id = profile
            .solids
            .iter()
            .filter_map(|s| {
                sk.bones
                    .iter()
                    .position(|b| b.name.eq_ignore_ascii_case(&s.name))
                    .map(|bone| (s.index, bone))
            })
            .collect::<std::collections::HashMap<_, _>>();
        for joint in &profile.joints {
            let (Some(&parent), Some(&child)) = (by_id.get(&joint.parent), by_id.get(&joint.child))
            else {
                continue;
            };
            let (Some(a), Some(b)) = (self.map[parent], self.map[child]) else {
                continue;
            };
            let length = self.points[a].position.distance(self.points[b].position);
            if length > 0.001 {
                self.links.push(Link {
                    a,
                    b,
                    min: length,
                    max: length,
                });
            }
            if self.children[parent].is_none()
                || sk.bones[child].name.to_ascii_lowercase().contains("spine")
            {
                self.children[parent] = Some(child);
            }
            let parent_bind = sk.bones[parent].pose_to_bone.inverse();
            let child_bind = sk.bones[child].pose_to_bone.inverse();
            let reference = Quat::from_mat3(&Mat3::from_mat4(Mat4::from_cols_array(
                &(parent_bind.inverse() * child_bind).to_cols_array(),
            )))
            .normalize();
            self.angular_limits.push(AngularLimit {
                parent,
                child,
                reference,
                min: joint.min,
                max: joint.max,
                friction: joint.friction,
            });
            if let Some(grand_joint) = profile.joints.iter().find(|j| j.child == joint.parent) {
                if let Some(&grand) = by_id.get(&grand_joint.parent) {
                    if let Some(g) = self.map[grand] {
                        let l1 = self.points[g].position.distance(self.points[a].position);
                        let l2 = self.points[a].position.distance(self.points[b].position);
                        let angle = joint
                            .min
                            .abs()
                            .max(joint.max.abs())
                            .max_element()
                            .min(std::f32::consts::PI);
                        self.links.push(Link {
                            a: g,
                            b,
                            min: (l1 * l1 + l2 * l2 + 2. * l1 * l2 * angle.cos())
                                .max(0.)
                                .sqrt(),
                            max: l1 + l2,
                        });
                    }
                }
            }
        }
        let conversion = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let mut bodies = Vec::new();
        for p in &self.points {
            let solid = matches[&p.bone];
            let mut vertices = profile
                .geometry
                .get(solid.index)
                .map(|shape| {
                    shape
                        .convexes
                        .iter()
                        .flat_map(|h| crate::rigid_contact::Hull::native(h).vertices)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if vertices.is_empty() {
                for x in [-p.radius, p.radius] {
                    for y in [-p.radius, p.radius] {
                        for z in [-p.radius, p.radius] {
                            vertices.push(Vec3::new(x, y, z));
                        }
                    }
                }
            }
            let low = vertices
                .iter()
                .copied()
                .fold(Vec3::splat(f32::INFINITY), Vec3::min);
            let high = vertices
                .iter()
                .copied()
                .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
            let physical = profile.geometry.get(solid.index).and_then(|shape| {
                crate::mass_properties::from_phy(shape, solid.mass, solid.inertia, 0.1)
                    .or_else(|| crate::mass_properties::from_convexes(&shape.convexes, solid.mass, solid.inertia)
                        .map(|p| (p.center, crate::mass_properties::limit_rotational_inertia(p.inverse, 0.1))))
            });
            let center = physical.map_or((low + high) * 0.5, |p| p.0);
            let rotation = self.original[p.bone].to_scale_rotation_translation().1;
            bodies.push(crate::articulated::Rigid {
                bone: p.bone,
                position: p.position + rotation * center,
                rotation,
                velocity: self.initial_velocity,
                angular: Vec3::ZERO,
                surface: surfaces.get(&solid.surface),
                center,
                inverse_mass: p.inverse_mass,
                inverse_inertia: physical.map_or_else(
                    || crate::mass_properties::limit_rotational_inertia(
                        Mat3::from_diagonal(crate::rigid_contact::inverse_box_inertia(
                            solid.mass, (high - low) * 0.5, solid.inertia)), 0.1),
                    |p| p.1,
                ),
                hulls: profile
                    .geometry
                    .get(solid.index)
                    .map(|shape| {
                        shape
                            .convexes
                            .iter()
                            .map(crate::rigid_contact::Hull::native)
                            .collect()
                    })
                    .unwrap_or_default(),
                vertices,
                damping: solid.damping,
                angular_damping: solid.angular_damping,
                world_contacts: Vec::new(), pose_cache: None,
            });
        }
        let mut joints = Vec::new();
        for joint in &self.angular_limits {
            let a = self.map[joint.parent].unwrap();
            let b = self.map[joint.child].unwrap();
            let parent_bind = sk.bones[joint.parent].pose_to_bone.inverse();
            let child_bind = sk.bones[joint.child].pose_to_bone.inverse();
            let offset = (parent_bind.inverse() * child_bind).w_axis.truncate();
            joints.push(crate::articulated::Joint { native: None, hinge_target:0.,
                a,
                b,
                anchor_a: Vec3::from_array(crate::binary::source_position(offset.to_array()))
                    - bodies[a].center,
                anchor_b: -bodies[b].center,
                frame: conversion * joint.reference * conversion.conjugate(),
                min: joint.min,
                max: joint.max,
                friction: joint.friction,
            });
        }
        let surface = surfaces.get(
            profile
                .solids
                .first()
                .map_or("flesh", |s| s.surface.as_str()),
        );
        let collision_pairs = if let Some(pairs) = &profile.collision_pairs {
            pairs
                .iter()
                .filter_map(|(a, b)| Some((self.map[*by_id.get(a)?]?, self.map[*by_id.get(b)?]?)))
                .filter(|(a, b)| a != b)
                .collect()
        } else {
            (0..bodies.len())
                .flat_map(|a| (a + 1..bodies.len()).map(move |b| (a, b)))
                .filter(|(a, b)| {
                    !joints
                        .iter()
                        .any(|j| j.a == *a && j.b == *b || j.a == *b && j.b == *a)
                })
                .collect()
        };
        self.rigid = Some(crate::articulated::Articulation { initial_intrusions: None, pending_penetrations: Vec::new(),
            bodies,
            joints,
            collision_pairs,
            friction: surface.friction,
            elasticity: surface.elasticity,
        });
        self.surface = surfaces.get(
            profile
                .solids
                .first()
                .map_or("flesh", |s| s.surface.as_str()),
        );
        self
    }
    /// Solver steps the next `tick(dt)` would run (0 while asleep).
    pub fn steps_due(&self, dt: f32) -> u32 {
        if self.sleeping {
            return 0;
        }
        (((self.accumulator + dt.clamp(0., 0.1)) * 60. + 1e-4) as u32).min(MAX_SUBSTEPS)
    }
    /// Skip this frame's solve but keep the elapsed time (up to the substep
    /// cap) so the body catches up on its next budgeted frame.
    pub fn defer(&mut self, dt: f32) {
        self.accumulator =
            (self.accumulator + dt.clamp(0., 0.1)).min(MAX_SUBSTEPS as f32 / 60.);
    }
    pub fn tick(&mut self, dt: f32, collision: &Collision) {
        self.accumulator += dt.clamp(0., 0.1);
        let h = 1. / 60.;
        // At most two substeps per frame. A slow frame otherwise makes every
        // awake body run up to six solver steps next frame, which slows that
        // frame too (the hitches compounded during hordes).
        let mut steps = 0;
        while self.accumulator + 1e-6 >= h {
            if steps == MAX_SUBSTEPS {
                self.accumulator = self.accumulator.min(h);
                break;
            }
            steps += 1;
            self.accumulator = (self.accumulator - h).max(0.);
            if !self.initial_pose_checked {
                if let Some(rigid) = &mut self.rigid {
                    rigid.recover_initial_pose(collision);
                }
                self.initial_pose_checked = true;
            }
            if self.sleeping {
                self.support_clock += h;
                if self.support_clock + 1e-6 < 0.1 {
                    continue;
                }
                self.support_clock = 0.;
                let supported = if let Some(rigid) = &self.rigid {
                    rigid.supported(collision)
                } else {
                    self.points.iter().any(|p| {
                        let hit = collision.corpse_trace(
                            p.position,
                            p.position - Vec3::Y * 0.02,
                            Vec3::splat(p.radius),
                        );
                        !hit.start_solid && hit.fraction < 1. && hit.normal.y > 0.6
                    })
                };
                if supported {
                    continue;
                }
                self.sleeping = false;
                self.quiet = 0.;
                self.awake = 0.;
            }
            self.step(h, collision);
        }
    }
    fn step(&mut self, h: f32, collision: &Collision) {
        if let Some(rigid) = &mut self.rigid {
            let (supported, _speed) = rigid.step(h, collision);
            for (p, b) in self.points.iter_mut().zip(&rigid.bodies) {
                p.position = b.position - b.rotation * b.center;
                p.velocity = b.velocity;
            }
            let supported = supported || rigid.supported(collision);
            // A supported pose can jitter each iteration without making progress.
            // Require a bounded half-second pose window, rather than a fresh
            // velocity threshold on every substep, before putting it to sleep.
            let stable = self.rest_pose.as_ref().is_some_and(|pose| {
                pose.len() == rigid.bodies.len()
                    && pose.iter().zip(&rigid.bodies).all(|((p, q), b)| {
                        // Measure the physical shape, not a fixed angular
                        // threshold that treats tiny and long limbs alike.
                        b.vertices.iter().all(|v| {
                            let local = *v - b.center;
                            (*p + *q * local).distance_squared(b.position + b.rotation * local)
                                < 0.01f32.powi(2)
                        })
                    })
            });
            if !supported || !stable {
                self.rest_pose = Some(
                    rigid
                        .bodies
                        .iter()
                        .map(|b| (b.position, b.rotation))
                        .collect(),
                );
                self.quiet = 0.;
            } else if rigid.anchor_error() < 0.004 {
                self.quiet += h;
            } else {
                self.quiet = 0.;
            }
            self.awake += h;
            if self.quiet > 0.5 || (supported && self.awake > SLEEP_AFTER) {
                self.sleeping = true;
                for b in &mut rigid.bodies {
                    b.velocity = Vec3::ZERO;
                    b.angular = Vec3::ZERO;
                }
            }
            return;
        }
        let starts = self.points.iter().map(|p| p.position).collect::<Vec<_>>();
        for p in &mut self.points {
            p.velocity.y -= 800. * crate::collision::UNIT * h;
            // The original common-model ragdoll drop follows undamped gravity
            // before first contact. Contact friction belongs to the response,
            // not to an unconditional reduction of airborne gravity/velocity.
            p.position += p.velocity * h;
        }
        for _ in 0..6 {
            for link in &self.links {
                let d = self.points[link.b].position - self.points[link.a].position;
                let length = d.length();
                if length < 1e-6 {
                    continue;
                }
                let target = length.clamp(link.min, link.max);
                let sum = self.points[link.a].inverse_mass + self.points[link.b].inverse_mass;
                let correction = d * ((length - target) / length);
                let a = self.points[link.a].inverse_mass / sum;
                let b = self.points[link.b].inverse_mass / sum;
                self.points[link.a].position += correction * a;
                self.points[link.b].position -= correction * b;
            }
        }
        let mut supported = false;
        let mut max_speed: f32 = 0.;
        for (p, start) in self.points.iter_mut().zip(starts) {
            let half = Vec3::splat(p.radius);
            let mut hit = collision.corpse_trace(start, p.position, half);
            let mut from = start;
            if hit.start_solid {
                // Authored terminal poses can intersect the floor. Recover
                // contact instead of sleeping while still inside a brush.
                if let Some(free) = collision.recover_corpse_position(start, half, 0.6) {
                    from = free;
                    hit = collision.corpse_trace(from, p.position, half);
                }
            }
            if hit.start_solid {
                p.position = from;
                p.velocity = Vec3::ZERO;
            } else if hit.fraction < 1. {
                p.position = from + (p.position - from) * hit.fraction + hit.normal * 0.002;
                let velocity = if from != start {
                    p.velocity
                } else {
                    (p.position - start) / h
                };
                let inward = velocity.dot(hit.normal).min(0.);
                p.velocity = velocity - hit.normal * inward;
                let tangent = p.velocity - hit.normal * p.velocity.dot(hit.normal);
                let speed = tangent.length();
                if speed > 0. {
                    p.velocity -= tangent * (self.surface.friction * (-inward) / speed).min(1.);
                }
                supported |= hit.normal.y > 0.6;
            } else {
                p.velocity = (p.position - start) / h;
            }
            max_speed = max_speed.max(p.velocity.length_squared());
        }
        self.quiet = if supported && max_speed < 0.015 {
            self.quiet + h
        } else {
            0.
        };
        self.awake += h;
        if self.quiet > 0.5 || (supported && self.awake > SLEEP_AFTER) {
            self.sleeping = true;
            for p in &mut self.points {
                p.velocity = Vec3::ZERO;
            }
        }
    }
    /// Seed per-bone velocities from consecutive world poses, as client gibs
    /// inherit both animation and actor movement before damage forces apply.
    pub fn inherit_pose_motion(&mut self, previous: &[XMat], root: &Transform, dt: f32) {
        if !dt.is_finite() || dt <= 0. || dt > 0.1001 || previous.len() != self.original.len() {
            return;
        }
        let c = basis();
        let actor = root.to_matrix();
        let inverse = c.inverse();
        if let Some(rigid) = &mut self.rigid {
            for body in &mut rigid.bodies {
                let old = actor
                    * c
                    * Mat4::from_cols_array(&previous[body.bone].to_cols_array())
                    * inverse;
                let rotation = old.to_scale_rotation_translation().1;
                let center = old.w_axis.truncate() + rotation * body.center;
                let linear = (body.position - center) / dt;
                let delta = (body.rotation * rotation.conjugate()).normalize();
                // q and -q describe the same orientation. Use the short arc.
                let delta = if delta.w < 0. { -delta } else { delta };
                let angular = delta.to_scaled_axis() / dt;
                if linear.is_finite() && angular.is_finite() {
                    body.velocity = self.initial_velocity + linear;
                    body.angular = angular;
                }
            }
        }
        for point in &mut self.points {
            let old =
                actor * c * Mat4::from_cols_array(&previous[point.bone].to_cols_array()) * inverse;
            let velocity =
                (self.original[point.bone].w_axis.truncate() - old.w_axis.truncate()) / dt;
            if velocity.is_finite() {
                point.velocity = self.initial_velocity + velocity;
            }
        }
    }
    /// Native EmitGibs calls ApplyForceCenter on each fragment PHY body.
    /// `impulse` is already converted once to renderer kg*m/s; mass remains kg.
    pub(crate) fn apply_fragment_impulse(&mut self, impulse: Vec3) {
        if !impulse.is_finite() { return; }
        if let Some(rigid) = &mut self.rigid {
            for body in &mut rigid.bodies {
                body.velocity = (body.velocity + impulse * body.inverse_mass)
                    .clamp_length_max(FRAGMENT_LINEAR_LIMIT);
            }
        }
        for point in &mut self.points {
            point.velocity = (point.velocity + impulse * point.inverse_mass)
                .clamp_length_max(FRAGMENT_LINEAR_LIMIT);
        }
        self.sleeping = false;
        self.quiet = 0.;
        self.awake = 0.;
        self.rest_pose = None;
    }
    /// Native EmitGibs adds radial spread only for wounds with >4 pieces.
    /// Its radial vector uses the first PHY mass centre rotated by the FINAL
    /// model bone (not the actor origin or the first body's orientation).
    /// RNG distribution/order match; the retail shared global seed does not.
    pub(crate) fn launch_fragment(&mut self, impulse: Option<Vec3>, scatter: bool,
        rng: &mut l4d2r_game_core::random::UniformRandomStream) -> Vec3 {
        let mut impulse = impulse.filter(|v| v.is_finite()).unwrap_or(Vec3::ZERO);
        if scatter {
            if let (Some(rigid), Some(last)) = (&mut self.rigid, self.original.last()) {
                if let Some(first) = rigid.bodies.first() {
                    let rotation = last.to_scale_rotation_translation().1;
                    let radial = rotation * first.center / crate::collision::UNIT;
                    // Native normalizes with FLT_EPSILON added to squared length.
                    let radial = radial / (radial.length_squared() + f32::EPSILON).sqrt();
                    impulse += (radial + gib_random_vector(rng, -0.1, 0.1))
                        * 10000. * crate::collision::UNIT;
                    for body in &mut rigid.bodies {
                        let torque = gib_random_vector(rng, -1., 1.) * 150. * std::f32::consts::PI / 180.;
                        // IVP ApplyTorqueCenter converts degrees once, rotates
                        // into core axes and applies inverse rotational inertia.
                        let r = Mat3::from_quat(body.rotation);
                        body.angular = (body.angular + r * body.inverse_inertia * r.transpose() * torque)
                            .clamp_length_max(FRAGMENT_ANGULAR_LIMIT);
                    }
                }
            }
        }
        self.apply_fragment_impulse(impulse);
        impulse
    }
    pub fn center(&self) -> Vec3 {
        if let Some(rigid) = &self.rigid {
            return rigid.bodies.iter().map(|b| b.position).sum::<Vec3>()
                / rigid.bodies.len().max(1) as f32;
        }
        self.points.iter().map(|p| p.position).sum::<Vec3>() / self.points.len().max(1) as f32
    }
    pub fn sleeping(&self) -> bool {
        self.sleeping
    }
    pub fn pose(&self, root: &Transform) -> Vec<XMat> {
        let mut world = Vec::<Mat4>::with_capacity(self.original.len());
        for i in 0..self.original.len() {
            if let Some(p) = self.map[i] {
                if let Some(rigid) = &self.rigid {
                    let b = &rigid.bodies[p];
                    debug_assert_eq!(b.bone, i);
                    let scale = self.original[i].to_scale_rotation_translation().0;
                    world.push(Mat4::from_scale_rotation_translation(
                        scale,
                        b.rotation,
                        b.position - b.rotation * b.center,
                    ));
                    continue;
                }
                let (scale, rotation, _) = self.original[i].to_scale_rotation_translation();
                let endpoint = self.children[i]
                    .or_else(|| (self.parents[i] >= 0).then_some(self.parents[i] as usize));
                let turn = endpoint
                    .and_then(|j| {
                        self.map[j].map(|q| {
                            let before = self.original[j].w_axis.truncate()
                                - self.original[i].w_axis.truncate();
                            let after = self.points[q].position - self.points[p].position;
                            if before.length_squared() > 1e-6 && after.length_squared() > 1e-6 {
                                Quat::from_rotation_arc(before.normalize(), after.normalize())
                            } else {
                                Quat::IDENTITY
                            }
                        })
                    })
                    .unwrap_or(Quat::IDENTITY);
                world.push(Mat4::from_scale_rotation_translation(
                    scale,
                    turn * rotation,
                    self.points[p].position,
                ));
            } else {
                world.push(if self.parents[i] >= 0 {
                    world[self.parents[i] as usize] * self.local[i]
                } else {
                    self.original[i]
                });
            }
        }
        // Imported native limits bound the physical-bone presentation in the
        // bind joint frame. This projection is not an IVP angular impulse solve.
        let conversion = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        for joint in self.angular_limits.iter().filter(|_| self.rigid.is_none()) {
            let parent = world[joint.parent].to_scale_rotation_translation().1;
            let (scale, child, pos) = world[joint.child].to_scale_rotation_translation();
            let relative = conversion.conjugate() * parent.conjugate() * child * conversion;
            let deviation = joint.reference.conjugate() * relative;
            let (x, y, z) = deviation.to_euler(EulerRot::XYZ);
            let angles = Vec3::new(x, y, z).clamp(joint.min, joint.max);
            let q = parent
                * conversion
                * joint.reference
                * Quat::from_euler(EulerRot::XYZ, angles.x, angles.y, angles.z)
                * conversion.conjugate();
            world[joint.child] = Mat4::from_scale_rotation_translation(scale, q, pos);
        }
        for i in 0..world.len() {
            if self.map[i].is_none() && self.parents[i] >= 0 {
                world[i] = world[self.parents[i] as usize] * self.local[i];
            }
        }
        let c = basis();
        let inverse = c.inverse();
        let actor = root.to_matrix().inverse();
        world
            .into_iter()
            .map(|m| XMat::from_cols_array(&(inverse * actor * m * c).to_cols_array()))
            .collect()
    }
}

/// Original BeginRagdollDrop counterpart: same model, sequence 0/cycle 0, map and origin.
pub fn verify_contact(root: &std::path::Path, output: &std::path::Path) -> std::io::Result<()> {
    let assets = std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![])?);
    let model = "models/infected/common_male01.mdl";
    let skeleton = Skeleton::load_with(model, |p| assets.borrow_mut().read(p).ok())
        .map_err(std::io::Error::other)?;
    let profile = crate::physics_data::Profile::load(&mut assets.borrow_mut(), model)?;
    let surfaces = crate::physics_data::Surfaces::load(&mut assets.borrow_mut())?;
    // Observed original prop_ragdoll sequence 0 / cycle 0.
    let pose = skeleton.pose_sequence(0, 0., false);
    let transform =
        Transform::from_translation(Vec3::from_array(crate::binary::source_position([
            600., 5400., 3100.,
        ])));
    let map = root.join("left4dead2/maps/c1m1_hotel.bsp");
    let logic = crate::l4d2r_cinematic::MapLogic::load(root, &map, true)?;
    let mut collision = Collision::load(&map)?;
    collision.enable_l4d2r(root, &logic)?;
    let mut body = Body::new(&skeleton, &pose, &transform, Vec3::ZERO)
        .with_profile(&skeleton, &profile, &surfaces);
    let mut csv = "time,x,y,z,anchor_error_metres,angle_error_radians\n".to_string();
    for frame in 0..=120 {
        let rigid = body
            .rigid
            .as_ref()
            .ok_or_else(|| std::io::Error::other("owned rigid profile unavailable"))?;
        let p = rigid.bodies[0].position - rigid.bodies[0].rotation * rigid.bodies[0].center;
        let angle = rigid
            .joints
            .iter()
            .map(|j| {
                let a = crate::articulated::angles(j, &rigid.bodies[j.a], &rigid.bodies[j.b]);
                a.distance(a.clamp(j.min, j.max))
            })
            .fold(0., f32::max);
        csv.push_str(&format!(
            "{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}\n",
            frame as f32 / 30.,
            p.x / crate::collision::UNIT,
            -p.z / crate::collision::UNIT,
            p.y / crate::collision::UNIT,
            rigid.anchor_error(),
            angle
        ));
        body.tick(1. / 30., &collision);
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, csv)?;
    println!("Ragdoll hotel contact trace: {}", output.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "manual Hotel ragdoll settling profile"]
    fn profile_hotel_death_settling() {
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let assets =
            std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![]).unwrap());
        let model = "models/infected/common_male01.mdl";
        let sk = Skeleton::load_with(model, |p| assets.borrow_mut().read(p).ok()).unwrap();
        let profile = crate::physics_data::Profile::load(&mut assets.borrow_mut(), model).unwrap();
        let surfaces = crate::physics_data::Surfaces::load(&mut assets.borrow_mut()).unwrap();
        let map = root.join("left4dead2/maps/c1m1_hotel.bsp");
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &map, true).unwrap();
        let mut collision = Collision::load(&map).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let seq = sk.find_sequence("HeadshotFront").unwrap();
        let pose = sk.pose_sequence(
            seq,
            sk.anims()[sk.sequence_anim(seq).unwrap()].duration(),
            false,
        );
        let position = Vec3::from_array(crate::binary::source_position([1830., 6280., 2464.]));
        for i in 0..24 {
            let point = position
                + Vec3::X * ((i % 4) as f32 - 1.5) * 0.35
                + Vec3::Z * (1.2 + (i / 4) as f32 * 0.35);
            println!(
                "HOTEL FIXTURE ROOT i={i} clear={} grounded={}",
                collision.clear_hull(point, crate::collision::HEIGHT),
                collision.grounded(point, crate::collision::HEIGHT)
            );
        }
        let mut body = Body::new(
            &sk,
            &pose,
            &Transform::from_translation(position),
            Vec3::ZERO,
        )
        .with_profile(&sk, &profile, &surfaces);
        let start = std::time::Instant::now();
        for frame in 0..=360 {
            if frame % 60 == 0 {
                let rig = body.rigid.as_ref().unwrap();
                let vertices = rig.bodies.iter().map(|b| b.vertices.len()).sum::<usize>();
                let embedded = rig
                    .bodies
                    .iter()
                    .flat_map(|b| {
                        b.vertices
                            .iter()
                            .map(move |v| b.position + b.rotation * (*v - b.center))
                    })
                    .filter(|p| {
                        collision
                            .corpse_trace(*p, *p, Vec3::splat(0.0005))
                            .start_solid
                    })
                    .count();
                let drift = body
                    .rest_pose
                    .as_ref()
                    .map(|p| {
                        p.iter()
                            .zip(&rig.bodies)
                            .map(|((v, _), b)| v.distance(b.position))
                            .fold(0., f32::max)
                    })
                    .unwrap_or(0.);
                let minimum = rig
                    .bodies
                    .iter()
                    .flat_map(|b| {
                        b.vertices
                            .iter()
                            .map(move |v| (b.position + b.rotation * (*v - b.center)).y)
                    })
                    .fold(f32::INFINITY, f32::min)
                    / crate::collision::UNIT;
                let velocity = rig
                    .bodies
                    .iter()
                    .map(|b| b.velocity.length())
                    .fold(0., f32::max);
                let angular = rig
                    .bodies
                    .iter()
                    .map(|b| b.angular.length())
                    .fold(0., f32::max);
                println!(
                    "HOTEL MOTION frame={frame} min_z={minimum} speed={velocity} angular={angular} support={}",
                    rig.supported(&collision)
                );
                println!(
                    "HOTEL SETTLE frame={frame} vertices={vertices} embedded={embedded} anchor={} asleep={} quiet={} drift={} elapsed_ms={:.2}",
                    rig.anchor_error(),
                    body.sleeping,
                    body.quiet,
                    drift,
                    start.elapsed().as_secs_f64() * 1000.
                );
            }
            body.tick(1. / 60., &collision);
        }
    }
    #[test]
    fn overlap_recovery_cache_matches_uncached_native_corpse_solver() {
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets =
            std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![]).unwrap());
        let path = "models/infected/common_male01.mdl";
        let sk = Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok()).unwrap();
        let profile = crate::physics_data::Profile::load(&mut assets.borrow_mut(), path).unwrap();
        let surfaces = crate::physics_data::Surfaces::load(&mut assets.borrow_mut()).unwrap();
        let seq = sk.find_sequence("HeadshotFront").unwrap();
        let pose = sk.pose_sequence(
            seq,
            sk.anims()[sk.sequence_anim(seq).unwrap()].duration(),
            false,
        );
        let map = root.join("left4dead2/maps/c1m1_hotel.bsp");
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &map, true).unwrap();
        let mut hotel = Collision::load(&map).unwrap();
        hotel.enable_l4d2r(root, &logic).unwrap();
        let floor = Collision::test_solid_floor();
        for (collision, position) in [
            (&floor, Vec3::new(0., -0.08, 0.)),
            (
                &hotel,
                Vec3::from_array(crate::binary::source_position([1830., 6280., 2464.])),
            ),
        ] {
            let body = Body::new(&sk, &pose, &Transform::from_translation(position), Vec3::X)
                .with_profile(&sk, &profile, &surfaces);
            let mut cached = body.rigid.unwrap();
            let mut reference = cached.clone();
            for _ in 0..8 {
                assert_eq!(
                    cached.step_with_cache(1. / 60., collision, true),
                    reference.step_with_cache(1. / 60., collision, false)
                );
                for (a, b) in cached.bodies.iter().zip(&reference.bodies) {
                    assert_eq!(a.position, b.position);
                    assert_eq!(a.rotation, b.rotation);
                    assert_eq!(a.velocity, b.velocity);
                    assert_eq!(a.angular, b.angular);
                }
            }
        }
    }
    #[test]
    fn owned_physical_bones_masses_and_limits_settle_without_invalid_poses() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets =
            std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![]).unwrap());
        let model = "models/infected/common_male01.mdl";
        let sk = Skeleton::load_with(model, |p| assets.borrow_mut().read(p).ok()).unwrap();
        let profile = crate::physics_data::Profile::load(&mut assets.borrow_mut(), model).unwrap();
        let native_bytes = assets
            .borrow_mut()
            .read("models/infected/anim_common.phy")
            .unwrap();
        std::fs::write(
            "local-research/parity/native-common-physics-keyvalues.txt",
            crate::physics_data::text(&native_bytes).unwrap(),
        )
        .unwrap();
        let surfaces = crate::physics_data::Surfaces::load(&mut assets.borrow_mut()).unwrap();
        let seq = sk.find_sequence("HeadshotBack").unwrap();
        let duration = sk.anims()[sk.sequence_anim(seq).unwrap()].duration();
        let pose = sk.pose_sequence(seq, duration, false);
        let transform = Transform::from_translation(Vec3::Y * 2.);
        let mut body = Body::new(&sk, &pose, &transform, Vec3::new(0.3, 0., 0.2))
            .with_profile(&sk, &profile, &surfaces);
        assert_eq!(body.points.len(), 16);
        assert_eq!(body.angular_limits.len(), 15);
        assert!(
            body.points
                .iter()
                .any(|p| (p.inverse_mass - body.points[0].inverse_mass).abs() > 0.01)
        );
        let mut report =
            "time,anchor_error_metres,angle_error_radians,min_vertex_height_metres\n".to_string();
        for frame in 0..480 {
            body.tick(1. / 60., &Collision::test_solid_floor());
            let rigid = body.rigid.as_ref().unwrap();
            let error = rigid
                .joints
                .iter()
                .map(|j| {
                    let value =
                        crate::articulated::angles(j, &rigid.bodies[j.a], &rigid.bodies[j.b]);
                    value.distance(value.clamp(j.min, j.max))
                })
                .fold(0., f32::max);
            let bottom = rigid
                .bodies
                .iter()
                .flat_map(|b| {
                    b.vertices
                        .iter()
                        .map(move |v| (b.position + b.rotation * (*v - b.center)).y)
                })
                .fold(f32::INFINITY, f32::min);
            report.push_str(&format!(
                "{:.6},{:.6},{:.6},{:.6}\n",
                (frame + 1) as f32 / 60.,
                rigid.anchor_error(),
                error,
                bottom
            ));
        }
        std::fs::write("local-research/parity/physical-joints-settling.csv", report).unwrap();
        let rigid = body.rigid.as_ref().unwrap();
        assert!(
            rigid.anchor_error() < 0.005,
            "physical joint anchors separated: {}",
            rigid.anchor_error()
        );
        let bottom = rigid
            .bodies
            .iter()
            .flat_map(|b| {
                b.vertices
                    .iter()
                    .map(move |v| (b.position + b.rotation * (*v - b.center)).y)
            })
            .fold(f32::INFINITY, f32::min);
        assert!(
            bottom >= -0.002,
            "native hull vertex penetrated solid floor: {bottom}"
        );
        let bones = rigid
            .bodies
            .iter()
            .map(|b| {
                format!(
                    "{}|{}|{:?}|{:?}\n",
                    b.bone,
                    sk.bones[b.bone].name,
                    b.position - b.rotation * b.center,
                    b.rotation
                )
            })
            .collect::<String>();
        std::fs::write("local-research/parity/physical-joints-bones.txt", bones).unwrap();
        assert!(body.pose(&transform).iter().all(|m| m.is_finite()));
        let height =
            body.points.iter().map(|p| p.position.y).sum::<f32>() / body.points.len() as f32;
        assert!(
            height < 0.7,
            "native-profile corpse did not settle: {height}"
        );
        assert!(body.points.iter().all(|p| p.position.y >= 0.02));
    }
    #[test]
    fn native_damage_death_variants_settle_at_both_low_and_high_frame_rates() {
        use crate::infected_death::{Cause, Context};
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets =
            std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let profile = crate::physics_data::Profile::load(
            &mut assets.borrow_mut(),
            "models/infected/common_male01.mdl",
        )
        .unwrap();
        let surfaces = crate::physics_data::Surfaces::load(&mut assets.borrow_mut()).unwrap();
        let floor = Collision::test_solid_floor();
        for context in [
            Context::default(),
            Context {
                head: true,
                ..Default::default()
            },
            Context {
                running: true,
                ..Default::default()
            },
            Context {
                cause: Cause::Shotgun,
                incoming: Vec3::NEG_Z,
                ..Default::default()
            },
            Context {
                cause: Cause::Slash,
                incoming: Vec3::X,
                ..Default::default()
            },
            Context {
                cause: Cause::Blunt,
                incoming: Vec3::Z,
                ..Default::default()
            },
            Context {
                cause: Cause::Fire,
                ..Default::default()
            },
            Context {
                cause: Cause::Explosion,
                ..Default::default()
            },
        ] {
            let seq = crate::infected_death::select(&sk, context, 17).unwrap();
            let duration = sk.anims()[sk.sequence_anim(seq).unwrap()].duration();
            let pose = sk.pose_sequence(seq, duration, false);
            let root =
                Transform::from_translation(Vec3::Y * 2.).with_rotation(Quat::from_rotation_y(1.3));
            let simulate = |hz| {
                let mut body = Body::new(&sk, &pose, &root, Vec3::new(0.3, 1., -0.2))
                    .with_profile(&sk, &profile, &surfaces);
                for _ in 0..(hz * 6) {
                    body.tick(1. / hz as f32, &floor);
                }
                assert!(body.points.iter().all(|p| p.position.is_finite()));
                let height = body.points.iter().map(|p| p.position.y).sum::<f32>()
                    / body.points.len() as f32;
                assert!(
                    height < 0.7,
                    "{} remained airborne: {height}",
                    sk.sequences()[seq].label
                );
                assert!(body.pose(&root).iter().all(|m| m.is_finite()));
                let rigid = body.rigid.as_ref().unwrap();
                assert!(
                    rigid.anchor_error() < 0.01,
                    "joint separation for {}: {}",
                    sk.sequences()[seq].label,
                    rigid.anchor_error()
                );
                let bottom = rigid
                    .bodies
                    .iter()
                    .flat_map(|b| {
                        b.vertices
                            .iter()
                            .map(move |v| (b.position + b.rotation * (*v - b.center)).y)
                    })
                    .fold(f32::INFINITY, f32::min);
                assert!(
                    bottom >= -0.002,
                    "{} penetrated floor: {bottom}",
                    sk.sequences()[seq].label
                );
                body
            };
            let slow = simulate(30);
            let fast = simulate(120);
            assert!(
                slow.points
                    .iter()
                    .zip(&fast.points)
                    .all(|(a, b)| a.position.distance(b.position) < 0.001),
                "frame-dependent corpse: {}",
                sk.sequences()[seq].label
            );
        }
    }
    #[test]
    fn airborne_native_corpse_keeps_falling_and_settles_with_connected_limbs() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets =
            std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let seq = sk.find_sequence("HeadshotBack").unwrap();
        let duration = sk
            .sequence_anim(seq)
            .map(|a| sk.anims()[a].duration())
            .unwrap();
        let pose = sk.pose_sequence(seq, duration, false);
        let transform = Transform::from_translation(Vec3::Y * 2.);
        let mut body = Body::new(&sk, &pose, &transform, Vec3::new(1., -2., 0.));
        let initial =
            body.points.iter().map(|p| p.position.y).sum::<f32>() / body.points.len() as f32;
        let floor = Collision::test_floor();
        for _ in 0..480 {
            body.tick(1. / 60., &floor);
        }
        let final_y =
            body.points.iter().map(|p| p.position.y).sum::<f32>() / body.points.len() as f32;
        assert!(
            final_y < initial - 1. && final_y < 0.6,
            "corpse remained suspended at {final_y}"
        );
        assert!(
            body.points
                .iter()
                .all(|p| p.position.is_finite() && p.position.y >= 0.025)
        );
        assert!(body.pose(&transform).iter().all(|p| p.is_finite()));
        for l in body.links.iter().filter(|l| l.min == l.max) {
            assert!(
                (body.points[l.a]
                    .position
                    .distance(body.points[l.b].position)
                    - l.min)
                    .abs()
                    < 0.2
            );
        }
        // Simulate a settled body losing its supporting map object.
        body.sleeping = true;
        let before = body.points[0].position.y;
        for _ in 0..60 {
            body.tick(1. / 60., &Collision::default());
        }
        assert!(
            !body.sleeping && body.points[0].position.y < before - 1.,
            "lost support left the corpse frozen"
        );
    }
    #[test]
    fn native_gib_profiles_keep_mass_scaled_impacts_small_fragments_and_settle() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets =
            std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![]).unwrap());
        let registry = crate::infected_gore::Registry::load(&mut assets.borrow_mut()).unwrap();
        let surfaces = crate::physics_data::Surfaces::load(&mut assets.borrow_mut()).unwrap();
        let paths = registry
            .wounds
            .iter()
            .flat_map(|w| w.gibs.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        let mut tested = std::collections::BTreeSet::new();
        for path in paths {
            let Ok(profile) = crate::physics_data::Profile::load(&mut assets.borrow_mut(), &path)
            else {
                continue;
            };
            let count = profile.solids.len();
            if count == 0 || tested.contains(&count.min(3)) {
                continue;
            }
            let sk = Skeleton::load_with(&path, |p| assets.borrow_mut().read(p).ok()).unwrap();
            let pose = sk
                .bones
                .iter()
                .map(|b| b.pose_to_bone.inverse())
                .collect::<Vec<_>>();
            let root = Transform::from_translation(Vec3::Y * 3.);
            let mut body = Body::new(&sk, &pose, &root, Vec3::X)
                .with_fragment_profile(&sk, &profile, &surfaces);
            let rigid = body.rigid.as_ref().expect(&path);
            assert_eq!(rigid.bodies.len(), count, "{path}");
            assert_eq!(rigid.joints.len(), profile.joints.len(), "{path}");
            assert!(rigid.bodies.iter().all(|b| !b.hulls.is_empty()), "{path}");
            let start = rigid.bodies[0].position.y;
            for dt in [1. / 30., 1. / 144.] {
                let previous = Transform::from_translation(root.translation - Vec3::X * 3. * dt);
                body.inherit_pose_motion(&pose, &previous, dt);
                let rigid = body.rigid.as_ref().unwrap();
                assert!(
                    rigid
                        .bodies
                        .iter()
                        .all(|b| (b.velocity - Vec3::X * 4.).length() < 0.001),
                    "lost root motion {path}"
                );
                assert!(
                    rigid.bodies.iter().all(|b| b.angular.length() < 0.001),
                    "invented spin {path}"
                );
                let previous = Transform::from_translation(root.translation)
                    .with_rotation(Quat::from_rotation_y(-2. * dt));
                body.inherit_pose_motion(&pose, &previous, dt);
                assert!(
                    body.rigid
                        .as_ref()
                        .unwrap()
                        .bodies
                        .iter()
                        .all(|b| (b.angular - Vec3::Y * 2.).length() < 0.001),
                    "lost angular motion {path}"
                );
            }
            body.inherit_pose_motion(&pose, &root, 1. / 60.);
            let before = body.rigid.as_ref().unwrap().bodies.iter()
                .map(|b| (b.velocity, b.angular, b.inverse_mass)).collect::<Vec<_>>();
            // 100 kg*inch/s becomes 2.54 kg*m/s once. ApplyForceCenter adds
            // momentum to every PHY body independently, preserving direction
            // and inherited angular motion without a fictitious upward kick.
            let impulse = Vec3::X * (100. * crate::collision::UNIT);
            body.apply_fragment_impulse(impulse);
            for (b, (velocity, angular, inverse_mass)) in
                body.rigid.as_ref().unwrap().bodies.iter().zip(&before)
            {
                assert!((b.velocity - *velocity - impulse * *inverse_mass).length() < 0.00001,
                    "fragment impulse ignored native body mass {path}");
                assert_eq!(b.velocity.y, velocity.y, "invented upward kick {path}");
                assert_eq!(b.angular, *angular, "center force invented torque {path}");
            }
            body.apply_fragment_impulse(Vec3::splat(f32::NAN));
            assert!(body.rigid.as_ref().unwrap().bodies.iter().all(|b| b.velocity.is_finite()));
            // Retail's IVP environment limits launch speed even on tiny pieces.
            // Ordinary cuts must neither consume scatter RNG nor invent torque.
            let mut scatter_rng = l4d2r_game_core::random::UniformRandomStream::new(127);
            let mut reference_rng = scatter_rng.clone();
            let before_angular = body.rigid.as_ref().unwrap().bodies.iter()
                .map(|b| b.angular).collect::<Vec<_>>();
            body.launch_fragment(Some(Vec3::X * 1e7), false, &mut scatter_rng);
            for (b, before) in body.rigid.as_ref().unwrap().bodies.iter().zip(before_angular) {
                assert!((b.velocity.length() - 50.8).abs() < 0.0001, "{path}");
                assert_eq!(b.angular, before, "ordinary cut invented scatter torque: {path}");
            }
            assert_eq!(scatter_rng.generate_random_number(), reference_rng.generate_random_number());
            // Native order: three quantized random axes once per piece, then
            // three torque axes independently for each physical body.
            let count = body.rigid.as_ref().unwrap().bodies.len();
            for _ in 0..3 + count * 3 { reference_rng.random_int(0, 32767); }
            let applied = body.launch_fragment(Some(Vec3::X * 1e7), true, &mut scatter_rng);
            assert!(applied.is_finite());
            assert_eq!(scatter_rng.generate_random_number(), reference_rng.generate_random_number());
            for b in &body.rigid.as_ref().unwrap().bodies {
                assert!(b.velocity.is_finite() && b.velocity.length() <= 50.8001, "{path}");
                assert!(b.angular.is_finite() && b.angular.length() <= 0.95199776 * 60. + 0.0001, "{path}");
                assert!(b.angular.length() > 0., "missing per-body scatter torque: {path}");
            }
            body.inherit_pose_motion(&pose, &root, 1. / 60.);
            let floor = Collision::test_floor();
            for _ in 0..360 {
                body.tick(1. / 60., &floor);
            }
            let rigid = body.rigid.as_ref().unwrap();
            assert!(
                rigid.bodies[0].position.y < start - 1.,
                "floating fragment {path}"
            );
            assert!(
                rigid
                    .bodies
                    .iter()
                    .all(|b| b.position.is_finite() && b.velocity.is_finite()),
                "{path}"
            );
            assert!(rigid.anchor_error() < 0.15, "disconnected fragment {path}");
            assert!(body.pose(&root).iter().all(|p| p.is_finite()), "{path}");
            println!(
                "Native gib checked {path}: {count} bodies, {} joints",
                profile.joints.len()
            );
            tested.insert(count.min(3));
        }
        assert!(
            tested.contains(&1),
            "single-body fragments must use native physics"
        );
        assert!(
            tested.iter().any(|n| *n > 1),
            "jointed fragments must be covered"
        );
    }
}
/// Ragdoll and gib solver steps allowed per rendered frame, shared by every
/// caller. Each awake body costs about 1 ms per step; without a budget a
/// horde's corpses all solved in the same frames and caused 30-140 ms hitches.
pub const FRAME_STEP_BUDGET: u32 = 8;
// Unlimited until the game's `First` refill runs, so tools and tests that
// step bodies directly are never throttled.
static BUDGET: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
/// Refills the shared step budget (runs in `First`).
pub fn refill_budget() {
    BUDGET.store(FRAME_STEP_BUDGET, std::sync::atomic::Ordering::Relaxed);
}
// Pack total remaining steps in the low half and a gib reservation in the
// high half. Corpses run before gibs, so an unreserved shared pool starves
// detached limbs whenever a horde fills the corpse worker batch.
fn claim_steps(value: u32, steps: u32, gib: bool) -> Option<u32> {
    if value == u32::MAX { return Some(value); } // tools/tests: unlimited
    let left = value & 0xffff;
    let reserved = value >> 16;
    let available = if gib { left } else { left.saturating_sub(reserved) };
    (available >= steps).then(|| (left - steps)
        | (if gib { reserved.saturating_sub(steps) } else { reserved } << 16))
}
pub(crate) fn reserve_gib_steps(demand: u32) {
    use std::sync::atomic::Ordering::Relaxed;
    let _ = BUDGET.try_update(Relaxed, Relaxed, |value| {
        if value == u32::MAX { return Some(value); }
        let left = value & 0xffff;
        Some(left | (demand.min(FRAME_STEP_BUDGET / 2).min(left) << 16))
    });
}
pub(crate) fn take_gib_budget(steps: u32) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    BUDGET.try_update(Relaxed, Relaxed, |value| claim_steps(value, steps, true)).is_ok()
}
#[cfg(test)]
pub(crate) fn unlimit_budget_for_test() {
    BUDGET.store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
}
/// Reserves `steps` from this frame's budget. A body whose steps do not fit
/// should `defer`; the first body of a frame always fits so none can stall.
pub fn take_budget(steps: u32) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if steps == 0 {
        return true;
    }
    BUDGET.try_update(Relaxed, Relaxed, |value| claim_steps(value, steps, false)).is_ok()
}

#[cfg(test)]
mod shared_budget_tests {
    use super::*;
    #[test]
    fn busy_corpses_cannot_spend_the_gib_reservation_or_expand_the_frame_budget() {
        let mut value = FRAME_STEP_BUDGET | (4 << 16);
        for _ in 0..2 { value = claim_steps(value, 2, false).unwrap(); }
        assert!(claim_steps(value, 2, false).is_none());
        for _ in 0..2 { value = claim_steps(value, 2, true).unwrap(); }
        assert_eq!(value, 0);
        assert!(claim_steps(value, 1, true).is_none());
        // With no competing corpses, gibs can use the entire frame budget.
        let mut value = FRAME_STEP_BUDGET | (4 << 16);
        for _ in 0..4 { value = claim_steps(value, 2, true).unwrap(); }
        assert_eq!(value, 0);
        assert_eq!(claim_steps(u32::MAX, 2, false), Some(u32::MAX));
    }
}
