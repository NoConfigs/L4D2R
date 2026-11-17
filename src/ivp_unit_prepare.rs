//! Represented core preparation from100eb7d0 before poll/controller execution.
//! Object caches, impact counts, contacts and subsequent prediction are separate.
use crate::{ivp_joint_apply::Body,ivp_motion,ivp_unit::{UnitFlags,Classification}};
use bevy::prelude::Vec3;
use std::collections::HashSet;

/// Native unit core order is reversed. The candidate owns the complete update,
/// including pending-impulse commit and speed-transition clock resets.
pub(crate) fn prepare(bodies:&mut [Body],flags:&mut UnitFlags,now:f64)->Result<Classification,&'static str>{
    if bodies.is_empty()||!now.is_finite(){return Err("invalid unit preparation context");}
    let mut ids=HashSet::new();
    for b in bodies.iter(){
        if !ids.insert(b.id)||b.flags&2!=0||b.flags>>8>=8||!b.prediction.time.is_finite()
            ||!b.prediction.start.is_finite()||!b.prediction.velocity.is_finite()
            ||!b.prediction.rotation_end.iter().all(|v|v.is_finite())
            ||[b.linear,b.angular,b.pending_linear,b.pending_angular].iter().any(|v|v[..3].iter().any(|x|!x.is_finite())){
            return Err("invalid unit preparation body");
        }
    }
    let mut candidate=bodies.to_vec();let mut next_flags=*flags;let mut linear=Vec::with_capacity(candidate.len());
    for b in candidate.iter_mut().rev(){
        let elapsed=(now-b.prediction.time) as f32;
        b.current.rotation=ivp_motion::matrix(b.prediction.rotation_end);
        //100eb873/100eb95f retain the X multiply/add in x87, AFTER
        //elapsed spills to float. Y/Z use separate SSE float operations.
        b.current.position=Vec3::new(
            ((elapsed as f64)*(b.prediction.velocity.x as f64)+b.prediction.start.x as f64) as f32,
            b.prediction.velocity.y*elapsed+b.prediction.start.y,
            b.prediction.velocity.z*elapsed+b.prediction.start.z);
        //100f13b0. Fourth lanes are opaque padding and remain untouched.
        b.angular[0]=b.angular[0]+b.pending_angular[0];
        b.angular[1]=b.angular[1]+b.pending_angular[1];
        b.angular[2]=b.pending_angular[2]+b.angular[2];
        b.linear[0]=b.linear[0]+b.pending_linear[0];
        b.linear[1]=b.pending_linear[1]+b.linear[1];
        b.linear[2]=b.linear[2]+b.pending_linear[2];
        b.pending_angular[..3].fill(0.);b.pending_linear[..3].fill(0.);
        b.flags&=0xff3f;b.clock=now;
        if !b.current.position.is_finite()||!b.current.rotation.is_finite()||b.linear[..3].iter().chain(&b.angular[..3]).any(|v|!v.is_finite()){
            return Err("unit preparation overflow");
        }
        linear.push(Vec3::new(b.linear[0],b.linear[1],b.linear[2]));
    }
    let result=next_flags.classify(&linear,1.)?;
    if result.reset_movement_clock{for b in &mut candidate{b.stamps=[now;2];}}
    bodies.copy_from_slice(&candidate);*flags=next_flags;Ok(result)
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::ivp_motion::{Pose,Prediction};
    use bevy::prelude::Mat3;
    use serde_json::{Value,json};
    fn fixture()->Value{serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-unit-prepare.json")).unwrap())).unwrap()}
    fn f(v:&Value)->f32{f32::from_bits(v.as_u64().unwrap() as u32)}
    fn d(v:&Value)->f64{f64::from_bits(v.as_u64().unwrap())}
    fn seed(c:&Value)->Body{
        let b=&c["before"];
        Body{id:0,flags:b["flags"].as_u64().unwrap() as u16,object_flags:0x1001,mass:10.,inverse:[1.;4],
            current:Pose{position:Vec3::ZERO,rotation:Mat3::IDENTITY},
            prediction:Prediction{start:Vec3::from_array(std::array::from_fn(|i|f(&c["start"][i]))),velocity:Vec3::from_array(std::array::from_fn(|i|f(&c["velocity"][i]))),rotation_start:[0.,0.,0.,1.],rotation_end:std::array::from_fn(|i|f(&c["q1"][i])),time:d(&c["cache_time"]),inverse_dt:60.},
            clock:d(&c["now"]),stamps:std::array::from_fn(|i|d(&b["stamps"][i])),
            pending_angular:std::array::from_fn(|i|f(&b["pending_angular"][i])),pending_linear:std::array::from_fn(|i|f(&b["pending_linear"][i])),
            angular:std::array::from_fn(|i|f(&b["angular"][i])),linear:std::array::from_fn(|i|f(&b["linear"][i]))}
    }
    fn projection(b:Body,flags:UnitFlags)->Value{json!({"flags":b.flags,"unit_flags":flags.flags,"position":b.current.position.to_array().map(f32::to_bits),"rotation":b.current.rotation.transpose().to_cols_array().map(f32::to_bits),"linear":b.linear.map(f32::to_bits),"angular":b.angular.map(f32::to_bits),"pending_linear":b.pending_linear.map(f32::to_bits),"pending_angular":b.pending_angular.map(f32::to_bits),"stamps":b.stamps.map(f64::to_bits)})}
    #[test]
    fn native_unit_pre_controller_boundaries_match_represented_core_bits(){
        let data=fixture();let(mut count,mut fast,mut resets)=(0,0,0);
        for c in data["cases"].as_array().unwrap(){
            let initial=seed(c);let mut bodies=[initial];let mut flags=UnitFlags{flags:c["before"]["unit_flags"].as_u64().unwrap() as u32};
            let result=prepare(&mut bodies,&mut flags,d(&c["now"])).unwrap();fast+=usize::from(result.fast);resets+=usize::from(result.reset_movement_clock);
            assert_eq!(projection(bodies[0],flags),c["prepared"],"case{count}");
            let mut unchanged=crate::ivp_joint_apply::tests::snapshot(initial);let mut after=crate::ivp_joint_apply::tests::snapshot(bodies[0]);
            for key in ["flags","position","rotation","linear","angular","pending_linear","pending_angular","clock","stamps"]{unchanged.as_object_mut().unwrap().remove(key);after.as_object_mut().unwrap().remove(key);}
            assert_eq!(after,unchanged);count+=1;
        }
        assert_eq!(count,288);assert!(fast>0&&fast<count&&resets>0);
    }
    #[test]
    fn preparation_rejects_invalid_later_body_without_partial_commit(){
        let c=&fixture()["cases"][0];let first=seed(c);let mut second=first;second.id=1;second.pending_linear[2]=f32::NAN;
        let mut bodies=[first,second];let mut flags=UnitFlags{flags:0x1401};let before=format!("{bodies:?}/{flags:?}");
        assert!(prepare(&mut bodies,&mut flags,d(&c["now"])).is_err());assert_eq!(format!("{bodies:?}/{flags:?}"),before);
        bodies[1]=first;assert!(prepare(&mut bodies,&mut flags,d(&c["now"])).is_err());
    }
}
