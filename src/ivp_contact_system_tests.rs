use crate::{ivp_contact_system::{Contacts,System,Pair,Binding},ivp_contact_pipeline::FrictionGroup,ivp_unit_registry::{Registry,Effect}};
use serde_json::{Value,json};
fn ids(v:&Value)->Vec<usize>{v.as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect()}
fn try_initial(c:&Value)->Result<Contacts,&'static str>{
    let boundary=&c["boundary"];let mut registry=Registry::new(4);
    let fixed=c["fixed"].as_array().unwrap().iter().enumerate().map(|(id,v)|{let fixed=v.as_bool().unwrap();registry.register_core(id,fixed).unwrap();(id,fixed)}).collect();
    let systems=boundary["systems"].as_array().unwrap().iter().enumerate().filter(|(_,s)|!s.is_null()).map(|(id,s)|{(id,System{cores:ids(&s["cores"]),contacts:ids(&s["contacts"]),dirty:s["dirty"]==1,pairs:s["groups"].as_array().unwrap().iter().map(|p|Pair{cores:ids(&p["cores"]).try_into().unwrap(),contacts:ids(&p["contacts"]),clock:FrictionGroup{timer:p["timer"].as_i64().unwrap() as i32,energy:f32::from_bits(p["energy"].as_u64().unwrap() as u32)}}).collect()})}).collect();
    let bindings=boundary["contact_bindings"].as_array().unwrap().iter().map(|b|(b["id"].as_u64().unwrap() as usize,Binding{cores:ids(&b["cores"]).try_into().unwrap(),system:b["system"].as_u64().unwrap() as usize})).collect();
    let links=boundary["core_links"].as_array().unwrap().iter().enumerate().flat_map(|(core,ls)|ls.as_array().unwrap().iter().map(move |l|((core,l["system"].as_u64().unwrap() as usize),ids(&l["contacts"])))).collect();
    Contacts::from_boundary(registry,fixed,systems,bindings,links)
}
fn initial(c:&Value)->Contacts{try_initial(c).unwrap()}
fn expected(s:&Value)->Value{json!({"systems":s["systems"],"core_links":s["core_links"],"contact_bindings":s["contact_bindings"],"controller_keys":s["controller_keys"]})}
fn compare(a:&Value,b:&Value,path:&str){match(a,b){
    (Value::Object(a),Value::Object(b))=>{assert_eq!(a.len(),b.len(),"{path}");for(k,v)in a{compare(v,&b[k],&format!("{path}.{k}"));}},
    (Value::Array(a),Value::Array(b))=>{assert_eq!(a.len(),b.len(),"{path}");for(i,(a,b))in a.iter().zip(b).enumerate(){compare(a,b,&format!("{path}[{i}]"));}},
    _=>assert_eq!(a,b,"{path}"),
}}
fn replay(c:&Value)->Vec<Value>{
    let mut world=initial(c);let mut snapshots:Vec<Value>=vec![];let mut released=vec![];
    for s in c["states"].as_array().unwrap(){
        let system=s["system"].as_i64().unwrap() as usize;
        match s["action"].as_str().unwrap(){
            "initial"=>{},"merge"=>world.merge(system,s["other"].as_u64().unwrap() as usize).unwrap(),
            "remove"=>{let id=s["contact"].as_u64().unwrap() as usize;let binding=&snapshots.last().unwrap()["contact_bindings"].as_array().unwrap().iter().find(|b|b["id"]==id).unwrap()["cores"];let cores=ids(binding);world.remove(id).unwrap();let (effects,ids)=world.take_effects();assert_eq!(ids,[id]);released.push(id);let reset=effects.iter().filter_map(|e|if let Effect::ResetClock(c)=e{Some(*c)}else{None}).collect::<Vec<_>>();assert_eq!(reset,cores);},
            "destroy"=>world.destroy_empty(system).unwrap(),_=>panic!("unknown contact system operation"),
        }
        snapshots.push(world.snapshot(3));
    }
    assert_eq!(released.len(),c["boundary"]["contact_bindings"].as_array().unwrap().len());snapshots
}
pub(crate) fn verify(){
    let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-pairs.json")).unwrap();let mut checkpoints=0;
    for c in data["cases"].as_array().unwrap(){let actual=replay(c);for(i,(a,s))in actual.iter().zip(c["states"].as_array().unwrap()).enumerate(){compare(a,&expected(s),&format!("world{} step{i} {}",c["scenario"],s["action"]));checkpoints+=1;}
        let mut corrupt=c.clone();for s in corrupt["states"].as_array_mut().unwrap(){for key in ["systems","core_links","contact_bindings","controller_keys"]{s[key]=Value::Null;}}assert_eq!(actual,replay(&corrupt));
    }assert_eq!(checkpoints,104);println!("real contact ownership: 8 native worlds /104 checkpoints; retained pair state, registered controllers and corruption controls pass");
}
pub(crate) fn invalid(){
    let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-pairs.json")).unwrap();let mut world=initial(&data["cases"][0]);let before=format!("{world:?}");
    for op in 0..5{let r=match op{0=>world.merge(0,0),1=>world.merge(0,99),2=>world.merge(99,0),3=>world.remove(999),_=>world.destroy_empty(0)};assert!(r.is_err());assert_eq!(format!("{world:?}"),before);}
    world.remove(0).unwrap();let before=format!("{world:?}");assert!(world.remove(0).is_err());assert_eq!(format!("{world:?}"),before);
    // Boundary validation must reject orphan/multiply-owned identities and
    // fixed classification disagreements before exposing a partial owner.
    let c=&data["cases"][0];let mut bad=c.clone();bad["boundary"]["core_links"][0][0]["contacts"]=json!([]);
    assert!(try_initial(&bad).is_err());
    for change in 0..5{let mut bad=c.clone();match change{
        0=>bad["boundary"]["systems"][0]["cores"]=json!([1,0,1]),
        1=>bad["boundary"]["systems"][0]["groups"][0]["energy"]=json!(f32::NAN.to_bits()),
        2=>bad["boundary"]["contact_bindings"][0]["system"]=json!(99),
        3=>bad["boundary"]["systems"][0]["groups"][0]["contacts"]=json!([]),
        _=>bad["boundary"]["systems"][1]["groups"][0]["cores"]=json!([1,0]),
    }assert!(try_initial(&bad).is_err(),"invalid boundary {change}");}
    println!("contact ownership invalid and repeated edits cannot commit prefixes");
}
#[test] fn real_native_contact_merges_deletions_and_pair_clocks_match(){verify();}
#[test] fn invalid_contact_ownership_edits_preserve_registry_and_lists(){invalid();}
