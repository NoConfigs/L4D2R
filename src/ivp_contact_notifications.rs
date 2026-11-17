//!100fa4a0/100fa4e0 registration and100f9e70..100f9f30 delivery selection.
//! Deliveries are immutable, deferred observations. Invoking callbacks which
//! mutate the physics environment during dispatch is deliberately unsupported.
use std::collections::BTreeMap;

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum Kind { Created, Deleted, PairAdded, PairRemoved }
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct Event {pub kind:Kind,pub contact:Option<usize>,pub cores:[usize;2]}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct Delivery {pub listener:usize,pub event:Event}
#[derive(Clone,Debug,Default)]
pub(crate) struct Notifications {
    // Native registration permits the same observer pointer more than once.
    order:Vec<usize>,flags:BTreeMap<usize,u32>,pending:Vec<Delivery>,
}
impl Notifications {
    pub(crate) fn subscribe(&mut self,id:usize,flags:u32)->Result<(),&'static str>{
        if id==0||self.order.len()==u16::MAX as usize{return Err("invalid/full contact listener registry");}
        if self.flags.get(&id).is_some_and(|&f|f!=flags){return Err("registered observer flags differ; update flags explicitly");}
        self.flags.insert(id,flags);self.order.push(id);Ok(())
    }
    pub(crate) fn set_flags(&mut self,id:usize,flags:u32)->Result<(),&'static str>{
        *self.flags.get_mut(&id).ok_or("unknown contact listener")?=flags;Ok(())
    }
    pub(crate) fn unsubscribe(&mut self,id:usize)->Result<(),&'static str>{
        // Last matching registration, stable erase. Native missing-identity
        // deletion is invalid; reject it without changing the source owner.
        let i=self.order.iter().rposition(|&v|v==id).ok_or("unknown contact listener")?;
        self.order.remove(i);if !self.order.contains(&id){self.flags.remove(&id);}Ok(())
    }
    pub(crate) fn publish(&mut self,event:Event){
        for &listener in self.order.iter().rev(){if self.flags[&listener]&4!=0{self.pending.push(Delivery{listener,event});}}
    }
    pub(crate) fn take(&mut self)->Vec<Delivery>{std::mem::take(&mut self.pending)}
}
