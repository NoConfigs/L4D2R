//! Authored metadata operations only: no contact discovery or numeric solving.
use crate::ivp_unit_registry::Registry;
use serde_json::{Value,json};
fn compare(a:&Value,b:&Value,path:&str){
    match (a,b){
        (Value::Object(a),Value::Object(b))=>{assert_eq!(a.len(),b.len(),"{path} object size");for (k,v) in a{compare(v,&b[k],&format!("{path}.{k}"));}},
        (Value::Array(a),Value::Array(b))=>{assert_eq!(a.len(),b.len(),"{path} array size");for (i,(a,b)) in a.iter().zip(b).enumerate(){compare(a,b,&format!("{path}[{i}]"));}},
        _=>assert_eq!(a,b,"{path}"),
    }
}
fn replay(case:&Value)->Vec<Value>{
    let mut r=Registry::new(3);
    for (id,fixed) in [(0,false),(1,false),(2,true),(4,false),(5,false)]{r.register_core(id,fixed).unwrap();}
    let mut poll=crate::ivp_movement::Poll{remaining:10,seed:1};
    let mut out=vec![];
    for op in case["states"].as_array().unwrap(){
        let body=op["body"].as_i64().unwrap() as usize;
        let system=op["system"].as_i64().unwrap() as usize;
        let other=op["other"].as_i64().unwrap() as usize;
        match op["action"].as_str().unwrap(){
            "created"=>{},
            "wake"=>r.wake(body).unwrap(),
            // Fresh native environments run two PSI scopes in this first
            // interval. No contact controller is attached during simulation.
            "step"=>for _ in 0..2{r.begin_psi().unwrap();r.finish_psi_units(&mut poll,|_|Ok(1)).unwrap();},
            "join"=>r.attach_ragdoll(0,0,4).unwrap(),
            "detach_joint"=>r.destroy_constraint(0).unwrap(),
            "create_system"=>r.create_contact_system(system).unwrap(),
            "destroy_system"=>r.destroy_contact_system(system).unwrap(),
            "attach"=>r.attach_contact_core(system,body).unwrap(),
            "detach"=>r.detach_contact_core(system,body).unwrap(),
            "merge"=>r.merge_contact_units(body,other).unwrap(),
            "mark"=>r.mark_contact_graph(body).unwrap(),
            "repair"=>r.repair_contact_graph(body).unwrap(),
            action=>panic!("unknown contact metadata operation {action}"),
        }
        out.push(r.contact_snapshot(2));
    }
    out
}
pub(crate) fn verify(){
    let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-registry.json")).unwrap();
    let mut count=0;
    for case in data["cases"].as_array().unwrap(){
        let actual=replay(case);
        for (i,(a,op)) in actual.iter().zip(case["states"].as_array().unwrap()).enumerate(){
            let expected=json!({"registry":op["registry"],"controllers":op["controllers"],"systems":op["systems"],"unit_flags":op["unit_flags"]});
            compare(a,&expected,&format!("case{} step{i} {}",case["scenario"],op["action"]));count+=1;
        }
        let mut corrupt=case.clone();
        for op in corrupt["states"].as_array_mut().unwrap(){for key in ["registry","controllers","systems","unit_flags"]{op[key]=Value::Null;}}
        assert_eq!(actual,replay(&corrupt));
    }
    println!("contact registry: 8 native worlds / {count} persistent metadata checkpoints; expected-state corruption controls pass");
}
pub(crate) fn invalid_operations(){
    let mut r=Registry::new(3);r.register_core(0,false).unwrap();r.register_core(1,false).unwrap();r.register_core(2,true).unwrap();
    r.create_contact_system(0).unwrap();r.attach_contact_core(0,0).unwrap();r.attach_contact_core(0,2).unwrap();
    let before=format!("{r:?}");
    for op in 0..12{
        let result=match op{
            0=>r.create_contact_system(0),1=>r.attach_contact_core(0,0),2=>r.attach_contact_core(8,1),
            3=>r.attach_contact_core(0,99),4=>r.detach_contact_core(0,1),5=>r.destroy_contact_system(0),
            6=>r.remove_core(0),7=>r.remove_core(2),8=>r.merge_contact_units(0,2),
            9=>r.merge_contact_units(99,0),10=>r.mark_contact_graph(99),_=>r.repair_contact_graph(99),
        };
        assert!(result.is_err(),"invalid operation {op}");assert_eq!(format!("{r:?}"),before,"invalid operation {op} changed owner");
    }
    r.detach_contact_core(0,0).unwrap();r.detach_contact_core(0,2).unwrap();r.destroy_contact_system(0).unwrap();
    r.create_contact_system(0).unwrap();r.destroy_contact_system(0).unwrap();r.contact_snapshot(2);
    println!("contact registry: invalid operations preserve entire owner; empty system identity reuse passes");
}
#[test] fn native_contact_controller_registration_and_graph_repair_match(){verify();}
#[test] fn invalid_contact_metadata_edits_are_atomic(){invalid_operations();}
