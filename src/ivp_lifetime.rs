//! Owned1011c840/1011c7b0 managed-contact list ordering and pruning.
//! These list transitions do not perform100ff090 cache/core/graph deletion
//! side effects, contact creation or geometric feature updates.
#[derive(Clone,Debug,PartialEq)]
pub(crate) struct Contact {pub id:usize,pub counter:i16,pub distance:f32,pub status:i16,pub core_flags:[u16;2]}
#[derive(Clone,Debug,PartialEq)]
pub(crate) struct State {pub contacts:Vec<Contact>,pub skipped:usize}
#[derive(Clone,Copy)]
pub(crate) struct Settings {pub retire_distance:f32,pub target_distance:f32,pub extra_distance:f32}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum Path {Single,Batch}

impl State {
    /// Native linked insertion sort is stable for equal signed counters.
    pub(crate) fn sort(&mut self) {self.contacts.sort_by_key(|c|c.counter);}
    /// Iterate the original next-node order even when a node moves to the head.
    /// Returned identities request deletion; native deletion side effects are
    /// not deferred here or substituted by list removal in the world solver.
    pub(crate) fn prune(&mut self,settings:Settings)->Vec<usize> {
        let order=self.contacts.iter().map(|c|c.id).collect::<Vec<_>>();let mut removed=vec![];
        let near=settings.extra_distance+settings.target_distance;
        for id in order {
            let i=self.contacts.iter().position(|c|c.id==id).unwrap();let c=&self.contacts[i];
            if c.distance>=settings.retire_distance || c.status==1 {self.contacts.remove(i);removed.push(id);}
            else if c.distance>near && c.core_flags[0]&c.core_flags[1]&1!=0 {let node=self.contacts.remove(i);self.contacts.insert(0,node);}
        }
        self.skipped=0;removed
    }
    ///100ffec0 chooses its branch before1011d5c0 sorts/prunes. A batch
    /// pruned down to one or zero contacts must not become the direct path.
    pub(crate) fn prepare(&mut self,settings:Settings)->Path {
        if self.contacts.len()<2 {Path::Single}else{self.sort();self.prune(settings);Path::Batch}
    }
}

#[cfg(test)]
pub(crate) fn reference_before(case:&serde_json::Value)->(State,Settings) {
    (reference_state(&case["before"]),Settings {retire_distance:case["constants"]["retire_distance"].as_f64().unwrap() as f32,
        target_distance:case["constants"]["target_distance"].as_f64().unwrap() as f32,extra_distance:case["constants"]["extra_distance"].as_f64().unwrap() as f32})
}
#[cfg(test)]
fn reference_state(value:&serde_json::Value)->State {
    State {skipped:value["skip"].as_u64().unwrap() as usize,contacts:value["nodes"].as_array().unwrap().iter().map(|n|Contact {id:n["id"].as_u64().unwrap() as usize,
        counter:n["counter"].as_i64().unwrap() as i16,distance:n["distance"].as_f64().unwrap() as f32,status:n["status"].as_i64().unwrap() as i16,
        core_flags:std::array::from_fn(|i|n["core_flags"][i].as_u64().unwrap() as u16)}).collect()}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture()->serde_json::Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-lifetime.json")).unwrap()}
    #[test]
    fn native_managed_ordering_and_pruned_list_states_match() {
        let data=fixture();let mut sorts=0;let mut prunes=0;let mut removed=0;
        for case in data["cases"].as_array().unwrap() {
            let (mut state,settings)=reference_before(case);
            if case["operation"]=="sort" {state.sort();sorts+=1;}else{removed+=state.prune(settings).len();prunes+=1;}
            assert_eq!(state,reference_state(&case["after"]),"{} step{} {}",case["model"],case["step"],case["operation"]);
        }
        assert_eq!((sorts,prunes,removed),(387,389,31));println!("{sorts} exact native sorts,{prunes} prune/list updates,{removed} removed identities; native deletion side effects remain separate");
    }
    #[test]
    fn repeated_controlled_lists_reuse_computed_state() {
        let data=fixture();let mut reuses=0;
        for step in [-16,-32] {
            let cases=data["cases"].as_array().unwrap().iter().filter(|c|c["step"].as_i64()==Some(step)).collect::<Vec<_>>();assert!(!cases.is_empty());
            let (mut state,settings)=reference_before(cases[0]);
            for (i,case) in cases.iter().enumerate() {if case["operation"]=="sort" {state.sort();}else{state.prune(settings);}assert_eq!(state,reference_state(&case["after"]));reuses+=(i>0) as usize;}
        }
        assert_eq!(reuses,8);println!("{reuses} chained native list updates reuse source state");
    }
    #[test]
    fn pre_prune_dispatch_and_normal_mode_assignment_compose() {
        let data=fixture();let cases=data["cases"].as_array().unwrap();
        let normal:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-history.json")).unwrap();
        let mut groups=0;let mut one=0;let mut zero=0;
        for case in cases.iter().filter(|c|c["step"].as_i64().unwrap()>=0 && c["operation"]=="sort") {
            let sequence=case["sequence"].as_u64().unwrap();let same=|other:&serde_json::Value|other["capture"]==case["capture"]&&other["model"]==case["model"];
            let prune=cases.iter().find(|c|same(c)&&c["sequence"].as_u64()==Some(sequence+1)).unwrap();assert_eq!(prune["operation"],"prune");
            let (mut state,settings)=reference_before(case);assert_eq!(state.prepare(settings),Path::Batch);
            let mut expected=reference_state(&prune["after"]);
            // Real per-call observation IDs are local before-list positions.
            // Rebind the prune IDs through the independently observed sort.
            let sorted=reference_state(&case["after"]);for node in &mut expected.contacts {node.id=sorted.contacts[node.id].id;}
            assert_eq!(state,expected);one+=(state.contacts.len()==1) as usize;zero+=state.contacts.is_empty() as usize;
            let record=normal["cases"].as_array().unwrap().iter().find(|c|same(c)&&c["sequence"].as_u64()==Some(sequence/2)).unwrap();assert_eq!(record["step"],case["step"]);
            let rows=record["before"].as_array().unwrap();assert_eq!(rows.len(),state.contacts.len());
            for (node,row) in state.contacts.iter().zip(rows) {assert_eq!(node.counter as i32,row["mode"].as_u64().unwrap() as u32 as i32);assert_eq!(node.distance,row["gap"].as_f64().unwrap() as f32);}
            groups+=1;
        }
        assert_eq!((groups,one,zero),(368,13,3));println!("{groups} exact source sort/prune/mode contexts;{one} one-row and{zero} empty groups preserve batch dispatch");
    }
}
