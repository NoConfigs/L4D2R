//! Native1010ce30 surface selection and1010b150 pair-list partitioning.
//! Local caller predictors are inputs; world scheduling and pair responses are separate.
use crate::ivp_motion::{Prediction,Pose,ObjectFrame};
use crate::ivp_compact_tree::{self,Precision};
use bevy::prelude::*;
use l4d2_formats::phy::CompactTree;
#[path="ivp_surface_contacts.rs"]
pub(crate) mod contacts;
#[derive(Clone,Copy)]
pub(crate) struct Body {pub prediction:Prediction,pub prepared_pose:Pose,pub core_radius:f32,pub margin:f32}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct Pair {pub slots:[usize;2],pub ledges:[usize;2]}
pub(crate) struct Query {pub point:[f32;3],pub radius:f32,pub restricted:Option<usize>}
/// Native uses the other core's predicted centre, not its shifted object origin.
/// X retains the double elapsed time until its final float store; Y/Z use the
/// earlier float elapsed time and float multiply/add instructions.
pub(crate) fn query(own:Body,other:Body,at:f64,margin:f32,restricted:Option<usize>)->Result<Query,&'static str>{
    if !at.is_finite()||!margin.is_finite()||margin<0.||!own.margin.is_finite()||own.margin<0.||!other.core_radius.is_finite()||other.core_radius<0.{return Err("invalid surface admission inputs");}
    let elapsed=at-other.prediction.time;let dt=elapsed as f32;
    let mut center=other.prediction.start+other.prediction.velocity*dt;
    center.x=(other.prediction.start.x as f64+other.prediction.velocity.x as f64*elapsed) as f32;
    let pose=own.prepared_pose;
    let delta=center-pose.position;let r=pose.rotation;
    //100ed9a0 inverse transform: transpose the prepared object rotation,
    // preserving native SSE product/add grouping.
    let point=std::array::from_fn(|i|{let col=r.col(i);(col.y*delta.y+col.x*delta.x)+col.z*delta.z});
    let radius=(own.margin+other.core_radius)+margin;
    if point.iter().any(|v|!v.is_finite())||!radius.is_finite(){return Err("nonfinite surface admission result");}
    Ok(Query{point,radius,restricted})
}
pub(crate) fn select(trees:[&CompactTree;2],bodies:[Body;2],at:f64,margin:f32,provided:[Option<usize>;2],restricted:[Option<usize>;2])->Result<([Vec<usize>;2],[Option<Query>;2]),&'static str>{
    let mut lists:[Vec<usize>;2]=std::array::from_fn(|_|Vec::new());let mut queries=[None,None];
    for i in 0..2{if let Some(key)=provided[i]{lists[i].push(key);}else{let q=query(bodies[i],bodies[1-i],at,margin,restricted[i])?;ivp_compact_tree::append(trees[i],q.point,q.radius,q.restricted,Precision::P53,&mut lists[i])?;queries[i]=Some(q);}}
    Ok((lists,queries))
}
/// Existing matches are swapped into a retained prefix in reverse Cartesian
/// traversal order. Unmatched suffix entries are removed back-to-front. New
/// entries are staged during traversal then appended in reverse staging order.
/// The hash table affects lookup cost, not these swaps, for unique pair keys.
pub(crate) fn reconcile(slots:[usize;2],lists:&[Vec<usize>;2],pairs:&mut Vec<Pair>)->Result<(Vec<Pair>,Vec<Pair>),&'static str>{
    for i in 0..pairs.len(){if pairs[i].slots!=slots||pairs[..i].contains(&pairs[i]){return Err("unsupported duplicate/foreign existing pair");}}
    let mut cursor=0;let mut created=Vec::new();
    for &left in lists[0].iter().rev(){for &right in lists[1].iter().rev(){let key=Pair{slots,ledges:[left,right]};
        if let Some(index)=pairs.iter().position(|p|*p==key){if index<cursor{return Err("repeated existing pair admission");}pairs.swap(cursor,index);cursor+=1;}
        else{created.push(key);}
    }}
    let removed=pairs[cursor..].iter().rev().copied().collect();pairs.truncate(cursor);pairs.extend(created.iter().rev());
    Ok((created,removed))
}
#[cfg(test)]
mod tests{
 use super::*;
 #[test]
 fn actual_surface_inputs_and_continuous_pair_identities_match_native_admission(){
    let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");if !root.exists(){return;}
    let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
    let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-surface-admission.json")).unwrap();let mut count=0;let mut queries_count=0;let mut births=0;let mut deaths=0;
    let v3=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|v[i].as_f64().unwrap() as f32));
    let q4=|v:&serde_json::Value|std::array::from_fn(|i|v[i].as_f64().unwrap() as f32);
    let pairs_json=|v:&Vec<Pair>|serde_json::Value::Array(v.iter().map(|p|serde_json::json!({"slots":p.slots,"ledges":p.ledges})).collect());
    let ids=|v:&serde_json::Value|serde_json::Value::Array(v.as_array().unwrap().iter().map(|p|serde_json::json!({"slots":p["slots"],"ledges":p["ledges"]})).collect());
    for c in d["cases"].as_array().unwrap(){
        let moving=l4d2_formats::phy::parse_phy(&assets.read(c["model"].as_str().unwrap()).unwrap()).unwrap();
        let hex=c["floor"].as_str().unwrap();let raw=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();let mut packet=(raw.len() as u32).to_le_bytes().to_vec();packet.extend(raw);let floor=l4d2_formats::phy::parse_collision(&packet,1).unwrap();
        let trees=[moving[0].compact_tree.as_ref().unwrap().as_ref(),floor[0].compact_tree.as_ref().unwrap().as_ref()];
        let mut pairs=Vec::new();let mut owned=contacts::Contacts::new([0,1],[&moving[0],&floor[0]]).unwrap();
        for (index,a)in c["admissions"].as_array().unwrap().iter().enumerate(){
            assert_eq!(pairs_json(&pairs),ids(&a["before"]),"{} before{index}",c["model"]);
            let bodies=std::array::from_fn(|i|{let p=&a["predictions"][i];Body{prediction:Prediction{start:v3(&p["start"]),velocity:v3(&p["velocity"]),rotation_start:q4(&p["rotation_start"]),rotation_end:q4(&p["rotation_end"]),time:p["time"].as_f64().unwrap(),inverse_dt:p["inverse_dt"].as_f64().unwrap() as f32},prepared_pose:{
                // A same-frame cache can have been prepared before the current
                // core predictor was replaced. Reconstruct its own preparation
                // inputs, never substitute the newer predictor or expected pose.
                let q=c["queries"].as_array().unwrap().iter().find(|q|q["admission"].as_u64()==Some(index as u64)&&q["slot"].as_u64()==Some(i as u64)).unwrap();
                assert_eq!(q["cache_observed"],true);let cached=&q["cache_input"];
                let prediction=Prediction{start:v3(&cached["start"]),velocity:v3(&cached["velocity"]),rotation_start:q4(&cached["rotation_start"]),rotation_end:q4(&cached["rotation_end"]),time:cached["time"].as_f64().unwrap(),inverse_dt:cached["inverse_dt"].as_f64().unwrap() as f32};
                let frame=ObjectFrame{shift:(cached["flags"].as_u64().unwrap()&0x800==0).then(||v3(&cached["shift"])),rotation:cached["extra"].as_bool().unwrap().then(||q4(&cached["object_rotation"]))};
                prediction.prepare_object_cache(cached["now"].as_f64().unwrap(),frame,cached["frame"].as_u64().unwrap() as u32).pose
            },core_radius:a["core_radii"][i].as_f64().unwrap() as f32,margin:a["object_margins"][i].as_f64().unwrap() as f32}});
            let optional=|key:&str|std::array::from_fn(|i|a[key][i].as_i64().filter(|v|*v>=0).map(|v|v as usize));
            let (lists,queries)=select(trees,bodies,a["predictions"][0]["now"].as_f64().unwrap(),f32::from_bits(a["margin_bits"].as_u64().unwrap() as u32),optional("provided"),optional("restricted")).unwrap();
            for native in c["queries"].as_array().unwrap().iter().filter(|q|q["admission"].as_u64()==Some(index as u64)){
                let slot=native["slot"].as_u64().unwrap() as usize;let q=queries[slot].as_ref().unwrap();
                assert_eq!(serde_json::json!(q.point.map(f32::to_bits)),native["point_bits"],"{} query point{index} slot{slot}",c["model"]);
                assert_eq!(q.radius.to_bits(),native["radius_bits"].as_u64().unwrap() as u32,"radius{index}");
                assert_eq!(serde_json::json!(lists[slot]),native["output"],"output{index}");queries_count+=1;
            }
            reconcile([0,1],&lists,&mut pairs).unwrap();assert_eq!(pairs_json(&pairs),ids(&a["after"]),"{} after{index}",c["model"]);
            let changes=owned.update(contacts::Input{bodies,at:a["predictions"][0]["now"].as_f64().unwrap(),margin:f32::from_bits(a["margin_bits"].as_u64().unwrap() as u32),provided:optional("provided"),restricted:optional("restricted")},|_,entry,convexes|{
                let contact=&mut entry.contact;
                // Ordinary construction links before refresh. This control
                // checks source-created queries, not global graph state.
                contact.flags=(contact.flags&0xffcfffff)|0xc0000;
                let hulls=convexes.map(|c|c.topology.as_deref().unwrap());let headers=convexes.map(|c|c.triangle_headers.as_slice());
                let prepared=std::array::from_fn(|j|{let q=c["queries"].as_array().unwrap().iter().find(|q|q["admission"].as_u64()==Some(index as u64)&&q["slot"].as_u64()==Some(j as u64)).unwrap();let cached=&q["cache_input"];let prediction=Prediction{start:v3(&cached["start"]),velocity:v3(&cached["velocity"]),rotation_start:q4(&cached["rotation_start"]),rotation_end:q4(&cached["rotation_end"]),time:cached["time"].as_f64().unwrap(),inverse_dt:cached["inverse_dt"].as_f64().unwrap() as f32};prediction.prepare_object_cache(cached["now"].as_f64().unwrap(),ObjectFrame{shift:(cached["flags"].as_u64().unwrap()&0x800==0).then(||v3(&cached["shift"])),rotation:cached["extra"].as_bool().unwrap().then(||q4(&cached["object_rotation"]))},cached["frame"].as_u64().unwrap() as u32)});
                let frame=a["predictions"][0]["frame"].as_u64().unwrap() as u32;
                let outcome=contact.query.refresh_contact(frame,hulls,prepared,a["object_margins"][0].as_f64().unwrap() as f32+a["object_margins"][1].as_f64().unwrap() as f32,headers,&mut contact.flags)?;
                assert_eq!(outcome,crate::ivp_minimizer::Refresh::Ordinary,"{} birth admission{index}",c["model"]);births+=1;Ok(())
            },|_,_|{deaths+=1;Ok(())}).unwrap_or_else(|e|panic!("{} owned admission{index}: {e}",c["model"]));
            assert_eq!(changes.lists,lists);assert_eq!(changes.queries.iter().filter(|q|q.is_some()).count(),queries.iter().filter(|q|q.is_some()).count());
            let owned_keys=owned.entries().iter().map(|e|e.key).collect::<Vec<_>>();assert_eq!(pairs_json(&owned_keys),ids(&a["after"]));
            for (entry,expected) in owned.entries().iter().zip(a["after"].as_array().unwrap()){assert_eq!(serde_json::json!(entry.contact.query.kinds),expected["kinds"],"{} owned kinds admission{index}",c["model"]);}
            count+=1;
        }
    }assert_eq!(count,51);assert_eq!(queries_count,102);assert_eq!((births,deaths),(8,0));println!("owned contact births{births}, removals{deaths}");
 }
 #[test]
 fn retained_pair_prefix_and_removal_order_match_native_hash_packets(){
    let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-pair-partition.json")).unwrap();let mut cases=0;
    for c in d["pair_partition_controls"].as_array().unwrap(){
        let key=|v:&serde_json::Value|Pair{slots:[0,1],ledges:[v[0].as_u64().unwrap() as usize,v[1].as_u64().unwrap() as usize]};
        let mut pairs=c["initial"].as_array().unwrap().iter().map(key).collect::<Vec<_>>();
        let lists=std::array::from_fn(|i|c["lists"][i].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>());
        let matched=c["matched"].as_u64().unwrap() as usize;
        let partition=c["partition"].as_array().unwrap().iter().map(key).collect::<Vec<_>>();
        let (_,removed)=reconcile([0,1],&lists,&mut pairs).unwrap();
        assert_eq!(&pairs[..matched],&partition[..matched],"retained prefix {}",c["config"]);
        assert_eq!(removed,partition[matched..].iter().rev().copied().collect::<Vec<_>>(),"removal order {}",c["config"]);cases+=1;
    }assert_eq!(cases,128);
 }

}
