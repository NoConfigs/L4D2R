//! Ordinary public SetVelocity numeric writes and100f0c50 caps. The owner
//! supplies native wake/clock effects; external motion-controller wrappers differ.
use bevy::prelude::{Vec2,Vec3};
use crate::ivp_joint_apply::Body;
#[cfg(any(target_arch="x86",target_arch="x86_64"))]
fn length(v:[f32;3],limit:f32)->(f32,bool){let mut rounded=0f32;let mut less=0u8;unsafe{core::arch::asm!("fld dword ptr [{v}+8]","fmul dword ptr [{v}+8]","fld dword ptr [{v}]","fmul dword ptr [{v}]","faddp st(1), st(0)","fld dword ptr [{v}+4]","fmul dword ptr [{v}+4]","faddp st(1), st(0)","fsqrt","fst dword ptr [{rounded}]","fld dword ptr [{limit}]","fcomip st(0), st(1)","setb byte ptr [{less}]","fstp st(0)",v=in(reg)v.as_ptr(),limit=in(reg)&limit,rounded=in(reg)&mut rounded,less=in(reg)&mut less,options(nostack));}(rounded,less!=0)}
#[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
fn length(_: [f32;3],_:f32)->(f32,bool){(0.,false)}
fn clip(v:&mut[f32;4],limit:f32){let (len,_)=length([v[0],v[1],v[2]],limit);if limit<len{let factor=limit/len;for x in &mut v[..3]{*x*=factor;}}}
pub(crate) fn assign(body:&mut Body,limits:Vec2,linear:Option<Vec3>,angular:Option<Vec3>)->Result<(),&'static str>{
    if body.flags&2!=0{return Ok(());}
    if !cfg!(any(target_arch="x86",target_arch="x86_64")){return Err("native public velocity arithmetic backend unavailable");}
    if !limits.is_finite()||limits.min_element()<=0.||linear.is_some_and(|v|!v.is_finite())||angular.is_some_and(|v|!v.is_finite()){return Err("invalid public velocity parameters");}
    let mut next=*body;
    if let Some(v)=linear{let factor=f32::from_bits(0x3cd013a9);next.pending_linear[..3].copy_from_slice(&[v.x*factor,-(v.z*factor),v.y*factor]);next.linear[..3].fill(0.);}
    if let Some(v)=angular{let factor=f32::from_bits(0x3c8efa35);next.pending_angular[..3].copy_from_slice(&[v.x*factor,-(v.z*factor),v.y*factor]);next.angular[..3].fill(0.);}
    clip(&mut next.linear,limits.x);clip(&mut next.pending_linear,limits.x);clip(&mut next.angular,limits.y);
    // Installed100f0c50 tests pending LINEAR length in its final angular branch.
    let (len,exceeded)=length([next.pending_linear[0],next.pending_linear[1],next.pending_linear[2]],limits.y);
    if exceeded{let factor=limits.y/len;for x in &mut next.pending_angular[..3]{*x*=factor;}}
    *body=next;Ok(())
}
