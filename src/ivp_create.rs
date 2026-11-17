//! Native101055e0 gate,10105590 lookup,100fd6f0/7d0 identity and
//!100fd860 constructor fields. Shape callbacks/refresh are separate stages.
use bevy::prelude::*;
#[derive(Clone,Debug,PartialEq)]
pub(crate) struct Feature {pub kind:i16,pub edge:u64,pub opposite:u64,pub triangle:u64,pub ledge:u64,pub vertex:u16,pub vertices:[Vec3;3]}
/// Stable source identities for real compact ledges. `ledge_key` is assigned by
/// the owner and must be unique in its feature arena; it is not a native address.
pub(crate) fn from_compact(kind:i16,ledge_key:u32,convex:&l4d2_formats::phy::Convex,id:l4d2_formats::phy::CompactEdgeId)->Result<Feature,&'static str>{
    if !(0..=2).contains(&kind)||ledge_key==0{return Err("invalid compact feature namespace/kind");}
    let topology=convex.topology.as_ref().ok_or("synthetic hull has no compact topology")?;
    let edge=topology.edge(id).ok_or("invalid compact edge")?;
    let opposite=topology.opposite(id).ok_or("invalid compact opposite edge")?;
    if convex.triangle_headers.get(id.triangle).ok_or("missing compact triangle header")?.0&0xfff!=id.triangle as u32{return Err("inconsistent compact triangle identity");}
    let base=(ledge_key as u64)<<32;
    let edge_key=|id:l4d2_formats::phy::CompactEdgeId|base+((id.triangle as u64+1)*16+(id.edge as u64+1)*4);
    let mut vertices=[Vec3::ZERO;3];
    if kind==2{for (out,id) in vertices.iter_mut().zip([id,topology.next(id).unwrap(),topology.previous(id).unwrap()]){*out=Vec3::from_array(topology.vertex(id).ok_or("missing compact vertex")?);}}
    Ok(Feature{kind,edge:edge_key(id),opposite:edge_key(opposite),triangle:base+(id.triangle as u64+1)*16,ledge:base,vertex:edge.vertex_index(),vertices})
}
#[derive(Clone,Debug,PartialEq)]
pub(crate) struct Endpoint {pub object:u64,pub feature:Feature}
#[derive(Clone,Debug,PartialEq)]
pub(crate) struct Candidate {pub key:u64,pub side:usize,pub endpoints:[Endpoint;2]}
#[derive(Clone)]
pub(crate) struct Input {pub flags:u32,pub time:f64,pub endpoints:[Endpoint;2],pub lists:[Vec<Candidate>;2]}
#[derive(Clone,Debug,PartialEq)]
pub(crate) struct Initial {
    pub time:f64,pub mix:[f32;2],pub work:f32,pub force:f32,pub energy:f32,pub distance:f32,pub area:Option<f32>,
    pub point:Vec3,pub normal_z:f32,pub marker:u8,pub age:u8,pub mode:u8,pub counter:u8,pub sign:u16,pub extra:u32,
}
pub(crate) struct Creation {pub key:Option<u64>,pub created:bool,pub endpoints:Option<[Endpoint;2]>,pub initial:Option<Initial>,pub lists:[Vec<Candidate>;2]}
pub(crate) fn same_feature(a:&Feature,b:&Feature)->Result<bool,&'static str> {
    if a.kind!=b.kind {return Ok(false);}
    Ok(match a.kind {0=>a.ledge==b.ledge&&a.vertex==b.vertex,1=>a.edge==b.edge||a.opposite==b.edge,2=>a.triangle==b.triangle,3=>true,_=>return Err("unsupported compact feature kind")})
}
pub(crate) fn same_contact(c:&Candidate,query:&[Endpoint;2])->Result<bool,&'static str> {
    for order in [[0,1],[1,0]] {
        let [a,b]=order;
        if c.endpoints[a].object==query[0].object&&c.endpoints[b].object==query[1].object&&same_feature(&c.endpoints[a].feature,&query[0].feature)?&&same_feature(&c.endpoints[b].feature,&query[1].feature)? {return Ok(true);}
    }
    Ok(false)
}
pub(crate) fn lookup(input:&Input)->Result<Option<u64>,&'static str> {
    let other=input.endpoints[1].object;
    for c in &input.lists[0] {
        if (c.endpoints[0].object==other||c.endpoints[1].object==other)&&same_contact(c,&input.endpoints)? {return Ok(Some(c.key));}
    }
    Ok(None)
}
pub(crate) fn triangle_area(vertices:[Vec3;3],epsilon:f64)->(Vec3,f64,f32) {
    let cross=(vertices[1]-vertices[0]).cross(vertices[2]-vertices[0]);
    let length=((cross.y as f64*cross.y as f64+cross.x as f64*cross.x as f64)+cross.z as f64*cross.z as f64).sqrt();
    (cross,length,(1.0/(length+epsilon)) as f32)
}
/// `allocation` is an opaque fresh identity supplied by the caller. Passing
/// None represents allocation failure, a branch not observed in owned drops.
/// Unwritten constructor bytes (including non-face area) are not zero-filled.
pub(crate) fn resolve(input:&Input,allocation:Option<u64>,distance:f32,epsilon:f64)->Result<Creation,&'static str> {
    let mut out=Creation {key:None,created:false,endpoints:None,initial:None,lists:input.lists.clone()};
    if input.flags&0x3c0000!=0xc0000 {return Ok(out);}
    if let Some(key)=lookup(input)? {out.key=Some(key);return Ok(out);}
    out.created=true;let Some(key)=allocation else{return Ok(out);};
    if key==0||out.lists.iter().flatten().any(|c|c.key==key) {return Err("allocator identity is not fresh");}
    let first=((input.flags>>8)&3) as usize;if first>1 {return Err("unsupported mindist endpoint selector");}
    let endpoints=[input.endpoints[first].clone(),input.endpoints[first^1].clone()];
    for side in 0..2 {
        for slot in 0..2 {
            if input.endpoints[slot].object==endpoints[side].object {out.lists[slot].insert(0,Candidate {key,side,endpoints:endpoints.clone()});}
        }
    }
    let area=if endpoints[1].feature.kind==2 {Some(triangle_area(endpoints[1].feature.vertices,epsilon).2)}else{None};
    out.initial=Some(Initial {time:input.time,mix:[0.;2],work:0.,force:0.,energy:0.,distance,area,point:Vec3::ZERO,normal_z:0.,marker:1,age:20,mode:0,counter:0,sign:0,extra:0});
    out.key=Some(key);out.endpoints=Some(endpoints);Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn actual_bsp_compact_topology_and_feature_identities_match_native(){
        use l4d2_formats::phy::{CompactEdge,CompactEdgeId};
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");if !root.exists(){return;}
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-bsp-topology.json")).unwrap();
        let mut edge_count=0;let mut identity_count=0;
        for case in data["maps"].as_array().unwrap(){
            let name=case["map"].as_str().unwrap();let bytes=std::fs::read(root.join("left4dead2/maps").join(format!("{name}.bsp"))).unwrap();
            let bsp=l4d2_formats::bsp::Bsp::parse(bytes).unwrap();let models=bsp.physics_models().unwrap();let solids=models.iter().find(|m|m.model_index==0).unwrap().geometry().unwrap();
            let mut packet=Vec::new();packet.extend((solids.len() as u32).to_le_bytes());let mut observations=Vec::new();let mut ledge_key=1;
            for solid in &solids{packet.extend((solid.convexes.len() as u32).to_le_bytes());let mut crcs=Vec::new();
                for convex in &solid.convexes{let topology=convex.topology.as_ref().unwrap();let mut body=Vec::new();body.extend((topology.edges.len() as u32).to_le_bytes());
                    let byte_offset=|id:CompactEdgeId|((id.triangle+1)*16+(id.edge+1)*4) as u32;
                    for (t,row) in topology.edges.iter().enumerate(){body.extend(convex.triangle_headers[t].0.to_le_bytes());
                        for (e,edge) in row.iter().enumerate(){let id=CompactEdgeId{triangle:t,edge:e};let next=topology.next(id).unwrap();let previous=topology.previous(id).unwrap();let opposite=topology.opposite(id).unwrap();
                            assert_eq!(topology.opposite(opposite),Some(id));
                            let point=from_compact(0,ledge_key,convex,id).unwrap();let line=from_compact(1,ledge_key,convex,id).unwrap();let face=from_compact(2,ledge_key,convex,id).unwrap();
                            let mask=(same_feature(&point,&from_compact(0,ledge_key,convex,topology.next(opposite).unwrap()).unwrap()).unwrap() as u32)
                                |((same_feature(&line,&line).unwrap() as u32)<<1)
                                |((same_feature(&line,&from_compact(1,ledge_key,convex,opposite).unwrap()).unwrap() as u32)<<2)
                                |((same_feature(&line,&from_compact(1,ledge_key,convex,next).unwrap()).unwrap() as u32)<<3)
                                |((same_feature(&face,&from_compact(2,ledge_key,convex,next).unwrap()).unwrap() as u32)<<4)
                                |((same_feature(&face,&from_compact(2,ledge_key,convex,opposite).unwrap()).unwrap() as u32)<<5);
                            assert_eq!(face.vertices,[id,next,previous].map(|i|Vec3::from_array(topology.vertex(i).unwrap())));
                            for value in [edge.0,byte_offset(opposite),byte_offset(next),byte_offset(previous),mask]{body.extend(value.to_le_bytes());}
                            for value in topology.vertex(id).unwrap(){body.extend(value.to_bits().to_le_bytes());}
                            edge_count+=1;identity_count+=6;
                        }
                    }crcs.push(crc32fast::hash(&body));packet.extend(body);ledge_key+=1;
                }observations.push(crcs);
            }
            let controls=case["solids"][0][0]["edge_controls"].as_array().unwrap();packet.extend((controls.len() as u32).to_le_bytes());
            for control in controls{let word=control["word"].as_u64().unwrap() as u32;let expected=control["offset"].as_i64().unwrap() as i32;let offset=CompactEdge(word).opposite_offset();
                assert_eq!(offset,expected);assert!(control["unchanged"].as_bool().unwrap());
                let equal=offset==expected;let neighbor=expected+1==0||offset==expected+1;
                assert_eq!(equal,control["native_equal"].as_bool().unwrap());assert_eq!(neighbor,control["native_neighbor_equal"].as_bool().unwrap());
                for value in [word,offset as u32,equal as u32,neighbor as u32]{packet.extend(value.to_le_bytes());}
            }
            let source=std::path::Path::new(file!());
            let workspace=if source.is_absolute(){source.parent().unwrap().parent().unwrap()}else{std::path::Path::new(env!("CARGO_MANIFEST_DIR"))};
            let output=workspace.join("local-research/parity");std::fs::create_dir_all(&output).unwrap();
            std::fs::write(output.join(format!("ivp-bsp-topology-1024-source-{name}.packet")),&packet).unwrap();
            assert_eq!(packet.len() as u64,case["packet_bytes"].as_u64().unwrap());
            assert_eq!(observations.len(),case["solids"].as_array().unwrap().len());
            for (s,(actual,expected)) in observations.iter().zip(case["solids"].as_array().unwrap()).enumerate(){
                assert_eq!(actual.len(),expected.as_array().unwrap().len());for (c,(crc,native)) in actual.iter().zip(expected.as_array().unwrap()).enumerate(){assert_eq!(*crc as u64,native["crc32"].as_u64().unwrap(),"{name} solid{s} ledge{c}");}
            }
        }assert_eq!(edge_count,278556);assert_eq!(identity_count,1671336);
    }
    #[test]
    fn compact_topology_rejects_invalid_links_and_unowned_feature_namespaces(){
        use l4d2_formats::phy::{CompactTopology,CompactEdge,CompactEdgeId,Convex};
        let id=CompactEdgeId{triangle:0,edge:0};
        let topology=CompactTopology{edges:vec![[CompactEdge(0x7fff0000);3]],vertices_ivp:Default::default()};
        assert!(topology.opposite(id).is_none());assert!(topology.opposite(CompactEdgeId{triangle:usize::MAX,edge:usize::MAX}).is_none());
        let convex=Convex{triangles:vec![],triangle_headers:vec![],topology:None,planes:vec![],edges:vec![],bevels:vec![],mins:Default::default(),maxs:Default::default()};
        assert!(from_compact(0,1,&convex,id).is_err());assert!(from_compact(0,0,&convex,id).is_err());assert!(from_compact(3,1,&convex,id).is_err());
    }
    use super::*;
    use serde_json::Value;
    fn fixture()->Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-create.json")).unwrap()}
    fn f(v:&Value)->f32 {v.as_f64().unwrap() as f32}
    fn v(v:&Value)->Vec3 {Vec3::from_array(std::array::from_fn(|i|f(&v[i])))}
    fn feature(c:&Value)->Feature {Feature {kind:c["kind"].as_i64().unwrap() as i16,edge:c["edge"].as_u64().unwrap(),opposite:c["opposite"].as_u64().unwrap(),triangle:c["triangle"].as_u64().unwrap(),ledge:c["ledge"].as_u64().unwrap(),vertex:c["vertex"].as_u64().unwrap() as u16,vertices:std::array::from_fn(|i|v(&c["vertices"][i]))}}
    fn endpoint(c:&Value)->Endpoint {Endpoint {object:c["object"].as_u64().unwrap(),feature:feature(&c["feature"])}}
    fn input(c:&Value)->Input {Input {flags:c["flags"].as_u64().unwrap() as u32,time:c["time"].as_f64().unwrap(),endpoints:std::array::from_fn(|i|endpoint(&c["endpoints"][i])),lists:std::array::from_fn(|i|c["lists"][i].as_array().unwrap().iter().map(|a|Candidate {key:a["key"].as_u64().unwrap(),side:a["side"].as_u64().unwrap() as usize,endpoints:std::array::from_fn(|i|endpoint(&a["endpoints"][i]))}).collect())}}
    fn initial(c:&Value,face:bool)->Initial {Initial {time:c["time"].as_f64().unwrap(),mix:std::array::from_fn(|i|f(&c["mix"][i])),work:f(&c["work"]),force:f(&c["force"]),energy:f(&c["energy"]),distance:f(&c["distance"]),area:face.then(||f(&c["area"])),point:v(&c["point"]),normal_z:f(&c["normal_z"]),marker:c["marker"].as_u64().unwrap() as u8,age:c["age"].as_u64().unwrap() as u8,mode:c["mode"].as_u64().unwrap() as u8,counter:c["counter"].as_u64().unwrap() as u8,sign:c["sign"].as_u64().unwrap() as u16,extra:c["extra"].as_u64().unwrap() as u32}}
    #[test]
    fn native_geometric_lookup_recognizes_equivalent_features_and_object_order() {
        let data=fixture();let mut counts=[0;2];
        for c in data["lookups"].as_array().unwrap() {
            let got=lookup(&input(&c["before"])).unwrap();let key=c["result"].as_u64().unwrap();assert_eq!(got,(key!=0).then_some(key),"step{}",c["step"]);counts[got.is_some() as usize]+=1;
        }
        assert_eq!(counts,[118,56]);
    }
    #[test]
    fn native_selection_constructor_state_and_both_list_heads_match() {
        let data=fixture();let mut counts=[0;3];
        for c in data["selects"].as_array().unwrap() {
            let before=input(&c["before"]);let fresh=before.lists.iter().flatten().map(|c|c.key).max().unwrap_or(0)+1;
            let got=resolve(&before,Some(fresh),f(&c["constants"]["distance"]),c["constants"]["area_epsilon"].as_f64().unwrap()).unwrap();
            assert_eq!(got.created,c["created"].as_u64().unwrap()!=0);let native=c["result"].as_u64().unwrap();
            if got.created {
                assert_eq!(got.key,Some(fresh));let endpoints=std::array::from_fn(|i|endpoint(&c["endpoints"][i]));assert_eq!(got.endpoints.as_ref(),Some(&endpoints));
                assert_eq!(got.initial.unwrap(),initial(&c["initial"],endpoints[1].feature.kind==2));counts[0]+=1;
            }else{assert_eq!(got.key,(native!=0).then_some(native));assert!(got.initial.is_none());counts[if native!=0 {1}else{2}]+=1;}
            for side in 0..2 {
                let expected=c["after_lists"][side].as_array().unwrap().iter().map(|r|{let key=r[0].as_u64().unwrap();(if got.created&&key==native {fresh}else{key},r[1].as_u64().unwrap() as usize)}).collect::<Vec<_>>();
                assert_eq!(got.lists[side].iter().map(|a|(a.key,a.side)).collect::<Vec<_>>(),expected,"list step{}",c["step"]);
            }
        }
        assert_eq!(counts,[82,28,64]);
    }
    #[test]
    fn native_triangle_cross_length_and_constructor_area_controls_match() {
        let data=fixture();let mut count=0;
        for c in data["controls"].as_array().unwrap() {
            let vertices=std::array::from_fn(|i|v(&c["area_vertices"][i]));let (cross,length,area)=triangle_area(vertices,c["constants"]["area_epsilon"].as_f64().unwrap());
            assert_eq!(cross,v(&c["native_cross"]));assert_eq!(length,c["native_length"].as_f64().unwrap(),"length kind{}",c["kind"]);assert_eq!(area,f(&c["inverse_area"]));count+=1;
        }
        assert_eq!(count,64);
    }
    #[test]
    fn consecutive_insertions_reuse_source_generated_object_lists() {
        use std::collections::HashMap;
        let data=fixture();let mut scenarios=HashMap::new();let mut reused=0;
        for c in data["selects"].as_array().unwrap().iter().filter(|c|c["step"].as_i64().unwrap()>=0) {
            let scenario=(c["capture"].as_u64().unwrap(),c["model"].as_str().unwrap().to_owned());
            let (cache,identities,next)=scenarios.entry(scenario).or_insert_with(||(HashMap::<u64,Vec<Candidate>>::new(),HashMap::<u64,u64>::new(),1_000_000u64));
            let mut before=input(&c["before"]);
            for list in &mut before.lists {for row in list {row.key=*identities.get(&row.key).unwrap_or(&row.key);}}
            if (0..2).all(|i|cache.get(&before.endpoints[i].object)==Some(&before.lists[i])) {
                before.lists=std::array::from_fn(|i|cache[&before.endpoints[i].object].clone());reused+=1;
            }
            let fresh=*next;*next+=1;
            let got=resolve(&before,Some(fresh),f(&c["constants"]["distance"]),c["constants"]["area_epsilon"].as_f64().unwrap()).unwrap();
            assert!(got.created);identities.insert(c["result"].as_u64().unwrap(),fresh);
            for side in 0..2 {
                let expected=c["after_lists"][side].as_array().unwrap().iter().map(|v|{let key=v[0].as_u64().unwrap();(*identities.get(&key).unwrap_or(&key),v[1].as_u64().unwrap() as usize)}).collect::<Vec<_>>();
                assert_eq!(got.lists[side].iter().map(|v|(v.key,v.side)).collect::<Vec<_>>(),expected);
                cache.insert(before.endpoints[side].object,got.lists[side].clone());
            }
        }
        assert_eq!(reused,45);
    }
}
