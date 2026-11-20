use crate::{
    bsp::{Prop, World},
    kv,
    material::{self, Material},
    source_assets::SourceAssets,
    studio::{self, ModelMesh},
    vtf::{self, Texture},
};
use std::{
    collections::{BTreeSet, HashMap},
    io,
    path::Path,
};
pub type ModelKey = (String, usize, usize);
/// Pickup geometry uses the dropped model, which can differ from the model
/// attached to a survivor's hand. Key order in the script is not precedence.
pub fn pickup_model(script: &str) -> Option<String> {
    let tokens = kv::tokens(script);
    ["worldmodel", "playermodel"].into_iter().find_map(|key| {
        tokens
            .windows(2)
            .find(|v| v[0].eq_ignore_ascii_case(key))
            .and_then(|v| crate::source_assets::normalize(&v[1]).ok())
    })
}
/// Stable local choice for generic gun spawns. The native Director's weights
/// are not recovered; preserve the map's weapon-selection group instead.
pub fn pickup_class(
    e: &std::collections::BTreeMap<String, String>,
    index: usize,
) -> Option<String> {
    let class = e.get("classname")?;
    if class == "upgrade_spawn" {
        let mut choices = vec![];
        if e.get("laser_sight").is_some_and(|v| v == "1") {
            choices.push("upgrade_laser_sight");
        }
        if e.get("upgradepack_incendiary").is_some_and(|v| v == "1") {
            choices.push("weapon_upgradepack_incendiary");
        }
        if e.get("upgradepack_explosive").is_some_and(|v| v == "1") {
            choices.push("weapon_upgradepack_explosive");
        }
        return (!choices.is_empty()).then(|| choices[index % choices.len()].into());
    }
    if matches!(
        class.as_str(),
        "upgrade_laser_sight" | "upgrade_ammo_incendiary" | "upgrade_ammo_explosive"
    ) {
        return Some(class.clone());
    }
    if class.starts_with("prop_physics") {
        if let Some(model) = e.get("model") {
            match model.replace('\\', "/").to_ascii_lowercase().as_str() {
                "models/props_junk/gascan001a.mdl" => return Some("weapon_gascan".into()),
                "models/w_models/weapons/w_cola.mdl" => return Some("weapon_cola_bottles".into()),
                _ => {}
            }
        }
    }
    if class == "weapon_scavenge_item_spawn" || class == "weapon_scavenge_item" {
        return Some("weapon_gascan".into());
    }
    if class == "weapon_spawn" {
        let selection = e
            .get("weapon_selection")
            .map(String::as_str)
            .unwrap_or("any");
        let choices: &[&str] = match selection {
            "tier1_any" => &[
                "weapon_smg",
                "weapon_smg_silenced",
                "weapon_pumpshotgun",
                "weapon_shotgun_chrome",
            ],
            "tier2_any" => &[
                "weapon_autoshotgun",
                "weapon_shotgun_spas",
                "weapon_rifle",
                "weapon_rifle_ak47",
                "weapon_rifle_desert",
                "weapon_hunting_rifle",
                "weapon_sniper_military",
            ],
            "any_primary" => &[
                "weapon_smg",
                "weapon_smg_silenced",
                "weapon_pumpshotgun",
                "weapon_shotgun_chrome",
                "weapon_autoshotgun",
                "weapon_shotgun_spas",
                "weapon_rifle",
                "weapon_rifle_ak47",
                "weapon_rifle_desert",
                "weapon_hunting_rifle",
                "weapon_sniper_military",
            ],
            "tier1_shotgun" => &["weapon_pumpshotgun", "weapon_shotgun_chrome"],
            "any_smg" => &["weapon_smg", "weapon_smg_silenced"],
            "tier2_shotgun" => &["weapon_autoshotgun", "weapon_shotgun_spas"],
            "any_rifle" => &["weapon_rifle", "weapon_rifle_ak47", "weapon_rifle_desert"],
            "any_sniper_rifle" => &["weapon_hunting_rifle", "weapon_sniper_military"],
            "any_pistol" => &["weapon_pistol", "weapon_pistol_magnum"],
            "any" => &[
                "weapon_smg",
                "weapon_pumpshotgun",
                "weapon_rifle",
                "weapon_hunting_rifle",
            ],
            exact if exact.starts_with("weapon_") => return Some(exact.into()),
            _ => return None,
        };
        return Some(choices[index % choices.len()].into());
    }
    if class == "weapon_melee_spawn" || class == "weapon_melee" {
        let name = e.get("melee_weapon").map(String::as_str).unwrap_or("any");
        let name = if name == "any" {
            ["fireaxe", "crowbar", "frying_pan", "machete"][index % 4]
        } else {
            name.split(',').next()?.trim()
        };
        return Some(format!("weapon_melee_{name}"));
    }
    if class == "weapon_item_spawn" {
        // Like generic gun spawns, this is a stable provisional choice from
        // the map's allowed pool. Native Director density/weights are pending.
        let allowed: Vec<_> = [
            (1, "weapon_ammo"),
            (2, "weapon_first_aid_kit"),
            (3, "weapon_molotov"),
            (4, "weapon_pain_pills"),
            (5, "weapon_pipe_bomb"),
            (6, "weapon_oxygentank"),
            (7, "weapon_propanetank"),
            (8, "weapon_gascan"),
            (11, "weapon_adrenaline"),
            (12, "weapon_defibrillator"),
            (13, "weapon_vomitjar"),
            (16, "weapon_chainsaw"),
            (17, "weapon_grenade_launcher"),
            (18, "weapon_rifle_m60"),
        ]
        .into_iter()
        .filter(|(flag, _)| e.get(&format!("item{flag}")).is_some_and(|v| v == "1"))
        .map(|(_, class)| class)
        .collect();
        return (!allowed.is_empty()).then(|| allowed[index % allowed.len()].into());
    }
    class
        .starts_with("weapon_")
        .then(|| class.strip_suffix("_spawn").unwrap_or(class).to_owned())
}
#[cfg(test)]
mod pickup_tests {
    use super::*;
    #[test]
    fn dropped_model_has_priority_over_hand_model_and_normalizes_case() {
        assert_eq!(
            pickup_model(r#""playermodel" "models/held.mdl" "worldmodel" "Models/Dropped.MDL""#)
                .as_deref(),
            Some("models/dropped.mdl")
        );
        assert_eq!(
            pickup_model(r#""playermodel" "Models/Melee.MDL""#).as_deref(),
            Some("models/melee.mdl")
        );
    }
    #[test]
    fn installed_pickups_use_compact_native_idle_geometry() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.is_dir() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        for class in [
            "weapon_pistol",
            "weapon_pistol_magnum",
            "weapon_smg",
            "weapon_rifle",
            "weapon_first_aid_kit",
            "weapon_pain_pills",
            "weapon_melee_fireaxe",
            "weapon_melee_crowbar",
        ] {
            let script = class
                .strip_prefix("weapon_melee_")
                .map(|n| format!("scripts/melee/{n}.txt"))
                .unwrap_or_else(|| format!("scripts/{class}.txt"));
            let model =
                pickup_model(&String::from_utf8_lossy(&assets.read(&script).unwrap())).unwrap();
            let mut meshes = studio::load(&mut assets, &model, 0, 0).unwrap();
            let pose = studio::pose::sample_l4d2r(&mut assets, &model, &["idle"], 0.).unwrap();
            studio::pose::apply(&mut assets, &model, &mut meshes, &pose, false).unwrap();
            let mut lo = bevy::math::Vec3::splat(f32::INFINITY);
            let mut hi = bevy::math::Vec3::splat(f32::NEG_INFINITY);
            // The roof medkits are authored with a 90-degree pitch.
            let rotation = if class == "weapon_first_aid_kit" {
                bevy::math::Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2)
            } else {
                bevy::math::Quat::IDENTITY
            };
            for p in meshes.iter().flat_map(|d| &d.positions) {
                let p = rotation * bevy::math::Vec3::from_array(*p);
                assert!(p.is_finite());
                lo = lo.min(p);
                hi = hi.max(p);
            }
            assert!(lo.is_finite(), "Empty pickup: {class}");
            assert!(
                ((lo + hi) * 0.5).length() < 1.5,
                "Pickup displaced from origin: {class}"
            );
            if class == "weapon_first_aid_kit" {
                assert!(
                    (hi.y - lo.y) < 0.25,
                    "Medkit must lie flat: {lo:?} / {hi:?}"
                );
            }
        }
    }
    #[test]
    fn named_melee_spawn_preserves_native_weapon() {
        let e = kv::entities(r#"{"classname" "weapon_melee_spawn" "melee_weapon" "fireaxe" "model" "models/weapons/melee/w_fireaxe.mdl"}"#).remove(0);
        assert_eq!(pickup_class(&e, 0).as_deref(), Some("weapon_melee_fireaxe"));
    }
    #[test]
    fn generic_item_spawn_is_not_discarded_and_respects_disabled_choices() {
        let gascan = kv::entities(r#"{"classname" "weapon_scavenge_item_spawn"}"#).remove(0);
        assert_eq!(pickup_class(&gascan, 0).as_deref(), Some("weapon_gascan"));
        let e = kv::entities(
            r#"{"classname" "weapon_item_spawn" "item2" "0" "item4" "1" "item5" "1"}"#,
        )
        .remove(0);
        for index in 0..8 {
            assert!(matches!(
                pickup_class(&e, index).as_deref(),
                Some("weapon_pain_pills" | "weapon_pipe_bomb")
            ));
        }
        let e = kv::entities(r#"{"classname" "weapon_item_spawn" "item4" "0"}"#).remove(0);
        assert!(pickup_class(&e, 0).is_none());
    }
    #[test]
    fn generic_spawn_preserves_map_weapon_group() {
        let e = kv::entities(r#"{"classname" "weapon_spawn" "weapon_selection" "tier1_shotgun"}"#)
            .remove(0);
        for index in 0..8 {
            assert!(matches!(
                pickup_class(&e, index).as_deref(),
                Some("weapon_pumpshotgun" | "weapon_shotgun_chrome")
            ));
        }
        let e = kv::entities(r#"{"classname" "weapon_spawn" "weapon_selection" "unknown_group"}"#)
            .remove(0);
        assert!(pickup_class(&e, 0).is_none());
        for group in ["tier1_any", "tier2_any", "any_primary"] {
            let e = kv::entities(&format!(
                r#"{{"classname" "weapon_spawn" "weapon_selection" "{group}"}}"#
            ))
            .remove(0);
            for index in 0..16 {
                assert!(
                    pickup_class(&e, index).is_some(),
                    "Missing native selection group {group}"
                );
            }
        }
    }
}
pub struct SceneCubemap {
    /// Source units.
    pub origin: [f32; 3],
    /// Material-style name, "maps/<map>/c<x>_<y>_<z>" (lower case).
    pub name: String,
    pub map: crate::vtf::Cubemap,
}
/// Loads each env_cubemap's HDR capture from the pakfile.
fn load_cubemaps(assets: &mut SourceAssets, origins: &[[i32; 3]]) -> Vec<SceneCubemap> {
    let names = assets.pak_names();
    let mut out = vec![];
    for o in origins {
        let suffix = format!("/c{}_{}_{}.hdr.vtf", o[0], o[1], o[2]);
        let Some(path) = names.iter().find(|n| n.starts_with("materials/maps/") && n.ends_with(&suffix)) else { continue };
        let Ok(bytes) = assets.read(path) else { continue };
        match crate::vtf::decode_cubemap_hdr(&bytes) {
            Ok(map) => out.push(SceneCubemap {
                origin: o.map(|v| v as f32),
                name: path.trim_start_matches("materials/").trim_end_matches(".hdr.vtf").to_string(),
                map,
            }),
            Err(e) => eprintln!("Cubemap {path}: {e}"),
        }
    }
    out
}
pub struct Scene {
    pub world: World,
    pub models: HashMap<ModelKey, Vec<ModelMesh>>,
    pub materials: HashMap<String, Material>,
    pub textures: HashMap<String, Texture>,
    pub report: Vec<String>,
    pub loaded_props: usize,
    pub failed_props: usize,
    pub sky_textures: Option<[String; 6]>,
    /// The map's env_cubemap captures (HDR), by origin; material names
    /// "maps/<map>/c<x>_<y>_<z>" select one, "env_cubemap" the nearest.
    pub cubemaps: Vec<SceneCubemap>,
    pub prop_lighting: HashMap<usize, Vec<Vec<[f32; 4]>>>,
}
impl Scene {
    pub fn add_material(&mut self, assets: &mut SourceAssets, name:&str) -> io::Result<()> {
        if self.materials.contains_key(name) {return Ok(());}
        let mut material=material::load(assets,name)?;
        material::prepare_palette(assets,&mut material,&mut self.textures)?;
        for texture in material.base.iter().chain(material.base2.iter()).chain(material.normal.iter()).chain(material.bump.iter()).chain(material.cap_phong.iter().filter_map(|c| c.exponent_texture.as_ref())).chain(material.cap_phong.iter().filter_map(|c| c.lightwarp.as_ref())).chain(material.wound.iter().map(|w|&w.texture)).chain(material.infected_detail.iter()).chain(material.infected_burn.iter()) {
            if !self.textures.contains_key(texture) {self.textures.insert(texture.clone(),vtf::decode_world(&assets.read(texture)?,1024)?);}
        }
        self.materials.insert(name.into(),material);Ok(())
    }
    pub fn pose_model(
        &mut self,
        root: &Path,
        path: &str,
        preferences: &[&str],
        merge: Option<&studio::pose::Pose>,
        view: bool,
    ) -> io::Result<studio::pose::Pose> {
        self.add_models(root, &[path.into()])?;
        let mut assets = SourceAssets::new(root, vec![])?;
        let pose = if let Some(p) = merge {
            p.clone()
        } else {
            studio::pose::sample_l4d2r(&mut assets, path, preferences, 0.)?
        };
        let draws = self.models.get_mut(&(path.into(), 0, 0)).unwrap();
        studio::pose::apply(&mut assets, path, draws, &pose, view)?;
        println!("Native posed model: {path}; camera bone {:?}", pose.camera);
        Ok(pose)
    }
    /// Door replacements and break pieces use the same initial-pose path as map props.
    pub fn add_posed_props(&mut self, root: &Path, paths: &[String]) -> io::Result<()> {
        self.add_models(root,paths)?;
        let mut assets=SourceAssets::new(root,vec![])?;
        for path in paths.iter().collect::<BTreeSet<_>>() {
            self.models.insert((path.clone(),0,0),studio::load_posed_prop(&mut assets,path)?);
        }
        Ok(())
    }
    pub fn add_models(&mut self, root: &Path, paths: &[String]) -> io::Result<()> {
        let mut assets = SourceAssets::new(root, vec![])?;
        for path in paths {
            let key = (path.clone(), 0, 0);
            if self.models.contains_key(&key) {
                continue;
            }
            let draws = studio::load(&mut assets, path, 0, 0)?;
            for draw in &draws {
                if self.materials.contains_key(&draw.material) {
                    continue;
                }
                let mut material = material::load(&mut assets, &draw.material)?;
                material::prepare_palette(&mut assets, &mut material, &mut self.textures)?;
                for texture in material.base.iter().chain(material.base2.iter()).chain(material.normal.iter()).chain(material.bump.iter()).chain(material.cap_phong.iter().filter_map(|c| c.exponent_texture.as_ref())).chain(material.cap_phong.iter().filter_map(|c| c.lightwarp.as_ref())).chain(material.wound.iter().map(|w| &w.texture)).chain(material.infected_detail.iter()).chain(material.infected_burn.iter()) {
                    if !self.textures.contains_key(texture) {
                        self.textures
                            .insert(texture.clone(), vtf::decode_world(&assets.read(texture)?, 1024)?);
                    }
                }
                self.materials.insert(draw.material.clone(), material);
            }
            self.models.insert(key, draws);
        }
        Ok(())
    }
    pub fn load(root: &Path, mut world: World) -> io::Result<Self> {
        let mut assets = SourceAssets::new(root, std::mem::take(&mut world.pak))?;
        let mut pickup_diagnostics = vec![];
        for (index, e) in world.entities.iter().enumerate() {
            let Some(class) = e.get("classname") else {
                continue;
            };
            let Some(weapon) = pickup_class(e, index) else {
                continue;
            };
            let script = weapon
                .strip_prefix("weapon_melee_")
                .map(|name| format!("scripts/melee/{name}.txt"))
                .unwrap_or_else(|| format!("scripts/{weapon}.txt"));
            // Hammer can save an editor placeholder (even an ammo stack for a
            // medkit). Weapon scripts supply the actual runtime pickup model.
            let model = assets
                .read(&script)
                .ok()
                .and_then(|bytes| pickup_model(&String::from_utf8_lossy(&bytes)))
                .or_else(|| {
                    (weapon == "weapon_ammo")
                        .then(|| "models/props/terror/ammo_stack.mdl".into())
                        .or_else(|| {
                            (weapon == "upgrade_laser_sight")
                                .then(|| "models/w_models/weapons/w_laser_sights.mdl".into())
                        })
                })
                .or_else(|| match weapon.as_str() {
                    "upgrade_ammo_incendiary" => {
                        Some("models/props/terror/incendiary_ammo.mdl".into())
                    }
                    "upgrade_ammo_explosive" => {
                        Some("models/props/terror/exploding_ammo.mdl".into())
                    }
                    _ => None,
                })
                .or_else(|| {
                    e.get("model").and_then(|p| {
                        p.starts_with("models/")
                            .then(|| crate::source_assets::normalize(p).ok())
                            .flatten()
                    })
                });
            if let Some(model) = model {
                // BSP parsing already inserted the generic placeholder model.
                if class == "weapon_spawn" {
                    world.props.retain(|p| {
                        p.classname != *class || p.origin != kv::vector(e.get("origin"))
                    });
                }
                world.props.push(Prop {
                    entity_index: Some(index),
                    clusters: vec![],
                    static_index: None,
                    lighting_flags: 0,
                    lighting_origin: None,
                    ambient: world
                        .props
                        .iter()
                        .min_by(|a, b| {
                            let distance = |p: &Prop| {
                                (0..3)
                                    .map(|i| (p.origin[i] - kv::vector(e.get("origin"))[i]).powi(2))
                                    .sum::<f32>()
                            };
                            distance(a).total_cmp(&distance(b))
                        })
                        .map(|p| p.ambient)
                        .unwrap_or([[0.5; 3]; 6]),
                    sky: false,
                    model,
                    origin: kv::vector(e.get("origin")),
                    angles: kv::vector(e.get("angles")),
                    skin: 0,
                    body: 0,
                    tint: [255; 4],
                    classname: class.clone(),
                });
            } else {
                pickup_diagnostics.push(format!(
                    "PICKUP MODEL entity={index} class={weapon}: no model in {script}"
                ));
            }
        }
        let mut scene = Self {
            world,
            models: HashMap::new(),
            materials: HashMap::new(),
            textures: HashMap::new(),
            report: pickup_diagnostics,
            loaded_props: 0,
            failed_props: 0,
            sky_textures: None,
            prop_lighting: HashMap::new(),
            cubemaps: vec![],
        };
        scene.cubemaps = load_cubemaps(&mut assets, &scene.world.cubemaps);
        let pickup_keys: BTreeSet<_> = scene
            .world
            .props
            .iter()
            .filter(|p| p.classname.starts_with("weapon_"))
            .map(|p| (p.model.clone(), p.skin, p.body))
            .collect();
        // Every animating prop entity spawns in sequence zero, not its bind
        // pose; only prop_static draws the bind pose. Some map assets encode
        // placement corrections in that first sequence (Streets tanker,
        // Whitaker's delivery hatch, and the medical cabinet's 90-degree root,
        // models/props_interiors/medicalcabinet02.mdl `idle`).
        let posed_prop_keys: BTreeSet<_> = scene.world.props.iter()
            .filter(|p| p.classname != "prop_static" && !p.classname.starts_with("weapon_"))
            .map(|p| (p.model.clone(), p.skin, p.body)).collect();
        let initial_sequences: HashMap<ModelKey,String> = scene.world.props.iter().filter_map(|p| {
            let id=p.entity_index?;
            let name=scene.world.entities.get(id)?.iter().find(|(k,_)|k.eq_ignore_ascii_case("DefaultAnim"))?.1;
            (!name.is_empty()).then(||((p.model.clone(),p.skin,p.body),name.clone()))
        }).collect();
        let model_lod = crate::graphics_options::Graphics::default().model_lod();
        for p in &scene.world.props {
            let key = (p.model.clone(), p.skin, p.body);
            if !scene.models.contains_key(&key) {
                let lod = if p.classname == "prop_static" {
                    model_lod
                } else {
                    0
                };
                match studio::load_lod(&mut assets, &p.model, p.skin, p.body, lod) {
                    Ok(mut meshes) => {
                        if pickup_keys.contains(&key) || posed_prop_keys.contains(&key) {
                            let initial=initial_sequences.get(&key).map(String::as_str);
                            let preferences: &[&str] = if pickup_keys.contains(&key) { &["idle"] }
                                else if let Some(name)=initial { &[name] }
                                else if p.classname.starts_with("prop_door_rotating") { &["closed", "idle"] }
                                else { &[] };
                            match studio::pose::sample_l4d2r(&mut assets, &p.model, preferences, 0.)
                                .and_then(|pose| {
                                    studio::pose::apply(
                                        &mut assets,
                                        &p.model,
                                        &mut meshes,
                                        &pose,
                                        false,
                                    )
                                }) {
                                Ok(()) => {}
                                Err(e) => {
                                    scene.report.push(format!("INITIAL PROP POSE {}: {e}", p.model))
                                }
                            }
                        }
                        scene.models.insert(key.clone(), meshes);
                    }
                    Err(e) => {
                        scene.report.push(format!(
                            "MODEL {} skin={} body={}: {e}",
                            p.model, p.skin, p.body
                        ));
                        scene.models.insert(key.clone(), vec![]);
                    }
                }
            }
            if scene.models[&key].is_empty() {
                scene.failed_props += 1;
            } else {
                scene.loaded_props += 1;
            }
        }
        for (instance, p) in scene.world.props.iter().enumerate() {
            if p.lighting_flags & 0x40 != 0 {
                continue;
            }
            let Some(id) = p.static_index else {
                continue;
            };
            let filename = format!("sp_hdr_{id}.vhv");
            if !assets.exists(&filename) {
                continue;
            }
            let key = (p.model.clone(), p.skin, p.body);
            let draws = &scene.models[&key];
            if draws.is_empty() {
                continue;
            }
            let result =
                assets
                    .read(&filename)
                    .and_then(|b| crate::lighting::vhv_lod(&b, draws[0].checksum, draws[0].lod))
                    .and_then(|groups| {
                        let mut colors = vec![];
                        for draw in draws {
                            let mut output = vec![];
                            for &(group, index) in &draw.lighting_indices {
                                let rgb = groups.get(group).and_then(|g| g.get(index)).ok_or_else(
                                    || crate::binary::invalid("VHV/VTX vertex mapping mismatch"),
                                )?;
                                output.push([rgb[0], rgb[1], rgb[2], 1.]);
                            }
                            colors.push(output);
                        }
                        Ok(colors)
                    });
            match result {
                Ok(colors) => {
                    scene.prop_lighting.insert(instance, colors);
                }
                Err(e) => scene.report.push(format!("PROP LIGHTING {filename}: {e}")),
            }
        }
        let mut names: BTreeSet<String> = scene
            .world
            .surfaces
            .iter()
            .map(|s| s.material.clone())
            .collect();
        for meshes in scene.models.values() {
            names.extend(meshes.iter().map(|m| m.material.clone()));
        }
        for sun in scene.world.entities.iter().filter_map(crate::source_sun::read) {
            names.extend(sun.materials().cloned());
        }
        for name in names {
            let mut material = match material::load(&mut assets, &name) {
                Ok(m) => m,
                Err(e) => {
                    scene.report.push(format!("MATERIAL {name}: {e}"));
                    Material::default()
                }
            };
            if let Err(e) =
                material::prepare_palette(&mut assets, &mut material, &mut scene.textures)
            {
                scene.report.push(format!("INFECTED MATERIAL {name}: {e}"));
            }
            for path in material.base.iter().chain(material.base2.iter()).chain(material.blend_modulate.iter()).chain(material.normal.iter()).chain(material.bump.iter()).chain(material.cap_phong.iter().filter_map(|c| c.exponent_texture.as_ref())).chain(material.cap_phong.iter().filter_map(|c| c.lightwarp.as_ref())).chain(material.emission_mask.iter()).chain(material.wound.iter().map(|w| &w.texture)).chain(material.water.iter().flat_map(|w| w.textures())).chain(material.infected_detail.iter()).chain(material.infected_burn.iter()) {
                if scene.textures.contains_key(path) {
                    continue;
                }
                match assets.read(path).and_then(|b| vtf::decode_world(&b, 1024)) {
                    Ok(texture) => {
                        scene.textures.insert(path.clone(), texture);
                    }
                    Err(e) => scene.report.push(format!("TEXTURE {path}: {e}")),
                }
            }
            scene.materials.insert(name, material);
        }
        if let Some(name) = scene.world.entities.iter().find_map(|e| e.get("skyname")) {
            let mut paths: [String; 6] = std::array::from_fn(|_| String::new());
            let mut complete = true;
            for (i, suffix) in ["rt", "lf", "up", "dn", "bk", "ft"].iter().enumerate() {
                let key = format!("skybox/{name}{suffix}");
                let material = material::load(&mut assets, &key)?;
                let Some(path) = material.base.clone() else {
                    complete = false;
                    continue;
                };
                paths[i] = path.clone();
                scene.materials.insert(key, material);
                match assets.read(&path).and_then(|b| vtf::decode_world(&b, 1024)) {
                    Ok(t) => {
                        scene.textures.insert(path.clone(), t);
                    }
                    Err(e) => {
                        scene.report.push(format!("SKY TEXTURE {path}: {e}"));
                        complete = false;
                    }
                }
            }
            if complete {
                scene.sky_textures = Some(paths);
            }
        }
        Ok(scene)
    }
    pub fn summary(&self) -> String {
        format!(
            "{} world faces ({} triangles); {} material batches; {} textures; {}/{} prop instances loaded; {} brush entities; {} diagnostics",
            self.world.faces,
            self.world.positions.len() / 3,
            self.world.surfaces.len(),
            self.textures.len(),
            self.loaded_props,
            self.loaded_props + self.failed_props,
            self.world.brushes.len(),
            self.report.len()
        )
    }
    pub fn write_report(&self) -> io::Result<()> {
        let mut report = format!(
            "{}\n\nInitial placements only. Dynamic simulation, animations, particles and full Source shader parity remain pending. Displacement meshes are loaded; terrain material blending remains approximate.\n\n",
            self.summary()
        );
        let shaders: BTreeSet<_> = self.materials.values().map(|m| m.shader.as_str()).collect();
        report.push_str(&format!(
            "Source material shaders: {shaders:?}\n3D sky props: {}\n\n",
            self.world.props.iter().filter(|p| p.sky).count()
        ));
        report.push_str(&format!(
            "Native VHV lit prop instances: {}\n",
            self.prop_lighting.len()
        ));
        for line in &self.report {
            report.push_str(line);
            report.push('\n');
        }
        std::fs::write("map-load-report.txt", report)
    }
}

#[cfg(test)]
mod placement_tests {
    use super::*;
    #[test]
    fn streets_props_use_native_spawn_pose_instead_of_bind_geometry() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        for model in ["models/destruction_tanker/pre_destruction_tanker_trailer.mdl",
            "models/props_doors/gun_store_door.mdl", "models/props_vehicles/cara_95sedan.mdl",
            "models/props_vehicles/police_car_rural.mdl", "models/props_vehicles/cara_82hatchback_wrecked.mdl"] {
            let mut draws = studio::load(&mut assets, model, 0, 0).unwrap();
            let before = draws.iter().flat_map(|d| d.positions.iter().copied()).collect::<Vec<_>>();
            let pose = studio::pose::sample_l4d2r(&mut assets, model, &[], 0.).unwrap();
            studio::pose::apply(&mut assets, model, &mut draws, &pose, false).unwrap();
            let after = draws.iter().flat_map(|d| d.positions.iter().copied()).collect::<Vec<_>>();
            let bounds = |points: &[[f32;3]]| points.iter().fold(([f32::INFINITY;3],[f32::NEG_INFINITY;3]), |(lo,hi),p|
                (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i]))));
            println!("STREETS INITIAL POSE {model}: bind {:?}, posed {:?}", bounds(&before), bounds(&after));
            assert!(after.iter().flatten().all(|p| p.is_finite()));
            if model.contains("tanker") || model.contains("gun_store") {
                assert!(before.iter().zip(&after).any(|(a,b)| (0..3).any(|i| (a[i]-b[i]).abs() > 0.1)), "missing native pose correction {model}");
            }
        }
    }
}
