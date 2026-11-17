//! Native100f3e10/100f4f60 edge coordinates and101238f0 routing.
//! Penetrating/cycling feature recovery remains a separate gap.
use super::*;
pub(super) struct Setup{pub local_a:[Vec3;2],pub a:[Vec3;2],pub b:[Vec3;2],pub axis_a:Vec3,pub axis_b:Vec3,pub cross:Vec3}
impl Setup{
    pub(super) fn new(ids:[CompactEdgeId;2],hulls:[&CompactTopology;2],frames:[PreparedObjectCache;2])->Result<Self,&'static str>{
        let poses=frames.map(|f|f.pose);
        let va=[vertex(hulls[0],ids[0])?,vertex(hulls[0],next(hulls[0],ids[0])?)?];
        let b=[vertex(hulls[1],ids[1])?,vertex(hulls[1],next(hulls[1],ids[1])?)?];
        let a=va.map(|p|inverse(poses[1].rotation,poses[0].transform_point(p)-poses[1].position));
        let axis_a=inverse(poses[1].rotation,rotate(poses[0].rotation,va[1]-va[0]));let axis_b=b[1]-b[0];
        Ok(Self{local_a:va,a,b,axis_a,axis_b,cross:axis_a.cross(axis_b)})
    }
    pub(super) fn coefficients(&self)->Option<[f32;4]>{
        if (self.cross.x*self.cross.x+self.cross.y*self.cross.y)+self.cross.z*self.cross.z<=1e-6{return None;}
        let na=self.axis_a.cross(self.cross);let nb=self.axis_b.cross(self.cross);
        let [b0,b1]=self.b.map(|p|dot(p,na));let db=b0-b1;
        let [a0,a1]=self.a.map(|p|dot(p,nb));let b=dot(self.b[0],nb);let da=a0-a1;
        Some([(a0-b)*da+f32::MIN_POSITIVE,(b-a1)*da+f32::MIN_POSITIVE,(b0-dot(self.a[0],na))*db,(dot(self.a[0],na)-b1)*db])
    }
    pub(super) fn full_coefficients(&self,f:[PreparedObjectCache;2])->Result<[f32;4],&'static str>{
        if let Some(c)=self.coefficients(){return Ok(c);}
        //100f4f60 tests eleven samples on A, then only the first nine on B.
        // A strict improvement retains the first equal-distance sample.
        const SAMPLES:[f32;11]=[-1.,0.5,2.,0.,1.,-0.001,0.001,0.999,1.001,-1e-6,1e-6];
        let lerp=|[a,b]:[Vec3;2],t:f32|{
            let u=1.-t;Vec3::new(a.x*u+b.x*t,a.y*u+b.y*t,b.z*t+a.z*u)
        };
        let line=|[a,b]:[Vec3;2],p:Vec3|{
            let d=b-a;let reverse=a-b;let cross=(p-a).cross(d);
            let denominator=((reverse.x*reverse.x+reverse.y*reverse.y)+reverse.z*reverse.z)+1e-18;
            (1./denominator)*((cross.x*cross.x+cross.y*cross.y)+cross.z*cross.z)
        };
        let coordinate=|[a,b]:[Vec3;2],p:Vec3|{
            let d=b-a;
            [-(((a.x-p.x)*d.x+(a.y-p.y)*d.y)+(a.z-p.z)*d.z)+f32::MIN_POSITIVE,
             (((b.x-p.x)*d.x+(b.y-p.y)*d.y)+(b.z-p.z)*d.z)+f32::MIN_POSITIVE]
        };
        let mut best=1e20f32;let mut result=None;
        for t in SAMPLES{
            let p=lerp(self.a,t);let distance=line(self.b,p);
            if distance<best{let c=coordinate(self.b,p);result=Some([t,1.-t,c[0],c[1]]);best=distance;}
        }
        let b_in_a=self.b.map(|p|inverse(f[0].pose.rotation,f[1].pose.transform_point(p)-f[0].pose.position));
        for t in SAMPLES.into_iter().take(9){
            let p=lerp(b_in_a,t);let distance=line(self.local_a,p);
            if distance<best{let c=coordinate(self.local_a,p);result=Some([c[0],c[1],t,1.-t]);best=distance;}
        }
        result.ok_or("native parallel sampling produced no finite improvement")
    }
    pub(super) fn line_distance(&self,h:&CompactTopology,e:CompactEdgeId)->Result<f64,&'static str>{
        let square=(self.cross.x*self.cross.x+self.cross.y*self.cross.y)+self.cross.z*self.cross.z;
        if square<=1e-12{return segment_distance_squared(h,e,self.a[0]);}
        let separation=dot(self.a[0],self.cross)-dot(self.b[0],self.cross);
        Ok((separation as f64*separation as f64)/square as f64)
    }
}
fn negative(values:[f32;2])->bool{values.iter().any(|x|x.is_sign_negative())}
pub(super) enum Outcome{Retired,Route(Route),Terminal(Geometry),Penetrating{geometry:Geometry,kinds:[i16;2],owner:usize,point:Vec3}}
//100f59d0, including its float best-so-far for the four-endpoint case.
pub(super) fn distance(ids:[CompactEdgeId;2],h:[&CompactTopology;2],f:[PreparedObjectCache;2])->Result<f64,&'static str>{
    let s=Setup::new(ids,h,f)?;let c=s.full_coefficients(f)?;
    let b_in_a=|i:usize|inverse(f[0].pose.rotation,f[1].pose.transform_point(s.b[i])-f[0].pose.position);
    if negative([c[2],c[3]]){
        if !negative([c[0],c[1]]){
            if c[2]<=0.{return segment_distance_squared(h[0],ids[0],b_in_a(0));}
            if c[3]<=0.{return segment_distance_squared(h[0],ids[0],b_in_a(1));}
            return Err("invalid native segment distance coordinates");
        }
        let mut best=1e20f32;
        for i in 0..2{
            let a=segment_distance_squared(h[0],ids[0],b_in_a(i))?;
            if a<best as f64{best=a as f32;}
            let b=segment_distance_squared(h[1],ids[1],s.a[i])?;
            if b<best as f64{best=b as f32;}
        }
        return Ok(best as f64);
    }
    if c[0]<=0.{return segment_distance_squared(h[1],ids[1],s.a[0]);}
    if c[1]<=0.{return segment_distance_squared(h[1],ids[1],s.a[1]);}
    s.line_distance(h[1],ids[1])
}
pub(super) fn descend(roles:[usize;2],ids:[CompactEdgeId;2],hulls:[&CompactTopology;2],frames:[PreparedObjectCache;2],margin:f32,query:&mut Query,descent:&mut Descent)->Result<Outcome,&'static str>{
    let h=roles.map(|i|hulls[i]);let f=roles.map(|i|frames[i]);let setup=Setup::new(ids,h,f)?;
    let c=setup.full_coefficients(f)?;
    let edge=|point_side:usize,p:CompactEdgeId|Outcome::Route(Route::Edge([roles[point_side],roles[1-point_side]],[p,ids[1-point_side]]));
    if negative([c[2],c[3]]){
        if negative([c[0],c[1]]){
            let ends=std::array::from_fn::<_,2,_>(|i|if c[i*2+1]<=c[i*2]{next(h[i],ids[i])}else{Ok(ids[i])});let ends=[ends[0]?,ends[1]?];
            let local=|side:usize,p:CompactEdgeId|->Result<Vec3,&'static str>{Ok(inverse(f[1-side].pose.rotation,f[side].pose.transform_point(vertex(h[side],p)?)-f[1-side].pose.position))};
            let ab=coordinates(h[1],ids[1],local(0,ends[0])?)?;
            if negative(ab){
                let ba=coordinates(h[0],ids[0],local(1,ends[1])?)?;
                if negative(ba){
                    //101238f0 retains the far B endpoint until the first
                    // product test succeeds. The near-endpoint assignment
                    // is inside that short-circuited condition.
                    let b=if ab[0]*c[2]>=0.{
                        if ba[0]*c[0]<0.{
                            let far_a=if ends[0]==ids[0]{next(h[0],ids[0])?}else{ids[0]};
                            return Ok(Outcome::Route(Route::Pair([roles[1],roles[0]],[ends[1],far_a])));
                        }
                        ends[1]
                    }else if ends[1]==ids[1]{next(h[1],ids[1])?}else{ids[1]};
                    return Ok(Outcome::Route(Route::Pair(roles,[ends[0],b])));
                }
                return Ok(edge(1,ends[1]));
            }
            return Ok(edge(0,ends[0]));
        }
        return Ok(edge(1,if c[2]>=0.{next(h[1],ids[1])?}else{ids[1]}));
    }
    if c[0]<0.{return Ok(edge(0,ids[0]));}
    if c[1]<0.{return Ok(edge(0,next(h[0],ids[0])?));}
    interior(roles,ids,h,f,&setup,c,margin,query,descent)
}
fn normal(h:&CompactTopology,e:CompactEdgeId)->Result<Vec3,&'static str>{let a=vertex(h,e)?;Ok((vertex(h,next(h,e)?)?-a).cross(vertex(h,previous(h,e)?)?-a))}
fn interior(roles:[usize;2],ids:[CompactEdgeId;2],h:[&CompactTopology;2],f:[PreparedObjectCache;2],s:&Setup,c:[f32;4],margin:f32,query:&mut Query,descent:&mut Descent)->Result<Outcome,&'static str>{
    if !descent.enter(roles,ids,[1,1]){return Ok(Outcome::Retired);}
    let signed=(s.cross.x*(s.b[0].x-s.a[0].x)+s.cross.y*(s.b[0].y-s.a[0].y))+s.cross.z*(s.b[0].z-s.a[0].z);
    let sign=usize::from(signed.is_sign_negative());
    let world=rotate(f[1].pose.rotation,s.cross);let in_a=inverse(f[0].pose.rotation,world);
    let square=(s.cross.y as f64*s.cross.y as f64+s.cross.x as f64*s.cross.x as f64)+s.cross.z as f64*s.cross.z as f64;
    let reciprocal=1./square.sqrt();let factor=((sign as f32-0.5)*2.)*(reciprocal as f32);
    let normal_world=world*factor;
    let geometry=Geometry{distance:((reciprocal*signed as f64).abs() as f32)-margin,projection:dot(normal_world,f[0].core_position-f[1].core_position),normal:normal_world};
    query.geometry=Some(geometry);
    let b_in_a=s.b.map(|p|inverse(f[0].pose.rotation,f[1].pose.transform_point(p)-f[0].pose.position));
    let opposite=[across(h[0],ids[0])?,across(h[1],ids[1])?];
    // Four adjacent faces, in101229a0's original order. Each record retains
    // the opposite object's endpoint/feature and its own local normal.
    let face=[opposite[0],ids[0],opposite[1],ids[1]];
    let point_edge=[ids[1],opposite[1],ids[0],opposite[0]];
    let point=[b_in_a[0],b_in_a[1],s.a[0],s.a[1]];
    let cross=[in_a,in_a,s.cross,s.cross];
    let normals=std::array::from_fn::<_,4,_>(|i|normal(h[i/2],face[i]));
    let normals=[normals[0]?,normals[1]?,normals[2]?,normals[3]?];
    let flags=std::array::from_fn::<_,4,_>(|i|usize::from(dot(cross[i],normals[i]).is_sign_negative())^(i/2)^sign);
    let mut best=-4e-6f32;let mut selected=None;
    for i in 0..4{
        let delta=point[i^sign]-point[i^sign^1];let n=normals[i];let score=dot(n,delta);
        if score<0.{
            let n_square=(n.z*n.z+n.y*n.y)+n.x*n.x;
            let inv_n=(1./(n_square as f64).sqrt()) as f32;
            let d_square=(delta.y*delta.y+delta.x*delta.x)+delta.z*delta.z;
            let normalized=((1./(d_square as f64).sqrt())*score as f64)*inv_n as f64;
            let index=flags[i]^i^sign;
            if normalized<best as f64{
                let weights=[opposite_weight(h[i/2],face[i],point[index])?,opposite_weight(h[i/2],next(h[i/2],face[i])?,point[index])?,opposite_weight(h[i/2],previous(h[i/2],face[i])?,point[index])?];
                if weights[0]>0.{best=normalized as f32;selected=Some((i,index,weights));}
            }
        }
    }
    if let Some((i,index,weights))=selected{
        let local_roles=[roles[1-i/2],roles[i/2]];let local_ids=[point_edge[index],face[i]];
        if weights.iter().any(|w|w.is_sign_negative()){
            let owner=i/2;let next_face=next(h[owner],face[i])?;let previous_face=previous(h[owner],face[i])?;
            let selected=if weights[2]<0.{
                if weights[1]<0.{
                    let local_h=[h[1-owner],h[owner]];let local_f=[f[1-owner],f[owner]];
                    let a=distance([point_edge[index],next_face],local_h,local_f)? as f32;
                    let b=distance([point_edge[index],across(h[owner],previous_face)?],local_h,local_f)?;
                    if a as f64<=b{next_face}else{previous_face}
                }else{previous_face}
            }else{next_face};
            return Ok(Outcome::Route(Route::Edges(local_roles,[point_edge[index],selected])));
        }
        return Ok(Outcome::Route(Route::Face(local_roles,local_ids)));
    }
    let lerp=|[a,b]:[Vec3;2],t:f32|{let u=1.-t;Vec3::new(a.x*u+b.x*t,a.y*u+b.y*t,b.z*t+a.z*u)};
    if flags[0]+flags[1]==2{return Ok(Outcome::Penetrating{geometry,kinds:[5,2],owner:0,point:lerp(b_in_a,c[2]/(c[3]+c[2]))});}
    if flags[2]+flags[3]==2{return Ok(Outcome::Penetrating{geometry,kinds:[2,5],owner:1,point:lerp(s.a,c[0]/(c[1]+c[0]))});}
    Ok(Outcome::Terminal(geometry))
}

#[cfg(test)]
mod tests{
    use super::*;
    use l4d2_formats::phy::CompactEdge;
    #[test]
    fn native_nonparallel_edge_pair_setup_routing_and_geometry(){
        check(include_str!("../../tools/fixtures/native-ivp-minimizer-edge-edge.json"),false);
    }
    #[test]
    fn native_parallel_sampling_coordinates_distances_and_ordinary_queries(){
        check(include_str!("../../tools/fixtures/native-ivp-minimizer-parallel-sampling.json"),true);
    }
    fn check(raw:&str,parallel:bool){
        let data:serde_json::Value=serde_json::from_str(raw).unwrap();
        assert_eq!(data["cases"].as_array().unwrap().len(),if parallel{1152}else{384});
        let mut ordinary=0;let mut penetration=0;
        for (n,c) in data["cases"].as_array().unwrap().iter().enumerate(){
            let word=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let float=|v:&serde_json::Value|f32::from_bits(word(v));
            let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{
                edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(word(&h[5+t*4+e])))).collect(),
                vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|float(&h[20+i*4+j])))).collect(),
            }).collect();
            let frames:[PreparedObjectCache;2]=std::array::from_fn(|i|{
                let f=&c["frames"][i];let core_position=Vec3::new(float(&f[0]),float(&f[1]),float(&f[2]));
                let rotation=Mat3::from_cols_array(&std::array::from_fn(|j|float(&f[8+(j%3)*4+j/3])));
                PreparedObjectCache{core_position,pose:crate::ivp_motion::Pose{position:Vec3::new(float(&f[20]),float(&f[21]),float(&f[22])),rotation},quaternion:[0.,0.,0.,1.],frame_stamp:1}
            });
            let reverse=c["reverse"].as_u64().unwrap() as usize;let order=[reverse,1-reverse];let h=order.map(|i|&hulls[i]);let f=order.map(|i|frames[i]);
            let edges=order.map(|physical|CompactEdgeId{triangle:if parallel&&physical==1{1}else{0},edge:2});let setup=Setup::new(edges,h,f).unwrap();
            let fields=[setup.a[0].x,setup.a[0].y,setup.a[0].z,0.,setup.a[1].x,setup.a[1].y,setup.a[1].z,0.,setup.axis_a.x,setup.axis_a.y,setup.axis_a.z,0.,setup.axis_b.x,setup.axis_b.y,setup.axis_b.z,0.];
            assert_eq!(fields.map(f32::to_bits),std::array::from_fn(|j|word(&c["scratch"][j])),"case{n} setup");
            assert_eq!(setup.cross.to_array().map(f32::to_bits),std::array::from_fn(|j|word(&c["cross"][j])),"case{n} cross");
            assert_eq!(setup.coefficients().is_none(),parallel,"case{n} sampling flag");
            assert_eq!(setup.full_coefficients(f).unwrap().map(f32::to_bits),std::array::from_fn(|j|word(&c["coefficients"][j])),"case{n} coordinates");
            assert_eq!(distance(edges,h,f).unwrap().to_bits(),c["distance"].as_u64().unwrap(),"case{n} edge distance");
            let mut query=Query{retired:false,order:0,recovery:None,frame:0,kinds:[1,1],vertex:edges[0],face:edges[1],geometry:None};
            let result=query.refresh(1,h[0],h[1],f,0.02);
            if c["features"].as_array().unwrap().iter().any(|s|s[2]==5){
                // Native intermediate penetration packets are retained as a
                // known recovery gap, never counted as ordinary parity passes.
                let error=result.expect_err("penetration recovery is still unsupported");
                assert!(error.contains("penetrat"),"case{n}: {error}");
                assert_eq!((query.frame,query.kinds,query.vertex,query.face),(0,[1,1],edges[0],edges[1]));assert!(query.geometry.is_none());penetration+=1;continue;
            }
            assert!(result.unwrap_or_else(|error|panic!("case{n}: {error}")));ordinary+=1;
            let ids=[query.vertex,query.face];
            for i in 0..2{let expected=&c["features"][order[i]];assert_eq!([ids[i].triangle,ids[i].edge,query.kinds[i] as usize],[expected[0].as_u64().unwrap() as usize,expected[1].as_u64().unwrap() as usize,expected[2].as_u64().unwrap() as usize],"case{n} endpoint{i}");}
            let g=query.geometry.unwrap();let expected=&c["geometry"];
            assert_eq!([g.projection.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()],[word(&expected[0]),word(&expected[2]),word(&expected[3]),word(&expected[4]),word(&expected[5])],"case{n} geometry");
            assert!(!query.refresh(1,h[0],h[1],f,0.02).unwrap());
        }
        assert_eq!((ordinary,penetration),if parallel{(1140,12)}else{(384,0)});
        eprintln!("{ordinary} ordinary queries match;{penetration} native penetrations still require recovery");
    }
}
