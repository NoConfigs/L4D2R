//! Owned PHY KeyValues and surface properties. Parsing is separate from solver parity.
use crate::{kv, source_assets::SourceAssets};
use bevy::prelude::*;
use std::{
    collections::{BTreeMap, HashMap},
    io,
};
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub friction: f32,
    pub elasticity: f32,
    pub density: f32,
}
impl Surface {
    /// Ordinary IVP pair material rule: direct getter products, clamped [0,1].
    /// Owned vphysics 10107730/10107760 and wrappers10092410/10092490.
    /// Vehicle/tire overrides are separate native branches.
    pub fn contact_with(self, other: Self) -> (f32, f32) {
        let coefficient = |a:f32,b:f32| {
            if a.is_finite()&&b.is_finite(){crate::ivp_material::clamped_product(a,b) as f32}else{0.}
        };
        (coefficient(self.friction, other.friction), coefficient(self.elasticity, other.elasticity))
    }
}
/// Velocity restitution for an IVP pair elasticity at a normal impact speed
/// (m/s). Measured with the owned vphysics.dll (tools/native_ivp_probe.cpp
/// --sphere, local-research/parity/ivp-contact-1019): IVP's elasticity is an
/// energy coefficient, so the rebound is sqrt(pair); a floor of 0.2 applies
/// at 1 m/s (39.37 in/s) and faster, and below that the floor is
/// 0.16845 - 0.0491*sqrt(pair) (fits every sample to 1e-5).
pub fn restitution(pair_elasticity: f32, impact_speed: f32) -> f32 {
    if !pair_elasticity.is_finite() || !impact_speed.is_finite() {
        return 0.;
    }
    let root = pair_elasticity.clamp(0., 1.).sqrt();
    let floor = if impact_speed >= 1. { 0.2 } else { 0.16845 - 0.0491 * root };
    root.max(floor)
}
impl Default for Surface {
    fn default() -> Self {
        Self {
            friction: 0.8,
            elasticity: 0.25,
            density: 2000.,
        }
    }
}
#[derive(Clone, Default)]
pub struct Surfaces(pub HashMap<String, Surface>);
impl Surfaces {
    pub fn load(assets: &mut SourceAssets) -> io::Result<Self> {
        let registry=crate::ivp_surface_registry::Registry::load(assets)?;
        Ok(Self(registry.records.iter().map(|r|{
            let p=registry.get(&r.name).unwrap().properties;
            (r.name.to_ascii_lowercase(),Surface{friction:p[0],elasticity:p[1],density:p[2]})
        }).collect()))
    }
    pub fn get(&self, name: &str) -> Surface {
        self.0
            .get(&name.to_ascii_lowercase())
            .or_else(|| self.0.get("default"))
            .cloned()
            .unwrap_or_default()
    }
}
#[derive(Clone, Debug)]
pub struct Solid {
    pub index: usize,
    pub name: String,
    pub parent: String,
    pub mass: f32,
    pub volume: Option<f32>,
    pub inertia: f32,
    pub damping: f32,
    pub angular_damping: f32,
    pub surface: String,
}
#[derive(Clone, Debug)]
pub struct Joint {
    pub parent: usize,
    pub child: usize,
    pub min: Vec3,
    pub max: Vec3,
    /// Authored zero-speed angular motor caps; native scales by reference mass.
    pub friction: Vec3,
}
#[derive(Clone, Default, Debug)]
pub struct Profile {
    pub geometry: Vec<l4d2_formats::phy::PhySolid>,
    /// Explicit collision rules enable only listed pairs; no block uses defaults.
    pub collision_pairs: Option<Vec<(usize, usize)>>,
    pub solids: Vec<Solid>,
    pub joints: Vec<Joint>,
}
pub fn text(bytes: &[u8]) -> io::Result<&str> {
    let int = |offset: usize| -> io::Result<usize> {
        let raw = bytes
            .get(offset..offset + 4)
            .ok_or_else(|| io::Error::other("truncated PHY"))?;
        Ok(u32::from_le_bytes(raw.try_into().unwrap()) as usize)
    };
    let mut offset = int(0)?;
    let count = int(8)?;
    if offset < 16 || count > 1024 {
        return Err(io::Error::other("invalid PHY header"));
    }
    for _ in 0..count {
        offset = offset
            .checked_add(4 + int(offset)?)
            .filter(|o| *o <= bytes.len())
            .ok_or_else(|| io::Error::other("invalid PHY solid length"))?;
    }
    let data = bytes
        .get(offset..)
        .ok_or_else(|| io::Error::other("invalid PHY text offset"))?;
    let end = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    std::str::from_utf8(&data[..end]).map_err(io::Error::other)
}
impl Profile {
    pub fn parse(text: &str) -> Self {
        let t = kv::tokens(text);
        let mut i = 0;
        let mut result = Self::default();
        while i + 1 < t.len() {
            let key = t[i].to_ascii_lowercase();
            i += 1;
            if t[i] != "{" {
                i += 1;
                continue;
            }
            i += 1;
            let mut depth = 1;
            let mut values = BTreeMap::new();
            if key == "collisionrules" {
                result.collision_pairs = Some(Vec::new());
            }
            while i < t.len() && depth > 0 {
                if t[i] == "{" {
                    depth += 1;
                    i += 1;
                } else if t[i] == "}" {
                    depth -= 1;
                    i += 1;
                } else if i + 1 < t.len() && t[i + 1] != "{" && t[i + 1] != "}" {
                    if depth == 1 {
                        if key == "collisionrules" && t[i].eq_ignore_ascii_case("collisionpair") {
                            if let Some((a, b)) = t[i + 1].split_once(',').and_then(|(a, b)| {
                                Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
                            }) {
                                result.collision_pairs.as_mut().unwrap().push((a, b));
                            }
                        }
                        values.insert(t[i].to_ascii_lowercase(), t[i + 1].clone());
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let number = |name: &str, default: f32| {
                values
                    .get(name)
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                    .unwrap_or(default)
            };
            let string = |name: &str, default: &str| {
                values.get(name).cloned().unwrap_or_else(|| default.into())
            };
            if key == "collisionrules" && values.get("selfcollisions").is_some_and(|v| v == "0") {
                result.collision_pairs.as_mut().unwrap().clear();
            }
            if key == "solid" {
                result.solids.push(Solid {
                    index: number("index", 0.) as usize,
                    name: string("name", ""),
                    parent: string("parent", ""),
                    mass: number("mass", 10.).max(0.01),
                    volume: values
                        .get("volume")
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite() && *v > 0.),
                    inertia: number("inertia", 1.).max(0.01),
                    damping: number("damping", 0.).max(0.),
                    angular_damping: number("rotdamping", 0.).max(0.),
                    surface: string("surfaceprop", "default"),
                });
            }
            if key == "ragdollconstraint" {
                result.joints.push(Joint {
                    parent: number("parent", 0.) as usize,
                    child: number("child", 0.) as usize,
                    min: Vec3::new(number("xmin", 0.), number("ymin", 0.), number("zmin", 0.))
                        * std::f32::consts::PI
                        / 180.,
                    max: Vec3::new(number("xmax", 0.), number("ymax", 0.), number("zmax", 0.))
                        * std::f32::consts::PI
                        / 180.,
                    friction: Vec3::new(number("xfriction", 0.), number("yfriction", 0.), number("zfriction", 0.)),
                });
            }
        }
        result
    }
    pub fn load(assets: &mut SourceAssets, model: &str) -> io::Result<Self> {
        fn find(
            assets: &mut SourceAssets,
            model: &str,
            seen: &mut Vec<String>,
        ) -> io::Result<Profile> {
            if seen.len() > 64 || seen.iter().any(|n| n == model) {
                return Err(io::Error::other("physics include cycle"));
            }
            seen.push(model.into());
            if let Ok(bytes) = assets.read(&model.replace(".mdl", ".phy")) {
                let mut profile = Profile::parse(text(&bytes)?);
                profile.geometry = l4d2_formats::phy::parse_phy(&bytes).unwrap_or_default();
                return Ok(profile);
            }
            let mdl = assets.read(model)?;
            for include in
                l4d2_formats::anim::include_model_names(&mdl).map_err(io::Error::other)?
            {
                if let Ok(profile) = find(assets, &include, seen) {
                    if !profile.solids.is_empty() {
                        return Ok(profile);
                    }
                }
            }
            Err(io::Error::other(format!(
                "no owned physics data for {model}"
            )))
        }
        find(assets, model, &mut Vec::new())
    }
}
pub fn inspect(root: &std::path::Path, model: &str) -> io::Result<()> {
    let mut assets = SourceAssets::new(root, vec![])?;
    let profile = Profile::load(&mut assets, model)?;
    println!("{profile:#?}");
    let surfaces = Surfaces::load(&mut assets)?;
    for name in [
        "default", "concrete", "wood", "metal", "flesh", "glass", "ice",
    ] {
        println!("SURFACE {name} {:?}", surfaces.get(name));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_bsp_world_geometry_matches_native_triangle_and_material_fingerprints(){
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");if !root.exists(){return;}
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-bsp-geometry.json")).unwrap();
        let mut triangle_count=0;let mut convex_count=0;
        for case in fixture["maps"].as_array().unwrap(){
            let name=case["map"].as_str().unwrap();let data=std::fs::read(root.join("left4dead2/maps").join(format!("{name}.bsp"))).unwrap();
            let bsp=l4d2_formats::bsp::Bsp::parse(data).unwrap();let models=bsp.physics_models().unwrap();let world=models.iter().find(|m|m.model_index==0).unwrap();
            assert_eq!(world.collision.len() as u64,case["collision_size"].as_u64().unwrap());
            let solids=world.geometry().unwrap();assert_eq!(solids.len(),world.solid_count);
            let mut packet=Vec::new();packet.extend((solids.len() as u32).to_le_bytes());let mut observations=Vec::new();
            for solid in &solids{packet.extend((solid.convexes.len() as u32).to_le_bytes());let mut fingerprints=Vec::new();
                for convex in &solid.convexes{
                    assert_eq!(convex.triangles.len(),convex.triangle_headers.len());
                    let mut body=Vec::new();body.extend((convex.triangles.len() as u32).to_le_bytes());let mut materials=[0u32;128];
                    for (triangle,header) in convex.triangles.iter().zip(&convex.triangle_headers){
                        body.extend(header.0.to_le_bytes());materials[header.material_index() as usize]+=1;
                        let mut points=triangle.map(|p|p.to_array().map(f32::to_bits));points.sort_unstable();
                        for point in points{for value in point{body.extend(value.to_le_bytes());}}triangle_count+=1;
                    }
                    let crc=crc32fast::hash(&body);packet.extend(body);
                    fingerprints.push((convex.triangles.len(),crc,materials));convex_count+=1;
                }observations.push(fingerprints);
            }
            // Raw geometry stays in local evidence, not the distributable fixture.
            // Final validation also compares this packet byte-for-byte with the
            // independent native packet, rather than relying on CRCs alone.
            std::fs::create_dir_all("local-research/parity").unwrap();
            std::fs::write(format!("local-research/parity/ivp-bsp-geometry-1024-source-{name}.packet"),&packet).unwrap();
            assert_eq!(packet.len() as u64,case["packet_bytes"].as_u64().unwrap());
            assert_eq!(observations.len(),case["solids"].as_array().unwrap().len());
            for (s,(actual,native)) in observations.iter().zip(case["solids"].as_array().unwrap()).enumerate(){
                assert_eq!(actual.len(),native.as_array().unwrap().len(),"{name} solid{s}");
                for (c,((count,crc,materials),expected)) in actual.iter().zip(native.as_array().unwrap()).enumerate(){
                    assert_eq!(*count as u64,expected["triangles"].as_u64().unwrap());
                    assert_eq!(*crc as u64,expected["crc32"].as_u64().unwrap(),"{name} solid{s} convex{c}");
                    let mut histogram=[0u32;128];for pair in expected["materials"].as_array().unwrap(){histogram[pair[0].as_u64().unwrap() as usize]=pair[1].as_u64().unwrap() as u32;}assert_eq!(*materials,histogram);
                }
            }
        }assert_eq!(triangle_count,92852);assert_eq!(convex_count,7496);
    }
    #[test]
    fn invalid_raw_collision_payloads_fail_without_synthesized_geometry(){
        assert!(l4d2_formats::phy::parse_collision(&[],1).is_err());
        assert!(l4d2_formats::phy::parse_collision(&[0;80],1).is_err());
        assert!(l4d2_formats::phy::parse_collision(&[],1025).is_err());
        assert!(l4d2_formats::phy::parse_collision(&[],0).unwrap().is_empty());
    }
    #[test]
    fn native_triangle_material_getters_and_setters_preserve_other_header_bits(){
        use l4d2_formats::phy::TriangleHeader;
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-triangle-material.json")).unwrap();
        let mut headers=0;let mut updates=0;
        for case in fixture["cases"].as_array().unwrap(){for solid in case["solids"].as_array().unwrap(){
            let mut convexes:Vec<Vec<TriangleHeader>>=solid["convexes"].as_array().unwrap().iter().map(|cv|cv.as_array().unwrap().iter().map(|t|{
                let header=TriangleHeader(t["header"].as_u64().unwrap() as u32);
                assert_eq!(header.material_index() as u64,t["material"].as_u64().unwrap());headers+=1;header
            }).collect()).collect();
            for update in solid["updates"].as_array().unwrap(){
                let header=&mut convexes[update["convex"].as_u64().unwrap() as usize][update["triangle"].as_u64().unwrap() as usize];
                assert_eq!(header.0 as u64,update["before"].as_u64().unwrap());
                header.set_material_index(update["input"].as_i64().unwrap() as i32);
                assert_eq!(header.0 as u64,update["after"].as_u64().unwrap());
                assert_eq!(header.material_index() as u64,update["material"].as_u64().unwrap());updates+=1;
            }
        }}
        assert_eq!(headers,3508);assert_eq!(updates,28672);
    }
    #[test]
    fn installed_phy_loader_retains_native_triangle_headers_and_seeded_materials(){
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists(){return;}
        let mut assets=SourceAssets::new(root,vec![]).unwrap();
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-triangle-material.json")).unwrap();
        fn integer(data:&[u8],p:usize)->i32{i32::from_le_bytes(data[p..p+4].try_into().unwrap())}
        let mut checked=0;
        for case in fixture["cases"].as_array().unwrap(){
            let mut data=assets.read(case["model"].as_str().unwrap()).unwrap();
            if case["variant"]=="seeded"{
                let mut index=0;let mut off=integer(&data,0) as usize;
                for _ in 0..integer(&data,8){let end=off+4+integer(&data,off) as usize;let surf=off+32;
                    let mut stack=vec![surf+integer(&data,surf+32) as usize];
                    while let Some(node)=stack.pop(){let right=integer(&data,node);
                        if right!=0{stack.push(node+right as usize);stack.push(node+28);}else{
                            let ledge=(node as i64+integer(&data,node+4) as i64) as usize;
                            let n=i16::from_le_bytes(data[ledge+12..ledge+14].try_into().unwrap());
                            for t in 0..n as usize{let p=ledge+16+t*16+3;data[p]=(data[p]&128)|(index%128);index=index.wrapping_add(1);}
                        }
                    }off=end;
                }
            }
            let parsed=l4d2_formats::phy::parse_phy(&data).unwrap();
            assert_eq!(parsed.len(),case["solids"].as_array().unwrap().len());
            for (solid,native) in parsed.iter().zip(case["solids"].as_array().unwrap()){
                let actual:Vec<Vec<u32>>=solid.convexes.iter().map(|cv|{
                    assert_eq!(cv.triangle_headers.len(),cv.triangles.len());checked+=cv.triangles.len();
                    cv.triangle_headers.iter().map(|h|h.0).collect()
                }).collect();
                let expected:Vec<Vec<u32>>=native["convexes"].as_array().unwrap().iter().map(|cv|{
                    cv.as_array().unwrap().iter().map(|t|t["header"].as_u64().unwrap() as u32).collect()
                }).collect();assert_eq!(actual,expected,"{} {}",case["model"],case["variant"]);
            }
        }assert_eq!(checked,3508);
    }
    #[test]
    fn ordinary_native_material_pairs_multiply_and_clamp_instead_of_sqrt_or_min() {
        let a = Surface { friction: 0.25, elasticity: 0.2, density: 1. };
        let b = Surface { friction: 0.8, elasticity: 0.5, density: 9000. };
        let (friction, bounce) = a.contact_with(b);
        assert!((friction - 0.2).abs() < 1e-6);
        assert!((bounce - 0.1).abs() < 1e-6);
        assert_eq!(a.contact_with(b), b.contact_with(a));
        let high = Surface { friction: 4., elasticity: 2., density: 1. };
        assert_eq!(high.contact_with(high), (1., 1.));
        let invalid = Surface { friction: f32::NAN, elasticity: -1., density: 1. };
        assert_eq!(invalid.contact_with(b), (0., 0.));
    }
    #[test]
    fn runtime_surface_pair_api_matches_native_stored_material_results(){
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-material.json")).unwrap();
        let f=|v:&serde_json::Value|v.as_f64().unwrap() as f32;let mut friction_cases=0;let mut elasticity_cases=0;
        for row in fixture["cases"].as_array().unwrap(){
            let a=Surface{friction:f(&row["coefficients"][0]),elasticity:f(&row["coefficients"][2]),density:2000.};
            let b=Surface{friction:f(&row["coefficients"][1]),elasticity:f(&row["coefficients"][3]),density:2000.};
            let (friction,elasticity)=a.contact_with(b);
            if !row["overridden"].as_bool().unwrap(){assert_eq!(friction.to_bits(),f(&row["friction"]).to_bits());friction_cases+=1;}
            assert_eq!(elasticity.to_bits(),f(&row["elasticity"]).to_bits());elasticity_cases+=1;
        }
        assert_eq!(friction_cases,691);assert_eq!(elasticity_cases,768);
    }
    #[test]
    fn runtime_loaded_retail_surfaces_match_native_core_properties(){
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");if !root.exists(){return;}
        let mut assets=SourceAssets::new(root,vec![]).unwrap();let loaded=Surfaces::load(&mut assets).unwrap();
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-surface-registry.json")).unwrap();
        let retail=fixture["captures"].as_array().unwrap().iter().find(|c|c["label"]=="retail").unwrap();let state=retail["snapshots"].as_array().unwrap().last().unwrap();
        assert_eq!(loaded.0.len(),105);
        for record in state["records"].as_array().unwrap(){let surface=loaded.get(record["name"].as_str().unwrap());
            for (i,value) in [surface.friction,surface.elasticity,surface.density].into_iter().enumerate(){assert_eq!(value.to_bits(),(record["properties"][i].as_f64().unwrap() as f32).to_bits());}
        }
        assert_eq!(Surface::default().density,2000.);
    }
    #[test]
    fn owned_profiles_are_readable_and_have_native_joint_data() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let paths = assets
            .matching_paths("models/")
            .into_iter()
            .filter(|p| p.ends_with(".phy") && (p.contains("infected") || p.contains("survivor")))
            .collect::<Vec<_>>();
        std::fs::write(
            "local-research/parity/native-physics-paths.txt",
            paths.join("\n"),
        )
        .unwrap();
        let p = Profile::load(&mut assets, "models/infected/common_male01.mdl").unwrap();
        std::fs::write(
            "local-research/parity/native-physics-profile.txt",
            format!("{p:#?}"),
        )
        .unwrap();
        assert!(p.solids.len() > 5 && p.joints.len() > 5);
        let mut mass_report =
            "solid,name,volume_source_units,volume_integrated_units,relative_error\n".to_string();
        for solid in &p.solids {
            let geometry = &p.geometry[solid.index];
            let physical = crate::mass_properties::from_convexes(
                &geometry.convexes,
                solid.mass,
                solid.inertia,
            )
            .expect("native solid must have valid volume inertia");
            let native = solid.volume.unwrap();
            let integrated = physical.volume / crate::collision::UNIT.powi(3);
            let error = (integrated - native).abs() / native;
            mass_report.push_str(&format!(
                "{},{},{},{},{}\n",
                solid.index, solid.name, native, integrated, error
            ));
            assert!(
                error < 0.01,
                "native volume mismatch for {}: {} vs {}",
                solid.name,
                integrated,
                native
            );
        }
        std::fs::write("local-research/parity/native-convex-mass.csv", mass_report).unwrap();
        let surfaces = Surfaces::load(&mut assets).unwrap();
        let report = format!(
            "{p:#?}\nwood {:?}\nmetal {:?}\nflesh {:?}\nconcrete {:?}\n",
            surfaces.get("wood"),
            surfaces.get("metal"),
            surfaces.get("flesh"),
            surfaces.get("concrete")
        );
        std::fs::write("local-research/parity/native-physics-profile.txt", report).unwrap();
    }
    #[test]
    fn repeated_phy_solids_and_joint_limits_are_preserved() {
        let p = Profile::parse(
            "solid { index 0 name pelvis mass 20 } solid { index 1 name arm mass 3 surfaceprop flesh } ragdollconstraint { parent 0 child 1 xmin -45 xmax 90 ymin -5 ymax 5 zmin 0 zmax 0 xfriction -5 yfriction 8 zfriction 9 }",
        );
        assert_eq!(p.solids.len(), 2);
        assert_eq!(p.solids[1].mass, 3.);
        assert!((p.joints[0].min.x + std::f32::consts::FRAC_PI_4).abs() < 0.0001);
        assert_eq!(p.joints[0].friction, Vec3::new(-5., 8., 9.));
    }
    #[test]
    fn collision_rules_preserve_all_enabled_pairs_and_explicit_disable() {
        let p = Profile::parse(
            "collisionrules { collisionpair 10,12 collisionpair 13,9 collisionpair 2,8 }",
        );
        assert_eq!(p.collision_pairs, Some(vec![(10, 12), (13, 9), (2, 8)]));
        assert!(
            Profile::parse("solid { index 0 }")
                .collision_pairs
                .is_none()
        );
        assert_eq!(
            Profile::parse("collisionrules { selfcollisions 0 }").collision_pairs,
            Some(vec![])
        );
    }
    #[test]
    fn ivp_pair_elasticity_rebounds_by_its_square_root() {
        // Owned vphysics.dll reference drop: floor elasticity 0.2, body 0.25.
        let floor = Surface { friction: 0.8, elasticity: 0.2, density: 2400. };
        let body = Surface { friction: 0.8, elasticity: 0.25, density: 900. };
        let e = restitution(body.contact_with(floor).1, 386.667 * 0.0254);
        assert!((386.667 * e - 86.461).abs() < 0.01, "first native rebound");
        assert!((86.872 * e - 19.425).abs() < 0.01, "second native rebound");
        assert_eq!(restitution(f32::NAN, 1.), 0.);
        assert_eq!(restitution(4., 1.), 1.);
        // Owned-DLL samples (ratio of rebound to impact speed).
        let slow = 10. * 0.0254;
        let fast = 60. * 0.0254;
        for (pair, speed, ratio) in [(0., fast, 0.19999), (0., slow, 0.16845), (0.01, slow, 0.16354),
            (0.02, slow, 0.16150), (0.025, slow, 0.16068), (0.028, slow, 0.16733), (0.2, slow, 0.44721)] {
            assert!((restitution(pair, speed) - ratio).abs() < 2e-4, "{pair} {speed}");
        }
        // The 1 m/s switch: 39.37 in/s slow, 39.5 in/s fast.
        assert!((restitution(0., 39.37 * 0.0254 - 1e-4) - 0.16845).abs() < 1e-4);
        assert!((restitution(0., 39.5 * 0.0254) - 0.2).abs() < 1e-4);
    }
}
