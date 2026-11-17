//!101055e0 feature gate/lookup/construction composed with10104010 admission.
//! Owns object-side links from empty state. Geometry/material response and
//! surface/cache callback providers are still separate numerical/lifetime work.
use std::collections::BTreeMap;
use crate::{ivp_create::{self as create,Endpoint,Candidate,Initial},ivp_contact_system::Contacts,ivp_unit_registry::Registry};
#[derive(Clone,Copy,Debug)]
pub(crate) struct Settings{pub distance:f32,pub area_epsilon:f64}
#[derive(Clone,Debug)]
pub(crate) struct Managed{
    owner:Contacts,objects:BTreeMap<u64,usize>,lists:BTreeMap<u64,Vec<Candidate>>,
    features:BTreeMap<u64,[Endpoint;2]>,next_key:u64,settings:Settings,released_features:Vec<[Endpoint;2]>,
}
#[derive(Debug)]
pub(crate) struct Admission{pub key:Option<u64>,pub created:bool,pub system:Option<usize>,pub initial:Option<Initial>}
impl Managed{
    pub(crate) fn new(registry:Registry,fixed:BTreeMap<usize,bool>,objects:BTreeMap<u64,usize>,settings:Settings)->Result<Self,&'static str>{
        if objects.contains_key(&0)||objects.values().any(|c|!fixed.contains_key(c))||!settings.distance.is_finite()||!settings.area_epsilon.is_finite()||settings.area_epsilon<=0.{return Err("invalid managed contact environment");}
        let lists=objects.keys().map(|&id|(id,vec![])).collect();
        Ok(Self{owner:Contacts::empty(registry,fixed)?,objects,lists,features:BTreeMap::new(),next_key:1,settings,released_features:vec![]})
    }
    pub(crate) fn resolve(&mut self,flags:u32,time:f64,endpoints:[Endpoint;2],hint:Option<usize>)->Result<Admission,&'static str>{
        if ((flags>>8)&3)>1||!time.is_finite()||endpoints[0].object==endpoints[1].object||endpoints.iter().any(|e|!self.objects.contains_key(&e.object)||!(0..=3).contains(&e.feature.kind)||e.feature.kind!=3&&(e.feature.edge==0||e.feature.triangle==0||e.feature.ledge==0)||e.feature.vertices.iter().any(|v|!v.is_finite())){return Err("invalid managed contact candidate");}
        let input=create::Input{flags,time,endpoints:endpoints.clone(),lists:std::array::from_fn(|i|self.lists[&endpoints[i].object].clone())};
        let result=create::resolve(&input,Some(self.next_key),self.settings.distance,self.settings.area_epsilon)?;
        let mut next=self.clone();let system;
        if result.created{
            let key=result.key.ok_or("managed contact allocation failed")?;let new=result.endpoints.as_ref().ok_or("missing constructed contact endpoints")?;
            let cores=new.each_ref().map(|e|next.objects[&e.object]);
            system=Some(next.owner.admit(usize::try_from(key).map_err(|_|"contact identity width overflow")?,cores,hint)?);
            next.next_key=next.next_key.checked_add(1).ok_or("managed contact namespace exhausted")?;
            for i in 0..2{next.lists.insert(endpoints[i].object,result.lists[i].clone());}
            next.features.insert(key,new.clone());
        }else if let Some(key)=result.key{system=Some(next.owner.binding(usize::try_from(key).map_err(|_|"contact identity width overflow")?)?.system);}
        else{
            let selected=((flags>>8)&3) as usize;if selected>1{return Err("unsupported candidate endpoint selector");}
            system=next.owner.candidate_system([next.objects[&endpoints[selected].object],next.objects[&endpoints[selected^1].object]])?;
        }
        let out=Admission{key:result.key,created:result.created,system,initial:result.initial};*self=next;Ok(out)
    }
    pub(crate) fn remove(&mut self,key:u64)->Result<(),&'static str>{
        let endpoints=self.features.get(&key).ok_or("unknown managed feature contact")?.clone();let mut next=self.clone();
        next.owner.remove(usize::try_from(key).map_err(|_|"contact identity width overflow")?)?;
        //100fcf00 releases both shape features and notifies providers before
        // unlinking the two object-side nodes. Retain release operands for the
        // resource owner; these queues do not pretend callbacks were executed.
        next.released_features.push(endpoints.clone());
        for (side,e) in endpoints.iter().enumerate(){let list=next.lists.get_mut(&e.object).unwrap();let at=list.iter().position(|c|c.key==key&&c.side==side).ok_or("missing object-side managed contact")?;list.remove(at);}
        next.features.remove(&key);*self=next;Ok(())
    }
    pub(crate) fn take_released_features(&mut self)->Vec<[Endpoint;2]>{std::mem::take(&mut self.released_features)}
    /// Drain the canonical registry's physical clock effects together with
    /// the contact and shape identities needed by the resource owner.
    pub(crate) fn take_effects(&mut self)->(Vec<crate::ivp_unit_registry::Effect>,Vec<usize>,Vec<[Endpoint;2]>){let(effects,contacts)=self.owner.take_effects();(effects,contacts,self.take_released_features())}
    pub(crate) fn destroy_empty(&mut self,id:usize)->Result<(),&'static str>{self.owner.destroy_empty(id)}
    pub(crate) fn subscribe(&mut self,id:usize,flags:u32)->Result<(),&'static str>{self.owner.subscribe(id,flags)}
    pub(crate) fn unsubscribe(&mut self,id:usize)->Result<(),&'static str>{self.owner.unsubscribe(id)}
    pub(crate) fn set_listener_flags(&mut self,id:usize,flags:u32)->Result<(),&'static str>{self.owner.set_listener_flags(id,flags)}
    pub(crate) fn take_notifications(&mut self)->Vec<crate::ivp_contact_notifications::Delivery>{self.owner.take_notifications()}
    pub(crate) fn reorder(&mut self,system:usize,order:&[usize])->Result<(),&'static str>{self.owner.reorder(system,order)}
    pub(crate) fn set_pair_clock(&mut self,id:usize,index:usize,cores:[usize;2],clock:crate::ivp_contact_pipeline::FrictionGroup)->Result<(),&'static str>{self.owner.set_pair_clock(id,index,cores,clock)}
    pub(crate) fn repair_system(&mut self,system:usize)->Result<Vec<usize>,&'static str>{self.owner.repair_system(system)}
    pub(crate) fn finish_unit_graph(&mut self,core:usize)->Result<(),&'static str>{self.owner.finish_unit_graph(core)}
    pub(crate) fn destroy_joint_registration(&mut self,id:usize)->Result<(),&'static str>{self.owner.destroy_joint_registration(id)}
    pub(crate) fn begin_psi(&mut self)->Result<(),&'static str>{self.owner.begin_psi()}
    pub(crate) fn next_unit_dispatch(&mut self,poll:&mut crate::ivp_movement::Poll,
        prepare:impl FnMut(&[usize],&mut crate::ivp_unit::UnitFlags)->Result<(),&'static str>)->Result<Option<crate::ivp_unit_registry::UnitDispatch>,&'static str>{self.owner.next_unit_dispatch(poll,prepare)}
    pub(crate) fn finish_unit_dispatch(&mut self,dispatch:crate::ivp_unit_registry::UnitDispatch,
        classify:impl FnMut(usize)->Result<u8,&'static str>)->Result<(),&'static str>{self.owner.finish_unit_dispatch(dispatch,classify)}
    pub(crate) fn apply_registered_impulses(&mut self,bodies:&mut[crate::ivp_joint_apply::Body],commands:&[crate::ivp_joint_tail::Command])->Result<(),&'static str>{self.owner.apply_registered_impulses(bodies,commands)}
    pub(crate) fn register_ordinary_object(&mut self,id:u64,core:usize,fixed:bool)->Result<(),&'static str>{
        if id==0||self.objects.contains_key(&id)||self.objects.values().any(|&c|c==core){return Err("invalid ordinary contact object identity");}
        self.owner.register_ordinary_core(core,fixed)?;self.objects.insert(id,core);self.lists.insert(id,vec![]);Ok(())
    }
    pub(crate) fn register_physical_group(&mut self,id:usize,cores:&[usize])->Result<(),&'static str>{self.owner.register_physical_group(id,cores)}
    pub(crate) fn rebind_registered_group(&mut self,id:usize,cores:&[usize])->Result<(),&'static str>{self.owner.rebind_registered_group(id,cores)}
    pub(crate) fn detach_registered_group(&mut self,id:usize)->Result<(),&'static str>{self.owner.detach_registered_group(id)}
    pub(crate) fn wake_registered_core(&mut self,id:usize)->Result<(),&'static str>{self.owner.wake_registered_core(id)}
    pub(crate) fn wake_registered_core_now(&mut self,id:usize)->Result<(),&'static str>{self.owner.wake_registered_core_now(id)}
    pub(crate) fn finish_registered_wake(&mut self,id:usize)->Result<(),&'static str>{self.owner.finish_registered_wake(id)}
    pub(crate) fn wake_registered_unit_now(&mut self,id:usize)->Result<(),&'static str>{self.owner.wake_registered_unit_now(id)}
    pub(crate) fn owner(&self)->&Contacts{&self.owner}
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn snapshot(&self)->serde_json::Value{
        let mut v=self.owner.snapshot(self.owner.system_slots());
        v["object_links"]=serde_json::json!(self.lists.values().map(|list|list.iter().map(|c|[c.key,c.side as u64]).collect::<Vec<_>>()).collect::<Vec<_>>());
        let r=self.owner.registry().snapshot();let mut units=r["active"].as_array().unwrap().iter().chain(r["dormant"].as_array().unwrap()).map(|u|u["cores"].as_array().unwrap().iter().filter_map(|c|{let id=c.as_u64().unwrap() as usize;self.objects.values().any(|&v|v==id).then_some(id)}).collect::<Vec<_>>()).filter(|u|!u.is_empty()).collect::<Vec<_>>();units.sort_by_key(|u|*u.iter().min().unwrap());v["unit_members"]=serde_json::json!(units);v
    }
}
