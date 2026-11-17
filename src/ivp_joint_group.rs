//!100af040/100af4d0 group scheduling. Individual constraint math is separate.
#[derive(Clone,Copy,Debug,PartialEq)]
pub(crate) struct Batch {
    pub prepare:bool,
    pub reverse:bool,
    pub p4:f32,
    pub p5:f32,
}
#[derive(Clone,Copy)]
pub(crate) struct Group{iterations:usize}
impl Group {
    /// Public additionalIterations adds to two native sweeps. The local
    /// coefficient table has a zero sentinel at iteration9, ending the loop.
    pub(crate) fn new(additional:u32)->Self{Self{iterations:additional.saturating_add(2) as usize}}
    pub(crate) fn batches(self)->impl Iterator<Item=Batch>{
        std::iter::once(Batch{prepare:true,reverse:false,p4:1.,p5:1.}).chain((0..self.iterations.min(9)).flat_map(|i|{
            let p4=match i{0|1=>1.,2=>0.8,3=>0.6,_=>0.4};let p5=if i<2{1.}else{0.8};
            [Batch{prepare:false,reverse:true,p4,p5},Batch{prepare:false,reverse:false,p4,p5}]
        }))
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn native_shared_joint_groups_match_setup_sweep_direction_and_iteration_factors(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-joint-group.json")).unwrap();
        for case in data["cases"].as_array().unwrap(){
            assert_eq!(Group::new(case["authored_iterations"].as_i64().unwrap().max(0) as u32).iterations as u64,case["native_iterations"].as_u64().unwrap());
        }
        let run=|data:&serde_json::Value|{
            let mut calls=vec![];
            for case in data["cases"].as_array().unwrap(){
                let additional=case["authored_iterations"].as_i64().unwrap().max(0) as u32;
                let order=case["order"].as_array().unwrap();
                for step in 0..3{for batch in Group::new(additional).batches(){for slot in 0..order.len(){
                    let index=if batch.reverse{order.len()-1-slot}else{slot};let joint=order[index].as_u64().unwrap() as usize;
                    calls.push(serde_json::json!({"scenario":case["scenario"],"step":step,"joint":joint,"phase":if batch.prepare{0}else{1},"type":case["types"][joint],"dt_bits":(1.0f32/60.).to_bits(),"p4":batch.p4.to_bits(),"p5":batch.p5.to_bits()}));
                }}}
            }calls
        };
        let actual=run(&data);assert_eq!(actual.len(),1764);assert_eq!(&actual,data["calls"].as_array().unwrap());
        let mut corrupted=data.clone();corrupted["calls"]=serde_json::Value::Null;
        for case in corrupted["cases"].as_array_mut().unwrap(){case["snapshots"]=serde_json::Value::Null;case["native_iterations"]=(-999).into();}
        assert_eq!(actual,run(&corrupted));
    }
}
