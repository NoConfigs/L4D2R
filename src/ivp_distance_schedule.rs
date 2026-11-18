//! Native1010b840 scheduling gates and1010aa10/1010a910 hull tracking.
//! The feature handler still supplies its candidate; full active-pair scheduling
//! and callback/constructor/destructor effects are separate.
use crate::ivp_bounds::{Motion,closing_speed_exact};
use bevy::prelude::*;
#[path="ivp_feature_schedule.rs"]
pub(crate) mod queue;
/// Default initialized settings read from the owned module after world setup.
/// Raw on-disk .data contains pre-initialization values and is not equivalent.
/// These are configuration inputs, never recorded pair/event timelines.
#[derive(Clone,Copy)]
pub(crate) struct Config {pub targets:[f32;64],pub slow_gate:f32,pub time_scale:f32,pub gap:f32}
impl Default for Config{
 fn default()->Self{Self{targets:[f32::from_bits(1003486810);64],slow_gate:f32::from_bits(1058291573),time_scale:1.,gap:f32::from_bits(975594773)}}
}
impl Config{
 /// Bare public-created sphere environments have not applied the configured
 /// performance settings represented by Default's independent native capture.
 pub(crate) fn bare_native()->Self{Self{slow_gate:0.,..Self::default()}}

 /// Public SetGravity converts the force vector before100f9ad0/1010a310
 /// rebuilds this distance profile. The environment recovery vector is separate.
 pub(crate) fn set_force_gravity(&mut self,gravity:Vec3)->Result<(),&'static str>{
  let distance=self.targets[0];
  //100ec3b0 retains all products/sums before spilling its length to float.
  let g=gravity.to_array().map(|v|v as f64);let square=(g[2]*g[2]+g[0]*g[0])+g[1]*g[1];
  if !gravity.is_finite()||!square.is_finite()||!distance.is_finite()||distance<0.{return Err("invalid distance-profile gravity");}
  let magnitude=square.sqrt() as f32;
  let gap=distance*0.1;let target=distance*0.9+gap;
  let escape=distance*0.3+(target+distance);
  // The two float operands are subtracted in x87; the difference does not
  // spill before doubling, multiplying by gravity and taking the square root.
  let spare=escape as f64-target as f64;
  self.slow_gate=((spare+spare)*magnitude as f64).sqrt() as f32;
  self.targets.fill(target);self.gap=gap;Ok(())
 }

 pub(crate) fn target(&self,flags:u32)->Result<f32,&'static str>{self.targets.get(((flags>>22)&255) as usize).copied().filter(|v|v.is_finite()&&*v>=0.).ok_or("invalid distance target index/value")}
}
#[derive(Clone,Copy)]
pub(crate) struct Budget {pub time:f64,pub flags:u32,pub rates:[f32;2],pub accumulated:[f32;2]}
#[derive(Clone,Copy)]
pub(crate) struct Core {pub motion:Motion,pub linear:f32}
pub(crate) struct Input {
 pub flags:u32,pub counter:u32,pub now:f64,pub deadline:f64,pub dt:f32,pub distance:f32,pub target:f32,
 pub normal:Vec3,pub cores:[Core;2],pub budgets:[Budget;2],pub allow_far:bool,pub slow_gate:f32,
}
#[derive(Debug)]
pub(crate) enum Action {Skip,Feature{total_bound:f32,closing_bound:f32},Hull{keys:[f32;2],linear_delta:f32}}
pub(crate) struct Decision {pub flags:u32,pub counter:u32,pub action:Action}
///1010a976..1010aa04 completes each endpoint's SSE rate/accumulated
/// expression before adding those two rounded deltas. The flattened
/// decompiler expression changes cancellation rounding in live glancing pairs.
fn hull_linear_delta(now:f64,budgets:[Budget;2])->f32{
 let delta=|k:usize|{let b=budgets[k];let elapsed=(now-b.time) as f32;(b.rates[0]-b.rates[1])*elapsed+(b.accumulated[0]-b.accumulated[1])};
 delta(1)+delta(0)
}
pub(crate) fn prepare(i:&Input)->Result<Decision,&'static str>{
 if ![i.now,i.deadline].iter().all(|v|v.is_finite())||!i.normal.is_finite()||![i.dt,i.distance,i.target,i.slow_gate].iter().all(|v|v.is_finite())||i.now>i.deadline||i.dt<=0.{return Err("invalid distance scheduling inputs");}
 let selector=((i.flags>>8)&3) as usize;if selector>1{return Err("invalid selected pair side");}
 let a=i.cores[selector];let b=i.cores[1-selector];
 let total=(a.linear+(a.motion.angular_bound+b.motion.angular_bound))+b.linear;
 if !total.is_finite()||total<0.{return Err("invalid total motion bound");}
 let mut out=Decision{flags:i.flags,counter:i.counter,action:Action::Skip};
 if (i.dt*total)*f32::from_bits(0x40066666)+i.target<i.distance{
    if i.allow_far{
        let spare=i.distance-i.target;let mut shares=[0.,0.];
        if i.budgets[0].flags&7==0{shares[1]=spare;}else if i.budgets[1].flags&7==0{shares[0]=spare;}else{
            let speeds=i.cores.map(|c|(c.motion.angular_bound+c.linear)+f32::from_bits(0x2edbe6ff));
            let weights=[speeds[1]*0.1+speeds[0],speeds[0]*0.1+speeds[1]];let scale=spare/(weights[1]+weights[0]);shares=[scale*weights[0],scale*weights[1]];
        }
        let elapsed=i.budgets.map(|b|(i.now-b.time) as f32);
        let keys=std::array::from_fn(|k|(elapsed[k]*i.budgets[k].rates[0]+i.budgets[k].accumulated[0])+shares[k]);
        let linear_delta=hull_linear_delta(i.now,i.budgets);
        out.flags=(i.flags&0xffd7ffff)|0x140000;out.action=Action::Hull{keys,linear_delta};
    }return Ok(out);
 }
 let closing=closing_speed_exact(a.motion,b.motion,i.normal).ok_or("invalid directional motion bound")?;
 if closing<i.slow_gate as f64&&(closing as f32)<f32::from_bits(0x2edbe6ff){return Ok(out);}
 if (i.deadline-i.now)*closing+i.target as f64<=i.distance as f64{return Ok(out);}
 if i.flags&0x3000==0x1000{return Ok(out);}
 let index=(i.flags>>22)&255;
 if index!=0{out.counter=i.counter.wrapping_add(1);if i.counter>2{out.flags=(i.flags&!0x3fc00000)|((index-1)<<22);out.counter=0;}}
 out.action=Action::Feature{total_bound:total,closing_bound:closing as f32};Ok(out)
}
/// The caller cancels an existing event before prepare, even if prepare skips.
/// Candidate geometry/time is returned by the separate feature handler.
pub(crate) fn complete(i:&Input,d:&mut Decision,kind:u32,candidate:Option<f64>,delay:u8,time_scale:f32,epoch:f64)->Result<Option<f32>,&'static str>{
 let Action::Feature{total_bound,..}=d.action else{if kind!=0||candidate.is_some(){return Err("unexpected feature output");}return Ok(None);};
 if kind==0{if candidate.is_some(){return Err("unwritten feature candidate");}return Ok(None);}
 let candidate=candidate.ok_or("missing feature candidate")?;
 let admission=crate::ivp_time::EventAdmission{now:i.now,deadline:i.deadline,candidate,epoch,dt:i.dt,distance:i.distance,total_bound,kind,delay_mode:delay,time_scale};
 let Some(at)=admission.queued()else{return Ok(None);};
 d.flags=(d.flags&!255)|(kind&255);Ok(Some((at-epoch) as f32))
}
///1010bc90 runs after removing its heap entry and refreshing closest geometry.
/// COMISS/JBE makes equality and unordered distances bypass physical dispatch.
#[derive(Debug,PartialEq,Eq)]
pub(crate) enum Callback {InvalidGeometry,Physical,Reschedule{delay:u8}}
pub(crate) fn callback(flags:u32,distance:f32,config:Config)->Result<Callback,&'static str>{
 if flags&0xc000!=0{return Ok(Callback::InvalidGeometry);}
 if flags&15!=0{return Ok(Callback::Reschedule{delay:1});}
 let threshold=config.target(flags)?+f32::from_bits(0x3a266515);
 if distance<threshold{Ok(Callback::Physical)}else{Ok(Callback::Reschedule{delay:2})}
}
#[cfg(test)]
pub(crate) fn captured_input(s:&serde_json::Value)->Input{
 let f=|v:&serde_json::Value|v.as_f64().unwrap() as f32;let vec=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|k|f(&v[k])));
 Input{flags:s["before"]["flags"].as_u64().unwrap() as u32,counter:s["counter_before"].as_u64().unwrap() as u32,now:s["now"].as_f64().unwrap(),deadline:s["deadline"].as_f64().unwrap(),dt:f(&s["dt"]),distance:f(&s["before"]["measures"][2]),target:f(&s["target"]),normal:vec(&s["before"]["normal"]),cores:std::array::from_fn(|k|{let m=&s["motion"][k];Core{motion:Motion{axis:vec(&m["axis"]),velocity:vec(&m["velocity"]),angular_bound:f(&m["angular"])},linear:f(&m["linear"])}}),budgets:std::array::from_fn(|k|{let b=&s["budgets"][k];Budget{time:b["time"].as_f64().unwrap(),flags:b["flags"].as_u64().unwrap() as u32,rates:std::array::from_fn(|j|f(&b["rates"][j])),accumulated:std::array::from_fn(|j|f(&b["accumulated"][j]))}}),allow_far:s["allow_far"]==1,slow_gate:f(&s["slow_gate"])}
}
#[cfg(test)]
mod tests{
 #[test]
 fn native_live_hull_endpoint_deltas_complete_before_pair_addition(){
  let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hull-linear-delta.json")).unwrap();let f=|v:&serde_json::Value|f32::from_bits(v.as_u64().unwrap() as u32);
  for (i,row) in data["cases"].as_array().unwrap().iter().enumerate(){let budgets=std::array::from_fn(|k|{let b=&row["budgets"][k];Budget{time:f64::from_bits(b["time"].as_u64().unwrap()),rates:std::array::from_fn(|j|f(&b["rates"][j])),accumulated:std::array::from_fn(|j|f(&b["accumulated"][j])),flags:0}});assert_eq!(hull_linear_delta(f64::from_bits(row["now"].as_u64().unwrap()),budgets).to_bits() as u64,row["delta"].as_u64().unwrap(),"native hull row{i}");}
  assert_eq!(data["cases"].as_array().unwrap().len(),646);
 }

 use super::*;
 #[test]
 fn public_gravity_rebuilds_the_native_distance_profile(){
  let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-gravity-settings.json")).unwrap();let mut config=Config::default();
  for case in data["cases"].as_array().unwrap(){
   let input=std::array::from_fn::<_,3,_>(|k|f32::from_bits(case["input"][k].as_u64().unwrap() as u32));let unit=f32::from_bits(0x3cd013a9);
   config.set_force_gravity(Vec3::new(input[0]*unit,-(input[2]*unit),input[1]*unit)).unwrap();
   assert_eq!(serde_json::json!(config.slow_gate.to_bits()),case["gate"]);
   assert_eq!(serde_json::json!(config.targets.map(f32::to_bits).to_vec()),case["targets"]);
   assert_eq!(serde_json::json!(config.gap.to_bits()),case["gap"]);assert_eq!(serde_json::json!(config.time_scale.to_bits()),case["time_scale"]);
   assert_eq!(case["environment_gravity"],serde_json::json!([0u32,9.83f32.to_bits(),0]));
  }assert_eq!(data["cases"].as_array().unwrap().len(),128);
 }
 #[test]
 fn callback_strict_threshold_and_kind_gates_follow_native_instructions(){
    let c=Config::default();let flags=0x0fcc0020;let threshold=c.target(flags).unwrap()+f32::from_bits(0x3a266515);
    assert_eq!(callback(flags,f32::from_bits(threshold.to_bits()-1),c).unwrap(),Callback::Physical);
    for distance in [threshold,f32::INFINITY,f32::NAN]{assert_eq!(callback(flags,distance,c).unwrap(),Callback::Reschedule{delay:2});}
    assert_eq!(callback(flags|1,0.,c).unwrap(),Callback::Reschedule{delay:1});
    assert_eq!(callback(flags|0x4000,0.,c).unwrap(),Callback::InvalidGeometry);
 }
 #[test]
 fn initialized_world_settings_match_independent_native_capture_and_all_schedule_inputs(){
    let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-distance-config.json")).unwrap();
    let c=Config::default();let settings=&d["settings"];
    assert_eq!(serde_json::json!(c.targets.iter().map(|v|v.to_bits()).collect::<Vec<_>>()),settings["targets_bits"]);
    for (key,value) in [("slow_gate_bits",c.slow_gate),("time_scale_bits",c.time_scale),("gap_bits",c.gap)]{assert_eq!(value.to_bits() as u64,settings[key].as_u64().unwrap());}
    let schedules:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-distance-schedule.json")).unwrap();let mut count=0;
    for case in schedules["cases"].as_array().unwrap(){for s in case["schedules"].as_array().unwrap(){
        let flags=s["before"]["flags"].as_u64().unwrap() as u32;
        assert_eq!(c.target(flags).unwrap().to_bits(),(s["target"].as_f64().unwrap() as f32).to_bits());
        assert_eq!(c.slow_gate.to_bits(),(s["slow_gate"].as_f64().unwrap() as f32).to_bits());
        assert_eq!(c.time_scale.to_bits(),(s["time_scale"].as_f64().unwrap() as f32).to_bits());count+=1;
    }}assert_eq!(count,278);
    assert!(c.target(64<<22).is_err());assert!(c.target(255<<22).is_err());
 }
 #[test]
 fn native_distance_schedule_decisions_flags_tracking_and_queue_keys_match(){
    let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-distance-schedule.json")).unwrap();let mut count=0;let mut feature=0;let mut hull=0;let mut queued=0;
    let f=|v:&serde_json::Value|v.as_f64().unwrap() as f32;
    for c in d["cases"].as_array().unwrap(){for s in c["schedules"].as_array().unwrap(){
        let config=Config::default();let mut input=captured_input(s);
        // Exercise source configuration instead of replaying each recorded
        // target/slow-gate input. The separate control checks those bits.
        input.target=config.target(input.flags).unwrap();input.slow_gate=config.slow_gate;
        let mut decision=prepare(&input).unwrap();let called=matches!(decision.action,Action::Feature{..});assert_eq!(called,s["feature_called"].as_bool().unwrap(),"{} {} step{} preflight",c["mode"],c["model"],s["step"]);
        if let Action::Hull{keys,linear_delta}=decision.action{
            assert_eq!(keys.map(f32::to_bits),std::array::from_fn::<_,2,_>(|j|f(&s["hull_keys"][j]).to_bits()),"hull keys {count}");
            assert_eq!(linear_delta.to_bits(),f(&s["after"]["measures"][1]).to_bits(),"linear delta {count}");hull+=1;
        }else{assert!(s["hull_keys"].is_null());}
        let key=complete(&input,&mut decision,s["feature_kind"].as_u64().unwrap() as u32,s["feature_candidate"].as_f64(),s["delay"].as_u64().unwrap() as u8,config.time_scale,s["queue_after"]["epoch"].as_f64().unwrap()).unwrap();
        assert_eq!(key.map(f32::to_bits),s["queue_after"]["key"].as_f64().map(|v|(v as f32).to_bits()),"queue key {count}");
        assert_eq!(decision.flags,s["after"]["flags"].as_u64().unwrap() as u32,"flags {count}");assert_eq!(decision.counter,s["counter_after"].as_u64().unwrap() as u32,"counter {count}");feature+=usize::from(called);queued+=usize::from(key.is_some());count+=1;
    }}assert_eq!(count,278);assert_eq!(feature,214);assert_eq!(hull,24);assert_eq!(queued,153);
 }
}
