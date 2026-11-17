//! Owned100fc080/100fc190 and100ff6f0 lone-contact normal response.
//! Unlike the batch path, this changes raw velocity immediately. Geometry,
//! contact creation and deletion side effects remain caller responsibilities.
use crate::ivp_contact::{Core,Row};
use crate::ivp_history::History;
use bevy::prelude::Vec3;

/// The single path consumes cached axes, not the batch row's target-gap field.
#[derive(Clone,Copy)]
pub(crate) struct Axes {pub bodies:[Option<usize>;2],pub normal:Vec3,pub jacobian:[Vec3;2]}
impl From<Row> for Axes {fn from(row:Row)->Self{Self{bodies:row.bodies,normal:row.normal,jacobian:row.jacobian}}}

pub(crate) fn speed(cores:&[Core],row:Row)->f32 {
    speed_axes(cores,row.into())
}
fn speed_axes(cores:&[Core],row:Axes)->f32 {
    let mut value=0.;
    for side in 0..2 {
        let Some(i)=row.bodies[side] else {continue;};let b=cores[i];let j=row.jacobian[side];let n=row.normal;
        let projected=((j.y*b.angular.y+j.x*b.angular.x)+j.z*b.angular.z)+((n.y*b.linear.y+n.x*b.linear.x)+n.z*b.linear.z);
        if side==0 {value=projected;}else{value-=projected;}
    }
    value
}

///100fc190 has the same per-axis multiplication groups as1011c160, but
/// writes raw rather than pending velocities. Pending impulses stay intact.
pub(crate) fn push(cores:&mut [Core],row:Row,impulse:f32) {
    push_axes(cores,row.into(),impulse)
}
fn push_axes(cores:&mut [Core],row:Axes,impulse:f32) {
    for side in 0..2 {
        let Some(i)=row.bodies[side] else {continue;};let b=&mut cores[i];let j=row.jacobian[side];let inv=b.inverse_inertia;
        if side==0 {
            let negative=-impulse;
            b.angular.x=negative*(j.x*inv.x)+b.angular.x;
            b.angular.y=(j.y*inv.y)*negative+b.angular.y;
            b.angular.z=(j.z*inv.z)*negative+b.angular.z;
            b.linear+=row.normal*(-(b.inverse_mass*impulse));
        }else{
            b.angular.x=(j.x*inv.x)*impulse+b.angular.x;
            b.angular.y=(j.y*inv.y)*impulse+b.angular.y;
            b.angular.z=(j.z*inv.z)*impulse+b.angular.z;
            b.linear+=row.normal*(b.inverse_mass*impulse);
        }
    }
}

/// Derive the physical row+68 reciprocal response from geometry and core
/// inverse mass/inertia. No observed solved impulse or response seeds this.
pub(crate) fn inverse_response(cores:&[Core],row:Row)->Option<f32> {
    let diagonal=crate::ivp_contact::assemble(cores,&[row],0.,1.)?.matrix[0];
    (diagonal>0.).then(||1./diagonal)
}

pub(crate) struct Response {pub impulse:Option<f32>,pub retire:bool}
pub(crate) fn respond(cores:&mut [Core],row:Row,history:&mut History,distance:f32,inverse_dt:f32,status:i16)->Option<Response> {
    let inverse=inverse_response(cores,row)?;
    respond_cached(cores,row.into(),inverse,history,distance,inverse_dt,status)
}
///100ff6f0 uses the reciprocal response already computed during refresh.
pub(crate) fn respond_cached(cores:&mut [Core],axes:Axes,inverse:f32,history:&mut History,distance:f32,inverse_dt:f32,status:i16)->Option<Response>{
    if !inverse.is_finite()||inverse<=0.||!distance.is_finite()||!inverse_dt.is_finite()||inverse_dt<=0.
        ||!axes.normal.is_finite()||axes.jacobian.iter().any(|j|!j.is_finite())
        ||axes.bodies.iter().flatten().any(|&i|i>=cores.len())
        ||axes.bodies[0].is_some()&&axes.bodies[0]==axes.bodies[1]{return None;}
    let gap=0.0126949195f32-distance;let factor=if gap<0. {20.}else{1.};
    let candidate=(factor*gap+speed_axes(cores,axes))*inverse;
    let impulse=if candidate>0. {history.force=inverse_dt*candidate;push_axes(cores,axes,candidate);Some(candidate)}else{history.force=0.;None};
    Some(Response {impulse,retire:distance>=0.0285635684f32||status==1})
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    fn fixture()->serde_json::Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-single.json")).unwrap()}
    fn f(v:&serde_json::Value)->f32 {v.as_f64().unwrap() as f32}
    fn v(a:&serde_json::Value)->Vec3 {Vec3::from_array(std::array::from_fn(|i|f(&a[i])))}
    fn core(c:&serde_json::Value)->Core {Core {inverse_mass:f(&c["inverse_mass"]),inverse_inertia:v(&c["inverse_inertia"]),linear:v(&c["linear"]),angular:v(&c["angular"]),pending_linear:v(&c["pending_linear"]),pending_angular:v(&c["pending_angular"])}}
    fn parse(value:&serde_json::Value)->([Core;2],Row) {
        let mut cores=[Core::default();2];let bodies=std::array::from_fn(|side|{let c=&value["bodies"][side];let key=c["key"].as_u64().unwrap() as usize;if key==0 {None}else{cores[key-1]=core(c);Some(key-1)}});
        (cores,Row {bodies,normal:v(&value["normal"]),jacobian:std::array::from_fn(|side|v(&value["jacobian"][side])),gap:f(&value["gap"])})
    }
    fn compare(actual:&[Core],expected:&serde_json::Value) {
        for c in expected["bodies"].as_array().unwrap() {let key=c["key"].as_u64().unwrap() as usize;if key==0 {continue;}let a=actual[key-1];let e=core(c);
            assert_eq!(a.linear,e.linear);assert_eq!(a.angular,e.angular);assert_eq!(a.pending_linear,e.pending_linear);assert_eq!(a.pending_angular,e.pending_angular);
        }
    }
    #[test]
    fn native_raw_contact_speed_matches_controlled_directions_and_core_pairs() {
        let data=fixture();let mut count=0;
        for c in data["controls"].as_array().unwrap() {let (cores,row)=parse(&c["before"]);assert_eq!(speed(&cores,row),f(&c["speed"]),"kind{}",c["kind"]);count+=1;}
        assert_eq!(count,64);println!("{count} exact native raw contact speed controls");
    }
    #[test]
    fn native_raw_impulses_preserve_pending_state() {
        let data=fixture();let mut count=0;
        for c in data["pushes"].as_array().unwrap() {let (mut cores,row)=parse(&c["before"]);push(&mut cores,row,f(&c["impulse"]));compare(&cores,&c["after"]);count+=1;}
        assert_eq!(count,87);println!("{count} exact native raw velocity impulses");
    }
    #[test]
    fn native_single_effective_response_is_derived_from_geometry() {
        let data=fixture();let mut count=0;
        for c in data["singles"].as_array().unwrap() {let (cores,row)=parse(&c["before"]);assert_eq!(inverse_response(&cores,row).unwrap(),f(&c["inverse_response"]));count+=1;}
        assert_eq!(count,40);println!("{count} exact native reciprocal responses derived from geometry/core properties");
    }
    #[test]
    fn complete_lone_contact_response_and_retirement_match() {
        let data=fixture();let mut count=0;let mut emitted=0;let mut retired=0;
        for c in data["singles"].as_array().unwrap() {
            let (mut cores,row)=parse(&c["before"]);let mut h=History {force:f(&c["force_before"]),counter:c["counter"].as_i64().unwrap() as i16};
            let response=respond(&mut cores,row,&mut h,f(&c["gap"]),f(&c["inverse_dt"]),c["status"].as_i64().unwrap() as i16).unwrap();
            assert_eq!(response.retire,c["retired"].as_bool().unwrap());retired+=response.retire as usize;
            if !response.retire {assert_eq!(h.force,f(&c["force_after"]));assert_eq!(h.counter,c["counter_after"].as_i64().unwrap() as i16);}
            let native=data["pushes"].as_array().unwrap().iter().filter(|p|p["capture"]==c["capture"] && p["model"]==c["model"] && p["single_index"]==c["sequence"] && p["step"]==c["step"]).collect::<Vec<_>>();
            match response.impulse {Some(value)=>{assert_eq!(native.len(),1);assert_eq!(value,f(&native[0]["impulse"]));emitted+=1;},None=>assert!(native.is_empty())}
            compare(&cores,&c["after"]);count+=1;
        }
        assert_eq!((count,emitted,retired),(40,23,7));println!("{count} exact complete lone-contact responses,{emitted} pushes,{retired} observed separation retirements; deletion side effects remain separate");
    }
}
