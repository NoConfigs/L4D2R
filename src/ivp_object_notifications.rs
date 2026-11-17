//!10106fe0/101070e0 ordered object listener ownership and10106a40 delivery.
use std::collections::BTreeMap;
#[derive(Clone,Default)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct Registry {objects:BTreeMap<usize,Object>}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
struct Object {flags:u32,listeners:Vec<usize>}
impl Registry {
 pub(crate) fn bind(&mut self,object:usize,flags:u32)->Result<(),&'static str>{
  if let Some(o)=self.objects.get_mut(&object){o.flags=(flags&!0x1000)|(o.flags&0x1000);return Ok(());}
  if flags&0x1000!=0{return Err("unowned initial object notification flag");}
  self.objects.insert(object,Object{flags,listeners:vec![]});Ok(())
 }
 pub(crate) fn flags(&self,object:usize)->Result<u32,&'static str>{Ok(self.objects.get(&object).ok_or("unbound object notification flags")?.flags)}
 pub(crate) fn listeners(&self,object:usize)->Result<&[usize],&'static str>{Ok(&self.objects.get(&object).ok_or("unbound object notifications")?.listeners)}
 pub(crate) fn add(&mut self,object:usize,listener:usize)->Result<(),&'static str>{let o=self.objects.get_mut(&object).ok_or("unbound notification object")?;o.listeners.push(listener);o.flags|=0x1000;Ok(())}
 pub(crate) fn remove(&mut self,object:usize,listener:usize)->Result<(),&'static str>{
  let o=self.objects.get_mut(&object).ok_or("unbound notification object")?;let index=o.listeners.iter().rposition(|&l|l==listener).ok_or("missing object notification listener")?;o.listeners.remove(index);if o.listeners.is_empty(){o.flags&=!0x1000;}Ok(())
 }
 pub(crate) fn emit(&mut self,object:usize,mut notify:impl FnMut(&mut Self,usize)->Result<(),&'static str>)->Result<usize,&'static str>{
  let mut index=self.listeners(object)?.len();let mut called=0;
  while index>0{index-=1;let id=*self.listeners(object)?.get(index).ok_or("object notifier lost current reverse index")?;notify(self,id)?;called+=1;if self.listeners(object)?.is_empty(){break;}}
  Ok(called)
 }
}
#[cfg(test)]
mod tests {
 use super::*;
 #[test]
 fn native_object_registry_flags_order_and_self_removal_match(){
  let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-object-notify-registry.json")).unwrap();let mut r=Registry::default();for object in 0..3{r.bind(object,2056).unwrap();}let mut callbacks=0;
  for row in d["steps"].as_array().unwrap(){let object=row["object"].as_u64().unwrap() as usize;let listener=row["listener"].as_u64().unwrap() as usize;let mut events=vec![];
   match row["op"].as_u64().unwrap(){0=>r.add(object,listener).unwrap(),1=>r.remove(object,listener).unwrap(),2=>{r.emit(object,|r,id|{events.push([id as u32,object as u32,r.flags(object)?]);if row["self_remove"]==true{r.remove(object,id)?;}Ok(())}).unwrap();},_=>panic!("unknown native registry op")}
   callbacks+=events.len();assert_eq!(serde_json::json!(events),row["events"]);assert_eq!(serde_json::json!((0..3).map(|o|r.flags(o).unwrap()).collect::<Vec<_>>()),row["flags"]);assert_eq!(serde_json::json!((0..3).map(|o|r.listeners(o).unwrap().to_vec()).collect::<Vec<_>>()),row["lists"]);
  }
  for object in 0..3{while let Some(&id)=r.listeners(object).unwrap().first(){r.remove(object,id).unwrap();}}
  assert_eq!(serde_json::json!((0..3).map(|o|r.flags(o).unwrap()).collect::<Vec<_>>()),d["cleanup_flags"]);assert_eq!(callbacks,456);assert_eq!(d["steps"].as_array().unwrap().len(),512);
 }
}
