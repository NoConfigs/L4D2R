//! Owned100ef730/1011d0a0 energy,1011d1b0 allowance and pending-state kernels.
//! Native helper uses round-to-nearest,53-bit x87 precision. World acceptance,
//! contact history and tangent friction remain separate caller responsibilities.
use bevy::prelude::*;

#[derive(Clone,Copy,Debug,Default)]
pub(crate) struct Core {
    pub flags:u16,pub mass:f32,pub inertia:Vec3,
    pub linear:Vec3,pub angular:Vec3,pub pending_linear:Vec3,pub pending_angular:Vec3,
}

/// The angular squares spill to float; linear squares retain53-bit precision.
/// Each preceding core's accumulated energy spills to float, while the last
/// return remains double. Reassociating either sum changes native results.
pub(crate) fn pending(cores:&[Core])->f64 {
    let mut stored=0f32;let mut value=0f64;
    for core in cores.iter().rev() {
        let v=core.linear+core.pending_linear;let w=core.angular+core.pending_angular;
        let linear=((v.y as f64*v.y as f64+v.x as f64*v.x as f64)+v.z as f64*v.z as f64)*core.mass as f64;
        let angular=(core.inertia.y as f64*(w.y*w.y) as f64+core.inertia.x as f64*(w.x*w.x) as f64)+core.inertia.z as f64*(w.z*w.z) as f64;
        value=(linear+angular)*0.5+stored as f64;stored=value as f32;
    }
    value
}

///1011d1b0 clamps finite mass between two identical caller-supplied bounds.
/// It therefore contributes the supplied bound, irrespective of finite mass;
/// fixed cores (flag2) contribute nothing. Preserve descending float additions.
pub(crate) fn allowance(cores:&[Core],dt:f32,bound:f32)->f32 {
    let mut result=0f32;
    for core in cores.iter().rev() {if core.flags&2==0 {result+=(bound*dt)*0.1f32;}}
    result
}

///1011d4a1 spills the pre-solve energy to float.1011d4ba adds that
/// rounded value to the float allowance in53-bit precision before comparing
/// the unspilled post-solve energy; equality commits pending impulses.
pub(crate) fn accepts(before:&[Core],after:&[Core],dt:f32,bound:f32)->bool {
    allowance(after,dt,bound) as f64+(pending(before) as f32) as f64>=pending(after)
}

///100f13b0 commits pending impulses;100ef8d0 only discards them.
pub(crate) fn finish(cores:&mut [Core],commit:bool) {
    for core in cores.iter_mut().rev() {
        if commit {core.angular+=core.pending_angular;core.linear+=core.pending_linear;}
        core.pending_angular=Vec3::ZERO;core.pending_linear=Vec3::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture()->serde_json::Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-energy.json")).unwrap()}
    fn v(a:&serde_json::Value)->Vec3 {Vec3::from_array(std::array::from_fn(|i|a[i].as_f64().unwrap() as f32))}
    fn cores(row:&serde_json::Value)->Vec<Core> {
        parse(&row["cores"])
    }
    fn parse(list:&serde_json::Value)->Vec<Core> {
        list.as_array().unwrap().iter().map(|c|Core {flags:c["flags"].as_u64().unwrap() as u16,mass:c["mass"].as_f64().unwrap() as f32,
            inertia:v(&c["inertia"]),linear:v(&c["linear"]),angular:v(&c["angular"]),pending_linear:v(&c["pending_linear"]),pending_angular:v(&c["pending_angular"])}).collect()
    }
    #[test]
    fn actual_native_normal_energy_gate_decisions_match() {
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-energy-gates.json")).unwrap();let mut decisions=0;let mut rejected=0;let mut empty=0;
        for row in data["cases"].as_array().unwrap() {
            let before=parse(&row["before"]);let after=parse(&row["after"]);
            let decision=accepts(&before,&after,row["dt"].as_f64().unwrap() as f32,row["allowance"].as_f64().unwrap() as f32);
            if after.is_empty() {empty+=1;continue;}
            let commits=row["commits"].as_u64().unwrap();let discards=row["discards"].as_u64().unwrap();assert!((commits>0)^(discards>0));
            assert_eq!(decision,commits>0,"{} step{} sequence{}",row["model"],row["step"],row["sequence"]);decisions+=1;rejected+=(discards>0) as usize;
        }
        assert_eq!(decisions,550);assert_eq!(empty,5);
        println!("{decisions} observed native energy decisions ({rejected} rejects);{empty} empty systems excluded; rejection coverage remains open");
    }
    #[test]
    fn native_queued_energy_matches_double_returns_and_float_spills() {
        let data=fixture();assert_eq!(data["x87_control"].as_u64().unwrap()&0xf00,0x200);let mut count=0;
        for row in data["cases"].as_array().unwrap() {
            let value=pending(&cores(row));assert_eq!(value,row["energy_f64"].as_f64().unwrap(),"variant{} n{}",row["variant"],cores(row).len());
            assert_eq!(value as f32,row["energy_f32"].as_f64().unwrap() as f32);count+=1;
        }
        assert_eq!(count,128);println!("{count} exact native pending-energy double/float returns");
    }
    #[test]
    fn native_allowance_uses_equal_bounds_and_skips_fixed_cores() {
        let data=fixture();let rows=data["cases"].as_array().unwrap();
        for row in rows {
            assert_eq!(allowance(&cores(row),row["dt"].as_f64().unwrap() as f32,row["allowance"].as_f64().unwrap() as f32),row["budget"].as_f64().unwrap() as f32);
        }
        for i in 0..64 {assert_eq!(rows[i]["budget"],rows[i+64]["budget"]);}
        println!("128 exact native allowances,64 mass-variation pairs");
    }
    #[test]
    fn native_commit_and_discard_preserve_the_correct_raw_velocity() {
        let data=fixture();let mut count=0;
        for row in data["cases"].as_array().unwrap() {for (commit,key) in [(true,"committed"),(false,"discarded")] {
            let mut actual=cores(row);finish(&mut actual,commit);
            for (core,expected) in actual.iter().zip(row[key].as_array().unwrap()) {
                assert_eq!(core.linear,v(&expected["linear"]));assert_eq!(core.angular,v(&expected["angular"]));
                assert_eq!(core.pending_linear,v(&expected["pending_linear"]));assert_eq!(core.pending_angular,v(&expected["pending_angular"]));count+=1;
            }
        }}
        assert_eq!(count,1088);println!("{count} exact native commit/discard core transitions");
    }
}
