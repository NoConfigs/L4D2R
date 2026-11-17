
 use super::*;
 use l4d2_formats::phy::CompactEdge;
 use crate::ivp_joint_apply::tests::{body,snapshot};
 #[test]
 fn recursive_query_notification_preserves_body_banks_and_cache_lifetimes(){
    let d:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-recursive-query-owner.json")).unwrap();let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let mut counts=[0;4];
    for c in d["cases"].as_array().unwrap(){let mut bodies=std::array::from_fn(|i|body(&c["before"][i]));let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()}).collect();let headers:Vec<Vec<_>>=c["hulls"].as_array().unwrap().iter().map(|h|(0..4).map(|i|TriangleHeader(w(&h[4+i*4]))).collect()).collect();let h=[&hulls[0],&hulls[1]];let head=[headers[0].as_slice(),headers[1].as_slice()];
        let clock=Context{time:bodies[0].clock,frame:w(&c["frame"])};let mut cache=WorldCache::new(8).unwrap();
        for i in 0..2{let b=body(&c["initial"][i]);assert_eq!(cache.register_core(b.prediction),b.id);assert_eq!(cache.register_object(b.id,b.object_flags,crate::ivp_motion::ObjectFrame{shift:None,rotation:None}).unwrap(),b.id);}
        let initial=Context{time:f64::from_bits(c["initial_clock"].as_u64().unwrap()),frame:w(&c["initial_frame"])};for i in 0..2{cache.access(i,initial).unwrap();}
        for b in bodies{cache.update_core(b.id,b.prediction).unwrap();cache.set_flags(b.id,b.object_flags).unwrap();}
        for i in 0..2{let slot=cache.access(i,clock).unwrap();let p=cache.get(slot).unwrap();let n=&c["frames"][i];assert_eq!(p.core_position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|j|w(&n[j])),"case{} cache core{i}",c["id"]);assert_eq!(p.pose.position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|j|w(&n[20+j])),"case{} cache origin{i}",c["id"]);assert_eq!(p.pose.rotation.to_cols_array().map(f32::to_bits),std::array::from_fn::<_,9,_>(|j|w(&n[8+(j%3)*4+j/3])),"case{} cache rotation{i}",c["id"]);}
        let mut contact=Contact::new([0,1],w(&c["input_flags"]),0.02).unwrap();let update=contact.refresh_geometry(&mut cache,clock,&bodies,h,head).unwrap_or_else(|error|panic!("case{}: {error}",c["id"]));
        assert_eq!(contact.flags,w(&c["flags"]),"case{} flags",c["id"]);assert_eq!(contact.query.frame,w(&c["query_frame"]));
        let callback=update.callback_references.is_some();assert_eq!(usize::from(callback),w(&c["callback_count"]) as usize,"case{} callback",c["id"]);if callback{assert_eq!(update.callback_references,Some([0,0]));counts[2]+=1;}
        counts[usize::from(contact.query.retirement_requested())]+=1;
        for (i,e) in [contact.query.vertex,contact.query.face].into_iter().enumerate(){assert_eq!([e.triangle,e.edge,contact.query.kinds[i] as usize],std::array::from_fn(|j|w(&c["features"][i][j]) as usize),"case{} side{i}",c["id"]);}
        if let Some(g)=contact.query.geometry{let n=&c["geometry"];assert_eq!([g.projection.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()],[w(&n[0]),w(&n[2]),w(&n[3]),w(&n[4]),w(&n[5])],"case{} geometry",c["id"]);}else{assert_eq!(c["geometry"],serde_json::json!([0,0,0,0,0,0]));}
        assert_eq!(serde_json::json!(bodies.map(snapshot)),c["after"],"case{} bodies",c["id"]);counts[3]+=usize::from(c["before"]!=c["after"]);
        let retained=serde_json::json!(bodies.map(snapshot));let flags=contact.flags;let again=contact.refresh_geometry(&mut cache,clock,&bodies,h,head).unwrap();assert_eq!(again.outcome,Refresh::Reused);assert_eq!(again.recovery,None);assert_eq!(flags,contact.flags);assert_eq!(serde_json::json!(bodies.map(snapshot)),retained);assert_eq!(retained,c["reuse_after"]);assert!(cache.metadata().0.iter().all(|s|s.references==0));
    }assert_eq!(counts,[954,198,66,0]);assert_eq!(d["cases"].as_array().unwrap().len(),1152);
 }
