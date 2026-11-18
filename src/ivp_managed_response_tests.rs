use crate::{ivp_managed_response::{ResponseWorld,Body,Object,Feature,Settings},ivp_contact_pipeline::CoreFrame,ivp_contact::Core,ivp_create::{Endpoint,Feature as CompactFeature},ivp_managed_contacts::Settings as AdmissionSettings,ivp_refresh,ivp_unit_registry::Registry};
use bevy::prelude::*;
use serde_json::{Value,json};
fn f(v:&Value)->f32{v.as_f64().unwrap() as f32}
fn vec(v:&Value)->Vec3{Vec3::from_array(std::array::from_fn(|i|f(&v[i])))}
fn rows(v:&Value)->[Vec3;3]{std::array::from_fn(|i|vec(&v[i]))}
fn endpoint(e:&Value)->Endpoint{let v=&e["feature"];Endpoint{object:e["object"].as_u64().unwrap(),feature:CompactFeature{kind:v["kind"].as_i64().unwrap() as i16,edge:v["edge"].as_u64().unwrap(),opposite:v["opposite"].as_u64().unwrap(),triangle:v["triangle"].as_u64().unwrap(),ledge:v["ledge"].as_u64().unwrap(),vertex:v["vertex"].as_u64().unwrap() as u16,vertices:rows(&v["vertices"])}}}
fn world(c:&Value)->ResponseWorld{
    let mut registry=Registry::new(4);let bodies=c["bodies"].as_array().unwrap().iter().enumerate().map(|(i,v)|{let flags=v["flags"].as_u64().unwrap() as u16;registry.register_core(i,flags&2!=0).unwrap();Body{flags,core:Core{inverse_mass:f(&v["inverse_mass"]),inverse_inertia:vec(&v["inverse_inertia"]),linear:vec(&v["linear"]),angular:vec(&v["angular"]),pending_linear:vec(&v["pending_linear"]),pending_angular:vec(&v["pending_angular"])},frame:CoreFrame{rotation:Mat3::from_cols_array_2d(&rows(&v["rotation"]).map(|v|v.to_array())).transpose(),position:vec(&v["position"])},stamps:std::array::from_fn(|i|v["stamps"][i].as_f64().unwrap())}}).collect();
    let objects=(0..2).map(|side|{let e=&c["endpoints"][side];let core=e["object"].as_u64().unwrap() as usize-1;let s=&c["shapes"][side];let m=&c["bodies"][core]["material"];(core as u64+1,Object{core,rotation:rows(&s["rotation"]),translation:vec(&s["translation"]),radius:f(&s["radius"]),material_flags:m["flags"].as_u64().unwrap() as u32,friction:f(&m["friction"])})}).collect();
    let k=&c["settings"];ResponseWorld::new(registry,bodies,objects,Settings{dt:f(&c["dt"]),admission:AdmissionSettings{distance:f(&k["distance"]),area_epsilon:k["area_epsilon"].as_f64().unwrap()},refresh:ivp_refresh::Settings{epsilon:f(&k["epsilon"]),bary_bias:f(&k["bary_bias"]),area_epsilon:k["area_epsilon"].as_f64().unwrap(),one:f(&k["one"]),opposite:f(&k["opposite"])},threshold_extra:f(&k["threshold_extra"]),minimum_impulse:f(&k["minimum_impulse"]),minimum_determinant_squared:f(&k["minimum_determinant_squared"])},c["time"].as_f64().unwrap()).unwrap()
}
fn features(c:&Value)->[Feature;2]{std::array::from_fn(|i|Feature{material_code:0,endpoint:endpoint(&c["endpoints"][i]),vertices:rows(&c["shapes"][i]["vertices"]),canonical:rows(&c["shapes"][i]["canonical"])})}
fn compare(a:&Value,b:&Value,path:&str){match(a,b){
    (Value::Object(a),Value::Object(b))=>for(k,v)in a{compare(v,&b[k],&format!("{path}.{k}"));},
    (Value::Array(a),Value::Array(b))=>{assert_eq!(a.len(),b.len(),"{path}");for(i,(a,b))in a.iter().zip(b).enumerate(){compare(a,b,&format!("{path}[{i}]"));}},
    (Value::Number(a),Value::Number(b))=>{if path.ends_with(".time")||path.contains(".stamps["){assert_eq!(a.as_f64().unwrap().to_bits(),b.as_f64().unwrap().to_bits(),"{path}");}else if a.is_f64()||b.is_f64(){assert_eq!((a.as_f64().unwrap() as f32).to_bits(),(b.as_f64().unwrap() as f32).to_bits(),"{path}: {a} vs {b}");}else{assert_eq!(a,b,"{path}");}},
    _=>assert_eq!(a,b,"{path}"),
}}
fn replay(c:&Value,check:bool)->Vec<Value>{
    let mut w=world(c);let key=w.resolve(0xc0000,features(c),Some(1)).unwrap().unwrap();assert_eq!(key,1);let mut actual=vec![w.snapshot(key)];
    if check{compare(&actual[0],&c["states"][0]["numeric"],&format!("world{} admission",c["scenario"]));}
    for(i,r)in c["states"].as_array().unwrap().iter().enumerate().skip(1){
        w.advance(1,vec(&r["kick"]),vec(&r["angular"]),vec(&r["shift"])).unwrap();w.refresh(0).unwrap();let refresh=w.snapshot(key);w.tangent(0).unwrap();let tangent=w.snapshot(key);let retired=w.normal(0).unwrap();let normal=w.snapshot(key);
        if check{for (phase,a) in [&refresh,&tangent,&normal].into_iter().enumerate(){compare(a,&r["phases"][phase],&format!("world{} tick{i} phase{phase}",c["scenario"]));}}
        assert_eq!(retired,i==13);actual.extend([refresh,tangent,normal]);
    }
    let d=w.diagnostics();assert!(d["metadata"]["contact_bindings"].as_array().unwrap().is_empty());assert!(d["metadata"]["systems"].as_array().unwrap().iter().all(Value::is_null));
    let(effects,contacts,features)=w.take_effects();assert_eq!(contacts,[1]);assert_eq!(features.len(),1);assert_eq!(effects.iter().filter(|e|matches!(e,crate::ivp_unit_registry::Effect::ResetClock(_))).count(),2);let(e,c,f)=w.take_effects();assert!(e.is_empty()&&c.is_empty()&&f.is_empty());actual
}
pub(crate) fn verify(){let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-response.json")).unwrap();let mut checkpoints=0;
    for c in data["cases"].as_array().unwrap(){let source=replay(c,true);checkpoints+=source.len();let mut corrupt=c.clone();corrupt["states"][0]["numeric"]=Value::Null;for r in corrupt["states"].as_array_mut().unwrap().iter_mut().skip(1){r["phases"]=Value::Null;r["time"]=json!(-99.);}
        assert_eq!(source,replay(&corrupt,false));
    }assert_eq!(checkpoints,320);println!("fresh managed numerical response:8 native worlds /320 phase checkpoints /104 retained steps /8 source-owned retirements; no later state reseeding");
}
pub(crate) fn invalid(){let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-response.json")).unwrap();let c=&data["cases"][0];let mut w=world(c);w.resolve(0xc0000,features(c),Some(1)).unwrap();let before=(w.snapshot(1),w.diagnostics());assert!(w.tangent(0).is_err());assert!(w.normal(0).is_err());assert!(w.advance(1,Vec3::NAN,Vec3::ZERO,Vec3::ZERO).is_err());assert!(w.advance(99,Vec3::ZERO,Vec3::ZERO,Vec3::ZERO).is_err());let mut bad=features(c);bad[0].canonical[0]=Vec3::NAN;assert!(w.resolve(0xc0000,bad,Some(1)).is_err());assert_eq!(before,(w.snapshot(1),w.diagnostics()));w.refresh(0).unwrap();let before=(w.snapshot(1),w.diagnostics());assert!(w.refresh(0).is_err());assert!(w.advance(1,Vec3::ZERO,Vec3::ZERO,Vec3::ZERO).is_err());assert_eq!(before,(w.snapshot(1),w.diagnostics()));}
#[test]fn fresh_managed_contacts_carry_native_numerical_state_to_retirement(){verify();}
#[test]fn managed_response_invalid_inputs_and_phase_order_are_atomic(){invalid();}
