use super::*;
use l4d2_formats::phy::{CompactEdge,TriangleHeader};
fn word(v:&serde_json::Value)->u32{v.as_u64().unwrap() as u32}
fn frame(row:&serde_json::Value,i:usize)->PreparedObjectCache{
    let f=&row["frames"][i];let value=|j|f32::from_bits(word(&f[j]));
    PreparedObjectCache{core_position:Vec3::new(value(0),value(1),value(2)),pose:crate::ivp_motion::Pose{position:Vec3::new(value(20),value(21),value(22)),rotation:Mat3::from_cols_array(&std::array::from_fn(|j|value(8+(j%3)*4+j/3)))},quaternion:[0.,0.,0.,1.],frame_stamp:word(&row["query_frame"])}
}
fn geometry(q:&Query)->[u32;5]{let g=q.geometry.unwrap();[g.projection.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()]}
#[test]
fn native_outer_retries_preferred_order_and_four_frame_continuation(){
    check(include_str!("../../tools/fixtures/native-ivp-minimizer-outer.json"),4608,0);
}
#[test]
fn native_outer_overlap_coincident_and_retirement_requests(){
    check(include_str!("../../tools/fixtures/native-ivp-minimizer-retirement.json"),1152,397);
}
#[test]
fn native_outer_lifecycle_flags_gate_callback_without_changing_query_geometry(){
    let data:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-minimizer-retirement-policy.json")).unwrap();
    let mut ordinary=0;let mut callback=0;let mut suppressed=0;
    for (n,c) in data["cases"].as_array().unwrap().iter().enumerate(){
        let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(word(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f32::from_bits(word(&h[20+i*4+j]))))).collect()}).collect();
        let headers:Vec<Vec<_>>=c["hulls"].as_array().unwrap().iter().map(|h|(0..4).map(|i|TriangleHeader(word(&h[4+i*4]))).collect()).collect();
        let mut q=Query{retired:false,order:0,recovery:None,frame:0,kinds:[1,1],vertex:CompactEdgeId{triangle:0,edge:2},face:CompactEdgeId{triangle:1,edge:2},geometry:Some(Geometry{projection:0.,distance:0.,normal:Vec3::ZERO})};
        let h=[&hulls[0],&hulls[1]];let head=[headers[0].as_slice(),headers[1].as_slice()];let frames=std::array::from_fn(|i|frame(c,i));let mut flags=word(&c["input_flags"]);
        let result=q.refresh_contact(1,h,frames,0.02,head,&mut flags).unwrap();
        if word(&c["flags"])&0x4000==0{assert_eq!(result,Refresh::Ordinary);ordinary+=1;}
        else{let notify=word(&c["retired"])==1;assert_eq!(result,Refresh::Retirement{notify_owner:notify});if notify{callback+=1;}else{suppressed+=1;}}
        assert_eq!(flags,word(&c["flags"]),"case{n} flags");assert_eq!(q.frame,1);assert_eq!(q.order,((flags>>8)&1) as usize);
        for (i,id) in [q.vertex,q.face].into_iter().enumerate(){let e=&c["features"][i];assert_eq!([id.triangle,id.edge,q.kinds[i] as usize],[word(&e[0]) as usize,word(&e[1]) as usize,word(&e[2]) as usize],"case{n} side{i}");}
        let e=&c["geometry"];assert_eq!(geometry(&q),[word(&e[0]),word(&e[2]),word(&e[3]),word(&e[4]),word(&e[5])],"case{n} geometry");
        let retained=(flags,geometry(&q),q.kinds,q.vertex,q.face,q.order,q.retired);
        assert_eq!(q.refresh_contact(1,h,frames,0.02,head,&mut flags).unwrap(),Refresh::Reused);
        assert_eq!(retained,(flags,geometry(&q),q.kinds,q.vertex,q.face,q.order,q.retired));assert_eq!(c["reused_unchanged"],true);
    }
    assert_eq!((ordinary,callback,suppressed),(512,360,152));
}
fn check(raw:&str,expected_count:usize,expected_retired:usize){
    let data:serde_json::Value=serde_json::from_str(raw).unwrap();
    let budgets:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-minimizer-budget.json")).unwrap();
    assert_eq!(data["cases"].as_array().unwrap().len(),1152);let mut count=0;let mut retired=0;
    for (n,c) in data["cases"].as_array().unwrap().iter().enumerate(){
        let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(word(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f32::from_bits(word(&h[20+i*4+j]))))).collect()}).collect();
        let headers:Vec<Vec<_>>=c["hulls"].as_array().unwrap().iter().map(|h|(0..4).map(|i|TriangleHeader(word(&h[4+i*4]))).collect()).collect();
        let reverse=c["reverse"].as_u64().unwrap() as usize;let order=[reverse,1-reverse];let h=order.map(|i|&hulls[i]);let head=order.map(|i|headers[i].as_slice());
        let ids=order.map(|physical|CompactEdgeId{triangle:usize::from(physical==1),edge:2});
        let initial=c["initial_geometry"].as_array().map(|g|Geometry{projection:f32::from_bits(word(&g[0])),distance:f32::from_bits(word(&g[2])),normal:Vec3::from_array(std::array::from_fn(|i|f32::from_bits(word(&g[3+i]))))});
        let mut query=Query{retired:false,order:0,recovery:None,frame:0,kinds:[1,1],vertex:ids[0],face:ids[1],geometry:initial};
        for row in std::iter::once(c).chain(c["continuation"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter()){
            let stamp=word(&row["query_frame"]);let f=order.map(|i|frame(row,i));
            take_budget_observations();
            assert!(query.refresh_with_headers(stamp,h,f,0.02,head).unwrap_or_else(|error|panic!("case{n} frame{stamp}: {error}")));
            let observed=take_budget_observations();
            if expected_retired>0{
                let expected:Vec<_>=budgets["cases"][n].as_array().unwrap().iter().map(|r|BudgetObservation{
                    remaining:r["remaining"].as_i64().unwrap() as i32,seen:r["seen"].as_u64().unwrap() as usize,
                    features:std::array::from_fn(|i|(r["ids"][i][0].as_u64().unwrap() as usize,r["ids"][i][1].as_u64().unwrap() as usize,r["ids"][i][2].as_u64().unwrap() as usize,r["kinds"][i].as_i64().unwrap() as i16)),
                }).collect();
                let physical:Vec<_>=observed.into_iter().map(|r|BudgetObservation{features:r.features.map(|(owner,t,e,k)|(order[owner],t,e,k)),..r}).collect();
                assert_eq!(physical,expected,"case{n} frame{stamp} ledger");
            }
            count+=1;assert_eq!(query.frame,stamp);assert!(query.recovery.is_none());
            assert_eq!(query.retirement_requested(),row["retired"]==1,"case{n} frame{stamp} retirement");retired+=usize::from(query.retirement_requested());
            assert_eq!(order[query.order],((word(&row["flags"])>>8)&1) as usize,"case{n} frame{stamp} preferred role");
            let ids=[query.vertex,query.face];for i in 0..2{let e=&row["features"][order[i]];assert_eq!([ids[i].triangle,ids[i].edge,query.kinds[i] as usize],[word(&e[0]) as usize,word(&e[1]) as usize,word(&e[2]) as usize],"case{n} frame{stamp} endpoint{i}");}
            let e=&row["geometry"];assert_eq!(geometry(&query),[word(&e[0]),word(&e[2]),word(&e[3]),word(&e[4]),word(&e[5])],"case{n} frame{stamp} geometry");
            let before=(query.frame,query.kinds,query.vertex,query.face,query.order,query.retired,geometry(&query));
            assert!(!query.refresh_with_headers(stamp,h,f,0.02,head).unwrap());
            assert_eq!(before,(query.frame,query.kinds,query.vertex,query.face,query.order,query.retired,geometry(&query)));assert!(query.recovery.is_none());
        }
    }
    assert_eq!(count,expected_count);assert_eq!(retired,expected_retired);
}
