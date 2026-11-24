//! Contact-local compact refresh/cache/default recovery ownership. Coarse
//! discovery, graph membership, queue scheduling and external vetoes are separate.
use super::*;
use crate::{ivp_joint_apply::Body,ivp_world_cache::{WorldCache,Context}};
use l4d2_formats::phy::TriangleHeader;
#[path="owner_schedule.rs"]
mod scheduling;
pub(crate) use scheduling::{Properties,Schedule};
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct Contact{pub query:Query,pub flags:u32,objects:[usize;2],margin:f32}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum RecoveryKind{Dynamic,Fixed}
#[derive(Debug,PartialEq,Eq)]
pub(crate) struct Update{pub outcome:Refresh,pub recovery:Option<RecoveryKind>,pub callback_references:Option<[u32;2]>}
impl Contact{
 pub(crate) fn new(objects:[usize;2],flags:u32,margin:f32)->Result<Self,&'static str>{
    if objects[0]==objects[1]||!margin.is_finite(){return Err("invalid compact contact binding");}
    Ok(Self{query:Query::compact_pair(),flags,objects,margin})
 }
 pub(crate) fn margin(&self)->f32{self.margin}
 /// Shared1010dcd0 query/cache phase. Recursive slot5/10121340 returns
 /// without ordinary recovery; the entry owner chooses its notification policy.
 pub(crate) fn refresh_geometry(&mut self,cache:&mut WorldCache,clock:Context,bodies:&[Body;2],hulls:[&CompactTopology;2],headers:[&[TriangleHeader];2])->Result<Update,&'static str>{
    self.refresh_query(cache,clock,bodies,hulls,headers,false)
 }
 ///1010db00's zero-budget geometry phase without ordinary recovery. Recursive
 /// parent callbacks use it even though their flags are not pending-class.
 pub(crate) fn refresh_geometry_zero_budget(&mut self,cache:&mut WorldCache,clock:Context,bodies:&[Body;2],hulls:[&CompactTopology;2],headers:[&[TriangleHeader];2])->Result<Update,&'static str>{
    self.refresh_query(cache,clock,bodies,hulls,headers,true)
 }
 pub(crate) fn refresh_pending(&mut self,cache:&mut WorldCache,clock:Context,bodies:&[Body;2],hulls:[&CompactTopology;2],headers:[&[TriangleHeader];2])->Result<Update,&'static str>{
    if self.flags&0x3000!=0x1000{return Err("pending query requires pending contact flags");}
    self.refresh_query(cache,clock,bodies,hulls,headers,true)
 }
 fn refresh_query(&mut self,cache:&mut WorldCache,clock:Context,bodies:&[Body;2],hulls:[&CompactTopology;2],headers:[&[TriangleHeader];2],zero_budget:bool)->Result<Update,&'static str>{
    if self.query.frame==clock.frame{return Ok(Update{outcome:Refresh::Reused,recovery:None,callback_references:None});}
    for i in 0..2{if cache.object_core(self.objects[i])?!=bodies[i].id{return Err("compact contact cache/core binding differs");}}
    for i in 0..2{cache.update_core(bodies[i].id,bodies[i].prediction)?;cache.set_flags(self.objects[i],bodies[i].object_flags)?;}
    let leases=cache.retain_pair(self.objects,clock)?;
    let frames=cache.lease_frame(&leases.0).and_then(|a|cache.lease_frame(&leases.1).map(|b|[a,b]));
    let outcome=cache.with_leases(leases,|release|{let frames=frames?;let result=if zero_budget{self.query.refresh_zero_budget(clock.frame,hulls,frames,self.margin,headers,&mut self.flags)}else{self.query.refresh_contact(clock.frame,hulls,frames,self.margin,headers,&mut self.flags)};release();result})?;
    let mut update=Update{outcome,recovery:None,callback_references:None};
    if outcome!= (Refresh::Retirement{notify_owner:true}){return Ok(update);}
    let (slots,objects,_)=cache.metadata();update.callback_references=Some(self.objects.map(|id|objects[id].cache.map_or(0,|slot|slots[slot].references)));
    Ok(update)
 }
 pub(crate) fn refresh(&mut self,cache:&mut WorldCache,clock:Context,bodies:&mut[Body;2],hulls:[&CompactTopology;2],headers:[&[TriangleHeader];2],dt:f32,gravity:[f32;3])->Result<Update,&'static str>{
    let update=self.refresh_geometry(cache,clock,bodies,hulls,headers)?;
    self.default_response(update,cache,clock,bodies,hulls,dt,gravity)
 }
 pub(crate) fn refresh_inactive(&mut self,cache:&mut WorldCache,clock:Context,bodies:&mut[Body;2],hulls:[&CompactTopology;2],headers:[&[TriangleHeader];2],dt:f32,gravity:[f32;3])->Result<Update,&'static str>{
    let update=self.refresh_query(cache,clock,bodies,hulls,headers,true)?;
    self.default_response(update,cache,clock,bodies,hulls,dt,gravity)
 }
 fn default_response(&mut self,mut update:Update,cache:&mut WorldCache,clock:Context,bodies:&mut[Body;2],hulls:[&CompactTopology;2],dt:f32,gravity:[f32;3])->Result<Update,&'static str>{
    if update.outcome!=(Refresh::Retirement{notify_owner:true}){return Ok(update);}
    //101003e0 chooses the second endpoint when its core is fixed, otherwise
    // the first. A sphere feature (kind3) takes the dynamic response even
    // against a fixed core. Failures retaining kind5 likewise take that path.
    let fixed=usize::from(bodies[1].flags&2!=0);
    if bodies[fixed].flags&2==0||self.query.kinds[fixed]>2{
        rescue::apply_dynamic(bodies,dt,gravity)?;update.recovery=Some(RecoveryKind::Dynamic);
    }else{
        let moving=1-fixed;
        // A sleeping mover returns before the fixed object's cache lookup.
        if bodies[moving].flags>>8<8{
            let slot=cache.access(self.objects[fixed],clock)?;let pose=cache.get(slot)?.pose;
            let b=bodies[moving];let input=crate::ivp_joint_tail::Body{id:b.id,flags:b.flags,mass:b.mass,position:b.current.position.to_array()};
            let r=rescue::fixed(hulls[fixed],pose,input,Vec3::from_array(b.pending_linear[..3].try_into().unwrap()),b.prediction.velocity,dt,gravity)?;
            if let Some(delta)=r.escape.and_then(|e|e.push){bodies[moving].push_velocity(delta.to_array())?;}
        }
        update.recovery=Some(RecoveryKind::Fixed);
    }Ok(update)
 }
}
#[cfg(test)]
#[path="recursive_query_owner_tests.rs"]
mod recursive_query_tests;
#[cfg(test)]
mod tests{
 use super::*;
 use l4d2_formats::phy::CompactEdge;
 use crate::ivp_joint_apply::tests::{body,snapshot};
 #[test]
 fn compact_owner_refresh_release_default_response_and_same_frame_reuse_match_native(){
    let d:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-compact-owner-initial.json")).unwrap();let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let mut counts=[0;4];
    for c in d["cases"].as_array().unwrap(){let mut bodies=std::array::from_fn(|i|body(&c["before"][i]));let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()}).collect();let headers:Vec<Vec<_>>=c["hulls"].as_array().unwrap().iter().map(|h|(0..4).map(|i|TriangleHeader(w(&h[4+i*4]))).collect()).collect();let h=[&hulls[0],&hulls[1]];let head=[headers[0].as_slice(),headers[1].as_slice()];
        let clock=Context{time:bodies[0].clock,frame:w(&c["frame"])};let mut cache=WorldCache::new(8).unwrap();
        for i in 0..2{let b=body(&c["initial"][i]);assert_eq!(cache.register_core(b.prediction),b.id);assert_eq!(cache.register_object(b.id,b.object_flags,crate::ivp_motion::ObjectFrame{shift:None,rotation:None}).unwrap(),b.id);}
        let initial=Context{time:f64::from_bits(c["initial_clock"].as_u64().unwrap()),frame:w(&c["initial_frame"])};for i in 0..2{cache.access(i,initial).unwrap();}
        for b in bodies{cache.update_core(b.id,b.prediction).unwrap();cache.set_flags(b.id,b.object_flags).unwrap();}
        for i in 0..2{let slot=cache.access(i,clock).unwrap();let p=cache.get(slot).unwrap();let n=&c["frames"][i];assert_eq!(p.core_position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|j|w(&n[j])),"case{} cache core{i}",c["id"]);assert_eq!(p.pose.position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|j|w(&n[20+j])),"case{} cache origin{i}",c["id"]);assert_eq!(p.pose.rotation.to_cols_array().map(f32::to_bits),std::array::from_fn::<_,9,_>(|j|w(&n[8+(j%3)*4+j/3])),"case{} cache rotation{i}",c["id"]);}
        let mut contact=Contact::new([0,1],w(&c["input_flags"]),0.02).unwrap();let gravity=std::array::from_fn(|i|f(&c["gravity"][i]));let update=contact.refresh(&mut cache,clock,&mut bodies,h,head,f(&c["dt"]),gravity).unwrap_or_else(|error|panic!("case{}: {error}",c["id"]));
        assert_eq!(contact.flags,w(&c["flags"]),"case{} flags",c["id"]);assert_eq!(contact.query.frame,w(&c["query_frame"]));
        let callback=update.callback_references.is_some();assert_eq!(usize::from(callback),w(&c["callback_count"]) as usize,"case{} callback",c["id"]);if callback{assert_eq!(update.callback_references,Some([0,0]));counts[2]+=1;}
        counts[usize::from(contact.query.retirement_requested())]+=1;
        for (i,e) in [contact.query.vertex,contact.query.face].into_iter().enumerate(){assert_eq!([e.triangle,e.edge,contact.query.kinds[i] as usize],std::array::from_fn(|j|w(&c["features"][i][j]) as usize),"case{} side{i}",c["id"]);}
        if let Some(g)=contact.query.geometry{let n=&c["geometry"];assert_eq!([g.projection.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()],[w(&n[0]),w(&n[2]),w(&n[3]),w(&n[4]),w(&n[5])],"case{} geometry",c["id"]);}else{assert_eq!(c["geometry"],serde_json::json!([0,0,0,0,0,0]));}
        assert_eq!(serde_json::json!(bodies.map(snapshot)),c["after"],"case{} bodies",c["id"]);counts[3]+=usize::from(c["before"]!=c["after"]);
        let retained=serde_json::json!(bodies.map(snapshot));let flags=contact.flags;let again=contact.refresh(&mut cache,clock,&mut bodies,h,head,f(&c["dt"]),gravity).unwrap();assert_eq!(again.outcome,Refresh::Reused);assert_eq!(again.recovery,None);assert_eq!(flags,contact.flags);assert_eq!(serde_json::json!(bodies.map(snapshot)),retained);assert_eq!(retained,c["reuse_after"]);assert!(cache.metadata().0.iter().all(|s|s.references==0));
    }assert_eq!(counts,[954,198,66,19]);assert_eq!(d["cases"].as_array().unwrap().len(),1152);
 }
}
