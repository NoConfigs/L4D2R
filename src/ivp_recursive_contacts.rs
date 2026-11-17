//! Native10121950 split selection and10121510 recursive admission requests.
//! Graph transitions/provider callbacks remain with the owning world.
use super::*;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct Plan{pub split:usize,pub provided:[Option<usize>;2],pub restricted:[Option<usize>;2]}
pub(crate) fn plan(coarse:[bool;2],radii:[Option<f32>;2],ledges:[usize;2],count:i32,limit:i32,fallback:f32)->Result<Option<Plan>,&'static str>{
 if count>limit{return Ok(None);}
 if !coarse.iter().any(|v|*v){return Err("recursive contact has no coarse endpoint");}
 let radii=radii.map(|v|v.unwrap_or(fallback));
 let split=if !coarse[0]{1}else if !coarse[1]{0}else if radii[0]<=radii[1]{1}else{0};
 let mut provided=[None;2];let mut restricted=[None;2];provided[1-split]=Some(ledges[1-split]);restricted[split]=Some(ledges[split]);Ok(Some(Plan{split,provided,restricted}))
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum ContactPlan{Physical,Expand(Plan)}
pub(crate) struct ContactInput{pub kinds:[i16;2],pub edges:[u32;2],pub triangles:[u32;2],pub radii:[Option<f32>;2],pub ledges:[usize;2],pub count:i32,pub limit:i32,pub fallback:f32}
///10121750 reads different markers for a point against an edge and an edge
/// against a point. Those cases cannot be replaced by a coarse-ledger test.
pub(crate) fn contact_plan(i:ContactInput)->Result<ContactPlan,&'static str>{
 if i.count>i.limit{return Ok(ContactPlan::Physical);}
 let marked=|v:u32|v&0x80000000!=0;
 let split=match i.kinds{
  [0|3,0|3]=>None,
  [0|3,1]=>marked(i.edges[1]).then_some(1),
  [0|3,2]=>marked(i.triangles[1]).then_some(1),
  [1,0|3]=>marked(i.triangles[0]).then_some(0),
  [1,1]=>if marked(i.edges[1]){if marked(i.edges[0])&&i.radii[1].unwrap_or(i.fallback)<i.radii[0].unwrap_or(i.fallback){Some(0)}else{Some(1)}}else{marked(i.edges[0]).then_some(0)},
  [2,_]=>marked(i.triangles[0]).then_some(0),
  _=>return Err("invalid native recursive near-contact dispatch feature combination"),
 };
 Ok(match split{None=>ContactPlan::Physical,Some(split)=>{let mut provided=[None;2];let mut restricted=[None;2];provided[1-split]=Some(i.ledges[1-split]);restricted[split]=Some(i.ledges[split]);ContactPlan::Expand(Plan{split,provided,restricted})}})
}
pub(crate) enum ContactOutcome{Physical,Expanded(Changes)}
impl<'a> Entry<'a>{
 pub(crate) fn expand(&mut self,input:Input,root:&Counter,limit:i32,fallback:f32,initialize:impl FnMut(&Pair,&mut Entry<'a>,[&Convex;2])->Result<(),&'static str>,retire:impl FnMut(&Pair,&mut Entry<'a>)->Result<(),&'static str>)->Result<Option<Changes>,&'static str>{
    let recursive=self.recursive.as_mut().ok_or("leaf contact cannot expand recursively")?;
    let trees=recursive.descendants.solids.map(|s|s.compact_tree.as_deref().ok_or("missing recursive surface tree"));let trees=[trees[0]?,trees[1]?];
    let coarse=std::array::from_fn::<_,2,_>(|j|trees[j].ledge_headers.get(&self.key.ledges[j]).map(|h|h[8]&3!=0).ok_or("missing recursive parent ledge"));let coarse=[coarse[0]?,coarse[1]?];
    let radii=std::array::from_fn(|j|trees[j].ledge_nodes.get(&self.key.ledges[j]).and_then(|n|trees[j].nodes.get(*n)).map(|n|n.radius));
    let global=root.load(Ordering::Relaxed);let effective=if global>0{global}else{recursive.count.load(Ordering::Relaxed)};
    let Some(plan)=plan(coarse,radii,self.key.ledges,effective,limit,fallback)?else{return Ok(None);};
    self.apply_plan(input,root,plan,initialize,retire).map(Some)
 }
 fn apply_plan(&mut self,mut input:Input,root:&Counter,plan:Plan,initialize:impl FnMut(&Pair,&mut Entry<'a>,[&Convex;2])->Result<(),&'static str>,retire:impl FnMut(&Pair,&mut Entry<'a>)->Result<(),&'static str>)->Result<Changes,&'static str>{
    let recursive=self.recursive.as_mut().ok_or("leaf contact cannot expand recursively")?;
    recursive.split=Some(plan.split);
    if !recursive.descendants.listeners.iter().any(|p|Arc::ptr_eq(p,root)){recursive.descendants.listeners.push(root.clone());}
    input.provided=plan.provided;input.restricted=plan.restricted;
    self.contact.flags=(self.contact.flags&0xffd3ffff)|0x100000;
    recursive.descendants.update(input,initialize,retire)
 }
 pub(crate) fn dispatch_contact(&mut self,input:Input,root:&Counter,limit:i32,fallback:f32,physical:impl FnOnce(&mut Entry<'a>)->Result<(),&'static str>,initialize:impl FnMut(&Pair,&mut Entry<'a>,[&Convex;2])->Result<(),&'static str>,retire:impl FnMut(&Pair,&mut Entry<'a>)->Result<(),&'static str>)->Result<ContactOutcome,&'static str>{
    let r=self.recursive.as_ref().ok_or("recursive near-contact dispatch requires a recursive owner")?;
    let edges=[self.contact.query.vertex,self.contact.query.face];
    let mut words=[0u32;2];let mut triangles=[0u32;2];let mut radii=[None;2];
    let global=root.load(Ordering::Relaxed);let count=if global>0{global}else{r.count.load(Ordering::Relaxed)};
    if count<=limit{for j in 0..2{
      let solid=r.descendants.solids[j];let convex=solid.compact_ledge(self.key.ledges[j]).ok_or("missing recursive near-contact dispatch ledge")?;let topology=convex.topology.as_deref().ok_or("missing contact topology")?;let edge=edges[j];
      words[j]=topology.edges.get(edge.triangle).and_then(|t|t.get(edge.edge)).ok_or("invalid contact edge")?.0;
      triangles[j]=convex.triangle_headers.get(edge.triangle).ok_or("invalid contact triangle")?.0;
      let tree=solid.compact_tree.as_deref().ok_or("missing contact surface tree")?;radii[j]=tree.ledge_nodes.get(&self.key.ledges[j]).and_then(|n|tree.nodes.get(*n)).map(|n|n.radius);
    }}
    let plan=contact_plan(ContactInput{kinds:self.contact.query.kinds,edges:words,triangles,radii,ledges:self.key.ledges,count,limit,fallback})?;
    match plan{ContactPlan::Physical=>{physical(self)?;Ok(ContactOutcome::Physical)},ContactPlan::Expand(plan)=>self.apply_plan(input,root,plan,initialize,retire).map(ContactOutcome::Expanded)}
 }

}
#[cfg(test)]
#[path="ivp_recursive_contacts_tests.rs"]
mod tests;
