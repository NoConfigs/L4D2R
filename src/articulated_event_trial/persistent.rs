//! Unseeded diagnostic contact dispatch for the controlled flat-floor drop.
//! CCD admission/feature selection and impact retries are still the trial's
//! approximate search, not the native minimizer/contact graph. Never enabled
//! by the playable solver, and never reads native contact/velocity snapshots.
use super::*;
use crate::ivp_contact::{Core,NormalSystem};
use crate::ivp_contact_pipeline::{CoreFrame,FrictionGroup,PreparedContact,RetainedContact,RetainedState,TangentSettings};
use crate::{ivp_refresh as refresh,ivp_history,ivp_lifetime as lifetime};
use l4d2_formats::phy::{Convex,CompactEdgeId,PhySolid};
use crate::ivp_world_cache::{WorldCache,Context,Lease};

struct Contact {id:usize,vertex:usize,floor:usize,edge:CompactEdgeId,solver:RetainedContact}
struct SurfaceAdmission {objects:[usize;2],pairs:Vec<crate::ivp_surface_admission::Pair>}
#[path="persistent_pairs.rs"]
mod pair_world;
#[derive(Clone,Copy)]
struct PairMotion{axis:Vec3,speed:f32,bound:f32,velocity:Vec3,linear:f32}
impl Default for PairMotion{fn default()->Self{Self{axis:Vec3::X,speed:0.,bound:0.,velocity:Vec3::ZERO,linear:0.}}}
#[derive(Clone,Copy)]
struct PairQueue{kind:u32}
const PSI_OWNER:usize=usize::MAX;
struct DetailedPair {id:usize,owner:usize,surface:crate::ivp_surface_admission::Pair,query:crate::ivp_minimizer::Query,hulls:[std::sync::Arc<l4d2_formats::phy::CompactTopology>;2],geometry:crate::ivp_far_distance::Geometry,is_far:bool,queued:Option<PairQueue>}
pub(crate) struct PersistentTrial {
    pub(super) deferred_interval:Option<Interval>,frame_cache:WorldCache,cache_context:Context,bounds_world:crate::ivp_trial_bounds::World,activation_cache:bool,floor_present:bool,
    moving:Vec<Convex>,floor:Vec<Convex>,vertices:Vec<(usize,CompactEdgeId)>,
    surface_trees:[std::sync::Arc<l4d2_formats::phy::CompactTree>;2],surface_radii:[f32;2],surface_pairs:std::collections::BTreeMap<usize,SurfaceAdmission>,
    detailed_pairs:Vec<DetailedPair>,mindist_graph:crate::ivp_mindist_graph::Graph,pair_queue:crate::ivp_event_queue::Queue,psi_open:bool,
    pair_motions:[PairMotion;2],pair_epoch:f64,pair_deadline:f64,pair_dt:f32,pair_counter:u32,
    movement:crate::ivp_movement::History,movement_state:u8,wake_queued:bool,poll:crate::ivp_movement::Poll,previous_prediction:Option<Prediction>,
    raw_linear:Vec3,raw_angular:Vec3,motion_initialized:bool,initial_activation:bool,core_low_flags:u16,impact_count:u16,
    unit:crate::ivp_unit::UnitFlags,pending_linear:Vec3,pending_angular:Vec3,environment_work:f32,geometry_delta:f32,floor_inertia:Vec3,pub(super) moving_inertia:Vec3,pub(super) angular_radius:f32,
    contacts:Vec<Contact>,friction_order:Vec<usize>,friction_group:Option<FrictionGroup>,next_id:usize,pub clock:f64,
}
fn settings()->refresh::Settings{refresh::Settings{epsilon:1.00000001e-10,bary_bias:f32::MIN_POSITIVE,area_epsilon:1.000000045813705e-18,one:1.,opposite:-1.}}
fn rows(m:Mat3)->[Vec3;3]{let r=m.transpose();[r.x_axis,r.y_axis,r.z_axis]}
fn pairs_snapshot(p:&[crate::ivp_surface_admission::Pair])->Vec<serde_json::Value>{p.iter().map(|p|serde_json::json!({"slots":p.slots,"ledges":p.ledges})).collect()}
impl PersistentTrial {
    pub(crate) fn new(moving:Vec<PhySolid>,floor:Vec<PhySolid>,body:&mut Rigid,dt:f32,inertia_scale:f32,floor_present:bool)->Self{
        assert_eq!(moving.len(),1,"controlled one-solid trial");
        assert_eq!(floor.len(),1,"controlled one-solid floor");
        let surface_trees=[moving[0].compact_tree.clone().expect("owned moving compact tree"),floor[0].compact_tree.clone().expect("owned floor compact tree")];
        let center=Vec3::from_array(moving[0].mass_properties.expect("owned moving mass properties").center_ivp.to_array())*BASIS;
        let moving_inertia=crate::mass_properties::phy_inertia(&moving[0],body.inverse_mass.recip(),inertia_scale,0.1).expect("owned moving inertia");
        assert_eq!(body.inverse_inertia,Mat3::from_diagonal(moving_inertia.recip()),"bound inverse must agree with retained authored inertia");
        let compact_bounds=crate::ivp_bounds::Compact::from(moving[0].bounds.expect("owned compact bounds")).at(center*BASIS).expect("finite compact bounds");
        let angular_radius=compact_bounds.angular_radius;
        let vertices=&body.vertices;
        let floor_center=Vec3::from_array(floor[0].mass_properties.expect("owned floor centre").center_ivp.to_array());
        let floor_radius=crate::ivp_bounds::Compact::from(floor[0].bounds.expect("owned floor compact bounds")).at(floor_center).expect("finite floor bounds").radius;
        let floor_inertia=Vec3::from_array(floor[0].mass_properties.expect("owned floor mass properties").unit_inertia_ivp.to_array())*10.;
        let moving=moving.into_iter().flat_map(|s|s.convexes).collect::<Vec<_>>();
        let floor=floor.into_iter().flat_map(|s|s.convexes).collect::<Vec<_>>();
        let mapping=vertices.iter().map(|vertex|{
            moving.iter().enumerate().find_map(|(c,h)|{
                let t=h.topology.as_ref()?;
                t.edges.iter().enumerate().find_map(|(i,e)|e.iter().enumerate().find_map(|(j,_)|{
                    let id=CompactEdgeId{triangle:i,edge:j};
                    ((Vec3::from_array(t.vertex(id)?)*BASIS).distance_squared(*vertex)<1e-10).then_some((c,id))
                }))
            }).expect("renderer vertex must map to actual owned compact topology")
        }).collect::<Vec<_>>();
        // CCD, impulses and retained refresh must share the actual compact
        //metre coordinates. The render-facing inch round trip loses ulps.
        let origin=body.position-body.rotation*body.center;
        body.vertices=mapping.iter().map(|&(c,id)|Vec3::from_array(moving[c].topology.as_ref().unwrap().vertex(id).unwrap())*BASIS).collect();
        body.center=center;body.position=origin+body.rotation*center;
        // The controlled helper advances its empty environment twice before
        // adding the body. No later native sample supplies this trial's clock.
        let mut frame_cache=WorldCache::new(256).unwrap();
        let moving_core=frame_cache.register_core(Prediction{start:body.position*BASIS,velocity:Vec3::ZERO,rotation_start:q_native(body.rotation),rotation_end:q_native(body.rotation),time:2.*dt as f64,inverse_dt:dt.recip()});
        assert_eq!(frame_cache.register_object(moving_core,0x108,ObjectFrame{shift:Some(-center*BASIS),rotation:None}).unwrap(),0);
        if floor_present{let floor_core=frame_cache.register_core(Prediction{start:floor_center,velocity:Vec3::ZERO,rotation_start:[0.,0.,0.,1.],rotation_end:[0.,0.,0.,1.],time:0.,inverse_dt:66.});
        assert_eq!(frame_cache.register_object(floor_core,0x110,ObjectFrame{shift:Some(-floor_center),rotation:None}).unwrap(),1);}
        let mut objects=vec![crate::ivp_trial_bounds::Object::new(0,0x108,false,compact_bounds.radius,(body.position*BASIS).to_array())];
        if floor_present{objects.push(crate::ivp_trial_bounds::Object::new(1,0x110,true,floor_radius,floor_center.to_array()));}
        let bounds_world=crate::ivp_trial_bounds::World::new(objects,if floor_present{&[1,0]}else{&[0]},dt).unwrap();
        let mut pair_queue=crate::ivp_event_queue::Queue::new(16,2.*dt as f64).unwrap();
        if Self::uses_pair_queue(){pair_queue.insert(PSI_OWNER,-0.).unwrap();}
        Self{pair_motions:[PairMotion::default();2],pair_epoch:2.*dt as f64,pair_deadline:3.*dt as f64,pair_dt:dt,pair_counter:0,pair_queue,psi_open:false,detailed_pairs:vec![],mindist_graph:crate::ivp_mindist_graph::Graph::new(if floor_present{2}else{1}),surface_trees,surface_radii:[compact_bounds.radius,floor_radius],surface_pairs:std::collections::BTreeMap::new(),bounds_world,raw_linear:Vec3::ZERO,raw_angular:Vec3::ZERO,motion_initialized:false,initial_activation:false,core_low_flags:0,impact_count:0,deferred_interval:None,frame_cache,cache_context:Context{time:dt as f64+dt as f64*0.99f32 as f64,frame:4},activation_cache:false,floor_present,movement:crate::ivp_movement::History{close_time:2.*dt as f64,far_time:2.*dt as f64,close_position:Vec3::ZERO,far_position:Vec3::ZERO,close_quat:[0.;4],far_quat:[0.;4]},movement_state:1,wake_queued:false,poll:crate::ivp_movement::Poll{remaining:10,seed:1},previous_prediction:None,unit:crate::ivp_unit::UnitFlags::dormant(),pending_linear:Vec3::ZERO,pending_angular:Vec3::ZERO,environment_work:0.,geometry_delta:0.,floor_inertia,moving_inertia,angular_radius,moving,floor,vertices:mapping,contacts:vec![],friction_order:vec![],friction_group:None,next_id:1,clock:2.*dt as f64}
    }
    /// The owned control performs Wake followed by SetVelocity before its
    ///first Simulate.100e9f90 queues the input while raw motion remains zero.
    ///Only the initial source launch state supplies these values.
    pub(crate) fn initialize_queued_motion(&mut self,b:&Rigid)->Result<(),&'static str>{
        if self.motion_initialized||!b.velocity.is_finite()||!b.angular.is_finite()||!b.rotation.is_finite(){return Err("invalid initial queued motion");}
        self.pending_linear=b.velocity*BASIS;self.pending_angular=(b.rotation.conjugate()*b.angular)*BASIS;
        self.raw_linear=Vec3::ZERO;self.raw_angular=Vec3::ZERO;self.motion_initialized=true;self.initial_activation=true;self.movement_state=8;self.wake_queued=true;Ok(())
    }
    pub(super) fn record_interval_motion(&mut self,f:&Interval){
        if let Some((v,w))=f.held_motion{
            self.raw_linear=Vec3::ZERO;self.raw_angular=Vec3::ZERO;self.pending_linear=v;self.pending_angular=w;
        }else{self.raw_linear=f.prediction.velocity;self.raw_angular=f.angular;}
        self.core_low_flags=f.impact_budget.flags&0xff;self.impact_count=f.impact_budget.count;
    }
    pub(super) fn published_motion(&self)->(Vec3,Vec3,Vec3){(self.raw_linear+self.pending_linear,self.raw_angular+self.pending_angular,self.raw_angular)}
    pub(crate) fn core_motion(&self)->serde_json::Value{
        assert!(self.motion_initialized);
        let mut result=serde_json::json!({"raw_linear_ivp":self.raw_linear.to_array(),"core_angular_local":self.raw_angular.to_array(),"pending_linear_ivp":self.pending_linear.to_array(),"pending_angular_local":self.pending_angular.to_array(),"core_flags":((self.movement_state as u16)<<8)|self.core_low_flags|if self.wake_queued{4}else{0},"impact_count":self.impact_count,"unit_flags":self.unit.flags});
        if let Some(p)=self.previous_prediction{result["diagnostic_prediction"]=serde_json::json!({"start":p.start.to_array(),"velocity":p.velocity.to_array(),"rotation_start":p.rotation_start,"rotation_end":p.rotation_end,"time":p.time,"inverse_dt":p.inverse_dt});}
        let(slots,objects,_)=self.frame_cache.metadata();let binding=objects[0].cache;
        result["registered_objects"]=serde_json::json!(objects.len());
        result["object_flags"]=serde_json::json!(objects[0].flags);
        result["object_cache_bound"]=serde_json::json!(binding.is_some());
        result["object_cache_frame"]=serde_json::json!(binding.map_or(0,|i|slots[i].frame_stamp));
        result["object_cache_references"]=serde_json::json!(binding.map_or(0,|i|slots[i].references));
        result["object_cache_owner"]=serde_json::json!(binding.is_some_and(|i|slots[i].owner==Some(0)));
        result["environment_frame"]=serde_json::json!(self.cache_context.frame);
        result
    }
    pub(super) fn begin_tick(&mut self,velocity:&mut Vec3,angular:&mut Vec3)->Result<(),&'static str>{
        // Public velocity includes queued motion. Restore actual raw state
        //before100f13b0; using the public sum here would commit it twice.
        assert!(self.motion_initialized);*velocity=self.raw_linear;*angular=self.raw_angular;
        self.core_low_flags=0;self.impact_count=0;
        *velocity+=self.pending_linear;*angular+=self.pending_angular;
        self.pending_linear=Vec3::ZERO;self.pending_angular=Vec3::ZERO;self.raw_linear=*velocity;self.raw_angular=*angular;
        self.unit.activate();if self.unit.classify(&[*velocity],1.)?.reset_movement_clock{self.movement.reset_clock(self.clock)?;}Ok(())
    }
    pub(super) fn refresh_work(&mut self,b:&Rigid,pose:crate::ivp_motion::Pose,velocity:Vec3,angular:Vec3,dt:f32)->Result<Option<serde_json::Value>,&'static str>{
        //100ffa70 bypasses the group-work path for a lone contact.
        if self.contacts.len()<2{return Ok(None);}
        let inverse=Vec3::new(b.inverse_inertia.x_axis.x,b.inverse_inertia.y_axis.y,b.inverse_inertia.z_axis.z);
        let moving=Core{inverse_mass:b.inverse_mass,inverse_inertia:inverse,linear:velocity,angular,pending_linear:self.pending_linear,pending_angular:self.pending_angular};
        let fixed=Core{inverse_mass:0.1,inverse_inertia:self.floor_inertia.recip(),..Default::default()};
        let mut bodies=[crate::ivp_dissipation::Body{flags:256,mass:b.inverse_mass.recip(),inertia:self.moving_inertia,rotation:pose.rotation,core:moving},
            crate::ivp_dissipation::Body{flags:2050,mass:10.,inertia:self.floor_inertia,rotation:Mat3::IDENTITY,core:fixed}];
        let k=crate::ivp_dissipation::Settings{epsilon:1e-10,one:1.,opposite:-1.,two:2.,half:0.5,huge:10000.,small:0.0001,fraction:0.1};
        let coefficient=crate::ivp_unit::timestep(dt,0.9f32 as f64)?.work_decay;
        let group=self.friction_group.as_mut().ok_or("contacts have no refresh group")?;let before=group.energy;
        let outcome=group.refresh_work(self.unit.flags,coefficient,[false;2],&mut bodies,&mut self.environment_work,&k)?;
        self.pending_linear=bodies[0].core.pending_linear;self.pending_angular=bodies[0].core.pending_angular;
        Ok(Some(serde_json::json!({"kind":"retained_work_refresh","unit_flags":self.unit.flags,"energy_before":before,"geometry_delta":self.geometry_delta,"energy_after":group.energy,"consumed":outcome.as_ref().map(|o|o.consumed),"environment_work":self.environment_work,"pending_linear":self.pending_linear.to_array(),"pending_angular":self.pending_angular.to_array()})))
    }
    pub(super) fn prepare_forces(&mut self,velocity:&mut Vec3,angular:&mut Vec3,dt:f32,linear_rate:f32,angular_rate:f32,gravity:Vec3)->Result<serde_json::Value,&'static str>{
        let mut core=Core{linear:*velocity,angular:*angular,pending_linear:self.pending_linear,pending_angular:self.pending_angular,..Default::default()};
        let before=core;
        crate::ivp_damping::prepare_forces(&mut core,dt,linear_rate,Vec3::splat(angular_rate),self.movement_state,gravity)?;
        *velocity=core.linear;*angular=core.angular;self.pending_linear=core.pending_linear;self.pending_angular=core.pending_angular;self.raw_linear=core.linear;self.raw_angular=core.angular;
        Ok(serde_json::json!({"kind":"retained_force_prepare","movement_state":self.movement_state,"linear_before":before.linear.to_array(),"angular_before":before.angular.to_array(),"pending_linear_before":before.pending_linear.to_array(),"pending_angular_before":before.pending_angular.to_array(),"linear_after":core.linear.to_array(),"angular_after":core.angular.to_array(),"pending_linear_after":core.pending_linear.to_array(),"pending_angular_after":core.pending_angular.to_array()}))
    }
    /// Public100971e0/100e9f90: queue a Source-world central impulse.
    /// Dormant activation is deferred until the following simulation tick.
    pub(crate) fn apply_central_impulse(&mut self,impulse_source:Vec3,inverse_mass:f32)->Result<(),&'static str>{
        if !impulse_source.is_finite()||!inverse_mass.is_finite()||inverse_mass<=0.{return Err("invalid central impulse");}
        if !self.is_sleeping(){return Err("wake trial requires a sleeping core");}
        let impulse=Vec3::new(impulse_source.x,-impulse_source.z,impulse_source.y)*0.0254*inverse_mass;
        if !(self.pending_linear+impulse).is_finite(){return Err("central impulse overflow");}
        self.pending_linear+=impulse;
        self.wake_queued=true;
        Ok(())
    }
    pub(super) fn activate_queued(&mut self,b:&mut Rigid)->Option<serde_json::Value>{
        if !self.wake_queued{return None;}
        let initial=self.initial_activation;self.initial_activation=false;self.activation_cache=true;self.frame_cache.set_flags(0,0x101).unwrap();
        self.wake_queued=false;self.movement_state=1;self.unit.activate();self.raw_linear=Vec3::ZERO;self.raw_angular=Vec3::ZERO;self.core_low_flags=0;self.movement.reset_clock(self.clock).unwrap();
        //100f1550 +100f02f0 retain the dormant predictor origin/qStart.
        // The physical sleep endpoint differs from that retained origin.
        if let Some(pred)=self.previous_prediction.as_mut(){
            pred.velocity=Vec3::ZERO;pred.rotation_end=pred.rotation_start;pred.time=self.clock;
            b.position=pred.start*BASIS;b.rotation=q_render(pred.rotation_start);
        }
        b.velocity=Vec3::ZERO;b.angular=Vec3::ZERO;b.pose_cache=None;
        Some(serde_json::json!({"kind":if initial{"retained_initial_activation"}else{"retained_wake"},"time":self.clock,"pending_linear":self.pending_linear.to_array(),"position":(b.position*BASIS).to_array(),"rotation":q_native(b.rotation)}))
    }
    pub(crate) fn public_time(&self,dt:f32)->f64{self.clock-dt as f64+dt as f64*0.99f32 as f64}
    pub(crate) fn is_sleeping(&self)->bool{self.movement_state==8}
    pub(super) fn sleep_hold(&mut self,dt:f32)->serde_json::Value{let time=self.clock;self.advance_cache_clock(time);self.advance_cache_clock(time+dt as f64*0.99f32 as f64);self.clock+=dt as f64;serde_json::json!({"kind":"retained_sleep_hold","time":time})}
    pub(super) fn remember_prediction(&mut self,mut p:Prediction,epoch:f64){p.time+=epoch;self.previous_prediction=Some(p);}
    pub(super) fn advance_cache_clock(&mut self,at:f64){
        assert!(at.is_finite()&&at>=self.cache_context.time,"cache clock moved backwards");
        if at!=self.cache_context.time{self.cache_context.frame=self.cache_context.frame.wrapping_add(1);self.cache_context.time=at;}
    }
    pub(super) fn cache_from_prediction(&mut self,p:Prediction,at:f64,_center:Vec3){
        self.advance_cache_clock(at);self.frame_cache.update_core(0,p).unwrap();
        self.frame_cache.set_flags(0,0x101).unwrap();
        self.frame_cache.access(0,self.cache_context).unwrap();
    }
    pub(super) fn feature_frames(&mut self,mut prediction:Prediction,epoch:f64,at:f64)->Result<[crate::ivp_motion::Pose;2],&'static str>{
        // Native feature changes recompute their closest geometry through
        // retained object-cache frames. They do not restamp core prediction.
        prediction.time+=epoch;self.advance_cache_clock(at);self.frame_cache.update_core(0,prediction)?;
        let leases=self.frame_cache.retain_pair([0,1],self.cache_context)?;
        let frames=(||Ok([self.frame_cache.lease_frame(&leases.0)?.pose,self.frame_cache.lease_frame(&leases.1)?.pose]))();
        self.frame_cache.with_leases(leases,|release|{release();frames})
    }
    fn with_pairs(&mut self,run:impl FnOnce(&mut crate::ivp_trial_bounds::World,&mut pair_world::Context<'_>)->Result<(),&'static str>)->Vec<serde_json::Value>{
        let mut context=pair_world::Context{cache:&mut self.frame_cache,context:self.cache_context,trees:&self.surface_trees,radii:self.surface_radii,moving:&self.moving,floor:&self.floor,
            admissions:&mut self.surface_pairs,records:&mut self.detailed_pairs,graph:&mut self.mindist_graph,queue:&mut self.pair_queue,motions:self.pair_motions,epoch:self.pair_epoch,deadline:self.pair_deadline,dt:self.pair_dt,counter:&mut self.pair_counter,events:vec![]};
        run(&mut self.bounds_world,&mut context).unwrap_or_else(|error|panic!("source pair world at {}: {error}",self.cache_context.time));context.events
    }
    pub(super) fn bounds_prediction(&mut self,mut prediction:Prediction,epoch:f64,at:f64,remaining_dt:f32,sweep:(Vec3,f32,f32),full_dt:f32)->Vec<serde_json::Value>{
        assert_eq!(self.pair_queue.epoch,epoch,"pair prediction lost PSI queue epoch");
        prediction.time+=epoch;self.advance_cache_clock(at);self.frame_cache.update_core(0,prediction).unwrap();self.pair_epoch=epoch;self.pair_deadline=epoch+full_dt as f64;self.pair_dt=full_dt;
        let linear=crate::ivp_bounds::speed(prediction.velocity);self.pair_motions[0]=PairMotion{axis:sweep.0,speed:sweep.1,bound:sweep.2,velocity:prediction.velocity,linear};
        let center=prediction.sample(at,ObjectFrame{shift:None,rotation:None}).position.to_array();
        self.with_pairs(|world,context|{
            let requests=world.step_with_hooks(0,at,remaining_dt,center,linear,sweep.2,context)?;context.audit(requests);
            let reaction=Self::uses_pair_queue()&&at!=epoch;
            //100fae10: prediction,1010f870 bounds dispatch,1010b6c0
            // ordinary active queries, then1010c450 event scheduling.
            if !reaction{context.refresh_active()?;}
            context.predict_active(world,reaction)?;Ok(())
        })
    }
    // Controlled diagnostic worlds now use their own compact-pair queue.
    // Keep the old approximate discovery path explicitly selectable for audits.
    pub(super) fn uses_pair_queue()->bool{std::env::var_os("L4D2R_PHY_LEGACY_EVENT_TRIAL").is_none()}
    fn next_pair_queue(&self)->Option<(usize,f64)>{
        self.pair_queue.first().filter(|&(owner,_)|owner!=PSI_OWNER)
    }
    pub(super) fn pair_queue_before(&self,end:f64)->Option<(f64,bool)>{
        let (id,at)=self.next_pair_queue()?;if at>end{return None;}
        let q=self.detailed_pairs.iter().find(|p|p.id==id)?.queued?;Some((at,q.kind==33))
    }
    pub(super) fn next_pair_event(&mut self,prediction:Prediction,epoch:f64,cutoff:f64,events:&mut Vec<serde_json::Value>)->Option<Event>{
        for _ in 0..4096{
            let (id,at)=self.next_pair_queue()?;if at>cutoff{return None;}
            assert!(at>=self.cache_context.time,"pair queue moved backwards");
            self.advance_cache_clock(at);self.frame_cache.update_core(0,absolute(&prediction,epoch)).unwrap();
            let mut physical=false;events.extend(self.with_pairs(|world,context|{physical=context.dispatch_queued(world,id)?;Ok(())}));
            if !physical{continue;}
            let pair=self.detailed_pairs.iter().find(|p|p.id==id).expect("physical pair lost ownership");
            assert_eq!(pair.surface.slots,[0,1],"controlled point/floor queue orientation");
            assert_eq!(pair.query.kinds,[0,2]);
            let raw=pair.hulls[0].vertex(pair.query.vertex).expect("physical compact vertex");
            let vertex=self.vertices.iter().position(|&(c,e)|self.moving[c].topology.as_ref().unwrap().vertex(e)==Some(raw)).expect("physical vertex missing from rigid body");
            let normal=pair.geometry.normal*BASIS;
            assert!(normal.dot(Vec3::Y)>0.9999,"controlled floor queue normal");
            return Some(Event{pair:Some(id),time:at-epoch,vertex,feature:None,normal,plane:0.,surface:None});
        }
        panic!("source pair queue failed to advance after 4096 callbacks")
    }
    pub(super) fn begin_psi(&mut self,dt:f32)->Option<serde_json::Value>{
        assert!(!self.psi_open,"PSI callback reentered before prediction completed");
        if !Self::uses_pair_queue(){
            // The explicit approximate backend does not consume native pair
            // events. Clear its diagnostic entries at the next comparison PSI.
            for p in &mut self.detailed_pairs{if p.queued.take().is_some(){self.pair_queue.cancel(p.id).unwrap();}}
            self.pair_queue.advance_empty_epoch(self.clock).unwrap();
            self.pair_epoch=self.clock;self.pair_deadline=self.clock+dt as f64;self.pair_dt=dt;return None;
        }
        let before=self.pair_queue.snapshot();
        assert_eq!(self.pair_queue.pop().unwrap(),Some((PSI_OWNER,self.clock)),"PSI callback lost list-head ownership");
        self.pair_deadline=crate::ivp_event_world::begin_prediction(&mut self.pair_queue,self.clock,dt).unwrap();
        self.pair_epoch=self.clock;self.pair_dt=dt;self.psi_open=true;
        Some(serde_json::json!({"kind":"psi_queue_begin","time":self.clock,"deadline":self.pair_deadline,"before":before,"after":self.pair_queue.snapshot()}))
    }
    pub(super) fn end_psi_prediction(&mut self)->Option<serde_json::Value>{
        if !Self::uses_pair_queue(){return None;}
        assert!(self.psi_open,"PSI prediction completed without its callback owner");
        crate::ivp_event_world::end_prediction(&mut self.pair_queue,PSI_OWNER,self.pair_deadline).unwrap();self.psi_open=false;
        Some(serde_json::json!({"kind":"psi_queue_prediction_complete","time":self.pair_epoch,"deadline":self.pair_deadline,"queue":self.pair_queue.snapshot()}))
    }
    pub(super) fn cache_for_psi(&mut self,b:&Rigid,dt:f32)->Vec<serde_json::Value>{
        let p=self.previous_prediction.unwrap_or(Prediction{start:b.position*BASIS,velocity:Vec3::ZERO,rotation_start:q_native(b.rotation),rotation_end:q_native(b.rotation),time:self.clock,inverse_dt:dt.recip()});
        self.advance_cache_clock(self.clock);self.frame_cache.update_core(0,p).unwrap();self.frame_cache.set_flags(0,0x101).unwrap();self.pair_epoch=self.clock;self.pair_deadline=self.clock+dt as f64;self.pair_dt=dt;
        let mut events=Vec::new();
        if self.activation_cache{
            self.pair_motions[0]=PairMotion::default();self.frame_cache.set_flags(0,0x121).unwrap();let now=self.clock;let center=(b.position*BASIS).to_array();
            events.extend(self.with_pairs(|world,context|{let requests=world.notify_with_hooks(0,now,dt,0x121,center,0.,0.,context)?;context.audit(requests);Ok(())}));
            self.frame_cache.set_flags(0,0x101).unwrap();self.bounds_world.set_flags(0,0x101);
        }
        //1010b5e0's pre-PSI pass is the separate fast list, not every
        // ordinary active pair. Controlled pairs currently have no fast hint.
        self.activation_cache=false;events
    }
    pub(super) fn poll_movement(&mut self,b:&Rigid,angular:Vec3,radius:f32,dt:f32)->Result<Option<serde_json::Value>,&'static str>{
        // This controlled world has one active unit. The real environment's
        // countdown/RNG is shared across units; it must not become per-body.
        if !self.poll.tick(1./65536.,-5.){return Ok(None);}
        let pred=self.previous_prediction.unwrap_or(Prediction{start:b.position*BASIS,velocity:Vec3::ZERO,rotation_start:q_native(b.rotation),rotation_end:q_native(b.rotation),time:0.,inverse_dt:dt.recip()});
        let input=crate::ivp_movement::Input{radius,sleep_time:0.3,angular,position:pred.start,start:pred.rotation_start,end:pred.rotation_end};
        let k=crate::ivp_movement::Settings{translation_squared:0.0001,rotation_squared:0.000025,far_translation_squared:0.1f32*0.1,far_rotation_squared:0.2f32*0.2,far_timeout:4.,one:1.,two:2.,angular_factor0:std::f32::consts::PI,angular_factor1:0.75};
        let result=crate::ivp_movement::classify(&mut self.movement,input,self.clock,&k)?;
        self.movement_state=if result==3{8}else{result};
        if self.is_sleeping(){
            self.pair_motions[0]=PairMotion::default();let now=self.clock;let center=(b.position*BASIS).to_array();
            let _=self.with_pairs(|world,context|{let requests=world.sleep_with_hooks(0,now,dt,center,context)?;context.audit(requests);Ok(())});
            self.frame_cache.set_flags(0,0x108)?;self.frame_cache.detach(0)?;
            if let Some(pred)=self.previous_prediction.as_mut(){pred.velocity=Vec3::ZERO;pred.rotation_end=pred.rotation_start;pred.inverse_dt=dt.recip();
                //1010f960 still runs once with zero angular/linear motion
                //after100f14c0. It retains the reset origin but stamps this PSI.
                pred.time=self.clock;pred.rotation_end=crate::ivp_motion::rotation_end(pred.rotation_start,[0.,0.,0.,1.]);self.frame_cache.update_core(0,*pred)?;}
            self.raw_linear=Vec3::ZERO;self.raw_angular=Vec3::ZERO;self.core_low_flags=0;self.pending_linear=Vec3::ZERO;self.pending_angular=Vec3::ZERO;self.unit.flags=(self.unit.flags&!0xff)|8;}
        Ok(Some(serde_json::json!({"kind":"retained_movement_poll","time":self.clock,"result":result,"movement_state":self.movement_state,"position":input.position.to_array(),"start":input.start,"end":input.end,"angular":angular.to_array(),"close_time":self.movement.close_time,"far_time":self.movement.far_time,"close_position":self.movement.close_position.to_array(),"far_position":self.movement.far_position.to_array(),"close_quat":self.movement.close_quat,"far_quat":self.movement.far_quat})))
    }
    pub(super) fn finish_tick(&mut self){
        // One dynamic core plus a static floor has no split/merge to perform.
        // This is not a general implementation of the native graph rebuild.
        assert!(self.frame_cache.metadata().0.iter().all(|s|s.references==0),"contact refresh leaked a cache reference");
        assert_eq!(self.detailed_pairs.len(),self.surface_pairs.values().map(|p|p.pairs.len()).sum::<usize>(),"detailed pair ownership differs from admitted surfaces");
        for p in &self.detailed_pairs{assert!(p.query.frame<=self.cache_context.frame,"detailed query has a future frame");}
        self.unit.graph_finished();
    }
    fn inputs(&self,b:&Rigid,pose:crate::ivp_motion::Pose,velocity:Vec3,angular:Vec3,vertex:usize,floor:usize,edge:CompactEdgeId)->Result<([refresh::Shape;2],[refresh::Body;2]),&'static str>{
        let (convex,id)=self.vertices[vertex];
        let object=self.frame_cache.object_frame(0)?.pose;let floor_object=self.frame_cache.object_frame(1)?;
        let shapes=[refresh::Shape::from_compact(0,Some((&self.moving[convex],id)),rows(object.rotation),object.position,0.)?,
            refresh::Shape::from_compact(2,Some((&self.floor[floor],edge)),rows(floor_object.pose.rotation),floor_object.pose.position,0.)?];
        let inverse=Vec3::new(b.inverse_inertia.x_axis.x,b.inverse_inertia.y_axis.y,b.inverse_inertia.z_axis.z);
        let bodies=[refresh::Body{flags:256,inverse_mass:b.inverse_mass,inverse_inertia:inverse,rotation:rows(pose.rotation),position:pose.position,linear:velocity,angular},
            refresh::Body{flags:2050,inverse_mass:0.,inverse_inertia:Vec3::ZERO,rotation:rows(Mat3::IDENTITY),position:floor_object.core_position,linear:Vec3::ZERO,angular:Vec3::ZERO}];
        Ok((shapes,bodies))
    }
    fn retained_inputs(&mut self,b:&Rigid,pose:crate::ivp_motion::Pose,velocity:Vec3,angular:Vec3,vertex:usize,floor:usize,edge:CompactEdgeId)->Result<((Lease,Lease),[refresh::Shape;2],[refresh::Body;2]),&'static str>{
        let leases=self.frame_cache.retain_pair([0,1],self.cache_context)?;
        match self.inputs(b,pose,velocity,angular,vertex,floor,edge){Ok((shapes,bodies))=>Ok((leases,shapes,bodies)),Err(error)=>self.frame_cache.with_leases(leases,|_|Err(error))}
    }
    pub(super) fn refresh_all(&mut self,b:&Rigid,pose:crate::ivp_motion::Pose,velocity:Vec3,angular:Vec3,now:f64)->Result<(),&'static str>{
        let order=self.friction_order.iter().rev().filter_map(|id|self.contacts.iter().position(|c|c.id==*id)).collect::<Vec<_>>();
        let mut changes=vec![];
        for i in order{let c=&self.contacts[i];let before=c.solver.geometry().ok_or("unprepared retained geometry")?.distance;let force=c.solver.state().normal.force;
            let (leases,s,body)=self.retained_inputs(b,pose,velocity,angular,c.vertex,c.floor,c.edge)?;
            let solver=&mut self.contacts[i].solver;
            let after=self.frame_cache.with_leases(leases,|release|solver.refresh(s,body,now,&settings(),||release()).map(|out|out.distance))?;
            changes.push(crate::ivp_contact_pipeline::DistanceChange{before,after,force});
        }
        self.geometry_delta=0.;
        if changes.len()>=2{changes.reverse();self.geometry_delta=self.friction_group.as_mut().ok_or("missing geometry work group")?.geometry_work(&changes)?;}
        Ok(())
    }
    pub(super) fn impact_geometry(&self,vertex:usize)->Option<(Vec3,f32)>{
        self.contacts.iter().find(|c|c.vertex==vertex).and_then(|c|c.solver.geometry()).map(|g|(g.arms[0],g.distance))
    }
    pub(super) fn retry_metric(&self,vertex:usize,velocity:Vec3,angular:Vec3,normal:Vec3)->Option<(f32,f32)>{
        let g=self.contacts.iter().find(|c|c.vertex==vertex)?.solver.geometry()?;let n=g.normal;let j=g.jacobians[0];
        //10103c10 uses the cached distance and six-term row projection, not
        //a newly transformed vertex/cross-product approximation.
        let speed=((((velocity.y*n.y+n.x*velocity.x)+velocity.z*n.z)+j.y*angular.y)+j.x*angular.x)+j.z*angular.z;
        Some((g.distance,speed*g.normal.dot(normal).signum()))
    }
    pub(super) fn live_vertices(&self)->impl Iterator<Item=usize>+'_ {self.contacts.iter().map(|c|c.vertex)}
    pub(super) fn refresh_vertex(&mut self,b:&Rigid,pose:crate::ivp_motion::Pose,velocity:Vec3,angular:Vec3,vertex:usize,now:f64)->Result<(),&'static str>{
        for i in 0..self.contacts.len(){let c=&self.contacts[i];if c.vertex!=vertex{continue;}
            let (leases,shapes,bodies)=self.retained_inputs(b,pose,velocity,angular,c.vertex,c.floor,c.edge)?;
            let solver=&mut self.contacts[i].solver;self.frame_cache.with_leases(leases,|release|solver.refresh(shapes,bodies,now,&settings(),||release()).map(|_|()))?;
        }Ok(())
    }
    ///10102f60 refreshes the other group links once after the primary impact,
    /// using its changed raw velocities. Retry selection then uses that cache;
    /// same-group retries do not refresh a selected link again.
    pub(super) fn refresh_impact_neighbours(&mut self,b:&Rigid,pose:crate::ivp_motion::Pose,velocity:Vec3,angular:Vec3,excluded:usize,now:f64)->Result<Vec<serde_json::Value>,&'static str>{
        if !self.contacts.iter().any(|c|c.id==excluded){return Err("impact contact is not retained");}
        let order=self.friction_order.iter().rev().copied().filter(|&id|id!=excluded).collect::<Vec<_>>();let mut retired=vec![];let mut audit=vec![];
        for id in order{let i=self.contacts.iter().position(|c|c.id==id).ok_or("unbound impact neighbour")?;let c=&self.contacts[i];
            let before=c.solver.state().refresh;let vertex=c.vertex;let (leases,shapes,bodies)=self.retained_inputs(b,pose,velocity,angular,c.vertex,c.floor,c.edge)?;
            let solver=&mut self.contacts[i].solver;
            let (mix,basis,point,distance,status)=self.frame_cache.with_leases(leases,|release|solver.refresh(shapes,bodies,now,&settings(),||release()).map(|out|(out.state.mix,out.basis,out.point,out.distance,out.geometry.status)))?;
            audit.push(serde_json::json!({"kind":"impact_neighbour_refresh","id":id,"vertex":vertex,"excluded":excluded,"time":now,"before_time":before.time,"mix_before":before.mix,"mix_after":mix,"basis":[basis[0].to_array(),basis[1].to_array()],"point":point.to_array(),"distance":distance,"status":status,"angular":angular.to_array(),"linear":velocity.to_array()}));
            if status==1{retired.push(id);}
        }
        if !retired.is_empty(){self.movement.reset_clock(now)?;}
        self.contacts.retain(|c|!retired.contains(&c.id));self.friction_order.retain(|id|!retired.contains(id));
        Ok(audit)
    }
    fn floor_at(&self,p:Vec3)->Option<(usize,CompactEdgeId)>{
        // Controlled floor only: choose an actually containing top triangle.
        // Axis-aligned edge canonicalization is a trial policy, not a claim
        // that native minimizer traversal selects the same directed edge.
        for (c,h) in self.floor.iter().enumerate(){let t=h.topology.as_ref()?;
            for triangle in 0..t.edges.len(){let v:[Vec3;3]=std::array::from_fn(|edge|Vec3::from_array(t.vertex(CompactEdgeId{triangle,edge}).unwrap()));
                if v.iter().any(|v|v.y.abs()>1e-6){continue;}
                let n=(v[1]-v[0]).cross(v[2]-v[0]);if n.y>=0.{continue;}
                let projected=Vec3::new(p.x,0.,p.z);
                if (0..3).any(|i|(v[(i+1)%3]-v[i]).cross(projected-v[i]).dot(n)< -1e-5){continue;}
                let edge=(0..3).min_by(|&a,&z|{let a=v[(a+1)%3]-v[a];let z=v[(z+1)%3]-v[z];a.z.abs().total_cmp(&z.z.abs())}).unwrap();
                return Some((c,CompactEdgeId{triangle,edge}));
            }
        }None
    }
    pub(super) fn birth(&mut self,b:&Rigid,pose:crate::ivp_motion::Pose,event:Event,velocity:Vec3,angular:Vec3,now:f64)->Result<(usize,serde_json::Value),&'static str>{
        if event.normal.dot(Vec3::Y)<0.9999||event.plane.abs()>1e-5{return Err("persistent trial supports only the controlled flat floor");}
        let (convex,id)=self.vertices[event.vertex];let raw=Vec3::from_array(self.moving[convex].topology.as_ref().unwrap().vertex(id).unwrap());
        let at=self.frame_cache.object_frame(0)?.pose.transform_point(raw);
        let (floor,edge)=if let Some(pair_id)=event.pair{
            let pair=self.detailed_pairs.iter().find(|p|p.id==pair_id).ok_or("physical pair lost its retained feature")?;
            let floor=self.floor.iter().position(|h|h.topology.as_ref().is_some_and(|t|std::sync::Arc::ptr_eq(t,&pair.hulls[1]))).ok_or("physical face hull missing")?;
            (floor,pair.query.face)
        }else{self.floor_at(at).ok_or("CCD point lies outside controlled compact floor")?};
        if let Some(i)=self.contacts.iter().position(|c|c.vertex==event.vertex&&c.floor==floor&&c.edge.triangle==edge.triangle){
            self.refresh_vertex(b,pose,velocity,angular,event.vertex,now)?;
            return Ok((self.contacts[i].id,serde_json::json!({"kind":"retained_reuse","id":self.contacts[i].id,"vertex":event.vertex})));
        }
        let (leases,shapes,bodies)=self.retained_inputs(b,pose,velocity,angular,event.vertex,floor,edge)?;
        let state=refresh::State{time:now,mix:[0.;2],marker:1,counter:0};
        let preview=self.frame_cache.with_leases(leases,|release|PreparedContact::prepare(state,shapes,bodies,[Some(0),None],now,&settings(),||release()))?;
        let mass=crate::ivp_mass_bound::unit_mass(crate::ivp_mass_bound::Core{flags:256,inverse_inertia:bodies[0].inverse_inertia,inverse_mass:b.inverse_mass,arm:preview.geometry().arms[0]}) as f32;
        let mut solver=RetainedContact::from_state(RetainedState{refresh:state,normal:ivp_history::History{force:0.,counter:0},energy:0.,work:0.,response_bound:1./mass},[Some(0),None])?;
        let leases=self.frame_cache.retain_pair([0,1],self.cache_context)?;
        self.frame_cache.with_leases(leases,|release|solver.refresh(shapes,bodies,now,&settings(),||release()).map(|_|()))?;
        let id=self.next_id;self.next_id+=1;
        if self.contacts.is_empty(){self.friction_group=Some(FrictionGroup::default());}
        self.contacts.insert(0,Contact{id,vertex:event.vertex,floor,edge,solver});
        self.friction_order.push(id);
        Ok((id,serde_json::json!({"kind":"retained_birth","id":id,"vertex":event.vertex,"time":now,"admission":"diagnostic CCD candidate; native minimizer remains unported"})))
    }
    pub(super) fn respond(&mut self,b:&Rigid,pose:crate::ivp_motion::Pose,velocity:Vec3,angular:Vec3,dt:f32,friction:f32)->Result<(Vec3,Vec3,Vec<serde_json::Value>),&'static str>{
        if self.contacts.is_empty(){return Ok((velocity,angular,vec![]));}
        let friction_group=self.friction_group.as_mut().ok_or("contacts have no friction group")?;
        let inverse=Vec3::new(b.inverse_inertia.x_axis.x,b.inverse_inertia.y_axis.y,b.inverse_inertia.z_axis.z);
        let mut cores=[Core{inverse_mass:b.inverse_mass,inverse_inertia:inverse,linear:velocity,angular,pending_linear:self.pending_linear,pending_angular:self.pending_angular}];
        let frames=[CoreFrame{rotation:pose.rotation,position:pose.position}];
        // The group array has insertion order independently of the managed
        // linked list (which normal response sorts by signed history). Do not
        // mistake reverse array traversal for reverse linked-list traversal.
        let mut group=self.contacts.iter_mut().collect::<Vec<_>>();group.sort_by_key(|c|self.friction_order.iter().position(|&id|id==c.id).unwrap());
        let parameters=group.iter().map(|c|TangentSettings{dt,inverse_dt:dt.recip(),friction,response_bound:c.solver.state().response_bound,threshold_extra:1e-6,minimum_impulse:1e-6,minimum_determinant_squared:1.00000005e-20,mode:0,alternate_candidate:false}).collect::<Vec<_>>();
        crate::ivp_contact_pipeline::TANGENT_AUDIT.with(|a|*a.borrow_mut()=std::env::var_os("L4D2R_PHY_FRICTION_AUDIT").map(|_|vec![]));
        let timer_before=friction_group.timer;let mut redistributed=false;
        if group.len()==1{group[0].solver.respond_friction(&mut cores,&frames,&parameters[0])?;}
        else{
            let mut solvers=group.iter_mut().map(|c|&mut c.solver).collect::<Vec<_>>();
            let outcomes=RetainedContact::respond_group_friction(&mut solvers,&mut cores,&frames,&parameters)?;
            // Controlled drops always have the same moving object on side0.
            redistributed=friction_group.finish_batch(&mut solvers,&vec![1;outcomes.len()],&outcomes)?.is_some();
        }
        let group_state=*friction_group;
        let count_before=self.contacts.len();
        let mut managed=lifetime::State{skipped:0,contacts:self.contacts.iter().map(|c|{let g=c.solver.geometry().unwrap();lifetime::Contact{id:c.id,counter:c.solver.state().normal.counter,distance:g.distance,status:g.geometry.status,core_flags:[256,2050]}}).collect()};
        let path=managed.prepare(lifetime::Settings{retire_distance:0.0285635684,target_distance:0.0126949195,extra_distance:6.34745957e-5});
        self.contacts.retain(|c|managed.contacts.iter().any(|m|m.id==c.id));
        self.contacts.sort_by_key(|c|managed.contacts.iter().position(|m|m.id==c.id).unwrap());
        let mut retire=false;let mut committed=true;let mut normal_audit=serde_json::Value::Null;
        if path==lifetime::Path::Single{
            if let Some(c)=self.contacts.first_mut(){retire=c.solver.respond_normal(&mut cores,dt.recip())?.retire;}
            if retire{self.contacts.clear();}
        }else if !self.contacts.is_empty(){
            let rows=self.contacts.iter().map(|c|c.solver.normal_row()).collect::<Result<Vec<_>,_>>()?;
            let mut system=crate::ivp_contact::assemble(&cores,&rows,0.0126949195,20.).ok_or("invalid retained batch assembly")?;
            let raw_matrix=system.matrix.clone();let raw_rhs=system.rhs.clone();
            let scale=crate::ivp_contact::normalize(&mut system,1e-10).ok_or("invalid retained batch normalization")?;
            let modes=self.contacts.iter().map(|c|c.solver.state().normal.counter).collect::<Vec<_>>();
            let indices=(0..rows.len()).filter(|&i|modes[i]!=0).collect::<Vec<_>>();let warm=modes.iter().take_while(|&&m|m<0).count();let n=rows.len();
            let mut selected=NormalSystem{matrix:indices.iter().flat_map(|&i|indices.iter().map(move |&j|(i,j))).map(|(i,j)|system.matrix[i*n+j]).collect(),rhs:indices.iter().map(|&i|system.rhs[i]).collect()};
            let candidate=crate::ivp_contact::gaussian(&mut selected,9.99999972e-10,1000.).ok_or("invalid retained batch Gaussian")?;
            let weights=self.contacts.iter().map(|c|c.solver.geometry().unwrap().response).collect::<Vec<_>>();
            let fast=crate::ivp_contact::validate_fast(&system,scale,&indices,&candidate.solution,&weights,0.0982999951).ok_or("invalid retained batch candidate")?;
            let impulses=if candidate.success&&fast.accepted{fast.impulses}else{crate::ivp_active::lcp_trial::solve_owned_normal(&system.matrix,&system.rhs,warm).ok_or("retained batch active-set solve failed")?};
            normal_audit=serde_json::json!({"matrix":raw_matrix,"rhs":raw_rhs,"scale":scale,"modes":modes,"indices":indices,"warm":warm,"fast":candidate.success&&fast.accepted,"impulses":impulses,"core":{"linear":cores[0].linear.to_array(),"angular":cores[0].angular.to_array(),"pending_linear":cores[0].pending_linear.to_array(),"pending_angular":cores[0].pending_angular.to_array()},"rows":self.contacts.iter().map(|c|{let g=c.solver.geometry().unwrap();serde_json::json!({"vertex":c.vertex,"normal":g.normal.to_array(),"jacobian":g.jacobians[0].to_array(),"gap":g.distance,"force":c.solver.state().normal.force})}).collect::<Vec<_>>()});
            let energy=|c:Core|crate::ivp_energy::Core{flags:256,mass:b.inverse_mass.recip(),inertia:self.moving_inertia,linear:c.linear,angular:c.angular,pending_linear:c.pending_linear,pending_angular:c.pending_angular};
            let before=[energy(cores[0])];
            for (i,c) in self.contacts.iter_mut().enumerate(){c.solver.apply_batch_normal(&mut cores,impulses[i]*scale,false,dt,dt.recip())?;}
            let mut after=[energy(cores[0])];committed=crate::ivp_energy::accepts(&before,&after,dt,10.);crate::ivp_energy::finish(&mut after,committed);
            cores[0].linear=after[0].linear;cores[0].angular=after[0].angular;cores[0].pending_linear=after[0].pending_linear;cores[0].pending_angular=after[0].pending_angular;
        }
        self.friction_order.retain(|id|self.contacts.iter().any(|c|c.id==*id));
        if self.contacts.len()<count_before{self.movement.reset_clock(self.clock)?;}
        if self.contacts.is_empty(){self.friction_group=None;self.unit.contact_removed();}
        self.pending_linear=cores[0].pending_linear;self.pending_angular=cores[0].pending_angular;self.raw_linear=cores[0].linear;self.raw_angular=cores[0].angular;
        let mut audit=serde_json::json!({"kind":"retained_response","time":self.clock,"path":if path==lifetime::Path::Single{"single"}else{"batch"},"count_before_prune":count_before,"count_after":self.contacts.len(),"retired_single":retire,"committed":committed,"normal_system":normal_audit,"friction_group":{"timer_before":timer_before,"timer_after":group_state.timer,"energy":group_state.energy,"redistributed":redistributed},"velocity_before":velocity.to_array(),"velocity_after":cores[0].linear.to_array(),"angular_before":angular.to_array(),"angular_after":cores[0].angular.to_array(),"contacts":self.contacts.iter().map(|c|{let s=c.solver.state();let g=c.solver.geometry().unwrap();serde_json::json!({"id":c.id,"vertex":c.vertex,"force":s.normal.force,"counter":s.normal.counter,"mix":s.refresh.mix,"energy":s.energy,"work":s.work,"response_bound":s.response_bound,"point":g.point.to_array(),"distance":g.distance,"normal_cache":std::env::var_os("L4D2R_PHY_FRICTION_AUDIT").map(|_|serde_json::json!({"normal":g.normal.to_array(),"jacobians":g.jacobians.map(|v|v.to_array()),"arms":g.arms.map(|v|v.to_array()),"response":g.response,"inverse_response":g.inverse_response,"frame_rotation":pose.rotation.to_cols_array()}))})}).collect::<Vec<_>>()});
        crate::ivp_contact_pipeline::TANGENT_AUDIT.with(|a|{if let Some(rows)=a.borrow_mut().take(){audit["tangent_calls"]=serde_json::json!(rows);}});
        Ok((cores[0].linear,cores[0].angular,vec![audit]))
    }
}
