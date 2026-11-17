//! Owned100fd5c0/100ef550 mass bound and101026a0 material dispatch contract.
//! Provider implementations and the complete contact graph remain separate.
use bevy::prelude::*;
#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct Material {pub key:u32,pub flags:u32}
#[derive(Clone,Copy)]
pub(crate) struct Endpoint {pub manager:u32,pub code:u8,pub default:Material}
#[derive(Clone,Copy)]
pub(crate) struct Core {pub flags:u16,pub inverse_inertia:Vec3,pub inverse_mass:f32,pub arm:Vec3}
#[derive(Debug,PartialEq)]
pub(crate) struct Setup {pub mode:u8,pub bound:f32,pub materials:[Material;2]}

pub(crate) fn resolve_materials(endpoints:[Endpoint;2],mut provider:impl FnMut(u32,usize,u8)->Material)->[Material;2]{
    std::array::from_fn(|side|{
        let e=endpoints[side];let index=e.code&127;
        if index==0 {e.default}else{provider(endpoints[0].manager,side,index)}
    })
}

pub(crate) fn unit_mass(core:Core)->f64 {
    let x=core.arm.x*core.arm.x;let y=core.arm.y*core.arm.y;let z=core.arm.z*core.arm.z;
    let mut maximum=core.inverse_inertia.x*(y+z);
    let other=core.inverse_inertia.y*(x+z);if maximum<other {maximum=other;}
    let other=core.inverse_inertia.z*(x+y);if maximum<other {maximum=other;}
    let denominator=if core.flags&16==0 {core.inverse_mass+maximum}else{maximum+1.};
    1./denominator as f64
}

pub(crate) fn setup(mode:u8,cores:[Core;2],endpoints:[Endpoint;2],provider:impl FnMut(u32,usize,u8)->Material)->Result<Setup,&'static str>{
    let materials=resolve_materials(endpoints,provider);
    let mode=if materials.iter().any(|m|m.flags!=0){1}else{mode};
    let mass=if cores[0].flags&18!=0 {unit_mass(cores[1]) as f32}
        else if cores[1].flags&18!=0 {unit_mass(cores[0]) as f32}
        else {let a=unit_mass(cores[0]) as f32;let b=unit_mass(cores[1]) as f32;(b*a)/(b+a)};
    let bound=1./mass;
    if !bound.is_finite()||bound<0. {return Err("unreconstructed singular/invalid contact mass bound");}
    Ok(Setup{mode,bound,materials})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn native_mass_bound_material_mode_and_provider_dispatch_match(){
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-mass-bound.json")).unwrap();
        let f=|v:&Value|v.as_f64().unwrap() as f32;let vector=|v:&Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));
        let mut count=0;let mut callback_count=0;let mut unit_count=0;let mut real_count=0;
        for row in fixture["cases"].as_array().unwrap(){
            let endpoints=std::array::from_fn(|side|{let b=&row["bodies"][side];Endpoint{manager:b["manager"].as_u64().unwrap() as u32,code:b["code"].as_u64().unwrap() as u8,default:Material{key:b["default_material"].as_u64().unwrap() as u32,flags:b["default_flags"].as_u64().unwrap() as u32}}});
            let cores=std::array::from_fn(|side|{let b=&row["bodies"][side];Core{flags:b["flags"].as_u64().unwrap() as u16,inverse_inertia:vector(&b["inverse_inertia"]),inverse_mass:f(&b["inverse_mass"]),arm:vector(&b["arm"])}});
            let mut callbacks=0;
            let output=setup(row["mode_before"].as_u64().unwrap() as u8,cores,endpoints,|manager,side,index|{
                let control=&row["control"];assert!(!control.is_null(),"real non-default provider is not reconstructed");
                let event=&control["callbacks"][callbacks];assert_eq!(manager,event["manager"].as_u64().unwrap() as u32);assert_eq!(side,event["object"].as_u64().unwrap() as usize);assert_eq!(index,event["index"].as_u64().unwrap() as u8);assert_eq!(event["zero"],0);
                // Reproduce the owned provider adapter, not its observed return.
                // Native must dispatch both endpoints through provider identity0.
                assert_eq!(manager,endpoints[0].manager);
                let m=&control["materials"][2+(side*3+index as usize)%6];let material=Material{key:m["key"].as_u64().unwrap() as u32,flags:m["flags"].as_u64().unwrap() as u32};
                assert_eq!(material.key,event["result"].as_u64().unwrap() as u32);callbacks+=1;callback_count+=1;material
            }).unwrap();
            assert_eq!(output.bound.to_bits(),f(&row["bound"]).to_bits(),"bound{count}");assert_eq!(output.mode,row["mode_after"].as_u64().unwrap() as u8);
            for side in 0..2 {assert_eq!(output.materials[side].key,row["materials"][side].as_u64().unwrap() as u32);assert_eq!(output.materials[side].flags,row["material_flags"][side].as_u64().unwrap() as u32);}
            if let Some(control)=row.get("control"){
                assert_eq!(callbacks,control["callbacks"].as_array().unwrap().len());
                for side in 0..2 {assert_eq!(unit_mass(cores[side]).to_bits(),control["unit_mass"][side].as_f64().unwrap().to_bits());unit_count+=1;}
            }else{real_count+=1;}
            count+=1;
        }
        assert_eq!(count,338);assert_eq!(real_count,82);assert_eq!(callback_count,384);assert_eq!(unit_count,512);
    }

    #[test]
    fn native_backend_registry_composes_into_complete_contact_mass_setup(){
        use crate::ivp_material_backend::{Backend,Manager};
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-backend-mass.json")).unwrap();
        let f=|v:&Value|v.as_f64().unwrap() as f32;let vector=|v:&Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));
        let mut count=0;let mut dispatches=0;let mut backend_calls=0;
        for row in fixture["cases"].as_array().unwrap(){
            let control=&row["control"];let registry=&control["backend"];
            let backend=Backend{count:registry["count"].as_u64().unwrap() as usize,shadow:registry["shadow"].as_i64().unwrap() as i32};
            let managers:[Manager;2]=std::array::from_fn(|side|Manager{map:std::array::from_fn(|i|registry["maps"][side][i].as_u64().unwrap() as u16)});
            let endpoints=std::array::from_fn(|side|{let b=&row["bodies"][side];Endpoint{manager:b["manager"].as_u64().unwrap() as u32,code:b["code"].as_u64().unwrap() as u8,default:Material{key:b["default_material"].as_u64().unwrap() as u32,flags:b["default_flags"].as_u64().unwrap() as u32}}});
            let cores=std::array::from_fn(|side|{let b=&row["bodies"][side];Core{flags:b["flags"].as_u64().unwrap() as u16,inverse_inertia:vector(&b["inverse_inertia"]),inverse_mass:f(&b["inverse_mass"]),arm:vector(&b["arm"])}});
            let mut queries=Vec::new();
            let output=setup(row["mode_before"].as_u64().unwrap() as u8,cores,endpoints,|manager,_,index|{
                let selected=endpoints.iter().position(|e|e.manager==manager).unwrap();assert_eq!(selected,0);
                let slot=managers[selected].resolve_with(index as u32,|i|{queries.push(i);backend.lookup(i)}).unwrap().unwrap();
                let m=&control["materials"][slot];dispatches+=1;
                Material{key:m["key"].as_u64().unwrap() as u32,flags:m["flags"].as_u64().unwrap() as u32}
            }).unwrap();
            assert_eq!(queries,row["backend_queries"].as_array().unwrap().iter().map(|v|v.as_i64().unwrap() as i32).collect::<Vec<_>>());backend_calls+=queries.len();
            assert_eq!(output.bound.to_bits(),f(&row["bound"]).to_bits());assert_eq!(output.mode,row["mode_after"].as_u64().unwrap() as u8);
            for side in 0..2 {assert_eq!(output.materials[side].key,row["materials"][side].as_u64().unwrap() as u32);assert_eq!(output.materials[side].flags,row["material_flags"][side].as_u64().unwrap() as u32);}
            count+=1;
        }
        assert_eq!(count,256);assert_eq!(dispatches,384);assert_eq!(backend_calls,461);
    }
}
