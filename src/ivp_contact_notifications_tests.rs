use crate::{ivp_managed_contacts::{Managed,Settings},ivp_unit_registry::Registry,
    ivp_create::{Endpoint,Feature},ivp_contact_notifications::{Notifications,Event,Kind}};
use serde_json::{Value,json};
use bevy::prelude::Vec3;
fn endpoint(v:&Value)->Endpoint{let f=&v["feature"];Endpoint{object:v["object"].as_u64().unwrap(),feature:Feature{kind:f["kind"].as_i64().unwrap() as i16,edge:f["edge"].as_u64().unwrap(),opposite:f["opposite"].as_u64().unwrap(),triangle:f["triangle"].as_u64().unwrap(),ledge:f["ledge"].as_u64().unwrap(),vertex:f["vertex"].as_u64().unwrap() as u16,vertices:std::array::from_fn(|i|Vec3::from_array(std::array::from_fn(|j|f["vertices"][i][j].as_f64().unwrap() as f32)))}}}
fn replay(c:&Value,compare:bool)->Vec<Value>{
    let mut r=Registry::new(4);let fixed=(0..4).map(|i|{r.register_core(i,i==0).unwrap();(i,i==0)}).collect();
    let mut w=Managed::new(r,fixed,(0..4).map(|i|(i as u64+1,i)).collect(),Settings{distance:c["settings"]["distance"].as_f64().unwrap() as f32,area_epsilon:c["settings"]["area_epsilon"].as_f64().unwrap()}).unwrap();
    for l in c["listeners"].as_array().unwrap(){w.subscribe(l["id"].as_u64().unwrap() as usize,l["flags"].as_u64().unwrap() as u32).unwrap();}
    let mut history=vec![];let mut states=vec![];
    for (i,row) in c["states"].as_array().unwrap().iter().enumerate(){let op=row.get("output").unwrap_or(row);
        match op["action"].as_str().unwrap(){
            "initial"=>{},
            "admit"=>{let input=&row["input"];let hint=input["hint"].as_i64().unwrap();w.resolve(input["flags"].as_u64().unwrap() as u32,input["time"].as_f64().unwrap(),std::array::from_fn(|s|endpoint(&input["endpoints"][s])),(hint>=0).then_some(hint as usize)).unwrap();},
            "remove"=>w.remove(op["contact"].as_u64().unwrap()).unwrap(),
            "destroy"=>w.destroy_empty(op["system"].as_u64().unwrap() as usize).unwrap(),
            _=>panic!("unknown notification operation"),
        }
        for d in w.take_notifications(){history.push(json!([d.listener,match d.event.kind{Kind::Created=>0,Kind::Deleted=>1,Kind::PairAdded=>2,Kind::PairRemoved=>3},d.event.contact.unwrap_or(0),d.event.cores[0],d.event.cores[1]]));}
        assert!(w.take_notifications().is_empty());
        if compare{assert_eq!(json!(history),op["notifications"],"world{} row{i}",c["scenario"]);}
        let mut state=w.snapshot();state["notifications"]=json!(history);states.push(state);
    }
    for l in c["listeners"].as_array().unwrap(){w.unsubscribe(l["id"].as_u64().unwrap() as usize).unwrap();}
    states
}
pub(crate) fn verify(){
    let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-notifications.json")).unwrap();let(mut count,mut checkpoints)=(0,0);
    for c in data["cases"].as_array().unwrap(){let actual=replay(c,true);checkpoints+=actual.len();count+=actual.last().unwrap()["notifications"].as_array().unwrap().len();
        let mut corrupt=c.clone();for row in corrupt["states"].as_array_mut().unwrap(){let op=if row.get("output").is_some(){&mut row["output"]}else{row};op["notifications"]=json!([[999,999]]);}
        assert_eq!(actual,replay(&corrupt,false));
    }
    assert_eq!((checkpoints,count),(216,888));println!("source-owned contact/pair notifications:216 native checkpoints /888 exact ordered deliveries; expected-output corruption controls pass");
}
pub(crate) fn lifecycle(){
    let mut n=Notifications::default();n.subscribe(1,4).unwrap();n.subscribe(2,0).unwrap();n.subscribe(1,4).unwrap();n.subscribe(3,12).unwrap();
    let event=Event{kind:Kind::PairAdded,contact:None,cores:[1,0]};n.publish(event);
    assert_eq!(n.take().iter().map(|d|d.listener).collect::<Vec<_>>(),[3,1,1]);
    n.unsubscribe(1).unwrap();n.set_flags(2,5).unwrap();n.set_flags(3,8).unwrap();n.publish(event);
    assert_eq!(n.take().iter().map(|d|d.listener).collect::<Vec<_>>(),[2,1]);
    let before=format!("{n:?}");assert!(n.unsubscribe(999).is_err());assert!(n.subscribe(0,4).is_err());assert!(n.subscribe(1,0).is_err());assert!(n.set_flags(999,4).is_err());assert_eq!(format!("{n:?}"),before);
    // Queued observations preserve their recipient selection after unsubscribe.
    n.publish(event);n.unsubscribe(2).unwrap();assert_eq!(n.take().iter().map(|d|d.listener).collect::<Vec<_>>(),[2,1]);assert!(n.take().is_empty());
}
#[test]fn native_contact_notification_order_and_filtering_match(){verify();}
#[test]fn contact_listener_lifecycle_and_delivery_drain(){lifecycle();}
