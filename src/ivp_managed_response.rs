//! Managed admission through single-contact phases and coupled normal response.
//! Owns either an authored contact boundary or the full physical body bank.
//! Prepared object frames still come from the caller/cache owner. Discovery,
//! resource callbacks and integration into the playable world remain separate.
use std::collections::BTreeMap;
use bevy::prelude::*;
use crate::{ivp_contact::Core,ivp_contact_pipeline::{CoreFrame,PreparedContact,RetainedContact,RetainedState,TangentSettings},
    ivp_refresh as refresh,ivp_managed_contacts::{Managed,Settings as AdmissionSettings},
    ivp_create::Endpoint,ivp_mass_bound as mass,ivp_unit_registry::{Registry,Effect},ivp_history::History};

/// Compact-feature rotation over the native five-millisecond scoring horizon.
///10103c10 retains the cosine, products and accumulation until the float store.
fn compact_wave_bound(angular:[Vec3;2],radii:[f32;2],compact:[bool;2],inverse_dt:f32)->f32{
    let horizon=f32::from_bits(0x3ba3d70a);
    let squared=angular.map(|v|((((v.x*v.x+v.y*v.y)+v.z*v.z)*horizon)*horizon).min(0.25));
    #[cfg(any(target_arch="x86",target_arch="x86_64"))]
    {
        let mut accumulated=[0u8;10];let mut out=0f32;
        for side in 0..2{if compact[side]{unsafe{core::arch::asm!(
            "fld dword ptr [{angle}]","fsqrt","fcos","fld1","fsub st(0), st(1)","fstp st(1)",
            "fmul dword ptr [{radius}]","fmul dword ptr [{inverse}]","fld tbyte ptr [{sum}]","faddp st(1), st(0)","fstp tbyte ptr [{sum}]",
            angle=in(reg)&squared[side],radius=in(reg)&radii[side],inverse=in(reg)&inverse_dt,sum=in(reg)&mut accumulated,
            options(nostack,preserves_flags));}}}
        unsafe{core::arch::asm!("fld tbyte ptr [{sum}]","fstp dword ptr [{out}]",sum=in(reg)&accumulated,out=in(reg)&mut out,options(nostack,preserves_flags));}out
    }
    #[cfg(not(any(target_arch="x86",target_arch="x86_64")))]
    {let mut accumulated=0f64;for side in 0..2{if compact[side]{accumulated=(1.-(squared[side] as f64).sqrt().cos())*radii[side] as f64*inverse_dt as f64+accumulated;}}accumulated as f32}
}

#[derive(Clone,Copy)]
pub(crate) struct Body {pub flags:u16,pub core:Core,pub frame:CoreFrame,pub stamps:[f64;2]}
/// Exactly one body representation is owned at a time. Contact kernels take
/// short-lived views; physical predictors, fourth lanes and object flags stay
/// in the canonical physical bank rather than a second synchronized world.
#[derive(Clone)]
enum Bodies{Authored(Vec<Body>),Physical(Vec<crate::ivp_joint_apply::Body>)}
fn physical_view(b:&crate::ivp_joint_apply::Body)->Body{Body{flags:b.flags,core:Core{inverse_mass:b.inverse[3],inverse_inertia:Vec3::from_array([b.inverse[0],b.inverse[1],b.inverse[2]]),linear:Vec3::from_array([b.linear[0],b.linear[1],b.linear[2]]),angular:Vec3::from_array([b.angular[0],b.angular[1],b.angular[2]]),pending_linear:Vec3::from_array([b.pending_linear[0],b.pending_linear[1],b.pending_linear[2]]),pending_angular:Vec3::from_array([b.pending_angular[0],b.pending_angular[1],b.pending_angular[2]])},frame:CoreFrame{position:b.current.position,rotation:b.current.rotation},stamps:b.stamps}}
impl Bodies{
    fn len(&self)->usize{match self{Self::Authored(v)=>v.len(),Self::Physical(v)=>v.len()}}
    fn get(&self,i:usize)->Body{match self{Self::Authored(v)=>v[i],Self::Physical(v)=>physical_view(&v[i])}}
    fn iter(&self)->impl ExactSizeIterator<Item=Body>+'_ {(0..self.len()).map(|i|self.get(i))}
    fn set_core(&mut self,i:usize,c:Core){match self{Self::Authored(v)=>v[i].core=c,Self::Physical(v)=>{let b=&mut v[i];b.inverse=[c.inverse_inertia.x,c.inverse_inertia.y,c.inverse_inertia.z,c.inverse_mass];b.linear[..3].copy_from_slice(&c.linear.to_array());b.angular[..3].copy_from_slice(&c.angular.to_array());b.pending_linear[..3].copy_from_slice(&c.pending_linear.to_array());b.pending_angular[..3].copy_from_slice(&c.pending_angular.to_array());}}}
    fn set_cores(&mut self,cores:Vec<Core>){debug_assert_eq!(cores.len(),self.len());for (i,c) in cores.into_iter().enumerate(){self.set_core(i,c);}}
    fn reset_stamps(&mut self,i:usize,now:f64){match self{Self::Authored(v)=>v[i].stamps=[now;2],Self::Physical(v)=>v[i].stamps=[now;2]}}
}
#[derive(Clone,Copy)]
pub(crate) struct Object {pub core:usize,pub rotation:[Vec3;3],pub translation:Vec3,pub radius:f32,pub material_flags:u32,pub friction:f32}
#[derive(Clone)]
pub(crate) struct Feature {pub endpoint:Endpoint,pub vertices:[Vec3;3],pub canonical:[Vec3;3]}
impl Feature {
    /// Keep lookup identity and numerical geometry on the same authored edge.
    /// These are local IVP vertices; world transforms come from retained caches.
    pub(crate) fn from_compact(object:u64,ledge_key:u32,convex:&l4d2_formats::phy::Convex,kind:i16,edge:l4d2_formats::phy::CompactEdgeId)->Result<Self,&'static str>{
        let feature=crate::ivp_create::from_compact(kind,ledge_key,convex,edge)?;
        let shape=refresh::Shape::from_compact(kind,Some((convex,edge)),[Vec3::X,Vec3::Y,Vec3::Z],Vec3::ZERO,0.)?;
        Ok(Self{endpoint:Endpoint{object,feature},vertices:shape.vertices,canonical:shape.canonical})
    }
}
#[derive(Clone,Copy)]
pub(crate) struct Settings {pub dt:f32,pub admission:AdmissionSettings,pub refresh:refresh::Settings,pub threshold_extra:f32,pub minimum_impulse:f32,pub minimum_determinant_squared:f32}
#[derive(Clone,Copy)]
pub(crate) struct StepChange {pub core:usize,pub linear:Vec3,pub angular:Vec3,pub translation:Vec3}
#[derive(Clone,Copy,PartialEq)]
enum Phase {Refresh,Tangent,Normal}
#[derive(Clone)]
struct PairWork {coefficient:f32,environment_work:f32,blocked:Vec<bool>,settings:crate::ivp_dissipation::Settings}
#[derive(Clone)]
struct Contact {features:[Feature;2],retained:RetainedContact,friction:f32,mode:u8,phase:Phase,impact_pushes:u16,impact_valid:bool,impact_rescue:f32,impact_score:f32}
#[derive(Clone)]
pub(crate) struct ResponseWorld {
    managed:Managed,bodies:Bodies,objects:BTreeMap<u64,Object>,contacts:BTreeMap<u64,Contact>,
    work:Option<PairWork>,energy:Option<(Vec<crate::ivp_batch_normal::Physical>,f32,f32)>,reports:[usize;3],settings:Settings,now:f64,effects:Vec<Effect>,released:Vec<usize>,released_features:Vec<[Endpoint;2]>,
}
fn rows(m:Mat3)->[Vec3;3]{let t=m.transpose();[t.x_axis,t.y_axis,t.z_axis]}
impl ResponseWorld {
    pub(crate) fn new(registry:Registry,bodies:Vec<Body>,objects:BTreeMap<u64,Object>,settings:Settings,now:f64)->Result<Self,&'static str>{
        if !now.is_finite()||!settings.dt.is_finite()||settings.dt<=0.||!settings.dt.recip().is_finite()
            ||![settings.threshold_extra,settings.minimum_impulse,settings.minimum_determinant_squared].iter().all(|v|v.is_finite())
            ||settings.threshold_extra<0.||settings.minimum_impulse<=0.||settings.minimum_determinant_squared<=0.
            ||![settings.refresh.epsilon,settings.refresh.bary_bias,settings.refresh.one,settings.refresh.opposite].iter().all(|v|v.is_finite())
            ||settings.refresh.epsilon<=0.||settings.refresh.area_epsilon!=settings.admission.area_epsilon{return Err("invalid contact response settings");}
        for b in &bodies{let c=b.core;if b.flags&16!=0||!b.frame.position.is_finite()||!b.frame.rotation.is_finite()
            ||!b.stamps.iter().all(|v|v.is_finite())||!c.linear.is_finite()||!c.angular.is_finite()||!c.pending_linear.is_finite()||!c.pending_angular.is_finite()
            ||!c.inverse_mass.is_finite()||c.inverse_mass<0.||!c.inverse_inertia.is_finite()||c.inverse_inertia.min_element()<0.{return Err("unsupported/invalid contact response body");}}
        for o in objects.values(){if o.core>=bodies.len()||!o.rotation.iter().all(|v|v.is_finite())||!o.translation.is_finite()||!o.radius.is_finite()||o.radius<0.
            ||o.material_flags!=0||!o.friction.is_finite(){return Err("unsupported/invalid ordinary contact object");}}
        let fixed=bodies.iter().enumerate().map(|(i,b)|(i,b.flags&2!=0)).collect();
        let managed=Managed::new(registry,fixed,objects.iter().map(|(&id,o)|(id,o.core)).collect(),settings.admission)?;
        Ok(Self{managed,bodies:Bodies::Authored(bodies),objects,contacts:BTreeMap::new(),work:None,energy:None,reports:[0;3],settings,now,effects:vec![],released:vec![],released_features:vec![]})
    }
    pub(crate) fn new_physical(registry:Registry,bodies:Vec<crate::ivp_joint_apply::Body>,objects:BTreeMap<u64,Object>,settings:Settings,now:f64)->Result<Self,&'static str>{
        let view=Bodies::Physical(bodies);let boundary=view.iter().collect();let mut world=Self::new(registry,boundary,objects,settings,now)?;
        let Bodies::Physical(bodies)=view else{unreachable!()};world.adopt_physical_bodies(bodies)?;Ok(world)
    }
    /// One-time ownership transfer before contact admission. It does not seed
    /// later numerical state or reconstruct predictors from inverse properties.
    pub(crate) fn adopt_physical_bodies(&mut self,bodies:Vec<crate::ivp_joint_apply::Body>)->Result<(),&'static str>{
        if !self.contacts.is_empty()||!matches!(self.bodies,Bodies::Authored(_))||bodies.len()!=self.bodies.len(){return Err("invalid physical-bank transfer phase");}
        for (i,b) in bodies.iter().enumerate(){
            if b.id!=i||!b.mass.is_finite()||b.mass<0.||!b.clock.is_finite()||!b.prediction.time.is_finite()||!b.prediction.start.is_finite()||!b.prediction.velocity.is_finite()||!b.prediction.inverse_dt.is_finite()||b.prediction.inverse_dt<=0.||b.prediction.rotation_start.iter().chain(&b.prediction.rotation_end).any(|v|!v.is_finite()){return Err("invalid physical-bank predictor boundary");}
            let old=self.bodies.get(i);
            let view=physical_view(b);
            if old.flags!=view.flags||old.stamps.map(f64::to_bits)!=view.stamps.map(f64::to_bits)
                ||old.frame.position.to_array().map(f32::to_bits)!=view.frame.position.to_array().map(f32::to_bits)
                ||old.frame.rotation.to_cols_array().map(f32::to_bits)!=view.frame.rotation.to_cols_array().map(f32::to_bits)
                ||old.core.inverse_mass.to_bits()!=view.core.inverse_mass.to_bits()
                ||[old.core.inverse_inertia,old.core.linear,old.core.angular,old.core.pending_linear,old.core.pending_angular].map(|v|v.to_array().map(f32::to_bits))!=[view.core.inverse_inertia,view.core.linear,view.core.angular,view.core.pending_linear,view.core.pending_angular].map(|v|v.to_array().map(f32::to_bits))
                ||self.energy.as_ref().is_some_and(|(p,_,_)|p[i].mass.to_bits()!=b.mass.to_bits()){return Err("physical bank disagrees with contact boundary");}
        }
        self.bodies=Bodies::Physical(bodies);Ok(())
    }
    pub(crate) fn body_views(&self)->impl ExactSizeIterator<Item=Body>+'_ {self.bodies.iter()}
    pub(crate) fn physical_bodies(&self)->Result<&[crate::ivp_joint_apply::Body],&'static str>{match &self.bodies{Bodies::Physical(v)=>Ok(v),_=>Err("authored contact boundary has no physical predictors")}}
    pub(crate) fn physical_bodies_mut(&mut self)->Result<&mut [crate::ivp_joint_apply::Body],&'static str>{match &mut self.bodies{Bodies::Physical(v)=>Ok(v),_=>Err("authored contact boundary has no physical predictors")}}
    /// Prepared object caches come from the physical/cache owner, never
    /// from a rounded inverse reconstruction. This changes no body motion.
    pub(crate) fn set_physical_context(&mut self,now:f64,frames:&[(u64,[Vec3;3],Vec3)])->Result<(),&'static str>{
        if frames.len()!=self.objects.len(){return Err("incomplete physical controller context");}
        self.set_selected_physical_context(now,frames)
    }
    pub(crate) fn set_selected_physical_context(&mut self,now:f64,frames:&[(u64,[Vec3;3],Vec3)])->Result<(),&'static str>{
        if !matches!(self.bodies,Bodies::Physical(_))||!now.is_finite()||self.contacts.values().any(|c|c.phase!=Phase::Refresh)
            ||frames.iter().map(|f|f.0).collect::<std::collections::BTreeSet<_>>().len()!=frames.len()
            ||frames.iter().any(|(id,r,p)|!self.objects.contains_key(id)||r.iter().any(|v|!v.is_finite())||!p.is_finite()){return Err("invalid physical controller context");}
        for (id,r,p) in frames{let object=self.objects.get_mut(id).unwrap();object.rotation=*r;object.translation=*p;}self.now=now;Ok(())
    }
    pub(crate) fn with_energy(mut self,properties:Vec<crate::ivp_batch_normal::Physical>,factor:f32,bound:f32)->Result<Self,&'static str>{
        if !self.contacts.is_empty()||properties.len()!=self.bodies.len()||!factor.is_finite()||factor<0.||!bound.is_finite()||bound<0.
            ||properties.iter().zip(self.bodies.iter()).any(|(p,b)|p.flags!=b.flags||!p.mass.is_finite()||p.mass<0.||!p.inertia.is_finite()||p.inertia.min_element()<0.){return Err("invalid normal energy properties");}
        if let Bodies::Physical(bodies)=&self.bodies{if properties.iter().zip(bodies).any(|(p,b)|p.mass.to_bits()!=b.mass.to_bits()){return Err("physical mass disagrees with normal energy properties");}}
        self.energy=Some((properties,factor,bound));Ok(self)
    }
    pub(crate) fn with_pair_work(mut self,coefficient:f32,environment_work:f32,blocked:Vec<bool>,settings:crate::ivp_dissipation::Settings)->Result<Self,&'static str>{
        if !self.contacts.is_empty()||!coefficient.is_finite()||coefficient<0.||!environment_work.is_finite()||blocked.len()!=self.bodies.len()
            ||![settings.epsilon,settings.one,settings.opposite,settings.two,settings.half,settings.huge,settings.small,settings.fraction].iter().all(|v|v.is_finite())||settings.epsilon<=0.{return Err("invalid pair work boundary");}
        self.work=Some(PairWork{coefficient,environment_work,blocked,settings});Ok(self)
    }
    /// Owned admitted geometry for10103940. The cached impact count is
    /// distinct from normal-force history and per-core impact budgets.
    pub(crate) fn impact_contact(&self,key:u64)->Result<([usize;2],refresh::Output,f32,u16),&'static str>{
        let c=self.contacts.get(&key).ok_or("unknown immediate impact contact")?;
        if c.phase!=Phase::Refresh{return Err("immediate response requires refreshed contact geometry");}
        Ok((c.features.each_ref().map(|f|self.objects[&f.endpoint.object].core),c.retained.geometry().ok_or("impact contact lacks prepared geometry")?.clone(),c.friction,c.impact_pushes))
    }
    pub(crate) fn impact_objects(&self,key:u64)->Result<[u64;2],&'static str>{Ok(self.contacts.get(&key).ok_or("unknown impact cache contact")?.features.each_ref().map(|f|f.endpoint.object))}
    /// Object/core cache access order used by the next refresh controller.
    pub(crate) fn refresh_cache_cores(&self,system:usize)->Result<Vec<usize>,&'static str>{
        let pairs=self.managed.owner().system_pairs(system)?;let mut cores=vec![];
        for pair in pairs.iter().rev(){for &id in pair.contacts.iter().rev(){for feature in &self.contacts[&(id as u64)].features{cores.push(self.objects[&feature.endpoint.object].core);}}}Ok(cores)
    }
    pub(crate) fn impact_groups(&self,key:u64)->Result<(usize,Vec<crate::ivp_contact_system::Pair>,usize),&'static str>{
        let system=self.managed.owner().binding(key as usize)?.system;let pairs=self.managed.owner().system_pairs(system)?.to_vec();let entry=pairs.iter().position(|p|p.contacts.contains(&(key as usize))).ok_or("impact lacks pair group")?;Ok((system,pairs,entry))
    }
    pub(crate) fn refresh_impact_contacts(&mut self,keys:&[usize])->Result<Vec<usize>,&'static str>{
        let mut retired=vec![];
        for &key in keys.iter().rev(){let key=key as u64;let f=self.contacts.get(&key).ok_or("unknown wave refresh contact")?.features.clone();
            let shapes=f.each_ref().map(|f|self.shape(f));let bodies=self.refresh_bodies(&f);let c=self.contacts.get_mut(&key).unwrap();let g=c.retained.refresh(shapes,bodies,self.now,&self.settings.refresh,||{})?;
            if g.geometry.status==1{
                self.managed.remove(key)?;self.contacts.remove(&key);self.drain_physical_effects();retired.push(key as usize);continue;
            }
            //10102f60 allocates a new10105650 cache. Its push count and
            //valid marker belong to that allocation, not the persistent contact.
            c.impact_pushes=0;c.impact_valid=false;
        }Ok(retired)
    }
    pub(crate) fn invalidate_impact_core(&mut self,system:usize,core:usize)->Result<(),&'static str>{
        for &key in self.managed.owner().core_contacts(system,core)?{self.contacts.get_mut(&(key as u64)).ok_or("wave lost a linked contact")?.impact_valid=false;}Ok(())
    }
    pub(crate) fn next_impact_push(&mut self,key:u64)->Result<(),&'static str>{let c=self.contacts.get_mut(&key).ok_or("unknown wave contact")?;c.impact_pushes=c.impact_pushes.wrapping_add(1);Ok(())}
    ///10103c10 scoring. Pending impulses deliberately do not enter
    /// the current raw point-speed projection used to choose the next contact.
    pub(crate) fn impact_score(&mut self,key:u64,dt:f32,radii:[f32;2])->Result<f32,&'static str>{
        let c=self.contacts.get(&key).ok_or("unknown wave score contact")?;if c.impact_valid{return Ok(c.impact_score);}
        let (ids,g,_,_)=self.impact_contact(key)?;let old_rescue=c.impact_rescue;
        let (score,rescue)=if g.distance<=f32::from_bits(1041170142){
            let target=f32::from_bits(1003486810);let bias=if g.distance<target{(target-g.distance)*dt.recip()}else{0.};
            let compact=std::array::from_fn(|side|g.active[side]&&c.features[side].endpoint.feature.kind!=3);
            let angular=ids.map(|id|self.bodies.get(id).core.angular);
            let correction=compact_wave_bound(angular,radii,compact,dt.recip());
            let rescue=(correction+bias)*2.;
            let speed=|side:usize|{if !g.active[side]{return 0.;}let b=self.bodies.get(ids[side]).core;let n=g.normal;let j=g.jacobians[side];((((n.y*b.linear.y+n.x*b.linear.x)+n.z*b.linear.z)+j.y*b.angular.y)+j.x*b.angular.x)+j.z*b.angular.z};
            (g.distance-(rescue*0.5+(speed(0)-speed(1)))*dt,rescue)
        }else{(f32::from_bits(1621981420),old_rescue)};
        let c=self.contacts.get_mut(&key).unwrap();c.impact_score=score;c.impact_rescue=rescue;c.impact_valid=true;Ok(score)
    }
    pub(crate) fn wave_impact_bias(&self,key:u64)->Result<f32,&'static str>{let c=self.contacts.get(&key).ok_or("unknown wave bias contact")?;if !c.impact_valid{return Err("wave response lacks a scored cache");}Ok(c.impact_rescue)}
    fn shape(&self,f:&Feature)->refresh::Shape{let o=self.objects[&f.endpoint.object];refresh::Shape{kind:f.endpoint.feature.kind,vertices:f.vertices,canonical:f.canonical,rotation:o.rotation,translation:o.translation,radius:o.radius}}
    fn refresh_bodies(&self,features:&[Feature;2])->[refresh::Body;2]{features.each_ref().map(|f|{let b=self.bodies.get(self.objects[&f.endpoint.object].core);refresh::Body{flags:b.flags,inverse_mass:b.core.inverse_mass,inverse_inertia:b.core.inverse_inertia,rotation:rows(b.frame.rotation),position:b.frame.position,linear:b.core.linear,angular:b.core.angular}})}
    fn drain_physical_effects(&mut self){let(effects,contacts,features)=self.managed.take_effects();for e in &effects{if let Effect::ResetClock(core)=*e{self.bodies.reset_stamps(core,self.now);}}self.effects.extend(effects);self.released.extend(contacts);self.released_features.extend(features);}
    pub(crate) fn core_has_contact_system(&self,core:usize)->bool{self.managed.owner().first_system(core).is_some()}
    pub(crate) fn resolve(&mut self,flags:u32,features:[Feature;2],hint:Option<usize>)->Result<Option<u64>,&'static str>{Ok(self.resolve_created(flags,features,hint)?.0)}
    pub(crate) fn resolve_created(&mut self,flags:u32,features:[Feature;2],hint:Option<usize>)->Result<(Option<u64>,bool),&'static str>{
        for f in &features{if !self.objects.contains_key(&f.endpoint.object)||f.vertices.iter().chain(&f.canonical).any(|v|!v.is_finite())
            ||f.endpoint.feature.kind==2&&f.endpoint.feature.vertices!=f.vertices{return Err("invalid managed response feature");}}
        let selector=((flags>>8)&3) as usize;if selector>1{return Err("invalid contact response selector");}
        let selected=[features[selector].clone(),features[selector^1].clone()];if selected[0].endpoint.feature.kind==2{return Err("face-first refresh is unsupported");}
        let mut next=self.clone();let admission=next.managed.resolve(flags,next.now,features.each_ref().map(|f|f.endpoint.clone()),hint)?;
        if let Some(key)=admission.key{
            if admission.created{
                let initial=admission.initial.ok_or("missing contact constructor state")?;
                let owners=selected.each_ref().map(|f|{let i=next.objects[&f.endpoint.object].core;(next.bodies.get(i).flags&2==0).then_some(i)});
                let prepared=PreparedContact::prepare(refresh::State{time:initial.time,mix:initial.mix,marker:initial.marker,counter:initial.counter},selected.each_ref().map(|f|next.shape(f)),next.refresh_bodies(&selected),owners,next.now,&next.settings.refresh,||{})?;
                let geometry=prepared.geometry();let bound=mass::setup(initial.mode,std::array::from_fn(|side|{let b=next.bodies.get(next.objects[&selected[side].endpoint.object].core);mass::Core{flags:b.flags,inverse_mass:b.core.inverse_mass,inverse_inertia:b.core.inverse_inertia,arm:geometry.arms[side]}}),std::array::from_fn(|side|mass::Endpoint{manager:side as u32,code:0,default:mass::Material{key:side as u32,flags:0}}),|_,_,_|unreachable!())?;
                let friction=crate::ivp_material::clamped_product(next.objects[&selected[0].endpoint.object].friction,next.objects[&selected[1].endpoint.object].friction) as f32;
                let retained=RetainedContact::from_prepared(RetainedState{refresh:geometry.state,normal:History{force:initial.force,counter:initial.sign as i16},energy:initial.energy,work:initial.work,response_bound:bound.bound},owners,prepared)?;
                next.contacts.insert(key,Contact{features:selected,retained,friction,mode:bound.mode,phase:Phase::Refresh,impact_pushes:0,impact_valid:false,impact_rescue:0.,impact_score:0.});
            }else{
                //10104010 refreshes a lookup hit, but preserves its existing
                // endpoint descriptors, mass bound and numerical history.
                let stored=next.contacts.get(&key).ok_or("metadata lacks numerical contact")?.features.clone();let shapes=stored.each_ref().map(|f|next.shape(f));let bodies=next.refresh_bodies(&stored);
                let c=next.contacts.get_mut(&key).unwrap();c.retained.refresh(shapes,bodies,next.now,&next.settings.refresh,||{})?;
                // Lookup admission also allocates10105650's fresh transient cache.
                c.impact_pushes=0;c.impact_valid=false;
            }
        }
        next.drain_physical_effects();*self=next;Ok((admission.key,admission.created))
    }
    /// Authored raw-velocity kick and frozen-frame translation control. This
    /// is not a physical pose integrator or a replacement for collision search.
    pub(crate) fn advance(&mut self,core:usize,linear:Vec3,angular:Vec3,translation:Vec3)->Result<(),&'static str>{self.advance_many(&[StepChange{core,linear,angular,translation}])}
    /// Advance the clock once for simultaneous authored changes to several
    /// bodies. The surrounding physical integrator remains a separate owner.
    pub(crate) fn advance_many(&mut self,changes:&[StepChange])->Result<(),&'static str>{
        if !matches!(self.bodies,Bodies::Authored(_)){return Err("physical bodies require scheduled integration, not authored translation");}
        if changes.iter().any(|c|c.core>=self.bodies.len()||self.bodies.get(c.core).flags&2!=0||![c.linear,c.angular,c.translation].iter().all(|v|v.is_finite()))
            ||changes.iter().map(|c|c.core).collect::<std::collections::BTreeSet<_>>().len()!=changes.len()
            ||self.contacts.values().any(|c|c.phase!=Phase::Refresh){return Err("invalid response advance/order");}
        let mut next=self.clone();next.now+=next.settings.dt as f64;
        for c in changes{let Bodies::Authored(values)=&mut next.bodies else{unreachable!()};let b=&mut values[c.core];b.core.linear+=c.linear;b.core.angular+=c.angular;b.frame.position+=c.translation;for o in next.objects.values_mut().filter(|o|o.core==c.core){o.translation+=c.translation;}}
        if !next.now.is_finite()||next.bodies.iter().any(|b|!b.core.linear.is_finite()||!b.core.angular.is_finite()||!b.frame.position.is_finite())||next.objects.values().any(|o|!o.translation.is_finite()){return Err("contact response advance overflow");}
        *self=next;Ok(())
    }
    fn single(&self,system:usize,phase:Phase)->Result<u64,&'static str>{
        let ids=self.managed.owner().system_contacts(system)?;if ids.len()!=1{return Err("single-contact phase requires one contact");}
        let key=ids[0] as u64;if self.contacts.get(&key).ok_or("metadata lacks numerical contact")?.phase!=phase{return Err("contact numerical phase order mismatch");}Ok(key)
    }
    pub(crate) fn refresh(&mut self,system:usize)->Result<(),&'static str>{let mut next=self.clone();next.execute_in_candidate(system,crate::ivp_unit_registry::ContactPhase::Refresh)?;*self=next;Ok(())}
    pub(crate) fn tangent(&mut self,system:usize)->Result<(),&'static str>{let mut next=self.clone();next.execute_in_candidate(system,crate::ivp_unit_registry::ContactPhase::Tangent)?;*self=next;Ok(())}
    pub(crate) fn normal(&mut self,system:usize)->Result<bool,&'static str>{let mut next=self.clone();let retired=next.execute_in_candidate(system,crate::ivp_unit_registry::ContactPhase::Normal)?;*self=next;Ok(retired)}
    /// The containing world already owns the rollback candidate. Avoid a
    /// second full body/registry clone for each controller in that candidate.
    /// On error the containing candidate must be discarded, not committed.
    pub(crate) fn execute_in_candidate(&mut self,system:usize,requested:crate::ivp_unit_registry::ContactPhase)->Result<bool,&'static str>{
        use crate::ivp_unit_registry::ContactPhase;
        let phase=match requested{ContactPhase::Refresh=>Phase::Refresh,ContactPhase::Tangent=>Phase::Tangent,ContactPhase::Normal=>Phase::Normal};
        let ids=self.managed.owner().system_contacts(system)?;
        if ids.len()>1{
            if ids.iter().any(|&id|self.contacts[&(id as u64)].phase!=phase){return Err("contact numerical phase order mismatch");}
            return match requested{ContactPhase::Refresh=>{self.coupled_refresh(system)?;Ok(false)},ContactPhase::Tangent=>{self.coupled_tangent(system)?;Ok(false)},ContactPhase::Normal=>self.coupled_normal(system)};
        }
        let key=self.single(system,phase)?;
        match requested{
            ContactPhase::Refresh=>{let features=self.contacts[&key].features.clone();let shapes=features.each_ref().map(|f|self.shape(f));let bodies=self.refresh_bodies(&features);let c=self.contacts.get_mut(&key).unwrap();c.retained.refresh(shapes,bodies,self.now,&self.settings.refresh,||{})?;c.phase=Phase::Tangent;},
            ContactPhase::Tangent=>{let mut cores=self.bodies.iter().map(|b|b.core).collect::<Vec<_>>();let frames=self.bodies.iter().map(|b|b.frame).collect::<Vec<_>>();let k=self.settings;let c=self.contacts.get_mut(&key).unwrap();c.retained.respond_friction(&mut cores,&frames,&TangentSettings{dt:k.dt,inverse_dt:k.dt.recip(),friction:c.friction,response_bound:c.retained.state().response_bound,threshold_extra:k.threshold_extra,minimum_impulse:k.minimum_impulse,minimum_determinant_squared:k.minimum_determinant_squared,mode:c.mode,alternate_candidate:false})?;self.bodies.set_cores(cores);c.phase=Phase::Normal;},
            ContactPhase::Normal=>{let mut cores=self.bodies.iter().map(|b|b.core).collect::<Vec<_>>();let c=self.contacts.get_mut(&key).unwrap();let response=c.retained.respond_normal(&mut cores,self.settings.dt.recip())?;self.bodies.set_cores(cores);
                if response.retire{self.managed.remove(key)?;self.contacts.remove(&key);self.managed.destroy_empty(system)?;self.drain_physical_effects();}else{c.phase=Phase::Refresh;self.managed.repair_system(system)?;}return Ok(response.retire);}
        }Ok(false)
    }
    fn coupled_refresh(&mut self,system:usize)->Result<(),&'static str>{
        use crate::ivp_contact_pipeline::DistanceChange;
        let mut work=self.work.clone().ok_or("coupled refresh lacks pair work providers")?;
        let properties=&self.energy.as_ref().ok_or("coupled refresh lacks physical energy properties")?.0;
        let physical=properties.clone();let mut pairs=self.managed.owner().system_pairs(system)?.to_vec();
        for pair in pairs.iter_mut().rev(){let mut changes=Vec::new();for &id in pair.contacts.iter().rev(){let features=self.contacts[&(id as u64)].features.clone();let shapes=features.each_ref().map(|f|self.shape(f));let bodies=self.refresh_bodies(&features);let c=self.contacts.get_mut(&(id as u64)).unwrap();let before=c.retained.geometry().unwrap().distance;let force=c.retained.state().normal.force;let after=c.retained.refresh(shapes,bodies,self.now,&self.settings.refresh,||{})?.distance;c.phase=Phase::Tangent;changes.push(DistanceChange{before,after,force});}changes.reverse();pair.clock.geometry_work(&changes)?;}
        let core=self.managed.owner().system_cores(system)?.iter().copied().find(|&id|self.bodies.get(id).flags&2==0).ok_or("contact system has no dynamic core")?;
        let flags=self.managed.owner().registry().unit_flags(core)?.flags;
        for pair in &mut pairs{pair.clock.begin_update(flags);}
        for (index,pair) in pairs.iter_mut().enumerate().rev(){let mut bodies=pair.cores.map(|id|{let b=self.bodies.get(id);let p=physical[id];crate::ivp_dissipation::Body{flags:b.flags,mass:p.mass,inertia:p.inertia,rotation:b.frame.rotation,core:b.core}});
            pair.clock.refresh_work(flags&!0x3000,work.coefficient,pair.cores.map(|id|work.blocked[id]),&mut bodies,&mut work.environment_work,&work.settings)?;
            for (id,b) in pair.cores.into_iter().zip(bodies){self.bodies.set_core(id,b.core);}self.managed.set_pair_clock(system,index,pair.cores,pair.clock)?;
        }self.work=Some(work);Ok(())
    }
    fn coupled_tangent(&mut self,system:usize)->Result<(),&'static str>{
        let pairs=self.managed.owner().system_pairs(system)?.to_vec();let mut cores=self.bodies.iter().map(|b|b.core).collect::<Vec<_>>();let frames=self.bodies.iter().map(|b|b.frame).collect::<Vec<_>>();let k=self.settings;let mut completed=Vec::new();
        for (index,pair) in pairs.into_iter().enumerate().rev(){let mut ordered=pair.contacts.iter().map(|&id|self.contacts.remove(&(id as u64)).unwrap()).collect::<Vec<_>>();
            let settings=ordered.iter().map(|c|TangentSettings{dt:k.dt,inverse_dt:k.dt.recip(),friction:c.friction,response_bound:c.retained.state().response_bound,threshold_extra:k.threshold_extra,minimum_impulse:k.minimum_impulse,minimum_determinant_squared:k.minimum_determinant_squared,mode:c.mode,alternate_candidate:false}).collect::<Vec<_>>();
            let outcomes=RetainedContact::respond_group_friction(&mut ordered.iter_mut().map(|c|&mut c.retained).collect::<Vec<_>>(),&mut cores,&frames,&settings)?;let mut clock=pair.clock;clock.accumulate_batch_work(&outcomes)?;completed.push((index,pair,clock,ordered));
        }
        for (index,pair,mut clock,mut ordered) in completed{let keys=ordered.iter().map(|c|self.objects[&c.features[0].endpoint.object].core as u64+1).collect::<Vec<_>>();clock.redistribute(&mut ordered.iter_mut().map(|c|&mut c.retained).collect::<Vec<_>>(),&keys)?;self.managed.set_pair_clock(system,index,pair.cores,clock)?;for (id,mut c) in pair.contacts.into_iter().zip(ordered){c.phase=Phase::Normal;self.contacts.insert(id as u64,c);}}
        self.bodies.set_cores(cores);Ok(())
    }
    fn coupled_normal(&mut self,system:usize)->Result<bool,&'static str>{
        let ids=self.managed.owner().system_contacts(system)?.to_vec();
        if ids.len()>150{return Err("large-system anomaly provider is not connected");}
        let mut list=crate::ivp_lifetime::State{skipped:0,contacts:ids.iter().map(|&id|{let c=&self.contacts[&(id as u64)];let g=c.retained.geometry().unwrap();crate::ivp_lifetime::Contact{id,counter:c.retained.state().normal.counter,distance:g.distance,status:g.geometry.status,core_flags:c.features.each_ref().map(|f|self.bodies.get(self.objects[&f.endpoint.object].core).flags)}}).collect()};
        list.sort();self.managed.reorder(system,&list.contacts.iter().map(|c|c.id).collect::<Vec<_>>())?;let removed=list.prune(crate::ivp_lifetime::Settings{retire_distance:0.0285635684,target_distance:0.0126949195,extra_distance:6.34745957e-5});
        for id in removed{self.managed.remove(id as u64)?;self.contacts.remove(&(id as u64));}
        let order=list.contacts.iter().map(|c|c.id).collect::<Vec<_>>();self.managed.reorder(system,&order)?;
        self.drain_physical_effects();
        if order.is_empty(){self.managed.destroy_empty(system)?;return Ok(true);}
        let (properties,factor,bound)=self.energy.as_ref().ok_or("coupled response lacks physical energy properties")?;
        let participating=self.managed.owner().system_cores(system)?.iter().copied().filter(|&id|self.bodies.get(id).flags&2==0).collect::<Vec<_>>();
        let mut cores=self.bodies.iter().map(|b|b.core).collect::<Vec<_>>();let mut ordered=order.iter().map(|&id|self.contacts.remove(&(id as u64)).unwrap()).collect::<Vec<_>>();
        let report=crate::ivp_batch_normal::respond(&mut ordered.iter_mut().map(|c|&mut c.retained).collect::<Vec<_>>(),&mut cores,properties,&participating,self.settings.dt,*factor,*bound)?;
        self.bodies.set_cores(cores);for (&id,mut c) in order.iter().zip(ordered){c.phase=Phase::Refresh;self.contacts.insert(id as u64,c);}self.managed.repair_system(system)?;if report.solved{let index=usize::from(!report.fast);self.reports[index]=self.reports[index].saturating_add(1);if !report.committed{self.reports[2]=self.reports[2].saturating_add(1);}}Ok(false)
    }
    /// Low-level normal-controller control at prepared/frozen geometry. This
    /// bypasses phase scheduling only, never numerical state. It is excluded
    /// from optimized production builds; full batch refresh/tangent is separate.
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn normal_prepared_control(&mut self,system:usize,refresh:bool)->Result<bool,&'static str>{
        let mut next=self.clone();let ids=next.managed.owner().system_contacts(system)?.to_vec();
        if refresh{for &id in &ids{let f=next.contacts[&(id as u64)].features.clone();let shapes=f.each_ref().map(|f|next.shape(f));let bodies=next.refresh_bodies(&f);next.contacts.get_mut(&(id as u64)).unwrap().retained.refresh(shapes,bodies,next.now,&next.settings.refresh,||{})?;}}
        let retired=if ids.len()>1{next.coupled_normal(system)?}else{for &id in &ids{next.contacts.get_mut(&(id as u64)).unwrap().phase=Phase::Normal;}next.normal(system)?};*self=next;Ok(retired)
    }
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn batch_snapshot(&self,system:usize)->serde_json::Value{
        let order=self.managed.owner().system_contacts(system).unwrap_or(&[]);serde_json::json!({"order":order,"bodies":self.snapshot(1)["bodies"],"nodes":self.contacts.keys().filter(|&&id|self.managed.owner().binding(id as usize).is_ok_and(|b|b.system==system)).map(|&id|serde_json::json!({"id":id,"numeric":self.snapshot(id)})).collect::<Vec<_>>()})
    }
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn pipeline_snapshot(&self,system:usize)->serde_json::Value{let mut v=self.batch_snapshot(system);v["environment_work"]=serde_json::json!(self.work.as_ref().map(|w|w.environment_work).unwrap_or(0.));v["pairs"]=serde_json::json!(self.managed.owner().system_pairs(system).unwrap_or(&[]).iter().map(|p|serde_json::json!({"cores":p.cores,"contacts":p.contacts,"timer":p.clock.timer,"energy":p.clock.energy})).collect::<Vec<_>>());v}
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn batch_report_counts(&self)->[usize;3]{self.reports}
    /// Called after controller work by the containing PSI scheduler. Contact
    /// splits must not independently discard joint connectivity or rebuild
    /// clean unit controller vectors.
    pub(crate) fn finish_unit_graph(&mut self,core:usize)->Result<(),&'static str>{self.managed.finish_unit_graph(core)}
    pub(crate) fn set_physical_time(&mut self,now:f64)->Result<(),&'static str>{
        if !matches!(self.bodies,Bodies::Physical(_))||!now.is_finite()||self.contacts.values().any(|c|c.phase!=Phase::Refresh){return Err("invalid physical PSI clock boundary");}self.now=now;Ok(())
    }
    /// Containing ContactWorld owns the transaction and discards on failure.
    pub(crate) fn physical_impulses_in_candidate(&mut self,commands:&[crate::ivp_joint_tail::Command])->Result<(),&'static str>{
        if self.contacts.values().any(|c|c.phase!=Phase::Refresh){return Err("impulses require a complete contact step boundary");}
        let Bodies::Physical(bodies)=&mut self.bodies else{return Err("registered impulses require physical bodies");};
        self.managed.apply_registered_impulses(bodies,commands)
    }
    /// Containing physical owner supplies the transaction and constructor output.
    pub(crate) fn append_physical_body_in_candidate(&mut self,b:crate::ivp_joint_apply::Body,id:u64,o:Object,inertia:Vec3,blocked:bool)->Result<(),&'static str>{
        if self.contacts.values().any(|c|c.phase!=Phase::Refresh)||self.objects.contains_key(&id)||id==0||o.core!=b.id||b.id!=self.bodies.len()||!b.clock.is_finite()||!b.mass.is_finite()||b.mass<=0.||!b.current.position.is_finite()||!b.current.rotation.is_finite()||b.flags>>8!=8||b.object_flags&0xff!=if b.flags&2!=0{16}else{8}||!o.radius.is_finite()||o.radius<0.||!o.friction.is_finite()||o.material_flags!=0||!o.translation.is_finite()||o.rotation.iter().any(|v|!v.is_finite())||!inertia.is_finite()||inertia.min_element()<=0.||b.inverse.iter().any(|v|!v.is_finite()||*v<0.){return Err("invalid physical body append boundary");}
        let Bodies::Physical(bodies)=&mut self.bodies else{return Err("body append requires a physical bank");};
        let Some((energy,_,_))=&mut self.energy else{return Err("missing appended body energy owner");};let Some(work)=&mut self.work else{return Err("missing appended body pair-work owner");};
        if energy.len()!=bodies.len()||work.blocked.len()!=bodies.len(){return Err("incoherent body append property banks");}
        self.managed.register_ordinary_object(id,b.id,b.flags&2!=0)?;energy.push(crate::ivp_batch_normal::Physical{flags:b.flags,mass:b.mass,inertia});work.blocked.push(blocked);bodies.push(b);self.objects.insert(id,o);Ok(())
    }
    pub(crate) fn register_physical_group_in_candidate(&mut self,id:usize,cores:&[usize])->Result<(),&'static str>{
        if !matches!(self.bodies,Bodies::Physical(_))||self.contacts.values().any(|c|c.phase!=Phase::Refresh){return Err("group creation requires physical contact step boundary");}
        self.managed.register_physical_group(id,cores)?;self.drain_physical_effects();Ok(())
    }
    pub(crate) fn rebind_physical_group_in_candidate(&mut self,id:usize,cores:&[usize])->Result<(),&'static str>{
        if !matches!(self.bodies,Bodies::Physical(_))||self.contacts.values().any(|c|c.phase!=Phase::Refresh){return Err("group edit requires physical contact step boundary");}
        self.managed.rebind_registered_group(id,cores)?;self.drain_physical_effects();Ok(())
    }
    pub(crate) fn detach_physical_group_in_candidate(&mut self,id:usize)->Result<(),&'static str>{
        if !matches!(self.bodies,Bodies::Physical(_))||self.contacts.values().any(|c|c.phase!=Phase::Refresh){return Err("group detachment requires physical contact step boundary");}
        self.managed.detach_registered_group(id)?;self.drain_physical_effects();Ok(())
    }
    pub(crate) fn physical_wake_in_candidate(&mut self,id:usize)->Result<(),&'static str>{
        if !matches!(self.bodies,Bodies::Physical(_))||self.contacts.values().any(|c|c.phase!=Phase::Refresh){return Err("wake requires a physical contact step boundary");}
        self.managed.wake_registered_core(id)?;self.drain_physical_effects();Ok(())
    }
    pub(crate) fn physical_wake_core_in_candidate(&mut self,id:usize)->Result<(),&'static str>{
        if !matches!(self.bodies,Bodies::Physical(_))||self.contacts.values().any(|c|c.phase!=Phase::Refresh){return Err("member wake requires physical contact boundary");}
        self.managed.wake_registered_core_now(id)?;self.drain_physical_effects();Ok(())
    }
    pub(crate) fn physical_finish_wake_in_candidate(&mut self,id:usize)->Result<(),&'static str>{self.managed.finish_registered_wake(id)?;self.drain_physical_effects();Ok(())}
    pub(crate) fn physical_wake_now_in_candidate(&mut self,id:usize)->Result<(),&'static str>{
        if !matches!(self.bodies,Bodies::Physical(_))||self.contacts.values().any(|c|c.phase!=Phase::Refresh){return Err("immediate wake requires a physical contact boundary");}
        self.managed.wake_registered_unit_now(id)?;self.drain_physical_effects();Ok(())
    }
    pub(crate) fn begin_physical_psi(&mut self)->Result<(),&'static str>{
        if !matches!(self.bodies,Bodies::Physical(_)){return Err("physical PSI requires full body bank");}self.managed.begin_psi()?;self.drain_physical_effects();Ok(())
    }
    pub(crate) fn next_physical_unit(&mut self,poll:&mut crate::ivp_movement::Poll,
        mut prepare:impl FnMut(&mut [crate::ivp_joint_apply::Body],&[usize],&mut crate::ivp_unit::UnitFlags)->Result<(),&'static str>)->Result<Option<crate::ivp_unit_registry::UnitDispatch>,&'static str>{
        let Bodies::Physical(bodies)=&mut self.bodies else{return Err("physical dispatch requires full body bank");};
        self.managed.next_unit_dispatch(poll,|cores,flags|prepare(bodies,cores,flags))
    }
    pub(crate) fn finish_physical_unit(&mut self,work:crate::ivp_unit_registry::UnitDispatch,
        mut classify:impl FnMut(usize,&mut crate::ivp_joint_apply::Body)->Result<u8,&'static str>)->Result<(),&'static str>{
        let Bodies::Physical(bodies)=&mut self.bodies else{return Err("physical dispatch requires full body bank");};
        self.managed.finish_unit_dispatch(work,|id|classify(id,&mut bodies[id]))?;self.drain_physical_effects();Ok(())
    }
    pub(crate) fn registry(&self)->&Registry{self.managed.owner().registry()}
    pub(crate) fn system_ids(&self)->Vec<usize>{self.managed.owner().system_ids()}
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn world_snapshot(&self)->serde_json::Value{let ids=self.system_ids();serde_json::json!({"bodies":self.snapshot(1)["bodies"],"environment_work":self.work.as_ref().map(|w|w.environment_work).unwrap_or(0.),"systems":(0..self.managed.owner().system_slots()).map(|id|ids.contains(&id).then(||self.pipeline_snapshot(id))).collect::<Vec<_>>()})}
    pub(crate) fn take_effects(&mut self)->(Vec<Effect>,Vec<usize>,Vec<[Endpoint;2]>){(std::mem::take(&mut self.effects),std::mem::take(&mut self.released),std::mem::take(&mut self.released_features))}
    #[cfg(test)]
    pub(crate) fn body_owners_snapshot(&self)->serde_json::Value{serde_json::json!({"energy":self.energy.as_ref().map(|(v,_,_)|v.iter().map(|p|serde_json::json!({"flags":p.flags,"mass":p.mass.to_bits(),"inertia":p.inertia.to_array().map(f32::to_bits)})).collect::<Vec<_>>()),"blocked":self.work.as_ref().map(|w|&w.blocked),"objects":self.objects.iter().map(|(id,o)|serde_json::json!({"id":id,"core":o.core,"rotation":o.rotation.map(|v|v.to_array().map(f32::to_bits)),"translation":o.translation.to_array().map(f32::to_bits),"radius":o.radius.to_bits(),"flags":o.material_flags,"friction":o.friction.to_bits()})).collect::<Vec<_>>()})}
    #[cfg(test)]
    pub(crate) fn impact_snapshot(&self)->serde_json::Value{serde_json::json!(self.contacts.iter().map(|(&key,c)|serde_json::json!({"key":key,"valid":c.impact_valid,"rescue":c.impact_rescue.to_bits(),"score":c.impact_score.to_bits(),"pushes":c.impact_pushes})).collect::<Vec<_>>())}
    #[cfg(any(test,debug_assertions))]
    pub(crate) fn diagnostics(&self)->serde_json::Value{serde_json::json!({"metadata":self.managed.snapshot(),"time":self.now,"phases":self.contacts.iter().map(|(&id,c)|(id,match c.phase{Phase::Refresh=>0,Phase::Tangent=>1,Phase::Normal=>2})).collect::<Vec<_>>(),"effects":self.effects.iter().map(|e|format!("{e:?}")).collect::<Vec<_>>(),"released":self.released,"released_features":self.released_features.iter().map(|s|s.each_ref().map(|e|e.object)).collect::<Vec<_>>()})}
    #[cfg(any(test,debug_assertions))]
    #[cfg(test)]
    pub(crate) fn contact_state_snapshot(&self)->serde_json::Value{
        let mut rows=self.contacts.values().map(|c|{let s=c.retained.state();serde_json::json!({"objects":c.features.each_ref().map(|f|f.endpoint.object),"time":s.refresh.time.to_bits(),"mix":s.refresh.mix.map(f32::to_bits),"marker":s.refresh.marker,"counter":s.refresh.counter,"force":s.normal.force.to_bits(),"sign":s.normal.counter,"energy":s.energy.to_bits(),"work":s.work.to_bits(),"bound":s.response_bound.to_bits(),"friction":c.friction.to_bits(),"mode":c.mode,"distance":c.retained.geometry().unwrap().distance.to_bits()})}).collect::<Vec<_>>();rows.sort_by_key(|r|r.to_string());serde_json::json!(rows)
    }
    pub(crate) fn snapshot(&self,key:u64)->serde_json::Value{
        use serde_json::json;let mut out=json!({"alive":self.contacts.contains_key(&key),"bodies":self.bodies.iter().map(|b|json!({"flags":b.flags,"inverse_mass":b.core.inverse_mass,"inverse_inertia":b.core.inverse_inertia.to_array(),"rotation":rows(b.frame.rotation).map(|v|v.to_array()),"position":b.frame.position.to_array(),"linear":b.core.linear.to_array(),"angular":b.core.angular.to_array(),"pending_linear":b.core.pending_linear.to_array(),"pending_angular":b.core.pending_angular.to_array(),"stamps":b.stamps})).collect::<Vec<_>>()});
        if let Some(c)=self.contacts.get(&key){let s=c.retained.state();let g=c.retained.geometry().unwrap();out["state"]=json!({"time":s.refresh.time,"mix":s.refresh.mix,"marker":s.refresh.marker,"counter":s.refresh.counter,"force":s.normal.force,"sign":s.normal.counter,"energy":s.energy,"work":s.work,"bound":s.response_bound,"friction":c.friction,"mode":c.mode});out["geometry"]=json!({"point":g.point.to_array(),"normal":g.normal.to_array(),"basis":g.basis.map(|v|v.to_array()),"distance":g.distance,"response":g.response,"inverse_response":g.inverse_response,"status":g.geometry.status,"arms":(0..2).map(|i|g.active[i].then(||g.arms[i].to_array())).collect::<Vec<_>>(),"jacobians":(0..2).map(|i|g.active[i].then(||g.jacobians[i].to_array())).collect::<Vec<_>>()});}out
    }
}
