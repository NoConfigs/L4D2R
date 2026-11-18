//!1011d320 numerical normal-response composition after list preparation.
//! Includes selected Gaussian validation, native active-set fallback, retained
//! force/sign updates and energy commit/discard. Discovery and anomaly/split
//! providers are not replaced by independent per-contact responses.
use bevy::prelude::*;
use crate::{ivp_contact::{self as normal,Core,NormalSystem},ivp_contact_pipeline::RetainedContact,ivp_energy as energy};
#[cfg(test)]
thread_local!{pub(crate) static BATCH_AUDIT:std::cell::RefCell<Option<Vec<serde_json::Value>>>=const{std::cell::RefCell::new(None)};}
#[cfg(test)]
fn audit_cores(cores:&[Core])->serde_json::Value{serde_json::json!(cores.iter().map(|c|serde_json::json!({"linear":c.linear.to_array(),"angular":c.angular.to_array(),"pending_linear":c.pending_linear.to_array(),"pending_angular":c.pending_angular.to_array()})).collect::<Vec<_>>())}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Physical {pub flags:u16,pub mass:f32,pub inertia:Vec3}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Report {pub solved:bool,pub fast:bool,pub committed:bool}
pub(crate) fn respond(contacts:&mut [&mut RetainedContact],cores:&mut [Core],physical:&[Physical],participating:&[usize],dt:f32,allowance_factor:f32,bound:f32)->Result<Report,&'static str>{
    if physical.len()!=cores.len()||contacts.is_empty()||contacts.len()>150||!dt.is_finite()||dt<=0.||!dt.recip().is_finite()
        ||!allowance_factor.is_finite()||allowance_factor<0.||!bound.is_finite()||bound<0.
        ||participating.iter().any(|&i|i>=cores.len()||physical[i].flags&2!=0)
        ||participating.iter().copied().collect::<std::collections::BTreeSet<_>>().len()!=participating.len()
        ||physical.iter().any(|p|!p.mass.is_finite()||p.mass<0.||!p.inertia.is_finite()||p.inertia.min_element()<0.)
        ||cores.iter().any(|c|!c.linear.is_finite()||!c.angular.is_finite()||!c.pending_linear.is_finite()||!c.pending_angular.is_finite()){
        return Err("invalid/unsupported coupled normal boundary");
    }
    let rows=contacts.iter().map(|c|c.normal_row()).collect::<Result<Vec<_>,_>>()?;
    if rows.iter().flat_map(|r|r.bodies.iter().flatten()).any(|i|!participating.contains(i)){return Err("normal row lacks participating dynamic core");}
    let mut system=normal::assemble(cores,&rows,0.0126949195,20.).ok_or("invalid coupled normal assembly")?;
    #[cfg(test)] let unnormalized=serde_json::json!({"matrix":system.matrix,"rhs":system.rhs});
    let scale=normal::normalize(&mut system,1e-10).ok_or("invalid coupled normal normalization")?;
    let modes=contacts.iter().map(|c|c.state().normal.counter).collect::<Vec<_>>();let indices=(0..rows.len()).filter(|&i|modes[i]!=0).collect::<Vec<_>>();let warm=modes.iter().take_while(|&&m|m<0).count();let n=rows.len();
    let mut selected=NormalSystem{matrix:indices.iter().flat_map(|&i|indices.iter().map(move |&j|(i,j))).map(|(i,j)|system.matrix[i*n+j]).collect(),rhs:indices.iter().map(|&i|system.rhs[i]).collect()};
    let candidate=normal::gaussian(&mut selected,9.99999972e-10,1000.).ok_or("invalid selected normal solve")?;
    let weights=contacts.iter().map(|c|c.geometry().unwrap().response).collect::<Vec<_>>();
    let fast=normal::validate_fast(&system,scale,&indices,&candidate.solution,&weights,0.0982999951).ok_or("invalid selected normal candidate")?;
    let used_fast=candidate.success&&fast.accepted;
    let impulses=if used_fast{fast.impulses}else{let Some(x)=crate::ivp_active::lcp_trial::solve_owned_normal(&system.matrix,&system.rhs,warm)else{return Ok(Report{solved:false,fast:false,committed:false});};x};
    #[cfg(test)] BATCH_AUDIT.with(|audit|{if let Some(a)=audit.borrow_mut().as_mut(){a.push(serde_json::json!({"time":contacts[0].state().refresh.time.to_bits(),"before":unnormalized,"normalized":{"matrix":system.matrix,"rhs":system.rhs},"scale":scale,"indices":indices,"candidate":candidate.solution,"candidate_success":candidate.success,"weights":weights,"fast":used_fast,"warm":warm,"impulses":impulses,"cores_before":audit_cores(cores),"rows":rows.iter().map(|r|serde_json::json!({"owners":r.bodies,"normal":r.normal.to_array(),"jacobian":r.jacobian.map(|v|v.to_array())})).collect::<Vec<_>>()}));}});
    let mut next_cores=cores.to_vec();let mut next_contacts=contacts.iter().map(|c|(**c).clone()).collect::<Vec<_>>();
    let energy_cores=|values:&[Core]|participating.iter().map(|&i|{let c=values[i];let p=physical[i];energy::Core{flags:p.flags,mass:p.mass,inertia:p.inertia,linear:c.linear,angular:c.angular,pending_linear:c.pending_linear,pending_angular:c.pending_angular}}).collect::<Vec<_>>();
    let before=energy_cores(&next_cores);
    for (c,x) in next_contacts.iter_mut().zip(impulses){c.apply_batch_normal(&mut next_cores,x*scale,false,dt,dt.recip())?;}
    let mut after=energy_cores(&next_cores);let committed=energy::accepts(&before,&after,allowance_factor,bound);energy::finish(&mut after,committed);
    for (&i,a) in participating.iter().zip(after){let c=&mut next_cores[i];c.linear=a.linear;c.angular=a.angular;c.pending_linear=a.pending_linear;c.pending_angular=a.pending_angular;}
    #[cfg(test)] BATCH_AUDIT.with(|audit|{if let Some(a)=audit.borrow_mut().as_mut(){let row=a.last_mut().unwrap();row["committed"]=serde_json::json!(committed);row["cores_after"]=audit_cores(&next_cores);}});
    if next_cores.iter().any(|c|!c.linear.is_finite()||!c.angular.is_finite()||!c.pending_linear.is_finite()||!c.pending_angular.is_finite()){return Err("coupled normal response overflow");}
    cores.copy_from_slice(&next_cores);for(c,next)in contacts.iter_mut().zip(next_contacts){**c=next;}
    Ok(Report{solved:true,fast:used_fast,committed})
}
