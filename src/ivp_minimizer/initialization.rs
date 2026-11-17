//! Native10121ea0 vertex-pair descent and101225a0 edge admission.
//! Unsupported response/penetration branches remain explicit errors.
use super::*;
#[path="edge_pair.rs"]
mod edge_pair;
#[path="penetration.rs"]
pub(super) mod penetration;
#[path="general.rs"]
mod general;
pub(super) fn compact_plane(h:&CompactTopology,e:CompactEdgeId)->Result<[f32;4],&'static str>{general::plane(h,e)}
#[derive(Clone,Copy,PartialEq,Eq)]
enum Route{Pair([usize;2],[CompactEdgeId;2]),Edge([usize;2],[CompactEdgeId;2]),Face([usize;2],[CompactEdgeId;2]),Edges([usize;2],[CompactEdgeId;2])}
fn commit_route(query:&mut Query,route:Route){
    let (roles,ids,kinds)=match route{Route::Pair(r,i)=>(r,i,[0,0]),Route::Edge(r,i)=>(r,i,[0,1]),Route::Face(r,i)=>(r,i,[0,2]),Route::Edges(r,i)=>(r,i,[1,1])};
    query.order=roles[0];
    if roles==[0,1]{query.vertex=ids[0];query.face=ids[1];query.kinds=kinds;}else{query.vertex=ids[1];query.face=ids[0];query.kinds=[kinds[1],kinds[0]];}
}
//100edcc0 followed by100ec4d0; deliberately local, as in10124753.
fn perpendicular(axis:Vec3)->Vec3{
    let v=axis.to_array();let mut index=0;let mut best=0.;
    if axis.z.abs()>0.{index=2;best=axis.z.abs();}
    if axis.y.abs()>best{index=1;best=axis.y.abs();}
    if axis.x.abs()>best{index=0;}
    let prior=(index+2)%3;let mut auxiliary=v;auxiliary[index]=v[prior];auxiliary[prior]=-v[index];
    let p=Vec3::from_array(auxiliary).cross(axis);let square=(p.x*p.x+p.y*p.y)+p.z*p.z;
    if square<1e-10{return p;}
    let inverse=1./(square as f64).sqrt();Vec3::new((p.x as f64*inverse) as f32,(p.y as f64*inverse) as f32,(p.z as f64*inverse) as f32)
}
fn next(t:&CompactTopology,id:CompactEdgeId)->Result<CompactEdgeId,&'static str>{t.next(id).ok_or("missing minimizer next edge")}
fn fan(t:&CompactTopology,id:CompactEdgeId)->Result<Vec<CompactEdgeId>,&'static str>{
    let last=previous(t,id)?;let mut edge=previous(t,across(t,last)?)?;let mut result=Vec::new();
    loop{if result.contains(&edge){return Err("invalid initialization vertex fan");}result.push(edge);if edge==last{return Ok(result);}edge=previous(t,across(t,edge)?)?;}
}

#[cfg(test)]
mod edge_controls{
    use super::*;
    use l4d2_formats::phy::CompactEdge;
    #[test]
    fn native_terminal_vertex_edge_geometry_and_role_ownership(){
        check(include_str!("../../tools/fixtures/native-ivp-minimizer-edge.json"),false);
    }
    #[test]
    fn native_face_admission_selects_edges_vertices_and_boundaries(){
        check(include_str!("../../tools/fixtures/native-ivp-minimizer-face-edge.json"),true);
    }
    #[test]
    fn native_face_neighbours_leave_via_edge_pair_descent(){
        check(include_str!("../../tools/fixtures/native-ivp-minimizer-face-neighbor.json"),true);
    }
    fn check(raw:&str,face_admission:bool){
        let data:serde_json::Value=serde_json::from_str(raw).unwrap();
        assert_eq!(data["cases"].as_array().unwrap().len(),if face_admission{576}else{48});
        for (n,c) in data["cases"].as_array().unwrap().iter().enumerate(){
            let word=|v:&serde_json::Value|v.as_u64().unwrap() as u32;
            let float=|v:&serde_json::Value|f32::from_bits(word(v));
            let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{
                edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(word(&h[5+t*4+e])))).collect(),
                vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|float(&h[20+i*4+j])))).collect(),
            }).collect();
            let frames:[PreparedObjectCache;2]=std::array::from_fn(|i|{
                let f=&c["frames"][i];let core_position=Vec3::new(float(&f[0]),float(&f[1]),float(&f[2]));
                let rotation=Mat3::from_cols_array(&std::array::from_fn(|j|float(&f[8+(j%3)*4+j/3])));
                PreparedObjectCache{core_position,pose:crate::ivp_motion::Pose{position:Vec3::new(float(&f[20]),float(&f[21]),float(&f[22])),rotation},quaternion:[0.,0.,0.,1.],frame_stamp:1}
            });
            if face_admission{
                let world=frames[0].pose.transform_point(vertex(&hulls[0],CompactEdgeId{triangle:0,edge:0}).unwrap());
                let local=inverse(frames[1].pose.rotation,world-frames[1].pose.position);
                assert_eq!(local.to_array().map(f32::to_bits),std::array::from_fn(|j|word(&c["local_point"][j])),"case{n} local point");
                for edge in 0..3{
                    let distance=segment_distance_squared(&hulls[1],CompactEdgeId{triangle:0,edge},local).unwrap();
                    assert_eq!(distance.to_bits(),c["distances"][edge].as_u64().unwrap(),"case{n} segment{edge}");
                }
            }
            let p=CompactEdgeId{triangle:0,edge:0};let e=CompactEdgeId{triangle:0,edge:if face_admission{c["start"].as_u64().unwrap() as usize}else{2}};
            let features=&c["features"];
            let selected: [CompactEdgeId;2]=std::array::from_fn(|i|CompactEdgeId{triangle:features[i][0].as_u64().unwrap() as usize,edge:features[i][1].as_u64().unwrap() as usize});
            let kinds:[i16;2]=std::array::from_fn(|i|features[i][2].as_i64().unwrap() as i16);
            for reversed in [false,true]{
                let (h,f)=if reversed{([&hulls[1],&hulls[0]],[frames[1],frames[0]])}else{([&hulls[0],&hulls[1]],frames)};
                let mut query=Query{retired:false,order:0,recovery:None,frame:0,kinds:if reversed{[if face_admission{2}else{1},0]}else{[0,if face_admission{2}else{1}]},vertex:if reversed{e}else{p},face:if reversed{p}else{e},geometry:None};
                assert!(query.refresh(1,h[0],h[1],f,0.02).unwrap_or_else(|error|panic!("case{n} reverse{reversed}: {error}")),"case{n}");
                let g=query.geometry.unwrap();let expected=&c["geometry"];
                assert_eq!([g.projection.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()],
                    [word(&expected[0]),word(&expected[2]),word(&expected[3]),word(&expected[4]),word(&expected[5])],"case{n} reverse{reversed}");
                assert_eq!(query.kinds,if reversed{[kinds[1],kinds[0]]}else{kinds},"case{n}");assert_eq!(query.vertex,selected[usize::from(reversed)],"case{n}");assert_eq!(query.face,selected[usize::from(!reversed)],"case{n}");
                assert!(!query.refresh(1,h[0],h[1],f,0.02).unwrap());
            }
        }
    }
}
fn coordinates(t:&CompactTopology,e:CompactEdgeId,p:Vec3)->Result<[f32;2],&'static str>{
    let a=vertex(t,e)?;let b=vertex(t,next(t,e)?)?;let d=b-a;
    Ok([-((a.x-p.x)*d.x+(a.y-p.y)*d.y+(a.z-p.z)*d.z)+f32::MIN_POSITIVE,
        ((b.x-p.x)*d.x+(b.y-p.y)*d.y+(b.z-p.z)*d.z)+f32::MIN_POSITIVE])
}
fn opposite_weight(t:&CompactTopology,e:CompactEdgeId,p:Vec3)->Result<f32,&'static str>{
    let a=vertex(t,CompactEdgeId{triangle:e.triangle,edge:0})?;let b=vertex(t,CompactEdgeId{triangle:e.triangle,edge:1})?;let c=vertex(t,CompactEdgeId{triangle:e.triangle,edge:2})?;
    let u=a-b;let v=c-b;let d=p-b;let xyz=|a:Vec3,b:Vec3|(a.x*b.x+a.y*b.y)+a.z*b.z;
    let uu=xyz(u,u);let vv=xyz(v,v);let uv=xyz(v,u);let ru=xyz(d,u);let rv=xyz(d,v);
    let det=vv*uu-uv*uv;let wc=rv*uu-ru*uv;let wa=ru*vv-rv*uv;
    Ok(match e.edge{0=>wc,1=>wa,2=>(det-wa)-wc,_=>return Err("invalid barycentric edge")}+f32::MIN_POSITIVE)
}
//100f48d0/100f4450: the endpoint and line distances return retained
// precision.10124190 compares that result against a float best-so-far.
fn segment_distance_squared(t:&CompactTopology,e:CompactEdgeId,p:Vec3)->Result<f64,&'static str>{
    let a=vertex(t,e)?;let b=vertex(t,next(t,e)?)?;let [u,v]=coordinates(t,e,p)?;
    if !u.is_sign_negative()&&!v.is_sign_negative(){
        let cross=(p-a).cross(b-a);
        let x=a.x as f64-b.x as f64;let y=a.y as f64-b.y as f64;let z=(a.z-b.z) as f64;
        let denominator=((x*x+y*y)+z*z)+1e-18f32 as f64;
        Ok(((cross.y as f64*cross.y as f64+cross.x as f64*cross.x as f64)+cross.z as f64*cross.z as f64)*(1./denominator))
    }else{
        let end=if u>=0.{b}else{a};
        let x=p.x as f64-end.x as f64;let y=p.y as f64-end.y as f64;let z=p.z as f64-end.z as f64;
        Ok((x*x+y*y)+z*z)
    }
}
pub(super) fn leave_face(query:&mut Query,selected:CompactEdgeId,frame:u32,h:[&CompactTopology;2],f:[PreparedObjectCache;2],margin:f32,descent:&mut Descent)->Result<bool,&'static str>{
    let local=inverse(f[1].pose.rotation,f[0].pose.transform_point(vertex(h[0],selected)?)-f[1].pose.position);
    let mut edge=query.face;let mut weights=[0.;3];let mut edges=[edge;3];
    for i in 0..3{weights[i]=opposite_weight(h[1],edge,local)?;edges[i]=edge;edge=next(h[1],edge)?;}
    let negative:Vec<_>=(0..3).filter(|&i|weights[i]<0.).collect();
    let chosen=if negative.len()==1{edges[negative[0]]}else{
        let mut best=1e20f32;let mut chosen=None;
        for i in 0..3{if weights[i]<=0.{let distance=edge_pair::distance([selected,edges[i]],h,f)?;if distance<best as f64{best=distance as f32;chosen=Some(edges[i]);}}}
        chosen.ok_or("native exiting face edge selection overflow")?
    };
    let mut proxy=Query{retired:false,order:0,recovery:None,frame:query.frame,kinds:[1,1],vertex:selected,face:chosen,geometry:query.geometry};
    initialize(&mut proxy,frame,h,f,margin,descent)?;
    if proxy.retired{query.retired=true;query.order=proxy.order;query.geometry=proxy.geometry;query.frame=proxy.frame;}else{*query=proxy;}Ok(true)
}
pub(super) fn initialize(query:&mut Query,frame:u32,hulls:[&CompactTopology;2],frames:[PreparedObjectCache;2],margin:f32,descent:&mut Descent)->Result<bool,&'static str>{
    let ids=[query.vertex,query.face];let preferred=[query.order,1-query.order];let ordered=preferred.map(|i|ids[i]);let mut route=match query.kinds{
        [0,0]=>Route::Pair(preferred,ordered),[0,1]=>Route::Edge([0,1],ids),[1,0]=>Route::Edge([1,0],[ids[1],ids[0]]),
        [1,1]=>Route::Edges(preferred,ordered),[0,2]=>Route::Face([0,1],ids),[2,0]=>Route::Face([1,0],[ids[1],ids[0]]),
        [1,2]|[2,1]|[2,2]=>{
            let selected=general::select(ordered,preferred.map(|i|hulls[i]),preferred.map(|i|frames[i]))?;
            match selected{Route::Pair(r,i)=>Route::Pair(r.map(|i|preferred[i]),i),Route::Edge(r,i)=>Route::Edge(r.map(|i|preferred[i]),i),Route::Face(r,i)=>Route::Face(r.map(|i|preferred[i]),i),Route::Edges(r,i)=>Route::Edges(r.map(|i|preferred[i]),i)}
        },_=>return Err("native initial feature combination not yet reconstructed"),
    };
    //1010df90 commits the selected feature packet before invoking descent.
    if matches!(query.kinds,[1,2]|[2,1]|[2,2]){commit_route(query,route);}
    let poses=frames.map(|f|f.pose);
    loop{
        let roles=match route{Route::Pair(r,_)|Route::Edge(r,_)|Route::Face(r,_)|Route::Edges(r,_)=>r};query.order=roles[0];
        match route{
            Route::Edges(roles,ids)=>match edge_pair::descend(roles,ids,hulls,frames,margin,query,descent)?{
                edge_pair::Outcome::Retired=>{query.retired=true;return Ok(true);}
                edge_pair::Outcome::Route(next)=>{route=next;continue;}
                edge_pair::Outcome::Terminal(geometry)=>{
                    query.order=roles[0];query.frame=frame;query.kinds=[1,1];query.geometry=Some(geometry);
                    if roles==[0,1]{query.vertex=ids[0];query.face=ids[1];}else{query.vertex=ids[1];query.face=ids[0];}
                    return Ok(true);
                }
                edge_pair::Outcome::Penetrating{geometry,kinds,owner,point}=>{
                    query.order=roles[0];query.frame=frame;query.geometry=Some(geometry);query.recovery=Some(Recovery{owner:roles[owner],point});
                    if roles==[0,1]{query.vertex=ids[0];query.face=ids[1];query.kinds=kinds;}else{query.vertex=ids[1];query.face=ids[0];query.kinds=[kinds[1],kinds[0]];}
                    return Ok(true);
                }
            },
            Route::Pair(roles,ids)=>{
                if !descent.enter(roles,ids,[0,0]){query.retired=true;return Ok(true);}
                let points=[vertex(hulls[roles[0]],ids[0])?,vertex(hulls[roles[1]],ids[1])?];
                let world=std::array::from_fn::<_,2,_>(|i|poses[roles[i]].transform_point(points[i]));let delta=world[0]-world[1];let sq=dot(delta,delta);
                if sq<=1e-6{query.retired=true;return Ok(true);}
                let reciprocal=1./(sq as f64).sqrt();let inv=reciprocal as f32;
                let normal=Vec3::new(((world[0].x as f64-world[1].x as f64)*reciprocal) as f32,delta.y*inv,delta.z*inv);
                let geometry=Geometry{distance:(reciprocal*sq as f64-margin as f64) as f32,projection:dot(normal,frames[roles[0]].core_position-frames[roles[1]].core_position),normal};query.geometry=Some(geometry);
                let mut best=0.;let mut choice=None;
                for side in 0..2{
                    let own=roles[side];let other=roles[1-side];let local=inverse(poses[own].rotation,world[1-side]-poses[own].position);let dir=local-points[side];
                    let base=(points[side].x*dir.x+points[side].y*dir.y)+points[side].z*dir.z;
                    for edge in fan(hulls[own],ids[side])?{
                        let p=vertex(hulls[own],edge)?;let score=dot(p,dir)-base;
                        if score>0.{let d=p-points[side];let square=dot(d,d)+1e-18;let score=score as f64/(square as f64).sqrt();
                            if (best as f64)<score{best=score as f32;choice=Some((side,own,other,edge,local));}
                        }
                    }
                }
                let terminal=|query:&mut Query|{
                    for i in 0..2{if roles[i]==0{query.vertex=ids[i];}else{query.face=ids[i];}}
                    query.order=roles[0];query.kinds=[0,0];query.frame=frame;
                };
                let Some((side,own,other,edge,local))=choice else{terminal(query);return Ok(true);};
                let [u,v]=coordinates(hulls[own],edge,local)?;
                if v<=0.{terminal(query);return Ok(true);}
                route=if u>=0.{Route::Edge([other,own],[ids[1-side],edge])}else{Route::Pair([own,other],[edge,ids[1-side]])};
            },
            Route::Edge(roles,ids)=>{
                let [point,edge]=roles;let local=inverse(poses[edge].rotation,poses[point].transform_point(vertex(hulls[point],ids[0])?)-poses[edge].position);
                let [u,v]=coordinates(hulls[edge],ids[1],local)?;
                if u<0.{route=Route::Pair(roles,ids);continue;}
                if v<0.{route=Route::Pair(roles,[ids[0],next(hulls[edge],ids[1])?]);continue;}
                if !descent.enter(roles,ids,[0,1]){query.retired=true;return Ok(true);}
                let a=vertex(hulls[edge],ids[1])?;let axis=vertex(hulls[edge],next(hulls[edge],ids[1])?)?-a;
                let third=vertex(hulls[edge],previous(hulls[edge],ids[1])?)?-a;let opposite=across(hulls[edge],ids[1])?;
                let back=vertex(hulls[edge],previous(hulls[edge],opposite)?)?-a;let direction=local-a;
                let n0=axis.cross(third);let n1=back.cross(axis);
                let d0=(direction.x*n0.x+n0.y*direction.y)+n0.z*direction.z;let d1=(n1.x*direction.x+n1.y*direction.y)+n1.z*direction.z;
                let w0=opposite_weight(hulls[edge],ids[1],local)?;let w1=opposite_weight(hulls[edge],opposite,local)?;
                if w0<=0.&&w1<=0.{
                    if d0<0.&&d1<0.{
                        query.order=roles[0];query.frame=frame;query.recovery=Some(Recovery{owner:edge,point:local});
                        if roles==[0,1]{query.vertex=ids[0];query.face=ids[1];query.kinds=[0,5];}else{query.vertex=ids[1];query.face=ids[0];query.kinds=[5,0];}return Ok(true);
                    }
                    //10124600..10124a56: the radial vector is deliberately
                    // retained unnormalized for the point's neighbour scan.
                    let cross=axis.cross(direction);
                    let inverse_axis=1./((axis.x*axis.x+axis.y*axis.y)+axis.z*axis.z);
                    let square=((cross.y*cross.y+cross.x*cross.x)+cross.z*cross.z)*inverse_axis;
                    let (distance,radial,normal)=if square<=1e-10{(-margin,cross,perpendicular(axis))}else{
                        let reciprocal=1./(square as f64).sqrt();let radial=rotate(poses[edge].rotation,axis.cross(cross));
                        ((reciprocal*square as f64-margin as f64) as f32,radial,radial*(-((reciprocal as f32)*inverse_axis)))
                    };
                    query.geometry=Some(Geometry{distance,projection:dot(normal,frames[point].core_position-frames[edge].core_position),normal});
                    let search=inverse(poses[point].rotation,radial);
                    let origin=vertex(hulls[point],ids[0])?;
                    let mut best=square*1e-6;let mut selected=None;
                    for candidate in fan(hulls[point],ids[0])?{
                        let delta=vertex(hulls[point],candidate)?-origin;
                        let score=(delta.x*search.x+delta.y*search.y)+delta.z*search.z;
                        if score>0.{
                            let length=(delta.y*delta.y+delta.x*delta.x)+delta.z*delta.z;
                            let normalized=score as f64/(length as f64).sqrt();
                            if normalized>best as f64{best=normalized as f32;selected=Some(candidate);}
                        }
                    }
                    if let Some(selected)=selected{
                        let mut descend=true;
                        if best<0.003{
                            let setup=edge_pair::Setup::new([ids[1],selected],[hulls[edge],hulls[point]],[frames[edge],frames[point]])?;
                            descend=setup.coefficients().is_some_and(|c|c[2]>=0.);
                        }
                        if descend{route=Route::Edges(roles,[selected,ids[1]]);continue;}
                    }
                    query.order=roles[0];query.kinds=if roles==[0,1]{[0,1]}else{[1,0]};
                    if roles==[0,1]{query.vertex=ids[0];query.face=ids[1];}else{query.vertex=ids[1];query.face=ids[0];}
                    query.frame=frame;return Ok(true);
                }
                let selected=if w0>0.&&(w1<=0.||d1<=0.){ids[1]}else{opposite};
                let _=d0;route=Route::Face(roles,[ids[0],selected]);
            },
            Route::Face(roles,ids)=>{
                let point=poses[roles[0]].transform_point(vertex(hulls[roles[0]],ids[0])?);
                let local=inverse(poses[roles[1]].rotation,point-poses[roles[1]].position);
                let canonical=std::array::from_fn::<_,3,_>(|edge|vertex(hulls[roles[1]],CompactEdgeId{triangle:ids[1].triangle,edge}));
                if !inside(local,[canonical[0]?,canonical[1]?,canonical[2]?]){
                    let mut best=1e20f32;let mut selected=None;let mut edge=ids[1];
                    for _ in 0..3{
                        let distance=segment_distance_squared(hulls[roles[1]],edge,local)?;
                        if distance<best as f64{best=distance as f32;selected=Some(edge);}
                        edge=next(hulls[roles[1]],edge)?;
                    }
                    let selected=selected.ok_or("native face/edge selection overflow")?;
                    route=Route::Edge(roles,[ids[0],selected]);continue;
                }
                let local_frames=roles.map(|i|frames[i]);let mut proxy=Query{retired:false,order:0,recovery:None,frame:query.frame,kinds:[0,2],vertex:ids[0],face:ids[1],geometry:query.geometry};
                descent.scoped(roles,|descent|proxy.refresh_features(frame,hulls[roles[0]],hulls[roles[1]],local_frames,margin,descent))?;
                query.retired=proxy.retired;query.order=roles[proxy.order];query.recovery=proxy.recovery.map(|s|Recovery{owner:roles[s.owner],point:s.point});query.frame=proxy.frame;query.geometry=proxy.geometry;
                if proxy.retired{return Ok(true);}
                query.kinds=if roles==[0,1]{proxy.kinds}else{[proxy.kinds[1],proxy.kinds[0]]};
                if roles==[0,1]{query.vertex=proxy.vertex;query.face=proxy.face;}else{query.vertex=proxy.face;query.face=proxy.vertex;}return Ok(true);
            }
        }
    }
}
