//! Actual reference lifetimes: numerical poses and metadata remain source-owned.
use crate::ivp_cache_pool::*;
use crate::ivp_motion::{ObjectFrame,Prediction,PreparedFrames,PreparedObjectCache};
use bevy::prelude::*;
use serde_json::Value;

pub(crate) fn controls(p:&Value)->(Prediction,ObjectFrame,f64,u32){
    let f=|v:&Value|v.as_f64().unwrap() as f32;
    let vec=|v:&Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));
    let quat=|v:&Value|std::array::from_fn(|i|f(&v[i]));
    let flags=p["flags"].as_u64().unwrap() as u32;
    (Prediction{start:vec(&p["start"]),velocity:vec(&p["velocity"]),rotation_start:quat(&p["rotation_start"]),rotation_end:quat(&p["rotation_end"]),time:p["time"].as_f64().unwrap(),inverse_dt:f(&p["inverse_dt"])},ObjectFrame{shift:(flags&0x800==0).then(||vec(&p["shift"])),rotation:p["extra"].as_bool().unwrap().then(||quat(&p["object_rotation"]))},p["now"].as_f64().unwrap(),p["frame"].as_u64().unwrap() as u32)
}
pub(crate) fn pose_crc(out:&PreparedObjectCache)->u64{
    let bytes:Vec<u8>=out.core_position.to_array().into_iter().chain(out.pose.position.to_array()).chain(out.quaternion).chain(out.pose.rotation.transpose().to_cols_array()).flat_map(|f|f.to_bits().to_le_bytes()).collect();crc32fast::hash(&bytes) as u64
}
#[derive(Clone,Copy)]
struct Pending {id:usize,slot:usize,target:usize,input:usize,old_stamp:u32,cause:u32,started:bool}
fn request(next:&mut usize,slot:usize,target:usize,input:usize,p:&Value,refs:u32,old_stamp:u32,cause:u32)->Pending{
    assert_eq!(refs,p["refs"].as_u64().unwrap() as u32);
    let pending=Pending{id:*next,slot,target,input,old_stamp,cause,started:false};*next+=1;pending
}
pub(crate) fn state_crc(slots:&[Slot],objects:&[Object],cursor:usize,frame:u32)->u64{
    let mut bytes=Vec::new();for word in [slots.len() as u32,cursor as u32,frame]{bytes.extend(word.to_le_bytes());}
    for slot in slots{
        bytes.extend(slot.references.to_le_bytes());
        bytes.extend(slot.frame_stamp.to_le_bytes());
        bytes.extend(slot.owner.map_or(-1,|v|v as i32).to_le_bytes());
    }
    for object in objects{bytes.extend(object.flags.to_le_bytes());bytes.extend(object.cache.map_or(-1,|v|v as i32).to_le_bytes());}
    crc32fast::hash(&bytes) as u64
}

#[test]
fn actual_reference_lifetimes_choose_all_retained_preparations_without_reseeding(){
    let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-cache-lifecycle.json")).unwrap();
    let (mut event_count,mut increments,mut decrements,mut prepared,mut allocated,mut unretained,mut retained)=(0,0,0,0,0,0,0);
    for profile in data["profiles"].as_array().unwrap(){
        let inputs=profile["inputs"].as_array().unwrap();let native_frames=profile["frames"].as_array().unwrap();
        let mut slots=vec![Slot{references:0,frame_stamp:0,owner:None};256];let mut objects=vec![Object{flags:0,cache:None};2];let mut cursor=0;
        let mut frames=PreparedFrames::default();let mut pending:Option<Pending>=None;let mut allocation=None;let mut next=0;
        for e in profile["events"].as_array().unwrap(){
            let id=e["id"].as_u64().unwrap() as usize;let kind=e["kind"].as_u64().unwrap();let target=e["target"].as_u64().unwrap() as usize;
            let input=e["input_id"].as_u64().unwrap() as usize;let p=&inputs[input];let frame=p["frame"].as_u64().unwrap() as u32;
            let rva=e["rva"].as_u64().unwrap() as u32;
            for i in 0..2{objects[i].flags=e["flags"][i].as_u64().unwrap() as u32;}
            match kind {
                0=>{
                    assert!(pending.is_none()&&allocation.is_none());assert_eq!(objects[target].cache,None);
                    assert_eq!(state_crc(&slots,&objects,cursor,frame),e["state_crc32"].as_u64().unwrap());
                    let result=allocate(&mut slots,&mut objects,&mut cursor,target).unwrap();
                    allocation=Some((id,target,result.slot));allocated+=1;
                    if result.prepare{pending=Some(request(&mut next,result.slot,target,input,p,slots[result.slot].references,0,0xe7250));}
                    event_count+=1;continue;
                }
                1=>{
                    assert!(pending.is_none());let (start,owner,slot)=allocation.take().unwrap();
                    assert_eq!(start,e["pair"].as_u64().unwrap() as usize);assert_eq!(owner,target);assert_eq!(slot,e["pool_slot"].as_u64().unwrap() as usize);
                }
                2=>{
                    assert!(pending.is_none()&&allocation.is_none());let slot=objects[target].cache.unwrap();
                    assert_eq!(slot,e["pool_slot"].as_u64().unwrap() as usize);assert_eq!(slots[slot].references,e["refs_before"].as_u64().unwrap() as u32);
                    match rva {
                        0x105734|0x1057b5|0x10eac4|0x10eb34|0x116bf9|0x116c69=>{
                            let selected=begin_retain(&mut slots,&objects,target,frame).unwrap();assert_eq!(selected.slot,slot);
                            if selected.prepare{pending=Some(request(&mut next,slot,target,input,p,slots[slot].references,slots[slot].frame_stamp,rva));retained+=1;}
                            increments+=1;
                        }
                        0x10597b|0x105981|0x10ebad|0x10ebb3|0x10ebf7|0x10ebfd|0x116d4c|0x116d52=>{release(&mut slots,slot).unwrap();decrements+=1;}
                        _=>panic!("Unverified native reference instruction {rva:x}"),
                    }
                    assert_eq!(slots[slot].references,e["refs_after"].as_u64().unwrap() as u32);
                }
                3=>{
                    let index=e["frame_event"].as_u64().unwrap() as usize;let native=&native_frames[index];
                    if pending.is_none(){
                        assert!(allocation.is_none());assert!(matches!(native["caller_rva"].as_u64().unwrap(),0x10cfbc|0x10d107));
                        let selected=begin_access(&slots,&objects,target,frame).unwrap();assert!(selected.prepare);
                        pending=Some(request(&mut next,selected.slot,target,input,p,slots[selected.slot].references,slots[selected.slot].frame_stamp,0x7bfa0));unretained+=1;
                    }
                    let q=pending.as_mut().expect("Native prepared a frame that source eligibility rejected");
                    assert!(!q.started);assert_eq!((q.id,q.target,q.input),(index,target,input));assert_eq!(q.slot,e["pool_slot"].as_u64().unwrap() as usize);q.started=true;
                    let caller=match q.cause{0xe7250=>0xe72c1,0x1057b5=>0x1057d1,0x10eac4=>0x10eae2,0x7bfa0=>native["caller_rva"].as_u64().unwrap() as u32,_=>panic!("Uncovered preparation caller")};
                    assert_eq!(caller,native["caller_rva"].as_u64().unwrap() as u32);
                    assert_eq!(slots[q.slot].frame_stamp,q.old_stamp);
                    assert_eq!(state_crc(&slots,&objects,cursor,frame),e["state_crc32"].as_u64().unwrap());
                    let (prediction,object,now,stamp)=controls(p);
                    let out=frames.prepare(q.slot as u32+1,q.target as u64+1,prediction,now,object,stamp).unwrap();
                    slots[q.slot].frame_stamp=out.frame_stamp;
                    let out=frames.get(q.slot as u32+1,q.target as u64+1,frame).unwrap();assert_eq!(pose_crc(out),native["pose_crc32"].as_u64().unwrap());prepared+=1;
                    event_count+=1;continue;
                }
                4=>{
                    let q=pending.take().unwrap();assert!(q.started);assert_eq!((q.id,q.target,q.input),(e["frame_event"].as_u64().unwrap() as usize,target,input));
                    let out=frames.get(q.slot as u32+1,q.target as u64+1,slots[q.slot].frame_stamp).unwrap();assert_eq!(pose_crc(out),native_frames[q.id]["pose_crc32"].as_u64().unwrap());
                }
                _=>panic!("Invalid lifecycle event"),
            }
            assert_eq!(state_crc(&slots,&objects,cursor,frame),e["state_crc32"].as_u64().unwrap(),"event{id} kind{kind} rva{rva:x}");event_count+=1;
        }
        assert!(allocation.is_none()&&pending.is_none());assert!(slots.iter().all(|s|s.references==0));assert_eq!(next,native_frames.len());
    }
    assert_eq!((event_count,increments,decrements,prepared,allocated,unretained,retained),(10476,4528,4528,686,24,48,614));
}
