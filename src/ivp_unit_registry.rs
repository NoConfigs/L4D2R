//! Owned active/dormant unit lists and deferred core wake ownership. Physical
//! movement eligibility, controller execution and joint impulses are separate.
use std::collections::{BTreeMap,BTreeSet};
use crate::ivp_unit::UnitFlags;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum ControllerKey{Base,Joint(usize),Contact{system:usize,phase:ContactPhase}}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum ContactPhase{Normal,Tangent,Refresh}
impl ControllerKey{fn priority(self)->i32{match self{Self::Base=>1000,Self::Joint(_)=>405,Self::Contact{phase,..}=>match phase{ContactPhase::Normal=>0,ContactPhase::Tangent=>600,ContactPhase::Refresh=>2000}}}}
#[derive(Clone,Debug)]
struct ControllerRef{key:ControllerKey,cores:Vec<usize>}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum ResetVisit{Controller(ControllerKey),Core(usize)}
/// One original-unit dispatch scope. The token is consumed by the tail;
/// controller callbacks may edit canonical registrations between these calls.
#[derive(Debug)]
pub(crate) struct UnitDispatch{unit:usize,cores:Vec<usize>,poll:bool,controllers:Vec<(ControllerKey,Vec<usize>)>}
pub(crate) struct PolledUnitDispatch(UnitDispatch);
impl UnitDispatch{
    pub(crate) fn cores(&self)->&[usize]{&self.cores}
    pub(crate) fn controllers(&self)->&[(ControllerKey,Vec<usize>)]{&self.controllers}
}
#[derive(Clone,Debug)]
struct Core{fixed:bool,movement:u8,queued:bool,unit:usize,controllers:Vec<ControllerKey>}
#[derive(Clone,Debug)]
struct Unit{cores:Vec<usize>,flags:UnitFlags,dirty:bool,controllers:Vec<ControllerRef>}
#[derive(Clone,Debug)]
struct Constraint{reference:usize,attached:usize,associated:Vec<usize>}
#[derive(Clone,Debug)]
pub(crate) enum Effect{ResetClock(usize),Activated(usize),ForcedSleep(usize),Slept(usize)}
#[derive(Clone,Debug)]
pub(crate) struct Registry{
    cores:BTreeMap<usize,Core>,units:Vec<Option<Unit>>,active:Vec<usize>,dormant:Vec<usize>,pending:Vec<usize>,constraints:BTreeMap<usize,Constraint>,groups:BTreeMap<usize,Vec<usize>>,contacts:BTreeMap<usize,Vec<usize>>,effects:Vec<Effect>,update_units:Vec<usize>,
}
impl Registry{
    pub(crate) fn new(anchor:usize)->Self{
        let mut r=Self{cores:BTreeMap::new(),units:vec![],active:vec![],dormant:vec![],pending:vec![],constraints:BTreeMap::new(),groups:BTreeMap::new(),contacts:BTreeMap::new(),effects:vec![],update_units:vec![]};
        r.register_core(anchor,true).unwrap();r
    }
    fn unit(&self,id:usize)->&Unit{self.units[id].as_ref().expect("missing owned unit")}
    fn unit_mut(&mut self,id:usize)->&mut Unit{self.units[id].as_mut().expect("missing owned unit")}
    fn allocate(&mut self,cores:Vec<usize>,state:u32)->usize{
        let id=self.units.iter().position(Option::is_none).unwrap_or(self.units.len());let value=Some(Unit{cores,flags:UnitFlags{flags:state},dirty:false,controllers:vec![]});
        if id==self.units.len(){self.units.push(value);}else{self.units[id]=value;}
        if state as u8>=8{self.dormant.insert(0,id);}else{self.active.insert(0,id);}id
    }
    fn add_controller_ref(&mut self,unit:usize,core:usize,key:ControllerKey){
        let refs=&mut self.unit_mut(unit).controllers;
        if let Some(entry)=refs.iter_mut().find(|e|e.key==key){entry.cores.push(core);}
        else{refs.push(ControllerRef{key,cores:vec![core]});}
        //100eaef0 is a stable ascending-priority insertion sort. Execution
        // and reset traversal consume the resulting vector backwards.
        refs.sort_by_key(|e|e.key.priority());
    }
    fn remove_controller_ref(&mut self,unit:usize,core:usize,key:ControllerKey){
        let refs=&mut self.unit_mut(unit).controllers;
        let index=refs.iter().position(|e|e.key==key).expect("missing controller reference");
        let at=refs[index].cores.iter().rposition(|&c|c==core).expect("missing controller core");
        refs[index].cores.remove(at);if refs[index].cores.is_empty(){refs.remove(index);}
    }
    fn rebuild_controllers(&mut self,unit:usize){
        self.unit_mut(unit).controllers.clear();
        for core in self.unit(unit).cores.clone().into_iter().rev(){
            for key in self.cores[&core].controllers.clone().into_iter().rev(){self.add_controller_ref(unit,core,key);}
        }
    }
    fn unlink(&mut self,id:usize){
        let list=if self.unit(id).flags.flags as u8>=8{&mut self.dormant}else{&mut self.active};
        let at=list.iter().position(|&x|x==id).expect("unit lost list ownership");list.remove(at);
    }
    pub(crate) fn register_core(&mut self,id:usize,fixed:bool)->Result<(),&'static str>{
        if self.cores.contains_key(&id){return Err("core already registered");}
        let unit=self.allocate(vec![id],8);self.cores.insert(id,Core{fixed,movement:8,queued:false,unit,controllers:if fixed{vec![]}else{vec![ControllerKey::Base]}});self.rebuild_controllers(unit);Ok(())
    }
    pub(crate) fn wake(&mut self,id:usize)->Result<(),&'static str>{
        let c=self.cores.get(&id).ok_or("unknown wake core")?;
        if c.fixed||c.movement!=8{self.effects.push(Effect::ResetClock(id));}
        else{self.enqueue_wake(id)?;}Ok(())
    }
    ///10106900: append once, then set the native queued flag. Movement/fixed
    /// eligibility belongs to the caller, rather than this low-level queue.
    pub(crate) fn enqueue_wake(&mut self,id:usize)->Result<(),&'static str>{
        let queued=self.cores.get(&id).ok_or("unknown wake core")?.queued;
        if !queued{self.pending.push(id);self.cores.get_mut(&id).unwrap().queued=true;}Ok(())
    }
    ///100e8b80/100eab20 collision entry activates the unit immediately,
    /// rather than placing a public-impulse wake on the next PSI's queue.
    pub(crate) fn wake_unit_now(&mut self,id:usize)->Result<(),&'static str>{
        let c=self.cores.get(&id).ok_or("unknown immediate wake core")?;
        if c.fixed||c.movement<8{return Ok(());}
        let unit=c.unit;
        // Immediate activation leaves pending entries/queued bits owned by
        //10106680. Its later drain resets already-active unit clocks.
        self.activate_unit(unit);self.check()
    }
    ///101073c0 changes one member before its bounds/contact callbacks. The
    ///outer100eab20 owns the later unit-list activation and scan restarts.
    pub(crate) fn wake_core_now(&mut self,id:usize)->Result<(),&'static str>{
        let c=self.cores.get_mut(&id).ok_or("unknown immediate member wake")?;
        if !c.fixed&&c.movement>=8{c.movement=1;self.effects.push(Effect::Activated(id));}self.check()
    }
    pub(crate) fn finish_wake_unit(&mut self,id:usize)->Result<(),&'static str>{
        let unit=self.cores.get(&id).ok_or("unknown completed unit wake")?.unit;
        if self.unit(unit).cores.iter().any(|id|!self.cores[id].fixed&&self.cores[id].movement>=8){return Err("unit wake still contains dormant members");}
        self.activate_unit(unit);self.check()
    }
    pub(crate) fn unit_cores(&self,id:usize)->Result<&[usize],&'static str>{Ok(&self.unit(self.cores.get(&id).ok_or("unknown unit member core")?.unit).cores)}
    pub(crate) fn unit_flags(&self,id:usize)->Result<UnitFlags,&'static str>{Ok(self.unit(self.cores.get(&id).ok_or("unknown unit core")?.unit).flags)}
    pub(crate) fn movement(&self,id:usize)->Result<u8,&'static str>{Ok(self.cores.get(&id).ok_or("unknown movement core")?.movement)}
    pub(crate) fn queued(&self,id:usize)->Result<bool,&'static str>{Ok(self.cores.get(&id).ok_or("unknown queued core")?.queued)}
    pub(crate) fn pending_cores(&self)->&[usize]{&self.pending}
    pub(crate) fn core_fixed(&self,id:usize)->Result<bool,&'static str>{Ok(self.cores.get(&id).ok_or("unknown contact core")?.fixed)}
    pub(crate) fn same_unit(&self,a:usize,b:usize)->Result<bool,&'static str>{Ok(self.cores.get(&a).ok_or("unknown first unit core")?.unit==self.cores.get(&b).ok_or("unknown second unit core")?.unit)}
    ///100f0460 resets clocks without activation or deferred wake eligibility.
    pub(crate) fn reset_core_clock(&mut self,id:usize)->Result<(),&'static str>{if !self.cores.contains_key(&id){return Err("unknown movement clock core");}self.effects.push(Effect::ResetClock(id));Ok(())}
    fn cancel_wake(&mut self,id:usize){
        let c=self.cores.get_mut(&id).unwrap();if c.queued{let at=self.pending.iter().rposition(|&x|x==id).expect("wake flag lost queue owner");self.pending.remove(at);c.queued=false;}
    }
    fn activate_unit(&mut self,id:usize){
        for core in self.unit(id).cores.clone().into_iter().rev(){let c=self.cores.get_mut(&core).unwrap();if !c.fixed&&c.movement>=8{c.movement=1;self.effects.push(Effect::Activated(core));}}
        if self.unit(id).flags.flags as u8==8{
            self.unlink(id);self.unit_mut(id).flags.activate();self.active.insert(0,id);
        }
    }
    /// Commit a caller-accepted single-core forced sleep. Physical movement
    /// eligibility must be established before committing this transition.
    pub(crate) fn commit_single_core_sleep(&mut self,id:usize)->Result<(),&'static str>{
        let c=self.cores.get(&id).ok_or("unknown sleep core")?;if c.fixed{return Ok(());}
        let unit=c.unit;if self.unit(unit).cores.len()!=1{return Err("multi-core sleep eligibility requires controller/movement integration");}
        self.cancel_wake(id);
        if self.cores[&id].movement<8{
            self.effects.push(Effect::ForcedSleep(id));self.cores.get_mut(&id).unwrap().movement=8;self.unlink(unit);let u=self.unit_mut(unit);u.flags.flags=(u.flags.flags&!0xff)|8;self.dormant.insert(0,unit);
        }Ok(())
    }
    pub(crate) fn remove_core(&mut self,id:usize)->Result<(),&'static str>{
        let c=self.cores.get(&id).ok_or("unknown removed core")?;let unit=c.unit;
        if self.unit(unit).cores.len()!=1||self.constraints.values().any(|x|x.reference==id||x.attached==id)||self.groups.values().chain(self.contacts.values()).any(|g|g.contains(&id)){return Err("connected-core destruction requires controller teardown");}
        self.cancel_wake(id);self.unlink(unit);self.units[unit]=None;self.cores.remove(&id);Ok(())
    }
    ///100af1f0 builds the group vector attached-first, then reference;100ebed0
    /// traverses that vector backwards and retains the first encountered unit.
    pub(crate) fn attach_ragdoll(&mut self,id:usize,reference:usize,attached:usize)->Result<(),&'static str>{
        if self.constraints.contains_key(&id)||self.groups.contains_key(&id)||reference==attached{return Err("invalid constraint identity");}
        if !self.cores.contains_key(&reference)||!self.cores.contains_key(&attached){return Err("unknown constraint core");}
        let associated=[attached,reference].into_iter().filter(|c|!self.cores[c].fixed).collect::<Vec<_>>();
        if associated.is_empty(){return Err("fixed-only constraint group is not connected");}
        self.connect_controller(id,&associated);
        self.constraints.insert(id,Constraint{reference,attached,associated});Ok(())
    }
    fn connect_controller(&mut self,id:usize,associated:&[usize]){
        if associated.is_empty(){return;}
        let target=self.cores[associated.last().unwrap()].unit;let mut movement=8u8;let mut merged=false;
        for core in associated.iter().rev(){
            movement&=self.cores[core].movement;let source=self.cores[core].unit;
            if source!=target{
                merged=true;let members=self.unit(source).cores.clone();self.unit_mut(target).cores.extend(members.iter().copied());
                for member in members{self.cores.get_mut(&member).unwrap().unit=target;}
                self.unlink(source);self.units[source]=None;
            }
            self.cores.get_mut(core).unwrap().controllers.push(ControllerKey::Joint(id));
            self.add_controller_ref(target,*core,ControllerKey::Joint(id));
        }
        if merged{self.rebuild_controllers(target);}
        if movement<8{self.activate_unit(target);}
    }
    ///100ebed0 registers one controller across the group's complete core vector.
    pub(crate) fn register_group(&mut self,id:usize,cores:&[usize])->Result<(),&'static str>{
        if self.groups.contains_key(&id)||self.constraints.contains_key(&id){return Err("duplicate controller group identity");}
        if cores.iter().any(|c|!self.cores.contains_key(c)){return Err("unknown controller group core");}
        let associated=cores.iter().copied().filter(|c|!self.cores[c].fixed).collect::<Vec<_>>();
        if associated.iter().copied().collect::<BTreeSet<_>>().len()!=associated.len(){return Err("shared core aliases require compound registration");}
        self.connect_controller(id,&associated);self.groups.insert(id,associated);Ok(())
    }
    ///100eb630: destruction detaches without waking; temporary removal for an
    /// active group edit activates a sleeping unit or resets active core clocks.
    pub(crate) fn unregister_group(&mut self,id:usize,detach_only:bool)->Result<(),&'static str>{
        let associated=self.groups.remove(&id).ok_or("unknown controller group identity")?;
        for &core in associated.iter().rev(){
            let key=ControllerKey::Joint(id);let owner=self.cores[&core].unit;
            let list=&mut self.cores.get_mut(&core).unwrap().controllers;
            let at=list.iter().rposition(|&k|k==key).unwrap();list.remove(at);self.remove_controller_ref(owner,core,key);
        }
        if let Some(core)=associated.first(){
            let unit=self.cores[core].unit;self.unit_mut(unit).dirty=true;self.unit_mut(unit).flags.contact_removed();
            if !detach_only {
                if self.unit(unit).flags.flags as u8>=8{self.activate_unit(unit);}
                else{for member in self.unit(unit).cores.clone().into_iter().rev(){self.effects.push(Effect::ResetClock(member));}}
            }
        }Ok(())
    }
    pub(crate) fn destroy_constraint(&mut self,id:usize)->Result<(),&'static str>{
        let c=self.constraints.remove(&id).ok_or("unknown constraint")?;
        // The public constraint teardown queues both object roles; its group
        // destructor removes connectivity without forcing immediate activation.
        self.wake(c.reference)?;self.wake(c.attached)?;
        for &core in c.associated.iter().rev(){
            let key=ControllerKey::Joint(id);let owner=self.cores[&core].unit;
            let list=&mut self.cores.get_mut(&core).unwrap().controllers;
            let at=list.iter().rposition(|&k|k==key).expect("missing core controller");list.remove(at);
            self.remove_controller_ref(owner,core,key);
        }
        if let Some(core)=c.associated.last(){let u=self.cores[core].unit;self.unit_mut(u).dirty=true;self.unit_mut(u).flags.contact_removed();}Ok(())
    }
    ///10106750/10106870 controller ownership only. Contact rows, pair groups
    /// and physical dispatch belong to the containing contact-world owner.
    pub(crate) fn create_contact_system(&mut self,id:usize)->Result<(),&'static str>{
        if self.contacts.contains_key(&id){return Err("contact system already registered");}self.contacts.insert(id,vec![]);Ok(())
    }
    pub(crate) fn attach_contact_core(&mut self,system:usize,core:usize)->Result<(),&'static str>{
        let fixed=self.cores.get(&core).ok_or("unknown contact core")?.fixed;
        let members=self.contacts.get(&system).ok_or("unknown contact system")?;
        if members.contains(&core){return Err("duplicate contact core");}
        let mut next=self.clone();next.contacts.get_mut(&system).unwrap().push(core);
        if !fixed{for phase in [ContactPhase::Normal,ContactPhase::Tangent,ContactPhase::Refresh]{let key=ControllerKey::Contact{system,phase};next.cores.get_mut(&core).unwrap().controllers.push(key);next.add_controller_ref(next.cores[&core].unit,core,key);}}
        *self=next;Ok(())
    }
    ///101065b0 removes refresh, tangent, then normal refs, without waking or
    /// marking a graph itself. Its contact-deletion caller owns that mark.
    pub(crate) fn detach_contact_core(&mut self,system:usize,core:usize)->Result<(),&'static str>{
        let members=self.contacts.get(&system).ok_or("unknown contact system")?;
        let at=members.iter().position(|&id|id==core).ok_or("core not bound to contact system")?;
        let mut next=self.clone();next.contacts.get_mut(&system).unwrap().remove(at);
        if !next.cores[&core].fixed{for phase in [ContactPhase::Refresh,ContactPhase::Tangent,ContactPhase::Normal]{let key=ControllerKey::Contact{system,phase};let owner=next.cores[&core].unit;let list=&mut next.cores.get_mut(&core).unwrap().controllers;let at=list.iter().rposition(|&k|k==key).ok_or("contact core lacks phase controller")?;list.remove(at);next.remove_controller_ref(owner,core,key);}}
        *self=next;Ok(())
    }
    pub(crate) fn destroy_contact_system(&mut self,id:usize)->Result<(),&'static str>{
        if !self.contacts.get(&id).ok_or("unknown contact system")?.is_empty(){return Err("contact system still owns cores");}self.contacts.remove(&id);Ok(())
    }
    ///10104010 selects the retained unit, then100eb600 merges/rebuilds it.
    /// Keep target movement/list/flags; this operation does not invent a wake.
    pub(crate) fn merge_contact_units(&mut self,target:usize,source:usize)->Result<(),&'static str>{
        let a=self.cores.get(&target).ok_or("unknown contact merge target")?;let b=self.cores.get(&source).ok_or("unknown contact merge source")?;
        if a.fixed||b.fixed{return Err("contact merge requires dynamic cores");}let (target,source)=(a.unit,b.unit);if target==source{return Ok(());}
        let mut next=self.clone();let members=next.unit(source).cores.clone();next.unit_mut(target).cores.extend(members.iter().copied());for core in members{next.cores.get_mut(&core).unwrap().unit=target;}next.unlink(source);next.units[source]=None;next.rebuild_controllers(target);next.check()?;*self=next;Ok(())
    }
    pub(crate) fn mark_contact_graph(&mut self,core:usize)->Result<(),&'static str>{let unit=self.cores.get(&core).ok_or("unknown contact graph core")?.unit;self.unit_mut(unit).dirty=true;self.unit_mut(unit).flags.contact_removed();Ok(())}
    /// The same repair is called after controller work during an ordinary PSI.
    pub(crate) fn repair_contact_graph(&mut self,core:usize)->Result<(),&'static str>{let unit=self.cores.get(&core).ok_or("unknown contact graph core")?.unit;let mut next=self.clone();next.split(unit);next.check()?;*self=next;Ok(())}
    ///100eb7d0 graph tail. A clean unit must retain its controller vector;
    /// graph repair includes both joint groups and contact memberships.
    pub(crate) fn finish_unit_graph(&mut self,core:usize)->Result<(),&'static str>{
        let unit=self.cores.get(&core).ok_or("unknown graph-tail core")?.unit;
        if !self.unit(unit).dirty{return Ok(());}
        let mut next=self.clone();next.split(unit);next.check()?;*self=next;Ok(())
    }
    fn split(&mut self,unit:usize){
        let members=self.unit(unit).cores.clone();let mut unseen=members.iter().copied().collect::<BTreeSet<_>>();let mut components=vec![];
        for &first in &members{
            if !unseen.remove(&first){continue;}let mut group=BTreeSet::from([first]);let mut todo=vec![first];
            while let Some(core)=todo.pop(){for associated in self.constraints.values().map(|c|&c.associated).chain(self.groups.values()).chain(self.contacts.values()){if associated.contains(&core){for &other in associated{if unseen.remove(&other){group.insert(other);todo.push(other);}}}}}
            components.push(members.iter().copied().filter(|c|group.contains(c)).collect::<Vec<_>>());
        }
        //100eb4b0 first transfers the first differing root, then the first
        // remaining root while more than one component remains. New units
        // enter the active list at its head; preserve member-vector order.
        let mut first=true;
        while components.len()>1{
            let group=components.remove(if first{1}else{0});first=false;
            self.unit_mut(unit).cores.retain(|c|!group.contains(c));let new=self.allocate(group.clone(),1);
            for core in group{self.cores.get_mut(&core).unwrap().unit=new;}self.rebuild_controllers(new);
        }
        self.rebuild_controllers(unit);self.unit_mut(unit).dirty=false;self.unit_mut(unit).flags.graph_finished();
    }
    pub(crate) fn begin_psi(&mut self)->Result<(),&'static str>{
        //10106680 drains the original pending array in reverse order.
        while let Some(id)=self.pending.pop(){let c=self.cores.get(&id).ok_or("pending wake core destroyed")?;
            if !c.fixed{if c.movement==8{self.activate_unit(c.unit);}else{for core in self.unit(c.unit).cores.clone().into_iter().rev(){self.effects.push(Effect::ResetClock(core));}}}self.cores.get_mut(&id).unwrap().queued=false;
        }
        //100ebbb0's three-unit lookahead processes the leading units forward,
        // then its final window in reverse. This is not reset/list order.
        let leading=self.active.len().saturating_sub(3);
        self.update_units=self.active[..leading].iter().copied().chain(self.active[leading..].iter().rev().copied()).collect();self.update_units.reverse();self.check()
    }
    pub(crate) fn take_effects(&mut self)->Vec<Effect>{std::mem::take(&mut self.effects)}
    ///100eb7d0 polls once per original active unit, then rebuilds dirty graphs.
    /// Newly split head units are not revisited during this traversal.
    pub(crate) fn finish_psi_units(&mut self,poll:&mut crate::ivp_movement::Poll,classify:impl FnMut(usize)->Result<u8,&'static str>)->Result<(),&'static str>{
        self.finish_psi_units_with(poll,|_,_|Ok(()),classify)
    }
    ///100eb7d0 decrements the shared poll before reverse controller dispatch,
    /// then classifies movement after controllers and rebuilds dirty graphs.
    pub(crate) fn finish_psi_units_with(&mut self,poll:&mut crate::ivp_movement::Poll,execute:impl FnMut(ControllerKey,&[usize])->Result<(),&'static str>,classify:impl FnMut(usize)->Result<u8,&'static str>)->Result<(),&'static str>{
        self.finish_prepared_units_with(poll,|_,_|Ok(()),execute,classify)
    }
    /// Physical preparation precedes shared polling and controller dispatch.
    /// The containing owner must roll back physical and registry state together.
    pub(crate) fn finish_prepared_units_with(&mut self,poll:&mut crate::ivp_movement::Poll,
        mut prepare:impl FnMut(&[usize],&mut UnitFlags)->Result<(),&'static str>,
        mut execute:impl FnMut(ControllerKey,&[usize])->Result<(),&'static str>,
        mut classify:impl FnMut(usize)->Result<u8,&'static str>)->Result<(),&'static str>{
        while let Some(dispatch)=self.next_unit_dispatch(poll,&mut prepare)?{
            for (key,cores) in dispatch.controllers(){execute(*key,cores)?;}
            self.finish_unit_dispatch(dispatch,&mut classify)?;
        }self.check()
    }
    /// Release the registry borrow before numerical callbacks. The unit list
    /// was captured by begin_psi: units created by a graph tail wait until the
    /// next PSI. Preparation precedes the single shared-poll decrement.
    pub(crate) fn next_unit_dispatch(&mut self,poll:&mut crate::ivp_movement::Poll,
        mut prepare:impl FnMut(&[usize],&mut UnitFlags)->Result<(),&'static str>)->Result<Option<UnitDispatch>,&'static str>{
        let Some(&unit)=self.update_units.last()else{return Ok(None);};
        let value=self.units.get(unit).and_then(Option::as_ref).ok_or("scheduled unit no longer exists")?;
        let cores=value.cores.clone();let mut flags=value.flags;
        prepare(&cores,&mut flags)?;
        self.unit_mut(unit).flags=flags;self.update_units.pop();
        let should_poll=poll.tick(1./65536.,-5.);
        let controllers=self.unit(unit).controllers.iter().rev().map(|entry|(entry.key,entry.cores.clone())).collect();
        Ok(Some(UnitDispatch{unit,cores,poll:should_poll,controllers}))
    }
    /// Classify the original unit only after its callbacks; then repair the
    /// now-current joint/contact graph. The caller rolls back physical state,
    /// registry and poll together if any callback or classification fails.
    pub(crate) fn finish_unit_dispatch(&mut self,dispatch:UnitDispatch,
        classify:impl FnMut(usize)->Result<u8,&'static str>)->Result<(),&'static str>{
        let polled=self.poll_unit_dispatch(dispatch,classify)?;self.finish_polled_unit(polled)
    }
    ///100eb7d0 polls movement before inactive-object notifications, then
    /// rebuilds the dirty graph. The token keeps these phases single-use.
    pub(crate) fn poll_unit_dispatch(&mut self,dispatch:UnitDispatch,
        mut classify:impl FnMut(usize)->Result<u8,&'static str>)->Result<PolledUnitDispatch,&'static str>{
        let id=dispatch.unit;
        let u=self.units.get(id).and_then(Option::as_ref).ok_or("dispatched unit no longer exists")?;
        if u.cores!=dispatch.cores{return Err("unit membership changed before dispatch tail");}
        // Validate all classifications before committing registry changes.
        // Do not clone the entire registry for every active unit.
        if dispatch.poll{
            let mut all=3u8;let mut classified=Vec::with_capacity(self.unit(id).cores.len());
            for core in self.unit(id).cores.iter().rev().copied(){let state=classify(core)?;if !(1..=3).contains(&state){return Err("invalid movement classification");}classified.push((core,state));all&=state;}
            for (core,state) in classified{self.cores.get_mut(&core).unwrap().movement=state;}
            if all==3{
                for core in self.unit(id).cores.clone().into_iter().rev(){self.cores.get_mut(&core).unwrap().movement=8;self.effects.push(Effect::Slept(core));}
                self.unlink(id);self.unit_mut(id).flags.flags=(self.unit(id).flags.flags&!0xff)|8;self.dormant.insert(0,id);
            }
        }
        Ok(PolledUnitDispatch(dispatch))
    }
    pub(crate) fn finish_polled_unit(&mut self,polled:PolledUnitDispatch)->Result<(),&'static str>{
        let dispatch=polled.0;let id=dispatch.unit;
        if self.units.get(id).and_then(Option::as_ref).is_none_or(|u|u.cores!=dispatch.cores){return Err("unit membership changed during inactive notifications");}
        if self.unit(id).dirty{self.split(id);}self.check()
    }
    pub(crate) fn active_reset_visits(&self)->Vec<ResetVisit>{
        self.active.iter().flat_map(|&id|{let u=self.unit(id);
            u.controllers.iter().rev().map(|r|ResetVisit::Controller(r.key)).chain(u.cores.iter().rev().copied().map(ResetVisit::Core)).collect::<Vec<_>>()
        }).collect()
    }
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn controller_snapshot(&self)->serde_json::Value{
        let metadata=|key:ControllerKey|match key{
            ControllerKey::Base=>serde_json::json!({"kind":"base","priority":1000,"vtable_rva":0x15ec54,"reset_rva":0x7e0c0}),
            ControllerKey::Joint(id)=>serde_json::json!({"kind":format!("joint_{id}"),"priority":405,"vtable_rva":0x1617f0,"reset_rva":0x7e0c0}),
            ControllerKey::Contact{system,phase}=>{let(name,priority,table,reset)=match phase{ContactPhase::Normal=>("normal",0,0x16f810,0x7e0c0),ContactPhase::Tangent=>("tangent",600,0x16f888,0xfdcc0),ContactPhase::Refresh=>("refresh",2000,0x16f840,0x7e0c0)};serde_json::json!({"kind":format!("contact_{system}_{name}"),"priority":priority,"vtable_rva":table,"reset_rva":reset})},
        };
        serde_json::json!({"cores":self.cores.iter().map(|(&id,c)|serde_json::json!({"id":id,"controllers":c.controllers.iter().copied().map(&metadata).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "units":self.active.iter().chain(&self.dormant).map(|&id|{let u=self.unit(id);serde_json::json!({"cores":u.cores,"controllers":u.controllers.iter().map(|r|serde_json::json!({"controller":metadata(r.key),"cores":r.cores})).collect::<Vec<_>>()})}).collect::<Vec<_>>()})
    }
    pub(crate) fn active_reset_cores(&self)->Vec<usize>{self.active.iter().flat_map(|&id|self.unit(id).cores.iter().rev().copied()).collect()}
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn contact_snapshot(&self,slots:usize)->serde_json::Value{
        serde_json::json!({"registry":self.snapshot(),"controllers":self.controller_snapshot(),"systems":(0..slots).map(|id|self.contacts.get(&id).map(|cores|serde_json::json!({"cores":cores,"dynamic":cores.iter().copied().filter(|c|!self.cores[c].fixed).collect::<Vec<_>>(),"count":cores.len()}))).collect::<Vec<_>>(),"unit_flags":self.active.iter().chain(&self.dormant).map(|&id|self.unit(id).flags.flags&0x3fff).collect::<Vec<_>>()})
    }
    fn check(&self)->Result<(),&'static str>{
        let mut units=BTreeSet::new();let mut members=BTreeSet::new();
        for (list,dormant) in [(&self.active,false),(&self.dormant,true)]{for &id in list{
            if !units.insert(id){return Err("duplicate unit list ownership");}let u=self.unit(id);
            if (u.flags.flags as u8>=8)!=dormant{return Err("unit state/list mismatch");}
            for &core in &u.cores{if !members.insert(core)||self.cores[&core].unit!=id{return Err("core/unit owner mismatch");}}
        }}
        if members.len()!=self.cores.len()||units.len()!=self.units.iter().filter(|u|u.is_some()).count(){return Err("orphan unit/core");}
        let queued=self.pending.iter().copied().collect::<BTreeSet<_>>();if queued.len()!=self.pending.len(){return Err("duplicate pending core");}
        for (&id,c) in &self.cores{if queued.contains(&id)!=c.queued{return Err("wake flag/list mismatch");}}Ok(())
    }
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn snapshot(&self)->serde_json::Value{
        self.check().unwrap();let list=|ids:&[usize]|ids.iter().map(|&id|{let u=self.unit(id);serde_json::json!({"state":u.flags.flags as u8,"cores":u.cores})}).collect::<Vec<_>>();
        serde_json::json!({"cores":self.cores.iter().map(|(&id,c)|serde_json::json!({"id":id,"movement":c.movement,"queued":c.queued,"fixed":c.fixed})).collect::<Vec<_>>(),"pending":self.pending,"active":list(&self.active),"dormant":list(&self.dormant)})
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn position(id:usize)->bevy::prelude::Vec3{use bevy::prelude::Vec3;match id{0=>Vec3::new(0.,-128.*0.0254,0.),1=>Vec3::new(128.*0.0254,-128.*0.0254,0.),4..=7=>Vec3::new((512.+(id-4) as f32*128.)*0.0254,-128.*0.0254,0.),_=>Vec3::ZERO}}
    fn initial_history()->crate::ivp_movement::History{use bevy::prelude::Vec3;crate::ivp_movement::History{close_time:0.,far_time:0.,close_position:Vec3::ZERO,far_position:Vec3::ZERO,close_quat:[0.;4],far_quat:[0.;4]}}
    fn effects(r:&mut Registry,history:&mut BTreeMap<usize,crate::ivp_movement::History>,now:f64){
        for effect in r.take_effects(){match effect{
            Effect::ResetClock(id)|Effect::Activated(id)=>history.get_mut(&id).unwrap().reset_clock(now).unwrap(),
            Effect::ForcedSleep(id)=>{let h=history.get_mut(&id).unwrap();h.close_time=now-20.;h.close_position=position(id);h.close_quat=[0.,0.,0.,1.];},
            Effect::Slept(id)=>{assert!(history.contains_key(&id));},
        }}
    }
    fn replay(data:&serde_json::Value)->Vec<serde_json::Value>{
        replay_with_reset(data,|_,world,_|{world.reset_time().unwrap();})
    }
    fn replay_with_reset(data:&serde_json::Value,reset:impl FnMut(&Registry,&mut crate::ivp_event_world::World,usize))->Vec<serde_json::Value>{
        replay_observed(data,reset,|_,_|{})
    }
    fn replay_observed(data:&serde_json::Value,reset:impl FnMut(&Registry,&mut crate::ivp_event_world::World,usize),observe:impl FnMut(&Registry,usize))->Vec<serde_json::Value>{
        replay_with_execution(data,reset,observe,|_,_,_|{})
    }
    fn replay_with_execution(data:&serde_json::Value,mut reset:impl FnMut(&Registry,&mut crate::ivp_event_world::World,usize),mut observe:impl FnMut(&Registry,usize),mut execute:impl FnMut(usize,ControllerKey,&[usize]))->Vec<serde_json::Value>{
        use crate::ivp_movement::{self as movement,Input,Settings,Poll};use bevy::prelude::Vec3;
        let settings=Settings{translation_squared:0.0001,rotation_squared:0.000025,far_translation_squared:0.1f32*0.1,far_rotation_squared:0.2f32*0.2,far_timeout:4.,one:1.,two:2.,angular_factor0:std::f32::consts::PI,angular_factor1:0.75};
        let mut history=(0..4).map(|i|(i,initial_history())).collect::<BTreeMap<_,_>>();
        let mut poll=Poll{remaining:10,seed:1};let mut world=crate::ivp_event_world::World::new(1000).unwrap();world.set_timestep(1./60.).unwrap();world.simulate(1./60.,|_,_,_|Ok(()),|_,_,_|Ok(()),|_,_|{}).unwrap();
        let mut r=Registry::new(3);for (id,fixed) in [(2,true),(0,false),(1,false)]{r.register_core(id,fixed).unwrap();}
        let mut result=vec![];
        for (index,op) in data["states"].as_array().unwrap().iter().enumerate(){
            let id=op["body"].as_i64().unwrap();match op["action"].as_str().unwrap(){
                "created"=>{},"immediate"=>r.wake_unit_now(id as usize).unwrap(),"drain"=>r.begin_psi().unwrap(),"reset"=>{reset(&r,&mut world,index);},"wake"=>r.wake(id as usize).unwrap(),"sleep"=>r.commit_single_core_sleep(id as usize).unwrap(),
                "step"=>{world.simulate(1./60.,|_,now,_|{
                    r.begin_psi()?;effects(&mut r,&mut history,now);
                    r.finish_psi_units_with(&mut poll,|key,cores|{execute(index,key,cores);Ok(())},|core|movement::classify(history.get_mut(&core).unwrap(),Input{radius:4.*0.0254,sleep_time:0.3,angular:Vec3::ZERO,position:position(core),start:[0.,0.,0.,1.],end:[0.,0.,0.,1.]},now,&settings))?;
                    effects(&mut r,&mut history,now);Ok(())
                },|_,_,_|Ok(()),|_,_|{}).unwrap();},
                "destroy"=>{r.remove_core(id as usize).unwrap();history.remove(&(id as usize));},"create"=>{r.register_core(id as usize,false).unwrap();history.insert(id as usize,initial_history());},
                "join"=>r.attach_ragdoll(0,1,0).unwrap(),"detach"=>r.destroy_constraint(0).unwrap(),"join_second"=>r.attach_ragdoll(1,4,1).unwrap(),"detach_second"=>r.destroy_constraint(1).unwrap(),_=>panic!("unknown authored action"),
            }
            effects(&mut r,&mut history,world.now);
            let snapshot=r.snapshot();let expected_order=snapshot["active"].as_array().unwrap().iter().flat_map(|u|u["cores"].as_array().unwrap().iter().rev().map(|v|v.as_u64().unwrap() as usize)).collect::<Vec<_>>();
            assert_eq!(r.active_reset_cores(),expected_order);observe(&r,index);result.push(snapshot);
        }result
    }
    #[test]
    fn native_immediate_activation_retains_deferred_wake_ownership(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-queued-immediate-wake.json")).unwrap();
        let actual=replay(&data);assert_eq!(actual.len(),16);
        for (index,(actual,expected)) in actual.iter().zip(data["states"].as_array().unwrap()).enumerate(){assert_eq!(actual,&expected["registry"],"queued immediate action{index}");}
    }
    #[test]
    fn native_unit_lists_pending_wakes_and_constraint_regrouping_are_source_owned(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-unit-registry.json")).unwrap();let actual=replay(&data);
        for (i,(a,b)) in actual.iter().zip(data["states"].as_array().unwrap()).enumerate(){assert_eq!(a,&b["registry"],"action{i}: {}",b["action"]);}
        assert_eq!(actual.len(),115);
        let mut perturbed=data.clone();for row in perturbed["states"].as_array_mut().unwrap(){row["registry"]=serde_json::json!({"wrong_expected_state":true});}
        assert_eq!(actual,replay(&perturbed));
    }
    #[test]
    fn native_wider_unit_windows_preserve_shared_poll_ownership(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-unit-wide.json")).unwrap();
        assert_eq!(data["states"].as_array().unwrap().iter().map(|s|s["registry"]["active"].as_array().unwrap().len()).max(),Some(6));
        let actual=replay(&data);
        for (i,(a,b)) in actual.iter().zip(data["states"].as_array().unwrap()).enumerate(){assert_eq!(a,&b["registry"],"wide action{i}: {}",b["action"]);}
        assert_eq!(actual.len(),514);
        let mut perturbed=data.clone();for row in perturbed["states"].as_array_mut().unwrap(){row["registry"]=serde_json::Value::Null;}
        assert_eq!(actual,replay(&perturbed));
    }

    #[test]
    fn native_unit_owned_resets_select_predictors_and_hulls_without_recorded_memberships(){
        use crate::{ivp_registered_reset::Bodies,ivp_motion::{Prediction,ObjectFrame},ivp_hull_budget::Budget,ivp_world_cache::Context};
        use bevy::prelude::Vec3;
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-unit-reset.json")).unwrap();
        let mut resets=0;let mut touched=0;let mut untouched=0;
        let mut check=|r:&Registry,world:&mut crate::ivp_event_world::World,index:usize|{
            let before=&data["states"][index-1]["clocks"];let after=&data["states"][index]["clocks"];
            assert_eq!(world.now.to_bits(),before["now"].as_f64().unwrap().to_bits());
            assert_eq!(world.start.to_bits(),before["start"].as_f64().unwrap().to_bits());
            assert_eq!(world.clock_serial as u64,before["serial"].as_u64().unwrap());
            let mut bodies=Bodies::new(8).unwrap();let mut objects=BTreeMap::new();
            // Pre-reset numeric packets are local-operation inputs. Membership
            // comes only from the continuously replayed authored registry API.
            // Reverse registration deliberately separates owner/cache IDs.
            for body in before["bodies"].as_array().unwrap().iter().rev(){
                let id=body["id"].as_u64().unwrap() as usize;
                let prediction=Prediction{start:position(id),velocity:Vec3::ZERO,rotation_start:[0.,0.,0.,1.],rotation_end:[0.,0.,0.,1.],time:f64::from_bits(body["prediction_time_bits"].as_u64().unwrap()),inverse_dt:60.};
                bodies.register_core(id,prediction).unwrap();
                let budget=Budget{time:f64::from_bits(body["hull_time_bits"].as_u64().unwrap()),rates:[0.5,0.25],accumulated:[0.2,0.1],predicted:0.3,threshold:1.};
                let object=bodies.register_object(id,0x101,ObjectFrame{shift:None,rotation:None},budget).unwrap();objects.insert(id,object);
                bodies.cache.access(object,Context{time:world.now,frame:world.clock_serial}).unwrap();
            }
            let payloads=objects.values().map(|&id|(id,crate::ivp_cache_lifecycle_tests::pose_crc(&bodies.cache.object_frame(id).unwrap()))).collect::<BTreeMap<_,_>>();
            let slots=bodies.cache.metadata().0.to_vec();let old_budgets=bodies.budgets.clone();
            let selected=bodies.reset_time(r,world).unwrap();
            assert_eq!(selected,r.active_reset_cores().iter().map(|id|objects[id]).collect::<Vec<_>>());
            assert_eq!(world.now.to_bits(),after["now"].as_f64().unwrap().to_bits());assert_eq!(world.start.to_bits(),after["start"].as_f64().unwrap().to_bits());assert_eq!(world.clock_serial as u64,after["serial"].as_u64().unwrap());
            assert_eq!(bodies.cache.metadata().0,slots);
            for body in after["bodies"].as_array().unwrap(){
                let id=body["id"].as_u64().unwrap() as usize;let object=objects[&id];
                assert_eq!(bodies.cache.object_inputs(object).unwrap().0.time.to_bits(),body["prediction_time_bits"].as_u64().unwrap(),"reset{index} core{id}");
                assert_eq!(bodies.budgets[object].time.to_bits(),body["hull_time_bits"].as_u64().unwrap(),"reset{index} hull{id}");
                let mut numeric=bodies.budgets[object];numeric.time=old_budgets[object].time;assert_eq!(numeric,old_budgets[object]);
                assert_eq!(crate::ivp_cache_lifecycle_tests::pose_crc(&bodies.cache.object_frame(object).unwrap()),payloads[&object]);
                if selected.contains(&object){touched+=1;}else{untouched+=1;}
            }
            resets+=1;
        };
        let actual=replay_with_reset(&data,&mut check);
        for (i,(source,native)) in actual.iter().zip(data["states"].as_array().unwrap()).enumerate(){assert_eq!(source,&native["registry"],"reset registry action{i}");}
        // Expected post-reset memberships never select participants.
        let mut corrupt=data.clone();for row in corrupt["states"].as_array_mut().unwrap(){row["registry"]=serde_json::Value::Null;}
        assert_eq!(actual,replay_with_reset(&corrupt,&mut check));
        drop(check);assert_eq!(actual.len(),195);assert_eq!(resets,162);assert!(touched>0&&untouched>0);
    }

    #[test]
    fn native_controller_references_priorities_and_rebuild_ties_are_source_owned(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-controller-registry.json")).unwrap();
        let run=|data:&serde_json::Value|{
            let mut controllers=vec![];let mut visits=vec![];
            let registry=replay_observed(data,|_,w,_|{w.reset_time().unwrap();},|r,_|{controllers.push(r.controller_snapshot());visits.push(r.active_reset_visits());});
            (registry,controllers,visits)
        };
        let actual=run(&data);assert_eq!(actual.0.len(),534);
        for (i,row) in data["states"].as_array().unwrap().iter().enumerate(){
            assert_eq!(actual.0[i],row["registry"],"controller registry action{i}");
            assert_eq!(actual.1[i],row["controllers"],"controller references action{i}");
            let mut expected=vec![];
            for unit in row["controllers"]["units"].as_array().unwrap().iter().take(row["registry"]["active"].as_array().unwrap().len()){
                for entry in unit["controllers"].as_array().unwrap().iter().rev(){
                    let key=match entry["controller"]["kind"].as_str().unwrap(){"base"=>ControllerKey::Base,"joint_0"=>ControllerKey::Joint(0),"joint_1"=>ControllerKey::Joint(1),_=>panic!("unknown native controller")};expected.push(ResetVisit::Controller(key));
                }
                for core in unit["cores"].as_array().unwrap().iter().rev(){expected.push(ResetVisit::Core(core.as_u64().unwrap() as usize));}
            }
            assert_eq!(actual.2[i],expected,"native reset traversal action{i}");
        }
        let mut corrupt=data.clone();for row in corrupt["states"].as_array_mut().unwrap(){row["registry"]=serde_json::Value::Null;row["controllers"]=serde_json::Value::Null;}
        assert_eq!(actual,run(&corrupt));
    }

    #[test]
    fn native_live_controller_dispatch_matches_owned_units_and_movement_classification_order(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-controller-dispatch.json")).unwrap();
        let run=|data:&serde_json::Value|{
            let mut calls=vec![];
            let snapshots=replay_with_execution(data,|_,world,_|{world.reset_time().unwrap();},|_,_|{},|index,key,cores|{
                let kind=match key{ControllerKey::Base=>0,ControllerKey::Joint(id)=>id+1,ControllerKey::Contact{..}=>panic!("legacy dispatch fixture contains contact controllers")};
                calls.push(serde_json::json!({"action":index,"kind":kind,"dt_bits":(1.0f32/60.).to_bits(),"cores":cores}));
            });(snapshots,calls)
        };
        let actual=run(&data);assert_eq!(actual.1,data["calls"].as_array().unwrap().clone());
        for (i,row) in data["states"].as_array().unwrap().iter().enumerate(){assert_eq!(actual.0[i],row["registry"],"live controller action{i}");}
        let mut corrupt=data.clone();corrupt["calls"]=serde_json::Value::Null;for row in corrupt["states"].as_array_mut().unwrap(){row["registry"]=serde_json::Value::Null;row["controllers"]=serde_json::Value::Null;}
        assert_eq!(actual,run(&corrupt));
    }

}
