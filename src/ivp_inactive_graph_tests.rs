use super::*;
fn snapshot(g:&Graph)->serde_json::Value{
 let links=|inactive:bool|(0..g.objects.len()).map(|j|{let a=if inactive{g.inactive_object_links(j).unwrap()}else{g.object_links(j).unwrap()};a.iter().map(|s|serde_json::json!({"pair":s.pair,"side":s.side})).collect::<Vec<_>>()}).collect::<Vec<_>>();
 serde_json::json!({"active":g.active(),"inactive":g.inactive(),"active_objects":links(false),"inactive_objects":links(true),"fast":g.fast(),"flags":(0..g.pairs.len()).map(|i|g.flags(i).unwrap()).collect::<Vec<_>>()})
}
#[test]
fn all_native_active_inactive_fast_lists_and_reactivation_transitions_match(){
 let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-inactive-links.json")).unwrap();let mut count=0;
 for c in d["cases"].as_array().unwrap(){let mut g=Graph::new(4);
  for p in c["bindings"].as_array().unwrap(){g.register([p[0].as_u64().unwrap() as usize,p[1].as_u64().unwrap() as usize]).unwrap();}
  assert_eq!(snapshot(&g),c["initial"]);
  for e in c["events"].as_array().unwrap(){let id=e["pair"].as_u64().unwrap() as usize;g.set_flags(id,e["flags_input"].as_u64().unwrap() as u32).unwrap();assert_eq!(snapshot(&g),e["before"]);
   let mask=e["fast_core_mask"].as_u64().unwrap();let fast=g.objects(id).unwrap().map(|j|mask&(1<<j)!=0);
   match e["operation"].as_str().unwrap(){"activate"=>g.activate(id,fast),"unlink"=>g.unlink(id),"inactive"=>g.link_inactive(id),"unlink_inactive"=>g.unlink_inactive(id),"invalid"=>g.make_inactive(id),"reactivate"=>g.reactivate(id,fast),_=>panic!("unknown native operation")}.unwrap();
   assert_eq!(snapshot(&g),e["after"],"scenario{} step{}",c["scenario"],e["step"]);count+=1;
  }
 }assert_eq!(count,824);
}
#[test]
fn inactive_retirement_removes_old_object_links_before_id_reuse(){
 let mut g=Graph::new(3);let a=g.register([0,1]).unwrap();let b=g.register([1,2]).unwrap();g.link_inactive(a).unwrap();g.link_inactive(b).unwrap();
 assert!(g.activate(a,[true,true]).is_err());assert!(g.link_inactive(a).is_err());g.remove(a).unwrap();let reused=g.register([0,2]).unwrap();assert_eq!(reused,a);
 g.activate(reused,[true,true]).unwrap();assert_eq!(g.inactive_object_links(1).unwrap(),&[Side{pair:b,side:0}]);assert!(g.inactive_object_links(0).unwrap().is_empty());g.remove(b).unwrap();g.remove(reused).unwrap();assert!(g.active().is_empty());assert!(g.inactive().is_empty());assert!(g.fast().is_empty());
}
