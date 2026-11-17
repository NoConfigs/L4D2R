//! Owned100f60e0 compact bounds and100e8ba0/100e95b0 aggregate updates.
//! Numeric object/core state only. World ownership, recheck queues and contacts
//! are separate; no native snapshots supply these calculations.
use bevy::prelude::*;
#[derive(Clone,Copy,Debug,Default,PartialEq)]
pub(crate) struct Bounds {pub radius:f32,pub angular_radius:f32}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Compact {pub center:Vec3,pub radius:f32,pub factor:u8}
impl From<l4d2_formats::phy::PhyBounds> for Compact {
 fn from(v:l4d2_formats::phy::PhyBounds)->Self{Self{center:Vec3::from_array(v.center_ivp.to_array()),radius:v.radius,factor:v.deviation_factor}}
}
impl Compact {
 pub(crate) fn at(self,point:Vec3)->Result<Bounds,&'static str>{
  if !point.is_finite()||!self.center.is_finite()||!self.radius.is_finite()||self.radius<0.{return Err("invalid compact bounds");}
  // Subtractions round to float, but x87 retains distance and packed scaling
  // through the final output stores. Rounding factor*0.004 to float is wrong.
  let [x,y,z]=(self.center-point).to_array().map(f64::from);
  let distance=((y*y+x*x)+z*z).sqrt();
  Ok(Bounds{radius:(self.radius as f64+distance) as f32,angular_radius:(self.factor as f64*0.004000000189989805*self.radius as f64+distance) as f32})
 }
}
#[derive(Clone,Copy,Debug)]
pub(crate) enum Shape {Compact(Compact),Sphere}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Object {pub shape:Shape,pub shift:Vec3,pub margin:f32}
impl Object {
 pub(crate) fn bounds(self)->Result<Bounds,&'static str>{
  if !self.shift.is_finite()||!self.margin.is_finite(){return Err("invalid object bound input");}
  let mut result=match self.shape {
   Shape::Compact(shape)=>shape.at(-self.shift)?,
   Shape::Sphere=>{let [x,y,z]=self.shift.to_array().map(f64::from);let length=((z*z+x*x)+y*y).sqrt() as f32;Bounds{radius:length,angular_radius:length}}
  };
  result.radius+=self.margin;Ok(result)
 }
}
impl Bounds {
 pub(crate) fn grow(&mut self,object:Object)->Result<(),&'static str>{
  let other=object.bounds()?;
  if self.radius<other.radius{self.radius=other.radius;}
  if self.angular_radius<other.angular_radius{self.angular_radius=other.angular_radius;}
  Ok(())
 }
 pub(crate) fn set_margin(&mut self,objects:&mut[Object],index:usize,margin:f32)->Result<(),&'static str>{
  if !margin.is_finite()||index>=objects.len(){return Err("invalid bound margin update");}
  let previous=objects[index].margin;objects[index].margin=margin;
  if margin<=previous{*self=Self::default();for object in objects.iter().rev(){self.grow(*object)?;}}
  else{self.grow(objects[index])?;}
  Ok(())
 }
}

///100ec3b0 rounds only the returned vector length, not its squared terms.
pub(crate) fn speed(v:Vec3)->f32{let[x,y,z]=v.to_array().map(f64::from);((z*z+x*x)+y*y).sqrt() as f32}
#[derive(Clone,Copy)]
pub(crate) struct Motion {pub axis:Vec3,pub angular_bound:f32,pub velocity:Vec3}
///1010b840/1010c450 cached-normal conservative closing speed. Native retains
/// the axis squares and safety-factor subtraction through x87 square roots.
pub(crate) fn closing_speed(a:Motion,b:Motion,normal:Vec3)->Option<f32>{closing_speed_exact(a,b,normal).and_then(|v|{let stored=v as f32;stored.is_finite().then_some(stored)})}
pub(crate) fn closing_speed_exact(a:Motion,b:Motion,normal:Vec3)->Option<f64>{
 let dot=|v:Vec3|(v.y*normal.y+v.x*normal.x)+v.z*normal.z;
 let x=dot(a.axis) as f64;let y=dot(b.axis) as f64;
 let relative=dot(b.velocity)-dot(a.velocity);
 let result=(1.0010000467300415-x*x).sqrt()*a.angular_bound as f64+(1.0010000467300415-y*y).sqrt()*b.angular_bound as f64+relative as f64;
 result.is_finite().then_some(result)
}

#[cfg(test)]
mod tests {
 use super::*;
 fn fixture()->serde_json::Value{serde_json::from_str(include_str!("../tools/fixtures/native-ivp-bounds.json")).unwrap()}
 fn vector(v:&serde_json::Value)->Vec3{Vec3::from_array(std::array::from_fn(|i|v[i].as_f64().unwrap() as f32))}
 fn pair(v:&serde_json::Value)->Bounds{Bounds{radius:v[0].as_f64().unwrap() as f32,angular_radius:v[1].as_f64().unwrap() as f32}}
 fn compact(v:&serde_json::Value)->Compact{Compact{center:vector(&v["center"]),radius:v["radius"].as_f64().unwrap() as f32,factor:v["factor"].as_u64().unwrap() as u8}}
 #[test]
 fn native_directional_bound_preserves_safety_factor_and_extended_square(){
  let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-time-refine.json")).unwrap();let mut count=0;
  for c in data["cases"].as_array().unwrap(){if c["evaluator_rva"]!=0x113970{continue;}let normal=vector(&c["normal"]);if normal!=Vec3::Y&&normal!=-Vec3::Y{continue;}
   let motion=|i|{let v=&c["objects"][i];Motion{axis:vector(&v["sweep_axis"]),velocity:vector(&v["velocity"]),angular_bound:v["angular_bound"].as_f64().unwrap() as f32}};
   assert_eq!(closing_speed(motion(0),motion(1),normal).unwrap(),c["bound_speed"].as_f64().unwrap() as f32,"{} step{}",c["model"],c["step"]);count+=1;
  }assert_eq!(count,44);
 }
 #[test]
 fn original_compact_bytes_and_core_bound_updates_match_native(){
  let data=fixture();let mut controls=0;let mut events=0;
  for profile in data["profiles"].as_array().unwrap(){
   let shape=compact(&profile["surface"]);assert_eq!(shape.at(shape.center).unwrap(),pair(&profile["initial"]));
   for c in profile["controls"].as_array().unwrap(){let mut shape=shape;shape.factor=c["packed"].as_u64().unwrap() as u8;assert_eq!(shape.at(vector(&c["point"])).unwrap(),pair(&c["expected"]),"{} packed{}",profile["model"],c["packed"]);controls+=1;}
   let mut objects=profile["objects"].as_array().unwrap().iter().map(|o|Object{shape:if o["kind"]==2{Shape::Compact(shape)}else{Shape::Sphere},shift:vector(&o["shift"]),margin:o["margin"].as_f64().unwrap() as f32}).collect::<Vec<_>>();
   let mut core=Bounds::default();
   for e in profile["events"].as_array().unwrap(){assert_eq!(core,pair(&e["before"]));let index=e["object"].as_u64().unwrap() as usize;
    if e["set_margin"].as_bool().unwrap(){core.set_margin(&mut objects,index,e["margin"].as_f64().unwrap() as f32).unwrap();}else{core.grow(objects[index]).unwrap();}
    assert_eq!(core,pair(&e["after"]),"{} event{events}",profile["model"]);events+=1;
   }
  }assert_eq!(controls,4608);assert_eq!(events,66);
 }
 #[test]
 fn installed_compact_bounds_preserve_original_radius_and_packed_byte(){
  let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");if !root.exists(){return;}
  let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
  let initial:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-bound-cores.json")).unwrap();
  let controls=fixture();let mut count=0;
  for p in controls["profiles"].as_array().unwrap().iter().chain(initial["profiles"].as_array().unwrap()){let bytes=assets.read(p["model"].as_str().unwrap()).unwrap();let solids=l4d2_formats::phy::parse_phy(&bytes).unwrap();let shape=Compact::from(solids[p["solid"].as_u64().unwrap() as usize].bounds.unwrap());let expected=compact(&p["surface"]);assert_eq!(shape.center,expected.center);assert_eq!(shape.radius,expected.radius);assert_eq!(shape.factor,expected.factor);assert_eq!(shape.at(shape.center).unwrap(),pair(&p["initial"]));count+=1;}assert_eq!(count,62);
 }
}
