//! Owned1011c6c0 persistent normal-force/sign history. This is a numerical
//! component; contact creation/lifetime/order and tangent friction are separate.
#[derive(Clone,Copy,Debug)]
pub(crate) struct History {pub force:f32,pub counter:i16}

/// Return the impulse emitted to1011c160, if any. Negative impulses are still
/// emitted, but never retained as positive force for the next step. Native
/// signed-short arithmetic wraps; skipped rows clear force without changing
/// their sign counter. Zero candidates reset the counter and emit no impulse.
pub(crate) fn apply(history:&mut History,impulse:f32,skip:bool,dt:f32,inverse_dt:f32)->Option<f32> {
    let mut value=impulse;let mut emitted=None;
    if skip {value=0.;}
    else if value<0. {
        if history.counter<0 {history.counter=0;}
        history.counter=history.counter.wrapping_add(1);
        if history.counter>9 {history.counter=0;}
        emitted=Some(value);
    }else if value>0. {
        history.counter=if history.counter<0 {history.counter.wrapping_sub(1)}else{-1};
        emitted=Some(value);
    }else{
        history.counter=0;value=(history.force*0.)*dt;
    }
    if value<0. {value=0.;}
    history.force=inverse_dt*value;emitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use crate::ivp_contact::{Core,Row,accumulate};
    fn fixture()->serde_json::Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-history.json")).unwrap()}
    fn f(v:&serde_json::Value)->f32 {v.as_f64().unwrap() as f32}
    fn v(a:&serde_json::Value)->Vec3 {Vec3::from_array(std::array::from_fn(|i|f(&a[i])))}
    fn history(link:&serde_json::Value)->History {History {force:f(&link["force"]),counter:link["counter"].as_i64().unwrap() as i16}}
    fn core(c:&serde_json::Value)->Core {Core {inverse_mass:f(&c["inverse_mass"]),inverse_inertia:v(&c["inverse_inertia"]),linear:v(&c["linear"]),angular:v(&c["angular"]),pending_linear:v(&c["pending_linear"]),pending_angular:v(&c["pending_angular"])}}
    fn clocks(case:&serde_json::Value)->(f32,f32) {(f(&case["dt"]),f(&case["inverse_dt"]))}
    fn emissions(case:&serde_json::Value)->Vec<(usize,f32)> {case["pushes"].as_array().unwrap().iter().map(|p|(p["row"].as_u64().unwrap() as usize,f(&p["impulse"]))).collect()}
    #[test]
    fn source_normal_solve_history_energy_and_commit_compose() {
        use crate::ivp_contact::{assemble,normalize,gaussian,validate_fast,NormalSystem};
        use crate::ivp_energy::{self,Core as EnergyCore};
        let data=fixture();
        let lifetime:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-lifetime.json")).unwrap();
        let mut groups=0;let mut fallbacks=0;let mut pushes=0;let mut empty=0;
        for case in data["cases"].as_array().unwrap().iter().filter(|c|c["step"].as_i64().unwrap()>=0) {
            let before=lifetime["cases"].as_array().unwrap().iter().find(|c|c["capture"]==case["capture"]&&c["model"]==case["model"]&&c["operation"]=="sort"&&c["sequence"].as_u64()==Some(case["sequence"].as_u64().unwrap()*2)).unwrap();
            assert_eq!(before["step"],case["step"]);let (mut managed,settings)=crate::ivp_lifetime::reference_before(before);
            assert_eq!(managed.prepare(settings),crate::ivp_lifetime::Path::Batch);
            let links=case["before"].as_array().unwrap();let n=links.len();assert_eq!(managed.contacts.len(),n);if n==0 {assert!(emissions(case).is_empty());empty+=1;continue;}
            let mut keys=vec![];let mut cores=vec![];let mut energies=vec![];let mut rows=vec![];let mut modes=vec![];
            for (position,link) in links.iter().enumerate() {
                let context=&managed.contacts[position];
                assert_eq!(context.counter as i32,link["mode"].as_u64().unwrap() as u32 as i32);assert_eq!(context.distance,f(&link["gap"]));
                let bodies=std::array::from_fn(|side|{
                    let c=&link["bodies"][side];let key=c["key"].as_u64().unwrap();if key==0 {return None;}
                    Some(keys.iter().position(|&k|k==key).unwrap_or_else(||{let i=keys.len();keys.push(key);cores.push(core(c));energies.push(EnergyCore {flags:c["flags"].as_u64().unwrap() as u16,mass:f(&c["mass"]),inertia:v(&c["inertia"]),
                        linear:v(&c["linear"]),angular:v(&c["angular"]),pending_linear:v(&c["pending_linear"]),pending_angular:v(&c["pending_angular"])});i}))
                });
                rows.push(Row {bodies,jacobian:std::array::from_fn(|side|v(&link["jacobian"][side])),normal:v(&link["normal"]),gap:context.distance});
                modes.push(context.counter as i32);
            }
            // These references contain one active core against a static floor.
            assert_eq!(cores.len(),1);let energy_before=energies.clone();
            let mut system=assemble(&cores,&rows,0.0126949195,20.).unwrap();let scale=normalize(&mut system,1e-10).unwrap();
            let indices=(0..n).filter(|&i|modes[i]!=0).collect::<Vec<_>>();let warm=modes.iter().take_while(|&&mode|mode<0).count();
            let mut selected=NormalSystem {matrix:indices.iter().flat_map(|&i|indices.iter().map(move |&j|(i,j))).map(|(i,j)|system.matrix[i*n+j]).collect(),rhs:indices.iter().map(|&i|system.rhs[i]).collect()};
            let candidate=gaussian(&mut selected,f(&case["epsilon"]),1000.).unwrap();assert!(candidate.success);
            let weights=links.iter().map(|l|f(&l["weight"])).collect::<Vec<_>>();
            let fast=validate_fast(&system,scale,&indices,&candidate.solution,&weights,f(&case["cutoff"])).unwrap();
            let solution=if fast.accepted {fast.impulses}else {fallbacks+=1;crate::ivp_active::lcp_trial::solve_owned_normal(&system.matrix,&system.rhs,warm).unwrap()};
            let mut emitted=vec![];let (dt,inverse)=clocks(case);
            for i in 0..n {
                let impulse=solution[i]*scale;assert_eq!(impulse,f(&links[i]["impulse"]),"{} step{} solved row{i}",case["model"],case["step"]);
                let mut h=history(&links[i]);if let Some(value)=apply(&mut h,impulse,i<case["skip"].as_u64().unwrap() as usize,dt,inverse) {accumulate(&mut cores,rows[i],value);emitted.push((i,value));}
                assert_eq!(h.counter,history(&case["after"][i]).counter);assert_eq!(h.force,history(&case["after"][i]).force);
            }
            assert_eq!(emitted,emissions(case));pushes+=emitted.len();
            for i in 0..cores.len() {energies[i].pending_linear=cores[i].pending_linear;energies[i].pending_angular=cores[i].pending_angular;}
            assert!(ivp_energy::accepts(&energy_before,&energies,dt,f(&case["allowance"])));
            assert!(case["finalized"].as_bool().unwrap());ivp_energy::finish(&mut energies,true);
            for link in case["committed"].as_array().unwrap() {for c in link["bodies"].as_array().unwrap() {let key=c["key"].as_u64().unwrap();if key==0 {continue;}
                let actual=energies[keys.iter().position(|&k|k==key).unwrap()];assert_eq!(actual.linear,v(&c["linear"]),"{} step{} committed linear",case["model"],case["step"]);assert_eq!(actual.angular,v(&c["angular"]));
                assert_eq!(actual.pending_linear,v(&c["pending_linear"]));assert_eq!(actual.pending_angular,v(&c["pending_angular"]));
            }}groups+=1;
        }
        assert_eq!(groups,365);assert_eq!(fallbacks,55);assert_eq!(pushes,937);assert_eq!(empty,3);
        println!("{groups} exact source normal solve/history/energy/commit compositions;{fallbacks} fallbacks,{pushes} pushes; observed geometry remains an input");
    }
    #[test]
    fn native_normal_history_and_emitted_impulses_match() {
        let data=fixture();let mut rows=0;let mut pushes=0;
        for case in data["cases"].as_array().unwrap() {
            let (dt,inverse)=clocks(case);let mut actual=vec![];
            for (i,(before,after)) in case["before"].as_array().unwrap().iter().zip(case["after"].as_array().unwrap()).enumerate() {
                let mut h=history(before);
                if let Some(impulse)=apply(&mut h,f(&before["impulse"]),i<case["skip"].as_u64().unwrap() as usize,dt,inverse) {actual.push((i,impulse));}
                assert_eq!(h.counter,history(after).counter,"{} step{} row{i}",case["model"],case["step"]);assert_eq!(h.force,history(after).force);rows+=1;
            }
            assert_eq!(actual,emissions(case));pushes+=actual.len();
        }
        assert_eq!(rows,1307);assert_eq!(pushes,1024);println!("{rows} exact native history rows and{pushes} emitted impulses");
    }
    #[test]
    fn native_history_and_pending_impulses_compose_before_energy_acceptance() {
        let data=fixture();let mut groups=0;
        for case in data["cases"].as_array().unwrap() {
            let (dt,inverse)=clocks(case);let mut cores=[Core::default();2];
            for link in case["before"].as_array().unwrap() {for c in link["bodies"].as_array().unwrap() {let key=c["key"].as_u64().unwrap() as usize;if key>0 {cores[key-1]=core(c);}}}
            for (i,link) in case["before"].as_array().unwrap().iter().enumerate() {
                let mut h=history(link);let Some(impulse)=apply(&mut h,f(&link["impulse"]),i<case["skip"].as_u64().unwrap() as usize,dt,inverse) else {continue;};
                let row=Row {bodies:std::array::from_fn(|side|{let key=link["bodies"][side]["key"].as_u64().unwrap() as usize;key.checked_sub(1)}),
                    jacobian:std::array::from_fn(|side|v(&link["jacobian"][side])),normal:v(&link["normal"]),gap:f(&link["gap"])};
                accumulate(&mut cores,row,impulse);
            }
            for link in case["after"].as_array().unwrap() {for c in link["bodies"].as_array().unwrap() {let key=c["key"].as_u64().unwrap() as usize;if key==0 {continue;}
                let actual=cores[key-1];let expected=core(c);assert_eq!(actual.linear,expected.linear);assert_eq!(actual.angular,expected.angular);
                assert_eq!(actual.pending_linear,expected.pending_linear,"{} step{}",case["model"],case["step"]);assert_eq!(actual.pending_angular,expected.pending_angular,"{} step{}",case["model"],case["step"]);
            }}groups+=1;
        }
        assert_eq!(groups,416);println!("{groups} exact history/pending-impulse group compositions");
    }
    #[test]
    fn repeated_controlled_histories_reuse_computed_state() {
        let data=fixture();let mut reuses=0;
        for step in [-31,-32] {
            let cases=data["cases"].as_array().unwrap().iter().filter(|c|c["step"].as_i64()==Some(step)).collect::<Vec<_>>();assert!(!cases.is_empty());
            let mut state=cases[0]["before"].as_array().unwrap().iter().map(history).collect::<Vec<_>>();
            for (iteration,case) in cases.iter().enumerate() {
                let (dt,inverse)=clocks(case);
                for (i,h) in state.iter_mut().enumerate() {apply(h,f(&case["before"][i]["impulse"]),i<case["skip"].as_u64().unwrap() as usize,dt,inverse);let expected=history(&case["after"][i]);assert_eq!(h.counter,expected.counter);assert_eq!(h.force,expected.force);}
                reuses+=(iteration>0) as usize;
            }
        }
        assert_eq!(reuses,16);println!("{reuses} chained native history updates reuse computed state");
    }
}
