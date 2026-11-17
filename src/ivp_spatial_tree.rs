//! Owned101131c0/101133d0 spatial membership and10112fa0/10112ee0 queries.
//! Native cell/hash ordering is retained. This does not create mindist/contact
//! listeners or implement the world's time/event scheduler.
use crate::ivp_broadphase::{self as grid,power};
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
struct Key([i32;5]);
impl Key{
    fn level(self)->i32{self.0[3]}
    fn hash(self)->u32{let mut bytes=[0u8;20];for (i,v) in self.0.iter().enumerate(){bytes[i*4..i*4+4].copy_from_slice(&v.to_le_bytes());}!crc32fast::hash(&bytes)|0x80000000}
}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
struct Node{key:Key,parent:Option<usize>,children:Vec<usize>,members:Vec<usize>}
#[derive(Clone,Copy)]
#[cfg_attr(test,derive(Debug))]
struct Bound{center:[f32;3],radius:f32,node:Option<usize>}
#[derive(Clone,Copy,Default)]
#[cfg_attr(test,derive(Debug))]
struct Slot{hash:u32,node:Option<usize>}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
struct Index{slots:Vec<Slot>,count:usize}
impl Index{
    fn new()->Self{Self{slots:vec![Slot::default();256],count:0}}
    fn mask(&self)->usize{self.slots.len()-1}
    fn find(&self,key:Key,nodes:&[Option<Node>])->Option<usize>{
        let hash=key.hash();let mut at=hash as usize&self.mask();
        loop{let slot=self.slots[at];let id=slot.node?;if slot.hash|0x80000000==hash&&nodes[id].as_ref().unwrap().key==key{return Some(id);}at=(at+1)&self.mask();}
    }
    fn insert(&mut self,node:usize,hash:u32){
        if self.mask()<self.count*2{
            let count=self.count;let size=self.slots.len()*2;let old=std::mem::replace(&mut self.slots,vec![Slot::default();size]);
            for s in old{if let Some(id)=s.node{self.insert(id,s.hash);}}
            self.count=count;
        }
        let mask=self.mask();let mut ideal=hash as usize&mask;let mut at=ideal;let mut pending=Slot{hash,node:Some(node)};self.count+=1;
        while self.slots[at].node.is_some(){
            let existing=self.slots[at].hash as usize&mask;
            if ideal<existing{pending=std::mem::replace(&mut self.slots[at],pending);ideal=existing;}
            at=(at+1)&mask;
        }
        self.slots[at]=pending;
    }
    fn remove(&mut self,key:Key,nodes:&[Option<Node>])->Result<usize,&'static str>{
        let hash=key.hash();let mask=self.mask();let mut hole=hash as usize&mask;
        let removed=loop{let slot=self.slots[hole];let id=slot.node.ok_or("native spatial hash key missing")?;if slot.hash|0x80000000==hash&&nodes[id].as_ref().unwrap().key==key{break id;}hole=(hole+1)&mask;};
        self.count-=1;
        let last=if self.slots[mask].node.is_some(){self.slots[mask].hash as usize&mask}else{mask};
        let mut scan=(hole+1)&mask;
        while self.slots[scan].node.is_some(){
            let ideal=self.slots[scan].hash as usize&mask;
            let shift=if hole<scan{(last<=ideal&&(ideal!=last||scan<ideal))||ideal<=hole}else{if hole!=mask{break;}last<=ideal};
            if shift{self.slots[hole]=self.slots[scan];hole=scan;}
            scan=(scan+1)&mask;
        }
        self.slots[hole]=Slot::default();Ok(removed)
    }
}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct Tree{root:Option<usize>,nodes:Vec<Option<Node>>,free:Vec<usize>,index:Index,elements:std::collections::BTreeMap<usize,Bound>}
impl Tree{
    pub(crate) fn new()->Self{Self{root:None,nodes:Vec::new(),free:Vec::new(),index:Index::new(),elements:Default::default()}}
    fn node(&self,id:usize)->&Node{self.nodes[id].as_ref().unwrap()}
    fn node_mut(&mut self,id:usize)->&mut Node{self.nodes[id].as_mut().unwrap()}
    fn allocate(&mut self,key:Key)->usize{let node=Some(Node{key,parent:None,children:Vec::new(),members:Vec::new()});if let Some(id)=self.free.pop(){self.nodes[id]=node;id}else{let id=self.nodes.len();self.nodes.push(node);id}}
    fn release(&mut self,id:usize){assert!(self.nodes[id].take().is_some());self.free.push(id);}
    fn register(&mut self,id:usize){self.index.insert(id,self.node(id).key.hash());}
    fn contains(parent:Key,child:Key)->bool{
        if parent.level()<child.level(){return false;}let shift=parent.level().wrapping_sub(child.level()) as u32&31;let extra=2i32.wrapping_shl(shift).wrapping_sub(2);
        (0..3).all(|a|{let low=parent.0[a].wrapping_shl(shift);low<=child.0[a]&&child.0[a]<=low.wrapping_add(extra)})
    }
    fn grow_root(&mut self,target:Key)->Result<(),&'static str>{
        let root=self.root.unwrap();let old=self.node(root).key;let mut words=old.0;words[3]+=1;words[4]+=1;
        let new_scale=power(words[3])?;let target_scale=power(target.level())?;
        for a in 0..3{let value=old.0[a];let mut half=value/2;let remainder=value%2;
            if remainder==-1||(remainder==0&&(target.0[a] as f32*target_scale)<(half as f32*new_scale)){half=half.wrapping_sub(1);}words[a]=half;
        }
        let parent=self.allocate(Key(words));self.node_mut(parent).children.push(root);self.node_mut(root).parent=Some(parent);self.register(parent);self.root=Some(parent);Ok(())
    }
    fn parent_for(&self,mut at:usize,target:Key)->usize{
        loop{if let Some(child)=self.node(at).children.iter().copied().find(|&id|Self::contains(self.node(id).key,target)){at=child;}else{return at;}}
    }
    fn attach(&mut self,mut parent:usize,leaf:usize){
        loop{
            let p=self.node(parent).key;let target=self.node(leaf).key;let gap=p.0[4]-target.0[4];
            if gap<2{self.node_mut(leaf).parent=Some(parent);self.node_mut(parent).children.push(leaf);return;}
            let mut words=p.0;words[3]-=1;words[4]-=1;
            for a in 0..3{
                let base=p.0[a].wrapping_mul(2);let shift=gap as u32&31;
                words[a]=if target.0[a]<p.0[a].wrapping_add(1).wrapping_shl(shift){
                    if target.0[a]<base.wrapping_add(1).wrapping_shl((gap-1) as u32&31){base}else{base.wrapping_add(1)}
                }else{base.wrapping_add(2)};
            }
            let child=self.allocate(Key(words));self.node_mut(child).parent=Some(parent);self.node_mut(parent).children.push(child);self.register(child);parent=child;
        }
    }
    pub(crate) fn insert(&mut self,id:usize,center:[f32;3],minimum:f32,maximum:f32,collect:bool)->Result<Vec<usize>,&'static str>{
        if self.elements.get(&id).is_some_and(|e|e.node.is_some()){return Err("spatial element already registered");}
        let cell=grid::select(center,minimum,maximum)?;let key=Key(cell.words());let node;
        if let Some(existing)=self.index.find(key,&self.nodes){node=existing;self.node_mut(node).members.push(id);}
        else{
            let leaf=self.allocate(key);self.node_mut(leaf).members.push(id);
            if self.root.is_none(){self.root=Some(leaf);self.register(leaf);self.elements.insert(id,Bound{center,radius:cell.radius,node:Some(leaf)});return Ok(Vec::new());}
            while !Self::contains(self.node(self.root.unwrap()).key,key){self.grow_root(key)?;}
            let root=self.root.unwrap();
            if self.node(root).key.level()==key.level(){self.node_mut(root).members.push(id);self.release(leaf);node=root;}
            else{self.register(leaf);let parent=self.parent_for(root,key);self.attach(parent,leaf);node=leaf;}
        }
        self.elements.insert(id,Bound{center,radius:cell.radius,node:Some(node)});
        if collect{self.query(id)}else{Ok(Vec::new())}
    }
    pub(crate) fn remove(&mut self,id:usize)->Result<(),&'static str>{
        // Native bounds belong to their external objects; the tree retains no
        // detached object pointer. Drop our registry copy on unregister too.
        let Some(bound)=self.elements.remove(&id) else{return Ok(());};let Some(mut at)=bound.node else{return Ok(());};
        let index=self.node(at).members.iter().rposition(|&owner|owner==id).ok_or("spatial member missing")?;self.node_mut(at).members.remove(index);
        loop{
            if !self.node(at).members.is_empty()||!self.node(at).children.is_empty(){return Ok(());}
            let key=self.node(at).key;let parent=self.node(at).parent;
            self.index.remove(key,&self.nodes)?;
            if let Some(p)=parent{let i=self.node(p).children.iter().rposition(|&v|v==at).ok_or("spatial child missing")?;self.node_mut(p).children.remove(i);}else{self.root=None;}
            self.release(at);
            if let Some(p)=parent{at=p;}else{return Ok(());}
        }
    }
    fn overlaps(a:Key,b:Key)->bool{
        let (large,small)=if a.level()>=b.level(){(a,b)}else{(b,a)};let shift=large.level().wrapping_sub(small.level()) as u32&31;
        (0..3).all(|axis|large.0[axis].wrapping_shl(shift)<small.0[axis].wrapping_add(2)&&small.0[axis]<large.0[axis].wrapping_add(2).wrapping_shl(shift))
    }
    fn candidates_at(&self,node:usize,query:Bound,out:&mut Vec<usize>){
        for &id in self.node(node).members.iter().rev(){let other=self.elements[&id];let x=other.center[0]-query.center[0];let y=other.center[1]-query.center[1];let z=other.center[2]-query.center[2];let radius=other.radius+query.radius;
            if (y*y+x*x)+z*z<=radius*radius{out.push(id);}
        }
    }
    fn gather_all(&self,at:usize,query:Bound,out:&mut Vec<usize>){self.candidates_at(at,query,out);for &child in self.node(at).children.iter().rev(){self.gather_all(child,query,out);}}
    fn gather(&self,at:usize,query:Bound,cell:usize,out:&mut Vec<usize>){
        self.candidates_at(at,query,out);
        for &child in &self.node(at).children{if child==cell{self.gather_all(child,query,out);}else if Self::overlaps(self.node(child).key,self.node(cell).key){self.gather(child,query,cell,out);}}
    }
    pub(crate) fn query(&self,id:usize)->Result<Vec<usize>,&'static str>{
        let bound=*self.elements.get(&id).ok_or("unknown spatial element")?;let cell=bound.node.ok_or("spatial element detached")?;let mut result=Vec::new();self.gather(self.root.ok_or("spatial root missing")?,bound,cell,&mut result);Ok(result)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn snapshot(tree:&Tree)->serde_json::Value{
        fn key(tree:&Tree,id:Option<usize>)->Option<[i32;5]>{id.map(|id|tree.node(id).key.0)}
        fn walk(tree:&Tree,id:usize,rows:&mut Vec<serde_json::Value>){let n=tree.node(id);rows.push(serde_json::json!({"cell":n.key.0,"hash":n.key.hash(),"parent":key(tree,n.parent),"members":n.members,"children":n.children.iter().map(|&id|tree.node(id).key.0).collect::<Vec<_>>()}));for &child in &n.children{walk(tree,child,rows);}}
        let mut rows=Vec::new();if let Some(root)=tree.root{walk(tree,root,&mut rows);}
        let slots=tree.index.slots.iter().enumerate().filter_map(|(i,s)|s.node.map(|id|serde_json::json!({"slot":i,"hash":s.hash,"cell":tree.node(id).key.0}))).collect::<Vec<_>>();
        let elements=tree.elements.iter().filter_map(|(&id,b)|b.node.map(|node|serde_json::json!({"id":id,"cell":tree.node(node).key.0,"center_bits":b.center.map(f32::to_bits),"radius_bits":b.radius.to_bits()}))).collect::<Vec<_>>();
        serde_json::json!({"root":key(tree,tree.root),"nodes":rows,"hash_mask":tree.index.mask(),"hash_count":tree.index.count,"hash_slots":slots,"elements":elements})
    }
    #[test]
    fn owned_tree_topology_hash_slots_pruning_and_ordered_queries_match_without_reseeding(){
        let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-spatial-tree.json")).unwrap();let mut count=0;let mut candidates=0;
        for c in d["spatial_tree_controls"].as_array().unwrap(){let mut tree=Tree::new();assert_eq!(snapshot(&tree),c["initial"]);
            for e in c["events"].as_array().unwrap(){let id=e["element"].as_u64().unwrap() as usize;let action=e["action"].as_u64().unwrap();let mut found=Vec::new();
                if action==1||action==2{tree.remove(id).unwrap();}
                if action==0||action==2{let center=std::array::from_fn(|i|f32::from_bits(e["center_bits"][i].as_u64().unwrap() as u32));found=tree.insert(id,center,e["minimum"].as_f64().unwrap() as f32,e["maximum"].as_f64().unwrap() as f32,e["query"].as_bool().unwrap()).unwrap();}
                else if action==3{found=tree.query(id).unwrap();}
                candidates+=found.len();assert_eq!(serde_json::json!(found),e["candidates"],"query scenario{} step{}",c["scenario"],e["step"]);
                assert_eq!(snapshot(&tree),e["after"],"tree scenario{} step{}",c["scenario"],e["step"]);count+=1;
                assert_eq!(tree.nodes.len(),tree.index.count+tree.free.len());
                assert_eq!(tree.elements.len(),e["after"]["elements"].as_array().unwrap().len());
            }
            let high_water=tree.nodes.len();let e=&c["empty_reuse"];let center=std::array::from_fn(|i|f32::from_bits(e["center_bits"][i].as_u64().unwrap() as u32));
            assert_eq!(serde_json::json!(tree.insert(0,center,0.5,1.,true).unwrap()),e["candidates"]);assert_eq!(snapshot(&tree),e["inserted"]);
            tree.remove(0).unwrap();assert_eq!(snapshot(&tree),e["detached"]);tree.remove(0).unwrap();assert_eq!(snapshot(&tree),e["detached_twice"]);
            assert_eq!(tree.nodes.len(),high_water,"empty-tree reuse must not grow the source arena");assert_eq!(tree.free.len(),tree.nodes.len());assert!(tree.elements.is_empty());
        }
        assert_eq!((count,candidates),(380,678));
    }
}
