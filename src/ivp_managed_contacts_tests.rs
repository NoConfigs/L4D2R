use crate::{ivp_managed_contacts::{Managed,Settings},ivp_unit_registry::Registry,ivp_create::{Endpoint,Feature}};
use serde_json::{Value,json};
use bevy::prelude::Vec3;
fn endpoint(v:&Value)->Endpoint{let f=&v["feature"];Endpoint{object:v["object"].as_u64().unwrap(),feature:Feature{kind:f["kind"].as_i64().unwrap() as i16,edge:f["edge"].as_u64().unwrap(),opposite:f["opposite"].as_u64().unwrap(),triangle:f["triangle"].as_u64().unwrap(),ledge:f["ledge"].as_u64().unwrap(),vertex:f["vertex"].as_u64().unwrap() as u16,vertices:std::array::from_fn(|i|Vec3::from_array(std::array::from_fn(|j|f["vertices"][i][j].as_f64().unwrap() as f32)))}}}
fn empty(c:&Value)->Managed{
    let mut r=Registry::new(4);let fixed=(0..4).map(|i|{r.register_core(i,i==0).unwrap();(i,i==0)}).collect();
    Managed::new(r,fixed,(0..4).map(|i|(i as u64+1,i)).collect(),Settings{distance:c["settings"]["distance"].as_f64().unwrap() as f32,area_epsilon:c["settings"]["area_epsilon"].as_f64().unwrap()}).unwrap()
}
fn projected(v:&Value)->Value{json!({"systems":v["systems"],"core_links":v["core_links"],"contact_bindings":v["contact_bindings"],"controller_keys":v["controller_keys"],"object_links":v["object_links"],"unit_members":v["unit_members"]})}
fn check(a:&Value,b:&Value,path:&str){match(a,b){
    (Value::Object(a),Value::Object(b))=>{assert_eq!(a.len(),b.len(),"{path}");for(k,v)in a{check(v,&b[k],&format!("{path}.{k}"));}},
    (Value::Array(a),Value::Array(b))=>{assert_eq!(a.len(),b.len(),"{path}");for(i,(a,b))in a.iter().zip(b).enumerate(){check(a,b,&format!("{path}[{i}]"));}},
    _=>assert_eq!(a,b,"{path}"),
}}
fn replay(c:&Value,compare:bool)->Vec<Value>{
    let mut world=empty(c);let mut states=vec![];
    for(i,row)in c["states"].as_array().unwrap().iter().enumerate(){let op=row.get("output").unwrap_or(row);
        match op["action"].as_str().unwrap(){
            "initial"=>{},
            "admit"=>{let input=&row["input"];let endpoints=std::array::from_fn(|s|endpoint(&input["endpoints"][s]));let hint=input["hint"].as_i64().unwrap();let out=world.resolve(input["flags"].as_u64().unwrap() as u32,input["time"].as_f64().unwrap(),endpoints,(hint>=0).then_some(hint as usize)).unwrap();
                assert_eq!(out.initial.is_some(),out.created);let result=json!({"key":out.key,"created":out.created,"system":out.system});
                if compare{let id=op["contact"].as_i64().unwrap();let system=op["system"].as_i64().unwrap();assert_eq!(result,json!({"key":(id>=0).then_some(id),"created":op["other"]==1,"system":(system>=0).then_some(system)}),"admission output world{} row{i}",c["scenario"]);}
                states.push(result);
            },
            "remove"=>{let key=op["contact"].as_u64().unwrap();world.remove(key).unwrap();let(effects,contacts,features)=world.take_effects();assert_eq!(contacts,[key as usize]);assert_eq!(features.len(),1);let reset=effects.iter().filter_map(|e|if let crate::ivp_unit_registry::Effect::ResetClock(core)=e{Some(*core)}else{None}).collect::<Vec<_>>();assert_eq!(reset,features[0].each_ref().map(|e|e.object as usize-1));let(empty_effects,empty_contacts,empty_features)=world.take_effects();assert!(empty_effects.is_empty()&&empty_contacts.is_empty()&&empty_features.is_empty());},
            "destroy"=>world.destroy_empty(op["system"].as_u64().unwrap() as usize).unwrap(),
            _=>panic!("unknown managed contact operation"),
        }
        let actual=world.snapshot();if compare{check(&actual,&projected(op),&format!("world{} row{i} {}",c["scenario"],op["action"]));}states.push(actual);
    }states
}
pub(crate) fn verify(){
    let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-admission.json")).unwrap();let mut checkpoints=0;
    for c in data["cases"].as_array().unwrap(){let source=replay(c,true);checkpoints+=c["states"].as_array().unwrap().len();let mut corrupt=c.clone();for row in corrupt["states"].as_array_mut().unwrap(){let op=if row.get("output").is_some(){&mut row["output"]}else{row};for key in ["systems","core_links","contact_bindings","controller_keys","object_links","unit_members"]{op[key]=Value::Null;}if op["action"]=="admit"{op["contact"]=json!(-99);op["system"]=json!(-99);op["other"]=json!(-99);}}
        assert_eq!(source,replay(&corrupt,false));
    }assert_eq!(checkpoints,216);println!("empty-state managed admission: 8 native worlds /216 checkpoints; object links, system choice, unit merges and corrupted-output controls pass");
}
pub(crate) fn invalid(){
    let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-admission.json")).unwrap();let c=&data["cases"][0];let first=&c["states"][1]["input"];let endpoints:[Endpoint;2]=std::array::from_fn(|i|endpoint(&first["endpoints"][i]));let mut world=empty(c);let before=format!("{world:?}");
    for case in 0..10{let mut ep=endpoints.clone();let mut time=first["time"].as_f64().unwrap();let mut hint=Some(1);let mut flags=first["flags"].as_u64().unwrap() as u32;match case{0=>ep[0].object=999,1=>time=f64::NAN,2=>ep[1].object=ep[0].object,3=>ep[0].feature.kind=-1,4=>ep[1].feature.vertices[0]=Vec3::NAN,5=>hint=Some(99),6=>ep[0].object=0,7=>flags=(flags&!0x300)|0x200,8=>ep[0].feature.ledge=0,_=>ep[0].feature.edge=0};assert!(world.resolve(flags,time,ep,hint).is_err());assert_eq!(format!("{world:?}"),before,"invalid admission {case}");}
    let key=world.resolve(first["flags"].as_u64().unwrap() as u32,first["time"].as_f64().unwrap(),endpoints,Some(1)).unwrap().key.unwrap();let before=format!("{world:?}");assert!(world.remove(999).is_err());assert_eq!(format!("{world:?}"),before);world.remove(key).unwrap();let before=format!("{world:?}");assert!(world.remove(key).is_err());assert_eq!(format!("{world:?}"),before);
    // A pre-existing joint must remain representable when contact ownership
    // adds its controller views; diagnostics previously assumed only base.
    let mut registry=Registry::new(4);for i in 0..4{registry.register_core(i,i==0).unwrap();}registry.attach_ragdoll(0,1,2).unwrap();
    let joint_world=Managed::new(registry,(0..4).map(|i|(i,i==0)).collect(),(0..4).map(|i|(i as u64+1,i)).collect(),Settings{distance:0.01,area_epsilon:1e-18}).unwrap();assert!(joint_world.snapshot().to_string().contains("joint_0"));
    println!("managed admission validation and repeated feature deletion preserve all owner/list/request state");
}
#[test]fn native_empty_state_admission_and_feature_lifetimes_match(){verify();}
#[test]fn invalid_managed_admission_and_deletion_are_atomic(){invalid();}
