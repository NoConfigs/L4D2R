//! Owned10118210/101184e0 entry lifetime,1010edc0 rebase and1010eea0 dispatch.
//! Geometry listeners and world membership are caller-owned. This manager does
//! not supply captured thresholds or silently cap native callback iteration.
use crate::ivp_hull_budget::Budget;
const NONE: u16 = 0xffff;
const NOT_ANCHOR: u16 = 0xfffe;
const EMPTY: f32 = 1e10;

#[derive(Clone, Debug)]
struct Entry {
    skip_next: u16, skip_previous: u16, next: u16, previous: u16,
    key: f32, owner: usize, live: bool,
}
impl Default for Entry {
    fn default() -> Self {
        Self { skip_next: NOT_ANCHOR, skip_previous: NONE, next: NONE, previous: NONE,
            key: 0., owner: 0, live: false }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Notice { pub owner: usize, pub handle: u16, pub a: f32, pub b: f32 }
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Stop { Satisfied, Quota }
/// Shared callback quota for a manager or a world borrowing that manager.
pub(crate) struct Quota {allowance:i32,remaining:i32}
impl Quota {
    pub(crate) fn new(initial:i32)->Self{Self{allowance:initial,remaining:initial}}
    pub(crate) fn after(&mut self,mut refill:impl FnMut(i32)->i32)->bool{
        self.remaining=self.remaining.wrapping_sub(1);
        if self.remaining<0{
            self.remaining=self.remaining.wrapping_add(refill(self.allowance));
            if self.remaining<0{return false;}
            self.allowance=self.allowance.wrapping_add(1).wrapping_add(self.remaining);
        }true
    }
}
/// Cursor keeps the native next link before each callback mutates ownership.
pub(crate) struct Rebase {next:u16,motion:f32,linear:f32}
impl Rebase {
    pub(crate) fn next(&mut self,m:&mut Manager)->Option<Notice>{
        if self.next==NONE{return None;}
        let at=self.next;let e=&mut m.entries[at as usize];self.next=e.next;e.key+=self.motion;
        Some(Notice{owner:e.owner,handle:at,a:self.motion,b:self.linear})
    }
    pub(crate) fn finish(self,m:&mut Manager){
        m.budget.threshold+=self.motion;m.budget.predicted+=self.motion;m.budget.accumulated=[0.;2];
    }
}

#[derive(Clone,Debug)]
pub(crate) struct Manager {
    pub budget: Budget,
    entries: Vec<Entry>, free: u16, head: u16, skip_head: u16, count: usize,
}
impl Manager {
    pub(crate) fn due(&self)->Result<Option<Notice>,&'static str>{
        let gap=self.budget.threshold-self.budget.predicted;
        if gap>=0.{return Ok(None);}
        let e=self.entries.get(self.head as usize).filter(|e|e.live).ok_or("empty hull manager exceeded sentinel")?;
        Ok(Some(Notice{owner:e.owner,handle:self.head,a:gap,b:0.}))
    }
    pub(crate) fn begin_rebase(&self)->Rebase{
        Rebase{next:self.head,motion:f32::from_bits(self.budget.accumulated[0].to_bits()^0x80000000),linear:f32::from_bits(self.budget.accumulated[1].to_bits()^0x80000000)}
    }
    /// Shared10118210 list head, also used by the native time manager.
    pub(crate) fn first(&self) -> Option<(u16,usize,f32)> {
        self.entries.get(self.head as usize).filter(|e|e.live).map(|e|(self.head,e.owner,e.key))
    }
    pub(crate) fn entry(&self,handle:u16)->Option<(usize,f32)>{
        self.entries.get(handle as usize).filter(|e|e.live).map(|e|(e.owner,e.key))
    }
    ///100fb130 shifts live time entries and the empty-list sentinel alike.
    pub(crate) fn subtract_keys(&mut self, amount:f32) {
        let mut at=self.head;
        while at!=NONE {let e=&mut self.entries[at as usize];e.key-=amount;at=e.next;}
        self.budget.threshold-=amount;
    }
    #[cfg(test)]
    pub(crate) fn ordered_keys(&self)->Vec<(u16,usize,u32)>{
        let mut out=vec![];let mut at=self.head;
        while at!=NONE{let e=&self.entries[at as usize];out.push((at,e.owner,e.key.to_bits()));at=e.next;}
        out
    }
    #[cfg(test)]
    pub(crate) fn capacity(&self)->usize{self.entries.len()}
    pub(crate) fn new(capacity: usize) -> Result<Self, &'static str> {
        // Native constructor writes the last free link even for capacity0;
        // reject that invalid memory domain rather than inventing an empty pool.
        if capacity == 0 || capacity > 0xfffc { return Err("invalid hull entry capacity"); }
        let mut entries = vec![Entry::default(); capacity];
        for (i, e) in entries.iter_mut().enumerate() { e.next = if i+1<capacity { (i+1) as u16 } else { NONE }; }
        Ok(Self { budget: Budget { time: 0., rates: [0.;2], accumulated: [0.;2], predicted: 0., threshold: EMPTY },
            entries, free: 0, head: NONE, skip_head: NONE, count: 0 })
    }
    pub(crate) fn insert(&mut self, owner: usize, key: f32) -> Result<u16, &'static str> {
        if !key.is_finite() { return Err("nonfinite hull threshold"); }
        let id = if self.free != NONE {
            let id = self.free; self.free = self.entries[id as usize].next; id
        } else {
            let old = self.entries.len(); let size = (old*2+1).min(0xfffc);
            if size <= old+1 { return Err("exhausted native hull handle domain"); }
            self.entries.resize(size, Entry::default());
            for i in old+1..size { self.entries[i].next = if i+1<size { (i+1) as u16 } else { NONE }; }
            self.free = (old+1) as u16; old as u16
        };
        self.count += 1;
        let e = &mut self.entries[id as usize];
        e.key = key; e.owner = owner; e.skip_next = NOT_ANCHOR; e.live = true;
        if key <= self.budget.threshold {
            e.next = self.head; e.previous = NONE;
            if self.head != NONE { self.entries[self.head as usize].previous = id; }
            self.head = id; self.budget.threshold = key; return Ok(id);
        }
        let mut search_budget = 3usize;
        let mut anchor = self.head;
        let mut at = self.skip_head;
        while at != NONE {
            if key <= self.entries[at as usize].key { break; }
            search_budget += 1; anchor = at; at = self.entries[at as usize].skip_next;
        }
        let mut previous = anchor;
        let mut next = self.entries[anchor as usize].next;
        let mut walked = 0usize;
        loop {
            if next == NONE { break; }
            walked += 1;
            if key <= self.entries[next as usize].key { break; }
            previous = next; next = self.entries[next as usize].next;
        }
        self.entries[id as usize].next = next; self.entries[id as usize].previous = previous;
        self.entries[previous as usize].next = id;
        if next != NONE { self.entries[next as usize].previous = id; }
        if search_budget < walked {
            let mut promoted = anchor;
            for _ in 0..search_budget-2 { promoted = self.entries[promoted as usize].next; }
            let old = self.entries[anchor as usize].skip_next;
            if old != NOT_ANCHOR {
                self.entries[promoted as usize].skip_next = old;
                self.entries[promoted as usize].skip_previous = anchor;
                if old != NONE { self.entries[old as usize].skip_previous = promoted; }
                self.entries[anchor as usize].skip_next = promoted;
            } else {
                self.entries[promoted as usize].skip_next = self.skip_head;
                self.entries[promoted as usize].skip_previous = NONE;
                if self.skip_head != NONE { self.entries[self.skip_head as usize].skip_previous = promoted; }
                self.skip_head = promoted;
            }
        }
        Ok(id)
    }
    pub(crate) fn remove(&mut self, id: u16) -> Result<usize, &'static str> {
        let e = self.entries.get(id as usize).filter(|e| e.live).ok_or("unknown hull handle")?.clone();
        if e.previous == NONE {
            self.head = e.next;
            self.budget.threshold = if e.next == NONE { EMPTY } else { self.entries[e.next as usize].key };
            if e.next != NONE { self.entries[e.next as usize].previous = NONE; }
        } else {
            self.entries[e.previous as usize].next = e.next;
            if e.next != NONE { self.entries[e.next as usize].previous = e.previous; }
        }
        if e.skip_next != NOT_ANCHOR {
            if e.skip_next != NONE { self.entries[e.skip_next as usize].skip_previous = e.skip_previous; }
            if e.skip_previous == NONE { self.skip_head = e.skip_next; }
            else { self.entries[e.skip_previous as usize].skip_next = e.skip_next; }
        }
        self.count -= 1; self.entries[id as usize].next = self.free; self.entries[id as usize].live = false; self.free = id;
        Ok(e.owner)
    }
    pub(crate) fn replace(&mut self, id: u16, key: f32) -> Result<u16, &'static str> {
        if !key.is_finite() { return Err("nonfinite hull threshold"); }
        let owner = self.remove(id)?; self.insert(owner,key)
    }
    pub(crate) fn rebase(&mut self, mut notify: impl FnMut(&mut Self, Notice)) {
        let mut cursor=self.begin_rebase();
        while let Some(notice)=cursor.next(self){notify(self,notice);}
        cursor.finish(self);
    }
    pub(crate) fn dispatch(&mut self, initial: i32,
        mut update: impl FnMut(&mut Self, Notice) -> Result<(), &'static str>,
        mut refill: impl FnMut(i32) -> i32) -> Result<Stop, &'static str> {
        let mut quota=Quota::new(initial);
        loop {
            let Some(notice)=self.due()? else{return Ok(Stop::Satisfied);};
            update(self,notice)?;
            // Native requests more quota after the callback, even if that
            // callback has just removed the final due entry.
            if !quota.after(&mut refill){return Ok(Stop::Quota);}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> serde_json::Value { serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hull-lifecycle.json")).unwrap() }
    fn state(m: &Manager) -> serde_json::Value {
        let mut entries = Vec::new(); let mut at = m.head;
        while at != NONE {
            let e = &m.entries[at as usize];
            entries.push(serde_json::json!({"id":at,"owner":e.owner,"key":e.key,"key_bits":e.key.to_bits(),"next":e.next,"previous":e.previous,"skip_next":e.skip_next,"skip_previous":if e.skip_next==NOT_ANCHOR{None}else{Some(e.skip_previous)}}));
            at = e.next;
        }
        let mut free=Vec::new();at=m.free;while at!=NONE{free.push(at);at=m.entries[at as usize].next;}
        serde_json::json!({"capacity":m.entries.len(),"head":m.head,"skip_head":m.skip_head,"count":m.count,"free_head":m.free,"minimum":m.budget.threshold,"minimum_bits":m.budget.threshold.to_bits(),"accumulated":m.budget.accumulated,"accumulated_bits":m.budget.accumulated.map(f32::to_bits),"predicted":m.budget.predicted,"predicted_bits":m.budget.predicted.to_bits(),"entries":entries,"free":free})
    }
    // Native decimals are display data. Compare their retained raw float bits
    // instead, including signed zero; all other defined fields compare as-is.
    fn exact(mut v:serde_json::Value)->serde_json::Value{
        if let Some(map)=v.as_object_mut(){for (bits,display) in [("key_bits","key"),("minimum_bits","minimum"),("accumulated_bits","accumulated"),("predicted_bits","predicted"),("a_bits","a"),("b_bits","b")]{if map.contains_key(bits){map.remove(display);}}for value in map.values_mut(){*value=exact(value.take());}}
        else if let Some(array)=v.as_array_mut(){for value in array{*value=exact(value.take());}}
        v
    }
    fn notice(v: Notice) -> serde_json::Value { serde_json::json!({"owner":v.owner,"handle":v.handle,"a":v.a,"b":v.b,"a_bits":v.a.to_bits(),"b_bits":v.b.to_bits()}) }
    #[test]
    fn owned_entry_lifetimes_skip_links_rebases_and_recycled_handles_match_without_reseeding() {
        let d=fixture();let mut events=0;let mut rebases=0;
        for c in d["hull_lifecycle_controls"].as_array().unwrap(){
            let mut m=Manager::new(c["initial_capacity"].as_u64().unwrap() as usize).unwrap();
            assert_eq!(exact(state(&m)),exact(c["initial"].clone()));
            let mut handles=std::collections::HashMap::new();
            for e in c["events"].as_array().unwrap(){
                let owner=e["owner"].as_u64().unwrap() as usize;
                let key=f32::from_bits(e["key_bits"].as_u64().unwrap() as u32);let mut notes=Vec::new();
                let result=match e["action"].as_u64().unwrap(){
                    0=>{let id=m.insert(owner,key).unwrap();handles.insert(owner,id);id},
                    1=>{let id=handles.remove(&owner).unwrap();assert_eq!(m.remove(id).unwrap(),owner);id},
                    2=>{let id=m.replace(handles[&owner],key).unwrap();handles.insert(owner,id);id},
                    3=>{m.budget.accumulated=std::array::from_fn(|i|e["motion"][i].as_f64().unwrap() as f32);m.budget.predicted=e["motion"][2].as_f64().unwrap() as f32;m.rebase(|_,v|notes.push(notice(v)));rebases+=1;NONE},
                    _=>panic!("unknown input operation"),
                };
                assert_eq!(result as u64,e["result"].as_u64().unwrap());
                assert_eq!(exact(state(&m)),exact(e["after"].clone()),"scenario{} step{}",c["scenario"],e["step"]);
                assert_eq!(exact(serde_json::json!(notes)),exact(e["notices"].clone()));events+=1;
            }
        }
        assert_eq!((events,rebases),(632,24));
    }
    #[test]
    fn owned_dispatch_quota_requests_callback_order_and_mutation_match_native() {
        let d=fixture();let mut calls=0;
        for c in d["hull_dispatch_controls"].as_array().unwrap(){
            let mut m=Manager::new(4).unwrap();m.budget.predicted=c["predicted"].as_f64().unwrap() as f32;
            let listeners=c["listeners"].as_array().unwrap();let mut next=Vec::new();
            for o in listeners{m.insert(o["owner"].as_u64().unwrap() as usize,o["key"].as_f64().unwrap() as f32).unwrap();next.push(o["next"].as_f64().unwrap() as f32);}
            assert_eq!(exact(state(&m)),exact(c["initial"].clone()));
            let mut notices=Vec::new();let mut refills=Vec::new();
            m.dispatch(c["allowance"].as_i64().unwrap() as i32,|m,v|{
                notices.push(notice(v));m.remove(v.handle)?;
                if listeners[v.owner]["replace"].as_bool().unwrap(){m.insert(v.owner,next[v.owner])?;next[v.owner]+=listeners[v.owner]["increment"].as_f64().unwrap() as f32;}Ok(())
            },|request|{let i=refills.len();refills.push(request);c["grants"][i.min(3)].as_i64().unwrap() as i32}).unwrap();
            calls+=notices.len();assert_eq!(exact(state(&m)),exact(c["after"].clone()),"scenario{}",c["scenario"]);
            assert_eq!(exact(serde_json::json!(notices)),exact(c["notices"].clone()));assert_eq!(serde_json::json!(refills),c["refills"]);
        }
        assert_eq!(calls,53);
    }
}
