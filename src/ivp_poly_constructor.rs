//! Fresh compiled-poly numeric body creation. Surface metadata is shape input,
//! never a captured body. Shape compilation and contact-world allocation are separate.
use bevy::prelude::{Vec2,Vec3};
use crate::{ivp_sphere_constructor::Binding,ivp_motion::{Pose,Prediction},ivp_movement::History,ivp_group_members::Object,ivp_joint_apply::Body,ivp_registered_physical::Properties};

#[derive(Clone,Copy,Debug)]
pub(crate) struct Surface {pub center:Vec3,pub unit_inertia:Vec3,pub radius:f32,pub factor:u8}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Parameters {
 pub surface:Surface,pub margin:f32,pub center:Option<[f32;3]>,pub position:[f32;3],pub angles:[f32;3],
 pub mass:f32,pub inertia_scale:f32,pub damping:f32,pub angular_damping:f32,pub rotational_limit:f32,pub fixed:bool,pub collisions:bool,
}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Created {pub binding:Binding,pub object_offset:Option<[f32;3]>,pub angular_radius:f32}

//100ec3b0 retains the x87 vector length through multiplication by the allowance.
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
fn inertia_floor(i:Vec3,limit:f32)->f32 {
 let [x,y,z]=i.to_array();let mut out=0f32;
 unsafe {core::arch::asm!("fld dword ptr [{z}]","fmul dword ptr [{z}]","fld dword ptr [{x}]","fmul dword ptr [{x}]","faddp st(1), st(0)","fld dword ptr [{y}]","fmul dword ptr [{y}]","faddp st(1), st(0)","fsqrt","fmul dword ptr [{limit}]","fstp dword ptr [{out}]",x=in(reg)&x,y=in(reg)&y,z=in(reg)&z,limit=in(reg)&limit,out=in(reg)&mut out,options(nostack,preserves_flags));}
 out
}
#[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
fn inertia_floor(_:Vec3,_:f32)->f32{0.}

pub(crate) fn construct(id:usize,p:Parameters,dt:f32)->Result<Created,&'static str>{
 if !cfg!(any(target_arch="x86",target_arch="x86_64")){return Err("native poly arithmetic backend unavailable");}
 if !dt.is_finite()||dt<=0.||!p.surface.center.is_finite()||!p.surface.unit_inertia.is_finite()||p.surface.unit_inertia.min_element()<=0.||!p.surface.radius.is_finite()||p.surface.radius<=0.||p.position.into_iter().chain(p.angles).chain(p.center.into_iter().flatten()).chain([p.margin,p.mass,p.inertia_scale,p.damping,p.angular_damping,p.rotational_limit]).any(|v|!v.is_finite())||p.margin<0.||p.damping<0.||p.angular_damping<0.{return Err("invalid compiled-poly numeric parameters");}
 let unit=f32::from_bits(0x3cd013a9);
 let source=|v:[f32;3]|Vec3::new(v[0]*unit,-(v[2]*unit),v[1]*unit);
 let center=p.center.map(source).unwrap_or(p.surface.center);
 //100ea370 compares the rounded squared inverse-frame translation against
 // DAT1016f3ac (strictly less); it zeros tiny object shifts but the core's
 // translated position still uses the original authored center below.
 let square=(center.y*center.y+center.x*center.x)+center.z*center.z;
 let object_offset=(square>=f32::from_bits(0x3716feb5)).then(||(-center).to_array());
 let (rotation,q)=crate::ivp_sphere_constructor::initial_rotation(p.angles);
 //100ee580 translation: X sums X,Y,Z; Y/Z sum Y,X,Z, then add origin.
 let rows=rotation.transpose().to_cols_array();let origin=source(p.position);
 let translated=Vec3::from_array(std::array::from_fn(|r|{let [x,y,z]=[rows[r*3]*center.x,rows[r*3+1]*center.y,rows[r*3+2]*center.z];let sum=if r==0{(x+y)+z}else{(y+x)+z};origin[r]+sum}));
 let bounds=crate::ivp_bounds::Compact{center:p.surface.center,radius:p.surface.radius,factor:p.surface.factor}.at(center)?;
 let radius=bounds.radius+p.margin*unit;
 let mass=p.mass.clamp(0.1,50000.);let scale=if p.inertia_scale>0.{p.inertia_scale.min(1e18)}else{1.};
 let mut inertia=Vec3::from_array(p.surface.unit_inertia.to_array().map(|i|(scale*i)*mass));
 if p.rotational_limit!=0.{let floor=inertia_floor(inertia,p.rotational_limit);inertia=inertia.max(Vec3::splat(floor));}
 inertia=Vec3::from_array(inertia.to_array().map(|i|if !i.is_finite()||i>1e18{1e18}else{i.max(-1e18)}));
 let [x,y,z]=inertia.to_array().map(f32::recip);let [a,b,c]=[y-z,z-x,x-y];
 let gyro=((x*x+y*y)+z*z)*f32::from_bits(0x3c23d70b)<=(a*a+b*b)+c*c;
 let flags=0x800|if gyro{0}else{8}|if p.fixed{2}else{0};
 let body=Body{id,flags,object_flags:if object_offset.is_none(){0x800}else{0}|if p.fixed{16}else{8}|if p.collisions{0x100}else{0},mass,inverse:[x,y,z,mass.recip()],current:Pose{position:translated,rotation},prediction:Prediction{start:translated,velocity:Vec3::ZERO,rotation_start:q,rotation_end:q,time:0.,inverse_dt:66.},clock:0.,stamps:[0.;2],pending_angular:[0.;4],pending_linear:[0.;4],angular:[0.;4],linear:[0.;4]};
 let history=History{close_time:0.,far_time:0.,close_position:Vec3::ZERO,far_position:Vec3::ZERO,close_quat:[0.;4],far_quat:[0.;4]};
 let properties=Properties{radius,sleep_time:0.3,linear_rate:p.damping,angular_rate:Vec3::splat(p.angular_damping),inertia,limits:Vec2::new(50.8,0.95199776*dt.recip()),angular_limit_exact:0.95199776f32 as f64*dt.recip() as f64,gyro};
 Ok(Created{binding:(Object{id,core:id,fixed:p.fixed},body,history,properties),object_offset,angular_radius:bounds.angular_radius})
}

/// Late creation shares the recovered environment interval with sphere cores.
pub(crate) fn construct_at(id:usize,p:Parameters,dt:f32,context:crate::ivp_group_effects::Context)->Result<Created,&'static str>{
 let inverse=context.inverse_dt()?;let mut created=construct(id,p,dt)?;created.binding.1.clock=context.now;created.binding.1.prediction.inverse_dt=inverse;Ok(created)
}
