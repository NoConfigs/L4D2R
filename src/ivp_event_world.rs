//! Owned event-clock/PSI dispatch for100fb1a0 and100fb360. Environment dynamics
//! run through the caller's prediction callback, before the next PSI is queued.
//! The diagnostic compact-pair world and playable solver are not connected yet.
use crate::ivp_event_queue::Queue;

/// Shared PSI callback phases; the caller removes the PSI owner before entry.
pub(crate) fn begin_prediction(queue:&mut Queue,now:f64,dt:f32)->Result<f64,&'static str>{
    if !dt.is_finite()||dt<=0.{return Err("invalid PSI timestep");}
    queue.advance_epoch(now)?;Ok(now+dt as f64)
}
pub(crate) fn end_prediction(queue:&mut Queue,owner:usize,deadline:f64)->Result<(),&'static str>{
    queue.insert(owner,(deadline-queue.epoch) as f32)?;Ok(())
}

#[derive(Clone,Debug)]
pub(crate) struct World{
    pub queue:Queue,
    pub now:f64,
    pub start:f64,
    pub deadline:f64,
    pub clock_serial:u32,
    dt:f32,
    psi_owner:usize,
    fixed_step:bool,
}
impl World{
    pub(crate) fn timestep(&self)->f32{self.dt}
    pub(crate) fn prediction_owner(&self)->usize{self.psi_owner}
    pub(crate) fn new(psi_owner:usize)->Result<Self,&'static str>{
        let dt=1./66.;let mut queue=Queue::new(16,0.)?;
        queue.insert(psi_owner,-0.)?;
        Ok(Self{queue,now:0.,start:0.,deadline:dt as f64,clock_serial:1,dt,psi_owner,fixed_step:true})
    }
    pub(crate) fn set_timestep(&mut self,dt:f32)->Result<(),&'static str>{
        if !dt.is_finite()||dt<=0.{return Err("invalid environment timestep");}
        // The native setter does not change the already authored deadline.
        self.dt=dt;Ok(())
    }
    ///100f9c20 rebases the queue and environment around the PSI start, not
    /// around the latest public time. The caller must also rebase body/cache
    /// predictor timestamps by the returned shift (native100eadc0).
    pub(crate) fn reset_time(&mut self)->Result<f64,&'static str>{
        self.reset_time_with(|_,_|{})
    }
    /// Native100eadc0 runs between queue rebasing and the environment's clock
    /// serial update. The owner supplies registered body/controller traversal.
    pub(crate) fn reset_time_with(&mut self,rebase:impl FnOnce(&mut Self,f64))->Result<f64,&'static str>{
        let shift=self.start;self.queue.reset_time(shift)?;rebase(self,shift);
        let current_start=self.start;
        self.clock_serial=self.clock_serial.wrapping_add(1);self.start=0.;
        self.now-=current_start;self.deadline=self.dt as f64;Ok(shift)
    }
    ///100f99f0 advances the serial even when the absolute time is unchanged.
    fn set_time(&mut self,time:f64){self.clock_serial=self.clock_serial.wrapping_add(1);self.now=time;}
    ///1008d440 selects100f99b0 while frame and PSI deltas match. Once a
    /// variable step selects100f9970, that wrapper's fixed-step flag clears.
    pub(crate) fn simulate(&mut self,delta:f32,
        mut predict:impl FnMut(&mut Queue,f64,f64)->Result<(),&'static str>,
        event:impl FnMut(&mut Queue,usize,f64)->Result<(),&'static str>,
        observe:impl FnMut(&Self,usize))->Result<(),&'static str>{
        self.simulate_serial(delta, |q, now, deadline, _| predict(q, now, deadline), event, observe)
    }
    /// Serial is the real environment generation inside the PSI callback.
    pub(crate) fn simulate_serial(&mut self,delta:f32,
        predict:impl FnMut(&mut Queue,f64,f64,u32)->Result<(),&'static str>,
        mut event:impl FnMut(&mut Queue,usize,f64)->Result<(),&'static str>,
        observe:impl FnMut(&Self,usize))->Result<(),&'static str>{
        self.simulate_callbacks(delta,predict,|q,id,at,_,_|event(q,id,at),observe)
    }
    /// Actual generation and PSI deadline at both prediction and collision dispatch.
    pub(crate) fn simulate_callbacks(&mut self,mut delta:f32,
        predict:impl FnMut(&mut Queue,f64,f64,u32)->Result<(),&'static str>,
        event:impl FnMut(&mut Queue,usize,f64,u32,f64)->Result<(),&'static str>,
        mut observe:impl FnMut(&Self,usize))->Result<(),&'static str>{
        if !(delta<=1. && delta as f64>0.0001){observe(self,20);return Ok(());}
        if delta as f64>0.1{delta=0.1;}
        let target=if self.fixed_step&&delta==self.dt{
            self.start+self.dt as f64*f32::from_bits(0x3ffeb852) as f64
        }else{self.fixed_step=false;self.now+delta as f64};
        self.advance_callbacks(target,predict,event,observe)
    }
    pub(crate) fn advance(&mut self,end:f64,
        mut predict:impl FnMut(&mut Queue,f64,f64)->Result<(),&'static str>,
        event:impl FnMut(&mut Queue,usize,f64)->Result<(),&'static str>,
        observe:impl FnMut(&Self,usize))->Result<(),&'static str>{
        self.advance_serial(end, |q, now, deadline, _| predict(q, now, deadline), event, observe)
    }
    pub(crate) fn advance_serial(&mut self,end:f64,
        predict:impl FnMut(&mut Queue,f64,f64,u32)->Result<(),&'static str>,
        mut event:impl FnMut(&mut Queue,usize,f64)->Result<(),&'static str>,
        observe:impl FnMut(&Self,usize))->Result<(),&'static str>{
        self.advance_callbacks(end,predict,|q,id,at,_,_|event(q,id,at),observe)
    }
    pub(crate) fn advance_callbacks(&mut self,end:f64,
        mut predict:impl FnMut(&mut Queue,f64,f64,u32)->Result<(),&'static str>,
        mut event:impl FnMut(&mut Queue,usize,f64,u32,f64)->Result<(),&'static str>,
        mut observe:impl FnMut(&Self,usize))->Result<(),&'static str>{
        while let Some((owner,time))=self.queue.pop_before(end)?{
            self.set_time(time);
            if owner==self.psi_owner{
                observe(self,0);
                self.start=self.now;self.deadline=begin_prediction(&mut self.queue,self.start,self.dt)?;observe(self,1);
                predict(&mut self.queue,self.start,self.deadline,self.clock_serial)?;observe(self,2);
                end_prediction(&mut self.queue,self.psi_owner,self.deadline)?;observe(self,3);
            }else{
                event(&mut self.queue,owner,self.now,self.clock_serial,self.deadline)?;observe(self,100+owner);
            }
        }
        self.set_time(end);observe(self,20);Ok(())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn state(w:&World,phase:usize,step:usize)->serde_json::Value{
        let mut state=w.queue.snapshot();let s=state.as_object_mut().unwrap();
        s.insert("phase".into(),phase.into());s.insert("step".into(),step.into());
        s.insert("now".into(),w.now.into());s.insert("start".into(),w.start.into());s.insert("end".into(),w.deadline.into());state
    }
    #[test]
    fn callback_serials_match_each_live_psi_in_multi_tick_frames(){
        use std::cell::RefCell;
        let mut w=World::new(1000).unwrap();w.set_timestep(1./60.).unwrap();
        let live=RefCell::new(Vec::new());let mut callbacks=vec![];
        w.simulate_serial(0.1,|q,now,deadline,serial|{
            assert!(q.scheduled(1000).is_none(),"PSI must not be reinserted yet");
            callbacks.push((now.to_bits(),deadline.to_bits(),serial));Ok(())
        },|_,_,_|Err("unexpected user event"),|w,phase|{
            if phase==1{live.borrow_mut().push((w.start.to_bits(),w.deadline.to_bits(),w.clock_serial));}
        }).unwrap();
        assert!(callbacks.len()>1);assert_eq!(callbacks,*live.borrow());
        assert_ne!(callbacks.last().unwrap().2,w.clock_serial,"public exit increments generation again");
    }
    #[test]
    fn native_psi_and_user_events_share_clock_handles_and_nonempty_epoch_shifts(){
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-psi-queue.json")).unwrap();
        let mut snapshots=0;
        for case in fixture["cases"].as_array().unwrap(){
            let mut w=World::new(1000).unwrap();w.set_timestep(f32::from_bits(case["physics_dt_bits"].as_u64().unwrap() as u32)).unwrap();
            for (i,bits) in fixture["user_offset_bits"].as_array().unwrap().iter().enumerate(){w.queue.insert(i+1,f32::from_bits(bits.as_u64().unwrap() as u32)).unwrap();}
            let mut actual=vec![state(&w,10,0)];let render_dt=f32::from_bits(fixture["render_dt_bits"].as_u64().unwrap() as u32);
            let mut callbacks=vec![];
            for step in 0..18{
                w.simulate(render_dt,|_,_,_|Ok(()),|_,owner,time|{callbacks.push((owner,time));Ok(())},|world,phase|actual.push(state(world,phase,step))).unwrap();
            }
            let expected=case["snapshots"].as_array().unwrap();assert_eq!(actual.len(),expected.len());
            for (i,(a,b)) in actual.iter().zip(expected).enumerate(){
                assert_eq!(a,b,"{} snapshot{i}",case["name"]);
                // Compare double bits explicitly; JSON numeric equality alone
                // should not establish exact clock parity.
                for key in ["now","start","end","epoch"]{assert_eq!(a[key].as_f64().unwrap().to_bits(),b[key].as_f64().unwrap().to_bits());}
            }
            assert_eq!(callbacks.len(),8);snapshots+=actual.len();
        }
        assert_eq!(snapshots,444);
    }
    #[test]
    fn native_mixed_frame_deltas_time_resets_and_timestep_changes_match(){
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-psi-variable.json")).unwrap();
        let mut total=0;
        for case in fixture["cases"].as_array().unwrap(){
            let mut w=World::new(1000).unwrap();w.set_timestep(f32::from_bits(case["physics_dt_bits"].as_u64().unwrap() as u32)).unwrap();
            for (i,bits) in fixture["user_offset_bits"].as_array().unwrap().iter().enumerate(){w.queue.insert(i+1,f32::from_bits(bits.as_u64().unwrap() as u32)).unwrap();}
            let capture=|world:&World,phase:usize,step:usize|{let mut v=state(world,phase,step);v["physics_dt_bits"]=world.dt.to_bits().into();v["clock_serial"]=world.clock_serial.into();v};
            let mut actual=vec![capture(&w,10,0)];let mut callbacks=vec![];
            for (step,bits) in case["frame_delta_bits"].as_array().unwrap().iter().enumerate(){
                let delta=f32::from_bits(bits.as_u64().unwrap() as u32);
                w.simulate(delta,|_,_,_|Ok(()),|_,owner,time|{callbacks.push((owner,time));Ok(())},|world,phase|actual.push(capture(world,phase,step))).unwrap();
                for action in case["actions"].as_array().unwrap().iter().filter(|a|a["step"].as_u64().unwrap() as usize==step){
                    match action["op"].as_str().unwrap(){
                        "reset"=>{w.reset_time().unwrap();actual.push(capture(&w,30,step));},
                        "timestep"=>{w.set_timestep(f32::from_bits(action["bits"].as_u64().unwrap() as u32)).unwrap();actual.push(capture(&w,40,step));},
                        _=>panic!("unknown authored action"),
                    }
                }
            }
            let expected=case["snapshots"].as_array().unwrap();assert_eq!(actual.len(),expected.len(),"{}",case["name"]);
            for (i,(a,b)) in actual.iter().zip(expected).enumerate(){
                assert_eq!(a,b,"{} snapshot{i}",case["name"]);
                for key in ["now","start","end","epoch"]{assert_eq!(a[key].as_f64().unwrap().to_bits(),b[key].as_f64().unwrap().to_bits());}
            }
            assert_eq!(callbacks.len(),8);total+=actual.len();
        }
        assert_eq!(total,577);
    }
}
