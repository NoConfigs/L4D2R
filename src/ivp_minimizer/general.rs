//! Native1010df90 ordered triangle-feature fallback before ordinary descent.
use super::*;
//100f4240 ->100ecaf0/100ecb80. X is spilled before the plane distance;
// normalization retains its reciprocal for D but spills a copy for XYZ.
pub(super) fn plane(h:&CompactTopology,e:CompactEdgeId)->Result<[f32;4],&'static str>{
    let a=vertex(h,e)?;let b=vertex(h,previous(h,e)?)?;let c=vertex(h,next(h,e)?)?;
    let u=[b.x as f64-a.x as f64,b.y as f64-a.y as f64,b.z as f64-a.z as f64];
    let v=[c.x as f64-a.x as f64,c.y as f64-a.y as f64,c.z as f64-a.z as f64];
    let x=v[1]*u[2]-v[2]*u[1];let y=v[2]*u[0]-v[0]*u[2];let z=v[0]*u[1]-v[1]*u[0];
    let raw=[x as f32,y as f32,z as f32];
    let d=-((a.x as f64*raw[0] as f64+a.y as f64*y)+a.z as f64*z) as f32;
    let square=(raw[0] as f64*raw[0] as f64+raw[1] as f64*raw[1] as f64)+raw[2] as f64*raw[2] as f64;
    let inverse=1./(square.sqrt()+1e-15f32 as f64);let inv=inverse as f32;
    let result=[raw[0]*inv,raw[1]*inv,raw[2]*inv,(d as f64*inverse) as f32];
    if result.iter().any(|v|!v.is_finite()){return Err("native general plane overflow");}Ok(result)
}
pub(super) fn select(ids:[CompactEdgeId;2],h:[&CompactTopology;2],f:[PreparedObjectCache;2])->Result<Route,&'static str>{
    let mut edges=[[ids[0];3],[ids[1];3]];
    for side in 0..2{for i in 1..3{edges[side][i]=next(h[side],edges[side][i-1])?;}}
    let local=|side:usize,e:CompactEdgeId|->Result<Vec3,&'static str>{Ok(inverse(f[1-side].pose.rotation,f[side].pose.transform_point(vertex(h[side],e)?)-f[1-side].pose.position))};
    let mut best=1e20f32;let mut selected=None;
    for a in edges[0]{for b in edges[1]{
        let delta=f[0].pose.transform_point(vertex(h[0],a)?)-f[1].pose.transform_point(vertex(h[1],b)?);
        let square=(delta.y*delta.y+delta.x*delta.x)+delta.z*delta.z;
        if square<best{best=square;selected=Some(Route::Pair([0,1],[a,b]));}
    }}
    let bias=f32::from_bits(1065353224);
    for side in 0..2{
        let other=1-side;let p=plane(h[other],ids[other])?;
        let canonical=std::array::from_fn::<_,3,_>(|edge|vertex(h[other],CompactEdgeId{triangle:ids[other].triangle,edge}));
        let canonical=[canonical[0]?,canonical[1]?,canonical[2]?];
        for own in edges[side]{
            let point=local(side,own)?;
            if inside(point,canonical){
                let signed=((point.y*p[1]+point.x*p[0])+point.z*p[2])+p[3];let square=signed*signed;
                if square*bias<best{best=square;selected=Some(Route::Face([side,other],[own,ids[other]]));}
            }
        }
    }
    for edge_side in 0..2{
        let point_side=1-edge_side;
        for point_edge in edges[point_side]{
            let point=local(point_side,point_edge)?;
            for e in edges[edge_side]{
                let c=coordinates(h[edge_side],e,point)?;
                if c.iter().all(|v|!v.is_sign_negative()){
                    let distance=segment_distance_squared(h[edge_side],e,point)?;
                    if (distance*bias as f64)<best as f64{best=distance as f32;selected=Some(Route::Edge([point_side,edge_side],[point_edge,e]));}
                }
            }
        }
    }
    for a in edges[0]{for b in edges[1]{
        let setup=edge_pair::Setup::new([a,b],h,f)?;let c=setup.full_coefficients(f)?;
        if c.iter().all(|v|!v.is_sign_negative()){
            let distance=setup.line_distance(h[1],b)?;
            if (distance*bias as f64)<best as f64{best=distance as f32;selected=Some(Route::Edges([0,1],[a,b]));}
        }
    }}
    selected.ok_or("native general feature selection produced no finite candidate")
}
#[cfg(test)]
mod tests{
    use super::*;
    use l4d2_formats::phy::CompactEdge;
    #[test]
    fn native_general_selection_planes_and_complete_ordinary_descents(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-minimizer-general.json")).unwrap();
        assert_eq!(data["cases"].as_array().unwrap().len(),3456);
        for (n,c) in data["cases"].as_array().unwrap().iter().enumerate(){
            let word=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let float=|v:&serde_json::Value|f32::from_bits(word(v));
            let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{
                edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(word(&h[5+t*4+e])))).collect(),
                vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|float(&h[20+i*4+j])))).collect(),
            }).collect();
            let frames:[PreparedObjectCache;2]=std::array::from_fn(|i|{
                let f=&c["frames"][i];let core_position=Vec3::new(float(&f[0]),float(&f[1]),float(&f[2]));let rotation=Mat3::from_cols_array(&std::array::from_fn(|j|float(&f[8+(j%3)*4+j/3])));
                PreparedObjectCache{core_position,pose:crate::ivp_motion::Pose{position:Vec3::new(float(&f[20]),float(&f[21]),float(&f[22])),rotation},quaternion:[0.,0.,0.,1.],frame_stamp:1}
            });
            let physical:[CompactEdgeId;2]=std::array::from_fn(|i|CompactEdgeId{triangle:c["initial_edges"][i][0].as_u64().unwrap() as usize,edge:c["initial_edges"][i][1].as_u64().unwrap() as usize});
            for i in 0..2{assert_eq!(plane(&hulls[i],physical[i]).unwrap().map(f32::to_bits),std::array::from_fn(|j|word(&c["planes"][i][j])),"case{n} plane{i}");}
            let reverse=c["reverse"].as_u64().unwrap() as usize;let order=[reverse,1-reverse];let h=order.map(|i|&hulls[i]);let f=order.map(|i|frames[i]);let initial=order.map(|i|physical[i]);
            let selected=select(initial,h,f).unwrap();
            let (rva,roles,ids)=match selected{Route::Pair(r,i)=>(0x121ea0,r,i),Route::Edge(r,i)=>(0x1242a0,r,i),Route::Face(r,i)=>(0x123c40,r,i),Route::Edges(r,i)=>(0x1238f0,r,i)};
            let expected=&c["routes"][0];assert_eq!(rva,word(&expected["rva"]),"case{n} selected kind");
            assert_eq!(roles.map(|i|order[i]),std::array::from_fn(|i|expected["roles"][i].as_u64().unwrap() as usize),"case{n} selected roles");
            for i in 0..2{assert_eq!([ids[i].triangle,ids[i].edge],[expected["ids"][i][0].as_u64().unwrap() as usize,expected["ids"][i][1].as_u64().unwrap() as usize],"case{n} selected endpoint{i}");}
            // Expected selected routes above are assertions, never fed into
            // the production query. It starts from the authored face pair.
            let mut query=Query{retired:false,order:0,recovery:None,frame:0,kinds:[2,2],vertex:initial[0],face:initial[1],geometry:None};
            assert!(query.refresh(1,h[0],h[1],f,0.02).unwrap_or_else(|error|panic!("case{n}: {error}")));
            let ids=[query.vertex,query.face];for i in 0..2{let e=&c["features"][order[i]];assert_eq!([ids[i].triangle,ids[i].edge,query.kinds[i] as usize],[e[0].as_u64().unwrap() as usize,e[1].as_u64().unwrap() as usize,e[2].as_u64().unwrap() as usize],"case{n} final endpoint{i}");}
            let g=query.geometry.unwrap();let e=&c["geometry"];assert_eq!([g.projection.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()],[word(&e[0]),word(&e[2]),word(&e[3]),word(&e[4]),word(&e[5])],"case{n} geometry");
            assert!(!query.refresh(1,h[0],h[1],f,0.02).unwrap());
        }
    }
}
