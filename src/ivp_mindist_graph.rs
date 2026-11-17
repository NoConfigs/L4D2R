//!1010b4f0/1010bdf0/1010afb0 active, object-side and fast-list ownership.
//! Queue ownership remains with the caller; it is not implemented here.
pub(crate) const COMPACT_PAIR_FLAGS:u32=0x0fc00000;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum InitialRoute { Schedule, InvalidGeometry }
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct Side {pub pair:usize,pub side:usize}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
struct Pair {objects:[usize;2],flags:u32,active:bool,inactive:bool}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct Graph {pairs:Vec<Option<Pair>>,active:Vec<usize>,objects:Vec<Vec<Side>>,fast:Vec<usize>,inactive:Vec<usize>,inactive_objects:Vec<Vec<Side>>}
impl Graph {
 pub(crate) fn new(objects:usize)->Self{Self{pairs:vec![],active:vec![],objects:vec![vec![];objects],fast:vec![],inactive:vec![],inactive_objects:vec![vec![];objects]}}
 pub(crate) fn register(&mut self,objects:[usize;2])->Result<usize,&'static str>{
  if objects[0]==objects[1]||objects.iter().any(|&i|i>=self.objects.len()){return Err("invalid mindist object binding");}
  let id=self.pairs.iter().position(Option::is_none).unwrap_or(self.pairs.len());let pair=Some(Pair{objects,flags:COMPACT_PAIR_FLAGS,active:false,inactive:false});
  if id==self.pairs.len(){self.pairs.push(pair);}else{self.pairs[id]=pair;}Ok(id)
 }
 pub(crate) fn flags(&self,id:usize)->Result<u32,&'static str>{Ok(self.pairs.get(id).and_then(Option::as_ref).ok_or("unknown mindist pair")?.flags)}
 pub(crate) fn objects(&self,id:usize)->Result<[usize;2],&'static str>{Ok(self.pairs.get(id).and_then(Option::as_ref).ok_or("unknown mindist pair")?.objects)}
 pub(crate) fn set_flags(&mut self,id:usize,flags:u32)->Result<(),&'static str>{self.pairs.get_mut(id).and_then(Option::as_mut).ok_or("unknown mindist pair")?.flags=flags;Ok(())}
 fn link(&mut self,id:usize)->Result<(),&'static str>{
  let p=self.pairs.get_mut(id).and_then(Option::as_mut).ok_or("unknown mindist pair")?;
  if p.active{return Err("mindist pair already active");}
  if p.inactive{return Err("mindist pair still owns inactive links");}
  p.flags=(p.flags&0xffcfffff)|0xc0000;p.active=true;self.active.insert(0,id);
  for side in 0..2{self.objects[p.objects[side]].insert(0,Side{pair:id,side});}
  Ok(())
 }
 pub(crate) fn activate(&mut self,id:usize,fast_cores:[bool;2])->Result<(),&'static str>{
  self.link(id)?;
  if fast_cores.into_iter().any(|v|v)&&self.flags(id)?&0x3000!=0x1000{self.fast.push(id);}Ok(())
 }
 ///1010bdf0 links both objects before querying, then appends fast ownership.
 /// Unlike1010b4f0, the constructor does not exclude flags&0x3000==0x1000.
 /// The refresh hook returns the live query flags; its error leaves the linked
 /// record owned by the caller, which must explicitly tear it down.
 /// Scheduling or virtual slot7 follows this function and remains caller-owned.
 pub(crate) fn activate_initial(
  &mut self,id:usize,fast_cores:[bool;2],
  refresh:impl FnOnce(&Self,u32)->Result<u32,&'static str>,
 )->Result<InitialRoute,&'static str>{
  self.link(id)?;
  let flags=refresh(self,self.flags(id)?)?;
  self.set_flags(id,flags)?;
  if fast_cores.into_iter().any(|v|v){self.fast.push(id);}
  Ok(if flags&0xc000==0{InitialRoute::Schedule}else{InitialRoute::InvalidGeometry})
 }
 /// The caller must cancel a queued event before releasing a pair's ownership.
 /// Native scans the fast array backwards and swaps in its final member.
 pub(crate) fn unlink(&mut self,id:usize)->Result<(),&'static str>{
  let p=self.pairs.get_mut(id).and_then(Option::as_mut).ok_or("unknown mindist pair")?;
  if !p.active{return Err("mindist pair is not active");}
  let at=self.active.iter().position(|&v|v==id).ok_or("missing global mindist link")?;
  let positions=std::array::from_fn::<_,2,_>(|side|self.objects[p.objects[side]].iter().position(|v|*v==Side{pair:id,side}));
  if positions.iter().any(Option::is_none){return Err("missing object mindist link");}
  self.active.remove(at);for side in 0..2{self.objects[p.objects[side]].remove(positions[side].unwrap());}
  if let Some(at)=self.fast.iter().rposition(|&v|v==id){self.fast.swap_remove(at);}p.active=false;Ok(())
 }
 pub(crate) fn remove(&mut self,id:usize)->Result<(),&'static str>{
  if self.pairs.get(id).and_then(Option::as_ref).ok_or("unknown mindist pair")?.active{self.unlink(id)?;}
  if self.pairs[id].as_ref().unwrap().inactive{self.unlink_inactive(id)?;}
  self.pairs[id]=None;Ok(())
 }
 ///1010a600 maintains separate global and per-object inactive heads.
 pub(crate) fn link_inactive(&mut self,id:usize)->Result<(),&'static str>{
  let p=self.pairs.get_mut(id).and_then(Option::as_mut).ok_or("unknown mindist pair")?;
  if p.active||p.inactive{return Err("inactive insertion requires detached pair");}
  p.flags=(p.flags&0xffcbffff)|0x80000;p.inactive=true;self.inactive.insert(0,id);
  for side in 0..2{self.inactive_objects[p.objects[side]].insert(0,Side{pair:id,side});}Ok(())
 }
 ///1010a860 removes inactive ownership without touching query flags.
 pub(crate) fn unlink_inactive(&mut self,id:usize)->Result<(),&'static str>{
  let p=self.pairs.get_mut(id).and_then(Option::as_mut).ok_or("unknown mindist pair")?;
  if !p.inactive{return Err("mindist pair is not inactive");}
  let at=self.inactive.iter().position(|&v|v==id).ok_or("missing global inactive link")?;
  let positions=std::array::from_fn::<_,2,_>(|side|self.inactive_objects[p.objects[side]].iter().position(|v|*v==Side{pair:id,side}));
  if positions.iter().any(Option::is_none){return Err("missing object inactive link");}
  self.inactive.remove(at);for side in 0..2{self.inactive_objects[p.objects[side]].remove(positions[side].unwrap());}p.inactive=false;Ok(())
 }
 /// Ordinary virtual slot7/1010b5c0, after caller-owned queue cancellation.
 pub(crate) fn make_inactive(&mut self,id:usize)->Result<(),&'static str>{self.unlink(id)?;self.link_inactive(id)}
 pub(crate) fn reactivate(&mut self,id:usize,fast_cores:[bool;2])->Result<(),&'static str>{self.unlink_inactive(id)?;self.activate(id,fast_cores)}
 pub(crate) fn active(&self)->&[usize]{&self.active}
 pub(crate) fn inactive(&self)->&[usize]{&self.inactive}
 pub(crate) fn inactive_object_links(&self,id:usize)->Result<&[Side],&'static str>{self.inactive_objects.get(id).map(Vec::as_slice).ok_or("unknown mindist object")}
 pub(crate) fn fast(&self)->&[usize]{&self.fast}
 pub(crate) fn object_links(&self,id:usize)->Result<&[Side],&'static str>{self.objects.get(id).map(Vec::as_slice).ok_or("unknown mindist object")}
}
#[cfg(test)]
#[path="ivp_inactive_graph_tests.rs"]
mod inactive_tests;
#[cfg(test)]
mod tests{
 use super::*;
 #[test]
 fn constructor_query_observes_links_before_fast_membership_and_uses_live_flags(){
  // Ordering/masks from owned1010bdf0; existing512 controls below cover b4f0.
  for fast in [[false,false],[true,false],[false,true],[true,true]] {
   for query_flags in [COMPACT_PAIR_FLAGS,COMPACT_PAIR_FLAGS|0x1000,COMPACT_PAIR_FLAGS|0x4000,COMPACT_PAIR_FLAGS|0xc000] {
    let mut g=Graph::new(3);let prior=g.register([0,2]).unwrap();g.activate(prior,[true,false]).unwrap();
    let id=g.register([0,1]).unwrap();
    let route=g.activate_initial(id,fast,|linked,flags|{
     assert_eq!(flags,(COMPACT_PAIR_FLAGS&0xffcfffff)|0xc0000);
     assert_eq!(linked.active(),&[id,prior]);assert_eq!(linked.fast(),&[prior]);
     assert_eq!(linked.object_links(0).unwrap(),&[Side{pair:id,side:0},Side{pair:prior,side:0}]);
     assert_eq!(linked.object_links(1).unwrap(),&[Side{pair:id,side:1}]);Ok(query_flags)
    }).unwrap();
    assert_eq!(g.flags(id).unwrap(),query_flags);
    assert_eq!(route,if query_flags&0xc000==0{InitialRoute::Schedule}else{InitialRoute::InvalidGeometry});
    assert_eq!(g.fast(),if fast.into_iter().any(|v|v){vec![prior,id]}else{vec![prior]});
    g.remove(id).unwrap();assert_eq!(g.active(),&[prior]);assert_eq!(g.fast(),&[prior]);
   }
  }
 }
 #[test]
 fn constructor_refresh_error_retains_explicit_cleanup_ownership(){
  let mut g=Graph::new(2);let id=g.register([0,1]).unwrap();
  assert_eq!(g.activate_initial(id,[true,true],|_,_|Err("query input invalid")),Err("query input invalid"));
  assert_eq!(g.active(),&[id]);assert!(g.fast().is_empty());
  assert!(g.activate_initial(id,[false,false],|_,_|panic!("duplicate refresh")).is_err());
  g.remove(id).unwrap();assert!(g.active().is_empty());assert!(g.object_links(0).unwrap().is_empty());
 }
 fn state(g:&Graph)->serde_json::Value{serde_json::json!({"active":g.active(),"fast":g.fast(),"objects":(0..g.objects.len()).map(|i|g.object_links(i).unwrap().iter().map(|s|serde_json::json!({"pair":s.pair,"side":s.side})).collect::<Vec<_>>()).collect::<Vec<_>>(),"flags":(0..g.pairs.len()).map(|i|g.flags(i).unwrap()).collect::<Vec<_>>()})}
 #[test]
 fn native_active_object_and_fast_lists_match_all_continuous_owned_operations(){
  let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-mindist-links.json")).unwrap();let mut count=0;
  for c in d["mindist_link_controls"].as_array().unwrap(){let mut g=Graph::new(4);
   for (i,p) in c["bindings"].as_array().unwrap().iter().enumerate(){assert_eq!(g.register([p[0].as_u64().unwrap() as usize,p[1].as_u64().unwrap() as usize]).unwrap(),i);}
   assert_eq!(state(&g),c["initial"]);
   for e in c["events"].as_array().unwrap(){let id=e["pair"].as_u64().unwrap() as usize;g.set_flags(id,e["flags_input"].as_u64().unwrap() as u32).unwrap();assert_eq!(state(&g),e["before"]);
    if e["operation"]=="activate"{let mask=e["fast_core_mask"].as_u64().unwrap();let objects=g.pairs[id].as_ref().unwrap().objects;g.activate(id,objects.map(|i|mask&(1<<i)!=0)).unwrap();}else{g.unlink(id).unwrap();}
    assert_eq!(state(&g),e["after"],"scenario{} step{}",c["scenario"],e["step"]);count+=1;
   }
  }assert_eq!(count,512);
 }
 #[test]
 fn removed_ids_can_be_reused_without_aliasing_active_object_links(){
  let mut g=Graph::new(3);let a=g.register([0,1]).unwrap();let b=g.register([1,2]).unwrap();g.activate(a,[true,false]).unwrap();g.activate(b,[false,true]).unwrap();
  assert!(g.activate(a,[false,false]).is_err());g.remove(a).unwrap();assert!(g.flags(a).is_err());assert_eq!(g.fast(),&[b]);
  let c=g.register([0,2]).unwrap();assert_eq!(c,a);g.activate(c,[false,false]).unwrap();assert_eq!(g.active(),&[c,b]);
  assert_eq!(g.object_links(1).unwrap(),&[Side{pair:b,side:0}]);g.remove(c).unwrap();g.remove(b).unwrap();assert!(g.active().is_empty());assert!(g.fast().is_empty());
  assert!(g.register([1,1]).is_err());assert!(g.register([0,3]).is_err());
 }
}
