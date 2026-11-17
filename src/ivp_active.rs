//! Owned1011a7b0 factor rebuild and1011a380 solve used by the active-set fallback.
//! Active-set iteration, recovery/state control and world integration
//! remain separate. Do not replace those updates with repeated Gaussian solves.
#[derive(Clone,Debug)]
pub(crate) struct Factors {
    pub size:usize, pub epsilon:f32, pub left:Vec<f32>, pub upper:Vec<f32>,
}
pub(crate) struct Rebuilt {pub factors:Factors,pub success:bool}

///1011a7b0 copies the selected matrix, starts the left transform at identity,
/// pivots using10119e60 and normalizes pivots using10119d40. SIMD operations
/// include the aligned group containing the current column, not only the
/// strictly upper portion. Preserve partial factors on a rejected pivot.
pub(crate) fn rebuild(source:&[f32],size:usize,epsilon:f32)->Option<Rebuilt> {
    if size.checked_mul(size)?!=source.len() || !epsilon.is_finite() || epsilon<=0. || !source.iter().all(|v|v.is_finite()) {return None;}
    let mut factors=Factors {size,epsilon,left:vec![0.;source.len()],upper:source.to_vec()};
    for row in 0..size {factors.left[row*size+row]=1.;}
    for pivot in 0..size {
        if pivot+1<size {
            let mut selected=pivot;let mut largest=factors.upper[pivot*size+pivot].abs();
            for row in (pivot+1..size).rev() {
                let value=factors.upper[row*size+pivot].abs();if value>largest {largest=value;selected=row;}
            }
            if selected!=pivot {
                for column in (pivot&!3)..size {factors.upper.swap(pivot*size+column,selected*size+column);}
                for column in 0..size {factors.left.swap(pivot*size+column,selected*size+column);}
            }
        }
        let diagonal=factors.upper[pivot*size+pivot];
        if diagonal.abs()<epsilon {return Some(Rebuilt{factors,success:false});}
        let inverse=1./diagonal;
        for column in 0..size {factors.left[pivot*size+column]*=inverse;}
        let aligned=(pivot+1)&!3;
        for column in aligned..size {factors.upper[pivot*size+column]*=inverse;}
        factors.upper[pivot*size+pivot]=1.;
        for row in (pivot+1..size).rev() {
            let entry=factors.upper[row*size+pivot];if entry==0. {continue;}
            let negative=-entry;
            for column in aligned..size {factors.upper[row*size+column]=factors.upper[pivot*size+column]*negative+factors.upper[row*size+column];}
            for column in 0..size {factors.left[row*size+column]=factors.left[pivot*size+column]*negative+factors.left[row*size+column];}
            factors.upper[row*size+pivot]=0.;
        }
    }
    Some(Rebuilt{factors,success:true})
}
impl Factors {
    fn valid(&self)->bool {
        self.epsilon.is_finite() && self.epsilon>0. && self.size.checked_mul(self.size)==Some(self.left.len())
            && self.upper.len()==self.left.len() && self.left.iter().chain(&self.upper).all(|v|v.is_finite())
    }
    ///10119d40 normalizes both transforms, retaining the aligned upper group.
    fn normalize_upper_row(&mut self,row:usize)->bool {
        let n=self.size;let diagonal=self.upper[row*n+row];
        if diagonal.abs()<self.epsilon {return false;}
        let inverse=1./diagonal;
        for column in 0..n {self.left[row*n+column]*=inverse;}
        for column in ((row+1)&!3)..n {self.upper[row*n+column]*=inverse;}
        self.upper[row*n+row]=1.;true
    }
    ///1011b260 adds the newly active last row/column without rebuilding old
    /// factors. The caller supplies the matrix in the current active order.
    /// A rejected last pivot leaves the expanded partial factors intact.
    pub(crate) fn insert(&mut self,source:&[f32])->Option<bool> {
        if !self.valid() {return None;}
        let old=self.size;let n=old.checked_add(1)?;
        if n.checked_mul(n)?!=source.len() || !source.iter().all(|v|v.is_finite()) {return None;}
        let mut left=vec![0.;source.len()];let mut upper=vec![0.;source.len()];
        for row in 0..old {
            for column in 0..old {left[row*n+column]=self.left[row*old+column];upper[row*n+column]=self.upper[row*old+column];}
            let mut sum=0f32;
            for column in (0..old).rev() {sum+=self.left[row*old+column]*source[column*n+old];}
            upper[row*n+old]=sum;
        }
        left[old*n+old]=1.;
        for column in 0..n {upper[old*n+column]=source[old*n+column];}
        self.size=n;self.left=left;self.upper=upper;
        for pivot in 0..old {
            let negative=-self.upper[old*n+pivot];
            for column in ((pivot+1)&!3)..n {self.upper[old*n+column]=self.upper[pivot*n+column]*negative+self.upper[old*n+column];}
            for column in 0..n {self.left[old*n+column]=self.left[pivot*n+column]*negative+self.left[old*n+column];}
            self.upper[old*n+pivot]=0.;
        }
        Some(self.normalize_upper_row(old))
    }
    ///1011ae00 removes an active row by swapping its column with the last,
    /// restoring triangular form, then eliminating the last transform column.
    /// Return/partial-state behavior includes the native pivot rescue path.
    pub(crate) fn remove(&mut self,index:usize)->Option<bool> {
        if !self.valid() || index>=self.size {return None;}
        let n=self.size;let last=n-1;
        for row in 0..n {self.left.swap(row*n+index,row*n+last);self.upper.swap(row*n+index,row*n+last);}
        for row in (index+1..last).rev() {
            let entry=self.upper[row*n+index];
            if entry!=0. {
                let negative=-entry;
                for column in 0..n {self.left[row*n+column]=self.left[last*n+column]*negative+self.left[row*n+column];}
            }
            self.upper[row*n+index]=0.;
        }
        if !self.normalize_upper_row(index) {
            for column in 0..n {self.left[index*n+column]=self.left[last*n+column]+self.left[index*n+column];}
            self.upper[index*n+index]=1.;
        }
        for pivot in index..last {
            let entry=self.upper[last*n+pivot];if entry==0. {continue;}
            let negative=-entry;
            for column in ((pivot+1)&!3)..n {self.upper[last*n+column]=self.upper[pivot*n+column]*negative+self.upper[last*n+column];}
            for column in 0..n {self.left[last*n+column]=self.left[pivot*n+column]*negative+self.left[last*n+column];}
            self.upper[last*n+pivot]=0.;
        }
        let diagonal=self.left[last*n+last];let success=diagonal.abs()>=self.epsilon;
        if success {
            let inverse=1./diagonal;for column in 0..n {self.left[last*n+column]*=inverse;}self.left[last*n+last]=1.;
            for row in (0..last).rev() {
                let entry=self.left[row*n+last];if entry==0. {continue;}let negative=-entry;
                for column in 0..n {self.left[row*n+column]=self.left[last*n+column]*negative+self.left[row*n+column];}
                self.left[row*n+last]=0.;
            }
        }
        let mut left=vec![0.;last*last];let mut upper=vec![0.;last*last];
        for row in 0..last {for column in 0..last {left[row*last+column]=self.left[row*n+column];upper[row*last+column]=self.upper[row*n+column];}}
        self.size=last;self.left=left;self.upper=upper;Some(success)
    }
    ///1011a180 applies the maintained left transform, then1011a270 performs
    /// descending back substitution against the unit-diagonal upper matrix.
    /// Native accumulates each dot product before one final subtraction.
    pub(crate) fn solve(&self,rhs:&[f32])->Option<Vec<f32>> {
        let n=self.size;
        if rhs.len()!=n || self.left.len()!=n.checked_mul(n)? || self.upper.len()!=self.left.len()
            || !rhs.iter().chain(&self.left).chain(&self.upper).all(|v|v.is_finite()) {return None;}
        let copied=rhs.to_vec();let mut output=vec![0.;n];
        for row in (0..n).rev() {
            let mut sum=0f32;for column in (0..n).rev() {sum+=self.left[row*n+column]*copied[column];}output[row]=sum;
        }
        for row in (0..n).rev() {
            let mut sum=0f32;for column in (row+1..n).rev() {sum+=self.upper[row*n+column]*output[column];}output[row]-=sum;
        }
        Some(output)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture()->serde_json::Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-active-factor.json")).unwrap()}
    fn floats(case:&serde_json::Value,key:&str)->Vec<f32> {case[key].as_array().unwrap().iter().map(|v|v.as_f64().unwrap() as f32).collect()}
    fn updates()->serde_json::Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-factor-updates.json")).unwrap()}
    fn factors(value:&serde_json::Value)->Factors {Factors {size:value["size"].as_u64().unwrap() as usize,epsilon:value["epsilon"].as_f64().unwrap() as f32,left:floats(value,"left"),upper:floats(value,"upper")}}
    fn assert_factors(actual:&Factors,expected:&serde_json::Value) {
        assert_eq!(actual.size,expected["size"].as_u64().unwrap() as usize);assert_eq!(actual.left,floats(expected,"left"));assert_eq!(actual.upper,floats(expected,"upper"));
    }
    #[test]
    fn native_incremental_insertions_match_including_rejected_pivot() {
        let data=updates();let mut failed=0;
        for c in data["insertions"].as_array().unwrap() {
            let mut actual=factors(&c["before"]);let success=actual.insert(&floats(c,"source")).unwrap();
            assert_eq!(success,c["flag_after"]==0,"{} step{}",c["model"],c["step"]);
            assert_factors(&actual,&c["after"]);if !success {failed+=1;}
        }
        assert_eq!(failed,1);println!("{} exact native insertions ({} rejected)",data["insertions"].as_array().unwrap().len(),failed);
    }
    #[test]
    fn native_incremental_removals_match_including_transform_failure() {
        let data=updates();let mut failed=0;
        for c in data["removals"].as_array().unwrap() {
            let mut actual=factors(&c["before"]);let success=actual.remove(c["index"].as_u64().unwrap() as usize).unwrap();
            assert_eq!(success,c["success"].as_bool().unwrap(),"{} step{}",c["model"],c["step"]);
            assert_factors(&actual,&c["after"]);if !success {failed+=1;}
        }
        assert_eq!(failed,1);println!("{} exact native removals ({} rejected)",data["removals"].as_array().unwrap().len(),failed);
    }
    #[test]
    fn native_controlled_update_chains_reuse_the_previous_factors() {
        let data=updates();let mut events=vec![];
        for c in data["insertions"].as_array().unwrap() {if c["step"].as_i64().unwrap()<0 {events.push((c,true));}}
        for c in data["removals"].as_array().unwrap() {if c["step"].as_i64().unwrap()<0 {events.push((c,false));}}
        events.sort_by_key(|(c,_)|c["sequence"].as_u64().unwrap());
        let mut state=None;let mut group=None;let mut reused=0;
        for (c,insert) in events {
            let step=c["step"].as_i64().unwrap();
            if group!=Some(step) {state=Some(factors(&c["before"]));group=Some(step);}else{reused+=1;}
            let actual=state.as_mut().unwrap();assert_factors(actual,&c["before"]);
            if insert {assert_eq!(actual.insert(&floats(c,"source")).unwrap(),c["flag_after"]==0);}
            else {assert_eq!(actual.remove(c["index"].as_u64().unwrap() as usize).unwrap(),c["success"].as_bool().unwrap());}
            assert_factors(actual,&c["after"]);
        }
        assert!(reused>=10);println!("{} chained native updates reuse computed factors",reused);
    }
    #[test]
    fn native_active_matrix_rebuilds_match_including_partial_failures() {
        let data=fixture();let mut failed=0;
        for c in data["rebuilds"].as_array().unwrap() {
            let result=rebuild(&floats(c,"source"),c["size"].as_u64().unwrap() as usize,c["epsilon"].as_f64().unwrap() as f32).unwrap();
            assert_eq!(result.success,c["success"].as_bool().unwrap(),"{} step{}",c["model"],c["step"]);
            assert_eq!(result.factors.left,floats(c,"left"),"{} step{} left",c["model"],c["step"]);
            assert_eq!(result.factors.upper,floats(c,"upper"),"{} step{} upper",c["model"],c["step"]);
            if !result.success {failed+=1;}
        }
        assert_eq!(failed,4);println!("{} exact native active rebuilds ({} rejected)",data["rebuilds"].as_array().unwrap().len(),failed);
    }
    #[test]
    fn native_maintained_factor_solves_match() {
        let data=fixture();
        for c in data["solves"].as_array().unwrap() {
            let factors=Factors {size:c["size"].as_u64().unwrap() as usize,epsilon:c["epsilon"].as_f64().unwrap() as f32,left:floats(c,"left"),upper:floats(c,"upper")};
            let rhs=floats(c,"rhs");let output=factors.solve(&rhs).unwrap();
            assert_eq!(output,floats(c,"output"),"{} step{}",c["model"],c["step"]);
            assert_eq!(output,floats(c,"intermediate"));assert_eq!(rhs,floats(c,"copied_rhs"));
        }
        println!("{} exact native maintained-factor solves",data["solves"].as_array().unwrap().len());
    }
    #[test]
    fn native_rebuild_and_solve_compose_without_substituting_gaussian() {
        let data=fixture();let mut matched=0;
        for c in data["rebuilds"].as_array().unwrap() {
            if c["success"]!=true {continue;}
            let partner=data["solves"].as_array().unwrap().iter().find(|s|s["capture"]==c["capture"] && s["model"]==c["model"] && s["step"]==c["step"] && s["left"]==c["left"] && s["upper"]==c["upper"] && s["rhs"]==c["rhs"]);
            let Some(s)=partner else {continue;};
            let result=rebuild(&floats(c,"source"),c["size"].as_u64().unwrap() as usize,c["epsilon"].as_f64().unwrap() as f32).unwrap();
            assert!(result.success);assert_eq!(result.factors.solve(&floats(c,"rhs")).unwrap(),floats(s,"output"));matched+=1;
        }
        assert!(matched>=6);println!("{} exact native rebuild/solve compositions",matched);
    }
}

#[path="ivp_lcp_trial.rs"]
pub(crate) mod lcp_trial;
