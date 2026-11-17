//! Constraint-group object/core membership (100af1f0/100afa80), composed
//! with recovered constraint ordering and object-deletion notifications.
//! Controller/unit graph updates and numerical effects remain separate.
use crate::ivp_constraint_order::{Entry,Order};
use std::collections::BTreeMap;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct Object { pub id:usize, pub core:usize, pub fixed:bool }
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct Constraint { pub entry:Entry, pub objects:[usize;2] }
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum DeletionEvent { Constraint(usize), Membership(usize) }
#[derive(Clone,Debug,Default,PartialEq,Eq)]
pub(crate) struct Members {
    order:Order, definitions:BTreeMap<usize,Object>, constraints:BTreeMap<usize,Constraint>,
    objects:Vec<usize>, cores:Vec<usize>, active:bool, listeners:BTreeMap<usize,Vec<usize>>,
}
impl Members {
    pub(crate) fn prepare_order(&mut self){self.order.prepare_solve();}
    pub(crate) fn order(&self)->&Order { &self.order }
    pub(crate) fn objects(&self)->&[usize] { &self.objects }
    pub(crate) fn cores(&self)->&[usize] { &self.cores }
    pub(crate) fn active(&self)->bool { self.active }
    pub(crate) fn activate(&mut self)->Result<(),&'static str> {
        if !self.active && !self.objects.is_empty() {
            if self.objects.iter().any(|id|!self.definitions.contains_key(id)) {
                return Err("activation with destroyed retained identities requires group teardown");
            }
            self.active=true;
        }
        Ok(())
    }
    pub(crate) fn register_object(&mut self,object:Object)->Result<(),&'static str> {
        if self.definitions.contains_key(&object.id)||self.objects.contains(&object.id) { return Err("duplicate group object binding"); }
        self.definitions.insert(object.id,object);self.listeners.insert(object.id,vec![]);Ok(())
    }
    pub(crate) fn insert_constraint(&mut self,constraint:Constraint)->Result<(),&'static str> {
        if constraint.objects.iter().any(|id|!self.definitions.contains_key(id)) { return Err("unknown constraint object binding"); }
        self.order.insert(constraint.entry)?;
        // The native kernel's object+8 precedes object+4. Object identity,
        // rather than core identity, controls membership deduplication.
        for id in constraint.objects {
            self.listeners.get_mut(&id).unwrap().push(constraint.entry.id);
            if !self.objects.contains(&id) {
                self.objects.push(id);let object=self.definitions[&id];
                if !object.fixed { self.cores.push(object.core); }
            }
        }
        self.constraints.insert(constraint.entry.id,constraint);Ok(())
    }
    pub(crate) fn remove_constraint(&mut self,id:usize)->Result<Constraint,&'static str> {
        if !self.constraints.contains_key(&id) { return Err("unknown group constraint binding"); }
        self.order.remove(id)?;
        for object in self.constraints[&id].objects {
            let listeners=self.listeners.get_mut(&object).unwrap();
            let at=listeners.iter().rposition(|&j|j==id).unwrap();listeners.remove(at);
        }
        // Native constraint destruction retains object/core membership but
        // stable-removes its registration from both object listener vectors.
        Ok(self.constraints.remove(&id).unwrap())
    }
    pub(crate) fn destroy_unconstrained_object(&mut self,id:usize)->Result<(),&'static str> {
        if self.constraints.values().any(|c|c.objects.contains(&id)) { return Err("connected object destruction requires constraint teardown"); }
        self.destroy_object(id)
    }
    pub(crate) fn destroy_object(&mut self,id:usize)->Result<(),&'static str> {
        self.destroy_object_with(id,|_,_|{})
    }
    /// Observers run at each notification entry, before its vector mutation.
    /// Physical/controller side effects beyond these entries remain external.
    pub(crate) fn destroy_object_with(&mut self,id:usize,mut observe:impl FnMut(DeletionEvent,&Self))->Result<(),&'static str> {
        let object=*self.definitions.get(&id).ok_or("unknown group object binding")?;
        let at=self.objects.iter().rposition(|&x|x==id);
        let core_at=if self.active&&!object.fixed&&at.is_some() {
            Some(self.cores.iter().rposition(|&x|x==object.core).ok_or("missing group dynamic core")?)
        } else { None };
        let mut next=self.clone();
        // Consume the object's listeners backwards. Solver order may differ
        // after native class-zero reorder; it must not drive notifications.
        while let Some(&joint)=next.listeners[&id].last() {
            observe(DeletionEvent::Constraint(joint),&next);next.remove_constraint(joint)?;
        }
        // The group controller is notified only for an actively owned dynamic
        // core. Static and inactive identities remain retained until teardown.
        if let Some(core_at)=core_at {
            observe(DeletionEvent::Membership(id),&next);next.objects.swap_remove(at.unwrap());next.cores.remove(core_at);
        }
        next.definitions.remove(&id);next.listeners.remove(&id);*self=next;Ok(())
    }
    pub(crate) fn reorder(&mut self) { self.order.reorder(); }
    pub(crate) fn prepare_solve(&mut self) { self.order.prepare_solve(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn data()->Value { serde_json::from_str(include_str!("../tools/fixtures/native-ivp-group-members.json")).unwrap() }
    fn fixed(s:usize,id:usize)->bool { match s%3 {1=>id==0||id==6,2=>id%2==1,_=>false} }
    fn check(m:&Members,e:&Value) {
        let ids=|name:&str|e[name].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();
        assert_eq!(m.objects(),ids("objects"));assert_eq!(m.cores(),ids("cores"));
        assert_eq!(m.order().dirty(),e["dirty"].as_u64().unwrap()!=0);
        assert_eq!(m.order().entries().iter().map(|e|e.id).collect::<Vec<_>>(),ids("order"));
        assert_eq!(m.order().entries().iter().map(|e|e.class).collect::<Vec<_>>(),e["classes"].as_array().unwrap().iter().map(|v|v.as_i64().unwrap() as i32).collect::<Vec<_>>());
    }
    fn joint(id:usize,class:i32)->Constraint { Constraint {entry:Entry{id,class},objects:[id,id+1]} }
    #[test]
    fn native_membership_retains_objects_after_joint_teardown_and_matches_object_destruction() {
        let data=data();let mut snapshots=0;let mut destroys=0;
        for case in data["cases"].as_array().unwrap() {
            let s=case["scenario"].as_u64().unwrap() as usize;let mut m=Members::default();
            for id in 0..7 { m.register_object(Object{id,core:id,fixed:fixed(s,id)}).unwrap(); }
            for i in 0..6 { let id=if s%2==0{i}else{5-i};let class=if s&(1<<id)!=0{0}else if id%2==1{1}else{3};m.insert_constraint(joint(id,class)).unwrap(); }
            check(&m,&case["before"]);m.reorder();check(&m,&case["after"]);m.reorder();check(&m,&case["repeat"]);
            let id=s%6;m.remove_constraint(id).unwrap();check(&m,&case["removed"]);
            m.insert_constraint(joint(id,if s&(1<<id)!=0{3}else{0})).unwrap();check(&m,&case["added"]);
            m.reorder();check(&m,&case["resorted"]);m.activate().unwrap();m.prepare_solve();check(&m,&case["stepped"]);
            let id=(s+3)%6;m.remove_constraint(id).unwrap();check(&m,&case["clean_removed"]);
            m.insert_constraint(joint(id,if s&(1<<id)!=0{3}else{0})).unwrap();check(&m,&case["clean_added"]);
            m.prepare_solve();check(&m,&case["final"]);
            for id in 0..6 { m.remove_constraint(id).unwrap(); }check(&m,&case["empty_constraints"]);snapshots+=11;
            for i in 0..7 { let id=(i+s)%7;assert_eq!(case["destroy_objects"][i]["id"].as_u64(),Some(id as u64));
                m.destroy_unconstrained_object(id).unwrap();check(&m,&case["destroy_objects"][i]["state"]);snapshots+=1;destroys+=1;
            }
            assert!(m.order().entries().is_empty()&&m.cores().is_empty());
            assert!(m.objects().iter().all(|&id|fixed(s,id)));
        }
        assert_eq!((snapshots,destroys),(1152,448));
    }
    #[test]
    fn invalid_membership_changes_and_connected_object_destruction_are_atomic() {
        let mut m=Members::default();for id in 0..2 {m.register_object(Object{id,core:id,fixed:false}).unwrap();}
        let before=m.clone();assert!(m.insert_constraint(joint(1,3)).is_err());assert_eq!(m,before);
        m.insert_constraint(joint(0,3)).unwrap();m.prepare_solve();let before=m.clone();
        assert!(m.destroy_unconstrained_object(0).is_err());assert_eq!(m,before);
        assert!(m.insert_constraint(joint(0,0)).is_err());assert_eq!(m,before);
        assert!(m.remove_constraint(99).is_err());assert_eq!(m,before);
        assert!(m.register_object(Object{id:0,core:99,fixed:true}).is_err());assert_eq!(m,before);
    }
    #[test]
    fn native_live_constraint_object_deletion_matches_active_and_inactive_groups() {
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-group-destroy.json")).unwrap();
        let mut snapshots=0;let mut destroyed=0;
        let compare=|m:&Members,e:&Value|{check(m,e);assert_eq!(m.active(),e["active"].as_u64().unwrap()!=0);};
        for case in data["cases"].as_array().unwrap() {
            let s=case["scenario"].as_u64().unwrap() as usize;let mut m=Members::default();
            m.activate().unwrap();compare(&m,&case["empty"]);snapshots+=1;
            for id in 0..7 {m.register_object(Object{id,core:id,fixed:fixed(s,id)}).unwrap();}
            for i in 0..6 {let id=if s%2==0{i}else{5-i};let class=if s&(1<<id)!=0{0}else if id%2==1{1}else{3};m.insert_constraint(joint(id,class)).unwrap();}
            if s%2==1 {m.activate().unwrap();m.prepare_solve();}
            compare(&m,&case["before"]);snapshots+=1;
            for i in 0..7 {let id=(i*3+s)%7;assert_eq!(case["destroy_objects"][i]["id"].as_u64(),Some(id as u64));
                m.destroy_object(id).unwrap();compare(&m,&case["destroy_objects"][i]["state"]);snapshots+=1;destroyed+=1;
            }
            assert!(m.order().entries().is_empty());
        }
        assert_eq!((snapshots,destroyed),(576,448));
    }
    #[test]
    fn destroyed_retained_identity_cannot_be_reused_or_reactivated_as_a_live_object() {
        let mut m=Members::default();for id in 0..2 {m.register_object(Object{id,core:id,fixed:false}).unwrap();}
        m.insert_constraint(joint(0,3)).unwrap();m.destroy_object(0).unwrap();
        let before=m.clone();assert!(m.activate().is_err());assert_eq!(m,before);
        assert!(m.register_object(Object{id:0,core:77,fixed:false}).is_err());assert_eq!(m,before);
        assert!(m.destroy_object(0).is_err());assert_eq!(m,before);
    }
    #[test]
    fn native_object_listener_notifications_match_reverse_stable_registration_across_four_graphs() {
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-group-destroy-events.json")).unwrap();
        let edge=|s:usize,j:usize|match s/64 {0=>[j,j+1],1=>[0,j+1],2=>[j,(j+1)%6],_=>[[0,1],[0,2],[0,3],[1,4],[1,5],[1,6]][j]};
        let mut snapshots=0;let mut events=0;
        for case in data["cases"].as_array().unwrap() {
            let s=case["scenario"].as_u64().unwrap() as usize;let mut m=Members::default();
            let compare=|m:&Members,e:&Value|{check(m,e);assert_eq!(m.active(),e["active"].as_u64().unwrap()!=0);};
            m.activate().unwrap();compare(&m,&case["empty"]);snapshots+=1;
            for id in 0..7 {m.register_object(Object{id,core:id,fixed:fixed(s,id)}).unwrap();}
            for i in 0..6 {let id=if s%2==0{i}else{5-i};let class=if s&(1<<id)!=0{0}else if id%2==1{1}else{3};
                m.insert_constraint(Constraint{entry:Entry{id,class},objects:edge(s,id)}).unwrap();}
            if s%2==1 {m.activate().unwrap();m.prepare_solve();}
            compare(&m,&case["before"]);snapshots+=1;
            for i in 0..7 {let id=(i*3+s)%7;let mut actual=vec![];
                m.destroy_object_with(id,|event,before|{
                    match event {
                        DeletionEvent::Constraint(j)=>{assert!(before.order().entries().iter().any(|e|e.id==j));actual.push([0,j]);},
                        DeletionEvent::Membership(o)=>{assert!(before.objects().contains(&o));actual.push([1,o]);},
                    }
                }).unwrap();
                let op=&case["destroy_objects"][i];assert_eq!(serde_json::json!(actual),op["events"],"group{s}/object{id}");events+=actual.len();
                compare(&m,&op["state"]);snapshots+=1;
            }
        }
        assert_eq!((snapshots,events),(2304,2198));
    }
}
