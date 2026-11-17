//! Owned material manager10092410/10092490 and1009c610 direction override.
//! Lookup/provider and anisotropic friction remain separate from these kernels.
use bevy::prelude::*;
#[derive(Clone,Copy)]
pub(crate) struct Body {
    pub controller:bool,
    pub user_flags:Option<u8>,
    pub rotation_rows:[Vec3;3],
}
pub(crate) struct Settings {
    pub minimum_normal_square:f32,
    pub axis_threshold:f32,
}

/// Native multiplies in x87, compares the wide product against one, but uses
/// its f32 spill for the negative test. Tiny negative products can survive that
/// test as a wide return value because their spill is negative zero.
pub(crate) fn clamped_product(a:f32,b:f32)->f64 {
    let product=a as f64*b as f64;
    if product>1. {1.} else if (product as f32)<0. {0.} else {product}
}

pub(crate) fn zero_friction_override(bodies:[Body;2],normal:Vec3,k:&Settings)->bool {
    if !bodies.iter().any(|b|b.controller){return false;}
    // Native chooses the first flagged object, even if the other object's core
    // is the one carrying a controller. It does not combine their directions.
    let Some(body)=bodies.iter().find(|b|b.user_flags.is_some_and(|f|f&64!=0)) else{return false;};
    let square=(normal.y*normal.y+normal.x*normal.x)+normal.z*normal.z;
    if square<k.minimum_normal_square{return false;}
    let r=body.rotation_rows;
    let local_x=(r[1].x*normal.y+r[0].x*normal.x)+r[2].x*normal.z;
    local_x.abs()>k.axis_threshold
}

pub(crate) fn friction(coefficients:[f32;2],bodies:[Body;2],normal:Vec3,k:&Settings)->f64 {
    if zero_friction_override(bodies,normal,k){0.}else{clamped_product(coefficients[0],coefficients[1])}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_material_products_clamps_and_controller_direction_match_native(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-material.json")).unwrap();
        let f=|v:&serde_json::Value|v.as_f64().unwrap() as f32;
        let k=Settings{minimum_normal_square:f(&data["constants"]["minimum_normal_square"]),axis_threshold:f(&data["constants"]["axis_threshold"])};
        let mut count=0;let mut overrides=0;let mut negative_underflows=0;
        for row in data["cases"].as_array().unwrap(){
            let b=std::array::from_fn(|side|{let b=&row["bodies"][side];Body{controller:b["controller"].as_bool().unwrap(),user_flags:b["user_flags"].as_u64().map(|v|v as u8),rotation_rows:std::array::from_fn(|i|Vec3::from_array(std::array::from_fn(|j|f(&b["rotation_rows"][i*3+j]))))}});
            let normal=Vec3::from_array(std::array::from_fn(|i|f(&row["normal"][i])));
            let overridden=zero_friction_override(b,normal,&k);assert_eq!(overridden,row["overridden"].as_bool().unwrap(),"override{count}");
            let coefficient=[f(&row["coefficients"][0]),f(&row["coefficients"][1])];
            let actual=friction(coefficient,b,normal,&k);let expected=row["friction"].as_f64().unwrap();
            assert_eq!(actual.to_bits(),expected.to_bits(),"friction{count}");
            let elasticity=clamped_product(f(&row["coefficients"][2]),f(&row["coefficients"][3]));
            assert_eq!(elasticity.to_bits(),row["elasticity"].as_f64().unwrap().to_bits(),"elasticity{count}");
            assert!(row["inputs_unchanged"].as_bool().unwrap());count+=1;overrides+=overridden as usize;
            negative_underflows+=(actual<0.&&(actual as f32)==0.) as usize;
        }
        assert_eq!(count,768);assert_eq!(overrides,77);assert!(negative_underflows>0);
    }
}
