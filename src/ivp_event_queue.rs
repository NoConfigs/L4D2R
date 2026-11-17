//! Owned100fb070 cancellation,100fb090 replacement and100fb130 time reset.
//! The environment owns callback dispatch, clock advancement and epoch changes.
//! This component is not yet the playable world's event manager.
use crate::ivp_hull_manager::Manager;
use std::collections::BTreeMap;

#[derive(Clone,Debug)]
pub(crate) struct Queue {
    entries:Manager,
    handles:BTreeMap<usize,u16>,
    pub epoch:f64,
    pub last_offset:f32,
}
impl Queue {
    pub(crate) fn new(capacity:usize,epoch:f64)->Result<Self,&'static str>{
        if !epoch.is_finite(){return Err("nonfinite event epoch");}
        Ok(Self{entries:Manager::new(capacity)?,handles:BTreeMap::new(),epoch,last_offset:0.})
    }
    pub(crate) fn insert(&mut self,owner:usize,offset:f32)->Result<u16,&'static str>{
        if self.handles.contains_key(&owner){return Err("event already queued");}
        let handle=self.entries.insert(owner,offset)?;self.handles.insert(owner,handle);Ok(handle)
    }
    pub(crate) fn cancel(&mut self,owner:usize)->Result<(),&'static str>{
        let handle=*self.handles.get(&owner).ok_or("event not queued")?;
        if self.entries.remove(handle)?!=owner{return Err("event handle owner mismatch");}
        self.handles.remove(&owner);Ok(())
    }
    pub(crate) fn replace(&mut self,owner:usize,absolute:f64)->Result<u16,&'static str>{
        let offset=(absolute-self.epoch) as f32;
        if !offset.is_finite(){return Err("nonfinite replacement time");}
        self.cancel(owner)?;self.insert(owner,offset)
    }
    /// Reset environment time, retaining list order even when float subtraction
    /// collapses distinct keys. Native100fb130 does not reinsert the entries.
    pub(crate) fn reset_time(&mut self,absolute:f64)->Result<(),&'static str>{
        let shift=(absolute-self.epoch) as f32;
        if !shift.is_finite(){return Err("nonfinite time reset");}
        self.entries.subtract_keys(shift);self.epoch=0.;self.last_offset=0.;Ok(())
    }
    pub(crate) fn first(&self)->Option<(usize,f64)>{
        self.entries.first().map(|(_,owner,offset)|(owner,self.epoch+offset as f64))
    }
    pub(crate) fn scheduled(&self,owner:usize)->Option<(f64,f32)>{
        let (actual,offset)=self.entries.entry(*self.handles.get(&owner)?)?;
        (actual==owner).then_some((self.epoch,offset))
    }
    /// Controlled PSI boundary, after all preceding pair entries were consumed.
    ///100fb360 subtracts the new epoch's float value before storing its double
    /// epoch. Nonempty boundaries require the full PSI callback owner.
    pub(crate) fn advance_empty_epoch(&mut self,epoch:f64)->Result<(),&'static str>{
        if self.first().is_some(){return Err("nonempty PSI boundary requires native callback ownership");}
        self.advance_epoch(epoch)
    }
    ///100fb360's PSI callback shifts live offsets in place. Deliberately use
    /// the new epoch's float value, not a mathematically substituted delta.
    pub(crate) fn advance_epoch(&mut self,epoch:f64)->Result<(),&'static str>{
        if !epoch.is_finite(){return Err("nonfinite PSI epoch");}
        self.entries.subtract_keys(epoch as f32);self.epoch=epoch;self.last_offset=0.;Ok(())
    }
    /// Caller has selected the native head and owns boundary/clock dispatch.
    pub(crate) fn pop(&mut self)->Result<Option<(usize,f64)>,&'static str>{
        let Some((_,owner,offset))=self.entries.first() else{return Ok(None)};
        self.cancel(owner)?;self.last_offset=offset;Ok(Some((owner,self.epoch+offset as f64)))
    }
    ///100fb1a0's strict cutoff: an entry exactly at the requested time waits.
    pub(crate) fn pop_before(&mut self,end:f64)->Result<Option<(usize,f64)>,&'static str>{
        if !end.is_finite(){return Err("nonfinite dispatch boundary");}
        let Some((_,owner,offset))=self.entries.first() else{return Ok(None)};
        if offset as f64>=end-self.epoch{return Ok(None);}
        // Instructions100fb1eb/1f3 add the double epoch and store a double;
        // the decompiler's float-cast pseudocode is not the native operation.
        let _=owner;self.pop()
    }
    #[cfg(test)]
    pub(crate) fn snapshot(&self)->serde_json::Value{
        serde_json::json!({"epoch":self.epoch,"last_bits":self.last_offset.to_bits(),"minimum_bits":self.entries.budget.threshold.to_bits(),
            "entries":self.entries.ordered_keys().into_iter().map(|(handle,owner,offset_bits)|serde_json::json!({"owner":owner,"handle":handle,"offset_bits":offset_bits})).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn native_event_handles_ties_replacements_and_time_resets_match(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-event-queue.json")).unwrap();
        let mut count=0;
        for scenario in data["scenarios"].as_array().unwrap(){
            let mut q=Queue::new(scenario["capacity"].as_u64().unwrap() as usize,scenario["epoch"].as_f64().unwrap()).unwrap();
            for op in scenario["operations"].as_array().unwrap(){
                let owner=op["owner"].as_u64().unwrap() as usize;let time=op["time"].as_f64().unwrap();
                match op["op"].as_str().unwrap(){
                    "insert"=>{q.insert(owner,f32::from_bits(op["input_offset_bits"].as_u64().unwrap() as u32)).unwrap();},
                    "replace"=>{q.replace(owner,time).unwrap();},
                    "cancel"=>q.cancel(owner).unwrap(),
                    "rebase"=>q.reset_time(time).unwrap(),
                    "pop"=>{let (_,at)=q.first().unwrap();assert_eq!(q.pop_before(at).unwrap(),None);assert_eq!(q.pop_before(at+1.).unwrap(),Some((owner,at)));},
                    _=>panic!("unknown native queue operation"),
                }
                let a=&op["after"];assert_eq!(q.epoch.to_bits(),a["epoch"].as_f64().unwrap().to_bits());
                assert_eq!(q.last_offset.to_bits(),a["last_offset_bits"].as_u64().unwrap() as u32);
                assert_eq!(q.entries.budget.threshold.to_bits(),a["minimum_bits"].as_u64().unwrap() as u32);
                assert_eq!(q.entries.capacity(),a["capacity"].as_u64().unwrap() as usize);
                let expected=a["entries"].as_array().unwrap().iter().map(|e|(e["handle"].as_u64().unwrap() as u16,e["owner"].as_u64().unwrap() as usize,e["offset_bits"].as_u64().unwrap() as u32)).collect::<Vec<_>>();
                assert_eq!(q.entries.ordered_keys(),expected,"operation{count}");
                assert_eq!(q.handles.len(),expected.len());
                for (handle,owner,_) in expected{assert_eq!(q.handles.get(&owner),Some(&handle));}
                count+=1;
            }
        }
        assert_eq!(count,484);
    }
}
