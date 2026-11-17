//! Campaign mode validation over the same BSP/entity initialization as play.
use crate::bsp::MapMode;
use l4d2r_game_core::{
    entities::{EntityWorld, Kind},
    mapents,
};
use std::{io, path::Path, sync::Arc};

pub fn verify(root: &Path) -> io::Result<()> {
    let catalog = crate::campaign_settings::Catalog::load(root)?;
    let mut report =
        String::from("map\tmode\tdormant_models\tdormant_pickups\tactive_nav_blockers\n");
    let mut count = 0;
    for chapter in catalog.campaigns.iter().flat_map(|c| &c.chapters) {
        let bsp = Arc::new(
            l4d2_formats::bsp::Bsp::parse(crate::bsp::load_bytes_for_mode(
                &chapter.map,
                root,
                MapMode::Campaign,
            )?)
            .map_err(|e| io::Error::other(e.to_string()))?,
        );
        let mut world = EntityWorld::new(bsp.clone(), mapents::collect(&bsp), 1);
        world.map_spawn(MapMode::Campaign.output());
        // Include ordinary delayed startup removals without simulating players.
        world.tick(1.);
        let models = world
            .ents
            .iter()
            .filter(|e| !e.alive && e.me.mdl.is_some())
            .count();
        let pickups = world
            .ents
            .iter()
            .filter(|e| !e.alive && e.me.classname.starts_with("weapon_"))
            .count();
        let blockers = world
            .ents
            .iter()
            .filter(|e| e.alive && matches!(e.kind, Kind::NavBlocker { blocked: true }))
            .count();
        report += &format!(
            "{}\tcoop\t{models}\t{pickups}\t{blockers}\n",
            chapter.map.display()
        );
        count += 1;
    }
    std::fs::create_dir_all("local-research")?;
    std::fs::write("local-research/campaign-mode-audit.tsv", report)?;
    println!(
        "Campaign mode initialization audit passed: {count} chapters (coop patches/outputs, template and blocker states recorded)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(text: &str) -> EntityWorld {
        let mut bytes = vec![0; 1036];
        bytes[..4].copy_from_slice(b"VBSP");
        bytes[4..8].copy_from_slice(&21i32.to_le_bytes());
        let mut bsp = l4d2_formats::bsp::Bsp::parse(bytes).unwrap();
        let tokens = crate::kv::tokens(text);
        let mut current = Vec::new();
        let mut at = 0;
        while at < tokens.len() {
            match tokens[at].as_str() {
                "{" => {
                    current.clear();
                    at += 1;
                }
                "}" => {
                    bsp.entities
                        .push(l4d2_formats::bsp::Entity(std::mem::take(&mut current)));
                    at += 1;
                }
                _ => {
                    current.push((tokens[at].clone(), tokens[at + 1].clone()));
                    at += 2;
                }
            }
        }
        let bsp = Arc::new(bsp);
        EntityWorld::new(bsp.clone(), mapents::collect(&bsp), 1)
    }
    const MAP: &str = r#"
    {"classname" "info_gamemode" "OnCoop" "mode_nav,UnblockNav,,0,-1" "OnCoopPostIO" "coop_template,ForceSpawn,,0,-1" "OnSurvivalPostIO" "survival_template,ForceSpawn,,0,-1"}
    {"classname" "point_template" "targetname" "survival_template" "spawnflags" "2" "Template01" "survival_*"}
    {"classname" "point_template" "targetname" "coop_template" "spawnflags" "2" "Template01" "coop_item"}
    {"classname" "prop_dynamic" "targetname" "survival_blocker" "origin" "100 100 100" "model" "models/test.mdl"}
    {"classname" "weapon_first_aid_kit_spawn" "targetname" "survival_kits"}
    {"classname" "logic_auto" "targetname" "survival_auto" "OnMapSpawn" "campaign_door,Kill,,0,-1"}
    {"classname" "weapon_first_aid_kit_spawn" "targetname" "coop_item"}
    {"classname" "func_nav_blocker" "targetname" "mode_nav"}
    {"classname" "prop_dynamic" "targetname" "campaign_door" "origin" "100 100 100" "model" "models/test.mdl"}
    "#;
    #[test]
    fn coop_templates_post_io_and_blockers_do_not_activate_survival_entities() {
        let mut w = fixture(MAP);
        w.map_spawn(MapMode::Campaign.output());
        for name in ["survival_blocker", "survival_kits", "survival_auto"] {
            assert!(!w.ents[w.find(name)[0]].alive, "{name}");
        }
        assert!(
            w.ents[w.find("campaign_door")[0]].alive,
            "Dormant autos must not run"
        );
        assert!(
            w.ents[w.find("coop_item")[0]].alive,
            "CoopPostIO must initialize the campaign template"
        );
        assert!(matches!(
            w.ents[w.find("mode_nav")[0]].kind,
            Kind::NavBlocker { blocked: false }
        ));
        let mut survival = fixture(MAP);
        survival.map_spawn(MapMode::Survival.output());
        assert!(survival.ents[survival.find("survival_blocker")[0]].alive);
        assert!(!survival.ents[survival.find("coop_item")[0]].alive);
    }
    #[test]
    fn template_dont_remove_and_kill_hierarchy_preserve_real_entity_semantics() {
        let mut w = fixture(
            r#"{"classname" "point_template" "spawnflags" "3" "Template01" "shared"} {"classname" "prop_dynamic" "targetname" "shared"} {"classname" "prop_dynamic" "targetname" "child" "parentname" "shared"}"#,
        );
        let parent = w.find("shared")[0];
        assert!(w.ents[parent].alive);
        w.input(parent, "KillHierarchy", "", None, None);
        assert!(!w.ents[w.find("child")[0]].alive);
    }
    #[test]
    fn installed_dead_center_campaign_and_holdout_patches_are_distinct() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        let load = |mode| {
            let b = Arc::new(
                l4d2_formats::bsp::Bsp::parse(
                    crate::bsp::load_bytes_for_mode(&path, root, mode).unwrap(),
                )
                .unwrap(),
            );
            let mut w = EntityWorld::new(b.clone(), mapents::collect(&b), 1);
            w.map_spawn(mode.output());
            w
        };
        let coop = load(MapMode::Campaign);
        let holdout = load(MapMode::Survival);
        assert!(
            coop.find("ptemplate_survival").is_empty(),
            "Holdout-only patch must never be applied in coop"
        );
        assert!(!holdout.find("ptemplate_survival").is_empty());
        let ids = holdout.find("survival_props");
        assert_eq!(ids.len(), 2);
        assert!(
            ids.iter().all(|id| holdout.ents[*id].alive),
            "OnSurvivalPostIO must spawn only the holdout version"
        );
        for name in ["c2m1_highway", "c8m5_rooftop", "c5m1_waterfront"] {
            let path = crate::campaign_settings::find_map(root, name).unwrap();
            let b = Arc::new(
                l4d2_formats::bsp::Bsp::parse(crate::bsp::load_bytes(&path, root).unwrap())
                    .unwrap(),
            );
            let mut w = EntityWorld::new(b.clone(), mapents::collect(&b), 1);
            w.map_spawn("OnCoop");
            w.tick(1.);
            for template in w.find("ptemplate_survival") {
                for i in 1..=16 {
                    if let Some(target) = w.ents[template].me.kv(&format!("Template{i:02}")) {
                        for id in w.find(target) {
                            assert!(
                                !w.ents[id].alive,
                                "{name}: holdout template member {target} active in coop"
                            );
                        }
                    }
                }
            }
        }
    }
}
