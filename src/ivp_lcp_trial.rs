//!1011b8d0/1011bb80 constrained normal solve and active-set recovery.
//! A failed solve returns no impulses. World discovery, anomaly callbacks and
//! complete physical trajectories remain separate from this numerical kernel.
use super::{Factors,rebuild};
#[derive(Debug)]
struct NativeFailure {reason:&'static str,x:Vec<f32>}
type Result<T>=std::result::Result<T,NativeFailure>;
struct Settings {epsilon:f32,lu_epsilon:f32,error:f32,step_limit:f32,sentinel:f32}
struct State<'a> {
    a:&'a [f32],b:&'a [f32],n:usize,perm:Vec<usize>,x:Vec<f32>,w:Vec<f32>,
    dx:Vec<f32>,dw:Vec<f32>,factors:Factors,c:usize,m:usize,settings:Settings,
    factor_flag:usize,factor_count:usize,reorder_counters:[usize;4],
}
impl State<'_> {
    fn unsupported<T>(&self,reason:&'static str)->Result<T> {Err(NativeFailure{reason,x:self.x.clone()})}
    fn selected_matrix(&self)->Vec<f32> {(0..self.c).flat_map(|i|(0..self.c).map(move |j|self.a[self.perm[i]*self.n+self.perm[j]])).collect()}
    fn swap(&mut self,a:usize,b:usize) {self.perm.swap(a,b);}
    ///1011a6e0: deterministic swaps within active and unprocessed-tail groups.
    fn reorder(&mut self) {
        if self.c<=1 {return;}
        self.reorder_counters[0]+=1;self.reorder_counters[1]+=2;
        self.reorder_counters[0]%=self.c;self.reorder_counters[1]%=self.c;
        self.swap(self.reorder_counters[0],self.reorder_counters[1]);
        self.reorder_counters[2]+=1;self.reorder_counters[3]+=2;
        let tail=self.n.saturating_sub(self.m+1);
        if tail>1 {
            self.reorder_counters[2]%=tail;self.reorder_counters[3]%=tail;
            self.swap(self.m+1+self.reorder_counters[2],self.m+1+self.reorder_counters[3]);
        }
    }
    ///10119100: stable descending insertion order by current active impulse.
    fn sort_active(&mut self) {
        for end in 1..self.c {
            let mut i=end;
            while i>0 && self.x[self.perm[i]]>self.x[self.perm[i-1]] {self.swap(i,i-1);i-=1;}
        }
    }
    ///10119440: move a constraint to the unprocessed end, preserving order.
    fn move_to_end(&mut self,index:usize) {for i in index+1..self.n {self.swap(i-1,i);}}
    ///10119480: remove negative active impulses and negative inactive residuals.
    fn repair(&mut self)->usize {
        let mut removed=0;let mut i=0;
        while i<self.c {
            let index=self.perm[i];let value=self.x[index];
            if value<self.settings.epsilon {
                if value<=-self.settings.epsilon {self.move_to_end(i);removed+=1;self.m-=1;}
                else {self.x[index]=0.;self.swap(i,self.c-1);}
                self.c-=1;self.factor_count-=1;self.factor_flag=1;
            }else{i+=1;}
        }
        i=self.c;
        while i<self.m {
            if self.w[self.perm[i]]<0. {self.move_to_end(i);self.m-=1;}else{i+=1;}
        }
        removed
    }
    ///10119310/10118aa0: native grouped matrix-vector arithmetic.
    fn recompute(&mut self,temporary:Vec<f32>) {
        for row in 0..self.n {
            let mut sum=0f32;let mut col=0;
            while col+3<self.n {
                let a=&self.a[row*self.n+col..];let x=&temporary[col..];
                //10118b00..10118b39 adds the first three products to the
                //previous sum before the fourth. Flattened pseudocode changes
                //cancellation here and can choose a different active contact.
                sum=a[3]*x[3]+(((a[0]*x[0]+sum)+a[1]*x[1])+a[2]*x[2]);col+=4;
            }
            for column in col..self.n {sum+=self.a[row*self.n+column]*temporary[column];}
            self.w[row]=sum-self.b[row];
        }
        for i in 0..self.c {self.w[self.perm[i]]=0.;}
        self.x=temporary;
    }
    ///1011ba10: rebuild, Gaussian rescue, residual recompute and active repair.
    /// False is the native singular-system failure, retaining its partial state.
    fn recover(&mut self)->bool {
        loop {
            self.factor_count=self.c;let mut temporary=vec![0.;self.n];self.reorder();
            let rebuilt=rebuild(&self.selected_matrix(),self.c,self.settings.lu_epsilon).unwrap();
            self.factors=rebuilt.factors;
            let solution=if rebuilt.success {
                self.factor_flag=0;
                let rhs=(0..self.c).map(|i|self.b[self.perm[i]]).collect::<Vec<_>>();
                self.factors.solve(&rhs).unwrap()
            }else{
                self.factor_flag=2;self.sort_active();self.reorder();
                let mut system=crate::ivp_contact::NormalSystem {matrix:self.selected_matrix(),rhs:(0..self.c).map(|i|self.b[self.perm[i]]).collect()};
                let solved=crate::ivp_contact::gaussian(&mut system,self.settings.epsilon,1000.).unwrap();
                if !solved.success {return false;}solved.solution
            };
            for i in 0..self.c {temporary[self.perm[i]]=solution[i];}
            self.recompute(temporary);
            if self.repair()==0 {return true;}
        }
    }
    fn delete(&mut self,index:usize)->Result<()> {
        if self.factor_flag==0 {
            if !self.factors.remove(index).unwrap() {self.factor_flag=2;}
            self.factor_count=self.factors.size;
        }else{self.factor_count-=1;}Ok(())
    }
    fn insert(&mut self)->Result<()> {
        if self.factor_flag==0 {
            let source=self.selected_matrix();if !self.factors.insert(&source).unwrap() {self.factor_flag=2;}
            self.factor_count=self.factors.size;
        }else{self.factor_count+=1;}Ok(())
    }
    fn clean_active(&mut self)->Result<()> {
        let mut i=0;
        while i<self.c {
            let index=self.perm[i];
            if self.x[index]<self.settings.epsilon {
                self.x[index]=0.;self.c-=1;self.swap(i,self.c);self.delete(i)?;
            } else {i+=1;}
        }
        Ok(())
    }
    fn warm(&mut self)->Result<()> {
        let rebuilt=rebuild(&self.selected_matrix(),self.c,self.settings.lu_epsilon).unwrap();
        self.factors=rebuilt.factors;self.factor_count=self.c;
        if !rebuilt.success {self.c=0;self.m=0;self.factor_count=0;self.factors=rebuild(&[],0,self.settings.lu_epsilon).unwrap().factors;return Ok(());}
        self.factor_flag=0;
        let mut temporary=vec![0.;self.n];
        loop {
            let rhs=(0..self.c).map(|i|self.b[self.perm[i]]).collect::<Vec<_>>();
            let solution=self.factors.solve(&rhs).unwrap();
            for i in 0..self.c {temporary[self.perm[i]]=solution[i];}
            self.recompute(temporary.clone());
            let negative=(0..self.c).rev().find(|&i|self.x[self.perm[i]]<0.);
            let Some(i)=negative else {return Ok(());};let index=self.perm[i];self.x[index]=0.;temporary[index]=0.;
            self.c-=1;self.m-=1;self.swap(i,self.c);self.delete(i)?;
            if self.factor_flag>0 {self.x.fill(0.);self.w.fill(0.);self.c=0;self.m=0;self.factor_count=0;return Ok(());}
        }
    }
    fn residual_valid(&self)->bool {
        for row in 0..self.n {
            let mut sum=0f32;let mut end=self.n;
            while end>=4 {let first=end-4;let a=&self.a[row*self.n+first..];let x=&self.x[first..];sum=(((a[0]*x[0]+a[3]*x[3])+sum)+a[2]*x[2])+a[1]*x[1];end-=4;}
            for column in (0..end).rev() {sum+=self.a[row*self.n+column]*self.x[column];}
            let residual=sum-self.b[row];let position=self.perm.iter().position(|&i|i==row).unwrap();
            let difference=if position<self.c {residual}else{residual-self.w[row]};
            if difference.abs()>self.settings.error {return false;}
        }
        true
    }
    fn promote_current(&mut self,position:usize)->Result<()> {
        self.clean_active()?;let index=self.perm[position];self.w[index]=0.;if self.x[index]<0. {self.x[index]=0.;}
        self.swap(position,self.c);self.m+=1;self.c+=1;
        if self.m<self.n {self.insert()?;}Ok(())
    }
    fn run(&mut self)->Result<Vec<f32>> {
        self.warm()?;self.iterate()
    }
    fn iterate(&mut self)->Result<Vec<f32>> {
        let mut had_step=false;let mut iterations=0;let mut countdown=7;let mut near_steps=0;let mut recover_now=false;let mut enter_top=true;
        loop {
            if enter_top && had_step {iterations+=1;}if iterations>250 {return self.unsupported("native iteration limit");}
            if !recover_now {
                if countdown==0 {recover_now=!self.residual_valid();countdown=7;}else{countdown-=1;}
            }
            if recover_now {
                iterations+=1;if iterations>250 {return self.unsupported("native iteration limit");}
                near_steps=0;if !self.recover() {return self.unsupported("native recovery failed");}countdown=7;recover_now=false;
            }
            enter_top=true;
            if self.m>=self.n {return Ok(self.x.clone());}
            let current=self.perm[self.m];let epsilon=self.settings.epsilon;
            if self.factor_flag>0 {
                if self.factor_flag==1 && had_step {
                    self.reorder();let rebuilt=rebuild(&self.selected_matrix(),self.c,self.settings.lu_epsilon).unwrap();self.factor_count=self.c;self.factors=rebuilt.factors;
                    if rebuilt.success {self.factor_flag=0;}
                }else{self.factor_flag=1;}
            }
            if self.w[current].abs()<epsilon {
                if self.x[current].abs()>=epsilon {self.promote_current(self.m)?;if self.m>=self.n {countdown=0;}continue;}
            } else if self.w[current]<0. {
                self.dx.fill(0.);self.dw.fill(0.);
                if self.c>0 {
                    let rhs=(0..self.c).map(|i|-self.a[self.perm[i]*self.n+current]).collect::<Vec<_>>();
                    let direction=if self.factor_flag==0 {self.factors.solve(&rhs).unwrap()}else{
                        let mut system=crate::ivp_contact::NormalSystem {matrix:self.selected_matrix(),rhs};
                        let solved=crate::ivp_contact::gaussian(&mut system,epsilon,1000.).unwrap();
                        if !solved.success {recover_now=true;enter_top=false;continue;}solved.solution
                    };for i in 0..self.c {self.dx[self.perm[i]]=direction[i];}
                }
                self.dx[current]=1.;
                for pos in self.c..self.n {
                    let row=self.perm[pos];let mut sum=0f32;
                    for i in 0..self.c {let column=self.perm[i];sum+=self.dx[column]*self.a[row*self.n+column];}
                    self.dw[row]=self.a[row*self.n+current]+sum;
                }
                let mut blocker=None;
                let mut step=if self.settings.step_limit*self.dw[current]<=-self.w[current] {self.settings.sentinel}else{blocker=Some(self.m);-(self.w[current]/self.dw[current])};
                let mut early=false;
                for i in 0..self.c {
                    let index=self.perm[i];if self.dx[index]>=-epsilon {continue;}
                    let candidate=-(self.x[index]/self.dx[index]);
                    if candidate.abs()<epsilon && self.x[index]<epsilon {blocker=Some(i);step=candidate;early=true;break;}
                    if candidate<step+epsilon {blocker=Some(i);step=candidate;}
                }
                if !early {
                    for i in self.c..self.m {
                        let index=self.perm[i];if self.dw[index]>=-epsilon {continue;}
                        let candidate=-(self.w[index]/self.dw[index]);if candidate<step-epsilon {blocker=Some(i);step=candidate;}
                    }
                    if step<0. {step=0.;}
                }
                let Some(blocker)=blocker else {return self.unsupported("native unbounded step");};
                if step>self.settings.step_limit {recover_now=true;enter_top=false;continue;}
                if step>=epsilon {near_steps=0;}else{near_steps+=1;if near_steps>(self.n>>1)+2 {recover_now=true;enter_top=false;continue;}}
                for i in 0..self.n {self.w[i]=self.dw[i]*step+self.w[i];self.x[i]=self.dx[i]*step+self.x[i];}
                for i in self.c..self.m {let index=self.perm[i];if self.w[index]<0. {self.w[index]=0.;}}
                for i in 0..self.c {let index=self.perm[i];if self.x[index]<0. {self.x[index]=0.;}}
                had_step=true;
                if blocker<self.c {
                    self.x[self.perm[blocker]]=0.;self.c-=1;self.swap(blocker,self.c);self.delete(blocker)?;
                }else if blocker<self.m {
                    if step>epsilon {self.clean_active()?;}self.w[self.perm[blocker]]=0.;self.swap(blocker,self.c);self.c+=1;self.insert()?;
                }else{
                    self.promote_current(blocker)?;if self.m>=self.n {countdown=0;}
                }
                continue;
            }
            self.x[current]=0.;if self.w[current]<0. {self.w[current]=0.;}self.m+=1;had_step=false;if self.m>=self.n {countdown=0;}
        }
    }
}
/// Composition entry using the owned normal caller's solver defaults.
pub(crate) fn solve_owned_normal(a:&[f32],b:&[f32],warm:usize)->Option<Vec<f32>> {
    let n=b.len();if a.len()!=n*n || warm>n {return None;}
    let settings=Settings {epsilon:1e-7,lu_epsilon:1e-6,error:0.000100000005,step_limit:10000.,sentinel:1.00000002e20};
    let mut state=State {a,b,n,perm:(0..n).collect(),x:vec![0.;n],w:b.iter().map(|v|-v).collect(),dx:vec![0.;n],dw:vec![0.;n],
        factors:rebuild(&[],0,settings.lu_epsilon).unwrap().factors,c:warm,m:warm,settings,factor_flag:0,factor_count:warm,reorder_counters:[0;4]};
    state.run().ok()
}

#[test]
fn controlled_native_recovery_states_match() {
    let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-recovery.json")).unwrap();
    let mut count=0;let mut failed=0;
    for case in data["cases"].as_array().unwrap() {
        let before=&case["before"];let after=&case["after"];
        let floats=|key:&str|before[key].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>();
        let integers=|key:&str|before[key].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();
        let a=floats("matrix");let b=floats("rhs");let n=b.len();
        let settings=Settings {epsilon:1e-7,lu_epsilon:1e-6,error:0.000100000005,step_limit:10000.,sentinel:1.00000002e20};
        let mut state=State {a:&a,b:&b,n,perm:integers("permutation"),x:floats("x"),w:floats("w"),dx:vec![0.;n],dw:vec![0.;n],
            factors:rebuild(&[],0,settings.lu_epsilon).unwrap().factors,c:before["active"].as_u64().unwrap() as usize,m:before["processed"].as_u64().unwrap() as usize,
            settings,factor_flag:before["flag"].as_u64().unwrap() as usize,factor_count:before["factor_size"].as_u64().unwrap() as usize,reorder_counters:integers("counters").try_into().unwrap()};
        if case["operation"]=="resume" && state.factor_flag==0 {state.factors=rebuild(&state.selected_matrix(),state.c,state.settings.lu_epsilon).unwrap().factors;}
        let result=match case["operation"].as_str().unwrap() {"reorder"=>{state.reorder();1},"sort"=>{state.sort_active();1},"repair"=>state.repair(),"recover"=>state.recover() as usize,"full"=>state.run().is_ok() as usize,"resume"=>state.iterate().is_ok() as usize,_=>panic!("Unknown native operation")};
        assert_eq!(result,case["result"].as_u64().unwrap() as usize,"kind{} result",case["kind"]);
        if result==0 && case["operation"]=="recover" {failed+=1;}
        let ints=|key:&str|after[key].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();
        let fs=|key:&str|after[key].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>();
        assert_eq!(state.perm,ints("permutation"),"kind{} permutation",case["kind"]);
        assert_eq!(state.reorder_counters.as_slice(),ints("counters"),"kind{} counters",case["kind"]);
        for (key,value) in [("active",state.c),("processed",state.m),("flag",state.factor_flag),("factor_size",state.factor_count)] {assert_eq!(value,after[key].as_u64().unwrap() as usize,"kind{} {}",case["kind"],key);}
        assert_eq!(state.x,fs("x"),"kind{} impulses",case["kind"]);assert_eq!(state.w,fs("w"),"kind{} residuals",case["kind"]);count+=1;
    }
    assert_eq!(count,86);assert_eq!(failed,2);println!("{count} exact controlled native recovery states ({failed} native singular failures)");
}
#[test]
fn ordinary_native_constrained_fallback_solves() {
    let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-constrained-fallback.json")).unwrap();let mut passed=0;
    for case in data["cases"].as_array().unwrap() {
        let floats=|key:&str|case["before"][key].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>();
        let a=floats("matrix");let b=floats("rhs");let n=b.len();let warm=case["warm_start"].as_u64().unwrap() as usize;
        let constants=&case["constants"];let f=|key:&str|constants[key].as_f64().unwrap() as f32;
        let settings=Settings {epsilon:case["after"]["epsilon"].as_f64().unwrap() as f32,lu_epsilon:1e-6,error:f("active_error"),step_limit:f("active_step_limit"),sentinel:f("active_step_sentinel")};
        let mut state=State {a:&a,b:&b,n,perm:(0..n).collect(),x:vec![0.;n],w:b.iter().map(|v|-v).collect(),dx:vec![0.;n],dw:vec![0.;n],factors:rebuild(&[],0,settings.lu_epsilon).unwrap().factors,c:warm,m:warm,settings,factor_flag:0,factor_count:warm,reorder_counters:[0;4]};
        let result=state.run();
        assert!(result.is_ok(),"{} step{} {:?}",case["model"],case["step"],result);
        let expected=case["after"]["solution"].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>();
        assert_eq!(result.unwrap(),expected,"{} step{}",case["model"],case["step"]);passed+=1;
    }
    println!("{} exact ordinary native constrained fallback solves; world integration remains separate",passed);
}

#[test]
fn native_normal_pipeline_reconstructs_all_physical_pushes() {
    use crate::ivp_contact::{Core,Row,NormalSystem,assemble,normalize,gaussian,validate_fast,accumulate};
    use bevy::prelude::Vec3;
    let normal:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-normal-contacts.json")).unwrap();
    let selected:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-normal-solver.json")).unwrap();
    let fallback:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-constrained-fallback.json")).unwrap();
    let same=|a:&serde_json::Value,b:&serde_json::Value|a["model"]==b["model"] && a["step"]==b["step"] && a["floor_angles_source"].as_array().unwrap().iter().zip(b["floor_angles_source"].as_array().unwrap()).all(|(a,b)|a.as_f64()==b.as_f64());
    let vector=|a:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|a[i].as_f64().unwrap() as f32));
    let mut push_count=0;let mut fallback_count=0;
    for case in normal["systems"].as_array().unwrap() {
        let mut cores=vec![];let mut keys=vec![];let mut rows=vec![];let mut modes=vec![];
        for row in case["rows"].as_array().unwrap() {
            let mut bodies=[None,None];
            for side in 0..2 {
                let b=&row["bodies"][side];let key=b["key"].as_u64().unwrap();if key==0 {continue;}
                let index=keys.iter().position(|&k|k==key).unwrap_or_else(||{
                    let index=cores.len();keys.push(key);cores.push(Core {inverse_mass:b["inverse_mass"].as_f64().unwrap() as f32,inverse_inertia:vector(&b["inverse_inertia"]),
                        linear:vector(&b["linear"]),angular:vector(&b["angular"]),pending_linear:vector(&b["pending_linear"]),pending_angular:vector(&b["pending_angular"])});index});
                bodies[side]=Some(index);
            }
            rows.push(Row{bodies,normal:vector(&row["normal"]),jacobian:[vector(&row["jacobian0"]),vector(&row["jacobian1"])],gap:row["gap_field"].as_f64().unwrap() as f32});
            modes.push(row["mode"].as_u64().unwrap() as u32 as i32);
        }
        let native=selected["fast"].as_array().unwrap().iter().find(|v|same(case,v)).unwrap();let n=rows.len();
        let mut system=assemble(&cores,&rows,case["constants"]["gap_target"].as_f64().unwrap() as f32,case["constants"]["negative_gap_factor"].as_f64().unwrap() as f32).unwrap();
        let scale=normalize(&mut system,native["constants"]["normalization_minimum"].as_f64().unwrap() as f32).unwrap();
        //1011cc90 selects nonzero contact modes;1011d320 warms the leading
        //negative modes after native contact ordering. No solved values seed this pipeline.
        let indices=(0..n).filter(|&i|modes[i]!=0).collect::<Vec<_>>();let warm=modes.iter().take_while(|&&v|v<0).count();
        let mut sub=NormalSystem {matrix:indices.iter().flat_map(|&i|indices.iter().map(move |&j|(i,j))).map(|(i,j)|system.matrix[i*n+j]).collect(),rhs:indices.iter().map(|&i|system.rhs[i]).collect()};
        let candidate=gaussian(&mut sub,native["before"]["epsilon"].as_f64().unwrap() as f32,native["constants"]["singular_rhs_factor"].as_f64().unwrap() as f32).unwrap();assert!(candidate.success);
        let weights=native["weights"].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>();
        let validated=validate_fast(&system,scale,&indices,&candidate.solution,&weights,native["cutoff"].as_f64().unwrap() as f32).unwrap();
        assert_eq!(validated.accepted,native["accepted"].as_bool().unwrap());
        let solution=if validated.accepted {validated.impulses}else{
            fallback_count+=1;let reference=fallback["cases"].as_array().unwrap().iter().find(|v|same(case,v)).unwrap();
            let constants=&reference["constants"];let f=|key:&str|constants[key].as_f64().unwrap() as f32;
            let settings=Settings {epsilon:reference["after"]["epsilon"].as_f64().unwrap() as f32,lu_epsilon:1e-6,error:f("active_error"),step_limit:f("active_step_limit"),sentinel:f("active_step_sentinel")};
            let mut state=State {a:&system.matrix,b:&system.rhs,n,perm:(0..n).collect(),x:vec![0.;n],w:system.rhs.iter().map(|v|-v).collect(),dx:vec![0.;n],dw:vec![0.;n],factors:rebuild(&[],0,settings.lu_epsilon).unwrap().factors,c:warm,m:warm,settings,factor_flag:0,factor_count:warm,reorder_counters:[0;4]};
            state.run().unwrap()
        };
        let physical=solution.iter().map(|v|v*scale).collect::<Vec<_>>();
        let pushes=normal["pushes"].as_array().unwrap().iter().filter(|v|same(case,v)).collect::<Vec<_>>();let mut i=0;
        for (row,&impulse) in physical.iter().enumerate() {
            if impulse==0. {continue;}let expected=pushes[i];i+=1;
            assert_eq!(impulse,expected["impulse"].as_f64().unwrap() as f32,"{} step{} row{}",case["model"],case["step"],row);
            accumulate(&mut cores,rows[row],impulse);
            for body in expected["after"]["bodies"].as_array().unwrap() {
                let key=body["key"].as_u64().unwrap();if key==0 {continue;}let index=keys.iter().position(|&k|k==key).unwrap();
                assert_eq!(cores[index].pending_linear,vector(&body["pending_linear"]));assert_eq!(cores[index].pending_angular,vector(&body["pending_angular"]));
            }
            push_count+=1;
        }
        assert_eq!(i,pushes.len());
        // Matrix inputs are observed geometry/velocity state. This is not an
        //unseeded world/manifold test or a claim about post-solve energy acceptance.
    }
    assert_eq!(fallback_count,55);assert_eq!(push_count,937);
    println!("368 exact normal numerical pipelines,55 ordinary fallbacks,937 physical pushes; world integration remains open");
}

#[test]
fn late_native_fallback_controls_match_owned_matrices(){
    let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-late-fallback.json")).unwrap();
    for c in d["cases"].as_array().unwrap(){
        let floats=|key:&str|c["before"][key].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>();
        let actual=solve_owned_normal(&floats("matrix"),&floats("rhs"),c["warm_start"].as_u64().unwrap() as usize).unwrap();
        let expected=c["after"]["solution"].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect::<Vec<_>>();
        assert_eq!(actual,expected,"{} step{}",c["model"],c["step"]);
    }
}
