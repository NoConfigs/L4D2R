//! Normal far-mindist1010c8a0: conservative rearm or geometry reactivation.
//! Coarse10121600 handling and the activation callback itself remain separate.
use bevy::prelude::*;
use crate::ivp_hull_budget::Budget;
#[derive(Clone,Copy,Debug)]
pub(crate) struct Core {pub time:f64,pub start:Vec3,pub velocity:Vec3,pub linear:f32,pub angular:f32}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Geometry {pub projection:f32,pub linear_delta:f32,pub distance:f32,pub normal:Vec3}
impl Geometry {pub(crate) fn rebase(&mut self,motion:f32,linear:f32){self.linear_delta=(motion-linear)+self.linear_delta;}}
pub(crate) struct Input {pub flags:u32,pub now:f64,pub dt:f32,pub gap:f32,pub geometry:Geometry,pub cores:[Core;2],pub budgets:[Budget;2]}
#[derive(Clone,Copy)]
pub(crate) struct Config {pub epsilon:f32,pub factor:f32}
impl Default for Config{fn default()->Self{Self{epsilon:f32::from_bits(0x2edbe6ff),factor:6.}}}
#[derive(Clone,Copy,Debug)]
pub(crate) enum Action {Rearm{geometry:Geometry,keys:[f32;2],first:usize},Activate{pending:bool}}
pub(crate) fn update(i:&Input,c:Config)->Result<Action,&'static str>{
 if i.flags&0x3c0000==0x100000{return Err("coarse far callback not yet reconstructed");}
 let selected=((i.flags>>8)&3) as usize;if selected>1{return Err("invalid far pair selector");}let other=1-selected;
 if !i.now.is_finite()||![i.dt,i.gap,i.geometry.projection,i.geometry.linear_delta,i.geometry.distance,c.epsilon,c.factor].iter().all(|v|v.is_finite())||i.dt<=0.||c.epsilon<=0.||c.factor<=0.||!i.geometry.normal.is_finite(){return Err("invalid far callback input");}
 for k in i.cores{if !k.time.is_finite()||!k.start.is_finite()||!k.velocity.is_finite()||![k.linear,k.angular].iter().all(|v|v.is_finite()&&*v>=0.){return Err("invalid far core input");}}
 for b in i.budgets{if !b.time.is_finite()||!b.rates.iter().chain(&b.accumulated).all(|v|v.is_finite()){return Err("invalid far budget input");}}
 let activate=Action::Activate{pending:i.flags&0x3000==0x1000};if i.flags&0x30000!=0{return Ok(activate);}
 let speeds=i.cores.map(|k|(k.angular+k.linear)+c.epsilon);let total=speeds[other]+speeds[selected];
 if !total.is_finite()||total<=0.{return Err("invalid far total motion");}
 // Unlike surface admission, all three elapsed-time components spill to f32.
 let positions=i.cores.map(|k|{let elapsed=(i.now-k.time) as f32;k.velocity*elapsed+k.start});
 let delta=positions[selected]-positions[other];let n=i.geometry.normal;
 let projection=(delta.y*n.y+delta.x*n.x)+delta.z*n.z;
 //1010cae1..1010cb4b: each object's rate product is added to its
 // accumulated difference, then the two completed x87 expressions are added.
 let budget_delta=|b:Budget|(i.now-b.time)*(b.rates[0] as f64-b.rates[1] as f64)+(b.accumulated[0] as f64-b.accumulated[1] as f64);
 let linear_delta=(budget_delta(i.budgets[other])+budget_delta(i.budgets[selected])) as f32;
 let available=((i.geometry.distance-(linear_delta-i.geometry.linear_delta))-(i.geometry.projection-projection))+i.gap;
 if !(i.dt*total*c.factor<available){return Ok(activate);}
 let scale=available/total;let keys=std::array::from_fn(|k|scale*speeds[k]+i.budgets[k].motion_at(i.now));
 let geometry=Geometry{projection,linear_delta,distance:available-i.gap,normal:n};
 if !keys.iter().all(|v|v.is_finite())||![geometry.distance,geometry.projection,geometry.linear_delta].iter().all(|v|v.is_finite()){return Err("nonfinite far rearm result");}
 Ok(Action::Rearm{geometry,keys,first:selected})
}
#[cfg(test)]
mod tests{
 use super::*;
 #[test]
 fn native_rebase_changes_only_the_retained_linear_delta(){
  let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-far-distance.json")).unwrap();let mut count=0;
  for c in d["rebase_controls"].as_array().unwrap(){let f=|name:&str|f32::from_bits(c[name].as_u64().unwrap() as u32);
   let mut geometry=Geometry{projection:0.25,linear_delta:f("before_bits"),distance:0.5,normal:Vec3::Y};geometry.rebase(f("motion_bits"),f("linear_bits"));
   assert_eq!(geometry.linear_delta.to_bits(),c["after_bits"].as_u64().unwrap() as u32,"control{}",c["index"]);assert_eq!((geometry.projection,geometry.distance,geometry.normal),(0.25,0.5,Vec3::Y));count+=1;
  }assert_eq!(count,96);
 }
 #[test]
 fn actual_native_far_callbacks_match_branch_and_exact_rearm_state(){
  let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-far-distance.json")).unwrap();let(mut count,mut rearmed,mut activated)=(0,0,0);
  let f=|v:&serde_json::Value|v.as_f64().unwrap() as f32;let vec=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|k|f(&v[k])));
  for c in d["cases"].as_array().unwrap(){for s in c["calls"].as_array().unwrap(){let b=&s["before"];
   let input=Input{flags:b["flags"].as_u64().unwrap() as u32,now:s["now"].as_f64().unwrap(),dt:f(&s["dt"]),gap:f(&s["gap"]),
    geometry:Geometry{projection:f(&b["projection"]),linear_delta:f(&b["linear_delta"]),distance:f(&b["distance"]),normal:vec(&b["normal"])},
    cores:std::array::from_fn(|k|{let x=&s["cores"][k];Core{time:x["time"].as_f64().unwrap(),start:vec(&x["start"]),velocity:vec(&x["velocity"]),linear:f(&x["linear"]),angular:f(&x["angular"])}}),
    budgets:std::array::from_fn(|k|{let x=&s["budgets"][k];Budget{time:x["time"].as_f64().unwrap(),rates:std::array::from_fn(|j|f(&x["rates"][j])),accumulated:std::array::from_fn(|j|f(&x["accumulated"][j])),predicted:f(&x["predicted"]),threshold:f(&x["threshold"])}})};
   let config=Config::default();assert_eq!(config.factor.to_bits(),f(&s["factor"]).to_bits());assert_eq!(config.epsilon.to_bits(),f(&s["epsilon"]).to_bits());
   match update(&input,config).unwrap(){
    Action::Activate{pending}=>{assert_eq!(s["activation"],if pending{2}else{1},"{} {} step{}",c["mode"],c["model"],s["step"]);activated+=1;},
    Action::Rearm{geometry,keys,first}=>{assert_eq!(s["activation"],0,"{} {} step{}",c["mode"],c["model"],s["step"]);let after=&s["after"];
     for(key,value)in[("projection",geometry.projection),("linear_delta",geometry.linear_delta),("distance",geometry.distance)]{assert_eq!(value.to_bits(),f(&after[key]).to_bits(),"{} {} step{} {key}",c["mode"],c["model"],s["step"]);}
     assert_eq!(keys.map(f32::to_bits),std::array::from_fn::<_,2,_>(|k|f(&after["keys"][k]).to_bits()),"{} {} step{} keys",c["mode"],c["model"],s["step"]);
     assert_eq!(geometry.normal.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|k|f(&after["normal"][k]).to_bits()));assert_eq!(first,((input.flags>>8)&3) as usize);rearmed+=1;
    }
   }count+=1;
  }}assert_eq!(count,d["counts"]["calls"].as_u64().unwrap() as usize);assert_eq!(rearmed,d["counts"]["rearmed"].as_u64().unwrap() as usize);assert_eq!(activated,d["counts"]["activated"].as_u64().unwrap() as usize);assert!(rearmed>0&&activated>0);
 }
}
