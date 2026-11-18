//! Owned100f0850 near/far pose histories and100f6560 unit-poll arithmetic.
//! Core/world activation, clock-reset providers and sleep dispatch are callers.
use bevy::prelude::*;
#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct History {pub close_time:f64,pub far_time:f64,pub close_position:Vec3,pub far_position:Vec3,pub close_quat:[f32;4],pub far_quat:[f32;4]}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Input {pub radius:f32,pub sleep_time:f32,pub angular:Vec3,pub position:Vec3,pub start:[f32;4],pub end:[f32;4]}
pub(crate) struct Settings {pub translation_squared:f32,pub rotation_squared:f32,pub far_translation_squared:f32,pub far_rotation_squared:f32,pub far_timeout:f32,pub one:f32,pub two:f32,pub angular_factor0:f32,pub angular_factor1:f32}
fn square(v:Vec3)->f32{(v.y*v.y+v.x*v.x)+v.z*v.z}
fn dot(a:[f32;4],b:[f32;4])->f32{((a[1]*b[1]+a[0]*b[0])+a[2]*b[2])+a[3]*b[3]}
fn rotation_metric(a:[f32;4],b:[f32;4],radius:f32,k:&Settings)->f32{let d=dot(a,b);(((k.one-d*d)*k.two)*radius)*radius}
impl History {
    ///100f0460 only resets movement clocks; it does not invalidate geometry.
    pub(crate) fn reset_clock(&mut self,now:f64)->Result<(),&'static str>{if !now.is_finite(){return Err("invalid movement reset clock");}self.close_time=now;self.far_time=now;Ok(())}
}
#[cfg(test)]
thread_local!{pub(crate) static AUDIT:std::cell::RefCell<Option<Vec<serde_json::Value>>>=const{std::cell::RefCell::new(None)};}
pub(crate) fn classify(h:&mut History,i:Input,now:f64,k:&Settings)->Result<u8,&'static str>{
    #[cfg(test)] let before=*h;
    let result=classify_inner(h,i,now,k);
    #[cfg(test)] AUDIT.with(|a|{if let Some(rows)=a.borrow_mut().as_mut(){
        let history=|h:History|serde_json::json!({"close_time":h.close_time.to_bits(),"far_time":h.far_time.to_bits(),"close_position":h.close_position.to_array().map(f32::to_bits),"far_position":h.far_position.to_array().map(f32::to_bits),"close_quat":h.close_quat.map(f32::to_bits),"far_quat":h.far_quat.map(f32::to_bits)});
        rows.push(serde_json::json!({"now":now.to_bits(),"before":history(before),"after":history(*h),"input":format!("{i:?}"),"result":result.ok()}));
    }});result
}
fn classify_inner(h:&mut History,i:Input,now:f64,k:&Settings)->Result<u8,&'static str>{
    if !now.is_finite()||!h.close_time.is_finite()||!h.far_time.is_finite()||!i.radius.is_finite()||i.radius<=0.||!i.sleep_time.is_finite()||i.sleep_time<=0.
        ||![i.angular,i.position,h.close_position,h.far_position].iter().all(|v|v.is_finite())||![i.start,i.end,h.close_quat,h.far_quat].iter().flatten().all(|v|v.is_finite())
        ||![k.translation_squared,k.rotation_squared,k.far_translation_squared,k.far_rotation_squared,k.far_timeout,k.one,k.two,k.angular_factor0,k.angular_factor1].iter().all(|v|v.is_finite()){return Err("invalid movement classifier input");}
    if square(i.position-h.close_position)<=k.translation_squared&&rotation_metric(i.end,h.close_quat,i.radius,k)<=k.rotation_squared{
        if now-h.close_time<=i.sleep_time as f64{return Ok(2);}
        let limit=(k.angular_factor0/i.sleep_time)*k.angular_factor1;
        if square(i.angular)<=limit*limit{return Ok(3);}
        //100eee40 uses x87 products/sums and does not spill the result.
        let a=i.end;let b=i.start;let d=((b[3] as f64*a[3] as f64+b[2] as f64*a[2] as f64)+b[0] as f64*a[0] as f64)+b[1] as f64*a[1] as f64;
        let metric=((1.-d*d)*2.)*i.radius as f64*i.radius as f64;
        if metric<=k.rotation_squared as f64{return Ok(3);}
    }
    let mut next=*h;next.close_time=now;next.close_position=i.position;next.close_quat=i.end;
    if square(i.position-next.far_position)>k.far_translation_squared||rotation_metric(i.start,next.far_quat,i.radius,k)>k.far_rotation_squared{
        next.far_time=now;next.far_position=i.position;next.far_quat=i.start;
    }else if now-next.far_time>k.far_timeout as f64{*h=next;return Ok(3);}
    *h=next;Ok(1)
}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Poll {pub remaining:u16,pub seed:u32}
impl Poll {
    /// The environment owns one countdown/RNG shared across active units.
    pub(crate) fn tick(&mut self,scale:f32,spread:f32)->bool{
        self.remaining=self.remaining.wrapping_sub(1);if self.remaining!=0{return false;}
        self.seed=self.seed.wrapping_mul(75);let jitter=(((self.seed&0xffff) as f64*scale as f64)*spread as f64).trunc() as i16;
        self.remaining=15i16.wrapping_sub(jitter) as u16;true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn data()->Value{serde_json::from_str(include_str!("../tools/fixtures/native-ivp-movement.json")).unwrap()}
    fn f(v:&Value)->f32{v.as_f64().unwrap() as f32}
    fn v(a:&Value)->Vec3{Vec3::new(f(&a[0]),f(&a[1]),f(&a[2]))}
    fn q(a:&Value)->[f32;4]{std::array::from_fn(|i|f(&a[i]))}
    fn history(a:&Value)->History{History{close_time:a["close_time"].as_f64().unwrap(),far_time:a["far_time"].as_f64().unwrap(),close_position:v(&a["close_position"]),far_position:v(&a["far_position"]),close_quat:q(&a["close_quat"]),far_quat:q(&a["far_quat"])}}
    fn input(a:&Value)->Input{Input{radius:f(&a["radius"]),sleep_time:f(&a["sleep_time"]),angular:v(&a["angular"]),position:v(&a["position"]),start:q(&a["start"]),end:q(&a["end"])}}
    fn settings(a:&Value)->Settings{Settings{translation_squared:f(&a["translation_squared"]),rotation_squared:f(&a["rotation_squared"]),far_translation_squared:f(&a["far_translation_squared"]),far_rotation_squared:f(&a["far_rotation_squared"]),far_timeout:f(&a["far_timeout"]),one:f(&a["one"]),two:f(&a["two"]),angular_factor0:f(&a["angular_factor0"]),angular_factor1:f(&a["angular_factor1"])}}
    #[test]
    fn native_movement_classifications_retain_source_pose_history(){
        let d=data();let mut retained=std::collections::HashMap::new();let mut counts=[0;4];let (mut reused,mut resets,mut real)=(0,0,0);
        for c in d["cases"].as_array().unwrap(){let key=(c["model"].as_str().unwrap().to_string(),c["chain"].as_i64().unwrap());
            let mut h=if c["repeat"]==0{history(&c["before"])}else{reused+=1;retained.remove(&key).unwrap()};
            if let Some(now)=c["clock_reset_before"].as_f64(){h.reset_clock(now).unwrap();resets+=1;}
            assert_eq!(h,history(&c["before"]),"before {} {}",c["model"],c["step"]);
            let result=classify(&mut h,input(&c["before"]),c["now"].as_f64().unwrap(),&settings(&c["constants"])).unwrap();
            assert_eq!(result as u64,c["result"].as_u64().unwrap(),"{} step{}",c["model"],c["step"]);assert_eq!(h,history(&c["after"]));counts[result as usize]+=1;
            if c["step"].as_i64().unwrap()>0{real+=1;}retained.insert(key,h);
        }
        assert_eq!(counts,[0,163,185,1218]);assert_eq!((reused,resets,real),(1176,6,30));
    }
    #[test]
    fn native_unit_poll_countdown_and_multiplicative_rng_match(){
        let d=data();let mut count=0;
        for p in d["profiles"].as_array().unwrap(){let r=&p["rng"];let mut poll=Poll{remaining:10,seed:r["initial_seed"].as_u64().unwrap() as u32};let mut steps=vec![];
            for step in 1..=75{if poll.tick(f(&r["scale"]),f(&r["spread"])){steps.push(step);}}
            assert_eq!(steps,p["poll_steps"].as_array().unwrap().iter().map(|v|v.as_i64().unwrap()).collect::<Vec<_>>());assert_eq!(poll.seed,r["final_seed"].as_u64().unwrap() as u32);assert_eq!(poll.remaining,19);count+=steps.len();
        }assert_eq!(count,30);
    }
    #[test]
    fn invalid_movement_input_and_clock_reset_are_atomic(){
        let d=data();let c=&d["cases"][0];let mut h=history(&c["before"]);let saved=h;let mut i=input(&c["before"]);i.position.x=f32::NAN;
        assert!(classify(&mut h,i,1.,&settings(&c["constants"])).is_err());assert_eq!(h,saved);assert!(h.reset_clock(f64::NAN).is_err());assert_eq!(h,saved);
        h.reset_clock(0.25).unwrap();assert_eq!(h.close_time,0.25);assert_eq!(h.far_time,0.25);assert_eq!(h.close_position,saved.close_position);assert_eq!(h.far_position,saved.far_position);assert_eq!(h.close_quat,saved.close_quat);assert_eq!(h.far_quat,saved.far_quat);
    }
}
