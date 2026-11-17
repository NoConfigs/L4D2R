//! Composed shared-group membership and simulation-unit/controller registration.
//! Initial core bindings are inputs; physical kernels and PSI execution remain
//! the caller's responsibility. Metadata edits commit transactionally.
use crate::{ivp_group_members::{Members,Object,Constraint},ivp_unit_registry::Registry};
use std::collections::BTreeMap;
#[derive(Clone,Debug)]
pub(crate) struct Groups { registry:Registry, objects:Vec<Object>, groups:BTreeMap<usize,Members> }
impl Groups {
    pub(crate) fn new(anchor:usize,objects:&[Object])->Result<Self,&'static str> {
        let mut registry=Registry::new(anchor);let mut ids=std::collections::BTreeSet::new();
        for object in objects {if !ids.insert(object.id){return Err("duplicate registered object identity");}registry.register_core(object.core,object.fixed)?;}
        Ok(Self{registry,objects:objects.to_vec(),groups:BTreeMap::new()})
    }
    pub(crate) fn registry(&self)->&Registry {&self.registry}
    /// Consume the wrapper when a contact-capable owner takes over the one
    /// canonical registry. Group metadata carries no second registry copy.
    pub(crate) fn into_parts(self)->(Registry,Vec<Object>,BTreeMap<usize,Members>){(self.registry,self.objects,self.groups)}
    /// Queue and numerical impulse changes share the same registered owner.
    pub(crate) fn apply_impulses(&mut self,bodies:&mut[crate::ivp_joint_apply::Body],commands:&[crate::ivp_joint_tail::Command])->Result<(),&'static str>{
        let bindings=bodies.iter().map(|b|crate::ivp_impulse_wake::Binding{body:b.id,current:b.id,real:b.id}).collect::<Vec<_>>();
        crate::ivp_impulse_wake::apply_with(bodies,&bindings,&mut self.registry,commands,|_,_,_,_,_|{})
    }
    pub(crate) fn take_effects(&mut self)->Vec<crate::ivp_unit_registry::Effect> {self.registry.take_effects()}
    /// Caller supplies physical controller execution and movement inputs. A
    /// containing scheduler owns transactional clock/history/poll rollback.
    pub(crate) fn psi_with(&mut self,poll:&mut crate::ivp_movement::Poll,
        mut effects:impl FnMut(Vec<crate::ivp_unit_registry::Effect>)->Result<(),&'static str>,
        execute:impl FnMut(crate::ivp_unit_registry::ControllerKey,&[usize])->Result<(),&'static str>,
        classify:impl FnMut(usize)->Result<u8,&'static str>)->Result<(),&'static str> {
        self.registry.begin_psi()?;effects(self.registry.take_effects())?;
        self.registry.finish_psi_units_with(poll,execute,classify)?;
        effects(self.registry.take_effects())
    }
    pub(crate) fn wake(&mut self,id:usize)->Result<(),&'static str>{self.registry.wake(id)}
    pub(crate) fn psi_prepared_with(&mut self,poll:&mut crate::ivp_movement::Poll,
        effects:impl FnMut(Vec<crate::ivp_unit_registry::Effect>)->Result<(),&'static str>,
        prepare:impl FnMut(&[usize],&mut crate::ivp_unit::UnitFlags)->Result<(),&'static str>,
        mut execute:impl FnMut(crate::ivp_unit_registry::ControllerKey,&[usize])->Result<(),&'static str>,
        classify:impl FnMut(usize)->Result<u8,&'static str>)->Result<(),&'static str>{
        self.psi_prepared_ordered_with(poll,effects,prepare,|key,cores,_|execute(key,cores),classify)
    }
    /// Numerical dispatch consumes the same prepared metadata vector. Class0
    /// reordering must move cached bindings with their constraint identities.
    pub(crate) fn psi_prepared_ordered_with(&mut self,poll:&mut crate::ivp_movement::Poll,
        mut effects:impl FnMut(Vec<crate::ivp_unit_registry::Effect>)->Result<(),&'static str>,
        prepare:impl FnMut(&[usize],&mut crate::ivp_unit::UnitFlags)->Result<(),&'static str>,
        mut execute:impl FnMut(crate::ivp_unit_registry::ControllerKey,&[usize],Option<&[crate::ivp_constraint_order::Entry]>)->Result<(),&'static str>,
        classify:impl FnMut(usize)->Result<u8,&'static str>)->Result<(),&'static str>{
        self.registry.begin_psi()?;effects(self.registry.take_effects())?;
        let groups=&mut self.groups;
        self.registry.finish_prepared_units_with(poll,prepare,|key,cores|{
            let order=if let crate::ivp_unit_registry::ControllerKey::Joint(id)=key{let group=groups.get_mut(&id).ok_or("missing prepared group")?;group.prepare_order();Some(group.order().entries())}else{None};
            execute(key,cores,order)
        },classify)?;
        effects(self.registry.take_effects())
    }
    pub(crate) fn group(&self,id:usize)->Option<&Members> {self.groups.get(&id)}
    pub(crate) fn create(&mut self,id:usize)->Result<(),&'static str> {
        if self.groups.contains_key(&id){return Err("duplicate registered group");}
        let mut members=Members::default();for &object in &self.objects {members.register_object(object)?;}
        self.groups.insert(id,members);Ok(())
    }
    pub(crate) fn insert(&mut self,id:usize,constraint:Constraint)->Result<(),&'static str> {
        if self.groups.values().any(|m|m.order().entries().iter().any(|e|e.id==constraint.entry.id)){return Err("duplicate registered constraint");}
        let active=self.groups.get(&id).ok_or("unknown registered group")?.active();let mut next=self.clone();
        if active{next.registry.unregister_group(id,false)?;}
        let members=next.groups.get_mut(&id).unwrap();members.insert_constraint(constraint)?;
        if active{next.registry.register_group(id,members.cores())?;}
        *self=next;Ok(())
    }
    /// Kernel destruction removes its listener/order entries but retains the
    /// group's object/core membership and registered controller, even when empty.
    pub(crate) fn remove_constraint(&mut self,id:usize,constraint:usize)->Result<(),&'static str>{
        self.groups.get_mut(&id).ok_or("unknown registered group")?.remove_constraint(constraint)?;
        Ok(())
    }
    pub(crate) fn activate(&mut self,id:usize)->Result<(),&'static str> {
        let m=self.groups.get(&id).ok_or("unknown registered group")?;
        if m.active()||m.objects().is_empty(){return Ok(());}
        let mut next=self.clone();let m=next.groups.get_mut(&id).unwrap();m.activate()?;
        next.registry.register_group(id,m.cores())?;*self=next;Ok(())
    }
    pub(crate) fn remove(&mut self,id:usize)->Result<(),&'static str> {
        let active=self.groups.get(&id).ok_or("unknown registered group")?.active();let mut next=self.clone();
        if active{next.registry.unregister_group(id,true)?;}
        next.groups.remove(&id);*self=next;Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ivp_constraint_order::Entry;
    use serde_json::{Value,json};
    fn fixed(s:usize,id:usize)->bool {match s%4{1=>id==0||id==6,2=>id%2==1,3=>id%2==0,_=>false}}
    fn members(m:Option<&Members>)->Value {m.map_or(Value::Null,|m|json!({"active":u8::from(m.active()),"dirty":u8::from(m.order().dirty()),
        "order":m.order().entries().iter().map(|e|e.id).collect::<Vec<_>>(),"objects":m.objects(),"cores":m.cores()}))}
    #[test]
    fn native_shared_groups_own_core_unit_controller_vectors_across_edits_and_removal() {
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-shared-group-registry.json")).unwrap();let mut states=0;
        for case in data["cases"].as_array().unwrap() {
            let s=case["scenario"].as_u64().unwrap() as usize;
            let objects=(0..7).map(|id|Object{id,core:id,fixed:fixed(s,id)}).collect::<Vec<_>>();
            let mut owner=Groups::new(7,&objects).unwrap();owner.create(0).unwrap();owner.create(1).unwrap();
            for op in case["states"].as_array().unwrap() {
                let group=op["group"].as_i64().unwrap();match op["action"].as_str().unwrap() {
                    "created"=>{},"add"=>{let id=op["joint"].as_u64().unwrap() as usize;
                        let objects=std::array::from_fn(|i|op["objects"][i].as_u64().unwrap() as usize);
                        owner.insert(group as usize,Constraint{entry:Entry{id,class:3},objects}).unwrap();},
                    "activate"=>owner.activate(group as usize).unwrap(),"destroy_group"=>owner.remove(group as usize).unwrap(),_=>panic!("unknown authored operation"),
                }
                assert_eq!(owner.registry().snapshot(),op["registry"],"group scenario{s}/state{states}/registry");
                assert_eq!(owner.registry().controller_snapshot(),op["controllers"],"group scenario{s}/state{states}/controllers");
                assert_eq!(json!([members(owner.group(0)),members(owner.group(1))]),op["members"],"group scenario{s}/state{states}/members");states+=1;
            }
        }
        assert_eq!(states,896);
    }
    #[test]
    fn invalid_registered_group_edit_cannot_partially_unregister_an_active_controller() {
        let objects=[Object{id:0,core:0,fixed:false},Object{id:1,core:1,fixed:false}];
        let mut owner=Groups::new(7,&objects).unwrap();owner.create(0).unwrap();owner.insert(0,Constraint{entry:Entry{id:3,class:3},objects:[0,1]}).unwrap();owner.activate(0).unwrap();
        let before=format!("{owner:?}");
        assert!(owner.insert(0,Constraint{entry:Entry{id:4,class:3},objects:[0,99]}).is_err());assert_eq!(format!("{owner:?}"),before);
        assert!(owner.insert(0,Constraint{entry:Entry{id:3,class:0},objects:[0,1]}).is_err());assert_eq!(format!("{owner:?}"),before);
        assert!(owner.remove(99).is_err());assert_eq!(format!("{owner:?}"),before);
    }
}
