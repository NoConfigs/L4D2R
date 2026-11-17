//! Public non-breakable hinge numeric construction (100870c0/100b6af0).
//! Bodies and their object frames are existing inputs; allocator/listener/world
//! effects and public ragdoll-to-hinge selection remain separate.
use bevy::prelude::{Mat3,Vec3};
use crate::{ivp_group_complete::Member,ivp_hinge_callback::Joint,ivp_joint_angular::Axis,ivp_motion::ObjectFrame};
#[derive(Clone,Copy,Debug)]
pub(crate) struct Parameters{pub position:[f32;3],pub direction:[f32;3],pub limits:[f32;2],pub velocity:f32,pub torque:f32,pub strength:f32,pub force_limit:f32,pub torque_limit:f32,pub mass_scale:[f32;2]}
fn square(v:[f32;3])->f32{(v[0]*v[0]+v[1]*v[1])+v[2]*v[2]}
fn normalize(v:[f32;3])->[f32;3]{
    let sq=square(v);let mut inverse=0f32;
    if sq!=0.{
        #[cfg(any(target_arch="x86",target_arch="x86_64"))]
        unsafe{core::arch::asm!("fld dword ptr [{sq}]","fsqrt","fld1","fdivrp st(1), st(0)","fstp dword ptr [{out}]",sq=in(reg)&sq,out=in(reg)&mut inverse,options(nostack,preserves_flags));}
        #[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
        {inverse=(1./(sq as f64).sqrt()) as f32;}
    }
    v.map(|x|x*inverse)
}
fn convert(v:[f32;3])->[f32;3]{[v[0],-v[2],v[1]]}
pub(crate) fn inverse(m:Mat3,v:[f32;3])->[f32;3]{std::array::from_fn(|i|{let c=m.col(i);(c[1]*v[1]+c[0]*v[0])+c[2]*v[2]})}
pub(crate) fn forward(m:Mat3,v:[f32;3])->[f32;3]{std::array::from_fn(|i|(m.y_axis[i]*v[1]+m.x_axis[i]*v[0])+m.z_axis[i]*v[2])}
fn perpendicular(v:[f32;3])->[f32;3]{
    let [x,y,z]=v;
    let p=if x.abs()>=0.57{[z-y*0.,x*0.-z*0.,y*0.-x]}else{[z*0.-y*0.,x*0.-z,y-x*0.]};normalize(p)
}
pub(crate) fn construct(p:Parameters,bodies:[Member;2])->Result<Joint,&'static str>{construct_extended(p,bodies,[[0.;3];2])}
pub(crate) fn construct_extended(p:Parameters,bodies:[Member;2],perps:[[f32;3];2])->Result<Joint,&'static str>{
    if !cfg!(any(target_arch="x86",target_arch="x86_64")){return Err("native hinge constructor arithmetic backend unavailable");}
    if p.position.into_iter().chain(p.direction).chain(p.limits).chain([p.velocity,p.torque,p.strength,p.force_limit,p.torque_limit]).chain(p.mass_scale).chain(perps.into_iter().flatten()).any(|v|!v.is_finite()){return Err("non-finite public hinge parameters");}
    if [p.force_limit,p.torque_limit].into_iter().any(|v|v!=0.&&v<f32::MAX)||p.mass_scale.into_iter().any(|v|v!=0.&&v!=1.){return Err("breakable hinge wrapper is not owned");}
    let unit=f32::from_bits(0x3cd013a9);let radians=f32::from_bits(0x3c8efa35);
    let position=convert(p.position.map(|x|x*unit));let world=normalize(convert(p.direction));
    let directions=bodies.map(|m|inverse(m.body.current.rotation,world));
    let anchors=bodies.map(|m|{
        let pose=m.body.prediction.prepare_object_cache(m.body.clock,ObjectFrame{shift:m.object_offset.map(Vec3::from_array),rotation:None},0).pose;
        inverse(pose.rotation,std::array::from_fn(|i|position[i]-pose.position[i]))
    });
    let mut perpendiculars=perps.map(|v|normalize(convert(v)));
    for side in 0..2{
        if square(perpendiculars[side])==0.{
            perpendiculars[side]=if side==0{perpendicular(directions[0])}else{inverse(bodies[1].body.current.rotation,forward(bodies[0].body.current.rotation,perpendiculars[0]))};
        }else{perpendiculars[side]=normalize(perpendiculars[side]);}
    }
    let mut axis=Axis{tau:1.,..Default::default()};let mut limit_enabled=false;
    if p.limits[0]!=p.limits[1]{axis.min=p.limits[0]*radians;axis.max=p.limits[1]*radians;limit_enabled=(axis.min+std::f32::consts::PI).abs()>0.0001||(axis.max-std::f32::consts::PI).abs()>0.0001;}
    if p.torque!=0.{axis.force=(p.torque*radians).abs();axis.velocity=p.velocity*radians;axis.motor=p.torque*radians!=0.;}
    Ok(Joint{anchors,directions,perpendiculars,axis,limit_enabled,tau:p.strength,damping:1.,work:None})
}
