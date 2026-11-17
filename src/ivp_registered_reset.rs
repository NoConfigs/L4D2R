//! Registered core/object reset traversal selected by source-owned unit lists.
//! Controller reset hooks and object destruction/cache reclamation are separate.
use std::collections::BTreeMap;
use crate::{ivp_event_world::World,ivp_unit_registry::Registry,
    ivp_world_cache::WorldCache,ivp_motion::{Prediction,ObjectFrame},ivp_hull_budget::Budget};

pub(crate) struct Bodies {
    pub cache:WorldCache,
    cores:BTreeMap<usize,usize>,
    pub budgets:Vec<Budget>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_multi_object_reset_preserves_core_order_and_rejects_missing_bindings_before_mutation(){
        let mut units=Registry::new(1000);
        for id in [90,6]{units.register_core(id,false).unwrap();units.wake(id).unwrap();}
        units.begin_psi().unwrap();assert_eq!(units.active_reset_cores(),[90,6]);
        let mut world=World::new(999).unwrap();
        world.simulate(1./66.,|_,_,_|Ok(()),|_,_,_|Ok(()),|_,_|{}).unwrap();
        let before=(world.now,world.start,world.clock_serial,world.queue.snapshot());
        let mut bodies=Bodies::new(8).unwrap();
        assert!(bodies.reset_time(&units,&mut world).is_err());
        assert_eq!((world.now,world.start,world.clock_serial,world.queue.snapshot()),before);
        let p=Prediction{start:bevy::prelude::Vec3::ZERO,velocity:bevy::prelude::Vec3::ZERO,
            rotation_start:[0.,0.,0.,1.],rotation_end:[0.,0.,0.,1.],time:7.,inverse_dt:66.};
        for id in [6,90]{bodies.register_core(id,p).unwrap();}
        assert!(bodies.register_core(90,p).is_err());
        for (i,owner) in [90,6,90].into_iter().enumerate(){
            let budget=Budget{time:8.+i as f64*0.25,rates:[1.,2.],accumulated:[3.,4.],predicted:5.,threshold:6.};
            assert_eq!(bodies.register_object(owner,0x101,ObjectFrame{shift:None,rotation:None},budget).unwrap(),i);
        }
        let shift=world.start;
        assert_eq!(bodies.reset_time(&units,&mut world).unwrap(),[2,0,1]);
        for object in 0..3 {
            assert_eq!(bodies.cache.object_inputs(object).unwrap().0.time.to_bits(),(7.-shift).to_bits());
            assert_eq!(bodies.budgets[object].time.to_bits(),(8.+object as f64*0.25-shift).to_bits());
        }
    }
}
impl Bodies {
    pub(crate) fn new(capacity:usize)->Result<Self,&'static str>{
        Ok(Self{cache:WorldCache::new(capacity)?,cores:BTreeMap::new(),budgets:vec![]})
    }
    pub(crate) fn register_core(&mut self,owner:usize,prediction:Prediction)->Result<(),&'static str>{
        if self.cores.contains_key(&owner){return Err("duplicate registered reset core");}
        let cache=self.cache.register_core(prediction);self.cores.insert(owner,cache);Ok(())
    }
    pub(crate) fn register_object(&mut self,owner:usize,flags:u32,frame:ObjectFrame,budget:Budget)->Result<usize,&'static str>{
        let core=*self.cores.get(&owner).ok_or("unknown registered reset core")?;
        let id=self.cache.register_object(core,flags,frame)?;
        assert_eq!(id,self.budgets.len());self.budgets.push(budget);Ok(id)
    }
    pub(crate) fn reset_time(&mut self,units:&Registry,world:&mut World)->Result<Vec<usize>,&'static str>{
        // Resolve the entire traversal before queue/clock mutation. Core IDs in
        // the unit owner need not equal dense cache IDs or object indices.
        let selected=units.active_reset_cores().into_iter().map(|id|
            self.cores.get(&id).copied().ok_or("active unit lacks a registered predictor")
        ).collect::<Result<Vec<_>,_>>()?;
        let mut objects=vec![];
        world.reset_time_with(|_,shift|{
            //100eadc0 traverses units forward, then cores in reverse. A core's
            // predictor and attached hull clocks are rebased before the next.
            for core in selected {
                let owned=self.cache.rebase_core_times(&[core],shift).expect("registered core lost cache ownership");
                for object in owned{self.budgets[object].rebase_time(shift);objects.push(object);}
            }
        })?;
        Ok(objects)
    }
}
