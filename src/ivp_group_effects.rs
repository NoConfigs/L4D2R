//! Shared-group edits -> registered effects -> represented core/predictor fields.
//! Contact, object-cache/hull and activation notification side effects remain
//! external; this owner does not stand in for the full 101073c0 world path.
use crate::{ivp_group_members::{Object,Constraint},ivp_joint_apply::Body,
    ivp_registered_groups::Groups,ivp_unit_registry::Effect};
use std::collections::BTreeMap;
#[derive(Clone,Copy,Debug)]
pub(crate) struct Context {pub now:f64,pub next:f64}
impl Context {
    pub(crate) fn inverse_dt(self)->Result<f32,&'static str> {
        if !self.now.is_finite()||!self.next.is_finite(){return Err("invalid activation clock context");}
        let delta=self.next-self.now;
        Ok(if delta>f32::from_bits(0x2edbe6ff) as f64 {1./(delta as f32)} else {f32::from_bits(0x501502f9)})
    }
}
fn bindings(bodies:&[Body],objects:&[Object])->Result<BTreeMap<usize,usize>,&'static str> {
    let mut ids=BTreeMap::new();for (i,b) in bodies.iter().enumerate(){if ids.insert(b.id,i).is_some(){return Err("duplicate physical object identity");}}
    let mut cores=BTreeMap::new();let mut bound_objects=std::collections::BTreeSet::new();
    for object in objects {
        if !bound_objects.insert(object.id){return Err("duplicate physical object binding");}
        let &i=ids.get(&object.id).ok_or("missing physical object binding")?;
        if cores.insert(object.core,i).is_some(){return Err("compound physical core effects require shared ownership");}
        if (bodies[i].flags&2!=0)!=object.fixed{return Err("incoherent physical fixed-core binding");}
    }
    if cores.len()!=bodies.len(){return Err("unbound physical object");}Ok(cores)
}
pub(crate) fn apply(bodies:&mut[Body],objects:&[Object],effects:&[Effect],context:Context)->Result<(),&'static str> {
    apply_inner(bodies,objects,effects,context,None)
}
/// Scheduled sleep uses the environment timestep, not the remaining activation
/// interval. Hull/contact/object notifications remain the containing caller's.
pub(crate) fn apply_scheduled(bodies:&mut[Body],objects:&[Object],effects:&[Effect],context:Context,dt:f32)->Result<(),&'static str>{
    if !dt.is_finite()||dt<=0.||!dt.recip().is_finite(){return Err("invalid scheduled sleep timestep");}
    apply_inner(bodies,objects,effects,context,Some(dt.recip()))
}
fn apply_inner(bodies:&mut[Body],objects:&[Object],effects:&[Effect],context:Context,sleep_inverse:Option<f32>)->Result<(),&'static str> {
    let inverse_dt=context.inverse_dt()?;let cores=bindings(bodies,objects)?;
    for effect in effects {
        let id=match effect {Effect::ResetClock(id)|Effect::Activated(id)=>id,Effect::Slept(id) if sleep_inverse.is_some()=>id,_=>return Err("sleep effects require physical sleep dispatch")};
        if !cores.contains_key(id){return Err("missing physical effect core");}
    }
    let mut next=bodies.to_vec();for b in &mut next {b.clock=context.now;}
    for effect in effects {
        match *effect {
            Effect::ResetClock(id)=>next[cores[&id]].stamps=[context.now;2],
            Effect::Activated(id)=>{
                let b=&mut next[cores[&id]];
                if b.flags&2!=0||b.flags>>8<8{return Err("activation effect requires a dormant dynamic core");}
                let preserve_velocity=b.prediction.time==0.;
                b.flags=(b.flags&0xff)|0x100;b.object_flags=(b.object_flags&0xffffff00)|1;
                b.stamps=[context.now;2];b.prediction.time=context.now;b.prediction.inverse_dt=inverse_dt;
                b.prediction.rotation_end=b.prediction.rotation_start;b.prediction.velocity=bevy::prelude::Vec3::ZERO;
                if !preserve_velocity {b.linear[..3].fill(0.);}
            },
            Effect::Slept(id)=>{
                let b=&mut next[cores[&id]];
                if b.flags&2!=0||b.flags>>8>=8{return Err("sleep effect requires an awake dynamic core");}
                b.flags=(b.flags&0xff)|0x800;b.object_flags=(b.object_flags&0xffffff00)|8;
                for vector in [&mut b.linear,&mut b.angular,&mut b.pending_linear,&mut b.pending_angular]{vector[..3].fill(0.);}
                b.prediction.velocity=bevy::prelude::Vec3::ZERO;
                b.prediction.rotation_end=b.prediction.rotation_start;
                b.prediction.inverse_dt=sleep_inverse.ok_or("sleep effect lacks environment timestep")?;
            },_=>unreachable!(),
        }
    }
    bodies.copy_from_slice(&next);Ok(())
}
#[derive(Clone,Copy,Debug)]
pub(crate) enum Operation {Create(usize),Insert(usize,Constraint),Activate(usize),Remove(usize)}
#[derive(Clone,Debug)]
pub(crate) struct PhysicalGroups {groups:Groups,bodies:Vec<Body>,objects:Vec<Object>}
impl PhysicalGroups {
    pub(crate) fn groups(&self)->&Groups {&self.groups}
    pub(crate) fn bodies(&self)->&[Body] {&self.bodies}
    pub(crate) fn new(anchor:usize,objects:&[Object],bodies:&[Body])->Result<Self,&'static str> {
        bindings(bodies,objects)?;
        Ok(Self{groups:Groups::new(anchor,objects)?,bodies:bodies.to_vec(),objects:objects.to_vec()})
    }
    pub(crate) fn transition(&mut self,operation:Operation,context:Context)->Result<Vec<Effect>,&'static str> {
        context.inverse_dt()?;let mut next=self.clone();
        match operation {
            Operation::Create(id)=>next.groups.create(id)?,
            Operation::Activate(id)=>next.groups.activate(id)?,
            Operation::Remove(id)=>next.groups.remove(id)?,
            Operation::Insert(id,constraint)=>{
                next.groups.insert(id,constraint)?;
                //10106fe0 marks an object when its first listener vector is
                // registered. Further registrations preserve the existing bit.
                // Removing a group does not destroy its public joint wrappers.
                for object in constraint.objects {next.bodies.iter_mut().find(|b|b.id==object).ok_or("missing listener physical binding")?.object_flags|=0x1000;}
            },
        }
        let effects=next.groups.take_effects();apply(&mut next.bodies,&next.objects,&effects,context)?;
        *self=next;Ok(effects)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ivp_constraint_order::Entry,ivp_joint_apply::tests::{body,snapshot}};
    use serde_json::{Value,json};
    fn data()->Value {serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-shared-group-effects.json")).unwrap())).unwrap()}
    fn d(v:&Value)->f64 {f64::from_bits(v.as_u64().unwrap())}
    fn context(v:&Value)->Context {Context{now:d(&v["now"]),next:d(&v["next"])}}
    fn fixed(s:usize,id:usize)->bool {match s%4{1=>id==0||id==6,2=>id%2==1,3=>id%2==0,_=>false}}
    fn members(m:Option<&crate::ivp_group_members::Members>)->Value {m.map_or(Value::Null,|m|json!({"active":u8::from(m.active()),"dirty":u8::from(m.order().dirty()),
        "order":m.order().entries().iter().map(|e|e.id).collect::<Vec<_>>(),"objects":m.objects(),"cores":m.cores()}))}
    #[test]
    fn native_registered_group_effects_own_prediction_clocks_flags_and_all_represented_core_fields() {
        let data=data();let mut states=0;let mut effects=0;let mut physical=0;let mut intervals=std::collections::BTreeSet::new();
        for case in data["cases"].as_array().unwrap() {
            let s=case["scenario"].as_u64().unwrap() as usize;let first=&case["states"][0];let c=context(&first["context"]);
            let objects=(0..7).map(|id|Object{id,core:id,fixed:fixed(s,id)}).collect::<Vec<_>>();
            let bodies=first["physical"].as_array().unwrap().iter().map(body).collect::<Vec<_>>();
            let mut world=PhysicalGroups::new(7,&objects,&bodies).unwrap();
            for id in 0..2 {world.transition(Operation::Create(id),c).unwrap();}
            for op in case["states"].as_array().unwrap() {
                let c=context(&op["context"]);let group=op["group"].as_i64().unwrap();
                let action=match op["action"].as_str().unwrap() {
                    "created"=>None,"add"=>Some(Operation::Insert(group as usize,Constraint{entry:Entry{id:op["joint"].as_u64().unwrap() as usize,class:3},
                        objects:std::array::from_fn(|i|op["objects"][i].as_u64().unwrap() as usize)})),
                    "activate"=>Some(Operation::Activate(group as usize)),"destroy_group"=>Some(Operation::Remove(group as usize)),_=>panic!("unknown authored action"),
                };
                let generated=action.map_or_else(Vec::new,|action|world.transition(action,c).unwrap());
                let actual=generated.iter().map(|e|match e {Effect::ResetClock(id)=>[0,*id],Effect::Activated(id)=>{intervals.insert(c.inverse_dt().unwrap().to_bits());[1,*id]},_=>panic!("unexpected physical group effect")}).collect::<Vec<_>>();
                assert_eq!(json!(actual),op["effects"],"group{s}/state{states}/effects");effects+=actual.len();
                assert_eq!(world.groups.registry().snapshot(),op["registry"],"group{s}/state{states}/registry");
                assert_eq!(world.groups.registry().controller_snapshot(),op["controllers"],"group{s}/state{states}/controllers");
                assert_eq!(json!([members(world.groups.group(0)),members(world.groups.group(1))]),op["members"]);
                for (b,expected) in world.bodies.iter().zip(op["physical"].as_array().unwrap()) {assert_eq!(snapshot(*b),*expected,"group{s}/state{states}/physical{}",b.id);physical+=1;}
                states+=1;
            }
        }
        assert_eq!((states,effects,physical,intervals.len()),(896,608,6272,4));
    }
    #[test]
    fn invalid_late_physical_effect_binding_or_clock_rolls_back_group_and_physical_state() {
        let data=data();let input=&data["cases"][0]["states"][0];let c=context(&input["context"]);
        let bodies=input["physical"].as_array().unwrap().iter().map(body).collect::<Vec<_>>();let objects=(0..7).map(|id|Object{id,core:id,fixed:false}).collect::<Vec<_>>();
        let mut world=PhysicalGroups::new(7,&objects,&bodies).unwrap();world.transition(Operation::Create(0),c).unwrap();let before=format!("{world:?}");
        let bad=Operation::Insert(0,Constraint{entry:Entry{id:3,class:3},objects:[0,99]});
        assert!(world.transition(bad,c).is_err());assert_eq!(format!("{world:?}"),before);
        assert!(world.transition(Operation::Activate(0),Context{now:f64::NAN,next:0.}).is_err());assert_eq!(format!("{world:?}"),before);
        let mut physical=bodies.clone();assert!(apply(&mut physical,&objects,&[Effect::ResetClock(0),Effect::Activated(99)],c).is_err());
        let mut duplicate=objects.clone();duplicate[1].id=0;
        assert!(apply(&mut physical,&duplicate,&[Effect::ResetClock(1)],c).is_err());
        assert_eq!(physical.into_iter().map(snapshot).collect::<Vec<_>>(),bodies.into_iter().map(snapshot).collect::<Vec<_>>());
    }
    #[test]
    fn native_retained_cache_activation_preserves_pending_motion_and_raw_padding_on_both_branches() {
        let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-retained-activation.json")).unwrap())).unwrap();
        let mut total=0;let mut zero=0;let mut prior=0;let mut pending=0;let mut authored=0;let mut preserved=0;let mut cleared=0;
        for case in data["cases"].as_array().unwrap() {
            let c=context(&case["context"]);let raw=case["input_kind"]=="authored_raw_fields";
            for p in case["controls"].as_array().unwrap() {
                // Constructor/public setup and labelled low-level fields are
                // controlled inputs here; no native after-state feeds apply.
                let before=body(&p["before"]);assert_eq!(snapshot(before),p["before"]);
                zero+=usize::from(before.prediction.time==0.);prior+=usize::from(before.prediction.time!=0.);
                pending+=usize::from(before.pending_linear[..3].iter().any(|&v|v!=0.));
                let mut bodies=[before];let objects=[Object{id:before.id,core:before.id,fixed:false}];
                apply(&mut bodies,&objects,&[Effect::Activated(before.id)],c).unwrap();
                assert_eq!(snapshot(bodies[0]),p["after"],"case{}/body{}",case["scenario"],before.id);
                assert_eq!(bodies[0].pending_linear.map(f32::to_bits),before.pending_linear.map(f32::to_bits));
                assert_eq!(bodies[0].pending_angular.map(f32::to_bits),before.pending_angular.map(f32::to_bits));
                if raw {
                    authored+=1;
                    assert_eq!(bodies[0].linear[3].to_bits(),before.linear[3].to_bits());
                    assert_eq!(bodies[0].angular.map(f32::to_bits),before.angular.map(f32::to_bits));
                    if before.prediction.time==0. {assert_eq!(bodies[0].linear.map(f32::to_bits),before.linear.map(f32::to_bits));preserved+=1;}
                    else {assert_eq!(bodies[0].linear[..3],[0.,0.,0.]);cleared+=1;}
                }
                total+=1;
            }
        }
        assert_eq!((total,zero,prior,pending,authored,preserved,cleared),(896,448,448,448,448,224,224));
    }
    #[test]
    fn scheduled_sleep_rejects_invalid_later_body_without_committing_a_sleep_prefix(){
        let data:Value=serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"),"/tools/fixtures/native-ivp-registered-sleep.json")).unwrap())).unwrap();
        let native=data["cases"][0]["states"].as_array().unwrap().iter().find(|s|s["physical"]["flags"].as_u64().unwrap()>>8<8).unwrap();
        let first=body(&native["physical"]);let mut second=first;second.id=1;second.flags=(second.flags&0xff)|0x800;second.object_flags=(second.object_flags&0xffffff00)|8;
        let original=[first,second];let mut bodies=original;let objects=[Object{id:0,core:0,fixed:false},Object{id:1,core:1,fixed:false}];
        let c=Context{now:first.clock,next:first.clock+1./60.};
        assert!(apply_scheduled(&mut bodies,&objects,&[Effect::Slept(0),Effect::Slept(1)],c,1./60.).is_err());
        assert_eq!(bodies.map(snapshot),original.map(snapshot));
        assert!(apply_scheduled(&mut bodies,&objects,&[Effect::Slept(0)],c,0.).is_err());assert_eq!(bodies.map(snapshot),original.map(snapshot));
    }

}
