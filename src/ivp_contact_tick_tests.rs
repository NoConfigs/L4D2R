use super::*;
use crate::ivp_far_distance as far;
use bevy::prelude::*;
fn tracked(o:&mut Ownership,objects:[usize;2],keys:[f32;2])->usize{
 let id=o.graph.register(objects).unwrap();o.graph.activate(id,[false,false]).unwrap();o.graph.unlink(id).unwrap();o.graph.set_flags(id,0x0fd40000).unwrap();o.track_far(id,keys,0.).unwrap();id
}
#[test]
fn event_world_predictions_dispatch_and_rebase_use_one_live_queue_and_current_budgets(){
 use std::cell::RefCell;
 let owner=RefCell::new(Ownership::new(2,8,8,0.).unwrap());
 let pair=tracked(&mut owner.borrow_mut(),[0,1],[0.05,0.05]);
 let event_pair=owner.borrow_mut().graph.register([0,1]).unwrap();owner.borrow_mut().graph.activate(event_pair,[false,false]).unwrap();
 let mut world=crate::ivp_event_world::World::new(usize::MAX).unwrap();world.set_timestep(0.1).unwrap();
 let geometry=RefCell::new(far::Geometry{projection:0.,linear_delta:0.,distance:10.,normal:Vec3::X});
 let phases=RefCell::new(Vec::new());let events=RefCell::new(Vec::new());
 world.advance_callbacks(11.,|queue,now,deadline,_serial|{
  let mut owner=owner.borrow_mut();
  owner.with_queue(queue,|o|{
   if now==0.{o.queue.insert(event_pair,0.025)?;}
   let prepared=o.prepare_predictions(now,(deadline-now) as f32,&[Prediction{object:0,linear:1.,angular:0.},Prediction{object:1,linear:1.,angular:0.}])?;
   o.dispatch_predictions(prepared,250,|o,event|{
    let (object,notice,rebase)=match event{TickEvent::Recheck{object,notice}=>(object,notice,false),TickEvent::Rebase{object,notice}=>(object,notice,true)};
    assert_eq!(o.listener_pair(object,notice),Some((pair,object)));
    assert_eq!(o.hulls[0].budget.time,now);assert_eq!(o.hulls[1].budget.time,now);
    phases.borrow_mut().push((now,object,rebase));
    if rebase{
     geometry.borrow_mut().linear_delta=o.far(pair).unwrap().linear_delta;
    }else{
     let mut g=*geometry.borrow();g.linear_delta=o.far(pair).unwrap().linear_delta;
     let input=far::Input{flags:o.graph.flags(pair)?,now,dt:(deadline-now) as f32,gap:notice.a,geometry:g,
      cores:[far::Core{time:0.,start:Vec3::ZERO,velocity:Vec3::X,linear:1.,angular:0.};2],budgets:[o.hulls[0].budget,o.hulls[1].budget]};
     match o.recheck(pair,&input,far::Config::default())?{far::Action::Rearm{geometry:g,..}=>*geometry.borrow_mut()=g,far::Action::Activate{..}=>panic!("separated equal-speed objects should renew")}
    }Ok(())
   },|_,_|0)
  })
 },|queue,id,now,_,_|{
  owner.borrow_mut().with_queue(queue,|o|{
   assert_eq!(id,event_pair);assert!(o.queue.scheduled(id).is_none());o.retire(id)?;events.borrow_mut().push(now);Ok(())
  })
 },|_,_|{}).unwrap();
 assert_eq!(*events.borrow(),vec![f32::from_bits(0x3ccccccd) as f64]);
 let phases=phases.borrow();assert_eq!(phases[0],(0.,1,false));
 assert!(phases.iter().filter(|p|!p.2).count()>1);assert!(phases.iter().any(|p|p.2));
 // Batched dispatch reverses prediction order. Renewal on side1 can make the
 // already-queued side0 no longer due, but both still receive periodic rebase.
 let rebased:Vec<_>=phases.iter().filter(|p|p.2).map(|p|p.1).collect();assert_eq!(rebased,&[1,0]);
 let o=owner.borrow();assert!(o.far(pair).unwrap().linear_delta.abs()>0.);assert!(o.queue.first().is_none());
 assert!(world.queue.scheduled(usize::MAX).is_some());assert!(world.queue.scheduled(event_pair).is_none());
 assert!(o.hulls.iter().all(|h|h.ordered_keys().len()==1));
}
#[test]
fn prediction_validation_and_callback_errors_preserve_explicit_resource_ownership(){
 let mut o=Ownership::new(2,8,8,0.).unwrap();let pair=tracked(&mut o,[0,1],[0.05,0.05]);
 let before=o.hulls[0].budget;
 assert!(o.prepare_predictions(0.,0.1,&[Prediction{object:0,linear:1.,angular:0.},Prediction{object:0,linear:1.,angular:0.}]).is_err());assert_eq!(o.hulls[0].budget,before);
 let old=o.prepare_predictions(0.,0.1,&[Prediction{object:0,linear:1.,angular:0.}]).unwrap();
 let current=o.prepare_predictions(0.,0.1,&[Prediction{object:0,linear:1.,angular:0.}]).unwrap();
 assert_eq!(o.dispatch_predictions(old,0,|_,_|panic!("stale batch ran"),|_,_|0),Err("superseded hull prediction batch"));
 assert_eq!(o.dispatch_predictions(current,0,|_,_|Err("query provider failed"),|_,_|0),Err("query provider failed"));assert!(o.far(pair).is_some());
 let mut world_queue=Queue::new(8,0.).unwrap();world_queue.insert(99,1.).unwrap();
 assert_eq!(o.with_queue(&mut world_queue,|o|{assert!(o.queue.scheduled(99).is_some());Err::<(),_>("event provider failed")}),Err("event provider failed"));
 assert!(world_queue.scheduled(99).is_some());assert!(o.queue.scheduled(99).is_none());
 o.retire(pair).unwrap();assert!(o.hulls.iter().all(|h|h.first().is_none()));
}
