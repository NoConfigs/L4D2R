//! Event clock -> registered units/controllers -> movement polling/graph split.
//! The caller executes physical controllers and updates their movement inputs;
//! this owner does not invent numerical dynamics for a missing controller.
use crate::{ivp_event_world::World,ivp_group_effects::Operation,
    ivp_group_members::Object,ivp_movement::{self as movement,History,Input,Poll,Settings},
    ivp_registered_groups::Groups,ivp_unit_registry::{ControllerKey,Effect}};
use std::{cell::RefCell,collections::BTreeMap};
#[derive(Clone,Copy,Debug)]
pub(crate) struct Binding {pub object:Object,pub history:History,pub input:Input}
#[derive(Clone,Debug)]
pub(crate) struct Call {pub key:ControllerKey,pub cores:Vec<usize>,pub dt:f32}
#[derive(Clone,Debug)]
pub(crate) struct Schedule {
    groups:Groups,clock:World,poll:Poll,dt:f32,
    histories:BTreeMap<usize,History>,inputs:BTreeMap<usize,Input>,
}
fn settings()->Settings {Settings{translation_squared:0.0001,rotation_squared:0.000025,
    far_translation_squared:0.1f32*0.1,far_rotation_squared:0.2f32*0.2,far_timeout:4.,
    one:1.,two:2.,angular_factor0:std::f32::consts::PI,angular_factor1:0.75}}
fn clock_effects(histories:&mut BTreeMap<usize,History>,effects:Vec<Effect>,now:f64)->Result<(),&'static str> {
    for effect in effects {match effect {
        Effect::ResetClock(id)|Effect::Activated(id)=>histories.get_mut(&id).ok_or("missing registered movement history")?.reset_clock(now)?,
        Effect::Slept(_)=>{},Effect::ForcedSleep(_)=>return Err("forced sleep requires caller eligibility and geometry"),
    }}Ok(())
}
impl Schedule {
    pub(crate) fn new(anchor:usize,bindings:&[Binding],dt:f32,seed:u32)->Result<Self,&'static str> {
        let objects=bindings.iter().map(|b|b.object).collect::<Vec<_>>();let groups=Groups::new(anchor,&objects)?;
        let mut clock=World::new(1000)?;clock.set_timestep(dt)?;let mut histories=BTreeMap::new();let mut inputs=BTreeMap::new();
        for b in bindings {let mut probe=b.history;movement::classify(&mut probe,b.input,clock.now,&settings())?;
            histories.insert(b.object.core,b.history);inputs.insert(b.object.core,b.input);}
        Ok(Self{groups,clock,poll:Poll{remaining:10,seed},dt,histories,inputs})
    }
    pub(crate) fn groups(&self)->&Groups {&self.groups}
    pub(crate) fn clock(&self)->&World {&self.clock}
    pub(crate) fn poll(&self)->Poll {self.poll}
    pub(crate) fn edit(&mut self,operation:Operation)->Result<(),&'static str> {
        let mut next=self.clone();match operation {
            Operation::Create(id)=>next.groups.create(id)?,Operation::Activate(id)=>next.groups.activate(id)?,
            Operation::Remove(id)=>next.groups.remove(id)?,Operation::Insert(id,c)=>next.groups.insert(id,c)?,
        }
        clock_effects(&mut next.histories,next.groups.take_effects(),next.clock.now)?;*self=next;Ok(())
    }
    pub(crate) fn advance(&mut self,delta:f32,
        mut execute:impl FnMut(ControllerKey,&[usize],f32,&mut[Input])->Result<(),&'static str>)->Result<Vec<Call>,&'static str> {
        let mut next=self.clone();let dt=next.dt;let k=settings();let mut calls=vec![];
        let histories=RefCell::new(std::mem::take(&mut next.histories));let inputs=RefCell::new(std::mem::take(&mut next.inputs));
        let (groups,poll)=(&mut next.groups,&mut next.poll);
        next.clock.simulate(delta,|_,now,_|{
            groups.psi_with(poll,
                |effects|clock_effects(&mut histories.borrow_mut(),effects,now),
                |key,cores|{
                    let mut values={let frames=inputs.borrow();cores.iter().map(|id|frames.get(id).copied().ok_or("missing controller movement input")).collect::<Result<Vec<_>,_>>()?};
                    execute(key,cores,dt,&mut values)?;
                    for (&id,input) in cores.iter().zip(values) {
                        let mut probe=*histories.borrow().get(&id).ok_or("missing controller movement history")?;
                        movement::classify(&mut probe,input,now,&k)?;inputs.borrow_mut().insert(id,input);
                    }
                    calls.push(Call{key,cores:cores.to_vec(),dt});Ok(())
                },|id|{
                    let input=*inputs.borrow().get(&id).ok_or("missing polled movement input")?;
                    movement::classify(histories.borrow_mut().get_mut(&id).ok_or("missing polled movement history")?,input,now,&k)
                })
        },|_,_,_|Err("registered schedule has no supplied pair-event dispatcher"),|_,_|{})?;
        next.histories=histories.into_inner();next.inputs=inputs.into_inner();*self=next;Ok(calls)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ivp_constraint_order::Entry,ivp_group_members::Constraint};
    use bevy::prelude::Vec3;
    use serde_json::{Value,json};
    fn fixed(s:usize,id:usize)->bool {match s%4{1=>id==0||id==6,2=>id%2==1,3=>id%2==0,_=>false}}
    fn binding(id:usize,fixed:bool)->Binding {Binding{object:Object{id,core:id,fixed},
        history:History{close_time:0.,far_time:0.,close_position:Vec3::ZERO,far_position:Vec3::ZERO,close_quat:[0.;4],far_quat:[0.;4]},
        input:Input{radius:4.*0.0254,sleep_time:0.3,angular:Vec3::ZERO,position:Vec3::new(id as f32*128.*0.0254,-128.*0.0254,0.),start:[0.,0.,0.,1.],end:[0.,0.,0.,1.]}}}
    fn members(m:Option<&crate::ivp_group_members::Members>)->Value {m.map_or(Value::Null,|m|json!({"active":u8::from(m.active()),"dirty":u8::from(m.order().dirty()),
        "order":m.order().entries().iter().map(|e|e.id).collect::<Vec<_>>(),"objects":m.objects(),"cores":m.cores()}))}
    #[test]
    fn native_registered_clock_dispatch_poll_sleep_and_delayed_group_splitting_are_source_owned() {
        let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-shared-group-split.json")).unwrap())).unwrap();
        let mut seed=1;let mut states=0;let mut steps=0;let mut dispatches=0;
        for case in data["cases"].as_array().unwrap() {
            let s=case["scenario"].as_u64().unwrap() as usize;let bindings=(0..7).map(|id|binding(id,fixed(s,id))).collect::<Vec<_>>();
            let mut owner=Schedule::new(7,&bindings,1./60.,seed).unwrap();for id in 0..2{owner.edit(Operation::Create(id)).unwrap();}
            for (index,op) in case["states"].as_array().unwrap().iter().enumerate() {
                let group=op["group"].as_i64().unwrap();let mut calls=vec![];
                match op["action"].as_str().unwrap() {
                    "created"=>{},"add"=>owner.edit(Operation::Insert(group as usize,Constraint{entry:Entry{id:op["joint"].as_u64().unwrap() as usize,class:3},objects:std::array::from_fn(|i|op["objects"][i].as_u64().unwrap() as usize)})).unwrap(),
                    "activate"=>owner.edit(Operation::Activate(group as usize)).unwrap(),"destroy_group"=>owner.edit(Operation::Remove(group as usize)).unwrap(),
                    "step"=>{calls=owner.advance(1./60.,|key,_,_,_|{assert_eq!(key,ControllerKey::Base);Ok(())}).unwrap();steps+=1;},_=>panic!("unknown authored action"),
                }
                assert_eq!(owner.groups().registry().snapshot(),op["registry"],"scenario{s}/action{index}/registry");
                assert_eq!(owner.groups().registry().controller_snapshot(),op["controllers"],"scenario{s}/action{index}/controllers");
                assert_eq!(json!([members(owner.groups().group(0)),members(owner.groups().group(1))]),op["members"]);
                assert_eq!(json!({"remaining":owner.poll().remaining,"seed":owner.poll().seed}),op["poll"],"scenario{s}/action{index}/poll");
                assert_eq!(json!({"now":owner.clock().now.to_bits(),"next":owner.clock().deadline.to_bits()}),op["clock"],"scenario{s}/action{index}/clock");
                let actual=calls.iter().map(|c|json!({"kind":match c.key{ControllerKey::Base=>0,ControllerKey::Joint(id)=>id+1,ControllerKey::Contact{..}=>panic!("legacy schedule fixture contains contact controllers")},"dt_bits":c.dt.to_bits(),"cores":c.cores})).collect::<Vec<_>>();
                assert_eq!(json!(actual),op["calls"],"scenario{s}/action{index}/calls");dispatches+=calls.len();states+=1;
            }
            assert!(owner.groups().registry().active_reset_cores().is_empty());seed=owner.poll().seed;
        }
        assert_eq!((states,steps,dispatches),(7040,6144,14148));
    }
    #[test]
    fn failed_controller_execution_rolls_back_clock_registry_poll_and_history() {
        let mut owner=Schedule::new(7,&[binding(0,false),binding(1,false)],1./60.,1).unwrap();
        owner.edit(Operation::Create(0)).unwrap();owner.edit(Operation::Insert(0,Constraint{entry:Entry{id:3,class:3},objects:[0,1]})).unwrap();owner.edit(Operation::Activate(0)).unwrap();
        owner.edit(Operation::Insert(0,Constraint{entry:Entry{id:4,class:3},objects:[0,1]})).unwrap();owner.edit(Operation::Remove(0)).unwrap();
        let before=format!("{owner:?}");assert!(owner.advance(1./60.,|_,_,_,_|Err("controlled execution failure")).is_err());assert_eq!(format!("{owner:?}"),before);
    }
}
