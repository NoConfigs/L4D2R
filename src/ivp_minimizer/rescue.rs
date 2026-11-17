//!10100320 default strength,10100130 dynamic/sphere recovery and101003e0
//! fixed-ledge escape. External game vetoes and owner scheduling are separate.
use crate::ivp_joint_tail::{Body,Command};
use bevy::prelude::*;
pub(crate) struct Response{pub strength:f32,pub commands:Vec<Command>}
pub(crate) struct FixedEscape{pub local_normal:Vec3,pub world_normal:Vec3,pub push:Option<Vec3>,#[cfg(test)]pub planes:Vec<[f32;4]>}
pub(crate) struct FixedResponse{pub strength:f32,pub escape:Option<FixedEscape>}
///101003e0 fixed compact-feature branch. `pending` and `linear` are core
/// velocity banks, not impulses. A returned push adds directly to pending
/// linear velocity through100e9f90 and the owner's native wake/stamp path.
pub(crate) fn fixed(hull:&l4d2_formats::phy::CompactTopology,pose:crate::ivp_motion::Pose,body:Body,pending:Vec3,linear:Vec3,dt:f32,gravity:[f32;3])->Result<FixedResponse,&'static str>{
    if !dt.is_finite()||dt<0.||gravity.iter().any(|v|!v.is_finite())||!pending.is_finite()||!linear.is_finite()||!pose.position.is_finite()||!pose.rotation.is_finite()||body.position.iter().any(|v|!v.is_finite()){return Err("invalid fixed failure recovery inputs");}
    #[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
    {let _=hull;return Err("native failure recovery arithmetic backend unavailable");}
    #[cfg(any(target_arch="x86",target_arch="x86_64"))]
    {
        let strength=strength(gravity,dt);
        if body.flags>>8>=8{return Ok(FixedResponse{strength,escape:None});}
        let local=super::inverse(pose.rotation,Vec3::from_array(body.position)-pose.position);
        let mut best=f32::from_bits(3769465068);let mut normal=Vec3::X;
        #[cfg(test)]let mut planes=Vec::new();
        for triangle in 0..hull.edges.len(){
            let p=super::initialization::compact_plane(hull,l4d2_formats::phy::CompactEdgeId{triangle,edge:0})?;
            let distance=((local.y*p[1]+local.x*p[0])+local.z*p[2])+p[3];
            if distance>best{best=distance;normal=Vec3::new(p[0],p[1],p[2]);}
            #[cfg(test)]planes.push(p);
        }
        let world_normal=super::rotate(pose.rotation,normal);let speed=super::dot(pending+linear,world_normal);
        let push=if speed<strength*f32::from_bits(1036831949){Some(world_normal*strength)}else{None};
        Ok(FixedResponse{strength,escape:Some(FixedEscape{local_normal:normal,world_normal,push,#[cfg(test)]planes})})
    }
}
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
fn strength(g:[f32;3],dt:f32)->f32{
    let factor=4f32;let mut result=0f32;
    unsafe{core::arch::asm!("fld dword ptr [{g}+8]","fmul dword ptr [{g}+8]","fld dword ptr [{g}]","fmul dword ptr [{g}]","faddp st(1), st(0)","fld dword ptr [{g}+4]","fmul dword ptr [{g}+4]","faddp st(1), st(0)","fsqrt","fmul dword ptr [{factor}]","fmul dword ptr [{dt}]","fstp dword ptr [{out}]",g=in(reg)g.as_ptr(),factor=in(reg)&factor,dt=in(reg)&dt,out=in(reg)&mut result,options(nostack));}result
}
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
fn direction(mut v:[f32;3])->[f32;3]{
    let square=(v[0]*v[0]+v[1]*v[1])+v[2]*v[2];
    if square<f32::from_bits(786163455){return [1.,0.,0.];}
    let threshold=0.01f64;let mut fallback=0u8;
    unsafe{core::arch::asm!("fld1","fld dword ptr [{square}]","fsqrt","fdivp st(1), st(0)","fld dword ptr [{v}]","fmul st(0), st(1)","fstp dword ptr [{v}]","fld dword ptr [{v}+4]","fmul st(0), st(1)","fstp dword ptr [{v}+4]","fld dword ptr [{v}+8]","fmul st(0), st(1)","fstp dword ptr [{v}+8]","fmul dword ptr [{square}]","fld qword ptr [{threshold}]","fcomip st(0), st(1)","setae byte ptr [{fallback}]","fstp st(0)",square=in(reg)&square,v=in(reg)v.as_mut_ptr(),threshold=in(reg)&threshold,fallback=in(reg)&mut fallback,options(nostack));}
    if fallback!=0{[1.,0.,0.]}else{v}
}
pub(crate) fn dynamic(bodies:[Body;2],dt:f32,gravity:[f32;3])->Result<Response,&'static str>{
    if !dt.is_finite()||dt<0.||gravity.iter().any(|v|!v.is_finite())||bodies.iter().any(|b|!b.mass.is_finite()||b.mass<0.||b.position.iter().any(|v|!v.is_finite())){return Err("invalid failure recovery inputs");}
    #[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
    {let _=bodies;return Err("native failure recovery arithmetic backend unavailable");}
    #[cfg(any(target_arch="x86",target_arch="x86_64"))]
    {
        let strength=strength(gravity,dt);let normal=direction(std::array::from_fn(|i|bodies[1].position[i]-bodies[0].position[i]));let mut commands=Vec::new();
        for side in 0..2{let b=bodies[side];let other=bodies[side^1];
            if b.flags>>8>=8||b.flags&0x10!=0{continue;}
            let mass=if other.flags&0x10==0&&other.mass<b.mass{other.mass}else{b.mass};
            let impulse=mass*strength;let impulse=if side==0{-impulse}else{impulse};
            // Retail uses the first core centre as the application point for
            // BOTH objects, so the second body's angular response is retained.
            commands.push(Command::PointImpulse{body:b.id,point:bodies[0].position,impulse:normal.map(|v|v*impulse)});
            commands.push(Command::AngularPush{body:b.id,delta:[if side==0{-dt}else{dt},0.,0.]});
        }
        Ok(Response{strength,commands})
    }
}
/// Default dynamic recovery through ordinary point/angular entry points. The
/// owner supplies live physical bodies; no expected command or after-state is
/// consumed. Registered sleeping-object transitions remain a separate path.
pub(crate) fn apply_dynamic(bodies:&mut [crate::ivp_joint_apply::Body;2],dt:f32,gravity:[f32;3])->Result<Response,&'static str>{
    let inputs=bodies.map(|b|Body{id:b.id,flags:b.flags,mass:b.mass,position:b.current.position.to_array()});
    let response=dynamic(inputs,dt,gravity)?;
    crate::ivp_joint_apply::apply_with(bodies,&response.commands,|_,_,_,_|{})?;
    Ok(response)
}
#[cfg(test)]
mod tests{
 use super::*;
 #[test]
 fn dynamic_recovery_generation_and_application_match_complete_native_body_banks(){
    use crate::ivp_joint_apply::tests::{body,snapshot,command};
    let d:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-retirement-rescue-apply.json")).unwrap();let f=|v:&serde_json::Value|f32::from_bits(v.as_u64().unwrap() as u32);let mut commands=0;let mut changed=0;
    for c in d["cases"].as_array().unwrap(){let mut bodies=std::array::from_fn(|i|body(&c["before"][i]));let response=apply_dynamic(&mut bodies,f(&c["dt"]),std::array::from_fn(|i|f(&c["gravity"][i]))).unwrap();
        assert_eq!(response.strength.to_bits(),c["scale"].as_u64().unwrap() as u32,"case{} strength",c["id"]);
        commands+=response.commands.len();assert_eq!(serde_json::json!(response.commands.into_iter().map(command).collect::<Vec<_>>()),c["commands"],"case{} commands",c["id"]);
        assert_eq!(serde_json::json!(bodies.map(snapshot)),c["after"],"case{} complete body state",c["id"]);changed+=usize::from(c["before"]!=c["after"]);
    }assert_eq!(d["cases"].as_array().unwrap().len(),768);assert_eq!((commands,changed),(768,288));
 }
 #[test]
 fn fixed_recovery_drives_native_ordinary_velocity_banks_and_clock_stamps(){
    use crate::{ivp_joint_apply,ivp_motion::{Pose,Prediction}};
    use l4d2_formats::phy::{CompactTopology,CompactEdge};
    let d:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-retirement-fixed-rescue-apply.json")).unwrap();
    let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let v=|a:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|f(&a[i])));let bank=|a:&serde_json::Value|std::array::from_fn(|i|if i<3{f(&a[i])}else{0.});let mut pushes=0;
    for c in d["cases"].as_array().unwrap(){let h=&c["hull"];let hull=CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()};
        let pose=Pose{position:v(&c["origin"]),rotation:Mat3::from_cols_array(&std::array::from_fn(|i|f(&c["pose"][(i%3)*4+i/3])))};
        let b=&c["before"];let input=Body{id:1-w(&c["fixed_side"]) as usize,flags:w(&b["flags"]) as u16,mass:1.,position:v(&b["position"]).to_array()};let clock=f64::from_bits(c["clock"].as_u64().unwrap());
        let r=fixed(&hull,pose,input,v(&b["pending"]),v(&b["linear"]),f(&c["dt"]),v(&c["gravity"]).to_array()).unwrap();
        // The direct-velocity entry consumes only the captured velocity banks,
        // object flags and clock. Pose/inertia/prediction are untouched here.
        let current=Pose{position:v(&b["position"]),rotation:Mat3::IDENTITY};
        let mut body=ivp_joint_apply::Body{id:input.id,flags:input.flags,object_flags:w(&b["object_flags"]),mass:1.,inverse:[0.;4],current,prediction:Prediction{start:current.position,velocity:Vec3::ZERO,rotation_start:[0.,0.,0.,1.],rotation_end:[0.,0.,0.,1.],time:clock,inverse_dt:1.},clock,stamps:std::array::from_fn(|i|f64::from_bits(b["stamps"][i].as_u64().unwrap())),pending_angular:[0.;4],pending_linear:bank(&b["pending"]),angular:[0.;4],linear:bank(&b["linear"])};
        if let Some(delta)=r.escape.and_then(|e|e.push){body.push_velocity(delta.to_array()).unwrap();pushes+=1;}
        let a=&c["after"];assert_eq!(serde_json::json!(body.pending_linear[..3].iter().map(|x|x.to_bits()).collect::<Vec<_>>()),a["pending"],"case{} pending",c["id"]);assert_eq!(serde_json::json!(body.stamps.map(f64::to_bits)),a["stamps"],"case{} stamps",c["id"]);assert_eq!(serde_json::json!(body.linear[..3].iter().map(|x|x.to_bits()).collect::<Vec<_>>()),a["linear"]);assert_eq!(body.flags as u32,w(&a["flags"]));assert_eq!(body.object_flags,w(&a["object_flags"]));
    }assert_eq!(pushes,1008);assert_eq!(d["cases"].as_array().unwrap().len(),3456);
 }
 #[test]
 fn actual_fixed_ledge_handler_matches_planes_escape_direction_and_direct_push(){
    use l4d2_formats::phy::{CompactTopology,CompactEdge};
    let d:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-retirement-fixed-rescue.json")).unwrap();
    let word=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(word(v));let vector=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));let mut counts=[0;3];
    assert_eq!(d["constants"],serde_json::json!([1036831949u32,3769465068u32]));
    for c in d["cases"].as_array().unwrap(){let h=&c["hull"];let topology=CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(word(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()};
        let pose=crate::ivp_motion::Pose{position:vector(&c["origin"]),rotation:Mat3::from_cols_array(&std::array::from_fn(|i|f(&c["pose"][(i%3)*4+i/3])))};
        let b=&c["before"];let body=Body{id:1-word(&c["fixed_side"]) as usize,flags:word(&b["flags"]) as u16,mass:1.,position:vector(&b["position"]).to_array()};
        let r=fixed(&topology,pose,body,vector(&b["pending"]),vector(&b["linear"]),f(&c["dt"]),vector(&c["gravity"]).to_array()).unwrap();
        assert_eq!(r.strength.to_bits(),word(&c["strength"]),"case{} strength",c["id"]);
        if let Some(e)=r.escape{assert_eq!(serde_json::json!(e.planes.iter().map(|p|p.map(f32::to_bits)).collect::<Vec<_>>()),c["planes"],"case{} planes",c["id"]);assert_eq!(serde_json::json!(e.local_normal.to_array().map(f32::to_bits)),c["normal"],"case{} normal",c["id"]);assert_eq!(serde_json::json!(e.push.map(|v|v.to_array().map(f32::to_bits))),c["push"],"case{} push",c["id"]);counts[if e.push.is_some(){1}else{2}]+=1;
        }else{assert!(c["normal"].is_null()&&c["push"].is_null());counts[0]+=1;}
    }assert_eq!(counts,[1728,1008,720]);
 }
 #[test]
 fn actual_default_handler_recovery_matches_all_native_command_packets(){
    let d:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-retirement-rescue.json")).unwrap();
    let f=|v:&serde_json::Value|f32::from_bits(v.as_u64().unwrap() as u32);let mut commands=0;
    assert_eq!(d["constants"][0],4f32.to_bits());assert_eq!(d["constants"][1],0.01f64.to_bits());assert_eq!(d["constants"][2],786163455u32);
    for c in d["cases"].as_array().unwrap(){let b=std::array::from_fn(|i|{let p=&c["before"][i];Body{id:i,flags:p["flags"].as_u64().unwrap() as u16,mass:f(&p["mass"]),position:std::array::from_fn(|j|f(&p["position"][j]))}});
        let r=dynamic(b,f(&c["dt"]),std::array::from_fn(|i|f(&c["gravity"][i]))).unwrap();assert_eq!(r.strength.to_bits(),c["scale"].as_u64().unwrap() as u32,"case{} strength",c["id"]);
        let observed:Vec<_>=r.commands.into_iter().map(crate::ivp_joint_apply::tests::command).collect();assert_eq!(serde_json::json!(observed),c["commands"],"case{} commands",c["id"]);commands+=observed.len();
    }assert_eq!(d["cases"].as_array().unwrap().len(),768);assert_eq!(commands,768);
 }
}
