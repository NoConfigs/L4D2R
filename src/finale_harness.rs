//! Offline finale runner (tests): starts a campaign finale and drives it to
//! the rescue with the same glue as `game::director` (map I/O <-> Director
//! scripts), with every horde and Tank dying at once. Records the stage
//! timeline and where it stops.
use bevy::prelude::Vec3;
use l4d2r_game_core::entities::Kind;

#[derive(Debug, Default)]
pub(crate) struct FinaleRun {
    pub map: String,
    pub log: Vec<String>,
    pub started: bool,
    pub escape_started: Option<f32>,
    pub vehicle_ready: Option<f32>,
    pub won: Option<f32>,
    pub stuck: Option<String>,
    pub script_errors: Vec<String>,
}

/// Run `map`'s finale for at most `limit` seconds of simulated time.
pub(crate) fn run(root: &std::path::Path, map: &str, limit: f32) -> std::io::Result<FinaleRun> {
    let path = ["left4dead2", "left4dead2_dlc1", "left4dead2_dlc2", "left4dead2_dlc3", "update"]
        .iter()
        .map(|d| root.join(d).join("maps").join(format!("{map}.bsp")))
        .find(|p| p.is_file())
        .ok_or_else(|| std::io::Error::other(format!("{map}.bsp not found")))?;
    let mut run = FinaleRun { map: map.into(), ..Default::default() };
    let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true)?;
    let mut assets = crate::source_assets::SourceAssets::new(root, vec![])?;
    let mut d = crate::source_director::Director::load(&mut assets, &path, crate::campaign_settings::Difficulty::Normal)?;
    let mut audio = crate::game_audio::Queue::default();
    let dt = 0.1;
    let step = |logic: &mut crate::l4d2r_cinematic::MapLogic, d: &mut crate::source_director::Director, audio: &mut crate::game_audio::Queue| {
        for _ in 0..6 {
            logic.world.tick(1. / 60.);
        }
        d.set_script_time(logic.world.time);
        if d.script_sync_due(dt) || !logic.world.cine.script_commands.is_empty() {
            d.sync_world(&logic.world);
        }
        for request in std::mem::take(&mut logic.world.cine.script_commands) {
            if std::env::var("L4D2R_FINALE_TRACE").is_ok() && request.param.contains("GasCan") {
                eprintln!("  t={:.1} script {} {} {}", logic.world.time, request.entity, request.input, request.param);
            }
            if request.input == "RegisterThink" {
                d.configure_think(request.entity, &request.param);
            } else {
                d.entity_script_context(request.entity, &request.input, &request.param, request.activator, request.caller);
            }
        }
        d.run_thinks(&logic.world);
        for (input, param) in std::mem::take(&mut logic.world.cine.director_commands) {
            if std::env::var("L4D2R_FINALE_TRACE").is_ok() {
                eprintln!("  t={:.1} director {input} {param}", logic.world.time);
            }
            d.command(&input, &param);
        }
        crate::game::flush_vscript_commands(d, Some(&mut *logic), audio);
        // Hordes die as they spawn; Tanks die at once.
        let n = d.tick(dt, 100., 0, 0, Vec3::ZERO);
        for _ in 0..n {
            d.spawned();
        }
        d.take_tank_requests();
        d.set_tank_alive(false);
        crate::game::flush_vscript_commands(d, Some(&mut *logic), audio);
        audio.0.clear();
    };
    // Settle the map (logic_auto, intro relays).
    for _ in 0..30 {
        step(&mut logic, &mut d, &mut audio);
    }
    // A finale kept in a point_template (c5m5's radio, c13m4's elevator
    // button) appears when survivors press the button that spawns it.
    if !logic.world.ents.iter().any(|e| e.alive && matches!(e.kind, Kind::Finale { .. })) {
        let finale_names: Vec<String> = logic
            .world
            .ents
            .iter()
            .filter(|e| matches!(e.kind, Kind::Finale { .. }))
            .map(|e| e.me.targetname.to_ascii_lowercase())
            .collect();
        let templates: Vec<String> = logic
            .world
            .ents
            .iter()
            .filter(|e| {
                e.me.classname == "point_template"
                    && (1..=16).any(|k| e.me.kv(&format!("Template{k:02}")).is_some_and(|n| finale_names.contains(&n.to_ascii_lowercase())))
            })
            .map(|e| e.me.targetname.to_ascii_lowercase())
            .collect();
        let buttons: Vec<usize> = (0..logic.world.ents.len())
            .filter(|&i| {
                let e = &logic.world.ents[i];
                e.alive
                    && e.conns.iter().any(|c| templates.contains(&c.target.to_ascii_lowercase()) && c.input.eq_ignore_ascii_case("ForceSpawn"))
            })
            .collect();
        for id in buttons {
            run.log.push(format!("pressed {} ({}) to spawn the finale", logic.world.ents[id].me.targetname, logic.world.ents[id].me.classname));
            logic.world.input(id, "Unlock", "", None, None);
            logic.world.input(id, "Press", "", None, None);
        }
        for _ in 0..150 {
            step(&mut logic, &mut d, &mut audio);
        }
    }
    let finale = logic
        .world
        .ents
        .iter()
        .position(|e| e.alive && matches!(e.kind, Kind::Finale { .. }))
        .ok_or_else(|| {
            let found: Vec<String> = logic
                .world
                .ents
                .iter()
                .filter(|e| e.me.classname == "trigger_finale")
                .map(|e| format!("{} alive {} templated {} kind {:?}", e.me.targetname, e.alive, e.templated, std::mem::discriminant(&e.kind)))
                .collect();
            std::io::Error::other(format!("no live trigger_finale: {found:?}"))
        })?;
    run.log.push(format!("finale entity {} ({})", logic.world.ents[finale].me.targetname, logic.world.ents[finale].me.classname));
    // Start it the way survivors do: enable if needed, then use.
    logic.world.input(finale, "Enable", "", None, None);
    logic.world.use_ent(finale, None);
    if !matches!(logic.world.ents[finale].kind, Kind::Finale { started: true, .. }) {
        logic.world.input(finale, "ForceFinaleStart", "", None, None);
    }
    run.started = matches!(logic.world.ents[finale].kind, Kind::Finale { started: true, .. });
    let mut last_stage = None;
    let mut stage_since = 0.;
    let mut boarded = false;
    let mut arrived = false;
    let mut pressed_after = false;
    let mut pours = 0;
    // Gas-can finales: survivors keep fetching and pouring cans.
    // (c14m2 spawns its cans from the finale script, so this is checked live.)
    let mut timed_seen = std::collections::HashMap::<usize, f32>::new();
    let mut t = 0.;
    while t < limit {
        step(&mut logic, &mut d, &mut audio);
        t += dt;
        for event in d.take_finale_events() {
            run.log.push(format!("{t:6.1}s Director finale event {event}"));
            logic.world.fire_output_pub(finale, &event);
            if event == "FinaleEscapeStarted" {
                run.escape_started.get_or_insert(t);
            }
        }
        let stage = d.finale().map(|f| (f.stage, f.kind, f.escape));
        if stage != last_stage {
            run.log.push(format!("{t:6.1}s stage {stage:?}"));
            last_stage = stage;
            stage_since = t;
        }
        // Scavenge / scripted stages wait on gameplay: 20 s into one, every
        // 3 s a survivor picks up a can (its spawn's OnItemPickedUp) and
        // pours one (info_director OnTeamScored, as the pour target does).
        if logic.world.ents.iter().any(|e| e.alive && e.me.classname == "weapon_scavenge_item_spawn") && stage.is_some_and(|(_, _, escape)| !escape) && t > 20. && ((t * 10.).round() as i32) % 30 == 0 {
            let spawn = (0..logic.world.ents.len())
                .find(|&i| logic.world.ents[i].alive && logic.world.ents[i].me.classname == "weapon_scavenge_item_spawn");
            if let Some(id) = spawn {
                logic.world.fire_output_pub(id, "OnItemPickedUp");
            }
            let directors: Vec<usize> = (0..logic.world.ents.len())
                .filter(|&i| logic.world.ents[i].alive && logic.world.ents[i].me.classname == "info_director")
                .collect();
            for id in directors {
                logic.world.fire_output_pub(id, "OnTeamScored");
            }
            pours += 1;
            if pours % 4 == 1 {
                run.log.push(format!("{t:6.1}s pours so far {pours}"));
            }
        }
        // Gameplay the finale waits on: hold any enabled timed button
        // (generators, switches) once it has been available for 10 s.
        let buttons: Vec<usize> = (0..logic.world.ents.len())
            .filter(|&i| logic.world.ents[i].alive && matches!(logic.world.ents[i].kind, Kind::TimedButton { disabled: false }))
            .collect();
        for id in buttons {
            let since = *timed_seen.entry(id).or_insert(t);
            if t - since > 10. {
                run.log.push(format!("{t:6.1}s held timed button {}", logic.world.ents[id].me.targetname));
                logic.world.fire_output_pub(id, "OnPressed");
                logic.world.fire_output_pub(id, "OnTimeUp");
                logic.world.input(id, "Disable", "", None, None);
                timed_seen.remove(&id);
            }
        }
        // Gauntlet finales (c5m5, c13m4): the vehicle is readied when the
        // survivors reach the trigger near it. 30 s into the escape, "arrive":
        // fire the triggers leading to the relays that ready the vehicle.
        if run.escape_started.is_some_and(|e| t - e > 30.) && !arrived && !logic.world.cine.finale_vehicle_ready {
            arrived = true;
            let finale_name = logic.world.ents[finale].me.targetname.to_ascii_lowercase();
            let readiers: Vec<String> = logic
                .world
                .ents
                .iter()
                .filter(|e| {
                    e.conns.iter().any(|c| {
                        c.target.eq_ignore_ascii_case(&finale_name)
                            && c.input.eq_ignore_ascii_case("FinaleEscapeVehicleReadyForSurvivors")
                    })
                })
                .map(|e| e.me.targetname.to_ascii_lowercase())
                .filter(|n| !n.is_empty())
                .collect();
            let triggers: Vec<usize> = (0..logic.world.ents.len())
                .filter(|&i| {
                    let e = &logic.world.ents[i];
                    e.alive
                        && e.me.classname.starts_with("trigger_")
                        && e.conns.iter().any(|c| readiers.contains(&c.target.to_ascii_lowercase()))
                })
                .collect();
            for id in triggers {
                run.log.push(format!("{t:6.1}s survivors reach trigger {} ({})", logic.world.ents[id].me.targetname, id));
                for output in ["OnStartTouch", "OnTrigger", "OnEntireTeamStartTouch"] {
                    logic.world.fire_output_pub(id, output);
                }
            }
        }
        if logic.world.cine.finale_vehicle_ready && run.vehicle_ready.is_none() {
            run.vehicle_ready = Some(t);
            run.log.push(format!("{t:6.1}s rescue vehicle ready"));
        }
        // Board: the vehicle's entire-team triggers (and plain triggers that
        // lead to the escape) see the whole team.
        if run.vehicle_ready.is_some_and(|r| t - r > 3.) && !boarded {
            boarded = true;
            let triggers: Vec<usize> = (0..logic.world.ents.len())
                .filter(|&i| {
                    let e = &logic.world.ents[i];
                    e.alive
                        && e.me.classname.starts_with("trigger_")
                        && e.me.kv_i32("entireteam").unwrap_or(0) > 0
                        && !matches!(e.kind, Kind::Trigger { disabled: true, .. })
                })
                .collect();
            run.log.push(format!("{t:6.1}s boarding: {} entire-team triggers", triggers.len()));
            for id in triggers {
                logic.world.fire_output_pub(id, "OnEntireTeamStartTouch");
            }
        }
        // Escapes that need one more action (c7m3: lower the bridge, then
        // restart the generator): 10 s after boarding, press unlocked buttons.
        if run.vehicle_ready.is_some_and(|r| t - r > 13.) && !pressed_after {
            pressed_after = true;
            let buttons: Vec<usize> = (0..logic.world.ents.len())
                .filter(|&i| logic.world.ents[i].alive && matches!(logic.world.ents[i].kind, Kind::Button { locked: false, .. }))
                .collect();
            for id in buttons {
                run.log.push(format!("{t:6.1}s pressed {}", logic.world.ents[id].me.targetname));
                logic.world.input(id, "Press", "", None, None);
            }
        }
        if logic.world.cine.finale_won {
            run.won = Some(t);
            run.log.push(format!("{t:6.1}s FinaleWon"));
            break;
        }
    }
    if run.won.is_none() {
        run.stuck = Some(format!(
            "stopped at {t:.0}s in stage {:?} (vehicle ready {:?})",
            d.finale().map(|f| (f.stage, f.kind, f.escape)),
            run.vehicle_ready
        ));
    }
    run.script_errors = d.script_error_list();
    Ok(run)
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_campaign_finale_reaches_the_rescue() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let maps = std::env::var("L4D2R_FINALES").unwrap_or_else(|_| {
            "c1m4_atrium c2m5_concert c3m4_plantation c4m5_milltown_escape c5m5_bridge c6m3_port c7m3_port \
             c8m5_rooftop c9m2_lots c10m5_houseboat c11m5_runway c12m5_cornfield c13m4_cutthroatcreek c14m2_lighthouse"
                .into()
        });
        let mut failed = vec![];
        for map in maps.split_whitespace() {
            match super::run(root, map, 1500.) {
                Ok(run) => {
                    println!("==== {map}: won {:?} escape {:?} vehicle {:?}", run.won, run.escape_started, run.vehicle_ready);
                    for line in &run.log {
                        println!("  {line}");
                    }
                    for e in &run.script_errors {
                        println!("  script error: {e}");
                    }
                    if run.won.is_none() {
                        failed.push(format!("{map}: {}", run.stuck.unwrap_or_default()));
                    }
                }
                Err(e) => failed.push(format!("{map}: {e}")),
            }
        }
        assert!(failed.is_empty(), "finales that do not finish:\n{}", failed.join("\n"));
    }
}
