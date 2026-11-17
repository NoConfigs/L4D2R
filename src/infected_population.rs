//! Native NAV population names and owned weighted common/uncommon model tables.
use crate::{
    game::{Campaign, Enemy},
    game_state::{Match, Rules},
    source_assets::SourceAssets,
};
use bevy::prelude::*;
use l4d2_formats::nav::{NavMesh, parse_nav};
use l4d2r_game_core::random::UniformRandomStream;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};
pub const GALLERY: [&str; 5] = [
    "common_male_ceda",
    "common_male_clown",
    "common_male_mud",
    "common_male_riot",
    "common_male_roadcrew",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Common,
    Ceda,
    Clown,
    Mud,
    Riot,
    Roadcrew,
    Fallen,
    Jimmy,
}
impl Kind {
    pub fn model(path: &str) -> Self {
        if path.contains("_ceda") {
            Self::Ceda
        } else if path.contains("_clown") {
            Self::Clown
        } else if path.contains("_mud") {
            Self::Mud
        } else if path.contains("_riot") {
            Self::Riot
        } else if path.contains("_roadcrew") {
            Self::Roadcrew
        } else if path.contains("_fallen_survivor") {
            Self::Fallen
        } else if path.contains("_jimmy") {
            Self::Jimmy
        } else {
            Self::Common
        }
    }
    pub fn fireproof(self) -> bool {
        matches!(self, Self::Ceda | Self::Jimmy)
    }
    pub fn blocks_front(self, transform: &Transform, incoming: Vec3) -> bool {
        self == Self::Riot
            && (transform.rotation * Vec3::X).dot(-incoming.normalize_or_zero()) >= 0.
    }
}
pub struct Population {
    pub name: String,
    tables: BTreeMap<String, Vec<(String, u32)>>,
    nav: Option<NavMesh>,
    rng: UniformRandomStream,
    fallen_after: f32,
    pills_density: f32,
    pipe_density: f32,
}
#[derive(Component)]
pub struct CarriedLoot(pub Vec<String>);
impl Default for Population {
    fn default() -> Self {
        Self {
            name: "default".into(),
            tables: BTreeMap::from([(
                "default".into(),
                vec![("models/infected/common_male01.mdl".into(), 100)],
            )]),
            nav: None,
            rng: UniformRandomStream::new(731),
            fallen_after: 0.,
            pills_density: 6.48,
            pipe_density: 6.48,
        }
    }
}
impl Population {
    fn tables(text: &str) -> BTreeMap<String, Vec<(String, u32)>> {
        let tokens = crate::kv::tokens(text);
        let mut tables = BTreeMap::new();
        for i in 0..tokens.len().saturating_sub(2) {
            if tokens[i + 1] != "{" {
                continue;
            }
            let end = tokens[i + 2..]
                .iter()
                .position(|t| t == "}")
                .map_or(tokens.len(), |n| i + 2 + n);
            let entries = tokens[i + 2..end]
                .windows(2)
                .filter_map(|pair| {
                    let weight = pair[1].parse::<u32>().ok()?;
                    if weight == 0 || !pair[0].starts_with("common_") {
                        return None;
                    }
                    Some((
                        format!("models/infected/{}.mdl", pair[0].to_lowercase()),
                        weight,
                    ))
                })
                .collect::<Vec<_>>();
            if !entries.is_empty() && tokens[i] != "Population" {
                tables.insert(tokens[i].to_lowercase(), entries);
            }
        }
        tables
    }
    pub fn load(assets: &mut SourceAssets, map: &Path, original: bool) -> io::Result<Self> {
        let file = if original {
            "scripts/population_l4d1coop.txt"
        } else {
            "scripts/population.txt"
        };
        let bytes = assets.read(file)?;
        let tables = Self::tables(&String::from_utf8_lossy(&bytes));
        let nav = std::fs::read(map.with_extension("nav"))
            .or_else(|_| {
                assets.read(&format!(
                    "maps/{}.nav",
                    map.file_stem().and_then(|s| s.to_str()).unwrap_or("")
                ))
            })
            .ok()
            .and_then(|b| parse_nav(&b).ok());
        // This TerrorNavMesh string is the population set, despite its historical
        // map_name label in the parser. Use the data, never a campaign-wide guess.
        let mut name = nav
            .as_ref()
            .map_or("default".into(), |n| n.map_name.to_lowercase());
        let args = std::env::args().collect::<Vec<_>>();
        if let Some(value) = args.windows(2).find(|a| a[0] == "--capture-population") {
            name = value[1].to_lowercase();
        }
        if !tables.contains_key(&name) {
            name = "default".into();
        }
        println!("Native infected population: {name} ({file})");
        Ok(Self {
            name,
            tables,
            nav,
            rng: UniformRandomStream::new(731),
            fallen_after: 0.,
            pills_density: 6.48,
            pipe_density: 6.48,
        })
    }
    fn table_at(&self, position: Vec3, mob: bool) -> &[(String, u32)] {
        let place = self.nav.as_ref().and_then(|n| {
            let p = [
                position.x / 0.0254,
                -position.z / 0.0254,
                position.y / 0.0254,
            ];
            n.area_at(p)
                .and_then(|i| n.place_name(&n.areas[i]))
                .map(str::to_lowercase)
        });
        let name = place
            .as_deref()
            .filter(|p| self.tables.contains_key(*p))
            .unwrap_or(&self.name);
        let mob_name = format!("{name}_mob");
        self.tables
            .get(if mob && self.tables.contains_key(&mob_name) {
                mob_name.as_str()
            } else {
                name
            })
            .or_else(|| self.tables.get("default"))
            .map_or(&[], Vec::as_slice)
    }
    pub fn choose(&mut self, position: Vec3, mob: bool) -> String {
        self.choose_allowed(position, mob, true)
    }
    pub fn choose_allowed(&mut self, position: Vec3, mob: bool, fallen_allowed: bool) -> String {
        let total = self
            .table_at(position, mob)
            .iter()
            .filter(|(m, _)| fallen_allowed || Kind::model(m) != Kind::Fallen)
            .map(|(_, w)| *w)
            .sum::<u32>();
        if total == 0 {
            return "models/infected/common_male01.mdl".into();
        }
        let mut pick = self.rng.random_int(0, total as i32 - 1) as u32;
        for (model, weight) in self.table_at(position, mob) {
            if !fallen_allowed && Kind::model(model) == Kind::Fallen {
                continue;
            }
            if pick < *weight {
                return model.clone();
            }
            pick -= weight;
        }
        unreachable!()
    }
    pub fn models(&self, lab: bool) -> Vec<String> {
        let mut names = BTreeSet::from([self.name.clone(), format!("{}_mob", self.name)]);
        if let Some(nav) = &self.nav {
            names.extend(nav.place_names.iter().map(|s| s.to_lowercase()));
            names.extend(
                nav.place_names
                    .iter()
                    .map(|s| format!("{}_mob", s.to_lowercase())),
            );
        }
        let mut models = BTreeSet::new();
        for name in names {
            if let Some(table) = self.tables.get(&name) {
                models.extend(table.iter().map(|(m, _)| m.clone()));
            }
        }
        if lab {
            models.extend(GALLERY.map(|s| format!("models/infected/{s}.mdl")));
            models.insert("models/infected/common_male_fallen_survivor.mdl".into());
            models.insert("models/infected/common_male_jimmy.mdl".into());
        }
        models.into_iter().collect()
    }
    pub fn loot(&mut self, model: &str) -> Option<String> {
        let (probability, item) = match Kind::model(model) {
            Kind::Ceda => (0.10, "weapon_vomitjar"),
            Kind::Riot => (0.15, "weapon_melee_tonfa"),
            _ => return None,
        };
        (self.rng.random_float(0., 1.) < probability).then(|| item.into())
    }
    pub fn health(&self, model: &str) -> f32 {
        if matches!(Kind::model(model), Kind::Fallen | Kind::Jimmy) {
            50. * 20.
        } else {
            50.
        }
    }
    pub fn restart(&mut self) {
        self.fallen_after = 0.;
    }
    pub fn fallen_ready(&self, elapsed: f32) -> bool {
        elapsed >= self.fallen_after
    }
    fn fallen_killed(&mut self, elapsed: f32) {
        self.fallen_after = elapsed + 300.;
    }
    pub fn configure_fallen(&mut self, entities: &[BTreeMap<String, String>]) {
        for e in entities.iter().filter(|e| {
            e.get("classname")
                .is_some_and(|c| c == "info_map_parameters")
        }) {
            for (k, v) in e {
                let Some(value) = v.parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.)
                else {
                    continue;
                };
                if k.eq_ignore_ascii_case("PainPillDensity") {
                    self.pills_density = value;
                }
                if k.eq_ignore_ascii_case("PipeBombDensity") {
                    self.pipe_density = value;
                }
            }
        }
    }
    pub fn fallen_items(&mut self, model: &str) -> Vec<String> {
        self.fallen_items_allowed(model, |_| true)
    }
    pub fn fallen_items_allowed(
        &mut self,
        model: &str,
        mut allow: impl FnMut(&str) -> bool,
    ) -> Vec<String> {
        if Kind::model(model) != Kind::Fallen {
            return vec![];
        }
        // Owned 10402450/10403c80, item table 107aeae8, weapon-name table
        // 107a3f50: molotov flag, pipe/pills/kit density rolls in [0,3].
        let mut out = vec![];
        if allow("weapon_molotov") {
            out.push("weapon_molotov".into());
        }
        for (class, density) in [
            ("weapon_pipe_bomb", self.pipe_density),
            ("weapon_pain_pills", self.pills_density),
            ("weapon_first_aid_kit", self.pills_density),
        ] {
            if allow(class) && self.rng.random_float(0., 3.) < density {
                out.push(class.into());
            }
        }
        out
    }
}
pub fn death_loot(
    mut commands: Commands,
    state: Res<Match>,
    rules: Res<Rules>,
    collision: Res<crate::collision::Collision>,
    models: Res<crate::viewer::RenderModels>,
    meshes: Res<Assets<Mesh>>,
    mut enemies: Query<(&Transform, &mut Enemy, Option<&CarriedLoot>)>,
    mut campaign: Option<ResMut<Campaign>>,
) {
    if state.paused {
        return;
    }
    for (transform, mut enemy, carried) in &mut enemies {
        if enemy.health > 0. || enemy.loot_dropped {
            continue;
        }
        enemy.loot_dropped = true;
        if Kind::model(&enemy.model) == Kind::Fallen {
            if let Some(campaign) = campaign.as_mut() {
                campaign.population.fallen_killed(state.elapsed);
            }
        }
        let mut drops = carried.map_or(vec![], |c| c.0.clone());
        if let Some(class) = enemy.death_loot.take() {
            drops.push(class);
        }
        for (i, class) in drops.iter().enumerate() {
            crate::loot::drop_item(
                &mut commands,
                Some(&models),
                Some(&meshes),
                &rules,
                &collision,
                class,
                transform.translation + Vec3::Y * 0.2 + Vec3::X * (i as f32 * 0.15 - 0.2),
                None,
            );
        }
    }
}
/// U in the laboratory stages real animated uncommons; capture flag does so once.
pub fn gallery(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<Match>,
    session: Res<crate::campaign_settings::SessionSettings>,
    models: Res<crate::viewer::RenderModels>,
    collision: Res<crate::collision::Collision>,
    mut campaign: ResMut<Campaign>,
    mut captured: Local<bool>,
) {
    if !crate::test_lab::active(&session.map) || state.paused {
        return;
    }
    let automatic = std::env::args().any(|a| a == "--capture-uncommon");
    if !keys.just_pressed(KeyCode::KeyU) && !(automatic && !*captured) {
        return;
    }
    *captured = true;
    let fallen = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let jimmy = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let names = if fallen {
        vec!["common_male_fallen_survivor"]
    } else if jimmy {
        vec!["common_male_jimmy"]
    } else {
        GALLERY.to_vec()
    };
    for (i, name) in names.iter().enumerate() {
        let model = format!("models/infected/{name}.mdl");
        let Some(draws) = models.0.get(&(model.clone(), 0, 0)) else {
            continue;
        };
        let position = Vec3::from_array(crate::binary::source_position([
            1850. + i as f32 * 150.,
            2400.,
            4.,
        ]));
        if !collision.clear_hull(position, crate::collision::HEIGHT) {
            continue;
        }
        let root = commands
            .spawn((
                Transform::from_translation(position)
                    .with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
                Visibility::Inherited,
                Enemy {
                    death_loot: campaign.population.loot(&model),
                    model,
                    loot_dropped: false,
                    death: Default::default(),
                    movement_speed: 0.,
                    reaction_serial: 0,
                    attack_serial: 0,
                    health: if automatic {
                        2000.
                    } else {
                        campaign
                            .population
                            .health(&format!("models/infected/{name}.mdl"))
                    },
                    attack: 0.,
                    reaction: 0,
                    corpse_time: 0.,
                    burning: 0.,
                    vertical: 0.,
                },
            ))
            .id();
        commands.entity(root).insert(CarriedLoot(
            campaign
                .population
                .fallen_items(&format!("models/infected/{name}.mdl")),
        ));
        if automatic {
            commands.entity(root).insert(crate::test_lab::ImpactTarget);
        }
        for (mesh, material) in draws {
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                crate::viewer::DynamicModelLighting::default(),
                Transform::default(),
                bevy::camera::visibility::RenderLayers::layer(1),
                ChildOf(root),
            ));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallen_health_inventory_density_and_kill_cooldown_follow_recovered_rules() {
        let mut p = Population::default();
        let model = "models/infected/common_male_fallen_survivor.mdl";
        assert_eq!(p.health(model), 1000.);
        assert_eq!(p.health("models/infected/common_male_jimmy.mdl"), 1000.);
        let items = p.fallen_items(model);
        assert_eq!(items.len(), 4);
        assert!(items.contains(&"weapon_first_aid_kit".into()));
        assert!(p.fallen_ready(0.));
        p.fallen_killed(10.);
        assert!(!p.fallen_ready(309.9));
        assert!(p.fallen_ready(310.));
        p.restart();
        assert!(p.fallen_ready(0.));
        p.pills_density = 0.;
        p.pipe_density = 0.;
        assert_eq!(p.fallen_items(model), vec!["weapon_molotov"]);
        p.tables.insert(
            "default".into(),
            vec![
                (model.into(), 99),
                ("models/infected/common_male01.mdl".into(), 1),
            ],
        );
        for _ in 0..100 {
            assert_eq!(
                p.choose_allowed(Vec3::ZERO, false, false),
                "models/infected/common_male01.mdl"
            );
        }
        let mut baseline = Population::default();
        let mut denied = Population::default();
        assert!(denied.fallen_items_allowed(model, |_| false).is_empty());
        assert_eq!(
            baseline.rng.random_float(0., 1.),
            denied.rng.random_float(0., 1.),
            "Denied loot must not consume density RNG draws"
        );
    }
    #[test]
    fn native_nav_selects_chapter_populations_without_cross_campaign_uncommons() {
        let root = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        for (map, name, uncommon) in [
            ("c1m1_hotel", "c1streets", Some("ceda")),
            ("c1m3_mall", "default", None),
            ("c2m1_highway", "default", None),
            ("c2m2_fairgrounds", "whisperingoaks", Some("clown")),
            ("c3m2_swamp", "deepswamp", Some("mud")),
            ("c4m1_milltown_a", "defaultmilltown", Some("roadcrew")),
            ("c5m1_waterfront", "crescentcity", Some("riot")),
            ("c5m3_cemetery", "defaultcemetery", None),
        ] {
            let mut p = Population::load(
                &mut assets,
                &root.join(format!("left4dead2/maps/{map}.bsp")),
                false,
            )
            .unwrap();
            assert_eq!(p.name, name);
            let allowed = p
                .table_at(Vec3::ZERO, false)
                .iter()
                .map(|(m, _)| m.clone())
                .collect::<BTreeSet<_>>();
            if let Some(kind) = uncommon {
                assert!(allowed.iter().any(|m| m.contains(kind)));
            } else {
                assert!(allowed.iter().all(|m| Kind::model(m) == Kind::Common));
            }
            let mut seen = BTreeSet::new();
            for _ in 0..2000 {
                let model = p.choose(Vec3::ZERO, false);
                assert!(allowed.contains(&model));
                seen.insert(model);
            }
            assert!(seen.len() > 1);
        }
    }
    #[test]
    fn weighted_mob_tables_and_native_loot_probabilities_are_preserved() {
        let root = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let p = Population::load(
            &mut assets,
            &root.join("left4dead2/maps/c4m1_milltown_a.bsp"),
            false,
        )
        .unwrap();
        assert_eq!(
            p.table_at(Vec3::ZERO, false)
                .iter()
                .find(|(m, _)| m.contains("roadcrew"))
                .unwrap()
                .1,
            1
        );
        assert_eq!(
            p.table_at(Vec3::ZERO, true)
                .iter()
                .find(|(m, _)| m.contains("roadcrew"))
                .unwrap()
                .1,
            10
        );
        let mut p = Population::default();
        let mut count = 0;
        for _ in 0..10000 {
            count += usize::from(p.loot("models/infected/common_male_ceda.mdl").is_some());
        }
        assert!((850..1150).contains(&count));
        assert!(p.loot("models/infected/common_male_clown.mdl").is_none());
        assert!(Kind::model("models/infected/common_male_ceda_l4d1.mdl").fireproof());
        assert!(!Kind::Riot.fireproof());
        assert!(Kind::Riot.blocks_front(&Transform::default(), Vec3::NEG_X));
        assert!(!Kind::Riot.blocks_front(&Transform::default(), Vec3::X));
    }
    #[test]
    fn uncommon_death_drops_one_supported_pickup_only_once() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let pistol = crate::game_state::tests_weapon();
        let mut state = Match::new(&pistol);
        state.paused = false;
        world.insert_resource(state);
        world.insert_resource(Rules {
            pistol,
            weapons: Default::default(),
        });
        world.insert_resource(crate::collision::Collision::default());
        world.insert_resource(crate::viewer::RenderModels(Default::default()));
        world.insert_resource(Assets::<Mesh>::default());
        world.spawn((
            Transform::default(),
            Enemy {
                model: "models/infected/common_male_ceda.mdl".into(),
                death_loot: Some("weapon_vomitjar".into()),
                loot_dropped: false,
                death: Default::default(),
                movement_speed: 0.,
                reaction_serial: 0,
                attack_serial: 0,
                health: 0.,
                attack: 0.,
                reaction: 0,
                corpse_time: 12.,
                burning: 0.,
                vertical: 0.,
            },
        ));
        world.run_system_once(death_loot).unwrap();
        world.run_system_once(death_loot).unwrap();
        let pickups = world
            .query::<&crate::game::Pickup>()
            .iter(&world)
            .collect::<Vec<_>>();
        assert_eq!(pickups.len(), 1);
        assert_eq!(pickups[0].class, "weapon_vomitjar");
        drop(pickups);
        let actor = world
            .query::<(Entity, &Enemy)>()
            .iter(&world)
            .next()
            .unwrap()
            .0;
        {
            let mut enemy = world.get_mut::<Enemy>(actor).unwrap();
            enemy.model = "models/infected/common_male_fallen_survivor.mdl".into();
            enemy.loot_dropped = false;
        }
        let items =
            Population::default().fallen_items("models/infected/common_male_fallen_survivor.mdl");
        world.entity_mut(actor).insert(CarriedLoot(items));
        world.run_system_once(death_loot).unwrap();
        world.run_system_once(death_loot).unwrap();
        let classes = world
            .query::<&crate::game::Pickup>()
            .iter(&world)
            .map(|p| p.class.clone())
            .collect::<Vec<_>>();
        assert_eq!(classes.len(), 5);
        for class in [
            "weapon_molotov",
            "weapon_pipe_bomb",
            "weapon_pain_pills",
            "weapon_first_aid_kit",
        ] {
            assert_eq!(classes.iter().filter(|s| s.as_str() == class).count(), 1);
        }
    }
    #[test]
    fn every_required_native_population_model_and_uncommon_skeleton_loads() {
        let root = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = std::cell::RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let read = |p: &str| assets.borrow_mut().read(p).ok();
        let mut report = String::new();
        for model in GALLERY {
            let path = format!("models/infected/{model}.mdl");
            let sk = l4d2_formats::anim::Skeleton::load_with(&path, &read).unwrap();
            if model.contains("ceda") || model.contains("clown") {
                let body = if model.contains("ceda") {
                    "cim_ceda_full"
                } else {
                    "cim_clown"
                };
                let material = crate::material::load(
                    &mut assets.borrow_mut(),
                    &format!("models/infected/common/l4d2/{body}"),
                )
                .unwrap();
                assert!(
                    material
                        .palette
                        .unwrap()
                        .0
                        .contains(if model.contains("ceda") {
                            "palette_ceda"
                        } else {
                            "palette_clown"
                        })
                );
            }
            for seq in sk.sequences() {
                if model.contains("mud") || seq.activity_name.contains("CRAWL") {
                    report += &format!("{model}\t{}\t{}\n", seq.label, seq.activity_name);
                }
            }
            assert!(
                sk.sequences()
                    .iter()
                    .any(|s| s.activity_name == "ACT_TERROR_RUN_INTENSE")
            );
            assert!(
                !crate::studio::load(&mut assets.borrow_mut(), &path, 0, 0)
                    .unwrap()
                    .is_empty()
            );
            let seq = sk
                .find_sequence(if Kind::model(&path) == Kind::Mud {
                    "ACT_TERROR_CRAWL_RUN"
                } else {
                    "ACT_TERROR_RUN_INTENSE"
                })
                .unwrap();
            for time in [0., 0.2, 0.6] {
                assert!(
                    sk.pose_sequence(seq, time, true)
                        .iter()
                        .all(|m| m.is_finite())
                );
            }
            let root_pose = Transform::from_translation(Vec3::Y * 2.);
            let pose = sk.pose_sequence(seq, 0.2, false);
            let mut body = crate::ragdoll::Body::new(&sk, &pose, &root_pose, Vec3::ZERO);
            let profile = crate::physics_data::Profile::load(&mut assets.borrow_mut(), &path);
            if let Ok(profile) = profile {
                let surfaces =
                    crate::physics_data::Surfaces::load(&mut assets.borrow_mut()).unwrap();
                body = body.with_profile(&sk, &profile, &surfaces);
            }
            let floor = crate::collision::Collision::test_solid_floor();
            for _ in 0..180 {
                body.tick(1. / 60., &floor);
            }
            let pose = body.pose(&root_pose);
            let highest = pose
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.w_axis.z.total_cmp(&b.w_axis.z))
                .unwrap();
            assert!(
                pose.iter().all(|m| m.is_finite()),
                "{model} ragdoll invalid"
            );
            assert!(
                pose.iter()
                    .map(|m| m.w_axis.z * 0.0254 + 2.)
                    .fold(f32::NEG_INFINITY, f32::max)
                    < 2.5,
                "{model} ragdoll remained airborne: bone {} Y {}",
                sk.bones[highest.0].name,
                highest.1.w_axis.z * 0.0254 + 2.
            );
        }
        std::fs::write(
            "local-research/parity/native-uncommon-sequences.tsv",
            report,
        )
        .unwrap();
    }
}
