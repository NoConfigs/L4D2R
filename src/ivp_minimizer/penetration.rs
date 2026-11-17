//! Native1010d980 visited-face walk after an intermediate kind5 result.
//! Full minimizer retry/face-pair recovery is not implemented by this kernel.
use super::*;
use l4d2_formats::phy::TriangleHeader;
pub(crate) fn recover_seed(h:&CompactTopology,headers:&[TriangleHeader],initial:CompactEdgeId,point:Vec3)->Result<CompactEdgeId,&'static str>{
    if !point.is_finite()||headers.len()!=h.edges.len()||h.edge(initial).is_none(){return Err("invalid native penetration seed inputs");}
    for (i,header) in headers.iter().enumerate(){if header.0 as usize&0xfff!=i{return Err("invalid indexed compact triangle header");}}
    let hint=((headers[initial.triangle].0>>12)&0xfff) as usize;
    if hint>=headers.len(){return Err("native penetration pierce hint outside compact ledge");}
    let mut active=CompactEdgeId{triangle:hint,edge:0};let mut visited=vec![false;headers.len()];
    loop{
        if visited[active.triangle]{return Err("invalid repeated native penetration walk face");}
        visited[active.triangle]=true;let mut edge=active;let mut chosen=None;
        for _ in 0..3{
            let other=across(h,edge)?;
            if !(opposite_weight(h,edge,point)?>0.)&&!visited[other.triangle]{chosen=Some(other);break;}
            edge=next(h,edge)?;
        }
        let Some(other)=chosen else{return Ok(active);};active=other;
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    use l4d2_formats::phy::CompactEdge;
    #[test]
    fn native_penetration_seed_walk_keeps_hint_and_visited_order(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../../tools/fixtures/native-ivp-penetration-seed.json")).unwrap();
        assert_eq!(data["cases"].as_array().unwrap().len(),48);
        for (n,c) in data["cases"].as_array().unwrap().iter().enumerate(){
            let h=&c["hull"];let word=|v:&serde_json::Value|v.as_u64().unwrap() as u32;
            let topology=CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(word(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f32::from_bits(word(&h[20+i*4+j]))))).collect()};
            let headers:Vec<_>=(0..4).map(|i|TriangleHeader(word(&h[4+i*4]))).collect();
            let initial=CompactEdgeId{triangle:c["initial"][0].as_u64().unwrap() as usize,edge:c["initial"][1].as_u64().unwrap() as usize};
            let point=Vec3::from_array(std::array::from_fn(|j|f32::from_bits(word(&c["point"][j]))));
            let result=recover_seed(&topology,&headers,initial,point).unwrap();
            assert_eq!([result.triangle,result.edge],[c["expected"][0].as_u64().unwrap() as usize,c["expected"][1].as_u64().unwrap() as usize],"seed{n}");
        }
    }
}
