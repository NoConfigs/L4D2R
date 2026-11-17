//! Ordinary owned 100fe760 tangent response. Controller/material-directed
//! branches and contact discovery/lifetimes are separate responsibilities.
use bevy::prelude::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Body {
    pub active: bool,
    pub inverse_mass: f32,
    pub inverse_inertia: Vec3,
    pub rotation: Mat3,
    pub position: Vec3,
    pub linear: Vec3,
    pub angular: Vec3,
}
pub(crate) struct Parameters {
    pub point: Vec3,
    pub basis: [Vec3; 2],
    pub mix: [f32; 2],
    pub dt: f32,
    pub inverse_dt: f32,
    pub normal_force: f32,
    pub friction: f32,
    pub minimum_impulse: f32,
    pub minimum_determinant_squared: f32,
    pub alternate_candidate: bool,
}
#[derive(Default, Debug)]
pub(crate) struct Outcome {
    pub initialized: bool,
    pub applied: bool,
    pub clamped: bool,
    pub matrix: [f32; 3],
    pub speed: [f32; 2],
    pub jacobian: [[Vec3; 2]; 2],
    pub torque: [[Vec3; 2]; 2],
    pub impulse: [f32; 2],
    pub work_delta: f64,
}
fn dot(a: Vec3, b: Vec3) -> f32 { (a.y*b.y+a.x*b.x)+a.z*b.z }

/// Uses raw velocities and principal core inertia axes. Pending velocities are
/// deliberately outside this API. Captured matrices/impulses are never inputs.
pub(crate) fn ordinary(bodies: &mut [Body; 2], p: &Parameters, work: &mut f32) -> Result<Outcome, &'static str> {
    if p.alternate_candidate { return Err("alternate friction controller is not reconstructed"); }
    let mut o=Outcome::default();
    let cap=(p.normal_force*p.friction)*p.dt;
    if cap<p.minimum_impulse { return Ok(o); }
    o.initialized=true;
    for side in 0..2 {
        let b=bodies[side];
        if !b.active { continue; }
        let arm=p.point-b.position;
        for axis in 0..2 {
            let cross=arm.cross(p.basis[axis]);
            let j=Vec3::new(dot(b.rotation.x_axis,cross),dot(b.rotation.y_axis,cross),dot(b.rotation.z_axis,cross));
            o.jacobian[side][axis]=j;
            o.torque[side][axis]=j*b.inverse_inertia;
            let projected=dot(p.basis[axis],b.linear)+dot(j,b.angular);
            o.speed[axis]+=if side==0 {projected}else{-projected};
        }
        let [j0,j1]=o.jacobian[side]; let [t0,t1]=o.torque[side];
        o.matrix[0]+=dot(j0,t0)+b.inverse_mass;
        o.matrix[1]+=dot(j0,t1);
        o.matrix[2]+=dot(j1,t1)+b.inverse_mass;
    }
    let [a,b,c]=o.matrix;
    let det=a*c-b*b;
    if det*det<p.minimum_determinant_squared { return Ok(o); }
    let rhs0=p.inverse_dt*p.mix[0]-o.speed[0];
    let rhs1=p.inverse_dt*p.mix[1]-o.speed[1];
    let inv=1./det; let cross=-(inv*b);
    o.impulse=[cross*rhs1+(inv*c)*rhs0,(inv*a)*rhs1+cross*rhs0];
    let length_squared=o.impulse[1]*o.impulse[1]+o.impulse[0]*o.impulse[0];
    if cap*cap<length_squared {
        o.clamped=true;
        let factor=(1.0/(length_squared as f64).sqrt())*cap as f64;
        o.impulse=o.impulse.map(|v|(v as f64*factor) as f32);
    }
    o.applied=true;
    for side in 0..2 {
        let body=&mut bodies[side]; if !body.active {continue;}
        let sign=if side==0 {1.}else{-1.};
        let s0=(o.impulse[0]*body.inverse_mass)*sign;
        let s1=(o.impulse[1]*body.inverse_mass)*sign;
        body.linear+=p.basis[1]*s1+p.basis[0]*s0;
        body.angular+=o.torque[side][1]*(o.impulse[1]*sign)+o.torque[side][0]*(o.impulse[0]*sign);
    }
    let mix_squared=(p.mix[0] as f64*p.mix[0] as f64)+(p.mix[1] as f64*p.mix[1] as f64);
    let scaled=p.dt as f64*(p.dt as f64*length_squared as f64);
    let new_work=(mix_squared*scaled).sqrt()*0.5;
    o.work_delta=new_work-*work as f64;
    *work=new_work as f32;
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn fixture()->Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-friction.json")).unwrap()}
    fn f(v:&Value)->f32 {v.as_f64().unwrap() as f32}
    fn v(v:&Value)->Vec3 {Vec3::from_array(std::array::from_fn(|i|f(&v[i])))}
    fn parse(c:&Value)->([Body;2],Parameters) {
        let bodies=std::array::from_fn(|side| {let b=&c["before"][side];Body {
            active:c["active"][side].as_bool().unwrap(),inverse_mass:f(&b["inverse_mass"]),inverse_inertia:v(&b["inverse_inertia"]),
            rotation:Mat3::from_cols_array(&std::array::from_fn(|i|f(&b["rotation_rows"][i]))).transpose(),
            position:v(&b["position"]),linear:v(&b["linear"]),angular:v(&b["angular"]),
        }});
        (bodies,Parameters {point:v(&c["point"]),basis:std::array::from_fn(|i|v(&c["basis"][i])),mix:std::array::from_fn(|i|f(&c["mix"][i])),
            dt:f(&c["dt"]),inverse_dt:f(&c["inverse_dt"]),normal_force:f(&c["normal_force"]),friction:f(&c["friction"]),
            minimum_impulse:f(&c["constants"]["minimum_impulse"]),minimum_determinant_squared:f(&c["constants"]["minimum_determinant_squared"]),alternate_candidate:c["alternate_candidate"].as_bool().unwrap()})
    }
    #[test]
    fn ordinary_response_derives_native_matrix_speeds_and_core_derivatives() {
        let data=fixture();let mut initialized=0;
        for (i,c) in data["cases"].as_array().unwrap().iter().enumerate() {
            let (mut bodies,p)=parse(c);let mut work=f(&c["work_before"]);let o=ordinary(&mut bodies,&p,&mut work).unwrap();let n=&c["ordinary"];
            assert_eq!(o.initialized,n["complete"].as_bool().unwrap(),"case{i}");
            if !o.initialized {continue;} initialized+=1;
            assert_eq!(o.matrix,std::array::from_fn(|j|f(&n["matrix"][j])),"matrix case{i}");
            assert_eq!(o.speed,std::array::from_fn(|j|f(&n["speed"][j])),"speed case{i}");
            for side in 0..2 {for axis in 0..2 {
                assert_eq!(o.jacobian[side][axis],v(&n["jacobian"][side][axis]),"jacobian case{i}");
                assert_eq!(o.torque[side][axis],v(&n["torque"][side][axis]),"torque case{i}");
            }}
        }
        assert_eq!(initialized,981);
    }
    #[test]
    fn ordinary_impulses_raw_velocities_and_work_match_native_branches() {
        let data=fixture();let mut counts=[0;5];
        for (i,c) in data["cases"].as_array().unwrap().iter().enumerate() {
            let (mut bodies,p)=parse(c);let mut work=f(&c["work_before"]);let o=ordinary(&mut bodies,&p,&mut work).unwrap();let n=&c["ordinary"];
            assert_eq!(o.applied,n["applied"].as_bool().unwrap(),"applied case{i}");
            assert_eq!(o.impulse,std::array::from_fn(|j|f(&n["impulse"][j])),"impulse case{i}");
            assert_eq!(work,f(&c["work_after"]),"work case{i}");
            for side in 0..2 {
                assert_eq!(bodies[side].linear,v(&c["after"][side]["linear"]),"linear case{i}");
                assert_eq!(bodies[side].angular,v(&c["after"][side]["angular"]),"angular case{i}");
            }
            counts[0]+=(!o.initialized) as usize;counts[1]+=(o.initialized&&!o.applied) as usize;
            counts[2]+=o.applied as usize;counts[3]+=o.clamped as usize;counts[4]+=(o.applied&&!o.clamped) as usize;
        }
        assert_eq!(counts,[386,24,957,524,433]);
    }
    #[test]
    fn controlled_friction_preserves_native_unspilled_double_work_return() {
        let data=fixture();let mut count=0;
        for c in data["cases"].as_array().unwrap() {
            let Some(expected)=c.get("return_value") else {continue;};
            let (mut bodies,p)=parse(c);let mut work=f(&c["work_before"]);let o=ordinary(&mut bodies,&p,&mut work).unwrap();
            assert_eq!(o.work_delta,expected.as_f64().unwrap(),"step{}",c["step"]);count+=1;
        }
        assert_eq!(count,96);
    }
    #[test]
    fn alternate_controller_is_explicitly_unsupported_without_mutation() {
        let data=fixture();let c=&data["cases"][0];let (mut bodies,mut p)=parse(c);let before=bodies;
        p.alternate_candidate=true;let mut work=1.25;
        assert!(ordinary(&mut bodies,&p,&mut work).is_err());assert_eq!(work,1.25);
        for side in 0..2 {assert_eq!(bodies[side].linear,before[side].linear);assert_eq!(bodies[side].angular,before[side].angular);}
    }
    #[test]
    fn native_caller_slip_to_ordinary_response_composes_without_solved_inputs() {
        use crate::ivp_slip::{self as slip,Coefficients,State};
        let friction=fixture();let caller:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-slip.json")).unwrap();
        let rows=friction["cases"].as_array().unwrap();let mut count=0;
        for c in caller["cases"].as_array().unwrap().iter().filter(|c|c["step"].as_i64().unwrap()>=0) {
            // Independent observations have the same native call chronology.
            let n=rows.iter().find(|n|n["capture"]==c["capture"]&&n["model"]==c["model"]&&n["sequence"]==c["sequence"]).unwrap();
            assert_eq!(n["step"],c["step"]);
            let group=c["group"].as_array().unwrap().iter().map(|v|Coefficients {force:f(&v[0]),friction:f(&v[1]),inverse_response:f(&v[2])}).collect::<Vec<_>>();
            let dt=f(&c["dt"]);let limit=if c["batch"].as_bool().unwrap(){slip::group_limit(&group,dt) as f32}else{slip::single_limit(group[0],dt)};
            let before=&c["before"];let mut state=State {mix:[f(&before["mix"][0]),f(&before["mix"][1])],energy:f(&before["energy"]),marker:before["marker"].as_u64().unwrap() as u8};
            slip::trim(&mut state,limit,f(&c["friction"]),f(&c["force"]),f(&c["threshold_extra"]));
            let (mut bodies,mut p)=parse(n);p.mix=state.mix;
            assert_eq!(p.mix,std::array::from_fn(|j|f(&n["mix"][j])));
            let mut work=f(&n["work_before"]);let o=ordinary(&mut bodies,&p,&mut work).unwrap();
            assert_eq!(work,f(&n["work_after"]));assert_eq!(o.impulse,std::array::from_fn(|j|f(&n["ordinary"]["impulse"][j])));
            for side in 0..2 {assert_eq!(bodies[side].linear,v(&n["after"][side]["linear"]));assert_eq!(bodies[side].angular,v(&n["after"][side]["angular"]));}
            count+=1;
        }
        assert_eq!(count,1271);
    }
    #[test]
    fn caller_and_friction_returns_derive_native_group_work_updates() {
        use crate::ivp_slip::{self as slip,Coefficients,State};
        use crate::ivp_redistribute::{add_delta,accumulate_work};
        let friction=fixture();let caller:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-slip.json")).unwrap();
        let groups:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-redistribute.json")).unwrap();
        let rows=friction["cases"].as_array().unwrap();let calls=caller["cases"].as_array().unwrap();let mut count=0;let mut responses=0;
        for g in groups["work"].as_array().unwrap() {
            let mut total=0.;
            for point in g["points"].as_array().unwrap().iter().rev() {
                let matches=rows.iter().filter(|n|n["capture"]==g["capture"]&&n["model"]==g["model"]&&n["step"]==g["step"]&&v(&n["point"])==v(point)).collect::<Vec<_>>();
                assert_eq!(matches.len(),1,"group{} step{} point{point}",g["sequence"],g["step"]);let n=matches[0];
                let c=calls.iter().find(|c|c["capture"]==n["capture"]&&c["model"]==n["model"]&&c["sequence"]==n["sequence"]).unwrap();
                let coefficients=c["group"].as_array().unwrap().iter().map(|v|Coefficients {force:f(&v[0]),friction:f(&v[1]),inverse_response:f(&v[2])}).collect::<Vec<_>>();
                let dt=f(&c["dt"]);let limit=if c["batch"].as_bool().unwrap(){slip::group_limit(&coefficients,dt) as f32}else{slip::single_limit(coefficients[0],dt)};
                let b=&c["before"];let mut state=State {mix:[f(&b["mix"][0]),f(&b["mix"][1])],energy:f(&b["energy"]),marker:b["marker"].as_u64().unwrap() as u8};
                slip::trim(&mut state,limit,f(&c["friction"]),f(&c["force"]),f(&c["threshold_extra"]));
                let (mut bodies,mut p)=parse(n);p.mix=state.mix;let mut work=f(&n["work_before"]);
                let response=ordinary(&mut bodies,&p,&mut work).unwrap();total=add_delta(total,response.work_delta);responses+=1;
            }
            assert_eq!(total,f(&g["delta"]),"group sum{} step{}",g["sequence"],g["step"]);
            let mut energy=f(&g["before"]);accumulate_work(&mut energy,total,f(&g["threshold"]));assert_eq!(energy,f(&g["after"]));count+=1;
        }
        assert_eq!((count,responses),(368,1231));
    }

    #[test]
    fn authored_owned_material_pair_drives_all_real_ordinary_friction_responses(){
        // native_ivp_phy_motion.h authors both owned surfaces with friction0.8.
        // These real ordinary contacts have no controller override. Material
        // lookup and directed/alternate-controller responses are not implied.
        let data=fixture();let mut count=0;
        for c in data["cases"].as_array().unwrap().iter().filter(|c|c["step"].as_i64().unwrap()>=0){
            let (mut bodies,mut p)=parse(c);
            p.friction=crate::ivp_material::clamped_product(0.8,0.8) as f32;
            assert_eq!(p.friction.to_bits(),f(&c["friction"]).to_bits());
            let mut work=f(&c["work_before"]);let out=ordinary(&mut bodies,&p,&mut work).unwrap();
            assert_eq!(out.impulse,std::array::from_fn(|i|f(&c["ordinary"]["impulse"][i])));assert_eq!(work,f(&c["work_after"]));
            assert_eq!(out.applied,c["ordinary"]["applied"].as_bool().unwrap());
            for side in 0..2 {assert_eq!(bodies[side].linear,v(&c["after"][side]["linear"]));assert_eq!(bodies[side].angular,v(&c["after"][side]["angular"]));}
            count+=1;
        }
        assert_eq!(count,1271);
    }
}
