//! Source-owned centred-sphere broad/far/active metadata over a borrowed body bank.
//! Physics response and body ownership remain with the caller. No captured state
//! or event times supply lifecycle decisions.
use bevy::prelude::*;
use std::collections::BTreeMap;
use crate::{ivp_trial_bounds as bounds,ivp_distance_schedule as schedule,ivp_far_distance as far,
    ivp_hull_budget::Budget,ivp_event_queue::Queue,ivp_joint_apply::Body,
    ivp_world_cache::{WorldCache,Context as Clock},ivp_motion::ObjectFrame};
const PAIR_BASE:usize=1usize<<(usize::BITS-1);
#[derive(Clone)]
struct Pair{broad:usize,objects:[usize;2],geometry:far::Geometry,frame:u32,far:bool,handle:Option<u16>}
#[derive(Clone)]
pub(crate) struct Owner{
    bounds:bounds::World,cache:WorldCache,graph:crate::ivp_mindist_graph::Graph,
    pairs:BTreeMap<usize,Pair>,radii:Vec<f32>,ranks:Vec<u32>,flags:Vec<u32>,counter:u32,dt:f32,
    revision:u32,core_revisions:Vec<u32>,config:schedule::Config,
}
#[derive(Debug,PartialEq)]
pub(crate) enum Event{Impact(usize),Deferred,Retry}
pub(crate) struct Active{pub objects:[usize;2],pub flags:u32,pub frame:u32,pub geometry:far::Geometry,pub handle:Option<u16>,pub scheduled:Option<(f64,f32)>}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum Phase{Inactive,Prediction,Predicted,Timed,Impact,Rebuilt}
pub(crate) struct Observation<'a>{pub phase:Phase,pub clock:Clock,pub objects:Option<[usize;2]>,pub flags:u32,pub bodies:&'a[Body]}
struct Context<'a>{cache:&'a mut WorldCache,graph:&'a mut crate::ivp_mindist_graph::Graph,pairs:&'a mut BTreeMap<usize,Pair>,
    radii:&'a[f32],ranks:&'a[u32],bodies:&'a[Body],counter:&'a mut u32,queue:&'a mut Queue,clock:Clock,deadline:f64,dt:f32,config:schedule::Config}
impl Owner{
    fn prepare_contact_cache(&mut self,bodies:&[Body],ids:&[usize],clock:Clock)->Result<(),&'static str>{
        if bodies.len()!=self.radii.len(){return Err("contact cache bank membership changed");}
        for &id in ids{
            let b=bodies.get(id).ok_or("unknown contact cache core")?;
            self.cache.update_core(id,b.prediction)?;self.cache.set_flags(id,b.object_flags)?;self.cache.access(id,clock)?;
        }Ok(())
    }
    pub(crate) fn new(bodies:&[Body],radii:Vec<f32>,order:&[usize],dt:f32)->Result<Self,&'static str>{
        Self::new_with_config(bodies,radii,order,dt,schedule::Config::bare_native())
    }
    pub(crate) fn new_with_config(bodies:&[Body],radii:Vec<f32>,order:&[usize],dt:f32,config:schedule::Config)->Result<Self,&'static str>{
        if config.targets.iter().any(|v|!v.is_finite()||*v<0.)||![config.slow_gate,config.time_scale,config.gap].iter().all(|v|v.is_finite()&&*v>=0.){return Err("invalid sphere scheduling configuration");}

        if bodies.len()!=radii.len()||order.len()!=bodies.len()||bodies.len()>u32::MAX as usize||bodies.iter().enumerate().any(|(id,b)|id!=b.id)||radii.iter().any(|r|!r.is_finite()||*r<=0.){return Err("invalid centred sphere owner bindings");}
        let mut ranks=vec![u32::MAX;bodies.len()];for (rank,&id)in order.iter().enumerate(){if id>=ranks.len()||ranks[id]!=u32::MAX{return Err("invalid sphere creation order");}ranks[id]=rank as u32;}
        let objects=bodies.iter().map(|b|bounds::Object::new(b.id,b.object_flags,b.flags&2!=0,radii[b.id],b.current.position.to_array())).collect();
        let bounds=bounds::World::new(objects,order,dt)?;let mut cache=WorldCache::new((bodies.len()*2).max(8).next_power_of_two())?;
        for b in bodies{let core=cache.register_core(b.prediction);cache.register_object(core,b.object_flags,ObjectFrame{shift:None,rotation:None})?;}
        Ok(Self{bounds,cache,graph:crate::ivp_mindist_graph::Graph::new(bodies.len()),pairs:BTreeMap::new(),radii,ranks,flags:bodies.iter().map(|b|b.object_flags).collect(),counter:0,dt,revision:0,core_revisions:vec![0;bodies.len()],config})
    }
    fn notify_objects(&mut self,bodies:&[Body],notifications:&[usize],clock:Clock,deadline:f64,queue:&mut Queue)->Result<(),&'static str>{
        let mut context=Context{cache:&mut self.cache,graph:&mut self.graph,pairs:&mut self.pairs,radii:&self.radii,ranks:&self.ranks,bodies,counter:&mut self.counter,queue,clock,deadline,dt:self.dt,config:self.config};
        for &id in notifications{
            let b=bodies.get(id).ok_or("unknown bounds notification core")?;
            if self.flags[b.id]!=b.object_flags{
                let before=self.flags[b.id];let after=b.object_flags;
                if before&!255==after&!255&&(1..=3).contains(&(before&255))&&(1..=3).contains(&(after&255)){
                    self.bounds.set_flags(b.id,after);self.flags[b.id]=after;continue;
                }
                if b.object_flags&0xff==8{
                    self.bounds.sleep_with_flags_and_hooks(b.id,clock.time,self.dt,b.current.position.to_array(),b.object_flags,&mut context)?;
                    context.cache.detach(b.id)?;
                }else{self.bounds.notify_with_hooks(b.id,clock.time,self.dt,b.object_flags,b.current.position.to_array(),0.,0.,&mut context)?;}
                self.flags[b.id]=b.object_flags;
            }
        }
        Ok(())
    }
    pub(crate) fn predict(&mut self,bodies:&[Body],order:&[usize],notifications:&[usize],clock:Clock,deadline:f64,queue:&mut Queue)->Result<(),&'static str>{
        if bodies.len()!=self.radii.len(){return Err("sphere owner body count changed without registration");}
        for b in bodies{self.cache.update_core(b.id,b.prediction)?;self.cache.set_flags(b.id,b.object_flags)?;}
        self.notify_objects(bodies,notifications,clock,deadline,queue)?;
        if bodies.iter().any(|b|self.flags[b.id]!=b.object_flags){return Err("object flag transition lacks a registry notification");}
        let mut context=Context{cache:&mut self.cache,graph:&mut self.graph,pairs:&mut self.pairs,radii:&self.radii,ranks:&self.ranks,bodies,counter:&mut self.counter,queue,clock,deadline,dt:self.dt,config:self.config};
        let predictions=order.iter().map(|&object|bounds::Prediction{object,center:bodies[object].prediction.sample(clock.time,ObjectFrame{shift:None,rotation:None}).position.to_array(),linear:crate::ivp_bounds::speed(context.velocity(object)),angular:0.}).collect::<Vec<_>>();
        let prepared=self.bounds.prepare_predictions(clock.time,self.dt,&predictions)?;
        self.bounds.dispatch_predictions(prepared,&mut context)?;
        for id in context.graph.active().to_vec(){context.refresh(id)?;}
        for id in context.graph.active().to_vec(){
            let objects=context.pairs[&id].objects;let budgets=[self.bounds.object_budget(objects[0])?,self.bounds.object_budget(objects[1])?];
            if let Some(keys)=context.schedule(id,true,1,budgets)?{self.bounds.register_distance(id,objects,keys)?;}
        }
        Ok(())
    }
    ///100eb7d0/100e8cd0: visit completed-unit cores backwards and refresh
    /// their inactive object heads before graph repair. This phase relinks valid
    /// geometry without scheduling; ordinary sphere queries always return1.
    fn refresh_inactive_unit(&mut self,bodies:&[Body],members:&[usize],clock:Clock,deadline:f64,queue:&mut Queue)->Result<usize,&'static str>{
        if bodies.len()!=self.radii.len(){return Err("sphere inactive body membership changed");}
        let mut count=0;
        for &object in members.iter().rev(){
            let links=self.graph.inactive_object_links(object)?.to_vec();
            for link in links{
                if !self.graph.inactive_object_links(object)?.contains(&link){return Err("sphere inactive notification lost saved object link");}
                let objects=self.graph.objects(link.pair)?;
                for id in objects{self.cache.update_core(id,bodies[id].prediction)?;self.cache.set_flags(id,bodies[id].object_flags)?;}
                let mut context=Context{cache:&mut self.cache,graph:&mut self.graph,pairs:&mut self.pairs,radii:&self.radii,ranks:&self.ranks,bodies,counter:&mut self.counter,queue,clock,deadline,dt:self.dt,config:self.config};
                context.refresh(link.pair)?;
                if context.graph.flags(link.pair)?&0xc000!=0x4000{context.graph.reactivate(link.pair,[false;2])?;}
                count+=1;
            }
        }
        Ok(count)
    }
    /// Advance physical state and discovery as one transaction. The actual PSI
    /// dispatcher supplies clock generation and queue ownership; no public-exit
    /// phase reconstruction or copied persistent physical bank is involved.
    /// Unsupported hold/wake/anomaly branches still roll back both owners.
    pub(crate) fn advance(&mut self,physical:&mut crate::ivp_registered_contacts::ContactWorld,delta:f32)->Result<(),&'static str>{self.advance_observed(physical,delta,|_|Ok(()))}
    pub(crate) fn assign_material(&mut self,physical:&mut crate::ivp_registered_contacts::ContactWorld,object:u64,index:i32)->Result<(),&'static str>{
        let mut next=self.clone();let mut world=physical.clone();if !world.prepare_material_assignment(object,index)?{return Ok(());}let core=world.object_binding(object)?.0;
        world.with_candidate_context(|context,clock,deadline,queue|next.wake_cores_observed([core],context,clock,deadline,queue,true,|_,_|Ok(())))?;
        world.refresh_material_assignment_cached(object,&mut next.cache)?;*self=next;*physical=world;Ok(())
    }
    pub(crate) fn wake_initial(&mut self,physical:&mut crate::ivp_registered_contacts::ContactWorld,core:usize,whole:bool)->Result<(),&'static str>{
        let mut next=self.clone();physical.with_candidate_context(|context,clock,deadline,queue|next.wake_cores_observed([core],context,clock,deadline,queue,whole,|_,_|Ok(())))?;*self=next;Ok(())
    }
    pub(crate) fn advance_observed(&mut self,physical:&mut crate::ivp_registered_contacts::ContactWorld,delta:f32,mut observe:impl FnMut(Observation<'_>)->Result<(),&'static str>)->Result<(),&'static str>{
        let next=std::cell::RefCell::new(self.clone());let observe=std::cell::RefCell::new(&mut observe);
        physical.advance_with_lifecycle_context(delta,|bodies,ids,clock|next.borrow_mut().prepare_contact_cache(bodies,ids,clock),|context,clock,deadline,queue|{
            let queued=context.pending_cores().into_iter().rev().collect::<Vec<_>>();next.borrow_mut().wake_cores_observed(queued,context,clock,deadline,queue,true,|_,_|Ok(()))?;
            let notices=context.take_object_notifications();next.borrow_mut().notify_objects(context.bodies()?,&notices,clock,deadline,queue)
        },|context,members,clock,deadline,queue|{
            next.borrow_mut().refresh_inactive_unit(context.bodies()?,members,clock,deadline,queue)?;
            observe.borrow_mut()(Observation{phase:Phase::Inactive,clock,objects:None,flags:0,bodies:context.bodies()?})
        },|context,order,notices,clock,deadline,queue|{
            observe.borrow_mut()(Observation{phase:Phase::Prediction,clock,objects:None,flags:0,bodies:context.bodies()?})?;
            next.borrow_mut().predict(context.bodies()?,order,notices,clock,deadline,queue)?;
            observe.borrow_mut()(Observation{phase:Phase::Predicted,clock,objects:None,flags:0,bodies:context.bodies()?})
        },|context,id,clock,deadline,queue|{
            let mut owner=next.borrow_mut();let pair=id.wrapping_sub(PAIR_BASE);let objects=owner.pairs.get(&pair).ok_or("unknown observed sphere event")?.objects;
            observe.borrow_mut()(Observation{phase:Phase::Timed,clock,objects:Some(objects),flags:owner.graph.flags(pair)?,bodies:context.bodies()?})?;
            match owner.event(id,context.bodies()?,clock,deadline,queue)?{
                Event::Impact(pair)=>{
                    observe.borrow_mut()(Observation{phase:Phase::Impact,clock,objects:Some(objects),flags:owner.graph.flags(pair)?,bodies:context.bodies()?})?;
                    owner.wake_impact_endpoints(pair,context,clock,deadline,queue)?;let admitted=owner.admit(pair,context,clock)?;
                    if !context.has_impact_elasticities(admitted.key)?{return Err("contact-world immediate impact response is not connected");}
                    context.primary_impact(admitted.key)?;let mut wave=crate::ivp_impact_wave::Wave::new_cached(context,admitted,&mut owner.cache,clock)?;
                    while wave.step_cached(context,&mut owner.cache,clock)?.is_some(){wave.continue_iteration()?;}
                    let changed=wave.rebuild(context)?;owner.rebuild_bounds(context.bodies()?,&changed,clock,deadline,queue)?;owner.recheck_impact_pairs(context.bodies()?,&changed,clock,deadline,queue)?;
                    observe.borrow_mut()(Observation{phase:Phase::Rebuilt,clock,objects:Some(objects),flags:owner.graph.flags(pair).unwrap_or(0),bodies:context.bodies()?})
                },Event::Deferred|Event::Retry=>Ok(())
            }
        })?;*self=next.into_inner();Ok(())
    }
    ///1010bc90 rechecks geometry before invoking an impact or retrying. The
    /// dispatcher has already removed the queued entry. Impact response itself
    /// remains the caller's next phase; an event is not an unconditional hit.
    pub(crate) fn event(&mut self,owner:usize,bodies:&[Body],clock:Clock,deadline:f64,queue:&mut Queue)->Result<Event,&'static str>{
        let id=owner.checked_sub(PAIR_BASE).ok_or("foreign sphere event identity")?;
        if !self.graph.active().contains(&id)||!self.pairs.contains_key(&id)||queue.scheduled(owner).is_some()||bodies.len()!=self.radii.len(){return Err("invalid dispatched sphere event");}
        self.pairs.get_mut(&id).unwrap().handle=None;
        for b in bodies{self.cache.update_core(b.id,b.prediction)?;self.cache.set_flags(b.id,b.object_flags)?;}
        let mut context=Context{cache:&mut self.cache,graph:&mut self.graph,pairs:&mut self.pairs,radii:&self.radii,ranks:&self.ranks,bodies,counter:&mut self.counter,queue,clock,deadline,dt:self.dt,config:self.config};
        context.refresh(id)?;let flags=context.graph.flags(id)?;
        if flags&0xc000!=0{return Ok(Event::Deferred);}
        let delay=if flags&0xf==0{
            let threshold=context.config.target(flags)?+f32::from_bits(975594773);
            if context.pairs[&id].geometry.distance<threshold{return Ok(Event::Impact(id));}2
        }else{1};
        let objects=context.pairs[&id].objects;let budgets=[self.bounds.object_budget(objects[0])?,self.bounds.object_budget(objects[1])?];
        if context.schedule(id,false,delay,budgets)?.is_some(){return Err("sphere retry unexpectedly transferred to far tracking");}
        Ok(Event::Retry)
    }
    pub(crate) fn admit(&mut self,id:usize,context:&mut crate::ivp_registered_contacts::CollisionContext<'_>,clock:Clock)->Result<crate::ivp_registered_contacts::AdmittedSphere,&'static str>{
        let p=self.pairs.get(&id).ok_or("unknown sphere impact pair")?;
        let objects=p.objects;let radii=objects.map(|id|self.radii[id]);let flags=self.graph.flags(id)?;
        //10105650 retains/refreshes endpoints after wake. A dormant object's
        // formerly static cache may still carry the previous time generation.
        let leases=self.cache.retain_pair(objects,clock)?;
        let cached=[self.cache.lease_frame(&leases.0)?,self.cache.lease_frame(&leases.1)?];
        let admitted=self.cache.with_leases(leases,|release|{let result=context.admit_spheres_with_frames(flags,objects,radii,Some(cached));release();result})?;
        //10104820 advances the collision revision independently of time serial.
        self.revision=self.revision.wrapping_add(1);Ok(admitted)
    }
    pub(crate) fn wake_impact_endpoints(&mut self,id:usize,physical:&mut crate::ivp_registered_contacts::CollisionContext<'_>,clock:Clock,deadline:f64,queue:&mut Queue)->Result<(),&'static str>{
        self.wake_impact_endpoints_observed(id,physical,clock,deadline,queue,|_,_|Ok(()))
    }
    fn wake_impact_endpoints_observed(&mut self,id:usize,physical:&mut crate::ivp_registered_contacts::CollisionContext<'_>,clock:Clock,deadline:f64,queue:&mut Queue,observe:impl FnMut(&Self,&crate::ivp_registered_contacts::CollisionContext<'_>)->Result<(),&'static str>)->Result<(),&'static str>{
        let objects=self.pairs.get(&id).ok_or("unknown waking impact pair")?.objects;
        self.wake_cores_observed(objects,physical,clock,deadline,queue,true,observe)
    }
    fn wake_cores_observed(&mut self,objects:impl IntoIterator<Item=usize>,physical:&mut crate::ivp_registered_contacts::CollisionContext<'_>,clock:Clock,deadline:f64,queue:&mut Queue,whole:bool,mut observe:impl FnMut(&Self,&crate::ivp_registered_contacts::CollisionContext<'_>)->Result<(),&'static str>)->Result<(),&'static str>{
        for core in objects{
            if physical.bodies()?[core].object_flags&255!=8{continue;}
            let mut initial=true;
            while let Some(object)=if whole{physical.dormant_unit_members(core)?.first().copied()}else if initial{initial=false;Some(core)}else{None}{
                let activated=physical.wake_impact_core(object)?;if activated!=[object]{return Err("member wake produced unrelated notifications");}
                let saved={let b=&mut physical.bodies_mut()?[object];let saved=(b.flags,b.object_flags);b.flags=(b.flags&0xff)|0x2100;b.object_flags=(b.object_flags&!0xff)|0x21;saved};
                let result={let bodies=physical.bodies()?;let b=bodies[object];
                    self.cache.update_core(object,b.prediction)?;self.cache.set_flags(object,b.object_flags)?;
                    let mut context=Context{cache:&mut self.cache,graph:&mut self.graph,pairs:&mut self.pairs,radii:&self.radii,ranks:&self.ranks,bodies,counter:&mut self.counter,queue,clock,deadline,dt:self.dt,config:self.config};
                    self.bounds.notify_with_hooks(object,clock.time,self.dt,b.object_flags,b.current.position.to_array(),0.,0.,&mut context)
                };
                let b=&mut physical.bodies_mut()?[object];b.flags=saved.0;b.object_flags=saved.1;
                self.flags[object]=saved.1;self.bounds.set_flags(object,saved.1);self.cache.set_flags(object,saved.1)?;result?;
                //100fdf50 follows each object's current mindist links. Membership
                // is rechecked after every admission because it can merge units.
                for link in self.graph.object_links(object)?.to_vec(){
                    let pair=link.pair;let ends=self.pairs.get(&pair).ok_or("wake lost mindist pair")?.objects;let other=ends[link.side^1];
                    if self.graph.flags(pair)?&0x3000!=0||physical.bodies()?[other].flags&2!=0||physical.core_has_contact_system(other){continue;}
                    {let mut context=Context{cache:&mut self.cache,graph:&mut self.graph,pairs:&mut self.pairs,radii:&self.radii,ranks:&self.ranks,bodies:physical.bodies()?,counter:&mut self.counter,queue,clock,deadline,dt:self.dt,config:self.config};context.refresh(pair)?;}
                    if self.graph.flags(pair)?&0xc000!=0||!(self.pairs[&pair].geometry.distance<f32::from_bits(1021967909)){continue;}
                    let leases=self.cache.retain_pair(ends,clock)?;
                    let frames=[self.cache.lease_frame(&leases.0)?,self.cache.lease_frame(&leases.1)?];let flags=self.graph.flags(pair)?;
                    self.cache.with_leases(leases,|release|{let result=physical.admit_wake_neighbour(flags,ends,frames,object,other);release();result})?;
                }
                //100eab20 restarts the reverse unit scan after a wake admits
                //new contacts: its retained unit may now contain dormant cores.
                observe(self,physical)?;
            }
            if whole{physical.finish_impact_wake(core)?;}
        }Ok(())
    }
    ///10104320 prepares all changed cores' remaining-tick budgets before the
    /// reverse hull dispatch. Core revision stamping/rechecks follow separately.
    pub(crate) fn rebuild_bounds(&mut self,bodies:&[Body],changed:&[usize],clock:Clock,deadline:f64,queue:&mut Queue)->Result<(),&'static str>{
        let remaining=(deadline-clock.time) as f32;
        if bodies.len()!=self.radii.len()||!remaining.is_finite()||remaining<0.{return Err("invalid post-impact bounds bank/interval");}
        let mut unique=std::collections::BTreeSet::new();
        if changed.iter().any(|&id|id>=bodies.len()||!unique.insert(id)||bodies[id].flags&0x12!=0){return Err("invalid post-impact changed cores");}
        for b in bodies{self.cache.update_core(b.id,b.prediction)?;self.cache.set_flags(b.id,b.object_flags)?;}
        let mut context=Context{cache:&mut self.cache,graph:&mut self.graph,pairs:&mut self.pairs,radii:&self.radii,ranks:&self.ranks,bodies,counter:&mut self.counter,queue,clock,deadline,dt:self.dt,config:self.config};
        let predictions=changed.iter().rev().map(|&object|bounds::Prediction{object,center:bodies[object].prediction.sample(clock.time,ObjectFrame{shift:None,rotation:None}).position.to_array(),linear:crate::ivp_bounds::speed(context.velocity(object)),angular:0.}).collect::<Vec<_>>();
        let prepared=self.bounds.prepare_predictions(clock.time,remaining,&predictions)?;
        self.bounds.dispatch_predictions(prepared,&mut context)?;Ok(())
    }
    ///100f1600 stamps one changed core, then100e8b20 walks its object links.
    /// Equal revisions skip the pair; time-cache generations are independent.
    pub(crate) fn recheck_impact_pairs(&mut self,bodies:&[Body],changed:&[usize],clock:Clock,deadline:f64,queue:&mut Queue)->Result<(),&'static str>{
        let mut context=Context{cache:&mut self.cache,graph:&mut self.graph,pairs:&mut self.pairs,radii:&self.radii,ranks:&self.ranks,bodies,counter:&mut self.counter,queue,clock,deadline,dt:self.dt,config:self.config};
        for &core in changed.iter().rev(){
            *self.core_revisions.get_mut(core).ok_or("unknown impact revision core")?=self.revision;
            for link in context.graph.object_links(core)?.to_vec(){
                let objects=context.pairs[&link.pair].objects;
                if self.core_revisions[objects[0]]==self.core_revisions[objects[1]]{continue;}
                context.refresh(link.pair)?;
                if context.graph.flags(link.pair)?&0xc000==0{
                    let budgets=[self.bounds.object_budget(objects[0])?,self.bounds.object_budget(objects[1])?];
                    if context.schedule(link.pair,false,2,budgets)?.is_some(){return Err("impact recheck unexpectedly transferred to far tracking");}
                }
            }
        }Ok(())
    }
    pub(crate) fn counter(&self)->u32{self.counter}
    pub(crate) fn active(&self,queue:&Queue)->Result<Vec<Active>,&'static str>{self.graph.active().iter().map(|&id|{let p=&self.pairs[&id];Ok(Active{objects:p.objects,flags:self.graph.flags(id)?,frame:p.frame,geometry:p.geometry,handle:p.handle,scheduled:queue.scheduled(PAIR_BASE+id)})}).collect()}
}
impl Context<'_>{
    fn retire_children(&mut self,broad:usize)->Result<Vec<bounds::DistanceEdit>,&'static str>{
        let ids=self.pairs.iter().filter_map(|(&id,p)|(p.broad==broad).then_some(id)).collect::<Vec<_>>();let mut edits=vec![];
        for id in ids{let p=self.pairs.remove(&id).unwrap();if self.queue.scheduled(PAIR_BASE+id).is_some(){self.queue.cancel(PAIR_BASE+id)?;}self.graph.remove(id)?;if p.far{edits.push(bounds::DistanceEdit::Clear{pair:id});}}Ok(edits)
    }
    fn velocity(&self,id:usize)->Vec3{Vec3::from_array(std::array::from_fn(|k|self.bodies[id].linear[k]))}
    fn refresh(&mut self,id:usize)->Result<(),&'static str>{
        let p=&self.pairs[&id];if p.frame==self.clock.frame{return Ok(());}
        let objects=p.objects;let leases=self.cache.retain_pair(objects,self.clock)?;
        let frames=[self.cache.lease_frame(&leases.0)?,self.cache.lease_frame(&leases.1)?];
        let geometry=crate::ivp_minimizer::sphere_geometry(frames.map(|c|c.pose.position),frames.map(|c|c.core_position),self.radii[objects[0]]+self.radii[objects[1]]);
        let geometry=self.cache.with_leases(leases,|release|{release();geometry})?;
        let p=self.pairs.get_mut(&id).unwrap();p.geometry.projection=geometry.projection;p.geometry.distance=geometry.distance;p.geometry.normal=geometry.normal;p.frame=self.clock.frame;
        let flags=self.graph.flags(id)?;self.graph.set_flags(id,flags&!0xc000)?;Ok(())
    }
    fn schedule(&mut self,id:usize,allow_far:bool,delay:u8,budgets:[Budget;2])->Result<Option<[f32;2]>,&'static str>{
        let p=&self.pairs[&id];let objects=p.objects;let flags=self.graph.flags(id)?;let config=self.config;
        let input=schedule::Input{flags,counter:*self.counter,now:self.clock.time,deadline:self.deadline,dt:self.dt,distance:p.geometry.distance,target:config.target(flags)?,normal:p.geometry.normal,
            cores:objects.map(|object|schedule::Core{motion:crate::ivp_bounds::Motion{axis:Vec3::ZERO,angular_bound:0.,velocity:self.velocity(object)},linear:crate::ivp_bounds::speed(self.velocity(object))}),
            budgets:std::array::from_fn(|k|schedule::Budget{time:budgets[k].time,flags:self.bodies[objects[k]].object_flags,rates:budgets[k].rates,accumulated:budgets[k].accumulated}),allow_far,slow_gate:config.slow_gate};
        let descriptors=[self.cache.object_inputs(objects[0])?,self.cache.object_inputs(objects[1])?];
        let leases=self.cache.retain_pair(objects,self.clock)?;let initial=[self.cache.lease_frame(&leases.0)?.pose,self.cache.lease_frame(&leases.1)?.pose];
        let outcome=crate::ivp_sphere_schedule::run(self.queue,PAIR_BASE.checked_add(id).ok_or("sphere pair queue identity overflow")?,crate::ivp_sphere_schedule::Input{schedule:input,initial,inactive:descriptors.map(|d|d.2 as u8 as i8>7),margin:self.radii[objects[0]]+self.radii[objects[1]],config,delay},|side,at|descriptors[side].0.sample(at,descriptors[side].1));
        let outcome=self.cache.with_leases(leases,|release|{release();outcome})?;
        *self.counter=outcome.counter;self.graph.set_flags(id,outcome.flags)?;self.pairs.get_mut(&id).unwrap().handle=outcome.handle;
        if let Some((keys,linear))=outcome.far{self.graph.unlink(id)?;let p=self.pairs.get_mut(&id).unwrap();p.far=true;p.geometry.linear_delta=linear;Ok(Some(keys))}else{Ok(None)}
    }
    fn allow_far(&self,objects:[usize;2])->bool{(self.bodies[objects[0]].object_flags as u8|self.bodies[objects[1]].object_flags as u8)<0x21}
}
impl bounds::Hooks for Context<'_>{
    fn geometry(&mut self,broad:usize,mut objects:[usize;2],margin:f32,mut budgets:[Budget;2])->Result<Vec<bounds::DistanceEdit>,&'static str>{
        // A coarse box overlap alone does not create a compact sphere pair.
        // Both native shape queries must return their sphere feature first.
        let radius=(self.radii[objects[0]]+self.radii[objects[1]])+margin;let mut inside=true;
        for side in 0..2{
            let other=self.bodies[objects[1-side]].prediction.prepare_object_cache(self.clock.time,ObjectFrame{shift:None,rotation:None},self.clock.frame).core_position;
            let slot=self.cache.access(objects[side],self.clock)?;let pose=self.cache.get(slot)?.pose;
            inside&=crate::ivp_minimizer::sphere_query_contains(pose,other,radius)?;
        }
        //1010ce30 renews the feature list even for an existing child. Native
        // destroys children absent from the new candidate set before replacing
        // the coarse listener thresholds.
        if !inside{return self.retire_children(broad);}
        if self.pairs.values().any(|p|p.broad==broad){return Ok(vec![]);}
        //1010cc80 compares the object serials for equal sphere types. The
        // later-created sphere occupies endpoint zero regardless of which
        // broadphase object renewed the coarse pair. No deletion/reuse yet.
        if self.ranks[objects[0]]<self.ranks[objects[1]]{objects.swap(0,1);budgets.swap(0,1);}
        let id=self.graph.register(objects)?;self.graph.activate(id,[false;2])?;
        self.pairs.insert(id,Pair{broad,objects,geometry:far::Geometry{distance:0.,projection:0.,linear_delta:0.,normal:Vec3::ZERO},frame:0,far:false,handle:None});self.refresh(id)?;
        Ok(self.schedule(id,self.allow_far(objects),0,budgets)?.map(|keys|bounds::DistanceEdit::Register{pair:id,objects,keys}).into_iter().collect())
    }
    fn removed(&mut self,broad:usize,_:[usize;2])->Result<Vec<bounds::DistanceEdit>,&'static str>{
        self.retire_children(broad)
    }
    fn distance(&mut self,event:bounds::DistanceEvent)->Result<bounds::DistanceAction,&'static str>{match event{
        bounds::DistanceEvent::Rebase{pair,motion,linear,..}=>{self.pairs.get_mut(&pair).ok_or("unknown sphere far rebase")?.geometry.rebase(motion,linear);Ok(bounds::DistanceAction::Keep)},
        bounds::DistanceEvent::Recheck{pair,objects,gap,budgets,..}=>{
            let p=self.pairs.get(&pair).ok_or("unknown sphere far callback")?;if !p.far||p.objects!=objects{return Err("sphere far callback lost membership");}
            let input=far::Input{flags:self.graph.flags(pair)?,now:self.clock.time,dt:self.dt,gap,geometry:p.geometry,cores:objects.map(|id|{let b=self.bodies[id];far::Core{time:b.prediction.time,start:b.prediction.start,velocity:b.prediction.velocity,linear:crate::ivp_bounds::speed(self.velocity(id)),angular:0.}}),budgets};
            match far::update(&input,far::Config::default())?{
                far::Action::Rearm{geometry,keys,first}=>{self.pairs.get_mut(&pair).unwrap().geometry=geometry;Ok(bounds::DistanceAction::Rearm{keys,first})},
                far::Action::Activate{pending}=>{if pending{return Err("pending sphere far activation is not connected");}Ok(bounds::DistanceAction::ActivateAfterRemoval)},
            }
        }
    }}
    fn activated(&mut self,pair:usize,objects:[usize;2],budgets:[Budget;2])->Result<Option<[f32;2]>,&'static str>{
        let p=self.pairs.get_mut(&pair).ok_or("unknown sphere far activation")?;
        if !p.far||p.objects!=objects{return Err("sphere activation lost paired ownership");}
        p.far=false;self.graph.activate(pair,[false;2])?;self.refresh(pair)?;self.schedule(pair,self.allow_far(objects),0,budgets)
    }

}
#[cfg(test)]pub(crate) mod tests{
    fn budgets(owner:&Owner)->serde_json::Value{serde_json::json!((0..owner.radii.len()).map(|id|{let b=owner.bounds.object_budget(id).unwrap();serde_json::json!({"time":b.time.to_bits(),"rates":b.rates.map(f32::to_bits),"accumulated":b.accumulated.map(f32::to_bits),"predicted":b.predicted.to_bits(),"threshold":b.threshold.to_bits()})}).collect::<Vec<_>>())}
    use super::*;
    fn pairs(owner:&Owner,physical:&crate::ivp_registered_contacts::ContactWorld)->serde_json::Value{
        let queue=&physical.clock().queue;
        serde_json::json!(owner.active(queue).unwrap().into_iter().map(|p|serde_json::json!({"objects":p.objects,"features":[3,3],"flags":p.flags,"query_frame":p.frame,"environment_frame":physical.clock().clock_serial,"margin":p.objects.map(|id|owner.radii[id]).into_iter().sum::<f32>().to_bits(),"measures":[p.geometry.projection.to_bits(),p.geometry.linear_delta.to_bits(),p.geometry.distance.to_bits()],"normal":p.geometry.normal.to_array().map(f32::to_bits),"queued":p.scheduled.is_some(),"handle":p.handle.unwrap_or(u16::MAX),"epoch":queue.epoch.to_bits(),"key":p.scheduled.map_or(0,|(_,key)|key.to_bits())})).collect::<Vec<_>>())
    }
    fn cache_frames(owner:&Owner)->serde_json::Value{serde_json::json!((0..owner.radii.len()).map(|id|owner.cache.object_frame(id).ok().map(|f|serde_json::json!({"frame":f.frame_stamp,"core":f.core_position.to_array().map(f32::to_bits),"position":f.pose.position.to_array().map(f32::to_bits)}))).collect::<Vec<_>>())}
    fn authored_position(c:&serde_json::Value,id:usize,spacing:f32)->[f32;3]{
        c.get("positions").and_then(|p|p.get(id)).map_or([spacing*id as f32,0.,128.],|p|std::array::from_fn(|k|p[k].as_f64().unwrap() as f32))
    }
    fn multibody_world(c:&serde_json::Value)->(Owner,crate::ivp_registered_contacts::ContactWorld){
        let n=c["count"].as_u64().unwrap() as usize;let dt=1./60.;let radius=c["radius"].as_f64().unwrap() as f32;let spacing=c["spacing"].as_f64().unwrap() as f32;
        let gravity=c.get("gravity").map_or(Vec3::new(0.,-0.,0.),|v|{let unit=f32::from_bits(0x3cd013a9);Vec3::new(v[0].as_f64().unwrap() as f32*unit,-(v[2].as_f64().unwrap() as f32*unit),v[1].as_f64().unwrap() as f32*unit)});
        let mut physical=crate::ivp_registered_contacts::ContactWorld::empty_native(dt,c["initial_seed"].as_u64().unwrap() as u32,gravity,Vec3::ZERO).unwrap();
        let spin=Vec3::from_array(std::array::from_fn(|k|c["spin"][k].as_f64().unwrap() as f32));
        for id in 0..n{physical.create_sphere(id as u64+1,crate::ivp_sphere_constructor::Parameters{radius,position:authored_position(c,id,spacing),angles:[0.;3],mass:10.+5.*id as f32,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:c.get("fixed").and_then(|v|v.get(id)).and_then(serde_json::Value::as_bool).unwrap_or(id+1==n&&c["fixed_last"].as_bool().unwrap()),collisions:true},0.8).unwrap();if !c["linear_commands"][id].is_null(){physical.set_velocity(id,Some(Vec3::from_array(std::array::from_fn(|k|c["linear_commands"][id][k].as_f64().unwrap() as f32))),Some(spin)).unwrap();}physical.configure_sphere_elasticity(id,0.).unwrap();}
        if let Some(a)=c.get("public_ragdoll"){
            let f=|index:usize|f32::from_bits(a[index].as_u64().unwrap() as u32);let flags=a[44].as_u64().unwrap();
            let parameters=crate::ivp_ragdoll_constructor::Parameters{frames:std::array::from_fn(|side|std::array::from_fn(|r|std::array::from_fn(|k|f(6+side*12+r*4+k)))),axes:std::array::from_fn(|i|crate::ivp_ragdoll_constructor::Limit{min:f(32+i*4),max:f(33+i*4),velocity:f(34+i*4),torque:f(35+i*4)}),force_limit:f(1),torque_limit:f(2),mass_scale:[f(3),f(4)],angular_only:flags&255!=0,clockwise:flags&0xff0000!=0};
            let offsets=[(1,None),(2,None)].into_iter().collect::<BTreeMap<_,_>>();let joint=physical.ragdoll_definition(0,[1,2],parameters,&offsets).unwrap();
            physical.register_joint_group(0,crate::ivp_registered_physical::GroupDefinition{constraints:vec![joint],offsets,additional:0,minimum:15,tolerance:3.,rescue_gravity:Vec3::new(0.,-0.,0.)}).unwrap();
        }
        (Owner::new(physical.bodies(),vec![radius*f32::from_bits(0x3cd013a9);n],&(0..n).collect::<Vec<_>>(),dt).unwrap(),physical)
    }
    #[test]
    fn native_collision_wake_matches_admission_wave_and_rebuild_phases(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-wake.json")).unwrap())).unwrap();let c=&data["case"];let delta=f32::from_bits(c["delta_bits"].as_u64().unwrap() as u32);
        let (mut owner,mut physical)=multibody_world(c);for _ in 0..data["provenance"]["prefix_calls"].as_u64().unwrap(){owner.advance(&mut physical,delta).unwrap();}
        let bank=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());assert_eq!(bank(physical.bodies()),c["before"]["physical"]);
        let owner=std::cell::RefCell::new(owner);let before=crate::ivp_registered_contacts::snapshot(&physical);let mut called=0;
        let result=physical.advance_with_cache_context(delta,|b,ids,clock|owner.borrow_mut().prepare_contact_cache(b,ids,clock),|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
            called+=1;let mut owner=owner.borrow_mut();let Event::Impact(pair)=owner.event(id,context.bodies()?,clock,deadline,q)? else{panic!("expected sleeping-body impact")};
            assert_eq!(bank(context.bodies()?),c["phases"][2]["state"]["physical"],"checked wake bank");assert_eq!(cache_frames(&owner),c["phases"][2]["state"]["cache_frames"]);
            owner.wake_impact_endpoints(pair,context,clock,deadline,q)?;let admitted=owner.admit(pair,context,clock)?;
            assert_eq!(bank(context.bodies()?),c["phases"][4]["state"]["physical"],"awake admitted bank");assert_eq!(cache_frames(&owner),c["phases"][4]["state"]["cache_frames"]);
            context.primary_impact(admitted.key)?;assert_eq!(bank(context.bodies()?),c["phases"][5]["state"]["physical"],"wake primary");
            let mut wave=crate::ivp_impact_wave::Wave::new_cached(context,admitted,&mut owner.cache,clock)?;assert_eq!(wave.snapshot(-1),c["phases"][6]["wave"]);assert_eq!(cache_frames(&owner),c["phases"][6]["state"]["cache_frames"]);
            assert!(wave.step_cached(context,&mut owner.cache,clock)?.is_none());assert_eq!(wave.snapshot(0),c["phases"][7]["wave"]);assert_eq!(bank(context.bodies()?),c["phases"][7]["state"]["physical"]);
            let changed=wave.rebuild(context)?;owner.rebuild_bounds(context.bodies()?,&changed,clock,deadline,q)?;owner.recheck_impact_pairs(context.bodies()?,&changed,clock,deadline,q)?;
            let expected=&c["phases"][9]["state"];assert_eq!(bank(context.bodies()?),expected["physical"],"wake restamped bank");assert_eq!(budgets(&owner),expected["hull_budgets"]);assert_eq!(cache_frames(&owner),expected["cache_frames"]);assert_eq!(owner.revision as u64,expected["revision_counter"].as_u64().unwrap());assert_eq!(serde_json::json!(owner.core_revisions),expected["body_revisions"]);
            Err("observed native collision wake phases")
        });
        assert_eq!(result,Err("observed native collision wake phases"));assert_eq!(called,1);assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),before);
    }
    #[test]
    fn native_untouched_neighbour_wake_matches_admission_and_rebuild_phases(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-dormant-event.json")).unwrap())).unwrap();let c=&data["case"];let delta=f32::from_bits(c["delta_bits"].as_u64().unwrap() as u32);
        let (mut owner,mut physical)=multibody_world(c);for _ in 0..data["provenance"]["prefix_calls"].as_u64().unwrap(){owner.advance(&mut physical,delta).unwrap();}
        let bank=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());assert_eq!(bank(physical.bodies()),c["before"]["physical"]);
        let owner=std::cell::RefCell::new(owner);let before=crate::ivp_registered_contacts::snapshot(&physical);let mut called=0;
        let result=physical.advance_with_cache_context(delta,|b,ids,clock|owner.borrow_mut().prepare_contact_cache(b,ids,clock),|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
            called+=1;let mut owner=owner.borrow_mut();let Event::Impact(pair)=owner.event(id,context.bodies()?,clock,deadline,q)? else{panic!("expected sleeping-body impact")};
            assert_eq!(bank(context.bodies()?),c["phases"][2]["state"]["physical"],"checked wake bank");assert_eq!(cache_frames(&owner),c["phases"][2]["state"]["cache_frames"]);
            owner.wake_impact_endpoints(pair,context,clock,deadline,q)?;assert_eq!(bank(context.bodies()?),c["phases"][3]["state"]["physical"],"wake neighbour clocks");assert_eq!(cache_frames(&owner),c["phases"][3]["state"]["cache_frames"]);let admitted=owner.admit(pair,context,clock)?;
            assert_eq!(bank(context.bodies()?),c["phases"][5]["state"]["physical"],"awake admitted bank");assert_eq!(cache_frames(&owner),c["phases"][5]["state"]["cache_frames"]);
            context.primary_impact(admitted.key)?;assert_eq!(bank(context.bodies()?),c["phases"][6]["state"]["physical"],"wake primary");
            let mut wave=crate::ivp_impact_wave::Wave::new_cached(context,admitted,&mut owner.cache,clock)?;assert_eq!(wave.snapshot(-1),c["phases"][7]["wave"]);assert_eq!(cache_frames(&owner),c["phases"][7]["state"]["cache_frames"]);
            assert!(wave.step_cached(context,&mut owner.cache,clock)?.is_none());assert_eq!(wave.snapshot(0),c["phases"][8]["wave"]);assert_eq!(bank(context.bodies()?),c["phases"][8]["state"]["physical"]);
            let changed=wave.rebuild(context)?;owner.rebuild_bounds(context.bodies()?,&changed,clock,deadline,q)?;owner.recheck_impact_pairs(context.bodies()?,&changed,clock,deadline,q)?;
            let expected=&c["phases"][10]["state"];assert_eq!(bank(context.bodies()?),expected["physical"],"wake restamped bank");assert_eq!(budgets(&owner),expected["hull_budgets"]);assert_eq!(cache_frames(&owner),expected["cache_frames"]);assert_eq!(owner.revision as u64,expected["revision_counter"].as_u64().unwrap());assert_eq!(serde_json::json!(owner.core_revisions),expected["body_revisions"]);
            Err("observed native collision wake phases")
        });
        assert_eq!(result,Err("observed native collision wake phases"));assert_eq!(called,1);assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),before);
    }
    #[test]
    fn native_cluster_wake_restarts_before_staging_and_coupled_response(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-cluster-event.json")).unwrap())).unwrap();let c=&data["case"];let delta=f32::from_bits(c["delta_bits"].as_u64().unwrap() as u32);
        let (mut owner,mut physical)=multibody_world(c);for _ in 0..data["provenance"]["prefix_calls"].as_u64().unwrap(){owner.advance(&mut physical,delta).unwrap();}
        let bank=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());assert_eq!(bank(physical.bodies()),c["before"]["physical"]);
        let owner=std::cell::RefCell::new(owner);let before=crate::ivp_registered_contacts::snapshot(&physical);let mut called=0;
        let result=physical.advance_with_cache_context(delta,|b,ids,clock|owner.borrow_mut().prepare_contact_cache(b,ids,clock),|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
            called+=1;let mut owner=owner.borrow_mut();let Event::Impact(pair)=owner.event(id,context.bodies()?,clock,deadline,q)? else{panic!("expected sleeping-body impact")};
            assert_eq!(bank(context.bodies()?),c["phases"][2]["state"]["physical"],"checked wake bank");assert_eq!(cache_frames(&owner),c["phases"][2]["state"]["cache_frames"]);
            owner.wake_impact_endpoints(pair,context,clock,deadline,q)?;assert_eq!(bank(context.bodies()?),c["phases"][3]["state"]["physical"],"wake neighbour clocks");assert_eq!(cache_frames(&owner),c["phases"][3]["state"]["cache_frames"]);assert_eq!(owner.counter() as u64,c["phases"][3]["state"]["collision_counter"].as_u64().unwrap());let admitted=owner.admit(pair,context,clock)?;
            assert_eq!(bank(context.bodies()?),c["phases"][5]["state"]["physical"],"awake admitted bank");assert_eq!(cache_frames(&owner),c["phases"][5]["state"]["cache_frames"]);
            context.primary_impact(admitted.key)?;assert_eq!(bank(context.bodies()?),c["phases"][6]["state"]["physical"],"wake primary");
            let mut wave=crate::ivp_impact_wave::Wave::new_cached(context,admitted,&mut owner.cache,clock)?;assert_eq!(wave.snapshot(-1),c["phases"][7]["wave"]);assert_eq!(cache_frames(&owner),c["phases"][7]["state"]["cache_frames"]);
            let phases=c["phases"].as_array().unwrap();let steps=phases[8..].iter().take_while(|p|p["phase"]=="wave_step").collect::<Vec<_>>();assert_eq!(steps.len(),5);
            for expected in steps{let active=wave.step_cached(context,&mut owner.cache,clock)?.is_some();assert_eq!(wave.snapshot(i32::from(active)),expected["wave"]);assert_eq!(bank(context.bodies()?),expected["state"]["physical"]);assert_eq!(cache_frames(&owner),expected["state"]["cache_frames"]);if active{wave.continue_iteration()?;}}

            let changed=wave.rebuild(context)?;owner.rebuild_bounds(context.bodies()?,&changed,clock,deadline,q)?;owner.recheck_impact_pairs(context.bodies()?,&changed,clock,deadline,q)?;
            let expected=&c["phases"].as_array().unwrap().last().unwrap()["state"];assert_eq!(bank(context.bodies()?),expected["physical"],"wake restamped bank");assert_eq!(budgets(&owner),expected["hull_budgets"]);assert_eq!(cache_frames(&owner),expected["cache_frames"]);assert_eq!(owner.revision as u64,expected["revision_counter"].as_u64().unwrap());assert_eq!(serde_json::json!(owner.core_revisions),expected["body_revisions"]);
            Err("observed native collision wake phases")
        });
        assert_eq!(result,Err("observed native collision wake phases"));assert_eq!(called,1);assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),before);
    }
    #[test]
    fn native_gravity_hold_matches_motion_transfer_and_wave(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-gravity-hold.json")).unwrap())).unwrap();let c=&data["case"];let delta=f32::from_bits(c["delta_bits"].as_u64().unwrap() as u32);
        let (mut owner,mut physical)=multibody_world(c);for _ in 0..data["provenance"]["prefix_calls"].as_u64().unwrap(){owner.advance(&mut physical,delta).unwrap();}
        let bank=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());assert_eq!(bank(physical.bodies()),c["before"]["physical"]);
        let owner=std::cell::RefCell::new(owner);let before=crate::ivp_registered_contacts::snapshot(&physical);let mut called=0;
        let result=physical.advance_with_cache_context(delta,|b,ids,clock|owner.borrow_mut().prepare_contact_cache(b,ids,clock),|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
            called+=1;let mut owner=owner.borrow_mut();let Event::Impact(pair)=owner.event(id,context.bodies()?,clock,deadline,q)? else{panic!("expected sleeping-body impact")};
            assert_eq!(bank(context.bodies()?),c["phases"][2]["state"]["physical"],"checked wake bank");assert_eq!(cache_frames(&owner),c["phases"][2]["state"]["cache_frames"]);
            owner.wake_impact_endpoints(pair,context,clock,deadline,q)?;assert_eq!(bank(context.bodies()?),c["phases"][3]["state"]["physical"],"wake neighbour clocks");assert_eq!(cache_frames(&owner),c["phases"][3]["state"]["cache_frames"]);assert_eq!(owner.counter() as u64,c["phases"][3]["state"]["collision_counter"].as_u64().unwrap());let admitted=owner.admit(pair,context,clock)?;
            assert_eq!(bank(context.bodies()?),c["phases"][5]["state"]["physical"],"awake admitted bank");assert_eq!(cache_frames(&owner),c["phases"][5]["state"]["cache_frames"]);
            context.primary_impact(admitted.key)?;assert_eq!(bank(context.bodies()?),c["phases"][6]["state"]["physical"],"wake primary");
            let mut wave=crate::ivp_impact_wave::Wave::new_cached(context,admitted,&mut owner.cache,clock)?;assert_eq!(wave.snapshot(-1),c["phases"][7]["wave"]);assert_eq!(cache_frames(&owner),c["phases"][7]["state"]["cache_frames"]);
            let phases=c["phases"].as_array().unwrap();let steps=phases[8..].iter().take_while(|p|p["phase"]=="wave_step").collect::<Vec<_>>();assert_eq!(steps.len(),10);
            for (step,expected) in steps.into_iter().enumerate(){let active=wave.step_cached(context,&mut owner.cache,clock)?.is_some();assert_eq!(wave.snapshot(i32::from(active)),expected["wave"]);assert_eq!(bank(context.bodies()?),expected["state"]["physical"],"gravity wave step {step}");assert_eq!(cache_frames(&owner),expected["state"]["cache_frames"]);if active{wave.continue_iteration()?;}}

            let changed=wave.rebuild(context)?;owner.rebuild_bounds(context.bodies()?,&changed,clock,deadline,q)?;owner.recheck_impact_pairs(context.bodies()?,&changed,clock,deadline,q)?;
            let expected=&c["phases"].as_array().unwrap().last().unwrap()["state"];assert_eq!(bank(context.bodies()?),expected["physical"],"wake restamped bank");assert_eq!(budgets(&owner),expected["hull_budgets"]);assert_eq!(cache_frames(&owner),expected["cache_frames"]);assert_eq!(owner.revision as u64,expected["revision_counter"].as_u64().unwrap());assert_eq!(serde_json::json!(owner.core_revisions),expected["body_revisions"]);
            Err("observed native collision wake phases")
        });
        assert_eq!(result,Err("observed native collision wake phases"));assert_eq!(called,1);assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),before);
    }
    #[test]
    fn native_prejoined_wake_processes_each_member_before_activating_unit(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-joined-event.json")).unwrap())).unwrap();let c=&data["case"];let delta=f32::from_bits(c["delta_bits"].as_u64().unwrap() as u32);
        let (mut owner,mut physical)=multibody_world(c);for _ in 0..data["provenance"]["prefix_calls"].as_u64().unwrap(){owner.advance(&mut physical,delta).unwrap();}
        let bank=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());assert_eq!(bank(physical.bodies()),c["before"]["physical"]);
        let owner=std::cell::RefCell::new(owner);let before=crate::ivp_registered_contacts::snapshot(&physical);let mut called=0;
        let result=physical.advance_with_cache_context(delta,|b,ids,clock|owner.borrow_mut().prepare_contact_cache(b,ids,clock),|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
            called+=1;let mut owner=owner.borrow_mut();let Event::Impact(pair)=owner.event(id,context.bodies()?,clock,deadline,q)? else{panic!("expected sleeping-body impact")};
            assert_eq!(bank(context.bodies()?),c["phases"][2]["state"]["physical"],"checked wake bank");assert_eq!(cache_frames(&owner),c["phases"][2]["state"]["cache_frames"]);
            let mut wake_visits=0;owner.wake_impact_endpoints_observed(pair,context,clock,deadline,q,|owner,context|{
                let expected=&c["phases"][3+wake_visits]["state"];assert_eq!(bank(context.bodies()?),expected["physical"],"per-core wake bank");assert_eq!(cache_frames(owner),expected["cache_frames"],"per-core cache");assert_eq!(budgets(owner),expected["hull_budgets"],"per-core hulls");assert_eq!(owner.counter() as u64,expected["collision_counter"].as_u64().unwrap());
                let flags=(0..context.bodies()?.len()).map(|id|context.contacts().registry().unit_flags(id).unwrap().flags&0x3fff).collect::<Vec<_>>();assert_eq!(serde_json::json!(flags),expected["unit_flags"],"unit stays dormant during member callbacks");wake_visits+=1;Ok(())
            })?;assert_eq!(wake_visits,2);assert_eq!(bank(context.bodies()?),c["phases"][5]["state"]["physical"],"wake neighbour clocks");assert_eq!(cache_frames(&owner),c["phases"][5]["state"]["cache_frames"]);assert_eq!(owner.counter() as u64,c["phases"][5]["state"]["collision_counter"].as_u64().unwrap());let admitted=owner.admit(pair,context,clock)?;
            assert_eq!(bank(context.bodies()?),c["phases"][7]["state"]["physical"],"awake admitted bank");assert_eq!(cache_frames(&owner),c["phases"][7]["state"]["cache_frames"]);
            context.primary_impact(admitted.key)?;assert_eq!(bank(context.bodies()?),c["phases"][8]["state"]["physical"],"wake primary");
            let mut wave=crate::ivp_impact_wave::Wave::new_cached(context,admitted,&mut owner.cache,clock)?;assert_eq!(wave.snapshot(-1),c["phases"][9]["wave"]);assert_eq!(cache_frames(&owner),c["phases"][9]["state"]["cache_frames"]);
            let phases=c["phases"].as_array().unwrap();let steps=phases[10..].iter().take_while(|p|p["phase"]=="wave_step").collect::<Vec<_>>();assert_eq!(steps.len(),5);
            for expected in steps{let active=wave.step_cached(context,&mut owner.cache,clock)?.is_some();assert_eq!(wave.snapshot(i32::from(active)),expected["wave"]);assert_eq!(bank(context.bodies()?),expected["state"]["physical"]);assert_eq!(cache_frames(&owner),expected["state"]["cache_frames"]);if active{wave.continue_iteration()?;}}

            let changed=wave.rebuild(context)?;owner.rebuild_bounds(context.bodies()?,&changed,clock,deadline,q)?;owner.recheck_impact_pairs(context.bodies()?,&changed,clock,deadline,q)?;
            let expected=&c["phases"].as_array().unwrap().last().unwrap()["state"];assert_eq!(bank(context.bodies()?),expected["physical"],"wake restamped bank");assert_eq!(budgets(&owner),expected["hull_budgets"]);assert_eq!(cache_frames(&owner),expected["cache_frames"]);assert_eq!(owner.revision as u64,expected["revision_counter"].as_u64().unwrap());assert_eq!(serde_json::json!(owner.core_revisions),expected["body_revisions"]);
            Err("observed native collision wake phases")
        });
        assert_eq!(result,Err("observed native collision wake phases"));assert_eq!(called,1);assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),before);
    }
    #[test]
    fn native_oblique_joint_event_matches_geometry_primary_and_wave(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-oblique-joint-event.json")).unwrap())).unwrap();let c=&data["case"];let delta=f32::from_bits(c["delta_bits"].as_u64().unwrap() as u32);
        let (mut owner,mut physical)=multibody_world(c);for _ in 0..data["provenance"]["prefix_calls"].as_u64().unwrap(){owner.advance(&mut physical,delta).unwrap();}
        let bank=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());assert_eq!(bank(physical.bodies()),c["before"]["physical"]);
        let owner=std::cell::RefCell::new(owner);let before=crate::ivp_registered_contacts::snapshot(&physical);let mut called=0;
        let result=physical.advance_with_cache_context(delta,|b,ids,clock|owner.borrow_mut().prepare_contact_cache(b,ids,clock),|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
            called+=1;let mut owner=owner.borrow_mut();let Event::Impact(pair)=owner.event(id,context.bodies()?,clock,deadline,q)? else{panic!("expected sleeping-body impact")};
            assert_eq!(bank(context.bodies()?),c["phases"][2]["state"]["physical"],"checked wake bank");assert_eq!(cache_frames(&owner),c["phases"][2]["state"]["cache_frames"]);
            let mut wake_visits=0;owner.wake_impact_endpoints_observed(pair,context,clock,deadline,q,|owner,context|{
                let expected=&c["phases"][3+wake_visits]["state"];assert_eq!(bank(context.bodies()?),expected["physical"],"per-core wake bank");assert_eq!(cache_frames(owner),expected["cache_frames"],"per-core cache");assert_eq!(budgets(owner),expected["hull_budgets"],"per-core hulls");assert_eq!(owner.counter() as u64,expected["collision_counter"].as_u64().unwrap());
                let flags=(0..context.bodies()?.len()).map(|id|context.contacts().registry().unit_flags(id).unwrap().flags&0x3fff).collect::<Vec<_>>();assert_eq!(serde_json::json!(flags),expected["unit_flags"],"unit stays dormant during member callbacks");wake_visits+=1;Ok(())
            })?;assert_eq!(wake_visits,2);assert_eq!(bank(context.bodies()?),c["phases"][5]["state"]["physical"],"wake neighbour clocks");assert_eq!(cache_frames(&owner),c["phases"][5]["state"]["cache_frames"]);assert_eq!(owner.counter() as u64,c["phases"][5]["state"]["collision_counter"].as_u64().unwrap());let admitted=owner.admit(pair,context,clock)?;
            assert_eq!(bank(context.bodies()?),c["phases"][7]["state"]["physical"],"awake admitted bank");assert_eq!(cache_frames(&owner),c["phases"][7]["state"]["cache_frames"]);
            let actual=context.contacts().snapshot(admitted.key);let geometry=&actual["geometry"];let native=&c["phases"][7]["entry_geometry"];let bits=|v:&serde_json::Value|serde_json::json!((v.as_f64().unwrap() as f32).to_bits());
            for key in ["point","normal"]{assert_eq!(serde_json::json!(geometry[key].as_array().unwrap().iter().map(&bits).collect::<Vec<_>>()),native[key],"oblique admitted {key}");}
            for key in ["basis","arms","jacobians"]{let values=geometry[key].as_array().unwrap().iter().map(|a|if a.is_null(){serde_json::Value::Null}else{serde_json::json!(a.as_array().unwrap().iter().map(&bits).collect::<Vec<_>>())}).collect::<Vec<_>>();assert_eq!(serde_json::json!(values),native[key],"oblique admitted {key}");}
            for key in ["response","inverse_response"]{assert_eq!(bits(&geometry[key]),native[key],"oblique admitted {key}");}
            context.primary_impact(admitted.key)?;assert_eq!(bank(context.bodies()?),c["phases"][8]["state"]["physical"],"wake primary");
            let mut wave=crate::ivp_impact_wave::Wave::new_cached(context,admitted,&mut owner.cache,clock)?;assert_eq!(wave.snapshot(-1),c["phases"][9]["wave"]);assert_eq!(cache_frames(&owner),c["phases"][9]["state"]["cache_frames"]);
            let phases=c["phases"].as_array().unwrap();let steps=phases[10..].iter().take_while(|p|p["phase"]=="wave_step").collect::<Vec<_>>();assert_eq!(steps.len(),5);
            for expected in steps{let active=wave.step_cached(context,&mut owner.cache,clock)?.is_some();assert_eq!(wave.snapshot(i32::from(active)),expected["wave"]);assert_eq!(bank(context.bodies()?),expected["state"]["physical"]);assert_eq!(cache_frames(&owner),expected["state"]["cache_frames"]);if active{wave.continue_iteration()?;}}

            let changed=wave.rebuild(context)?;owner.rebuild_bounds(context.bodies()?,&changed,clock,deadline,q)?;owner.recheck_impact_pairs(context.bodies()?,&changed,clock,deadline,q)?;
            let expected=&c["phases"].as_array().unwrap().last().unwrap()["state"];assert_eq!(bank(context.bodies()?),expected["physical"],"wake restamped bank");assert_eq!(budgets(&owner),expected["hull_budgets"]);assert_eq!(cache_frames(&owner),expected["cache_frames"]);assert_eq!(owner.revision as u64,expected["revision_counter"].as_u64().unwrap());assert_eq!(serde_json::json!(owner.core_revisions),expected["body_revisions"]);
            Err("observed native collision wake phases")
        });
        assert_eq!(result,Err("observed native collision wake phases"));assert_eq!(called,1);assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),before);
    }
    #[test]
    fn native_queued_collision_wake_keeps_pending_flag_until_next_psi(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-queued-wake.json")).unwrap())).unwrap();let c=&data["case"];let delta=f32::from_bits(c["delta_bits"].as_u64().unwrap() as u32);
        let (mut owner,mut physical)=multibody_world(c);for _ in 0..data["provenance"]["prefix_calls"].as_u64().unwrap(){owner.advance(&mut physical,delta).unwrap();}
        let bank=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());assert_eq!(bank(physical.bodies()),c["before"]["physical"]);
        let owner=std::cell::RefCell::new(owner);let mut called=0;
        let result=physical.advance_with_cache_context(delta,|b,ids,clock|owner.borrow_mut().prepare_contact_cache(b,ids,clock),|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
            called+=1;let mut owner=owner.borrow_mut();let Event::Impact(pair)=owner.event(id,context.bodies()?,clock,deadline,q)? else{panic!("expected sleeping-body impact")};
            assert_eq!(bank(context.bodies()?),c["phases"][2]["state"]["physical"],"checked wake bank");assert_eq!(cache_frames(&owner),c["phases"][2]["state"]["cache_frames"]);
            context.queue_wake(1)?;assert_eq!(bank(context.bodies()?),c["phases"][3]["state"]["physical"],"queued wake flags");assert_eq!(context.contacts().registry().pending_cores(),&[1]);owner.wake_impact_endpoints(pair,context,clock,deadline,q)?;assert_eq!(context.contacts().registry().pending_cores(),&[1]);assert_eq!(bank(context.bodies()?),c["phases"][4]["state"]["physical"],"wake neighbour clocks");assert_eq!(cache_frames(&owner),c["phases"][4]["state"]["cache_frames"]);let admitted=owner.admit(pair,context,clock)?;
            assert_eq!(bank(context.bodies()?),c["phases"][6]["state"]["physical"],"awake admitted bank");assert_eq!(cache_frames(&owner),c["phases"][6]["state"]["cache_frames"]);
            context.primary_impact(admitted.key)?;assert_eq!(bank(context.bodies()?),c["phases"][7]["state"]["physical"],"wake primary");
            let mut wave=crate::ivp_impact_wave::Wave::new_cached(context,admitted,&mut owner.cache,clock)?;assert_eq!(wave.snapshot(-1),c["phases"][8]["wave"]);assert_eq!(cache_frames(&owner),c["phases"][8]["state"]["cache_frames"]);
            assert!(wave.step_cached(context,&mut owner.cache,clock)?.is_none());assert_eq!(wave.snapshot(0),c["phases"][9]["wave"]);assert_eq!(bank(context.bodies()?),c["phases"][9]["state"]["physical"]);
            let changed=wave.rebuild(context)?;owner.rebuild_bounds(context.bodies()?,&changed,clock,deadline,q)?;owner.recheck_impact_pairs(context.bodies()?,&changed,clock,deadline,q)?;
            let expected=&c["phases"][11]["state"];assert_eq!(bank(context.bodies()?),expected["physical"],"wake restamped bank");assert_eq!(budgets(&owner),expected["hull_budgets"]);assert_eq!(cache_frames(&owner),expected["cache_frames"]);assert_eq!(owner.revision as u64,expected["revision_counter"].as_u64().unwrap());assert_eq!(serde_json::json!(owner.core_revisions),expected["body_revisions"]);
            Ok(())
        });
        assert_eq!(result,Ok(()));assert_eq!(called,1);let mut owner=owner.into_inner();
        fn check(owner:&Owner,physical:&crate::ivp_registered_contacts::ContactWorld,expected:&serde_json::Value){
            assert_eq!(serde_json::json!(physical.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),expected["physical"]);
            assert_eq!(cache_frames(owner),expected["cache_frames"]);assert_eq!(pairs(owner,physical),expected["pairs"]);assert_eq!(budgets(owner),expected["hull_budgets"]);
            let metadata=crate::ivp_registered_contacts::continuation_metadata(physical);for key in ["histories","poll","clock","impact_counts","unit_flags"]{assert_eq!(metadata[key],expected[key],"queued continuation {key}");}
            assert_eq!(owner.counter() as u64,expected["collision_counter"].as_u64().unwrap());assert_eq!(owner.revision as u64,expected["revision_counter"].as_u64().unwrap());assert_eq!(serde_json::json!(owner.core_revisions),expected["body_revisions"]);
        }
        check(&owner,&physical,&c["after"]);assert_eq!(physical.registry().pending_cores(),&[1]);
        for expected in c["continuation"].as_array().unwrap(){owner.advance(&mut physical,delta).unwrap();check(&owner,&physical,expected);assert!(physical.registry().pending_cores().is_empty());}

    }
    #[test]
    fn spinning_second_impact_matches_native_admission_and_primary_stages(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-multi-event.json")).unwrap())).unwrap();let c=&data["case"];
        let n=c["count"].as_u64().unwrap() as usize;let dt=1./60.;let radius=c["radius"].as_f64().unwrap() as f32;let spacing=c["spacing"].as_f64().unwrap() as f32;
        let mut physical=crate::ivp_registered_contacts::ContactWorld::empty_native(dt,c["initial_seed"].as_u64().unwrap() as u32,Vec3::new(0.,-0.,0.),Vec3::ZERO).unwrap();
        let spin=Vec3::from_array(std::array::from_fn(|k|c["spin"][k].as_f64().unwrap() as f32));
        for id in 0..n{
            physical.create_sphere(id as u64+1,crate::ivp_sphere_constructor::Parameters{radius,position:authored_position(c,id,spacing),angles:[0.;3],mass:10.+5.*id as f32,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:id+1==n&&c["fixed_last"].as_bool().unwrap(),collisions:true},0.8).unwrap();
            if !c["linear_commands"][id].is_null(){physical.set_velocity(id,Some(Vec3::from_array(std::array::from_fn(|k|c["linear_commands"][id][k].as_f64().unwrap() as f32))),Some(spin)).unwrap();}physical.configure_sphere_elasticity(id,0.).unwrap();
        }
        let mut owner=Owner::new(physical.bodies(),vec![radius*f32::from_bits(0x3cd013a9);n],&(0..n).collect::<Vec<_>>(),dt).unwrap();
        let bank=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());
        for _ in 0..data["provenance"]["prefix_calls"].as_u64().unwrap(){owner.advance(&mut physical,0.001).unwrap();}
        assert_eq!(bank(physical.bodies()),c["before"]["physical"]);
        let owner=std::cell::RefCell::new(owner);let before=crate::ivp_registered_contacts::snapshot(&physical);let mut called=0;
        let result=physical.advance_with_cache_context(0.001,|b,ids,clock|owner.borrow_mut().prepare_contact_cache(b,ids,clock),|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
            called+=1;let mut owner=owner.borrow_mut();let Event::Impact(pair)=owner.event(id,context.bodies()?,clock,deadline,q)? else{panic!("expected spinning second impact")};
            assert_eq!(bank(context.bodies()?),c["phases"][1]["state"]["physical"],"checked");
            let admitted=owner.admit(pair,context,clock)?;let expected=&c["phases"][3]["state"];
            assert_eq!(bank(context.bodies()?),expected["physical"],"admitted bank");
            let actual=context.contacts().snapshot(admitted.key);let geometry=&actual["geometry"];let native=&expected["entry_geometry"];let bits=|v:&serde_json::Value|serde_json::json!((v.as_f64().unwrap() as f32).to_bits());
            for key in ["point","normal"]{assert_eq!(serde_json::json!(geometry[key].as_array().unwrap().iter().map(&bits).collect::<Vec<_>>()),native[key],"admitted {key}");}
            for key in ["basis","arms","jacobians"]{let v=geometry[key].as_array().unwrap().iter().map(|a|if a.is_null(){serde_json::Value::Null}else{serde_json::json!(a.as_array().unwrap().iter().map(&bits).collect::<Vec<_>>())}).collect::<Vec<_>>();assert_eq!(serde_json::json!(v),native[key],"admitted {key}");}
            for key in ["response","inverse_response"]{assert_eq!(bits(&geometry[key]),native[key],"admitted {key}");}
            context.primary_impact(admitted.key)?;assert_eq!(bank(context.bodies()?),c["phases"][4]["state"]["physical"],"primary bank");
            Err("observed spinning primary response")
        });
        assert_eq!(result,Err("observed spinning primary response"));assert_eq!(called,1);assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),before);
    }
    #[test]
    fn native_multibody_impacts_and_retirement_continue_automatically(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-multi.json")).unwrap())).unwrap();
        let caches:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-cache.json")).unwrap())).unwrap();
        assert_multibody_cases(&data,Some(&caches));
    }
    #[test]
    fn native_multibody_stress_continues_across_full_physics_ticks(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-stress.json")).unwrap())).unwrap();
        assert_multibody_cases(&data,None);
    }
    #[test]
    fn native_impacts_against_untouched_dormant_targets(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-dormant.json")).unwrap())).unwrap();
        assert_multibody_cases(&data,None);
    }
    #[test]
    fn native_tight_dormant_clusters_expand_waking_units(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-cluster.json")).unwrap())).unwrap();
        assert_multibody_cases(&data,None);
    }
    #[test]
    fn native_impacts_wake_prejoined_dormant_ragdoll_members(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-joined.json")).unwrap())).unwrap();
        assert_multibody_cases(&data,None);
    }
    #[test]
    fn native_oblique_spinning_impacts_continue_with_ragdoll_joints(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-oblique-joint.json")).unwrap())).unwrap();
        assert_multibody_cases(&data,None);
    }
    #[test]
    fn native_gravity_stacks_rest_on_fixed_sphere_contacts(){
        let data:serde_json::Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-impact-gravity-stack.json")).unwrap())).unwrap();
        assert_multibody_cases(&data,None);
    }
    #[test]
    fn native_sphere_inactive_notifications_reactivate_without_scheduling(){
        use serde_json::{Value,json};
        use crate::ivp_joint_apply::tests::{body,snapshot};
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-inactive.json")).unwrap();
        let packet=|owner:&Owner,id:usize|{let p=&owner.pairs[&id];let g=p.geometry;let (slots,objects,_)=owner.cache.metadata();
            json!({"flags":owner.graph.flags(id).unwrap(),"frame":p.frame,"geometry":[g.projection.to_bits(),g.linear_delta.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()],"active":owner.graph.active().contains(&id),"inactive":owner.graph.inactive().contains(&id),"handle":p.handle.unwrap_or(u16::MAX),"object_active":p.objects.map(|o|owner.graph.object_links(o).unwrap().iter().any(|s|s.pair==id)),"object_inactive":p.objects.map(|o|owner.graph.inactive_object_links(o).unwrap().iter().any(|s|s.pair==id)),"refs":p.objects.map(|o|slots[objects[o].cache.unwrap()].references)})};
        let mut count=0;
        for c in data["cases"].as_array().unwrap(){
            let f=|v:&Value|f32::from_bits(v.as_u64().unwrap() as u32);let dt=f(&c["dt"]);
            let initial=[body(&c["initial"][0]),body(&c["initial"][1])];let mut owner=Owner::new(&initial,vec![f(&c["margin"])*0.5;2],&[0,1],dt).unwrap();
            let clock=Clock{time:f64::from_bits(c["initial_clock"].as_u64().unwrap()),frame:c["initial_frame"].as_u64().unwrap() as u32};let mut queue=Queue::new(8,0.).unwrap();
            let id=owner.graph.register([0,1]).unwrap();owner.graph.activate(id,[false;2]).unwrap();owner.pairs.insert(id,Pair{broad:usize::MAX,objects:[0,1],geometry:far::Geometry{projection:0.,linear_delta:0.,distance:0.,normal:Vec3::ZERO},frame:0,far:false,handle:None});
            {let mut context=Context{cache:&mut owner.cache,graph:&mut owner.graph,pairs:&mut owner.pairs,radii:&owner.radii,ranks:&owner.ranks,bodies:&initial,counter:&mut owner.counter,queue:&mut queue,clock,deadline:clock.time+dt as f64,dt,config:owner.config};context.refresh(id).unwrap();}
            assert_eq!(packet(&owner,id),c["initial_packet"],"case{} initial",c["id"]);
            owner.graph.make_inactive(id).unwrap();assert_eq!(packet(&owner,id),c["before_packet"],"case{} inactive",c["id"]);
            // These are borrowed phase inputs, not recorded outcomes used to drive
            // an end-to-end simulation. Full-world controls remain separate.
            let bodies=[body(&c["before"][0]),body(&c["before"][1])];let before=bodies.map(snapshot);
            let clock=Clock{time:f64::from_bits(c["clock"].as_u64().unwrap()),frame:c["frame"].as_u64().unwrap() as u32};
            assert_eq!(owner.refresh_inactive_unit(&bodies,&[0,1],clock,clock.time+dt as f64,&mut queue).unwrap(),1);
            assert_eq!(packet(&owner,id),c["after_packet"],"case{} refreshed",c["id"]);
            assert!(queue.scheduled(PAIR_BASE+id).is_none());assert_eq!(json!(bodies.map(snapshot)),c["after"]);assert_eq!(bodies.map(snapshot),before);
            let owner_before=metadata(&owner);assert_eq!(owner.refresh_inactive_unit(&bodies,&[1,0],clock,clock.time+dt as f64,&mut queue).unwrap(),0);
            assert_eq!(metadata(&owner),owner_before);assert_eq!(packet(&owner,id),c["repeat_packet"]);count+=1;
        }
        assert_eq!(count,192);
    }
    #[test]
    fn native_sphere_material_assignments_preserve_public_wake_and_contact_sequences(){
        use serde_json::{Value,json};
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-material-assign.json")).unwrap();
        let systems:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-material-systems.json")).unwrap();
        let system_bank=|w:&crate::ivp_registered_contacts::ContactWorld|json!((1..=3).map(|object|w.contacts().material_system_snapshot(object)).collect::<Vec<_>>());
        let f=|v:&Value|f32::from_bits(v.as_u64().unwrap() as u32);let vec=|v:&Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));
        let bank=|v:&Value|json!(v.as_array().unwrap().iter().enumerate().map(|(i,p)|{let mut a=p[0].clone();let mut b=p[1].clone();a["id"]=json!(i);b["id"]=json!(i);assert_eq!(a,b);a}).collect::<Vec<_>>());
        let physical_bank=|w:&crate::ivp_registered_contacts::ContactWorld|json!(w.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());
        let mut impacts=0;
        for c in data["cases"].as_array().unwrap(){
            let id=c["id"].as_u64().unwrap();let dt=f(&c["dt"]);let unit=f32::from_bits(0x3cd013a9);let g=vec(&c["gravity_input"]);let gravity=Vec3::new(g.x*unit,-g.z*unit,g.y*unit);
            let mut world=crate::ivp_registered_contacts::ContactWorld::empty_native(dt,123,gravity,Vec3::ZERO).unwrap();let prefix=f(&c["prefix"]);if prefix>0.{world.advance(prefix).unwrap();}
            let mut registry=crate::ivp_surface_registry::Registry::default();
            for (i,text) in data["material_texts"].as_array().unwrap().iter().enumerate(){let text=text.as_str().unwrap();let bytes=(0..text.len()).step_by(2).map(|i|u8::from_str_radix(&text[i..i+2],16).unwrap()).collect::<Vec<_>>();registry.parse_file(&format!("sphere-material-{i}"),std::str::from_utf8(&bytes).unwrap()).unwrap();}
            let shared=crate::ivp_material_backend::SharedSurfaceRegistry::new(registry);let table=crate::ivp_material_backend::WorldMaterialTable::default();let input=data["material_table"].as_array().unwrap().iter().map(|v|v.as_i64().unwrap() as i32).collect::<Vec<_>>();table.set(&input,128).unwrap();world.bind_shared_materials(&shared,&table).unwrap();
            let radius=f(&data["sphere_radius"]);
            for actor in 0..3{world.create_sphere(actor as u64+1,crate::ivp_sphere_constructor::Parameters{radius,position:vec(&c["positions"][actor]).to_array(),angles:[0.;3],mass:5.+5.*actor as f32,inertia_scale:1.,damping:0.,angular_damping:0.,rotational_limit:0.1,fixed:actor==2,collisions:true},0.8).unwrap();world.set_object_material(actor as u64+1,1).unwrap();world.configure_sphere_elasticity(actor,0.2).unwrap();let command=&c["velocity_inputs"][actor];let linear=(!command["linear"].is_null()).then(||vec(&command["linear"]));let angular=(!command["angular"].is_null()).then(||vec(&command["angular"]));if linear.is_some()||angular.is_some(){world.set_velocity(actor,linear,angular).unwrap();}}
            let mut config=schedule::Config::bare_native();config.set_force_gravity(gravity).unwrap();let mut owner=Owner::new_with_config(world.bodies(),vec![radius*unit;3],&[0,1,2],dt,config).unwrap();
            assert_eq!(physical_bank(&world),bank(&c["before"]),"sphere{id} constructed");
            owner.wake_initial(&mut world,c["start"].as_u64().unwrap() as usize,c["whole"].as_bool().unwrap()).unwrap();assert_eq!(physical_bank(&world),bank(&c["after"]),"sphere{id} initial wake");
            for actor in 0..3{assert_eq!(json!(world.registry().unit_cores(actor).unwrap()),c["units_after"][actor]["members"],"sphere{id} initial unit{actor}");assert_eq!(json!(world.registry().unit_flags(actor).unwrap().flags&255),c["units_after"][actor]["state"]);}
            let mut trace=vec![];let mut budget_steps=vec![];
            for iteration in 0..c["prediction_steps"].as_u64().unwrap(){
                for update in c["material_assignments"].as_array().unwrap().iter().filter(|u|u["iteration"].as_u64()==Some(iteration)){
                    assert_eq!(physical_bank(&world),bank(&update["before"]),"sphere{id} iteration{iteration} before material");
                    let expected_systems=systems["cases"][id as usize]["updates"].as_array().unwrap().iter().find(|u|u["iteration"].as_u64()==Some(iteration)).unwrap();
                    assert_eq!(system_bank(&world),expected_systems["before_systems"],"sphere{id} iteration{iteration} before systems");
                    let before_world=crate::ivp_registered_contacts::snapshot(&world);let before_owner=metadata(&owner);
                    owner.assign_material(&mut world,update["actor"].as_u64().unwrap()+1,update["index"].as_i64().unwrap() as i32).unwrap();
                    assert_eq!(physical_bank(&world),bank(&update["after"]),"sphere{id} iteration{iteration} after material");
                    assert_eq!(system_bank(&world),expected_systems["after_systems"],"sphere{id} iteration{iteration} after systems");
                    if iteration==16{assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before_world,"sphere{id} same material physical no-op");assert_eq!(metadata(&owner),before_owner,"sphere{id} same material geometry/cache no-op");}

                }
                if id==12&&iteration==12{
                    let before_world=crate::ivp_registered_contacts::snapshot(&world);let before_owner=metadata(&owner);let mut rejected=false;
                    let result=owner.advance_observed(&mut world,f(&c["step"]),|e|{if e.phase==Phase::Predicted{rejected=true;Err("reject sphere post-material prediction")}else{Ok(())}});
                    assert!(rejected);assert_eq!(result,Err("reject sphere post-material prediction"));
                    assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before_world);assert_eq!(metadata(&owner),before_owner);
                }
                owner.advance_observed(&mut world,f(&c["step"]),|e|{let kind=match e.phase{Phase::Prediction=>0,Phase::Timed=>1,Phase::Impact=>{impacts+=1;2},_=>return Ok(())};trace.push(json!({"kind":kind,"time":e.clock.time.to_bits(),"frame":e.clock.frame,"flags":e.flags&0x3fffffff,"objects":e.objects.unwrap_or([999;2])}));Ok(())}).unwrap_or_else(|e|panic!("sphere{id} iteration{iteration}: {e}"));
                budget_steps.push(budgets(&owner));
            }
            if id==12{if let Some(path)=std::env::var_os("L4D2R_TRACE_SPHERE_BUDGETS"){std::fs::write(path,serde_json::to_vec(&budget_steps).unwrap()).unwrap();}}
            let mut expected=c["events"].clone();for e in expected.as_array_mut().unwrap(){e["flags"]=json!(e["flags"].as_u64().unwrap()&0x3fffffff);}for (i,(a,b)) in trace.iter().zip(expected.as_array().unwrap()).enumerate(){assert_eq!(a,b,"sphere{id} callback{i}");}assert_eq!(trace.len(),expected.as_array().unwrap().len(),"sphere{id} callback length");
            assert_eq!(system_bank(&world),systems["cases"][id as usize]["post_systems"],"sphere{id} final systems");
            assert_eq!(physical_bank(&world),bank(&c["predicted_after"]),"sphere{id} final physical");assert_eq!(world.clock().now.to_bits(),c["prediction_now"].as_u64().unwrap());assert_eq!(world.clock().deadline.to_bits(),c["prediction_deadline"].as_u64().unwrap());assert_eq!(world.clock().clock_serial as u64,c["prediction_frame"].as_u64().unwrap());
            for actor in 0..3{assert_eq!(json!(world.registry().unit_cores(actor).unwrap()),c["predicted_units"][actor]["members"]);assert_eq!(json!(world.registry().unit_flags(actor).unwrap().flags&255),c["predicted_units"][actor]["state"]);let b=owner.bounds.object_budget(actor).unwrap();let native=&c["post_motion"][actor];assert_eq!(json!(b.time.to_bits()),native["time"],"sphere{id} budget time{actor}");assert_eq!(json!(b.rates.map(f32::to_bits)),native["rates"],"sphere{id} rates{actor}");assert_eq!(json!(b.accumulated.map(f32::to_bits)),native["accumulated"],"sphere{id} accumulated{actor}");assert_eq!(json!(b.predicted.to_bits()),native["predicted"],"sphere{id} predicted{actor}");assert_eq!(json!(b.threshold.to_bits()),native["threshold"],"sphere{id} threshold{actor}");
                let mut rows=vec![];for (&pair,p) in &owner.pairs{if !p.far{continue;}if let Some(side)=p.objects.iter().position(|&a|a==actor){let keys=owner.bounds.distance_keys(pair).unwrap();let g=p.geometry;rows.push(json!({"objects":p.objects,"flags":owner.graph.flags(pair).unwrap()&0x3fffffff,"frame":p.frame,"key":keys[side].to_bits(),"geometry":[g.projection.to_bits(),g.linear_delta.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()]}));}}
                rows.sort_by_key(|v|v["objects"].to_string());let mut expected=c["post_far"][actor].as_array().unwrap().clone();for e in &mut expected{e["flags"]=json!(e["flags"].as_u64().unwrap()&0x3fffffff);}expected.sort_by_key(|v|v["objects"].to_string());assert_eq!(rows,expected,"sphere{id} far{actor}");
            }
        }
        assert_eq!(impacts,208);assert_eq!(data["cases"].as_array().unwrap().len(),96);
    }
    fn assert_multibody_cases(data:&serde_json::Value,caches:Option<&serde_json::Value>){
        for c in data["cases"].as_array().unwrap(){
            let scenario=c["scenario"].as_u64().unwrap();let delta=c["delta_bits"].as_u64().map_or(0.001,|bits|f32::from_bits(bits as u32));
            let (mut owner,mut physical)=multibody_world(c);
            assert_eq!(serde_json::json!(physical.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),c["constructed"]["physical"],"constructed {scenario}");
            for (step,expected) in c["rows"].as_array().unwrap().iter().enumerate(){
                owner.advance(&mut physical,delta).unwrap_or_else(|e|panic!("multibody {scenario}/{step}: {e}"));
                let cache=caches.map_or(&expected["cache_frames"],|caches|&caches["cases"][scenario as usize]["rows"][step]);
                assert_eq!(cache_frames(&owner),*cache,"cache frames {scenario}/{step}");
                assert_eq!(serde_json::json!(physical.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),expected["physical"],"physical {scenario}/{step}");
                assert_eq!(pairs(&owner,&physical),expected["pairs"],"pairs {scenario}/{step}");
                assert_eq!(budgets(&owner),expected["hull_budgets"],"hulls {scenario}/{step}");
                let metadata=crate::ivp_registered_contacts::continuation_metadata(&physical);
                for key in ["histories","poll","clock","impact_counts","unit_flags"]{assert_eq!(metadata[key],expected[key],"{key} {scenario}/{step}");}
                assert_eq!(owner.counter() as u64,expected["collision_counter"].as_u64().unwrap(),"collision counter {scenario}/{step}");
                assert_eq!(owner.revision as u64,expected["revision_counter"].as_u64().unwrap(),"revision {scenario}/{step}");
                assert_eq!(serde_json::json!(owner.core_revisions),expected["body_revisions"],"body revisions {scenario}/{step}");
            }
        }
    }
    fn metadata(owner:&Owner)->String{
        format!("{:?}/{:?}/{:?}/{:?}/{:?}/{:?}/{:?}",(owner.revision,&owner.core_revisions),owner.counter,owner.flags,owner.cache.metadata(),
            (0..owner.radii.len()).map(|id|owner.bounds.object_budget(id).unwrap()).collect::<Vec<_>>(),
            owner.graph.active(),owner.pairs.iter().map(|(id,p)|(id,p.broad,p.objects,p.geometry,p.frame,p.far,p.handle)).collect::<Vec<_>>())
    }
    #[test]
    fn primary_response_uses_source_materials_and_admitted_native_geometry(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-wave.json")).unwrap();
        for c in data["cases"].as_array().unwrap(){
            let dt=1./60.;let mut physical=crate::ivp_registered_contacts::ContactWorld::empty_native(dt,c["initial_seed"].as_u64().unwrap() as u32,Vec3::new(0.,-0.,0.),Vec3::ZERO).unwrap();
            let spin=Vec3::from_array(std::array::from_fn(|k|c["spin"][k].as_f64().unwrap() as f32));
            for id in 0..2{
                physical.create_sphere(id+1,crate::ivp_sphere_constructor::Parameters{radius:c["radius"].as_f64().unwrap() as f32,position:[c["separation"].as_f64().unwrap() as f32*id as f32,0.,128.],angles:[0.;3],mass:10.+5.*id as f32,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:id==0&&c["fixed0"].as_bool().unwrap(),collisions:true},0.8).unwrap();
                physical.set_velocity(id as usize,Some(Vec3::from_array(std::array::from_fn(|k|c["linear_commands"][id as usize][k].as_f64().unwrap() as f32))),Some(spin)).unwrap();
                physical.configure_sphere_elasticity(id as usize,c["elasticities"][id as usize].as_f64().unwrap() as f32).unwrap();
            }
            let radius=c["radius"].as_f64().unwrap() as f32*f32::from_bits(0x3cd013a9);let mut owner=Owner::new(physical.bodies(),vec![radius;2],&[0,1],dt).unwrap();
            let scenario=c["scenario"].as_u64().unwrap() as usize;
            let coarse:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-coarse.json")).unwrap();
            for step in 0..[255,234,536,401][scenario%4]{
                owner.advance(&mut physical,0.001).unwrap();
                if step==0{
                    let expected=&coarse["cases"][scenario]["first"]["state"];
                    assert_eq!(serde_json::json!(owner.bounds.coarse_objects()),expected["coarse_pairs"],"first coarse creation {scenario}");
                    assert_eq!(budgets(&owner),expected["hull_budgets"],"first coarse budgets {scenario}");
                    assert_eq!(serde_json::json!(physical.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),expected["physical"],"first coarse physical {scenario}");
                    assert_eq!(owner.counter() as u64,expected["collision_counter"].as_u64().unwrap());
                }
            }
            let snapshot=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());assert_eq!(snapshot(physical.bodies()),c["before"]["physical"]);
            let mut continuation_owner=owner.clone();let mut continuation_physical=physical.clone();
            let revisions:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-revision.json")).unwrap();let revisions=&revisions["cases"][scenario];
            let owner=std::cell::RefCell::new(owner);let before=crate::ivp_registered_contacts::snapshot(&physical);let mut called=0;
            let result=physical.advance_with_discovery_context(0.001,|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
                called+=1;let mut owner=owner.borrow_mut();let Event::Impact(pair)=owner.event(id,context.bodies()?,clock,deadline,q)? else{panic!("expected generated native impact")};
                let admitted=owner.admit(pair,context,clock)?;assert_eq!(snapshot(context.bodies()?),c["phases"][3]["state"]["physical"]);
                let primary=context.primary_impact(admitted.key)?;
                let expected=&c["phases"][4]["state"];
                let delayed=expected["physical"].as_array().unwrap().iter().position(|b|b["pending_linear"].as_array().unwrap()[..3].iter().chain(&b["pending_angular"].as_array().unwrap()[..3]).any(|v|v.as_u64().unwrap()&0x7fffffff!=0));assert_eq!(primary.deferred_core,delayed,"deferred core {scenario}");assert_eq!(snapshot(context.bodies()?),expected["physical"],"primary bank {scenario}");
                let counts=context.impact_counts().values().copied().collect::<Vec<_>>();assert_eq!(serde_json::json!(counts),expected["impact_counts"]);
                let mut wave=crate::ivp_impact_wave::Wave::new(context,admitted)?;
                assert_eq!(wave.snapshot(-1),c["phases"][5]["wave"]);assert_eq!(snapshot(context.bodies()?),c["phases"][5]["state"]["physical"]);
                assert!(wave.step(context)?.is_none());assert_eq!(wave.snapshot(0),c["phases"][6]["wave"]);assert_eq!(snapshot(context.bodies()?),c["phases"][6]["state"]["physical"]);
                let cache=context.contacts().impact_snapshot();let native=&c["phases"][6]["state"]["contact_geometry"];assert_eq!(cache[0]["valid"],true);assert_eq!(cache[0]["rescue"],native["rescue"]);assert_eq!(cache[0]["score"],native["score"]);assert_eq!(cache[0]["pushes"],native["system_pushes"]);
                let changed=wave.rebuild(context)?;
                let responded=c["phases"].as_array().unwrap().iter().find(|p|p["phase"]=="responded").unwrap();
                assert_eq!(snapshot(context.bodies()?),responded["state"]["physical"],"rebuilt generated bank {scenario}");
                owner.rebuild_bounds(context.bodies()?,&changed,clock,deadline,q)?;
                assert_eq!(budgets(&owner),responded["state"]["hull_budgets"],"post-impact hulls {scenario}");
                assert_eq!(owner.counter(),responded["state"]["collision_counter"].as_u64().unwrap() as u32);
                owner.recheck_impact_pairs(context.bodies()?,&changed,clock,deadline,q)?;
                assert_eq!(owner.counter(),responded["state"]["collision_counter"].as_u64().unwrap() as u32);
                let rev=revisions["phases"].as_array().unwrap().iter().find(|p|p["phase"]=="responded").unwrap();
                assert_eq!(serde_json::json!(owner.core_revisions),rev["cores"]);assert_eq!(owner.revision as u64,rev["counter"].as_u64().unwrap());
                Err("observed complete impact before public exit")
            });
            assert_eq!(result,Err("observed complete impact before public exit"));assert_eq!(called,1);assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),before);
            for (step,expected) in std::iter::once(&c["after"]).chain(c["continuation"].as_array().unwrap()).enumerate(){
                continuation_owner.advance(&mut continuation_physical,0.001).unwrap();
                assert_eq!(snapshot(continuation_physical.bodies()),expected["physical"],"automatic continuation {scenario}/{step}");
                assert_eq!(budgets(&continuation_owner),expected["hull_budgets"],"automatic hull continuation {scenario}/{step}");
                assert_eq!(continuation_owner.counter() as u64,expected["collision_counter"].as_u64().unwrap(),"automatic counter {scenario}/{step}");
                let metadata=crate::ivp_registered_contacts::continuation_metadata(&continuation_physical);
                for key in ["histories","poll","clock","impact_counts","unit_flags"]{assert_eq!(metadata[key],expected[key],"automatic {key} {scenario}/{step}");}
                let rev=if step==0{&revisions["after"]}else{&revisions["continuation"][step-1]};
                assert_eq!(serde_json::json!(continuation_owner.core_revisions),rev["cores"]);assert_eq!(continuation_owner.revision as u64,rev["counter"].as_u64().unwrap());
                let queue=&continuation_physical.clock().queue;
                let pairs=continuation_owner.active(queue).unwrap().into_iter().map(|p|serde_json::json!({"objects":p.objects,"features":[3,3],"flags":p.flags,"query_frame":p.frame,"environment_frame":continuation_physical.clock().clock_serial,"margin":p.objects.map(|id|continuation_owner.radii[id]).into_iter().sum::<f32>().to_bits(),"measures":[p.geometry.projection.to_bits(),p.geometry.linear_delta.to_bits(),p.geometry.distance.to_bits()],"normal":p.geometry.normal.to_array().map(f32::to_bits),"queued":p.scheduled.is_some(),"handle":p.handle.unwrap_or(u16::MAX),"epoch":queue.epoch.to_bits(),"key":p.scheduled.map_or(0,|(_,key)|key.to_bits())})).collect::<Vec<_>>();
                assert_eq!(serde_json::json!(pairs),expected["pairs"],"automatic pairs {scenario}/{step}");
            }
        }
        assert_eq!(data["cases"].as_array().unwrap().len(),16);
    }
    #[test]
    fn generated_contacts_admit_with_native_physical_and_registry_state(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-admission.json")).unwrap();
        for c in data["cases"].as_array().unwrap(){
            let dt=1./60.;let mut physical=crate::ivp_registered_contacts::ContactWorld::empty_native(dt,c["initial_seed"].as_u64().unwrap() as u32,Vec3::new(0.,-0.,0.),Vec3::ZERO).unwrap();
            let spin=Vec3::from_array(std::array::from_fn(|k|c["spin"][k].as_f64().unwrap() as f32));
            for id in 0..2{
                physical.create_sphere(id+1,crate::ivp_sphere_constructor::Parameters{radius:c["radius"].as_f64().unwrap() as f32,position:[c["separation"].as_f64().unwrap() as f32*id as f32,0.,128.],angles:[0.;3],mass:10.+5.*id as f32,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:id==0&&c["fixed0"].as_bool().unwrap(),collisions:true},0.8).unwrap();
                physical.set_velocity(id as usize,Some(Vec3::from_array(std::array::from_fn(|k|c["linear_commands"][id as usize][k].as_f64().unwrap() as f32))),Some(spin)).unwrap();
            }
            let radius=c["radius"].as_f64().unwrap() as f32*f32::from_bits(0x3cd013a9);let mut owner=Owner::new(physical.bodies(),vec![radius;2],&[0,1],dt).unwrap();
            let scenario=c["scenario"].as_u64().unwrap() as usize;for _ in 0..[255,234,536,401][scenario%4]{owner.advance(&mut physical,0.001).unwrap();}
            let snapshot=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());assert_eq!(snapshot(physical.bodies()),c["before"]["physical"]);
            let owner=std::cell::RefCell::new(owner);let before=crate::ivp_registered_contacts::snapshot(&physical);let mut called=0;
            let result=physical.advance_with_discovery_context(0.001,|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
                called+=1;let mut owner=owner.borrow_mut();let Event::Impact(pair)=owner.event(id,context.bodies()?,clock,deadline,q)? else{panic!("expected generated native impact")};
                let p=&owner.pairs[&pair];let before_bank=snapshot(context.bodies()?);let before_contacts=context.contacts().diagnostics();
                assert!(context.admit_spheres(owner.graph.flags(pair)?,p.objects,[radius*2.;2]).is_err());assert_eq!(snapshot(context.bodies()?),before_bank);assert_eq!(context.contacts().diagnostics(),before_contacts);
                let admitted=owner.admit(pair,context,clock)?;assert_eq!(admitted.saved.len(),if c["fixed0"]==true{1}else{2});
                let expected=&c["phases"][3]["state"];assert_eq!(snapshot(context.bodies()?),expected["physical"],"admitted bank {scenario}");
                let histories=context.histories().values().map(|h|serde_json::json!({"close_time":h.close_time.to_bits(),"far_time":h.far_time.to_bits(),"close_position":h.close_position.to_array().map(f32::to_bits),"far_position":h.far_position.to_array().map(f32::to_bits),"close_quat":h.close_quat.map(f32::to_bits),"far_quat":h.far_quat.map(f32::to_bits)})).collect::<Vec<_>>();assert_eq!(serde_json::json!(histories),expected["histories"],"admitted histories {scenario}");

                let flags=(0..2).map(|id|context.contacts().registry().unit_flags(id).unwrap().flags&0x3fff).collect::<Vec<_>>();assert_eq!(serde_json::json!(flags),expected["unit_flags"]);
                let actual=context.contacts().snapshot(admitted.key);let state=&actual["state"];let native=&expected["contact"];
                let bits=|v:&serde_json::Value|serde_json::json!((v.as_f64().unwrap() as f32).to_bits());
                let geometry=&actual["geometry"];let native_geometry=&expected["contact_geometry"];
                for key in ["point","normal"]{let v=geometry[key].as_array().unwrap().iter().map(&bits).collect::<Vec<_>>();assert_eq!(serde_json::json!(v),native_geometry[key],"geometry {key} {scenario}");}
                for key in ["basis","arms","jacobians"]{let v=geometry[key].as_array().unwrap().iter().map(|a|if a.is_null(){serde_json::Value::Null}else{serde_json::json!(a.as_array().unwrap().iter().map(&bits).collect::<Vec<_>>())}).collect::<Vec<_>>();assert_eq!(serde_json::json!(v),native_geometry[key],"geometry {key} {scenario}");}
                for key in ["response","inverse_response"]{assert_eq!(bits(&geometry[key]),native_geometry[key],"geometry {key} {scenario}");}

                assert_eq!(state["time"].as_f64().unwrap().to_bits(),native["time"].as_u64().unwrap());
                for key in ["force","energy","work","bound","friction"]{assert_eq!((state[key].as_f64().unwrap() as f32).to_bits() as u64,native[key].as_u64().unwrap(),"{key} {scenario}");}
                for key in ["marker","counter","sign","mode"]{assert_eq!(state[key].as_u64(),native[key].as_u64(),"{key} {scenario}");}
                let mix=std::array::from_fn::<_,2,_>(|k|(state["mix"][k].as_f64().unwrap() as f32).to_bits());assert_eq!(serde_json::json!(mix),native["mix"]);
                assert_eq!((actual["geometry"]["distance"].as_f64().unwrap() as f32).to_bits() as u64,native["distance"].as_u64().unwrap(),"distance {scenario}");
                Err("observed admitted contact before response")
            });
            assert_eq!(result,Err("observed admitted contact before response"));assert_eq!(called,1);assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),before);
        }
        assert_eq!(data["cases"].as_array().unwrap().len(),8);
    }
    #[test]
    fn first_generated_events_recheck_and_stage_at_native_callback_time(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-event.json")).unwrap();
        for c in data["cases"].as_array().unwrap(){
            let dt=1./60.;let mut physical=crate::ivp_registered_contacts::ContactWorld::empty_native(dt,c["initial_seed"].as_u64().unwrap() as u32,Vec3::new(0.,-0.,0.),Vec3::ZERO).unwrap();
            for id in 0..2{
                physical.create_sphere(id+1,crate::ivp_sphere_constructor::Parameters{radius:c["radius"].as_f64().unwrap() as f32,position:[c["separation"].as_f64().unwrap() as f32*id as f32,0.,128.],angles:[0.;3],mass:10.+5.*id as f32,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:id==0&&c["fixed0"].as_bool().unwrap(),collisions:true},0.8).unwrap();
                physical.set_velocity(id as usize,Some(Vec3::from_array(std::array::from_fn(|k|c["linear_commands"][id as usize][k].as_f64().unwrap() as f32))),Some(Vec3::ZERO)).unwrap();
            }
            let radius=c["radius"].as_f64().unwrap() as f32*f32::from_bits(0x3cd013a9);let mut owner=Owner::new(physical.bodies(),vec![radius;2],&[0,1],dt).unwrap();
            let scenario=c["scenario"].as_u64().unwrap() as usize;for _ in 0..[255,234,536,401][scenario]{owner.advance(&mut physical,0.001).unwrap();}
            let snapshot=|b:&[Body]|serde_json::json!(b.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());
            assert_eq!(snapshot(physical.bodies()),c["before"]["physical"]);
            let owner=std::cell::RefCell::new(owner);let mut called=0;
            let before=crate::ivp_registered_contacts::snapshot(&physical);
            let result=physical.advance_with_discovery_context(0.001,|b,order,notices,clock,deadline,q|owner.borrow_mut().predict(b,order,notices,clock,deadline,q),|context,id,clock,deadline,q|{
                let b=context.bodies_mut()?;
                called+=1;let mut owner=owner.borrow_mut();
                assert_eq!(snapshot(b),c["phases"][0]["state"]["physical"],"event entry {scenario}");
                assert_eq!(owner.event(id,b,clock,deadline,q)?,Event::Impact(0));
                let active=owner.active(q)?;assert_eq!(active.len(),1);let a=&active[0];let expected=&c["phases"][1]["state"]["pairs"][0];
                assert_eq!(serde_json::json!(a.objects),expected["objects"]);assert_eq!(a.flags as u64,expected["flags"].as_u64().unwrap());assert_eq!(a.frame as u64,expected["query_frame"].as_u64().unwrap());
                assert_eq!(a.geometry.distance.to_bits() as u64,expected["measures"][2].as_u64().unwrap());assert_eq!(a.geometry.projection.to_bits() as u64,expected["measures"][0].as_u64().unwrap());assert_eq!(a.geometry.linear_delta.to_bits() as u64,expected["measures"][1].as_u64().unwrap());assert_eq!(serde_json::json!(a.geometry.normal.to_array().map(f32::to_bits)),expected["normal"]);assert!(a.handle.is_none()&&a.scheduled.is_none());
                for body in b.iter_mut(){let _backup=crate::ivp_impact_state::begin(body,clock.time)?;}
                assert_eq!(snapshot(b),c["phases"][2]["state"]["physical"],"staging {scenario}");
                Err("observed before response")
            });
            assert_eq!(result,Err("observed before response"));assert_eq!(called,1);assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),before);
        }
    }
    #[test]
    pub(crate) fn native_preimpact_sphere_lifecycle_is_source_owned(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-queue.json")).unwrap();let mut checked=0;let mut pending=0;
        for c in data["cases"].as_array().unwrap(){
            let rows=c["states"].as_array().unwrap();let end=rows.iter().position(|r|!r["contact"].is_null()).unwrap();let dt=1./60.;
            let mut physical=crate::ivp_registered_contacts::ContactWorld::empty_native(dt,rows[0]["poll"]["seed"].as_u64().unwrap() as u32,Vec3::new(0.,-0.,0.),Vec3::ZERO).unwrap();
            for id in 0..2{
                physical.create_sphere(id+1,crate::ivp_sphere_constructor::Parameters{radius:c["radius"].as_f64().unwrap() as f32,position:[c["separation"].as_f64().unwrap() as f32*id as f32,0.,128.],angles:[0.;3],mass:10.+5.*id as f32,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:id==0&&c["fixed0"].as_bool().unwrap(),collisions:true},0.8).unwrap();
                physical.set_velocity(id as usize,Some(Vec3::from_array(std::array::from_fn(|k|c["linear_commands"][id as usize][k].as_f64().unwrap() as f32))),Some(Vec3::ZERO)).unwrap();
            }
            let radius=c["radius"].as_f64().unwrap() as f32*f32::from_bits(0x3cd013a9);
            let mut owner=Owner::new(physical.bodies(),vec![radius;2],&[0,1],dt).unwrap();
            for(step,row)in rows.iter().enumerate().take(end){
                if step>0{owner.advance(&mut physical,0.001).unwrap();}
                assert_eq!(serde_json::json!(physical.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),row["physical"],"physical scenario {} step{step}",c["scenario"]);
                // Cross the first impact after several real PSI callbacks in
                // one public call. A failure must roll back both owners. The
                // remaining native rows then also verify clean continuation.
                if step==end-50{
                    let body_before=crate::ivp_registered_contacts::snapshot(&physical);let owner_before=metadata(&owner);
                    assert_eq!(owner.advance(&mut physical,0.1),Err("contact-world immediate impact response is not connected"));
                    assert_eq!(crate::ivp_registered_contacts::snapshot(&physical),body_before,"impact failure leaked physical state");
                    assert_eq!(metadata(&owner),owner_before,"impact failure leaked discovery state");
                }
                let active=owner.active(&physical.clock().queue).unwrap();let expected=row["pairs"].as_array().unwrap();
                assert_eq!(active.len(),expected.len(),"active pairs scenario {} step{step}",c["scenario"]);
                for (a,b)in active.iter().zip(expected){
                    assert_eq!(serde_json::json!(a.objects),b["objects"],"pair objects scenario {} step{step}",c["scenario"]);
                    assert_eq!(a.flags as u64,b["flags"].as_u64().unwrap());assert_eq!(a.frame as u64,b["query_frame"].as_u64().unwrap());
                    assert_eq!(a.geometry.distance.to_bits() as u64,b["measures"][2].as_u64().unwrap());assert_eq!(a.geometry.projection.to_bits() as u64,b["measures"][0].as_u64().unwrap());assert_eq!(a.geometry.linear_delta.to_bits() as u64,b["measures"][1].as_u64().unwrap());
                    assert_eq!(serde_json::json!(a.geometry.normal.to_array().map(f32::to_bits)),b["normal"]);
                    assert_eq!(a.scheduled.is_some(),b["queued"]==true);
                    if let Some((epoch,key))=a.scheduled{assert_eq!(epoch.to_bits(),b["epoch"].as_u64().unwrap());assert_eq!(key.to_bits() as u64,b["key"].as_u64().unwrap());assert_eq!(a.handle.map(u64::from),b["handle"].as_u64());pending+=1;}
                }
                assert_eq!(owner.counter() as u64,row["collision_counter"].as_u64().unwrap(),"counter scenario {} step{step}",c["scenario"]);checked+=1;
            }
        }
        assert_eq!(checked,1430);assert_eq!(pending,10);
    }
}
