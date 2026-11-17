//! Hammer VMF reader. Reference only: the game loads the BSP entity lump, but the decompiled VMFs
//! (`$L4D2_VMF_DIR`, BSPSource output) make entity I/O, brush entities and finale setups readable.
//! VMF entity `id` corresponds to the BSP entity's `hammerid` key.

use crate::kv::{Kv, KvValue};

/// One entity output connection: `Output` -> `target.Input(param)` after `delay`, fired `times` times (-1 = forever).
#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub output: String,
    pub target: String,
    pub input: String,
    pub param: String,
    pub delay: f32,
    pub times: i32,
}

impl Connection {
    /// Parse a connection value. Compiled BSPs separate fields with 0x1B (ESC); Hammer/older files use commas.
    pub fn parse(output: &str, value: &str) -> Option<Connection> {
        let parts: Vec<&str> = if value.contains('\x1b') {
            value.split('\x1b').collect()
        } else {
            value.split(',').collect()
        };
        if parts.len() != 5 {
            return None;
        }
        let delay = parts[3].trim().parse::<f32>().ok()?;
        let times = parts[4].trim().parse::<i32>().ok()?;
        Some(Connection {
            output: output.to_string(),
            target: parts[0].to_string(),
            input: parts[1].to_string(),
            param: parts[2].to_string(),
            delay,
            times,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct VmfEntity {
    /// Hammer object id (== `hammerid` in the BSP entity lump).
    pub id: i64,
    /// Plain string keyvalues, in file order (excluding `id`).
    pub keys: Vec<(String, String)>,
    pub connections: Vec<Connection>,
    /// Number of `solid` blocks (brush entity if > 0).
    pub solids: usize,
}

impl VmfEntity {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
    pub fn classname(&self) -> &str {
        self.get("classname").unwrap_or("")
    }
}

#[derive(Clone, Debug, Default)]
pub struct Vmf {
    /// The `world` block (worldspawn keys; `solids` = world brushes).
    pub world: VmfEntity,
    pub entities: Vec<VmfEntity>,
}

fn read_entity(kv: &Kv) -> VmfEntity {
    let mut e = VmfEntity::default();
    for c in kv.children() {
        match &c.value {
            KvValue::Str(v) if c.key.eq_ignore_ascii_case("id") => {
                e.id = v.trim().parse().unwrap_or(0)
            }
            KvValue::Str(v) => e.keys.push((c.key.clone(), v.clone())),
            KvValue::Obj(_) if c.key.eq_ignore_ascii_case("solid") => e.solids += 1,
            KvValue::Obj(_) if c.key.eq_ignore_ascii_case("connections") => {
                for o in c.children() {
                    if let Some(conn) = o.as_str().and_then(|v| Connection::parse(&o.key, v)) {
                        e.connections.push(conn);
                    }
                }
            }
            KvValue::Obj(_) => {} // editor, hidden, group, etc.
        }
    }
    e
}

impl Vmf {
    pub fn parse(src: &str) -> Vmf {
        let root = Kv::parse(src);
        let mut vmf = Vmf::default();
        for c in root.children() {
            if c.key.eq_ignore_ascii_case("world") {
                vmf.world = read_entity(c);
            } else if c.key.eq_ignore_ascii_case("entity") {
                vmf.entities.push(read_entity(c));
            } else if c.key.eq_ignore_ascii_case("hidden") {
                // Hidden entities are wrapped: hidden { entity { ... } }
                for h in c
                    .children()
                    .iter()
                    .filter(|h| h.key.eq_ignore_ascii_case("entity"))
                {
                    vmf.entities.push(read_entity(h));
                }
            }
        }
        vmf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entity_and_connections() {
        let src = "world\n{\n\"id\" \"1\"\n\"classname\" \"worldspawn\"\nsolid\n{\n\"id\" \"2\"\n}\n}\n\
                   entity\n{\n\"id\" \"7\"\n\"classname\" \"logic_relay\"\nconnections\n{\n\
                   \"OnTrigger\" \"door,Open,,1.5,-1\"\n\"OnTrigger\" \"lamp\x1bTurnOn\x1b\x1b0\x1b1\"\n}\n}\n";
        let v = Vmf::parse(src);
        assert_eq!(v.world.solids, 1);
        assert_eq!(v.entities.len(), 1);
        let e = &v.entities[0];
        assert_eq!((e.id, e.classname()), (7, "logic_relay"));
        assert_eq!(e.connections.len(), 2);
        assert_eq!(e.connections[0].delay, 1.5);
        assert_eq!(e.connections[1].times, 1);
        assert_eq!(e.connections[1].input, "TurnOn");
    }
}

/// Result of matching a map's BSP entity lump against its decompiled VMF by `hammerid`.
#[derive(Debug, Default)]
pub struct CrossCheck {
    pub bsp_entities: usize,
    pub vmf_entities: usize,
    /// BSP entities with a `hammerid` found in the VMF.
    pub matched: usize,
    /// BSP entities whose `hammerid` is absent from the VMF (or have none), as (hammerid, classname).
    pub missing_in_vmf: Vec<(Option<i64>, String)>,
    /// (hammerid, field, bsp value, vmf value) for matched entities that differ.
    pub diffs: Vec<(i64, String, String, String)>,
}

impl CrossCheck {
    pub fn run(bsp: &[crate::bsp::Entity], vmf: &Vmf) -> CrossCheck {
        use std::collections::HashMap;
        let by_id: HashMap<i64, &VmfEntity> = vmf.entities.iter().map(|e| (e.id, e)).collect();
        let mut r = CrossCheck {
            bsp_entities: bsp.len(),
            vmf_entities: vmf.entities.len(),
            ..Default::default()
        };
        for be in bsp.iter().skip(1) {
            let Some(ve) = be.hammerid().and_then(|id| by_id.get(&id)) else {
                r.missing_in_vmf
                    .push((be.hammerid(), be.classname().to_string()));
                continue;
            };
            r.matched += 1;
            let mut diff = |f: &str, a: String, b: String| {
                if a != b {
                    r.diffs.push((ve.id, f.to_string(), a, b));
                }
            };
            diff("classname", be.classname().into(), ve.classname().into());
            diff(
                "targetname",
                be.get("targetname").unwrap_or("").into(),
                ve.get("targetname").unwrap_or("").into(),
            );
            let bc = be.connections();
            diff(
                "connections",
                bc.len().to_string(),
                ve.connections.len().to_string(),
            );
            if bc.len() == ve.connections.len() {
                for (a, b) in bc.iter().zip(&ve.connections) {
                    let fmt = |c: &Connection| {
                        format!(
                            "{}>{}.{}({})@{}x{}",
                            c.output, c.target, c.input, c.param, c.delay, c.times
                        )
                    };
                    diff("connection", fmt(a), fmt(b));
                }
            }
            if let (Some(a), Some(b)) = (be.vec3("origin"), ve.get("origin")) {
                let bv: Vec<f32> = b
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if bv.len() == 3
                    && (a - glam::Vec3::new(bv[0], bv[1], bv[2]))
                        .abs()
                        .max_element()
                        > 0.5
                {
                    diff("origin", be.get("origin").unwrap_or("").into(), b.into());
                }
            }
        }
        r
    }
}
