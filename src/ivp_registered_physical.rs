//! Canonical registered physical owner for contact-free, base-controller cores.
//! Contacts and object/hull notifications
//! are explicitly unsupported; this is not the playable-world solver.
use bevy::prelude::{Vec2,Vec3};
use std::{cell::RefCell,collections::{BTreeMap,BTreeSet}};
use crate::{ivp_event_world::World,ivp_group_effects::{self,Context},ivp_group_members::Object,
    ivp_joint_apply::Body,ivp_movement::{self as movement,History,Input,Poll,Settings},
    ivp_registered_groups::Groups,ivp_unit_registry::{ControllerKey,Effect}};
#[derive(Clone,Copy,Debug)]
pub(crate) struct Properties{pub radius:f32,pub sleep_time:f32,pub linear_rate:f32,pub angular_rate:Vec3,pub inertia:Vec3,pub limits:Vec2,pub angular_limit_exact:f64,pub gyro:bool}
#[derive(Clone,Copy,Debug)]
pub(crate) struct JointDefinition{pub id:usize,pub objects:[usize;2],pub joint:crate::ivp_joint_mixed::Joint}
#[derive(Clone,Debug)]
pub(crate) struct GroupDefinition{pub constraints:Vec<JointDefinition>,pub offsets:BTreeMap<usize,Option<[f32;3]>>,pub additional:u32,pub minimum:i32,pub tolerance:f32,pub rescue_gravity:Vec3}
#[derive(Clone,Debug)]
pub(crate) struct ActiveGroup{pub constraint_ids:Vec<usize>,pub solver:crate::ivp_group_complete::Group,pub offsets:Vec<Option<[f32;3]>>,pub additional:u32,pub maximum:[f32;4]}
#[derive(Clone,Debug)]
pub(crate) struct FreeWorld{groups:Groups,clock:World,poll:Poll,dt:f32,gravity:Vec3,alternate:Vec3,rescue_gravity:Option<Vec3>,objects:Vec<Object>,bodies:BTreeMap<usize,Body>,histories:BTreeMap<usize,History>,properties:BTreeMap<usize,Properties>,joints:BTreeMap<usize,ActiveGroup>,object_offsets:BTreeMap<usize,Option<[f32;3]>>,retired_constraints:BTreeSet<usize>,detached_constraints:BTreeMap<usize,[usize;2]>,object_joint_listeners:BTreeMap<usize,BTreeSet<usize>>,prediction_calls:Vec<usize>}
fn settings()->Settings{Settings{translation_squared:0.0001,rotation_squared:0.000025,far_translation_squared:0.1f32*0.1,far_rotation_squared:0.2f32*0.2,far_timeout:4.,one:1.,two:2.,angular_factor0:std::f32::consts::PI,angular_factor1:0.75}}
fn input(b:Body,p:Properties)->Input{Input{radius:p.radius,sleep_time:p.sleep_time,angular:Vec3::new(b.angular[0],b.angular[1],b.angular[2]),position:b.prediction.start,start:b.prediction.rotation_start,end:b.prediction.rotation_end}}
impl FreeWorld{
    /// One-time transfer into the contact-capable owner; physical bodies,
    /// joint caches, movement histories, registry and event clock move together.
    pub(crate) fn into_contacts(self,definition:crate::ivp_registered_contacts::Definition)->Result<crate::ivp_registered_contacts::ContactWorld,&'static str>{
        let Self{groups,clock,poll,dt,gravity,alternate,rescue_gravity,objects,bodies,histories,properties,joints,object_offsets,retired_constraints,detached_constraints,object_joint_listeners,prediction_calls}=self;
        let (registry,registered_objects,metadata)=groups.into_parts();if registered_objects!=objects{return Err("physical/registered object boundary disagrees");}
        crate::ivp_registered_contacts::ContactWorld::from_parts(crate::ivp_registered_contacts::Parts{registry,clock,poll,dt,gravity,alternate,rescue_gravity,objects,bodies,histories,properties,joints,object_offsets,retired_constraints,detached_constraints,object_joint_listeners,prediction_calls,metadata},definition)
    }
    pub(crate) fn joint_group(&self,id:usize)->Option<&ActiveGroup>{self.joints.get(&id)}
    pub(crate) fn prediction_calls(&self)->&[usize]{&self.prediction_calls}
    pub(crate) fn bodies(&self)->&BTreeMap<usize,Body>{&self.bodies}
    pub(crate) fn histories(&self)->&BTreeMap<usize,History>{&self.histories}
    pub(crate) fn groups(&self)->&Groups{&self.groups}
    pub(crate) fn clock(&self)->&World{&self.clock}
    pub(crate) fn poll(&self)->Poll{self.poll}

    pub(crate) fn new(anchor:usize,bindings:&[(Object,Body,History,Properties)],dt:f32,seed:u32)->Result<Self,&'static str>{
        Self::with_forces(anchor,bindings,dt,seed,Vec3::ZERO,Vec3::ZERO)
    }
    pub(crate) fn with_forces(anchor:usize,bindings:&[(Object,Body,History,Properties)],dt:f32,seed:u32,gravity:Vec3,alternate:Vec3)->Result<Self,&'static str>{
        if !gravity.is_finite()||!alternate.is_finite(){return Err("invalid free-world gravity");}
        let objects=bindings.iter().map(|b|b.0).collect::<Vec<_>>();let groups=Groups::new(anchor,&objects)?;
        let mut clock=World::new(1000)?;clock.set_timestep(dt)?;
        let(mut bodies,mut histories,mut properties)=(BTreeMap::new(),BTreeMap::new(),BTreeMap::new());
        for &(object,body,history,p) in bindings{
            if object.id!=object.core||body.id!=object.id||body.flags>>8!=8||(body.object_flags&0xff)!=if object.fixed{16}else{8}||(body.flags&2!=0)!=object.fixed||body.clock!=0.||body.object_flags&0x1000!=0{return Err("unsupported free-world binding");}
            if !p.linear_rate.is_finite()||p.linear_rate<0.||!p.angular_rate.is_finite()||p.angular_rate.min_element()<0.||!p.inertia.is_finite()||p.inertia.min_element()<=0.||!p.limits.is_finite()||p.limits.min_element()<=0.||!p.angular_limit_exact.is_finite()||p.angular_limit_exact<=0.||(p.angular_limit_exact as f32).to_bits()!=p.limits.y.to_bits(){return Err("invalid free-world properties");}
            let mut h=history;movement::classify(&mut h,input(body,p),0.,&settings())?;
            bodies.insert(object.core,body);histories.insert(object.core,history);properties.insert(object.core,p);
        }
        Ok(Self{groups,clock,poll:Poll{remaining:10,seed},dt,gravity,alternate,rescue_gravity:None,objects,bodies,histories,properties,joints:BTreeMap::new(),object_offsets:BTreeMap::new(),retired_constraints:BTreeSet::new(),detached_constraints:BTreeMap::new(),object_joint_listeners:BTreeMap::new(),prediction_calls:vec![]})
    }
    /// Fresh sphere bodies are reconstructed from public parameters. No native
    /// initial physical fields or properties are supplied by the caller.
    pub(crate) fn from_spheres(anchor:usize,descriptions:&[(usize,crate::ivp_sphere_constructor::Parameters)],dt:f32,seed:u32,gravity:Vec3,alternate:Vec3)->Result<Self,&'static str>{
        let bindings=descriptions.iter().map(|&(id,p)|crate::ivp_sphere_constructor::construct(id,p,dt)).collect::<Result<Vec<_>,_>>()?;
        Self::with_forces(anchor,&bindings,dt,seed,gravity,alternate)
    }
    /// Numeric creation from authored compact-surface data and public parameters.
    pub(crate) fn from_polyhedra(anchor:usize,descriptions:&[(usize,crate::ivp_poly_constructor::Parameters)],dt:f32,seed:u32,gravity:Vec3,alternate:Vec3)->Result<Self,&'static str>{
        let created=descriptions.iter().map(|&(id,p)|crate::ivp_poly_constructor::construct(id,p,dt)).collect::<Result<Vec<_>,_>>()?;
        let bindings=created.iter().map(|c|c.binding).collect::<Vec<_>>();let mut world=Self::with_forces(anchor,&bindings,dt,seed,gravity,alternate)?;
        world.object_offsets=created.iter().map(|c|(c.binding.0.id,c.object_offset)).collect();Ok(world)
    }
    /// Native IVP-space contact/rescue impulses, with canonical wake ownership.
    /// Geometry discovery and conversion from public force APIs are separate.
    pub(crate) fn apply_impulses(&mut self,commands:&[crate::ivp_joint_tail::Command])->Result<(),&'static str>{
        use crate::ivp_joint_tail::Command;
        for &command in commands{
            let(id,finite)=match command{Command::PointImpulse{body,point,impulse}=>(body,point.into_iter().chain(impulse).all(f32::is_finite)),Command::AngularPush{body,delta}=>(body,delta.into_iter().all(f32::is_finite))};
            if !finite||!self.bodies.contains_key(&id){return Err("invalid registered physical impulse");}
        }
        let mut next=self.clone();let mut values=next.bodies.values().copied().collect::<Vec<_>>();
        next.groups.apply_impulses(&mut values,commands)?;
        for body in values{
            if !body.pending_linear.into_iter().chain(body.pending_angular).all(f32::is_finite){return Err("registered physical impulse overflow");}
            let history=next.histories.get_mut(&body.id).ok_or("missing impulse history")?;
            history.close_time=body.stamps[0];history.far_time=body.stamps[1];next.bodies.insert(body.id,body);
        }
        *self=next;Ok(())
    }
    pub(crate) fn set_velocity(&mut self,id:usize,linear:Option<Vec3>,angular:Option<Vec3>)->Result<(),&'static str>{
        let b=self.bodies.get(&id).ok_or("unknown public velocity core")?;
        if b.flags&2!=0{return Ok(());}
        let mut next=self.clone();next.wake(id)?;let limits=next.properties[&id].limits;
        crate::ivp_public_velocity::assign(next.bodies.get_mut(&id).unwrap(),limits,linear,angular)?;
        *self=next;Ok(())
    }
    pub(crate) fn wake(&mut self,id:usize)->Result<(),&'static str>{
        let b=self.bodies.get(&id).ok_or("unknown free-world core")?;let dormant=b.object_flags&0xff==8;
        let mut next=self.clone();next.groups.wake(id)?;
        if dormant{next.bodies.get_mut(&id).unwrap().flags|=4;}
        else{
            let effects=next.groups.take_effects();let mut values=next.bodies.values().copied().collect::<Vec<_>>();
            ivp_group_effects::apply_scheduled(&mut values,&next.objects,&effects,Context{now:next.clock.now,next:next.clock.deadline},next.dt)?;
            for e in effects{if let Effect::ResetClock(core)=e{next.histories.get_mut(&core).ok_or("missing awake wake history")?.reset_clock(next.clock.now)?;}else{return Err("unexpected awake wake effect");}}
            for b in values{next.bodies.insert(b.id,b);}
        }
        *self=next;Ok(())
    }
    pub(crate) fn register_joint_group(&mut self,id:usize,definition:GroupDefinition)->Result<(),&'static str>{
        if definition.constraints.is_empty()||!definition.tolerance.is_finite()||definition.tolerance<0.{return Err("invalid numerical group definition");}
        let mut next=self.clone();
        if !definition.rescue_gravity.is_finite(){return Err("invalid environment rescue gravity");}
        if next.rescue_gravity.is_some_and(|g|g.to_array().map(f32::to_bits)!=definition.rescue_gravity.to_array().map(f32::to_bits)){return Err("inconsistent shared environment rescue gravity");}
        next.rescue_gravity=Some(definition.rescue_gravity);next.groups.create(id)?;
        for c in &definition.constraints{
            if next.retired_constraints.contains(&c.id){return Err("orphan constraint wrapper identity remains reserved");}
            if c.objects[0]==c.objects[1]{return Err("self-bound numerical joint");}
            let class=match c.joint{crate::ivp_joint_mixed::Joint::Fixed(_)=>0,crate::ivp_joint_mixed::Joint::Ragdoll(_)=>3,crate::ivp_joint_mixed::Joint::Hinge(_)=>1};
            next.groups.insert(id,crate::ivp_group_members::Constraint{entry:crate::ivp_constraint_order::Entry{id:c.id,class},objects:[c.objects[1],c.objects[0]]})?;
            for object in c.objects{next.bodies.get_mut(&object).ok_or("joint object lacks physical body")?.object_flags|=0x1000;next.object_joint_listeners.entry(object).or_default().insert(c.id);}
        }
        next.groups.activate(id)?;let members=next.groups.group(id).unwrap().objects().to_vec();let mut offsets=vec![];
        for object in &members{
            let offset=*definition.offsets.get(object).ok_or("joint member lacks offset binding")?;let b=next.bodies[object];
            if offset.is_some()!=(b.object_flags&0x800==0)||offset.is_some_and(|v|v.iter().any(|x|!x.is_finite())){return Err("invalid joint object offset");}
            if next.object_offsets.get(object).is_some_and(|old|old.map(|v|v.map(f32::to_bits))!=offset.map(|v|v.map(f32::to_bits))){return Err("conflicting shared physical object offset");}
            next.object_offsets.insert(*object,offset);offsets.push(offset);
        }
        let bindings=definition.constraints.iter().map(|c|crate::ivp_joint_mixed::Binding{bodies:c.objects.map(|object|members.iter().position(|&id|id==object).unwrap()),joint:c.joint}).collect();
        let solver=crate::ivp_group_complete::Group{bindings,tail:crate::ivp_joint_tail::State::new(members,definition.minimum,definition.tolerance)};
        next.joints.insert(id,ActiveGroup{constraint_ids:definition.constraints.iter().map(|c|c.id).collect(),solver,offsets,additional:definition.additional,maximum:[0.;4]});
        let effects=next.groups.take_effects();let mut values=next.bodies.values().copied().collect::<Vec<_>>();
        ivp_group_effects::apply_scheduled(&mut values,&next.objects,&effects,Context{now:next.clock.now,next:next.clock.deadline},next.dt)?;
        for e in effects{match e{Effect::ResetClock(id)|Effect::Activated(id)=>next.histories.get_mut(&id).ok_or("missing registration history")?.reset_clock(next.clock.now)?,_=>return Err("unsupported registration sleep effect")}}
        for b in values{next.bodies.insert(b.id,b);}*self=next;Ok(())
    }
    fn constructor_members(&self,objects:[usize;2],offsets:&BTreeMap<usize,Option<[f32;3]>>)->Result<[crate::ivp_group_complete::Member;2],&'static str>{
        if objects[0]==objects[1]{return Err("self-bound public joint");}
        let mut pair=vec![];
        for object in objects{
            let body=*self.bodies.get(&object).ok_or("public joint object lacks physical body")?;
            let offset=*offsets.get(&object).ok_or("public joint lacks object offset")?;
            if offset.is_some()!=(body.object_flags&0x800==0)||offset.is_some_and(|v|v.iter().any(|x|!x.is_finite())){return Err("invalid public joint object offset");}
            if self.object_offsets.get(&object).is_some_and(|old|old.map(|v|v.map(f32::to_bits))!=offset.map(|v|v.map(f32::to_bits))){return Err("conflicting shared physical object offset");}
            pair.push(crate::ivp_group_complete::Member{body,object_offset:offset});
        }
        Ok([pair[0],pair[1]])
    }
    /// Native numeric fields from public parameters and owned body state.
    pub(crate) fn hinge_definition(&self,id:usize,objects:[usize;2],parameters:crate::ivp_hinge_constructor::Parameters,offsets:&BTreeMap<usize,Option<[f32;3]>>)->Result<JointDefinition,&'static str>{
        let joint=crate::ivp_hinge_constructor::construct(parameters,self.constructor_members(objects,offsets)?)?;
        Ok(JointDefinition{id,objects,joint:crate::ivp_joint_mixed::Joint::Hinge(joint)})
    }
    pub(crate) fn fixed_definition(&self,id:usize,objects:[usize;2],parameters:crate::ivp_fixed_callback::Parameters,offsets:&BTreeMap<usize,Option<[f32;3]>>)->Result<JointDefinition,&'static str>{
        self.constructor_members(objects,offsets)?;
        Ok(JointDefinition{id,objects,joint:crate::ivp_joint_mixed::Joint::Fixed(crate::ivp_fixed_callback::construct(parameters)?)})
    }
    pub(crate) fn ragdoll_definition(&self,id:usize,objects:[usize;2],parameters:crate::ivp_ragdoll_constructor::Parameters,offsets:&BTreeMap<usize,Option<[f32;3]>>)->Result<JointDefinition,&'static str>{
        let joint=crate::ivp_ragdoll_constructor::construct(parameters,self.constructor_members(objects,offsets)?)?;
        Ok(JointDefinition{id,objects,joint})
    }
    /// Append a constructor-supplied joint to an already registered group.
    /// Existing numerical caches, recovery state and member identities survive.
    pub(crate) fn insert_joint(&mut self,group:usize,c:JointDefinition,offsets:&BTreeMap<usize,Option<[f32;3]>>)->Result<(),&'static str>{
        if !self.joints.contains_key(&group){return Err("unknown numerical group");}
        if self.retired_constraints.contains(&c.id){return Err("orphan constraint wrapper identity remains reserved");}
        if c.objects[0]==c.objects[1]{return Err("self-bound numerical joint");}
        let mut next=self.clone();
        let class=match c.joint{crate::ivp_joint_mixed::Joint::Fixed(_)=>0,crate::ivp_joint_mixed::Joint::Ragdoll(_)=>3,crate::ivp_joint_mixed::Joint::Hinge(_)=>1};
        next.groups.insert(group,crate::ivp_group_members::Constraint{entry:crate::ivp_constraint_order::Entry{id:c.id,class},objects:[c.objects[1],c.objects[0]]})?;
        let members=next.groups.group(group).unwrap().objects().to_vec();let mut bound_offsets=vec![];
        for object in &members{
            let offset=*offsets.get(object).ok_or("joint member lacks offset binding")?;
            let b=next.bodies.get(object).ok_or("joint object lacks physical body")?;
            if offset.is_some()!=(b.object_flags&0x800==0)||offset.is_some_and(|v|v.iter().any(|x|!x.is_finite())){return Err("invalid joint object offset");}
            if next.object_offsets.get(object).is_some_and(|old|old.map(|v|v.map(f32::to_bits))!=offset.map(|v|v.map(f32::to_bits))){return Err("conflicting shared physical object offset");}
            next.object_offsets.insert(*object,offset);bound_offsets.push(offset);
        }
        let active=next.joints.get_mut(&group).unwrap();
        if !members.starts_with(&active.solver.tail.members){return Err("live insertion changed retained member indices");}
        active.solver.bindings.push(crate::ivp_joint_mixed::Binding{bodies:c.objects.map(|object|members.iter().position(|&id|id==object).unwrap()),joint:c.joint});
        active.constraint_ids.push(c.id);active.solver.tail.members=members;active.offsets=bound_offsets;
        for object in c.objects{next.bodies.get_mut(&object).unwrap().object_flags|=0x1000;next.object_joint_listeners.entry(object).or_default().insert(c.id);}
        let effects=next.groups.take_effects();let mut values=next.bodies.values().copied().collect::<Vec<_>>();
        ivp_group_effects::apply_scheduled(&mut values,&next.objects,&effects,Context{now:next.clock.now,next:next.clock.deadline},next.dt)?;
        for e in effects{match e{Effect::ResetClock(id)|Effect::Activated(id)=>next.histories.get_mut(&id).ok_or("missing insertion history")?.reset_clock(next.clock.now)?,_=>return Err("unsupported insertion sleep effect")}}
        for b in values{next.bodies.insert(b.id,b);}*self=next;Ok(())
    }
    /// Public constraint destruction retains group membership/controller and
    /// removes only this numerical kernel and its object-listener registrations.
    pub(crate) fn destroy_joint(&mut self,group:usize,constraint:usize)->Result<(),&'static str>{
        let active=self.joints.get(&group).ok_or("unknown numerical group")?;
        let slot=active.constraint_ids.iter().position(|&id|id==constraint).ok_or("unknown numerical constraint")?;
        if active.constraint_ids.len()!=active.solver.bindings.len(){return Err("incoherent numerical constraint identities");}
        let endpoints=active.solver.bindings[slot].bodies.map(|i|active.solver.tail.members[i]);
        let mut next=self.clone();
        //1008e590 wakes both public endpoints before wrapper/kernel destruction.
        for object in endpoints{next.wake(object)?;}
        next.groups.remove_constraint(group,constraint)?;
        for object in endpoints{
            let listeners=next.object_joint_listeners.get_mut(&object).ok_or("missing physical joint listeners")?;
            if !listeners.remove(&constraint){return Err("missing physical constraint listener");}
            if listeners.is_empty(){next.bodies.get_mut(&object).unwrap().object_flags&=!0x1000;}
        }
        let active=next.joints.get_mut(&group).unwrap();active.constraint_ids.remove(slot);active.solver.bindings.remove(slot);
        *self=next;Ok(())
    }
    /// Group/controller removal retains ordered public-wrapper endpoints.
    /// Constraint identities stay reserved until explicit detached destruction.
    pub(crate) fn detach_joint_group(&mut self,id:usize)->Result<(),&'static str>{
        if !self.joints.contains_key(&id){return Err("unknown numerical group");}
        let mut next=self.clone();let members=next.groups.group(id).ok_or("missing registered numerical group")?;
        next.retired_constraints.extend(members.order().entries().iter().map(|e|e.id));
        let active=&next.joints[&id];if active.constraint_ids.len()!=active.solver.bindings.len(){return Err("incoherent detached constraint identities");}
        for (&constraint,binding) in active.constraint_ids.iter().zip(&active.solver.bindings){let endpoints=binding.bodies.map(|i|active.solver.tail.members[i]);if next.detached_constraints.insert(constraint,endpoints).is_some(){return Err("duplicate detached wrapper identity");}}
        next.groups.remove(id)?;next.joints.remove(&id);
        let effects=next.groups.take_effects();let mut values=next.bodies.values().copied().collect::<Vec<_>>();
        ivp_group_effects::apply_scheduled(&mut values,&next.objects,&effects,Context{now:next.clock.now,next:next.clock.deadline},next.dt)?;
        for e in effects{match e{Effect::ResetClock(id)|Effect::Activated(id)=>next.histories.get_mut(&id).ok_or("missing detach history")?.reset_clock(next.clock.now)?,_=>return Err("unsupported detach sleep effect")}}
        for b in values{next.bodies.insert(b.id,b);}*self=next;Ok(())
    }
    /// A detached public wrapper keeps its ordered endpoints until destruction.
    pub(crate) fn destroy_detached_joint(&mut self,id:usize)->Result<(),&'static str>{
        let endpoints=*self.detached_constraints.get(&id).ok_or("unknown detached constraint wrapper")?;if !self.retired_constraints.contains(&id){return Err("unreserved detached constraint wrapper");}
        let mut next=self.clone();for object in endpoints{next.wake(object)?;}
        for object in endpoints{let listeners=next.object_joint_listeners.get_mut(&object).ok_or("missing detached wrapper listeners")?;if !listeners.remove(&id){return Err("missing detached constraint listener");}if listeners.is_empty(){next.bodies.get_mut(&object).ok_or("missing detached wrapper body")?.object_flags&=!0x1000;}}
        next.detached_constraints.remove(&id);next.retired_constraints.remove(&id);*self=next;Ok(())
    }
    pub(crate) fn queue_recovery(&mut self,id:usize,a:Option<usize>,b:Option<usize>)->Result<bool,&'static str>{Ok(self.joints.get_mut(&id).ok_or("unknown numerical group")?.solver.tail.queue(a,b))}
    pub(crate) fn advance(&mut self,delta:f32)->Result<(),&'static str>{
        let mut next=self.clone();next.prediction_calls.clear();let dt=next.dt;let gravity=next.gravity;let alternate=next.alternate;let rescue_gravity=next.rescue_gravity.unwrap_or(Vec3::ZERO);let properties=&next.properties;let objects=&next.objects;
        let bodies=RefCell::new(std::mem::take(&mut next.bodies));let histories=RefCell::new(std::mem::take(&mut next.histories));
        let joints=RefCell::new(std::mem::take(&mut next.joints));let prediction_calls=RefCell::new(std::mem::take(&mut next.prediction_calls));
        let(groups,poll)=(&mut next.groups,&mut next.poll);
        next.clock.simulate(delta,|_,now,deadline|{
            let mut processed=vec![];
            groups.psi_prepared_ordered_with(poll,
                |effects|{
                    if effects.iter().any(|e|matches!(e,Effect::ForcedSleep(_))){return Err("forced sleep requires explicit eligibility and geometry");}
                    let mut values=bodies.borrow().values().copied().collect::<Vec<_>>();
                    ivp_group_effects::apply_scheduled(&mut values,objects,&effects,Context{now,next:deadline},dt)?;
                    for e in &effects{let id=match *e{Effect::Activated(id)|Effect::ResetClock(id)=>id,Effect::Slept(_)=>continue,_=>unreachable!()};histories.borrow_mut().get_mut(&id).ok_or("missing effect history")?.reset_clock(now)?;
                        if matches!(e,Effect::Activated(_)){values.iter_mut().find(|b|b.id==id).unwrap().flags&=!4;}}
                    for b in values{bodies.borrow_mut().insert(b.id,b);}Ok(())
                },|cores,flags|{
                    let mut values=cores.iter().map(|id|bodies.borrow().get(id).copied().ok_or("missing preparation body")).collect::<Result<Vec<_>,_>>()?;
                    let result=crate::ivp_unit_prepare::prepare(&mut values,flags,now)?;
                    for b in values{if result.reset_movement_clock{histories.borrow_mut().get_mut(&b.id).ok_or("missing preparation history")?.reset_clock(now)?;}bodies.borrow_mut().insert(b.id,b);}
                    processed.extend(cores.iter().rev().copied());Ok(())
                },|key,cores,order|{
                    if let ControllerKey::Joint(id)=key{
                        let mut controllers=joints.borrow_mut();let g=controllers.get_mut(&id).ok_or("missing numerical group")?;
                        let order=order.ok_or("joint controller lacks prepared order")?;
                        if order.len()!=g.constraint_ids.len()||order.len()!=g.solver.bindings.len(){return Err("numerical constraint order size mismatch");}
                        let mut bindings=g.constraint_ids.iter().copied().zip(g.solver.bindings.iter().copied()).collect::<BTreeMap<_,_>>();
                        let reordered=order.iter().map(|entry|bindings.remove(&entry.id).ok_or("prepared constraint lacks numerical binding")).collect::<Result<Vec<_>,_>>()?;
                        if !bindings.is_empty(){return Err("numerical binding absent from prepared order");}
                        g.constraint_ids=order.iter().map(|e|e.id).collect();g.solver.bindings=reordered;

                        let mut members=g.solver.tail.members.iter().zip(&g.offsets).map(|(id,offset)|bodies.borrow().get(id).copied().map(|body|crate::ivp_group_complete::Member{body,object_offset:*offset}).ok_or("missing group body")).collect::<Result<Vec<_>,_>>()?;
                        let dynamic_members=g.solver.tail.members.iter().filter(|id|bodies.borrow()[id].flags&2==0).count();
                        if cores.len()!=dynamic_members||cores.iter().any(|id|!g.solver.tail.members.contains(id)||bodies.borrow()[id].flags&2!=0){return Err("group controller membership mismatch");}
                        let result=g.solver.run_with(&mut members,g.additional,crate::ivp_group_complete::Environment{dt,inverse_dt:dt.recip(),rescue_dt:dt,gravity:rescue_gravity.to_array()},|_,_,_,_,_,_|{},|_,_,_,_,_|{})?;g.maximum=result.error.maximum;
                        for m in members{let b=m.body;let mut hs=histories.borrow_mut();let h=hs.get_mut(&b.id).ok_or("missing group history")?;h.close_time=b.stamps[0];h.far_time=b.stamps[1];bodies.borrow_mut().insert(b.id,b);}return Ok(());
                    }
                    if !matches!(key,ControllerKey::Base){return Err("contact controller numerical dispatch is not connected");}
                    let mut forces=BTreeMap::new();
                    for id in cores{let b=bodies.borrow()[id];let p=properties[id];forces.insert(*id,crate::ivp_base_controller::Body{core:crate::ivp_contact::Core{inverse_mass:b.inverse[3],inverse_inertia:Vec3::new(b.inverse[0],b.inverse[1],b.inverse[2]),linear:Vec3::new(b.linear[0],b.linear[1],b.linear[2]),angular:Vec3::new(b.angular[0],b.angular[1],b.angular[2]),pending_linear:Vec3::new(b.pending_linear[0],b.pending_linear[1],b.pending_linear[2]),pending_angular:Vec3::new(b.pending_angular[0],b.pending_angular[1],b.pending_angular[2])},flags:b.flags as u8,movement:(b.flags>>8) as u8,linear_rate:p.linear_rate,angular_rate:p.angular_rate});}
                    crate::ivp_base_controller::Controller{gravity,alternate}.execute(&mut forces,cores,dt)?;
                    for (id,f) in forces{let mut map=bodies.borrow_mut();let b=map.get_mut(&id).unwrap();b.linear[..3].copy_from_slice(&f.core.linear.to_array());b.angular[..3].copy_from_slice(&f.core.angular.to_array());b.pending_linear[..3].copy_from_slice(&f.core.pending_linear.to_array());b.pending_angular[..3].copy_from_slice(&f.core.pending_angular.to_array());}Ok(())
                },|id|{
                    let mut map=bodies.borrow_mut();let b=map.get_mut(&id).ok_or("missing movement body")?;
                    let mut hs=histories.borrow_mut();let h=hs.get_mut(&id).ok_or("missing movement history")?;
                    let state=movement::classify(h,input(*b,properties[&id]),now,&settings())?;
                    b.flags=(b.flags&0xff)|((state as u16)<<8);b.stamps=[h.close_time,h.far_time];Ok(state)
                })?;
            //1010fd30 consumes the complete collected core vector backwards.
            for id in processed.into_iter().rev(){
                prediction_calls.borrow_mut().push(id);
                let mut map=bodies.borrow_mut();let b=map.get_mut(&id).unwrap();let p=properties[&id];
                let result=b.prediction.advance(crate::ivp_motion::MotionStep{time:now,dt,velocity:Vec3::new(b.linear[0],b.linear[1],b.linear[2]),angular:Vec3::new(b.angular[0],b.angular[1],b.angular[2]),inertia:p.inertia,inverse_inertia:Vec3::new(b.inverse[0],b.inverse[1],b.inverse[2]),limits:p.limits,angular_limit_exact:p.angular_limit_exact,gyro:p.gyro});
                b.linear[..3].copy_from_slice(&b.prediction.velocity.to_array());b.angular[..3].copy_from_slice(&result.angular.to_array());
                if !b.prediction.start.is_finite()||!b.prediction.velocity.is_finite()||!result.angular.is_finite()||!b.prediction.rotation_end.iter().all(|v|v.is_finite()){return Err("free-world prediction overflow");}
            }Ok(())
        },|_,_,_|Err("free-world contact events are not implemented"),|_,_|{})?;
        next.joints=joints.into_inner();next.prediction_calls=prediction_calls.into_inner();next.bodies=bodies.into_inner();next.histories=histories.into_inner();for b in next.bodies.values_mut(){b.clock=next.clock.now;}
        *self=next;Ok(())
    }
}
#[cfg(test)]
mod tests{
    use super::*;use serde_json::{Value,json};
    fn fixture()->Value{serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-motion.json")).unwrap())).unwrap()}
    fn f(v:&Value)->f32{f32::from_bits(v.as_u64().unwrap() as u32)}fn d(v:&Value)->f64{f64::from_bits(v.as_u64().unwrap())}fn v(a:&Value)->Vec3{Vec3::from_array(std::array::from_fn(|i|f(&a[i])))}
    fn history(h:&Value)->History{History{close_time:d(&h["close_time"]),far_time:d(&h["far_time"]),close_position:v(&h["close_position"]),far_position:v(&h["far_position"]),close_quat:std::array::from_fn(|i|f(&h["close_quat"][i])),far_quat:std::array::from_fn(|i|f(&h["far_quat"][i]))}}
    fn snapshot_history(h:History)->Value{json!({"close_time":h.close_time.to_bits(),"far_time":h.far_time.to_bits(),"close_position":h.close_position.to_array().map(f32::to_bits),"far_position":h.far_position.to_array().map(f32::to_bits),"close_quat":h.close_quat.map(f32::to_bits),"far_quat":h.far_quat.map(f32::to_bits)})}
    #[test]
    fn failed_force_execution_rolls_back_preparation_clock_registry_poll_and_physical_state(){
        let data=fixture();let c=&data["cases"][0];let initial=&c["states"][0];let p=&c["properties"];
        let properties=Properties{radius:f(&p["radius"]),sleep_time:f(&p["sleep_time"]),linear_rate:f(&p["linear_rate"]),angular_rate:v(&p["angular_rate"]),inertia:v(&p["inertia"]),limits:Vec2::new(f(&p["limits"][0]),f(&p["limits"][1])),angular_limit_exact:0.95199776f32 as f64*(1f32/60.).recip() as f64,gyro:p["gyro"].as_bool().unwrap()};
        let body=crate::ivp_joint_apply::tests::body(&initial["physical"]);
        let mut world=FreeWorld::new(99,&[(Object{id:0,core:0,fixed:false},body,history(&initial["history"]),properties)],1./60.,1).unwrap();
        world.wake(0).unwrap();world.properties.get_mut(&0).unwrap().linear_rate=-1.;
        let before=format!("{world:?}");assert!(world.advance(1./60.).is_err());assert_eq!(format!("{world:?}"),before);
    }
    fn sleep_fixture()->Value{serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-sleep.json")).unwrap())).unwrap()}
    #[test]
    fn native_registered_settle_sleep_wake_and_resleep_trajectories_are_source_owned(){
        let data=sleep_fixture();let mut seed=1;let(mut steps,mut sleeps,mut wakes)=(0,0,0);
        for c in data["cases"].as_array().unwrap(){let states=c["states"].as_array().unwrap();let initial=&states[0];let p=&c["properties"];
            let properties=Properties{radius:f(&p["radius"]),sleep_time:f(&p["sleep_time"]),linear_rate:f(&p["linear_rate"]),angular_rate:v(&p["angular_rate"]),inertia:v(&p["inertia"]),limits:Vec2::new(f(&p["limits"][0]),f(&p["limits"][1])),angular_limit_exact:0.95199776f32 as f64*(1f32/60.).recip() as f64,gyro:p["gyro"].as_bool().unwrap()};
            let body=crate::ivp_joint_apply::tests::body(&initial["physical"]);let binding=(Object{id:0,core:0,fixed:false},body,history(&initial["history"]),properties);
            let mut world=FreeWorld::new(99,&[binding],1./60.,seed).unwrap();world.wake(0).unwrap();
            for (index,s) in states.iter().enumerate(){if index>0{
                    if index-1==c["wake_before_step"].as_u64().unwrap() as usize{assert_eq!(world.bodies[&0].flags>>8,8);world.wake(0).unwrap();wakes+=1;}
                    let was_asleep=world.bodies[&0].flags>>8>=8;world.advance(1./60.).unwrap();sleeps+=usize::from(!was_asleep&&world.bodies[&0].flags>>8>=8);steps+=1;
                }
                assert_eq!(crate::ivp_joint_apply::tests::snapshot(world.bodies[&0]),s["physical"],"scenario{}/step{index}/physical",c["scenario"]);
                assert_eq!(snapshot_history(world.histories[&0]),s["history"],"scenario{}/step{index}/history",c["scenario"]);
                assert_eq!(json!({"now":world.clock.now.to_bits(),"next":world.clock.deadline.to_bits()}),s["clock"]);
                assert_eq!(json!({"remaining":world.poll.remaining,"seed":world.poll.seed}),s["poll"]);
                assert_eq!(world.groups.registry().unit_flags(0).unwrap().flags&0x3fff,s["unit_flags"].as_u64().unwrap() as u32);
            }seed=world.poll.seed;
        }assert_eq!((steps,sleeps,wakes),(3072,32,16));
    }
    #[test]
    fn native_registered_moving_trajectories_own_preparation_damping_history_and_prediction(){
        let data=fixture();let mut seed=1;let mut steps=0;
        for c in data["cases"].as_array().unwrap(){let states=c["states"].as_array().unwrap();let initial=&states[0];let p=&c["properties"];
            let properties=Properties{radius:f(&p["radius"]),sleep_time:f(&p["sleep_time"]),linear_rate:f(&p["linear_rate"]),angular_rate:v(&p["angular_rate"]),inertia:v(&p["inertia"]),limits:Vec2::new(f(&p["limits"][0]),f(&p["limits"][1])),angular_limit_exact:0.95199776f32 as f64*(1f32/60.).recip() as f64,gyro:p["gyro"].as_bool().unwrap()};
            let body=crate::ivp_joint_apply::tests::body(&initial["physical"]);let binding=(Object{id:0,core:0,fixed:false},body,history(&initial["history"]),properties);
            let mut world=FreeWorld::new(99,&[binding],1./60.,seed).unwrap();world.wake(0).unwrap();
            for (index,s) in states.iter().enumerate(){if index>0{world.advance(1./60.).unwrap();steps+=1;}
                assert_eq!(crate::ivp_joint_apply::tests::snapshot(world.bodies[&0]),s["physical"],"scenario{}/step{index}/physical",c["scenario"]);
                assert_eq!(snapshot_history(world.histories[&0]),s["history"],"scenario{}/step{index}/history",c["scenario"]);
                assert_eq!(json!({"now":world.clock.now.to_bits(),"next":world.clock.deadline.to_bits()}),s["clock"]);
                assert_eq!(json!({"remaining":world.poll.remaining,"seed":world.poll.seed}),s["poll"]);
                assert_eq!(world.groups.registry().unit_flags(0).unwrap().flags&0x3fff,s["unit_flags"].as_u64().unwrap() as u32);
            }seed=world.poll.seed;
        }assert_eq!(steps,1024);
    }    fn forces_fixture()->Value{serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-forces.json")).unwrap())).unwrap()}
    #[test]
    fn native_registered_gravity_and_anisotropic_gyroscopic_trajectories_match(){
        let data=forces_fixture();let mut seed=1;let mut steps=0;let mut wakes=0;
        for c in data["cases"].as_array().unwrap(){let states=c["states"].as_array().unwrap();let initial=&states[0];let p=&c["properties"];
            let properties=Properties{radius:f(&p["radius"]),sleep_time:f(&p["sleep_time"]),linear_rate:f(&p["linear_rate"]),angular_rate:v(&p["angular_rate"]),inertia:v(&p["inertia"]),limits:Vec2::new(f(&p["limits"][0]),f(&p["limits"][1])),angular_limit_exact:0.95199776f32 as f64*(1f32/60.).recip() as f64,gyro:p["gyro"].as_bool().unwrap()};
            let body=crate::ivp_joint_apply::tests::body(&initial["physical"]);let binding=(Object{id:0,core:0,fixed:false},body,history(&initial["history"]),properties);
            let mut world=FreeWorld::with_forces(99,&[binding],1./60.,seed,v(&p["gravity"]),v(&p["alternate"])).unwrap();world.wake(0).unwrap();
            for (index,s) in states.iter().enumerate(){if index>0{
                    if c["wake_before_steps"].as_array().unwrap().iter().any(|n|n.as_u64().unwrap() as usize==index-1){assert!(world.bodies[&0].flags>>8<8);world.wake(0).unwrap();wakes+=1;}
                    world.advance(1./60.).unwrap();steps+=1;}
                assert_eq!(crate::ivp_joint_apply::tests::snapshot(world.bodies[&0]),s["physical"],"scenario{}/step{index}/physical",c["scenario"]);
                assert_eq!(snapshot_history(world.histories[&0]),s["history"],"scenario{}/step{index}/history",c["scenario"]);
                assert_eq!(json!({"now":world.clock.now.to_bits(),"next":world.clock.deadline.to_bits()}),s["clock"]);
                assert_eq!(json!({"remaining":world.poll.remaining,"seed":world.poll.seed}),s["poll"]);
                assert_eq!(world.groups.registry().unit_flags(0).unwrap().flags&0x3fff,s["unit_flags"].as_u64().unwrap() as u32);
            }seed=world.poll.seed;
        }assert_eq!((steps,wakes),(1024,32));
    }
    fn joint_fixture()->Value{serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-joint.json")).unwrap())).unwrap()}
    fn joint_world(c:&Value,seed:u32)->FreeWorld{
        let initial=&c["initial"];let bindings=initial["physical"].as_array().unwrap().iter().enumerate().map(|(id,b)|{
            let p=&c["properties"][id];let properties=Properties{radius:f(&p["radius"]),sleep_time:f(&p["sleep_time"]),linear_rate:f(&p["linear_rate"]),angular_rate:v(&p["angular_rate"]),inertia:v(&p["inertia"]),limits:Vec2::new(f(&p["limits"][0]),f(&p["limits"][1])),angular_limit_exact:0.95199776f32 as f64*(1f32/60.).recip() as f64,gyro:p["gyro"].as_bool().unwrap()};
            (Object{id,core:id,fixed:b["flags"].as_u64().unwrap()&2!=0},crate::ivp_joint_apply::tests::body(b),history(&initial["histories"][id]),properties)
        }).collect::<Vec<_>>();
        let mut world=FreeWorld::with_forces(99,&bindings,1./60.,seed,v(&c["gravity"]),v(&c["alternate"])).unwrap();for (o,_,_,_) in &bindings{if !o.fixed{world.wake(o.id).unwrap();}}world
    }
    fn joint_definition(c:&Value)->GroupDefinition{
        let constraints=c["constructors"].as_array().unwrap().iter().map(|j|JointDefinition{id:j["id"].as_u64().unwrap() as usize,objects:std::array::from_fn(|i|j["objects"][i].as_u64().unwrap() as usize),joint:if j["kind"]==3{crate::ivp_joint_mixed::Joint::Ragdoll(crate::ivp_joint_callback::tests::joint(&j["parameters"]))}else{crate::ivp_joint_mixed::Joint::Hinge(crate::ivp_hinge_callback::tests::joint(&j["parameters"]))}}).collect();
        let offsets=c["properties"].as_array().unwrap().iter().enumerate().map(|(id,p)|(id,(!p["object_offset"].is_null()).then(||std::array::from_fn(|i|f(&p["object_offset"][i]))))).collect();
        GroupDefinition{constraints,offsets,additional:c["additional"].as_u64().unwrap() as u32,minimum:c["minimum"].as_i64().unwrap() as i32,tolerance:c["tolerance"].as_f64().unwrap() as f32,rescue_gravity:v(&c["rescue_gravity"])}
    }
    fn joint_snapshot(world:&FreeWorld)->Value{
        let group=world.joints.get(&0).map(|g|{let m=world.groups.group(0).unwrap();json!({"members":g.solver.tail.members,"order":m.order().entries().iter().map(|e|e.id).collect::<Vec<_>>(),"dirty":u8::from(m.order().dirty()),"active":u8::from(m.active()),"ticks":g.solver.tail.ticks,"minimum":g.solver.tail.minimum,"tolerance":g.solver.tail.tolerance.to_bits(),"maximum":g.maximum.map(f32::to_bits)})});
        json!({"physical":world.bodies.values().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>(),"histories":world.histories.values().copied().map(snapshot_history).collect::<Vec<_>>(),"unit_flags":world.bodies.keys().map(|&id|world.groups.registry().unit_flags(id).unwrap().flags&0x3fff).collect::<Vec<_>>(),"clock":{"now":world.clock.now.to_bits(),"next":world.clock.deadline.to_bits()},"poll":{"remaining":world.poll.remaining,"seed":world.poll.seed},"group":group,"prediction_order":world.prediction_calls})
    }
    #[test]
    fn native_registered_joint_chains_own_coupled_physics_histories_error_and_prediction_order(){
        let data=joint_fixture();let mut seed=1;let(mut states,mut steps,mut body_states,mut queues)=(0,0,0,0);
        for c in data["cases"].as_array().unwrap(){let mut world=joint_world(c,seed);assert_eq!(joint_snapshot(&world),c["initial"]);states+=1;body_states+=world.bodies.len();
            world.register_joint_group(0,joint_definition(c)).unwrap();
            for (index,s) in c["states"].as_array().unwrap().iter().enumerate(){
                if index>0{if c["queue_before_steps"].as_array().unwrap().iter().any(|n|n.as_u64().unwrap() as usize==index-1){assert!(world.queue_recovery(0,Some(0),Some(world.bodies.len()-1)).unwrap());queues+=1;}world.advance(1./60.).unwrap();steps+=1;}
                assert_eq!(joint_snapshot(&world),*s,"scenario{}/step{index}",c["scenario"]);states+=1;body_states+=world.bodies.len();
            }seed=world.poll.seed;
        }assert_eq!((states,steps,body_states,queues),(408,384,1428,24));
    }
    #[test]
    fn failed_late_group_registration_and_numerical_execution_roll_back_the_whole_owner(){
        let data=joint_fixture();let c=&data["cases"][0];let mut world=joint_world(c,1);let mut bad=joint_definition(c);let first=bad.constraints[0];bad.constraints.push(JointDefinition{id:99,objects:[0,999],joint:first.joint});let before=format!("{world:?}");assert!(world.register_joint_group(0,bad).is_err());assert_eq!(format!("{world:?}"),before);
        world.register_joint_group(0,joint_definition(c)).unwrap();world.joints.get_mut(&0).unwrap().solver.bindings[0].bodies[1]=usize::MAX;
        let before=format!("{world:?}");assert!(world.advance(1./60.).is_err());assert_eq!(format!("{world:?}"),before);
    }

    fn overlap_fixture()->Value{serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-overlap.json")).unwrap())).unwrap()}
    fn overlap_definition(c:&Value,id:usize)->GroupDefinition{
        let mut selected=c.clone();selected["constructors"]=json!(c["constructors"].as_array().unwrap().iter().filter(|j|j["group"].as_u64()==Some(id as u64)).cloned().collect::<Vec<_>>());selected["additional"]=json!((c["additional"].as_u64().unwrap()+id as u64)%3);joint_definition(&selected)
    }
    fn overlap_snapshot(world:&FreeWorld)->Value{
        let mut value=joint_snapshot(world);value.as_object_mut().unwrap().remove("group");value["groups"]=json!((0..2).map(|id|world.joint_group(id).map(|g|{let m=world.groups.group(id).unwrap();json!({"members":g.solver.tail.members,"order":m.order().entries().iter().map(|e|e.id).collect::<Vec<_>>(),"dirty":u8::from(m.order().dirty()),"active":u8::from(m.active()),"ticks":g.solver.tail.ticks,"minimum":g.solver.tail.minimum,"tolerance":g.solver.tail.tolerance.to_bits(),"maximum":g.maximum.map(f32::to_bits)})})).collect::<Vec<_>>());value
    }
    #[test]
    fn native_overlapping_groups_detach_and_split_with_source_owned_physics(){
        let data=overlap_fixture();let mut seed=1;let(mut steps,mut states,mut detaches,mut requests)=(0,0,0,0);
        for c in data["cases"].as_array().unwrap(){let mut world=joint_world(c,seed);assert_eq!(overlap_snapshot(&world),c["initial"]);states+=1;
            for id in 0..2{world.register_joint_group(id,overlap_definition(c,id)).unwrap();}
            for (index,s) in c["states"].as_array().unwrap().iter().enumerate(){if index>0{
                    let step=index-1;
                    for req in c["requests"].as_array().unwrap().iter().filter(|r|r["step"].as_u64()==Some(step as u64)){assert!(world.queue_recovery(req["group"].as_u64().unwrap() as usize,Some(req["objects"][0].as_u64().unwrap() as usize),Some(req["objects"][1].as_u64().unwrap() as usize)).unwrap());requests+=1;}
                    for (id,at) in c["detach_before_steps"].as_array().unwrap().iter().enumerate(){if at.as_u64()==Some(step as u64){world.detach_joint_group(id).unwrap();detaches+=1;}}
                    world.advance(1./60.).unwrap();steps+=1;
                }
                assert_eq!(overlap_snapshot(&world),*s,"scenario{}/step{index}",c["scenario"]);states+=1;
            }seed=world.poll.seed;
        }assert_eq!((steps,states,detaches,requests),(384,400,16,24));
    }
    #[test]
    fn shared_object_offsets_and_detached_constraint_identities_cannot_diverge(){
        let data=overlap_fixture();let c=&data["cases"][4];let mut world=joint_world(c,1);world.register_joint_group(0,overlap_definition(c,0)).unwrap();
        let mut bad=overlap_definition(c,1);bad.offsets.get_mut(&0).unwrap().as_mut().unwrap()[0]+=0.01;let before=format!("{world:?}");assert!(world.register_joint_group(1,bad).is_err());assert_eq!(format!("{world:?}"),before);
        world.register_joint_group(1,overlap_definition(c,1)).unwrap();world.detach_joint_group(0).unwrap();let before=format!("{world:?}");assert!(world.register_joint_group(2,overlap_definition(c,0)).is_err());assert_eq!(format!("{world:?}"),before);
        assert!(world.detach_joint_group(99).is_err());assert_eq!(format!("{world:?}"),before);
    }

fn sphere_parameters(c:&Value)->crate::ivp_sphere_constructor::Parameters{let p=&c["inputs"];crate::ivp_sphere_constructor::Parameters{radius:f(&p["radius"]),position:std::array::from_fn(|i|f(&p["position"][i])),angles:std::array::from_fn(|i|f(&p["angles"][i])),mass:f(&p["mass"]),inertia_scale:f(&p["inertia"]),damping:f(&p["damping"]),angular_damping:f(&p["rot_damping"]),rotational_limit:f(&p["rot_limit"]),fixed:p["fixed"].as_bool().unwrap(),collisions:p["collisions"].as_bool().unwrap()}}
fn sphere_properties(p:Properties)->Value{json!({"radius":p.radius.to_bits(),"sleep_time":p.sleep_time.to_bits(),"linear_rate":p.linear_rate.to_bits(),"angular_rate":p.angular_rate.to_array().map(f32::to_bits),"inertia":p.inertia.to_array().map(f32::to_bits),"limits":p.limits.to_array().map(f32::to_bits),"gyro":p.gyro})}
fn sphere_snapshot(world:&FreeWorld)->Value{json!({"physical":crate::ivp_joint_apply::tests::snapshot(world.bodies()[&0]),"history":snapshot_history(world.histories()[&0]),"unit_flags":world.groups().registry().unit_flags(0).unwrap().flags&0x3fff,"poll":{"remaining":world.poll().remaining,"seed":world.poll().seed},"clock":{"now":world.clock().now.to_bits(),"next":world.clock().deadline.to_bits()}})}
#[test]
fn native_public_sphere_construction_owns_initial_body_and_properties(){
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-sphere-constructor.json")).unwrap())).unwrap();let mut seed=1;
 for c in data["cases"].as_array().unwrap(){let desc=sphere_parameters(c);let dt=f(&c["inputs"]["dt"]);let mut world=FreeWorld::from_spheres(99,&[(0,desc)],dt,seed,Vec3::ZERO,Vec3::ZERO).unwrap();
 let(_,_,_,props)=crate::ivp_sphere_constructor::construct(0,desc,dt).unwrap();assert_eq!(sphere_properties(props),c["properties"]);
 assert_eq!(sphere_snapshot(&world),c["constructed"],"constructed sphere{}",c["scenario"]);
 let p=&c["inputs"];let linear=v(&p["linear"]);let angular=v(&p["angular"]);world.set_velocity(0,Some(linear),Some(angular)).unwrap();
 for(index,state)in c["states"].as_array().unwrap().iter().enumerate(){if index>0{if [16,48].contains(&(index-1)){world.set_velocity(0,Some(linear),Some(angular)).unwrap();}world.advance(dt).unwrap();}
 let actual=sphere_snapshot(&world);
 assert_eq!(actual,*state,"sphere{}/step{index}",c["scenario"]);}
 seed=world.poll().seed;
 }
}

    #[test]
    fn sphere_creation_and_invalid_velocity_changes_cannot_commit_partial_world_state(){
        let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-sphere-constructor.json")).unwrap())).unwrap();let p=sphere_parameters(&data["cases"][1]);let mut world=FreeWorld::from_spheres(99,&[(0,p)],1./60.,1,Vec3::ZERO,Vec3::ZERO).unwrap();
        assert!(FreeWorld::from_spheres(99,&[(0,p),(0,p)],1./60.,1,Vec3::ZERO,Vec3::ZERO).is_err());
        assert!(FreeWorld::from_spheres(99,&[(0,crate::ivp_sphere_constructor::Parameters{radius:-1.,..p})],1./60.,1,Vec3::ZERO,Vec3::ZERO).is_err());
        let before=format!("{world:?}");assert!(world.set_velocity(0,Some(Vec3::splat(f32::NAN)),None).is_err());assert_eq!(format!("{world:?}"),before);
        assert!(world.set_velocity(0,None,Some(Vec3::splat(f32::INFINITY))).is_err());assert_eq!(format!("{world:?}"),before);
        assert!(world.set_velocity(99,Some(Vec3::X),None).is_err());assert_eq!(format!("{world:?}"),before);
    }

fn poly_parameters(c:&Value)->crate::ivp_poly_constructor::Parameters{let p=&c["inputs"];crate::ivp_poly_constructor::Parameters{surface:crate::ivp_poly_constructor::Surface{center:v(&p["shape_center"]),unit_inertia:v(&p["shape_inertia"]),radius:f(&p["shape_radius"]),factor:p["shape_factor"].as_u64().unwrap() as u8},margin:f(&p["shape_margin"]),center:(!p["center"].is_null()).then(||std::array::from_fn(|i|f(&p["center"][i]))),position:std::array::from_fn(|i|f(&p["position"][i])),angles:std::array::from_fn(|i|f(&p["angles"][i])),mass:f(&p["mass"]),inertia_scale:f(&p["inertia"]),damping:f(&p["damping"]),angular_damping:f(&p["rot_damping"]),rotational_limit:f(&p["rot_limit"]),fixed:p["fixed"].as_bool().unwrap(),collisions:p["collisions"].as_bool().unwrap()}}
#[test]
fn native_public_poly_construction_owns_body_centers_bounds_and_trajectories(){
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-poly-constructor.json")).unwrap())).unwrap();let mut seed=1;
 for c in data["cases"].as_array().unwrap(){let p=poly_parameters(c);let dt=f(&c["inputs"]["dt"]);let created=crate::ivp_poly_constructor::construct(0,p,dt).unwrap();let mut props=sphere_properties(created.binding.3);props["object_offset"]=created.object_offset.map(|o|json!(o.map(f32::to_bits))).unwrap_or(Value::Null);props["angular_radius"]=json!(created.angular_radius.to_bits());assert_eq!(props,c["properties"],"poly properties{}",c["scenario"]);
 let mut world=FreeWorld::from_polyhedra(99,&[(0,p)],dt,seed,Vec3::ZERO,Vec3::ZERO).unwrap();assert_eq!(world.object_offsets[&0],created.object_offset);
 assert_eq!(sphere_snapshot(&world),c["constructed"],"constructed poly{}",c["scenario"]);
 let input=&c["inputs"];let linear=v(&input["linear"]);let angular=v(&input["angular"]);world.set_velocity(0,Some(linear),Some(angular)).unwrap();
 for(index,state)in c["states"].as_array().unwrap().iter().enumerate(){if index>0{if [16,48].contains(&(index-1)){world.set_velocity(0,Some(linear),Some(angular)).unwrap();}world.advance(dt).unwrap();}assert_eq!(sphere_snapshot(&world),*state,"poly{}/step{index}",c["scenario"]);}seed=world.poll().seed;
 }
}
#[test]
fn poly_creation_rejects_invalid_shapes_and_preserves_authored_center_bindings(){
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-poly-constructor.json")).unwrap())).unwrap();let p=poly_parameters(&data["cases"][1]);
 assert!(FreeWorld::from_polyhedra(99,&[(0,p),(0,p)],1./60.,1,Vec3::ZERO,Vec3::ZERO).is_err());
 for bad in [crate::ivp_poly_constructor::Parameters{margin:-1.,..p},crate::ivp_poly_constructor::Parameters{center:Some([f32::NAN,0.,0.]),..p},crate::ivp_poly_constructor::Parameters{surface:crate::ivp_poly_constructor::Surface{unit_inertia:Vec3::ZERO,..p.surface},..p}] {assert!(crate::ivp_poly_constructor::construct(0,bad,1./60.).is_err());}
 let world=FreeWorld::from_polyhedra(99,&[(0,p),(1,p)],1./60.,1,Vec3::ZERO,Vec3::ZERO).unwrap();let before=format!("{world:?}");let mut offsets=world.object_offsets.clone();offsets.get_mut(&0).unwrap().as_mut().unwrap()[0]+=1.;assert!(world.hinge_definition(0,[0,1],crate::ivp_hinge_constructor::Parameters{position:[0.;3],direction:[1.,0.,0.],limits:[0.;2],velocity:0.,torque:0.,strength:1.,force_limit:0.,torque_limit:0.,mass_scale:[1.;2]},&offsets).is_err());assert_eq!(format!("{world:?}"),before);
}

fn impulse_commands(c:&Value)->[crate::ivp_joint_tail::Command;3]{use crate::ivp_joint_tail::Command;let p=&c["inputs"];let point=Command::PointImpulse{body:0,point:v(&p["impulse_point"]).to_array(),impulse:v(&p["point_impulse"]).to_array()};[point,Command::AngularPush{body:0,delta:v(&p["angular_push"]).to_array()},point]}
fn impulse_snapshot(world:&FreeWorld)->Value{let mut state=sphere_snapshot(world);state["pending"]=json!(world.groups().registry().pending_cores());state}
#[test]
fn native_registered_impulse_bursts_own_wake_history_and_following_motion(){
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-impulses.json")).unwrap())).unwrap();let mut seed=1;
 for c in data["cases"].as_array().unwrap(){let p=poly_parameters(c);let dt=f(&c["inputs"]["dt"]);let mut world=FreeWorld::from_polyhedra(99,&[(0,p)],dt,seed,Vec3::ZERO,Vec3::ZERO).unwrap();assert_eq!(impulse_snapshot(&world),c["constructed"],"constructed impulse{}",c["scenario"]);let commands=impulse_commands(c);world.apply_impulses(&commands).unwrap();
 for(index,state)in c["states"].as_array().unwrap().iter().enumerate(){if index>0{if [16,48].contains(&(index-1)){world.apply_impulses(&commands).unwrap();}world.advance(dt).unwrap();}assert_eq!(impulse_snapshot(&world),*state,"impulse{}/step{index}",c["scenario"]);}seed=world.poll().seed;
 }
}
#[test]
fn invalid_later_physical_impulses_cannot_commit_wake_body_or_history_prefixes(){
 use crate::ivp_joint_tail::Command;
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-impulses.json")).unwrap())).unwrap();let c=&data["cases"][1];let p=poly_parameters(c);let mut world=FreeWorld::from_polyhedra(99,&[(0,p)],1./60.,1,Vec3::ZERO,Vec3::ZERO).unwrap();let good=impulse_commands(c)[0];
 let before=format!("{world:?}");for bad in [Command::AngularPush{body:99,delta:[0.;3]},Command::PointImpulse{body:0,point:[f32::NAN,0.,0.],impulse:[0.;3]},Command::AngularPush{body:0,delta:[f32::INFINITY,0.,0.]}]{assert!(world.apply_impulses(&[good,bad]).is_err());assert_eq!(format!("{world:?}"),before);}
 let huge=Command::AngularPush{body:0,delta:[f32::MAX;3]};assert!(world.apply_impulses(&[good,huge,huge]).is_err());assert_eq!(format!("{world:?}"),before);
 world.apply_impulses(&[good]).unwrap();world.advance(1./60.).unwrap();let before=format!("{world:?}");assert!(world.apply_impulses(&[good,huge,huge]).is_err());assert_eq!(format!("{world:?}"),before);world.apply_impulses(&[]).unwrap();assert_eq!(format!("{world:?}"),before);
}

fn group_impulse_commands(burst:&Value)->Vec<crate::ivp_joint_tail::Command>{use crate::ivp_joint_tail::Command;burst["commands"].as_array().unwrap().iter().map(|c|{let body=c["body"].as_u64().unwrap() as usize;if c["kind"]==0{Command::PointImpulse{body,point:v(&c["point"]).to_array(),impulse:v(&c["vector"]).to_array()}}else{Command::AngularPush{body,delta:v(&c["vector"]).to_array()}}}).collect()}
#[test]
fn native_registered_joint_impulses_preserve_shared_bodies_and_prediction_order(){
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-group-impulses.json")).unwrap())).unwrap();let mut seed=1;
 for c in data["cases"].as_array().unwrap(){let mut world=joint_world(c,seed);assert_eq!(joint_snapshot(&world),c["initial"]);world.register_joint_group(0,joint_definition(c)).unwrap();
 for(index,state)in c["states"].as_array().unwrap().iter().enumerate(){if index>0{for burst in c["impulses"].as_array().unwrap(){if burst["before_step"].as_u64().unwrap() as usize==index-1{world.apply_impulses(&group_impulse_commands(burst)).unwrap();}}if c["queue_before_steps"].as_array().unwrap().iter().any(|v|v.as_u64().unwrap() as usize==index-1){world.queue_recovery(0,Some(0),Some(world.bodies().len()-1)).unwrap();}world.advance(1./60.).unwrap();}assert_eq!(joint_snapshot(&world),*state,"joint impulse{}/step{index}",c["scenario"]);}seed=world.poll().seed;
 }
}
#[test]
fn invalid_shared_body_impulses_preserve_joint_registry_and_solver_caches(){
 use crate::ivp_joint_tail::Command;
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-group-impulses.json")).unwrap())).unwrap();let c=&data["cases"][3];let mut world=joint_world(c,1);world.register_joint_group(0,joint_definition(c)).unwrap();world.advance(1./60.).unwrap();let mut commands=group_impulse_commands(&c["impulses"][0]);let before=format!("{world:?}");commands.push(Command::PointImpulse{body:999,point:[0.;3],impulse:[0.;3]});assert!(world.apply_impulses(&commands).is_err());assert_eq!(format!("{world:?}"),before);commands.pop();let huge=Command::AngularPush{body:0,delta:[f32::MAX;3]};commands.extend([huge,huge]);assert!(world.apply_impulses(&commands).is_err());assert_eq!(format!("{world:?}"),before);
}

fn fixed_parameters(v:&Value)->crate::ivp_fixed_callback::Parameters{crate::ivp_fixed_callback::Parameters{frame:std::array::from_fn(|i|std::array::from_fn(|j|f(&v[i*4+j]))),strength:f(&v[12]),force_limit:f(&v[13]),torque_limit:f(&v[14]),mass_scale:[f(&v[15]),f(&v[16])]}}
fn fixed_geometry(j:&crate::ivp_fixed_callback::Joint)->Value{let mut v=vec![];for c in j.columns{v.extend(c.map(f32::to_bits));v.push(0);}v.extend(j.position.map(f32::to_bits));v.push(0);json!({"geometry":v,"tau":j.tau.to_bits(),"damping":j.damping.to_bits()})}
fn fixed_definition(world:&FreeWorld,c:&Value)->GroupDefinition{
 let offsets=c["properties"].as_array().unwrap().iter().enumerate().map(|(id,p)|(id,(!p["object_offset"].is_null()).then(||std::array::from_fn(|i|f(&p["object_offset"][i]))))).collect();
 let constraints=c["constructors"].as_array().unwrap().iter().map(|j|{let id=j["id"].as_u64().unwrap() as usize;let objects=std::array::from_fn(|i|j["objects"][i].as_u64().unwrap() as usize);if j["kind"]==0{let p=&j["parameters"];let def=world.fixed_definition(id,objects,fixed_parameters(&p["public"]),&offsets).unwrap();let crate::ivp_joint_mixed::Joint::Fixed(ref joint)=def.joint else{panic!("fixed factory selected another kernel")};assert_eq!(fixed_geometry(joint),json!({"geometry":p["geometry"],"tau":p["tau"],"damping":p["damping"]}));def}else{JointDefinition{id,objects,joint:if j["kind"]==3{crate::ivp_joint_mixed::Joint::Ragdoll(crate::ivp_joint_callback::tests::joint(&j["parameters"]))}else{crate::ivp_joint_mixed::Joint::Hinge(crate::ivp_hinge_callback::tests::joint(&j["parameters"]))}}}}).collect();
 GroupDefinition{constraints,offsets,additional:c["additional"].as_u64().unwrap() as u32,minimum:c["minimum"].as_i64().unwrap() as i32,tolerance:c["tolerance"].as_f64().unwrap() as f32,rescue_gravity:v(&c["rescue_gravity"])}
}
#[test]
fn native_registered_fixed_and_mixed_joint_trajectories_are_source_owned(){
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-fixed-joint.json")).unwrap())).unwrap();let mut seed=1;
 for c in data["cases"].as_array().unwrap(){let mut world=joint_world(c,seed);assert_eq!(joint_snapshot(&world),c["initial"]);let definition=fixed_definition(&world,c);world.register_joint_group(0,definition).unwrap();
 for(index,state)in c["states"].as_array().unwrap().iter().enumerate(){if index>0{if [12,28].contains(&(index-1)){world.queue_recovery(0,Some(0),Some(world.bodies().len()-1)).unwrap();}world.advance(1./60.).unwrap();}crate::ivp_fixed_callback::tests::check(&joint_snapshot(&world),state,&format!("fixed world{}/step{index}",c["scenario"]));}seed=world.poll().seed;
 }
}
#[test]
fn fixed_joint_binding_and_failed_numerical_execution_preserve_the_entire_world(){
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-fixed-joint.json")).unwrap())).unwrap();let c=&data["cases"][2];let mut world=joint_world(c,1);let d=fixed_definition(&world,c);let p=fixed_parameters(&c["constructors"][0]["parameters"]["public"]);let before=format!("{world:?}");assert!(world.fixed_definition(99,[0,0],p,&d.offsets).is_err());assert!(world.fixed_definition(99,[0,999],p,&d.offsets).is_err());assert!(world.fixed_definition(99,[1,0],crate::ivp_fixed_callback::Parameters{force_limit:1.,..p},&d.offsets).is_err());assert_eq!(format!("{world:?}"),before);world.register_joint_group(0,d).unwrap();let binding=world.joints.get_mut(&0).unwrap().solver.bindings.iter_mut().find(|b|matches!(b.joint,crate::ivp_joint_mixed::Joint::Fixed(_))).unwrap();let crate::ivp_joint_mixed::Joint::Fixed(ref mut fixed)=binding.joint else{unreachable!()};fixed.columns[0][0]=f32::NAN;let before=format!("{world:?}");assert!(world.advance(1./60.).is_err());assert_eq!(format!("{world:?}"),before);
}

fn public_ragdoll(c:&Value)->crate::ivp_ragdoll_constructor::Parameters{let a=&c["public_ragdoll"];let flags=a[44].as_u64().unwrap();crate::ivp_ragdoll_constructor::Parameters{frames:std::array::from_fn(|s|std::array::from_fn(|r|std::array::from_fn(|i|f(&a[6+s*12+r*4+i])))),axes:std::array::from_fn(|i|crate::ivp_ragdoll_constructor::Limit{min:f(&a[32+i*4]),max:f(&a[33+i*4]),velocity:f(&a[34+i*4]),torque:f(&a[35+i*4])}),force_limit:f(&a[1]),torque_limit:f(&a[2]),mass_scale:[f(&a[3]),f(&a[4])],angular_only:flags&0xff!=0,clockwise:flags&0xff0000!=0}}
fn ragdoll_parameters(j:&crate::ivp_joint_callback::Joint)->Value{let mut frames=vec![];for m in j.frames{for v in m{frames.extend(v.map(f32::to_bits));frames.push(0);}}json!({"frames":frames,"cores":j.anchors.map(|a|json!({"anchor":a.map(f32::to_bits)})),"axes":std::array::from_fn::<_,3,_>(|i|{let a=j.axes[i];json!({"enabled":u8::from(j.limit_enabled[i]),"motor":u8::from(a.motor),"min":a.min.to_bits(),"max":a.max.to_bits(),"tau":a.tau.to_bits(),"force":a.force.to_bits(),"velocity":a.velocity.to_bits(),"target":if a.motor{a.target.to_bits()}else{0}})}),"enabled":u8::from(j.linear_enabled),"damping":j.damping.to_bits(),"tau":j.tau.to_bits()})}
#[test]
fn native_public_ragdolls_construct_their_own_kernels_and_trajectories(){
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-ragdoll-constructor.json")).unwrap())).unwrap();let mut seed=1;
 for c in data["cases"].as_array().unwrap(){let mut world=joint_world(c,seed);let offsets:std::collections::BTreeMap<_,_>=c["properties"].as_array().unwrap().iter().enumerate().map(|(id,p)|(id,(!p["object_offset"].is_null()).then(||std::array::from_fn(|i|f(&p["object_offset"][i]))))).collect();let constructed=world.ragdoll_definition(0,[1,0],public_ragdoll(c),&offsets).unwrap();let joint=constructed.joint;
 let (kind,actual)=match joint{crate::ivp_joint_mixed::Joint::Fixed(_)=>panic!("ragdoll factory unexpectedly returned fixed joint"),crate::ivp_joint_mixed::Joint::Hinge(ref j)=>(1,hinge_parameters(j)),crate::ivp_joint_mixed::Joint::Ragdoll(ref j)=>(3,ragdoll_parameters(j))};

 assert_eq!(kind,c["constructors"][0]["kind"].as_i64().unwrap());assert_eq!(actual,c["constructors"][0]["parameters"],"constructor scenario{}",c["scenario"]);
 let definition=GroupDefinition{constraints:vec![constructed],offsets,additional:c["additional"].as_u64().unwrap() as u32,minimum:c["minimum"].as_i64().unwrap() as i32,tolerance:c["tolerance"].as_f64().unwrap() as f32,rescue_gravity:v(&c["rescue_gravity"])};world.register_joint_group(0,definition).unwrap();
 for (index,s) in c["states"].as_array().unwrap().iter().enumerate(){if index>0{if [12,28].contains(&(index-1)){world.queue_recovery(0,Some(0),Some(1)).unwrap();}world.advance(1./60.).unwrap();}assert_eq!(joint_snapshot(&world),*s,"trajectory scenario{}/step{index}",c["scenario"]);}seed=world.poll().seed;
 }
}

    #[test]
    fn public_ragdoll_factory_rejects_unowned_breakage_and_bad_bindings_atomically(){
        let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-ragdoll-constructor.json")).unwrap())).unwrap();let c=&data["cases"][0];let world=joint_world(c,1);let offsets=joint_definition(c).offsets;let p=public_ragdoll(c);let before=format!("{world:?}");
        for bad in [crate::ivp_ragdoll_constructor::Parameters{force_limit:1.,..p},crate::ivp_ragdoll_constructor::Parameters{mass_scale:[1.,2.],..p}]{assert!(world.ragdoll_definition(0,[1,0],bad,&offsets).is_err());assert_eq!(format!("{world:?}"),before);}
        assert!(world.ragdoll_definition(0,[0,0],p,&offsets).is_err());assert!(world.ragdoll_definition(0,[99,0],p,&offsets).is_err());assert_eq!(format!("{world:?}"),before);
    }

fn public_hinge(c:&Value)->crate::ivp_hinge_constructor::Parameters{
 let a=&c["public_hinge"];crate::ivp_hinge_constructor::Parameters{position:std::array::from_fn(|i|f(&a[i])),direction:std::array::from_fn(|i|f(&a[3+i])),limits:[f(&a[6]),f(&a[7])],velocity:f(&a[8]),torque:f(&a[9]),strength:f(&a[10]),force_limit:f(&a[11]),torque_limit:f(&a[12]),mass_scale:[f(&a[13]),f(&a[14])] }
}
fn hinge_parameters(j:&crate::ivp_hinge_callback::Joint)->Value{
 let mut geometry=vec![];for side in 0..2{geometry.extend(j.anchors[side].map(f32::to_bits));geometry.push(0);geometry.extend(j.directions[side].map(f32::to_bits));geometry.push(0);}for v in j.perpendiculars{geometry.extend(v.map(f32::to_bits));geometry.push(0);}
 json!({"geometry":geometry,"limit":u8::from(j.limit_enabled),"motor":u8::from(j.axis.motor),"axis":([j.axis.min,j.axis.max,j.axis.tau,j.axis.force,j.axis.velocity,if j.axis.motor{j.axis.target}else{0.}].map(f32::to_bits)),"tau":j.tau.to_bits(),"damping":j.damping.to_bits()})
}
#[test]
fn native_public_hinges_construct_their_own_geometry_and_trajectories(){
 let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-hinge-constructor.json")).unwrap())).unwrap();let mut seed=1;
 for c in data["cases"].as_array().unwrap(){let mut world=joint_world(c,seed);let offsets=c["properties"].as_array().unwrap().iter().enumerate().map(|(id,p)|(id,(!p["object_offset"].is_null()).then(||std::array::from_fn(|i|f(&p["object_offset"][i]))))).collect();
  let constructed=world.hinge_definition(0,[1,0],public_hinge(c),&offsets).unwrap();
  let crate::ivp_joint_mixed::Joint::Hinge(joint)=constructed.joint else{panic!("public hinge constructor selected another kernel")};

  assert_eq!(hinge_parameters(&joint),c["constructors"][0]["parameters"],"constructor scenario{}",c["scenario"]);
  let definition=GroupDefinition{constraints:vec![constructed],offsets,additional:c["additional"].as_u64().unwrap() as u32,minimum:c["minimum"].as_i64().unwrap() as i32,tolerance:c["tolerance"].as_f64().unwrap() as f32,rescue_gravity:v(&c["rescue_gravity"])};world.register_joint_group(0,definition).unwrap();
  for (index,s) in c["states"].as_array().unwrap().iter().enumerate(){if index>0{if [12,28].contains(&(index-1)){world.queue_recovery(0,Some(0),Some(1)).unwrap();}world.advance(1./60.).unwrap();}assert_eq!(joint_snapshot(&world),*s,"trajectory scenario{}/step{index}",c["scenario"]);}seed=world.poll().seed;
 }
}

    #[test]
    fn public_hinge_factory_rejects_unowned_breakable_wrappers_without_world_changes(){
        let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-hinge-constructor.json")).unwrap())).unwrap();let c=&data["cases"][0];let world=joint_world(c,1);let offsets=joint_definition(c).offsets;
        let baseline=format!("{world:?}");let p=public_hinge(c);
        for bad in [crate::ivp_hinge_constructor::Parameters{force_limit:1.,..p},crate::ivp_hinge_constructor::Parameters{torque_limit:1.,..p},crate::ivp_hinge_constructor::Parameters{mass_scale:[2.,1.],..p}]{assert!(world.hinge_definition(0,[1,0],bad,&offsets).is_err());assert_eq!(format!("{world:?}"),baseline);}
        assert!(world.hinge_definition(0,[0,0],p,&offsets).is_err());assert!(world.hinge_definition(0,[1,99],p,&offsets).is_err());assert_eq!(format!("{world:?}"),baseline);
    }

    fn insert_fixture()->Value{serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-insert.json")).unwrap())).unwrap()}
    fn inserted_definition(entry:&Value)->JointDefinition{
        let j=&entry["definition"];JointDefinition{id:j["id"].as_u64().unwrap() as usize,objects:std::array::from_fn(|i|j["objects"][i].as_u64().unwrap() as usize),joint:if j["kind"]==3{crate::ivp_joint_mixed::Joint::Ragdoll(crate::ivp_joint_callback::tests::joint(&j["parameters"]))}else{crate::ivp_joint_mixed::Joint::Hinge(crate::ivp_hinge_callback::tests::joint(&j["parameters"]))}}
    }
    #[test]
    fn native_live_joint_insertion_owns_unit_merges_and_growing_numerical_groups(){
        let data=insert_fixture();let mut seed=1;let(mut states,mut steps,mut inserted,mut predictions)=(0,0,0,0);
        for c in data["cases"].as_array().unwrap(){let mut world=joint_world(c,seed);assert_eq!(joint_snapshot(&world),c["initial"]);states+=1;
            let definition=joint_definition(c);let offsets=definition.offsets.clone();world.register_joint_group(0,definition).unwrap();
            for (index,s) in c["states"].as_array().unwrap().iter().enumerate(){
                if index>0{let step=index-1;
                    for entry in c["insertions"].as_array().unwrap(){if entry["before_step"].as_u64().unwrap() as usize==step{world.insert_joint(0,inserted_definition(entry),&offsets).unwrap();inserted+=1;}}
                    if c["queue_before_steps"].as_array().unwrap().iter().any(|n|n.as_u64().unwrap() as usize==step){assert!(world.queue_recovery(0,Some(c["queue_objects"][0].as_u64().unwrap() as usize),Some(c["queue_objects"][1].as_u64().unwrap() as usize)).unwrap());}
                    world.advance(1./60.).unwrap();steps+=1;
                }
                assert_eq!(joint_snapshot(&world),*s,"scenario{}/step{index}",c["scenario"]);states+=1;predictions+=world.prediction_calls().len();
            }seed=world.poll.seed;
        }assert_eq!((states,steps,inserted,predictions),(408,384,20,1056));
    }
    #[test]
    fn invalid_live_joint_insertion_cannot_commit_registry_or_physical_prefixes(){
        let data=insert_fixture();let c=&data["cases"][0];let mut world=joint_world(c,1);let def=joint_definition(c);let mut offsets=def.offsets.clone();world.register_joint_group(0,def).unwrap();world.advance(1./60.).unwrap();
        let new=inserted_definition(&c["insertions"][0]);
        for bad in [JointDefinition{objects:[0,999],..new},JointDefinition{objects:[0,0],..new}]{let before=format!("{world:?}");assert!(world.insert_joint(0,bad,&offsets).is_err());assert_eq!(format!("{world:?}"),before);}
        offsets.remove(&new.objects[0]);let before=format!("{world:?}");assert!(world.insert_joint(0,new,&offsets).is_err());assert_eq!(format!("{world:?}"),before);
    }
    fn remove_fixture()->Value{serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-remove.json")).unwrap())).unwrap()}
    #[test]
    fn native_live_joint_destruction_retains_membership_and_empty_controllers(){
        let data=remove_fixture();let mut seed=1;let(mut states,mut steps,mut removed,mut predictions)=(0,0,0,0);
        for c in data["cases"].as_array().unwrap(){let mut world=joint_world(c,seed);assert_eq!(joint_snapshot(&world),c["initial"]);states+=1;
            world.register_joint_group(0,joint_definition(c)).unwrap();let count=c["constructors"].as_array().unwrap().len();
            for (index,s) in c["states"].as_array().unwrap().iter().enumerate(){
                if index>0{let step=index-1;
                    if step==8||(step==20&&count>1){world.destroy_joint(0,if step==8{0}else{count-1}).unwrap();removed+=1;}
                    if c["queue_before_steps"].as_array().unwrap().iter().any(|n|n.as_u64().unwrap() as usize==step){assert!(world.queue_recovery(0,Some(0),Some(world.bodies.len()-1)).unwrap());}
                    world.advance(1./60.).unwrap();steps+=1;
                }
                assert_eq!(joint_snapshot(&world),*s,"scenario{}/step{index}",c["scenario"]);states+=1;predictions+=world.prediction_calls().len();
            }seed=world.poll.seed;
        }assert_eq!((states,steps,removed,predictions),(408,384,21,891));
    }
    #[test]
    fn invalid_or_repeated_joint_destruction_preserves_the_entire_world(){
        let data=remove_fixture();let c=&data["cases"][0];let mut world=joint_world(c,1);world.register_joint_group(0,joint_definition(c)).unwrap();
        for (group,joint) in [(99,0),(0,99)]{let before=format!("{world:?}");assert!(world.destroy_joint(group,joint).is_err());assert_eq!(format!("{world:?}"),before);}
        world.destroy_joint(0,0).unwrap();let before=format!("{world:?}");assert!(world.destroy_joint(0,0).is_err());assert_eq!(format!("{world:?}"),before);
        assert!(world.joints[&0].solver.bindings.is_empty());assert!(world.groups.group(0).unwrap().active());
    }
    fn fixed_fixture()->Value{serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-fixed.json")).unwrap())).unwrap()}
    #[test]
    fn native_fixed_endpoint_ragdoll_and_hinge_trajectories_preserve_static_bodies(){
        let data=fixed_fixture();let mut seed=1;let(mut states,mut steps,mut body_states,mut fixed_rows)=(0,0,0,0);
        for c in data["cases"].as_array().unwrap(){let mut world=joint_world(c,seed);assert_eq!(joint_snapshot(&world),c["initial"]);states+=1;body_states+=world.bodies.len();
            world.register_joint_group(0,joint_definition(c)).unwrap();
            for (index,s) in c["states"].as_array().unwrap().iter().enumerate(){
                if index>0{if c["queue_before_steps"].as_array().unwrap().iter().any(|n|n.as_u64().unwrap() as usize==index-1){assert!(world.queue_recovery(0,Some(0),Some(world.bodies.len()-1)).unwrap());}world.advance(1./60.).unwrap();steps+=1;}
                assert_eq!(joint_snapshot(&world),*s,"scenario{}/step{index}",c["scenario"]);
                for (&id,b) in &world.bodies{if b.flags&2!=0{assert!(!world.prediction_calls.contains(&id));assert_eq!(b.current.position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|i|c["initial"]["physical"][id]["position"][i].as_u64().unwrap() as u32));fixed_rows+=1;}}
                states+=1;body_states+=world.bodies.len();
            }seed=world.poll.seed;
        }assert_eq!((states,steps,body_states,fixed_rows),(408,384,1428,495));
    }
    #[test]
    fn incoherent_static_bindings_are_rejected_before_registering_a_world(){
        let data=fixed_fixture();let c=&data["cases"][0];let b=crate::ivp_joint_apply::tests::body(&c["initial"]["physical"][0]);assert!(b.flags&2!=0);
        let p=&c["properties"][0];let properties=Properties{radius:f(&p["radius"]),sleep_time:f(&p["sleep_time"]),linear_rate:f(&p["linear_rate"]),angular_rate:v(&p["angular_rate"]),inertia:v(&p["inertia"]),limits:Vec2::new(f(&p["limits"][0]),f(&p["limits"][1])),angular_limit_exact:0.95199776f32 as f64*(1f32/60.).recip() as f64,gyro:p["gyro"].as_bool().unwrap()};
        let h=history(&c["initial"]["histories"][0]);assert!(FreeWorld::new(99,&[(Object{id:0,core:0,fixed:false},b,h,properties)],1./60.,1).is_err());
        let mut bad=b;bad.object_flags=(bad.object_flags&!0xff)|8;assert!(FreeWorld::new(99,&[(Object{id:0,core:0,fixed:true},bad,h,properties)],1./60.,1).is_err());
    }

    fn contact_definition(world:&FreeWorld)->crate::ivp_registered_contacts::Definition{
        use crate::{ivp_managed_response::{Object as ContactObject,Settings},ivp_motion::ObjectFrame};
        let d:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-graph-pipeline.json")).unwrap();let c=&d["cases"][0];let k=&c["settings"];let value=|v:&Value|v.as_f64().unwrap() as f32;let admission=crate::ivp_managed_contacts::Settings{distance:value(&k["distance"]),area_epsilon:k["area_epsilon"].as_f64().unwrap()};let work=&c["work"];let w=&work["settings"];
        crate::ivp_registered_contacts::Definition{objects:world.bodies.iter().map(|(&id,b)|{let r=b.current.rotation.transpose();(id as u64+1,ContactObject{core:id,rotation:[r.x_axis,r.y_axis,r.z_axis],translation:b.current.position,radius:0.,material_flags:0,friction:0.8})}).collect(),frames:world.bodies.keys().map(|&id|(id as u64+1,ObjectFrame{shift:None,rotation:None})).collect(),settings:Settings{dt:world.dt,admission,refresh:crate::ivp_refresh::Settings{epsilon:value(&k["epsilon"]),bary_bias:value(&k["bary_bias"]),area_epsilon:admission.area_epsilon,one:value(&k["one"]),opposite:value(&k["opposite"])},threshold_extra:value(&k["threshold_extra"]),minimum_impulse:value(&k["minimum_impulse"]),minimum_determinant_squared:value(&k["minimum_determinant_squared"])},decay:value(&work["coefficient"]),initial_work:value(&work["initial"]),blocked:vec![false;world.bodies.len()],dissipation:crate::ivp_dissipation::Settings{epsilon:value(&w["epsilon"]),one:value(&w["one"]),opposite:value(&w["opposite"]),two:value(&w["two"]),half:value(&w["half"]),huge:value(&w["huge"]),small:value(&w["small"]),fraction:value(&w["fraction"])},allowance:value(&c["allowance_factor"]),energy_bound:value(&c["energy_bound"])}
    }
    fn contact_joint_snapshot(world:&crate::ivp_registered_contacts::ContactWorld)->Value{
        let group=world.joint_group(0).map(|g|{let m=world.group(0).unwrap();json!({"members":g.solver.tail.members,"order":m.order().entries().iter().map(|e|e.id).collect::<Vec<_>>(),"dirty":u8::from(m.order().dirty()),"active":u8::from(m.active()),"ticks":g.solver.tail.ticks,"minimum":g.solver.tail.minimum,"tolerance":g.solver.tail.tolerance.to_bits(),"maximum":g.maximum.map(f32::to_bits)})});
        json!({"physical":world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>(),"histories":world.histories().values().copied().map(snapshot_history).collect::<Vec<_>>(),"unit_flags":world.bodies().iter().map(|b|world.registry().unit_flags(b.id).unwrap().flags&0x3fff).collect::<Vec<_>>(),"clock":{"now":world.clock().now.to_bits(),"next":world.clock().deadline.to_bits()},"poll":{"remaining":world.poll().remaining,"seed":world.poll().seed},"group":group,"prediction_order":world.prediction_calls()})
    }
    #[test]
    fn native_joint_chains_continue_after_ownership_moves_to_contact_world(){
        let datasets=[joint_fixture(),fixed_fixture()];let(mut states,mut steps,mut requests,mut fixed_rows)=(0,0,0,0);
        for data in &datasets{let mut seed=1;for c in data["cases"].as_array().unwrap(){let mut source=joint_world(c,seed);source.register_joint_group(0,joint_definition(c)).unwrap();let definition=contact_definition(&source);let before=joint_snapshot(&source);let mut world=source.into_contacts(definition).unwrap();assert_eq!(contact_joint_snapshot(&world),before);
            for (index,state) in c["states"].as_array().unwrap().iter().enumerate(){if index>0{if c["queue_before_steps"].as_array().unwrap().iter().any(|n|n.as_u64()==Some(index as u64-1)){assert!(world.queue_recovery(0,Some(0),Some(world.bodies().len()-1)).unwrap());requests+=1;}world.advance(1./60.).unwrap();steps+=1;}assert_eq!(contact_joint_snapshot(&world),*state,"contact-owner joint scenario{} step{index}",c["scenario"]);assert!(world.contacts().system_ids().is_empty());fixed_rows+=world.bodies().iter().filter(|b|b.flags&2!=0).count();states+=1;}assert!(world.take_releases().0.is_empty());seed=world.poll().seed;
        }}assert_eq!((states,steps,requests),(792,768,48));assert_eq!(fixed_rows,495);println!("transferred contact-capable owner:24 native joint worlds /792 states /768 steps /48 recovery requests /495 fixed-body rows match");
    }

    #[test]
    fn contact_and_joint_callbacks_execute_against_one_moving_physical_bank(){
        use crate::ivp_create::{Endpoint,Feature as CompactFeature};use crate::ivp_managed_response::Feature;
        let data=joint_fixture();let c=&data["cases"][0];let mut source=joint_world(c,1);source.register_joint_group(0,joint_definition(c)).unwrap();let mut baseline=source.clone();let mut definition=contact_definition(&source);
        let separation=(source.bodies[&1].current.position-source.bodies[&0].current.position).length();for object in definition.objects.values_mut(){object.radius=(separation+0.01)*0.5;}
        let mut world=source.into_contacts(definition).unwrap();let features=std::array::from_fn(|i|Feature{material_code:0,endpoint:Endpoint{object:i as u64+1,feature:CompactFeature{kind:3,edge:0,opposite:0,triangle:0,ledge:0,vertex:0,vertices:[Vec3::ZERO;3]}},vertices:[Vec3::ZERO;3],canonical:[Vec3::ZERO;3]});let id=world.resolve(0xc0000,features,Some(0)).unwrap().unwrap();assert_eq!(world.contacts().system_ids().len(),1);
        let mut response_seen=false;for _ in 0..8{world.advance(1./60.).unwrap();baseline.advance(1./60.).unwrap();let calls=world.controller_calls();for phase in [crate::ivp_unit_registry::ContactPhase::Refresh,crate::ivp_unit_registry::ContactPhase::Tangent,crate::ivp_unit_registry::ContactPhase::Normal]{assert!(calls.iter().any(|k|matches!(k,ControllerKey::Contact{phase:p,..} if *p==phase)));}assert!(calls.contains(&ControllerKey::Base)&&calls.contains(&ControllerKey::Joint(0)));let refresh=calls.iter().position(|k|matches!(k,ControllerKey::Contact{phase:crate::ivp_unit_registry::ContactPhase::Refresh,..})).unwrap();let base=calls.iter().position(|k|*k==ControllerKey::Base).unwrap();let tangent=calls.iter().position(|k|matches!(k,ControllerKey::Contact{phase:crate::ivp_unit_registry::ContactPhase::Tangent,..})).unwrap();let joint=calls.iter().position(|k|*k==ControllerKey::Joint(0)).unwrap();let normal=calls.iter().position(|k|matches!(k,ControllerKey::Contact{phase:crate::ivp_unit_registry::ContactPhase::Normal,..})).unwrap();assert!(refresh<base&&base<tangent&&tangent<joint&&joint<normal);let state=world.contacts().snapshot(id);assert!(state["alive"].as_bool().unwrap());response_seen|=state["state"]["force"].as_f64().unwrap()!=0.;for b in world.bodies(){assert!(b.current.position.is_finite()&&b.prediction.start.is_finite()&&b.linear[..3].iter().chain(&b.angular[..3]).all(|v|v.is_finite()));}}
        assert!(response_seen);assert_ne!(world.bodies()[0].linear.map(f32::to_bits),baseline.bodies[&0].linear.map(f32::to_bits),"contact kernels had no physical effect");crate::ivp_registered_contacts::verify_rollback_controls(&world);
    }

    #[test]
    fn contact_world_keeps_retirement_requests_until_resource_owner_takes_them(){
        use crate::ivp_create::{Endpoint,Feature as CompactFeature};use crate::ivp_managed_response::Feature;
        let data=joint_fixture();let c=&data["cases"][0];let mut source=joint_world(c,1);source.register_joint_group(0,joint_definition(c)).unwrap();let definition=contact_definition(&source);let mut world=source.into_contacts(definition).unwrap();
        let features=std::array::from_fn(|i|Feature{material_code:0,endpoint:Endpoint{object:i as u64+1,feature:CompactFeature{kind:3,edge:0,opposite:0,triangle:0,ledge:0,vertex:0,vertices:[Vec3::ZERO;3]}},vertices:[Vec3::ZERO;3],canonical:[Vec3::ZERO;3]});let id=world.resolve(0xc0000,features,Some(0)).unwrap().unwrap();world.advance(1./60.).unwrap();assert!(world.contacts().system_ids().is_empty());let (ids,features)=world.take_releases();assert_eq!(ids,vec![id as usize]);assert_eq!(features.len(),1);assert_eq!(features[0].each_ref().map(|e|e.object),[1,2]);assert!(world.take_releases().0.is_empty());assert!(world.take_releases().1.is_empty());
    }

    fn native_joint_contact_snapshot(world:&crate::ivp_registered_contacts::ContactWorld,pairs:u64)->Value{
        let mut out=contact_joint_snapshot(world);out.as_object_mut().unwrap().remove("prediction_order");let bits=|v:&Value|(v.as_f64().unwrap() as f32).to_bits();let contacts=(1..=pairs).map(|id|{let contact=world.contacts().snapshot(id);if contact["alive"].as_bool().unwrap(){let state=&contact["state"];json!({"time":state["time"].as_f64().unwrap().to_bits(),"mix":state["mix"].as_array().unwrap().iter().map(bits).collect::<Vec<_>>(),"marker":state["marker"],"counter":state["counter"],"force":bits(&state["force"]),"sign":state["sign"],"energy":bits(&state["energy"]),"work":bits(&state["work"]),"bound":bits(&state["bound"]),"friction":bits(&state["friction"]),"mode":state["mode"],"distance":bits(&contact["geometry"]["distance"])})}else{Value::Null}}).collect::<Vec<_>>();out["contact"]=if pairs==1{contacts[0].clone()}else{json!(contacts)};out["environment_work"]=json!(bits(&world.contacts().world_snapshot()["environment_work"]));out
    }
    fn native_joint_contact_definition(world:&FreeWorld,c:&Value)->crate::ivp_registered_contacts::Definition{
        let mut definition=contact_definition(world);let k=&c["contact_settings"];let v=|v:&Value|v.as_f64().unwrap() as f32;definition.settings.admission=crate::ivp_managed_contacts::Settings{distance:v(&k["distance"]),area_epsilon:k["area_epsilon"].as_f64().unwrap()};definition.settings.refresh=crate::ivp_refresh::Settings{epsilon:v(&k["epsilon"]),bary_bias:v(&k["bary_bias"]),area_epsilon:definition.settings.admission.area_epsilon,one:v(&k["one"]),opposite:v(&k["opposite"])};definition.settings.threshold_extra=v(&k["threshold_extra"]);definition.settings.minimum_impulse=v(&k["minimum_impulse"]);definition.settings.minimum_determinant_squared=v(&k["minimum_determinant_squared"]);let w=&c["work"]["settings"];definition.dissipation=crate::ivp_dissipation::Settings{epsilon:v(&w["epsilon"]),one:v(&w["one"]),opposite:v(&w["opposite"]),two:v(&w["two"]),half:v(&w["half"]),huge:v(&w["huge"]),small:v(&w["small"]),fraction:v(&w["fraction"])};definition.decay=v(&c["work"]["coefficient"]);definition.initial_work=v(&c["work"]["initial"]);definition.allowance=v(&c["allowance_factor"]);definition.energy_bound=v(&c["energy_bound"]);
        for (i,o) in c["object_inputs"].as_array().unwrap().iter().enumerate(){let id=i as u64+1;let object=definition.objects.get_mut(&id).unwrap();object.radius=f(&o["radius"]);object.friction=f(&o["friction"]);object.material_flags=o["material_flags"].as_u64().unwrap() as u32;definition.blocked[i]=o["blocked"].as_bool().unwrap();definition.frames.insert(id,crate::ivp_motion::ObjectFrame{shift:(!o["shift"].is_null()).then(||super::tests::v(&o["shift"])),rotation:(!o["rotation"].is_null()).then(||std::array::from_fn(|j|f(&o["rotation"][j])))});}definition
    }
    fn compare_joint_contact(a:&Value,b:&Value,path:&str){match(a,b){(Value::Object(a),Value::Object(b))=>{assert_eq!(a.len(),b.len(),"{path}");for (k,v) in a{compare_joint_contact(v,&b[k],&format!("{path}.{k}"));}},(Value::Array(a),Value::Array(b))=>{assert_eq!(a.len(),b.len(),"{path}");for (i,(a,b)) in a.iter().zip(b).enumerate(){compare_joint_contact(a,b,&format!("{path}[{i}]"));}},_=>assert_eq!(a,b,"{path}")}}
    fn late_sphere_parameters(p:&Value)->crate::ivp_sphere_constructor::Parameters{
        let n=|key:&str|p[key].as_f64().unwrap() as f32;crate::ivp_sphere_constructor::Parameters{radius:n("radius"),position:std::array::from_fn(|i|p["position"][i].as_f64().unwrap() as f32),angles:std::array::from_fn(|i|p["angles"][i].as_f64().unwrap() as f32),mass:n("mass"),inertia_scale:n("inertia_scale"),damping:n("damping"),angular_damping:n("angular_damping"),rotational_limit:n("rotational_limit"),fixed:p["fixed"].as_bool().unwrap(),collisions:p["collisions"].as_bool().unwrap()}
    }
    fn late_poly_parameters(p:&Value)->crate::ivp_poly_constructor::Parameters{
        let n=|key:&str|p[key].as_f64().unwrap() as f32;crate::ivp_poly_constructor::Parameters{surface:crate::ivp_poly_constructor::Surface{center:v(&p["shape_center"]),unit_inertia:v(&p["shape_inertia"]),radius:f(&p["shape_radius"]),factor:p["shape_factor"].as_u64().unwrap() as u8},margin:f(&p["shape_margin"]),center:(!p["center"].is_null()).then(||std::array::from_fn(|i|p["center"][i].as_f64().unwrap() as f32)),position:std::array::from_fn(|i|p["position"][i].as_f64().unwrap() as f32),angles:std::array::from_fn(|i|p["angles"][i].as_f64().unwrap() as f32),mass:n("mass"),inertia_scale:n("inertia_scale"),damping:n("damping"),angular_damping:n("angular_damping"),rotational_limit:n("rotational_limit"),fixed:p["fixed"].as_bool().unwrap(),collisions:p["collisions"].as_bool().unwrap()}
    }
    // Expected snapshots are comparison outputs only, never continuation inputs.
    fn replay_native_joint_contact(data:&Value,check:bool)->Vec<Value>{
        use crate::ivp_create::{Endpoint,Feature as CompactFeature};use crate::ivp_managed_response::Feature;
        let mut seed=1;let mut actual=Vec::new();let count=data["authored_steps"].as_u64().unwrap();let admissions=data["authored_pairs"].as_u64().unwrap_or(1);let pairs=data["snapshot_pairs"].as_u64().unwrap_or(admissions);
        for c in data["cases"].as_array().unwrap(){
            let mut source=joint_world(c,seed);if c["defer_group"]!=true{source.register_joint_group(0,joint_definition(c)).unwrap();}
            if c["detach_before_admission"]==true{source.detach_joint_group(0).unwrap();}let definition=native_joint_contact_definition(&source,c);let mut world=if data["runtime_defaults"]==true{crate::ivp_registered_contacts::ContactWorld::empty_native(1./60.,seed,v(&c["gravity"]),v(&c["alternate"])).unwrap()}else{source.into_contacts(definition).unwrap()};
            let before=native_joint_contact_snapshot(&world,pairs);
            if check{compare_joint_contact(&before,&c["before_contact"],&format!("joint/contact scenario{} before admission",c["scenario"]));}actual.push(before);
            for pair in 0..admissions{let features=std::array::from_fn(|i|Feature{material_code:0,endpoint:Endpoint{object:pair+i as u64+1,feature:CompactFeature{kind:3,edge:0,opposite:0,triangle:0,ledge:0,vertex:0,vertices:[Vec3::ZERO;3]}},vertices:[Vec3::ZERO;3],canonical:[Vec3::ZERO;3]});assert_eq!(world.resolve(0xc0000,features,Some(0)).unwrap(),Some(pair+1));}
            for index in 0..=count{
                if index>0{
                    if let Some(births)=c["births"].as_array(){for entry in births{if entry["before_step"].as_u64()==Some(index-1){let object_id=entry["object_id"].as_u64().unwrap();let friction=entry["friction"].as_f64().unwrap() as f32;let id=if entry["kind"]=="poly"{world.create_poly(object_id,late_poly_parameters(&entry["public"]),friction).unwrap()}else{world.create_sphere(object_id,late_sphere_parameters(&entry["public"]),friction).unwrap()};assert_eq!(id as u64,entry["id"].as_u64().unwrap());
                        if check{compare_joint_contact(&crate::ivp_joint_apply::tests::snapshot(world.bodies()[id]),&entry["physical"],"late sphere constructor physical");compare_joint_contact(&snapshot_history(world.histories()[&id]),&entry["history"],"late sphere constructor history");if entry["kind"]=="poly"{compare_joint_contact(&json!(world.object_offsets()[&id].map(|v|v.map(f32::to_bits))),&entry["object_offset"],"late poly object offset");}}
                        let vector=|v:&Value|Vec3::from_array(std::array::from_fn(|i|v[i].as_f64().unwrap() as f32));world.set_velocity(id,Some(vector(&entry["velocity"]["linear"])),Some(vector(&entry["velocity"]["angular"]))).unwrap();
                    }}}
                    if let Some(admissions)=c["admissions"].as_array(){for entry in admissions{if entry["before_step"].as_u64()==Some(index-1){let features=std::array::from_fn(|i|Feature{material_code:0,endpoint:Endpoint{object:entry["objects"][i].as_u64().unwrap(),feature:CompactFeature{kind:3,edge:0,opposite:0,triangle:0,ledge:0,vertex:0,vertices:[Vec3::ZERO;3]}},vertices:[Vec3::ZERO;3],canonical:[Vec3::ZERO;3]});assert_eq!(world.resolve(entry["flags"].as_u64().unwrap() as u32,features,Some(entry["hint"].as_u64().unwrap() as usize)).unwrap(),entry["id"].as_u64());}}}
                    if c["create_group_before_step"].as_u64()==Some(index-1){let mut definition=joint_definition(c);definition.constraints.clear();world.register_joint_group(0,definition).unwrap();}
                    if let Some(insertions)=c["insertions"].as_array(){for entry in insertions{if entry["before_step"].as_u64()==Some(index-1){
                        let mut offsets=joint_definition(c).offsets;offsets.extend(world.object_offsets().iter().map(|(&id,&offset)|(id,offset)));
                        let created=world.ragdoll_definition(entry["id"].as_u64().unwrap() as usize,std::array::from_fn(|i|entry["objects"][i].as_u64().unwrap() as usize),public_ragdoll(entry),&offsets).unwrap();
                        if check{let(kind,parameters)=match created.joint{crate::ivp_joint_mixed::Joint::Fixed(_)=>panic!("ragdoll factory returned fixed"),crate::ivp_joint_mixed::Joint::Hinge(ref j)=>(1,hinge_parameters(j)),crate::ivp_joint_mixed::Joint::Ragdoll(ref j)=>(3,ragdoll_parameters(j))};compare_joint_contact(&json!(kind),&entry["definition"]["kind"],"inserted constructor kind");compare_joint_contact(&parameters,&entry["definition"]["parameters"],"inserted constructor parameters");}
                        world.insert_joint(0,created,&offsets).unwrap();
                    }}}
                    if c["activate_group_before_step"].as_u64()==Some(index-1){world.activate_joint_group(0).unwrap();}
                    if let Some(edits)=c["destroy_detached_before_steps"].as_array(){for edit in edits{if edit["before_step"].as_u64()==Some(index-1){world.destroy_detached_joint(edit["constraint"].as_u64().unwrap() as usize).unwrap();}}}
                    if let Some(edits)=c["destroy_before_steps"].as_array(){for edit in edits{if edit["before_step"].as_u64()==Some(index-1){world.destroy_joint(0,edit["constraint"].as_u64().unwrap() as usize).unwrap();}}}
                    if c["detach_before_step"].as_u64()==Some(index-1){world.detach_joint_group(0).unwrap();}
                    if let Some(bursts)=c["impulses"].as_array(){for burst in bursts{if burst["before_step"].as_u64()==Some(index-1){world.apply_impulses(&group_impulse_commands(burst)).unwrap();}}}
                    if let Some(commands)=c["velocities"].as_array(){for command in commands{if command["before_step"].as_u64()==Some(index-1){world.set_velocity(command["body"].as_u64().unwrap() as usize,(!command["linear"].is_null()).then(||v(&command["linear"])),(!command["angular"].is_null()).then(||v(&command["angular"]))).unwrap();}}}
                    if c["queue_before_steps"].as_array().unwrap().iter().any(|v|v.as_u64()==Some(index-1)){let ends=if c["queue_objects"].is_array(){[c["queue_objects"][0].as_u64().unwrap() as usize,c["queue_objects"][1].as_u64().unwrap() as usize]}else{[0,world.bodies().len()-1]};world.queue_recovery(0,Some(ends[0]),Some(ends[1])).unwrap();}world.advance(1./60.).unwrap();}
                let state=native_joint_contact_snapshot(&world,pairs);if check{compare_joint_contact(&state,&c["states"][index as usize],&format!("joint/contact scenario{} step{index}",c["scenario"]));}actual.push(state);
            }seed=world.poll().seed;
        }actual
    }
    #[test]
    fn native_initialized_joint_and_contact_trajectories_match_shared_owner(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-joint-contact.json")).unwrap();
        assert_eq!(data["authored_steps"],24);assert_eq!(data["cases"].as_array().unwrap().len(),8);
        let actual=replay_native_joint_contact(&data,true);assert_eq!(actual.len(),208);
        let mut corrupted=data.clone();for c in corrupted["cases"].as_array_mut().unwrap(){c["before_contact"]=Value::Null;c["states"]=Value::Null;}
        assert_eq!(actual,replay_native_joint_contact(&corrupted,false),"expected native outputs influenced execution");
        println!("native initialized joint/contact trajectories:8 worlds /200 states /192 complete steps match; 208 snapshots unchanged with expected outputs removed");
    }

    #[test]
    fn native_joint_contact_chains_continue_through_contact_retirement(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-joint-contact-chain.json")).unwrap();
        assert_eq!(data["authored_pairs"],2);assert_eq!(data["authored_steps"],24);
        let actual=replay_native_joint_contact(&data,true);assert_eq!(actual.len(),208);
        let mut corrupted=data.clone();for c in corrupted["cases"].as_array_mut().unwrap(){c["before_contact"]=Value::Null;c["states"]=Value::Null;}
        assert_eq!(actual,replay_native_joint_contact(&corrupted,false));
    }

    #[test]
    fn native_contact_joint_world_commands_continue_exact_trajectories(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-contact-commands.json")).unwrap();
        let actual=replay_native_joint_contact(&data,true);assert_eq!(actual.len(),208);
        let mut corrupted=data.clone();for c in corrupted["cases"].as_array_mut().unwrap(){c["before_contact"]=Value::Null;c["states"]=Value::Null;}
        assert_eq!(actual,replay_native_joint_contact(&corrupted,false));
    }
    #[test]
    fn transferred_contact_owner_retains_native_impulse_sleep_and_wake_trajectories(){
        let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-group-impulses.json")).unwrap())).unwrap();let mut seed=1;let mut sleeping_commands=0;
        for c in data["cases"].as_array().unwrap(){let mut source=joint_world(c,seed);source.register_joint_group(0,joint_definition(c)).unwrap();let definition=contact_definition(&source);let mut world=source.into_contacts(definition).unwrap();
            for(index,state)in c["states"].as_array().unwrap().iter().enumerate(){if index>0{for burst in c["impulses"].as_array().unwrap(){if burst["before_step"].as_u64()==Some(index as u64-1){sleeping_commands+=world.bodies().iter().filter(|b|b.object_flags&0xff==8).count();world.apply_impulses(&group_impulse_commands(burst)).unwrap();}}if c["queue_before_steps"].as_array().unwrap().iter().any(|v|v.as_u64()==Some(index as u64-1)){world.queue_recovery(0,Some(0),Some(world.bodies().len()-1)).unwrap();}world.advance(1./60.).unwrap();}compare_joint_contact(&contact_joint_snapshot(&world),state,&format!("transferred impulses scenario{} step{index}",c["scenario"]));}seed=world.poll().seed;
        }assert!(sleeping_commands>0,"dormant impulse path was not exercised");
    }
    #[test]
    fn transferred_contact_owner_preserves_native_sleep_wake_and_resleep(){
        let data=sleep_fixture();let mut seed=1;let(mut steps,mut sleeps,mut wakes)=(0,0,0);
        for c in data["cases"].as_array().unwrap(){let states=c["states"].as_array().unwrap();let initial=&states[0];let p=&c["properties"];
            let properties=Properties{radius:f(&p["radius"]),sleep_time:f(&p["sleep_time"]),linear_rate:f(&p["linear_rate"]),angular_rate:v(&p["angular_rate"]),inertia:v(&p["inertia"]),limits:Vec2::new(f(&p["limits"][0]),f(&p["limits"][1])),angular_limit_exact:0.95199776f32 as f64*(1f32/60.).recip() as f64,gyro:p["gyro"].as_bool().unwrap()};
            let body=crate::ivp_joint_apply::tests::body(&initial["physical"]);let binding=(Object{id:0,core:0,fixed:false},body,history(&initial["history"]),properties);
            let mut source=FreeWorld::new(99,&[binding],1./60.,seed).unwrap();source.wake(0).unwrap();let definition=contact_definition(&source);let mut world=source.into_contacts(definition).unwrap();
            for (index,s) in states.iter().enumerate(){if index>0{
                    if index-1==c["wake_before_step"].as_u64().unwrap() as usize{assert_eq!(world.bodies()[0].flags>>8,8);world.wake(0).unwrap();wakes+=1;}
                    let was_asleep=world.bodies()[0].flags>>8>=8;world.advance(1./60.).unwrap();sleeps+=usize::from(!was_asleep&&world.bodies()[0].flags>>8>=8);steps+=1;
                }
                assert_eq!(crate::ivp_joint_apply::tests::snapshot(world.bodies()[0]),s["physical"],"scenario{}/step{index}/physical",c["scenario"]);
                assert_eq!(snapshot_history(world.histories()[&0]),s["history"],"scenario{}/step{index}/history",c["scenario"]);
                assert_eq!(json!({"now":world.clock().now.to_bits(),"next":world.clock().deadline.to_bits()}),s["clock"]);
                assert_eq!(json!({"remaining":world.poll().remaining,"seed":world.poll().seed}),s["poll"]);
                assert_eq!(world.registry().unit_flags(0).unwrap().flags&0x3fff,s["unit_flags"].as_u64().unwrap() as u32);
            }seed=world.poll().seed;
        }assert_eq!((steps,sleeps,wakes),(3072,32,16));
    }

    #[test]
    fn native_contact_world_joint_deletion_and_detachment_preserve_continuation(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-contact-teardown.json")).unwrap();
        let actual=replay_native_joint_contact(&data,true);assert_eq!(actual.len(),208);
        let mut corrupted=data.clone();for c in corrupted["cases"].as_array_mut().unwrap(){c["before_contact"]=Value::Null;c["states"]=Value::Null;}
        assert_eq!(actual,replay_native_joint_contact(&corrupted,false));
    }

    #[test]
    fn native_contact_world_live_public_joint_insertion_constructs_its_own_kernel(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-contact-insert.json")).unwrap();
        let actual=replay_native_joint_contact(&data,true);assert_eq!(actual.len(),208);
        let mut corrupted=data.clone();for c in corrupted["cases"].as_array_mut().unwrap(){c["before_contact"]=Value::Null;c["states"]=Value::Null;for entry in c["insertions"].as_array_mut().unwrap(){entry["definition"]=Value::Null;}}
        assert_eq!(actual,replay_native_joint_contact(&corrupted,false));
    }
    #[test]
    fn transferred_contact_owner_matches_existing_fixed_and_offset_joint_insertions(){
        let data=insert_fixture();let mut seed=1;let(mut states,mut steps,mut inserted,mut predictions)=(0,0,0,0);
        for c in data["cases"].as_array().unwrap(){let mut source=joint_world(c,seed);assert_eq!(joint_snapshot(&source),c["initial"]);states+=1;
            let definition=joint_definition(c);let offsets=definition.offsets.clone();source.register_joint_group(0,definition).unwrap();let contacts=contact_definition(&source);let mut world=source.into_contacts(contacts).unwrap();
            for (index,s) in c["states"].as_array().unwrap().iter().enumerate(){
                if index>0{let step=index-1;
                    for entry in c["insertions"].as_array().unwrap(){if entry["before_step"].as_u64().unwrap() as usize==step{world.insert_joint(0,inserted_definition(entry),&offsets).unwrap();inserted+=1;}}
                    if c["queue_before_steps"].as_array().unwrap().iter().any(|n|n.as_u64().unwrap() as usize==step){assert!(world.queue_recovery(0,Some(c["queue_objects"][0].as_u64().unwrap() as usize),Some(c["queue_objects"][1].as_u64().unwrap() as usize)).unwrap());}
                    world.advance(1./60.).unwrap();steps+=1;
                }
                assert_eq!(contact_joint_snapshot(&world),*s,"scenario{}/step{index}",c["scenario"]);states+=1;predictions+=world.prediction_calls().len();
            }seed=world.poll().seed;
        }assert_eq!((states,steps,inserted,predictions),(408,384,20,1056));
    }

    #[test]
    fn native_contact_world_creates_empty_groups_and_activates_live_public_joints(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-contact-groups.json")).unwrap();
        let actual=replay_native_joint_contact(&data,true);assert_eq!(actual.len(),208);
        let mut corrupted=data.clone();for c in corrupted["cases"].as_array_mut().unwrap(){c["before_contact"]=Value::Null;c["states"]=Value::Null;for entry in c["insertions"].as_array_mut().unwrap(){entry["definition"]=Value::Null;}}
        assert_eq!(actual,replay_native_joint_contact(&corrupted,false));
    }
    fn contact_overlap_snapshot(world:&crate::ivp_registered_contacts::ContactWorld)->Value{
        let mut value=contact_joint_snapshot(world);value.as_object_mut().unwrap().remove("group");value["groups"]=json!((0..2).map(|id|world.joint_group(id).map(|g|{let m=world.group(id).unwrap();json!({"members":g.solver.tail.members,"order":m.order().entries().iter().map(|e|e.id).collect::<Vec<_>>(),"dirty":u8::from(m.order().dirty()),"active":u8::from(m.active()),"ticks":g.solver.tail.ticks,"minimum":g.solver.tail.minimum,"tolerance":g.solver.tail.tolerance.to_bits(),"maximum":g.maximum.map(f32::to_bits)})})).collect::<Vec<_>>());value
    }
    #[test]
    fn transferred_contact_owner_registers_overlapping_groups_without_losing_shared_state(){
        let data=overlap_fixture();let mut seed=1;let(mut steps,mut states,mut detaches,mut requests)=(0,0,0,0);
        for c in data["cases"].as_array().unwrap(){let source=joint_world(c,seed);assert_eq!(overlap_snapshot(&source),c["initial"]);states+=1;let definition=contact_definition(&source);let mut world=source.into_contacts(definition).unwrap();
            for id in 0..2{world.register_joint_group(id,overlap_definition(c,id)).unwrap();}
            for (index,s) in c["states"].as_array().unwrap().iter().enumerate(){if index>0{
                    let step=index-1;
                    for req in c["requests"].as_array().unwrap().iter().filter(|r|r["step"].as_u64()==Some(step as u64)){assert!(world.queue_recovery(req["group"].as_u64().unwrap() as usize,Some(req["objects"][0].as_u64().unwrap() as usize),Some(req["objects"][1].as_u64().unwrap() as usize)).unwrap());requests+=1;}
                    for (id,at) in c["detach_before_steps"].as_array().unwrap().iter().enumerate(){if at.as_u64()==Some(step as u64){world.detach_joint_group(id).unwrap();detaches+=1;}}
                    world.advance(1./60.).unwrap();steps+=1;
                }
                assert_eq!(contact_overlap_snapshot(&world),*s,"scenario{}/step{index}",c["scenario"]);states+=1;
            }seed=world.poll().seed;
        }assert_eq!((steps,states,detaches,requests),(384,400,16,24));
    }

    #[test]
    fn native_contact_world_destroys_detached_wrappers_before_and_after_transfer(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-contact-orphans.json")).unwrap();
        let actual=replay_native_joint_contact(&data,true);assert_eq!(actual.len(),208);
        let mut corrupted=data.clone();for c in corrupted["cases"].as_array_mut().unwrap(){c["before_contact"]=Value::Null;c["states"]=Value::Null;}
        assert_eq!(actual,replay_native_joint_contact(&corrupted,false));
    }
    #[test]
    fn detached_wrapper_ownership_survives_transfer_and_deletes_in_both_owners(){
        let data=joint_fixture();let c=&data["cases"][3];let mut free=joint_world(c,1);free.register_joint_group(0,joint_definition(c)).unwrap();for _ in 0..8{free.advance(1./60.).unwrap();}free.detach_joint_group(0).unwrap();
        let definition=contact_definition(&free);let mut shared=free.clone().into_contacts(definition).unwrap();let ids=free.detached_constraints.keys().copied().collect::<Vec<_>>();assert!(!ids.is_empty());
        let mut bad=free.clone();let ends=bad.detached_constraints[&ids[0]];bad.object_joint_listeners.get_mut(&ends[1]).unwrap().remove(&ids[0]);let before=format!("{bad:?}");assert!(bad.destroy_detached_joint(ids[0]).is_err());assert_eq!(format!("{bad:?}"),before);
        for id in ids{free.destroy_detached_joint(id).unwrap();shared.destroy_detached_joint(id).unwrap();assert_eq!(joint_snapshot(&free),contact_joint_snapshot(&shared));let before=format!("{free:?}");assert!(free.destroy_detached_joint(id).is_err());assert_eq!(format!("{free:?}"),before);free.advance(1./60.).unwrap();shared.advance(1./60.).unwrap();assert_eq!(joint_snapshot(&free),contact_joint_snapshot(&shared));}
        assert!(free.retired_constraints.is_empty()&&free.detached_constraints.is_empty());
    }

    #[test]
    fn native_contact_world_creates_late_dynamic_and_fixed_spheres_without_seeding_bodies(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-contact-birth.json")).unwrap();
        let actual=replay_native_joint_contact(&data,true);assert_eq!(actual.len(),208);
        let mut corrupted=data.clone();for c in corrupted["cases"].as_array_mut().unwrap(){c["before_contact"]=Value::Null;c["states"]=Value::Null;for entry in c["births"].as_array_mut().unwrap(){entry["physical"]=Value::Null;entry["history"]=Value::Null;}for entry in c["insertions"].as_array_mut().unwrap(){entry["definition"]=Value::Null;}}
        assert_eq!(actual,replay_native_joint_contact(&corrupted,false));
    }

    #[test]
    fn native_empty_world_builds_bodies_contacts_and_joints_without_imported_body_or_joint_state(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-empty-world.json")).unwrap();
        assert!(data["cases"].as_array().unwrap().iter().all(|c|c["initial"]["physical"].as_array().unwrap().is_empty()));
        let actual=replay_native_joint_contact(&data,true);assert_eq!(actual.len(),208);
        let mut corrupted=data.clone();for c in corrupted["cases"].as_array_mut().unwrap(){c["before_contact"]=Value::Null;c["states"]=Value::Null;for entry in c["births"].as_array_mut().unwrap(){entry["physical"]=Value::Null;entry["history"]=Value::Null;}for entry in c["insertions"].as_array_mut().unwrap(){entry["definition"]=Value::Null;}}
        assert_eq!(actual,replay_native_joint_contact(&corrupted,false));
    }

    #[test]
    fn native_late_poly_creation_derives_centers_inertia_and_joint_motion_from_shapes(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-contact-poly.json")).unwrap();
        let actual=replay_native_joint_contact(&data,true);assert_eq!(actual.len(),208);
        let mut corrupted=data.clone();for c in corrupted["cases"].as_array_mut().unwrap(){c["before_contact"]=Value::Null;c["states"]=Value::Null;for entry in c["births"].as_array_mut().unwrap(){entry["physical"]=Value::Null;entry["history"]=Value::Null;entry["object_offset"]=Value::Null;}for entry in c["insertions"].as_array_mut().unwrap(){entry["definition"]=Value::Null;}}
        assert_eq!(actual,replay_native_joint_contact(&corrupted,false));
    }

    #[test]fn runtime_default_factory_matches_native_empty_world_trajectories(){let mut data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-registered-empty-world.json")).unwrap();data["runtime_defaults"]=json!(true);assert_eq!(replay_native_joint_contact(&data,true).len(),208);}

}
