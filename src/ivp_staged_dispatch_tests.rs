//! Staged native controller traversal, plus source-owned structural edits
//! during dispatch. Mutation controls are regressions, not retail trajectories.
use crate::ivp_unit_registry::{Registry,ControllerKey,Effect};
use std::collections::{BTreeMap,BTreeSet};
use serde_json::{Value,json};
    fn position(id:usize)->bevy::prelude::Vec3{use bevy::prelude::Vec3;match id{0=>Vec3::new(0.,-128.*0.0254,0.),1=>Vec3::new(128.*0.0254,-128.*0.0254,0.),4..=7=>Vec3::new((512.+(id-4) as f32*128.)*0.0254,-128.*0.0254,0.),_=>Vec3::ZERO}}
    fn initial_history()->crate::ivp_movement::History{use bevy::prelude::Vec3;crate::ivp_movement::History{close_time:0.,far_time:0.,close_position:Vec3::ZERO,far_position:Vec3::ZERO,close_quat:[0.;4],far_quat:[0.;4]}}
    fn effects(r:&mut Registry,history:&mut BTreeMap<usize,crate::ivp_movement::History>,now:f64){
        for effect in r.take_effects(){match effect{
            Effect::ResetClock(id)|Effect::Activated(id)=>history.get_mut(&id).unwrap().reset_clock(now).unwrap(),
            Effect::ForcedSleep(id)=>{let h=history.get_mut(&id).unwrap();h.close_time=now-20.;h.close_position=position(id);h.close_quat=[0.,0.,0.,1.];},
            Effect::Slept(id)=>{assert!(history.contains_key(&id));},
        }}
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
                "created"=>{},"reset"=>{reset(&r,&mut world,index);},"wake"=>r.wake(id as usize).unwrap(),"sleep"=>r.commit_single_core_sleep(id as usize).unwrap(),
                "step"=>{world.simulate(1./60.,|_,now,_|{
                    r.begin_psi()?;effects(&mut r,&mut history,now);
                    while let Some(dispatch)=r.next_unit_dispatch(&mut poll,|_,_|Ok(()))?{
                        for (key,cores) in dispatch.controllers(){execute(index,*key,cores);}
                        r.finish_unit_dispatch(dispatch,|core|movement::classify(history.get_mut(&core).unwrap(),Input{radius:4.*0.0254,sleep_time:0.3,angular:Vec3::ZERO,position:position(core),start:[0.,0.,0.,1.],end:[0.,0.,0.,1.]},now,&settings))?;
                    }
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

pub(crate) fn verify(){
 let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-controller-dispatch.json")).unwrap();
 let run=|data:&Value|{let mut calls=vec![];let snapshots=replay_with_execution(data,|_,w,_|{w.reset_time().unwrap();},|_,_|{},|index,key,cores|{let kind=match key{ControllerKey::Base=>0,ControllerKey::Joint(id)=>id+1,_=>panic!("legacy native control has no contacts")};calls.push(json!({"action":index,"kind":kind,"dt_bits":(1f32/60.).to_bits(),"cores":cores}));});(snapshots,calls)};
 let actual=run(&data);assert_eq!(actual.1,*data["calls"].as_array().unwrap());for (i,row) in data["states"].as_array().unwrap().iter().enumerate(){assert_eq!(actual.0[i],row["registry"],"staged native dispatch action{i}");}
 let mut bad=data.clone();bad["calls"]=Value::Null;for row in bad["states"].as_array_mut().unwrap(){row["registry"]=Value::Null;row["controllers"]=Value::Null;}assert_eq!(actual,run(&bad));
 println!("staged native unit dispatch:{} registry checkpoints /{} exact controller calls; expected-output corruption control passes",actual.0.len(),actual.1.len());
 mutations();errors();
}
fn groups(r:&Registry)->BTreeSet<Vec<usize>>{let s=r.snapshot();s["active"].as_array().unwrap().iter().map(|u|{let mut ids=u["cores"].as_array().unwrap().iter().map(|c|c.as_u64().unwrap() as usize).collect::<Vec<_>>();ids.sort();ids}).collect()}
fn mutations(){
 use crate::{ivp_contact_system::Contacts,ivp_unit_registry::ContactPhase,ivp_movement::Poll};
 let mut r=Registry::new(4);for i in 0..4{r.register_core(i,i==0).unwrap();}r.attach_ragdoll(0,2,3).unwrap();for i in 1..4{r.wake(i).unwrap();}
 let mut owner=Contacts::empty(r,(0..4).map(|i|(i,i==0)).collect()).unwrap();for (id,pair) in [(1,[1,0]),(2,[2,0]),(3,[3,0]),(4,[1,2])]{owner.admit(id,pair,None).unwrap();}
 let bridge_system=owner.binding(4).unwrap().system;let mut poll=Poll{remaining:10,seed:1};owner.begin_psi().unwrap();let dispatch=owner.next_unit_dispatch(&mut poll,|_,_|Ok(())).unwrap().unwrap();assert_eq!(dispatch.cores().len(),3);let initial=dispatch.controllers().to_vec();let mut split_ids=vec![];
 for (key,_) in dispatch.controllers(){if *key==(ControllerKey::Contact{system:bridge_system,phase:ContactPhase::Normal}){owner.remove(4).unwrap();split_ids=owner.repair_system(bridge_system).unwrap();}}
 assert_eq!(split_ids.len(),1);assert!(!initial.iter().any(|(k,_)|matches!(k,ControllerKey::Contact{system,..} if split_ids.contains(system))));
 owner.finish_unit_dispatch(dispatch,|_|panic!("poll should not run yet")).unwrap();assert!(owner.next_unit_dispatch(&mut poll,|_,_|Ok(())).unwrap().is_none());assert_eq!(groups(owner.registry()),BTreeSet::from([vec![1],vec![2,3]]));
 owner.begin_psi().unwrap();let mut count=0;let mut normals=vec![];while let Some(work)=owner.next_unit_dispatch(&mut poll,|_,_|Ok(())).unwrap(){count+=1;for (key,_)in work.controllers(){if let ControllerKey::Contact{system,phase:ContactPhase::Normal}=key{normals.push(*system);}}owner.finish_unit_dispatch(work,|_|panic!("poll should not run yet")).unwrap();}assert_eq!(count,2);assert_eq!(normals.len(),3);assert!(split_ids.iter().all(|id|normals.contains(id)));
 owner.destroy_joint_registration(0).unwrap();owner.begin_psi().unwrap();count=0;while let Some(work)=owner.next_unit_dispatch(&mut poll,|_,_|Ok(())).unwrap(){count+=1;owner.finish_unit_dispatch(work,|_|panic!("poll should not run yet")).unwrap();}assert_eq!(count,2);assert_eq!(groups(owner.registry()),BTreeSet::from([vec![1],vec![2],vec![3]]));assert_eq!(poll.remaining,5);
 println!("staged callback mutation regression:contact split and joint removal share one registry; new units/controllers wait until the next PSI");
}
fn errors(){
 use crate::ivp_movement::Poll;let mut r=Registry::new(9);r.register_core(0,false).unwrap();r.register_core(1,false).unwrap();r.attach_ragdoll(0,0,1).unwrap();r.wake(0).unwrap();r.begin_psi().unwrap();let mut poll=Poll{remaining:1,seed:1};let before=format!("{r:?}");assert!(r.next_unit_dispatch(&mut poll,|_,flags|{flags.flags=0xffff;Err("late preparation failure")}).is_err());assert_eq!(format!("{r:?}"),before);assert_eq!(poll.remaining,1);assert_eq!(poll.seed,1);
 let work=r.next_unit_dispatch(&mut poll,|_,_|Ok(())).unwrap().unwrap();let before=format!("{r:?}");let mut n=0;assert!(r.finish_unit_dispatch(work,|_|{n+=1;Ok(if n==1{2}else{9})}).is_err());assert_eq!(format!("{r:?}"),before);
}
#[test]fn staged_native_dispatch_and_callback_mutations_match(){verify();}
