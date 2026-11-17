//! Owned IVP10103020 single-impact response. Contact discovery, time of
//! impact, delayed cores and the surrounding friction system are separate.
use bevy::prelude::*;

///10102be0/10103020 increment a per-PSI count for movable cores, then
///install the anomaly callback's two-bit decision. Caller owns the actual
///callback and queued/raw motion transfer; this is not a global retry cap.
#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct ImpactBudget {pub count:u16,pub flags:u16,pub limit:i32}
impl ImpactBudget {
    pub(crate) fn register(&mut self,accept_hold:bool)->bool{
        self.count=self.count.wrapping_add(if self.flags&2==0{1}else{0});
        if self.count as i16 as i32>self.limit{
            let decision=u16::from(accept_hold)<<6;
            self.flags^=(decision^self.flags)&0xc0;
        }
        self.flags&0xc0!=0
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Body {
    pub inverse_mass: f32,
    pub inverse_inertia: Mat3,
    pub arm: Vec3,
    pub velocity: Vec3,
    pub angular: Vec3,
}
impl Body {
    fn surface(&self) -> Vec3 {
        self.velocity + self.angular.cross(self.arm)
    }
    fn change(&self, push: Vec3) -> (Vec3, Vec3) {
        if self.inverse_mass == 0. {
            return (Vec3::ZERO, Vec3::ZERO);
        }
        (
            push * self.inverse_mass,
            self.inverse_inertia * self.arm.cross(push),
        )
    }
    fn response(&self, push: Vec3) -> Vec3 {
        let (v, w) = self.change(push);
        v + w.cross(self.arm)
    }
}

/// Native event-space body. Linear velocity and the matrix use IVP world
/// axes; angular velocity, contact arm and diagonal inverse inertia remain in
/// the physical core frame. Raw static-core masses are retained with flags.
#[derive(Clone, Copy)]
pub(crate) struct CoreBody {
    pub flags: u16,
    pub inverse_mass: f32,
    pub inverse_inertia: Vec3,
    pub rotation: Mat3,
    pub arm: Vec3,
    pub velocity: Vec3,
    pub angular: Vec3,
}
trait ImpactBody: Copy {
    fn inverse_mass(&self) -> f32;
    fn valid(&self) -> bool;
    fn surface(&self) -> Vec3;
    fn change(&self, push: Vec3) -> (Vec3, Vec3);
    fn response(&self, push: Vec3) -> Vec3;
    fn motion_mut(&mut self) -> (&mut Vec3, &mut Vec3);
    fn clip(&mut self, settings: Settings) -> bool;
}
impl ImpactBody for Body {
    fn inverse_mass(&self) -> f32 { self.inverse_mass }
    fn valid(&self) -> bool {
        self.inverse_mass >= 0. && self.inverse_mass.is_finite()
            && self.arm.is_finite() && self.velocity.is_finite()
            && self.angular.is_finite() && self.inverse_inertia.is_finite()
    }
    fn surface(&self) -> Vec3 { Body::surface(self) }
    fn change(&self, push: Vec3) -> (Vec3, Vec3) { Body::change(self,push) }
    fn response(&self, push: Vec3) -> Vec3 { Body::response(self,push) }
    fn motion_mut(&mut self) -> (&mut Vec3,&mut Vec3) { (&mut self.velocity,&mut self.angular) }
    fn clip(&mut self, settings: Settings) -> bool { clip(self,settings) }
}
impl ImpactBody for CoreBody {
    fn inverse_mass(&self) -> f32 { if self.flags & 2 != 0 {0.} else {self.inverse_mass} }
    fn valid(&self) -> bool {
        self.inverse_mass >= 0. && self.inverse_mass.is_finite()
            && self.inverse_inertia.is_finite() && self.inverse_inertia.min_element() >= 0.
            && self.rotation.is_finite() && self.arm.is_finite()
            && self.velocity.is_finite() && self.angular.is_finite()
    }
    fn surface(&self) -> Vec3 {
        //100eee70: local angular cross local arm, then rotate to world.
        self.velocity + self.rotation * self.angular.cross(self.arm)
    }
    fn change(&self, push: Vec3) -> (Vec3, Vec3) {
        if self.flags & 2 != 0 {return (Vec3::ZERO,Vec3::ZERO);}
        //10102240 ->100f0080 retains torque in the core's principal axes.
        let local = self.rotation.transpose() * push;
        (push * self.inverse_mass, self.arm.cross(local) * self.inverse_inertia)
    }
    fn response(&self, push: Vec3) -> Vec3 {
        //100f0f10's raw static estimate is overwritten by the opposite
        // mass below;10101bb0 must omit static response from rescue entirely.
        if self.flags & 0x12 != 0 {return Vec3::ZERO;}
        let local = self.rotation.transpose() * push;
        let angular = self.arm.cross(local) * self.inverse_inertia;
        self.rotation * angular.cross(self.arm) + push * self.inverse_mass
    }
    fn motion_mut(&mut self) -> (&mut Vec3,&mut Vec3) { (&mut self.velocity,&mut self.angular) }
    fn clip(&mut self, s: Settings) -> bool {
        self.flags & 2 == 0 && clip_vectors(&mut self.velocity,&mut self.angular,
            s.linear_limit,s.angular_limit,s.angular_limit_exact)
    }
}
fn adjust<T: ImpactBody>(body: &mut T, change: (Vec3,Vec3), subtract: bool) {
    let (v,w)=body.motion_mut();
    if subtract { *v-=change.0;*w-=change.1; }
    else { *v+=change.0;*w+=change.1; }
}

fn squared(v: Vec3) -> f32 {
    (v.x * v.x + v.y * v.y) + v.z * v.z
}

//100f0f10 ->100ec3b0 -> reciprocal: the SSE response components
// enter an x87 squared length without a float spill before the square root.
fn response_inverse_length(v:Vec3)->f32{
    let [x,y,z]=v.to_array().map(|v|v as f64);
    (1./((x*x+y*y)+z*z).sqrt()) as f32
}

// Owned100ec450/100ec4d0: sum squares in SSE float, then retain the
// x87 reciprocal square root through each component multiplication.
fn normalize(v: Vec3) -> Vec3 {
    let square = squared(v);
    if square < 1e-10 {
        return v;
    }
    let inverse = 1. / (square as f64).sqrt();
    Vec3::new(
        (v.x as f64 * inverse) as f32,
        (v.y as f64 * inverse) as f32,
        (v.z as f64 * inverse) as f32,
    )
}

#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub energy: f32,
    pub friction: f32,
    pub system_pushes: u32,
    pub rescue_speed: f32,
    pub linear_limit: f32,
    pub angular_limit: f32,
    pub angular_limit_exact: f64,
}
#[derive(Debug)]
pub(crate) struct Outcome {
    pub compression_pushes: u32,
    pub clipped: bool,
}

// Owned10100030/101000b0 reduce an EXCEEDED limit to99%/90%.
// They do not continuously damp velocities below those thresholds.
fn clip(body: &mut Body, settings: Settings) -> bool {
    if body.inverse_mass == 0. {
        return false;
    }
    clip_vectors(
        &mut body.velocity,
        &mut body.angular,
        settings.linear_limit,
        settings.angular_limit,
        settings.angular_limit_exact,
    )
}

pub(crate) fn clip_vectors(
    velocity: &mut Vec3,
    angular: &mut Vec3,
    linear_limit: f32,
    angular_limit: f32,
    angular_limit_exact: f64,
) -> bool {
    let mut clipped = false;
    for (value, limit, exact, part) in [
        (velocity, linear_limit, linear_limit as f64, 0.99f32),
        (angular, angular_limit, angular_limit_exact, 0.9f32),
    ] {
        let square = squared(*value);
        if square > limit * limit {
            //10100030/101000b0 use x87 squared length and numerator, spill
            // the scale to float, then multiply components in SSE float.
            let [x, y, z] = value.to_array().map(|v| v as f64);
            let factor = (exact * part as f64 / ((x * x + y * y) + z * z).sqrt()) as f32;
            *value *= factor;
            clipped = true;
        }
    }
    clipped
}

/// Apply the owned post-impact anomaly callbacks to renderer-space motion.
/// This is not continuous damping or the ordinary public setter's cap.
pub(crate) fn limit_render_impact(velocity: &mut Vec3, angular: &mut Vec3, dt: f32) -> bool {
    if !dt.is_finite() || dt <= 0. {
        return false;
    }
    let basis = Vec3::new(1., -1., -1.); // Renderer -> native IVP, a180-degree X turn.
    let mut v = *velocity * basis;
    let mut w = *angular * basis;
    let inverse_dt = dt.recip();
    let clipped = clip_vectors(
        &mut v,
        &mut w,
        50.7999992,
        0.95199776 * inverse_dt,
        0.95199776f32 as f64 * inverse_dt as f64,
    );
    if clipped {
        *velocity = v * basis;
        *angular = w * basis;
        static AUDIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *AUDIT.get_or_init(|| std::env::args().any(|s| s == "--audit-ragdoll-contacts")) {
            println!(
                "Native world-impact limit: linear={} angular={}",
                velocity.length(),
                angular.length()
            );
        }
    }
    clipped
}


fn push<T: ImpactBody>(a: &mut T, b: &mut T, force: Vec3) -> [(Vec3, Vec3); 2] {
    let changes = [a.change(force), b.change(-force)];
    adjust(a,changes[0],false);
    adjust(b,changes[1],false);
    changes
}
fn direction(relative: Vec3, normal: Vec3, cosine: f32, sine: f32) -> Vec3 {
    let mut d = normalize(relative);
    let angle = d.dot(normal);
    if angle > 0. {
        return Vec3::ZERO;
    } // Native's zero integral after crossing.
    if angle > -cosine {
        d = normalize(d - normal * angle) * sine - normal * cosine;
    }
    d
}

/// Normal points from body A toward body B. Inputs use metres, radians,
/// Source-world velocities/inertia and the native PSI-frame contact levers.
/// Delay is disabled here; the event scheduler must own deferred impulses.
pub(crate) fn resolve(a: &mut Body, b: &mut Body, normal: Vec3, s: Settings) -> Option<Outcome> {
    // Preserve native XYZ float accumulation order through long sequences
    // of friction-cone pushes. Source->IVP is an exact signed permutation.
    let basis = Mat3::from_cols(Vec3::X, Vec3::Z, Vec3::NEG_Y);
    let convert = |body: &Body| Body {
        inverse_mass: body.inverse_mass,
        inverse_inertia: basis * body.inverse_inertia * basis.transpose(),
        arm: basis * body.arm,
        velocity: basis * body.velocity,
        angular: basis * body.angular,
    };
    let mut left = convert(a);
    let mut right = convert(b);
    let result = resolve_in_ivp(&mut left, &mut right, basis * normal, s)?;
    a.velocity = basis.transpose() * left.velocity;
    a.angular = basis.transpose() * left.angular;
    b.velocity = basis.transpose() * right.velocity;
    b.angular = basis.transpose() * right.angular;
    Some(result)
}

///10103940 places a static callback object first in the impact solver.
/// This processes the response only; cache/event restamping and delay remain
/// the caller's responsibility.
pub(crate) fn resolve_cores(a: &mut CoreBody,b: &mut CoreBody,normal: Vec3,s: Settings) -> Option<Outcome> {
    if b.flags & 2 != 0 {resolve_in_ivp(b,a,-normal,s)}
    else {resolve_in_ivp(a,b,normal,s)}
}

fn used_energy(energy:f32,pushes:u32)->f32{
    //1010306c DIVSS precedes10103086 MULSS and the final subtraction.
    1.-(1./(pushes as f32*0.5+1.))*(1.-energy)
}
fn compression_scalars(masses:[f32;2],inward:f32,relative:Vec3,normal:Vec3,used:f32,pushes:u32)->(f32,f32){
    //10102640 keeps the estimate through the reciprocal/multiplications;
    //10103237 scales it before10103250 makes its single float store.
    let [a,b]=masses.map(|v|v as f64);
    //10102640 recomputes the projection from float vector components in x87.
    // The separately spilled SSE inward speed drives loop/refund decisions;
    // using that rounded scalar here loses precision for oblique contacts.
    let projection=-((normal.y as f64*relative.y as f64+normal.x as f64*relative.x as f64)+normal.z as f64*relative.z as f64);
    let estimate=((b+b)*projection)*(1./(a+b))*a;
    let impulse=(estimate*0.1f32 as f64) as f32;
    //101032d3..10103346 keeps both FSQRT products through FADDP,
    // then stores one float refund before the compression loop.
    let refund=((used as f64).sqrt()*inward as f64+(pushes as f32 as f64).sqrt()*0.01f32 as f64) as f32;
    (impulse,refund)
}

/// Effective masses used by both the impulse kernel and10102be0's delayed
/// commit decision. Velocity does not enter this point-response estimate.
fn response_masses<T:ImpactBody>(a:&T,b:&T,normal:Vec3)->Option<[f32;2]>{
    let mut masses=[response_inverse_length(a.response(normal)),response_inverse_length(b.response(normal))];
    if a.inverse_mass()==0.{masses[0]=masses[1]*100000.;}
    if b.inverse_mass()==0.{masses[1]=masses[0]*100000.;}
    masses.iter().all(|v|v.is_finite()&&*v>0.).then_some(masses)
}
///100ef310 uses an unmodified candidate core against the opposite solved
/// core. Held/fixed cores contribute zero point speed in this decision.
fn delay_surface(body:CoreBody)->Vec3{
    if body.flags&0x12!=0{return Vec3::ZERO;}
    let c=body.angular.cross(body.arm);let r=body.rotation.transpose().to_cols_array();
    Vec3::new(body.velocity.x+((r[1]*c.y+r[0]*c.x)+r[2]*c.z),
        body.velocity.y+((r[3]*c.x+r[4]*c.y)+r[5]*c.z),
        body.velocity.z+((r[6]*c.x+r[7]*c.y)+r[8]*c.z))
}
/// Return the original endpoint index deferred by10102be0. `before` uses
/// raw velocities (not raw+pending); `after` is the complete solved motion.
/// This selects a commit path; the owner moves deltas/counts into its bank.
pub(crate) fn deferred_core(before:[CoreBody;2],after:[CoreBody;2],normal:Vec3,rescue:f32)->Option<usize>{
    let order=if before[1].flags&2!=0{[1,0]}else{[0,1]};
    let n=if order[0]==0{normal}else{-normal};
    let masses=response_masses(&before[order[0]],&before[order[1]],n)?;
    let candidate=usize::from(masses[0]<=masses[1]);let id=order[candidate];let other=order[candidate^1];
    if before[id].flags&2!=0{return None;}
    let relative=delay_surface(before[id])-delay_surface(after[other]);
    let mut speed=(n.y*relative.y+n.x*relative.x)+n.z*relative.z;if candidate!=0{speed=-speed;}
    (speed<rescue*f32::from_bits(0xbf555555)).then_some(id)
}
fn resolve_in_ivp<T: ImpactBody>(a: &mut T, b: &mut T, normal: Vec3, s: Settings) -> Option<Outcome> {
    if !normal.is_finite()
        || (normal.length_squared() - 1.).abs() > 1e-4
        || ![
            s.energy,
            s.friction,
            s.rescue_speed,
            s.linear_limit,
            s.angular_limit,
        ]
        .iter()
        .all(|v| v.is_finite())
        || s.energy < 0.
        || s.friction < 0.
        || s.rescue_speed < 0.
        || s.linear_limit <= 0.
        || s.angular_limit <= 0.
        || !a.valid() || !b.valid()
        || (a.inverse_mass() == 0. && b.inverse_mass() == 0.)
    {
        return None;
    }
    // Native uses length of the point-speed response, not normal projection.
    let masses=response_masses(a,b,normal)?;
    // Owned10103940 keeps sqrt/product through x87 FPATAN, then rounds
    // the angle and friction factor for its float cosine polynomial.
    let factor = (1. + (s.energy as f64).sqrt()) * s.friction as f64;
    let angle = factor.atan() as f32;
    let square = angle * angle;
    // Owned10103940: cosine's fourth-order polynomial, not library cos.
    let cosine = (1. - square * 0.5) + square * (1. / 24.) * square;
    let sine = cosine * factor as f32;
    let used = used_energy(s.energy,s.system_pushes);
    let mut relative = b.surface() - a.surface();
    let inward = -relative.dot(normal);
    let mut pushes = 0;
    if inward >= 0.0001 {
        let (impulse,mut refund)=compression_scalars(masses,inward,relative,normal,used,s.system_pushes);
        let mut remaining = inward;
        let mut d = direction(relative, normal, cosine, sine);
        while remaining > 0. && pushes < 100 {
            push(a, b, d * impulse);
            relative = b.surface() - a.surface();
            remaining = -relative.dot(normal);
            d = direction(relative, normal, cosine, sine);
            pushes += 1;
        }
        refund += remaining;
        if refund > 0. && pushes != 100 {
            let changes = push(a, b, -normal);
            let delta=b.surface()-a.surface();
            //101034a0..101034c1 retains the Y/X/Z projection and the
            //remaining speed in x87 before spilling one float denominator.
            //The epsilon comparison uses the retained value; DIVSS uses its spill.
            let difference=((normal.y as f64*delta.y as f64+delta.x as f64*normal.x as f64)+normal.z as f64*delta.z as f64)+remaining as f64;
            let amount = if difference.abs() > 0.0001f32 as f64 {
                refund / difference as f32
            } else {
                0.
            };
            adjust(a,changes[0],true);
            adjust(b,changes[1],true);
            push(a, b, -normal * amount);
        }
    }
    // The panic branch deliberately adds a rescue push even when the
    // contact is already separating (native isolated case3 readback).
    let outgoing = (b.surface() - a.surface()).dot(normal);
    let desired = s.rescue_speed
        - if inward < 0.0001 {
            outgoing.min(0.)
        } else {
            outgoing
        };
    if desired >= 0. {
        //10101bb0 projects each body's response separately (B first), then
        // adds the scalar responses. Summing vectors before projection changes
        // tilted spinning contacts by float steps even with identical geometry.
        let k = b.response(normal).dot(normal) + a.response(normal).dot(normal);
        if k > 0. {
            push(a, b, -normal * (desired / (k + 1e-15)));
        }
    }
    let clipped = a.clip(s) | b.clip(s);
    Some(Outcome {
        compression_pushes: pushes,
        clipped,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn compression_scalars_match_observed_native_energy_impulse_and_refund_bits(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-scalars.json")).unwrap();
        let cases=data["cases"].as_array().unwrap();assert_eq!(cases.len(),72);let mut active=0;
        for case in cases{
            let s=&case["sample"];let n=&s["intermediates"];let pushes=case["pushes"].as_u64().unwrap() as u32;
            let used=used_energy(s["energy"].as_f64().unwrap() as f32,pushes);
            assert_eq!(used.to_bits() as u64,n["used_bits"].as_u64().unwrap(),"{} energy",case["key"]);
            if n["refund_valid"].as_bool().unwrap(){
                let masses=std::array::from_fn(|k|f32::from_bits(n["masses_bits"][k].as_u64().unwrap() as u32));
                let inward=f32::from_bits(n["inward_bits"].as_u64().unwrap() as u32);
                let vector=|a:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|a[i].as_f64().unwrap() as f32));
                let bodies=std::array::from_fn::<_,2,_>(|i|{let b=&s["before"][i];CoreBody{flags:b["flags"].as_u64().unwrap() as u16,inverse_mass:b["inverse_mass"].as_f64().unwrap() as f32,inverse_inertia:vector(&b["inverse_inertia"]),rotation:Mat3::from_cols_array(&std::array::from_fn(|j|b["matrix"][j].as_f64().unwrap() as f32)).transpose(),arm:vector(&b["arm"]),velocity:vector(&b["velocity"]),angular:vector(&b["angular"])}});
                let (impulse,refund)=compression_scalars(masses,inward,bodies[1].surface()-bodies[0].surface(),vector(&s["normal"]),used,pushes);
                assert_eq!(refund.to_bits() as u64,n["refund_bits"].as_u64().unwrap(),"{} refund",case["key"]);
                assert_eq!(impulse.to_bits() as u64,n["impulse_bits"].as_u64().unwrap(),"{} impulse",case["key"]);active+=1;
            }
        }assert_eq!(active,48);
    }
    #[test]
    fn response_length_matches_owned_native_x87_controls_without_float_spill(){
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-response-length.json")).unwrap();
        assert_eq!(fixture["rva"].as_u64().unwrap(),0xec3b0);
        let rows=fixture["rows"].as_array().unwrap();assert_eq!(rows.len(),256);
        for row in rows{
            let v=Vec3::from_array(std::array::from_fn(|k|f32::from_bits(row["input_bits"][k].as_u64().unwrap() as u32)));
            let actual=response_inverse_length(v).to_bits() as u64;
            assert_eq!(actual,row["inverse_bits"][1].as_u64().unwrap());
            assert_eq!(actual,row["inverse_bits"][2].as_u64().unwrap());
        }
    }
    #[test]
    fn native_per_psi_impact_count_and_default_hold_match(){
        let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-hold.json")).unwrap();let mut holds=0;
        for c in d["impact_budget"].as_array().unwrap(){
            let mut budget=ImpactBudget{count:c["before_count"].as_i64().unwrap() as u16,flags:c["before_flags"].as_u64().unwrap() as u16,limit:c["limit"].as_i64().unwrap() as i32};
            let held=budget.register(true);
            assert_eq!(budget.count as i16 as i64,c["after_count"].as_i64().unwrap());assert_eq!(budget.flags as u64,c["after_flags"].as_u64().unwrap());
            if held{holds+=1;assert_eq!(budget.count,7);assert_eq!(budget.limit,6);}
        }
        assert_eq!(d["impact_budget"].as_array().unwrap().len(),108);assert_eq!(holds,4);
    }
    use super::*;
    fn fixture(inertia: f32, arm: Vec3, v: Vec3, w: Vec3) -> (Body, Body, Settings) {
        let a = Body {
            inverse_mass: 0.,
            inverse_inertia: Mat3::ZERO,
            arm: Vec3::ZERO,
            velocity: Vec3::ZERO,
            angular: Vec3::ZERO,
        };
        let b = Body {
            inverse_mass: 0.1,
            inverse_inertia: Mat3::IDENTITY / inertia,
            arm: arm * 0.0254,
            velocity: v * 0.0254,
            angular: w * std::f32::consts::PI / 180.,
        };
        (
            a,
            b,
            Settings {
                energy: 0.05,
                friction: 0.8 * 0.8,
                system_pushes: 0,
                rescue_speed: 0.0152339041,
                linear_limit: 50.7999992,
                angular_limit: 57.1198616,
                angular_limit_exact: 57.11986178589723,
            },
        )
    }
    #[test]
    fn authored_glass_impacts_match_owned_kernel_entry_exit() {
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-glass-impacts.json")).unwrap();
        let vec=|a:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|a[i].as_f64().unwrap() as f32));
        let mut max_v=0f32;let mut max_w=0f32;
        for sample in data["cases"].as_array().unwrap() {
            let mut bodies=std::array::from_fn::<_,2,_>(|i| {
                let b=&sample["before"][i];
                CoreBody {flags:b["flags"].as_u64().unwrap() as u16,
                    inverse_mass:b["inverse_mass"].as_f64().unwrap() as f32,
                    inverse_inertia:vec(&b["inverse_inertia"]),
                    rotation:Mat3::from_cols_array(&std::array::from_fn(|j|b["matrix"][j].as_f64().unwrap() as f32)).transpose(),
                    arm:vec(&b["arm"]),velocity:vec(&b["velocity"]),angular:vec(&b["angular"])}
            });
            let [a,b]=&mut bodies;
            resolve_cores(a,b,vec(&sample["normal"]),Settings {
                energy:sample["energy"].as_f64().unwrap() as f32,friction:data["friction"].as_f64().unwrap() as f32,
                system_pushes:sample["system_pushes"].as_u64().unwrap() as u32,
                rescue_speed:sample["rescue_speed"].as_f64().unwrap() as f32,
                linear_limit:50.7999992,angular_limit:0.95199776*(1f32/60.).recip(),
                angular_limit_exact:0.95199776f32 as f64*(1f32/60.).recip() as f64,
            }).unwrap();
            for (i,b) in bodies.iter().enumerate() {
                let v=b.velocity.distance(vec(&sample["after"][i]["velocity"]))/0.0254;
                let w=b.angular.distance(vec(&sample["after"][i]["angular"])).to_degrees();
                max_v=max_v.max(v);max_w=max_w.max(w);
                if v>=0.003 || w>=0.03 {println!("{} step{} body{} v{} w{}",sample["model"],sample["step"],i,v,w);}
            }
        }
        println!("{} authored glass kernel calls: max v{max_v}, w{max_w}",data["cases"].as_array().unwrap().len());
        assert!(max_v<0.003 && max_w<0.03);
    }
    #[test]
    fn core_frame_impacts_match_owned_kernel_entry_exit() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-core-contacts.json")).unwrap();
        let vec = |v: &serde_json::Value| Vec3::new(v[0].as_f64().unwrap() as f32,v[1].as_f64().unwrap() as f32,v[2].as_f64().unwrap() as f32);
        let mut max_v = 0f32; let mut max_w = 0f32; let mut count=0;
        for case in fixture["cases"].as_array().unwrap() {
            let before=&case["before"];let after=&case["after"];
            let mut bodies=std::array::from_fn::<_,2,_>(|i| {
                let rows=before["event_core_matrix_ivp"][i].as_array().unwrap();
                let values=std::array::from_fn::<_,9,_>(|j|rows[j].as_f64().unwrap() as f32);
                CoreBody {
                    flags: before["event_core_flags"][i].as_u64().unwrap() as u16,
                    inverse_mass: before["event_inverse_mass"][i].as_f64().unwrap() as f32,
                    inverse_inertia: vec(&before["event_core_inverse_inertia"][i]),
                    rotation:Mat3::from_cols_array(&values).transpose(),
                    arm:vec(&before["event_core_arm_local"][i]),
                    velocity:vec(&before["event_core_velocity_ivp"][i]),
                    angular:vec(&before["event_core_angular_local"][i]),
                }
            });
            let n=vec(&before["normal"]);let normal=Vec3::new(n.x,-n.z,n.y);
            let [a,b]=&mut bodies;
            let result=resolve_cores(a,b,normal,Settings {
                energy:before["energy_conservation"].as_f64().unwrap() as f32,
                friction:before["friction"].as_f64().unwrap() as f32, system_pushes:before["system_pushes"].as_u64().unwrap() as u32,
                rescue_speed:before["rescue_speed"].as_f64().unwrap() as f32, linear_limit:50.7999992,
                angular_limit:0.95199776*(1f32/60.).recip(),
                angular_limit_exact:0.95199776f32 as f64*(1f32/60.).recip() as f64,
            });
            assert!(result.is_some());
            for (i,b) in bodies.iter().enumerate() {
                let v=(b.velocity-vec(&after["event_core_velocity_ivp"][i])).length()/0.0254;
                let w=(b.angular-vec(&after["event_core_angular_local"][i])).length().to_degrees();
                max_v=max_v.max(v); max_w=max_w.max(w); count+=1;
                if v>=0.003 || w>=0.03 {
                    println!("FAIL {} step{} body{} velocity{} angular{} actual_v{:?} expected_v{:?} actual_w{:?} expected_w{:?} pushes{} clipped{}",
                        case["case"],before["step"],i,v,w,b.velocity,vec(&after["event_core_velocity_ivp"][i]),b.angular,vec(&after["event_core_angular_local"][i]),
                        result.as_ref().unwrap().compression_pushes,result.as_ref().unwrap().clipped);
                }
            }
        }
        println!("Owned core events: {count} body states; max velocity {max_v} Source-units/s; max angular {max_w} deg/s");
        assert!(max_v<0.003 && max_w<0.03);
    }

    #[test]
    fn owned_box_and_sphere_kernel_readbacks() {
        // tools/run_native_impact_kernel.py, owned DLL45aa54cd; no fitted
        // rebound/rotation coefficients and no collision-time alignment.
        let references = [
            (
                0.0486610122,
                [-40.6629257, -40.6629257, -263.513947],
                [2082.75073, -2082.75073, 0.],
                [84.9421539, -50.4506989, -96.352005],
                [-1006.1131, -888.733093, -157.380051],
            ),
            (
                0.0412902385,
                [-35.39534, -35.39534, -278.31604],
                [2082.75073, -2082.75073, 0.],
                [89.6072235, -54.3534775, -103.878403],
                [-1061.30823, -941.345398, -159.962814],
            ),
        ];
        for (inertia, v0, w0, v1, w1) in references {
            for (arm, v, w, expected_v, expected_w) in [
                (
                    Vec3::new(4., 4., -4.),
                    Vec3::new(0., 0., -386.66684),
                    Vec3::ZERO,
                    v0,
                    w0,
                ),
                (
                    Vec3::new(4., -4., -4.),
                    Vec3::new(120., -80., -160.),
                    Vec3::new(30., -20., 10.),
                    v1,
                    w1,
                ),
                (
                    Vec3::new(0., 0., -4.),
                    Vec3::new(0., 0., -10.),
                    Vec3::ZERO,
                    [0., 0., 2.23606849],
                    [0.; 3],
                ),
                (
                    Vec3::new(0., 0., -4.),
                    Vec3::new(0., 0., 10.),
                    Vec3::ZERO,
                    [0., 0., 10.5997601],
                    [0.; 3],
                ),
            ] {
                let (mut a, mut b, s) = fixture(inertia, arm, v, w);
                let result = resolve(&mut a, &mut b, Vec3::Z, s).unwrap();
                let error_v = (b.velocity / 0.0254 - Vec3::from_array(expected_v)).length();
                let degrees = b.angular * (180. / std::f32::consts::PI);
                let error_w = (degrees - Vec3::from_array(expected_w)).length();
                assert!(
                    error_v < 0.003,
                    "v error{error_v}: {:?}",
                    b.velocity / 0.0254
                );
                assert!(error_w < 0.03, "w error{error_w}: {degrees:?}");
                assert!(result.compression_pushes <= 100);
                if v.z < -300. {
                    assert!(result.clipped);
                }
            }
        }
    }
    #[test]
    fn invalid_normal_does_not_mutate_states() {
        let (mut a, mut b, s) = fixture(0.05, Vec3::ZERO, Vec3::NEG_Z, Vec3::ZERO);
        let before = b.velocity;
        assert!(resolve(&mut a, &mut b, Vec3::ZERO, s).is_none());
        assert_eq!(b.velocity, before);
    }
    #[test]
    fn owned_high_friction_response_preserves_native_normalization_precision() {
        let (mut a, mut b, mut s) = fixture(
            0.0412902385,
            Vec3::new(4., -4., -4.),
            Vec3::new(120., -80., -160.),
            Vec3::new(30., -20., 10.),
        );
        s.friction = 1.5;
        resolve(&mut a, &mut b, Vec3::Z, s).unwrap();
        assert!(
            (b.velocity / 0.0254 - Vec3::new(-2.48474312, 37.2018433, -27.358696)).length() < 0.003
        );
        assert!(
            (b.angular * (180. / std::f32::consts::PI)
                - Vec3::new(-522.884338, -383.705261, -179.178986))
            .length()
                < 0.03
        );
    }
    #[test]
    fn owned_two_movable_body_response() {
        let (mut a, mut b, s) = fixture(
            0.0486610122,
            Vec3::NEG_Z * 4.,
            Vec3::NEG_Z * 10.,
            Vec3::ZERO,
        );
        a.inverse_mass = 0.2;
        a.inverse_inertia = Mat3::IDENTITY / (0.0486610122 * 0.5);
        a.arm = Vec3::Z * (4. * 0.0254);
        resolve(&mut a, &mut b, Vec3::Z, s).unwrap();
        assert!((a.velocity / 0.0254 - Vec3::NEG_Z * 8.15737915).length() < 0.00002);
        assert!((b.velocity / 0.0254 - Vec3::NEG_Z * 5.9213109).length() < 0.00002);
        assert_eq!(a.angular, Vec3::ZERO);
        assert_eq!(b.angular, Vec3::ZERO);
    }
    #[test]
    fn owned_linear_and_angular_anomaly_readbacks() {
        let (mut a, mut b, s) = fixture(
            0.0486610122,
            Vec3::NEG_Z * 4.,
            Vec3::new(8000., 0., -5.),
            Vec3::ZERO,
        );
        assert!(resolve(&mut a, &mut b, Vec3::Z, s).unwrap().clipped);
        assert!((b.velocity / 0.0254 - Vec3::new(1980., 0., 0.276862741)).length() < 0.003);
        assert!((b.angular * (180. / std::f32::consts::PI) - Vec3::Y * 131.158936).length() < 0.03);
        let (mut a, mut b, s) = fixture(
            0.0486610122,
            Vec3::NEG_Z * 4.,
            Vec3::Z * 10.,
            Vec3::new(9000., 1000., -5000.),
        );
        assert!(resolve(&mut a, &mut b, Vec3::Z, s).unwrap().clipped);
        assert!(
            (b.angular * (180. / std::f32::consts::PI)
                - Vec3::new(2562.73022, 284.747833, -1423.73901))
            .length()
                < 0.03
        );
    }
    #[test]
    #[ignore = "manual single-kernel replay against an owned DLL capture"]
    fn replay_owned_impact_kernel_report() {
        let path = std::env::var("L4D2R_IVP_IMPACT_REFERENCE").expect("owned reference JSON path");
        let native: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            native["native_module_sha256"],
            "45aa54cd8907b62ba1e7798ed3b3a27575ba761b500968338dcdfbf3ee93327d"
        );
        let mut worst_v = 0.0f32;
        let mut worst_w = 0.0f32;
        let mut cases = Vec::new();
        for case in native["cases"].as_array().unwrap() {
            let value = |key: &str| case[key].as_f64().unwrap() as f32;
            let vector = |key: &str| {
                let a = case[key].as_array().unwrap();
                assert_eq!(a.len(), 3);
                Vec3::new(
                    a[0].as_f64().unwrap() as f32,
                    a[1].as_f64().unwrap() as f32,
                    a[2].as_f64().unwrap() as f32,
                )
            };
            let tensor = |key: &str| {
                let a = case[key].as_array().unwrap();
                assert_eq!(a.len(), 9);
                let mut values = [0.; 9];
                for (i, v) in a.iter().enumerate() {
                    values[i] = v.as_f64().unwrap() as f32;
                }
                Mat3::from_cols_array(&values).transpose()
            };
            assert_eq!(case["rva"], "10103020");
            let pair = case["dynamic_support"].as_bool().unwrap();
            let mut a = Body {
                inverse_mass: 0.,
                inverse_inertia: Mat3::ZERO,
                arm: Vec3::ZERO,
                velocity: Vec3::ZERO,
                angular: Vec3::ZERO,
            };
            if pair {
                a = Body {
                    inverse_mass: value("support_mass").recip(),
                    inverse_inertia: tensor("support_inverse_inertia_world_rows"),
                    arm: vector("support_arm_source_units") * 0.0254,
                    velocity: vector("support_before_velocity") * 0.0254,
                    angular: vector("support_before_angular") * (std::f32::consts::PI / 180.),
                };
            }
            let mut b = Body {
                inverse_mass: value("mass").recip(),
                inverse_inertia: tensor("inverse_inertia_world_rows"),
                arm: vector("arm_source_units") * 0.0254,
                velocity: vector("before_velocity") * 0.0254,
                angular: vector("before_angular") * (std::f32::consts::PI / 180.),
            };
            let settings = Settings {
                energy: value("energy"),
                friction: value("friction"),
                system_pushes: case["system_pushes"].as_u64().unwrap() as u32,
                rescue_speed: value("rescue_speed"),
                linear_limit: value("linear_limit"),
                angular_limit: value("angular_limit"),
                angular_limit_exact: case["angular_limit_exact"]
                    .as_f64()
                    .unwrap_or(value("angular_limit") as f64),
            };
            let outcome = resolve(&mut a, &mut b, vector("normal_source"), settings).unwrap();
            let mut v = (b.velocity / 0.0254 - vector("after_velocity")).length();
            let mut w =
                (b.angular * (180. / std::f32::consts::PI) - vector("after_angular")).length();
            if pair {
                v = v.max((a.velocity / 0.0254 - vector("support_after_velocity")).length());
                w = w.max(
                    (a.angular * (180. / std::f32::consts::PI) - vector("support_after_angular"))
                        .length(),
                );
            }
            worst_v = worst_v.max(v);
            worst_w = worst_w.max(w);
            cases.push(serde_json::json!({"shape":case["shape"],"case":case["case"],"dynamic_support":pair,
                "velocity_error_units_s":v,"angular_error_degrees_s":w,"compression_pushes":outcome.compression_pushes,"clipped":outcome.clipped}));
        }
        let report = serde_json::json!({"cases":cases,"max_velocity_error_units_s":worst_v,"max_angular_error_degrees_s":worst_w,
            "scope":"single native impact kernel; no contact detection/event order/deferred-core parity claim"});
        if let Ok(path) = std::env::var("L4D2R_IVP_IMPACT_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
        println!("{report}");
        assert!(worst_v < 0.003, "native velocity error{worst_v}");
        assert!(worst_w < 0.03, "native angular error{worst_w}");
    }
}
