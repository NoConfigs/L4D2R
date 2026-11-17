//! Owned100fd100 pair projection,100fdd70 work cap and100fd3a0 queued
//! updates. Unit flags, group decay and pending commit remain caller stages.
use bevy::prelude::*;
use crate::ivp_contact::Core;
#[derive(Clone,Copy)]
pub(crate) struct Body {pub flags:u16,pub mass:f32,pub inertia:Vec3,pub rotation:Mat3,pub core:Core}
#[derive(Clone,Copy)]
pub(crate) struct Settings {pub epsilon:f32,pub one:f32,pub opposite:f32,pub two:f32,pub half:f32,pub huge:f32,pub small:f32,pub fraction:f32}
pub(crate) struct Projection {
    pub order:[usize;2],pub direction:Vec3,pub axes:[Vec3;2],
    pub linear_speed:f32,pub mass:[f32;2],pub inverse_mass:[f32;2],
    pub angular_speed:f32,pub inertia:[f32;2],pub inverse_inertia:[f32;2],
}
pub(crate) struct Outcome {pub projection:Projection,pub available:[f32;2],pub total:f32,pub capped:f32,pub ratio:f32,pub consumed:f32,pub applied:bool}
fn rotate(m:Mat3,v:Vec3)->Vec3{let r=m.transpose();Vec3::new((r.x_axis.y*v.y+v.x*r.x_axis.x)+r.x_axis.z*v.z,(r.y_axis.x*v.x+r.y_axis.y*v.y)+r.y_axis.z*v.z,(r.z_axis.x*v.x+r.z_axis.y*v.y)+r.z_axis.z*v.z)}
fn inverse(m:Mat3,v:Vec3)->Vec3{Vec3::new((m.x_axis.y*v.y+v.x*m.x_axis.x)+m.x_axis.z*v.z,(m.y_axis.x*v.x+m.y_axis.y*v.y)+m.y_axis.z*v.z,(m.z_axis.x*v.x+m.z_axis.y*v.y)+m.z_axis.z*v.z)}
fn normalize(v:&mut Vec3,k:&Settings)->f32{
    let square=(v.x*v.x+v.y*v.y)+v.z*v.z;
    if square<k.epsilon{return 0.;}
    let inverse=1./(square as f64).sqrt();
    *v=Vec3::new((v.x as f64*inverse) as f32,(v.y as f64*inverse) as f32,(v.z as f64*inverse) as f32);
    (inverse*square as f64) as f32
}
fn effective(inertia:Vec3,axis:Vec3,k:&Settings)->(f32,f32){
    let p=inertia*axis;
    let length=((p.y as f64*p.y as f64+p.x as f64*p.x as f64)+p.z as f64*p.z as f64).sqrt();
    if length>=k.epsilon as f64{let value=length as f32;(value,k.one/value)}else{(k.one,k.one)}
}
pub(crate) fn project(bodies:&[Body;2],k:&Settings)->Result<Projection,&'static str>{
    if ![k.epsilon,k.one,k.opposite,k.two,k.half,k.huge,k.small,k.fraction].iter().all(|v|v.is_finite())||k.epsilon<=0.
        ||bodies.iter().any(|b|!b.mass.is_finite()||b.mass<=0.||!b.inertia.is_finite()||b.inertia.min_element()<=0.||!b.rotation.is_finite()||!b.core.inverse_mass.is_finite()||b.core.inverse_mass<0.||!b.core.linear.is_finite()||!b.core.angular.is_finite()||!b.core.pending_linear.is_finite()||!b.core.pending_angular.is_finite()){
        return Err("invalid pair work projection input");
    }
    let order=if bodies[1].flags&0x12==0{[0,1]}else{[1,0]};let a=bodies[order[0]];let b=bodies[order[1]];
    let mut direction=b.core.linear-a.core.linear;let linear_speed=normalize(&mut direction,k);
    let mut angular=rotate(b.rotation,b.core.angular)-rotate(a.rotation,a.core.angular);let angular_speed=normalize(&mut angular,k);
    let axes=[inverse(a.rotation,angular),inverse(b.rotation,angular*k.opposite)];
    let (mut i0,mut inverse_i0)=effective(a.inertia,axes[0],k);let (i1,inverse_i1)=effective(b.inertia,axes[1],k);
    let mut mass=[a.mass,b.mass];let mut inverse_mass=[a.core.inverse_mass,b.core.inverse_mass];
    if a.flags&0x12!=0{mass[0]=mass[1]*k.huge;inverse_mass[0]=inverse_mass[1]*k.small;i0=i1*k.huge;inverse_i0=inverse_i1*k.small;}
    Ok(Projection{order,direction,axes,linear_speed,mass,inverse_mass,angular_speed,inertia:[i0,i1],inverse_inertia:[inverse_i0,inverse_i1]})
}
fn available(speed:f32,mass:[f32;2],inverse:[f32;2],k:&Settings)->f32{
    let scale=speed/(inverse[0]+inverse[1]);let left=inverse[0]*scale;let right=speed-inverse[1]*scale;
    let value=((mass[1]*speed)*speed+k.epsilon)-((mass[0]*left)*left+(mass[1]*right)*right);
    value.max(0.)*k.half
}
fn delta(speed:f32,energy:f32,ratio:f32,inverse:f32,k:&Settings)->f32{
    let subtract=((energy as f64*ratio as f64)*k.two as f64)*inverse as f64;
    let difference=speed as f64-((speed as f64*speed as f64)-subtract).abs().sqrt();
    ((1./inverse as f64)*difference) as f32
}
pub(crate) fn dissipate(bodies:&mut [Body;2],requested:f32,k:&Settings)->Result<Outcome,&'static str>{
    if !requested.is_finite()||requested<0.{return Err("invalid pair work amount");}
    let p=project(bodies,k)?;
    let angular=available(p.angular_speed,p.inertia,p.inverse_inertia,k);let linear=available(p.linear_speed,p.mass,p.inverse_mass,k);
    let total=linear+angular;let mut capped=requested;let cap=total*k.fraction;if capped>cap{capped=cap;}
    if total<k.epsilon{return Ok(Outcome{projection:p,available:[angular,linear],total,capped,ratio:0.,consumed:0.,applied:false});}
    let ratio=capped/total;
    let angular_delta=delta(p.angular_speed,angular,ratio,p.inverse_inertia[0]+p.inverse_inertia[1],k);
    let linear_delta=delta(p.linear_speed,linear,ratio,p.inverse_mass[0]+p.inverse_mass[1],k);
    if ![angular_delta,linear_delta,ratio].iter().all(|v|v.is_finite()){return Err("unreconstructed singular pair work response");}
    let [a,b]=p.order;
    if bodies[a].flags&0x12==0{
        let linear_scale=p.inverse_mass[0]*linear_delta;let angular_scale=p.inverse_inertia[0]*angular_delta;
        bodies[a].core.pending_linear+=p.direction*linear_scale;bodies[a].core.pending_angular+=p.axes[0]*angular_scale;
    }
    let linear_scale=linear_delta*p.inverse_mass[1];let angular_scale=p.inverse_inertia[1]*angular_delta;
    bodies[b].core.pending_linear+=(p.direction*k.opposite)*linear_scale;bodies[b].core.pending_angular+=p.axes[1]*angular_scale;
    Ok(Outcome{projection:p,available:[angular,linear],total,capped,ratio,consumed:capped,applied:true})
}

#[cfg(test)]
mod tests {
    use super::*;use serde_json::Value;
    fn f(v:&Value)->f32{v.as_f64().unwrap() as f32}
    fn v(a:&Value)->Vec3{Vec3::from_array(std::array::from_fn(|i|f(&a[i])))}
    fn bodies(a:&Value)->[Body;2]{std::array::from_fn(|i|{let b=&a[i];Body{flags:b["flags"].as_u64().unwrap() as u16,mass:f(&b["mass"]),inertia:v(&b["inertia"]),rotation:Mat3::from_cols_array(&std::array::from_fn(|i|f(&b["rotation_rows"][i]))).transpose(),core:Core{inverse_mass:f(&b["inverse_mass"]),inverse_inertia:v(&b["inverse_inertia"]),linear:v(&b["linear"]),angular:v(&b["angular"]),pending_linear:v(&b["pending_linear"]),pending_angular:v(&b["pending_angular"])}}})}
    fn settings(a:&Value)->Settings{Settings{epsilon:f(&a["epsilon"]),one:f(&a["one"]),opposite:f(&a["opposite"]),two:f(&a["two"]),half:f(&a["half"]),huge:f(&a["huge"]),small:f(&a["small"]),fraction:f(&a["fraction"])}}
    fn same(a:f32,b:f32){assert_eq!(a.to_bits(),b.to_bits(),"{a} != {b}");}
    fn same_v(a:Vec3,b:Vec3){for (a,b) in a.to_array().into_iter().zip(b.to_array()){same(a,b);}}
    fn same_b(a:[Body;2],b:[Body;2]){for i in 0..2{assert_eq!(a[i].flags,b[i].flags);same(a[i].mass,b[i].mass);same_v(a[i].inertia,b[i].inertia);same(a[i].core.inverse_mass,b[i].core.inverse_mass);same_v(a[i].core.inverse_inertia,b[i].core.inverse_inertia);for (x,y) in a[i].rotation.to_cols_array().into_iter().zip(b[i].rotation.to_cols_array()){same(x,y);}same_v(a[i].core.linear,b[i].core.linear);same_v(a[i].core.angular,b[i].core.angular);same_v(a[i].core.pending_linear,b[i].core.pending_linear);same_v(a[i].core.pending_angular,b[i].core.pending_angular);}}
    #[test]
    fn native_pair_projection_caps_and_pending_dissipation_match(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-dissipation.json")).unwrap();let mut retained=None;
        let (mut count,mut reused,mut real)=(0,0,0);
        for row in data["cases"].as_array().unwrap(){
            let mut b=if row["repeat"]==1{reused+=1;retained.take().unwrap()}else{bodies(&row["before"])};same_b(b,bodies(&row["before"]));
            let out=dissipate(&mut b,f(&row["requested"]),&settings(&row["constants"])).unwrap();let p=&out.projection;
            assert_eq!(p.order,std::array::from_fn(|i|row["order"][i].as_u64().unwrap() as usize));same_v(p.direction,v(&row["direction"]));same_v(p.axes[0],v(&row["axis0"]));same_v(p.axes[1],v(&row["axis1"]));
            let values=[p.linear_speed,p.mass[0],p.mass[1],p.inverse_mass[0],p.inverse_mass[1],p.angular_speed,p.inertia[0],p.inertia[1],p.inverse_inertia[0],p.inverse_inertia[1]];
            for (i,a) in values.into_iter().enumerate(){same(a,f(&row["projection"][i]));}
            for (i,a) in out.available.into_iter().enumerate(){same(a,f(&row["available"][i]));}same(out.total,f(&row["total"]));same(out.capped,f(&row["capped"]));same(out.ratio,f(&row["ratio"]));same(out.consumed,f(&row["consumed"]));assert_eq!(out.applied,row["applied"].as_bool().unwrap());same_b(b,bodies(&row["after"]));
            if row["repeat"]==0{retained=Some(b);}if row["step"].as_i64().unwrap()>0{real+=1;}count+=1;
        }
        assert_eq!((count,reused,real),(1702,768,166));
    }
    #[test]
    fn native_group_clear_decay_pending_and_environment_work_compose(){
        use crate::ivp_contact_pipeline::FrictionGroup;
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-dissipation.json")).unwrap();let (mut calls,mut cleared,mut skipped)=(0,0,0);
        for row in data["group_phases"].as_array().unwrap(){let before=&row["before"][0];let after=&row["after"][0];let flags=row["flags"].as_u64().unwrap() as u32;
            let mut group=FrictionGroup{timer:before["timer"].as_i64().unwrap() as i32,energy:f(&before["energy"])};
            let mut b=bodies(&before["bodies"]);let mut work=f(&before["environment_work"]);let k=settings(&row["constants"]);
            let result=group.refresh_work(flags,f(&before["coefficient"]),std::array::from_fn(|i|before["blocked"][i].as_u64().unwrap()!=0),&mut b,&mut work,&k).unwrap();
            assert_eq!(group.timer,after["timer"].as_i64().unwrap() as i32);same(group.energy,f(&after["energy"]));same(work,f(&after["environment_work"]));same_b(b,bodies(&after["bodies"]));
            if flags&0x3000!=0{cleared+=1;}if result.is_some(){calls+=1;}else{skipped+=1;}
            // Failed caller input cannot partially clear or queue core state.
            let saved_group=group;let saved_b=b;let saved_work=work;
            assert!(group.refresh_work(0x3000,f32::NAN,[false;2],&mut b,&mut work,&k).is_err());assert_eq!(group,saved_group);same_b(b,saved_b);same(work,saved_work);
        }
        assert_eq!((calls,cleared,skipped),(166,6,21));
    }
    #[test]
    fn native_geometry_work_then_clear_decay_and_pending_dissipation_compose(){
        use crate::ivp_contact_pipeline::{DistanceChange,FrictionGroup};
        let d:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-geometry-work.json")).unwrap();let (mut positive,mut nonpositive)=(0,0);
        for p in d["phases"].as_array().unwrap(){let g=&p["geometry"];let r=&p["refresh"];let before=&r["before"][0];let after=&r["after"][0];
            let mut group=FrictionGroup{timer:before["timer"].as_i64().unwrap() as i32,energy:f(&g["energy_before"])};
            let contacts=g["contacts"].as_array().unwrap().iter().map(|c|DistanceChange{before:f(&c["distance_before"]),after:f(&c["distance_after"]),force:f(&c["force_after"])}).collect::<Vec<_>>();
            let delta=group.geometry_work(&contacts).unwrap();same(group.energy,f(&g["energy_after"]));if delta>0.{positive+=1;}else{nonpositive+=1;}
            let mut b=bodies(&before["bodies"]);let mut work=f(&before["environment_work"]);
            group.refresh_work(r["flags"].as_u64().unwrap() as u32,f(&before["coefficient"]),std::array::from_fn(|i|before["blocked"][i].as_u64().unwrap()!=0),&mut b,&mut work,&settings(&p["constants"])).unwrap();
            same(group.energy,f(&after["energy"]));same(work,f(&after["environment_work"]));same_b(b,bodies(&after["bodies"]));
            let saved=group;let mut invalid=contacts;invalid.last_mut().unwrap().after=f32::NAN;
            assert!(group.geometry_work(&invalid).is_err());assert_eq!(group,saved);
        }
        assert_eq!((positive,nonpositive),(61,126));
    }

}
