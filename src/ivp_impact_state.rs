//! Transient100f04b0/100f0650 collision-frame staging over the canonical body.
//! This is not impact response or event dispatch. An unchanged contact restores
//! angular velocity and the full prediction endpoint, retaining event-time pose.
use crate::ivp_joint_apply::Body;
use crate::ivp_motion::ObjectFrame;
/// Single-use backup. No persistent second body bank or body ownership.
pub(crate) struct Saved{body:usize,angular:[f32;3],rotation_end:[f32;4]}
pub(crate) fn begin(body:&mut Body,at:f64)->Result<Option<Saved>,&'static str>{
    if body.flags>>8>=8||body.flags&0x10!=0{return Ok(None);}
    if !at.is_finite()||at<body.prediction.time{return Err("invalid impact staging time");}
    let saved=Saved{body:body.id,angular:body.angular[..3].try_into().unwrap(),rotation_end:body.prediction.rotation_end};
    let pose=body.prediction.sample(at,ObjectFrame{shift:None,rotation:None});
    if !pose.position.is_finite()||!pose.rotation.is_finite(){return Err("invalid impact staging pose");}
    let angular=if body.flags&8==0{Some(body.prediction.impact_angular())}else{None};
    if angular.is_some_and(|a|!a.is_finite()){return Err("invalid impact staging angular velocity");}
    body.prediction.trim_impact_rotation(at);body.current=pose;
    if let Some(a)=angular{body.angular[..3].copy_from_slice(&a.to_array());}
    Ok(Some(saved))
}
impl Saved{
    pub(crate) fn body_id(&self)->usize{self.body}
    pub(crate) fn restore(self,body:&mut Body)->Result<(),&'static str>{
        if body.id!=self.body{return Err("impact backup belongs to another body");}
        body.angular[..3].copy_from_slice(&self.angular);body.prediction.rotation_end=self.rotation_end;Ok(())
    }
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]
    fn skipped_cores_and_failed_stage_leave_the_bank_unchanged(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-stage.json")).unwrap();
        let original=crate::ivp_joint_apply::tests::body(&data["cases"][0]["before"]);
        for flags in [original.flags|0x10,(original.flags&0xff)|0x800]{
            let mut body=Body{flags,..original};let before=crate::ivp_joint_apply::tests::snapshot(body);
            assert!(begin(&mut body,original.clock).unwrap().is_none());assert_eq!(crate::ivp_joint_apply::tests::snapshot(body),before);
        }
        for at in [f64::NAN,f64::INFINITY,original.prediction.time-1.]{
            let mut body=original;assert!(begin(&mut body,at).is_err());assert_eq!(crate::ivp_joint_apply::tests::snapshot(body),crate::ivp_joint_apply::tests::snapshot(original));
        }
    }
    #[test]
    fn native_impact_staging_and_unchanged_restore_match(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-stage.json")).unwrap();
        for c in data["cases"].as_array().unwrap(){
            let mut body=crate::ivp_joint_apply::tests::body(&c["before"]);
            let saved=begin(&mut body,f64::from_bits(c["at"].as_u64().unwrap())).unwrap().unwrap();
            assert_eq!(crate::ivp_joint_apply::tests::snapshot(body),c["prepared"],"prepared {}",c["id"]);
            body.angular[3]=f32::from_bits(c["restore_input"]["angular"][3].as_u64().unwrap() as u32);
            saved.restore(&mut body).unwrap();
            assert_eq!(crate::ivp_joint_apply::tests::snapshot(body),c["restored"],"restored {}",c["id"]);
        }
        assert_eq!(data["cases"].as_array().unwrap().len(),64);
    }
}
