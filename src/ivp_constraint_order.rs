//! Native constraint-vector lifecycle: append (100af1f0), stable deletion
//! (100afae0), zero-class reorder (100af350), dirty dispatch (100af500).
//! Object/member connectivity and each constraint's numerical kernel are separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry { pub id: usize, pub class: i32 }
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Order { entries: Vec<Entry>, dirty: bool }
impl Order {
    pub(crate) fn entries(&self) -> &[Entry] { &self.entries }
    pub(crate) fn dirty(&self) -> bool { self.dirty }
    pub(crate) fn insert(&mut self, entry: Entry) -> Result<(), &'static str> {
        if self.entries.iter().any(|e|e.id==entry.id) { return Err("duplicate constraint identity"); }
        self.entries.push(entry); self.dirty=true; Ok(())
    }
    pub(crate) fn remove(&mut self, id: usize) -> Result<Entry, &'static str> {
        let at=self.entries.iter().rposition(|e|e.id==id).ok_or("unknown constraint identity")?;
        // Destruction shifts entries left, and does not dirty a clean vector.
        Ok(self.entries.remove(at))
    }
    pub(crate) fn reorder(&mut self) {
        let selected=self.entries.iter().rev().copied().filter(|e|e.class==0).collect::<Vec<_>>();
        // Temporary entries are gathered backwards, then visited backwards.
        // Swap removal changes the order of the retained nonzero entries.
        for entry in selected.iter().rev() {
            let at=self.entries.iter().rposition(|e|e.id==entry.id).unwrap();
            self.entries.swap_remove(at);
        }
        self.entries.extend(selected.into_iter().rev());
    }
    pub(crate) fn prepare_solve(&mut self) {
        if self.dirty { self.reorder(); self.dirty=false; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn data()->Value { serde_json::from_str(include_str!("../tools/fixtures/native-ivp-group-order.json")).unwrap() }
    fn check(order:&Order, expected:&Value) {
        assert_eq!(order.dirty(),expected["dirty"].as_u64().unwrap()!=0);
        assert_eq!(order.entries().iter().map(|e|e.id).collect::<Vec<_>>(),
            expected["order"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>());
        assert_eq!(order.entries().iter().map(|e|e.class).collect::<Vec<_>>(),
            expected["classes"].as_array().unwrap().iter().map(|v|v.as_i64().unwrap() as i32).collect::<Vec<_>>());
    }
    #[test]
    fn native_fixed_hinge_ragdoll_order_and_dirty_lifecycle_match_all_mixtures() {
        let data=data();let mut total=0;let mut changed=0;
        for case in data["cases"].as_array().unwrap() {
            let scenario=case["scenario"].as_u64().unwrap() as usize;
            let mut order=Order::default();
            for i in 0..6 { let id=if scenario%2==0{i}else{5-i};
                // Constructor choice is authored, not read from native output.
                let class=if scenario&(1<<id)!=0 {0} else if id%2==1 {1} else {3};
                order.insert(Entry{id,class}).unwrap();
            }
            check(&order,&case["before"]);let before=order.entries.clone();
            order.reorder();check(&order,&case["after"]);changed+=usize::from(order.entries!=before);
            order.reorder();check(&order,&case["repeat"]);
            let removed=scenario%6;order.remove(removed).unwrap();check(&order,&case["removed"]);
            let replacement=Entry{id:removed,class:if scenario&(1<<removed)!=0{3}else{0}};
            order.insert(replacement).unwrap();check(&order,&case["added"]);
            order.reorder();check(&order,&case["resorted"]);
            order.prepare_solve();check(&order,&case["stepped"]);
            let second=(scenario+3)%6;order.remove(second).unwrap();check(&order,&case["clean_removed"]);
            order.insert(Entry{id:second,class:if scenario&(1<<second)!=0{3}else{0}}).unwrap();check(&order,&case["clean_added"]);
            order.prepare_solve();check(&order,&case["final"]);total+=1;
        }
        assert_eq!((total,changed),(64,52));
    }
    #[test]
    fn invalid_constraint_lifecycle_requests_leave_order_and_dirty_state_unchanged() {
        let mut order=Order::default();order.insert(Entry{id:7,class:0}).unwrap();order.prepare_solve();
        let before=order.clone();assert!(order.insert(Entry{id:7,class:3}).is_err());assert_eq!(order,before);
        assert!(order.remove(123).is_err());assert_eq!(order,before);
        order.remove(7).unwrap();assert!(!order.dirty());order.prepare_solve();assert!(order.entries().is_empty());
    }
}
