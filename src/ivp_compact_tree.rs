//! Owned100f6340/100f61c0 sphere/quantized-box admission and ordered traversal.
//! Zero-offset terminal/triangle fallback is still separate.
use l4d2_formats::phy::CompactTree;
use std::cmp::Ordering;
#[derive(Clone,Copy)]
pub(crate) enum Precision{P24,P53,P64}
impl Precision{fn bits(self)->u32{match self{Self::P24=>24,Self::P53=>53,Self::P64=>64}}}
/// Positive x87 values. Only finite float inputs and short products are used;
/// their exponents stay within extended range. Integer rounding preserves the
/// configured significand without imposing float32 exponent underflow.
#[derive(Clone,Copy)]
struct Extended{m:u64,e:i32}
impl Extended{
    fn float(v:f32)->Self{let b=v.to_bits();let ex=(b>>23)&255;Self{m:if ex==0{(b&0x7fffff) as u64}else{((b&0x7fffff)|0x800000) as u64},e:if ex==0{-149}else{ex as i32-150}}}
    fn round(n:u128,mut e:i32,p:u32)->Self{
        if n==0{return Self{m:0,e:0};}let bits=128-n.leading_zeros();let mut m=n;
        if bits>p{let shift=bits-p;let mask=(1u128<<shift)-1;let tail=n&mask;let half=1u128<<(shift-1);m=n>>shift;e+=shift as i32;
            if tail>half||tail==half&&m&1!=0{m+=1;}if m>>p!=0{m>>=1;e+=1;}
        }Self{m:m as u64,e}
    }
    fn mul(self,b:Self,p:u32)->Self{Self::round(self.m as u128*b.m as u128,self.e+b.e,p)}
    fn top(self)->i32{self.e+63-self.m.leading_zeros() as i32}
    fn add(self,b:Self,p:u32)->Self{
        if self.m==0{return b;}if b.m==0{return self;}
        let unit=self.top().max(b.top())-p as i32-3;
        let scaled=|x:Self|->u128{if x.e>=unit{(x.m as u128)<<(x.e-unit)}else{let shift=(unit-x.e) as u32;
            if shift>=64{u128::from(x.m!=0)}else{((x.m>>shift) as u128)|u128::from(x.m&((1u64<<shift)-1)!=0)}
        }};
        Self::round(scaled(self)+scaled(b),unit,p)
    }
    fn cmp(self,b:Self)->Ordering{
        match(self.m==0,b.m==0){(true,true)=>Ordering::Equal,(true,false)=>Ordering::Less,(false,true)=>Ordering::Greater,_=>{
            self.top().cmp(&b.top()).then_with(||(self.m<<self.m.leading_zeros()).cmp(&(b.m<<b.m.leading_zeros())))
        }}
    }
}
fn admitted(center:[f32;3],radius:f32,box_sizes:[u8;3],point:[f32;3],query_radius:f32,precision:Precision)->bool{
    let d=std::array::from_fn::<_,3,_>(|i|center[i]-point[i]);let bound=radius+query_radius;
    if bound*bound<(d[1]*d[1]+d[0]*d[0])+d[2]*d[2]{return false;}
    let p=precision.bits();let scale=Extended::float(radius).mul(Extended::float(f32::from_bits(0x3b83126f)),p);
    for i in 0..3{
        if !d[i].is_finite(){return false;}
        let extent=scale.mul(Extended{m:box_sizes[i] as u64,e:0},p).add(Extended::float(query_radius),p);
        if extent.cmp(Extended::float(d[i].abs()))!=Ordering::Greater{return false;}
    }true
}
pub(crate) fn append(tree:&CompactTree,point:[f32;3],radius:f32,restricted:Option<usize>,precision:Precision,output:&mut Vec<usize>)->Result<(),&'static str>{
    if !radius.is_finite()||radius<0.||point.iter().any(|v|!v.is_finite()){return Err("invalid compact-tree query");}
    enum Visit{Enter(usize),Leave(usize)}
    let mut stack=Vec::new();let mut path=Vec::new();
    if let Some(key)=restricted{let root=*tree.ledge_nodes.get(&key).ok_or("unresolved compact-ledge root backlink")?;
        let children=tree.nodes.get(root).ok_or("missing restricted root")?.children.ok_or("restricted root has no children")?;
        stack.push(Visit::Enter(children[1]));stack.push(Visit::Enter(children[0]));
    }else{stack.push(Visit::Enter(tree.root));}
    while let Some(v)=stack.pop(){let index=match v{Visit::Leave(i)=>{if path.pop()!=Some(i){return Err("invalid traversal lifetime");}continue;},Visit::Enter(i)=>i};
        let node=tree.nodes.get(index).ok_or("missing compact tree node")?;
        if !node.radius.is_finite()||node.radius<0.||!node.center_ivp.is_finite(){return Err("invalid compact node bounds");}
        if !admitted(node.center_ivp.to_array(),node.radius,node.box_sizes,point,radius,precision){continue;}
        if let Some(ledge)=node.ledge{output.push(ledge);continue;}
        let children=node.children.ok_or("zero-offset terminal triangle query not yet reconstructed")?;
        if path.contains(&index){return Err("cyclic compact tree");}path.push(index);stack.push(Visit::Leave(index));stack.push(Visit::Enter(children[1]));stack.push(Visit::Enter(children[0]));
    }Ok(())
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn native_compact_tree_bounds_order_and_x87_modes_match_owned_packets(){
        let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-compact-tree.json")).unwrap();let mut count=0;
        for c in d["compact_tree_controls"].as_array().unwrap(){
            let mut tree=CompactTree::default();tree.root=c["root"].as_u64().unwrap() as usize;
            for n in c["nodes"].as_array().unwrap(){
                tree.nodes.push(l4d2_formats::phy::CompactTreeNode{key:n["key"].as_u64().unwrap() as usize,center_ivp:std::array::from_fn::<_,3,_>(|i|f32::from_bits(n["center_bits"][i].as_u64().unwrap() as u32)).into(),radius:f32::from_bits(n["radius_bits"].as_u64().unwrap() as u32),box_sizes:std::array::from_fn(|i|n["box"][i].as_u64().unwrap() as u8),children:n["children"].as_array().map(|a|[a[0].as_u64().unwrap() as usize,a[1].as_u64().unwrap() as usize]),ledge:n["ledge"].as_u64().map(|v|v as usize),flags:0});
            }
            for(i,n)in c["ledge_roots"].as_array().unwrap().iter().enumerate(){tree.ledge_nodes.insert(i,n.as_u64().unwrap() as usize);}
            for q in c["queries"].as_array().unwrap(){
                let mode=match q["mode"].as_u64().unwrap(){24=>Precision::P24,53=>Precision::P53,64=>Precision::P64,_=>panic!("unsupported captured precision")};
                let mut output=q["prefix"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();
                append(&tree,std::array::from_fn(|i|f32::from_bits(q["point_bits"][i].as_u64().unwrap() as u32)),f32::from_bits(q["radius_bits"].as_u64().unwrap() as u32),q["restricted"].as_u64().map(|v|v as usize),mode,&mut output).unwrap();
                assert_eq!(serde_json::json!(output),q["output"],"tree{} query{count} mode{}",c["config"],q["mode"]);count+=1;
            }
        }assert_eq!(count,6048);
    }
    #[test]
    fn authored_compact_trees_and_native_queries_match_original_phy_geometry(){
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists(){return;}
        let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
        let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-compact-tree-authored.json")).unwrap();
        let mut count=0;let mut coarse_count=0;
        for c in d["cases"].as_array().unwrap(){
            let solids=if c["role"]=="body"{
                l4d2_formats::phy::parse_phy(&assets.read(c["model"].as_str().unwrap()).unwrap()).unwrap()
            }else{
                let hex=c["tree"]["collision_hex"].as_str().unwrap();
                let raw=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();
                let mut packet=(raw.len() as u32).to_le_bytes().to_vec();packet.extend(raw);
                l4d2_formats::phy::parse_collision(&packet,1).unwrap()
            };
            assert_eq!(solids.len(),1);let solid=&solids[0];let tree=solid.compact_tree.as_ref().unwrap();let t=&c["tree"];
            assert_eq!(tree.root,t["root"].as_u64().unwrap() as usize);
            assert_eq!(tree.nodes.len(),t["nodes"].as_array().unwrap().len());
            for(n,r)in tree.nodes.iter().zip(t["nodes"].as_array().unwrap()){
                assert_eq!(n.key,r["key"].as_u64().unwrap() as usize);
                assert_eq!(n.center_ivp.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|i|r["center_bits"][i].as_u64().unwrap() as u32));
                assert_eq!(n.radius.to_bits(),r["radius_bits"].as_u64().unwrap() as u32);
                assert_eq!(n.box_sizes,std::array::from_fn(|i|r["box"][i].as_u64().unwrap() as u8));
                assert_eq!(n.flags,if c["role"]=="body"{r["authored_flags"].as_u64().unwrap() as u8}else{r["flags"].as_u64().unwrap() as u8});
                assert_eq!(n.children,r["children"].as_array().map(|v|[v[0].as_u64().unwrap() as usize,v[1].as_u64().unwrap() as usize]));
                assert_eq!(n.ledge,r["ledge"].as_u64().map(|v|v as usize));
                if let Some(key)=n.ledge{
                    assert!(solid.compact_ledge(key).is_some());assert!(tree.ledge_headers.contains_key(&key));
                    if n.children.is_some(){coarse_count+=1;assert!(tree.coarse_convexes.contains_key(&key));assert!(!tree.leaf_convexes.contains_key(&key));}
                }
            }
            let roots=t["ledge_roots"].as_array().unwrap().iter().map(|v|(v[0].as_u64().unwrap() as usize,v[1].as_u64().unwrap() as usize)).collect::<std::collections::BTreeMap<_,_>>();
            assert_eq!(tree.ledge_nodes,roots);
            assert_eq!(tree.leaf_convexes.len(),solid.convexes.len());
            let precision=match t["control_word"].as_u64().unwrap()&0x300{0=>Precision::P24,0x200=>Precision::P53,0x300=>Precision::P64,_=>panic!("invalid x87 precision")};
            for q in t["queries"].as_array().unwrap(){
                let mut output=q["prefix"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();
                append(tree,std::array::from_fn(|i|f32::from_bits(q["point_bits"][i].as_u64().unwrap() as u32)),f32::from_bits(q["radius_bits"].as_u64().unwrap() as u32),q["restricted"].as_u64().map(|v|v as usize),precision,&mut output).unwrap();
                assert_eq!(serde_json::json!(output),q["output"],"{} {} query{count}",c["model"],c["role"]);count+=1;
            }
        }assert_eq!(count,768);assert!(coarse_count>0);
    }

}
