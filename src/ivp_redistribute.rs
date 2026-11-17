//! Native100ff020 countdown,100feaf0 redistribution and100fcc30 pair adjustment.
use bevy::prelude::*;
#[derive(Clone,Debug,PartialEq)]
pub(crate) struct Contact {pub core_key:u64,pub point:Vec3,pub normal:Vec3,pub basis:[Vec3;2],pub mix:[f32;2]}
#[derive(Clone,Debug,PartialEq)]
pub(crate) struct Group {pub timer:i32,pub energy:f32,pub contacts:Vec<Contact>}
pub(crate) struct Settings {pub one:f32,pub opposite:f32,pub half:f32,pub epsilon:f32,pub normal_tolerance:f32}
#[derive(Default,Debug)]
pub(crate) struct Stats {pub pairs:usize,pub skipped:usize,pub unnormalized:usize}
fn dot(a:Vec3,b:Vec3)->f32 {(a.y*b.y+a.x*b.x)+a.z*b.z}

pub(crate) fn redistribute(contacts:&mut [Contact],s:&Settings)->Stats {
    let mut stats=Stats::default();let mut delta=vec![Vec3::ZERO;contacts.len()];
    let factor=s.one/(contacts.len() as f32+s.epsilon);
    for i in 0..contacts.len() {
        for j in i+1..contacts.len() {
            let a=&contacts[j];let b=&contacts[i];
            // The caller's eligibility test uses x87 double, unlike the pair's
            // projected mix and final writes, which use SSE single precision.
            let alignment=(a.normal.y as f64*b.normal.y as f64+a.normal.x as f64*b.normal.x as f64)+a.normal.z as f64*b.normal.z as f64;
            if (alignment.abs()-s.one as f64).abs()>=s.normal_tolerance as f64 {stats.skipped+=1;continue;}
            stats.pairs+=1;
            let mut direction=a.point-b.point;
            let squared=(direction.x*direction.x+direction.y*direction.y)+direction.z*direction.z;
            if squared>=s.epsilon {
                let inverse=1.0/(squared as f64).sqrt();
                direction=Vec3::new((direction.x as f64*inverse) as f32,(direction.y as f64*inverse) as f32,(direction.z as f64*inverse) as f32);
            }else{stats.unnormalized+=1;}
            let sign=if a.core_key==b.core_key {s.one}else{s.opposite};
            let va=a.basis[1]*a.mix[1]+a.basis[0]*a.mix[0];
            let vb=b.basis[1]*b.mix[1]+b.basis[0]*b.mix[0];
            let pa=direction*dot(direction,va);
            let pb=direction*(dot(direction,vb)*sign);
            let average=(pb+pa)*s.half;
            delta[j]+=(average-pa)*factor;
            delta[i]+=(average-pb)*(factor*sign);
        }
    }
    // All pairs see the old mix. Writes happen only after the pair loop.
    for (contact,change) in contacts.iter_mut().zip(delta) {
        for axis in 0..2 {contact.mix[axis]=dot(contact.basis[axis],change)+contact.mix[axis];}
    }
    stats
}
pub(crate) fn tick(groups:&mut [Group],s:&Settings)->Vec<Option<Stats>> {
    let mut output=(0..groups.len()).map(|_|None).collect::<Vec<_>>();
    for i in (0..groups.len()).rev() {
        let g=&mut groups[i];g.timer=g.timer.wrapping_sub(1);
        if g.timer==0 {output[i]=Some(redistribute(&mut g.contacts,s));g.timer=5;}
    }
    output
}
///100fe9b0 spills after each ordinary double return, then only positive net
///work is added to the group's single-precision accumulator.
pub(crate) fn add_delta(total:f32,delta:f64)->f32 {(total as f64+delta) as f32}
pub(crate) fn accumulate_work(energy:&mut f32,delta:f32,threshold:f32)->bool {
    if delta>threshold {*energy+=delta;true}else{false}
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn fixture()->Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-redistribute.json")).unwrap()}
    fn f(v:&Value)->f32 {v.as_f64().unwrap() as f32}
    fn v(v:&Value)->Vec3 {Vec3::from_array(std::array::from_fn(|i|f(&v[i])))}
    fn groups(vv:&Value)->Vec<Group> {vv.as_array().unwrap().iter().map(|g|Group {timer:g["timer"].as_i64().unwrap() as i32,energy:f(&g["energy"]),contacts:g["rows"].as_array().unwrap().iter().map(|c|Contact {core_key:c["core_key"].as_u64().unwrap(),point:v(&c["point"]),normal:v(&c["normal"]),basis:std::array::from_fn(|i|v(&c["basis"][i])),mix:std::array::from_fn(|i|f(&c["mix"][i]))}).collect()}).collect()}
    fn settings(c:&Value)->Settings {let k=&c["constants"];Settings {one:f(&k["one"]),opposite:f(&k["opposite"]),half:f(&k["half"]),epsilon:f(&k["epsilon"]),normal_tolerance:f(&k["normal_tolerance"])}}
    #[test]
    fn native_countdown_and_coupled_slip_redistribution_match() {
        let data=fixture();let mut counts=[0;5];
        for (i,c) in data["systems"].as_array().unwrap().iter().enumerate() {
            let mut g=groups(&c["before"]);let stats=tick(&mut g,&settings(c));
            assert_eq!(g,groups(&c["after"]),"system{i}");counts[0]+=1;
            for s in stats.into_iter().flatten(){counts[1]+=1;counts[2]+=s.pairs;counts[3]+=s.unnormalized;counts[4]+=s.skipped;}
        }
        assert_eq!(counts,[480,106,460,18,110]);
    }
    #[test]
    fn repeated_control_calls_reuse_source_computed_state() {
        let data=fixture();let mut prior=None;let mut reused=0;
        for c in data["systems"].as_array().unwrap().iter().filter(|c|c.get("kind").is_some()) {
            let mut g=if c["repeat"].as_u64().unwrap()==0 {groups(&c["before"])}else{reused+=1;prior.take().unwrap()};
            assert_eq!(g,groups(&c["before"]));tick(&mut g,&settings(c));assert_eq!(g,groups(&c["after"]));prior=Some(g);
        }
        assert_eq!(reused,48);
    }
    #[test]
    fn positive_only_group_work_accumulation_matches() {
        let data=fixture();let mut counts=[0;3];
        for c in data["work"].as_array().unwrap() {
            let delta=f(&c["delta"]);let mut energy=f(&c["before"]);
            let changed=accumulate_work(&mut energy,delta,f(&c["threshold"]));assert_eq!(energy,f(&c["after"]));assert_eq!(changed,delta>0.);
            counts[if delta>0. {0}else if delta<0. {1}else{2}]+=1;
        }
        assert_eq!(counts,[118,217,33]);
    }
}
