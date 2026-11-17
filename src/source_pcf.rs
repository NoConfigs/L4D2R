//! Checked reader for the binary-3/4 DMX particle definitions shipped with L4D2.
use crate::binary::*;
use std::{collections::BTreeMap, io};

#[derive(Clone, Debug)]
pub enum Value {
    Int(i32),
    Reference(i32),
    Float(f32),
    Bool(bool),
    Text(String),
    Vector(Vec<f32>),
    Array(Vec<Value>),
    Other,
}
#[derive(Clone, Debug)]
pub struct Element {
    pub name: String,
    pub kind: String,
    pub attributes: BTreeMap<String, Value>,
}
impl Element {
    pub fn number(&self, key: &str, default: f32) -> f32 {
        match self.attributes.get(key) {
            Some(Value::Float(v)) => *v,
            Some(Value::Int(v) | Value::Reference(v)) => *v as f32,
            _ => default,
        }
    }
    pub fn text(&self, key: &str) -> &str {
        match self.attributes.get(key) {
            Some(Value::Text(v)) => v,
            _ => "",
        }
    }
    pub fn boolean(&self, key: &str) -> bool {
        matches!(self.attributes.get(key), Some(Value::Bool(true)))
    }
    pub fn vector(&self, key: &str, default: [f32; 3]) -> [f32; 3] {
        match self.attributes.get(key) {
            Some(Value::Vector(v)) if v.len() >= 3 => [v[0], v[1], v[2]],
            _ => default,
        }
    }
    pub fn references(&self, key: &str) -> Vec<usize> {
        match self.attributes.get(key) {
            Some(Value::Array(v)) => v
                .iter()
                .filter_map(|v| match v {
                    Value::Int(v) | Value::Reference(v) if *v >= 0 => Some(*v as usize),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        }
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    strings: Vec<String>,
    version: u8,
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        let b = range(self.bytes, self.at, n)?;
        self.at += n;
        Ok(b)
    }
    fn int(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn count(&mut self) -> io::Result<usize> {
        let n = self.int()?;
        if !(0..=1_000_000).contains(&n) {
            return Err(invalid("Invalid DMX count"));
        }
        Ok(n as usize)
    }
    fn raw_string(&mut self)->io::Result<String> {
        let text=cstring(self.bytes,self.at)?;
        self.at+=self.bytes[self.at..].iter().position(|b|*b==0).ok_or_else(||invalid("Unterminated DMX string"))?+1;
        Ok(text)
    }
    fn string(&mut self) -> io::Result<String> {
        let i = u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize;
        self.strings
            .get(i)
            .cloned()
            .ok_or_else(|| invalid("Invalid DMX string index"))
    }
    fn value(&mut self, kind: u8, depth: usize) -> io::Result<Value> {
        if depth > 2 {
            return Err(invalid("Nested DMX arrays"));
        }
        if (15..=28).contains(&kind) {
            let n = self.count()?;
            return (0..n)
                .map(|_| self.value(kind - 14, depth + 1))
                .collect::<io::Result<Vec<_>>>()
                .map(Value::Array);
        }
        Ok(match kind {
            1 => Value::Reference(self.int()?),
            2 | 7 => Value::Int(self.int()?),
            3 => {
                let v = f32::from_le_bytes(self.take(4)?.try_into().unwrap());
                if !v.is_finite() {
                    return Err(invalid("Nonfinite DMX float"));
                }
                Value::Float(v)
            }
            4 => Value::Bool(self.take(1)?[0] != 0),
            5 => Value::Text(if self.version<4 {self.raw_string()?} else {self.string()?}),
            6 => {
                let n = self.count()?;
                self.take(n)?;
                Value::Other
            }
            8 => Value::Vector(self.take(4)?.iter().map(|b| *b as f32).collect()),
            9..=14 => {
                let n = match kind {
                    9 => 2,
                    10 | 12 => 3,
                    11 | 13 => 4,
                    _ => 16,
                };
                let mut v = vec![];
                for _ in 0..n {
                    v.push(f32::from_le_bytes(self.take(4)?.try_into().unwrap()));
                }
                if v.iter().any(|v| !v.is_finite()) {
                    return Err(invalid("Nonfinite DMX vector"));
                }
                Value::Vector(v)
            }
            _ => return Err(invalid(format!("Unsupported DMX attribute type {kind}"))),
        })
    }
}
pub fn parse(bytes: &[u8]) -> io::Result<Vec<Element>> {
    let end = bytes
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| invalid("Missing DMX header terminator"))?;
    let header = String::from_utf8_lossy(&bytes[..end]);
    let version=if header.contains("encoding binary 4 format pcf") {4} else if header.contains("encoding binary 3 format pcf") {3} else {0};
    if version==0 {
        return Err(invalid(format!("Unsupported particle encoding: {header}")));
    }
    let mut r = Reader {
        bytes,
        at: end + 1,
        strings: vec![],
        version,
    };
    let strings=if version<4 {u16::from_le_bytes(r.take(2)?.try_into().unwrap()) as usize} else {r.count()?};
    for _ in 0..strings {let text=r.raw_string()?;r.strings.push(text);}
    let mut elements = vec![];
    for _ in 0..r.count()? {
        let kind = r.string()?;
        let name = if version<4 {r.raw_string()?} else {r.string()?};
        r.take(16)?;
        elements.push(Element {
            name,
            kind,
            attributes: BTreeMap::new(),
        });
    }
    for e in &mut elements {
        for _ in 0..r.count()? {
            let name = r.string()?;
            let kind = r.take(1)?[0];
            e.attributes.insert(name, r.value(kind, 0)?);
        }
    }
    if r.at != bytes.len() {
        return Err(invalid("Unconsumed DMX particle bytes"));
    }
    Ok(elements)
}

#[derive(Clone, Debug)]
pub struct SheetSequence {
    pub id: usize,
    pub looping: bool,
    pub frames: Vec<(f32, [f32; 4])>,
    pub duration: f32,
}
pub fn sheet(bytes: &[u8]) -> io::Result<Vec<SheetSequence>> {
    let mut data = None;
    if u32_at(bytes, 8)? < 3 {
        return Ok(vec![]);
    }
    for i in 0..count(bytes, 68, 64)? {
        let r = range(bytes, 80 + i * 8, 8)?;
        if r[..3] == [0x10, 0, 0] && r[3] & 2 == 0 {
            let at = u32_at(r, 4)? as usize;
            data = Some(range(bytes, at + 4, u32_at(bytes, at)? as usize)?);
        }
    }
    let Some(data) = data else {
        return Ok(vec![]);
    };
    let version = u32_at(data, 0)?;
    if version > 1 {
        return Err(invalid("Unsupported VTF sheet version"));
    }
    let mut at = 8;
    let mut sequences = vec![];
    for _ in 0..count(data, 4, 4096)? {
        let id = u32_at(data, at)? as usize;
        let looping = u32_at(data, at + 4)? == 0;
        let n = count(data, at + 8, 4096)?;
        let duration = f32_at(data, at + 12)?;
        at += 16;
        let mut frames = vec![];
        for _ in 0..n {
            let duration = f32_at(data, at)?;
            let uv = [
                f32_at(data, at + 4)?,
                f32_at(data, at + 8)?,
                f32_at(data, at + 12)?,
                f32_at(data, at + 16)?,
            ];
            at += if version == 1 { 68 } else { 20 };
            frames.push((duration, uv));
        }
        sequences.push(SheetSequence {
            id,
            looping,
            frames,
            duration,
        });
    }
    if at != data.len() {
        return Err(invalid("Unconsumed VTF sheet bytes"));
    }
    Ok(sequences)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary3_inline_names_and_values_decode_like_binary4_dictionary_strings() {
        fn fixture(version:u8)->Vec<u8> {
            let mut b=format!("<!-- dmx encoding binary {version} format pcf 2 -->\n\0").into_bytes();
            let words=["DmeParticleSystemDefinition","effect","material","particle/test"];
            if version==3 {b.extend_from_slice(&4u16.to_le_bytes());} else {b.extend_from_slice(&4i32.to_le_bytes());}
            for word in words {b.extend_from_slice(word.as_bytes());b.push(0);}
            b.extend_from_slice(&1i32.to_le_bytes());b.extend_from_slice(&0u16.to_le_bytes());
            if version==3 {b.extend_from_slice(b"effect\0");} else {b.extend_from_slice(&1u16.to_le_bytes());}
            b.extend_from_slice(&[0;16]);b.extend_from_slice(&1i32.to_le_bytes());b.extend_from_slice(&2u16.to_le_bytes());b.push(5);
            if version==3 {b.extend_from_slice(b"particle/test\0");} else {b.extend_from_slice(&3u16.to_le_bytes());}
            b
        }
        let a=parse(&fixture(3)).unwrap();let b=parse(&fixture(4)).unwrap();
        assert_eq!(a[0].name,b[0].name);assert_eq!(a[0].text("material"),"particle/test");assert_eq!(format!("{:?}",a),format!("{:?}",b));
        let mut truncated=fixture(3);truncated.pop();assert!(parse(&truncated).is_err());
    }
    #[test]
    fn owned_binary3_matches_valve_converter_graph() {
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");if !root.exists(){return;}
        let oracle=std::path::Path::new("local-research/parity/infected-fx-converted-1016.pcf");if !oracle.exists(){return;}
        let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
        let a=parse(&assets.read("particles/infected_fx.pcf").unwrap()).unwrap();let b=parse(&std::fs::read(oracle).unwrap()).unwrap();
        fn value(v:&Value,nodes:&[Element])->String {match v {
            Value::Reference(id)=>format!("ref:{:?}",if *id>=0 {nodes.get(*id as usize).map(|e|(&e.kind,&e.name))}else{None}),
            Value::Array(a)=>format!("[{}]",a.iter().map(|v|value(v,nodes)).collect::<Vec<_>>().join(",")),
            _=>format!("{v:?}")
        }}
        fn graph(nodes:&[Element])->Vec<String> {let mut rows=nodes.iter().map(|e|format!("{}:{}:{:?}",e.kind,e.name,e.attributes.iter().map(|(k,v)|(k,value(v,nodes))).collect::<Vec<_>>())).collect::<Vec<_>>();rows.sort();rows}
        assert_eq!(graph(&a),graph(&b));println!("Native binary3/converter binary4: {} matching graph elements",a.len());
    }
    #[test]
    fn rejects_unknown_encoding_and_truncated_dmx() {
        assert!(parse(b"<!-- dmx encoding binary 9 format pcf 2 -->\0").is_err());
        assert!(parse(b"<!-- dmx encoding binary 4 format pcf 2 -->\0\x01").is_err());
    }
}

#[cfg(test)]
mod dump_boomer_tmp {
    #[test]
    #[ignore]
    fn dump_boomer_screen_effect() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let bytes = assets.read("particles/boomer_fx.pcf").unwrap();
        let els = super::parse(&bytes).unwrap();
        let mut todo: Vec<usize> = els.iter().enumerate().filter(|(_, e)| e.name.contains("vomit") && e.kind == "DmeParticleSystemDefinition").map(|(i, _)| i).collect();
        let mut seen = std::collections::HashSet::new();
        while let Some(i) = todo.pop() {
            if !seen.insert(i) { continue; }
            let e = &els[i];
            println!("== [{i}] {} ({})", e.name, e.kind);
            for (k, v) in &e.attributes {
                let s = format!("{v:?}");
                println!("   {k} = {}", &s[..s.len().min(160)]);
                if let super::Value::Array(a) = v {
                    for r in a { if let super::Value::Reference(r) | super::Value::Int(r) = r { if *r >= 0 && (*r as usize) < els.len() && matches!(k.as_str(), "operators"|"renderers"|"initializers"|"emitters"|"children"|"forces"|"constraints") { todo.push(*r as usize); } } }
                }
                if let super::Value::Reference(r) = v { if *r >= 0 && k == "child" { todo.push(*r as usize); } }
            }
        }
    }
}
