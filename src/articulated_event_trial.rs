//! Diagnostic independent-core trial. Not enabled in gameplay.
//! Controlled floor contacts, idle/sleep and waking are integrated; general
//! event/world graphs, coupled contacts and native joints remain separate.
use super::*;
use crate::ivp_motion::{Prediction,ObjectFrame};
#[path="articulated_event_trial/persistent.rs"]
mod persistent;
pub(crate) use persistent::PersistentTrial;
const BASIS:Vec3=Vec3::new(1.,-1.,-1.);
const GAP:f32=0.00634745974; // owned DAT1018db9c / 1018dc9c
fn q_native(q:Quat)->[f32;4] {[q.x,-q.y,-q.z,q.w]}
fn q_render(q:[f32;4])->Quat {Quat::from_xyzw(q[0],-q[1],-q[2],q[3])}
// Native refinement and prediction updates operate on absolute environment
// doubles. Rebasing the interpolation/search arithmetic to zero changes ties.
fn absolute(p:&Prediction,epoch:f64)->Prediction{let mut p=*p;p.time+=epoch;p}
fn sample_at(p:&Prediction,at:f64,epoch:f64)->crate::ivp_motion::Pose{absolute(p,epoch).sample(at+epoch,ObjectFrame{shift:None,rotation:None})}
fn trim_at(p:&mut Prediction,at:f64,epoch:f64){let mut global=absolute(p,epoch);global.trim_impact_rotation(at+epoch);p.rotation_end=global.rotation_end;}
fn restamp_at(p:&mut Prediction,at:f64,end:f64,velocity:Vec3,angular:Vec3,epoch:f64){let mut global=absolute(p,epoch);global.restamp_impact(at+epoch,end+epoch,velocity,angular).unwrap();global.time-=epoch;*p=global;}
fn point(p:&Prediction,t:f64,vertex:Vec3,center:Vec3)->Vec3 {
    let sample=p.sample(t,ObjectFrame{shift:Some(-center*BASIS),rotation:None});
    sample.transform_point(vertex*BASIS)*BASIS
}
fn sweep(p:&Prediction,owned:Option<(Vec3,f32,f32)>)->(Vec3,f32) {
    if let Some((axis,speed,_))=owned{return (axis,speed);}
    let start=Quat::from_array(p.rotation_start);let end=Quat::from_array(p.rotation_end);
    let delta=start.conjugate()*end;let xyz=Vec3::new(delta.x,delta.y,delta.z);
    let square=(xyz.x*xyz.x+xyz.y*xyz.y)+xyz.z*xyz.z;
    if square<=1e-10 {return (Vec3::X,0.);}
    let inverse=square.sqrt().recip();let length=inverse*square;let cube=length*length*length;
    let speed=((cube*length*length*0.40413999557495117+cube*0.1666666716337204)+length)*2.*p.inverse_dt;
    let pose=p.sample(p.time,ObjectFrame{shift:None,rotation:None});
    (pose.rotation*xyz*inverse,speed)
}
fn motion_bound(p:&Prediction,normal:Vec3,radius:f32,owned:Option<(Vec3,f32,f32)>)->f32 {
    if let Some((axis,_,bound))=owned{return crate::ivp_bounds::closing_speed(
        crate::ivp_bounds::Motion{axis,angular_bound:bound,velocity:p.velocity},
        crate::ivp_bounds::Motion{axis:Vec3::X,angular_bound:0.,velocity:Vec3::ZERO},normal).expect("finite native contact direction");}
    let (axis,speed)=sweep(p,None);let dot=axis.dot(normal);
    ((1.-(dot*dot) as f64).max(0.).sqrt()*(radius*speed) as f64-p.velocity.dot(normal) as f64) as f32
}
#[derive(Clone,Copy)]
struct Event {pair:Option<usize>,time:f64,vertex:usize,feature:Option<usize>,normal:Vec3,plane:f32,surface:Option<crate::physics_data::Surface>}
fn add_plane(planes:&mut Vec<Event>,start:Vec3,end:Vec3,collision:&Collision) {
    let (hit,surface)=collision.corpse_contact(start,end,Vec3::ZERO);
    if hit.start_solid || hit.fraction>=1. || hit.normal==Vec3::ZERO {return;}
    let plane=hit.plane_distance.unwrap_or_else(||(start+(end-start)*hit.fraction).dot(hit.normal)-collision.trace_epsilon());
    if !planes.iter().any(|p|p.normal.dot(hit.normal)>0.99999&&(p.plane-plane).abs()<1e-5) {
        planes.push(Event{pair:None,time:0.,vertex:0,feature:None,normal:hit.normal,plane,surface});
    }
}
fn planes(p:&Prediction,b:&Rigid,collision:&Collision,cursor:f64,end:f64,radius:f32)->Vec<Event> {
    let mut result=vec![];
    for (i,normal) in b.world_contacts.iter().enumerate() {
        if *normal==Vec3::ZERO {continue;}
        let at=point(p,cursor,b.vertices[i],b.center);let reach=2.*radius+0.139644116;
        add_plane(&mut result,at+*normal*reach,at-*normal*reach,collision);
    }
    for v in &b.vertices {
        for part in 0..8 {
            let lo=cursor+(end-cursor)*(part as f64/8.);let hi=cursor+(end-cursor)*((part+1) as f64/8.);
            let a=point(p,lo,*v,b.center);let z=point(p,hi,*v,b.center);
            let (hit,_)=collision.corpse_contact(a,z,Vec3::splat(GAP));
            if hit.start_solid || hit.fraction>=1. || hit.normal==Vec3::ZERO {continue;}
            let at=a+(z-a)*hit.fraction;
            add_plane(&mut result,at+hit.normal*FRICTION_DISTANCE,at-hit.normal*FRICTION_DISTANCE,collision);
        }
    }
    result
}
// Diagnostic port of 10127cc0's initially-inside phase. The complete native
// feature/minimizer topology and scratch-pose reuse are still unported.
fn face_time(p:&Prediction,start:f64,end:f64,initial:f32,inverse_bound:f32,mut distance:impl FnMut(f64)->f64)->Option<f64> {
    if initial>GAP {return crate::ivp_time::search(start,end,GAP,initial,inverse_bound,distance).map(|r|r.refined.time);}
    let mut time=start;let mut current=initial;let mut index=0;
    while time<end {
        let mut delta=(current as f64-0.000634745986f32 as f64)*inverse_bound as f64;
        if (time-end)+delta>0. {delta=(end-time)+0.002999999992425728;}else{delta=delta.max(0.);}
        let steps=((delta as f32*200.) as i32).max(1);index+=steps;if index>20 {return None;}
        let at=time+steps as f64*0.004999999888241291;let value=distance(at);
        if value>GAP as f64 {
            if at>end {return None;}
            return crate::ivp_time::search(at,end,GAP,value as f32,inverse_bound,distance).map(|r|r.refined.time);
        }
        if value as f32<=initial {return Some(time);}
        time=at;current=value as f32;
    }
    let _=p;None
}
fn discover(p:&Prediction,b:&Rigid,collision:&Collision,cursor:f64,end:f64,radius:f32,adjacency:&[(usize,usize)],inverse_diameter:f32,owned_sweep:Option<(Vec3,f32,f32)>,epoch:f64)->Option<Event> {
    let global=absolute(p,epoch);let p=&global;let cursor=cursor+epoch;let end=end+epoch;
    let cached=p.prepare_object_cache(cursor,ObjectFrame{shift:Some(-b.center*BASIS),rotation:None},0).pose;
    let cached_point=|vertex:Vec3|cached.transform_point(vertex*BASIS)*BASIS;
    let mut first:Option<Event>=None;
    for mut plane in planes(p,b,collision,cursor,end,radius) {
        let vertex=(0..b.vertices.len()).min_by(|&a,&z|{
            let sample=|i|if owned_sweep.is_some(){cached_point(b.vertices[i])}else{point(p,cursor,b.vertices[i],b.center)};
            sample(a).dot(plane.normal).total_cmp(&sample(z).dot(plane.normal))})?;
        plane.vertex=vertex;
        let distance=|t|point(p,t,b.vertices[vertex],b.center).dot(plane.normal)-plane.plane;
        let initial=if owned_sweep.is_some(){cached_point(b.vertices[vertex]).dot(plane.normal)-plane.plane}else{distance(cursor)};let bound=motion_bound(p,plane.normal*BASIS,radius,owned_sweep);
        if bound>0. {
            if let Some(time)=face_time(p,cursor,end,initial,bound.recip(),|t|distance(t) as f64) {
                plane.time=time;
                if plane.time<=end && first.as_ref().is_none_or(|e|plane.time<e.time) {first=Some(plane);}
            }
        }
        let Some(target)=crate::ivp_time::feature_target(0.,initial,GAP,inverse_diameter) else {continue;};
        let angular_bound=sweep(p,owned_sweep).1+1e-10;
        for &(a,z) in adjacency {
            let other=if a==vertex {z}else if z==vertex {a}else{continue;};
            let edge=(b.vertices[other]-b.vertices[vertex])*BASIS;
            let square=(edge.x*edge.x+edge.y*edge.y)+edge.z*edge.z;if square<=0. {continue;}
            let edge=edge*((1./(square as f64).sqrt()) as f32);
            let direction=|t|crate::ivp_time::direction_distance(p.sample(t,ObjectFrame{shift:None,rotation:None}).rotation,Mat3::IDENTITY,edge,plane.normal*BASIS);
            let deadline=first.as_ref().map_or(end,|e|e.time.min(end));
            let initial_direction=if owned_sweep.is_some(){crate::ivp_time::direction_distance(cached.rotation,Mat3::IDENTITY,edge,plane.normal*BASIS)}else{direction(cursor)};
            let Some(candidate)=crate::ivp_time::search(cursor,deadline,target,initial_direction as f32,angular_bound.recip(),direction) else {continue;};
            if candidate.refined.exhausted {continue;}
            let time=candidate.refined.time;
            if time>=cursor && time<=deadline {
                let mut event=plane;event.time=time;event.feature=Some(other);first=Some(event);
            }
        }
    }
    let mut event=first?;
    let distance=point(p,cursor,b.vertices[event.vertex],b.center).dot(event.normal)-event.plane;
    let total_bound=owned_sweep.map_or_else(||sweep(p,None).1*radius+p.velocity.length(),|(_,_,bound)|bound+crate::ivp_bounds::speed(p.velocity));
    event.time=crate::ivp_time::EventAdmission {now:cursor,deadline:end,candidate:event.time,epoch,dt:(end-epoch) as f32,
        distance,total_bound,kind:if event.feature.is_some() {33}else{32},delay_mode:1,time_scale:1.}.queued()?;
    event.time-=epoch;Some(event)
}
fn curvature(angular:Vec3,radius:f32,inverse_dt:f32)->f64 {
    let sq=((angular.x*angular.x+angular.y*angular.y)+angular.z*angular.z)*0.005f32*0.005f32;
    (1.-(sq.min(0.25) as f64).sqrt().cos())*radius as f64*inverse_dt as f64
}
fn respond(b:&Rigid,pose:crate::ivp_motion::Pose,event:Event,velocity:Vec3,angular:Vec3,radius:f32,dt:f32,
    friction:f32,energy:f32,pushes:u32,cached:Option<(Vec3,f32)>)->(Vec3,Vec3,serde_json::Value) {
    let normal=event.normal*BASIS;
    let (arm,gap)=cached.unwrap_or_else(||{let arm=(b.vertices[event.vertex]-b.center)*BASIS;(arm,(pose.position+pose.rotation*arm).dot(normal)-event.plane)});
    let correction=(GAP-gap).max(0.)*dt.recip();
    let extra=((curvature(angular,radius,dt.recip())+correction as f64)*2.) as f32;
    let rescue_speed=(0.0126949195+extra)*1.2;
    let inverse=Vec3::new(b.inverse_inertia.x_axis.x,b.inverse_inertia.y_axis.y,b.inverse_inertia.z_axis.z);
    let (friction,energy)=event.surface.map_or((friction,energy),|s|b.surface.contact_with(s));
    let mut fixed=crate::ivp_impact::CoreBody {flags:2,inverse_mass:0.,inverse_inertia:Vec3::ZERO,
        rotation:Mat3::IDENTITY,arm:Vec3::ZERO,velocity:Vec3::ZERO,angular:Vec3::ZERO};
    let mut moving=crate::ivp_impact::CoreBody {flags:256,inverse_mass:b.inverse_mass,inverse_inertia:inverse,
        rotation:pose.rotation,arm,velocity,angular};
    crate::ivp_impact::resolve_cores(&mut fixed,&mut moving,normal,crate::ivp_impact::Settings {
        energy,friction,system_pushes:pushes,rescue_speed,linear_limit:50.7999992,
        angular_limit:0.95199776*dt.recip(),angular_limit_exact:0.95199776f32 as f64*dt.recip() as f64,
    }).unwrap();
    let audit=serde_json::json!({"kind":if pushes==0 {"impact"}else{"retry"},"time":event.time,"vertex":event.vertex,"arm":arm.to_array(),"rotation":pose.rotation.to_cols_array(),
        "position":pose.position.to_array(),"normal":normal.to_array(),"gap":gap,"extra_bias":extra,"system_pushes":pushes,
        "velocity_before":velocity.to_array(),"angular_before":angular.to_array(),"velocity_after":moving.velocity.to_array(),"angular_after":moving.angular.to_array()});
    (moving.velocity,moving.angular,audit)
}
// One fixed PSI prediction can straddle two public Simulate calls. Retain
//the queued event and response state instead of executing future collisions.
struct Interval {
    prediction:Prediction,angular:Vec3,core_rotation:Mat3,owned_sweep:Option<(Vec3,f32,f32)>,active:Vec<(Event,u32)>,
    cursor:f64,reconstruct_angular:bool,impact_budget:crate::ivp_impact::ImpactBudget,held_motion:Option<(Vec3,Vec3)>,queued:Option<Event>,
    epoch:f64,dt:f32,friction:f32,elasticity:f32,
}
fn advance_interval(frame:Interval,b:&mut Rigid,collision:&Collision,cutoff:f64,radius:f32,adjacency:&[(usize,usize)],inverse_diameter:f32,mut persistent:Option<&mut PersistentTrial>,events:&mut Vec<serde_json::Value>)->Interval {
    let Interval{mut prediction,mut angular,mut core_rotation,mut owned_sweep,mut active,mut cursor,reconstruct_angular,mut impact_budget,mut held_motion,mut queued,epoch,dt,friction,elasticity}=frame;
    let time_epoch=if persistent.is_some(){epoch}else{0.};
    let mut exhausted=true;
        for _ in 0..256 {
            let next=if PersistentTrial::uses_pair_queue()&&persistent.is_some(){
                persistent.as_deref_mut().unwrap().next_pair_event(prediction,epoch,epoch+cutoff,events)
            }else{queued.take().or_else(||discover(&prediction,b,collision,cursor,dt as f64,radius,adjacency,inverse_diameter,owned_sweep,time_epoch))};
            let Some(event)=next else {exhausted=false;break;};
            // Search/queue admission uses the complete PSI prediction. Only
            //events at or before the current client time may execute now.
            if event.time>cutoff{queued=Some(event);exhausted=false;break;}
            cursor=event.time;
            if let Some(p)=persistent.as_deref_mut(){p.advance_cache_clock(epoch+event.time);}
            if let Some(other)=event.feature {
                let cached_point=if let Some(p)=persistent.as_deref_mut(){
                    let frames=p.feature_frames(prediction,epoch,epoch+event.time).unwrap();
                    Some(frames[0].transform_point(b.vertices[other]*BASIS).to_array())
                }else{None};
                let mut audit=serde_json::json!({"kind":"feature","time":event.time,"vertex":event.vertex,"other":other});
                if let Some(point)=cached_point{audit["cached_point_ivp"]=serde_json::json!(point);}
                events.push(audit);
                continue;
            }
            //10104820 calls100f04b0 for every primary contact, including
            // later contacts in the same PSI after prediction restamping.
            // Only anisotropic cores (native flag8 clear) reconstruct angular.
            if reconstruct_angular{angular=prediction.impact_angular();}
            let pose=sample_at(&prediction,event.time,time_epoch);
            let mut impact_contact=None;
            if let Some(p)=persistent.as_deref_mut(){
                let mut cached_prediction=prediction;cached_prediction.time+=epoch;
                p.cache_from_prediction(cached_prediction,epoch+event.time,b.center);
                let (id,audit)=p.birth(b,pose,event,prediction.velocity,angular,epoch+event.time).unwrap();impact_contact=Some(id);events.push(audit);
            }
            let cached=persistent.as_ref().and_then(|p|p.impact_geometry(event.vertex));
            let (mut velocity,next_angular,audit)=respond(b,pose,event,prediction.velocity,angular,radius,dt,friction,elasticity,0,cached);
            angular=next_angular;events.push(audit);impact_budget.register(true);
            if let Some(p)=persistent.as_deref_mut(){events.extend(p.refresh_impact_neighbours(b,pose,velocity,angular,impact_contact.unwrap(),epoch+event.time).unwrap());}
            core_rotation=pose.rotation;b.world_contacts[event.vertex]=event.normal;
            if let Some(existing)=active.iter_mut().find(|(e,_)|e.vertex==event.vertex && e.normal.dot(event.normal)>0.99) {*existing=(event,0);}
            else {active.push((event,0));}
            //10105650 clears the physical row's retry count (+52) when the
            // primary link and its neighbours refresh. A second primary in
            // this tick therefore starts a new count, even for the same link.
            if persistent.is_some(){for (_,pushes) in &mut active{*pushes=0;}}
            // Owned 10103c10 reconsiders retained contacts after an impulse.
            // Choose the worst predicted separation, even if that vertex is
            // currently away from the collision skin; keep the event epoch.
            for retry_index in 0..32 {
                if persistent.is_some()&&impact_budget.flags&0x40!=0{break;}
                let mut selected=None;let mut lowest=GAP;
                for (i,(contact,_)) in active.iter().enumerate().rev() {
                    let arm=(b.vertices[contact.vertex]-b.center)*BASIS;let normal=contact.normal*BASIS;
                    let (gap,outgoing)=if let Some(p)=persistent.as_ref(){
                        let Some(metric)=p.retry_metric(contact.vertex,velocity,angular,normal)else{continue;};metric
                    }else{((pose.position+pose.rotation*arm).dot(normal)-contact.plane,(velocity+pose.rotation*angular.cross(arm)).dot(normal))};
                    if gap>0.139644116 {continue;}
                    let bias=(curvature(angular,radius,dt.recip()) as f32+(GAP-gap).max(0.)*dt.recip())*2.;
                    let predicted=gap-(bias*0.5-outgoing)*dt;
                    if predicted<lowest {lowest=predicted;selected=Some(i);}
                }
                let Some(i)=selected else {break;};
                active[i].1+=1;let mut retry=active[i].0;retry.time=event.time;
                let cached=persistent.as_ref().and_then(|p|p.impact_geometry(retry.vertex));
                let (v,w,audit)=respond(b,pose,retry,velocity,angular,radius,dt,friction,elasticity,active[i].1,cached);
                velocity=v;angular=w;events.push(audit);impact_budget.register(true);
                //10102be0 increments the per-PSI dynamic-core count. The
                //owned default anomaly handler1008c670 accepts holding the
                //prediction after count exceeds the environment limit6.
                if persistent.is_some()&&impact_budget.flags&0x40!=0{held_motion=Some((velocity,angular));break;}
                if retry_index==31 {events.push(serde_json::json!({"retry_exhausted":true}));}
            }
            trim_at(&mut prediction,event.time,time_epoch);
            if persistent.is_some()&&impact_budget.flags&0x40!=0{
                //10103020 keeps response motion queued but clears the raw
                //motion before1010f960 predicts the remaining interval.
                held_motion=Some((velocity,angular));
                restamp_at(&mut prediction,event.time,dt as f64,Vec3::ZERO,Vec3::ZERO,time_epoch);
                owned_sweep=Some((Vec3::X,0.,0.));
                if let Some(p)=persistent.as_deref_mut(){events.extend(p.bounds_prediction(prediction,epoch,epoch+event.time,((epoch+dt as f64)-(epoch+event.time)) as f32,(Vec3::X,0.,0.),dt));}
                events.push(serde_json::json!({"kind":"impact_prediction_hold","time":event.time,"impact_count":impact_budget.count,"limit":impact_budget.limit,"position":prediction.start.to_array(),"rotation":prediction.rotation_start,"queued_linear":velocity.to_array(),"queued_angular":angular.to_array()}));
                exhausted=false;break;
            }
            let delta=crate::ivp_motion::sine_delta(angular,((dt as f64+time_epoch)-(event.time+time_epoch)) as f32);
            restamp_at(&mut prediction,event.time,dt as f64,velocity,angular,time_epoch);
            if let Some(p)=persistent.as_deref_mut(){
                owned_sweep=Some(crate::ivp_motion::rotation_sweep(delta,core_rotation,prediction.inverse_dt,p.angular_radius));
                events.extend(p.bounds_prediction(prediction,epoch,epoch+event.time,((epoch+dt as f64)-(epoch+event.time)) as f32,owned_sweep.unwrap(),dt));
            }
        }
        if exhausted {events.push(serde_json::json!({"exhausted":true}));}
    Interval{prediction,angular,core_rotation,owned_sweep,active,cursor,reconstruct_angular,impact_budget,held_motion,queued,epoch,dt,friction,elasticity}
}
fn publish_interval(frame:&Interval,b:&mut Rigid,at:f64,persistent:Option<&PersistentTrial>){
    let prediction=&frame.prediction;
    let epoch=if persistent.is_some(){frame.epoch}else{0.};
    let endpoint=sample_at(prediction,frame.dt as f64,epoch);
    let position=endpoint.position*BASIS;let rotation=q_render(prediction.rotation_end);
    let sample=sample_at(prediction,at,epoch);
    b.position=sample.position*BASIS;
    let convert=Mat3::from_diagonal(BASIS);
    b.rotation=Quat::from_mat3(&(convert*sample.rotation*convert)).normalize();
    let (linear,angular,raw_angular)=persistent.map(|p|p.published_motion()).unwrap_or((prediction.velocity,frame.angular,frame.angular));
    b.velocity=linear*BASIS;b.angular=(frame.core_rotation*angular)*BASIS;
    b.pose_cache=Some(PoseCache{position,rotation,published_position:b.position,published_rotation:b.rotation,core_angular:Some(raw_angular),angular_world:b.angular});
}

impl Articulation {
    pub(crate) fn step_event_trial(&mut self,dt:f32,collision:&Collision,acceleration:Vec3,radius:f32,adjacency:&[(usize,usize)],inverse_diameter:f32,mut persistent:Option<&mut PersistentTrial>)->Vec<serde_json::Value> {
        assert!(self.joints.is_empty() && self.collision_pairs.is_empty() && self.bodies.len()==1);
        assert!(dt>0. && radius.is_finite() && radius>0.);
        let mut events=vec![];
        if let Some(p)=persistent.as_deref_mut(){if let Some(frame)=p.deferred_interval.take(){
            let epoch=frame.epoch;let end=frame.dt as f64;
            events.push(serde_json::json!({"kind":"retained_interval_resume","epoch":epoch,"from":frame.cursor,"through":end,"next_psi":p.clock,"impact_count":frame.impact_budget.count}));
            let frame=advance_interval(frame,&mut self.bodies[0],collision,end,radius,adjacency,inverse_diameter,Some(p),&mut events);
            assert!(frame.queued.is_none(),"previous PSI still has an unprocessed event");
            p.advance_cache_clock(epoch+end);p.record_interval_motion(&frame);publish_interval(&frame,&mut self.bodies[0],end,Some(p));p.remember_prediction(frame.prediction,frame.epoch);p.finish_tick();
        }}
        if let Some(p)=persistent.as_deref_mut(){if let Some(audit)=p.begin_psi(dt){events.push(audit);}}
        if let Some(p)=persistent.as_deref_mut(){if let Some(wake)=p.activate_queued(&mut self.bodies[0]){events.push(wake);}}
        if let Some(p)=persistent.as_deref_mut(){if p.is_sleeping(){
            if let Some(audit)=p.end_psi_prediction(){events.push(audit);}
            // Preserve the single sleeping-core hold report. Queue ownership
            // still advances; retain those observations inside that report.
            let mut hold=p.sleep_hold(dt);hold["psi_events"]=serde_json::json!(events);return vec![hold];
        }}
        let b=&mut self.bodies[0];let mut local=None;
        if let Some(cache)=b.pose_cache.take() {
            if b.position==cache.published_position && b.rotation==cache.published_rotation {
                b.position=cache.position;b.rotation=cache.rotation;
                if b.angular==cache.angular_world {local=cache.core_angular;}
            }
        }
        let inverse=Vec3::new(b.inverse_inertia.x_axis.x,b.inverse_inertia.y_axis.y,b.inverse_inertia.z_axis.z);
        assert!(b.inverse_inertia==Mat3::from_diagonal(inverse) && inverse.min_element()>0.);
        let mut angular=local.unwrap_or_else(||(b.rotation.conjugate()*b.angular)*BASIS);
        let mut velocity=b.velocity*BASIS;
        if let Some(p)=persistent.as_deref_mut(){p.begin_tick(&mut velocity,&mut angular).unwrap();}
        if persistent.is_none(){crate::ivp_damping::apply(&mut velocity,&mut angular,dt,b.damping,Vec3::splat(b.angular_damping),1);}
        if let Some(p)=persistent.as_deref_mut(){
            // Physical core matrices use the stored quaternion directly. The
            // object/presentation sampler's interpolation also normalizes at0.
            let pose=crate::ivp_motion::Pose{position:b.position*BASIS,rotation:crate::ivp_motion::matrix(q_native(b.rotation))};
            events.extend(p.cache_for_psi(b,dt));p.refresh_all(b,pose,velocity,angular,p.clock).unwrap();
            if let Some(audit)=p.refresh_work(b,pose,velocity,angular,dt).unwrap(){events.push(audit);}
        }
        if let Some(p)=persistent.as_deref_mut(){events.push(p.prepare_forces(&mut velocity,&mut angular,dt,b.damping,b.angular_damping,acceleration*BASIS).unwrap());}
        else{velocity+=acceleration*BASIS*dt;}
        if let Some(p)=persistent.as_deref_mut(){
            // Physical core matrices use the stored quaternion directly. The
            // object/presentation sampler's interpolation also normalizes at0.
            let pose=crate::ivp_motion::Pose{position:b.position*BASIS,rotation:crate::ivp_motion::matrix(q_native(b.rotation))};
            let (v,w,audit)=p.respond(b,pose,velocity,angular,dt,self.friction).unwrap();
            velocity=v;angular=w;events.extend(audit);
            // Only the live source contact list supplies retained impact retry
            // candidates. The old diagnostic path deliberately remains intact.
            b.world_contacts.fill(Vec3::ZERO);
            for vertex in p.live_vertices(){b.world_contacts[vertex]=Vec3::Y;}
        }
        if let Some(p)=persistent.as_deref_mut(){
            if let Some(audit)=p.poll_movement(b,angular,radius,dt).unwrap(){events.push(audit);}
            if p.is_sleeping(){
                //100f03a0 zeros raw/pending motion before the next predictor
                // advance. The physical core is already at this tick's pose.
                b.velocity=Vec3::ZERO;b.angular=Vec3::ZERO;b.pose_cache=None;
                events.push(serde_json::json!({"kind":"retained_sleep","time":p.clock,"position":(b.position*BASIS).to_array(),"rotation":q_native(b.rotation)}));
                if let Some(audit)=p.end_psi_prediction(){events.push(audit);}
                p.advance_cache_clock(p.clock+dt as f64*0.99f32 as f64);p.finish_tick();p.clock+=dt as f64;return events;
            }
        }
        let differences=Vec3::new(inverse.y-inverse.z,inverse.z-inverse.x,inverse.x-inverse.y);
        let square=(inverse.x*inverse.x+inverse.y*inverse.y)+inverse.z*inverse.z;
        let spread=(differences.x*differences.x+differences.y*differences.y)+differences.z*differences.z;
        let inertia=persistent.as_ref().map_or_else(||inverse.recip(),|p|p.moving_inertia);
        let reconstruct_angular=square*f32::from_bits(0x3c23d70b)<=spread;
        let step=crate::ivp_motion::angular_step(angular,inertia,inverse,dt,reconstruct_angular);
        angular=step.angular;
        let start=q_native(b.rotation);
        let prediction=Prediction {start:b.position*BASIS,velocity,
            rotation_start:start,rotation_end:crate::ivp_motion::rotation_end(start,step.delta),time:0.,inverse_dt:dt.recip()};
        let core_rotation=crate::ivp_motion::matrix(start);
        b.world_contacts.resize(b.vertices.len(),Vec3::ZERO);
        let mut active:Vec<(Event,u32)>=vec![];
        // Reconsider preceding-tick contacts in the same impulse group. Native
        //10103c10 may select an old vertex before the next CCD skin crossing.
        // General contact lifetime/topology and continuing friction are unported.
        for (vertex,normal) in b.world_contacts.iter().copied().enumerate() {
            if normal==Vec3::ZERO {continue;}
            let at=point(&prediction,0.,b.vertices[vertex],b.center);
            let reach=2.*radius+0.139644116;let mut retained=vec![];
            add_plane(&mut retained,at+normal*reach,at-normal*reach,collision);
            for mut event in retained {event.vertex=vertex;active.push((event,0));}
        }
        let owned_sweep=persistent.as_ref().map(|p|crate::ivp_motion::rotation_sweep(step.delta,crate::ivp_motion::matrix(start),prediction.inverse_dt,p.angular_radius));
        if let Some(p)=persistent.as_deref_mut(){events.extend(p.bounds_prediction(prediction,p.clock,p.clock,dt,owned_sweep.unwrap(),dt));}
        if let Some(p)=persistent.as_deref_mut(){if let Some(audit)=p.end_psi_prediction(){events.push(audit);}}
        let frame=Interval{prediction,angular,core_rotation,owned_sweep,active,cursor:0.,reconstruct_angular,impact_budget:crate::ivp_impact::ImpactBudget{count:0,flags:256,limit:6},held_motion:None,queued:None,
            epoch:persistent.as_ref().map_or(0.,|p|p.clock),dt,friction:self.friction,elasticity:self.elasticity};
        // Owned sample clocks retain the widened float phase, rather than
        //decimal double0.99. Preserve legacy diagnostic publication unchanged.
        let at=dt as f64*if persistent.is_some(){0.99f32 as f64}else{0.99};
        // Legacy CCD diagnostics retain their original complete-interval path.
        let frame=advance_interval(frame,b,collision,if persistent.is_some(){at}else{dt as f64},radius,adjacency,inverse_diameter,persistent.as_deref_mut(),&mut events);
        if let Some(p)=persistent.as_deref_mut(){p.record_interval_motion(&frame);}
        publish_interval(&frame,b,at,persistent.as_deref());
        if let Some(p)=persistent{p.advance_cache_clock(frame.epoch+at);p.remember_prediction(frame.prediction,frame.epoch);if let Some(queued)=frame.queued{
            events.push(serde_json::json!({"kind":"retained_interval_defer","epoch":frame.epoch,"public_time":frame.epoch+at,"event_time":frame.epoch+queued.time,"feature":queued.feature.is_some(),"impact_count":frame.impact_budget.count}));
            p.deferred_interval=Some(frame);
        }else if let Some((event_time,feature))=if PersistentTrial::uses_pair_queue(){p.pair_queue_before(frame.epoch+dt as f64)}else{None}{
            // Both event backends publish the same deferred-interval contract.
            events.push(serde_json::json!({"kind":"retained_interval_defer","epoch":frame.epoch,"public_time":frame.epoch+at,"event_time":event_time,"feature":feature,"impact_count":frame.impact_budget.count,"owner":"compact_pair_queue"}));p.deferred_interval=Some(frame);
        }else{p.finish_tick();}p.clock+=dt as f64;}
        events
    }
}
