//! Owned10117260 broadphase margin,101124b0 grid placement and101127a0
//! decoding. Octree membership/overlap queries and listener ownership remain
//! separate. Coordinates are physical IVP metres, with no render-axis mapping.
use crate::{ivp_hull_budget::Budget,ivp_hull_policy::Body};

#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct Cell {
    pub coordinates:[i32;3],pub lower_level:i32,pub upper_level:i32,pub radius:f32,
}
pub(crate) fn power(level:i32)->Result<f32,&'static str>{
    if !(-40..=40).contains(&level){return Err("outside initialized native broadphase powers");}
    Ok(f32::from_bits(((level+127) as u32)<<23))
}
fn integer(value:f64)->Result<i32,&'static str>{
    if !value.is_finite()||value<i32::MIN as f64||value>i32::MAX as f64{return Err("broadphase coordinate outside native index domain");}
    Ok(value as i32)
}
fn fit(center:[f32;3],radius:f32,inverse:f32)->Result<Option<[i32;3]>,&'static str>{
    let mut lower=[0;3];
    for axis in 0..3{
        //10112510/13 retain the lower subtraction/product in x87 before the
        //double floor argument. Upper10112528/2d uses ADDSS/MULSS stores.
        lower[axis]=integer(((f64::from(center[axis])-f64::from(radius))*f64::from(inverse)).floor())?;
        let sum=center[axis]+radius;let scaled=sum*inverse;
        let upper=integer(f64::from(scaled).ceil())?;
        if upper>lower[axis].wrapping_add(2){return Ok(None);}
    }
    Ok(Some(lower))
}
pub(crate) fn select(center:[f32;3],minimum:f32,maximum:f32)->Result<Cell,&'static str>{
    if center.iter().any(|v|!v.is_finite())||!minimum.is_finite()||minimum<=0.||!maximum.is_finite(){return Err("invalid broadphase bounds");}
    let diameter=minimum*2.;let mut level=((((diameter.to_bits()>>23)&255) as i32)-126).max(-40);
    loop{
        let inverse=power(1-level)?;
        if let Some(coordinates)=fit(center,minimum,inverse)?{
            let mut cell=Cell{coordinates,lower_level:level-1,upper_level:level,radius:minimum};
            // Native tests the requested expansion at one coarser level once.
            // A failed expansion preserves the original selected node/radius.
            if minimum<maximum{
                if let Some(coordinates)=fit(center,maximum,power(-level)?)?{
                    cell=Cell{coordinates,lower_level:level,upper_level:level+1,radius:maximum};
                }
            }
            return Ok(cell);
        }
        level+=1;
    }
}
impl Cell{
    pub(crate) fn words(self)->[i32;5]{[self.coordinates[0],self.coordinates[1],self.coordinates[2],self.lower_level,self.upper_level]}
    pub(crate) fn decode(self)->Result<[f32;4],&'static str>{
        let scale=power(self.lower_level)?;let c=self.coordinates.map(|v|v as f32*scale);
        Ok([c[0],c[1],c[2],power(self.upper_level)?])
    }
}
#[derive(Clone,Copy)]
pub(crate) struct Policy{pub lookahead:f32,pub radius_factor:f32,pub minimum:f32,pub maximum:f32,pub minimum_time:f32}
impl Default for Policy{
    fn default()->Self{Self{lookahead:1.,radius_factor:5.,minimum:0.5,maximum:15.,minimum_time:0.06}}
}
impl Policy{
    pub(crate) fn margin(self,dt:f32,body:Body)->f32{
        let speed=(body.angular+body.linear)+1e-10_f32;
        let mut distance=self.lookahead*speed;let size=self.radius_factor*body.radius;
        if size<distance{distance=size;}
        if distance<self.minimum{distance=self.minimum;}
        if self.maximum<distance{distance=self.maximum;}
        let projected=dt*speed;distance-=projected;
        let reserve=self.minimum_time*speed+body.radius;
        if distance<reserve{distance=reserve;}
        distance
    }
    ///1010bfe0 adds the physical radius for the spatial bound.101123d0 then
    ///renews the manager entry with the selected bound minus that radius.
    pub(crate) fn renew(self,now:f64,dt:f32,body:Body,budget:Budget,center:[f32;3])->Result<(Cell,f32),&'static str>{
        let requested=self.margin(dt,body)+body.radius;
        let cell=select(center,requested,requested)?;let margin=cell.radius-body.radius;
        // Unlike10127f00's pair renewal, no float spill precedes adding margin.
        let key=((now-budget.time)*f64::from(budget.rates[0])+f64::from(budget.accumulated[0])+f64::from(margin)) as f32;
        Ok((cell,key))
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn fixture()->serde_json::Value{serde_json::from_str(include_str!("../tools/fixtures/native-ivp-broadphase.json")).unwrap()}
    fn pair_policy(v:&serde_json::Value)->Policy{Policy{lookahead:v[0].as_f64().unwrap() as f32,radius_factor:v[1].as_f64().unwrap() as f32,minimum:v[2].as_f64().unwrap() as f32,maximum:v[3].as_f64().unwrap() as f32,minimum_time:v[4].as_f64().unwrap() as f32}}
    #[test]
    fn owned_margin_constructor_and_all_numeric_controls_match_exact_float_bits(){
        let d=fixture();let p=Policy::default();
        assert_eq!([p.lookahead,p.radius_factor,p.minimum,p.maximum,p.minimum_time],std::array::from_fn(|i|d["broadphase_policy_defaults"][i].as_f64().unwrap() as f32));
        let controls=d["broadphase_policy_controls"].as_array().unwrap();
        for c in controls{let f=|k:&str|c[k].as_f64().unwrap() as f32;let body=Body{radius:f("radius"),linear:f("linear"),angular:f("angular")};assert_eq!(p.margin(f("dt"),body).to_bits(),c["expected_bits"].as_u64().unwrap() as u32,"control{}",c["id"]);}
        assert_eq!(controls.len(),960);
    }
    #[test]
    fn owned_grid_nodes_decoding_and_requested_expansion_match_every_native_control(){
        let d=fixture();let controls=d["broadphase_grid_controls"].as_array().unwrap();let mut expanded=0;let mut rejected=0;
        for c in controls{
            let center=std::array::from_fn(|i|f32::from_bits(c["center_bits"][i].as_u64().unwrap() as u32));
            let min=c["minimum"].as_f64().unwrap() as f32;let max=c["maximum"].as_f64().unwrap() as f32;
            let cell=select(center,min,max).unwrap();
            assert_eq!(cell.words(),std::array::from_fn(|i|c["node"][i].as_i64().unwrap() as i32),"node control{}",c["id"]);
            assert_eq!(cell.radius.to_bits(),c["selected_bits"].as_u64().unwrap() as u32);
            assert_eq!(cell.decode().unwrap().map(f32::to_bits),std::array::from_fn(|i|c["decoded_bits"][i].as_u64().unwrap() as u32));
            if min<max{if cell.radius==max{expanded+=1;}else{rejected+=1;}}
        }
        assert_eq!((controls.len(),expanded,rejected),(2592,1736,424));
    }
    #[test]
    fn actual_broadphase_callbacks_renew_bounds_cells_and_thresholds_exactly(){
        let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-broadphase-calls.json")).unwrap();let mut types=[0;3];
        for c in d["cases"].as_array().unwrap(){
            let head=&c["before"]["entries"][0];let method=head["methods_rva"][1].as_u64().unwrap();let role=match method{0x112490=>0,0x1280a0=>1,0x10d500=>2,_=>panic!("unknown listener")};types[role]+=1;if role!=0{continue;}
            let b=&head["broadphase"];let f=|k:&str|b[k].as_f64().unwrap() as f32;
            let body=Body{radius:b["body"][0].as_f64().unwrap() as f32,linear:b["body"][1].as_f64().unwrap() as f32,angular:b["body"][2].as_f64().unwrap() as f32};
            let budget=Budget{time:b["budget_time"].as_f64().unwrap(),rates:[f("rate"),0.],accumulated:[f("accumulated"),0.],predicted:0.,threshold:0.};
            let (cell,key)=pair_policy(&b["settings"]).renew(b["time"].as_f64().unwrap(),f("dt"),body,budget,std::array::from_fn(|i|b["center"][i].as_f64().unwrap() as f32)).unwrap();
            let after=c["after"]["entries"].as_array().unwrap().iter().filter(|e|e["methods_rva"][1]==0x112490).collect::<Vec<_>>();assert_eq!(after.len(),1);
            assert_eq!(key.to_bits(),(after[0]["key"].as_f64().unwrap() as f32).to_bits(),"{} {} step{}",c["phase"],c["model"],c["step"]);
            assert_eq!(cell.radius.to_bits(),(after[0]["broadphase"]["stored_radius"].as_f64().unwrap() as f32).to_bits());
            assert_eq!(cell.words(),std::array::from_fn(|i|after[0]["broadphase"]["grid"][i].as_i64().unwrap() as i32),"grid {} {} step{}",c["phase"],c["model"],c["step"]);
        }
        assert_eq!(types,[23,37,26]);
    }
}
