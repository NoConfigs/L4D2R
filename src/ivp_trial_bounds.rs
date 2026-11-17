//! Bounds/pair-recheck integration for the diagnostic world's default filter.
//! Synchronous hooks own detailed geometry/listener changes during dispatch;
//! physical contact-event execution remains a separate diagnostic path. No reference samples or recorded dispatch times are read.
use crate::{ivp_broadphase,ivp_hull_budget::Budget,ivp_hull_manager::Manager,ivp_hull_policy,ivp_spatial_tree::Tree};
const BROAD:usize=usize::MAX;
const DISTANCE:usize=1usize<<(usize::BITS-1);
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct Object{
    pub core:usize,pub flags:u32,pub fixed:bool,pub body:ivp_hull_policy::Body,pub center:[f32;3],
    manager:Option<Manager>,broad:u16,deadline:i32,
}
impl Object{
    pub(crate) fn new(core:usize,flags:u32,fixed:bool,radius:f32,center:[f32;3])->Self{
        Self{core,flags,fixed,body:ivp_hull_policy::Body{radius,linear:0.,angular:0.},center,manager:Some(Manager::new(8).unwrap()),broad:0,deadline:0}
    }
}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
struct Pair{objects:[usize;2],handles:[u16;2]}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
struct Distance{objects:[usize;2],handles:[u16;2]}
#[derive(Clone,Copy,Debug)]
pub(crate) enum Request{Geometry{pair:usize,objects:[usize;2],margin:f32},Distance{pair:usize,objects:[usize;2]},Removed{pair:usize},Dispatch{object:usize,before:Budget,after:Budget}}
#[derive(Clone,Copy,Debug)]
pub(crate) enum DistanceEvent{
    Recheck{pair:usize,objects:[usize;2],object:usize,gap:f32,budgets:[Budget;2]},
    Rebase{pair:usize,objects:[usize;2],object:usize,motion:f32,linear:f32},
}
#[derive(Clone,Copy,Debug)]
pub(crate) enum DistanceAction{Remove,Rearm{keys:[f32;2],first:usize},Activate{replacement:Option<[f32;2]>},ActivateAfterRemoval,Keep}
#[derive(Clone,Copy,Debug)]
pub(crate) enum DistanceEdit{Register{pair:usize,objects:[usize;2],keys:[f32;2]},Clear{pair:usize}}
pub(crate) trait Hooks{
    fn activated(&mut self,_pair:usize,_objects:[usize;2],_budgets:[Budget;2])->Result<Option<[f32;2]>,&'static str>{Err("post-removal distance activation provider is missing")}
    fn geometry(&mut self,_pair:usize,_objects:[usize;2],_margin:f32,_budgets:[Budget;2])->Result<Vec<DistanceEdit>,&'static str>{Ok(vec![])}
    fn removed(&mut self,_pair:usize,_objects:[usize;2])->Result<Vec<DistanceEdit>,&'static str>{Ok(vec![])}
    fn distance(&mut self,event:DistanceEvent)->Result<DistanceAction,&'static str>{Ok(match event{DistanceEvent::Recheck{..}=>DistanceAction::Remove,DistanceEvent::Rebase{..}=>DistanceAction::Keep})}
}
struct Deferred;impl Hooks for Deferred{}
struct DistanceOnly<F>(F);
impl<F:FnMut(DistanceEvent)->Result<DistanceAction,&'static str>> Hooks for DistanceOnly<F>{fn distance(&mut self,event:DistanceEvent)->Result<DistanceAction,&'static str>{(self.0)(event)}}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct World{objects:Vec<Object>,tree:Tree,pairs:Vec<Option<Pair>>,distances:Vec<Option<Distance>>,dt:f32}
pub(crate) struct Prediction{pub object:usize,pub center:[f32;3],pub linear:f32,pub angular:f32}
/// One queued hull dispatch, produced during prediction and consumed once.
pub(crate) struct Prepared{object:usize,now:f64,due:bool}
impl World{
    #[cfg(test)]
    pub(crate) fn distance_keys(&self,id:usize)->Result<[f32;2],&'static str>{
        let p=self.distances.get(id).and_then(Option::as_ref).ok_or("unknown detailed distance")?;
        Ok([self.objects[p.objects[0]].manager.as_ref().ok_or("distance manager is borrowed")?.entry(p.handles[0]).ok_or("distance handle disappeared")?.1,self.objects[p.objects[1]].manager.as_ref().ok_or("distance manager is borrowed")?.entry(p.handles[1]).ok_or("distance handle disappeared")?.1])
    }
    #[cfg(test)]
    pub(crate) fn coarse_objects(&self)->Vec<[usize;2]>{self.pairs.iter().flatten().map(|p|p.objects).collect()}
    pub(crate) fn new(objects:Vec<Object>,order:&[usize],dt:f32)->Result<Self,&'static str>{
        let mut result=Self{objects,tree:Tree::new(),pairs:Vec::new(),distances:Vec::new(),dt};
        for &id in order{
            let o=&mut result.objects[id];let m=o.manager.as_mut().unwrap();
            let (cell,key)=ivp_broadphase::Policy::default().renew(0.,dt,o.body,m.budget,o.center)?;
            o.broad=m.insert(BROAD,key)?;result.tree.insert(id,o.center,cell.radius,cell.radius,false)?;
        }
        Ok(result)
    }
    fn budget(&self,id:usize,active:usize,m:&Manager)->Budget{if id==active{m.budget}else{self.objects[id].manager.as_ref().unwrap().budget}}
    fn replace(&mut self,id:usize,active:usize,m:&mut Manager,handle:u16,key:f32)->Result<u16,&'static str>{
        if id==active{m.replace(handle,key)}else{self.objects[id].manager.as_mut().unwrap().replace(handle,key)}
    }
    fn remove(&mut self,id:usize,active:usize,m:&mut Manager,handle:u16)->Result<(),&'static str>{
        if id==active{m.remove(handle)?;}else{self.objects[id].manager.as_mut().unwrap().remove(handle)?;}Ok(())
    }
    fn insert(&mut self,id:usize,active:usize,m:&mut Manager,owner:usize,key:f32)->Result<u16,&'static str>{
        if id==active{m.insert(owner,key)}else{self.objects[id].manager.as_mut().unwrap().insert(owner,key)}
    }
    fn clear_inside(&mut self,pair:usize,active:usize,m:&mut Manager)->Result<(),&'static str>{
        if let Some(p)=self.distances.get_mut(pair).and_then(Option::take){self.remove(p.objects[0],active,m,p.handles[0])?;self.remove(p.objects[1],active,m,p.handles[1])?;}Ok(())
    }
    fn register_inside(&mut self,pair:usize,objects:[usize;2],keys:[f32;2],active:usize,m:&mut Manager)->Result<(),&'static str>{
        if pair>=DISTANCE-1||objects[0]==objects[1]||objects.iter().any(|&i|i>=self.objects.len())||keys.iter().any(|v|!v.is_finite()){return Err("invalid in-dispatch distance registration");}
        if self.distances.get(pair).and_then(Option::as_ref).is_some(){return Err("distance listener registered twice");}
        let left=self.insert(objects[0],active,m,DISTANCE|pair,keys[0])?;
        let right=match self.insert(objects[1],active,m,DISTANCE|pair,keys[1]){Ok(h)=>h,Err(error)=>{self.remove(objects[0],active,m,left)?;return Err(error);}};
        if self.distances.len()<=pair{self.distances.resize_with(pair+1,||None);}self.distances[pair]=Some(Distance{objects,handles:[left,right]});Ok(())
    }
    fn edits(&mut self,edits:Vec<DistanceEdit>,active:usize,m:&mut Manager)->Result<(),&'static str>{
        for edit in edits{match edit{DistanceEdit::Register{pair,objects,keys}=>self.register_inside(pair,objects,keys,active,m)?,DistanceEdit::Clear{pair}=>self.clear_inside(pair,active,m)?,}}Ok(())
    }
    fn eligible(&self,a:usize,b:usize)->bool{
        let a=&self.objects[a];let b=&self.objects[b];
        (a.flags&7!=0||b.flags&7!=0)&&a.core!=b.core&&!(a.fixed&&b.fixed)&&!(a.flags&0x200!=0&&b.flags&0x200!=0)
    }
    fn renew_pair(&mut self,pair:usize,active:usize,m:&mut Manager,now:f64,dt:f32,requests:&mut Vec<Request>,hooks:&mut dyn Hooks)->Result<(),&'static str>{
        let p=self.pairs[pair].as_ref().ok_or("missing pair listener")?;let ids=p.objects;let handles=p.handles;
        let bodies=ids.map(|id|self.objects[id].body);let budgets=ids.map(|id|self.budget(id,active,m));
        let allowances=ivp_hull_policy::Policy::default().allowances(dt,bodies);let margin=allowances[0]+allowances[1];
        let edits=hooks.geometry(pair,ids,margin,budgets)?;self.edits(edits,active,m)?;
        let keys=ivp_hull_policy::Policy::default().thresholds(now,dt,bodies,&budgets);
        let handles=[self.replace(ids[0],active,m,handles[0],keys[0])?,self.replace(ids[1],active,m,handles[1],keys[1])?];
        self.pairs[pair].as_mut().unwrap().handles=handles;requests.push(Request::Geometry{pair,objects:ids,margin});Ok(())
    }
    fn broadphase(&mut self,active:usize,m:&mut Manager,now:f64,dt:f32,requests:&mut Vec<Request>,hooks:&mut dyn Hooks)->Result<(),&'static str>{
        let o=&self.objects[active];let (cell,key)=ivp_broadphase::Policy::default().renew(now,dt,o.body,m.budget,o.center)?;
        self.tree.remove(active)?;let found=self.tree.insert(active,o.center,cell.radius,cell.radius,true)?;
        self.objects[active].broad=m.replace(self.objects[active].broad,key)?;
        let eligible=found.into_iter().filter(|&id|self.eligible(active,id)).collect::<Vec<_>>();
        let old=self.pairs.iter().enumerate().filter_map(|(i,p)|{let p=p.as_ref()?;p.objects.contains(&active).then_some(i)}).collect::<Vec<_>>();
        for pair in old{
            let ids=self.pairs[pair].as_ref().unwrap().objects;let other=if ids[0]==active{ids[1]}else{ids[0]};
            if !eligible.contains(&other){let edits=hooks.removed(pair,ids)?;self.edits(edits,active,m)?;let p=self.pairs[pair].take().unwrap();self.remove(p.objects[1],active,m,p.handles[1])?;self.remove(p.objects[0],active,m,p.handles[0])?;requests.push(Request::Removed{pair});}
        }
        for other in eligible{
            if self.pairs.iter().flatten().any(|p|p.objects.contains(&active)&&p.objects.contains(&other)){continue;}
            let id=self.pairs.iter().position(Option::is_none).unwrap_or(self.pairs.len());
            let ids=[active,other];let budgets=ids.map(|id|self.budget(id,active,m));
            //101280d0 installs temporary huge thresholds, queries children,
            // then10127f00 renews the generic entries after child allocation.
            let keys=budgets.map(|b|((now-b.time)*b.rates[0] as f64+b.accumulated[0] as f64+f32::from_bits(0x60ad78ec) as f64) as f32);
            let handles=[m.insert(id,keys[0])?,self.objects[other].manager.as_mut().unwrap().insert(id,keys[1])?];
            let p=Some(Pair{objects:ids,handles});if id==self.pairs.len(){self.pairs.push(p);}else{self.pairs[id]=p;}
            self.renew_pair(id,active,m,now,dt,requests,hooks)?;
        }
        Ok(())
    }
    pub(crate) fn object_budget(&self,id:usize)->Result<Budget,&'static str>{Ok(self.objects.get(id).ok_or("unknown bounds object")?.manager.as_ref().ok_or("manager is inside dispatch")?.budget)}
    /// Detailed mindist listeners use their own handles and keys; broad-pair
    /// renewal must not overwrite or impersonate these entries.
    pub(crate) fn register_distance(&mut self,pair:usize,objects:[usize;2],keys:[f32;2])->Result<(),&'static str>{
        if pair>=DISTANCE-1||objects[0]==objects[1]||objects.iter().any(|&i|i>=self.objects.len())||keys.iter().any(|v|!v.is_finite()){return Err("invalid distance listener");}
        // Check both managers before changing either list. In particular, a
        // reentrant call must not leak the first handle or erase a live pair.
        for &id in &objects{if self.objects[id].manager.is_none(){return Err("manager is inside dispatch");}}
        if let Some(old)=self.distances.get(pair).and_then(Option::as_ref){for &id in &old.objects{if self.objects[id].manager.is_none(){return Err("manager is inside dispatch");}}}
        let left=self.objects[objects[0]].manager.as_mut().ok_or("manager is inside dispatch")?.insert(DISTANCE|pair,keys[0])?;
        let right=match self.objects[objects[1]].manager.as_mut().ok_or("manager is inside dispatch")?.insert(DISTANCE|pair,keys[1]){Ok(handle)=>handle,Err(error)=>{self.objects[objects[0]].manager.as_mut().unwrap().remove(left)?;return Err(error);}};
        // Allocate the replacement before retiring the old registration. A
        // failed allocation leaves the previous distance thresholds intact.
        self.clear_distance(pair)?;
        if self.distances.len()<=pair{self.distances.resize_with(pair+1,||None);}
        self.distances[pair]=Some(Distance{objects,handles:[left,right]});Ok(())
    }
    pub(crate) fn clear_distance(&mut self,pair:usize)->Result<(),&'static str>{
        if let Some(p)=self.distances.get(pair).and_then(Option::as_ref){for &id in &p.objects{if self.objects[id].manager.is_none(){return Err("manager is inside dispatch");}}}
        if let Some(p)=self.distances.get_mut(pair).and_then(Option::take){for i in 0..2{self.objects[p.objects[i]].manager.as_mut().ok_or("manager is inside dispatch")?.remove(p.handles[i])?;}}Ok(())
    }
    pub(crate) fn notify(&mut self,id:usize,now:f64,dt:f32,flags:u32,center:[f32;3],linear:f32,angular:f32)->Result<Vec<Request>,&'static str>{
        self.notify_with_hooks(id,now,dt,flags,center,linear,angular,&mut Deferred)
    }
    pub(crate) fn notify_with_hooks(&mut self,id:usize,now:f64,dt:f32,flags:u32,center:[f32;3],linear:f32,angular:f32,hooks:&mut dyn Hooks)->Result<Vec<Request>,&'static str>{
        let o=&mut self.objects[id];o.flags=flags;o.center=center;o.body.linear=linear;o.body.angular=angular;
        let mut m=o.manager.take().unwrap();let mut requests=Vec::new();let result=self.broadphase(id,&mut m,now,dt,&mut requests,hooks);self.objects[id].manager=Some(m);result?;Ok(requests)
    }
    pub(crate) fn set_flags(&mut self,id:usize,flags:u32){self.objects[id].flags=flags;}
    pub(crate) fn step(&mut self,id:usize,now:f64,dt:f32,center:[f32;3],linear:f32,angular:f32)->Result<Vec<Request>,&'static str>{
        self.step_with_distance(id,now,dt,center,linear,angular,|event|Ok(match event{DistanceEvent::Recheck{..}=>DistanceAction::Remove,DistanceEvent::Rebase{..}=>DistanceAction::Keep}))
    }
    /// Rechecks run inside native-style dispatch, before the manager rebase.
    /// Returning Rearm keeps the listener live and can cause another immediate
    /// dispatch. Use step_with_hooks for synchronous geometry and activation.
    pub(crate) fn step_with_distance(&mut self,id:usize,now:f64,dt:f32,center:[f32;3],linear:f32,angular:f32,mut callback:impl FnMut(DistanceEvent)->Result<DistanceAction,&'static str>)->Result<Vec<Request>,&'static str>{
        self.step_with_hooks(id,now,dt,center,linear,angular,&mut DistanceOnly(&mut callback))
    }
    pub(crate) fn step_with_hooks(&mut self,id:usize,now:f64,dt:f32,center:[f32;3],linear:f32,angular:f32,hooks:&mut dyn Hooks)->Result<Vec<Request>,&'static str>{
        let prepared=self.prepare_prediction(id,now,dt,center,linear,angular)?;
        self.dispatch_prediction(prepared,hooks)
    }
    fn prepare_prediction(&mut self,id:usize,now:f64,dt:f32,center:[f32;3],linear:f32,angular:f32)->Result<Prepared,&'static str>{
        if !now.is_finite()||!dt.is_finite()||dt<0.||center.iter().any(|v|!v.is_finite())||![linear,angular].iter().all(|v|v.is_finite()&&*v>=0.){return Err("invalid bounds prediction");}
        let o=self.objects.get_mut(id).ok_or("unknown bounds prediction object")?;
        let m=o.manager.as_mut().ok_or("bounds prediction during dispatch")?;
        o.center=center;o.body.linear=linear;o.body.angular=angular;
        let due=m.budget.advance(now,dt,linear,angular);Ok(Prepared{object:id,now,due})
    }
    ///1010fd30/1010f960 finish all core/object predictions before1010f870
    /// consumes the hull dispatch vector backwards. Geometry callbacks must
    /// therefore see the other objects' updated budgets, not their old ticks.
    pub(crate) fn prepare_predictions(&mut self,now:f64,dt:f32,inputs:&[Prediction])->Result<Vec<Prepared>,&'static str>{
        let mut ids=std::collections::BTreeSet::new();
        if !now.is_finite()||!dt.is_finite()||dt<0.||inputs.iter().any(|i|!ids.insert(i.object)||self.objects.get(i.object).is_none_or(|o|o.manager.is_none())||i.center.iter().any(|v|!v.is_finite())||![i.linear,i.angular].iter().all(|v|v.is_finite()&&*v>=0.)){return Err("invalid or duplicate batched bounds prediction");}
        inputs.iter().map(|i|self.prepare_prediction(i.object,now,dt,i.center,i.linear,i.angular)).collect()
    }
    pub(crate) fn dispatch_predictions(&mut self,prepared:Vec<Prepared>,hooks:&mut dyn Hooks)->Result<Vec<Request>,&'static str>{
        let mut requests=vec![];
        for p in prepared.into_iter().rev(){requests.extend(self.dispatch_prediction(p,hooks)?);}
        Ok(requests)
    }
    fn dispatch_prediction(&mut self,prepared:Prepared,hooks:&mut dyn Hooks)->Result<Vec<Request>,&'static str>{
        let Prepared{object:id,now,due}=prepared;
        let mut m=self.objects.get_mut(id).ok_or("unknown prepared bounds object")?.manager.take().ok_or("prepared bounds dispatch is reentrant")?;
        let mut requests=Vec::new();let mut quota=false;
        let before=m.budget;let policy_dt=self.dt;
        let mut result=if due{m.dispatch(250,|m,n|{
            if n.owner==BROAD{self.broadphase(id,m,now,policy_dt,&mut requests,hooks)}
            else if n.owner&DISTANCE!=0{
                let pair=n.owner&!DISTANCE;let p=self.distances.get(pair).and_then(Option::as_ref).ok_or("missing distance listener")?;let objects=p.objects;let handles=p.handles;
                let budgets=objects.map(|object|self.budget(object,id,m));
                match hooks.distance(DistanceEvent::Recheck{pair,objects,object:id,gap:n.a,budgets})?{
                    DistanceAction::Remove=>{self.remove(objects[0],id,m,handles[0])?;self.remove(objects[1],id,m,handles[1])?;self.distances[pair]=None;requests.push(Request::Distance{pair,objects});},
                    DistanceAction::Rearm{keys,first}=>{
                        if first>1||keys.iter().any(|v|!v.is_finite()){return Err("invalid synchronous distance rearm");}
                        let mut renewed=handles;
                        for side in [first,1-first]{renewed[side]=self.replace(objects[side],id,m,handles[side],keys[side])?;}
                        self.distances[pair].as_mut().unwrap().handles=renewed;
                    },
                    DistanceAction::Activate{replacement}=>{
                        if replacement.is_some_and(|keys|keys.iter().any(|v|!v.is_finite())){return Err("invalid activation replacement");}
                        self.clear_inside(pair,id,m)?;if let Some(keys)=replacement{self.register_inside(pair,objects,keys,id,m)?;}requests.push(Request::Distance{pair,objects});
                    },
                    DistanceAction::ActivateAfterRemoval=>{
                        self.clear_inside(pair,id,m)?;
                        let budgets=objects.map(|object|self.budget(object,id,m));
                        if let Some(keys)=hooks.activated(pair,objects,budgets)?{self.register_inside(pair,objects,keys,id,m)?;}
                        requests.push(Request::Distance{pair,objects});
                    },
                    DistanceAction::Keep=>return Err("due distance listener was not advanced"),
                }Ok(())
            }else{self.renew_pair(n.owner,id,m,now,policy_dt,&mut requests,hooks)}
        },|_|{quota=true;0}).map(|_|())}else{Ok(())};
        if due&&result.is_ok(){requests.push(Request::Dispatch{object:id,before,after:m.budget});}
        if due&&result.is_ok()&&(self.objects[id].deadline as f64)<m.budget.time{
            let mut failure=None;
            m.rebase(|_,notice|{
                if notice.owner!=BROAD&&notice.owner&DISTANCE!=0{
                    let pair=notice.owner&!DISTANCE;
                    let r=match self.distances.get(pair).and_then(Option::as_ref){Some(p)=>hooks.distance(DistanceEvent::Rebase{pair,objects:p.objects,object:id,motion:notice.a,linear:notice.b}),None=>Err("rebased distance listener has no owner")};
                    match r{Ok(DistanceAction::Keep)=>{},Ok(_)=>failure=Some("distance rebase callback changed listener membership"),Err(error)=>failure=Some(error)}
                }
            });if let Some(error)=failure{result=Err(error);}self.objects[id].deadline=(m.budget.time+10.).trunc() as i32;
        }
        self.objects[id].manager=Some(m);result?;if quota{return Err("world recheck quota callback not recovered");}Ok(requests)
    }
    pub(crate) fn sleep(&mut self,id:usize,now:f64,dt:f32,center:[f32;3])->Result<Vec<Request>,&'static str>{
        self.sleep_with_hooks(id,now,dt,center,&mut Deferred)
    }
    pub(crate) fn sleep_with_hooks(&mut self,id:usize,now:f64,dt:f32,center:[f32;3],hooks:&mut dyn Hooks)->Result<Vec<Request>,&'static str>{
        self.sleep_with_flags_and_hooks(id,now,dt,center,0x108,hooks)
    }
    pub(crate) fn sleep_with_flags_and_hooks(&mut self,id:usize,now:f64,dt:f32,center:[f32;3],flags:u32,hooks:&mut dyn Hooks)->Result<Vec<Request>,&'static str>{
        if flags&0xff!=8{return Err("sleep notification has an awake object flag");}
        let requests=self.notify_with_hooks(id,now,dt,flags,center,0.,0.,hooks)?;
        let m=self.objects[id].manager.as_mut().unwrap();
        let elapsed=now-m.budget.time;for i in 0..2{m.budget.accumulated[i]=(elapsed*m.budget.rates[i] as f64+m.budget.accumulated[i] as f64) as f32;}
        m.budget.rates=[0.;2];let mut failure=None;
        m.rebase(|_,n|{if n.owner!=BROAD&&n.owner&DISTANCE!=0{let pair=n.owner&!DISTANCE;let result=match self.distances.get(pair).and_then(Option::as_ref){Some(p)=>hooks.distance(DistanceEvent::Rebase{pair,objects:p.objects,object:id,motion:n.a,linear:n.b}),None=>Err("sleep rebase lost distance owner")};match result{Ok(DistanceAction::Keep)=>{},Ok(_)=>failure=Some("sleep rebase changed membership"),Err(error)=>failure=Some(error)}}});
        m.budget.advance(now,dt,0.,0.);if let Some(error)=failure{return Err(error);}Ok(requests)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn distance_activation_constructor_observes_both_old_listeners_removed(){
        let mut w=World::new(vec![Object::new(0,0x101,false,0.1,[0.;3]),Object::new(1,0x101,false,0.1,[10.,0.,0.])],&[0,1],0.015).unwrap();
        let thresholds=[w.object_budget(0).unwrap().threshold,w.object_budget(1).unwrap().threshold];
        w.register_distance(7,[0,1],[-1.,-2.]).unwrap();
        struct Observe{thresholds:[f32;2],calls:usize}
        impl Hooks for Observe{
            fn distance(&mut self,event:DistanceEvent)->Result<DistanceAction,&'static str>{
                match event{DistanceEvent::Recheck{pair,objects,..}=>{assert_eq!(pair,7);assert_eq!(objects,[0,1]);Ok(DistanceAction::ActivateAfterRemoval)},DistanceEvent::Rebase{..}=>Ok(DistanceAction::Keep)}
            }
            fn activated(&mut self,pair:usize,objects:[usize;2],budgets:[Budget;2])->Result<Option<[f32;2]>,&'static str>{
                assert_eq!(pair,7);assert_eq!(objects,[0,1]);assert_eq!(budgets.map(|b|b.threshold),self.thresholds);self.calls+=1;Ok(Some([10.,20.]))
            }
        }
        let mut observer=Observe{thresholds,calls:0};
        let batch=w.prepare_predictions(0.,0.015,&[Prediction{object:0,center:[0.;3],linear:0.,angular:0.},Prediction{object:1,center:[10.,0.,0.],linear:0.,angular:0.}]).unwrap();
        w.dispatch_predictions(batch,&mut observer).unwrap();assert_eq!(observer.calls,1);assert_eq!(w.distance_keys(7).unwrap(),[10.,20.]);
    }
    #[test]
    fn batched_predictions_precede_reverse_order_hull_callbacks(){
        let mut w=World::new(vec![Object::new(0,0x101,false,0.1,[0.;3]),Object::new(1,0x101,false,0.1,[10.,0.,0.])],&[0,1],0.015).unwrap();
        w.register_distance(0,[0,1],[-1.,100.]).unwrap();w.register_distance(1,[0,1],[100.,-2.]).unwrap();
        struct Observe{seen:Vec<(usize,usize,[[f32;2];2])>}
        impl Hooks for Observe{
            fn distance(&mut self,event:DistanceEvent)->Result<DistanceAction,&'static str>{match event{
                DistanceEvent::Recheck{pair,object,budgets,..}=>{self.seen.push((pair,object,budgets.map(|b|b.rates)));Ok(DistanceAction::Remove)},
                DistanceEvent::Rebase{..}=>Ok(DistanceAction::Keep),
            }}
        }
        let mut observe=Observe{seen:vec![]};
        let prepared=w.prepare_predictions(0.1,0.015,&[Prediction{object:0,center:[0.;3],linear:1.,angular:0.},Prediction{object:1,center:[10.,0.,0.],linear:2.,angular:0.}]).unwrap();
        assert!(observe.seen.is_empty());w.dispatch_predictions(prepared,&mut observe).unwrap();
        assert_eq!(observe.seen.len(),2);assert_eq!((observe.seen[0].0,observe.seen[0].1),(1,1));assert_eq!((observe.seen[1].0,observe.seen[1].1),(0,0));
        for (_,_,rates) in observe.seen{assert_eq!(rates,[[1.0000100135803223,1.],[2.*1.0000100135803223,2.]]);}
    }
    #[test]
    fn duplicate_batched_objects_fail_before_updating_any_budget(){
        let mut w=world();let before=[w.object_budget(0).unwrap(),w.object_budget(1).unwrap()];
        assert!(w.prepare_predictions(1.,0.015,&[Prediction{object:0,center:[0.;3],linear:2.,angular:0.},Prediction{object:0,center:[1.;3],linear:3.,angular:0.}]).is_err());
        assert_eq!([w.object_budget(0).unwrap(),w.object_budget(1).unwrap()],before);
    }
    fn world()->World{
        World::new(vec![Object::new(0,0x101,false,0.1,[0.;3]),Object::new(1,0x110,true,0.1,[0.;3])],&[1,0],0.015).unwrap()
    }
    fn distance_requests(requests:&[Request])->Vec<(usize,[usize;2])>{requests.iter().filter_map(|r|if let Request::Distance{pair,objects}=r{Some((*pair,*objects))}else{None}).collect()}
    #[test]
    fn distance_listener_notifies_once_and_removes_both_handles(){
        let mut w=world();w.register_distance(0,[0,1],[-1.,-1.]).unwrap();
        let r=w.step(0,0.015,0.015,[0.;3],0.,0.).unwrap();assert_eq!(distance_requests(&r),vec![(0,[0,1])]);
        assert!(w.distances[0].is_none());
        let r=w.step(1,0.015,0.015,[0.;3],0.,0.).unwrap();assert!(distance_requests(&r).is_empty());
        assert_eq!(w.object_budget(0).unwrap().threshold,w.objects[0].manager.as_ref().unwrap().budget.threshold);
    }
    #[test]
    fn either_object_can_dispatch_and_multiple_distances_keep_their_order(){
        let mut w=world();w.register_distance(0,[0,1],[4.,-0.5]).unwrap();w.register_distance(1,[0,1],[4.,-1.]).unwrap();
        let r=w.step(1,0.015,0.015,[0.;3],0.,0.).unwrap();assert_eq!(distance_requests(&r),vec![(1,[0,1]),(0,[0,1])]);
        assert!(w.distances.iter().all(Option::is_none));
        assert!(distance_requests(&w.step(0,0.015,0.015,[0.;3],0.,0.).unwrap()).is_empty());
    }
    #[test]
    fn replacing_and_clearing_distance_preserves_broadphase_entries(){
        let mut w=world();let broad=w.objects.iter().map(|o|o.broad).collect::<Vec<_>>();
        w.register_distance(0,[0,1],[-1.;2]).unwrap();w.register_distance(0,[1,0],[0.5;2]).unwrap();
        assert!(distance_requests(&w.step(0,0.015,0.015,[0.;3],0.,0.).unwrap()).is_empty());
        w.clear_distance(0).unwrap();w.clear_distance(0).unwrap();
        assert_eq!(broad,w.objects.iter().map(|o|o.broad).collect::<Vec<_>>());
        for o in &w.objects{assert_eq!(o.manager.as_ref().unwrap().budget.threshold,0.5);}
    }
    #[test]
    fn rejected_reentrant_or_invalid_edits_preserve_existing_registration(){
        let mut w=world();w.register_distance(0,[0,1],[-1.;2]).unwrap();
        for (ids,keys) in [([0,0],[0.;2]),([0,2],[0.;2]),([0,1],[f32::NAN,0.])]{assert!(w.register_distance(0,ids,keys).is_err());}
        let manager=w.objects[1].manager.take().unwrap();let threshold=w.object_budget(0).unwrap().threshold;
        assert!(w.register_distance(0,[0,1],[2.;2]).is_err());assert!(w.clear_distance(0).is_err());
        assert_eq!(w.object_budget(0).unwrap().threshold,threshold);assert!(w.distances[0].is_some());w.objects[1].manager=Some(manager);
        assert_eq!(distance_requests(&w.step(0,0.015,0.015,[0.;3],0.,0.).unwrap()),vec![(0,[0,1])]);
    }
    #[test]
    fn distance_threshold_equality_waits_until_the_motion_budget_exceeds_it(){
        let mut w=world();w.register_distance(0,[0,1],[0.;2]).unwrap();
        assert!(distance_requests(&w.step(0,0.015,0.015,[0.;3],0.,0.).unwrap()).is_empty());
        assert_eq!(distance_requests(&w.step(0,0.030,0.015,[0.;3],0.1,0.).unwrap()),vec![(0,[0,1])]);
    }
    #[test]
    fn synchronous_rearm_is_rechecked_immediately_and_rebase_runs_last(){
        let mut w=world();w.register_distance(0,[0,1],[-1.;2]).unwrap();let mut order=vec![];let mut rechecks=0;
        let requests=w.step_with_distance(0,0.015,0.015,[0.;3],0.,0.,|event|Ok(match event{
            DistanceEvent::Recheck{pair,objects,object,gap,budgets}=>{assert_eq!((pair,objects,object),(0,[0,1],0));assert_eq!(budgets[0].time,0.015);assert_eq!(budgets[1].time,0.);order.push("recheck");rechecks+=1;
                assert_eq!(gap,if rechecks==1{-1.}else{-0.5});DistanceAction::Rearm{keys:[if rechecks==1{-0.5}else{1.},100.],first:0}},
            DistanceEvent::Rebase{pair,objects,object,motion,linear}=>{assert_eq!((pair,objects,object),(0,[0,1],0));assert_eq!(rechecks,2);assert_eq!(motion.to_bits(),(-0.0f32).to_bits());assert_eq!(linear.to_bits(),(-0.0f32).to_bits());order.push("rebase");DistanceAction::Keep}
        })).unwrap();
        assert_eq!(order,vec!["recheck","recheck","rebase"]);assert!(distance_requests(&requests).is_empty());assert!(w.distances[0].is_some());
        assert_eq!(w.object_budget(0).unwrap().threshold,0.5); // broadphase threshold survives the distance retry.
    }
    #[test]
    fn synchronous_rebase_notifies_retained_state_before_returning(){
        let mut w=world();w.register_distance(0,[0,1],[0.02,100.]).unwrap();
        let mut events=vec![];let mut retained=crate::ivp_far_distance::Geometry{projection:0.,linear_delta:0.75,distance:1.,normal:bevy::prelude::Vec3::Y};let mut expected=None;
        w.step_with_distance(0,0.01,0.01,[0.;3],1.,0.,|_|panic!("distance not due yet")).unwrap();
        w.step_with_distance(0,0.03,0.01,[0.;3],1.,0.,|event|Ok(match event{
            DistanceEvent::Recheck{budgets,..}=>{events.push("recheck");assert!(budgets[0].accumulated[0]>0.);expected=Some(budgets[0]);DistanceAction::Rearm{keys:[1.,100.],first:0}},
            DistanceEvent::Rebase{motion,linear,..}=>{events.push("rebase");let b=expected.unwrap();assert_eq!(motion.to_bits(),(-b.accumulated[0]).to_bits());assert_eq!(linear.to_bits(),(-b.accumulated[1]).to_bits());retained.rebase(motion,linear);DistanceAction::Keep}
        })).unwrap();
        let b=expected.unwrap();assert_eq!(retained.linear_delta.to_bits(),((-b.accumulated[0]+b.accumulated[1])+0.75).to_bits());
        assert_eq!(events,vec!["recheck","rebase"]);assert_eq!(w.object_budget(0).unwrap().accumulated,[0.;2]);
    }
    #[test]
    fn invalid_synchronous_rearm_keeps_the_existing_handles_owned(){
        let mut w=world();w.register_distance(0,[0,1],[-1.;2]).unwrap();let handles=w.distances[0].as_ref().unwrap().handles;
        assert!(w.step_with_distance(0,0.015,0.015,[0.;3],0.,0.,|_|Ok(DistanceAction::Rearm{keys:[f32::NAN,1.],first:0})).is_err());
        assert_eq!(w.distances[0].as_ref().unwrap().handles,handles);assert!(w.object_budget(0).is_ok());
        assert_eq!(distance_requests(&w.step(0,0.03,0.015,[0.;3],0.,0.).unwrap()),vec![(0,[0,1])]);
    }
    #[test]
    fn activation_can_return_to_far_tracking_before_dispatch_rebase(){
        struct Activate { order:Vec<&'static str> }
        impl Hooks for Activate {
            fn distance(&mut self,event:DistanceEvent)->Result<DistanceAction,&'static str>{Ok(match event{
                DistanceEvent::Recheck{..}=>{self.order.push("activate");DistanceAction::Activate{replacement:Some([1.,100.])}},
                DistanceEvent::Rebase{..}=>{self.order.push("rebase");DistanceAction::Keep}
            })}
        }
        let mut w=world();w.register_distance(7,[0,1],[-1.;2]).unwrap();
        let mut hook=Activate{order:vec![]};
        let requests=w.step_with_hooks(0,0.015,0.015,[0.;3],0.,0.,&mut hook).unwrap();
        assert_eq!(hook.order,vec!["activate","rebase"]);
        assert_eq!(distance_requests(&requests),vec![(7,[0,1])]);
        assert!(w.distances[7].is_some());
        assert!(distance_requests(&w.step(1,0.015,0.015,[0.;3],0.,0.).unwrap()).is_empty());
        w.clear_distance(7).unwrap();
    }
    #[test]
    fn broad_pair_geometry_owns_children_until_synchronous_retirement(){
        struct Children { order:Vec<&'static str> }
        impl Hooks for Children {
            fn geometry(&mut self,pair:usize,objects:[usize;2],_:f32,_:[Budget;2])->Result<Vec<DistanceEdit>,&'static str>{
                assert_eq!(pair,0);self.order.push("create");
                Ok(vec![DistanceEdit::Register{pair:7,objects,keys:[-1.;2]}])
            }
            fn removed(&mut self,pair:usize,_:[usize;2])->Result<Vec<DistanceEdit>,&'static str>{
                assert_eq!(pair,0);self.order.push("retire");Ok(vec![DistanceEdit::Clear{pair:7}])
            }
            fn distance(&mut self,_:DistanceEvent)->Result<DistanceAction,&'static str>{panic!("retired child must never dispatch")}
        }
        let mut w=world();let mut hook=Children{order:vec![]};
        let requests=w.notify_with_hooks(0,0.,0.015,0x101,[0.;3],0.,0.,&mut hook).unwrap();
        assert!(matches!(requests.as_slice(),[Request::Geometry{pair:0,..}]));
        assert!(w.pairs[0].is_some()&&w.distances[7].is_some());
        // Separation retires children while generic ownership still exists.
        let requests=w.notify_with_hooks(0,0.01,0.015,0x101,[1000.;3],0.,0.,&mut hook).unwrap();
        assert!(matches!(requests.as_slice(),[Request::Removed{pair:0}]));
        assert_eq!(hook.order,vec!["create","retire"]);
        assert!(w.pairs[0].is_none()&&w.distances[7].is_none());
        assert!(distance_requests(&w.step(1,0.015,0.015,[0.;3],0.,0.).unwrap()).is_empty());
    }
    #[test]
    fn invalid_activation_replacement_keeps_both_listener_handles(){
        let mut w=world();w.register_distance(3,[0,1],[-1.;2]).unwrap();
        let handles=w.distances[3].as_ref().unwrap().handles;
        let mut hook=DistanceOnly(|_|Ok(DistanceAction::Activate{replacement:Some([1.,f32::NAN])}));
        assert!(w.step_with_hooks(0,0.015,0.015,[0.;3],0.,0.,&mut hook).is_err());
        assert_eq!(w.distances[3].as_ref().unwrap().handles,handles);
        assert_eq!(distance_requests(&w.step(1,0.03,0.015,[0.;3],0.,0.).unwrap()),vec![(3,[0,1])]);
    }

}
