//! Owned10092510 manager remap/fallback and100928a0 indexed material backend.
//! Registry population and attachment to retail map features remain separate.
use std::sync::{Arc,RwLock};
pub(crate) struct Backend {pub count:usize,pub shadow:i32}
pub(crate) struct Manager {pub map:[u16;128]}
#[derive(Clone,Copy,Debug)]
pub(crate) struct ContactMaterial {pub key:u32,pub friction:f32,pub elasticity:f32}
/// Ordinary surface records with either an authored remap snapshot or the
/// native singleton's shared live remap. Ordinary PSI retains contact friction;
/// native impact-cache rebuilds resolve it again against the current table.
#[derive(Clone)]
pub(crate) struct ContactMaterials {records:Vec<ContactMaterial>,map:[u16;128],shadow:i32,shared:Option<WorldMaterialTable>,registry:Option<SharedSurfaceRegistry>}
impl ContactMaterials {
    pub(crate) fn from_registry(registry:&crate::ivp_surface_registry::Registry,manager:&Manager)->Result<Self,&'static str>{
        if registry.records.is_empty()||registry.shadow>=registry.records.len()||registry.records.len()>i32::MAX as usize{return Err("invalid ordinary contact material registry");}
        let records=registry.records.iter().enumerate().map(|(key,r)|ContactMaterial{key:key as u32,friction:r.properties[0],elasticity:r.properties[1]}).collect::<Vec<_>>();
        if records.iter().any(|r|!r.friction.is_finite()||!r.elasticity.is_finite()){return Err("nonfinite ordinary contact material");}
        Ok(Self{records,map:manager.map,shadow:registry.shadow as i32,shared:None,registry:None})
    }
    /// Object defaults already carry a backend material index; the world
    /// triangle remap must not be applied to this identity.
    pub(crate) fn resolve_object(&self,index:i32)->Result<ContactMaterial,&'static str>{
        if let Some(registry)=&self.registry{
            let registry=registry.0.read().unwrap();let slot=Backend{count:registry.records.len(),shadow:registry.shadow as i32}.lookup(index).ok_or("invalid object material identity")?;let record=&registry.records[slot];
            return Ok(ContactMaterial{key:slot as u32,friction:record.properties[0],elasticity:record.properties[1]});
        }
        let slot=Backend{count:self.records.len(),shadow:self.shadow}.lookup(index).ok_or("invalid object material identity")?;Ok(self.records[slot])
    }
    pub(crate) fn resolve(&self,code:u8)->Result<ContactMaterial,&'static str>{
        if let Some(registry)=&self.registry{
            let registry=registry.0.read().unwrap();let backend=Backend{count:registry.records.len(),shadow:registry.shadow as i32};
            let selected=if let Some(table)=&self.shared{table.resolve(&backend,code as u32)?}else{Manager{map:self.map}.resolve(&backend,code as u32)?}.ok_or("ordinary feature material lacks fallback")?;
            let record=&registry.records[selected];return Ok(ContactMaterial{key:selected as u32,friction:record.properties[0],elasticity:record.properties[1]});
        }
        let backend=Backend{count:self.records.len(),shadow:self.shadow};
        let selected=if let Some(table)=&self.shared{table.resolve(&backend,code as u32)?}else{Manager{map:self.map}.resolve(&backend,code as u32)?};
        let slot=selected.ok_or("ordinary feature material lacks fallback");
        Ok(self.records[slot?])
    }
}
/// Native surface records are shared by existing environment/material handles.
/// Parsing a new filename updates future lookups; duplicate filenames are ignored.
#[derive(Clone)]
pub(crate) struct SharedSurfaceRegistry(Arc<RwLock<crate::ivp_surface_registry::Registry>>);
impl SharedSurfaceRegistry{
    pub(crate) fn new(registry:crate::ivp_surface_registry::Registry)->Self{Self(Arc::new(RwLock::new(registry)))}
    pub(crate) fn parse_file(&self,file:&str,text:&str)->Result<(),&'static str>{self.0.write().unwrap().parse_file(file,text)}
    pub(crate) fn contact_materials(&self,table:&WorldMaterialTable)->Result<ContactMaterials,&'static str>{
        let mut binding=table.contact_materials(&self.0.read().unwrap())?;binding.registry=Some(self.clone());Ok(binding)
    }
}
impl Backend {
    pub(crate) fn lookup(&self,index:i32)->Option<usize>{
        let index=if index>127 {if index==0xf000 {self.shadow}else{0}}else{index};
        // Shadow substitution is not passed through the ordinary >127 rule again.
        (index>=0&&(index as usize)<self.count).then_some(index as usize)
    }
}
impl Manager {
    /// Public100925c0: cap at128, truncate to16 bits, retain the untouched tail.
    pub(crate) fn set_world_table(&mut self,input:&[i32],count:i32)->Result<(),&'static str>{
        let n=count.clamp(0,128) as usize;
        // Native would read outside the caller's buffer. Fail before any writes.
        if input.len()<n{return Err("short native material table buffer is unsupported");}
        for (target,value) in self.map[..n].iter_mut().zip(&input[..n]){*target=*value as u16;}
        Ok(())
    }
    pub(crate) fn resolve_with(&self,index:u32,mut backend:impl FnMut(i32)->Option<usize>)->Result<Option<usize>,&'static str>{
        // Native treats the argument as signed and would index before its map
        // for negative inputs. Reject that invalid state instead of reading OOB.
        if index>i32::MAX as u32{return Err("negative native manager index is unsupported");}
        let query=if index<128 {self.map[index as usize] as i32}else{index as i32};
        Ok(backend(query).or_else(||backend(0)))
    }
    pub(crate) fn resolve(&self,backend:&Backend,index:u32)->Result<Option<usize>,&'static str>{self.resolve_with(index,|i|backend.lookup(i))}
}

/// Environment handles share the singleton's live table, rather than copying
/// it on creation. This models native ownership; map-feature wiring is separate.
#[derive(Clone)]
pub(crate) struct WorldMaterialTable(Arc<RwLock<Manager>>);
impl Default for WorldMaterialTable {
    fn default()->Self{Self(Arc::new(RwLock::new(Manager{map:std::array::from_fn(|i|i as u16)})))}
}
impl WorldMaterialTable {
    /// Keep the same manager handle in every bound physics world/transaction.
    /// Publishing a table changes future lookups without rewriting contacts.
    pub(crate) fn contact_materials(&self,registry:&crate::ivp_surface_registry::Registry)->Result<ContactMaterials,&'static str>{
        let mut binding=ContactMaterials::from_registry(registry,&self.0.read().unwrap())?;
        binding.shared=Some(self.clone());Ok(binding)
    }
    pub(crate) fn set(&self,input:&[i32],count:i32)->Result<(),&'static str>{self.0.write().unwrap().set_world_table(input,count)}
    pub(crate) fn resolve(&self,backend:&Backend,index:u32)->Result<Option<usize>,&'static str>{self.0.read().unwrap().resolve(backend,index)}
    /// Server101612b0 clears a128-entry array for each materialtable block,
    /// then submits it in full. Missing blocks leave the existing table alone.
    pub(crate) fn load_world_keys(&self,text:&str,registry:&crate::ivp_surface_registry::Registry)->Result<usize,&'static str>{
        let tables=parse_world_tables(text,registry,[0;128])?;
        for table in &tables{self.set(table,128)?;}Ok(tables.len())
    }
    pub(crate) fn load_bsp_world(&self,bsp:&l4d2_formats::bsp::Bsp,registry:&crate::ivp_surface_registry::Registry)->Result<usize,String>{
        let models=bsp.physics_models().map_err(|e|e.to_string())?;
        match models.iter().find(|m|m.model_index==0){Some(world)=>self.load_world_keys(world.keyvalues,registry).map_err(str::to_owned),None=>Ok(0)}
    }
}

/// Public collision parser100aae60: ordered material-name/index pairs. This
/// accepts valid flat tables; native negative indices write outside the array.
pub(crate) fn parse_world_tables(text:&str,registry:&crate::ivp_surface_registry::Registry,initial:[i32;128])->Result<Vec<[i32;128]>,&'static str>{
    let tokens=crate::kv::tokens(text);let mut at=0;let mut tables=Vec::new();
    while at<tokens.len(){
        let name=&tokens[at];at+=1;if tokens.get(at).map(String::as_str)!=Some("{"){return Err("missing collision block");}at+=1;
        let mut table=initial;let mut depth=1;
        while depth>0{
            let key=tokens.get(at).ok_or("unclosed collision block")?;at+=1;
            if key=="}"{depth-=1;continue;}
            if key=="{"{depth+=1;continue;}
            if !name.eq_ignore_ascii_case("materialtable"){continue;}
            if depth!=1{return Err("nested material table is unsupported");}
            let value=tokens.get(at).ok_or("missing material table value")?;at+=1;
            if value=="{"||value=="}"{return Err("malformed material table value");}
            let index=crate::ivp_surface_registry::atoi(value)?;
            if index<0{return Err("negative native material table index is unsupported");}
            if index<128{table[index as usize]=registry.index(key).map(|i|i as i32).unwrap_or(-1);}
        }
        if name.eq_ignore_ascii_case("materialtable"){tables.push(table);}
    }Ok(tables)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn live_binding_tracks_native_registry_growth_redefinition_and_shadow(){
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-surface-registry.json")).unwrap();let mut checked=0;
        for capture in fixture["captures"].as_array().unwrap(){
            let shared=SharedSurfaceRegistry::new(crate::ivp_surface_registry::Registry::default());let table=WorldMaterialTable::default();let binding=shared.contact_materials(&table).unwrap();
            for (file,state) in capture["files"].as_array().unwrap().iter().zip(capture["snapshots"].as_array().unwrap().iter().skip(1)){
                shared.parse_file(file["file"].as_str().unwrap(),file["text"].as_str().unwrap()).unwrap();
                let input=std::array::from_fn::<_,128,_>(|i|i as i32);table.set(&input,128).unwrap();
                for code in 0..128u8{let slot=if (code as u64)<state["count"].as_u64().unwrap(){code as usize}else{0};let actual=binding.resolve(code).unwrap();let native=&state["records"][slot];assert_eq!(actual.key,slot as u32);
                    assert_eq!(actual.friction.to_bits(),(native["properties"][0].as_f64().unwrap() as f32).to_bits());assert_eq!(actual.elasticity.to_bits(),(native["properties"][1].as_f64().unwrap() as f32).to_bits());checked+=1;
                }
                let mut input=input;input[127]=0xf000;table.set(&input,128).unwrap();let actual=binding.resolve(127).unwrap();let slot=state["shadow"].as_u64().unwrap() as usize;let native=&state["records"][slot];assert_eq!(actual.key,slot as u32);assert_eq!(actual.friction.to_bits(),(native["properties"][0].as_f64().unwrap() as f32).to_bits());assert_eq!(actual.elasticity.to_bits(),(native["properties"][1].as_f64().unwrap() as f32).to_bits());
            }
        }assert_eq!(checked,1024);
    }
    fn world_parse_fixture()->(Value,crate::ivp_surface_registry::Registry){
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-world-table-parse.json")).unwrap();
        let mut registry=crate::ivp_surface_registry::Registry::default();
        for file in fixture["surface_files"].as_array().unwrap(){registry.parse_file(file["file"].as_str().unwrap(),file["text"].as_str().unwrap()).unwrap();}
        (fixture,registry)
    }
    #[test]
    fn native_world_key_parser_order_and_full_table_submission_match(){
        let (fixture,registry)=world_parse_fixture();let mut blocks=0;
        for (i,case) in fixture["cases"].as_array().unwrap().iter().enumerate(){
            let initial=std::array::from_fn(|j|if i%2==0{0}else{j as i32*17+i as i32});
            let tables=parse_world_tables(case["text"].as_str().unwrap(),&registry,initial).unwrap();
            assert_eq!(tables.len(),case["blocks"].as_array().unwrap().len());
            let table=WorldMaterialTable::default();
            for (actual,native) in tables.iter().zip(case["blocks"].as_array().unwrap()){
                assert_eq!(initial,std::array::from_fn(|j|native["before"][j].as_i64().unwrap() as i32));
                assert_eq!(*actual,std::array::from_fn(|j|native["after"][j].as_i64().unwrap() as i32));
                table.set(actual,128).unwrap();assert_eq!(table.0.read().unwrap().map,std::array::from_fn(|j|native["map"][j].as_u64().unwrap() as u16));blocks+=1;
            }
        }assert_eq!(blocks,42);
    }
    #[test]
    fn installed_bsp_world_material_tables_match_native_key_parser(){
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");if !root.exists(){return;}
        let (fixture,_)=world_parse_fixture();let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
        let registry=crate::ivp_surface_registry::Registry::load(&mut assets).unwrap();let mut checked=0;
        for (i,case) in fixture["cases"].as_array().unwrap().iter().enumerate().filter(|(_,c)|c["kind"]=="retail"){
            let bytes=std::fs::read(root.join("left4dead2/maps").join(format!("{}.bsp",case["map"].as_str().unwrap()))).unwrap();
            let bsp=l4d2_formats::bsp::Bsp::parse(bytes).unwrap();let models=bsp.physics_models().unwrap();let world=models.iter().find(|m|m.model_index==0).unwrap();
            assert_eq!(world.collision.len() as u64,case["collision_size"].as_u64().unwrap());assert_eq!(world.solid_count as u64,case["solid_count"].as_u64().unwrap());
            let initial=std::array::from_fn(|j|if i%2==0{0}else{j as i32*17+i as i32});
            let actual=parse_world_tables(world.keyvalues,&registry,initial).unwrap();
            for (table,native) in actual.iter().zip(case["blocks"].as_array().unwrap()){
                assert_eq!(*table,std::array::from_fn(|j|native["after"][j].as_i64().unwrap() as i32));
            }
            let table=WorldMaterialTable::default();assert_eq!(table.load_bsp_world(&bsp,&registry).unwrap(),actual.len());
            let expected=parse_world_tables(world.keyvalues,&registry,[0;128]).unwrap();assert_eq!(table.0.read().unwrap().map,expected.last().unwrap().map(|i|i as u16));checked+=1;
        }assert_eq!(checked,3);
    }
    #[test]
    fn malformed_world_table_indices_fail_before_shared_table_changes(){
        let registry=crate::ivp_surface_registry::Registry::default();let table=WorldMaterialTable::default();let initial=table.0.read().unwrap().map;
        for text in ["materialtable { default -1 }","materialtable { default 9999999999999999 }","materialtable { default }","materialtable { default 1"]{
            assert!(table.load_world_keys(text,&registry).is_err());assert_eq!(table.0.read().unwrap().map,initial);
        }
        assert_eq!(table.load_world_keys("solid { index 0 }",&registry).unwrap(),0);assert_eq!(table.0.read().unwrap().map,initial);
    }
    #[test]
    fn native_world_table_updates_propagate_to_existing_and_new_environments(){
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-world-material.json")).unwrap();
        let backend=Backend{count:fixture["count"].as_u64().unwrap() as usize,shadow:fixture["shadow"].as_i64().unwrap() as i32};
        let table=WorldMaterialTable::default();let mut environments=vec![table.clone(),table.clone()];let mut queries=0;
        let mut check=|state:&Value,environments:&[WorldMaterialTable]|{
            let expected=std::array::from_fn::<_,128,_>(|i|state["map"][i].as_u64().unwrap() as u16);
            assert_eq!(table.0.read().unwrap().map,expected);
            assert_eq!(state["environments"].as_array().unwrap().len(),environments.len());
            for (env,native) in environments.iter().zip(state["environments"].as_array().unwrap()){
                assert!(native["shared_backend"].as_bool().unwrap());assert!(native["embedded_manager"].as_bool().unwrap());
                assert_eq!(env.0.read().unwrap().map,expected);
                for (i,result) in native["results"].as_array().unwrap().iter().enumerate(){
                    let index=if i<128{i as u32}else{[128,129,0xf000,65535][i-128]};
                    assert_eq!(env.resolve(&backend,index).unwrap().map(|v|v as i64).unwrap_or(-1),result.as_i64().unwrap());queries+=1;
                }
            }
        };
        check(&fixture["initial"],&environments);
        for (i,update) in fixture["updates"].as_array().unwrap().iter().enumerate(){
            if i==32{environments.push(table.clone());}
            check(&update["before"],&environments);
            let input:Vec<i32>=update["input"].as_array().unwrap().iter().map(|v|v.as_i64().unwrap() as i32).collect();
            table.set(&input,update["count"].as_i64().unwrap() as i32).unwrap();
            assert!(update["input_unchanged"].as_bool().unwrap());check(&update["after"],&environments);
        }
        assert_eq!(queries,42504);
    }
    #[test]
    fn short_world_material_buffers_fail_without_partial_mutation(){
        let table=WorldMaterialTable::default();let before=table.0.read().unwrap().map;
        assert!(table.set(&[7],2).is_err());assert_eq!(table.0.read().unwrap().map,before);
        table.set(&[],i32::MIN).unwrap();table.set(&[],0).unwrap();assert_eq!(table.0.read().unwrap().map,before);
    }
    #[test]
    fn native_backend_indices_shadow_and_null_fallback_match_every_call(){
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-material-backend.json")).unwrap();
        let mut count=0;let mut calls=0;
        for row in fixture["cases"].as_array().unwrap(){
            let backend=Backend{count:row["count"].as_u64().unwrap() as usize,shadow:row["shadow"].as_i64().unwrap() as i32};
            let manager=Manager{map:std::array::from_fn(|i|row["map"][i].as_u64().unwrap() as u16)};
            let index=row["direct_input"].as_i64().unwrap() as i32;let mut observed=vec![(index,backend.lookup(index))];
            let result=manager.resolve_with(row["request"].as_u64().unwrap() as u32,|i|{let r=backend.lookup(i);observed.push((i,r));r}).unwrap();
            let native_slot=|v:&Value|{let i=v.as_i64().unwrap();if i<0{assert_eq!(i,-1);None}else{Some(i as usize)}};
            assert_eq!(observed[0].1,native_slot(&row["direct_result"]));assert_eq!(result,native_slot(&row["manager_result"]));
            assert_eq!(observed.len(),row["calls"].as_array().unwrap().len());
            for ((input,result),event) in observed.into_iter().zip(row["calls"].as_array().unwrap()){
                assert_eq!(input,event["input"].as_i64().unwrap() as i32);assert_eq!(result,native_slot(&event["result"]));assert!(event["complete"].as_bool().unwrap());calls+=1;
            }
            assert!(row["unchanged"].as_bool().unwrap());count+=1;
        }
        assert_eq!(count,512);assert_eq!(calls,1157);
    }
    #[test]
    fn native_initialized_environment_manager_queries_match(){
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-material-backend.json")).unwrap();let mut count=0;
        for state in fixture["actual"].as_array().unwrap(){
            let backend=Backend{count:state["count"].as_u64().unwrap() as usize,shadow:state["shadow"].as_i64().unwrap() as i32};let manager=Manager{map:std::array::from_fn(|i|state["map"][i].as_u64().unwrap() as u16)};
            for query in state["queries"].as_array().unwrap(){let slot=manager.resolve(&backend,query["request"].as_u64().unwrap() as u32).unwrap();assert_eq!(slot.map(|i|i as i64).unwrap_or(-1),query["result"].as_i64().unwrap());count+=1;}
        }
        assert_eq!(count,132);
    }
    #[test]
    fn negative_manager_indices_fail_before_any_backend_access(){
        let manager=Manager{map:[0;128]};let mut calls=0;
        assert!(manager.resolve_with(u32::MAX,|_|{calls+=1;None}).is_err());assert_eq!(calls,0);
    }
}
