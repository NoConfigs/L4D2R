//! Native 100ff7a0 / 100fe9b0 caller slip trimming and 100fda80 group limit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct State {pub mix:[f32;2],pub energy:f32,pub marker:u8}
#[derive(Clone, Copy)]
pub(crate) struct Coefficients {pub force:f32,pub friction:f32,pub inverse_response:f32}
impl Coefficients {fn product(self)->f32 {(self.force*self.friction)*self.inverse_response}}
pub(crate) fn single_limit(c:Coefficients,dt:f32)->f32 {(c.product()*dt)*dt}
/// Reverse list order is significant. Native accumulates the products with
/// SSE single precision, then returns the time-scaled value in x87 double.
pub(crate) fn group_limit(group:&[Coefficients],dt:f32)->f64 {
    let mut sum=0.;for c in group.iter().rev(){sum+=c.product();}
    (sum as f64*dt as f64)*dt as f64
}
pub(crate) fn trim(state:&mut State,limit:f32,friction:f32,force:f32,threshold_extra:f32)->bool {
    let [x,y]=state.mix;let length_squared=x*x+y*y;
    if length_squared<=limit*limit+threshold_extra {return false;}
    let inverse=1.0/(length_squared as f64).sqrt();
    let factor=(inverse as f32)*limit;
    state.mix=[x*factor,y*factor];state.marker=1;
    state.energy=(((((inverse*length_squared as f64)-limit as f64)*friction as f64)*force as f64)+state.energy as f64) as f32;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn fixture()->Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-slip.json")).unwrap()}
    fn f(v:&Value)->f32 {v.as_f64().unwrap() as f32}
    fn coeffs(v:&Value)->Vec<Coefficients> {v.as_array().unwrap().iter().map(|c|Coefficients {force:f(&c[0]),friction:f(&c[1]),inverse_response:f(&c[2])}).collect()}
    fn state(v:&Value)->State {State {mix:[f(&v["mix"][0]),f(&v["mix"][1])],energy:f(&v["energy"]),marker:v["marker"].as_u64().unwrap() as u8}}
    #[test]
    fn caller_limits_and_trimmed_mix_energy_flags_match_native() {
        let data=fixture();let mut counts=[0;3];
        for (i,c) in data["cases"].as_array().unwrap().iter().enumerate() {
            let group=coeffs(&c["group"]);let batch=c["batch"].as_bool().unwrap();let dt=f(&c["dt"]);
            let limit=if batch {group_limit(&group,dt) as f32}else{single_limit(group[0],dt)};
            assert_eq!(limit,f(&c["limit"]),"limit case{i}");
            let mut s=state(&c["before"]);let trimmed=trim(&mut s,limit,f(&c["friction"]),f(&c["force"]),f(&c["threshold_extra"]));
            assert_eq!(s,state(&c["after"]),"state case{i}");counts[0]+=1;counts[1]+=batch as usize;counts[2]+=trimmed as usize;
        }
        assert_eq!(counts,[1367,1231,508]);
    }
    #[test]
    fn aggregate_limits_match_unspilled_double_and_stored_single_controls() {
        let data=fixture();let mut count=0;
        for c in data["limit_controls"].as_array().unwrap() {
            let value=group_limit(&coeffs(&c["group"]),f(&c["dt"]));
            assert_eq!(value,c["return_value"].as_f64().unwrap(),"kind{}",c["kind"]);
            assert_eq!(value as f32,f(&c["stored_limit"]));count+=1;
        }
        assert_eq!(count,64);
    }
}
