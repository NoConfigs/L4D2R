//! Persistent100ff200/100ff090 contact, pair-group and core-link ownership.
//! A boundary may be imported once; discovery, geometry and numeric controller
//! work are separate. Resource release requests still need their cache owner.
use std::collections::{BTreeMap,BTreeSet};
use crate::ivp_contact_notifications::{Notifications,Event,Kind,Delivery};
use crate::{ivp_contact_pipeline::FrictionGroup,ivp_unit_registry::Registry};
#[derive(Clone,Debug)]
pub(crate) struct Pair {pub cores:[usize;2],pub contacts:Vec<usize>,pub clock:FrictionGroup}
#[derive(Clone,Debug)]
pub(crate) struct System {pub cores:Vec<usize>,pub contacts:Vec<usize>,pub pairs:Vec<Pair>,pub dirty:bool}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Binding {pub system:usize,pub cores:[usize;2]}
#[derive(Clone,Debug)]
pub(crate) struct Contacts {
    registry:Registry,fixed:BTreeMap<usize,bool>,systems:BTreeMap<usize,System>,
    bindings:BTreeMap<usize,Binding>,links:BTreeMap<(usize,usize),Vec<usize>>,released:Vec<usize>,next_system:usize,notifications:Notifications,
}
fn unique(ids:&[usize])->bool{ids.iter().copied().collect::<BTreeSet<_>>().len()==ids.len()}
fn erase(ids:&mut Vec<usize>,id:usize)->Result<(),&'static str>{let i=ids.iter().rposition(|&v|v==id).ok_or("contact list lacks owned identity")?;ids.remove(i);Ok(())}
impl Contacts {
    /// The initial boundary includes retained pair clocks/list order; later
    /// expected metadata is never an execution input.
    pub(crate) fn from_boundary(registry:Registry,fixed:BTreeMap<usize,bool>,systems:BTreeMap<usize,System>,bindings:BTreeMap<usize,Binding>,links:BTreeMap<(usize,usize),Vec<usize>>)->Result<Self,&'static str>{
        let next_system=systems.keys().next_back().map(|id|id.checked_add(1).ok_or("contact system namespace exhausted")).transpose()?.unwrap_or(0);
        let mut out=Self{registry,fixed,systems,bindings,links,released:vec![],next_system,notifications:Notifications::default()};
        for (&core,&fixed) in &out.fixed{if out.registry.core_fixed(core)?!=fixed{return Err("contact fixed classification disagrees with registry");}}
        out.check()?;
        for (&id,s) in &out.systems{out.registry.create_contact_system(id)?;for &core in &s.cores{if out.registry.core_fixed(core)?!=out.fixed[&core]{return Err("contact fixed classification disagrees with registry");}out.registry.attach_contact_core(id,core)?;}}
        Ok(out)
    }
    pub(crate) fn empty(registry:Registry,fixed:BTreeMap<usize,bool>)->Result<Self,&'static str>{Self::from_boundary(registry,fixed,BTreeMap::new(),BTreeMap::new(),BTreeMap::new())}
    pub(crate) fn system_ids(&self)->Vec<usize>{self.systems.keys().copied().collect()}
    pub(crate) fn system_slots(&self)->usize{self.next_system}
    pub(crate) fn system_cores(&self,id:usize)->Result<&[usize],&'static str>{Ok(&self.systems.get(&id).ok_or("unknown contact system")?.cores)}
    pub(crate) fn system_dirty(&self,id:usize)->Result<bool,&'static str>{Ok(self.systems.get(&id).ok_or("unknown contact system")?.dirty)}
    pub(crate) fn reorder(&mut self,id:usize,order:&[usize])->Result<(),&'static str>{
        let s=self.systems.get_mut(&id).ok_or("unknown contact system")?;
        if !unique(order)||order.len()!=s.contacts.len()||order.iter().copied().collect::<BTreeSet<_>>()!=s.contacts.iter().copied().collect(){return Err("normal order changes contact membership");}
        s.contacts=order.to_vec();Ok(())
    }
    pub(crate) fn system_pairs(&self,id:usize)->Result<&[Pair],&'static str>{Ok(&self.systems.get(&id).ok_or("unknown contact system")?.pairs)}
    pub(crate) fn set_pair_clock(&mut self,id:usize,index:usize,cores:[usize;2],clock:FrictionGroup)->Result<(),&'static str>{
        let pair=self.systems.get_mut(&id).ok_or("unknown contact system")?.pairs.get_mut(index).ok_or("unknown contact pair")?;
        if pair.cores!=cores||!clock.energy.is_finite(){return Err("invalid/stale contact pair clock");}pair.clock=clock;Ok(())
    }
    pub(crate) fn system_contacts(&self,id:usize)->Result<&[usize],&'static str>{Ok(&self.systems.get(&id).ok_or("unknown contact system")?.contacts)}
    pub(crate) fn core_contacts(&self,system:usize,core:usize)->Result<&[usize],&'static str>{self.links.get(&(core,system)).map(Vec::as_slice).ok_or("unknown core contact links")}
    pub(crate) fn binding(&self,id:usize)->Result<Binding,&'static str>{self.bindings.get(&id).copied().ok_or("unknown managed contact")}
    pub(crate) fn first_system(&self,core:usize)->Option<usize>{self.links.range((core,0)..=(core,usize::MAX)).next().map(|(&(c,s),_)|{debug_assert_eq!(c,core);s})}
    pub(crate) fn candidate_system(&self,cores:[usize;2])->Result<Option<usize>,&'static str>{let first=if *self.fixed.get(&cores[0]).ok_or("unknown candidate core")?{cores[1]}else{cores[0]};if *self.fixed.get(&first).ok_or("unknown candidate core")?{return Err("static-only contact admission is unsupported");}Ok(self.first_system(first))}
    fn attach_empty_core(&mut self,system:usize,core:usize)->Result<(),&'static str>{self.registry.attach_contact_core(system,core)?;self.systems.get_mut(&system).unwrap().cores.push(core);self.links.insert((core,system),vec![]);Ok(())}
    ///10104010's newly created-contact branch. The feature owner performs
    /// gate/lookup first; this method owns system choice, allocation, core
    /// registration, system merges, pair insertion and dynamic unit merging.
    pub(crate) fn admit(&mut self,id:usize,cores:[usize;2],hint:Option<usize>)->Result<usize,&'static str>{
        if self.bindings.contains_key(&id)||cores[0]==cores[1]{return Err("invalid new contact identity/endpoints");}
        for core in cores{if !self.fixed.contains_key(&core){return Err("unknown admitted core");}}
        if let Some(hint)=hint{self.registry.core_fixed(hint)?;}
        let [first,other]=if self.fixed[&cores[0]]{[cores[1],cores[0]]}else{cores};
        if self.fixed[&first]{return Err("static-only contact admission is unsupported");}
        let mut next=self.clone();let system;
        next.notifications.publish(Event{kind:Kind::Created,contact:Some(id),cores});
        if let Some(existing)=next.first_system(first){
            system=existing;
            if !next.links.contains_key(&(other,system)){
                if next.fixed[&other]||next.first_system(other).is_none(){next.attach_empty_core(system,other)?;}
                else{next.merge(system,next.first_system(other).unwrap())?;}
            }
        }else if !next.fixed[&other]&&next.first_system(other).is_some(){system=next.first_system(other).unwrap();next.attach_empty_core(system,first)?;}
        else{
            system=next.next_system;next.next_system=system.checked_add(1).ok_or("contact system namespace exhausted")?;
            next.registry.create_contact_system(system)?;next.systems.insert(system,System{cores:vec![],contacts:vec![],pairs:vec![],dirty:false});
            next.attach_empty_core(system,first)?;next.attach_empty_core(system,other)?;
        }
        next.insert_pair_and_head(system,id,cores);
        for core in cores{next.links.get_mut(&(core,system)).unwrap().push(id);}
        if !next.fixed[&other]&&!next.registry.same_unit(first,other)?{
            if hint.map(|h|next.registry.same_unit(other,h)).transpose()?.unwrap_or(false){next.registry.merge_contact_units(other,first)?;}
            else{next.registry.merge_contact_units(first,other)?;}
        }
        next.check()?;*self=next;Ok(system)
    }
    fn check(&self)->Result<(),&'static str>{
        let mut all=BTreeSet::new();let mut dynamic=BTreeSet::new();let mut expected_links=BTreeSet::new();
        for (&id,s) in &self.systems{
            if !unique(&s.cores)||!unique(&s.contacts){return Err("duplicate contact system membership");}
            let mut pairs=BTreeSet::new();let mut covered=BTreeSet::new();
            for p in &s.pairs{
                if p.contacts.is_empty()||!unique(&p.contacts)||!p.clock.energy.is_finite(){return Err("invalid retained contact pair");}
                let [a,b]=p.cores;if a==b||!pairs.insert((a.min(b),a.max(b))){return Err("duplicate/self contact pair");}
                for &c in &p.contacts{let binding=self.bindings.get(&c).ok_or("unknown pair contact")?;if binding.system!=id||!binding.cores.iter().all(|v|p.cores.contains(v))||!covered.insert(c){return Err("incoherent pair binding");}}
            }
            if covered!=s.contacts.iter().copied().collect(){return Err("pair/system membership mismatch");}
            for &c in &s.contacts{if !all.insert(c){return Err("contact belongs to multiple systems");}let b=self.bindings.get(&c).ok_or("unknown system contact")?;if b.system!=id||b.cores[0]==b.cores[1]||!b.cores.iter().all(|v|s.cores.contains(v)){return Err("incoherent system contact binding");}}
            for &core in &s.cores{
                let fixed=*self.fixed.get(&core).ok_or("unknown contact core")?;
                if !fixed&&!dynamic.insert(core){return Err("dynamic core belongs to multiple systems");}
                let contacts=self.links.get(&(core,id)).ok_or("missing contact core link")?;
                if contacts.is_empty()||!unique(contacts){return Err("invalid contact core link");}
                let expected=s.contacts.iter().copied().filter(|c|self.bindings[c].cores.contains(&core)).collect::<BTreeSet<_>>();
                if contacts.iter().copied().collect::<BTreeSet<_>>()!=expected{return Err("core link membership mismatch");}expected_links.insert((core,id));
            }
        }
        if all!=self.bindings.keys().copied().collect()||expected_links!=self.links.keys().copied().collect(){return Err("orphan contact/link ownership");}Ok(())
    }
    fn insert_pair_and_head(&mut self,system:usize,id:usize,cores:[usize;2]){
        let s=self.systems.get_mut(&system).unwrap();s.contacts.insert(0,id);
        if let Some(p)=s.pairs.iter_mut().rev().find(|p|cores.iter().all(|c|p.cores.contains(c))){p.contacts.push(id);}
        else{self.notifications.publish(Event{kind:Kind::PairAdded,contact:None,cores});s.pairs.push(Pair{cores,contacts:vec![id],clock:FrictionGroup::default()});}
        self.bindings.insert(id,Binding{system,cores});
    }
    fn remove_pair_and_list(&mut self,system:usize,id:usize)->Result<bool,&'static str>{
        let s=self.systems.get_mut(&system).ok_or("unknown contact system")?;erase(&mut s.contacts,id)?;
        let i=s.pairs.iter().position(|p|p.contacts.contains(&id)).ok_or("missing pair contact")?;erase(&mut s.pairs[i].contacts,id)?;
        let empty=s.pairs[i].contacts.is_empty();if empty{self.notifications.publish(Event{kind:Kind::PairRemoved,contact:None,cores:s.pairs[i].cores});s.pairs.remove(i);}Ok(empty)
    }
    ///100ff200 re-inserts source contacts head-first, rebuilding source pairs
    /// with fresh clocks, then moves core links in reverse source-core order.
    pub(crate) fn merge(&mut self,target:usize,source:usize)->Result<(),&'static str>{
        if target==source||!self.systems.contains_key(&target)||!self.systems.contains_key(&source){return Err("invalid contact system merge");}
        let mut next=self.clone();let old=next.systems[&source].clone();
        for &id in &old.contacts{let b=next.bindings[&id];next.remove_pair_and_list(source,id)?;next.insert_pair_and_head(target,id,b.cores);}
        for &core in old.cores.iter().rev(){
            let moved=next.links.remove(&(core,source)).ok_or("missing merging core link")?;
            if let Some(existing)=next.links.get_mut(&(core,target)){existing.extend(moved.into_iter().rev());}
            else{
                next.registry.detach_contact_core(source,core)?;
                next.registry.attach_contact_core(target,core)?;
                next.systems.get_mut(&source).unwrap().cores.retain(|&v|v!=core);
                next.systems.get_mut(&target).unwrap().cores.push(core);next.links.insert((core,target),moved);
            }
        }
        // Shared fixed cores still occur in the retired system's core array.
        // They have no controller refs; release their registry membership.
        for core in next.systems[&source].cores.clone(){next.registry.detach_contact_core(source,core)?;}
        next.registry.destroy_contact_system(source)?;next.systems.remove(&source);next.check()?;*self=next;Ok(())
    }
    ///100fdbc0 joins only dynamic/dynamic pairs. Fixed geometry may occur
    /// in several components without welding their friction systems together.
    fn component_roots(&self,system:usize)->Result<BTreeMap<usize,usize>,&'static str>{
        let s=self.systems.get(&system).ok_or("unknown contact system")?;
        let mut parents=s.cores.iter().filter(|&&c|!self.fixed[&c]).map(|&c|(c,c)).collect::<BTreeMap<_,_>>();
        fn root(parents:&BTreeMap<usize,usize>,mut c:usize)->usize{while parents[&c]!=c{c=parents[&c];}c}
        for pair in s.pairs.iter().rev(){let [a,b]=pair.cores;if !self.fixed[&a]&&!self.fixed[&b]{let a=root(&parents,a);let b=root(&parents,b);if a!=b{parents.insert(b,a);}}}
        Ok(parents.keys().map(|&c|(c,root(&parents,c))).collect())
    }
    ///100ffec0's dirty-system tail, including100ffad0 transfers. Pair
    /// identities/clocks survive; only the global list and fixed-core link
    /// arrays are reordered. Simulation-unit graph repair is deferred to PSI.
    pub(crate) fn repair_system(&mut self,system:usize)->Result<Vec<usize>,&'static str>{
        if !self.systems.get(&system).ok_or("unknown contact system")?.dirty{return Ok(vec![]);}
        let mut next=self.clone();next.systems.get_mut(&system).unwrap().dirty=false;let mut created=vec![];let mut first_root=None;
        loop{
            let roots=next.component_roots(system)?;let cores=next.systems[&system].cores.clone();
            let Some(kept)=cores.iter().find_map(|c|roots.get(c)).copied()else{break;};
            let Some(selected)=cores.iter().filter_map(|c|roots.get(c)).copied().find(|&r|r!=kept)else{break;};
            first_root.get_or_insert(selected);let target=next.next_system;next.next_system=target.checked_add(1).ok_or("contact system namespace exhausted")?;
            next.registry.create_contact_system(target)?;next.systems.insert(target,System{cores:vec![],contacts:vec![],pairs:vec![],dirty:false});created.push(target);
            // Native first copies every fixed link, then deletes the unused
            // copies after pair transfer. Dynamic link arrays retain order.
            for &core in cores.iter().rev(){
                if next.fixed[&core]{next.registry.attach_contact_core(target,core)?;next.systems.get_mut(&target).unwrap().cores.push(core);next.links.insert((core,target),vec![]);}
                else if roots[&core]==selected{
                    next.registry.detach_contact_core(system,core)?;next.registry.attach_contact_core(target,core)?;
                    erase(&mut next.systems.get_mut(&system).unwrap().cores,core)?;next.systems.get_mut(&target).unwrap().cores.push(core);
                    let link=next.links.remove(&(core,system)).ok_or("missing splitting dynamic link")?;next.links.insert((core,target),link);
                }
            }
            let pairs=next.systems[&system].pairs.clone();
            for (index,pair) in pairs.into_iter().enumerate().rev(){
                let Some(dynamic)=pair.cores.iter().find(|&&c|!next.fixed[&c])else{continue;};if roots[dynamic]!=selected{continue;}
                next.notifications.publish(Event{kind:Kind::PairRemoved,contact:None,cores:pair.cores});next.systems.get_mut(&system).unwrap().pairs.remove(index);
                next.systems.get_mut(&target).unwrap().pairs.push(pair.clone());next.notifications.publish(Event{kind:Kind::PairAdded,contact:None,cores:pair.cores});
                for &id in pair.contacts.iter().rev(){
                    erase(&mut next.systems.get_mut(&system).unwrap().contacts,id)?;next.systems.get_mut(&target).unwrap().contacts.insert(0,id);next.bindings.get_mut(&id).unwrap().system=target;
                    for core in pair.cores{if next.fixed[&core]{erase(next.links.get_mut(&(core,system)).ok_or("missing splitting fixed link")?,id)?;next.links.get_mut(&(core,target)).unwrap().push(id);}}
                }
            }
            let retained=next.systems[&system].cores.clone();
            for &core in retained.iter().rev(){if next.fixed[&core]{for id in [target,system]{if next.links.get(&(core,id)).is_some_and(Vec::is_empty){next.links.remove(&(core,id));next.registry.detach_contact_core(id,core)?;erase(&mut next.systems.get_mut(&id).unwrap().cores,core)?;}}}}
            if next.systems[&target].cores.len()<2||next.systems[&system].cores.len()<2{return Err("unsupported degenerate split boundary");}
        }
        if let Some(root)=first_root{next.registry.mark_contact_graph(root)?;}next.check()?;*self=next;Ok(created)
    }
    ///100ff090 resets both movement clocks, removes global/pair/core links,
    /// marks newly empty core memberships, then requests feature/cache release.
    pub(crate) fn remove(&mut self,id:usize)->Result<(),&'static str>{
        let binding=*self.bindings.get(&id).ok_or("unknown managed contact")?;
        let mut next=self.clone();let system=binding.system;
        for core in binding.cores{next.registry.reset_core_clock(core)?;}
        if next.remove_pair_and_list(system,id)?{next.systems.get_mut(&system).unwrap().dirty=true;}
        for core in binding.cores{
            let link=next.links.get_mut(&(core,system)).ok_or("missing retiring contact core link")?;erase(link,id)?;
            if link.is_empty(){next.links.remove(&(core,system));next.registry.detach_contact_core(system,core)?;next.registry.mark_contact_graph(core)?;erase(&mut next.systems.get_mut(&system).unwrap().cores,core)?;}
        }
        next.notifications.publish(Event{kind:Kind::Deleted,contact:Some(id),cores:binding.cores});
        next.bindings.remove(&id);next.released.push(id);next.check()?;*self=next;Ok(())
    }
    pub(crate) fn destroy_empty(&mut self,system:usize)->Result<(),&'static str>{
        let s=self.systems.get(&system).ok_or("unknown contact system")?;
        if !s.cores.is_empty()||!s.contacts.is_empty()||!s.pairs.is_empty(){return Err("contact system still owns members");}
        self.registry.destroy_contact_system(system)?;self.systems.remove(&system);Ok(())
    }
    pub(crate) fn subscribe(&mut self,id:usize,flags:u32)->Result<(),&'static str>{self.notifications.subscribe(id,flags)}
    pub(crate) fn unsubscribe(&mut self,id:usize)->Result<(),&'static str>{self.notifications.unsubscribe(id)}
    pub(crate) fn set_listener_flags(&mut self,id:usize,flags:u32)->Result<(),&'static str>{self.notifications.set_flags(id,flags)}
    pub(crate) fn take_notifications(&mut self)->Vec<Delivery>{self.notifications.take()}
    /// Complete the unit graph tail using this contact owner's canonical
    /// registry, including any joint registrations imported by its owner.
    pub(crate) fn finish_unit_graph(&mut self,core:usize)->Result<(),&'static str>{
        let mut next=self.clone();next.registry.finish_unit_graph(core)?;next.check()?;*self=next;Ok(())
    }
    /// Joint numerical/resource teardown belongs to the joint owner. This
    /// operation removes only its canonical controller/connectivity record.
    pub(crate) fn destroy_joint_registration(&mut self,id:usize)->Result<(),&'static str>{
        let mut next=self.clone();next.registry.destroy_constraint(id)?;next.check()?;*self=next;Ok(())
    }
    pub(crate) fn begin_psi(&mut self)->Result<(),&'static str>{self.registry.begin_psi()}
    pub(crate) fn next_unit_dispatch(&mut self,poll:&mut crate::ivp_movement::Poll,
        prepare:impl FnMut(&[usize],&mut crate::ivp_unit::UnitFlags)->Result<(),&'static str>)->Result<Option<crate::ivp_unit_registry::UnitDispatch>,&'static str>{self.registry.next_unit_dispatch(poll,prepare)}
    pub(crate) fn finish_unit_dispatch(&mut self,dispatch:crate::ivp_unit_registry::UnitDispatch,
        classify:impl FnMut(usize)->Result<u8,&'static str>)->Result<(),&'static str>{self.registry.finish_unit_dispatch(dispatch,classify)}
    // These operations use the registry already owned by the contact graph.
    pub(crate) fn apply_registered_impulses(&mut self,bodies:&mut[crate::ivp_joint_apply::Body],commands:&[crate::ivp_joint_tail::Command])->Result<(),&'static str>{
        let bindings=bodies.iter().map(|b|crate::ivp_impulse_wake::Binding{body:b.id,current:b.id,real:b.id}).collect::<Vec<_>>();
        crate::ivp_impulse_wake::apply_with(bodies,&bindings,&mut self.registry,commands,|_,_,_,_,_|{})
    }
    pub(crate) fn register_ordinary_core(&mut self,id:usize,fixed:bool)->Result<(),&'static str>{
        if self.fixed.contains_key(&id){return Err("duplicate ordinary contact core");}
        let mut next=self.clone();next.registry.register_core(id,fixed)?;next.fixed.insert(id,fixed);next.check()?;*self=next;Ok(())
    }
    pub(crate) fn register_physical_group(&mut self,id:usize,cores:&[usize])->Result<(),&'static str>{
        let mut next=self.clone();next.registry.register_group(id,cores)?;next.check()?;*self=next;Ok(())
    }
    pub(crate) fn rebind_registered_group(&mut self,id:usize,cores:&[usize])->Result<(),&'static str>{
        let mut next=self.clone();next.registry.unregister_group(id,false)?;next.registry.register_group(id,cores)?;next.check()?;*self=next;Ok(())
    }
    pub(crate) fn detach_registered_group(&mut self,id:usize)->Result<(),&'static str>{
        let mut next=self.clone();next.registry.unregister_group(id,true)?;next.check()?;*self=next;Ok(())
    }
    pub(crate) fn wake_registered_core(&mut self,id:usize)->Result<(),&'static str>{self.registry.wake(id)}
    pub(crate) fn wake_registered_core_now(&mut self,id:usize)->Result<(),&'static str>{self.registry.wake_core_now(id)}
    pub(crate) fn finish_registered_wake(&mut self,id:usize)->Result<(),&'static str>{self.registry.finish_wake_unit(id)}
    pub(crate) fn wake_registered_unit_now(&mut self,id:usize)->Result<(),&'static str>{self.registry.wake_unit_now(id)}
    pub(crate) fn registry(&self)->&Registry{&self.registry}
    pub(crate) fn take_effects(&mut self)->(Vec<crate::ivp_unit_registry::Effect>,Vec<usize>){(self.registry.take_effects(),std::mem::take(&mut self.released))}
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn snapshot(&self,slots:usize)->serde_json::Value{
        use serde_json::json;
        let controller_keys=self.registry.controller_snapshot()["cores"].as_array().unwrap().iter().filter(|c|self.fixed.contains_key(&(c["id"].as_u64().unwrap() as usize))).map(|c|c["controllers"].as_array().unwrap().iter().map(|v|{let kind=v["kind"].as_str().unwrap();if kind=="base"{json!(0)}else if kind.starts_with("contact_"){let mut parts=kind.split('_');parts.next();let id=parts.next().unwrap().parse::<usize>().unwrap();json!(id*3+match parts.next().unwrap(){"normal"=>1,"tangent"=>2,"refresh"=>3,_=>panic!("unexpected controller")})}else{json!(kind)}}).collect::<Vec<_>>()).collect::<Vec<_>>();
        json!({"systems":(0..slots).map(|id|self.systems.get(&id).map(|s|json!({"cores":s.cores,"dynamic":s.cores.iter().copied().filter(|c|!self.fixed[c]).collect::<Vec<_>>(),"dirty":s.dirty as u8,"contacts":s.contacts,"groups":s.pairs.iter().map(|p|json!({"cores":p.cores,"contacts":p.contacts,"timer":p.clock.timer,"energy":p.clock.energy.to_bits()})).collect::<Vec<_>>()}))).collect::<Vec<_>>(),"core_links":self.fixed.keys().map(|&core|self.links.range((core,0)..=(core,usize::MAX)).map(|(&(c,system),contacts)|{assert_eq!(c,core);json!({"system":system,"contacts":contacts})}).collect::<Vec<_>>()).collect::<Vec<_>>(),"contact_bindings":self.bindings.iter().map(|(&id,b)|json!({"id":id,"cores":b.cores,"system":b.system})).collect::<Vec<_>>(),"controller_keys":controller_keys})
    }
}
