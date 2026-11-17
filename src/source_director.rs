//! Common-infected Director pacing with owned NUC option tables.
//! Persistent Squirrel scopes drive live options/callbacks. Native intensity,
//! special/boss and finale simulation still require engine behavior reconstruction.
use crate::{campaign_settings::Difficulty, source_assets::SourceAssets};
use bevy::prelude::*;
use l4d2r_game_core::random::UniformRandomStream;
use std::{
    collections::{HashMap, HashSet},
    io,
    path::Path,
    sync::Mutex,
};
const CONSUMED: &[&str] = &[
    "CommonLimit",
    "MobMinSize",
    "MobMaxSize",
    "MobSpawnSize",
    "MobMaxPending",
    "MegaMobSize",
    "MobSpawnMinTime",
    "MobSpawnMaxTime",
    "IntensityRelaxThreshold",
    "SustainPeakMinTime",
    "SustainPeakMaxTime",
    "RelaxMinInterval",
    "RelaxMaxInterval",
    "RelaxMaxFlowTravel",
    "PanicForever",
    "PausePanicWhenRelaxing",
    "ZombieSpawnRange",
    "MaxSpecials",
    "SpecialRespawnInterval",
    "DominatorLimit",
    "SpecialsDuringRelax",
    "HunterLimit",
    "SmokerLimit",
    "BoomerLimit",
    "SpitterLimit",
    "JockeyLimit",
    "ChargerLimit",
    "cm_MaxSpecials",
    "ProhibitBosses",
    "cm_BaseSpecialLimit",
    "cm_SpecialRespawnInterval",
    "cm_DominatorLimit",
    "cm_AggressiveSpecials",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    BuildUp,
    SustainPeak,
    PeakFade,
    Relax,
}
struct Think {
    function: String,
    next: f32,
}
#[derive(Default, Clone)]
struct Script {
    options: HashMap<String, f32>,
    unsupported: Vec<String>,
}
#[cfg(test)]
fn script(text: &str) -> Script {
    let sources = HashMap::from([("test".into(), text.into())]);
    let parsed = crate::source_script_options::evaluate(&sources, "test");
    Script {
        options: parsed.options,
        unsupported: parsed.unsupported,
    }
}
/// One survivor's state for intensity events (see `Director::observe_intensity`).
#[derive(Clone, Copy, Debug)]
pub struct SurvivorSample {
    pub id: usize,
    pub position: Vec3,
    /// Permanent plus temporary health.
    pub health: f32,
    pub incap: bool,
    pub incap_health: f32,
    pub ledge: bool,
    pub dead: bool,
}
/// What died, for the enemy-death intensity rule (102d1910).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Killed {
    /// A common infected; `fast` when it was moving faster than 150 u/s.
    Common { fast: bool },
    Special,
    /// Tank or Witch (zombie classes 7 and 8).
    Boss,
}
#[derive(Clone, Copy, Debug, Default)]
struct SurvivorIntensity {
    value: f32,
    /// No decay before this Director clock time (1029c670).
    hold_until: f32,
    position: Vec3,
    health: f32,
    incap: bool,
    incap_health: f32,
    ledge: bool,
}
/// intensity_factor 0.25; the event weights by type (1029c860).
const INTENSITY_FACTOR: f32 = 0.25;
const INTENSITY_WEIGHTS: [f32; 6] = [0., 0.05, 0.2, 0.5, 1.0, 999999.875];
/// intensity_decay_time: seconds to decay full intensity to zero.
const INTENSITY_DECAY_TIME: f32 = 30.;
/// Decay is held for 5 s after every increase.
const INTENSITY_HOLD: f32 = 5.;
/// intensity_enemy_death_near_range / far_range (units).
const ENEMY_DEATH_NEAR: f32 = 150.;
const ENEMY_DEATH_FAR: f32 = 500.;
#[derive(Resource)]
pub struct Director {
    pub pace: Pace,
    pub intensity: f32,
    pub enabled: bool,
    pub pending: usize,
    forced_mob: bool,
    horde_alert: bool,
    scripts: HashMap<String, Script>,
    base: HashMap<String, f32>,
    active: HashMap<String, f32>,
    pub nav: Vec<Vec3>,
    rng: UniformRandomStream,
    clock: f32,
    pace_until: f32,
    mob_at: f32,
    spawn_at: f32,
    panic_at: f32,
    /// Director::StartPanicEvent (server 10289fb0): STAGE_INITIAL_DELAY
    /// lasts RandomFloat(1, 2) s. Then STAGE_MEGA_MOB (1028c400) plays
    /// MegaMobIncoming (1028b460) and the wanderers break into their run
    /// (recorded on c1m1_hotel 1.7 and 2.2 s after the panic input).
    alert_at: f32,
    /// STAGE_MEGA_MOB arms the zombie manager's mob timer for another
    /// RandomFloat(2, 4) s (10359ed0) before SpawnMob (1035a880) brings
    /// the mob in: recorded 3.6 to 5.5 s after the panic input.
    mob_start: f32,
    /// The panic's MegaMobIncoming warning, held until `alert_at`.
    panic_warn: bool,
    previous_health: f32,
    /// Per-survivor intensity (server 1029c740 / 1029c860), by survivor id.
    survivor_intensity: HashMap<usize, SurvivorIntensity>,
    previous_position: Option<Vec3>,
    travel: f32,
    initialized: bool,
    difficulty: Difficulty,
    vm: Option<Mutex<crate::source_vm::Vm>>,
    vm_sources: HashMap<String, String>,
    map_script: String,
    script_at: f32,
    failed_updates: HashSet<i32>,
    script_errors: HashSet<String>,
    pub script_commands: Vec<crate::source_vm::Command>,
    pub flow: f32,
    pub survivor_speed: f32,
    entity_cache: HashMap<usize, (bool, [f32; 3], [f32; 3])>,
    map_started: bool,
    round_started: bool,
    nav_flow: Option<crate::source_director_flow::Flow>,
    /// Wandering commons still to place, per `nav` spot (retail TerrorNavArea
    /// +0x124), and the next MaintainWanderingPopulation pass.
    wanderers: Vec<u16>,
    wander_at: f32,
    /// NAV spawn attributes of each `nav` spot.
    nav_attributes: Vec<u32>,
    survivor_positions: HashMap<usize, Vec3>,
    previous_flow: Option<f32>,
    thinks: HashMap<usize, Think>,
    world_time: f32,
    last_think_time: Option<f32>,
    session_start: Option<bool>,
    pub temporary_decay: f32,
    pub fall_damage: crate::fall_damage::Settings,
    challenge_script: Option<String>,
    special_at: Option<f32>,
    /// Retail special scheduling (server 1026f910 / 102707e0): per-class
    /// spawn timers (first set to a random initial delay when the survivors
    /// leave the safe area), each class's last death, slots reopening
    /// SpecialRespawnInterval after a death, and the live counts last seen.
    class_ready: Option<[f32; 6]>,
    class_died: [Option<f32>; 6],
    slot_reopen: Vec<f32>,
    special_live: [usize; 6],
    special_convars: HashMap<String, f32>,
    /// Custom finale stage machine (A_CustomFinale<N>).
    finale: Option<FinaleState>,
    /// trigger_finale outputs the host must fire (FinaleEscapeStarted).
    finale_events: Vec<String>,
    /// Tanks requested by finale TANK stages, spawned by the host.
    tank_requests: usize,
    /// Host report: a Tank is alive.
    tank_alive: bool,
    /// Director clock when the survivors last left the safe area.
    outside_since: Option<f32>,
    /// Latched once survivors leave the start area (retail
    /// player_left_safe_area, CDirector +0x160).
    left_safe_area: bool,
    /// Director clock of the next co-op Tank.
    tank_at: Option<f32>,
    /// Retail boss threats: NAV areas (flow graph indices) still armed.
    threat_areas: Vec<usize>,
    /// Shuffled threat order (7 Witch, 8 Tank, 9 nothing) and position.
    threats: [u8; 3],
    threat_index: usize,
    /// One threat Tank per map (CDirector +0x4f5).
    threat_tank_allowed: bool,
    /// Survival maps: the Survival round's waves (survival.rs); None in
    /// co-op.
    pub survival: Option<crate::survival::SurvivalDirector>,
}
/// Running custom finale. Stage types (finale scripts): PANIC 0, TANK 1,
/// DELAY 2, ONSLAUGHT/SCRIPTED 3.
#[derive(Clone, Debug, Default)]
pub struct FinaleState {
    pub stage: u32,
    pub kind: i32,
    waves_left: u32,
    wave_at: f32,
    until: f32,
    started_at: f32,
    /// Onslaught stage running a script in LocalScript.
    scripted: bool,
    pub escape: bool,
    escape_mob_at: f32,
}
impl Default for Director {
    fn default() -> Self {
        Self {
            pace: Pace::BuildUp,
            intensity: 0.,
            enabled: true,
            pending: 0,
            forced_mob: false,
            horde_alert: false,
            scripts: HashMap::new(),
            base: HashMap::new(),
            active: HashMap::new(),
            nav: vec![],
            rng: UniformRandomStream::new(1),
            clock: 0.,
            pace_until: 15.,
            mob_at: 0.,
            spawn_at: 8.,
            panic_at: 0.,
            alert_at: 0.,
            mob_start: 0.,
            panic_warn: false,
            previous_health: 100.,
            survivor_intensity: HashMap::new(),
            previous_position: None,
            travel: 0.,
            initialized: false,
            difficulty: Difficulty::Normal,
            vm: None,
            vm_sources: HashMap::new(),
            map_script: String::new(),
            script_at: 0.,
            failed_updates: HashSet::new(),
            script_errors: HashSet::new(),
            script_commands: vec![],
            flow: 0.,
            survivor_speed: 0.,
            entity_cache: HashMap::new(),
            map_started: false,
            round_started: false,
            nav_flow: None,
            wanderers: vec![],
            wander_at: 0.,
            nav_attributes: vec![],
            survivor_positions: HashMap::new(),
            previous_flow: None,
            thinks: HashMap::new(),
            world_time: 0.,
            last_think_time: None,
            session_start: None,
            temporary_decay: 0.27,
            fall_damage: Default::default(),
            challenge_script: None,
            special_at: None,
            class_ready: None,
            class_died: [None; 6],
            slot_reopen: vec![],
            special_live: [0; 6],
            special_convars: HashMap::new(),
            outside_since: None,
            left_safe_area: false,
            tank_at: None,
            threat_areas: vec![],
            threats: [7, 8, 9],
            threat_index: 0,
            threat_tank_allowed: true,
            finale: None,
            finale_events: vec![],
            tank_requests: 0,
            tank_alive: false,
            survival: None,
        }
    }
}
impl Director {
    pub fn set_difficulty(&mut self, difficulty: Difficulty) {
        self.difficulty = difficulty;
        if let Some(vm) = &self.vm {
            let _ = vm.lock().unwrap().string("Difficulty", difficulty.name());
        }
    }
    pub fn load(
        assets: &mut SourceAssets,
        path: &Path,
        difficulty: Difficulty,
    ) -> io::Result<Self> {
        let mut d = Self {
            difficulty,
            survival: crate::survival::active().then(Default::default),
            ..Default::default()
        };
        let mut report = String::from(
            "script\tstatic_analysis_options\tstatic_analysis_exclusions_not_vm_failures\tunconsumed_engine_options\tincludes\tother_option_tables\n",
        );
        let mut sources = HashMap::new();
        let mut paths = assets.matching_paths("scripts/vscripts/");
        paths.sort();
        for p in paths
            .into_iter()
            .filter(|p| p.ends_with(".nuc") || p.ends_with(".nut"))
        {
            let b = assets.read(&p)?;
            let b = if p.ends_with(".nuc") {
                crate::source_ice::decode(&b)
            } else {
                b
            };
            let text = String::from_utf8(b.clone())
                .unwrap_or_else(|_| b.iter().map(|v| char::from(*v)).collect());
            let key = p
                .trim_start_matches("scripts/vscripts/")
                .trim_end_matches(".nuc")
                .trim_end_matches(".nut")
                .to_lowercase();
            let output = Path::new("local-research/vscripts").join(format!("{key}.nut"));
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(output, &b)?;
            sources.insert(key, text);
        }
        let mut names = sources.keys().cloned().collect::<Vec<_>>();
        names.sort();
        for key in names {
            let parsed = crate::source_script_options::evaluate(&sources, &key);
            let mut keys = parsed.options.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            let mut tables = parsed
                .tables
                .keys()
                .filter(|n| n.as_str() != "DirectorOptions")
                .cloned()
                .collect::<Vec<_>>();
            tables.sort();
            if !keys.is_empty() || !parsed.unsupported.is_empty() || !tables.is_empty() {
                report.push_str(&format!(
                    "{key}\t{}\t{}\t{}\t{}\t{}\n",
                    keys.join(","),
                    parsed.unsupported.join(","),
                    keys.iter()
                        .filter(|k| !CONSUMED.contains(&k.as_str()))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(","),
                    parsed.includes.join(","),
                    tables.join(",")
                ));
            }
            d.scripts.insert(
                key,
                Script {
                    options: parsed.options,
                    unsupported: parsed.unsupported,
                },
            );
        }
        d.base = d
            .scripts
            .get("director_base")
            .map_or_else(HashMap::new, |s| s.options.clone());
        let map = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if let Some(s) = d.scripts.get(&map) {
            d.base.extend(s.options.clone());
        }
        d.vm_sources = sources;
        d.map_script = map.clone();
        // Retail seeds its random stream from the clock, so wanderers,
        // specials and the boss threat order differ every game. Captures keep
        // seed 1 to stay repeatable; `--director-seed N` picks one.
        let args: Vec<String> = std::env::args().collect();
        let seed = args
            .windows(2)
            .find(|w| w[0] == "--director-seed")
            .and_then(|w| w[1].parse::<i32>().ok())
            .unwrap_or_else(|| {
                if args.iter().any(|a| a == "--capture-game") {
                    1
                } else {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(1, |t| (t.as_micros() % i32::MAX as u128) as i32)
                        .max(1)
                }
            });
        d.rng.set_seed(seed);
        d.initialize_vm()?;
        if let Ok(b) = std::fs::read(path.with_extension("nav")) {
            if let Ok(mesh) = l4d2_formats::nav::parse_nav(&b) {
                d.nav = mesh
                    .areas
                    .iter()
                    .filter(|a| a.spawn_attributes & 0x800 == 0)
                    .map(|a| Vec3::from_array(crate::binary::source_position(a.center())))
                    .collect();
                d.nav_attributes = mesh
                    .areas
                    .iter()
                    .filter(|a| a.spawn_attributes & 0x800 == 0)
                    .map(|a| a.spawn_attributes)
                    .collect();
                d.nav_flow = Some(crate::source_director_flow::Flow::new(mesh));
            }
        }
        std::fs::create_dir_all("local-research")?;
        std::fs::write("local-research/director-script-options.tsv", report)?;
        println!(
            "Native Director: {} decoded scripts; {} NAV spawn areas; map option table {}",
            d.scripts.len(),
            d.nav.len(),
            map
        );
        Ok(d)
    }
    pub fn reset(&mut self) {
        // A restarted Survival round waits for the radio again.
        if self.survival.is_some() {
            self.survival = Some(Default::default());
        }
        self.pace = Pace::BuildUp;
        self.intensity = 0.;
        self.enabled = true;
        self.pending = 0;
        self.horde_alert = false;
        self.panic_warn = false;
        self.forced_mob = false;
        self.active.clear();
        self.clock = 0.;
        self.special_at = None;
        self.class_ready = None;
        self.class_died = [None; 6];
        self.slot_reopen.clear();
        self.special_live = [0; 6];
        self.pace_until = 15.;
        self.mob_at = 0.;
        self.spawn_at = 8.;
        self.panic_at = 0.;
        self.alert_at = 0.;
        self.mob_start = 0.;
        self.panic_warn = false;
        self.previous_position = None;
        self.previous_health = 100.;
        self.survivor_intensity.clear();
        self.travel = 0.;
        self.initialized = false;
        self.script_at = 0.;
        self.flow = 0.;
        self.left_safe_area = false;
        self.survivor_speed = 0.;
        self.failed_updates.clear();
        self.script_commands.clear();
        self.entity_cache.clear();
        self.map_started = false;
        self.round_started = false;
        self.survivor_positions.clear();
        self.previous_flow = None;
        self.thinks.clear();
        self.world_time = 0.;
        self.last_think_time = None;
        self.temporary_decay = 0.27;
        self.fall_damage = Default::default();
        if !self.vm_sources.is_empty() {
            if let Err(e) = self.initialize_vm() {
                self.report_script_error("reset", e);
            }
        }
        if let Some(name) = self.challenge_script.clone() {
            if let Err(e) = self.challenge_options(&name) {
                self.report_script_error("challenge reset", e);
            }
        }
    }
    fn initialize_vm(&mut self) -> io::Result<()> {
        let vm = crate::source_vm::Vm::new(&self.vm_sources)?;
        vm.string("MapName", &self.map_script)?;
        vm.string("Difficulty", self.difficulty.name())?;
        if let Some(start) = self.session_start {
            vm.number("SessionStartMap", if start { 1. } else { 0. });
        }
        if self.vm_sources.contains_key("director_base") {
            vm.run(1, "director_base")?;
            vm.collect_callbacks(1)?;
        }
        self.active = vm.options()?;
        self.script_commands.extend(vm.commands());
        self.vm = Some(Mutex::new(vm));
        Ok(())
    }
    pub fn sync_world(&mut self, world: &l4d2r_game_core::entities::EntityWorld) -> bool {
        self.set_script_time(world.time);
        let Some(vm) = &self.vm else { return true };
        // SpawnEntityFromTable reserves indices after the world's entities.
        let mut changes = format!(
            "if (__EntitySpawnNext < {n}) __EntitySpawnNext = {n};\n",
            n = world.ents.len()
        );
        let mut errors = vec![];
        let mut staged = Vec::new();
        {
            let vm = vm.lock().unwrap();
            for (id, e) in world.ents.iter().enumerate() {
                let p = if e.parent.is_some() {
                    world.world_matrix(id).w_axis.truncate()
                } else {
                    e.me.origin
                };
                let a = e.me.angles;
                let state = (e.alive, p.to_array(), a.to_array());
                if self.entity_cache.get(&id) == Some(&state) {
                    continue;
                }
                if !e.alive && self.entity_cache.get(&id).is_some_and(|v| v.0) {
                    if let Err(e) = vm.release_entity_scope(id) {
                        errors.push(e);
                    }
                    self.thinks.remove(&id);
                }
                staged.push((id,state));
                let model =
                    e.me.mdl
                        .clone()
                        .or_else(|| e.me.brush.map(|b| format!("*{b}")))
                        .unwrap_or_default();
                changes.push_str(&format!("__EntityData[{id}] <- {{name={},classname={},model={},alive={},origin=Vector({},{},{}),angles=Vector({},{},{})}};\n",crate::source_vm::quote(&e.me.targetname),crate::source_vm::quote(&e.me.classname),crate::source_vm::quote(&model),e.alive,p.x,p.y,p.z,a.x,a.y,a.z));
                if changes.len() > 8000 {
                    match vm.eval(0, "entity snapshot", &changes) {
                        Ok(()) => self.entity_cache.extend(staged.drain(..)),
                        Err(e) => { errors.push(e); staged.clear(); }
                    }
                    changes.clear();
                }
            }
            if !changes.is_empty() {
                match vm.eval(0, "entity snapshot", &changes) {
                    Ok(()) => self.entity_cache.extend(staged.drain(..)),
                    Err(e) => errors.push(e),
                }
            }
        }
        let ready = errors.is_empty();
        for error in errors {
            self.report_script_error("entity snapshot", error);
        }
        if !ready { return false; }
        if !self.map_started {
            self.map_started = true;
            if self.vm_sources.contains_key(&self.map_script) {
                let result = {
                    let vm = self.vm.as_ref().unwrap().lock().unwrap();
                    vm.run(2, &self.map_script)
                        .and_then(|_| vm.collect_callbacks(2))
                };
                if let Err(e) = result {
                    self.report_script_error(&self.map_script.clone(), e);
                    self.failed_updates.insert(2);
                }
            }
        }
        true
    }
    pub fn script_sync_due(&self, dt: f32) -> bool {
        self.clock + dt >= self.script_at
            || !self.map_started
            || self.thinks.values().any(|t| t.next <= self.world_time)
    }
    pub fn set_script_time(&mut self, time: f32) {
        self.world_time = time;
        if let Some(vm) = &self.vm {
            vm.lock().unwrap().number("Time", time as f64);
        }
    }
    fn prepare_entity_scope(vm: &crate::source_vm::Vm, id: usize) -> io::Result<i32> {
        let scope = 1000 + id as i32;
        vm.eval(
            0,
            "entity scope",
            &format!("__EntityHandle({id}).ValidateScriptScope();"),
        )?;
        vm.scope(scope, &format!("__EntityScopes.e{id}"))?;
        Ok(scope)
    }
    pub fn configure_think(&mut self, id: usize, function: &str) {
        self.thinks.remove(&id);
        if function.is_empty() {
            return;
        }
        let Some(vm) = &self.vm else { return };
        let result = Self::prepare_entity_scope(&vm.lock().unwrap(), id);
        match result {
            Ok(_) => {
                self.thinks.insert(
                    id,
                    Think {
                        function: function.into(),
                        next: self.world_time + 0.1,
                    },
                );
            }
            Err(e) => self.report_script_error("AddThinkToEnt", e),
        }
    }
    pub fn run_thinks(&mut self, world: &l4d2r_game_core::entities::EntityWorld) {
        self.thinks
            .retain(|id, _| world.ents.get(*id).is_some_and(|e| e.alive));
        if self.last_think_time == Some(self.world_time) {
            return;
        }
        self.last_think_time = Some(self.world_time);
        let due = self
            .thinks
            .iter()
            .filter(|(_, t)| t.next <= self.world_time)
            .map(|(id, t)| (*id, t.function.clone()))
            .collect::<Vec<_>>();
        for (id, function) in due {
            let (result, commands) = {
                let vm = self.vm.as_ref().unwrap().lock().unwrap();
                (vm.think(1000 + id as i32, &function), vm.commands())
            };
            self.script_commands.extend(commands);
            match result {
                Ok(interval) => {
                    if let Some(t) = self.thinks.get_mut(&id) {
                        t.next = self.world_time + interval.max(0.);
                    }
                }
                Err(e) => {
                    self.thinks.remove(&id);
                    self.report_script_error(&format!("entity {id} think {function}"), e);
                }
            }
        }
    }
    /// NAV flow distance (Source units from the start area) at `position`.
    pub fn flow_at(&self, position: Vec3) -> Option<f32> {
        self.nav_flow.as_ref()?.at(position)
    }
    /// Flow at an object's NAV area (the nearest within `max` units).
    pub fn flow_near(&self, position: Vec3, max: f32) -> Option<f32> {
        self.nav_flow.as_ref()?.at_near(position, max)
    }
    pub fn flow_start(&mut self, position: Vec3) {
        if let Some(flow) = &mut self.nav_flow {
            flow.start(position);
        }
        // CDirector reset (1026bdc0): reshuffle the threats, allow a Tank.
        self.reset_threats(9);
        self.threat_tank_allowed = true;
        self.place_threats();
        self.seed_wanderers();
    }
    /// Owned 1025e000: shuffle Witch/Tank/nothing; a Tank never leads the
    /// new order right after a Tank (`last`).
    fn reset_threats(&mut self, last: u8) {
        self.threats = [7, 8, 9];
        for i in (1..=2).rev() {
            let j = self.rng.random_int(0, i as i32) as usize;
            self.threats.swap(i, j);
        }
        if self.threats[0] == 8 && last == 8 {
            let k = self.rng.random_int(1, 2) as usize;
            self.threats[0] = self.threats[k];
            self.threats[k] = 8;
        }
        self.threat_index = 0;
    }
    fn advance_threat(&mut self, last: u8) {
        self.threat_index += 1;
        if self.threat_index > 2 {
            self.reset_threats(last);
        }
    }
    /// Owned 10265fd0 (with the director_force_witch/tank override from
    /// 10274bc0): the threat type due now. A Tank is skipped unless this
    /// map's threat Tank is still available, and then only 75% of the time
    /// (RandomInt(1,100) < 76). DirectorOptions.DisallowThreatType skips a type.
    fn current_threat(&mut self) -> u8 {
        if self.special_cvar("director_force_witch", 0.) != 0. {
            return 7;
        }
        if self.special_cvar("director_force_tank", 0.) != 0. {
            return 8;
        }
        let disallowed = self.option("DisallowThreatType", 0.) as u8;
        for _ in 0..100 {
            let t = self.threats[self.threat_index];
            if t != 8 && t != disallowed {
                return t;
            }
            if self.threat_tank_allowed && self.rng.random_int(1, 100) < 76 && t != disallowed {
                return t;
            }
            self.advance_threat(9);
        }
        9
    }
    /// Owned OnFlowDistancesInitialized (1035f370), co-op threat placement.
    /// Walks the escape route: the first threat comes 2 x ZombieSpawnRange +
    /// RandomFloat(0,1) x max separation in, later ones every
    /// RandomFloat(min x [0.8,1], max x [1,1.2]) units
    /// (director_threat_min/max_separation 5000). Each is a random area
    /// within 240 units of travel of that route area, not NOTHREAT and at
    /// least 24 x 24. At most director_max_threat_areas (4); with more room
    /// they are spread evenly.
    pub fn place_threats(&mut self) {
        self.threat_areas.clear();
        let Some(flow) = self.nav_flow.as_ref() else { return };
        let route = flow.escape_route();
        if route.len() < 2 {
            return;
        }
        let centers: Vec<_> = route.iter().map(|&i| flow.area_center(i)).collect();
        let length: f32 = centers.windows(2).map(|w| w[0].distance(w[1])).sum();
        let range = self.spawn_range() / crate::collision::UNIT;
        let mut min = self.special_cvar("director_threat_min_separation", 5000.);
        let mut max = self.special_cvar("director_threat_max_separation", 5000.);
        let mut remaining = range + range + self.rng.random_float(0., 1.) * max;
        let mut count = ((length as i32) as f32 / ((min + max) * 0.5)) as i32;
        let cap = self.special_cvar("director_max_threat_areas", 4.) as i32;
        if cap < count {
            min = length / cap as f32;
            max = min;
            count = cap;
        }
        min *= self.rng.random_float(0.8, 1.);
        max *= self.rng.random_float(1., 1.2);
        let mut placed = 0;
        for i in 1..route.len() {
            if placed == count {
                break;
            }
            remaining -= centers[i - 1].distance(centers[i]);
            if remaining > 0. {
                continue;
            }
            let mut near = self.nav_flow.as_ref().unwrap().surrounding(route[i], 240.);
            for j in 0..near.len() {
                let k = self.rng.random_int(0, near.len() as i32 - 1) as usize;
                near.swap(j, k);
            }
            let flow = self.nav_flow.as_ref().unwrap();
            if let Some(&area) = near.iter().find(|&&a| flow.threat_candidate(a)) {
                self.threat_areas.push(area);
                placed += 1;
                remaining = self.rng.random_float(min, max);
            }
        }
        let flow = self.nav_flow.as_ref().unwrap();
        let at: Vec<[i32; 3]> = self
            .threat_areas
            .iter()
            .map(|&a| {
                let c = flow.area_center(a);
                [c.x as i32, c.y as i32, c.z as i32]
            })
            .collect();
        println!(
            "Director threats: {} areas on a {length:.0}-unit escape route at {at:?}; order {:?}",
            self.threat_areas.len(),
            self.threats
        );
    }
    /// Owned 102e9bb0 / 10274bc0 gate: the first time a survivor stands in
    /// an armed threat area (not EMPTY, PLAYER_START, CHECKPOINT or
    /// RESCUE_CLOSET), the threat due now. A "nothing" threat is used up
    /// here. Returns the area and the type (7 Witch, 8 Tank) to spawn; call
    /// `threat_spawned` afterwards.
    pub fn threat_entered(&mut self, survivors: &[Vec3]) -> Option<(usize, u8)> {
        // Survival has no co-op Witches or flow Tanks.
        if self.survival.is_some() {
            return None;
        }
        let flow = self.nav_flow.as_ref()?;
        let entered: Vec<usize> = survivors.iter().filter_map(|p| flow.area_of(*p)).collect();
        let at = self
            .threat_areas
            .iter()
            .position(|a| entered.contains(a) && flow.area_attributes(*a) & 0x10882 == 0)?;
        if !self.enabled
            || self.special_cvar("director_no_bosses", 0.) != 0.
            || self.option("ProhibitBosses", 0.) != 0.
            || self.option("IgnoreNavThreatAreas", 0.) != 0.
        {
            return None;
        }
        let area = self.threat_areas[at];
        let kind = self.current_threat();
        if kind != 7 && kind != 8 {
            println!("Director threat: nothing (area {area})");
            self.threat_areas.remove(at);
            self.advance_threat(kind);
            return None;
        }
        Some((area, kind))
    }
    /// After a threat spawn attempt: the area is disarmed either way; a
    /// spawned threat advances the order and a threat Tank uses up this
    /// map's Tank.
    pub fn threat_spawned(&mut self, area: usize, kind: u8, spawned: bool) {
        self.threat_areas.retain(|a| *a != area);
        if !spawned {
            return;
        }
        for cvar in ["director_force_witch", "director_force_tank"] {
            self.special_convars.insert(cvar.into(), 0.);
        }
        if kind == 8 {
            self.threat_tank_allowed = false;
        }
        self.advance_threat(kind);
    }
    /// Owned 10274bc0 placement: from the threat area, walk toward the goal
    /// to the first area (big enough, not NOTHREAT) where the boss fits and
    /// that no survivor sees. `None` when the walk reaches the goal or an
    /// area survivors can see (retail then picks a random spawn area).
    pub fn threat_position(
        &self,
        area: usize,
        fits: impl Fn(Vec3) -> bool,
        visible: impl Fn(Vec3) -> bool,
    ) -> Option<Vec3> {
        let flow = self.nav_flow.as_ref()?;
        let mut at = area;
        for _ in 0..100 {
            let p = flow.area_world(at);
            if flow.threat_candidate(at) && fits(p) && !visible(p) {
                return Some(p);
            }
            let next = flow.toward_goal(at)?;
            if flow.is_goal(next) || visible(flow.area_world(next)) {
                return None;
            }
            at = next;
        }
        None
    }
    /// Witch facing: RandomFloat(0, 360) degrees of yaw.
    pub fn threat_yaw(&mut self) -> f32 {
        self.rng.random_float(0., 360.)
    }
    /// Owned OnFlowDistancesInitialized (1035f370): the map's wanderer count
    /// is round(escape route length * z_wandering_density (0.03) *
    /// WanderingZombieDensityModifier), each one added to a random NAV area
    /// (1000 consecutive misses end the scatter).
    pub fn seed_wanderers(&mut self) {
        self.wanderers = vec![0; self.nav.len()];
        let Some(route) = self.nav_flow.as_ref().and_then(|f| f.route_length()) else {
            return;
        };
        if self.nav.is_empty() {
            return;
        }
        let density = self.special_cvar("z_wandering_density", 0.03)
            * self.option("WanderingZombieDensityModifier", 1.);
        let mut left = (route * density + 0.5).max(0.) as usize;
        let mut misses = 0;
        while left > 0 && misses < 1000 {
            let i = self.rng.random_int(0, self.nav.len() as i32 - 1) as usize;
            // FUN_102e2510: no EMPTY, PLAYER_START, CHECKPOINT or
            // RESCUE_CLOSET areas.
            if self.wanderers[i] == u16::MAX || self.nav_attributes.get(i).is_some_and(|a| a & 0x10882 != 0) {
                misses += 1;
                continue;
            }
            self.wanderers[i] += 1;
            left -= 1;
            misses = 0;
        }
        println!(
            "Wandering zombie population = {} wanderers (escape route {route:.0})",
            self.wanderers_left()
        );
    }
    pub fn wanderers_left(&self) -> usize {
        self.wanderers.iter().map(|&n| n as usize).sum()
    }
    /// Owned CDirector wanderer gate (10265530): AlwaysAllowWanderers, else
    /// during PEAK_FADE/RELAX only below
    /// director_intensity_relax_allow_wanderers_threshold (0.3, Hard 0.5,
    /// Expert 0.8). No wanderers during a finale.
    pub fn wanderers_allowed(&self) -> bool {
        if !self.enabled || self.finale.is_some() || self.survival.is_some() {
            return false;
        }
        if self.option("AlwaysAllowWanderers", 0.) != 0. {
            return true;
        }
        if matches!(self.pace, Pace::PeakFade | Pace::Relax) {
            let threshold = match self.difficulty {
                Difficulty::Advanced => 0.5,
                Difficulty::Expert => 0.8,
                _ => 0.3,
            };
            let threshold = self.option("IntensityRelaxAllowWanderersThreshold", threshold);
            return self.intensity < threshold;
        }
        true
    }
    /// Owned TerrorNavMesh::MaintainWanderingPopulation (102f0ab0): once a
    /// second, up to 100 areas of the survivors' active set that still hold
    /// wanderers. The host places each out of the survivors' sight and calls
    /// `wanderer_spawned`. The active set is approximated by
    /// ZombieSpawnRange around any survivor.
    pub fn wanderer_spots(&mut self, dt: f32, survivors: &[Vec3]) -> Vec<(usize, Vec3)> {
        self.wander_at -= dt;
        if self.wander_at > 0. || !self.wanderers_allowed() {
            return vec![];
        }
        self.wander_at = 1.;
        let range = self.spawn_range();
        self.wanderers
            .iter()
            .enumerate()
            .filter(|(i, n)| {
                **n > 0 && survivors.iter().any(|s| s.distance(self.nav[*i]) <= range)
            })
            .take(100)
            .map(|(i, _)| (i, self.nav[i]))
            .collect()
    }
    pub fn wanderers_at(&self, spot: usize) -> u16 {
        self.wanderers.get(spot).copied().unwrap_or(0)
    }
    /// A random horizontal offset (Source units) within a NAV area's middle.
    pub fn wander_offset(&mut self) -> Vec3 {
        let angle = self.rng.random_float(0., std::f32::consts::TAU);
        let radius = self.rng.random_float(0., 48.);
        Vec3::new(angle.cos() * radius, 0., angle.sin() * radius)
    }
    pub fn wanderer_spawned(&mut self, spot: usize) {
        if let Some(n) = self.wanderers.get_mut(spot) {
            *n = n.saturating_sub(1);
        }
    }
    /// A mob or panic wave is queued or spawning: alerted commons attack at
    /// once (retail InfectedAlert's mob check).
    pub fn mob_active(&self) -> bool {
        (self.pending > 0 || self.forced_mob) && self.clock >= self.alert_at || self.finale.is_some()
    }
    pub fn observe_survivors(&mut self, dt: f32, positions: &[(usize, Vec3)]) {
        let mut flow = 0f32;
        let mut speed = 0.;
        let mut living = HashSet::new();
        for &(id, p) in positions {
            living.insert(id);
            if let Some(nav) = &self.nav_flow {
                if let Some(value) = nav.at(p) {
                    flow = flow.max(value);
                }
            }
            if let Some(previous) = self.survivor_positions.insert(id, p) {
                let delta = (p - previous) * Vec3::new(1., 0., 1.);
                if dt > 0. {
                    let s = delta.length() / dt / crate::collision::UNIT;
                    if s < 400. {
                        speed += s;
                    }
                }
            }
        }
        self.survivor_positions.retain(|id, _| living.contains(id));
        self.flow = flow;
        self.left_safe_area |= flow > 100.;
        self.survivor_speed = speed / positions.len().max(1) as f32;
    }
    pub fn entity_script(&mut self, id: usize, input: &str, param: &str) {
        self.entity_script_context(id, input, param, None, None);
    }
    /// Console code runs in the persistent root scope, just like global VScript.
    pub fn console_script(&mut self, code: &str, file: bool) -> String {
        let Some(vm) = &self.vm else {
            return "VScript VM is not initialized for this map.".into();
        };
        let (result, output, commands, options) = {
            let vm = vm.lock().unwrap();
            vm.take_output();
            let result = if file {
                vm.run(0, code.trim().trim_matches('"'))
            } else {
                vm.eval(0, "console", code)
            };
            (result, vm.take_output(), vm.commands(), vm.options())
        };
        self.script_commands.extend(commands);
        if let Ok(options) = options {
            self.active = options;
        }
        match result {
            Ok(()) => output,
            Err(error) => format!("{output}\nVScript: {error}"),
        }
    }
    pub fn entity_script_context(
        &mut self,
        id: usize,
        input: &str,
        param: &str,
        activator: Option<usize>,
        caller: Option<usize>,
    ) {
        let Some(vm) = &self.vm else { return };
        let scope = 1000 + id as i32;
        let (result, commands) = {
            let vm = vm.lock().unwrap();
            let handle = |id: Option<usize>| {
                id.map_or_else(|| "null".into(), |id| format!("__EntityHandle({id})"))
            };
            let prepare = Self::prepare_entity_scope(&vm, id).and_then(|_| {
                vm.eval(
                    scope,
                    "I/O context",
                    &format!(
                        "activator <- {}; caller <- {};",
                        handle(activator),
                        handle(caller)
                    ),
                )
            });
            let result = prepare.and_then(|_| match input.to_ascii_lowercase().as_str() {
                "runscriptfile" => vm
                    .run(scope, param)
                    .and_then(|_| vm.collect_callbacks(scope)),
                "runscriptcode" => vm
                    .eval(scope, "RunScriptCode", param)
                    .and_then(|_| vm.collect_callbacks(scope)),
                "callscriptfunction" => vm.think(scope, param).map(|_| ()),
                _ => Err(io::Error::other("unknown script input")),
            });
            (result, vm.commands())
        };
        self.script_commands.extend(commands);
        if let Err(e) = result {
            self.report_script_error(&format!("entity {id} {input}"), e);
        }
    }
    fn report_script_error(&mut self, scope: &str, error: impl std::fmt::Display) {
        let text = format!("{scope}: {error}");
        if self.script_errors.insert(text.clone()) {
            eprintln!("VScript: {text}");
        }
    }
    pub fn game_event(&mut self, name: &str, params: &[(&str, &str)]) {
        let Some(vm) = &self.vm else { return };
        let (result, commands, options) = {
            let vm = vm.lock().unwrap();
            (vm.event(name, params), vm.commands(), vm.options())
        };
        self.script_commands.extend(commands);
        if let Err(e) = result {
            self.report_script_error(name, e);
        }
        if let Ok(options) = options {
            self.active = options;
        }
    }
    fn update_scripts(&mut self, alive: usize, nearby: usize) {
        let Some(vm) = &self.vm else { return };
        let mut failures = vec![];
        let (commands, options) = {
            let vm = vm.lock().unwrap();
            for (key, value) in [
                ("Time", self.world_time),
                ("Intensity", self.intensity),
                ("CommonCount", alive as f32),
                ("PendingMobCount", self.pending as f32),
                ("InCombat", if nearby > 0 { 1. } else { 0. }),
                ("Flow", self.flow),
                ("Speed", self.survivor_speed),
                ("LeftSafeArea", if self.flow > 100. { 1. } else { 0. }),
                ("TankInPlay", if self.tank_alive || self.tank_requests > 0 { 1. } else { 0. }),
            ] {
                vm.number(key, value as f64);
            }
            for scope in [1, 2, 3, 4] {
                if !self.failed_updates.contains(&scope) {
                    if let Err(e) = vm.call(scope, "", "Update") {
                        failures.push((scope, e));
                    }
                }
            }
            (vm.commands(), vm.options())
        };
        for (scope, error) in failures {
            self.failed_updates.insert(scope);
            self.report_script_error(&format!("Update scope {scope}"), error);
        }
        self.script_commands.extend(commands);
        match options {
            Ok(options) => self.active = options,
            Err(e) => self.report_script_error("DirectorOptions", e),
        }
    }
    pub fn allow_fallen_item(&mut self, class: &str) -> bool {
        if class == "weapon_first_aid_kit" && self.option("cm_TempHealthOnly", 0.) != 0. {
            return false;
        }
        let Some(vm) = &self.vm else {
            return true;
        };
        let (result, commands, options) = {
            let vm = vm.lock().unwrap();
            (
                vm.director_policy("AllowFallenSurvivorItem", class),
                vm.commands(),
                vm.options(),
            )
        };
        self.script_commands.extend(commands);
        if let Ok(options) = options {
            self.active = options;
        }
        match result {
            Ok(value) => value.unwrap_or(true),
            Err(error) => {
                self.report_script_error("AllowFallenSurvivorItem", error);
                false
            }
        }
    }
    pub fn session_start_map(&mut self, start: bool) {
        self.session_start = Some(start);
        if let Some(vm) = &self.vm {
            vm.lock()
                .unwrap()
                .number("SessionStartMap", if start { 1. } else { 0. });
        }
    }
    pub fn convar(&mut self, name: &str, value: &str) {
        if let Ok(v)=value.parse::<f32>() {
            if v.is_finite() && v>=0. {
                match name {
                    "fall_speed_safe" => self.fall_damage.safe=v,
                    "fall_speed_fatal" => self.fall_damage.fatal=v,
                    "survivor_incap_max_fall_damage" => self.fall_damage.max_incap_damage=v,
                    _ => {}
                }
            }
        }
        if matches!(
            name,
            "director_no_specials"
                | "z_minion_limit"
                | "z_special_spawn_interval"
                | "director_special_respawn_interval"
                | "director_special_initial_spawn_delay_min"
                | "director_special_initial_spawn_delay_max"
                | "z_hunter_limit"
                | "z_smoker_limit"
                | "z_boomer_limit"
                | "z_spitter_limit"
                | "z_jockey_limit"
                | "z_charger_limit"
                | "director_no_bosses"
                | "director_force_tank"
                | "director_force_witch"
                | "director_threat_min_separation"
                | "director_threat_max_separation"
                | "director_max_threat_areas"
                | "director_tank_min_interval"
                | "director_tank_max_interval"
                | "director_tank_checkpoint_interval"
                | "z_wandering_density"
                | "z_grab_ledges_solo"
                | "survivor_ledge_grab_health"
        ) {
            if let Ok(value) = value.parse::<f32>() {
                if value.is_finite() && value >= 0. {
                    self.special_convars.insert(name.into(), value);
                    if name == "z_special_spawn_interval" {
                        self.special_convars
                            .insert("director_special_respawn_interval".into(), value);
                    }
                }
            }
        }
        if name == "pain_pills_decay_rate" {
            if let Ok(value) = value.parse::<f32>() {
                if value.is_finite() && value >= 0. {
                    self.temporary_decay = value;
                }
            }
        }
    }
    pub fn challenge_options(&mut self, name: &str) -> io::Result<()> {
        let vm = self
            .vm
            .as_ref()
            .ok_or_else(|| io::Error::other("Director VM is unavailable"))?;
        let (result, commands, options) = {
            let vm = vm.lock().unwrap();
            (vm.load_challenge_options(name), vm.commands(), vm.options())
        };
        self.script_commands.extend(commands);
        result?;
        self.active = options?;
        self.failed_updates.remove(&4);
        self.challenge_script = Some(name.into());
        Ok(())
    }
    /// The ZombieDontClear script option (z_dont_clear 0): survivors never
    /// mark NAV areas cleared (server 102eded0).
    pub fn dont_clear(&self) -> bool {
        self.option("ZombieDontClear", 0.) != 0.
    }
    fn option(&self, key: &str, default: f32) -> f32 {
        if self.vm.is_some() {
            return self.active.get(key).copied().unwrap_or(default);
        }
        self.active
            .get(key)
            .or_else(|| self.base.get(key))
            .copied()
            .unwrap_or(default)
    }
    fn range(&mut self, min: &str, max: &str, lo: f32, hi: f32) -> f32 {
        let a = self.option(min, lo).max(0.);
        let b = self.option(max, hi).max(a);
        self.rng.random_float(a, b)
    }
    pub fn limit(&self) -> usize {
        self.option("CommonLimit", 30.).clamp(0., 128.) as usize
    }
    pub(crate) fn special_cvar(&self, name: &str, default: f32) -> f32 {
        self.special_convars.get(name).copied().unwrap_or(default)
    }
    /// Bounded local AI-special scheduling. Retail cvar defaults are used;
    /// native class weighting, offers and encounter scheduling remain open.
    pub fn special_wave(
        &mut self,
        live: [usize; 6],
        survivors_outside: bool,
    ) -> Vec<crate::special_infected::SpecialClass> {
        use crate::special_infected::SpecialClass;
        // Survival: the round's growing limits, no safe area and no lull
        // spawns.
        if let Some(s) = self.survival.as_ref() {
            if !self.enabled || !s.running() || s.in_lull(self.clock) {
                return vec![];
            }
            let due = *self.special_at.get_or_insert(self.clock + 5.);
            if self.clock < due {
                return vec![];
            }
            let (maximum, limits) = (s.max_specials as usize, s.limits);
            let mut counts = live;
            let mut result = vec![];
            for _ in 0..maximum.saturating_sub(live.iter().sum()) {
                let candidates: Vec<usize> = (0..6).filter(|&i| counts[i] < limits[i] as usize).collect();
                if candidates.is_empty() {
                    break;
                }
                let i = candidates[self.rng.random_int(0, candidates.len() as i32 - 1) as usize];
                counts[i] += 1;
                result.push(SpecialClass::MINIONS[i]);
            }
            self.special_at = Some(self.clock + 2.);
            return result;
        }
        if !self.enabled
            || !survivors_outside
            || self.special_cvar("director_no_specials", 0.) != 0.
        {
            return vec![];
        }
        let maximum = self
            .option(
                "MaxSpecials",
                self.option("cm_MaxSpecials", self.special_cvar("z_minion_limit", 3.)),
            )
            .clamp(0., 16.) as usize;
        let interval = self.respawn_interval();
        // Deaths since the last call start the class's timer and reopen a
        // slot after SpecialRespawnInterval (102707e0).
        for i in 0..6 {
            for _ in live[i]..self.special_live[i] {
                self.class_died[i] = Some(self.clock);
                self.slot_reopen.push(self.clock + interval);
            }
        }
        self.special_live = live;
        let clock = self.clock;
        self.slot_reopen.retain(|t| *t > clock);
        // The six class timers start at a random initial delay each.
        if self.class_ready.is_none() {
            let min = self.special_cvar("director_special_initial_spawn_delay_min", 30.);
            let max = self.special_cvar("director_special_initial_spawn_delay_max", 60.).max(min);
            let mut ready = [0.; 6];
            for r in &mut ready {
                *r = clock + self.rng.random_float(min, max);
            }
            self.class_ready = Some(ready);
        }
        let room = maximum.saturating_sub(live.iter().sum::<usize>() + self.slot_reopen.len());
        if room == 0
            || self.special_at.is_some_and(|t| clock < t)
            || (matches!(self.pace, Pace::Relax | Pace::PeakFade)
                && self.pending == 0
                && self.option(
                    "SpecialsDuringRelax",
                    self.option("cm_AggressiveSpecials", 0.),
                ) == 0.)
        {
            return vec![];
        }
        let dominators = self
            .option(
                "DominatorLimit",
                self.option("cm_DominatorLimit", maximum as f32),
            )
            .clamp(0., 16.) as usize;
        let mut ready = self.class_ready.unwrap_or_default();
        let mut candidates = vec![];
        for (i, role) in SpecialClass::MINIONS.into_iter().enumerate() {
            if clock < ready[i] || self.class_died[i].is_some_and(|t| clock - t <= interval) {
                continue;
            }
            let key = format!("{}Limit", role.label()[..1].to_owned() + &role.name()[1..]);
            let limit = self
                .option(
                    &key,
                    self.option(
                        "cm_BaseSpecialLimit",
                        self.special_cvar(&format!("z_{}_limit", role.name()), 1.),
                    ),
                )
                .clamp(0., 16.) as usize;
            let pinning = matches!(
                role,
                SpecialClass::Hunter | SpecialClass::Smoker | SpecialClass::Jockey | SpecialClass::Charger
            );
            let pin_count = live[0] + live[1] + live[4] + live[5];
            if live[i] < limit && (!pinning || pin_count < dominators) {
                candidates.push(role);
            } else {
                // Not allowed now: look again in 20 s (1026f910).
                ready[i] = clock + 20.;
            }
        }
        self.class_ready = Some(ready);
        if candidates.is_empty() {
            return vec![];
        }
        // One special per attempt, a random allowed class.
        vec![candidates[self.rng.random_int(0, candidates.len() as i32 - 1) as usize]]
    }
    fn respawn_interval(&self) -> f32 {
        self.option(
            "SpecialRespawnInterval",
            self.option(
                "cm_SpecialRespawnInterval",
                self.special_cvar("director_special_respawn_interval", 45.),
            ),
        )
        .clamp(0.1, 600.)
    }
    /// Script errors reported so far (finale harness).
    pub fn script_error_list(&self) -> Vec<String> {
        let mut v: Vec<String> = self.script_errors.iter().cloned().collect();
        v.sort();
        v
    }
    pub fn finale(&self) -> Option<&FinaleState> {
        self.finale.as_ref()
    }
    pub fn take_finale_events(&mut self) -> Vec<String> {
        std::mem::take(&mut self.finale_events)
    }
    /// Retail ForceSurvivalStart: starts the Survival round now. None when
    /// this is not a Survival map.
    pub fn force_survival_start(&mut self) -> Option<()> {
        let s = self.survival.as_mut()?;
        if !s.running() {
            s.start(self.clock);
            self.game_event("survival_round_start", &[]);
        }
        Some(())
    }
    pub fn take_tank_requests(&mut self) -> usize {
        std::mem::take(&mut self.tank_requests)
    }
    /// Requests the host could not place yet (retried next tick).
    pub fn return_tank_requests(&mut self, count: usize) {
        self.tank_requests += count;
    }
    fn raise_intensity(&mut self, id: usize, kind: usize) {
        let clock = self.clock;
        if let Some(s) = self.survivor_intensity.get_mut(&id) {
            s.value = (s.value + INTENSITY_FACTOR * INTENSITY_WEIGHTS[kind]).min(1.);
            if s.hold_until - clock < INTENSITY_HOLD {
                s.hold_until = clock + INTENSITY_HOLD;
            }
        }
    }
    /// Damage, incapacitation and ledge grabs raise a survivor's intensity:
    /// pain levels by damage / max health (server 10306120: under 0.2 minor,
    /// under 0.5 major, else severe; damage while down counts as major),
    /// incapacitation to the maximum (1032eb40) and a ledge grab as major
    /// (10320710).
    /// A survivor's intensity, 0..1 (m_clientIntensity / 100).
    pub fn survivor_intensity(&self, id: usize) -> f32 {
        self.survivor_intensity.get(&id).map_or(0., |s| s.value)
    }
    pub fn observe_intensity(&mut self, samples: &[SurvivorSample]) {
        let living: HashSet<usize> = samples.iter().filter(|s| !s.dead).map(|s| s.id).collect();
        self.survivor_intensity.retain(|id, _| living.contains(id));
        for sample in samples.iter().filter(|s| !s.dead) {
            let Some(prev) = self.survivor_intensity.get(&sample.id).copied() else {
                self.survivor_intensity.insert(
                    sample.id,
                    SurvivorIntensity {
                        position: sample.position,
                        health: sample.health,
                        incap: sample.incap,
                        incap_health: sample.incap_health,
                        ledge: sample.ledge,
                        ..Default::default()
                    },
                );
                continue;
            };
            let event = if sample.ledge && !prev.ledge {
                Some(3)
            } else if sample.incap && !prev.incap && !sample.ledge {
                Some(5)
            } else if sample.incap && prev.incap && !sample.ledge && sample.incap_health < prev.incap_health {
                Some(3)
            } else if !sample.incap && !prev.incap && sample.health < prev.health {
                let fraction = (prev.health - sample.health) / 100.;
                Some(if fraction < 0.2 { 2 } else if fraction < 0.5 { 3 } else { 4 })
            } else {
                None
            };
            if let Some(s) = self.survivor_intensity.get_mut(&sample.id) {
                s.position = sample.position;
                s.health = sample.health;
                s.incap = sample.incap;
                s.incap_health = sample.incap_health;
                s.ledge = sample.ledge;
            }
            if let Some(kind) = event {
                self.raise_intensity(sample.id, kind);
            }
        }
    }
    /// An infected died at `position` (server 102d1910, every living
    /// survivor): beyond 500 units nothing; from 150 to 500 a Tank/Witch is
    /// major, anything else slight; within 150 a Tank/Witch is severe, a
    /// special minor, and a common slight when it was moving at 150 u/s or
    /// less, otherwise minor.
    pub fn enemy_killed(&mut self, position: Vec3, killed: Killed) {
        let survivors: Vec<(usize, Vec3)> =
            self.survivor_intensity.iter().map(|(id, s)| (*id, s.position)).collect();
        for (id, at) in survivors {
            let distance = at.distance(position) / crate::collision::UNIT;
            let kind = if distance >= ENEMY_DEATH_NEAR {
                if distance >= ENEMY_DEATH_FAR {
                    continue;
                }
                if killed == Killed::Boss { 3 } else { 1 }
            } else {
                match killed {
                    Killed::Boss => 4,
                    Killed::Special => 2,
                    Killed::Common { fast: false } => 1,
                    Killed::Common { fast: true } => 2,
                }
            };
            self.raise_intensity(id, kind);
        }
    }
    /// The highest survivor intensity (what the tempo compares).
    pub fn max_survivor_intensity(&self) -> Option<f32> {
        self.survivor_intensity.values().map(|s| s.value).reduce(f32::max)
    }
    pub fn set_tank_alive(&mut self, alive: bool) {
        self.tank_alive = alive;
    }
    /// A string-valued DirectorOptions entry (e.g. an onslaught stage's
    /// script name), read through the VM: LocalScript, MapScript, then root.
    fn option_text(&mut self, key: &str) -> Option<String> {
        let vm = self.vm.as_ref()?;
        let vm = vm.lock().unwrap();
        let code = format!(
            "local k={k}; foreach (s in [DirectorScript.MapScript.LocalScript, DirectorScript.MapScript, DirectorScript]) {{ if ((\"DirectorOptions\" in s) && (k in s.DirectorOptions)) {{ DoEntFire(\"__rewrite_option\", k, s.DirectorOptions[k].tostring(), 0, null, null); break; }} }}",
            k = crate::source_vm::quote(key)
        );
        vm.eval(3, "finale option", &code).ok()?;
        let mut found = None;
        for command in vm.commands() {
            if command.target == "__rewrite_option" {
                found = Some(command.param);
            } else {
                self.script_commands.push(command);
            }
        }
        found
    }
    /// Start the custom finale after the finale script ran (BeginFinale).
    fn begin_finale(&mut self) {
        self.finale = Some(FinaleState::default());
        self.advance_finale();
    }
    /// Next custom finale stage, or the escape after the last one.
    fn advance_finale(&mut self) {
        let Some(mut f) = self.finale.take() else { return };
        // A finished onslaught stage's script options do not outlive it.
        if f.scripted {
            if let Some(vm) = &self.vm {
                let vm = vm.lock().unwrap();
                let _ = vm.eval(3, "end stage", "if (\"DirectorOptions\" in DirectorScript.MapScript.LocalScript && !(\"A_CustomFinale1\" in DirectorScript.MapScript.LocalScript.DirectorOptions)) delete DirectorScript.MapScript.LocalScript.DirectorOptions");
            }
        }
        f.scripted = false;
        f.stage += 1;
        let count = self.option("A_CustomFinale_StageCount", f32::INFINITY);
        let kind = self.option(&format!("A_CustomFinale{}", f.stage), -1.);
        if f.escape || (f.stage as f32) > count || kind < 0. {
            if !f.escape {
                println!("Director finale: escape after {} stages", f.stage - 1);
                f.escape = true;
                f.escape_mob_at = self.clock;
                self.finale_events.push("FinaleEscapeStarted".into());
            }
            self.finale = Some(f);
            return;
        }
        f.kind = kind as i32;
        f.started_at = self.clock;
        // Script hook first: it rewrites MapScript.DirectorOptions per stage.
        if let Some(vm) = &self.vm {
            let result = {
                let vm = vm.lock().unwrap();
                let r = vm.eval(
                    3,
                    "finale stage",
                    &format!("local l=DirectorScript.MapScript.LocalScript; if (\"OnBeginCustomFinaleStage\" in l) l.OnBeginCustomFinaleStage.call(l, {}, {})", f.stage, f.kind),
                );
                (r, vm.commands(), vm.options())
            };
            if let Err(e) = result.0 {
                self.report_script_error("OnBeginCustomFinaleStage", e);
            }
            self.script_commands.extend(result.1);
            if let Ok(options) = result.2 {
                self.active = options;
            }
        }
        let value_key = format!("A_CustomFinaleValue{}", f.stage);
        let value = self.option(&value_key, 1.);
        println!("Director finale: stage {} type {} value {}", f.stage, f.kind, value);
        match f.kind {
            0 => {
                f.waves_left = value.max(1.) as u32;
                f.wave_at = self.clock;
                self.command("forcepanicevent", "");
            }
            1 => self.tank_requests += value.max(1.) as usize,
            2 => f.until = self.clock + value.max(0.),
            _ => {
                if let Some(script) = self.option_text(&value_key).filter(|s| !s.trim().is_empty()) {
                    let key = script.trim().to_ascii_lowercase();
                    if let Some(vm) = &self.vm {
                        // Same scope as the finale script, not reset: stage
                        // scripts redefine its hooks (c1m4_delay AbortDelay).
                        let result = {
                            let vm = vm.lock().unwrap();
                            let r = vm.run(3, &key).and_then(|_| vm.collect_callbacks(3));
                            (r, vm.commands(), vm.options())
                        };
                        if let Err(e) = result.0 {
                            self.report_script_error(&key, e);
                        }
                        self.script_commands.extend(result.1);
                        if let Ok(options) = result.2 {
                            self.active = options;
                        }
                        f.scripted = true;
                    }
                }
            }
        }
        self.finale = Some(f);
    }
    /// Finale stage progress (called from `tick`).
    fn finale_tick(&mut self, alive: usize) {
        let Some(f) = self.finale.clone() else { return };
        if f.escape {
            // Endless escape hordes until the survivors leave.
            if self.pending == 0 && self.clock >= f.escape_mob_at {
                let size = self.wave_size();
                self.enqueue(size);
                if let Some(f) = self.finale.as_mut() {
                    f.escape_mob_at = self.clock + 10.;
                }
            }
            return;
        }
        let done = match f.kind {
            // A wave is over once spawned and (nearly) cleared, or after a
            // minute without clearing (a stalled wave must not stop the
            // finale; retail's exact wave-end rule is not reconstructed).
            0 => {
                // pending cannot drain while CommonLimit is full of live
                // infected, so the cap ignores it.
                let since = self.clock - f.wave_at;
                if (self.pending == 0 && alive <= 3 && since > 5.) || since > 60. {
                    if f.waves_left > 1 {
                        if let Some(f) = self.finale.as_mut() {
                            f.waves_left -= 1;
                            f.wave_at = self.clock;
                        }
                        self.command("forcepanicevent", "");
                    }
                    f.waves_left <= 1
                } else {
                    false
                }
            }
            1 => self.tank_requests == 0 && !self.tank_alive && self.clock - f.started_at > 3.,
            2 => self.clock >= f.until,
            // Onslaught: until the script fires EndCustomScriptedStage.
            _ => false,
        };
        if done {
            self.advance_finale();
        }
    }
    /// Co-op Tank schedule. Retail cvars: director_tank_checkpoint_interval
    /// (15 s after leaving a checkpoint before a Tank can spawn) and
    /// director_tank_min/max_interval (350-500 s between Tanks);
    /// director_no_bosses and DirectorOptions.ProhibitBosses forbid them and
    /// director_force_tank requests one now. The first Tank is due one
    /// interval after leaving the checkpoint: retail also places co-op bosses
    /// by flow, which is not reconstructed.
    pub fn tank_due(&mut self, survivors_outside: bool, tank_alive: bool) -> bool {
        // Survival: Tank stages on the round's schedule, several at once.
        if let Some(s) = self.survival.as_mut() {
            let clock = self.clock;
            return self.enabled && s.tank_due(clock);
        }
        // Retail co-op places Tanks by threat areas (threat_entered;
        // director_force_tank applies there too). The director_tank_*_interval
        // cvars only time Versus tank offers, so this schedule is the
        // fallback for maps without a NAV flow.
        if self.threats_active() {
            return false;
        }
        let forced = self.special_cvar("director_force_tank", 0.) != 0.;
        if forced && !tank_alive {
            self.special_convars.insert("director_force_tank".into(), 0.);
            return true;
        }
        if !self.enabled
            || self.special_cvar("director_no_bosses", 0.) != 0.
            || self.option("ProhibitBosses", 0.) != 0.
        {
            return false;
        }
        if !survivors_outside {
            self.outside_since = None;
            return false;
        }
        let since = *self.outside_since.get_or_insert(self.clock);
        if self.clock - since < self.special_cvar("director_tank_checkpoint_interval", 15.) {
            return false;
        }
        let min = self.special_cvar("director_tank_min_interval", 350.);
        let max = self.special_cvar("director_tank_max_interval", 500.).max(min);
        let due = match self.tank_at {
            Some(due) => due,
            None => {
                let due = self.clock + self.rng.random_float(min, max);
                self.tank_at = Some(due);
                due
            }
        };
        if self.clock < due || tank_alive {
            return false;
        }
        self.tank_at = Some(self.clock + self.rng.random_float(min, max));
        true
    }
    /// Survivors have left the start area this map (player_left_safe_area).
    pub fn left_safe_area(&self) -> bool {
        self.left_safe_area
    }
    /// Threat placement is in use (this map has a NAV flow).
    pub fn threats_active(&self) -> bool {
        self.nav_flow.is_some()
    }
    pub fn specials_spawned(&mut self, count: usize) {
        if let Some(s) = self.survival.as_ref() {
            if count > 0 {
                self.special_at = Some(self.clock + s.special_interval);
            }
            return;
        }
        // Co-op: a failed placement waits 2 s before the next attempt; a
        // spawn is counted through the live counts on the next call.
        self.special_at = (count == 0).then_some(self.clock + 2.);
    }
    fn wave_size(&mut self) -> usize {
        let size = self.option("MobSpawnSize", -1.);
        if size >= 0. {
            size as usize
        } else {
            self.range("MobMinSize", "MobMaxSize", 10., 30.).round() as usize
        }
    }
    /// Owned FUN_104abb20 (survivor becomes IT): wanderers within
    /// z_notice_it_range are grabbed first, then a mob of the remaining
    /// MobMaxSize (z_mob_spawn_max_size=30) is queued. Returns the queued count.
    pub fn it_mob(&mut self, grabbed: usize) -> usize {
        let desired = self.option("MobMaxSize", 30.).max(0.) as usize;
        let missing = desired.saturating_sub(grabbed);
        if !self.enabled || missing == 0 {
            return 0;
        }
        let before = self.pending;
        self.enqueue(missing);
        self.forced_mob = self.pending > 0;
        self.pending.saturating_sub(before)
    }
    pub fn take_horde_alert(&mut self) -> bool {
        std::mem::take(&mut self.horde_alert)
    }
    fn enqueue(&mut self, size: usize) {
        let before = self.pending;
        // Owned SpawnMob pseudocode (1035a880): add to the existing queue;
        // positive MobMaxPending caps it. Zero leaves it uncapped.
        self.pending = self.pending.saturating_add(size);
        let cap = self.option("MobMaxPending", 0.) as usize;
        if cap > 0 {
            self.pending = self.pending.min(cap);
        }
        if before == 0 && self.pending > 1 {
            self.horde_alert = true;
        }
    }
    fn panic_size(&mut self) -> usize {
        let size = self.option("MegaMobSize", -1.);
        if size >= 0. {
            if size > 1. { size as usize } else { 0 }
        } else {
            self.range("MobMinSize", "MobMaxSize", 10., 30.).round() as usize
        }
    }
    pub fn command(&mut self, input: &str, param: &str) {
        match input.to_ascii_lowercase().as_str() {
            "beginfinale" => {
                let script=format!("{}_finale",self.map_script);
                if self.vm_sources.contains_key(&script) { self.command("BeginScript",&script); }
                self.begin_finale();
            }
            "endcustomscriptedstage" | "advancefinalestate" => {
                if self.finale.as_ref().is_some_and(|f| !f.escape) {
                    self.advance_finale();
                }
            }

            "beginscript" => {
                let path = param.trim().replace('\\', "/").to_lowercase();
                let key = path
                    .trim_start_matches("scripts/vscripts/")
                    .trim_end_matches(".nut")
                    .trim_end_matches(".nuc");
                if let Some(vm) = &self.vm {
                    let (result, commands, options) = {
                        let vm = vm.lock().unwrap();
                        let result = vm
                            .reset_local()
                            .and_then(|_| vm.run(3, key))
                            .and_then(|_| vm.collect_callbacks(3));
                        (result, vm.commands(), vm.options())
                    };
                    self.failed_updates.remove(&3);
                    self.script_commands.extend(commands);
                    if let Err(e) = result {
                        self.failed_updates.insert(3);
                        self.report_script_error(key, e);
                    }
                    if let Ok(options) = options {
                        self.active = options;
                    }
                    self.pending = 0;
                    self.horde_alert = false;
                    self.panic_warn = false;
                    self.forced_mob = false;
                    return;
                }
                if let Some(s) = self.scripts.get(key) {
                    self.active = s.options.clone();
                    self.pending = 0;
                    self.horde_alert = false;
                    self.panic_warn = false;
                    self.forced_mob = false;
                    println!(
                        "Director BeginScript: {key}: {} evaluated options; unsupported {:?}",
                        s.options.len(),
                        s.unsupported
                    );
                } else {
                    eprintln!("Director script unavailable: {key}");
                }
            }
            "endscript" => {
                self.active.clear();
                if let Some(vm) = &self.vm {
                    let result = {
                        let vm = vm.lock().unwrap();
                        vm.reset_local().and_then(|_| vm.options())
                    };
                    match result {
                        Ok(options) => self.active = options,
                        Err(e) => self.report_script_error("EndScript", e),
                    }
                }
                self.failed_updates.remove(&3);
                self.pending = 0;
                self.horde_alert = false;
                self.panic_warn = false;
                self.forced_mob = false;
            }
            "disable" | "stop" => {
                self.enabled = false;
                self.pending = 0;
                self.horde_alert = false;
                self.panic_warn = false;
                self.forced_mob = false;
            }
            "enable" | "start" => self.enabled = true,
            "spawnmob" | "createmob" => {
                let size = self.wave_size();
                self.enqueue(size);
                self.forced_mob = self.pending > 0;
            }
            "forcesurvivalstart" => {
                let _ = self.force_survival_start();
            }
            "panicevent" | "forcepanicevent" => {
                // Survival: the map's radio starts the round with this.
                if let Some(s) = self.survival.as_mut() {
                    if !s.running() {
                        s.start(self.clock);
                        self.game_event("survival_round_start", &[]);
                        return;
                    }
                }
                self.pace = Pace::BuildUp;
                self.pace_until = self.clock + 15.;
                let size = self.panic_size();
                self.enqueue(size);
                self.alert_at = self.clock + self.rng.random_float(1., 2.);
                self.mob_start = self.alert_at + self.rng.random_float(2., 4.);
                self.panic_at = self.mob_start;
                self.spawn_at = self.mob_start;
                self.panic_warn |= std::mem::take(&mut self.horde_alert);
                self.forced_mob = self.pending > 0;
            }
            // Owned CDirector::ResetMobTimer (server 10262e90): the mob timer
            // restarts at now with no duration, so the next BUILD_UP check
            // spawns a mob straight away.
            "resetmobtimer" => self.mob_at = self.clock,
            _ => eprintln!("Unimplemented Director input: {input} {param}"),
        }
    }
    pub fn tick(
        &mut self,
        dt: f32,
        health: f32,
        nearby: usize,
        alive: usize,
        position: Vec3,
    ) -> usize {
        self.clock += dt;
        if self.panic_warn && self.clock >= self.alert_at {
            self.panic_warn = false;
            self.horde_alert = true;
        }
        if !self.round_started {
            self.round_started = true;
            self.game_event("round_start", &[]);
        }
        if self.clock >= self.script_at {
            self.script_at = self.clock + 0.1;
            self.update_scripts(alive, nearby);
        }
        if !self.enabled {
            return 0;
        }
        // Survival: nothing before the round starts; then the Survival
        // schedule drives the hordes and the director never relaxes.
        if let Some(s) = self.survival.as_mut() {
            if !s.running() {
                return 0;
            }
            let wave = s.tick(self.clock);
            self.pace = Pace::BuildUp;
            self.pace_until = self.clock + 15.;
            if wave.horde {
                let size = self.panic_size().max(30);
                println!("Survival: horde of {size} (commons alive {alive})");
                self.enqueue(size);
                self.forced_mob = true;
                self.spawn_at = self.clock;
            } else if wave.trickle && self.pending < 6 {
                let size = (self.wave_size() / 3).max(4);
                self.enqueue(size);
                self.forced_mob = true;
            }
        }
        self.finale_tick(alive);
        if !self.initialized {
            self.mob_at = self.clock + self.mob_interval();
            self.initialized = true;
            self.previous_health = health;
        }
        // Retail survivor intensity (1029c740): each survivor decays by
        // dt / intensity_decay_time once its 5 s hold has passed, and the
        // tempo reads the highest. Without survivor samples (unit tests,
        // harnesses) the old player-only estimate is kept.
        let clock = self.clock;
        for s in self.survivor_intensity.values_mut() {
            if clock >= s.hold_until {
                s.value = (s.value - dt / INTENSITY_DECAY_TIME).max(0.);
            }
        }
        let damage = (self.previous_health - health).max(0.);
        self.previous_health = health;
        self.intensity = match self.max_survivor_intensity() {
            Some(max) => max,
            None => (self.intensity + damage / 100. + nearby as f32 * dt * 0.015
                - if nearby == 0 { dt / 30. } else { 0. })
            .clamp(0., 1.5),
        };
        if self.nav_flow.is_some() {
            if let Some(old) = self.previous_flow {
                self.travel += (self.flow - old).max(0.);
            }
            self.previous_flow = Some(self.flow);
        } else if let Some(old) = self.previous_position {
            let delta = (position - old) * Vec3::new(1., 0., 1.);
            if delta.length() < 3. {
                self.travel += delta.length() / crate::collision::UNIT;
            }
        }
        self.previous_position = Some(position);
        let old = self.pace;
        match self.pace {
            Pace::BuildUp
                if self.clock >= self.pace_until
                    && self.intensity >= self.option("IntensityThreshold", 0.9) =>
            {
                self.pace = Pace::SustainPeak;
                self.pace_until =
                    self.clock + self.range("SustainPeakMinTime", "SustainPeakMaxTime", 3., 5.);
            }
            Pace::SustainPeak if self.clock >= self.pace_until => {
                self.pace = Pace::PeakFade;
                self.travel = 0.;
            }
            // 10266b50: no Tank in play and every survivor below
            // IntensityRelaxThreshold, or the team moved on far enough.
            Pace::PeakFade
                if (!self.tank_alive
                    && self.intensity < self.option("IntensityRelaxThreshold", 0.9))
                    || self.travel > self.option("RelaxMaxFlowTravel", 3000.) =>
            {
                self.pace = Pace::Relax;
                self.pace_until =
                    self.clock + self.range("RelaxMinInterval", "RelaxMaxInterval", 30., 45.);
                self.travel = 0.;
            }
            Pace::Relax
                if self.clock >= self.pace_until
                    || self.travel > self.option("RelaxMaxFlowTravel", 3000.) =>
            {
                self.pace = Pace::BuildUp;
                self.pace_until = self.clock + 15.;
                self.travel = 0.;
            }
            _ => {}
        }
        if old != self.pace {
            println!(
                "Director pace: {:?}; intensity={:.2}",
                self.pace, self.intensity
            );
        }
        // L4D2R_DIRECTOR_LOG=1: each survivor's intensity every 5 s.
        if std::env::var_os("L4D2R_DIRECTOR_LOG").is_some() && (self.clock / 5.).floor() != ((self.clock - dt) / 5.).floor() {
            let mut each: Vec<_> = self.survivor_intensity.iter().map(|(id, s)| (*id, s.value)).collect();
            each.sort_by_key(|(id, _)| *id);
            println!("Director {:.0}s {:?} intensity {:?}", self.clock, self.pace, each);
        }
        let spawning = matches!(self.pace, Pace::BuildUp | Pace::SustainPeak);
        if self.option("PanicForever", 0.) > 0.
            && self.pending == 0
            && self.clock >= self.panic_at
            && (self.pace != Pace::Relax || self.option("PausePanicWhenRelaxing", 0.) == 0.)
        {
            let size = self.wave_size();
            self.enqueue(size);
            self.panic_at = self.clock + self.range("MobSpawnMinTime", "MobSpawnMaxTime", 5., 7.);
        }
        if spawning && self.clock >= self.mob_at {
            let size = self.wave_size();
            self.enqueue(size);
            self.mob_at = self.clock + self.mob_interval();
        }
        let room = self.limit().saturating_sub(alive);
        if room == 0 {
            return 0;
        }
        if self.pending > 0
            && self.clock >= self.spawn_at
            && (self.forced_mob
                || self.pace != Pace::Relax
                || (self.option("PanicForever", 0.) > 0.
                    && self.option("PausePanicWhenRelaxing", 0.) == 0.))
        {
            // ZombieManager::StartFrame (1035ed70) spawns up to
            // max(4, 2 x spawn areas) per server frame; with retail's two
            // areas that is 4 a frame at 30 Hz (measured: 12 per 0.1 s).
            self.spawn_at = self.clock + 1. / 30.;
            return room.min(self.pending).min(4);
        }
        0
    }
    fn mob_interval(&mut self) -> f32 {
        let easy = self.difficulty == Difficulty::Easy;
        self.range(
            "MobSpawnMinTime",
            "MobSpawnMaxTime",
            if easy { 120. } else { 90. },
            if easy { 240. } else { 180. },
        )
    }
    pub fn spawned(&mut self) {
        self.pending = self.pending.saturating_sub(1);
        if self.pending == 0 {
            self.forced_mob = false;
        }
    }
    pub fn candidate(&mut self) -> Option<Vec3> {
        if self.nav.is_empty() {
            None
        } else {
            Some(self.nav[self.rng.random_int(0, self.nav.len() as i32 - 1) as usize])
        }
    }
    pub fn spawn_range(&self) -> f32 {
        self.option("ZombieSpawnRange", 1600.).clamp(200., 10000.) * crate::collision::UNIT
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn c1m3_mall_alarm_scripts_spawn_hordes_until_the_alarm_is_switched_off() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let path = root.join("left4dead2/maps/c1m3_mall.bsp");
        let mut d = Director::load(&mut assets, &path, Difficulty::Normal).unwrap();
        // Count commons the Director asks for over `seconds`, all killed at
        // once; script Director calls are routed like game::flush_vscript_commands.
        let run = |d: &mut Director, seconds: f32| {
            let mut spawned = 0;
            for _ in 0..(seconds * 10.) as usize {
                for c in std::mem::take(&mut d.script_commands) {
                    if c.target == "!director" {
                        d.command(&c.input, &c.param);
                    }
                }
                let n = d.tick(0.1, 100., 0, 0, Vec3::ZERO);
                for _ in 0..n {
                    d.spawned();
                }
                spawned += n;
            }
            spawned
        };
        run(&mut d, 5.);
        let calm = run(&mut d, 60.);
        // The alarm door: C1_mall_onslaught (relay_door_minifinale).
        d.command("BeginScript", "C1_mall_onslaught");
        let alarm = run(&mut d, 60.);
        println!("onslaught options {:?}", d.active);
        // The alarm button: c1_mall_crescendo_cooldown.
        d.command("BeginScript", "c1_mall_crescendo_cooldown");
        run(&mut d, 10.);
        let after = run(&mut d, 60.);
        println!("commons per minute: calm {calm}, alarm {alarm}, after the button {after}");
        assert!(alarm >= 40, "the alarm keeps hordes coming ({alarm})");
        assert!(after < alarm / 4, "switching the alarm off stops them ({after})");
    }

    #[test]
    fn c1m4_onslaught_stages_run_their_script_in_the_finale_scope() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let mut d = Director::default();
        d.enabled = true;
        d.map_script = "c1m4_atrium".into();
        for name in ["c1m4_atrium_finale", "c1m4_delay"] {
            let bytes = crate::source_ice::decode(&assets.read(&format!("scripts/vscripts/{name}.nuc")).unwrap());
            d.vm_sources.insert(name.into(), String::from_utf8_lossy(&bytes).into_owned());
        }
        d.initialize_vm().unwrap();
        d.command("BeginFinale", "");
        // Stage 1: ONSLAUGHT "" (no script) until EndCustomScriptedStage.
        assert_eq!(d.finale().map(|f| (f.stage, f.kind)), Some((1, 3)));
        for _ in 0..50 {
            d.tick(0.1, 100., 0, 0, Vec3::ZERO);
        }
        assert_eq!(d.finale().unwrap().stage, 1, "onslaught waits for the script");
        d.command("EndCustomScriptedStage", "");
        assert_eq!(d.finale().map(|f| (f.stage, f.kind)), Some((2, 0)), "PANIC 1");
        d.script_commands.clear();
        // Stage 3: ONSLAUGHT "c1m4_delay" runs the delay script, which
        // enables its timer and redefines the finale's AbortDelay hook.
        let mut guard = 0;
        while d.finale().unwrap().stage == 2 && guard < 2000 {
            d.pending = 0;
            d.tick(0.1, 100., 0, 0, Vec3::ZERO);
            guard += 1;
        }
        assert_eq!(d.finale().map(|f| (f.stage, f.kind)), Some((3, 3)));
        assert!(
            d.script_commands.iter().any(|c| c.target == "timer_delay_end" && c.input.eq_ignore_ascii_case("enable")),
            "{:?}",
            d.script_commands
        );
        // The finale script's own functions survive the stage script.
        let vm = d.vm.as_ref().unwrap().lock().unwrap();
        vm.eval(3, "check", "DirectorScript.MapScript.LocalScript.GasCanPoured()").unwrap();
    }
    #[test]
    fn c3m4_custom_finale_runs_its_stages_then_starts_the_escape() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let script = crate::source_ice::decode(&assets.read("scripts/vscripts/c3m4_plantation_finale.nuc").unwrap());
        let mut d = Director::default();
        d.enabled = true;
        d.map_script = "c3m4_plantation".into();
        d.vm_sources.insert("c3m4_plantation_finale".into(), String::from_utf8_lossy(&script).into_owned());
        d.initialize_vm().unwrap();
        d.command("BeginFinale", "");
        let mut stages = vec![];
        let mut events = vec![];
        for _ in 0..20000 {
            let f = d.finale().unwrap().clone();
            if f.escape {
                break;
            }
            if stages.last() != Some(&(f.stage, f.kind)) {
                stages.push((f.stage, f.kind));
            }
            // Host: spawns queued commons and Tanks; everything dies at once.
            d.pending = 0;
            d.take_tank_requests();
            d.set_tank_alive(false);
            d.tick(0.1, 100., 0, 0, Vec3::ZERO);
            events.extend(d.take_finale_events());
        }
        events.extend(d.take_finale_events());
        // c3m4: PANIC 2, DELAY 12, TANK 1, DELAY 12, PANIC 2, DELAY 15, TANK 2, DELAY 10.
        assert_eq!(
            stages,
            vec![(1, 0), (2, 2), (3, 1), (4, 2), (5, 0), (6, 2), (7, 1), (8, 2)]
        );
        assert!(d.finale().unwrap().escape);
        assert_eq!(events, vec!["FinaleEscapeStarted".to_string()]);
    }
    #[test]
    fn boss_threats_follow_the_retail_witch_tank_nothing_order() {
        let mut d = Director::default();
        for seed in 1..200 {
            d.rng.set_seed(seed);
            d.reset_threats(8);
            let mut order = d.threats;
            assert_ne!(order[0], 8, "a Tank never leads right after a Tank (seed {seed})");
            order.sort();
            assert_eq!(order, [7, 8, 9]);
        }
        // Without this map's threat Tank, a Tank slot is always skipped.
        d.threat_tank_allowed = false;
        for seed in 1..200 {
            d.rng.set_seed(seed);
            d.reset_threats(9);
            assert_ne!(d.current_threat(), 8);
        }
        // With it, a Tank slot is taken about 75% of the time.
        let mut tanks = 0;
        for seed in 1..2001 {
            d.rng.set_seed(seed);
            d.threats = [8, 7, 9];
            d.threat_index = 0;
            d.threat_tank_allowed = true;
            tanks += (d.current_threat() == 8) as i32;
        }
        assert!((1350..1650).contains(&tanks), "{tanks}");
        // A spawned threat Tank uses up the map's Tank and advances the order.
        d.threats = [8, 7, 9];
        d.threat_index = 0;
        d.threat_areas = vec![5];
        d.threat_spawned(5, 8, true);
        assert!(!d.threat_tank_allowed && d.threat_index == 1 && d.threat_areas.is_empty());
        // DisallowThreatType and director_force_witch.
        d.active.insert("DisallowThreatType".into(), 7.);
        d.threats = [7, 9, 8];
        d.threat_index = 0;
        assert_eq!(d.current_threat(), 9);
        d.convar("director_force_witch", "1");
        assert_eq!(d.current_threat(), 7);
    }
    #[test]
    fn threat_areas_are_spaced_along_the_escape_route() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m2_streets.nav");
        let Ok(bytes) = std::fs::read(&path) else { return };
        let mesh = l4d2_formats::nav::parse_nav(&bytes).unwrap();
        let checkpoints: Vec<_> = mesh.areas.iter().filter(|a| a.spawn_attributes & 0x800 != 0).map(|a| a.center()).collect();
        let mut d = Director::default();
        d.nav_flow = Some(crate::source_director_flow::Flow::new(mesh));
        // The start saferoom: the checkpoint area with the longest route out.
        let start = checkpoints
            .into_iter()
            .max_by(|a, b| {
                let mut length = |c: [f32; 3]| {
                    let flow = d.nav_flow.as_mut().unwrap();
                    flow.start(Vec3::from_array(crate::binary::source_position(c)));
                    flow.route_length().unwrap_or(0.)
                };
                length(*a).total_cmp(&length(*b))
            })
            .unwrap();
        for seed in 1..6 {
            d.rng.set_seed(seed);
            d.flow_start(Vec3::from_array(crate::binary::source_position(start)));
            let flow = d.nav_flow.as_ref().unwrap();
            let route = flow.escape_route();
            assert!(route.len() > 10);
            // c1m2's ~20600-unit route: 4 threat areas (20600 / 5000, capped at 4).
            assert!((3..=4).contains(&d.threat_areas.len()), "{:?}", d.threat_areas);
            let spawn = d.spawn_range() / crate::collision::UNIT;
            let mut last = 0.;
            for &a in &d.threat_areas {
                assert!(flow.threat_candidate(a));
                let p = flow.area_world(a);
                let along = flow.at(p).unwrap_or(f32::INFINITY);
                assert!(along >= 2. * spawn - 300., "first threat too early: {along}");
                assert!(along > last, "threats follow the route order");
                last = along;
            }
        }
    }
    #[test]
    fn coop_tank_waits_for_the_checkpoint_interval_and_retail_spacing() {
        let mut d = Director::default();
        d.enabled = true;
        assert!(!d.tank_due(false, false));
        d.clock = 0.;
        assert!(!d.tank_due(true, false), "just left the checkpoint");
        d.clock = 16.;
        assert!(!d.tank_due(true, false), "interval scheduled, not yet due");
        let due = d.tank_at.unwrap();
        assert!((16. + 350. ..=16. + 500.).contains(&due), "{due}");
        d.clock = due + 1.;
        assert!(!d.tank_due(true, true), "one Tank at a time");
        assert!(d.tank_due(true, false));
        assert!(d.tank_at.unwrap() >= d.clock + 350.);
        // ProhibitBosses and director_no_bosses forbid; director_force_tank forces once.
        d.active.insert("ProhibitBosses".into(), 1.);
        d.clock = 1e6;
        assert!(!d.tank_due(true, false));
        d.convar("director_force_tank", "1");
        assert!(d.tank_due(true, false));
        assert!(!d.tank_due(true, false));
    }    #[test]
    fn failed_entity_snapshots_retry_without_running_map_callbacks_early() {
        let root=Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let logic=crate::l4d2r_cinematic::MapLogic::load(root,&crate::test_lab::path(),false).unwrap();
        let mut d=Director::default(); d.vm_sources.insert("keep_vm".into(),"".into()); d.initialize_vm().unwrap();
        d.vm.as_ref().unwrap().lock().unwrap().eval(0,"break snapshot target","__EntityData=null;").unwrap();
        assert!(!d.sync_world(&logic.world));
        assert!(d.entity_cache.is_empty());
        assert!(!d.map_started);
        d.vm.as_ref().unwrap().lock().unwrap().eval(0,"restore snapshot target","__EntityData={};").unwrap();
        assert!(d.sync_world(&logic.world));
        assert_eq!(d.entity_cache.len(),logic.world.ents.len());
        assert!(d.map_started);
        let id=logic.world.find("lab_door")[0];
        d.vm.as_ref().unwrap().lock().unwrap().eval(0,"verify recovered handle",&format!("Assert(__EntityHandle({id}).IsValid());")).unwrap();
    }

    use super::*;
    #[test]
    fn fall_damage_script_convars_change_real_policy_and_reset() {
        let mut d=Director::default();d.vm_sources.insert("fixture".into(),"".into());d.initialize_vm().unwrap();
        let output=d.console_script(r#"Assert(Convars.GetFloat("fall_speed_safe")==560);Assert(Convars.GetFloat("fall_speed_fatal")==720);Convars.SetValue("fall_speed_safe",600);Convars.SetValue("survivor_incap_max_fall_damage",300);"#,false);
        assert!(!output.contains("VScript:"),"{output}");
        for c in std::mem::take(&mut d.script_commands) { if c.target=="!convar" {d.convar(&c.input,&c.param);} }
        assert_eq!(d.fall_damage.damage(600.),0.);assert_eq!(d.fall_damage.max_incap_damage,300.);
        d.reset();assert_eq!(d.fall_damage.safe,560.);assert_eq!(d.fall_damage.max_incap_damage,200.);
    }
    #[test]
    fn console_scripts_share_root_scope_print_output_and_enqueue_real_io() {
        let mut d = Director::default();
        d.vm_sources
            .insert("console_fixture".into(), "printl(consoleValue);".into());
        d.initialize_vm().unwrap();
        assert_eq!(d.console_script("consoleValue <- 42;", false), "");
        assert_eq!(
            d.console_script("printl(consoleValue);", false).trim(),
            "42"
        );
        assert_eq!(d.console_script("console_fixture", true).trim(), "42");
        d.console_script(r#"EntFire("console_door","Open","",0.25);"#, false);
        assert_eq!(d.script_commands.last().unwrap().target, "console_door");
        assert!(
            d.console_script("not_defined()", false)
                .contains("VScript:")
        );
        assert_eq!(
            d.console_script("printl(consoleValue+1);", false).trim(),
            "43"
        );
    }
    #[test]
    fn script_think_uses_world_time_return_interval_removal_and_stops_on_error() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut logic =
            crate::l4d2r_cinematic::MapLogic::load(root, &crate::test_lab::path(), false).unwrap();
        let mut d = Director::default();
        d.vm_sources.insert("keep_vm".into(), "".into());
        d.initialize_vm().unwrap();
        d.sync_world(&logic.world);
        let id = logic.world.find("lab_door")[0];
        d.entity_script(id,"RunScriptCode", "calls <- 0; function Pulse(){calls++; EntFire(\"marker\",\"Pulse\",Time());return 0.25;}");
        d.configure_think(id, "Pulse");
        for delta in [0.09, 0.01, 0.24, 0.02] {
            logic.world.tick(delta);
            d.set_script_time(logic.world.time);
            d.run_thinks(&logic.world);
        }
        assert_eq!(d.script_commands.len(), 2);
        assert!(d.script_commands[0].param.parse::<f32>().unwrap() >= 0.09);
        d.configure_think(id, "");
        logic.world.tick(1.);
        d.set_script_time(logic.world.time);
        d.run_thinks(&logic.world);
        assert_eq!(d.script_commands.len(), 2);
        d.configure_think(id, "Missing");
        logic.world.tick(0.11);
        d.set_script_time(logic.world.time);
        d.run_thinks(&logic.world);
        assert!(!d.thinks.contains_key(&id));
        d.configure_think(id, "Pulse");
        logic.world.ents[id].alive = false;
        d.run_thinks(&logic.world);
        assert!(!d.thinks.contains_key(&id));
    }
    #[test]
    fn output_binding_and_self_io_keep_caller_and_activator_and_disconnect() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut logic =
            crate::l4d2r_cinematic::MapLogic::load(root, &crate::test_lab::path(), false).unwrap();
        let mut d = Director::default();
        d.vm_sources.insert("keep_vm".into(), "".into());
        d.initialize_vm().unwrap();
        d.sync_world(&logic.world);
        let id = logic.world.find("lab_door")[0];
        let activator = logic.world.find("lab_relay")[0];
        d.entity_script(id,"RunScriptCode", "function Opened(){ Assert(caller.id==self.id); EntFire(\"!activator\",\"FireUser1\",\"done\",0,activator); } function Reflected(){EntFire(\"marker\",\"Verified\",\"yes\");} self.ConnectOutput(\"OnOpen\",\"Opened\");");
        logic.world.input(
            activator,
            "AddOutput",
            "OnUser1 lab_door:CallScriptFunction:Reflected:0:-1",
            None,
            None,
        );
        let command = d.script_commands.pop().unwrap();
        assert_eq!(command.target, "!connect");
        logic
            .world
            .connect_script_output(id, "OnOpen", &command.param, true);
        logic.world.fire_output(id, "OnOpen", Some(activator));
        logic.world.flush_io();
        for request in std::mem::take(&mut logic.world.cine.script_commands) {
            d.entity_script_context(
                request.entity,
                &request.input,
                &request.param,
                request.activator,
                request.caller,
            );
        }
        assert!(d.script_errors.is_empty(), "{:?}", d.script_errors);
        let command = d.script_commands.pop().unwrap();
        assert_eq!(command.activator, Some(activator));
        assert_eq!(command.caller, Some(id));
        logic.world.queue_input_context(
            &command.target,
            &command.input,
            &command.param,
            0.,
            command.activator,
            command.caller,
        );
        logic.world.flush_io();
        for request in std::mem::take(&mut logic.world.cine.script_commands) {
            d.entity_script_context(
                request.entity,
                &request.input,
                &request.param,
                request.activator,
                request.caller,
            );
        }
        assert!(
            d.script_commands
                .iter()
                .any(|c| c.target == "marker" && c.input == "Verified" && c.param == "yes")
        );
        logic
            .world
            .connect_script_output(id, "OnOpen", "Opened", false);
        logic.world.fire_output(id, "OnOpen", Some(activator));
        logic.world.flush_io();
        assert!(logic.world.cine.script_commands.is_empty());
    }
    #[test]
    fn entity_scope_named_function_and_delayed_io_open_the_real_lab_door() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut logic =
            crate::l4d2r_cinematic::MapLogic::load(root, &crate::test_lab::path(), true).unwrap();
        let mut d = Director::default();
        d.vm_sources.insert(
            "fixture".into(),
            "function OpenLater(){DoEntFire(\"!self\",\"Open\",\"\",0.25,self,self);}".into(),
        );
        d.initialize_vm().unwrap();
        d.sync_world(&logic.world);
        let id = logic.world.find("lab_door")[0];
        logic
            .world
            .input(id, "RunScriptFile", "fixture", None, None);
        logic
            .world
            .input(id, "CallScriptFunction", "OpenLater", None, None);
        for request in std::mem::take(&mut logic.world.cine.script_commands) {
            d.entity_script_context(
                request.entity,
                &request.input,
                &request.param,
                request.activator,
                request.caller,
            );
        }
        assert!(d.script_errors.is_empty(), "{:?}", d.script_errors);
        assert_eq!(d.script_commands.len(), 1);
        let command = d.script_commands.pop().unwrap();
        logic.world.queue_input_context(
            &command.target,
            &command.input,
            &command.param,
            command.delay,
            command.activator,
            command.caller,
        );
        logic.world.tick(0.24);
        assert!(
            matches!(&logic.world.ents[id].kind,l4d2r_game_core::entities::Kind::Door(door) if door.state==l4d2r_game_core::entities::DoorState::Closed)
        );
        logic.world.tick(0.02);
        assert!(
            matches!(&logic.world.ents[id].kind,l4d2r_game_core::entities::Kind::Door(door) if door.state!=l4d2r_game_core::entities::DoorState::Closed)
        );
    }
    #[test]
    fn horde_alert_is_one_event_per_wave_not_per_ambient_spawn() {
        let mut d = Director::default();
        for _ in 0..60 {
            d.tick(0.25, 100., 0, 0, Vec3::ZERO);
            d.spawned();
            assert!(!d.take_horde_alert());
        }
        d.command("ForcePanicEvent", "");
        // MegaMobIncoming waits out STAGE_INITIAL_DELAY (1 to 2 s).
        assert!(!d.take_horde_alert());
        d.tick(0.9, 100., 0, 30, Vec3::ZERO);
        assert!(!d.take_horde_alert());
        d.tick(1.2, 100., 0, 30, Vec3::ZERO);
        assert!(d.take_horde_alert());
        assert!(!d.take_horde_alert());
        d.command("ForcePanicEvent", "");
        d.tick(2.1, 100., 0, 30, Vec3::ZERO);
        assert!(!d.take_horde_alert());
        while d.pending > 0 {
            d.spawned();
            assert!(!d.take_horde_alert());
        }
        d.command("SpawnMob", "");
        assert!(d.take_horde_alert());
        assert!(!d.take_horde_alert());
        d.command("Disable", "");
        assert!(!d.take_horde_alert());
        d.command("ForcePanicEvent", "");
        d.reset();
        d.tick(2.1, 100., 0, 30, Vec3::ZERO);
        assert!(!d.take_horde_alert());
    }
    #[test]
    fn script_precedence_and_pending_cap_survive_begin_end_and_reset() {
        let mut d = Director::default();
        d.base = HashMap::from([
            ("CommonLimit".into(), 25.),
            ("MobSpawnSize".into(), 12.),
            ("MegaMobSize".into(), 12.),
            ("MobMaxPending".into(), 20.),
        ]);
        d.scripts.insert(
            "local".into(),
            script("DirectorOptions <- { CommonLimit=8, MobSpawnSize=99, MegaMobSize=6, MobMaxPending=10 }"),
        );
        d.command("ForcePanicEvent", "");
        assert_eq!(d.pending, 12);
        d.command("ForcePanicEvent", "");
        assert_eq!(d.pending, 20);
        assert_eq!(d.tick(6.1, 100., 0, 19, Vec3::ZERO), 4);
        d.command("BeginScript", "scripts/vscripts/LOCAL.NUT");
        assert_eq!(d.limit(), 8);
        d.command("ForcePanicEvent", "");
        assert_eq!(d.pending, 6);
        d.command("ForcePanicEvent", "");
        assert_eq!(d.pending, 10);
        assert_eq!(d.tick(0.02, 100., 0, 8, Vec3::ZERO), 0);
        d.command("EndScript", "");
        assert_eq!(d.limit(), 25);
        assert_eq!(d.pending, 0);
        d.command("BeginScript", "local");
        d.reset();
        assert_eq!(d.limit(), 25);
        assert_eq!(d.pending, 0);
        d.base.insert("MobMaxPending".into(), 0.);
        d.enqueue(12);
        d.enqueue(12);
        assert_eq!(d.pending, 24);
        d.base.insert("MegaMobSize".into(), 0.);
        d.pending = 0;
        d.command("ForcePanicEvent", "");
        assert_eq!(d.pending, 0);
        assert_eq!(d.wave_size(), 12);
    }
    #[test]
    fn forced_mob_starts_after_the_initial_delay_and_respects_population_and_disable() {
        let mut d = Director::default();
        d.command("ForcePanicEvent", "");
        let pending = d.pending;
        assert!(pending >= 10);
        // STAGE_INITIAL_DELAY RandomFloat(1, 2) s, when the wanderers join,
        // then the mob timer RandomFloat(2, 4) s before SpawnMob.
        assert_eq!(d.tick(0.9, 100., 0, 0, Vec3::ZERO), 0);
        assert!(!d.mob_active());
        assert_eq!(d.tick(1.2, 100., 0, 0, Vec3::ZERO), 0);
        assert!(d.mob_active());
        assert_eq!(d.tick(0.8, 100., 0, 0, Vec3::ZERO), 0);
        assert_eq!(d.tick(3.2, 100., 0, 0, Vec3::ZERO), 4);
        for _ in 0..4 {
            d.spawned();
        }
        assert_eq!(d.pending, pending - 4);
        d.pace = Pace::Relax;
        assert_eq!(d.tick(0.034, 100., 0, 0, Vec3::ZERO), 4);
        assert_eq!(d.tick(0.11, 100., 0, 30, Vec3::ZERO), 0);
        d.command("Disable", "");
        assert_eq!(d.pending, 0);
        assert_eq!(d.tick(20., 100., 0, 0, Vec3::ZERO), 0);
    }
    #[test]
    fn owned_mutation_item_callback_and_temporary_health_rule_reach_director() {
        let root = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let bytes = assets.read("scripts/vscripts/community5.nuc").unwrap();
        let code = String::from_utf8(crate::source_ice::decode(&bytes)).unwrap();
        let mut d = Director::default();
        d.vm_sources.insert("community5".into(), code);
        d.session_start_map(true);
        d.initialize_vm().unwrap();
        d.challenge_options("community5").unwrap();
        assert!(!d.allow_fallen_item("weapon_first_aid_kit"));
        assert!(d.allow_fallen_item("weapon_pipe_bomb"));
        assert_eq!(d.active["SurvivorMaxIncapacitatedCount"], 0.);
        d.game_event("round_start", &[]);
        let commands = std::mem::take(&mut d.script_commands);
        for command in commands {
            if command.target == "!convar" {
                d.convar(&command.input, &command.param);
            }
        }
        assert_eq!(d.temporary_decay, 0.);
        d.reset();
        assert!(!d.allow_fallen_item("weapon_first_aid_kit"));
        d.active.insert("cm_TempHealthOnly".into(), 1.);
        assert!(!d.allow_fallen_item("weapon_first_aid_kit"));
    }
    #[test]
    fn quiet_and_restore_limit_and_disable_panic() {
        let s = script(
            "DirectorOptions <- { CommonLimit = 0, ProhibitBosses = true } function Later() { DirectorOptions <- { CommonLimit = 100 } }",
        );
        assert_eq!(s.options["CommonLimit"], 0.);
        let mut d = Director::default();
        d.scripts.insert("quiet".into(), s);
        d.command("beginscript", "quiet");
        d.command("forcepanicevent", "");
        assert_eq!(d.tick(20., 100., 0, 0, Vec3::ZERO), 0);
        d.command("endscript", "");
        assert_eq!(d.limit(), 30);
        d.command("disable", "");
        assert_eq!(d.tick(20., 100., 0, 0, Vec3::ZERO), 0);
    }
    #[test]
    fn survivor_intensity_follows_retail_events() {
        let mut d = Director::default();
        let sample = |health: f32, incap: bool| SurvivorSample {
            id: 1,
            position: Vec3::ZERO,
            health,
            incap,
            incap_health: 300.,
            ledge: false,
            dead: false,
        };
        d.observe_intensity(&[sample(100., false)]);
        assert_eq!(d.max_survivor_intensity(), Some(0.));
        // 10 damage of 100: minor pain, 0.25 x 0.2.
        d.observe_intensity(&[sample(90., false)]);
        assert!((d.max_survivor_intensity().unwrap() - 0.05).abs() < 1e-6);
        // 30 damage: major, 0.25 x 0.5.
        d.observe_intensity(&[sample(60., false)]);
        assert!((d.max_survivor_intensity().unwrap() - 0.175).abs() < 1e-6);
        // Held for 5 s, then 1/30 per second.
        d.tick(4., 100., 0, 0, Vec3::ZERO);
        assert!((d.max_survivor_intensity().unwrap() - 0.175).abs() < 1e-6);
        d.tick(2., 100., 0, 0, Vec3::ZERO);
        assert!((d.max_survivor_intensity().unwrap() - (0.175 - 2. / 30.)).abs() < 1e-5);
        // Incapacitation goes to the maximum.
        d.observe_intensity(&[sample(0., true)]);
        assert_eq!(d.max_survivor_intensity(), Some(1.));
        // Deaths: a Witch 300 units away is major for a calm survivor.
        let mut d = Director::default();
        d.observe_intensity(&[sample(100., false)]);
        d.enemy_killed(Vec3::X * 300. * crate::collision::UNIT, Killed::Boss);
        assert!((d.max_survivor_intensity().unwrap() - 0.125).abs() < 1e-6);
        d.enemy_killed(Vec3::X * 600. * crate::collision::UNIT, Killed::Boss);
        assert!((d.max_survivor_intensity().unwrap() - 0.125).abs() < 1e-6);
        d.enemy_killed(Vec3::X * 100. * crate::collision::UNIT, Killed::Common { fast: true });
        assert!((d.max_survivor_intensity().unwrap() - 0.175).abs() < 1e-6);
    }
    #[test]
    fn peak_fade_waits_for_the_tank_not_for_nearby_infected() {
        let mut d = Director::default();
        let calm = SurvivorSample {
            id: 1,
            position: Vec3::ZERO,
            health: 100.,
            incap: false,
            incap_health: 300.,
            ledge: false,
            dead: false,
        };
        d.observe_intensity(&[calm]);
        d.pace = Pace::PeakFade;
        d.set_tank_alive(true);
        d.tick(0.1, 100., 0, 0, Vec3::ZERO);
        assert_eq!(d.pace, Pace::PeakFade);
        d.set_tank_alive(false);
        d.tick(0.1, 100., 25, 0, Vec3::ZERO);
        assert_eq!(d.pace, Pace::Relax, "infected nearby do not hold PEAK_FADE in retail");
    }
    #[test]
    fn specials_spawn_one_at_a_time_and_slots_reopen_after_deaths() {
        use crate::special_infected::SpecialClass;
        let mut d = Director::default();
        d.clock = 0.;
        // Before any class timer (30-60 s) nothing spawns.
        assert!(d.special_wave([0; 6], true).is_empty());
        d.clock = 61.;
        let first = d.special_wave([0; 6], true);
        assert_eq!(first.len(), 1, "one special per attempt");
        let i = SpecialClass::MINIONS.iter().position(|c| *c == first[0]).unwrap();
        let mut live = [0; 6];
        live[i] = 1;
        // Fill to z_minion_limit 3.
        for _ in 0..2 {
            let next = d.special_wave(live, true);
            assert_eq!(next.len(), 1);
            let k = SpecialClass::MINIONS.iter().position(|c| *c == next[0]).unwrap();
            assert_eq!(live[k], 0, "z_<class>_limit 1");
            live[k] = 1;
        }
        assert!(d.special_wave(live, true).is_empty(), "three live");
        // One dies: its slot and its class wait SpecialRespawnInterval.
        live[i] = 0;
        d.clock = 70.;
        assert!(d.special_wave(live, true).is_empty());
        d.clock = 114.;
        assert!(d.special_wave(live, true).is_empty());
        d.clock = 116.;
        assert_eq!(d.special_wave(live, true).len(), 1);
    }
    #[test]
    fn peak_fades_and_relaxes_before_new_build_up() {
        let mut d = Director::default();
        d.tick(16., 1., 4, 30, Vec3::ZERO);
        d.tick(1., 0., 100, 30, Vec3::ZERO);
        assert_eq!(d.pace, Pace::SustainPeak);
        d.tick(6., 100., 0, 30, Vec3::ZERO);
        assert_eq!(d.pace, Pace::PeakFade);
        d.tick(50., 100., 0, 30, Vec3::ZERO);
        assert_eq!(d.pace, Pace::Relax);
        d.tick(50., 100., 0, 30, Vec3::ZERO);
        assert_eq!(d.pace, Pace::BuildUp);
    }
}
