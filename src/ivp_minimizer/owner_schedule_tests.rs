
 use super::*;
 use l4d2_formats::phy::CompactEdge;
 use crate::ivp_joint_apply::tests::{body,snapshot};
 #[test]
 fn compact_owner_refresh_then_source_dispatch_and_queue_match_native(){
    let d:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-compact-owner-schedule-early.json")).unwrap();let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let mut counts=[0;4];
    for c in d["cases"].as_array().unwrap(){let mut bodies=std::array::from_fn(|i|body(&c["before"][i]));let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()}).collect();let headers:Vec<Vec<_>>=c["hulls"].as_array().unwrap().iter().map(|h|(0..4).map(|i|TriangleHeader(w(&h[4+i*4]))).collect()).collect();let h=[&hulls[0],&hulls[1]];let head=[headers[0].as_slice(),headers[1].as_slice()];
        let clock=Context{time:bodies[0].clock,frame:w(&c["frame"])};let mut cache=WorldCache::new(8).unwrap();
        for i in 0..2{let b=body(&c["initial"][i]);assert_eq!(cache.register_core(b.prediction),b.id);assert_eq!(cache.register_object(b.id,b.object_flags,crate::ivp_motion::ObjectFrame{shift:None,rotation:None}).unwrap(),b.id);}
        let initial=Context{time:f64::from_bits(c["initial_clock"].as_u64().unwrap()),frame:w(&c["initial_frame"])};for i in 0..2{cache.access(i,initial).unwrap();}
        for b in bodies{cache.update_core(b.id,b.prediction).unwrap();cache.set_flags(b.id,b.object_flags).unwrap();}
        for i in 0..2{let slot=cache.access(i,clock).unwrap();let p=cache.get(slot).unwrap();let n=&c["frames"][i];assert_eq!(p.core_position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|j|w(&n[j])),"case{} cache core{i}",c["id"]);assert_eq!(p.pose.position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|j|w(&n[20+j])),"case{} cache origin{i}",c["id"]);assert_eq!(p.pose.rotation.to_cols_array().map(f32::to_bits),std::array::from_fn::<_,9,_>(|j|w(&n[8+(j%3)*4+j/3])),"case{} cache rotation{i}",c["id"]);}
        let mut contact=Contact::new([0,1],w(&c["input_flags"]),0.02).unwrap();let gravity=std::array::from_fn(|i|f(&c["gravity"][i]));let update=contact.refresh(&mut cache,clock,&mut bodies,h,head,f(&c["dt"]),gravity).unwrap_or_else(|error|panic!("case{}: {error}",c["id"]));
        assert_eq!(contact.flags,w(&c["flags"]),"case{} flags",c["id"]);assert_eq!(contact.query.frame,w(&c["query_frame"]));
        let callback=update.callback_references.is_some();assert_eq!(usize::from(callback),w(&c["callback_count"]) as usize,"case{} callback",c["id"]);if callback{assert_eq!(update.callback_references,Some([0,0]));counts[2]+=1;}
        counts[usize::from(contact.query.retirement_requested())]+=1;
        for (i,e) in [contact.query.vertex,contact.query.face].into_iter().enumerate(){assert_eq!([e.triangle,e.edge,contact.query.kinds[i] as usize],std::array::from_fn(|j|w(&c["features"][i][j]) as usize),"case{} side{i}",c["id"]);}
        if let Some(g)=contact.query.geometry{let n=&c["geometry"];assert_eq!([g.projection.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()],[w(&n[0]),w(&n[2]),w(&n[3]),w(&n[4]),w(&n[5])],"case{} geometry",c["id"]);}else{assert_eq!(c["geometry"],serde_json::json!([0,0,0,0,0,0]));}
        assert_eq!(serde_json::json!(bodies.map(snapshot)),c["after"],"case{} bodies",c["id"]);counts[3]+=usize::from(c["before"]!=c["after"]);
        let retained=serde_json::json!(bodies.map(snapshot));let flags=contact.flags;let again=contact.refresh(&mut cache,clock,&mut bodies,h,head,f(&c["dt"]),gravity).unwrap();assert_eq!(again.outcome,Refresh::Reused);assert_eq!(again.recovery,None);assert_eq!(flags,contact.flags);assert_eq!(serde_json::json!(bodies.map(snapshot)),retained);assert_eq!(retained,c["reuse_after"]);assert!(cache.metadata().0.iter().all(|s|s.references==0));
        let s=&c["schedule"];let mut counter=w(&s["counter_before"]);let mut queue=crate::ivp_event_queue::Queue::new(8,f64::from_bits(s["epoch"].as_u64().unwrap())).unwrap();
        if s["called"]==true{
          let config=crate::ivp_distance_schedule::Config{targets:std::array::from_fn(|j|f(&s["targets"][j])),slow_gate:f(&s["slow_gate"]),time_scale:f(&s["time_scale"]),gap:f(&s["gap"])};
          let properties=std::array::from_fn(|j|{let p=&s["metadata"][j];let velocity=Vec3::from_array(bodies[j].linear[..3].try_into().unwrap());assert_eq!(crate::ivp_bounds::speed(velocity).to_bits(),w(&p["linear_speed"]));Properties{radius:f(&p["radius"]),inverse_diameter:f(&p["inverse_diameter"]),angular_speed:f(&p["angular_speed"]),angular_bound:f(&p["angular_bound"]),sweep_axis:Vec3::from_array(std::array::from_fn(|k|f(&p["axis"][k])))}});
          let outcome=contact.schedule(&mut cache,clock,&bodies,h,&mut queue,0,&mut counter,Schedule{deadline:f64::from_bits(s["deadline"].as_u64().unwrap()),dt:f(&c["dt"]),delay:w(&s["delay"]) as u8,config,face_epsilon:f(&s["face_epsilon"]),properties,budgets:[crate::ivp_distance_schedule::Budget{time:clock.time,flags:0,rates:[0.;2],accumulated:[0.;2]};2],allow_far:false}).unwrap_or_else(|e|panic!("case{} schedule: {e}",c["id"]));
          assert_eq!(outcome.kind,w(&s["kind"]),"case{} kind",c["id"]);if outcome.kind!=0{assert_eq!(outcome.candidate.unwrap().to_bits(),s["candidate"].as_u64().unwrap(),"case{} candidate",c["id"]);}assert_eq!(outcome.handle.is_some(),!s["key"].is_null());
        }
        assert_eq!(contact.flags,w(&s["flags"]),"case{} scheduled flags",c["id"]);assert_eq!(counter,w(&s["counter_after"]));assert_eq!(queue.scheduled(0).map(|(_,k)|k.to_bits()),s["key"].as_u64().map(|v|v as u32),"case{} queue key",c["id"]);assert_eq!(serde_json::json!(bodies.map(snapshot)),s["after"],"case{} schedule body bank",c["id"]);assert!(cache.metadata().0.iter().all(|s|s.references==0));
    }assert_eq!(counts,[963,189,63,12]);assert_eq!(d["cases"].as_array().unwrap().len(),1152);
 }
