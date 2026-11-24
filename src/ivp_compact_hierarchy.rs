//! Transfer freshly constructed recursive ownership into spatial discovery.
//! The outer tick stages graph, counters, cache, bodies and bounds together.
use super::*;
use crate::ivp_surface_admission::Pair;
use super::super::{ownership::Ownership,recursion::construction::Binding};

fn descendants(entry:&Entry<'_>,prefix:&mut Vec<Pair>,out:&mut Vec<Vec<Pair>>){
 if let Some(r)=&entry.recursive{for child in r.descendants.entries(){prefix.push(child.key);out.push(prefix.clone());descendants(child,prefix,out);prefix.pop();}}
}
fn relative<'a,'b>(entry:&'b Entry<'a>,path:&[Pair])->Result<&'b Entry<'a>,&'static str>{
 if path.is_empty(){return Ok(entry);}
 entry.recursive.as_ref().ok_or("constructed hierarchy crosses an ordinary contact")?.descendants.entry_at_path(path)
}
pub(super) fn retirement_paths(entry:&Entry<'_>)->Vec<Vec<Pair>>{
 fn walk(entry:&Entry<'_>,path:&mut Vec<Pair>,out:&mut Vec<Vec<Pair>>){if let Some(r)=&entry.recursive{for child in r.descendants.entries().iter().rev(){path.push(child.key);walk(child,path,out);path.pop();}}out.push(path.clone());}
 let mut out=vec![];walk(entry,&mut vec![entry.key],&mut out);out
}
pub(super) struct Previous{pub id:usize,pub location:Location,pub far:Option<FarRegistration>}
/// Commit only births/removals; retained real hull listeners stay in place.
pub(super) fn reconcile(id:usize,root:&Location,entry:&Entry<'_>,created:Created,previous:&[Previous],locations:&mut BTreeMap<usize,Location>,far:&mut BTreeMap<usize,f32>)->Result<bounds::DistanceAction,&'static str>{
 if created.id!=id{return Err("recursive callback replaced parent identity");}
 let mut expected=vec![];descendants(entry,&mut vec![],&mut expected);if expected.len()!=created.descendants.len(){return Err("recursive callback omitted descendant identities");}
 let path_key=|path:&[Pair]|path.iter().map(|p|(p.slots,p.ledges)).collect::<Vec<_>>();
 let old_ids=previous.iter().map(|p|(p.id,p)).collect::<BTreeMap<_,_>>();let old_paths=previous.iter().map(|p|(path_key(&p.location.path),p.id)).collect::<BTreeMap<_,_>>();
 if old_ids.len()!=previous.len()||old_paths.len()!=previous.len()||previous.len()!=locations.values().filter(|l|l.broad==root.broad&&l.path.starts_with(&root.path)).count(){return Err("recursive callback has incomplete previous ownership");}
 for p in previous{let l=locations.get(&p.id).ok_or("recursive previous identity disappeared")?;if l.broad!=p.location.broad||l.path!=p.location.path{return Err("recursive previous location differs");}}
 let parent_keys=created.far.map(|(keys,_)|keys);
 let mut rows=vec![Descendant{id,path:vec![],far:created.far}];rows.extend(created.descendants);let mut ids=std::collections::BTreeSet::new();let mut paths=std::collections::BTreeSet::new();
 for row in &rows{
  if !ids.insert(row.id)||!paths.insert(path_key(&row.path)){return Err("recursive callback has duplicate or invalid bindings");}
  let child=relative(entry,&row.path)?;if child.key.slots!=entry.key.slots{return Err("recursive callback changed endpoint order");}
  let mut path=root.path.clone();path.extend_from_slice(&row.path);
  if let Some(old)=locations.get(&row.id){if old.broad!=root.broad||old.path!=path{return Err("recursive callback reused another location identity");}}
  if old_paths.get(&path_key(&path)).is_some_and(|&other|other!=row.id){return Err("recursive callback replaced a retained path identity");}
  if let Some((keys,delta))=row.far{if keys.iter().any(|v|!v.is_finite())||!delta.is_finite(){return Err("recursive callback returned nonfinite far state");}}
  if row.id!=id{if let Some(old)=old_ids.get(&row.id){let bits=|f:Option<FarRegistration>|f.map(|(k,d)|(k.map(f32::to_bits),d.to_bits()));if bits(row.far)!=bits(old.far){return Err("recursive callback changed a retained descendant listener");}}}
 }
 let mut edits=vec![];
 // Constructors run before the removed suffix's destructors.
 for row in &rows{if !old_ids.contains_key(&row.id){if let Some((keys,_))=row.far{edits.push(bounds::DistanceEdit::Register{pair:row.id,objects:entry.key.slots,keys});}}}
 for old in previous{if !ids.contains(&old.id){if old.far.is_some(){edits.push(bounds::DistanceEdit::Clear{pair:old.id});}locations.remove(&old.id);far.remove(&old.id);}}
 for row in rows{let child=relative(entry,&row.path)?;let mut path=root.path.clone();path.extend(row.path);locations.insert(row.id,Location{broad:root.broad,key:child.key,path});if let Some((_,delta))=row.far{far.insert(row.id,delta);}else{far.remove(&row.id);}}
 Ok(bounds::DistanceAction::Hierarchy{edits,keys:parent_keys})
}
impl Created{
 /// Read every far registration before releasing auxiliary hull ownership.
 /// Returned paths omit the root and retain constructor order, which need not
 /// be the final descendant vector order. No graph/queue identity is replaced.
 pub(crate) fn from_tree(owner:&mut Ownership,id:usize,entry:&Entry<'_>,bindings:&[Binding])->Result<Self,&'static str>{
  let mut paths=vec![];descendants(entry,&mut vec![],&mut paths);
  if paths.len()!=bindings.len(){return Err("constructed hierarchy has missing ownership bindings");}
  let mut ids=std::collections::BTreeSet::from([id]);let mut seen=vec![];
  let mut records=vec![(id,vec![])];
  for binding in bindings{
   if binding.path.first()!=Some(&entry.key)||binding.path.len()<2||!ids.insert(binding.id){return Err("constructed hierarchy has invalid or duplicate identity");}
   let path=binding.path[1..].to_vec();
   if !paths.contains(&path)||seen.contains(&path){return Err("constructed hierarchy has invalid or duplicate path");}
   seen.push(path.clone());records.push((binding.id,path));
  }
  let far=records.iter().map(|(id,path)|{
   let e=relative(entry,path)?;
   if owner.graph.objects(*id)?!=e.key.slots||owner.graph.flags(*id)?!=e.contact.flags{return Err("constructed hierarchy differs from graph ownership");}
   owner.far(*id).map(|r|{
    if r.objects!=e.key.slots{return Err("constructed hierarchy far objects differ");}
    let keys=std::array::from_fn::<_,2,_>(|i|{
     let (token,key)=owner.hulls.get(r.objects[i]).and_then(|h|h.entry(r.handles[i])).ok_or("constructed hierarchy lost hull listener")?;
     if token!=id.checked_mul(2).and_then(|n|n.checked_add(i)).ok_or("constructed hull token overflow")?||!key.is_finite()||!r.linear_delta.is_finite(){return Err("constructed hierarchy hull identity or key differs");}
     Ok(key)
    });let [a,b]=keys;
    Ok(([a?,b?],r.linear_delta))
   }).transpose()
  }).collect::<Result<Vec<_>,_>>()?;
  for ((id,_),registration) in records.iter().zip(&far){if registration.is_some(){owner.release_far(*id)?;}}
  Ok(Self{id,far:far[0],descendants:records.into_iter().zip(far).skip(1).map(|((id,path),far)|Descendant{id,path,far}).collect()})
 }
}

/// Validate the complete batch before publishing any identity, delta or edit.
pub(super) fn publish(broad:usize,entry:&Entry<'_>,created:Created,locations:&mut BTreeMap<usize,Location>,far:&mut BTreeMap<usize,f32>,edits:&mut Vec<bounds::DistanceEdit>)->Result<(),&'static str>{
 let mut expected=vec![];descendants(entry,&mut vec![],&mut expected);
 if expected.len()!=created.descendants.len(){return Err("created hierarchy is incompletely bound");}
 let mut rows=vec![Descendant{id:created.id,path:vec![],far:created.far}];rows.extend(created.descendants);
 let mut ids=std::collections::BTreeSet::new();let mut paths=vec![];
 for row in &rows{
  if locations.contains_key(&row.id)||far.contains_key(&row.id)||!ids.insert(row.id){return Err("compact lifecycle reused live child identity");}
  if paths.contains(&row.path)||(!row.path.is_empty()&&!expected.contains(&row.path)){return Err("compact lifecycle returned duplicate or invalid descendant path");}
  let child=relative(entry,&row.path)?;
  if child.key.slots!=entry.key.slots{return Err("compact descendant object order differs");}
  if let Some((keys,delta))=row.far{if keys.iter().any(|v|!v.is_finite())||!delta.is_finite(){return Err("compact lifecycle returned nonfinite far registration");}}
  let mut path=vec![entry.key];path.extend_from_slice(&row.path);
  if locations.values().any(|l|l.broad==broad&&l.path==path){return Err("compact lifecycle reused live descendant path");}
  paths.push(row.path.clone());
 }
 for row in rows{
  let child=relative(entry,&row.path)?;let key=child.key;
  let mut path=vec![entry.key];path.extend(row.path);
  locations.insert(row.id,Location{broad,key,path});
  if let Some((keys,delta))=row.far{far.insert(row.id,delta);edits.push(bounds::DistanceEdit::Register{pair:row.id,objects:key.slots,keys});}
 }Ok(())
}

/// Capture destructor order before the lifecycle provider clears descendants.
pub(super) fn retire(broad:usize,entry:&mut Entry<'_>,lifecycle:&mut dyn Lifecycle,locations:&mut BTreeMap<usize,Location>,far:&mut BTreeMap<usize,f32>,edits:&mut Vec<bounds::DistanceEdit>)->Result<(),&'static str>{
 let paths=retirement_paths(entry);
 let records=paths.iter().map(|path|locations.iter().find_map(|(&id,l)|(l.broad==broad&&l.path==*path).then_some(id)).ok_or("retiring compact hierarchy lacks identity")).collect::<Result<Vec<_>,_>>()?;
 let owned=locations.values().filter(|l|l.broad==broad&&l.path.starts_with(&[entry.key])).count();
 if owned!=records.len(){return Err("retiring compact hierarchy has stale descendant identities");}
 let bindings=records.iter().zip(&paths).take(records.len()-1).map(|(&id,path)|Binding{id,path:path.clone()}).collect::<Vec<_>>();
 lifecycle.retired(*records.last().ok_or("empty compact retirement")?,entry,&bindings)?;
 for id in records{locations.remove(&id);if far.remove(&id).is_some(){edits.push(bounds::DistanceEdit::Clear{pair:id});}}
 Ok(())
}

#[cfg(test)]
pub(crate) fn assert_live_parent_callback(owner:&Ownership,id:usize,entry:&Entry<'_>,bindings:&[Binding],context:&super::super::recursion::construction::Context<'_>,threshold:f32){
 assert_live_parent_sequence(owner,id,entry,bindings,context,threshold,&[(*context.bodies,context.clock)]);
}

#[cfg(test)]
pub(crate) fn assert_live_parent_sequence(owner:&Ownership,id:usize,entry:&Entry<'_>,bindings:&[Binding],context:&super::super::recursion::construction::Context<'_>,threshold:f32,phases:&[([Body;2],Clock)]){
 assert_live_distance_sequence(owner,id,entry,bindings,context,threshold,phases,None);
}

#[cfg(test)]
pub(crate) fn assert_live_far_recheck(owner:&Ownership,id:usize,entry:&Entry<'_>,bindings:&[Binding],context:&super::super::recursion::construction::Context<'_>,gap:f32){
 assert_live_distance_sequence(owner,id,entry,bindings,context,0.,&[(*context.bodies,context.clock)],Some(gap));
}

#[cfg(test)]
fn assert_live_distance_sequence(owner:&Ownership,id:usize,entry:&Entry<'_>,bindings:&[Binding],context:&super::super::recursion::construction::Context<'_>,threshold:f32,phases:&[([Body;2],Clock)],far_gap:Option<f32>){
 use super::super::{CounterSnapshot,recursion::construction};
 use crate::ivp_joint_apply::tests::snapshot;
 let solids=entry.recursive.as_ref().unwrap().descendants.solids;
 for side in 0..2{
  let mut map=CounterSnapshot::default();let mut expected=entry.snapshot_with_counters(&mut map);let expected_counter=map.counter(&context.root);
  let mut expected_owner=owner.try_clone_unbound().unwrap();let mut expected_bindings=bindings.to_vec();let mut expected_cache=context.cache.clone();
  let mut candidate=owner.try_clone_unbound().unwrap();let root=entry.snapshot_with_counters(&mut CounterSnapshot::default());let budgets=entry.key.slots.map(|o|owner.hulls[o].budget);
  let created=Created::from_tree(&mut candidate,id,&root,bindings).unwrap();let mut bank=*context.bodies;let mut cache=context.cache.clone();
  let definitions=(0..2).map(|i|Object{id:entry.key.slots[i],core:i,solid:solids[i],margin:context.margins[i],core_radius:context.timing.properties[i].radius,core_angular_radius:context.timing.properties[i].angular_bound,cached_motion:context.motion[i]}).collect();
  let mut d=Discovery::new(definitions,&entry.key.slots,&bank,context.timing.dt).unwrap();
  for i in 0..2{d.motion.insert(entry.key.slots[i],context.timing.properties[i]);d.world.callback_budget_inputs(entry.key.slots[i],budgets[i]);}
  let broad=123;let mut edits=vec![];publish(broad,&root,created,&mut d.locations,&mut d.far,&mut edits).unwrap();
  for edit in edits{match edit{bounds::DistanceEdit::Register{pair,objects,keys}=>d.world.register_distance(pair,objects,keys).unwrap(),_=>unreachable!()}}
  let before=entry.key.slots.map(|o|d.world.callback_hull_state(o).0);
  let mut forest=Contacts::new(entry.key.slots,solids).unwrap();forest.entries.push(root);d.coarse.insert(broad,forest);
  for (inputs,clock) in phases{
  bank=*inputs;let mut expected_bank=*inputs;
  for object in entry.key.slots{cache.detach(object).unwrap();expected_cache.detach(object).unwrap();}
  let mut expected_context=construction::Context{cache:&mut expected_cache,bodies:&mut expected_bank,clock:*clock,timing:context.timing,gravity:context.gravity,margins:context.margins,motion:context.motion,root:expected_counter.clone(),limit:context.limit,fallback:context.fallback};
  let activation_request=if let Some(gap)=far_gap{matches!(construction::recheck_parent(&mut expected_owner,id,&mut expected,std::array::from_fn(|i|solids[i].compact_ledge(entry.key.ledges[i]).unwrap()),&mut expected_bindings,&mut expected_context,gap).unwrap(),construction::FarRecheck::Activated)}else{construction::refresh_parent(&mut expected_owner,id,&mut expected,std::array::from_fn(|i|solids[i].compact_ledge(entry.key.ledges[i]).unwrap()),&mut expected_bindings,&mut expected_context,threshold).unwrap();expected_owner.far(id).is_none()};
  let mut queue=candidate.queue.clone();let mut lifecycle=super::owner::native_lifecycle(&mut candidate,&mut queue,*clock,context.timing,context.gravity);
  let requests={let mut dispatch=Dispatch{dt:d.dt,motion:&d.motion,far:&mut d.far,objects:&d.objects,coarse:&mut d.coarse,locations:&mut d.locations,bodies:&mut bank,cache:&mut cache,clock:*clock,lifecycle:&mut lifecycle};d.world.recheck_distance_with_gap_for_test(id,side,far_gap,&mut dispatch).unwrap()};drop(lifecycle);candidate.queue=queue;
  assert_eq!(serde_json::json!(bank.map(snapshot)),serde_json::json!(expected_bank.map(snapshot)),"live recursive body bank side{side}");assert_eq!(candidate.queue.snapshot(),expected_owner.queue.snapshot());assert_eq!(candidate.graph.active(),expected_owner.graph.active());assert_eq!(candidate.graph.inactive(),expected_owner.graph.inactive());
  assert_eq!(d.locations.len(),expected_bindings.len()+1);assert_eq!(format!("{:?}",d.entry_mut(id).unwrap().0.contact),format!("{:?}",expected.contact));
  for b in &expected_bindings{let actual=d.entry_mut(b.id).unwrap().0;let e=expected.recursive.as_ref().unwrap().descendants.entry_at_path(&b.path[1..]).unwrap();assert_eq!(format!("{:?}",actual.contact),format!("{:?}",e.contact));if let (Some(a),Some(b))=(&actual.recursive,&e.recursive){assert_eq!(a.count.load(std::sync::atomic::Ordering::Relaxed),b.count.load(std::sync::atomic::Ordering::Relaxed));assert_eq!(a.split,b.split);}}
  let mut expected_far=0;
  for child in std::iter::once(id).chain(expected_bindings.iter().map(|b|b.id)){
   if let Some(r)=expected_owner.far(child){expected_far+=1;assert_eq!(d.far[&child].to_bits(),r.linear_delta.to_bits());let keys=std::array::from_fn::<_,2,_>(|i|expected_owner.hulls[r.objects[i]].entry(r.handles[i]).unwrap().1.to_bits());assert_eq!(d.world.distance_keys(child).unwrap().map(f32::to_bits),keys);}else{assert!(!d.far.contains_key(&child));assert!(d.world.distance_keys(child).is_err());}
   assert!(candidate.far(child).is_none());
  }
  assert_eq!(d.far.len(),expected_far);
  for i in 0..2{let (broad_key,order)=d.world.callback_hull_state(entry.key.slots[i]);assert_eq!(broad_key,before[i]);assert_eq!(order,expected_owner.hulls[entry.key.slots[i]].ordered_keys().iter().map(|(_,token,_)|token/2).collect::<Vec<_>>(),"live recursive hull order side{side} object{i}");}
  assert_eq!(requests.iter().filter(|r|matches!(r,bounds::Request::Distance{pair,..} if *pair==id)).count(),usize::from(activation_request));assert!(cache.metadata().0.iter().all(|s|s.references==0));
  }
 }
}

#[cfg(test)]
pub(crate) fn assert_native_tree_handoff(owner:&Ownership,id:usize,entry:&Entry<'_>,bindings:&[Binding],bodies:&[Body;2]){
 use crate::ivp_surface_admission::contacts::{CounterSnapshot,recursion::construction};
 use crate::ivp_trial_bounds::Hooks;
 struct Destructor<'a>{owner:&'a mut Ownership,bindings:&'a [Binding]}
 impl Lifecycle for Destructor<'_>{
  fn created(&mut self,_:&mut Entry<'_>,_:[&Convex;2],_:CreationInput,_:&mut[Body;2],_:&mut WorldCache)->Result<Created,&'static str>{Err("unexpected construction during removal")}
  fn retired(&mut self,id:usize,entry:&mut Entry<'_>,bindings:&[Binding])->Result<(),&'static str>{
   assert_eq!(bindings.len(),self.bindings.len());for b in bindings{assert!(self.bindings.iter().any(|old|old.id==b.id&&old.path==b.path));}
   construction::retire_tree(self.owner,id,entry,bindings)
  }
 }
 let solids=entry.recursive.as_ref().unwrap().descendants.solids;
 let mut candidate=owner.try_clone_unbound().unwrap();let root=entry.snapshot_with_counters(&mut CounterSnapshot::default());
 let old_far=std::iter::once(id).chain(bindings.iter().map(|b|b.id)).filter_map(|id|candidate.far(id).map(|f|(id,f.objects,std::array::from_fn::<_,2,_>(|i|candidate.hulls[f.objects[i]].entry(f.handles[i]).unwrap().1.to_bits()),f.linear_delta.to_bits()))).collect::<Vec<_>>();
 let before=format!("{candidate:?}");
 if !bindings.is_empty(){assert!(Created::from_tree(&mut candidate,id,&root,&bindings[..bindings.len()-1]).is_err());assert_eq!(format!("{candidate:?}"),before);}
 let created=Created::from_tree(&mut candidate,id,&root,bindings).unwrap();
 assert!(std::iter::once(id).chain(bindings.iter().map(|b|b.id)).all(|id|candidate.far(id).is_none()));
 let definitions=(0..2).map(|core|Object{id:entry.key.slots[core],core,solid:solids[core],margin:0.,core_radius:1.,core_angular_radius:0.,cached_motion:[0.;2]}).collect();
 let mut d=Discovery::new(definitions,&entry.key.slots,bodies,1./60.).unwrap();let broad=123;let mut edits=vec![];
 if !created.descendants.is_empty(){
  let mut bad=created.clone();bad.descendants.last_mut().unwrap().id=id;
  assert!(publish(broad,&root,bad,&mut d.locations,&mut d.far,&mut edits).is_err());
  assert!(d.locations.is_empty()&&d.far.is_empty()&&edits.is_empty());
  let mut bad=created.clone();bad.descendants.last_mut().unwrap().path.clear();
  assert!(publish(broad,&root,bad,&mut d.locations,&mut d.far,&mut edits).is_err());
  assert!(d.locations.is_empty()&&d.far.is_empty()&&edits.is_empty());
 }
 publish(broad,&root,created,&mut d.locations,&mut d.far,&mut edits).unwrap();
 assert_eq!(d.locations.len(),bindings.len()+1);assert_eq!(d.far.len(),old_far.len());
 for edit in edits{match edit{bounds::DistanceEdit::Register{pair,objects,keys}=>d.world.register_distance(pair,objects,keys).unwrap(),_=>panic!("unexpected creation clear")}}
 for &(id,_,keys,delta) in &old_far{assert_eq!(d.world.distance_keys(id).unwrap().map(f32::to_bits),keys);assert_eq!(d.far[&id].to_bits(),delta);}
 let mut forest=Contacts::new(entry.key.slots,solids).unwrap();forest.entries.push(root);d.coarse.insert(broad,forest);
 for binding in bindings{assert_eq!(d.entry_mut(binding.id).unwrap().0.key,*binding.path.last().unwrap());}
 let mut bank=*bodies;let mut cache=WorldCache::new(8).unwrap();let mut lifecycle=Destructor{owner:&mut candidate,bindings};
 let edits={let mut dispatch=Dispatch{dt:d.dt,motion:&d.motion,far:&mut d.far,objects:&d.objects,coarse:&mut d.coarse,locations:&mut d.locations,bodies:&mut bank,cache:&mut cache,clock:Clock{time:0.,frame:0},lifecycle:&mut lifecycle};dispatch.removed(broad,entry.key.slots).unwrap()};
 assert_eq!(edits.len(),old_far.len());for edit in edits{match edit{bounds::DistanceEdit::Clear{pair}=>d.world.clear_distance(pair).unwrap(),_=>panic!("unexpected retirement register")}}
 assert!(d.locations.is_empty()&&d.far.is_empty()&&d.coarse.is_empty());
 for &(id,_,_,_) in &old_far{assert!(d.world.distance_keys(id).is_err());}
 for id in std::iter::once(id).chain(bindings.iter().map(|b|b.id)){assert!(candidate.graph.objects(id).is_err());}
}
