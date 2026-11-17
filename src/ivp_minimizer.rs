//! Native compact-feature descent, retained endpoint order and header-aware
//! penetration retry on borrowed prepared caches. Retirement/cycle ownership
//! and campaign collision/event integration remain separate.
use bevy::prelude::*;
use l4d2_formats::phy::{CompactTopology,CompactEdgeId};
use crate::ivp_motion::PreparedObjectCache;
#[path="ivp_minimizer/initialization.rs"]
mod initialization;
#[path="ivp_minimizer/rescue.rs"]
pub(crate) mod rescue;
#[path="ivp_minimizer/owner.rs"]
pub(crate) mod owner;
#[cfg(test)]
#[path="ivp_minimizer/outer_tests.rs"]
mod outer_tests;
#[derive(Clone,Copy,Debug)]
pub(crate) struct Geometry{pub distance:f32,pub projection:f32,pub normal:Vec3}
///1010ce30 sphere candidate queries call10113850 after100ed9a0 converts
/// the opposite core centre into object-local coordinates. Equality is inside.
pub(crate) fn sphere_query_contains(pose:crate::ivp_motion::Pose,other:Vec3,radius:f32)->Result<bool,&'static str>{
    if !pose.position.is_finite()||!pose.rotation.is_finite()||!other.is_finite()||!radius.is_finite()||radius<0.{return Err("invalid sphere candidate query");}
    let local=inverse(pose.rotation,other-pose.position);
    Ok((local.x*local.x+local.y*local.y)+local.z*local.z<=radius*radius)
}
///1010d7e0 sphere/sphere numeric query after the owner acquires both caches.
/// The reciprocal is spilled to float before distance and normal are stored.
pub(crate) fn sphere_geometry(positions:[Vec3;2],core_positions:[Vec3;2],margin:f32)->Result<Geometry,&'static str>{
    if !margin.is_finite()||positions.into_iter().chain(core_positions).any(|p|!p.is_finite()){return Err("invalid sphere closest geometry");}
    let delta=positions[0]-positions[1];let square=(delta.x*delta.x+delta.y*delta.y)+delta.z*delta.z;
    let inverse=if square.abs()>f32::from_bits(0x2edbe6ff){(1./(square as f64).sqrt()) as f32}else{1.};
    let normal=delta*inverse;let distance=inverse*square-margin;
    let projection=dot(normal,core_positions[0]-core_positions[1]);
    if !normal.is_finite()||!distance.is_finite()||!projection.is_finite(){return Err("sphere closest geometry overflow");}
    Ok(Geometry{distance,projection,normal})
}
#[derive(Clone,Copy)]
#[cfg_attr(test,derive(Debug))]
struct Recovery{owner:usize,point:Vec3}
struct Descent{remaining:i32,owners:[usize;2],seen:Vec<[(usize,usize,usize,i16);2]>}
#[cfg(test)]
#[derive(Debug,PartialEq,Eq)]
struct BudgetObservation{remaining:i32,seen:usize,features:[(usize,usize,usize,i16);2]}
#[cfg(test)]
thread_local!{static BUDGET_OBSERVATIONS:std::cell::RefCell<Vec<BudgetObservation>>=const{std::cell::RefCell::new(Vec::new())};}
#[cfg(test)]
fn take_budget_observations()->Vec<BudgetObservation>{BUDGET_OBSERVATIONS.with(|v|std::mem::take(&mut *v.borrow_mut()))}
#[cfg(test)]
#[inline(never)]
fn record_budget_observation(remaining:i32,seen:usize,features:[(usize,usize,usize,i16);2]){BUDGET_OBSERVATIONS.with(|v|v.borrow_mut().push(BudgetObservation{remaining,seen,features}));}
impl Descent{
    fn new(hulls:[&CompactTopology;2])->Self{Self{remaining:20,owners:[0,usize::from(!std::ptr::eq(hulls[0],hulls[1]))],seen:Vec::new()}}
    #[cfg_attr(test,inline(never))]
    fn enter(&mut self,roles:[usize;2],ids:[CompactEdgeId;2],kinds:[i16;2])->bool{
        self.remaining-=1;if self.remaining>=0{return true;}
        let mut key=std::array::from_fn(|i|(self.owners[roles[i]],ids[i].triangle,ids[i].edge,kinds[i]));
        #[cfg(test)]
        record_budget_observation(self.remaining,self.seen.len(),key);
        key.sort();
        if self.seen.contains(&key)||self.seen.len()>=256{return false;}self.seen.push(key);true
    }
    fn scoped<T>(&mut self,order:[usize;2],f:impl FnOnce(&mut Self)->T)->T{let saved=self.owners;self.owners=order.map(|i|saved[i]);let result=f(self);self.owners=saved;result}
}
#[derive(Clone,Copy)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct Query{retired:bool,order:usize,recovery:Option<Recovery>,pub frame:u32,pub kinds:[i16;2],pub vertex:CompactEdgeId,pub face:CompactEdgeId,pub geometry:Option<Geometry>}
///1010dcd0's query outcome. Owner notification is separate from the numeric
/// retirement state: lifecycle flags can suppress the virtual callback.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum Refresh{Reused,Ordinary,Retirement{notify_owner:bool}}
fn dot(a:Vec3,b:Vec3)->f32{(a.y*b.y+a.x*b.x)+a.z*b.z}
fn rotate(r:Mat3,v:Vec3)->Vec3{let rows=r.transpose();Vec3::new(dot(rows.x_axis,v),dot(rows.y_axis,v),dot(rows.z_axis,v))}
fn inverse(r:Mat3,v:Vec3)->Vec3{Vec3::new(dot(r.x_axis,v),dot(r.y_axis,v),dot(r.z_axis,v))}
fn vertex(t:&CompactTopology,id:CompactEdgeId)->Result<Vec3,&'static str>{t.vertex(id).map(Vec3::from_array).ok_or("missing minimizer compact vertex")}
fn previous(t:&CompactTopology,id:CompactEdgeId)->Result<CompactEdgeId,&'static str>{t.previous(id).ok_or("missing minimizer previous edge")}
fn across(t:&CompactTopology,id:CompactEdgeId)->Result<CompactEdgeId,&'static str>{t.opposite(id).ok_or("missing minimizer opposite edge")}
fn inside(point:Vec3,canonical:[Vec3;3])->bool{
    let [p,q,r]=canonical;let u=p-q;let v=r-q;let d=point-q;
    let xyz=|a:Vec3,b:Vec3|(a.x*b.x+a.y*b.y)+a.z*b.z;
    let uu=xyz(u,u);let vv=xyz(v,v);let uv=xyz(v,u);let ru=xyz(d,u);let rv=xyz(d,v);
    let det=vv*uu-uv*uv;let wc=rv*uu-ru*uv;let wa=ru*vv-rv*uv;
    [wc+f32::MIN_POSITIVE,((det-wa)-wc)+f32::MIN_POSITIVE,wa+f32::MIN_POSITIVE].iter().all(|v|!v.is_sign_negative())
}
impl Query{
    ///1010ce30/1010cc80 initialize compact ledges at their first edge. Native
    /// geometry is unwritten until the first query; None preserves that state.
    pub(crate) fn compact_pair()->Self{Self{retired:false,order:0,recovery:None,frame:0,kinds:[0,0],vertex:CompactEdgeId{triangle:0,edge:0},face:CompactEdgeId{triangle:0,edge:0},geometry:None}}
    pub(crate) fn refresh(&mut self,frame:u32,point_hull:&CompactTopology,face_hull:&CompactTopology,frames:[PreparedObjectCache;2],margin:f32)->Result<bool,&'static str>{
        self.refresh_outer(frame,[point_hull,face_hull],frames,margin,None)
    }
    pub(crate) fn refresh_with_headers(&mut self,frame:u32,hulls:[&CompactTopology;2],frames:[PreparedObjectCache;2],margin:f32,headers:[&[l4d2_formats::phy::TriangleHeader];2])->Result<bool,&'static str>{
        self.refresh_outer(frame,hulls,frames,margin,Some(headers))
    }
    /// Flag-aware outer boundary for contact owners. Preserves unrelated bits,
    /// clears stale failure fields on success and gates retirement notification
    /// with native lifecycle fields. The caller performs callback/cleanup after
    /// this returns; no owner destruction is implied by a failed query alone.
    pub(crate) fn refresh_contact(&mut self,frame:u32,hulls:[&CompactTopology;2],frames:[PreparedObjectCache;2],margin:f32,headers:[&[l4d2_formats::phy::TriangleHeader];2],flags:&mut u32)->Result<Refresh,&'static str>{
        if self.frame==frame{return Ok(Refresh::Reused);}
        let mut working=*self;working.order=((*flags>>8)&3) as usize;
        working.refresh_outer(frame,hulls,frames,margin,Some(headers))?;
        let updated=(*flags&!0xc100)|((working.order as u32)<<8)|if working.retired{0x4000}else{0};
        let outcome=if working.retired{Refresh::Retirement{notify_owner:updated&0x3000!=0x1000&&updated&0x3c0000!=0x100000}}else{Refresh::Ordinary};
        *self=working;*flags=updated;Ok(outcome)
    }
    ///1010db00 uses a zero-budget ledger and recovers a penetration seed once,
    /// without1010dcd0's second feature attempt. The pending owner supplies its
    /// own lifecycle policy after this query returns. Inactive object notification
    /// uses the same kernel with ordinary virtual recovery.
    pub(crate) fn refresh_zero_budget(&mut self,frame:u32,hulls:[&CompactTopology;2],frames:[PreparedObjectCache;2],margin:f32,headers:[&[l4d2_formats::phy::TriangleHeader];2],flags:&mut u32)->Result<Refresh,&'static str>{
        if self.frame==frame{return Ok(Refresh::Reused);}
        let mut working=*self;working.order=((*flags>>8)&3) as usize;
        if !margin.is_finite()||working.order>1{return Err("invalid pending minimizer input");}
        working.frame=frame;working.retired=false;working.recovery=None;
        let mut descent=Descent::new(hulls);descent.remaining=0;
        working.refresh_features(frame,hulls[0],hulls[1],frames,margin,&mut descent)?;
        if let Some(seed)=working.recovery{
            let edge=if seed.owner==0{working.vertex}else{working.face};
            let selected=initialization::penetration::recover_seed(hulls[seed.owner],headers[seed.owner],edge,seed.point)?;
            if seed.owner==0{working.vertex=selected;}else{working.face=selected;}
            working.kinds[seed.owner]=2;working.retired=true;working.recovery=None;
        }
        let updated=(*flags&!0xc100)|((working.order as u32)<<8)|if working.retired{0x4000}else{0};
        let outcome=if working.retired{Refresh::Retirement{notify_owner:updated&0x3000!=0x1000&&updated&0x3c0000!=0x100000}}else{Refresh::Ordinary};
        *self=working;*flags=updated;Ok(outcome)
    }
    fn refresh_outer(&mut self,frame:u32,hulls:[&CompactTopology;2],frames:[PreparedObjectCache;2],margin:f32,headers:Option<[&[l4d2_formats::phy::TriangleHeader];2]>)->Result<bool,&'static str>{
        if self.frame==frame{return Ok(false);}
        if !margin.is_finite()||self.order>1{return Err("invalid minimizer margin or preferred order");}
        let mut working=*self;working.frame=frame;working.retired=false;let mut descent=Descent::new(hulls);
        for _ in 0..2{
            working.recovery=None;
            working.refresh_features(frame,hulls[0],hulls[1],frames,margin,&mut descent)?;
            if working.retired{working.recovery=None;*self=working;return Ok(true);}
            let Some(seed)=working.recovery else{*self=working;return Ok(true);};
            let headers=headers.ok_or("native penetration recovery requires authored triangle headers")?;
            let edge=if seed.owner==0{working.vertex}else{working.face};
            let selected=initialization::penetration::recover_seed(hulls[seed.owner],headers[seed.owner],edge,seed.point)?;
            if seed.owner==0{working.vertex=selected;}else{working.face=selected;}
            working.kinds[seed.owner]=2;
        }
        working.retired=true;working.recovery=None;*self=working;Ok(true)
    }
    pub(crate) fn retirement_requested(&self)->bool{self.retired}
    fn refresh_features(&mut self,frame:u32,point_hull:&CompactTopology,face_hull:&CompactTopology,frames:[PreparedObjectCache;2],margin:f32,descent:&mut Descent)->Result<bool,&'static str>{
        if self.kinds!=[0,2]{return initialization::initialize(self,frame,[point_hull,face_hull],frames,margin,descent);}
        let poses=frames.map(|f|f.pose);
        let face=[self.face,face_hull.next(self.face).ok_or("missing face edge")?,previous(face_hull,self.face)?].map(|id|vertex(face_hull,id));
        let face=[face[0]?,face[1]?,face[2]?];let cross=(face[1]-face[0]).cross(face[2]-face[0]);
        let square=(cross.x*cross.x+cross.y*cross.y)+cross.z*cross.z;
        if square<1e-10{return Err("degenerate minimizer face");}
        let inv=1./(square as f64).sqrt();
        let normal_local=Vec3::new((cross.x as f64*inv) as f32,(cross.y as f64*inv) as f32,(cross.z as f64*inv) as f32);let normal=rotate(poses[1].rotation,normal_local);
        let mut active=self.vertex;
        loop{
            let point=vertex(point_hull,active)?;
            let world=poses[0].transform_point(point);let local=inverse(poses[1].rotation,world-poses[1].position);
            let canonical=std::array::from_fn::<_,3,_>(|edge|vertex(face_hull,CompactEdgeId{triangle:self.face.triangle,edge}));
            let canonical=[canonical[0]?,canonical[1]?,canonical[2]?];
            if !inside(local,canonical){
                //10124190 admits an already-outside point through the nearest
                // segment. A neighbour leaving the face in10123c40 instead
                // needs edge/edge descent; do not replace it with this path.
                if active!=self.vertex{return Err("native face neighbour edge/edge transition not yet reconstructed");}
                return initialization::initialize(self,frame,[point_hull,face_hull],frames,margin,descent);
            }
            self.order=0;
            if !descent.enter([0,1],[active,self.face],[0,2]){self.retired=true;return Ok(true);}
            let raw=dot(local,normal_local)-dot(face[0],normal_local);
            self.geometry=Some(Geometry{distance:raw-margin,projection:dot(normal,frames[0].core_position-frames[1].core_position),normal});
            let mut direction=inverse(poses[0].rotation,normal);if raw<0.{direction=-direction;}
            let last=previous(point_hull,active)?;let mut edge=previous(point_hull,across(point_hull,last)?)?;
            let mut best=0.;let mut selected=None;let mut fan=Vec::new();
            loop{
                if fan.contains(&edge){return Err("invalid vertex neighbour fan");}fan.push(edge);
                let delta=vertex(point_hull,edge)?-point;let score=dot(delta,direction);
                if score<0.{let square=(delta.y*delta.y+delta.x*delta.x)+delta.z*delta.z;
                    let normalized=score as f64/(square as f64).sqrt();
                    if normalized<best as f64{best=normalized as f32;selected=Some(edge);}
                }
                if edge==last{break;}edge=previous(point_hull,across(point_hull,edge)?)?;
            }
            if let Some(next)=selected{
                let next_local=inverse(poses[1].rotation,poses[0].transform_point(vertex(point_hull,next)?)-poses[1].position);
                if !inside(next_local,canonical){return initialization::leave_face(self,next,frame,[point_hull,face_hull],frames,margin,descent);}
                active=next;continue;
            }
            if raw<0.{self.kinds=[0,5];self.recovery=Some(Recovery{owner:1,point:local});}
            self.order=0;self.vertex=active;self.geometry=Some(Geometry{distance:raw-margin,projection:dot(normal,frames[0].core_position-frames[1].core_position),normal});self.frame=frame;return Ok(true);
        }
    }
}
#[cfg(test)]
mod tests{
    #[test]
    fn native_sphere_candidate_radius_controls_match_including_equality(){
        use bevy::prelude::*;
        let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-query.json")).unwrap();
        for c in d["controls"].as_array().unwrap(){
            let point=Vec3::from_array(std::array::from_fn(|k|f32::from_bits(c["point"][k].as_u64().unwrap() as u32)));
            let radius=f32::from_bits(c["radius"].as_u64().unwrap() as u32);
            let inside=super::sphere_query_contains(crate::ivp_motion::Pose{position:Vec3::ZERO,rotation:Mat3::IDENTITY},point,radius).unwrap();
            assert_eq!(inside,c["count"]==1,"candidate {c}");
        }
        assert_eq!(d["controls"].as_array().unwrap().len(),32);
    }
    #[test]
    fn owned_sphere_closest_query_matches_native_cache_geometry(){
        use bevy::prelude::*;
        let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-geometry.json")).unwrap();
        let f=|v:&serde_json::Value|f32::from_bits(v.as_u64().unwrap() as u32);
        let vector=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|k|f(&v[k])));
        for c in d["cases"].as_array().unwrap(){
            let positions=std::array::from_fn(|side|vector(&c["initial_frames"][side]["position"]));
            let cores=std::array::from_fn(|side|vector(&c["initial_frames"][side]["core_position"]));
            let g=super::sphere_geometry(positions,cores,f(&c["margin"])).unwrap();
            assert_eq!(g.projection.to_bits() as u64,c["geometry"][0].as_u64().unwrap(),"projection case {}",c["id"]);
            assert_eq!(g.distance.to_bits() as u64,c["geometry"][2].as_u64().unwrap(),"distance case {}",c["id"]);
            for k in 0..3{assert_eq!(g.normal[k].to_bits() as u64,c["geometry"][k+3].as_u64().unwrap(),"normal case {}",c["id"]);}
            assert_eq!(c["before"],c["after"]);
        }
        assert_eq!(d["cases"].as_array().unwrap().len(),48);
    }
    use super::*;
    #[test]
    fn actual_vertex_face_queries_retain_native_edges_and_same_frame_reuse(){check(true,false,false);}
    #[test]
    fn retained_vertex_face_branch_matches_all_admitted_native_queries(){check(false,false,false);}
    #[test]
    fn native_initial_pair_creation_matches_all_six_source_descents(){check(true,true,false);}
    #[test]
    fn native_pairs_carry_source_features_from_creation_through_contact(){check(true,false,true);}
    fn check(include_cold_start:bool,initial:bool,continuous:bool){
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");if !root.exists(){return;}
        let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
        let mut d:serde_json::Value=serde_json::from_str(if initial{include_str!("../tools/fixtures/native-ivp-minimizer-startup.json")}else{include_str!("../tools/fixtures/native-ivp-minimizer.json")}).unwrap();
        if continuous{
            let startup:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-minimizer-startup.json")).unwrap();
            let cases=d["cases"].as_array().unwrap().iter().filter(|c|c["mode"]=="drop").map(|c|{
                let initial=startup["cases"].as_array().unwrap().iter().find(|i|i["model"]==c["model"]).unwrap();assert_eq!(initial["compact_bindings"],c["compact_bindings"]);
                let mut result=c.clone();let mut events=initial["events"].as_array().unwrap().clone();events.extend(c["events"].as_array().unwrap().iter().cloned());
                result["events"]=serde_json::Value::Array(events);result["mode"]=serde_json::json!("initial-through-contact");result
            }).collect();d["cases"]=serde_json::Value::Array(cases);
        }
        let f=|v:&serde_json::Value|v.as_f64().unwrap() as f32;
        let vec=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));
        let id=|v:&serde_json::Value|CompactEdgeId{triangle:v["triangle"].as_u64().unwrap() as usize,edge:v["edge"].as_u64().unwrap() as usize};
        let mut count=0;let mut reused=0;
        for c in d["cases"].as_array().unwrap(){
            let moving=l4d2_formats::phy::parse_phy(&assets.read(c["model"].as_str().unwrap()).unwrap()).unwrap();
            let hex=c["compact_bindings"].as_array().unwrap().iter().find(|v|v["slot"]==1).unwrap()["collision_hex"].as_str().unwrap();
            let raw=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();
            let mut packet=(raw.len() as u32).to_le_bytes().to_vec();packet.extend(raw);
            let floor=l4d2_formats::phy::parse_collision(&packet,1).unwrap();
            let all=c["events"].as_array().unwrap();
            // The branch regression starts from a valid local vertex/face
            // caller packet. The separate full query regression above also
            // includes native point/point cold starts and remains strict.
            let begin=if include_cold_start{0}else{all.iter().position(|e|e["before"]["sides"][0]["kind"]==0&&e["before"]["sides"][1]["kind"]==2).unwrap()};
            let before=&all[begin]["before"];
            let mut query=if before["query_frame"]==0{Query::compact_pair()}else{Query{retired:false,order:((before["flags"].as_u64().unwrap()>>8)&1) as usize,recovery:None,kinds:std::array::from_fn(|i|before["sides"][i]["kind"].as_i64().unwrap() as i16),frame:before["query_frame"].as_u64().unwrap() as u32,vertex:id(&before["sides"][0]),face:id(&before["sides"][1]),geometry:Some(Geometry{distance:f(&before["measures"][2]),projection:f(&before["measures"][0]),normal:vec(&before["normal"])})}};
            for e in &all[begin..]{
                let before=&e["before"];let after=&e["after"];let sides=before["sides"].as_array().unwrap();
                assert_eq!(query.frame,before["query_frame"].as_u64().unwrap() as u32);assert_eq!(query.kinds,std::array::from_fn(|i|sides[i]["kind"].as_i64().unwrap() as i16));
                assert_eq!(query.vertex,id(&sides[0]),"{} step{} incoming feature",c["model"],e["step"]);
                let frames=std::array::from_fn(|i|{let(p,o,time,frame)=crate::ivp_cache_lifecycle_tests::controls(&sides[i]["prediction"]);p.prepare_object_cache(time,o,frame)});
                let a=sides[0]["convex"].as_u64().unwrap() as usize;let b=sides[1]["convex"].as_u64().unwrap() as usize;
                let changed=query.refresh(before["environment_frame"].as_u64().unwrap() as u32,moving[0].convexes[a].topology.as_ref().unwrap(),floor[0].convexes[b].topology.as_ref().unwrap(),frames,f(&before["margin"])).unwrap_or_else(|err|panic!("{} {} step{}: {err}",c["mode"],c["model"],e["step"]));
                if !changed{reused+=1;}count+=1;
                assert_eq!(query.vertex,id(&after["sides"][0]),"{} step{} selected edge",c["model"],e["step"]);assert_eq!(query.face,id(&after["sides"][1]));assert_eq!(query.kinds,std::array::from_fn(|i|after["sides"][i]["kind"].as_i64().unwrap() as i16));assert_eq!(query.frame,after["query_frame"].as_u64().unwrap() as u32);
                if after["flags"].as_u64().unwrap()&0xf000==0{
                    let mut contact=owner::Contact::new([0,1],after["flags"].as_u64().unwrap() as u32,f(&before["margin"])).unwrap();contact.query=query;
                    let entry=crate::ivp_surface_admission::contacts::Entry{key:crate::ivp_surface_admission::Pair{slots:[0,1],ledges:[a,b]},contact,recursive:None};
                    let features=entry.physical_features([1,2],[&moving[0].convexes[a],&floor[0].convexes[b]]).unwrap();
                    for side in 0..2{assert_eq!(features[side].endpoint.feature.kind,after["sides"][side]["kind"].as_i64().unwrap() as i16);assert_eq!((features[side].endpoint.feature.triangle&0xffffffff)/16-1,after["sides"][side]["triangle"].as_u64().unwrap());assert_eq!(features[side].endpoint.object,side as u64);}
                }
                let geometry=query.geometry.expect("queried native geometry must be defined");
                for(a,b)in [(geometry.distance,f(&after["measures"][2])),(geometry.projection,f(&after["measures"][0]))]{assert!((a-b).abs()<=1e-6,"{} step{} metric {a} != {b}",c["model"],e["step"]);}
                assert!(geometry.normal.distance(vec(&after["normal"]))<=1e-6);
            }
        }assert_eq!(count,if continuous{248}else if initial{6}else if include_cold_start{426}else{420});assert_eq!(reused,if continuous{44}else if initial{0}else{82});
    }
}
