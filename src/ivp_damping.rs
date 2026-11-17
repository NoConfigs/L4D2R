//! Owned 100ef970 damping and 100f10e0's movement-state adjustment.
//! PSI preparation commits external impulses first; gravity follows damping.
use bevy::prelude::*;

pub(crate) fn apply(
    velocity: &mut Vec3,
    angular: &mut Vec3,
    dt: f32,
    linear_rate: f32,
    mut angular_rate: Vec3,
    movement_state: u8,
) {
    let mut linear_rate=linear_rate;
    if movement_state>1 {linear_rate+=0.1;angular_rate+=Vec3::splat(0.1);}
    let a=angular_rate*dt;
    let squared=(a.x*a.x+a.y*a.y)+a.z*a.z;
    let factors=if squared>=0.5 {
        // Native x87 exponential is retained until its float store.
        Vec3::from_array(a.to_array().map(|v| (-(v as f64)).exp() as f32))
    } else {Vec3::ONE-a};
    let l=dt*linear_rate;
    let linear=if l>=0.25 {(-(l as f64)).exp() as f32} else {1.-l};
    *angular*=factors;
    *velocity*=linear;
}

///100eabe0 force stage:100f10e0 damps raw motion,100f13b0 commits queued
/// impulses, then gravity is added. The caller produces movement state.
pub(crate) fn prepare_forces(core:&mut crate::ivp_contact::Core,dt:f32,linear_rate:f32,angular_rate:Vec3,movement_state:u8,gravity:Vec3)->Result<(),&'static str>{
    if !dt.is_finite()||dt<=0.||!linear_rate.is_finite()||linear_rate<0.||!angular_rate.is_finite()||angular_rate.min_element()<0.||!gravity.is_finite()
        ||![core.linear,core.angular,core.pending_linear,core.pending_angular].iter().all(|v|v.is_finite()){return Err("invalid force preparation input");}
    let mut next=*core;apply(&mut next.linear,&mut next.angular,dt,linear_rate,angular_rate,movement_state);
    next.linear+=next.pending_linear;next.angular+=next.pending_angular;
    next.pending_linear=Vec3::ZERO;next.pending_angular=Vec3::ZERO;
    next.linear+=gravity*dt;
    if !next.linear.is_finite()||!next.angular.is_finite(){return Err("force preparation overflow");}
    *core=next;Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture()->serde_json::Value {
        serde_json::from_str(include_str!("../tools/fixtures/native-ivp-damping.json")).unwrap()
    }
    fn v(a:&serde_json::Value)->Vec3 {
        Vec3::from_array(std::array::from_fn(|i|a[i].as_f64().unwrap() as f32))
    }
    #[test]
    fn native_damping_pending_commit_and_gravity_match_force_stage_boundaries(){
        let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-force-stage.json")).unwrap();let (mut count,mut idle,mut queued_idle)=(0,0,0);
        let f=|v:&serde_json::Value|v.as_f64().unwrap() as f32;
        for row in d["cases"].as_array().unwrap(){let b=&row["before"];let a=&row["after"];
            let mut core=crate::ivp_contact::Core{inverse_mass:f(&b["inverse_mass"]),inverse_inertia:v(&b["inverse_inertia"]),linear:v(&b["linear"]),angular:v(&b["angular"]),pending_linear:v(&b["pending_linear"]),pending_angular:v(&b["pending_angular"])};
            let properties=(core.inverse_mass,core.inverse_inertia);let state=row["movement_state"].as_u64().unwrap() as u8;
            if state>1{idle+=1;if core.pending_linear!=Vec3::ZERO||core.pending_angular!=Vec3::ZERO{queued_idle+=1;}}
            prepare_forces(&mut core,f(&row["dt"]),f(&row["linear_rate"]),v(&row["angular_rate"]),state,v(&row["gravity"])).unwrap();
            for (actual,key) in [(core.linear,"linear"),(core.angular,"angular"),(core.pending_linear,"pending_linear"),(core.pending_angular,"pending_angular")]{for (x,y) in actual.to_array().into_iter().zip(v(&a[key]).to_array()){assert_eq!(x.to_bits(),y.to_bits(),"{} step{} {key}",row["model"],row["step"]);}}
            assert_eq!((core.inverse_mass,core.inverse_inertia),properties);count+=1;
        }
        assert_eq!((count,idle,queued_idle),(187,15,13));
    }
    #[test]
    fn invalid_force_preparation_cannot_commit_pending_motion(){
        let mut c=crate::ivp_contact::Core{linear:Vec3::ONE,angular:Vec3::splat(2.),pending_linear:Vec3::splat(3.),pending_angular:Vec3::splat(4.),..Default::default()};
        let before=c;
        for (dt,gravity) in [(1./60.,Vec3::splat(f32::NAN)),(f32::MAX,Vec3::splat(f32::MAX)),(0.,Vec3::ZERO)]{
            assert!(prepare_forces(&mut c,dt,0.,Vec3::ZERO,1,gravity).is_err());assert_eq!(c.linear,before.linear);assert_eq!(c.angular,before.angular);assert_eq!(c.pending_linear,before.pending_linear);assert_eq!(c.pending_angular,before.pending_angular);
        }
    }
    #[test]
    fn owned_kernel_anisotropic_rates_and_branch_boundaries() {
        let fixture=fixture();let mut max_v=0f32;let mut max_w=0f32;
        for case in fixture["kernel_cases"].as_array().unwrap() {
            let mut velocity=v(&case["before_linear"]);let mut angular=v(&case["before_angular"]);
            apply(&mut velocity,&mut angular,case["dt"].as_f64().unwrap() as f32,
                case["linear_rate"].as_f64().unwrap() as f32,v(&case["angular_rate"]),case["movement_state"].as_u64().unwrap_or(1) as u8);
            max_v=max_v.max(velocity.distance(v(&case["after_linear"])));
            max_w=max_w.max(angular.distance(v(&case["after_angular"])));
        }
        println!("166 damping kernels: max linear{max_v} m/s, angular{max_w} rad/s");
        assert!(max_v<0.000002 && max_w<0.000002);
    }
    #[test]
    fn owned_free_spheres_match_damping_impulse_gravity_order() {
        let fixture=fixture();let mut max_v=0f32;let mut max_w=0f32;
        for case in fixture["cases"].as_array().unwrap() {
            let samples=case["samples"].as_array().unwrap();
            let mut velocity=v(&samples[0]["linear_source"]);let mut angular=v(&samples[0]["angular_core_ivp"]);
            let dt=case["dt"].as_f64().unwrap() as f32;
            for (i,sample) in samples.iter().enumerate().skip(1) {
                apply(&mut velocity,&mut angular,dt,case["core_damping"].as_f64().unwrap() as f32,
                    v(&case["core_angular_damping"]),case["controller_inputs"][i-1]["movement_state"].as_u64().unwrap() as u8);
                velocity.z+=case["gravity_source_z"].as_f64().unwrap() as f32*dt;
                max_v=max_v.max(velocity.distance(v(&sample["linear_source"])));
                max_w=max_w.max(angular.distance(v(&sample["angular_core_ivp"])));
            }
        }
        println!("38 native free profiles: max linear{max_v} Source-units/s, angular{max_w} rad/s");
        assert!(max_v<0.003 && max_w.to_degrees()<0.03);
    }
}
