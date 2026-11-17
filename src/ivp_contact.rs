//! Owned1011cc90/1011cb80 normal-contact assembly and1011c160 pending pushes.
//! This is a numerical component. Contact topology, constrained solve, tangent
//! friction, acceptance/energy rollback and world integration remain separate.
use bevy::prelude::*;

#[derive(Clone,Copy,Debug,Default)]
pub(crate) struct Core {
    pub inverse_mass:f32, pub inverse_inertia:Vec3,
    pub linear:Vec3, pub angular:Vec3,
    pub pending_linear:Vec3, pub pending_angular:Vec3,
}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Row {
    pub bodies:[Option<usize>;2], pub jacobian:[Vec3;2], pub normal:Vec3,
    /// Native contact+64, supplied by the continuing-manifold update.
    pub gap:f32,
}
pub(crate) struct NormalSystem {pub matrix:Vec<f32>,pub rhs:Vec<f32>}

/// Build the full pair-coupled matrix, including rows sharing either body.
/// Native velocities here exclude pending impulses. Preserve its Y/X/Z sums
/// and the distinct accumulation order for the two body sides. SSE groups
/// the linear and angular triples separately; decompiler expressions flatten
/// those groups and reassociate the negative-side X impulse product.
pub(crate) fn assemble(cores:&[Core],rows:&[Row],target:f32,negative_factor:f32)->Option<NormalSystem> {
    if !target.is_finite() || !negative_factor.is_finite() {return None;}
    for row in rows {
        if !row.normal.is_finite() || !row.gap.is_finite() || !row.jacobian.iter().all(|v|v.is_finite())
            || row.bodies.iter().flatten().any(|&i|i>=cores.len())
            || row.bodies[0].is_some() && row.bodies[0]==row.bodies[1] {return None;}
    }
    let n=rows.len();let mut matrix=vec![0.;n.checked_mul(n)?];let mut rhs=vec![0.;n];
    for (i,row) in rows.iter().enumerate() {
        let mut speed=0.;
        if let Some(index)=row.bodies[0] {
            let b=cores[index];let j=row.jacobian[0];let v=b.linear;let w=b.angular;let normal=row.normal;
            speed=((normal.y*v.y+normal.x*v.x)+normal.z*v.z)+((j.y*w.y+j.x*w.x)+j.z*w.z);
        }
        if let Some(index)=row.bodies[1] {
            let b=cores[index];let j=row.jacobian[1];let v=b.linear;let w=b.angular;let normal=row.normal;
            speed-=((j.y*w.y+w.x*j.x)+j.z*w.z)+((normal.y*v.y+normal.x*v.x)+normal.z*v.z);
        }
        let gap=target-row.gap;let factor=if gap<0. {negative_factor}else{1.};
        rhs[i]=factor*gap+speed;
        for side in 0..2 {
            let Some(index)=row.bodies[side] else {continue;};let b=cores[index];
            let sign=if side==0 {-1.}else{1.};
            let angular=row.jacobian[side]*b.inverse_inertia*sign;
            let linear=row.normal*(b.inverse_mass*sign);
            for (j,other) in rows.iter().enumerate() {
                let other_side=if other.bodies[0]==Some(index) {0}else if other.bodies[1]==Some(index) {1}else{continue;};
                let jac=other.jacobian[other_side];let normal=other.normal;
                let delta=((jac.y*angular.y+jac.x*angular.x)+jac.z*angular.z)+((normal.y*linear.y+normal.x*linear.x)+normal.z*linear.z);
                matrix[j*n+i]+=delta*if other_side==0 {-1.}else{1.};
            }
        }
    }
    Some(NormalSystem{matrix,rhs})
}
///1011c3c0 rescales matrix diagonals and RHS independently; the returned
/// factor converts the solved impulse back to physical units.
pub(crate) fn normalize(system:&mut NormalSystem,minimum:f32)->Option<f32> {
    let n=dimension(system)?;if !minimum.is_finite() || minimum<0. {return None;}
    let max_diagonal=(0..n).map(|i|system.matrix[i*n+i]).fold(0f32,f32::max);
    let matrix_scale=if max_diagonal>minimum {1./max_diagonal}else{1.};
    let max_rhs=system.rhs.iter().map(|v|v.abs()).fold(0f32,f32::max);
    let (rhs_scale,magnitude)=if max_rhs>minimum {(1./max_rhs,max_rhs)}else{(1.,1.)};
    let scale=matrix_scale*magnitude;
    for value in &mut system.matrix {*value*=matrix_scale;}
    for value in &mut system.rhs {*value*=rhs_scale;}
    Some(scale)
}
fn dimension(system:&NormalSystem)->Option<usize> {
    let n=system.rhs.len();
    (n.checked_mul(n)?==system.matrix.len() && system.matrix.iter().chain(&system.rhs).all(|v|v.is_finite())).then_some(n)
}
pub(crate) struct Gaussian {pub success:bool,pub solution:Vec<f32>}
///1011a400/1011a590/10118730. Pivot ties scan from the last row upward.
/// SIMD elimination starts at the aligned group containing the pivot, including
/// earlier columns; back substitution retains native descending order.
pub(crate) fn gaussian(system:&mut NormalSystem,epsilon:f32,singular_rhs_factor:f32)->Option<Gaussian> {
    let n=dimension(system)?;
    if !epsilon.is_finite() || epsilon<=0. || !singular_rhs_factor.is_finite() || singular_rhs_factor<0. {return None;}
    for pivot in 0..n {
        let mut selected=pivot;let mut largest=system.matrix[pivot*n+pivot].abs();
        for row in (pivot+1..n).rev() {
            let value=system.matrix[row*n+pivot].abs();if value>largest {largest=value;selected=row;}
        }
        if selected!=pivot {
            for column in 0..n {system.matrix.swap(pivot*n+column,selected*n+column);}
            system.rhs.swap(pivot,selected);
        }
        let diagonal=system.matrix[pivot*n+pivot];
        if diagonal.abs()<epsilon {continue;}
        let negative_inverse=-1./diagonal;
        for row in pivot+1..n {
            let entry=system.matrix[row*n+pivot];if entry.abs()<=epsilon {continue;}
            let factor=entry*negative_inverse;
            for column in (pivot&!3)..n {
                system.matrix[row*n+column]=system.matrix[pivot*n+column]*factor+system.matrix[row*n+column];
            }
            system.rhs[row]=system.rhs[pivot]*factor+system.rhs[row];
        }
    }
    for row in (0..n).rev() {
        let mut value=system.rhs[row];
        for column in (row+1..n).rev() {value-=system.rhs[column]*system.matrix[row*n+column];}
        let diagonal=system.matrix[row*n+row];
        if diagonal.abs()>=epsilon {system.rhs[row]=value/diagonal;}
        else if epsilon as f64*singular_rhs_factor as f64<=value.abs() as f64 {
            return Some(Gaussian{success:false,solution:vec![0.;n]});
        } else {system.rhs[row]=0.;}
    }
    Some(Gaussian{success:true,solution:system.rhs.clone()})
}
pub(crate) struct FastCandidate {pub accepted:bool,pub impulses:Vec<f32>}
///1011c910 and101185a0 validate a selected Gaussian solution. Rejected
/// candidates are retained for observation; they must go through the native
/// constrained fallback rather than being applied to bodies.
pub(crate) fn validate_fast(system:&NormalSystem,scale:f32,indices:&[usize],candidate:&[f32],weights:&[f32],cutoff:f32)->Option<FastCandidate> {
    let n=dimension(system)?;
    if indices.len()!=candidate.len() || weights.len()!=n || indices.iter().any(|&i|i>=n)
        || ![scale,cutoff].iter().chain(candidate).chain(weights).all(|v|v.is_finite()) {return None;}
    let mut impulses=vec![0.;n];let mut selected=vec![false;n];let mut accepted=true;
    for (&row,&value) in indices.iter().zip(candidate) {
        if selected[row] {return None;}selected[row]=true;impulses[row]=value;
        if (scale*value)*weights[row]<cutoff {accepted=false;}
    }
    for row in 0..n {
        if selected[row] {continue;}
        let mut sum=0f32;
        for column in 0..n {sum+=system.matrix[row*n+column]*impulses[column];}
        let rhs=system.rhs[row];
        // Native uses x87 for the final relative-residual comparison.
        if (0.00001f32 as f64*rhs as f64).abs()+(sum as f64)<rhs as f64 {return Some(FastCandidate{accepted:false,impulses});}
    }
    Some(FastCandidate{accepted,impulses})
}
/// Accumulate the native normal impulse; anomaly limiting100f0c50 and the
/// energy acceptance/rollback surrounding this kernel are caller obligations.
pub(crate) fn accumulate(cores:&mut [Core],row:Row,impulse:f32) {
    for side in 0..2 {
        let Some(index)=row.bodies[side] else {continue;};let b=&mut cores[index];let j=row.jacobian[side];let inv=b.inverse_inertia;
        if side==0 {
            let negative=-impulse;
            b.pending_angular.x=negative*(j.x*inv.x)+b.pending_angular.x;
            b.pending_angular.y=(j.y*inv.y)*negative+b.pending_angular.y;
            b.pending_angular.z=(j.z*inv.z)*negative+b.pending_angular.z;
            let scale=-(b.inverse_mass*impulse);
            b.pending_linear+=row.normal*scale;
        } else {
            b.pending_angular.x=(j.x*inv.x)*impulse+b.pending_angular.x;
            b.pending_angular.y=(j.y*inv.y)*impulse+b.pending_angular.y;
            b.pending_angular.z=(j.z*inv.z)*impulse+b.pending_angular.z;
            b.pending_linear+=row.normal*(b.inverse_mass*impulse);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture()->serde_json::Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-normal-contacts.json")).unwrap()}
    fn v(a:&serde_json::Value)->Vec3 {Vec3::from_array(std::array::from_fn(|i|a[i].as_f64().unwrap() as f32))}
    fn parse(rows:&[serde_json::Value])->(Vec<Core>,Vec<Row>) {
        let mut cores=vec![];let mut keys=vec![];let mut out=vec![];
        for row in rows {
            let mut bodies=[None,None];
            for side in 0..2 {
                let b=&row["bodies"][side];let key=b["key"].as_u64().unwrap();if key==0 {continue;}
                let index=keys.iter().position(|&k|k==key).unwrap_or_else(||{
                    let index=cores.len();keys.push(key);cores.push(Core {inverse_mass:b["inverse_mass"].as_f64().unwrap() as f32,
                        inverse_inertia:v(&b["inverse_inertia"]),linear:v(&b["linear"]),angular:v(&b["angular"]),pending_linear:v(&b["pending_linear"]),pending_angular:v(&b["pending_angular"])});index});
                bodies[side]=Some(index);
            }
            out.push(Row{bodies,normal:v(&row["normal"]),jacobian:[v(&row["jacobian0"]),v(&row["jacobian1"])],gap:row["gap_field"].as_f64().unwrap() as f32});
        }
        (cores,out)
    }
    fn solver_fixture()->serde_json::Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-normal-solver.json")).unwrap()}
    fn system(value:&serde_json::Value)->NormalSystem {
        let floats=|key:&str|value[key].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect();
        NormalSystem{matrix:floats("matrix"),rhs:floats("rhs")}
    }
    fn assert_system(actual:&NormalSystem,expected:&serde_json::Value) {
        let expected=system(expected);assert_eq!(actual.matrix,expected.matrix);assert_eq!(actual.rhs,expected.rhs);
    }
    #[test]
    fn native_normal_system_scaling_matches() {
        let data=solver_fixture();
        for c in data["normalizations"].as_array().unwrap() {
            let mut actual=system(&c["before"]);
            let scale=normalize(&mut actual,c["constants"]["normalization_minimum"].as_f64().unwrap() as f32).unwrap();
            assert_system(&actual,&c["after"]);assert_eq!(scale,c["after"]["scale"].as_f64().unwrap() as f32);
        }
        println!("{} exact native normalizations",data["normalizations"].as_array().unwrap().len());
    }
    #[test]
    fn native_gaussian_pivots_and_singular_cases_match() {
        let data=solver_fixture();let mut rejected=0;
        for c in data["gaussian"].as_array().unwrap() {
            let mut actual=system(&c["before"]);
            let result=gaussian(&mut actual,c["before"]["epsilon"].as_f64().unwrap() as f32,c["constants"]["singular_rhs_factor"].as_f64().unwrap() as f32).unwrap();
            assert_eq!(result.success,c["accepted"].as_bool().unwrap(),"{} step{}",c["model"],c["step"]);
            if !result.success {rejected+=1;}
            assert_system(&actual,&c["after"]);
            assert_eq!(result.solution,c["after"]["solution"].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>());
        }
        assert_eq!(rejected,2);println!("{} exact native Gaussian cases ({} rejected)",data["gaussian"].as_array().unwrap().len(),rejected);
    }
    #[test]
    fn native_selected_contact_pipeline_matches_before_fallback() {
        let data=solver_fixture();let norms=data["normalizations"].as_array().unwrap();let mut count=0;
        for c in data["fast"].as_array().unwrap() {
            let original=norms.iter().find(|v|v["model"]==c["model"] && v["floor_angles_source"]==c["floor_angles_source"] && v["step"]==c["step"]).unwrap();
            let mut full=system(&original["before"]);let n=full.rhs.len();
            let scale=normalize(&mut full,c["constants"]["normalization_minimum"].as_f64().unwrap() as f32).unwrap();
            let indices=c["indices"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();
            let mut selected=NormalSystem {matrix:indices.iter().flat_map(|&i|indices.iter().map(move |&j|(i,j))).map(|(i,j)|full.matrix[i*n+j]).collect(),rhs:indices.iter().map(|&i|full.rhs[i]).collect()};
            let solved=gaussian(&mut selected,c["before"]["epsilon"].as_f64().unwrap() as f32,c["constants"]["singular_rhs_factor"].as_f64().unwrap() as f32).unwrap();
            assert!(solved.success);assert_eq!(solved.solution,c["candidate"].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>());
            let weights=c["weights"].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>();
            let result=validate_fast(&full,scale,&indices,&solved.solution,&weights,c["cutoff"].as_f64().unwrap() as f32).unwrap();
            assert_eq!(result.accepted,c["accepted"].as_bool().unwrap());
            assert_eq!(result.impulses,c["after"]["solution"].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>());count+=1;
        }
        println!("{} exact selected-contact pipelines; native constrained fallback remains separate",count);
    }
    #[test]
    fn native_normal_fast_path_acceptance_matches() {
        let data=solver_fixture();let mut accepted=0;
        for c in data["fast"].as_array().unwrap() {
            let indices=c["indices"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();
            let floats=|key:&str|c[key].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>();
            let result=validate_fast(&system(&c["before"]),c["before"]["scale"].as_f64().unwrap() as f32,&indices,&floats("candidate"),&floats("weights"),c["cutoff"].as_f64().unwrap() as f32).unwrap();
            assert_eq!(result.accepted,c["accepted"].as_bool().unwrap(),"{} step{}",c["model"],c["step"]);
            if result.accepted {accepted+=1;}
            assert_eq!(result.impulses,c["after"]["solution"].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>());
        }
        assert_eq!(accepted,313);println!("{} native acceptance decisions ({} accepted)",data["fast"].as_array().unwrap().len(),accepted);
    }
    #[test]
    fn native_continuing_contact_matrices_and_biases_match() {
        let data=fixture();let mut max_matrix=0f32;let mut max_rhs=0f32;
        for c in data["systems"].as_array().unwrap() {
            let (cores,rows)=parse(c["rows"].as_array().unwrap());
            let system=assemble(&cores,&rows,c["constants"]["gap_target"].as_f64().unwrap() as f32,c["constants"]["negative_gap_factor"].as_f64().unwrap() as f32).unwrap();
            for (key,actual) in [("matrix",system.matrix),("rhs",system.rhs)] {
                let expected=c[key].as_array().unwrap();assert_eq!(actual.len(),expected.len());
                for (a,e) in actual.iter().zip(expected) {
                    let e=e.as_f64().unwrap() as f32;let error=(a-e).abs();let relative=error/e.abs().max(1.);
                    if key=="matrix" {max_matrix=max_matrix.max(relative);}else{max_rhs=max_rhs.max(relative);}
                    assert_eq!(*a,e,"{} step{} {}",c["model"],c["step"],key);
                }
            }
        }
        println!("{} native normal systems: max scaled errors matrix{} rhs{}",data["systems"].as_array().unwrap().len(),max_matrix,max_rhs);
    }
    #[test]
    fn native_normal_impulses_accumulate_before_commit() {
        let data=fixture();let mut max=0f32;
        for c in data["pushes"].as_array().unwrap() {
            let (mut cores,rows)=parse(std::slice::from_ref(&c["before"]));
            accumulate(&mut cores,rows[0],c["impulse"].as_f64().unwrap() as f32);
            let (expected,_)=parse(std::slice::from_ref(&c["after"]));
            for (a,e) in cores.iter().zip(expected) {
                for (actual,expected) in [(a.pending_linear,e.pending_linear),(a.pending_angular,e.pending_angular)] {
                    let error=(actual-expected).abs().max_element();max=max.max(error);
                    assert_eq!(actual,expected,"{} step{}",c["model"],c["step"]);
                }
                assert_eq!(a.linear,e.linear);assert_eq!(a.angular,e.angular);
            }
        }
        println!("{} native normal pushes: max{}",data["pushes"].as_array().unwrap().len(),max);
    }
}
