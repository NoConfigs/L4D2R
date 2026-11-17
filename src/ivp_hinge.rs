//! Owned hinge100b7660/100b7210: five coupled bilateral rows, with a separate
//! once-per-PSI angular limit/motor. Contact and cache scheduling are separate.
use crate::articulated::{Joint, Rigid};
use bevy::prelude::*;

#[derive(Clone, Copy)]
struct Row {
    linear_a: Vec3,
    angular_a: Vec3,
    linear_b: Vec3,
    angular_b: Vec3,
}

pub(crate) struct Solver {
    rows: [Row; 5],
    inverse: [[f32; 5]; 5],
    rhs: [f32; 5],
    inertia_a: Mat3,
    inertia_b: Mat3,
    axis: Vec3,
    angle: f32,
    low: f32,
    high: f32,
    friction: f32,
    dt: f32,
}

//100bf5d0: positive-definite five-row matrix; forward elimination then back
// substitution without pivot reordering. Keep the native float divisions.
fn invert(mut a: [[f32; 5]; 5]) -> Option<[[f32; 5]; 5]> {
    let mut b = [[0.; 5]; 5];
    for i in 0..5 {
        b[i][i] = 1.;
    }
    for i in 0..5 {
        let pivot = a[i][i];
        if !pivot.is_finite() || pivot.abs() < 1e-12 {
            return None;
        }
        let reciprocal = 1. / pivot;
        for j in i..5 {
            a[i][j] *= reciprocal;
        }
        for j in 0..=i {
            b[i][j] *= reciprocal;
        }
        for k in i + 1..5 {
            let factor = a[k][i];
            if factor != 0. {
                for j in 0..=i {
                    b[k][j] -= b[i][j] * factor;
                }
                for j in i..5 {
                    a[k][j] -= a[i][j] * factor;
                }
            }
        }
    }
    for i in (0..5).rev() {
        for k in (0..i).rev() {
            let factor = a[k][i];
            if factor != 0. {
                for j in (0..5).rev() {
                    b[k][j] -= b[i][j] * factor;
                }
                for j in (i..5).rev() {
                    a[k][j] -= a[i][j] * factor;
                }
            }
        }
    }
    Some(b)
}

impl Solver {
    pub(crate) fn prepare(j: &Joint, a: &Rigid, b: &Rigid, dt: f32) -> Option<Self> {
        let mut free = (0..3).filter(|&i| j.max[i] != j.min[i]);
        let i = free.next()?;
        if free.next().is_some() || dt <= 0. {
            return None;
        }
        // Source axes expressed in renderer local coordinates.
        let axes = [Vec3::X, Vec3::NEG_Z, Vec3::Y];
        let axis = b.rotation * axes[i];
        let attached = a.rotation * (j.frame * axes[i]);
        let u = b.rotation * axes[(i + 1) % 3];
        let v = u.cross(axis);
        let other = a.rotation * (j.frame * axes[(i + 1) % 3]);
        let angle = (v.dot(other) as f64).atan2(u.dot(other) as f64) as f32;
        let ra = a.rotation * j.anchor_a;
        let rb = b.rotation * j.anchor_b;
        let rotation_a = Mat3::from_quat(a.rotation);
        let rotation_b = Mat3::from_quat(b.rotation);
        let ia = rotation_a * a.inverse_inertia * rotation_a.transpose();
        let ib = rotation_b * b.inverse_inertia * rotation_b.transpose();
        // Native linear rows are IVP world XYZ, not renderer XYZ.
        let directions = [Vec3::X, Vec3::NEG_Y, Vec3::NEG_Z];
        let rows = std::array::from_fn(|row| {
            if row < 3 {
                let n = directions[row];
                Row {
                    linear_a: -n,
                    angular_a: -ra.cross(n),
                    linear_b: n,
                    angular_b: rb.cross(n),
                }
            } else {
                let n = if row == 3 { u } else { v };
                Row {
                    linear_a: Vec3::ZERO,
                    angular_a: -n,
                    linear_b: Vec3::ZERO,
                    angular_b: n,
                }
            }
        });
        let k = std::array::from_fn(|r| {
            std::array::from_fn(|c| {
                let x = rows[r];
                let y = rows[c];
                // Native reference body B contributes before attached body A.
                ((x.linear_b.dot(y.linear_b) * b.inverse_mass + x.angular_b.dot(ib * y.angular_b))
                    + x.linear_a.dot(y.linear_a) * a.inverse_mass)
                    + x.angular_a.dot(ia * y.angular_a)
            })
        });
        let error = (a.position + ra) - (b.position + rb);
        let inverse_dt = dt.recip();
        let rhs = [
            error.dot(directions[0]) * inverse_dt,
            error.dot(directions[1]) * inverse_dt,
            error.dot(directions[2]) * inverse_dt,
            attached.dot(v) * inverse_dt,
            -(attached.dot(u) * inverse_dt),
        ];
        Some(Self {
            rows,
            inverse: invert(k)?,
            rhs,
            inertia_a: ia,
            inertia_b: ib,
            axis,
            angle,
            low: j.min[i],
            high: j.max[i],
            friction: j.friction[i].abs(),
            dt,
        })
    }

    ///100b7660 processes this before preparing/solving the five-row block;
    ///100b7210's later passes do not repeat the motor/limit.
    pub(crate) fn limit(&self, a: &mut Rigid, b: &mut Rigid, target: &mut f32) {
        let k = self.axis.dot((self.inertia_a + self.inertia_b) * self.axis);
        if k <= 1e-12 {
            return;
        }
        let relative = (b.angular - a.angular).dot(self.axis);
        let low = ((self.low - self.angle) / self.dt - relative) / k;
        let high = ((self.high - self.angle) / self.dt - relative) / k;
        let bound = if b.inverse_mass > 0. {
            self.friction / b.inverse_mass * self.dt
        } else {
            0.
        };
        let mut motor = 0.;
        if self.friction > 0. {
            let mut error = *target - self.angle;
            if error > std::f32::consts::PI {
                error -= std::f32::consts::TAU;
            }
            if error < -std::f32::consts::PI {
                error += std::f32::consts::TAU;
            }
            let requested = (0.8 * error / self.dt - relative) / k;
            let fraction = if requested == 0. {
                1.
            } else {
                (bound / requested.abs()).min(1.)
            };
            motor = requested * fraction;
            *target -= error - fraction * error;
        }
        let impulse = motor.clamp(low, high);
        a.angular -= self.inertia_a * (self.axis * impulse);
        b.angular += self.inertia_b * (self.axis * impulse);
    }

    pub(crate) fn solve(&self, a: &mut Rigid, b: &mut Rigid) {
        let rhs = std::array::from_fn::<_, 5, _>(|i| {
            let r = self.rows[i];
            let relative = ((r.linear_b.dot(b.velocity) + r.angular_b.dot(b.angular))
                + r.linear_a.dot(a.velocity))
                + r.angular_a.dot(a.angular);
            self.rhs[i] - relative
        });
        let delta = std::array::from_fn::<_, 5, _>(|i| {
            (0..5).map(|j| self.inverse[i][j] * rhs[j]).sum::<f32>()
        });
        let mut va = Vec3::ZERO;
        let mut wa = Vec3::ZERO;
        let mut vb = Vec3::ZERO;
        let mut wb = Vec3::ZERO;
        for (r, d) in self.rows.iter().zip(delta) {
            va += r.linear_a * d;
            wa += r.angular_a * d;
            vb += r.linear_b * d;
            wb += r.angular_b * d;
        }
        a.velocity += va * a.inverse_mass;
        a.angular += self.inertia_a * wa;
        b.velocity += vb * b.inverse_mass;
        b.angular += self.inertia_b * wb;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn body(x: f32, mass: f32) -> Rigid {
        Rigid {
            bone: 0,
            position: Vec3::new(x * 0.0254, 96. * 0.0254, 0.),
            rotation: Quat::IDENTITY,
            velocity: Vec3::ZERO,
            angular: Vec3::ZERO,
            center: Vec3::ZERO,
            inverse_mass: mass.recip(),
            inverse_inertia: Mat3::IDENTITY / (0.0412902385 * (mass / 10.)),
            vertices: vec![],
            hulls: vec![],
            damping: 0.,
            angular_damping: 0.,
            surface: Default::default(),
            world_contacts: vec![], pose_cache: None,
        }
    }
    #[test]
    fn owned_first_hinge_step_couples_five_rows_and_runs_motor_once() {
        for motor in [false, true] {
            let mut a = body(-8., 10.);
            let mut b = body(8., 5.);
            a.angular = Vec3::new(60., 30., -90.) * (std::f32::consts::PI / 180.);
            let j = Joint {
                a: 0,
                b: 1,
                anchor_a: Vec3::X * 8. * 0.0254,
                anchor_b: Vec3::NEG_X * 8. * 0.0254,
                frame: Quat::IDENTITY,
                min: Vec3::new(0., -10., 0.) * (std::f32::consts::PI / 180.),
                max: Vec3::new(0., 50., 0.) * (std::f32::consts::PI / 180.),
                friction: if motor { Vec3::Y * 5. } else { Vec3::ZERO },
                native: None,
                hinge_target: 0.,
            };
            let solver = Solver::prepare(&j, &a, &b, 1. / 60.).unwrap();
            assert!((solver.low - (-10f32).to_radians()).abs() < 1e-7);
            assert!((solver.high - 50f32.to_radians()).abs() < 1e-7);
            solver.limit(&mut a, &mut b, &mut 0.);
            for _ in 0..5 {
                solver.solve(&mut a, &mut b);
            }
            // Owned first PSI: locked X/Z angular motion becomes common;
            // the free Y motion couples to both anchor velocities.
            let expected = if motor {
                Vec3::new(40.0000038, 2.02247214, -23.6363659)
            } else {
                Vec3::new(40.0000038, 2.02247214, -62.7272758)
            };
            assert!(
                (a.angular - expected * (std::f32::consts::PI / 180.))
                    .abs()
                    .max_element()
                    < 0.00002,
                "{:?}",
                a.angular
            );
            let z = if motor { 0.507732153 } else { 0.380799115 };
            assert!(a.velocity.distance(Vec3::new(0., z, 0.188260257) * 0.0254) < 0.000002);
        }
    }
    #[test]
    fn weak_motor_keeps_native_sliding_target_across_three_ticks() {
        let mut a = body(-8., 10.);
        a.angular = Vec3::new(60., 30., -90.) * (std::f32::consts::PI / 180.);
        let j = Joint {
            a: 0, b: 1,
            anchor_a: Vec3::X * 8. * 0.0254,
            anchor_b: Vec3::NEG_X * 8. * 0.0254,
            frame: Quat::IDENTITY,
            min: Vec3::new(0., -10f32.to_radians(), 0.),
            max: Vec3::new(0., 50f32.to_radians(), 0.),
            friction: Vec3::Y * 0.002,
            native: None, hinge_target: 0.,
        };
        let mut rig = crate::articulated::Articulation {
            bodies: vec![a, body(8., 5.)], joints: vec![j],
            collision_pairs: vec![], initial_intrusions: None,
            pending_penetrations: vec![], friction: 0.64, elasticity: 0.05,
        };
        let collision = crate::collision::Collision::test_solid_floor();
        for _ in 0..3 {
            rig.step_acceleration(1. / 60., &collision, true, Vec3::ZERO);
        }
        // Owned 32-bit IVP weak-motor fixture, third PSI. A stateless
        // velocity brake cannot reproduce this sliding angular target.
        for (b, expected) in rig.bodies.iter().zip([
            Vec3::new(39.9480782, 3.52808547, -61.3399696),
            Vec3::new(40.1066284, 0.194589213, 52.9420128),
        ]) {
            assert!((b.angular - expected * (std::f32::consts::PI / 180.))
                .abs().max_element() < 0.01f32.to_radians(), "{:?}", b.angular);
        }
        assert!(rig.joints[0].hinge_target.abs() > 0.001);
    }

}
