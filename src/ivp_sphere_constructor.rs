//! Fresh-environment public sphere numeric body construction. Object/surface,
//! collision-world allocation and wrapper buoyancy/drag ownership remain separate.
use bevy::prelude::{Mat3,Vec2,Vec3};
use crate::{ivp_joint_apply::Body,ivp_motion::{Pose,Prediction},ivp_movement::History,ivp_group_members::Object,ivp_registered_physical::Properties};
#[derive(Clone,Copy,Debug)]
pub(crate) struct Parameters{pub radius:f32,pub position:[f32;3],pub angles:[f32;3],pub mass:f32,pub inertia_scale:f32,pub damping:f32,pub angular_damping:f32,pub rotational_limit:f32,pub fixed:bool,pub collisions:bool}
pub(crate) type Binding=(Object,Body,History,Properties);
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
fn normalize(mut q:[f32;4])->[f32;4]{let sq=((q[0]*q[0]+q[1]*q[1])+q[2]*q[2])+q[3]*q[3];if sq>1e-10{unsafe{core::arch::asm!("fld dword ptr [{sq}]","fsqrt","fld1","fdivrp st(1), st(0)","fld dword ptr [{q}]","fmul st(0), st(1)","fstp dword ptr [{q}]","fld dword ptr [{q}+4]","fmul st(0), st(1)","fstp dword ptr [{q}+4]","fld dword ptr [{q}+8]","fmul st(0), st(1)","fstp dword ptr [{q}+8]","fmul dword ptr [{q}+12]","fstp dword ptr [{q}+12]",sq=in(reg)&sq,q=in(reg)q.as_mut_ptr(),options(nostack,preserves_flags));}}q}
#[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
fn normalize(q:[f32;4])->[f32;4]{q}
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
fn root_half(a:f32,b:f32,c:f32,positive:bool)->(f32,f32){let mut root=0f32;let mut half_root=0f32;let half=0.5f32;unsafe{if positive{core::arch::asm!("fld dword ptr [{a}]","fld1","faddp st(1), st(0)","fsqrt","fst dword ptr [{root}]","fmul dword ptr [{half}]","fstp dword ptr [{out}]",a=in(reg)&a,root=in(reg)&mut root,half=in(reg)&half,out=in(reg)&mut half_root,options(nostack,preserves_flags));}else{core::arch::asm!("fld dword ptr [{c}]","fadd dword ptr [{b}]","fsubr dword ptr [{a}]","fld1","faddp st(1), st(0)","fsqrt","fst dword ptr [{root}]","fmul dword ptr [{half}]","fstp dword ptr [{out}]",a=in(reg)&a,b=in(reg)&b,c=in(reg)&c,root=in(reg)&mut root,half=in(reg)&half,out=in(reg)&mut half_root,options(nostack,preserves_flags));}}(root,half_root)}
#[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
fn root_half(_:f32,_:f32,_:f32,_:bool)->(f32,f32){(0.,0.)}
fn quaternion(m:[[f32;3];3])->[f32;4]{
    let trace=(m[0][0]+m[1][1])+m[2][2];let mut q=[0.;4];
    if trace>0.{let(root,half)=root_half(trace,0.,0.,true);let s=0.5/root;q=[(m[2][1]-m[1][2])*s,(m[0][2]-m[2][0])*s,(m[1][0]-m[0][1])*s,half];}
    else{let mut i=usize::from(m[0][0]<m[1][1]);if m[i][i]<m[2][2]{i=2;}let(j,k)=((i+1)%3,(i+2)%3);let(root,half)=root_half(m[i][i],m[j][j],m[k][k],false);let s=if root!=0.{0.5/root}else{root};q[i]=half;q[3]=(m[k][j]-m[j][k])*s;q[j]=(m[j][i]+m[i][j])*s;q[k]=(m[k][i]+m[i][k])*s;}
    normalize(q)
}
fn angle_quaternion(angles:[f32;3])->[f32;4]{
    use crate::ivp_ragdoll_constructor::{sin,cos};let rad=f32::from_bits(0x3c8efa35);let [pitch,yaw,roll]=angles.map(|x|x*rad);let(cy,sy,cp,sp,cr,sr)=(cos(yaw),sin(yaw),cos(pitch),sin(pitch),cos(roll),sin(roll));
    let forward=[cp*cy,cp*sy,-sp];let right=[cr*sy-(sr*sp)*cy,(-cr)*cy-(sr*sp)*sy,(-sr)*cp];let up=[(cr*sp)*cy+sr*sy,(cr*sp)*sy-sr*cy,cr*cp];
    //1007c700 maps the AngleVectors basis into a native row-major matrix.
    let m=[[forward[0],-up[0],-right[0]],[-forward[2],up[2],right[2]],[forward[1],-up[1],-right[1]]];
    quaternion(m)
}
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
fn inertia_cap(i:f32,limit:f32)->f32{let mut out=0f32;unsafe{core::arch::asm!("fld dword ptr [{i}]","fmul dword ptr [{i}]","fld st(0)","fadd st(0), st(1)","faddp st(1), st(0)","fsqrt","fmul dword ptr [{limit}]","fstp dword ptr [{out}]",i=in(reg)&i,limit=in(reg)&limit,out=in(reg)&mut out,options(nostack,preserves_flags));}out}
#[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
fn inertia_cap(_:f32,_:f32)->f32{0.}
/// Native object-to-core identity-frame composition and both quaternion conversions.
pub(crate) fn initial_rotation(angles:[f32;3])->(Mat3,[f32;4]){
    let q=normalize(angle_quaternion(angles));let rotation=crate::ivp_motion::matrix(q);let rows=rotation.transpose().to_cols_array();
    let m: [[f32;3];3]=std::array::from_fn(|r|std::array::from_fn(|c|{let e=|k|if k==c{1.}else{0.};(rows[r*3+1]*e(1)+rows[r*3]*e(0))+rows[r*3+2]*e(2)}));
    let rotation=Mat3::from_cols_array(&std::array::from_fn(|i|m[i/3][i%3])).transpose();let q=quaternion(m);
    (rotation,q)
}
pub(crate) fn construct(id:usize,p:Parameters,dt:f32)->Result<Binding,&'static str>{
    if !cfg!(any(target_arch="x86",target_arch="x86_64")){return Err("native sphere arithmetic backend unavailable");}
    if !dt.is_finite()||dt<=0.||!p.radius.is_finite()||p.radius<=0.||p.position.into_iter().chain(p.angles).chain([p.mass,p.inertia_scale,p.damping,p.angular_damping,p.rotational_limit]).any(|x|!x.is_finite())||p.damping<0.||p.angular_damping<0.{return Err("invalid sphere numeric parameters");}
    let unit=f32::from_bits(0x3cd013a9);let radius=p.radius*unit;let mass=p.mass.clamp(0.1,50000.);let scale=if p.inertia_scale>0.{p.inertia_scale.min(1e18)}else{1.};let mut inertia=(scale*((radius*0.4)*radius))*mass;
    if p.rotational_limit!=0.{let cap=inertia_cap(inertia,p.rotational_limit);if inertia<cap{inertia=cap;}}
    if !inertia.is_finite()||inertia>1e18{inertia=1e18;}else if inertia < -1e18{inertia=-1e18;}
    let (rotation,q)=initial_rotation(p.angles);
    // Identity object-to-core translation still transforms its zero offset.
    // Preserve that composition: its signed zeros depend on the rotation,
    // so clamping zero components would lose native oblique-origin signs.
    let position=rotation*Vec3::ZERO+Vec3::new(p.position[0]*unit,-(p.position[2]*unit),p.position[1]*unit);let inverse=[inertia.recip(),inertia.recip(),inertia.recip(),mass.recip()];
    let body=Body{id,flags:0x808|if p.fixed{2}else{0},object_flags:0x800|if p.fixed{16}else{8}|if p.collisions{0x100}else{0},mass,inverse,current:Pose{position,rotation},prediction:Prediction{start:position,velocity:Vec3::ZERO,rotation_start:q,rotation_end:q,time:0.,inverse_dt:66.},clock:0.,stamps:[0.;2],pending_angular:[0.;4],pending_linear:[0.;4],angular:[0.;4],linear:[0.;4]};
    let history=History{close_time:0.,far_time:0.,close_position:Vec3::ZERO,far_position:Vec3::ZERO,close_quat:[0.;4],far_quat:[0.;4]};
    let properties=Properties{radius,sleep_time:0.3,linear_rate:p.damping,angular_rate:Vec3::splat(p.angular_damping),inertia:Vec3::splat(inertia),limits:Vec2::new(50.8,0.95199776*dt.recip()),angular_limit_exact:0.95199776f32 as f64*dt.recip() as f64,gyro:false};
    Ok((Object{id,core:id,fixed:p.fixed},body,history,properties))
}

/// A late-created core keeps zero constructor history/cache time, but uses the
/// current environment clock and the remaining interval's inverse timestep.
pub(crate) fn construct_at(id:usize,p:Parameters,dt:f32,context:crate::ivp_group_effects::Context)->Result<Binding,&'static str>{
    let inverse=context.inverse_dt()?;let mut binding=construct(id,p,dt)?;binding.1.clock=context.now;binding.1.prediction.inverse_dt=inverse;Ok(binding)
}

#[cfg(test)]mod zero_origin_tests{
 use super::*;
 #[test]fn native_sphere_origins_compose_rotated_zero_offsets(){
  let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-zero-origin.json")).unwrap();
  for c in data["cases"].as_array().unwrap(){let v=|name:&str|std::array::from_fn(|i|f32::from_bits(c[name][i].as_u64().unwrap() as u32));
   let (_,body,_,_)=construct(0,Parameters{radius:16.1,position:v("position_bits"),angles:v("angle_bits"),mass:10.,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:c["fixed"].as_bool().unwrap(),collisions:true},1./60.).unwrap();
   assert_eq!(crate::ivp_joint_apply::tests::snapshot(body),c["physical"],"zero origin {}",c["scenario"]);
  }
 }
}
