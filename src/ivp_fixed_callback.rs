//! Native100bc2c0/100bbf30 fixed-joint numeric callbacks. Wrapper allocation,
//! breakage, compound object rotations and collision-world ownership are separate.
pub(crate) use crate::ivp_joint_callback::{Body,Step,Phase};
pub(crate) use crate::ivp_hinge_rows::Entry;
#[derive(Clone,Copy,Debug)]
pub(crate) struct Cache {pub bias:[f32;6],pub inverse:[[f32;8];6],pub rows:[[Entry;6];2],pub relative:[f32;8]}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Joint {pub columns:[[f32;3];3],pub position:[f32;3],pub tau:f32,pub damping:f32,pub work:Option<Cache>}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Parameters {pub frame:[[f32;4];3],pub strength:f32,pub force_limit:f32,pub torque_limit:f32,pub mass_scale:[f32;2]}
pub(crate) fn construct(p:Parameters)->Result<Joint,&'static str>{
 if p.frame.into_iter().flatten().chain([p.strength,p.force_limit,p.torque_limit]).chain(p.mass_scale).any(|x|!x.is_finite()) {return Err("invalid fixed-joint parameters");}
 let threshold=f32::from_bits(0x5368d4a5);
 if (p.force_limit!=0.&&p.force_limit<threshold)||(p.torque_limit!=0.&&p.torque_limit<threshold)||p.mass_scale.into_iter().any(|v|v!=0.&&v!=1.){return Err("breakable fixed wrappers are not owned");}
 let m=p.frame;let unit=f32::from_bits(0x3cd013a9);
 Ok(Joint{columns:[[m[0][0],-m[2][0],m[1][0]],[-m[0][2],m[2][2],-m[1][2]],[m[0][1],-m[2][1],m[1][1]]],position:[m[0][3]*unit,-(m[2][3]*unit),m[1][3]*unit],tau:p.strength,damping:1.,work:None})
}
fn transform(r:[[f32;3];3],v:[f32;3])->[f32;3]{std::array::from_fn(|i|(r[i][0]*v[0]+r[i][1]*v[1])+r[i][2]*v[2])}
fn dot(a:[f32;3],b:[f32;3])->f32{(a[1]*b[1]+a[0]*b[0])+a[2]*b[2]}
fn invert(matrix:&mut[[f32;8];6])->Result<(),&'static str>{
 let mut a=*matrix;let mut b:[[f32;6];6]=std::array::from_fn(|i|std::array::from_fn(|j|if i==j{1.}else{0.}));
 for i in 0..6 {let pivot=a[i][i];if !pivot.is_finite()||pivot==0.{return Err("invalid fixed matrix pivot");}let reciprocal=1./pivot;
  for j in i..6{a[i][j]=reciprocal*a[i][j];}for j in 0..=i{b[i][j]*=reciprocal;}
  for k in i+1..6{let f=a[k][i];if f!=0.{for j in 0..=i{b[k][j]-=b[i][j]*f;}for j in i..6{a[k][j]-=a[i][j]*f;}}}
 }
 for i in (0..6).rev(){for k in (0..i).rev(){let f=a[k][i];if f!=0.{for j in (0..6).rev(){b[k][j]-=b[i][j]*f;}for j in (i..6).rev(){a[k][j]-=a[i][j]*f;}}}}
 for i in 0..6{matrix[i][..6].copy_from_slice(&b[i]);}Ok(())
}
impl Joint {
 pub(crate) fn prepare(&self,bodies:&[Body;2],inverse_dt:f32)->Result<Cache,&'static str>{
  let origins:[[_;3];2]=std::array::from_fn(|s|{let b=bodies[s];if let Some(offset)=b.object_offset{let v=transform(b.rotation,offset);std::array::from_fn(|i|v[i]+b.position[i])}else{b.position}});
  let columns=self.columns.map(|c|transform(bodies[0].rotation,c));let delta=transform(bodies[0].rotation,self.position);let point:[f32;3]=std::array::from_fn(|i|delta[i]+origins[0][i]);
  let attached:[[f32;3];3]=std::array::from_fn(|j|std::array::from_fn(|i|bodies[1].rotation[i][j]));
  let mut cache=Cache{bias:[0.;6],inverse:[[0.;8];6],rows:[[Entry::default();6];2],relative:[0.;8]};
  for i in 0..3{cache.bias[i]=(origins[1][i]-point[i])*inverse_dt;}
  cache.bias[3]=dot(columns[2],attached[1])*inverse_dt;cache.bias[4]=-(dot(columns[2],attached[0])*inverse_dt);cache.bias[5]=dot(columns[1],attached[0])*inverse_dt;
  for s in 0..2{let b=bodies[s];let [x,y,z]=std::array::from_fn(|i|point[i]-b.position[i]);let rt=std::array::from_fn(|i|std::array::from_fn(|j|b.rotation[j][i]));
   let angular=if s==0{[[0.,z,-y],[-z,0.,x],[y,-x,0.],columns[0],columns[1],columns[2]]}else{[[0.,-z,y],[z,0.,-x],[-y,x,0.],columns[0].map(|v|-v),columns[1].map(|v|-v),columns[2].map(|v|-v)]};
   for i in 0..6{let angular=transform(rt,angular[i]);cache.rows[s][i]=Entry{linear:std::array::from_fn(|j|if i<3&&i==j{if s==0{1.}else{-1.}}else{0.}),angular,index:i,response:if b.flags&0x12!=0{[0.;3]}else{std::array::from_fn(|j|angular[j]*b.inverse[j])}};}
  }
  for s in 0..2{let b=bodies[s];if b.flags&0x12!=0{continue;}for i in 0..6{for j in 0..=i{let cur=cache.rows[s][i];let prev=cache.rows[s][j];let a=prev.angular;let r=cur.response;
   let angular=if i>=4&&j<4{(a[0]*r[0]+a[1]*r[1])+a[2]*r[2]}else{dot(a,r)};
   let linear=(cur.linear[0]*prev.linear[0]+cur.linear[1]*prev.linear[1])+cur.linear[2]*prev.linear[2];let contribution=linear*b.inverse[3]+angular;
   cache.inverse[j][i]=contribution+cache.inverse[j][i];if i!=j{cache.inverse[i][j]=contribution+cache.inverse[i][j];}
  }}}
  invert(&mut cache.inverse)?;Ok(cache)
 }
 pub(crate) fn run(&mut self,bodies:&mut[Body;2],step:Step,phase:Phase)->Result<(),&'static str>{
  if ![step.dt,step.inverse_dt,step.alpha,step.beta,self.tau,self.damping].into_iter().all(f32::is_finite)||step.dt<=0.||step.inverse_dt<=0.{return Err("invalid fixed callback parameters");}
  let initial=matches!(phase,Phase::Initial);let mut cache=if initial{self.prepare(bodies,step.inverse_dt)?}else{self.work.ok_or("cached fixed joint lacks preparation")?};let mut end=*bodies;
  if cache.bias.into_iter().chain(cache.inverse.into_iter().flatten()).any(|v|!v.is_finite())||cache.rows.iter().flatten().any(|r|r.index>=6||r.linear.into_iter().chain(r.angular).chain(r.response).any(|v|!v.is_finite())){return Err("invalid fixed row index");}
  let mut relative=[0.;8];for s in 0..2{let b=end[s];if initial&&b.flags&0x12!=0{continue;}for row in cache.rows[s]{let angular=dot(row.angular,[b.angular[0],b.angular[1],b.angular[2]]);let linear=if initial{(row.linear[0]*b.linear[0]+row.linear[1]*b.linear[1])+row.linear[2]*b.linear[2]}else{dot(row.linear,[b.linear[0],b.linear[1],b.linear[2]])};relative[row.index]=(linear+angular)+relative[row.index];}}
  //100bbf30 uses alpha directly; initial100bc2c0 multiplies it by tau.
  let tau=if initial{self.tau*step.alpha}else{step.alpha};let damping=(self.damping * -1.)*step.beta;
  let rhs:[f32;6]=std::array::from_fn(|i|relative[i]*damping+cache.bias[i]*tau);let impulse:[f32;6]=std::array::from_fn(|i|{let mut v=0.;for j in 0..6{v=cache.inverse[j][i]*rhs[j]+v;}v});
  for s in 0..2{if end[s].flags&0x12!=0{continue;}for row in cache.rows[s]{let impulse=impulse[row.index];for i in 0..3{end[s].angular[i]=row.response[i]*impulse+end[s].angular[i];}let scale=impulse*end[s].inverse[3];for i in 0..3{end[s].linear[i]=row.linear[i]*scale+end[s].linear[i];}}}
  if end.iter().any(|b|b.angular.into_iter().chain(b.linear).any(|v|!v.is_finite())){return Err("fixed impulse overflow");}
  cache.relative=relative;*bodies=end;self.work=Some(cache);Ok(())
 }
}

#[cfg(test)]
pub(crate) mod tests {
 use super::*;
 use serde_json::{Value,json};use std::collections::BTreeMap;
 fn fixture()->Value{serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-fixed-joint.json")).unwrap())).unwrap()}
 fn f(v:&Value)->f32{f32::from_bits(v.as_u64().unwrap() as u32)}
 fn body(v:&Value)->Body{Body{flags:v["flags"].as_u64().unwrap() as u16,inverse:std::array::from_fn(|i|f(&v["inverse"][i])),rotation:std::array::from_fn(|i|std::array::from_fn(|j|f(&v["rotation"][i*3+j]))),position:std::array::from_fn(|i|f(&v["position"][i])),object_offset:(v["object_flags"].as_u64().unwrap()&0x800==0).then(||std::array::from_fn(|i|f(&v["offset"][i]))),angular:std::array::from_fn(|i|f(&v["angular"][i])),linear:std::array::from_fn(|i|f(&v["linear"][i]))}}
 fn parameters(v:&Value)->Parameters{Parameters{frame:std::array::from_fn(|i|std::array::from_fn(|j|f(&v[i*4+j]))),strength:f(&v[12]),force_limit:f(&v[13]),torque_limit:f(&v[14]),mass_scale:[f(&v[15]),f(&v[16])]}}
 fn geometry(j:&Joint)->Value{let mut v=vec![];for c in j.columns{v.extend(c.map(f32::to_bits));v.push(0);}v.extend(j.position.map(f32::to_bits));v.push(0);json!(v)}
 pub(crate) fn check(a:&Value,b:&Value,path:&str){if let(Some(a),Some(b))=(a.as_object(),b.as_object()){assert_eq!(a.keys().collect::<Vec<_>>(),b.keys().collect::<Vec<_>>(),"keys {path}");for(k,v)in a{check(v,&b[k],&format!("{path}/{k}"));}}else if let(Some(a),Some(b))=(a.as_array(),b.as_array()){assert_eq!(a.len(),b.len(),"length {path}");for(i,(a,b))in a.iter().zip(b).enumerate(){check(a,b,&format!("{path}/{i}"));}}else{assert_eq!(a,b,"{path}");}}
 fn snapshot(before:&Value,bodies:&[Body;2],joint:&Joint)->Value{let mut p=before.clone();p.as_object_mut().unwrap().remove("work");p["geometry"]=geometry(joint);p["tau"]=json!(joint.tau.to_bits());p["damping"]=json!(joint.damping.to_bits());let c=joint.work.unwrap();p["stride"]=json!(8);p["bias"]=json!(c.bias.map(f32::to_bits));p["matrix"]=json!(c.inverse.into_iter().flatten().map(f32::to_bits).collect::<Vec<_>>());p["relative"]=json!(c.relative.map(f32::to_bits));p["rows"]=json!(c.rows.iter().enumerate().map(|(s,rows)|rows.iter().map(|r|json!({"linear":r.linear.map(f32::to_bits),"angular":r.angular.map(f32::to_bits),"index":r.index,"response":if bodies[s].flags&0x12!=0{Value::Null}else{json!(r.response.map(f32::to_bits))}})).collect::<Vec<_>>()).collect::<Vec<_>>());for s in 0..2{p["cores"][s]["angular"]=json!(bodies[s].angular.map(f32::to_bits));p["cores"][s]["linear"]=json!(bodies[s].linear.map(f32::to_bits));}p}
 #[test]
 fn native_fixed_geometry_rows_bias_and_six_row_inverse_are_source_owned(){
  let data=fixture();let mut count=0;
  for call in data["calls"].as_array().unwrap(){if call["phase"]!=0{continue;}let case=data["cases"].as_array().unwrap().iter().find(|c|c["scenario"]==call["scenario"]).unwrap();let joint=construct(parameters(&case["public_fixed"][call["joint"].as_u64().unwrap() as usize])).unwrap();let b=std::array::from_fn(|s|body(&call["before"]["cores"][s]));let mut joint=joint;joint.work=Some(joint.prepare(&b,f(&call["inverse_dt_bits"])).unwrap());let mut actual=snapshot(&call["before"],&b,&joint);actual["relative"]=call["after"]["relative"].clone();for s in 0..2{actual["cores"][s]["angular"]=call["after"]["cores"][s]["angular"].clone();actual["cores"][s]["linear"]=call["after"]["cores"][s]["linear"].clone();}check(&actual,&call["after"],&format!("fixed preparation scenario{} step{} joint{}",call["scenario"],call["step"],call["joint"]));count+=1;
  }assert_eq!(count,182);
 }
 #[test]
 fn native_fixed_shared_groups_preserve_initial_and_cached_callback_state(){
  let data=fixture();let mut total=0;
  for case in data["cases"].as_array().unwrap(){let mut joints=case["public_fixed"].as_array().unwrap().iter().map(|p|construct(parameters(p)).unwrap()).collect::<Vec<_>>();let order=case["order"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();
   for tick in 0..case["num_steps"].as_u64().unwrap(){let calls=data["calls"].as_array().unwrap().iter().filter(|c|c["scenario"]==case["scenario"]&&c["step"]==tick).collect::<Vec<_>>();let per_psi=crate::ivp_joint_group::Group::new(case["authored_iterations"].as_i64().unwrap().max(0) as u32).batches().count()*order.len();for calls in calls.chunks(per_psi){let mut bodies=BTreeMap::new();for c in calls{for b in c["before"]["cores"].as_array().unwrap(){bodies.entry(b["id"].as_u64().unwrap() as usize).or_insert_with(||body(b));}}let mut at=0;
    for batch in crate::ivp_joint_group::Group::new(case["authored_iterations"].as_i64().unwrap().max(0) as u32).batches(){for slot in 0..order.len(){let slot=if batch.reverse{order.len()-1-slot}else{slot};let id=order[slot];let c=calls[at];assert_eq!(id,c["joint"].as_u64().unwrap() as usize);let ids:[usize;2]=std::array::from_fn(|s|c["before"]["cores"][s]["id"].as_u64().unwrap() as usize);let mut pair=ids.map(|id|bodies[&id]);let phase=if batch.prepare{Phase::Initial}else{Phase::Cached};let step=Step{dt:f(&c["dt_bits"]),inverse_dt:f(&c["inverse_dt_bits"]),alpha:batch.p4,beta:batch.p5};assert_eq!((batch.p4.to_bits(),batch.p5.to_bits()),(c["p4"].as_u64().unwrap() as u32,c["p5"].as_u64().unwrap() as u32));joints[id].run(&mut pair,step,phase).unwrap();check(&snapshot(&c["before"],&pair,&joints[id]),&c["after"],&format!("fixed callback scenario{} tick{tick} call{at}",case["scenario"]));for s in 0..2{bodies.insert(ids[s],pair[s]);}at+=1;}}
    assert_eq!(at,calls.len());total+=at;
   }}
  }assert_eq!(total,1834);
 }
 #[test]
 fn invalid_fixed_parameters_and_cache_cannot_commit_partial_impulses(){
  let data=fixture();let c=&data["calls"][0];let p=parameters(&data["cases"][0]["public_fixed"][0]);for bad in [Parameters{force_limit:1.,..p},Parameters{mass_scale:[1.,2.],..p},Parameters{strength:f32::NAN,..p}]{assert!(construct(bad).is_err());}
  let mut joint=construct(p).unwrap();let mut bodies=std::array::from_fn(|s|body(&c["before"]["cores"][s]));let step=Step{dt:f(&c["dt_bits"]),inverse_dt:f(&c["inverse_dt_bits"]),alpha:f(&c["p4"]),beta:f(&c["p5"])};let before=format!("{joint:?}/{bodies:?}");assert!(joint.run(&mut bodies,step,Phase::Cached).is_err());assert_eq!(format!("{joint:?}/{bodies:?}"),before);joint.run(&mut bodies,step,Phase::Initial).unwrap();joint.work.as_mut().unwrap().rows[1][5].index=6;let before=format!("{joint:?}/{bodies:?}");assert!(joint.run(&mut bodies,step,Phase::Cached).is_err());assert_eq!(format!("{joint:?}/{bodies:?}"),before);assert!(joint.run(&mut bodies,Step{inverse_dt:f32::NAN,..step},Phase::Initial).is_err());assert_eq!(format!("{joint:?}/{bodies:?}"),before);
 }

}
