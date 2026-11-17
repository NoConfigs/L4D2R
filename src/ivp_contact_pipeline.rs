//! Source-owned refresh cache consumed by normal response. Graph discovery,
//! scheduling and deletion side effects remain separate stages.
use crate::{ivp_contact::Core,ivp_history::History,ivp_refresh as refresh,ivp_single as single};
use crate::{ivp_friction as friction,ivp_slip as slip};
use bevy::prelude::*;
#[cfg(test)]
thread_local!{pub(crate) static TANGENT_AUDIT:std::cell::RefCell<Option<Vec<serde_json::Value>>>=const{std::cell::RefCell::new(None)};}


#[derive(Clone,Copy)]
pub(crate) struct CoreFrame {pub rotation:Mat3,pub position:Vec3}
#[derive(Clone,Copy)]
pub(crate) struct FrictionMemory {pub slip:slip::State,pub work:f32}
pub(crate) struct TangentSettings {
    pub dt:f32,pub inverse_dt:f32,pub friction:f32,
    /// Persisted100fd5c0 contact mass bound (link+30), not the refreshed
    /// reciprocal response in the physical row. The creation provider owns it.
    pub response_bound:f32,
    pub threshold_extra:f32,pub minimum_impulse:f32,pub minimum_determinant_squared:f32,
    pub mode:u8,pub alternate_candidate:bool,
}

#[derive(Clone)]
pub(crate) struct PreparedContact {geometry:refresh::Output,axes:single::Axes}

/// State retained by one managed contact. A controller entering from an
/// unported batch path may supply a boundary snapshot, but subsequent refresh,
/// tangent and normal calls must carry this state rather than reseeding it.
#[derive(Clone,Copy)]
pub(crate) struct RetainedState {
    pub refresh:refresh::State,pub normal:History,
    pub energy:f32,pub work:f32,pub response_bound:f32,
}
#[derive(Clone)]
pub(crate) struct RetainedContact {
    state:RetainedState,owners:[Option<usize>;2],
    prepared:Option<PreparedContact>,retired:bool,
}
///100ff6b0 constructor and100ff020 post-batch countdown. The caller owns
/// membership and supplies unit flags for the selective refresh-phase clear.
#[derive(Clone,Copy,Debug)]
pub(crate) struct DistanceChange {pub before:f32,pub after:f32,pub force:f32}
#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct FrictionGroup {pub timer:i32,pub energy:f32}
impl Default for FrictionGroup {fn default()->Self{Self{timer:1,energy:0.}}}
impl FrictionGroup {
    ///10106460 accumulates distance-change work in reverse group-array order,
    /// then adds only a positive aggregate BEFORE flag clear/decay/dissipation.
    /// Refresh itself must already have supplied each new contact distance.
    pub(crate) fn geometry_work(&mut self,contacts:&[DistanceChange])->Result<f32,&'static str>{
        if !self.energy.is_finite()||contacts.is_empty()||contacts.iter().any(|r|![r.before,r.after,r.force].iter().all(|v|v.is_finite())){return Err("invalid geometry work state");}
        let mut delta=0.;for r in contacts.iter().rev(){delta=(r.before-r.after)*r.force+delta;}
        let energy=if delta>0.{self.energy+delta}else{self.energy};
        if !delta.is_finite()||!energy.is_finite(){return Err("geometry work overflow");}
        self.energy=energy;Ok(delta)
    }
    pub(crate) fn begin_update(&mut self,unit_flags:u32){if unit_flags&0x3000!=0{self.energy=0.;}}
    ///100ffa70/100ff9f0 refresh phase. Unit flag generation, blocked-core
    /// providers and the environment's decay/work storage remain caller-owned.
    /// Pair work changes pending impulses; ordinary raw velocities stay intact.
    pub(crate) fn refresh_work(&mut self,unit_flags:u32,coefficient:f32,blocked:[bool;2],bodies:&mut [crate::ivp_dissipation::Body;2],environment_work:&mut f32,k:&crate::ivp_dissipation::Settings)->Result<Option<crate::ivp_dissipation::Outcome>,&'static str>{
        if ![self.energy,coefficient,*environment_work].iter().all(|v|v.is_finite()){return Err("invalid group work state");}
        let mut next=*self;let mut next_bodies=*bodies;let mut work=*environment_work;let mut outcome=None;
        next.begin_update(unit_flags);
        if unit_flags&0xc00==0{
            next.energy=coefficient*next.energy;
            if !next.energy.is_finite(){return Err("group work decay overflow");}
            if next.energy>=0.&&!blocked.iter().any(|v|*v){
                let result=crate::ivp_dissipation::dissipate(&mut next_bodies,next.energy,k)?;
                work=(work as f64+result.consumed as f64) as f32;
                next.energy=(next.energy as f64-result.consumed as f64) as f32;
                outcome=Some(result);
            }
        }
        *self=next;*bodies=next_bodies;*environment_work=work;Ok(outcome)
    }
    /// Ordinary responses have already updated retained mixes. Work returns
    /// are accumulated in reverse array order before countdown/redistribution.
    /// `core_keys` identify each contact's object0 core, including fixed cores;
    /// an absent active owner must not conflate distinct fixed-core identities.
    pub(crate) fn finish_batch(&mut self,contacts:&mut [&mut RetainedContact],core_keys:&[u64],outcomes:&[friction::Outcome])->Result<Option<crate::ivp_redistribute::Stats>,&'static str>{
        if contacts.len()!=core_keys.len()||contacts.len()!=outcomes.len()||contacts.is_empty()
            ||!self.energy.is_finite()||core_keys.contains(&0)||outcomes.iter().any(|o|!o.work_delta.is_finite())
            ||contacts.iter().any(|c|c.retired||c.prepared.is_none()){
            return Err("invalid retained redistribution group");
        }
        self.accumulate_batch_work(outcomes)?;
        self.redistribute(contacts,core_keys)
    }
    ///100fe9b0 completes all pair friction before100ff020 counts down any
    /// pair. Keep work accumulation independent from mix redistribution.
    pub(crate) fn accumulate_batch_work(&mut self,outcomes:&[friction::Outcome])->Result<(),&'static str>{
        if !self.energy.is_finite()||outcomes.is_empty()||outcomes.iter().any(|o|!o.work_delta.is_finite()){return Err("invalid batch work outcomes");}
        let mut delta=0.;for outcome in outcomes.iter().rev(){delta=crate::ivp_redistribute::add_delta(delta,outcome.work_delta);}
        let mut energy=self.energy;crate::ivp_redistribute::accumulate_work(&mut energy,delta,0.);if !energy.is_finite(){return Err("batch pair work overflow");}self.energy=energy;Ok(())
    }
    pub(crate) fn redistribute(&mut self,contacts:&mut [&mut RetainedContact],core_keys:&[u64])->Result<Option<crate::ivp_redistribute::Stats>,&'static str>{
        if contacts.len()!=core_keys.len()||contacts.is_empty()||!self.energy.is_finite()||core_keys.contains(&0)||contacts.iter().any(|c|c.retired||c.prepared.is_none()){return Err("invalid retained redistribution group");}
        let mut groups=[crate::ivp_redistribute::Group{timer:self.timer,energy:self.energy,contacts:contacts.iter().zip(core_keys).map(|(c,&key)|{
            let g=c.geometry().unwrap();crate::ivp_redistribute::Contact{core_key:key,point:g.point,normal:g.normal,basis:g.basis,mix:c.state.refresh.mix}
        }).collect()}];
        let mut stats=crate::ivp_redistribute::tick(&mut groups,&crate::ivp_redistribute::Settings{one:1.,opposite:-1.,half:0.5,epsilon:1.00000001e-10,normal_tolerance:0.00100000005});
        for (c,row) in contacts.iter_mut().zip(&groups[0].contacts){c.state.refresh.mix=row.mix;}
        self.timer=groups[0].timer;self.energy=groups[0].energy;
        Ok(stats.remove(0))
    }
}
impl RetainedContact {
    pub(crate) fn from_state(state:RetainedState,owners:[Option<usize>;2])->Result<Self,&'static str>{
        if !state.refresh.time.is_finite()
            ||!state.refresh.mix.iter().all(|x|x.is_finite())
            ||![state.normal.force,state.energy,state.work,state.response_bound].iter().all(|x|x.is_finite())
            ||state.normal.force<0.||state.energy<0.||state.response_bound<0.
            ||owners.iter().all(Option::is_none)
            ||owners[0].is_some()&&owners[0]==owners[1]{return Err("invalid retained contact state");}
        Ok(Self{state,owners,prepared:None,retired:false})
    }
    /// Attach geometry computed by the source admission factory, preserving
    /// the refreshed state rather than rerunning refresh with seeded results.
    pub(crate) fn from_prepared(state:RetainedState,owners:[Option<usize>;2],prepared:PreparedContact)->Result<Self,&'static str>{
        if prepared.axes.bodies!=owners||prepared.geometry.state!=state.refresh{return Err("prepared admission state/owners disagree");}
        let mut out=Self::from_state(state,owners)?;out.prepared=Some(prepared);Ok(out)
    }
    pub(crate) fn state(&self)->RetainedState{self.state}
    pub(crate) fn is_retired(&self)->bool{self.retired}
    pub(crate) fn geometry(&self)->Option<&refresh::Output>{self.prepared.as_ref().map(PreparedContact::geometry)}
    pub(crate) fn normal_row(&self)->Result<crate::ivp_contact::Row,&'static str>{
        if self.retired{return Err("contact has retired");}
        let p=self.prepared.as_ref().ok_or("contact has no prepared geometry")?;
        Ok(crate::ivp_contact::Row{bodies:self.owners,normal:p.geometry.normal,jacobian:p.geometry.jacobians,gap:p.geometry.distance})
    }
    /// Batch response queues impulses; the caller still owns the coupled
    /// solve and its energy acceptance/commit decision.
    pub(crate) fn apply_batch_normal(&mut self,cores:&mut [Core],impulse:f32,skip:bool,dt:f32,inverse_dt:f32)->Result<(),&'static str>{
        let row=self.normal_row()?;
        if ![impulse,dt,inverse_dt].iter().all(|v|v.is_finite())||dt<=0.||inverse_dt<=0.
            ||row.bodies.iter().flatten().any(|&i|i>=cores.len()){return Err("invalid queued normal contact response");}
        if let Some(value)=crate::ivp_history::apply(&mut self.state.normal,impulse,skip,dt,inverse_dt){crate::ivp_contact::accumulate(cores,row,value);}
        Ok(())
    }
    pub(crate) fn refresh(
        &mut self,shapes:[refresh::Shape;2],bodies:[refresh::Body;2],
        now:f64,settings:&refresh::Settings,release:impl FnOnce(),
    )->Result<&refresh::Output,&'static str>{
        if self.retired{return Err("contact has retired");}
        if !now.is_finite()||now<self.state.refresh.time{return Err("contact refresh clock moved backwards");}
        let prepared=PreparedContact::prepare(self.state.refresh,shapes,bodies,self.owners,now,settings,release)?;
        self.state.refresh=prepared.geometry().state;
        self.prepared=Some(prepared);
        Ok(self.prepared.as_ref().unwrap().geometry())
    }
    pub(crate) fn respond_friction(&mut self,cores:&mut [Core],frames:&[CoreFrame],settings:&TangentSettings)->Result<friction::Outcome,&'static str>{
        if self.retired{return Err("contact has retired");}
        if settings.response_bound.to_bits()!=self.state.response_bound.to_bits(){return Err("persisted contact mass bound changed");}
        let prepared=self.prepared.as_ref().ok_or("contact has no prepared geometry")?;
        let mut memory=FrictionMemory{slip:slip::State{mix:self.state.refresh.mix,marker:self.state.refresh.marker,energy:self.state.energy},work:self.state.work};
        let result=prepared.respond_single_friction(cores,frames,self.state.normal.force,&mut memory,settings)?;
        self.state.refresh.mix=memory.slip.mix;self.state.refresh.marker=memory.slip.marker;
        self.state.energy=memory.slip.energy;self.state.work=memory.work;
        Ok(result)
    }
    pub(crate) fn respond_normal(&mut self,cores:&mut [Core],inverse_dt:f32)->Result<single::Response,&'static str>{
        if self.retired{return Err("contact has retired");}
        let prepared=self.prepared.as_ref().ok_or("contact has no prepared geometry")?;
        let response=prepared.respond_single(cores,&mut self.state.normal,inverse_dt).ok_or("invalid normal contact response")?;
        if response.retire{self.retired=true;self.prepared=None;}
        Ok(response)
    }
    /// The batch caller computes one reverse-list aggregate limit before
    /// trimming any contact. Commit state only if the entire group succeeds.
    pub(crate) fn respond_group_friction(contacts:&mut [&mut Self],cores:&mut [Core],frames:&[CoreFrame],settings:&[TangentSettings])->Result<Vec<friction::Outcome>,&'static str>{
        if contacts.len()!=settings.len()||contacts.is_empty(){return Err("invalid friction group dimensions");}
        let dt=settings[0].dt;
        let mut coefficients=Vec::with_capacity(contacts.len());
        for (c,p) in contacts.iter().zip(settings){
            if c.retired||c.prepared.is_none()||p.dt.to_bits()!=dt.to_bits()
                ||p.response_bound.to_bits()!=c.state.response_bound.to_bits(){return Err("invalid retained friction group");}
            coefficients.push(slip::Coefficients{force:c.state.normal.force,friction:p.friction,inverse_response:c.state.response_bound});
        }
        let limit=slip::group_limit(&coefficients,dt) as f32;
        let mut next_cores=cores.to_vec();
        let mut next=contacts.iter().map(|c|FrictionMemory{slip:slip::State{mix:c.state.refresh.mix,marker:c.state.refresh.marker,energy:c.state.energy},work:c.state.work}).collect::<Vec<_>>();
        let mut outcomes=(0..contacts.len()).map(|_|None).collect::<Vec<_>>();
        //100fe9b0 traverses the stored group in reverse. Every following
        // response must observe the preceding response's changed raw cores.
        for i in (0..contacts.len()).rev(){let c=&contacts[i];
            #[cfg(test)] let before=next_cores.clone();
            outcomes[i]=Some(c.prepared.as_ref().unwrap().respond_friction_limit(&mut next_cores,frames,c.state.normal.force,&mut next[i],&settings[i],Some(limit))?);
            #[cfg(test)] TANGENT_AUDIT.with(|audit|{if let Some(a)=audit.borrow_mut().as_mut(){let g=c.geometry().unwrap();let o=outcomes[i].as_ref().unwrap();a.push(serde_json::json!({"time":c.state.refresh.time.to_bits(),"owners":c.owners,"index":i,"point":g.point.to_array(),"basis":[g.basis[0].to_array(),g.basis[1].to_array()],"force":c.state.normal.force,"mix_before":c.state.refresh.mix,"mix_after":next[i].slip.mix,"matrix":o.matrix,"speed":o.speed,"impulse":o.impulse,"initialized":o.initialized,"applied":o.applied,"linear_before":before[0].linear.to_array(),"angular_before":before[0].angular.to_array(),"linear_after":next_cores[0].linear.to_array(),"angular_after":next_cores[0].angular.to_array()}));}});

        }
        cores.copy_from_slice(&next_cores);
        for (c,m) in contacts.iter_mut().zip(next){c.state.refresh.mix=m.slip.mix;c.state.refresh.marker=m.slip.marker;c.state.energy=m.slip.energy;c.state.work=m.work;}
        Ok(outcomes.into_iter().map(Option::unwrap).collect())
    }
}
impl PreparedContact {
    pub(crate) fn prepare(
        state:refresh::State,shapes:[refresh::Shape;2],bodies:[refresh::Body;2],
        owners:[Option<usize>;2],now:f64,settings:&refresh::Settings,release:impl FnOnce(),
    )->Result<Self,&'static str>{
        if owners[0].is_some()&&owners[0]==owners[1]
            ||(0..2).any(|i|owners[i].is_some()!=(bodies[i].flags&2==0)){
            return Err("contact core ownership disagrees with refresh active flags");
        }
        let geometry=refresh::refresh_with_cache_release(state,shapes,bodies,now,settings,release)?;
        let axes=single::Axes{bodies:owners,normal:geometry.normal,jacobian:geometry.jacobians};
        Ok(Self{geometry,axes})
    }
    pub(crate) fn geometry(&self)->&refresh::Output{&self.geometry}
    /// Core velocities may change after refresh. Use current raw/pending core
    /// state with this contact's cached geometry and cached reciprocal response.
    pub(crate) fn respond_single(&self,cores:&mut [Core],history:&mut History,inverse_dt:f32)->Option<single::Response>{
        single::respond_cached(cores,self.axes,self.geometry.inverse_response,history,self.geometry.distance,inverse_dt,self.geometry.geometry.status)
    }
    pub(crate) fn friction_memory(&self,energy:f32,work:f32)->FrictionMemory{
        FrictionMemory{slip:slip::State{mix:self.geometry.state.mix,marker:self.geometry.state.marker,energy},work}
    }
    ///100ff7a0 trims slip before100fe760. The caller supplies the retained
    /// normal-force history and current core frames: tangent friction can run
    /// before this tick's normal solve, after the core pose has advanced.
    pub(crate) fn respond_single_friction(&self,cores:&mut [Core],frames:&[CoreFrame],normal_force:f32,memory:&mut FrictionMemory,p:&TangentSettings)->Result<friction::Outcome,&'static str>{
        self.respond_friction_limit(cores,frames,normal_force,memory,p,None)
    }
    fn respond_friction_limit(&self,cores:&mut [Core],frames:&[CoreFrame],normal_force:f32,memory:&mut FrictionMemory,p:&TangentSettings,group_limit:Option<f32>)->Result<friction::Outcome,&'static str>{
        if p.mode==1||p.alternate_candidate{return Err("unreconstructed directed/alternate contact friction");}
        if group_limit.is_some_and(|v|!v.is_finite()||v<0.){return Err("invalid group slip limit");}
        if ![p.dt,p.inverse_dt,p.friction,p.response_bound,p.threshold_extra,p.minimum_impulse,p.minimum_determinant_squared,normal_force,memory.slip.energy,memory.work].iter().all(|x|x.is_finite())
            ||p.dt<=0.||p.inverse_dt<=0.||p.friction<0.||p.response_bound<0.||normal_force<0.
            ||p.threshold_extra<0.||p.minimum_impulse<=0.||p.minimum_determinant_squared<=0.
            ||memory.slip.mix.iter().any(|v|!v.is_finite())
            ||self.axes.bodies.iter().flatten().any(|&i|i>=cores.len()||i>=frames.len()){
            return Err("invalid tangent contact state");
        }
        for &i in self.axes.bodies.iter().flatten(){let b=cores[i];let f=frames[i];
            if !b.linear.is_finite()||!b.angular.is_finite()||!b.inverse_mass.is_finite()||b.inverse_mass<0.
                ||!b.inverse_inertia.is_finite()||b.inverse_inertia.min_element()<0.||!f.rotation.is_finite()||!f.position.is_finite(){return Err("invalid current contact core");}
        }
        let inactive=friction::Body{active:false,inverse_mass:0.,inverse_inertia:Vec3::ZERO,rotation:Mat3::IDENTITY,position:Vec3::ZERO,linear:Vec3::ZERO,angular:Vec3::ZERO};
        let mut bodies=[inactive;2];
        for side in 0..2{if let Some(i)=self.axes.bodies[side]{let b=cores[i];let f=frames[i];bodies[side]=friction::Body{active:true,inverse_mass:b.inverse_mass,inverse_inertia:b.inverse_inertia,rotation:f.rotation,position:f.position,linear:b.linear,angular:b.angular};}}
        let mut next=*memory;
        let limit=group_limit.unwrap_or_else(||slip::single_limit(slip::Coefficients{force:normal_force,friction:p.friction,inverse_response:p.response_bound},p.dt));
        slip::trim(&mut next.slip,limit,p.friction,normal_force,p.threshold_extra);
        let outcome=friction::ordinary(&mut bodies,&friction::Parameters{point:self.geometry.point,basis:self.geometry.basis,mix:next.slip.mix,dt:p.dt,inverse_dt:p.inverse_dt,normal_force,friction:p.friction,minimum_impulse:p.minimum_impulse,minimum_determinant_squared:p.minimum_determinant_squared,alternate_candidate:false},&mut next.work)?;
        for side in 0..2{if let Some(i)=self.axes.bodies[side]{cores[i].linear=bodies[side].linear;cores[i].angular=bodies[side].angular;}}
        *memory=next;Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn f(v:&Value)->f32{v.as_f64().unwrap() as f32}
    fn v(a:&Value)->Vec3{Vec3::from_array(std::array::from_fn(|i|f(&a[i])))}
    fn core(b:&Value)->Core{Core{inverse_mass:f(&b["inverse_mass"]),inverse_inertia:v(&b["inverse_inertia"]),linear:v(&b["linear"]),angular:v(&b["angular"]),pending_linear:v(&b["pending_linear"]),pending_angular:v(&b["pending_angular"])}}
    #[test]
    fn native_group_constructor_clear_and_refresh_flag_policy_match(){
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-group-phase.json")).unwrap();
        for row in data["constructors"].as_array().unwrap(){let group=FrictionGroup::default();assert_eq!(group.timer,row["timer"].as_i64().unwrap() as i32);assert_eq!(group.energy.to_bits(),f(&row["energy"]).to_bits());assert!(row["other_bytes_unchanged"].as_bool().unwrap());}
        for row in data["clears"].as_array().unwrap(){let kind=row["kind"].as_i64().unwrap() as i32;let n=row["size"].as_u64().unwrap() as usize;
            let mut groups=(0..n).map(|i|FrictionGroup{timer:kind-i as i32,energy:0.031*(kind+i as i32-17) as f32}).collect::<Vec<_>>();
            for (i,g) in groups.iter_mut().enumerate(){g.begin_update(0x3000);assert_eq!(g.energy.to_bits(),f(&row["energy"][i]).to_bits());assert_eq!(g.timer,kind-i as i32);}
            assert!(row["other_bytes_unchanged"].as_bool().unwrap());
        }
        let (mut cleared,mut preserved,mut dissipation)=(0,0,0);
        for phase in data["phases"].as_array().unwrap(){let flags=phase["flags"].as_u64().unwrap() as u32;
            if flags&(0x3000|0xc00)==0{dissipation+=1;continue;}
            for (b,a) in phase["before"].as_array().unwrap().iter().zip(phase["after"].as_array().unwrap()){
                let mut group=FrictionGroup{timer:b["timer"].as_i64().unwrap() as i32,energy:f(&b["energy"])};group.begin_update(flags);
                assert_eq!(group.timer,a["timer"].as_i64().unwrap() as i32);assert_eq!(group.energy.to_bits(),f(&a["energy"]).to_bits());
            }
            if flags&0x3000!=0{cleared+=1;}else{preserved+=1;}
        }
        assert_eq!((cleared,preserved,dissipation),(6,21,160));
    }
    #[test]
    fn retained_batch_friction_preserves_native_reverse_order_and_shared_core_evolution(){
        let friction:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-friction.json")).unwrap();
        let slip:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-slip.json")).unwrap();
        let groups:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-redistribute.json")).unwrap();
        let rows=friction["cases"].as_array().unwrap();let calls=slip["cases"].as_array().unwrap();
        let (mut group_count,mut response_count)=(0,0);
        for group in groups["work"].as_array().unwrap(){
            let mut contacts=Vec::new();let mut settings=Vec::new();let mut expected=Vec::new();
            for point in group["points"].as_array().unwrap(){
                let candidates=rows.iter().filter(|n|n["capture"]==group["capture"]&&n["model"]==group["model"]&&n["step"]==group["step"]&&v(&n["point"])==v(point)).collect::<Vec<_>>();assert_eq!(candidates.len(),1);let n=candidates[0];
                let c=calls.iter().find(|c|c["capture"]==n["capture"]&&c["model"]==n["model"]&&c["sequence"]==n["sequence"]).unwrap();
                let state=refresh::State{time:0.,mix:std::array::from_fn(|i|f(&c["before"]["mix"][i])),marker:c["before"]["marker"].as_u64().unwrap() as u8,counter:0};
                let mut contact=RetainedContact::from_state(RetainedState{refresh:state,normal:History{force:f(&c["force"]),counter:0},energy:f(&c["before"]["energy"]),work:f(&n["work_before"]),response_bound:f(&c["inverse_response"])},[Some(0),None]).unwrap();
                // This is a controller-order component test with native cache
                // inputs. Source geometry is covered separately by refresh
                // tests; no unseeded geometry claim is made for these rows.
                let geometry=refresh::Geometry{point:v(point),normal:Vec3::Y,tangent:Vec3::X,distance:0.,status:0,marker:state.marker,counter:0};
                contact.prepared=Some(PreparedContact{geometry:refresh::Output{geometry,point:v(point),normal:Vec3::Y,basis:std::array::from_fn(|i|v(&n["basis"][i])),arms:[Vec3::ZERO;2],jacobians:[Vec3::ZERO;2],response:1.,inverse_response:1.,active:[true,false],state,distance:0.},axes:single::Axes{bodies:[Some(0),None],normal:Vec3::Y,jacobian:[Vec3::ZERO;2]}});
                contacts.push(contact);
                settings.push(TangentSettings{dt:f(&n["dt"]),inverse_dt:f(&n["inverse_dt"]),friction:f(&c["friction"]),response_bound:f(&c["inverse_response"]),threshold_extra:f(&c["threshold_extra"]),minimum_impulse:f(&n["constants"]["minimum_impulse"]),minimum_determinant_squared:f(&n["constants"]["minimum_determinant_squared"]),mode:0,alternate_candidate:false});
                expected.push((n,c));
            }
            assert!(!contacts.is_empty());
            let first=expected.last().unwrap().0;let last=expected[0].0;let b=&first["before"][0];
            let mut cores=[core(b)];let frames=[CoreFrame{rotation:Mat3::from_cols_array(&std::array::from_fn(|i|f(&b["rotation_rows"][i]))).transpose(),position:v(&b["position"])}];
            let outcomes=RetainedContact::respond_group_friction(&mut contacts.iter_mut().collect::<Vec<_>>(),&mut cores,&frames,&settings).unwrap();
            let final_core=core(&last["after"][0]);assert_eq!(cores[0].linear,final_core.linear,"group{group_count}");assert_eq!(cores[0].angular,final_core.angular);assert_eq!(cores[0].pending_linear,final_core.pending_linear);assert_eq!(cores[0].pending_angular,final_core.pending_angular);
            let mut delta=0.;
            for i in (0..contacts.len()).rev(){let (n,c)=expected[i];let actual=contacts[i].state();
                assert_eq!(actual.refresh.mix,std::array::from_fn(|j|f(&c["after"]["mix"][j])));assert_eq!(actual.refresh.marker,c["after"]["marker"].as_u64().unwrap() as u8);
                assert_eq!(actual.energy.to_bits(),f(&c["after"]["energy"]).to_bits());assert_eq!(actual.work.to_bits(),f(&n["work_after"]).to_bits());
                assert_eq!(outcomes[i].impulse,std::array::from_fn(|j|f(&n["ordinary"]["impulse"][j])));delta=crate::ivp_redistribute::add_delta(delta,outcomes[i].work_delta);response_count+=1;
            }
            assert_eq!(delta,f(&group["delta"]));
            let system=groups["systems"].as_array().unwrap().iter().find(|s|s["capture"]==group["capture"]&&s["model"]==group["model"]&&s["step"]==group["step"]).unwrap();
            assert_eq!(system["before"].as_array().unwrap().len(),1);
            let before=&system["before"][0];let after=&system["after"][0];
            let keys=before["rows"].as_array().unwrap().iter().map(|r|r["core_key"].as_u64().unwrap()).collect::<Vec<_>>();
            for (row,point) in before["rows"].as_array().unwrap().iter().zip(group["points"].as_array().unwrap()){assert_eq!(v(&row["point"]),v(point));}
            // Entry work can be retained or cleared by the refresh phase's
            // unit flags; it is not unconditionally reset every physics tick.
            let mut clock=FrictionGroup{timer:before["timer"].as_i64().unwrap() as i32,energy:f(&group["before"])};clock.begin_update(0);
            assert_eq!(clock.energy.to_bits(),f(&group["before"]).to_bits());
            clock.finish_batch(&mut contacts.iter_mut().collect::<Vec<_>>(),&keys,&outcomes).unwrap();
            assert_eq!(clock.timer,after["timer"].as_i64().unwrap() as i32);assert_eq!(clock.energy.to_bits(),f(&after["energy"]).to_bits());
            for (contact,row) in contacts.iter().zip(after["rows"].as_array().unwrap()){assert_eq!(contact.state().refresh.mix,std::array::from_fn(|i|f(&row["mix"][i])));}
            // A later invalid contact must not partially commit an earlier
            // contact's raw cores, mix or energy.
            let saved=cores;let states=contacts.iter().map(RetainedContact::state).collect::<Vec<_>>();settings[0].mode=1;
            assert!(RetainedContact::respond_group_friction(&mut contacts.iter_mut().collect::<Vec<_>>(),&mut cores,&frames,&settings).is_err());
            assert_eq!(cores[0].linear,saved[0].linear);assert_eq!(cores[0].angular,saved[0].angular);
            for (c,s) in contacts.iter().zip(states){let actual=c.state();assert_eq!(actual.refresh,s.refresh);assert_eq!(actual.energy,s.energy);assert_eq!(actual.work,s.work);}
            group_count+=1;
        }
        assert_eq!((group_count,response_count),(368,1231));
    }
}
