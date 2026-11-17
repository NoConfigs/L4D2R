//!10105650 refresh and owned point, edge, triangle and sphere feature geometry.
use bevy::prelude::*;
#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct State {pub time:f64,pub mix:[f32;2],pub marker:u8,pub counter:u8}
#[derive(Clone,Copy)]
pub(crate) struct Shape {pub kind:i16,pub vertices:[Vec3;3],pub canonical:[Vec3;3],pub rotation:[Vec3;3],pub translation:Vec3,pub radius:f32}
impl Shape {
    /// Build the refresh geometry from the original IVP edge cycle, without
    /// converting outward Source hull triangles back into IVP coordinates.
    pub(crate) fn from_compact(kind:i16,geometry:Option<(&l4d2_formats::phy::Convex,l4d2_formats::phy::CompactEdgeId)>,rotation:[Vec3;3],translation:Vec3,radius:f32)->Result<Self,&'static str>{
        if !(0..=3).contains(&kind)||!radius.is_finite()||radius<0.||!translation.is_finite()||rotation.iter().any(|v|!v.is_finite()){return Err("invalid compact refresh shape");}
        let mut vertices=[Vec3::ZERO;3];let mut canonical=[Vec3::ZERO;3];
        if kind!=3 {
            let (convex,id)=geometry.ok_or("missing compact refresh geometry")?;
            let topology=convex.topology.as_ref().ok_or("synthetic hull has no compact refresh topology")?;
            if convex.triangle_headers.get(id.triangle).ok_or("missing compact triangle")?.0&0xfff!=id.triangle as u32{return Err("inconsistent compact refresh triangle");}
            let next=topology.next(id).ok_or("invalid compact refresh edge")?;let previous=topology.previous(id).unwrap();
            for (out,id) in vertices.iter_mut().zip([id,next,previous]){*out=Vec3::from_array(topology.vertex(id).ok_or("missing compact refresh vertex")?);}
            for (e,out) in canonical.iter_mut().enumerate(){*out=Vec3::from_array(topology.vertex(l4d2_formats::phy::CompactEdgeId{triangle:id.triangle,edge:e}).ok_or("missing canonical compact vertex")?);}
        }
        Ok(Self{kind,vertices,canonical,rotation,translation,radius})
    }
}
#[derive(Clone,Copy)]
pub(crate) struct Body {pub flags:u16,pub inverse_mass:f32,pub inverse_inertia:Vec3,pub rotation:[Vec3;3],pub position:Vec3,pub linear:Vec3,pub angular:Vec3}
#[derive(Clone,Copy)]
pub(crate) struct Settings {pub epsilon:f32,pub bary_bias:f32,pub area_epsilon:f64,pub one:f32,pub opposite:f32}
#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct Geometry {pub point:Vec3,pub normal:Vec3,pub tangent:Vec3,pub distance:f32,pub status:i16,pub marker:u8,pub counter:u8}
#[derive(Clone)]
pub(crate) struct Output {pub geometry:Geometry,pub point:Vec3,pub normal:Vec3,pub basis:[Vec3;2],pub arms:[Vec3;2],pub jacobians:[Vec3;2],pub response:f32,pub inverse_response:f32,pub active:[bool;2],pub state:State,pub distance:f32}
fn dot(a:Vec3,b:Vec3)->f32 {(a.y*b.y+a.x*b.x)+a.z*b.z}
fn xyz(a:Vec3,b:Vec3)->f32 {(a.x*b.x+a.y*b.y)+a.z*b.z}
fn rotate(r:[Vec3;3],v:Vec3)->Vec3 {Vec3::new(dot(r[0],v),dot(r[1],v),dot(r[2],v))}
fn inverse(r:[Vec3;3],v:Vec3)->Vec3 {Vec3::new((r[1].x*v.y+r[0].x*v.x)+r[2].x*v.z,(r[0].y*v.x+r[1].y*v.y)+r[2].y*v.z,(r[1].z*v.y+r[0].z*v.x)+r[2].z*v.z)}
fn normalized(v:Vec3,epsilon:f32)->Vec3 {
    let square=xyz(v,v);if square<epsilon {return v;}
    let inv=1.0/(square as f64).sqrt();Vec3::new((v.x as f64*inv) as f32,(v.y as f64*inv) as f32,(v.z as f64*inv) as f32)
}
pub(crate) const POINT_AXIS_THRESHOLD:f32=0.899999976;
pub(crate) const EDGE_FALLBACK_DISTANCE:f32=0.0145991575;
fn length(v:Vec3)->f64 {((v.y as f64*v.y as f64+v.x as f64*v.x as f64)+v.z as f64*v.z as f64).sqrt()}
fn world(s:Shape,v:Vec3)->Vec3 {rotate(s.rotation,v)+s.translation}
fn point_geometry(state:State,point:Vec3,other:Vec3,k:&Settings)->Geometry {
    let delta=other-point;let sq=xyz(delta,delta);
    let (normal,distance)=if sq<k.epsilon {(delta,0.)}else{let inv=1.0/(sq as f64).sqrt();(Vec3::new((delta.x as f64*inv) as f32,(delta.y as f64*inv) as f32,(delta.z as f64*inv) as f32),(inv*sq as f64) as f32)};
    let axis=if normal.x*normal.x>=POINT_AXIS_THRESHOLD {Vec3::Z}else{Vec3::X};
    Geometry {point,normal,tangent:normalized(normal.cross(axis),k.epsilon),distance,status:0,marker:state.marker,counter:state.counter}
}
fn edge_geometry(mut state:State,point:Vec3,b:Shape,k:&Settings)->Geometry {
    let start=world(b,b.vertices[0]);let end=world(b,b.vertices[1]);let edge=end-start;let delta=point-start;
    let inverse=(1.0/(xyz(edge,edge) as f64).sqrt()) as f32;
    let cross=edge.cross(delta);let distance=length(cross)*inverse as f64;
    let tangent=if distance*distance<=k.epsilon as f64 {Vec3::X}else{cross*(inverse/distance as f32)};
    let normal=normalized(edge.cross(tangent),k.epsilon);let mut status=0;
    if state.marker==1 {state.marker=0;let t=(dot(delta,edge)*inverse)*inverse;if t<0.||k.one<t {status=1;}}
    Geometry {point,normal,tangent,distance:distance as f32,status,marker:state.marker,counter:state.counter}
}
fn edge_pair_geometry(mut state:State,a:Shape,b:Shape,k:&Settings)->Geometry {
    let a0=world(a,a.vertices[0]);let a1=world(a,a.vertices[1]);let b0=world(b,b.vertices[0]);let b1=world(b,b.vertices[1]);
    let da=normalized(a1-a0,k.epsilon);let db=normalized(b1-b0,k.epsilon);let cross=da.cross(db);let sq=xyz(cross,cross);
    let fallback=||Geometry {point:a0,normal:Vec3::X,tangent:Vec3::Y,distance:EDGE_FALLBACK_DISTANCE,status:1,marker:state.marker,counter:state.counter};
    if sq<=k.epsilon {return fallback();}
    let ca=da.cross(cross);let cb=db.cross(cross);let vb=xyz(ca,b0);let den_b=vb-xyz(ca,b1);
    if den_b.abs()<k.epsilon {return fallback();}
    let tb=(vb-xyz(ca,a0))*(k.one/den_b);
    let va=xyz(cb,a0);let den_a=va-xyz(cb,a1);
    if den_a.abs()<k.epsilon {return fallback();}
    // This numerator and reciprocal are evaluated in double by the native
    // x87 path, while tb and both denominators use SSE single precision.
    let projected=(cb.x as f64*b0.x as f64+cb.y as f64*b0.y as f64)+cb.z as f64*b0.z as f64;
    let ta=((va as f64-projected)*(1.0/den_a as f64)) as f32;
    let p=a0*(k.one-ta)+a1*ta;let q=b0*(k.one-tb)+b1*tb;let delta=q-p;let distance=length(delta);
    let (normal,distance)=if distance<=k.epsilon as f64 {
        let inv=1.0/(sq as f64).sqrt();(Vec3::new((cross.x as f64*inv) as f32,(cross.y as f64*inv) as f32,(cross.z as f64*inv) as f32),0.)
    }else{(delta*(k.one/distance as f32),distance as f32)};
    let mut status=0;if state.marker==1 {state.marker=0;if tb<0.||k.one<tb||ta<0.||k.one<ta {status=1;}}
    Geometry {point:p,normal,tangent:da,distance,status,marker:state.marker,counter:state.counter}
}
pub(crate) fn geometry(state:State,shapes:[Shape;2],k:&Settings)->Result<Geometry,&'static str> {
    let [a,b]=shapes;
    if a.kind==1&&b.kind==1 {return Ok(edge_pair_geometry(state,a,b,k));}
    if a.kind!=0&&a.kind!=3 {return Err("unreconstructed contact feature configuration");}
    let mut state=state;if a.kind==3 {state.marker=1;}
    let point=if a.kind==3 {a.translation}else{world(a,a.vertices[0])};
    match b.kind {
        0=>return Ok(point_geometry(state,point,world(b,b.vertices[0]),k)),
        3=>return Ok(point_geometry(state,point,b.translation,k)),
        1=>return Ok(edge_geometry(state,point,b,k)),
        2=>{},_=>return Err("unreconstructed contact feature configuration"),
    }
    let local=inverse(b.rotation,point-b.translation);
    let counter=state.counter.wrapping_add(1)&31;let mut marker=state.marker;let mut status=0;
    if counter==0||marker==1 {
        marker=0;
        let [p,q,r]=b.canonical;let u=p-q;let v=r-q;let d=local-q;
        let uu=xyz(u,u);let vv=xyz(v,v);let uv=xyz(v,u);let ru=xyz(d,u);let rv=xyz(d,v);
        let det=vv*uu-uv*uv;let wc=rv*uu-ru*uv;let wa=ru*vv-rv*uv;
        if [wc+k.bary_bias,((det-wa)-wc)+k.bary_bias,wa+k.bary_bias].into_iter().any(|v|v.is_sign_negative()) {status=1;}
    }
    let (cross,_,area)=crate::ivp_create::triangle_area(b.vertices,k.area_epsilon);
    let normal_local=cross*area;
    let distance=dot(normal_local,local)-dot(b.vertices[0],normal_local);
    let normal=rotate(b.rotation,normal_local)*k.opposite;
    let tangent=rotate(b.rotation,b.vertices[1]-b.vertices[0]);
    Ok(Geometry {point,normal,tangent,distance,status,marker,counter})
}
pub(crate) fn refresh(state:State,shapes:[Shape;2],bodies:[Body;2],now:f64,k:&Settings)->Result<Output,&'static str> {
    refresh_with_cache_release(state,shapes,bodies,now,k,||{})
}
/// Caches remain retained through geometry/radius handling, then release before
/// basis normalization and body response, matching10105981.
pub(crate) fn refresh_with_cache_release(state:State,shapes:[Shape;2],bodies:[Body;2],now:f64,k:&Settings,release:impl FnOnce())->Result<Output,&'static str> {
    let geometry=geometry(state,shapes,k)?;let point=geometry.normal*shapes[0].radius+geometry.point;
    let mut distance=geometry.distance-(shapes[1].radius+shapes[0].radius);if distance<0. {distance=0.;}
    release();
    let t0=normalized(geometry.tangent,k.epsilon);let basis=[t0,geometry.normal.cross(t0)];
    let mut arms=[Vec3::ZERO;2];let mut jacobians=[Vec3::ZERO;2];let mut active=[false;2];let mut response=0.;let mut velocity=[Vec3::ZERO;2];
    for side in 0..2 {
        let b=bodies[side];if b.flags&2!=0 {continue;}active[side]=true;
        arms[side]=inverse(b.rotation,point-b.position);let normal=inverse(b.rotation,geometry.normal);
        let j=arms[side].cross(normal);jacobians[side]=j;
        velocity[side]=b.linear+rotate(b.rotation,b.angular.cross(arms[side]));
        let x=(j.x*b.inverse_inertia.x)*j.x;let y=(j.y*b.inverse_inertia.y)*j.y;let z=(j.z*b.inverse_inertia.z)*j.z;
        response+=((y+x)+z)+b.inverse_mass;
    }
    if !response.is_finite()||response<=0. {return Err("unreconstructed singular physical response");}
    let relative=velocity[0]-velocity[1];let dt=now-state.time;
    let mix=std::array::from_fn(|axis| {let t=basis[axis];let speed=(relative.x as f64*t.x as f64+t.y as f64*relative.y as f64)+t.z as f64*relative.z as f64;(state.mix[axis] as f64-speed*dt) as f32});
    Ok(Output {geometry,point,normal:geometry.normal,basis,arms,jacobians,response,inverse_response:k.one/response,active,state:State {time:now,mix,marker:geometry.marker,counter:geometry.counter},distance})
}

#[cfg(test)]
mod tests {
    fn prepare_frame_event(store:&mut crate::ivp_motion::PreparedFrames,event:&Value)->crate::ivp_motion::PreparedObjectCache{
        use crate::ivp_motion::{Prediction,ObjectFrame};
        let b=&event["input"];let quat=|a:&Value|std::array::from_fn(|i|f(&a[i]));
        let prediction=Prediction{start:v(&b["start"]),velocity:v(&b["velocity"]),rotation_start:quat(&b["rotation_start"]),rotation_end:quat(&b["rotation_end"]),time:b["time"].as_f64().unwrap(),inverse_dt:f(&b["inverse_dt"])};
        let object=ObjectFrame{shift:(b["flags"].as_u64().unwrap()&0x800==0).then(||v(&b["shift"])),rotation:b["extra"].as_bool().unwrap().then(||quat(&b["object_rotation"]))};
        store.prepare(event["cache"].as_u64().unwrap() as u32,event["slot"].as_u64().unwrap()+1,prediction,b["now"].as_f64().unwrap(),object,event["frame"].as_u64().unwrap() as u32).unwrap()
    }
    #[test]
    fn actual_prepared_and_reused_cache_frames_drive_loaded_contact_refresh(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-frame-refresh.json")).unwrap();
        let data=generated_frame_inputs(data,686,1413,true);check_loaded_refresh(&data);
    }
    fn generated_frame_inputs(mut data:Value,expected_frames:usize,expected_calls:usize,write_packet:bool)->Value{
        let profiles=data["profiles"].as_array().unwrap();let mut packet=(profiles.len() as u32).to_le_bytes().to_vec();let mut comparisons=Vec::new();
        for profile in profiles{
            let events=profile["frame_events"].as_array().unwrap();packet.extend((events.len() as u32).to_le_bytes());let mut store=crate::ivp_motion::PreparedFrames::default();
            for event in events{
                assert!(event.get("output").is_none());assert!(!event["seed"].as_bool().unwrap());let frame=prepare_frame_event(&mut store,event);
                let mut values=frame.core_position.to_array().to_vec();values.extend(frame.pose.position.to_array());values.extend(frame.quaternion);values.extend(frame.pose.rotation.transpose().to_cols_array());
                let bytes:Vec<u8>=values.into_iter().flat_map(|v|v.to_bits().to_le_bytes()).collect();
                for value in [event["id"].as_u64().unwrap() as u32,event["cache"].as_u64().unwrap() as u32,event["slot"].as_u64().unwrap() as u32,frame.frame_stamp]{packet.extend(value.to_le_bytes());}packet.extend(&bytes);
                comparisons.push((crc32fast::hash(&bytes),event["pose_crc32"].as_u64().unwrap() as u32));
            }
        }
        if write_packet{
            let source=std::path::Path::new(file!());
            let workspace=if source.is_absolute(){source.parent().unwrap().parent().unwrap()}else{std::path::Path::new(env!("CARGO_MANIFEST_DIR"))};
            let output=workspace.join("local-research/parity");std::fs::create_dir_all(&output).unwrap();
            std::fs::write(output.join("ivp-frame-refresh-1024-source.packet"),packet).unwrap();
        }
        for (i,(actual,expected)) in comparisons.iter().enumerate(){assert_eq!(actual,expected,"prepared cache event{i}");}assert_eq!(comparisons.len(),expected_frames);
        let mut stores:Vec<_>=profiles.iter().map(|_|crate::ivp_motion::PreparedFrames::default()).collect();let mut consumed=vec![0;profiles.len()];let mut uses=0;
        let profiles=data["profiles"].clone();
        for case in data["cases"].as_array_mut().unwrap(){
            let profile=case["profile"].as_u64().unwrap() as usize;let events=profiles[profile]["frame_events"].as_array().unwrap();
            let last=case["shapes"].as_array().unwrap().iter().map(|s|s["frame_event"].as_u64().unwrap() as usize).max().unwrap();
            while consumed[profile]<=last{prepare_frame_event(&mut stores[profile],&events[consumed[profile]]);consumed[profile]+=1;}
            for shape in case["shapes"].as_array_mut().unwrap(){
                assert!(shape.get("rotation_rows").is_none());assert!(shape.get("translation").is_none());
                let event=&events[shape["frame_event"].as_u64().unwrap() as usize];
                let frame=stores[profile].get(event["cache"].as_u64().unwrap() as u32,shape["compact_source"]["slot"].as_u64().unwrap()+1,shape["frame_stamp"].as_u64().unwrap() as u32).unwrap();
                let rotation=frame.pose.rotation.transpose().to_cols_array();let position=frame.pose.position.to_array();
                let bytes:Vec<u8>=rotation.into_iter().chain(position).flat_map(|v|v.to_bits().to_le_bytes()).collect();assert_eq!(crc32fast::hash(&bytes) as u64,shape["frame_transform_crc32"].as_u64().unwrap());
                shape["rotation_rows"]=serde_json::json!(rotation);shape["translation"]=serde_json::json!(position);uses+=1;
            }
        }assert_eq!(uses,expected_calls*2);data
    }
    #[test]
    fn actual_contact_refresh_uses_source_loaded_compact_geometry(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-bound-refresh.json")).unwrap();check_loaded_refresh(&data);
    }
    fn check_loaded_refresh(data:&Value){check_loaded_refresh_with(data,1413,|_,_,_,_,_,_|{});}
    #[test]
    fn retained_contacts_carry_refresh_slip_work_and_force_across_native_ticks(){
        use crate::{ivp_contact::Core,ivp_contact_pipeline::{CoreFrame,RetainedContact,RetainedState,TangentSettings},ivp_history::History};
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-tangent-pipeline.json")).unwrap();
        let data=generated_frame_inputs(data,1019,2127,false);
        let singles=data["singles"].as_array().unwrap();let tangents=data["tangents"].as_array().unwrap();
        let mut retained:Vec<Option<RetainedContact>>=(0..singles.len()).map(|_|None).collect();
        let core=|c:&Value|Core{inverse_mass:f(&c["inverse_mass"]),inverse_inertia:v(&c["inverse_inertia"]),linear:v(&c["linear"]),angular:v(&c["angular"]),pending_linear:v(&c["pending_linear"]),pending_angular:v(&c["pending_angular"])};
        let compare=|a:Core,b:Core|{assert_eq!(a.linear,b.linear);assert_eq!(a.angular,b.angular);assert_eq!(a.pending_linear,b.pending_linear);assert_eq!(a.pending_angular,b.pending_angular);};
        let (mut roots,mut carries,mut calls,mut retired,mut releases)=(0,0,0,0,0);
        check_loaded_refresh_with(&data,2127,|index,case,expected_state,shapes,bodies,k|{
            let Some(t)=tangents.iter().find(|t|t["refresh_case"].as_u64()==Some(index as u64)) else{return;};
            let (normal_index,n)=singles.iter().enumerate().find(|(_,n)|n["refresh_case"]==t["refresh_case"]).unwrap();
            let owners:[Option<usize>;2]=std::array::from_fn(|i|(bodies[i].flags&2==0).then(||case["shapes"][i]["compact_source"]["slot"].as_u64().unwrap() as usize));
            let previous=t["normal_source"].as_i64().unwrap();
            let mut contact=if previous>=0{
                carries+=1;
                // Native identity/epoch association chooses the predecessor;
                // no numeric result is used to find or rebuild this state.
                retained[previous as usize].take().expect("missing live predecessor")
            }else{
                roots+=1;
                // Entry from creation/batch is still an observed boundary.
                // Only these25 roots seed state;28 subsequent ticks do not.
                RetainedContact::from_state(RetainedState{refresh:expected_state,normal:History{force:f(&t["force"]),counter:n["counter"].as_i64().unwrap() as i16},energy:f(&t["energy_before"]),work:f(&t["work_before"]),response_bound:f(&t["response_bound"])},owners).unwrap()
            };
            let before=contact.state();
            assert_eq!(before.refresh,expected_state,"retained refresh {index}");
            assert_eq!(before.normal.force.to_bits(),f(&t["force"]).to_bits());
            assert_eq!(before.energy.to_bits(),f(&t["energy_before"]).to_bits());assert_eq!(before.work.to_bits(),f(&t["work_before"]).to_bits());
            assert_eq!(before.response_bound.to_bits(),f(&t["response_bound"]).to_bits());
            assert!(contact.refresh(shapes,bodies,before.refresh.time-1.,k,||panic!("stale refresh released a cache")).is_err());
            assert_eq!(contact.state().refresh,before.refresh);
            let output=contact.refresh(shapes,bodies,case["now"].as_f64().unwrap(),k,||releases+=1).unwrap();
            assert_eq!(output.state.time,case["now"].as_f64().unwrap());
            assert_eq!(output.state.mix,std::array::from_fn(|i|f(&case["after"]["state"]["mix"][i])));
            let mut cores=[Core::default();2];let mut frames=[CoreFrame{rotation:Mat3::IDENTITY,position:Vec3::ZERO};2];
            for side in 0..2{if let Some(i)=owners[side]{let b=&t["bodies_before"][side];cores[i]=core(b);frames[i]=CoreFrame{rotation:Mat3::from_cols_array(&std::array::from_fn(|j|f(&b["rotation_rows"][j]))).transpose(),position:v(&b["position"])};}}
            let profile=&data["profiles"][case["profile"].as_u64().unwrap() as usize];
            let mu=crate::ivp_material::clamped_product(f(&profile["friction_coefficients"][0]),f(&profile["friction_coefficients"][1])) as f32;
            let p=&t["constants"];let settings=TangentSettings{dt:f(&t["dt"]),inverse_dt:f(&t["inverse_dt"]),friction:mu,response_bound:before.response_bound,threshold_extra:f(&p["threshold_extra"]),minimum_impulse:f(&p["minimum_impulse"]),minimum_determinant_squared:f(&p["minimum_determinant_squared"]),mode:t["mode"].as_u64().unwrap() as u8,alternate_candidate:t["alternate"].as_bool().unwrap()};
            let invalid=TangentSettings{response_bound:before.response_bound+1.,..settings};let saved=cores;
            assert!(contact.respond_friction(&mut cores,&frames,&invalid).is_err());for i in 0..2{compare(cores[i],saved[i]);}
            contact.respond_friction(&mut cores,&frames,&settings).unwrap();
            let state=contact.state();assert_eq!(state.refresh.mix,std::array::from_fn(|i|f(&t["slip_after"]["mix"][i])));
            assert_eq!(state.refresh.marker,t["slip_after"]["marker"].as_u64().unwrap() as u8);
            assert_eq!(state.energy.to_bits(),f(&t["slip_after"]["energy"]).to_bits());assert_eq!(state.work.to_bits(),f(&t["work_after"]).to_bits());
            for side in 0..2{if let Some(i)=owners[side]{compare(cores[i],core(&t["bodies_after"][side]));}}
            // Normal consumes the actual output cores from tangent, not an
            // independently reseeded copy of its native before-snapshot.
            for b in n["raw_before"].as_array().unwrap(){if b["key"].as_u64().unwrap()!=0{compare(cores[0],core(b));}}
            assert_eq!(state.normal.force.to_bits(),f(&n["force_before"]).to_bits());assert_eq!(state.normal.counter,n["counter"].as_i64().unwrap() as i16);
            let response=contact.respond_normal(&mut cores,f(&n["inverse_dt"])).unwrap();
            for b in n["raw_after"].as_array().unwrap(){if b["key"].as_u64().unwrap()!=0{compare(cores[0],core(b));}}
            assert_eq!(response.retire,n["retired"].as_bool().unwrap());
            if response.retire{
                retired+=1;assert!(contact.is_retired());let saved=cores;
                assert!(contact.respond_normal(&mut cores,f(&n["inverse_dt"])).is_err());
                assert!(contact.respond_friction(&mut cores,&frames,&settings).is_err());
                assert!(contact.refresh(shapes,bodies,case["now"].as_f64().unwrap(),k,||panic!("retired contact released a cache")).is_err());
                for i in 0..2{compare(cores[i],saved[i]);}
            }else{
                assert_eq!(contact.state().normal.force.to_bits(),f(&n["force_after"]).to_bits());
                assert_eq!(contact.state().normal.counter,n["counter_after"].as_i64().unwrap() as i16);
                retained[normal_index]=Some(contact);
            }
            calls+=1;
        });
        assert_eq!((roots,carries,calls,retired,releases),(25,28,53,10,53));
    }
    #[test]
    fn source_normal_history_and_refresh_cache_feed_native_single_group_tangent_response(){
        use crate::{ivp_contact::Core,ivp_contact_pipeline::{CoreFrame,PreparedContact,TangentSettings},ivp_history::History};
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-tangent-pipeline.json")).unwrap();
        let data=generated_frame_inputs(data,1019,2127,false);let mut caches=Vec::new();
        check_loaded_refresh_with(&data,2127,|index,case,state,shapes,bodies,k|{
            assert_eq!(index,caches.len());
            let owners=std::array::from_fn(|i|(bodies[i].flags&2==0).then(||case["shapes"][i]["compact_source"]["slot"].as_u64().unwrap() as usize));
            caches.push(PreparedContact::prepare(state,shapes,bodies,owners,case["now"].as_f64().unwrap(),k,||{}).unwrap());
        });
        let core=|c:&Value|Core{inverse_mass:f(&c["inverse_mass"]),inverse_inertia:v(&c["inverse_inertia"]),linear:v(&c["linear"]),angular:v(&c["angular"]),pending_linear:v(&c["pending_linear"]),pending_angular:v(&c["pending_angular"])};
        let mut normal_histories=Vec::new();
        for row in data["singles"].as_array().unwrap(){
            let mut cores=[Core::default();2];for c in row["raw_before"].as_array().unwrap(){if c["key"].as_u64().unwrap()!=0{cores[0]=core(c);}}
            let mut h=History{force:f(&row["force_before"]),counter:row["counter"].as_i64().unwrap() as i16};
            let response=caches[row["refresh_case"].as_u64().unwrap() as usize].respond_single(&mut cores,&mut h,f(&row["inverse_dt"])).unwrap();
            assert_eq!(response.retire,row["retired"].as_bool().unwrap());if !response.retire{assert_eq!(h.force.to_bits(),f(&row["force_after"]).to_bits());}
            normal_histories.push(h);
        }
        let (mut calls,mut carried,mut zero_history,mut batch_history,mut applied,mut baseline_carry)=(0,0,0,0,0,0);
        for row in data["tangents"].as_array().unwrap(){
            let index=row["refresh_case"].as_u64().unwrap() as usize;let cache=&caches[index];let context=&data["cases"][index];
            let owners:[Option<usize>;2]=std::array::from_fn(|i|cache.geometry().active[i].then(||context["shapes"][i]["compact_source"]["slot"].as_u64().unwrap() as usize));
            let mut cores=[Core::default();2];let mut frames=[CoreFrame{rotation:Mat3::IDENTITY,position:Vec3::ZERO};2];
            for side in 0..2{assert_eq!(owners[side].is_some(),row["active"][side].as_bool().unwrap());if let Some(i)=owners[side]{let b=&row["bodies_before"][side];cores[i]=core(b);frames[i]=CoreFrame{rotation:Mat3::from_cols_array(&std::array::from_fn(|j|f(&b["rotation_rows"][j]))).transpose(),position:v(&b["position"])};}}
            let source=row["normal_source"].as_i64().unwrap();
            let force=if source>=0{carried+=1;normal_histories[source as usize].force}else if f(&row["force"])==0.{zero_history+=1;0.}else{batch_history+=1;f(&row["force"])};
            assert_eq!(force.to_bits(),f(&row["force"]).to_bits());
            let profile=&data["profiles"][context["profile"].as_u64().unwrap() as usize];assert_eq!(profile["controlled_mass"],1.);
            let mu=crate::ivp_material::clamped_product(f(&profile["friction_coefficients"][0]),f(&profile["friction_coefficients"][1])) as f32;
            assert_eq!(mu.to_bits(),f(&row["friction"]).to_bits());
            let k=&row["constants"];let settings=TangentSettings{dt:f(&row["dt"]),inverse_dt:f(&row["inverse_dt"]),friction:mu,response_bound:f(&row["response_bound"]),threshold_extra:f(&k["threshold_extra"]),minimum_impulse:f(&k["minimum_impulse"]),minimum_determinant_squared:f(&k["minimum_determinant_squared"]),mode:row["mode"].as_u64().unwrap() as u8,alternate_candidate:row["alternate"].as_bool().unwrap()};
            let mut memory=cache.friction_memory(f(&row["energy_before"]),f(&row["work_before"]));
            let result=cache.respond_single_friction(&mut cores,&frames,force,&mut memory,&settings).unwrap();applied+=usize::from(result.applied);
            assert_eq!(memory.work.to_bits(),f(&row["work_after"]).to_bits(),"work {} step{}",row["model"],row["step"]);
            assert_eq!(memory.slip.mix,std::array::from_fn(|i|f(&row["slip_after"]["mix"][i])));assert_eq!(memory.slip.energy.to_bits(),f(&row["slip_after"]["energy"]).to_bits());assert_eq!(memory.slip.marker,row["slip_after"]["marker"].as_u64().unwrap() as u8);
            for side in 0..2{if let Some(i)=owners[side]{let expected=core(&row["bodies_after"][side]);let actual=cores[i];assert_eq!(actual.linear,expected.linear,"tangent {} step{}",row["model"],row["step"]);assert_eq!(actual.angular,expected.angular);assert_eq!(actual.pending_linear,expected.pending_linear);assert_eq!(actual.pending_angular,expected.pending_angular);}}
            if row["capture"]==2&&row["model"]=="models/gibs/glass_shard01.phy"&&row["step"]==31{assert!(source>=0);assert_eq!(data["singles"][source as usize]["step"],30);baseline_carry+=1;}
            // Unsupported controller state must leave live cores/history intact.
            let before=cores;let before_memory=memory;let invalid=TangentSettings{mode:1,..settings};
            assert!(cache.respond_single_friction(&mut cores,&frames,force,&mut memory,&invalid).is_err());assert_eq!(memory.work.to_bits(),before_memory.work.to_bits());assert_eq!(memory.slip,before_memory.slip);
            for i in 0..2{assert_eq!(cores[i].linear,before[i].linear);assert_eq!(cores[i].angular,before[i].angular);}
            calls+=1;
        }
        assert_eq!((calls,carried,zero_history,batch_history,baseline_carry),(53,28,23,2,1));assert!(applied>0);
    }
    #[test]
    fn loaded_geometry_and_generated_frames_feed_identity_linked_single_responses(){
        use crate::{ivp_contact::Core,ivp_contact_pipeline::PreparedContact,ivp_history::History};
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-single-pipeline.json")).unwrap();
        let data=generated_frame_inputs(data,1019,2127,false);let mut caches=Vec::new();
        check_loaded_refresh_with(&data,2127,|index,case,state,shapes,bodies,k|{
            assert_eq!(index,caches.len());
            let owners=std::array::from_fn(|i|(bodies[i].flags&2==0).then(||case["shapes"][i]["compact_source"]["slot"].as_u64().unwrap() as usize));
            let cache=PreparedContact::prepare(state,shapes,bodies,owners,case["now"].as_f64().unwrap(),k,||{}).unwrap();
            assert_eq!(cache.geometry().normal,v(&case["after"]["normal"]));caches.push(cache);
        });
        let parse_core=|c:&Value|Core{inverse_mass:f(&c["inverse_mass"]),inverse_inertia:v(&c["inverse_inertia"]),linear:v(&c["linear"]),angular:v(&c["angular"]),pending_linear:v(&c["pending_linear"]),pending_angular:v(&c["pending_angular"])};
        let (mut singles,mut retired,mut baseline30,mut impulses)=(0,0,0,0);
        for row in data["singles"].as_array().unwrap(){
            // Native supplies current raw core state and force history, never
            // cached normals/Jacobians, solved inverse response, distance/status.
            let mut cores=[Core::default();2];
            for c in row["raw_before"].as_array().unwrap(){let key=c["key"].as_u64().unwrap();if key>0{assert_eq!(key,1);cores[0]=parse_core(c);}}
            let mut history=History{force:f(&row["force_before"]),counter:row["counter"].as_i64().unwrap() as i16};
            let result=caches[row["refresh_case"].as_u64().unwrap() as usize].respond_single(&mut cores,&mut history,f(&row["inverse_dt"])).unwrap();
            assert_eq!(result.retire,row["retired"].as_bool().unwrap());
            if !result.retire{assert_eq!(history.force,f(&row["force_after"]));assert_eq!(history.counter,row["counter_after"].as_i64().unwrap() as i16);}else{retired+=1;}
            for c in row["raw_after"].as_array().unwrap(){if c["key"].as_u64().unwrap()==0{continue;}let expected=parse_core(c);let actual=cores[0];
                assert_eq!(actual.linear,expected.linear,"{} step{} linked single linear",row["model"],row["step"]);assert_eq!(actual.angular,expected.angular);
                assert_eq!(actual.pending_linear,expected.pending_linear);assert_eq!(actual.pending_angular,expected.pending_angular);
            }
            if row["capture"]==2&&row["model"]=="models/gibs/glass_shard01.phy"&&row["step"]==30{baseline30+=1;assert!(result.impulse.is_some());}
            impulses+=usize::from(result.impulse.is_some());singles+=1;
        }
        assert_eq!((singles,retired,baseline30),(53,10,1));assert!(impulses>0);
    }
    fn check_loaded_refresh_with(data:&Value,expected_calls:usize,mut prepared:impl FnMut(usize,&Value,State,[Shape;2],[Body;2],&Settings)){
        use l4d2_formats::phy::CompactEdgeId;
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");if !root.exists(){return;}
        let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();let mut profiles=Vec::new();
        for profile in data["profiles"].as_array().unwrap(){
            let body=l4d2_formats::phy::parse_phy(&assets.read(profile["model"].as_str().unwrap()).unwrap()).unwrap();assert_eq!(body.len(),1);
            let hex=profile["floor_collision_hex"].as_str().unwrap();let floor_bytes:Vec<u8>=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect();
            let mut serialized=(floor_bytes.len() as u32).to_le_bytes().to_vec();serialized.extend(floor_bytes);let floor=l4d2_formats::phy::parse_collision(&serialized,1).unwrap();
            let shapes=[body,floor];for i in 0..2{assert_eq!(shapes[i][0].convexes.len() as u64,profile["convex_counts"][i].as_u64().unwrap());}
            assert!(profile["registration_motion_unchanged"].as_bool().unwrap());assert!(profile["observation_motion_unchanged"].as_bool().unwrap());profiles.push(shapes);
        }
        let mut local_geometry_count=0;let mut calls=0;
        for (case_index,original) in data["cases"].as_array().unwrap().iter().enumerate(){
            let profile=&profiles[original["profile"].as_u64().unwrap() as usize];let mut case=original.clone();
            for shape in case["shapes"].as_array_mut().unwrap(){
                assert!(shape.get("vertices").is_none());assert!(shape.get("canonical").is_none());
                let binding=&shape["compact_source"];assert!(binding["available"].as_bool().unwrap());let kind=shape["kind"].as_i64().unwrap() as i16;
                let geometry=if kind==3{None}else{
                    Some((&profile[binding["slot"].as_u64().unwrap() as usize][0].convexes[binding["convex"].as_u64().unwrap() as usize],CompactEdgeId{triangle:binding["triangle"].as_u64().unwrap() as usize,edge:binding["edge"].as_u64().unwrap() as usize}))
                };
                let mut loaded=Shape::from_compact(kind,geometry,rows(&shape["rotation_rows"]),v(&shape["translation"]),f(&shape["radius"])).unwrap();
                if let Some((convex,edge))=geometry{
                    // Exercise the physical-admission handoff against native
                    // local-geometry hashes, not copies of expected vertices.
                    let feature=crate::ivp_managed_response::Feature::from_compact(binding["slot"].as_u64().unwrap(),1,convex,kind,edge).unwrap();
                    loaded.vertices=feature.vertices;loaded.canonical=feature.canonical;
                    if kind==2{assert_eq!(feature.endpoint.feature.vertices,loaded.vertices);}
                }
                let words:Vec<u8>=loaded.vertices.into_iter().chain(loaded.canonical).flat_map(|p|p.to_array().map(f32::to_bits)).flat_map(u32::to_le_bytes).collect();
                assert_eq!(crc32fast::hash(&words) as u64,shape["local_geometry_crc32"].as_u64().unwrap(),"local geometry case{case_index}");local_geometry_count+=1;
                // Only source-derived arrays enter the solver. The fixture has
                // hashes of observed geometry, never its local vertex arrays.
                shape["vertices"]=serde_json::json!(loaded.vertices.map(|p|p.to_array()));shape["canonical"]=serde_json::json!(loaded.canonical.map(|p|p.to_array()));
            }
            let (s,shapes,bodies,k)=parse(&case);let output=refresh(s,shapes,bodies,case["now"].as_f64().unwrap(),&k).unwrap();let expected=&case["after"];let g=&case["geometry"];
            assert_eq!(output.geometry.point,v(&g["point"]),"geometry point case{case_index}");assert_eq!(output.geometry.normal,v(&g["normal"]));assert_eq!(output.geometry.tangent,v(&g["tangent"]));assert_eq!(output.geometry.distance,f(&g["distance"]));assert_eq!(output.geometry.status,g["status"].as_i64().unwrap() as i16);
            assert_eq!(output.point,v(&expected["point"]));assert_eq!(output.point,v(&expected["cached_point"]));assert_eq!(output.normal,v(&expected["normal"]));assert_eq!(output.normal,v(&expected["cached_normal"]));assert_eq!(output.distance,f(&expected["distance"]));
            assert_eq!(output.active,std::array::from_fn(|i|expected["active"][i].as_bool().unwrap()));
            for i in 0..2{assert_eq!(output.basis[i],v(&expected["basis"][i]));assert_eq!(output.arms[i],v(&expected["arms"][i]));assert_eq!(output.jacobians[i],v(&expected["jacobians"][i]));}
            assert_eq!(output.response,f(&expected["response"]));assert_eq!(output.inverse_response,f(&expected["inverse_response"]));assert_eq!(output.state,state(&expected["state"]));
            assert_eq!(crate::ivp_create::triangle_area(shapes[1].vertices,k.area_epsilon).2,f(&case["inverse_area"]));prepared(case_index,&case,s,shapes,bodies,&k);calls+=1;
        }assert_eq!(calls,expected_calls);assert_eq!(local_geometry_count,expected_calls*2);
    }
    #[test]
    fn refresh_shape_factory_requires_finite_owned_geometry(){
        let identity=[Vec3::X,Vec3::Y,Vec3::Z];
        assert!(Shape::from_compact(0,None,identity,Vec3::ZERO,0.).is_err());
        assert!(Shape::from_compact(4,None,identity,Vec3::ZERO,0.).is_err());
        assert!(Shape::from_compact(3,None,identity,Vec3::ZERO,f32::NAN).is_err());
        assert!(Shape::from_compact(3,None,identity,Vec3::ZERO,0.).is_ok());
    }
    use super::*;
    use serde_json::Value;
    fn fixture()->Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-refresh.json")).unwrap()}
    fn f(v:&Value)->f32 {v.as_f64().unwrap() as f32}
    fn v(v:&Value)->Vec3 {Vec3::from_array(std::array::from_fn(|i|f(&v[i])))}
    fn rows(vv:&Value)->[Vec3;3] {std::array::from_fn(|i|Vec3::new(f(&vv[i*3]),f(&vv[i*3+1]),f(&vv[i*3+2])))}
    fn state(a:&Value)->State {State {time:a["time"].as_f64().unwrap(),mix:std::array::from_fn(|i|f(&a["mix"][i])),marker:a["marker"].as_u64().unwrap() as u8,counter:a["counter"].as_u64().unwrap() as u8}}
    fn parse(c:&Value)->(State,[Shape;2],[Body;2],Settings) {
        let shapes=std::array::from_fn(|i|{let a=&c["shapes"][i];Shape {kind:a["kind"].as_i64().unwrap() as i16,vertices:std::array::from_fn(|i|v(&a["vertices"][i])),canonical:std::array::from_fn(|i|v(&a["canonical"][i])),rotation:rows(&a["rotation_rows"]),translation:v(&a["translation"]),radius:f(&a["radius"])}});
        let bodies=std::array::from_fn(|i|{let a=&c["bodies"][i];Body {flags:a["flags"].as_u64().unwrap() as u16,inverse_mass:f(&a["inverse_mass"]),inverse_inertia:v(&a["inverse_inertia"]),rotation:rows(&a["rotation_rows"]),position:v(&a["position"]),linear:v(&a["linear"]),angular:v(&a["angular"])}});
        let a=&c["constants"];(state(&c["before"]),shapes,bodies,Settings {epsilon:f(&a["epsilon"]),bary_bias:f(&a["bary_bias"]),area_epsilon:a["area_epsilon"].as_f64().unwrap(),one:f(&a["one"]),opposite:f(&a["opposite"])})
    }
    #[test]
    fn point_triangle_geometry_is_derived_from_compact_vertices_and_frames() {
        let data=fixture();let mut count=0;
        for (i,c) in data["cases"].as_array().unwrap().iter().enumerate() {
            let (s,shapes,_,k)=parse(c);let g=geometry(s,shapes,&k).unwrap();let a=&c["geometry"];
            assert_eq!(g.point,v(&a["point"]),"point case{i}");assert_eq!(g.normal,v(&a["normal"]),"normal case{i}");assert_eq!(g.tangent,v(&a["tangent"]),"tangent case{i}");assert_eq!(g.distance,f(&a["distance"]),"distance case{i}");
            assert_eq!(g.status,a["status"].as_i64().unwrap() as i16,"status case{i}");assert_eq!(g.marker,a["marker"].as_u64().unwrap() as u8);assert_eq!(g.counter,a["counter"].as_u64().unwrap() as u8);
            assert_eq!(crate::ivp_create::triangle_area(shapes[1].vertices,k.area_epsilon).2,f(&c["inverse_area"]));count+=1;
        }
        assert_eq!(count,1552);
    }
    #[test]
    fn complete_refresh_arms_response_basis_and_slip_match_native() {
        let data=fixture();let mut count=0;
        for (i,c) in data["cases"].as_array().unwrap().iter().enumerate() {
            let (s,shapes,bodies,k)=parse(c);let o=refresh(s,shapes,bodies,c["now"].as_f64().unwrap(),&k).unwrap();let a=&c["after"];
            assert_eq!(o.point,v(&a["point"]),"point case{i}");assert_eq!(o.normal,v(&a["normal"]),"normal case{i}");
            assert_eq!(o.point,v(&a["cached_point"]));assert_eq!(o.normal,v(&a["cached_normal"]));assert_eq!(o.distance,f(&a["distance"]),"distance case{i}");
            assert_eq!(o.active,std::array::from_fn(|j|a["active"][j].as_bool().unwrap()));
            for j in 0..2 {assert_eq!(o.basis[j],v(&a["basis"][j]),"basis{j} case{i}");assert_eq!(o.arms[j],v(&a["arms"][j]),"arm{j} case{i}");assert_eq!(o.jacobians[j],v(&a["jacobians"][j]),"jacobian{j} case{i}");}
            assert_eq!(o.response,f(&a["response"]),"response case{i}");assert_eq!(o.inverse_response,f(&a["inverse_response"]),"inverse case{i}");assert_eq!(o.state,state(&a["state"]),"state case{i}");assert_eq!(o.geometry.status,a["status"].as_i64().unwrap() as i16);count+=1;
        }
        assert_eq!(count,1552);
    }
    #[test]
    fn other_feature_configuration_is_explicitly_unsupported() {
        let data=fixture();let c=&data["cases"][0];let (s,mut shapes,bodies,k)=parse(c);shapes[0].kind=1;
        assert!(refresh(s,shapes,bodies,c["now"].as_f64().unwrap(),&k).is_err());
    }
    #[test]
    fn actual_constructors_supply_the_first_refresh_history() {
        use crate::ivp_create::{self as create,Feature,Endpoint,Candidate,Input};
        let refresh_data=fixture();let constructors:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-create.json")).unwrap();
        let first=refresh_data["cases"].as_array().unwrap().iter().filter(|c| {let b=&c["before"];c["step"].as_i64().unwrap()>=0&&b["time"]==c["now"]&&b["counter"].as_u64()==Some(0)&&b["marker"].as_u64()==Some(1)&&b["mix"].as_array().unwrap().iter().all(|x|f(x)==0.)}).collect::<Vec<_>>();
        let new=constructors["selects"].as_array().unwrap().iter().filter(|c|c["created"].as_u64()==Some(1)&&c["result"].as_u64()!=Some(0)).collect::<Vec<_>>();
        assert_eq!(first.len(),82);assert_eq!(new.len(),82);
        let endpoint=|a:&Value| {let c=&a["feature"];Endpoint {object:a["object"].as_u64().unwrap(),feature:Feature {kind:c["kind"].as_i64().unwrap() as i16,edge:c["edge"].as_u64().unwrap(),opposite:c["opposite"].as_u64().unwrap(),triangle:c["triangle"].as_u64().unwrap(),ledge:c["ledge"].as_u64().unwrap(),vertex:c["vertex"].as_u64().unwrap() as u16,vertices:std::array::from_fn(|i|v(&c["vertices"][i]))}}};
        for (c,r) in new.into_iter().zip(first) {
            assert_eq!(c["capture"],r["capture"]);assert_eq!(c["model"],r["model"]);assert_eq!(c["step"],r["step"]);
            let b=&c["before"];let input=Input {flags:b["flags"].as_u64().unwrap() as u32,time:b["time"].as_f64().unwrap(),endpoints:std::array::from_fn(|i|endpoint(&b["endpoints"][i])),lists:std::array::from_fn(|i|b["lists"][i].as_array().unwrap().iter().map(|a|Candidate {key:a["key"].as_u64().unwrap(),side:a["side"].as_u64().unwrap() as usize,endpoints:std::array::from_fn(|j|endpoint(&a["endpoints"][j]))}).collect())};
            let fresh=input.lists.iter().flatten().map(|c|c.key).max().unwrap_or(0)+1;
            let made=create::resolve(&input,Some(fresh),f(&c["constants"]["distance"]),c["constants"]["area_epsilon"].as_f64().unwrap()).unwrap();assert!(made.created);
            let init=made.initial.unwrap();let history=State {time:init.time,mix:init.mix,marker:init.marker,counter:init.counter};
            assert_eq!(history,state(&r["before"]));assert_eq!(init.area,Some(f(&r["inverse_area"])));
            let (_,shapes,bodies,k)=parse(r);let o=refresh(history,shapes,bodies,r["now"].as_f64().unwrap(),&k).unwrap();
            assert_eq!(o.state,state(&r["after"]["state"]));assert_eq!(o.point,v(&r["after"]["point"]));assert_eq!(o.response,f(&r["after"]["response"]));
        }
    }
    #[test]
    fn native_edge_point_and_sphere_features_match_geometry_and_complete_cache() {
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-features.json")).unwrap();let mut count=0;
        for (i,c) in data["cases"].as_array().unwrap().iter().enumerate() {
            let (s,shapes,bodies,k)=parse(c);let o=refresh(s,shapes,bodies,c["now"].as_f64().unwrap(),&k).unwrap();let g=&c["geometry"];let a=&c["after"];
            assert_eq!(POINT_AXIS_THRESHOLD,f(&c["constants"]["point_axis_threshold"]));assert_eq!(EDGE_FALLBACK_DISTANCE,f(&c["constants"]["edge_fallback_distance"]));
            assert_eq!(o.geometry.point,v(&g["point"]),"geometry point{i}");assert_eq!(o.geometry.normal,v(&g["normal"]),"geometry normal{i}");assert_eq!(o.geometry.tangent,v(&g["tangent"]),"geometry tangent{i}");assert_eq!(o.geometry.distance,f(&g["distance"]),"geometry distance{i}");assert_eq!(o.geometry.status,g["status"].as_i64().unwrap() as i16,"status{i}");
            assert_eq!(o.geometry.marker,g["marker"].as_u64().unwrap() as u8,"marker{i}");assert_eq!(o.geometry.counter,g["counter"].as_u64().unwrap() as u8,"counter{i}");
            assert_eq!(o.point,v(&a["point"]),"point{i}");assert_eq!(o.point,v(&a["cached_point"]));assert_eq!(o.normal,v(&a["normal"]),"normal{i}");assert_eq!(o.normal,v(&a["cached_normal"]));assert_eq!(o.distance,f(&a["distance"]),"distance{i}");
            for j in 0..2 {assert_eq!(o.basis[j],v(&a["basis"][j]),"basis{i}/{j}");assert_eq!(o.arms[j],v(&a["arms"][j]),"arm{i}/{j}");assert_eq!(o.jacobians[j],v(&a["jacobians"][j]),"jacobian{i}/{j}");}
            assert_eq!(o.response,f(&a["response"]),"response{i}");assert_eq!(o.inverse_response,f(&a["inverse_response"]));assert_eq!(o.state,state(&a["state"]),"state{i}");count+=1;
        }
        assert_eq!(count,512);
    }
    #[test]
    fn generated_object_cache_frames_feed_complete_contact_refresh() {
        use crate::ivp_motion::{Prediction,ObjectFrame};
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-cached-refresh.json")).unwrap();let mut count=0;
        for (i,c) in data["cases"].as_array().unwrap().iter().enumerate() {
            let (state_in,mut shapes,bodies,k)=parse(c);
            for side in 0..2 {
                let a=&c["shapes"][side];let b=&a["cache_prediction"];
                let p=Prediction {start:v(&b["start"]),velocity:v(&b["velocity"]),rotation_start:std::array::from_fn(|i|f(&b["rotation_start"][i])),rotation_end:std::array::from_fn(|i|f(&b["rotation_end"][i])),time:b["time"].as_f64().unwrap(),inverse_dt:f(&b["inverse_dt"])};
                let object=ObjectFrame {shift:(b["flags"].as_u64().unwrap()&0x800==0).then(||v(&b["shift"])),rotation:b["extra"].as_bool().unwrap().then(||std::array::from_fn(|i|f(&b["object_rotation"][i])))};
                let cache=p.prepare_object_cache(b["now"].as_f64().unwrap(),object,b["frame"].as_u64().unwrap() as u32);
                assert_eq!(cache.pose.position,v(&a["translation"]),"cache position{i}/{side}");assert_eq!(cache.quaternion,std::array::from_fn(|j|f(&a["cache_quaternion"][j])));assert_eq!(cache.frame_stamp,a["cache_frame_stamp"].as_u64().unwrap() as u32);
                assert_eq!(cache.pose.rotation.transpose().to_cols_array(),std::array::from_fn(|j|f(&a["rotation_rows"][j])));
                let r=cache.pose.rotation.transpose();shapes[side].rotation=[r.x_axis,r.y_axis,r.z_axis];shapes[side].translation=cache.pose.position;
            }
            let o=refresh(state_in,shapes,bodies,c["now"].as_f64().unwrap(),&k).unwrap();let a=&c["after"];let g=&c["geometry"];
            assert_eq!(o.geometry.point,v(&g["point"]));assert_eq!(o.geometry.normal,v(&g["normal"]));assert_eq!(o.geometry.tangent,v(&g["tangent"]));assert_eq!(o.geometry.distance,f(&g["distance"]));
            assert_eq!(o.point,v(&a["point"]));assert_eq!(o.point,v(&a["cached_point"]));assert_eq!(o.normal,v(&a["normal"]));assert_eq!(o.normal,v(&a["cached_normal"]));assert_eq!(o.distance,f(&a["distance"]));
            for side in 0..2 {assert_eq!(o.basis[side],v(&a["basis"][side]));assert_eq!(o.arms[side],v(&a["arms"][side]));assert_eq!(o.jacobians[side],v(&a["jacobians"][side]));}
            assert_eq!(o.response,f(&a["response"]));assert_eq!(o.inverse_response,f(&a["inverse_response"]));assert_eq!(o.state,state(&a["state"]));assert_eq!(o.geometry.status,a["status"].as_i64().unwrap() as i16);count+=1;
        }
        assert_eq!(count,96);
    }

    #[test]
    fn pooled_cache_lifecycle_and_generated_frames_drive_chained_native_refresh() {
        use crate::ivp_cache_pool::{self as pool,Slot,Object};
        use crate::ivp_motion::{Prediction,ObjectFrame,PreparedObjectCache};
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-pooled-refresh.json")).unwrap();
        let index=|v:&Value|{let i=v.as_i64().unwrap();if i<0 {assert_eq!(i,-1);None}else{Some(i as usize)}};
        let parse_slots=|v:&Value|v.as_array().unwrap().iter().map(|s|Slot{references:s["refs"].as_u64().unwrap() as u32,frame_stamp:s["frame"].as_u64().unwrap() as u32,owner:index(&s["owner"])}).collect::<Vec<_>>();
        let parse_objects=|v:&Value|v.as_array().unwrap().iter().map(|o|Object{flags:o["flags"].as_u64().unwrap() as u32,cache:index(&o["cache"])}).collect::<Vec<_>>();
        let mut slots=Vec::new();let mut objects=Vec::new();let mut cursor=0;
        let mut payload:[Option<PreparedObjectCache>;4]=[None,None,None,None];
        let mut history=None;let mut count=0;let mut callbacks=0;let mut chained=0;
        for (i,c) in data["cases"].as_array().unwrap().iter().enumerate(){
            let (input,mut shapes,bodies,k)=parse(c);let b=&c["pool_before"];let a=&c["pool_after"];
            if i%3==0 {
                slots=parse_slots(&b["slots"]);objects=parse_objects(&b["objects"]);cursor=b["cursor"].as_u64().unwrap() as usize;
                payload=[None,None,None,None];history=Some(input);
            }else{chained+=1;}
            assert_eq!(history,Some(input),"history{i}");
            assert_eq!(slots,parse_slots(&b["slots"]));assert_eq!(objects,parse_objects(&b["objects"]));assert_eq!(cursor,b["cursor"].as_u64().unwrap() as usize);
            let mut acquired=[usize::MAX;2];let mut preparation=0;
            for side in [1,0] {
                let frame=b["frame"].as_u64().unwrap() as u32;
                let slot=pool::retain_for_refresh(&mut slots,&mut objects,&mut cursor,side,frame,|slot,refs,stamp|{
                    let event=&c["cache_preparations"][preparation];
                    assert_eq!(event["object"].as_u64().unwrap() as usize,side,"preparation order{i}");assert_eq!(event["slot"].as_u64().unwrap() as usize,slot);
                    assert_eq!(event["refs"].as_u64().unwrap() as u32,refs,"prepare refs{i}");assert_eq!(event["frame"].as_u64().unwrap() as u32,stamp);
                    let b=&c["shapes"][side]["cache_prediction"];
                    let p=Prediction{start:v(&b["start"]),velocity:v(&b["velocity"]),rotation_start:std::array::from_fn(|j|f(&b["rotation_start"][j])),rotation_end:std::array::from_fn(|j|f(&b["rotation_end"][j])),time:b["time"].as_f64().unwrap(),inverse_dt:f(&b["inverse_dt"])};
                    let object=ObjectFrame{shift:(b["flags"].as_u64().unwrap()&0x800==0).then(||v(&b["shift"])),rotation:b["extra"].as_bool().unwrap().then(||std::array::from_fn(|j|f(&b["object_rotation"][j])))};
                    payload[slot]=Some(p.prepare_object_cache(b["now"].as_f64().unwrap(),object,frame));preparation+=1;callbacks+=1;
                }).unwrap();
                acquired[side]=slot;
                assert_eq!(slot,a["objects"][side]["cache"].as_u64().unwrap() as usize);
            }
            assert_eq!(preparation,c["cache_preparations"].as_array().unwrap().len());
            for side in 0..2 {
                let shape=&c["shapes"][side];let slot=acquired[side];let cache=payload[slot].as_ref().unwrap();
                assert_eq!(slots[slot].references,shape["cache_prediction"]["refs"].as_u64().unwrap() as u32);
                assert_eq!(slots[slot].frame_stamp,shape["cache_frame_stamp"].as_u64().unwrap() as u32);
                assert_eq!(cache.pose.position,v(&shape["translation"]),"cache position{i}/{side}");assert_eq!(cache.quaternion,std::array::from_fn(|j|f(&shape["cache_quaternion"][j])));
                assert_eq!(cache.frame_stamp,slots[slot].frame_stamp);assert_eq!(cache.pose.rotation.transpose().to_cols_array(),std::array::from_fn(|j|f(&shape["rotation_rows"][j])));
                let r=cache.pose.rotation.transpose();shapes[side].rotation=[r.x_axis,r.y_axis,r.z_axis];shapes[side].translation=cache.pose.position;
            }
            let o=refresh_with_cache_release(history.unwrap(),shapes,bodies,c["now"].as_f64().unwrap(),&k,||{
                pool::release(&mut slots,acquired[0]).unwrap();pool::release(&mut slots,acquired[1]).unwrap();
                assert_eq!(slots,parse_slots(&c["pool_released"]["slots"]));assert_eq!(objects,parse_objects(&c["pool_released"]["objects"]));assert_eq!(cursor,c["pool_released"]["cursor"].as_u64().unwrap() as usize);
            }).unwrap();let expected=&c["after"];let g=&c["geometry"];
            assert_eq!(o.geometry.point,v(&g["point"]));assert_eq!(o.geometry.normal,v(&g["normal"]));assert_eq!(o.geometry.tangent,v(&g["tangent"]));assert_eq!(o.geometry.distance,f(&g["distance"]));assert_eq!(o.geometry.status,g["status"].as_i64().unwrap() as i16);assert_eq!(o.geometry.marker,g["marker"].as_u64().unwrap() as u8);assert_eq!(o.geometry.counter,g["counter"].as_u64().unwrap() as u8);
            assert_eq!(o.point,v(&expected["point"]));assert_eq!(o.point,v(&expected["cached_point"]));assert_eq!(o.normal,v(&expected["normal"]));assert_eq!(o.normal,v(&expected["cached_normal"]));assert_eq!(o.distance,f(&expected["distance"]));
            assert_eq!(o.active,std::array::from_fn(|side|expected["active"][side].as_bool().unwrap()));
            for side in 0..2 {assert_eq!(o.basis[side],v(&expected["basis"][side]));assert_eq!(o.arms[side],v(&expected["arms"][side]));assert_eq!(o.jacobians[side],v(&expected["jacobians"][side]));}
            assert_eq!(o.response,f(&expected["response"]));assert_eq!(o.inverse_response,f(&expected["inverse_response"]));assert_eq!(o.state,state(&expected["state"]));assert_eq!(o.geometry.status,expected["status"].as_i64().unwrap() as i16);
            assert_eq!(slots,parse_slots(&a["slots"]));assert_eq!(objects,parse_objects(&a["objects"]));assert_eq!(cursor,a["cursor"].as_u64().unwrap() as usize);
            history=Some(o.state);count+=1;
        }
        assert_eq!(count,288);assert_eq!(callbacks,268);assert_eq!(chained,192);
    }

    #[test]
    fn source_contact_refresh_arms_feed_all_real_initial_mass_bounds(){
        use crate::ivp_mass_bound::{self as mass,Core,Endpoint,Material};
        let data=fixture();let bounds:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-mass-bound.json")).unwrap();
        let refreshes=data["cases"].as_array().unwrap().iter().filter(|c|{let b=&c["before"];c["step"].as_i64().unwrap()>=0&&b["time"]==c["now"]&&b["counter"].as_u64()==Some(0)&&b["marker"].as_u64()==Some(1)&&b["mix"].as_array().unwrap().iter().all(|x|f(x)==0.)}).collect::<Vec<_>>();
        let calls=bounds["cases"].as_array().unwrap().iter().filter(|c|c["step"].as_i64().unwrap()>=0).collect::<Vec<_>>();assert_eq!(calls.len(),82);assert_eq!(refreshes.len(),82);
        for (row,c) in calls.into_iter().zip(refreshes){
            assert_eq!(row["capture"],c["capture"]);assert_eq!(row["model"],c["model"]);assert_eq!(row["step"],c["step"]);assert_eq!(row["mode_before"],0);
            let (history,shapes,bodies,k)=parse(c);let out=refresh(history,shapes,bodies,c["now"].as_f64().unwrap(),&k).unwrap();
            let cores=std::array::from_fn(|side|{let b=bodies[side];let observed=&row["bodies"][side];assert_eq!(b.flags,observed["flags"].as_u64().unwrap() as u16);assert_eq!(b.inverse_mass,f(&observed["inverse_mass"]));assert_eq!(b.inverse_inertia,v(&observed["inverse_inertia"]));assert_eq!(out.arms[side],v(&observed["arm"]));Core{flags:b.flags,inverse_mass:b.inverse_mass,inverse_inertia:b.inverse_inertia,arm:out.arms[side]}});
            let endpoints=std::array::from_fn(|side|{let b=&row["bodies"][side];Endpoint{manager:b["manager"].as_u64().unwrap() as u32,code:b["code"].as_u64().unwrap() as u8,default:Material{key:b["default_material"].as_u64().unwrap() as u32,flags:b["default_flags"].as_u64().unwrap() as u32}}});
            let result=mass::setup(0,cores,endpoints,|_,_,_|panic!("Unexpected non-default provider in owned actual contacts")).unwrap();
            assert_eq!(result.bound.to_bits(),f(&row["bound"]).to_bits());assert_eq!(result.mode,row["mode_after"].as_u64().unwrap() as u8);
        }
    }
}
