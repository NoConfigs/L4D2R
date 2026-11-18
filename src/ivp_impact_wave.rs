//!10104490/10103b50/10103c10 contact propagation. Owns transient
//! traversal IDs/backups and physical prediction rebuilding over the canonical
//! world. Bounds dispatch, geometry retirement and hold/anomaly callbacks are separate.
use std::collections::BTreeMap;
use crate::{ivp_contact_system::Pair,ivp_impact_state::Saved,ivp_registered_contacts::{CollisionContext,AdmittedContact}};
struct CacheAccess<'a>{cache:&'a mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context}
fn refresh(context:&mut CollisionContext<'_>,keys:&[usize],cache:&mut Option<CacheAccess<'_>>)->Result<Vec<usize>,&'static str>{
    if let Some(c)=cache.as_mut(){context.refresh_impact_contacts_cached(keys,c.cache,c.clock)}else{context.refresh_impact_contacts(keys)}
}
pub(crate) struct Wave{
    pub loops:u32,pub changed:Vec<usize>,pub backed:Vec<usize>,pub pairs:Vec<usize>,
    pub saved:BTreeMap<usize,Saved>,groups:Vec<Pair>,system:usize,
}
impl Wave{
    pub(crate) fn new(context:&mut CollisionContext<'_>,admitted:AdmittedContact)->Result<Self,&'static str>{
        Self::new_inner(context,admitted,None)
    }
    pub(crate) fn new_cached(context:&mut CollisionContext<'_>,admitted:AdmittedContact,cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context)->Result<Self,&'static str>{Self::new_inner(context,admitted,Some(CacheAccess{cache,clock}))}
    fn new_inner(context:&mut CollisionContext<'_>,admitted:AdmittedContact,mut cache:Option<CacheAccess<'_>>)->Result<Self,&'static str>{
        let (system,groups,entry)=context.impact_groups(admitted.key)?;let (cores,_)=context.impact_endpoints(admitted.key)?;
        let mut wave=Self{loops:0,changed:vec![],backed:vec![],pairs:vec![],saved:admitted.saved.into_iter().map(|s|(s.body_id(),s)).collect(),groups,system};
        for side in 0..2{
            if context.bodies()?[cores[side]].flags&0x12==0{wave.mark(context,cores[side],entry,&mut cache)?;}
            let core=wave.groups[entry].cores[side];if context.bodies()?[core].flags&0x12==0{wave.backed.push(core);}
        }
        wave.pairs.push(entry);
        let other=wave.groups[entry].contacts.iter().copied().filter(|&key|key!=admitted.key as usize).collect::<Vec<_>>();let retired=refresh(context,&other,&mut cache)?;wave.discard(&retired);
        Ok(wave)
    }
    fn mark(&mut self,context:&mut CollisionContext<'_>,core:usize,except:usize,cache:&mut Option<CacheAccess<'_>>)->Result<(),&'static str>{
        if !self.saved.contains_key(&core){return Err("changed wave core has no staged backup");}
        if self.changed.contains(&core){return Err("duplicate changed wave core");}self.changed.push(core);
        for pair in (0..self.groups.len()).rev(){
            if pair==except||!self.groups[pair].cores.contains(&core)||self.pairs.contains(&pair){continue;}
            let retired=refresh(context,&self.groups[pair].contacts,cache)?;self.discard(&retired);
            if !self.groups[pair].contacts.is_empty(){self.pairs.push(pair);}
        }Ok(())
    }
    /// Native refresh can delete features and their empty pair group. Keep
    /// stable traversal indices, but never score a retired contact or group.
    fn discard(&mut self,retired:&[usize]){
        for group in &mut self.groups{group.contacts.retain(|id|!retired.contains(id));}
        self.pairs.retain(|&id|!self.groups[id].contacts.is_empty());
    }
    /// One native10103c10 query/solve, before the caller increments loops.
    /// Reverse pair/contact order and strict comparison preserve equal-score ties.
    pub(crate) fn step(&mut self,context:&mut CollisionContext<'_>)->Result<Option<u64>,&'static str>{
        self.step_inner(context,None)
    }
    pub(crate) fn step_cached(&mut self,context:&mut CollisionContext<'_>,cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context)->Result<Option<u64>,&'static str>{self.step_inner(context,Some(CacheAccess{cache,clock}))}
    fn step_inner(&mut self,context:&mut CollisionContext<'_>,mut cache:Option<CacheAccess<'_>>)->Result<Option<u64>,&'static str>{
        let mut best=f32::from_bits(1003486810);let mut selected=None;
        for &pair in self.pairs.iter().rev(){
            let bodies=context.bodies()?;let flags=self.groups[pair].cores.map(|id|bodies[id].flags as u8);
            if (((flags[0]>>5)|(flags[0]&2))&((flags[1]>>5)|(flags[1]&2))&6)!=0{continue;}
            for &contact in self.groups[pair].contacts.iter().rev(){let score=context.impact_score(contact as u64)?;
                if score<best{best=score;selected=Some((pair,contact as u64));}
            }
        }
        let Some((pair,key))=selected else{return Ok(None);};let (cores,active)=context.impact_endpoints(key)?;
        for side in (0..2).rev(){let core=cores[side];if active[side]&&!self.saved.contains_key(&core){
            let backup=context.stage_impact_core(core)?;self.saved.insert(core,backup);self.backed.push(core);
        }}
        context.next_impact_push(key)?;let response=context.primary_wave_impact(key)?;
        for core in cores.into_iter().rev(){
            if response.deferred_core==Some(core)||context.bodies()?[core].flags&0x12!=0{continue;}
            if !self.saved.contains_key(&core)||self.changed.contains(&core){context.invalidate_impact_core(self.system,core)?;}
            else{self.mark(context,core,pair,&mut cache)?;}
        }Ok(Some(key))
    }
    pub(crate) fn continue_iteration(&mut self)->Result<(),&'static str>{self.loops+=1;if self.loops>5000{return Err("impact wave anomaly callback is not connected");}Ok(())}
    /// Physical portion of10104320. Bounds dispatch and pair rechecks follow
    /// this phase; callers must not treat this alone as a completed collision.
    pub(crate) fn rebuild(mut self,context:&mut CollisionContext<'_>)->Result<Vec<usize>,&'static str>{
        for &id in self.backed.iter().rev(){
            let saved=self.saved.remove(&id).ok_or("impact backup ownership mismatch")?;
            if !self.changed.contains(&id){saved.restore(&mut context.bodies_mut()?[id])?;}
        }
        if !self.saved.is_empty(){return Err("unconsumed impact backups");}
        for &id in self.changed.iter().rev(){context.rebuild_impact_core(id)?;context.invalidate_impact_core(self.system,id)?;}
        Ok(self.changed)
    }
    #[cfg(test)]
    pub(crate) fn snapshot(&self,result:i32)->serde_json::Value{serde_json::json!({"loops":self.loops,"result":result,"changed":self.changed,"backed":self.backed,"pairs":self.pairs.iter().map(|&id|self.groups[id].cores).collect::<Vec<_>>()})}
}
#[cfg(test)]mod tests{
 use super::*;
 use bevy::prelude::*;
 fn features(cores:[usize;2])->[crate::ivp_managed_response::Feature;2]{cores.map(|id|crate::ivp_managed_response::Feature{material_code:0,endpoint:crate::ivp_create::Endpoint{object:id as u64+1,feature:crate::ivp_create::Feature{kind:3,edge:0,opposite:0,triangle:0,ledge:0,vertex:0,vertices:[Vec3::ZERO;3]}},vertices:[Vec3::ZERO;3],canonical:[Vec3::ZERO;3]})}
 #[test]
 fn native_neighbor_waves_select_and_solve_over_the_source_body_bank(){
  let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-neighbor.json")).unwrap();let mut queries=0;let mut solves=0;
  let bank=|b:&[crate::ivp_joint_apply::Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());
  for c in data["cases"].as_array().unwrap(){
   let dt=1./60.;let mut world=crate::ivp_registered_contacts::ContactWorld::empty_native(dt,c["initial_seed"].as_u64().unwrap() as u32,Vec3::new(0.,-0.,0.),Vec3::ZERO).unwrap();
   let n=c["count"].as_u64().unwrap() as usize;let spacing=c["spacing"].as_f64().unwrap() as f32;
   for (id,p) in c["body_inputs"].as_array().unwrap().iter().enumerate(){
    world.create_sphere(id as u64+1,crate::ivp_sphere_constructor::Parameters{radius:16.1,position:[spacing*id as f32,0.,128.],angles:[0.;3],mass:p["mass"].as_f64().unwrap() as f32,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:p["fixed"].as_bool().unwrap(),collisions:false},0.8).unwrap();
    let v=|name:&str|Vec3::from_array(std::array::from_fn(|i|p[name][i].as_f64().unwrap() as f32));world.set_velocity(id,Some(v("linear")),Some(v("angular"))).unwrap();world.configure_sphere_elasticity(id,0.3).unwrap();
   }
   world.advance(dt).unwrap();assert_eq!(bank(world.bodies()),c["before"]["physical"],"constructed {}",c["scenario"]);
   for j in 1..n-1{world.resolve(0xc0000,features([j,j+1]),Some(j)).unwrap();}
   assert_eq!(bank(world.bodies()),c["phases"][0]["state"]["physical"],"neighbours {}",c["scenario"]);
   let before=crate::ivp_registered_contacts::snapshot(&world);
   let result=world.inspect_collision(|context|{
    let radius=16.1*f32::from_bits(0x3cd013a9);let admitted=context.admit_spheres(0xc0000,[0,1],[radius;2])?;
    assert_eq!(bank(context.bodies()?),c["phases"][1]["state"]["physical"],"entry {}",c["scenario"]);
    context.primary_impact(admitted.key)?;assert_eq!(bank(context.bodies()?),c["phases"][2]["state"]["physical"],"primary {}",c["scenario"]);
    let mut wave=Wave::new(context,admitted)?;assert_eq!(wave.snapshot(-1),c["phases"][3]["wave"]);assert_eq!(bank(context.bodies()?),c["phases"][3]["state"]["physical"],"wave init {}",c["scenario"]);
    for phase in c["phases"].as_array().unwrap().iter().filter(|p|p["phase"]=="wave_step"){
     let selected=wave.step(context)?;queries+=1;solves+=usize::from(selected.is_some());let code=i32::from(selected.is_some());
     assert_eq!(wave.snapshot(code),phase["wave"],"wave metadata {}",c["scenario"]);assert_eq!(bank(context.bodies()?),phase["state"]["physical"],"wave step {} query{}",c["scenario"],wave.loops);
     let counts=context.impact_counts().values().copied().collect::<Vec<_>>();assert_eq!(serde_json::json!(counts),phase["state"]["impact_counts"]);
     if selected.is_some(){wave.continue_iteration()?;}
    }
    wave.rebuild(context)?;
    let responded=c["phases"].as_array().unwrap().iter().find(|p|p["phase"]=="responded").unwrap();
    assert_eq!(bank(context.bodies()?),responded["state"]["physical"],"rebuilt {}",c["scenario"]);
    Err("observed coupled rebuilding before bounds dispatch")
   });
   assert_eq!(result,Err("observed coupled rebuilding before bounds dispatch"));assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before);
  }
  assert_eq!(queries,58);assert_eq!(solves,46);
 }
}
