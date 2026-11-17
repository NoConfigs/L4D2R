//!100eabe0 default-controller execution over its owned member vector.
//! Damping/queued-impulse math stays in the independently recovered kernel.
use bevy::prelude::Vec3;
use std::collections::BTreeMap;
#[derive(Clone,Copy)]
pub(crate) struct Body {
    pub core:crate::ivp_contact::Core,
    pub flags:u8,
    pub movement:u8,
    pub linear_rate:f32,
    pub angular_rate:Vec3,
}
pub(crate) struct Controller{pub gravity:Vec3,pub alternate:Vec3}
impl Controller {
    pub(crate) fn execute(&self,bodies:&mut BTreeMap<usize,Body>,members:&[usize],dt:f32)->Result<(),&'static str>{
        // A broken owner binding must not partially commit a valid prefix.
        if members.iter().any(|id|!bodies.contains_key(id)){return Err("controller member lacks a force body");}
        for id in members.iter().rev(){
            let body=bodies.get_mut(id).unwrap();
            // Native skip preserves pending impulses too. Alternate gravity
            // selection happens only after damping and pending-work commit.
            if body.flags&0x10!=0{continue;}
            let gravity=if body.flags&0x20!=0{self.alternate}else{self.gravity};
            crate::ivp_damping::prepare_forces(&mut body.core,dt,body.linear_rate,body.angular_rate,body.movement,gravity)?;
        }Ok(())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn native_default_controller_batches_match_skip_alternate_gravity_and_damping_bits(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-base-controller.json")).unwrap();
        let f=|v:&serde_json::Value|f32::from_bits(v.as_u64().unwrap() as u32);
        let v=|p:&serde_json::Value|Vec3::new(f(&p[0]),f(&p[1]),f(&p[2]));
        let mut cases=0;let mut skipped=0;let mut alternate=0;let mut executed=0;
        for (index,case) in data["cases"].as_array().unwrap().iter().enumerate(){
            let mut bodies=case["before"].as_array().unwrap().iter().map(|p|{
                (p["id"].as_u64().unwrap() as usize,Body{core:crate::ivp_contact::Core{linear:v(&p["linear"]),angular:v(&p["angular"]),pending_linear:v(&p["pending_linear"]),pending_angular:v(&p["pending_angular"]),..Default::default()},flags:p["flags"].as_u64().unwrap() as u8,movement:p["movement"].as_u64().unwrap() as u8,linear_rate:f(&p["linear_rate"]),angular_rate:v(&p["angular_rate"])})
            }).collect::<BTreeMap<_,_>>();
            let members=case["members"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();
            for id in &members{if bodies[id].flags&0x10!=0{skipped+=1;}else{executed+=1;if bodies[id].flags&0x20!=0{alternate+=1;}}}
            let controller=Controller{gravity:v(&case["gravity"]),alternate:v(&case["alternate"])};
            controller.execute(&mut bodies,&members,f(&case["dt_bits"])).unwrap();
            for p in case["after"].as_array().unwrap(){
                let id=p["id"].as_u64().unwrap() as usize;let body=bodies[&id];
                assert_eq!((body.flags,body.movement),(p["flags"].as_u64().unwrap() as u8,p["movement"].as_u64().unwrap() as u8));
                assert_eq!(body.linear_rate.to_bits(),p["linear_rate"].as_u64().unwrap() as u32);assert_eq!(body.angular_rate.to_array().map(f32::to_bits),v(&p["angular_rate"]).to_array().map(f32::to_bits));
                for (actual,key) in [(body.core.linear,"linear"),(body.core.angular,"angular"),(body.core.pending_linear,"pending_linear"),(body.core.pending_angular,"pending_angular")]{
                    assert_eq!(actual.to_array().map(f32::to_bits),v(&p[key]).to_array().map(f32::to_bits),"case{index} core{id} {key}");
                }
            }cases+=1;
        }
        assert_eq!(cases,128);assert!(skipped>0&&alternate>0&&executed>0);
    }
}
