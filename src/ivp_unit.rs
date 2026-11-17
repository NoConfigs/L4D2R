//! Owned100eb7d0 flag arithmetic and100f9770 timestep scalars. This does
//! not implement unit lists, movement-clock reset, graph rebuilding or sleeping.
use bevy::prelude::*;
#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct UnitFlags {pub flags:u32}
#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct Classification {pub fast:bool,pub intermediate:u32,pub reset_movement_clock:bool}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Timestep {pub dt:f32,pub inverse_dt:f32,pub work_decay:f32}
impl UnitFlags {
    pub(crate) fn dormant()->Self{Self{flags:8}}
    /// Flag portion of100eab20. The caller owns activation eligibility/lists.
    pub(crate) fn activate(&mut self){if self.flags&0xff==8{self.flags=(self.flags&!0xff)|1;}}
    /// Caller must commit pending velocities before classifying the unit.
    /// The native accumulator ORs FLOAT BITS, not numeric float-to-int casts.
    pub(crate) fn classify(&mut self,committed_linear:&[Vec3],one:f32)->Result<Classification,&'static str>{
        if committed_linear.is_empty()||!one.is_finite()||committed_linear.iter().any(|v|!v.is_finite()){return Err("invalid unit speed inputs");}
        let mut bits=0u32;
        for v in committed_linear {let square=(v.y*v.y+v.x*v.x)+v.z*v.z;bits|=(one-square).to_bits();}
        let fast=bits&0x80000000!=0;
        let intermediate=if fast{(self.flags&!0x800)|0x400}else{(((self.flags<<2)^self.flags)&0x3000)^self.flags};
        let reset_movement_clock=!fast&&intermediate&0x3000!=0;
        self.flags=if fast{intermediate}else{intermediate&!0xc00};
        Ok(Classification{fast,intermediate,reset_movement_clock})
    }
    ///100ff090/100ffec0 mark; graph editing is a separate caller operation.
    pub(crate) fn contact_removed(&mut self){self.flags=(self.flags&!0x200)|0x100;}
    ///100eb7d0 clears these bits AFTER the caller rebuilds/splits the graph.
    pub(crate) fn graph_finished(&mut self){self.flags&=!0x300;}
}
pub(crate) fn timestep(dt:f32,base:f64)->Result<Timestep,&'static str>{
    if !dt.is_finite()||dt<=0.||!base.is_finite()||base<=0.{return Err("invalid native timestep input");}
    let inverse_dt=1./dt;let work_decay=base.powf(dt as f64) as f32;
    if !inverse_dt.is_finite()||!work_decay.is_finite(){return Err("native timestep overflow");}
    Ok(Timestep{dt,inverse_dt,work_decay})
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn corpus()->Value{serde_json::from_str(include_str!("../tools/fixtures/native-ivp-unit-state.json")).unwrap()}
    fn u(v:&Value)->u32{v.as_u64().unwrap() as u32}
    fn f(v:&Value)->f32{v.as_f64().unwrap() as f32}
    #[test]
    fn native_unit_writer_streams_retain_source_flags(){
        let d=corpus();let (mut fast,mut slow,mut removed,mut finished,mut bound)=(0,0,0,0,0);
        for p in d["profiles"].as_array().unwrap(){
            let mut state=UnitFlags{flags:u(&p["bindings"][0]["value"])};let writes=p["writes"].as_array().unwrap();let mut i=0;
            loop {
                for b in p["bindings"].as_array().unwrap().iter().filter(|b|b["first_write"].as_u64().unwrap() as usize==i){assert_eq!(state.flags,u(&b["value"]));bound+=1;}
                if i==writes.len(){break;}
                let w=&writes[i];assert_eq!(state.flags,u(&w["before"]));
                match u(&w["after_rva"]) {
                    0xeba20|0xeba36=>{
                        let v=w["linear"].as_array().unwrap().iter().map(|v|Vec3::new(f(&v[0]),f(&v[1]),f(&v[2]))).collect::<Vec<_>>();
                        let result=state.classify(&v,1.).unwrap();assert_eq!(result.intermediate,u(&w["after"]));
                        assert_eq!(result.fast,u(&w["after_rva"])==0xeba20);
                        if result.fast{fast+=1;}else{slow+=1;assert_eq!(result.reset_movement_clock,result.intermediate&0x3000!=0);i+=1;assert_eq!(u(&writes[i]["after_rva"]),0xeba58);assert_eq!(result.intermediate,u(&writes[i]["before"]));}
                    }
                    0xff162|0xfff15=>{state.contact_removed();removed+=1;}
                    0xebb86=>{state.graph_finished();finished+=1;}
                    _=>panic!("unknown flag producer"),
                }
                assert_eq!(state.flags,u(&writes[i]["after"]));i+=1;
            }
        }
        assert_eq!((fast,slow,removed,finished,bound),(29,169,8,4,187));
    }
    #[test]
    fn native_timestep_decay_controls_match_bits(){
        let d=corpus();let mut count=0;
        for p in d["profiles"].as_array().unwrap(){for c in p["decay_controls"].as_array().unwrap(){let t=timestep(f(&c["dt"]),p["decay_base"].as_f64().unwrap()).unwrap();
            for (x,y) in [(t.dt,f(&c["stored_dt"])),(t.inverse_dt,f(&c["inverse_dt"])),(t.work_decay,f(&c["coefficient"]))]{assert_eq!(x.to_bits(),y.to_bits());}count+=1;}}
        assert_eq!(count,384);
    }
    #[test]
    fn speed_boundary_transition_and_invalid_input_are_preserved(){
        let mut s=UnitFlags::dormant();s.activate();assert_eq!(s.flags,1);
        assert!(s.classify(&[Vec3::new(f32::from_bits(1f32.to_bits()+1),0.,0.)],1.).unwrap().fast);assert_eq!(s.flags,0x401);
        let r=s.classify(&[Vec3::X],1.).unwrap();assert!(!r.fast&&r.reset_movement_clock);assert_eq!(s.flags,0x1001);
        s.classify(&[Vec3::ZERO],1.).unwrap();assert_eq!(s.flags,1);
        let before=s;assert!(s.classify(&[Vec3::splat(f32::NAN)],1.).is_err());assert_eq!(s,before);assert!(s.classify(&[],1.).is_err());assert_eq!(s,before);
        assert!(timestep(0.,0.9).is_err());assert!(timestep(f32::MIN_POSITIVE/1024.,0.9).is_err());
        // One fast member determines the whole unit, regardless of order.
        for v in [[Vec3::ZERO,Vec3::splat(2.)],[Vec3::splat(2.),Vec3::ZERO]]{assert!(s.classify(&v,1.).unwrap().fast);}
    }
}
