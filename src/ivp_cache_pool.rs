//! Owned100e7250 cache selection/rebinding. Source component, not world integration.
//! IDs represent stable object/slot identity; numeric payloads remain caller-owned.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Slot {
    pub references: u32,
    pub frame_stamp: u32,
    pub owner: Option<usize>,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Object {
    pub flags: u32,
    pub cache: Option<usize>,
}
#[derive(Debug, PartialEq)]
pub(crate) enum AllocationError { InvalidPool, InvalidIdentity, Exhausted }
#[derive(Debug, PartialEq)]
pub(crate) struct Allocation {
    pub slot: usize,
    /// Invoke100e6fd0-equivalent preparation after rebinding, before use.
    pub prepare: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) struct FrameAccess {pub slot:usize,pub prepare:bool}

/// Begin an already-bound lookup. Preparation is a subsequent operation: the
/// cached stamp must stay unchanged until its numeric frame has been computed.
pub(crate) fn begin_access(
    slots:&[Slot],objects:&[Object],object:usize,frame:u32,
) -> Result<FrameAccess,AllocationError>{
    let object=objects.get(object).ok_or(AllocationError::InvalidIdentity)?;
    let slot=object.cache.ok_or(AllocationError::InvalidIdentity)?;
    let cached=slots.get(slot).ok_or(AllocationError::InvalidIdentity)?;
    Ok(FrameAccess{slot,prepare:(object.flags as u8 as i8)<8&&(cached.frame_stamp as i32)<(frame as i32)})
}

/// Native post-retain checkpoint, before any stale-frame preparation. Keeping
/// this phase explicit lets the world provider preserve reference lifetimes.
pub(crate) fn begin_retain(
    slots:&mut [Slot],objects:&[Object],object:usize,frame:u32,
) -> Result<FrameAccess,AllocationError>{
    let access=begin_access(slots,objects,object,frame)?;
    slots[access.slot].references=slots[access.slot].references.wrapping_add(1);
    Ok(access)
}

pub(crate) fn allocate(
    slots: &mut [Slot], objects: &mut [Object], cursor: &mut usize, object: usize,
) -> Result<Allocation, AllocationError> {
    if !slots.len().is_power_of_two() || *cursor >= slots.len() {
        return Err(AllocationError::InvalidPool);
    }
    if object >= objects.len()
        || slots.iter().any(|s|s.owner.is_some_and(|i|i>=objects.len()))
        || objects.iter().any(|o|o.cache.is_some_and(|i|i>=slots.len())) {
        return Err(AllocationError::InvalidIdentity);
    }
    let mask=slots.len()-1;
    let Some(slot)=(0..slots.len()).map(|i|(*cursor+i)&mask)
        .find(|&i|slots[i].references==0) else {
        // Native spins forever here. Explicit failure prevents a source deadlock;
        // no observed successful allocation is replaced with a fallback slot.
        return Err(AllocationError::Exhausted);
    };
    *cursor=(slot+1)&mask;
    if let Some(previous)=slots[slot].owner {
        if let Some(previous_slot)=objects[previous].cache {
            // Native follows the previous owner's pointer, even if stale and
            // pointing to a different slot; do not assume reciprocal links.
            slots[previous_slot].owner=None;
            objects[previous].cache=None;
        }
    }
    slots[slot].owner=Some(object);
    objects[object].cache=Some(slot);
    slots[slot].frame_stamp=0;
    Ok(Allocation {slot,prepare:(objects[object].flags as u8 as i8)>7})
}

/// Shared retained lookup in10105650,1010ea30 and10116b70. Allocation preparation
/// precedes the retain; dynamic stale-frame preparation follows it.
pub(crate) fn retain(
    slots:&mut [Slot], objects:&mut [Object], cursor:&mut usize,
    object:usize, frame:u32, mut prepare:impl FnMut(usize,u32,u32),
) -> Result<usize,AllocationError> {
    let binding=objects.get(object).ok_or(AllocationError::InvalidIdentity)?.cache;
    let slot=if let Some(slot)=binding {
        if slot>=slots.len(){return Err(AllocationError::InvalidIdentity);}
        slot
    }else{
        let allocation=allocate(slots,objects,cursor,object)?;
        if allocation.prepare {
            prepare(allocation.slot,slots[allocation.slot].references,slots[allocation.slot].frame_stamp);
            slots[allocation.slot].frame_stamp=frame;
        }
        allocation.slot
    };
    let access=begin_retain(slots,objects,object,frame)?;
    debug_assert_eq!(slot,access.slot);
    if access.prepare {
        prepare(slot,slots[slot].references,slots[slot].frame_stamp);
        slots[slot].frame_stamp=frame;
    }
    Ok(slot)
}

/// Existing contact-refresh entry point uses the same retained lookup policy.
pub(crate) fn retain_for_refresh(
    slots:&mut [Slot],objects:&mut [Object],cursor:&mut usize,
    object:usize,frame:u32,prepare:impl FnMut(usize,u32,u32),
) -> Result<usize,AllocationError>{retain(slots,objects,cursor,object,frame,prepare)}

/// Owned1007bfa0 access. Unlike contact refresh, this borrows the cached frame
/// without incrementing its reference count. Signed byte/frame comparisons are
/// intentional: these are the native eligibility gates, including wraparound.
pub(crate) fn access(
    slots:&mut [Slot], objects:&mut [Object], cursor:&mut usize,
    object:usize, frame:u32, mut prepare:impl FnMut(usize,u32,u32),
) -> Result<usize,AllocationError> {
    let binding=objects.get(object).ok_or(AllocationError::InvalidIdentity)?.cache;
    let slot=if let Some(slot)=binding {
        if slot>=slots.len(){return Err(AllocationError::InvalidIdentity);}
        slot
    }else{
        let allocation=allocate(slots,objects,cursor,object)?;
        if allocation.prepare {
            prepare(allocation.slot,slots[allocation.slot].references,slots[allocation.slot].frame_stamp);
            slots[allocation.slot].frame_stamp=frame;
        }
        allocation.slot
    };
    let access=begin_access(slots,objects,object,frame)?;
    debug_assert_eq!(slot,access.slot);
    if access.prepare {
        prepare(slot,slots[slot].references,slots[slot].frame_stamp);
        slots[slot].frame_stamp=frame;
    }
    Ok(slot)
}

/// Native geometry completion releases the left cache, then the right cache.
pub(crate) fn release(slots:&mut [Slot],slot:usize)->Result<(),AllocationError>{
    let slot=slots.get_mut(slot).ok_or(AllocationError::InvalidIdentity)?;
    slot.references=slot.references.wrapping_sub(1);
    Ok(())
}
/// Owned100e6f20 clears the two binding pointers, leaving references, frame
/// stamps and numeric payloads untouched, even when a slot has another owner.
pub(crate) fn detach(slots:&mut[Slot],objects:&mut[Object],object:usize)->Result<(),AllocationError>{
 let entry=objects.get(object).ok_or(AllocationError::InvalidIdentity)?;
 if let Some(slot)=entry.cache{slots.get_mut(slot).ok_or(AllocationError::InvalidIdentity)?.owner=None;objects[object].cache=None;}
 Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ivp_motion::{Prediction,ObjectFrame};
    use bevy::prelude::*;
    use serde_json::Value;
    fn index(v:&Value)->Option<usize>{let i=v.as_i64().unwrap();if i<0 {assert_eq!(i,-1);None}else{Some(i as usize)}}
    fn slots(v:&Value)->Vec<Slot>{v.as_array().unwrap().iter().map(|s|Slot{references:s["refs"].as_u64().unwrap() as u32,frame_stamp:s["frame"].as_u64().unwrap() as u32,owner:index(&s["owner"])}).collect()}
    fn objects(v:&Value)->Vec<Object>{v.as_array().unwrap().iter().map(|o|Object{flags:o["flags"].as_u64().unwrap() as u32,cache:index(&o["cache"])}).collect()}

    #[test]
    fn nonretaining_access_matches_native_frames_and_repeated_reuse(){
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-cache-access.json")).unwrap();
        let mut preparations=0;let mut unchanged=0;
        for row in fixture["cases"].as_array().unwrap(){
            let p=&row["prediction"];let f=|v:&Value|v.as_f64().unwrap() as f32;
            let vec=|v:&Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));
            let quat=|v:&Value|std::array::from_fn(|i|f(&v[i]));
            let prediction=Prediction{start:vec(&p["start"]),velocity:vec(&p["velocity"]),rotation_start:quat(&p["rotation_start"]),rotation_end:quat(&p["rotation_end"]),time:p["time"].as_f64().unwrap(),inverse_dt:f(&p["inverse_dt"])};
            let flags=p["flags"].as_u64().unwrap() as u32;
            let frame=p["frame"].as_u64().unwrap() as u32;
            let refs=p["refs"].as_u64().unwrap() as u32;
            let bound=row["bound"].as_bool().unwrap();
            let mut slots=vec![Slot{references:refs,frame_stamp:row["old_frame"].as_u64().unwrap() as u32,owner:Some(0)}];
            let mut objects=vec![Object{flags,cache:bound.then_some(0)}];let mut cursor=0;
            // Initial helper payload is known caller storage, not a native pose.
            let mut values:Vec<f32>=[0,1,2,20,21,22,4,5,6,7,8,9,10,12,13,14,16,17,18].into_iter().map(|i|(i+1) as f32).collect();
            let mut calls=0;
            assert_eq!(access(&mut slots,&mut objects,&mut cursor,0,frame,|slot,r,_|{
                assert_eq!(slot,0);assert_eq!(r,refs);calls+=1;
                let object=ObjectFrame{shift:(flags&0x800==0).then(||vec(&p["shift"])),rotation:p["extra"].as_bool().unwrap().then(||quat(&p["object_rotation"]))};
                let out=prediction.prepare_object_cache(p["now"].as_f64().unwrap(),object,frame);
                values=out.core_position.to_array().into_iter().chain(out.pose.position.to_array()).chain(out.quaternion).chain(out.pose.rotation.transpose().to_cols_array()).collect();
            }),Ok(0));
            assert_eq!(calls,usize::from(row["prepared"].as_bool().unwrap()));
            preparations+=calls;unchanged+=usize::from(calls==0);
            assert_eq!(values,row["values"].as_array().unwrap().iter().map(f).collect::<Vec<_>>(),"case {}",row["kind"]);
            assert_eq!(slots[0].references,row["after_refs"].as_u64().unwrap() as u32);
            assert_eq!(slots[0].frame_stamp,row["after_frame"].as_u64().unwrap() as u32);
            let before=(slots.clone(),objects.clone(),cursor);
            assert_eq!(access(&mut slots,&mut objects,&mut cursor,0,frame,|_,_,_|panic!("repeated native access reuses its frame")),Ok(0));
            assert_eq!((slots,objects,cursor),before);
        }
        assert_eq!(fixture["cases"].as_array().unwrap().len(),128);
        assert!(preparations>0&&unchanged>0);
    }

    #[test]
    fn actual_collision_search_uses_source_allocation_refresh_and_numeric_reuse(){
        use crate::ivp_motion::{PreparedFrames,PreparedObjectCache};
        fn controls(p:&Value)->(Prediction,ObjectFrame,f64,u32){
            let f=|v:&Value|v.as_f64().unwrap() as f32;
            let vec=|v:&Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));
            let quat=|v:&Value|std::array::from_fn(|i|f(&v[i]));
            let flags=p["flags"].as_u64().unwrap() as u32;
            (Prediction{start:vec(&p["start"]),velocity:vec(&p["velocity"]),rotation_start:quat(&p["rotation_start"]),rotation_end:quat(&p["rotation_end"]),time:p["time"].as_f64().unwrap(),inverse_dt:f(&p["inverse_dt"])},
             ObjectFrame{shift:(flags&0x800==0).then(||vec(&p["shift"])),rotation:p["extra"].as_bool().unwrap().then(||quat(&p["object_rotation"]))},
             p["now"].as_f64().unwrap(),p["frame"].as_u64().unwrap() as u32)
        }
        fn fingerprint(out:&PreparedObjectCache)->u64{
            let bytes:Vec<u8>=out.core_position.to_array().into_iter().chain(out.pose.position.to_array()).chain(out.quaternion).chain(out.pose.rotation.transpose().to_cols_array()).flat_map(|f|f.to_bits().to_le_bytes()).collect();
            crc32fast::hash(&bytes) as u64
        }
        fn pool(state:&Value)->(Vec<Slot>,Vec<Object>,usize){
            let mut slots=vec![Slot{references:0,frame_stamp:0,owner:None};state["capacity"].as_u64().unwrap() as usize];
            for s in state["slots"].as_array().unwrap(){slots[s["index"].as_u64().unwrap() as usize]=Slot{references:s["refs"].as_u64().unwrap() as u32,frame_stamp:s["frame"].as_u64().unwrap() as u32,owner:index(&s["owner"])};}
            (slots,objects(&state["objects"]),state["cursor"].as_u64().unwrap() as usize)
        }
        fn replay(frames:&mut PreparedFrames,latest:&mut [Option<usize>],e:&Value,p:&Value){
            assert_eq!(p,&e["input"]);
            let (prediction,object,now,stamp)=controls(p);
            let slot=e["pool_slot"].as_u64().unwrap() as u32;
            let owner=e["slot"].as_u64().unwrap()+1;
            let out=frames.prepare(slot+1,owner,prediction,now,object,stamp).unwrap();
            assert_eq!(fingerprint(&out),e["pose_crc32"].as_u64().unwrap());
            latest[slot as usize]=Some(e["id"].as_u64().unwrap() as usize);
        }
        fn external_preparation(slots:&mut [Slot],objects:&[Object],frames:&mut PreparedFrames,latest:&mut [Option<usize>],e:&Value){
            let slot=e["pool_slot"].as_u64().unwrap() as usize;
            let owner=e["slot"].as_u64().unwrap() as usize;
            assert_eq!(slots[slot].owner,Some(owner));assert_eq!(objects[owner].cache,Some(slot));
            replay(frames,latest,e,&e["input"]);
            slots[slot].frame_stamp=frames.get(slot as u32+1,owner as u64+1,e["input"]["frame"].as_u64().unwrap() as u32).unwrap().frame_stamp;
        }
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-cache-search.json")).unwrap();
        let (mut total,mut allocated,mut refreshed,mut reused,mut preparations)=(0,0,0,0,0);
        for profile in data["profiles"].as_array().unwrap(){
            let events=profile["frame_events"].as_array().unwrap();let mut next=0;
            let mut frames=PreparedFrames::default();let mut latest=vec![None;256];
            let mut slots=vec![Slot{references:0,frame_stamp:0,owner:None};256];
            let mut objects=vec![Object{flags:0,cache:None};2];let mut cursor=0;
            for row in profile["cases"].as_array().unwrap(){
                let begin=row["begin"].as_u64().unwrap() as usize;let end=row["end"].as_u64().unwrap() as usize;
                while next<begin{external_preparation(&mut slots,&objects,&mut frames,&mut latest,&events[next]);next+=1;preparations+=1;}
                assert_eq!(next,begin);
                // Body flags and other preparation requests remain observations.
                // Pool ownership, bindings, cursor, stamps and poses carry forward
                // from source operations; no per-span metadata reseeding.
                for i in 0..2{objects[i].flags=row["before"]["objects"][i]["flags"].as_u64().unwrap() as u32;}
                assert_eq!((slots.clone(),objects.clone(),cursor),pool(&row["before"]));
                let target=row["target"].as_u64().unwrap() as usize;
                let was_bound=objects[target].cache.is_some();let mut calls=0;
                let slot=access(&mut slots,&mut objects,&mut cursor,target,row["before"]["frame"].as_u64().unwrap() as u32,|slot,refs,_|{
                    assert!(next<end,"source prepared an unobserved frame");let e=&events[next];
                    assert_eq!(e["pool_slot"].as_u64().unwrap() as usize,slot);
                    assert_eq!(e["slot"].as_u64().unwrap() as usize,target);
                    assert_eq!(e["input"]["refs"].as_u64().unwrap() as u32,refs);
                    replay(&mut frames,&mut latest,e,&row["input"]);next+=1;calls+=1;preparations+=1;
                }).unwrap();
                assert_eq!(next,end,"source missed a native preparation");
                assert_eq!(latest[slot],Some(row["frame_event"].as_u64().unwrap() as usize));
                let out=frames.get(slot as u32+1,target as u64+1,slots[slot].frame_stamp).expect("source cache owner/stamp must match");
                assert_eq!(fingerprint(out),row["pose_crc32"].as_u64().unwrap());
                assert_eq!(out.frame_stamp,row["frame"].as_u64().unwrap() as u32);
                assert_eq!(slots[slot].references,row["refs"].as_u64().unwrap() as u32);
                assert_eq!((slots.clone(),objects.clone(),cursor),pool(&row["after"]));
                total+=1;if !was_bound{allocated+=1;}else if calls>0{refreshed+=1;}else{reused+=1;}
            }
            while next<events.len(){external_preparation(&mut slots,&objects,&mut frames,&mut latest,&events[next]);next+=1;preparations+=1;}
        }
        assert_eq!((total,allocated,refreshed,reused,preparations),(156,24,48,84,686));
    }

    #[test]
    fn native_cache_pool_reuses_source_ownership_across_consecutive_allocations(){
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-cache-pool.json")).unwrap();
        let mut state=(Vec::new(),Vec::new(),0);let mut chained=0;let mut prepared=0;
        for row in fixture["cases"].as_array().unwrap(){
            let b=&row["before"];let a=&row["after"];let target=row["target"].as_u64().unwrap() as usize;
            if row["step"]==0 {state=(slots(&b["slots"]),objects(&b["objects"]),b["cursor"].as_u64().unwrap() as usize);}else{chained+=1;}
            state.1[target].flags=b["objects"][target]["flags"].as_u64().unwrap() as u32;
            assert_eq!(state.0,slots(&b["slots"]));assert_eq!(state.1,objects(&b["objects"]));assert_eq!(state.2,b["cursor"].as_u64().unwrap() as usize);
            let result=allocate(&mut state.0,&mut state.1,&mut state.2,target).unwrap();
            assert_eq!(result.slot,row["selected"].as_u64().unwrap() as usize);assert_eq!(result.prepare,row["prepared"].as_bool().unwrap());
            assert!(row["untouched_payload"].as_bool().unwrap());
            if result.prepare {
                let p=&row["prediction"];let f=|v:&Value|v.as_f64().unwrap() as f32;
                let vec=|v:&Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));
                let quat=|v:&Value|std::array::from_fn(|i|f(&v[i]));
                let prediction=Prediction{start:vec(&p["start"]),velocity:vec(&p["velocity"]),rotation_start:quat(&p["rotation_start"]),rotation_end:quat(&p["rotation_end"]),time:p["time"].as_f64().unwrap(),inverse_dt:f(&p["inverse_dt"])};
                let frame=ObjectFrame{shift:(state.1[target].flags&0x800==0).then(||vec(&p["shift"])),rotation:p["extra"].as_bool().unwrap().then(||quat(&p["object_rotation"]))};
                let out=prediction.prepare_object_cache(p["now"].as_f64().unwrap(),frame,p["frame"].as_u64().unwrap() as u32);
                let values:Vec<f32>=out.core_position.to_array().into_iter().chain(out.pose.position.to_array()).chain(out.quaternion).chain(out.pose.rotation.transpose().to_cols_array()).collect();
                assert_eq!(values,row["values"].as_array().unwrap().iter().map(f).collect::<Vec<_>>());
                state.0[result.slot].frame_stamp=out.frame_stamp;prepared+=1;
            }
            assert_eq!(state.0,slots(&a["slots"]));assert_eq!(state.1,objects(&a["objects"]));assert_eq!(state.2,a["cursor"].as_u64().unwrap() as usize);
        }
        assert_eq!(fixture["cases"].as_array().unwrap().len(),256);assert_eq!(chained,224);assert_eq!(prepared,96);
    }

    #[test]
    fn invalid_or_fully_referenced_cache_pool_fails_without_mutation(){
        let mut slots=vec![Slot{references:1,frame_stamp:9,owner:Some(0)}];let mut objects=vec![Object{flags:8,cache:Some(0)}];let mut cursor=0;
        let before=(slots.clone(),objects.clone(),cursor);
        assert_eq!(allocate(&mut slots,&mut objects,&mut cursor,0),Err(AllocationError::Exhausted));
        assert_eq!((slots.clone(),objects.clone(),cursor),before);
        assert_eq!(allocate(&mut slots,&mut objects,&mut cursor,1),Err(AllocationError::InvalidIdentity));
        assert_eq!((slots.clone(),objects.clone(),cursor),before);
        cursor=1;assert_eq!(allocate(&mut slots,&mut objects,&mut cursor,0),Err(AllocationError::InvalidPool));
        assert_eq!(allocate(&mut [],&mut objects,&mut 0,0),Err(AllocationError::InvalidPool));
    }
}
