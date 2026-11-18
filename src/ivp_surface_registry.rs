//! Native104-byte surface parser payload and shared name/sound symbol binding.
//! Auto-creation of unknown names and broader malformed/numeric dialect remain separate.
use std::collections::HashSet;
#[derive(Clone,Debug)]
pub(crate) struct Record {pub name:String,pub properties:[f32;5],pub words:[u32;26],pub name_symbol:u16}
impl Record {fn new(name:String,name_symbol:u16,words:[u32;26])->Self{Self{name,name_symbol,properties:std::array::from_fn(|i|f32::from_bits(words[i])),words}}}
pub(crate) const SOUND_KEYS:[&str;12]=["walkleft","walkright","runleft","runright","impactsoft","impacthard","scrapesmooth","scraperough","bulletimpact","rolling","break","strain"];
#[derive(Clone)]
pub(crate) struct Registry {pub records:Vec<Record>,pub shadow:usize,pub shadow_created:bool,pub files:HashSet<String>,pub symbols:Vec<String>}
impl Default for Registry {
    fn default()->Self{let mut words=[0;26];words[0]=0.8f32.to_bits();words[1]=0.25f32.to_bits();words[2]=2000f32.to_bits();Self{records:vec![Record::new("default".into(),1,words)],shadow:0,shadow_created:false,files:HashSet::new(),symbols:vec![String::new(),"default".into()]}}
}
fn atof(text:&str)->Result<f32,&'static str>{
    let text=text.trim_start();let bytes=text.as_bytes();let mut at=0;
    let unsigned=text.strip_prefix('+').or_else(||text.strip_prefix('-')).unwrap_or(text).to_ascii_lowercase();
    if unsigned.starts_with("nan")||unsigned.starts_with("inf"){return Err("nonfinite surface physics value is unsupported");}
    if bytes.first().is_some_and(|c|matches!(c,b'+'|b'-')){at+=1;}
    let mut digits=0;while at<bytes.len()&&bytes[at].is_ascii_digit(){at+=1;digits+=1;}
    if bytes.get(at)==Some(&b'.'){at+=1;while at<bytes.len()&&bytes[at].is_ascii_digit(){at+=1;digits+=1;}}
    if digits==0{return Ok(0.);}
    if bytes.get(at).is_some_and(|c|matches!(c,b'e'|b'E')){
        let exponent=at;at+=1;if bytes.get(at).is_some_and(|c|matches!(c,b'+'|b'-')){at+=1;}let start=at;
        while at<bytes.len()&&bytes[at].is_ascii_digit(){at+=1;}if at==start{at=exponent;}
    }
    let value=text[..at].parse::<f64>().map_err(|_|"unsupported surface number")? as f32;
    if !value.is_finite(){return Err("nonfinite surface physics value is unsupported");}Ok(value)
}
pub(crate) fn atoi(text:&str)->Result<i32,&'static str>{
    let text=text.trim_start();let b=text.as_bytes();let mut at=0;if b.first().is_some_and(|c|matches!(c,b'+'|b'-')){at+=1;}let start=at;
    while at<b.len()&&b[at].is_ascii_digit(){at+=1;}if at==start{return Ok(0);}
    text[..at].parse::<i32>().map_err(|_|"surface integer overflow is unsupported")
}
impl Registry {
    pub(crate) fn load(assets:&mut crate::source_assets::SourceAssets)->std::io::Result<Self>{
        let manifest=assets.read("scripts/surfaceproperties_manifest.txt")?;let tokens=crate::kv::tokens(&String::from_utf8_lossy(&manifest));let mut registry=Self::default();
        for pair in tokens.windows(2).filter(|p|p[0].eq_ignore_ascii_case("file")){let bytes=assets.read(&pair[1])?;registry.parse_file(&pair[1],&String::from_utf8_lossy(&bytes)).map_err(|e|std::io::Error::new(std::io::ErrorKind::InvalidData,e))?;}
        Ok(registry)
    }
    fn intern(&mut self,text:&str)->Result<u16,&'static str>{
        if let Some(i)=self.symbols.iter().position(|s|s.eq_ignore_ascii_case(text)){return Ok(i as u16);}
        if self.symbols.len()>=65535{return Err("surface symbol table exhaustion is unsupported");}
        let i=self.symbols.len();self.symbols.push(text.into());Ok(i as u16)
    }
    pub(crate) fn sound<'a>(&'a self,record:&Record,slot:usize)->Option<&'a str>{
        if slot>=12{return None;}let id=(record.words[11+slot/2]>>(16*(slot%2)))&65535;
        if id==65535{Some("")}else{self.symbols.get(id as usize).map(String::as_str)}
    }
    pub(crate) fn index(&self,name:&str)->Option<usize>{
        if name.eq_ignore_ascii_case("$MATERIAL_INDEX_SHADOW"){return Some(0xf000);}
        self.records.iter().position(|r|r.name.eq_ignore_ascii_case(name))
    }
    fn slot(&self,index:usize)->Option<usize>{crate::ivp_material_backend::Backend{count:self.records.len(),shadow:self.shadow as i32}.lookup(index as i32)}
    pub(crate) fn get(&self,name:&str)->Option<&Record>{self.records.get(self.slot(self.index(name)?)?)}
    pub(crate) fn parse_file(&mut self,file:&str,text:&str)->Result<(),&'static str>{
        self.intern(file)?;if !self.files.insert(file.to_ascii_lowercase()){return Ok(());}
        let t=crate::kv::tokens(text);let mut at=0;
        while at<t.len(){
            let name=t[at].to_ascii_lowercase();at+=1;
            if t.get(at).map(String::as_str)!=Some("{"){return Err("expected flat surface block");}at+=1;
            let name_symbol=self.intern(&name)?;let initial=self.index(&name).unwrap_or(0);
            let mut words=self.slot(initial).map(|i|self.records[i].words).unwrap_or([0;26]);
            while t.get(at).map(String::as_str)!=Some("}"){
                let key=t.get(at).ok_or("unclosed surface block")?.to_ascii_lowercase();let value=t.get(at+1).ok_or("missing surface value")?;at+=2;
                if value=="{"||value=="}"{return Err("nested/malformed surface block is unsupported");}
                if key=="base" {if let Some(base)=self.get(value){words=base.words;}}
                else if let Some(field)=["friction","elasticity","density","thickness","dampening","audioreflectivity","audiohardnessfactor","audioroughnessfactor","scraperoughthreshold","impacthardthreshold","audiohardminvelocity"].iter().position(|k|*k==key){words[field]=atof(value)?.to_bits();}
                else if key=="maxspeedfactor" {words[17]=atof(value)?.to_bits();}
                else if key=="jumpfactor" {words[18]=atof(value)?.to_bits();}
                else if key=="gamematerial" {let b=value.as_bytes();let v=if b.len()==1&&!b[0].is_ascii_digit(){b[0].to_ascii_uppercase() as u16}else{atoi(value)? as u16};words[19]=(words[19]&0xffff0000)|v as u32;}
                else if key=="climbable" {words[19]=(words[19]&0xff00ffff)|((atoi(value)? as u8 as u32)<<16);}
                else {
                    let first=match key.as_str(){"stepleft"=>Some(0),"stepright"=>Some(1),_=>SOUND_KEYS.iter().position(|k|*k==key)};
                    if let Some(slot)=first {let id=self.intern(&value.to_ascii_lowercase())? as u32;let set=|words:&mut [u32;26],slot:usize|{let shift=16*(slot%2);let i=11+slot/2;words[i]=(words[i]&!(65535<<shift))|(id<<shift);};set(&mut words,slot);if key=="stepleft"||key=="stepright"{set(&mut words,slot+2);}}
                }
            }at+=1;
            if let Some(index)=self.index(&name){
                let slot=self.slot(index).ok_or("invalid existing material slot")?;self.records[slot].words=words;self.records[slot].properties=std::array::from_fn(|i|f32::from_bits(words[i]));
            }else{self.records.push(Record::new(self.symbols[name_symbol as usize].clone(),name_symbol,words));}
        }
        if !self.shadow_created {
            let mut words=self.records[0].words;words[0]=0.8f32.to_bits();words[1]=0.001f32.to_bits();let id=self.intern("$MATERIAL_INDEX_SHADOW")?;
            self.shadow=self.records.len();self.records.push(Record::new(self.symbols[id as usize].clone(),id,words));self.shadow_created=true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn check(registry:&Registry,state:&Value){
        assert_eq!(registry.records.len(),state["count"].as_u64().unwrap() as usize);assert_eq!(registry.shadow,state["shadow"].as_u64().unwrap() as usize);assert_eq!(registry.shadow_created,state["shadow_created"].as_bool().unwrap());assert_eq!(registry.files.len(),state["parsed_file_count"].as_u64().unwrap() as usize);
        for (record,native) in registry.records.iter().zip(state["records"].as_array().unwrap()){
            assert_eq!(record.name,native["name"].as_str().unwrap());
            for i in 0..5 {assert_eq!(record.properties[i].to_bits(),(native["properties"][i].as_f64().unwrap() as f32).to_bits(),"{} field{i}",record.name);}
        }
        for query in state["queries"].as_array().unwrap(){assert_eq!(registry.index(query["name"].as_str().unwrap()).map(|i|i as i64).unwrap_or(-1),query["index"].as_i64().unwrap());}
    }
    fn check_full(registry:&Registry,state:&Value){
        check(registry,state);
        assert_eq!(registry.symbols,state["symbols"].as_array().unwrap().iter().map(|s|s.as_str().unwrap().to_string()).collect::<Vec<_>>());
        for (record,native) in registry.records.iter().zip(state["records"].as_array().unwrap()){
            assert_eq!(record.name_symbol,native["name_symbol"].as_u64().unwrap() as u16);
            assert_eq!(record.words,std::array::from_fn(|i|native["words"][i].as_u64().unwrap() as u32),"payload {}",record.name);
            for i in 0..12 {assert_eq!(registry.sound(record,i),Some(native["sound_names"][i].as_str().unwrap()),"sound {} slot{i}",record.name);}
        }
    }
    #[test]
    fn complete_surface_payload_and_shared_symbols_match_native_per_file(){
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-surface-full.json")).unwrap();let mut snapshots=0;
        for capture in fixture["captures"].as_array().unwrap(){let mut registry=Registry::default();check_full(&registry,&capture["snapshots"][0]);snapshots+=1;
            for (i,file) in capture["files"].as_array().unwrap().iter().enumerate(){registry.parse_file(file["file"].as_str().unwrap(),file["text"].as_str().unwrap()).unwrap();check_full(&registry,&capture["snapshots"][i+1]);snapshots+=1;}
        }
        assert_eq!(snapshots,10);
    }
    #[test]
    fn complete_installed_retail_payloads_and_sound_symbols_match_native(){
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");if !root.exists(){return;}
        let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-surface-full.json")).unwrap();let retail=fixture["captures"].as_array().unwrap().iter().find(|c|c["label"]=="retail").unwrap();let registry=Registry::load(&mut assets).unwrap();
        check_full(&registry,retail["snapshots"].as_array().unwrap().last().unwrap());
    }
    #[test]
    fn ordered_core_surface_registry_matches_native_after_every_file(){
        let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-surface-registry.json")).unwrap();let mut snapshots=0;
        for capture in fixture["captures"].as_array().unwrap(){let mut registry=Registry::default();check(&registry,&capture["snapshots"][0]);snapshots+=1;
            for (i,file) in capture["files"].as_array().unwrap().iter().enumerate(){registry.parse_file(file["file"].as_str().unwrap(),file["text"].as_str().unwrap()).unwrap();check(&registry,&capture["snapshots"][i+1]);snapshots+=1;}
        }
        assert_eq!(snapshots,10);
    }
    #[test]
    fn installed_retail_full_files_match_native_core_registry(){
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");if !root.exists(){return;}
        let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();let fixture:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-surface-registry.json")).unwrap();
        let retail=fixture["captures"].as_array().unwrap().iter().find(|c|c["label"]=="retail").unwrap();let mut registry=Registry::default();
        for (i,file) in retail["files"].as_array().unwrap().iter().enumerate(){let name=file["file"].as_str().unwrap();let bytes=assets.read(name).unwrap();registry.parse_file(name,std::str::from_utf8(&bytes).unwrap()).unwrap();check(&registry,&retail["snapshots"][i+1]);}
    }
    #[test]
    fn unsupported_nonfinite_surface_values_fail_instead_of_becoming_zero(){
        for text in ["nan","-NaN","inf","+infinity","1e999"]{assert!(atof(text).is_err(),"{text}");}
    }
}
