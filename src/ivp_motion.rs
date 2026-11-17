//! Owned IVP100e8fc0 pose sampling, including100e7550/100e7440/100eda40.
//! Cache preparation and event scheduling are separate, not yet integrated.
use bevy::prelude::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Prediction {
    /// Native IVP axes, metres; these are physical cache inputs, not a public pose.
    pub start: Vec3,
    pub velocity: Vec3,
    pub rotation_start: [f32; 4],
    pub rotation_end: [f32; 4],
    pub time: f64,
    pub inverse_dt: f32,
}

#[derive(Clone, Copy)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct ObjectFrame {
    /// None is native's zero-shift flag, not a small-offset epsilon test.
    pub shift: Option<Vec3>,
    /// None is native's null core-from-object quaternion pointer.
    pub rotation: Option<[f32; 4]>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Pose {
    pub position: Vec3,
    pub rotation: Mat3,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CoreSample {
    pub pose: Pose,
    pub quaternion: Option<[f32; 4]>,
}

#[derive(Clone,Copy)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct PreparedObjectCache {
    pub core_position:Vec3,
    pub pose:Pose,
    pub quaternion:[f32;4],
    pub frame_stamp:u32,
}

/// Numeric cache contents, separate from the allocator/retention policy. A slot
/// can only be populated through source prediction and object-frame preparation.
#[derive(Default)]
pub(crate) struct PreparedFrames {slots:std::collections::HashMap<u32,(u64,PreparedObjectCache)>}
impl PreparedFrames {
    pub(crate) fn prepare(&mut self,slot:u32,owner:u64,prediction:Prediction,at:f64,object:ObjectFrame,stamp:u32)->Result<PreparedObjectCache,&'static str>{
        if slot==0||owner==0||!at.is_finite()||!prediction.time.is_finite(){return Err("invalid prepared-frame identity/time");}
        let frame=prediction.prepare_object_cache(at,object,stamp);
        if !frame.pose.position.is_finite()||!frame.pose.rotation.is_finite()||!frame.core_position.is_finite()||frame.quaternion.iter().any(|v|!v.is_finite()){return Err("nonfinite prepared frame");}
        self.slots.insert(slot,(owner,frame));Ok(frame)
    }
    pub(crate) fn get(&self,slot:u32,owner:u64,stamp:u32)->Option<&PreparedObjectCache>{
        let (actual,frame)=self.slots.get(&slot)?;(*actual==owner&&frame.frame_stamp==stamp).then_some(frame)
    }
    pub(crate) fn invalidate_owner(&mut self,owner:u64){self.slots.retain(|_,(actual,_)|*actual!=owner);}
}

impl Pose {
    /// Owned100eda40 (also100f4a50) accumulates the first two products,
    /// then Z, then translation.100e8fc0 uses this same routine for its shift.
    pub(crate) fn transform_point(&self, point:Vec3)->Vec3 {
        let rows=self.rotation.transpose();
        Vec3::new(
            ((rows.x_axis.y*point.y+rows.x_axis.x*point.x)+rows.x_axis.z*point.z)+self.position.x,
            ((rows.y_axis.y*point.y+rows.y_axis.x*point.x)+rows.y_axis.z*point.z)+self.position.y,
            ((rows.z_axis.y*point.y+rows.z_axis.x*point.x)+rows.z_axis.z*point.z)+self.position.z)
    }
}

// Preserve native float operation order; glam slerp normalizes differently.
fn interpolate(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let dot = ((b[0] * a[0] + b[1] * a[1]) + b[2] * a[2]) + b[3] * a[3];
    let (dot, sign) = if dot <= 0. { (-dot, -1.) } else { (dot, 1.) };
    // Owned DAT10161b6c, bit pattern3f7fbe77.
    if dot >= f32::from_bits(0x3f7fbe77) {
        let mut q = std::array::from_fn::<_, 4, _>(|i| a[i] + (b[i] * sign - a[i]) * t);
        let half = (((q[1] * q[1] + q[0] * q[0]) + q[2] * q[2]) + q[3] * q[3]) * 0.5;
        let mut scale = 1.5 - half;
        scale = (0.5 - scale * scale * half) + scale;
        scale = (0.5 - scale * scale * half) + scale;
        for v in &mut q {
            *v *= scale;
        }
        return q;
    }
    // Native retains these weights through x87 component multiplication.
    let dot = dot as f64;
    let angle = dot.acos();
    let inverse_sine = 1. / (1. - dot * dot).sqrt();
    let left = ((1. - t as f64) * angle).sin() * inverse_sine;
    let right = (angle * t as f64).sin() * inverse_sine * sign as f64;
    std::array::from_fn(|i| (a[i] as f64 * left + b[i] as f64 * right) as f32)
}

pub(crate) fn matrix(q: [f32; 4]) -> Mat3 {
    let [x, y, z, w] = q;
    let z2 = z * 2.;
    let zz = z * z2;
    let y2 = y * 2.;
    let xx = x * x * 2.;
    let wx = w * x * 2.;
    Mat3::from_cols(
        Vec3::new(1. - (zz + y * y2), w * z2 + x * y2, x * z2 - w * y2),
        Vec3::new(x * y2 - w * z2, 1. - (zz + xx), wx + y * z2),
        Vec3::new(w * y2 + x * z2, y * z2 - wx, 1. - (y * y2 + xx)),
    )
}

/// Owned1010f3b0 sweep uses the original delta, before endpoint normalization.
/// Rotation is the physical core matrix, not the interpolated object cache.
pub(crate) fn rotation_sweep(delta:[f32;4],rotation:Mat3,inverse_dt:f32,radius:f32)->(Vec3,f32,f32){
    let [x,y,z,_]=delta;
    let square=(x*x+y*y)+z*z;
    if square<=1e-10{return (Vec3::X,0.,0.);}
    let inverse=(1./(square as f64).sqrt()) as f32;let length=inverse*square;let squared_length=length*length;let cube=squared_length*length;
    let speed=(cube*squared_length*0.40413999557495117+(cube*0.1666666716337204+length))*2.*inverse_dt;
    let r=rotation.transpose();
    let axis=Vec3::new(((r.x_axis.y*y+r.x_axis.x*x)+r.x_axis.z*z)*inverse,
        ((r.y_axis.x*x+r.y_axis.y*y)+r.y_axis.z*z)*inverse,
        ((r.z_axis.x*x+r.z_axis.y*y)+r.z_axis.z*z)*inverse);
    (axis,speed,radius*speed)
}

fn compose(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [x, y, z, w] = a;
    let [bx, by, bz, bw] = b;
    [
        ((bw * x + bx * w) + bz * y) - by * z,
        ((by * w + bw * y) + bx * z) - bz * x,
        ((bw * z + bz * w) + by * x) - bx * y,
        ((bw * w - bx * x) - by * y) - bz * z,
    ]
}

pub(crate) struct AngularStep {
    pub delta: [f32; 4],
    /// Core-local angular velocity after the gyroscopic update.
    pub angular: Vec3,
    pub subdivisions: u32,
}
/// Original1010ef30 delta and physical matrix retained at prediction time.
/// Reconstructing this from normalized endpoints or post-gyro velocity loses
/// the sweep used by1010f3b0 and therefore changes future collision bounds.
#[derive(Clone,Copy,Debug)]
pub(crate) struct PredictionMotion {
    pub time:f64,
    pub dt:f32,
    pub delta:[f32;4],
    pub rotation:Mat3,
    pub inverse_dt:f32,
}

pub(crate) struct MotionStep {
    pub time: f64,
    pub dt: f32,
    pub velocity: Vec3,
    pub angular: Vec3,
    pub inertia: Vec3,
    pub inverse_inertia: Vec3,
    /// Native environment anomaly limits; not necessarily the query interval.
    pub limits: Vec2,
    pub angular_limit_exact: f64,
    pub gyro: bool,
}

fn polynomial_delta(angular: Vec3, dt: f32) -> [f32; 4] {
    let half = dt * 0.5;
    let xyz = angular.to_array().map(|w| {
        let a = w * half;
        a - ((a * a) * a) * (1. / 6.)
    });
    //100e72d0 retains the three squared components through x87 FSQRT.
    let [x, y, z] = xyz.map(|v| v as f64);
    [
        xyz[0],
        xyz[1],
        xyz[2],
        (1. - ((y * y + x * x) + z * z)).sqrt() as f32,
    ]
}

pub(crate) fn sine_delta(angular: Vec3, dt: f32) -> [f32; 4] {
    let half = dt as f64 * 0.5;
    let mut v = angular.to_array().map(|w| (w as f64 * half).sin() as f32);
    let squared = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1]) + v[2] * v[2];
    let mut length = squared(v);
    if length > 1. {
        let scale = 0.997f32 / length.sqrt();
        v = v.map(|w| w * scale);
        //100e7380 uses Y then X after the exceptional reduction.
        length = (v[1] * v[1] + v[0] * v[0]) + v[2] * v[2];
    }
    [v[0], v[1], v[2], (1. - length as f64).sqrt() as f32]
}

/// Owned1010ef30 ordinary dynamic and gyroscopic-disabled paths. Inertia and
/// angular velocity use native core-local axes, not world/render axes.
/// Pinned/wheel and environment-stage selection are handled by the caller.
pub(crate) fn angular_step(
    mut angular: Vec3,
    inertia: Vec3,
    inverse: Vec3,
    dt: f32,
    gyro: bool,
) -> AngularStep {
    if !gyro {
        return AngularStep {
            delta: sine_delta(angular, dt),
            angular,
            subdivisions: 1,
        };
    }
    let squared =
        ((angular.x * angular.x + angular.y * angular.y) + angular.z * angular.z) * dt * dt;
    let subdivisions = if squared > 0.0277777798473835 {
        ((squared as f64 * 143.99998474121094).sqrt() as u32) + 1
    } else {
        1
    };
    let h = dt / subdivisions as f32;
    let coupling = Vec3::new(
        inertia.y - inertia.z,
        inertia.z - inertia.x,
        inertia.x - inertia.y,
    ) * inverse;
    let mut delta = polynomial_delta(angular, h);
    for i in 0..subdivisions {
        if i > 0 {
            delta = compose(polynomial_delta(angular, h), delta);
        }
        let old = angular;
        angular.x = ((old.z * old.y) * coupling.x) * h + old.x;
        angular.y = ((old.z * old.x) * coupling.y) * h + old.y;
        angular.z = ((old.y * old.x) * coupling.z) * h + old.z;
    }
    AngularStep {
        delta,
        angular,
        subdivisions,
    }
}

fn normalize_prediction(mut q: [f32; 4]) -> [f32; 4] {
    let square = ((q[0] * q[0] + q[1] * q[1]) + q[2] * q[2]) + q[3] * q[3];
    if (1. - square).abs() > 1e-6 {
        let mut scale = 1.5 - square * 0.5;
        let mut error = 1. - scale * scale * square;
        while error.abs() > 1e-6 {
            scale += error * 0.5;
            error = 1. - scale * scale * square;
        }
        for v in &mut q {
            *v *= scale;
        }
    }
    q
}

/// Owned ordinary-core quaternion composition and conditional normalization.
pub(crate) fn rotation_end(start: [f32;4], delta: [f32;4])->[f32;4] {
    normalize_prediction(compose(start,delta))
}

impl Prediction {
    /// Cache portion of owned1010f960 for an ordinary dynamic core. Broadphase
    /// bounds/queues and pinned/wheel/static branches remain outside this port.
    pub(crate) fn advance(&mut self, input: MotionStep) -> AngularStep {
        let mut velocity = input.velocity;
        let mut angular = input.angular;
        crate::ivp_impact::clip_vectors(
            &mut velocity,
            &mut angular,
            input.limits.x,
            input.limits.y,
            input.angular_limit_exact,
        );
        let result = angular_step(
            angular,
            input.inertia,
            input.inverse_inertia,
            input.dt,
            input.gyro,
        );
        let elapsed = input.time - self.time;
        let short = elapsed as f32;
        self.start = Vec3::new(
            (elapsed * self.velocity.x as f64 + self.start.x as f64) as f32,
            self.velocity.y * short + self.start.y,
            self.velocity.z * short + self.start.z,
        );
        self.time = input.time;
        self.inverse_dt = input.dt.recip();
        self.velocity = velocity;
        self.rotation_start = self.rotation_end;
        self.rotation_end = rotation_end(self.rotation_start, result.delta);
        result
    }
    /// Owned100f04b0 reconstructs collision-time local angular velocity from
    /// the full prediction's relative quaternion, before trimming its endpoint.
    /// This is not the pre-gyro or post-gyro velocity. Keep per-component asin
    /// and the rounded float scale used by the native x87 path.
    pub(crate) fn impact_angular(&self) -> Vec3 {
        let [x,y,z,w]=self.rotation_start;
        let relative=compose([-x,-y,-z,w],self.rotation_end);
        let scale=(self.inverse_dt+self.inverse_dt) as f64;
        Vec3::from_array(std::array::from_fn(|i|((relative[i] as f64).asin()*scale) as f32))
    }
    /// The impact scheduler commits the event's interpolated rotation before
    /// solving impulses; restamp_impact then promotes it to the new anchor.
    pub(crate) fn trim_impact_rotation(&mut self, at:f64) {
        let fraction=((at-self.time)*self.inverse_dt as f64) as f32;
        self.rotation_end=interpolate(self.rotation_start,self.rotation_end,fraction);
    }
    /// Restart a prediction after an impact, up to the current PSI deadline.
    /// The event scheduler must already have trimmed rotation_end to the
    /// impact rotation. The response has committed core-local angular velocity;
    /// this short continuation does not perform another gyroscopic update.
    pub(crate) fn restamp_impact(&mut self, at:f64, deadline:f64, velocity:Vec3, angular:Vec3) -> Option<()> {
        let dt=(deadline-at) as f32;
        if !at.is_finite() || !deadline.is_finite() || dt<=0. || !dt.is_finite()
            || at<self.time || !velocity.is_finite() || !angular.is_finite() {return None;}
        let elapsed=at-self.time;let short=elapsed as f32;
        self.start=Vec3::new(
            (elapsed*self.velocity.x as f64+self.start.x as f64) as f32,
            self.velocity.y*short+self.start.y,self.velocity.z*short+self.start.z);
        self.time=at;self.inverse_dt=dt.recip();self.velocity=velocity;
        self.rotation_start=self.rotation_end;
        self.rotation_end=rotation_end(self.rotation_start,sine_delta(angular,dt));
        Some(())
    }
    ///100ef7e0 is the physical-core sampler used by point impulses. Unlike
    /// public/object sampling it spills elapsed time before interpolation and
    /// copies the current physical matrix for inactive/equal-time requests.
    pub(crate) fn sample_physical_core(&self, at: f64, movement: u8, current: Pose) -> CoreSample {
        if movement >= 8 { return CoreSample { pose: current, quaternion: None }; }
        let elapsed = at - self.time;
        if elapsed == 0. { return CoreSample { pose: current, quaternion: None }; }
        let short = elapsed as f32;
        let q = interpolate(self.rotation_start, self.rotation_end, self.inverse_dt * short);
        CoreSample { pose: Pose { rotation: matrix(q), position: Vec3::new(
            self.velocity.x * short + self.start.x,
            self.velocity.y * short + self.start.y,
            self.velocity.z * short + self.start.z,
        ) }, quaternion: Some(q) }
    }
    /// Sample without clamping time: IVP also extrapolates outside the interval.
    pub(crate) fn sample(&self, at: f64, object: ObjectFrame) -> Pose {
        let elapsed = at - self.time;
        let q = interpolate(
            self.rotation_start,
            self.rotation_end,
            (elapsed * self.inverse_dt as f64) as f32,
        );
        let mut rotation = matrix(q);
        let short = elapsed as f32;
        // Native X retains the extended delta through multiply/add, Y/Z spill it.
        let mut position = Vec3::new(
            (elapsed * self.velocity.x as f64 + self.start.x as f64) as f32,
            self.velocity.y * short + self.start.y,
            self.velocity.z * short + self.start.z,
        );
        if let Some(s) = object.shift {
            // Offset uses the core rotation, BEFORE the object quaternion.
            // Owned100e8fc0 calls100eda40 at100e907e. Preserve its SSE
            // Y/X, then Z, then translation order; the decompiler's flattened
            // additions misleadingly suggested a different inline expression.
            position = Pose {position,rotation}.transform_point(s);
        }
        if let Some(object_rotation) = object.rotation {
            rotation = matrix(compose(q, object_rotation));
        }
        Pose { position, rotation }
    }

    ///100e6fd0 differs from100e8fc0: equal timestamps copy the stored quaternion
    ///and position, and nonzero elapsed position uses a single delta on all axes.
    ///The offset is applied with the core orientation before optional object rotation.
    pub(crate) fn prepare_object_cache(&self,at:f64,object:ObjectFrame,frame_stamp:u32)->PreparedObjectCache {
        let elapsed=at-self.time;
        let mut quaternion=if elapsed==0. {self.rotation_start}else{interpolate(self.rotation_start,self.rotation_end,(elapsed*self.inverse_dt as f64) as f32)};
        let core_position=if elapsed==0. {self.start}else{self.velocity*(elapsed as f32)+self.start};
        let mut pose=Pose {position:core_position,rotation:matrix(quaternion)};
        if let Some(shift)=object.shift {pose.position=pose.transform_point(shift);}
        if let Some(rotation)=object.rotation {quaternion=compose(quaternion,rotation);pose.rotation=matrix(quaternion);}
        PreparedObjectCache {core_position,pose,quaternion,frame_stamp}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_delta_sweep_matches_isolated_native_controls(){
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sweep.json")).unwrap();
        for c in fixture["cases"].as_array().unwrap(){
            let v=|k:&str,i:usize|c[k][i].as_f64().unwrap() as f32;
            let rotation=Mat3::from_cols(Vec3::new(v("rotation",0),v("rotation",4),v("rotation",8)),Vec3::new(v("rotation",1),v("rotation",5),v("rotation",9)),Vec3::new(v("rotation",2),v("rotation",6),v("rotation",10)));
            let (axis,speed,bound)=rotation_sweep(std::array::from_fn(|i|v("delta",i)),rotation,c["inverse_dt"].as_f64().unwrap() as f32,c["radius"].as_f64().unwrap() as f32);
            for (i,actual) in [axis.x,axis.y,axis.z,speed,bound].into_iter().enumerate(){
                let expected=v("expected",i);assert!(actual==expected,"model{} step{} channel{i}: {actual} != {expected}",c["model"],c["step"]);
            }
        }
    }

    #[test]
    fn prepared_frame_slots_rebind_and_reject_stale_owner_or_stamp(){
        let prediction=Prediction{start:Vec3::ZERO,velocity:Vec3::X,rotation_start:[0.,0.,0.,1.],rotation_end:[0.,0.,0.,1.],time:0.,inverse_dt:60.};
        let object=ObjectFrame{shift:None,rotation:None};let mut frames=PreparedFrames::default();
        frames.prepare(1,10,prediction,0.,object,7).unwrap();assert!(frames.get(1,10,7).is_some());assert!(frames.get(1,10,8).is_none());
        frames.prepare(1,20,prediction,1.,object,8).unwrap();assert!(frames.get(1,10,7).is_none());assert_eq!(frames.get(1,20,8).unwrap().pose.position,Vec3::X);
        assert!(frames.prepare(0,30,prediction,0.,object,1).is_err());assert!(frames.prepare(1,20,prediction,f64::NAN,object,9).is_err());assert!(frames.get(1,20,8).is_some());
        frames.invalidate_owner(20);assert!(frames.get(1,20,8).is_none());
    }

    #[test]
    fn native_object_cache_preparation_matches_prediction_and_object_inputs() {
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-object-cache.json")).unwrap();
        let f=|v:&serde_json::Value|v.as_f64().unwrap() as f32;
        let vec=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));
        let quat=|v:&serde_json::Value|std::array::from_fn(|i|f(&v[i]));let mut count=0;
        for (i,c) in data["cases"].as_array().unwrap().iter().enumerate() {
            let b=&c["before"];let a=&c["after"];
            let p=Prediction {start:vec(&b["start"]),velocity:vec(&b["velocity"]),rotation_start:quat(&b["rotation_start"]),rotation_end:quat(&b["rotation_end"]),time:b["time"].as_f64().unwrap(),inverse_dt:f(&b["inverse_dt"])};
            let object=ObjectFrame {shift:(b["flags"].as_u64().unwrap()&0x800==0).then(||vec(&b["shift"])),rotation:b["extra"].as_bool().unwrap().then(||quat(&b["object_rotation"]))};
            let o=p.prepare_object_cache(b["now"].as_f64().unwrap(),object,b["frame"].as_u64().unwrap() as u32);
            assert_eq!(o.core_position,vec(&a["core_position"]),"core position{i}");assert_eq!(o.pose.position,vec(&a["position"]),"position{i}");assert_eq!(o.quaternion,quat(&a["quaternion"]),"quaternion{i}");
            assert_eq!(o.pose.rotation.transpose().to_cols_array(),std::array::from_fn(|j|f(&a["rotation_rows"][j])),"matrix{i}");assert_eq!(o.frame_stamp,a["frame"].as_u64().unwrap() as u32);count+=1;
        }
        assert_eq!(count,848);
    }

    #[test]
    fn owned_collision_angular_is_reconstructed_from_full_prediction() {
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-angular.json")).unwrap();
        let quat=|a:&serde_json::Value|std::array::from_fn::<_,4,_>(|i|a[i].as_f64().unwrap() as f32);
        let mut max=0f32;
        for c in fixture["cases"].as_array().unwrap() {
            let start=quat(&c["rotation_start"]);
            let p=Prediction {start:Vec3::ZERO,velocity:Vec3::ZERO,rotation_start:start,
                rotation_end:rotation_end(start,quat(&c["delta"])),time:0.,inverse_dt:c["inverse_dt"].as_f64().unwrap() as f32};
            let expected=Vec3::from_array(std::array::from_fn(|i|c["expected"][i].as_f64().unwrap() as f32));
            let error=(p.impact_angular()-expected).abs().max_element();max=max.max(error);
            assert!(error<0.00003,"{} step{} error{}",c["model"],c["step"],error);
        }
        println!("{} native collision angular reconstructions: max{} rad/s",fixture["cases"].as_array().unwrap().len(),max);
    }
    #[test]
    fn owned_first_impacts_trim_the_free_rotation_at_event_time() {
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-trim.json")).unwrap();
        let quat=|a:&serde_json::Value|std::array::from_fn::<_,4,_>(|i|a[i].as_f64().unwrap() as f32);
        let mut max=0f32;
        for c in fixture["cases"].as_array().unwrap() {
            let mut p=Prediction {start:Vec3::ZERO,velocity:Vec3::ZERO,
                rotation_start:quat(&c["rotation_start"]),rotation_end:quat(&c["full_rotation_end"]),
                time:c["time"].as_f64().unwrap(),inverse_dt:c["inverse_dt"].as_f64().unwrap() as f32};
            let original=p;p.trim_impact_rotation(c["at"].as_f64().unwrap());
            for (a,b) in p.rotation_end.into_iter().zip(quat(&c["expected_trimmed_end"])) {max=max.max((a-b).abs());}
            assert_eq!(p.rotation_start,original.rotation_start);assert_eq!(p.time,original.time);assert_eq!(p.inverse_dt,original.inverse_dt);
        }
        println!("6 native first-event rotation trims: max quaternion error{max}");
        assert!(max<0.000002);
    }
    #[test]
    fn owned_glass_impact_cache_restamps_match_remaining_tick_predictions() {
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-restamp.json")).unwrap();
        let vec=|a:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|a[i].as_f64().unwrap() as f32));
        let quat=|a:&serde_json::Value|std::array::from_fn::<_,4,_>(|i|a[i].as_f64().unwrap() as f32);
        let mut max_p=0f32;let mut max_q=0f32;let mut max_inverse=0f32;
        for case in fixture["cases"].as_array().unwrap() {
            let b=&case["before"];let expected=&case["expected"];
            let mut p=Prediction {start:vec(&b["cache_start"]),velocity:vec(&b["cache_velocity"]),
                rotation_start:quat(&b["rotation_start"]),rotation_end:quat(&b["rotation_end"]),
                time:b["cache_time"].as_f64().unwrap(),inverse_dt:b["inverse_dt"].as_f64().unwrap() as f32};
            p.restamp_impact(case["event_time"].as_f64().unwrap(),case["deadline"].as_f64().unwrap(),
                vec(&case["velocity_after"]),vec(&case["angular_after"])).unwrap();
            max_p=max_p.max(p.start.distance(vec(&expected["start"])));
            for (a,b) in p.rotation_end.into_iter().zip(quat(&expected["rotation_end"])) {max_q=max_q.max((a-b).abs());}
            let inverse=expected["inverse_dt"].as_f64().unwrap() as f32;
            max_inverse=max_inverse.max((p.inverse_dt-inverse).abs()/inverse);
            assert_eq!(p.rotation_start,quat(&expected["rotation_start"]));
            assert_eq!(p.velocity,vec(&expected["velocity"]));assert_eq!(p.time,expected["time"].as_f64().unwrap());
        }
        println!("30 native impact restamps: position{max_p}m quaternion{max_q} inverse_dt_relative{max_inverse}");
        assert!(max_p<0.000002 && max_q<0.000002 && max_inverse<0.000001);
    }
    #[test]
    fn invalid_impact_deadlines_leave_prediction_unchanged() {
        for at in [f64::NAN,-1.,1.,2.] {
            let mut p=cache();let before=p;
            assert!(p.restamp_impact(at,1.,Vec3::ZERO,Vec3::ZERO).is_none());
            assert_eq!(p.start,before.start);assert_eq!(p.rotation_end,before.rotation_end);
            assert_eq!(p.time,before.time);assert_eq!(p.inverse_dt,before.inverse_dt);
        }
    }
    fn cache() -> Prediction {
        Prediction {
            start: Vec3::ZERO,
            velocity: Vec3::X,
            rotation_start: [0., 0., 0., 1.],
            rotation_end: [0., 0., 0., 1.],
            time: 0.,
            inverse_dt: 60.,
        }
    }
    const ORIGIN: ObjectFrame = ObjectFrame {
        shift: None,
        rotation: None,
    };

    #[test]
    fn pose_query_extrapolates_and_keeps_double_clock_precision() {
        let mut p = cache();
        p.time = 1_000_000_000.123456;
        assert_eq!(p.sample(p.time - 0.25, ORIGIN).position, Vec3::X * -0.25);
        assert_eq!(p.sample(p.time + 1.5, ORIGIN).position, Vec3::X * 1.5);
    }
    #[test]
    fn quaternion_antipodes_preserve_pose_and_offsets_use_core_frame() {
        let mut p = cache();
        p.velocity = Vec3::ZERO;
        p.rotation_end = [0., 0., 0., -1.];
        let pose = p.sample(
            0.005,
            ObjectFrame {
                shift: Some(Vec3::X),
                rotation: Some([0., 0., 1., 0.]),
            },
        );
        assert_eq!(pose.position, Vec3::X);
        assert_eq!(pose.rotation * Vec3::X, Vec3::NEG_X);
    }

    #[test]
    fn owned_large_clock_pose_keeps_reference_bits_and_composed_offset_order() {
        // Owned100e8fc0 controlled case7/2/3. An inexact JSON double parser
        // changes the query by one ULP here and produces a false pose mismatch.
        let clocks: serde_json::Value =
            serde_json::from_str(r#"{"start":1000000000.123456,"query":1000000000.1261567}"#)
                .unwrap();
        let start = clocks["start"].as_f64().unwrap();
        let query = clocks["query"].as_f64().unwrap();
        assert_eq!(start.to_bits(), 4741671816367426920);
        assert_eq!(query.to_bits(), 4741671816367449575);
        let prediction = Prediction {
            start: Vec3::new(1.23456705, -2.34567809, 0.987654328),
            velocity: Vec3::new(17.2345009, -3.45670009, 7.65430021),
            rotation_start: [-0.200441465, 0.30066219, -0.40088293, 0.841854095],
            rotation_end: [0.703917623, -0.100559667, 0.201119334, 0.673749745],
            time: start,
            inverse_dt: 137.,
        };
        let actual = prediction.sample(
            query,
            ObjectFrame {
                shift: Some(Vec3::new(0.25, -0.5, 0.125)),
                rotation: Some([0.5, -0.5, 0.5, 0.5]),
            },
        );
        assert!(
            actual
                .position
                .distance(Vec3::new(1.30131626, -2.91286731, 0.879811347))
                < 0.000002
        );
        let expected = [
            0.260846674,
            -0.85331291,
            -0.451459885,
            -0.415383756,
            0.322937548,
            -0.85039264,
            0.871444345,
            0.40935117,
            -0.270215154,
        ];
        for (a, b) in actual
            .rotation
            .transpose()
            .to_cols_array()
            .iter()
            .zip(expected)
        {
            assert!((a - b).abs() < 0.000002);
        }
    }

    #[test]
    fn owned_anisotropic_spin_uses_subdivision_and_changes_local_angular_velocity() {
        let a = angular_step(
            Vec3::new(100., -80., 50.),
            Vec3::new(0.025, 0.07, 0.11),
            Vec3::new(40., 14.2857141, 9.090909),
            1. / 60.,
            true,
        );
        assert_eq!(a.subdivisions, 28);
        assert!(
            (a.angular - Vec3::new(106.373703, 80.7325974, 53.2656593))
                .abs()
                .max_element()
                < 0.00002
        );
        for (x, y) in a
            .delta
            .iter()
            .zip([0.899935544, -0.033309985, 0.174501717, 0.398190528])
        {
            assert!((x - y).abs() < 0.000002);
        }
    }

    #[test]
    fn owned_cache_update_advances_old_prediction_before_installing_new_motion() {
        let mut p = Prediction {
            start: Vec3::new(1.25, -2.5, 0.75),
            velocity: Vec3::new(-3.5, 0.25, 1.75),
            rotation_start: [0.5; 4],
            rotation_end: [-0.5, 0.5, -0.5, 0.5],
            time: 0.020820989655388856,
            inverse_dt: 6000.00537,
        };
        let a = p.advance(MotionStep {
            time: 0.033166668555388856,
            dt: 1. / 60.,
            velocity: Vec3::new(80., 5., -15.),
            angular: Vec3::new(75., 10., -20.),
            inertia: Vec3::new(0.025, 0.07, 0.11),
            inverse_inertia: Vec3::new(40., 14.2857141, 9.090909),
            limits: Vec2::new(50.7999992, 57.1198616),
            angular_limit_exact: 57.11986178589723,
            gyro: true,
        });
        assert!(
            p.start
                .distance(Vec3::new(1.20679009, -2.49691367, 0.771604955))
                < 0.000002
        );
        assert!(
            p.velocity
                .distance(Vec3::new(49.3376083, 3.08360052, -9.25080109))
                < 0.00002
        );
        assert!(
            a.angular
                .distance(Vec3::new(49.3966331, -7.10043716, -13.2675638))
                < 0.00002
        );
        assert_eq!(p.rotation_start, [-0.5, 0.5, -0.5, 0.5]);
        for (x, y) in
            p.rotation_end
                .iter()
                .zip([-0.302145243, 0.203151599, -0.709006548, 0.603943288])
        {
            assert!((x - y).abs() < 0.000002);
        }
        assert_eq!(p.time, 0.033166668555388856);
        assert_eq!(p.inverse_dt, (1f32 / 60.).recip());
    }

    #[test]
    #[ignore = "requires independently captured owned angular integration inputs/readbacks"]
    fn replay_owned_angular_report() {
        use serde_json::{Value, json};
        let source: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("L4D2R_IVP_ANGULAR_REFERENCE").unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            source["native_module_sha256"],
            "45aa54cd8907b62ba1e7798ed3b3a27575ba761b500968338dcdfbf3ee93327d"
        );
        let inputs = source["cases"].as_array().unwrap();
        assert_eq!(inputs.len(), 96);
        let mut ids = std::collections::HashSet::new();
        let mut cases = Vec::new();
        let mut worst_delta = 0f32;
        let mut worst_angular = 0f32;
        for v in inputs {
            let label = format!(
                "{}/{}/{}/{}",
                v["kind"], v["timestep"], v["profile"], v["gyro"]
            );
            assert!(ids.insert(label.clone()));
            let array = |key: &str| {
                v[key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_f64().unwrap() as f32)
                    .collect::<Vec<_>>()
            };
            let vector = |key| Vec3::from_slice(&array(key));
            let a = angular_step(
                vector("angular_before"),
                vector("inertia"),
                vector("inverse_inertia"),
                v["dt"].as_f64().unwrap() as f32,
                v["gyro"].as_bool().unwrap(),
            );
            let delta_error = a
                .delta
                .iter()
                .zip(array("delta"))
                .map(|(a, b)| (a - b).abs())
                .fold(0., f32::max);
            let angular_error = (a.angular - vector("angular_after")).abs().max_element();
            assert!(delta_error.is_finite() && angular_error.is_finite());
            worst_delta = worst_delta.max(delta_error);
            worst_angular = worst_angular.max(angular_error);
            cases.push(json!({"case":label,"delta_error":delta_error,"angular_error_rad_s":angular_error,"subdivisions":a.subdivisions}));
        }
        let report = json!({"cases":cases,"max_delta_error":worst_delta,"max_angular_error_rad_s":worst_angular,"scope":"ordinary and gyro-disabled rotation kernel only; pinned/wheel/cache/events are separate"});
        if let Ok(p) = std::env::var("L4D2R_IVP_ANGULAR_REPORT") {
            std::fs::write(p, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
        println!("96 cases max_delta={worst_delta} max_angular={worst_angular}rad/s");
        assert!(worst_delta < 0.000002, "delta error{worst_delta}");
        assert!(worst_angular < 0.00002, "angular error{worst_angular}");
    }

    #[test]
    #[ignore = "requires independently captured owned1010f960 cache-update readbacks"]
    fn replay_owned_cache_report() {
        use serde_json::{Value, json};
        let source: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("L4D2R_IVP_CACHE_REFERENCE").unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            source["native_module_sha256"],
            "45aa54cd8907b62ba1e7798ed3b3a27575ba761b500968338dcdfbf3ee93327d"
        );
        let inputs = source["cases"].as_array().unwrap();
        assert_eq!(inputs.len(), 96);
        let array = |v: &Value, key: &str| {
            v[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap() as f32)
                .collect::<Vec<_>>()
        };
        let vector = |v: &Value, key| Vec3::from_slice(&array(v, key));
        let quat = |v: &Value, key| <[f32; 4]>::try_from(array(v, key)).unwrap();
        let mut maxima = [0f32; 4];
        let mut cases = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for v in inputs {
            let label = format!(
                "{}/{}/{}/{}",
                v["kind"], v["timestep"], v["profile"], v["gyro"]
            );
            assert!(ids.insert(label.clone()));
            let c = &v["cache"];
            assert_eq!(c["rva"], "1010f960");
            let b = &c["before"];
            let expected = &c["after"];
            let mut p = Prediction {
                start: vector(b, "start"),
                velocity: vector(b, "velocity"),
                rotation_start: quat(b, "rotation_start"),
                rotation_end: quat(b, "rotation_end"),
                time: b["time"].as_f64().unwrap(),
                inverse_dt: b["inverse_dt"].as_f64().unwrap() as f32,
            };
            let a = p.advance(MotionStep {
                time: c["time"].as_f64().unwrap(),
                dt: v["dt"].as_f64().unwrap() as f32,
                velocity: vector(c, "new_velocity"),
                angular: vector(v, "angular_before"),
                inertia: vector(v, "inertia"),
                inverse_inertia: vector(v, "inverse_inertia"),
                limits: Vec2::new(
                    c["linear_limit"].as_f64().unwrap() as f32,
                    c["angular_limit"].as_f64().unwrap() as f32,
                ),
                angular_limit_exact: c["angular_limit_exact"].as_f64().unwrap(),
                gyro: v["gyro"].as_bool().unwrap(),
            });
            let errors = [
                (p.start - vector(expected, "start")).abs().max_element(),
                (p.velocity - vector(expected, "velocity"))
                    .abs()
                    .max_element(),
                (a.angular - vector(c, "angular_after")).abs().max_element(),
                p.rotation_end
                    .iter()
                    .zip(quat(expected, "rotation_end"))
                    .map(|(a, b)| (a - b).abs())
                    .fold(0., f32::max),
            ];
            assert_eq!(p.rotation_start, quat(expected, "rotation_start"));
            assert_eq!(
                p.time.to_bits(),
                expected["time"].as_f64().unwrap().to_bits()
            );
            assert_eq!(
                p.inverse_dt,
                expected["inverse_dt"].as_f64().unwrap() as f32
            );
            for (max, error) in maxima.iter_mut().zip(errors) {
                assert!(error.is_finite());
                *max = max.max(error);
            }
            cases.push(json!({"case":label,"position_error_m":errors[0],"velocity_error_m_s":errors[1],"angular_error_rad_s":errors[2],"quaternion_error":errors[3],"subdivisions":a.subdivisions}));
        }
        let report = json!({"cases":cases,"max_position_error_m":maxima[0],"max_velocity_error_m_s":maxima[1],"max_angular_error_rad_s":maxima[2],"max_quaternion_error":maxima[3],"clock_and_inverse_dt_exact":true,
            "scope":"ordinary dynamic core cache portion of1010f960; bound expansion/queue, contact ordering, pinned/wheel/static and full event integration are not covered"});
        if let Ok(p) = std::env::var("L4D2R_IVP_CACHE_REPORT") {
            std::fs::write(p, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
        println!("96 cache updates max_errors={maxima:?}");
        assert!(
            maxima[0] < 0.000002
                && maxima[1] < 0.00002
                && maxima[2] < 0.00002
                && maxima[3] < 0.000002,
            "native cache errors{maxima:?}"
        );
    }

    #[test]
    #[ignore = "requires independently captured owned native pose/cache readbacks"]
    fn replay_owned_pose_report() {
        use serde_json::{Value, json};
        let path = std::env::var("L4D2R_IVP_POSE_REFERENCE").unwrap();
        let source: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let array = |v: &Value, key: &str| -> Vec<f32> {
            v[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap() as f32)
                .collect()
        };
        let mut count = 0;
        let mut worst_position = 0f32;
        let mut worst_matrix = 0f32;
        let mut cases = Vec::new();
        let mut compare = |v: &Value, label: String| {
            let vec = |key| Vec3::from_slice(&array(v, key));
            let quat = |key| <[f32; 4]>::try_from(array(v, key)).unwrap();
            let p = Prediction {
                start: vec("prediction_start_ivp"),
                velocity: vec("prediction_velocity_ivp"),
                rotation_start: quat("prediction_quat_start"),
                rotation_end: quat("prediction_quat_end"),
                time: v["prediction_time"].as_f64().unwrap(),
                inverse_dt: v["prediction_inv_dt"].as_f64().unwrap() as f32,
            };
            if let Some(bits) = v["prediction_time_bits"].as_u64() {
                assert_eq!(
                    p.time.to_bits(),
                    bits,
                    "reference parser changed native start clock"
                );
                assert_eq!(
                    v["env_time"].as_f64().unwrap().to_bits(),
                    v["env_time_bits"].as_u64().unwrap(),
                    "reference parser changed native query clock"
                );
            }
            let object = ObjectFrame {
                shift: if v["zero_shift"] == true {
                    None
                } else {
                    Some(vec("object_shift_ivp"))
                },
                rotation: if v["has_object_quat"] == false {
                    None
                } else {
                    Some(quat("object_quat"))
                },
            };
            let actual = p.sample(v["env_time"].as_f64().unwrap(), object);
            let expected = array(v, "sample_matrix_ivp");
            assert_eq!(expected.len(), 12);
            let pos = actual.position.distance(Vec3::from_slice(&expected[9..]));
            let rows = actual.rotation.transpose().to_cols_array();
            let rot = rows
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0., f32::max);
            assert!(pos.is_finite() && rot.is_finite());
            worst_position = worst_position.max(pos);
            worst_matrix = worst_matrix.max(rot);
            count += 1;
            cases.push(
                json!({"case":label,"position_error_metres":pos,"rotation_matrix_error":rot}),
            );
        };
        if let Some(inputs) = source["cases"].as_array() {
            assert_eq!(
                source["native_module_sha256"],
                "45aa54cd8907b62ba1e7798ed3b3a27575ba761b500968338dcdfbf3ee93327d"
            );
            assert_eq!(inputs.len(), 160);
            let mut identities = std::collections::HashSet::new();
            for v in inputs {
                let label = format!("{}/{}/{}", v["kind"], v["fraction_case"], v["mode"]);
                assert!(identities.insert(label.clone()));
                compare(v, label);
            }
        } else {
            let samples = source["samples"].as_array().unwrap();
            assert_eq!(samples.len(), 241);
            for s in samples {
                for (i, v) in s["bodies"].as_array().unwrap().iter().enumerate() {
                    compare(v, format!("{}/{i}", s["step"]));
                }
            }
        }
        let report = json!({"samples":count,"max_position_error_metres":worst_position,"max_rotation_matrix_error":worst_matrix,"cases":cases,
            "scope":"pose query only; cache preparation, event clock and contact scheduling are not implemented by this component"});
        if let Ok(path) = std::env::var("L4D2R_IVP_POSE_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
        println!("samples={count} max_position={worst_position}m max_matrix={worst_matrix}");
        assert!(
            worst_position < 0.000002,
            "native position error{worst_position}"
        );
        assert!(
            worst_matrix < 0.000002,
            "native rotation matrix error{worst_matrix}"
        );
    }
}
