//! Native surface-property movement sounds and model-authored door sound sets.
use crate::{kv, source_assets::SourceAssets};
use bevy::prelude::*;
use std::collections::{BTreeMap, HashMap};
#[derive(Resource, Default)]
pub struct NativeAudio {
    properties: HashMap<String, BTreeMap<String, String>>,
    floors: HashMap<(i32, i32), Vec<([Vec3; 3], String)>>,
    pub doors: HashMap<usize, BTreeMap<String, String>>,
}
fn resolved_properties(registry:&crate::ivp_surface_registry::Registry)->HashMap<String,BTreeMap<String,String>>{
    registry.records.iter().map(|record|{
        let value=registry.get(&record.name).unwrap();let sounds=crate::ivp_surface_registry::SOUND_KEYS.iter().enumerate().map(|(i,key)|((*key).to_string(),registry.sound(value,i).unwrap_or("").to_string())).collect();
        (record.name.to_ascii_lowercase(),sounds)
    }).collect()
}
#[cfg(test)]
fn blocks(text:&str)->HashMap<String,BTreeMap<String,String>>{
    let mut registry=crate::ivp_surface_registry::Registry::default();registry.parse_file("inline",text).unwrap();resolved_properties(&registry)
}
impl NativeAudio {
    pub fn load(
        assets: &mut SourceAssets,
        world: &crate::bsp::World,
        logic: &crate::l4d2r_cinematic::MapLogic,
    ) -> Self {
        let mut out = Self::default();
        if let Ok(registry)=crate::ivp_surface_registry::Registry::load(assets){out.properties=resolved_properties(&registry);}
        for surface in world.surfaces.iter().filter(|s| s.model == 0 && !s.sky) {
            let prop = crate::material::surface_property(assets, &surface.material);
            for tri in surface.positions.chunks_exact(3) {
                let t = [
                    Vec3::from_array(tri[0]),
                    Vec3::from_array(tri[1]),
                    Vec3::from_array(tri[2]),
                ];
                if (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero().y.abs() < 0.45 {
                    continue;
                }
                let lo = t.iter().fold(Vec3::splat(f32::MAX), |a, b| a.min(*b));
                let hi = t.iter().fold(Vec3::splat(f32::MIN), |a, b| a.max(*b));
                for x in (lo.x / 4.).floor() as i32..=(hi.x / 4.).floor() as i32 {
                    for z in (lo.z / 4.).floor() as i32..=(hi.z / 4.).floor() as i32 {
                        out.floors
                            .entry((x, z))
                            .or_default()
                            .push((t, prop.clone()));
                    }
                }
            }
        }
        let mut cache = HashMap::new();
        for (id, e) in logic.world.ents.iter().enumerate().filter(|(_, e)| {
            matches!(
                e.kind,
                l4d2r_game_core::entities::Kind::Door(_)
                    | l4d2r_game_core::entities::Kind::Elevator { .. }
            )
        }) {
            let mut sounds = BTreeMap::new();
            if let Some(model) = &e.me.mdl {
                let data = cache.entry(model.clone()).or_insert_with(|| {
                    let b = assets.read(model).ok()?;
                    let at = crate::binary::u32_at(&b, 312).ok()? as usize;
                    let len = crate::binary::u32_at(&b, 316).ok()? as usize;
                    let text = String::from_utf8_lossy(b.get(at..at + len)?);
                    Some(kv::tokens(&text))
                });
                if let Some(t) = data {
                    if let Some(i) = t.iter().position(|s| s == "defaults") {
                        for p in t[i..].windows(2) {
                            if matches!(
                                p[0].as_str(),
                                "move" | "open" | "close" | "locked" | "unlock"
                            ) {
                                sounds.insert(p[0].clone(), p[1].clone());
                            }
                        }
                    }
                }
            }
            for (key, field) in [
                ("move", "soundmoveoverride"),
                ("open", "soundopenoverride"),
                ("close", "soundcloseoverride"),
            ] {
                if let Some(v) = e.me.kv(field).filter(|v| !v.is_empty()) {
                    sounds.insert(key.into(), v.into());
                }
            }
            if let Some(v) = e.me.kv("noise1").filter(|v| !v.is_empty()) {
                sounds.entry("move".into()).or_insert_with(|| v.into());
            }
            if let Some(v) = e.me.kv("noise2").filter(|v| !v.is_empty()) {
                sounds.entry("open".into()).or_insert_with(|| v.into());
                sounds.entry("close".into()).or_insert_with(|| v.into());
            }
            if let Some(v) = e.me.kv("startsound").filter(|v| !v.is_empty()) {
                sounds.insert("move".into(), v.into());
            }
            if let Some(v) = e.me.kv("stopsound").filter(|v| !v.is_empty()) {
                sounds.insert("open".into(), v.into());
                sounds.insert("close".into(), v.into());
            }
            out.doors.insert(id, sounds);
        }
        println!(
            "Native movement audio: {} surface properties, {} floor cells, {} door sound sets",
            out.properties.len(),
            out.floors.len(),
            out.doors.len()
        );
        if let Ok(registry) = crate::source_sound::Registry::load(assets) {
            let mut events = std::collections::BTreeSet::from([
                "Player.JumpLand".to_string(),
                "Player.ShoeScuff".to_string(),
            ]);
            for name in out.floors.values().flatten().map(|(_, name)| name) {
                for key in ["walkleft", "walkright", "runleft", "runright"] {
                    if let Some(sound) = out.property(name, key, 0).filter(|s|!s.is_empty()) {
                        events.insert(format!("Survivor.{sound}"));
                    }
                }
            }
            events.extend(out.doors.values().flat_map(|s| s.values().cloned()));
            let mut missing = 0;
            let mut report = String::from("event\tstatus\n");
            for event in &events {
                let present = registry
                    .definitions
                    .get(&event.to_lowercase())
                    .is_some_and(|d| {
                        !d.waves.is_empty() && d.waves.iter().all(|p| assets.exists(p))
                    })
                    || crate::source_sound::wave_path(event).is_ok_and(|p| assets.exists(&p));
                if !present {
                    missing += 1;
                }
                report.push_str(&format!(
                    "{event}\t{}\n",
                    if present { "mounted" } else { "missing" }
                ));
            }
            let _ = std::fs::write("local-research/native-movement-door-audio.tsv", report);
            println!(
                "Native movement/door audio audit: {} events, {missing} missing",
                events.len()
            );
        }
        out
    }
    pub fn surface(&self, p: Vec3) -> &str {
        let mut best = 1.;
        let mut prop = "default";
        if let Some(list) = self
            .floors
            .get(&((p.x / 4.).floor() as i32, (p.z / 4.).floor() as i32))
        {
            for (t, name) in list {
                if let Some(y) = floor_height(*t, p) {
                    let distance = p.y - y;
                    if (-0.08..best).contains(&distance) {
                        best = distance;
                        prop = name;
                    }
                }
            }
        }
        prop
    }
    fn property(&self, surface: &str, key: &str, depth: usize) -> Option<&str> {
        if depth > 16 {
            return None;
        }
        let p = self.properties.get(surface)?;
        p.get(key).map(String::as_str).or_else(|| {
            self.property(
                p.get("base").map(String::as_str).unwrap_or("default"),
                key,
                depth + 1,
            )
        })
    }
    /// Footstep soundscript for a surface. `prefix` is the character family
    /// (Survivor, Infected, Boomer, Jockey, Tank; client.dll step tables).
    pub fn step(&self, p: Vec3, walk: bool, left: bool, prefix: &str) -> String {
        let key = match (walk, left) {
            (true, true) => "walkleft",
            (true, false) => "walkright",
            (false, true) => "runleft",
            (false, false) => "runright",
        };
        format!(
            "{prefix}.{}",
            self.property(self.surface(p), key, 0).unwrap_or(if walk {
                if left {
                    "default.WalkLeft"
                } else {
                    "default.WalkRight"
                }
            } else if left {
                "default.RunLeft"
            } else {
                "default.RunRight"
            })
        )
    }
    pub fn door(&self, id: usize, output: &str) -> Option<&str> {
        let key = match output {
            "OnOpen" | "OnClose" | "OnMoveStarted" => "move",
            "OnFullyOpen" | "OnReachedTop" => "open",
            "OnFullyClosed" | "OnReachedBottom" => "close",
            "OnLockedUse" => "locked",
            // Not a Source output: the first opening of a checkpoint door
            // (CPropDoorRotatingCheckpoint plays its door_options "unlock").
            "Unlock" => "unlock",
            _ => return None,
        };
        self.doors.get(&id)?.get(key).map(String::as_str)
    }
}
fn floor_height(t: [Vec3; 3], p: Vec3) -> Option<f32> {
    let a = Vec2::new(t[1].x - t[0].x, t[1].z - t[0].z);
    let b = Vec2::new(t[2].x - t[0].x, t[2].z - t[0].z);
    let q = Vec2::new(p.x - t[0].x, p.z - t[0].z);
    let d = a.perp_dot(b);
    if d.abs() < 1e-7 {
        return None;
    }
    let u = q.perp_dot(b) / d;
    let v = a.perp_dot(q) / d;
    if u < -0.001 || v < -0.001 || u + v > 1.001 {
        return None;
    }
    Some(t[0].y + u * (t[1].y - t[0].y) + v * (t[2].y - t[0].y))
}
#[cfg(test)]
mod elevator_door_tests {
    use super::*;
    /// c1m1's lower elevator doors are pried open with SetPosition 0.9:
    /// Source plays the func_movelinear start sound for that move and the
    /// stop sound when it arrives, without OnOpen/OnFullyOpen.
    #[test]
    fn c1m1_elevator_doors_play_start_and_stop_sounds_on_setposition() {
        use l4d2r_game_core::entities::EntEvent;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        if !path.is_file() {
            return;
        }
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, false).unwrap();
        let world = crate::bsp::load_file(&path, root).unwrap();
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let audio = NativeAudio::load(&mut assets, &world, &logic);
        // The lower pair (their brush models are the later *37/*38).
        let doors: Vec<usize> = logic.world.find("elevator_1_door1");
        let door = *doors.last().unwrap();
        assert_eq!(audio.door(door, "OnMoveStarted"), Some("Doors.Elevator.Open"));
        assert_eq!(audio.door(door, "OnFullyOpen"), Some("Elevator.Stop"));
        logic.world.events.clear();
        logic.world.input(door, "SetSpeed", "10", None, None);
        logic.world.input(door, "SetPosition", "0.9", None, None);
        let mut outputs = vec![];
        for _ in 0..1200 {
            logic.world.tick(1. / 60.);
            for e in logic.world.events.drain(..) {
                if let EntEvent::DoorSound { ent, output } = e {
                    if ent == door {
                        outputs.push(output);
                    }
                }
            }
        }
        assert_eq!(outputs.first().map(String::as_str), Some("OnMoveStarted"), "{outputs:?}");
        assert!(outputs.iter().any(|o| o == "OnFullyOpen"), "{outputs:?}");
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loaded_movement_audio_bindings_match_all_native_retail_sound_slots(){
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");if !root.exists(){return;}
        let mut assets=SourceAssets::new(root,vec![]).unwrap();let registry=crate::ivp_surface_registry::Registry::load(&mut assets).unwrap();let audio=NativeAudio{properties:resolved_properties(&registry),..Default::default()};
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-surface-full.json")).unwrap();let retail=fixture["captures"].as_array().unwrap().iter().find(|c|c["label"]=="retail").unwrap();let state=retail["snapshots"].as_array().unwrap().last().unwrap();let mut count=0;
        for record in state["records"].as_array().unwrap(){let name=record["name"].as_str().unwrap().to_ascii_lowercase();for (slot,key) in crate::ivp_surface_registry::SOUND_KEYS.iter().enumerate(){assert_eq!(audio.property(&name,key,0),record["sound_names"][slot].as_str(),"{name} {key}");count+=1;}}
        assert_eq!(count,1260);
    }
    #[test]
    fn surface_inheritance_and_floor_level_choose_native_steps() {
        let mut a = NativeAudio::default();
        a.properties = blocks(
            "default { runleft default.RunLeft } metal { base default runleft SolidMetal.RunLeft walkright SolidMetal.WalkRight } metalpanel { base metal }",
        );
        let t = [Vec3::ZERO, Vec3::new(4., 0., 0.), Vec3::new(0., 0., 4.)];
        a.floors.insert(
            (0, 0),
            vec![
                (t, "metalpanel".into()),
                (t.map(|p| p - Vec3::Y * 3.), "default".into()),
            ],
        );
        assert_eq!(
            a.step(Vec3::new(1., 0., 1.), false, true, "Survivor"),
            "Survivor.solidmetal.runleft"
        );
        assert_eq!(
            a.step(Vec3::new(1., 0., 1.), true, false, "Survivor"),
            "Survivor.solidmetal.walkright"
        );
        assert_eq!(a.surface(Vec3::new(3., 0., 3.)), "default");
    }
}
