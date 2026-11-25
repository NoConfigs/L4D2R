//! Map entity logic: the Source I/O system (outputs -> delayed inputs) and the behaviours the campaign maps rely on:
//! logic_auto/relay/timer/case/branch, triggers, doors, buttons. Written from the Source entity semantics and
//! cross-checked against the decompiled VMFs; class behaviours are UNVERIFIED against server.dll except where noted
//! in docs/re/entities.md.

use crate::mapents::{angle_matrix, MapEnt};
use crate::random::UniformRandomStream;
use glam::{Mat4, Vec3};
use l4d2_formats::bsp::Bsp;
use l4d2_formats::vmf::Connection;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// Source's `developer` level, set by the gameplay console.
pub static DEVELOPER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// DevMsg lines for the gameplay console, drained by the rewrite each frame.
pub static DEV_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
/// Source DevMsg(level, ...): kept only when `developer` is at least `level`.
pub fn dev_msg(level: u32, line: String) {
    if DEVELOPER.load(std::sync::atomic::Ordering::Relaxed) >= level {
        if let Ok(mut log) = DEV_LOG.lock() {
            if log.len() < 4096 {
                log.push(line);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Durability {
    pub health: f32,
    pub maximum: f32,
    pub bullets: f32,
    pub club: f32,
    pub explosive: f32,
    pub fire: f32,
    pub sound: String,
}

#[derive(Clone, Copy, Debug)]
pub enum DamageKind {
    Bullet,
    Club,
    Burn,
    Blast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorState {
    Closed,
    Opening,
    Open,
    Closing,
}

#[derive(Clone, Debug)]
pub struct Door {
    pub rotating: bool,
    pub state: DoorState,
    /// 0 = closed, 1 = fully open.
    pub frac: f32,
    /// Units/s (or deg/s).
    pub speed: f32,
    /// Travel distance (or degrees).
    pub dist: f32,
    /// Signed yaw direction for the model's native forward swing.
    pub forward_sign: f32,
    /// Unit travel direction (linear) or rotation axis (rotating).
    pub dir: Vec3,
    /// Seconds to stay open (`wait`); -1 = stay open.
    pub wait: f32,
    pub close_at: f32,
    pub locked: bool,
    pub passable: bool,
    /// Target fraction of the current move (1 = open, 0 = closed, or a `SetPosition` value).
    pub goal: f32,
}

#[derive(Clone, Debug)]
pub enum Kind {
    None,
    Auto,
    Relay {
        disabled: bool,
    },
    Timer {
        enabled: bool,
        next: f32,
        refire: f32,
        lo: f32,
        hi: f32,
        random: bool,
    },
    Case {
        values: Vec<String>,
    },
    Branch {
        value: bool,
    },
    Door(Door),
    Button {
        locked: bool,
        pressed: bool,
        wait: f32,
        ready_at: f32,
    },
    /// func_button_timed: held with use for `use_time` seconds (the host
    /// drives the hold and fires OnPressed / OnUnPressed / OnTimeUp).
    TimedButton {
        disabled: bool,
    },
    Trigger {
        disabled: bool,
        fired: bool,
        touching: bool,
        ready_at: f32,
    },
    /// func_brush / prop_dynamic style visibility + collision toggles.
    Brush {
        enabled: bool,
        solid: bool,
    },
    /// trigger_finale: started by `ForceFinaleStart`/`UseFinale`; the sim runs the panic waves and escape.
    /// `ftype` = the entity's `type` key (2 stage/vehicle finales, 4 gas-can scavenge finale, ...).
    Finale {
        started: bool,
        phase: u8,
        next_at: f32,
        waves_left: u32,
        ftype: i32,
        /// `StartDisabled` / Enable / Disable: survivors cannot use it.
        disabled: bool,
        /// FirstUseStart already fired.
        used: bool,
    },
    /// ambient_generic: sound source (`message`), looping unless flag 32.
    Ambient {
        playing: bool,
    },
    /// env_tonemap_controller: auto-exposure parameters set through inputs (client.dll/engine consume them).
    Tonemap,
    ScavengeProgress { enabled: bool, total: u32, collected: u32 },
    /// prop_car_alarm: set off once by a survivor's damage or touch (a panic
    /// event); `until` is when `OnCarAlarmEnd` fires. Chirps warn survivors
    /// who come near an armed car.
    CarAlarm {
        armed: bool,
        until: f32,
        triggered: bool,
        /// When the current chirp ends (`OnCarAlarmChirpEnd`), and the
        /// earliest next chirp.
        chirp_until: f32,
        next_chirp: f32,
    },
    /// math_counter: integer with `OnHitMax`/`OnHitMin`/`OutValue` outputs.
    Counter {
        value: f32,
        min: f32,
        max: f32,
    },
    /// func_nav_blocker: toggled with Block/UnblockNav (the sim applies it to the nav graph).
    NavBlocker {
        blocked: bool,
    },
    /// trigger_hurt: damages whatever stands in it (`damage` per second, applied every 0.5 s).
    Hurt {
        disabled: bool,
        damage: f32,
    },
    /// trigger_push: base-velocity push (`dir` * `speed` u/s).
    Push {
        disabled: bool,
        dir: Vec3,
        speed: f32,
    },
    /// func_breakable / prop_breakable: breaks (fires `OnBreak`, disappears) when health runs out.
    Breakable {
        health: f32,
    },
    Particle {
        active: bool,
        started: f32,
    },
    Fire {
        active: bool,
        size: f32,
        damage: f32,
        extinguish_at: f32,
    },
    /// func_elevator: translation `offset` from its map position, moving toward `target` (a floor) at `speed` u/s.
    Elevator {
        offset: Vec3,
        target: Vec3,
        speed: f32,
        moving: bool,
        top: Vec3,
        bottom: Vec3,
    },
    /// func_tracktrain: travels between `path_track` corners (translation
    /// only; turning and banking are not modelled). Speeds are units/s.
    Train {
        offset: Vec3,
        speed: f32,
        /// Speed before the last stop, restored by Start* inputs.
        old_speed: f32,
        /// `startspeed`: SetSpeed's ratio is of this.
        max_speed: f32,
        forward: bool,
        moving: bool,
        /// The path_track the train sits on or last passed.
        at: Option<usize>,
    },
}

#[derive(Clone, Debug)]
pub struct Ent {
    pub me: MapEnt,
    pub conns: Vec<Connection>,
    pub kind: Kind,
    pub alive: bool,
    /// Referenced by a `point_template`: dormant until that template spawns.
    pub templated: bool,
    /// `parentname` target: this entity follows its parent's motion.
    pub parent: Option<usize>,
}

#[derive(Clone, Debug)]
pub enum EntEvent {
    SurfaceDamage { ent: usize, damage: f32, kind: DamageKind },
    SurfaceShatter { ent: usize, point_radius: Vec3 },
    Broken {
        ent: usize,
    },
    Hurt {
        ent: usize,
        damage: f32,
    },
    /// Brush/prop entity moved: new world transform.
    Moved {
        ent: usize,
        matrix: Mat4,
    },
    /// Collision should be toggled for this entity.
    Solid {
        ent: usize,
        solid: bool,
    },
    /// Draw/hide an entity.
    Visible {
        ent: usize,
        visible: bool,
    },
    /// ambient_generic start/stop.
    Sound {
        ent: usize,
        play: bool,
    },
    HudHint {
        ent: usize,
        show: bool,
    },
    DoorSound {
        ent: usize,
        output: String,
    },
    /// A templated entity became active (point_template spawn).
    Spawned {
        ent: usize,
    },
    /// A logged I/O message (for tests/debugging).
    Fired {
        src: usize,
        output: String,
    },
}

struct Pending {
    time: f32,
    seq: u64,
    target: String,
    input: String,
    param: String,
    actor: Option<u64>,
    activator: Option<usize>,
    caller: Option<usize>,
}
impl PartialEq for Pending {
    fn eq(&self, o: &Self) -> bool {
        self.seq == o.seq
    }
}
impl Eq for Pending {}
impl Ord for Pending {
    fn cmp(&self, o: &Self) -> Ordering {
        o.time
            .partial_cmp(&self.time)
            .unwrap_or(Ordering::Equal)
            .then(o.seq.cmp(&self.seq))
    }
}
impl PartialOrd for Pending {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

pub struct EntityWorld {
    pub bsp: std::sync::Arc<Bsp>,
    pub ents: Vec<Ent>,
    // Parenting offsets refer to spawn transforms, not the parent's mutable
    // current origin (physics updates that origin directly).
    parent_bind: Vec<Mat4>,
    names: HashMap<String, Vec<usize>>,
    queue: BinaryHeap<Pending>,
    pub time: f32,
    frame_dt: f32,
    seq: u64,
    pub events: Vec<EntEvent>,
    rng: UniformRandomStream,
    /// Summed health of the living, standing survivors (logic_director_query
    /// anger); None before the host reports it (treated as 4 x 100).
    pub survivor_health: Option<f32>,
    /// Game mode is versus or scavenge (logic_director_query answers its minimum).
    pub competitive: bool,
    /// Entities that fire `OnUser1..4`/inputs the sim wants to see (name, input, param): game-level hooks.
    pub unhandled: Vec<(usize, String, String)>,
    pub tonemap: Tonemap,
    /// Local player's explicit SetFogController input (camera adapter consumes it).
    pub player_fog_controller: Option<usize>,
    pub player_fog_revision: u64,
    pub fog: crate::fog::FogSystem,
    io_actor: Option<u64>,
    pub cine: Cine,
    pub durability: HashMap<usize, Durability>,
    touchers: HashMap<usize, HashSet<u64>>,
    /// Every accepted `use_ent` (entity, the opener's position for a prop
    /// door), for multiplayer to replay in the other games.
    pub uses: Vec<(usize, Option<Vec3>)>,
    opener: Option<(usize, Vec3)>,
}

/// `env_fade` in progress.
#[derive(Clone, Copy, Debug)]
pub struct Fade {
    pub start: f32,
    pub hold: f32,
    pub duration: f32,
    pub color: [f32; 3],
    pub alpha: f32,
    /// spawnflag 1: fade *from* the colour to clear (otherwise to the colour).
    pub from: bool,
    /// spawnflag 4: stay faded after the effect ends.
    pub stay: bool,
}

impl Fade {
    /// Overlay alpha (0..1) at `now`.
    pub fn alpha_at(&self, now: f32) -> f32 {
        let t = now - self.start;
        if t < 0.0 {
            return 0.0;
        }
        let a = self.alpha / 255.0;
        if self.from {
            if t < self.hold {
                a
            } else {
                (a * (1.0 - (t - self.hold) / self.duration.max(1e-3))).max(0.0)
            }
        } else if t < self.duration {
            a * t / self.duration.max(1e-3)
        } else if t < self.duration + self.hold || self.stay {
            a
        } else {
            0.0
        }
    }
}

/// Cinematic state driven by map I/O (the c1m1 intro): scripted prop animations, survivor view-control cameras, screen fades.
#[derive(Default)]
pub struct Cine {
    /// `prop_dynamic` SetAnimation: entity -> (sequence name, start time).
    pub anims: HashMap<usize, (String, f32)>,
    /// point_viewcontrol_survivor currently driving the player's view.
    pub view_control: Option<usize>,
    pub selected_survivor: Option<String>,
    /// Authored camera handoff: camera, start time, duration.
    pub view_movement: Option<(usize, f32, f32)>,
    pub parent_attach: HashMap<usize, String>,
    pub fade: Option<Fade>,
    /// `info_director` Start/FinishIntro.
    pub intro: bool,
    /// Set by `ForceSurvivorPositions`; consumed by the sim (teleports survivors to `info_survivor_position`).
    pub force_positions: bool,
    pub release_positions: bool,
    /// trigger_finale escape inputs (map I/O from the rescue-vehicle relays).
    pub finale_vehicle_ready: bool,
    pub finale_force_positions: bool,
    /// FinaleEscapeFinished: the survivors escaped (FinaleWon fired).
    pub finale_won: bool,
    pub rescue_vehicle_nav_unblocked: bool,
    /// `info_director` FireConceptTo*: (target survivor or None for any, concept).
    pub concepts: Vec<(Option<String>, String)>,
    /// Script-spawned item spawners told to SpawnItem (c14m2's gas cans):
    /// the host creates their pickups.
    pub item_spawns: Vec<usize>,
    /// `info_zombie_spawn` told to SpawnZombie: the game spawns its
    /// `population` (e.g. c6m1's witch_bride) at the entity.
    pub zombie_spawns: Vec<usize>,
    /// `logic_choreographed_scene` Start: entities whose scene begins this tick.
    pub scene_starts: Vec<usize>,
    pub director_commands: Vec<(String, String)>,
    pub script_commands: Vec<ScriptRequest>,
}
#[derive(Clone, Debug)]
pub struct ScriptRequest {
    pub entity: usize,
    pub input: String,
    pub param: String,
    pub activator: Option<usize>,
    pub caller: Option<usize>,
}

/// `env_tonemap_controller` state (SDK2013 defaults; the map sets these via I/O).
#[derive(Clone, Copy, Debug)]
pub struct Tonemap {
    pub auto_min: Option<f32>,
    pub auto_max: Option<f32>,
    pub percent_target: f32,
    pub percent_bright_pixels: f32,
    pub min_avg_lum: f32,
    pub rate: f32,
    pub bloom_scale: Option<f32>,
}

impl Default for Tonemap {
    fn default() -> Self {
        Tonemap {
            auto_min: None,
            auto_max: None,
            // client.dll's values when no controller overrides them.
            percent_target: 45.0,
            percent_bright_pixels: 1.0,
            min_avg_lum: 3.0,
            rate: 1.0,
            bloom_scale: None,
        }
    }
}

fn truthy(v: &str) -> bool {
    v.trim().parse::<i32>().map(|n| n != 0).unwrap_or(false)
}
fn target_matches(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        pattern == name
    }
}

impl EntityWorld {
    pub fn new(bsp: std::sync::Arc<Bsp>, map_ents: Vec<MapEnt>, seed: i32) -> EntityWorld {
        let mut ents = Vec::with_capacity(map_ents.len());
        for me in map_ents {
            // Connections live as key/values whose value has 4 separators (0x1B in compiled maps).
            let conns: Vec<Connection> = me
                .ent
                .0
                .iter()
                // Outputs not named On*/Out* (trigger_finale's FinaleStart,
                // UseStart, FinaleEscapeStarted, FinaleWon...) are still
                // connections: compiled maps separate output fields with 0x1B,
                // which no plain key/value uses.
                .filter_map(|(k, v)| Some((Connection::parse(k, v)?, v.contains('\u{1b}'))))
                .filter(|(c, compiled)| *compiled || k_is_output(&c.output))
                .map(|(c, _)| c)
                .collect();
            let kind = initial_kind(&me, &bsp);
            ents.push(Ent {
                me,
                conns,
                kind,
                alive: true,
                templated: false,
                parent: None,
            });
        }
        // point_template: referenced entities stay dormant until ForceSpawn.
        let mut template_names: Vec<String> = Vec::new();
        for e in &ents {
            if e.me.classname == "point_template" && e.me.spawnflags() & 1 == 0 {
                for k in 1..=16 {
                    if let Some(n) = e.me.kv(&format!("Template{k:02}")) {
                        template_names.push(n.to_ascii_lowercase());
                    }
                }
            }
        }
        for e in ents.iter_mut() {
            if !e.me.targetname.is_empty()
                && e.me.classname != "point_template"
                && template_names
                    .iter()
                    .any(|n| target_matches(n, &e.me.targetname))
            {
                e.alive = false;
                e.templated = true;
            }
        }
        let mut names: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, e) in ents.iter().enumerate() {
            if !e.me.targetname.is_empty() {
                names
                    .entry(e.me.targetname.to_ascii_lowercase())
                    .or_default()
                    .push(i);
            }
        }
        // Parenting (`parentname` may carry an attachment after a comma).
        let parents: Vec<Option<usize>> = ents
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let pn =
                    e.me.kv("parentname")?
                        .split(',')
                        .next()?
                        .trim()
                        .to_ascii_lowercase();
                names.get(&pn)?.iter().copied().find(|&p| p != i)
            })
            .collect();
        for (e, p) in ents.iter_mut().zip(parents) {
            e.parent = p;
        }
        let parent_bind = ents.iter().map(|e| crate::mapents::entity_matrix(e.me.origin, e.me.angles)).collect();
        let fog = crate::fog::FogSystem::new(&ents);
        EntityWorld {
            bsp,
            ents,
            parent_bind,
            names,
            queue: BinaryHeap::new(),
            time: 0.0,
            frame_dt: 1. / crate::TICK_RATE as f32,
            seq: 0,
            events: Vec::new(),
            rng: UniformRandomStream::new(seed),
            survivor_health: None,
            competitive: false,
            unhandled: Vec::new(),
            tonemap: Tonemap::default(),
            player_fog_controller: None,
            player_fog_revision: 0,
            fog,
            io_actor: None,
            cine: Cine::default(),
            durability: HashMap::new(),
            touchers: HashMap::new(),
            uses: Vec::new(),
            opener: None,
        }
    }

    /// Fire `OnMapSpawn`/`OnNewGame` on logic_auto and `OnSpawn` where present (call once after loading).
    pub fn map_spawn(&mut self, gamemode_output: &str) {
        for (id,e) in self.ents.iter().enumerate() {
            if e.me.classname.starts_with("prop_dynamic") {
                if let Some(name)=e.me.kv("DefaultAnim").filter(|s|!s.is_empty()) {
                    self.cine.anims.insert(id,(name.into(),self.time));
                }
            }
        }
        for (id, e) in self.ents.iter().enumerate() {
            if e.alive {
                // logic_script: its Group00..Group15 entities are the
                // script's EntityGroup array (jukebox_dlc1 uses it).
                if e.me.classname == "logic_script" && !e.me.kv("vscripts").unwrap_or("").trim().is_empty() {
                    // All 16 slots, null where a group is unset.
                    let code = format!(
                        "EntityGroup <- [{}];",
                        (0..16)
                            .map(|k| match e.me.kv(&format!("Group{k:02}")).filter(|n| !n.is_empty()) {
                                Some(n) => format!("Entities.FindByName(null, \"{}\")", n.replace('"', "")),
                                None => "null".into(),
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    self.cine.script_commands.push(ScriptRequest {
                        entity: id,
                        input: "RunScriptCode".into(),
                        param: code,
                        activator: None,
                        caller: None,
                    });
                }
                for script in e.me.kv("vscripts").unwrap_or("").split_whitespace() {
                    self.cine.script_commands.push(ScriptRequest {
                        entity: id,
                        input: "RunScriptFile".into(),
                        param: script.into(),
                        activator: None,
                        caller: None,
                    });
                }
                if let Some(function) = e.me.kv("thinkfunction").filter(|s| !s.is_empty()) {
                    self.cine.script_commands.push(ScriptRequest {
                        entity: id,
                        input: "RegisterThink".into(),
                        param: function.into(),
                        activator: None,
                        caller: None,
                    });
                }
            }
        }
        // ambient_generic without "start silent" (16) plays from the start.
        for i in 0..self.ents.len() {
            if self.ents[i].alive
                && matches!(self.ents[i].kind, Kind::Ambient { .. })
                && self.ents[i].me.spawnflags() & 16 == 0
                && !self.ents[i].me.kv("message").unwrap_or("").is_empty()
            {
                if let Kind::Ambient { playing } = &mut self.ents[i].kind {
                    *playing = true;
                }
                self.events.push(EntEvent::Sound { ent: i, play: true });
            }
        }
        // Game mode (`mp_gamemode`): campaign (coop) / versus / survival / scavenge. info_gamemode outputs toggle mode-specific entities (scavenge boundaries, versus weapons...).
        for i in 0..self.ents.len() {
            if self.ents[i].alive && self.ents[i].me.classname == "info_gamemode" {
                self.fire_output(i, gamemode_output, None);
            }
        }
        for i in 0..self.ents.len() {
            if self.ents[i].alive && matches!(self.ents[i].kind, Kind::Auto) {
                self.fire_output(i, "OnMapSpawn", None);
                self.fire_output(i, "OnNewGame", None);
                // CLogicAuto: L4D2 is always multiplayer, so a new map also
                // fires OnMultiNewMap (c4m5 spawns its finale radio there).
                self.fire_output(i, "OnMultiNewMap", None);
                if self.ents[i].me.spawnflags() & 1 != 0 {
                    self.ents[i].alive = false;
                }
            }
        }
        // Resolve initialization before scene/collision consumers see the map.
        self.flush_io();
        let post = format!("{gamemode_output}PostIO");
        for i in 0..self.ents.len() {
            if self.ents[i].alive && self.ents[i].me.classname == "info_gamemode" {
                self.fire_output(i, &post, None);
            }
        }
        self.flush_io();
    }

    pub fn find(&self, name: &str) -> Vec<usize> {
        let n = name.to_ascii_lowercase();
        if let Some(prefix) = n.strip_suffix('*') {
            return self
                .names
                .iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .flat_map(|(_, v)| v.iter().copied())
                .collect();
        }
        self.names.get(&n).cloned().unwrap_or_default()
    }
    /// SpawnEntityFromTable: a new entity from key/values (outputs use the
    /// compiled 0x1B form), with its map kind and connections.
    pub fn spawn_from_keyvalues(&mut self, classname: &str, kvs: Vec<(String, String)>) -> usize {
        let mut pairs = vec![("classname".to_string(), classname.to_string())];
        pairs.extend(kvs.into_iter().filter(|(k, _)| !k.eq_ignore_ascii_case("classname")));
        let ent = l4d2_formats::bsp::Entity(pairs);
        let model = ent.get("model").unwrap_or("").to_string();
        let me = MapEnt {
            index: self.ents.len(),
            classname: classname.into(),
            targetname: ent.get("targetname").unwrap_or("").to_string(),
            origin: ent.origin(),
            angles: ent.angles(),
            mdl: model.to_ascii_lowercase().ends_with(".mdl").then(|| model.replace('\\', "/")),
            brush: model.strip_prefix('*').and_then(|n| n.parse().ok()),
            ent,
        };
        let conns = me
            .ent
            .0
            .iter()
            .filter_map(|(k, v)| Some((Connection::parse(k, v)?, v.contains('\u{1b}'))))
            .filter(|(c, compiled)| *compiled || k_is_output(&c.output))
            .map(|(c, _)| c)
            .collect();
        let kind = initial_kind(&me, &self.bsp);
        self.add_runtime_entity(Ent { me, conns, kind, alive: true, templated: false, parent: None })
    }
    /// Register runtime-created entities in the same name table used by map I/O.
    pub fn add_runtime_entity(&mut self, mut entity: Ent) -> usize {
        let index = self.ents.len();
        entity.me.index = index;
        if !entity.me.targetname.is_empty() {
            self.names
                .entry(entity.me.targetname.to_ascii_lowercase())
                .or_default()
                .push(index);
        }
        self.parent_bind.push(crate::mapents::entity_matrix(entity.me.origin, entity.me.angles));
        self.ents.push(entity);
        self.fog.changed(index,&self.ents[index]);
        index
    }

    /// `ent_create` / `prop_dynamic_create`: a new entity of `classname`
    /// with keyvalues, given the same behaviour (Kind) a map entity of that
    /// class gets at load.
    pub fn create_entity(&mut self, classname: &str, keyvalues: &[(String, String)], origin: Vec3, angles: Vec3, model: Option<String>) -> usize {
        let mut kv = vec![("classname".to_string(), classname.to_string())];
        kv.extend(keyvalues.iter().cloned());
        let targetname = keyvalues.iter().find(|(k, _)| k.eq_ignore_ascii_case("targetname")).map(|(_, v)| v.clone()).unwrap_or_default();
        let me = MapEnt {
            index: 0,
            classname: classname.to_ascii_lowercase(),
            targetname,
            origin,
            angles,
            mdl: model,
            brush: None,
            ent: l4d2_formats::bsp::Entity(kv),
        };
        let kind = initial_kind(&me, &self.bsp);
        self.add_runtime_entity(Ent { me, conns: vec![], kind, alive: true, templated: false, parent: None })
    }

    /// Source's keyvalue path (`ent_keyvalue`, AddOutput "key value"):
    /// targetname keeps the name index, classname/origin/angles/model change
    /// the entity, `On...` keys add a connection ("target,input,param,delay,
    /// times", ':' also accepted), anything else is stored as a keyvalue.
    pub fn set_keyvalue(&mut self, ent: usize, key: &str, value: &str) {
        let Some(e) = self.ents.get_mut(ent) else { return };
        let lower = key.to_ascii_lowercase();
        let vec3 = |v: &str| {
            let n: Vec<f32> = v.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (n.len() == 3).then(|| Vec3::new(n[0], n[1], n[2]))
        };
        match lower.as_str() {
            "targetname" => {
                let old = e.me.targetname.to_ascii_lowercase();
                if let Some(ids) = self.names.get_mut(&old) {
                    ids.retain(|&id| id != ent);
                }
                self.ents[ent].me.targetname = value.to_string();
                self.names.entry(value.to_ascii_lowercase()).or_default().push(ent);
            }
            // Only the targeting name changes (stored as the keyvalue
            // below); behaviour keeps the real class, as in Source.
            "classname" => {}
            "origin" => {
                if let Some(v) = vec3(value) {
                    e.me.origin = v;
                    if let Some(m) = self.parent_bind.get_mut(ent) {
                        *m = crate::mapents::entity_matrix(v, self.ents[ent].me.angles);
                    }
                }
            }
            "angles" => {
                if let Some(v) = vec3(value) {
                    e.me.angles = v;
                    if let Some(m) = self.parent_bind.get_mut(ent) {
                        *m = crate::mapents::entity_matrix(self.ents[ent].me.origin, v);
                    }
                }
            }
            "model" => e.me.mdl = Some(value.to_string()),
            k if k.starts_with("on") && value.contains([',', ':']) => {
                let parts: Vec<&str> = value.split([',', ':']).collect();
                e.conns.push(Connection {
                    output: key.into(),
                    target: parts[0].trim().into(),
                    input: parts.get(1).map_or("", |s| s.trim()).into(),
                    param: parts.get(2).copied().unwrap_or("").into(),
                    delay: parts.get(3).and_then(|v| v.trim().parse().ok()).unwrap_or(0.0),
                    times: parts.get(4).and_then(|v| v.trim().parse().ok()).unwrap_or(-1),
                });
                return;
            }
            _ => {}
        }
        let e = &mut self.ents[ent];
        e.me.ent.0.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
        e.me.ent.0.push((key.to_string(), value.to_string()));
        if matches!(e.me.classname.as_str(), "fog_volume" | "env_fog_controller") {
            crate::fog::set_key(&mut self.ents[ent], key, value.to_string());
            self.fog.changed(ent, &self.ents[ent]);
        }
    }

    /// Entities whose HammerId keyvalue is `id` (`ent_keyvalue 9768024 ...`).
    pub fn find_hammer_id(&self, id: &str) -> Vec<usize> {
        self.ents.iter().enumerate()
            .filter(|(_, e)| e.alive && e.me.kv("hammerid").is_some_and(|h| h.trim() == id.trim()))
            .map(|(i, _)| i).collect()
    }

    /// Queue every connection of `ent` named `output`.
    /// logic_branch_listener: a listened branch changed value, so each
    /// listener naming it (Branch01..Branch16, `*` wildcards) re-checks all
    /// its branches. Without this c5m2's CEDA trailer exit door never
    /// unlocked (OnAllTrue -> finale_cleanse_exit_door Unlock).
    fn update_branch_listeners(&mut self, branch: usize, activator: Option<usize>) {
        let name = self.ents[branch].me.targetname.clone();
        if name.is_empty() {
            return;
        }
        let listeners: Vec<usize> = self
            .ents
            .iter()
            .enumerate()
            .filter(|(_, e)| e.alive && e.me.classname == "logic_branch_listener")
            .filter(|(_, e)| {
                (1..=16).any(|i| {
                    e.me.kv(&format!("Branch{i:02}")).is_some_and(|pattern| name_matches(pattern, &name))
                })
            })
            .map(|(i, _)| i)
            .collect();
        for listener in listeners {
            self.evaluate_branch_listener(listener, false, activator);
        }
    }
    /// Fires OnAllTrue / OnAllFalse / OnMixed when the combined state of the
    /// listener's branches changes (always, for its Test input).
    fn evaluate_branch_listener(&mut self, listener: usize, force: bool, activator: Option<usize>) {
        let patterns: Vec<String> = (1..=16)
            .filter_map(|i| self.ents[listener].me.kv(&format!("Branch{i:02}")).map(str::to_owned))
            .filter(|p| !p.is_empty())
            .collect();
        let values: Vec<bool> = self
            .ents
            .iter()
            .filter(|e| e.alive && patterns.iter().any(|p| name_matches(p, &e.me.targetname)))
            .filter_map(|e| match e.kind {
                Kind::Branch { value } => Some(value),
                _ => None,
            })
            .collect();
        if values.is_empty() {
            return;
        }
        let state = if values.iter().all(|v| *v) {
            "OnAllTrue"
        } else if values.iter().all(|v| !*v) {
            "OnAllFalse"
        } else {
            "OnMixed"
        };
        let me = &mut self.ents[listener].me;
        if !force && me.kv("__branch_state") == Some(state) {
            return;
        }
        me.ent.0.retain(|(k, _)| k != "__branch_state");
        me.ent.0.push(("__branch_state".into(), state.into()));
        self.fire_output(listener, state, activator);
    }
    pub fn fire_output(&mut self, ent: usize, output: &str, activator: Option<usize>) {
        self.fire_output_value(ent, output, None, activator);
    }

    /// Fire an output that carries a value (used as the parameter of connections that give none).
    pub fn fire_output_value(
        &mut self,
        ent: usize,
        output: &str,
        value: Option<&str>,
        activator: Option<usize>,
    ) {
        let mut to_fire = Vec::new();
        if matches!(self.ents[ent].kind, Kind::Door(_) | Kind::Elevator { .. }) {
            self.events.push(EntEvent::DoorSound {
                ent,
                output: output.into(),
            });
        }
        let e = &mut self.ents[ent];
        e.conns.retain_mut(|c| {
            if !c.output.eq_ignore_ascii_case(output) {
                return true;
            }
            let param = if c.param.is_empty() {
                value.unwrap_or("").to_string()
            } else {
                c.param.clone()
            };
            to_fire.push((c.target.clone(), c.input.clone(), param, c.delay));
            if c.times > 0 {
                c.times -= 1;
                c.times > 0
            } else {
                true
            }
        });
        if !to_fire.is_empty() {
            self.events.push(EntEvent::Fired {
                src: ent,
                output: output.to_string(),
            });
        }
        for (target, input, param, delay) in to_fire {
            // CBaseEntityOutput::FireOutput's developer 2 trace.
            if DEVELOPER.load(std::sync::atomic::Ordering::Relaxed) >= 2 && !target.starts_with("!script_entity:") {
                let caller = &self.ents[ent].me;
                dev_msg(2, format!("({:.2}) output: ({},{}) -> ({},{},{:.1})({})", self.time, caller.classname, caller.targetname, target, input, delay, param));
            }
            self.seq += 1;
            self.queue.push(Pending {
                time: self.time + delay,
                seq: self.seq,
                target,
                input,
                param,
                actor: self.io_actor,
                activator,
                caller: Some(ent),
            });
        }
    }

    /// Fire `output` of `ent` after `delay` seconds.
    pub fn schedule_output(&mut self, ent: usize, output: &str, delay: f32) {
        self.seq += 1;
        self.queue.push(Pending {
            time: self.time + delay,
            seq: self.seq,
            target: "!caller".into(),
            input: format!("__fire:{output}"),
            param: String::new(),
            actor: self.io_actor,
            activator: None,
            caller: Some(ent),
        });
    }

    pub fn fire_output_pub(&mut self, ent: usize, output: &str) {
        self.fire_output(ent, output, None);
    }
    pub fn connect_script_output(
        &mut self,
        entity: usize,
        output: &str,
        function: &str,
        connect: bool,
    ) {
        let Some(ent) = self.ents.get_mut(entity).filter(|e| e.alive) else {
            return;
        };
        let target = format!("!script_entity:{entity}");
        ent.conns.retain(|c| {
            !(c.output.eq_ignore_ascii_case(output)
                && c.target == target
                && c.input == "CallScriptFunction"
                && c.param == function)
        });
        if connect {
            ent.conns.push(Connection {
                output: output.into(),
                target,
                input: "CallScriptFunction".into(),
                param: function.into(),
                delay: 0.,
                times: -1,
            });
        }
    }

    /// Queue an input directly (delay seconds from now).
    pub fn queue_input(
        &mut self,
        target: &str,
        input: &str,
        param: &str,
        delay: f32,
        activator: Option<usize>,
    ) {
        self.queue_input_context(target, input, param, delay, activator, None);
    }
    pub fn queue_input_context(
        &mut self,
        target: &str,
        input: &str,
        param: &str,
        delay: f32,
        activator: Option<usize>,
        caller: Option<usize>,
    ) {
        self.seq += 1;
        self.queue.push(Pending {
            time: self.time + delay,
            seq: self.seq,
            target: target.into(),
            input: input.into(),
            param: param.into(),
            actor: self.io_actor,
            activator,
            caller,
        });
    }

    fn resolve(&self, target: &str, activator: Option<usize>, caller: Option<usize>) -> Vec<usize> {
        match target.to_ascii_lowercase().as_str() {
            t if t.starts_with("!script_entity:") => t[15..]
                .parse::<usize>()
                .ok()
                .filter(|id| self.ents.get(*id).is_some_and(|e| e.alive))
                .into_iter()
                .collect(),
            "!activator" => activator.into_iter().collect(),
            "!caller" | "!self" => caller.into_iter().collect(),
            "!player" => Vec::new(),
            t => {
                let mut v = self.find(t);
                if v.is_empty() {
                    // Also match by classname (Source allows targeting classnames).
                    v = self
                        .ents
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| e.alive && name_matches(t, Self::target_class(e)))
                        .map(|(i, _)| i)
                        .collect();
                }
                v
            }
        }
    }

    /// The class name I/O targets match: a `ClassName` keyvalue renames it
    /// (retail testscripts rename relays to "Butt") while the entity keeps
    /// behaving as its real class.
    pub fn target_class(e: &Ent) -> &str {
        e.me.kv("classname").unwrap_or(&e.me.classname)
    }

    /// Advance time, deliver due inputs, run entity think logic.
    pub fn tick(&mut self, dt: f32) {
        self.frame_dt = dt.max(0.);
        self.time += dt;
        self.flush_io();
        self.fog.tick(&mut self.ents,self.time);
        if self
            .cine
            .view_movement
            .is_some_and(|(_, start, duration)| self.time >= start + duration)
        {
            self.cine.view_control = None;
            self.cine.view_movement = None;
        }
        for i in 0..self.ents.len() {
            if self.ents[i].alive {
                self.think(i);
            }
        }
    }
    pub fn flush_io(&mut self) {
        let mut budget = 32768;
        while self.queue.peek().is_some_and(|p| p.time <= self.time) {
            if budget == 0 {
                break;
            }
            budget -= 1;
            let p = self.queue.pop().unwrap();
            // None is the local player in the gameplay adapter. Preserve the
            // touching actor through delayed relays so a bot cannot change its
            // camera fog by firing an !activator connection.
            let local_fog = p.input.eq_ignore_ascii_case("SetFogController")
                && (p.target.eq_ignore_ascii_case("!player")
                    || (p.target.eq_ignore_ascii_case("!activator") && p.activator.is_none()
                        && p.actor.is_none_or(|a| a == 0 || a == u64::MAX)));
            if local_fog {
                if let Some(id) = self.find(p.param.trim()).into_iter().find(|&id| {
                    self.ents[id].alive && self.ents[id].me.classname == "env_fog_controller"
                }) {
                    self.player_fog_controller = Some(id);
                    self.player_fog_revision = self.player_fog_revision.wrapping_add(1);
                }
                continue;
            }
            // SpeakResponseConcept on a survivor (!activator, !francis, ...):
            // survivors are not map entities, so hand the concept to the
            // dialogue system. "!actorN" is the touching actor (0 = player).
            if p.input.eq_ignore_ascii_case("SpeakResponseConcept")
                && p.target.starts_with('!')
                && self.resolve(&p.target, p.activator, p.caller).is_empty()
            {
                let who = if p.target.eq_ignore_ascii_case("!activator") {
                    format!("!actor{}", p.actor.filter(|a| *a != u64::MAX).unwrap_or(0))
                } else {
                    p.target.to_ascii_lowercase()
                };
                self.cine.concepts.push((Some(who), p.param.trim().to_string()));
                continue;
            }
            let previous_actor = self.io_actor;
            self.io_actor = p.actor;
            let targets = self.resolve(&p.target, p.activator, p.caller);
            // CEventQueue::ServiceEvents' developer 2 traces.
            if DEVELOPER.load(std::sync::atomic::Ordering::Relaxed) >= 2 && !p.input.starts_with("__") && !p.target.starts_with("!script_entity:") {
                let caller = p.caller.and_then(|c| self.ents.get(c)).map(|c| &c.me);
                if targets.is_empty() {
                    dev_msg(2, format!(
                        "unhandled input: ({}) -> ({}), from ({},{}); target entity not found",
                        p.input, p.target, caller.map_or("", |c| c.classname.as_str()), caller.map_or("", |c| c.targetname.as_str())
                    ));
                }
                for &t in &targets {
                    let name = if self.ents[t].me.targetname.is_empty() { &self.ents[t].me.classname } else { &self.ents[t].me.targetname };
                    dev_msg(2, format!("({:.2}) input {}: {}.{}({})", self.time, caller.map_or("", |c| c.targetname.as_str()), name, p.input, p.param));
                }
            }
            for t in targets {
                if self.ents[t].alive {
                    self.input(t, &p.input, &p.param, p.activator, p.caller);
                }
            }
            self.io_actor = previous_actor;
        }
    }

    fn think(&mut self, i: usize) {
        let now = self.time;
        match &mut self.ents[i].kind {
            Kind::Timer {
                enabled: true,
                next,
                refire,
                lo,
                hi,
                random,
            } if now >= *next => {
                let interval = if *random {
                    self.rng.random_float(*lo, *hi)
                } else {
                    *refire
                };
                *next = now + interval.max(0.01);
                self.fire_output(i, "OnTimer", None);
            }
            Kind::Door(_) => self.think_door(i),
            Kind::Elevator { .. } => self.think_elevator(i),
            Kind::Train { .. } => self.think_train(i),
            Kind::CarAlarm { until, .. } if *until > 0.0 && now >= *until => {
                *until = 0.0;
                self.fire_output(i, "OnCarAlarmEnd", None);
            }
            Kind::CarAlarm { chirp_until, .. } if *chirp_until > 0.0 && now >= *chirp_until => {
                *chirp_until = 0.0;
                self.fire_output(i, "OnCarAlarmChirpEnd", None);
            }
            Kind::Fire {
                active,
                extinguish_at,
                ..
            } if *active && *extinguish_at > 0. && now >= *extinguish_at => {
                *active = false;
                *extinguish_at = 0.;
                self.fire_output(i, "OnExtinguished", None);
            }
            _ => {}
        }
    }

    fn think_door(&mut self, i: usize) {
        let (now, dt) = (self.time, self.frame_dt);
        let mut moved = false;
        let mut fire: Option<&'static str> = None;
        let mut also: Option<&'static str> = None;
        let mut stopped = false;
        if let Kind::Door(d) = &mut self.ents[i].kind {
            match d.state {
                DoorState::Opening => {
                    d.frac += d.speed * dt / d.dist.abs().max(0.001);
                    moved = true;
                    if d.frac >= d.goal {
                        d.frac = d.goal;
                        d.state = DoorState::Open;
                        if d.goal >= 1.0 {
                            fire = Some("OnFullyOpen");
                            if d.wait >= 0.0 {
                                d.close_at = now + d.wait;
                            }
                        } else {
                            d.wait = -1.0; // a partial `SetPosition` move holds its position
                            stopped = true;
                        }
                    }
                }
                DoorState::Closing => {
                    d.frac -= d.speed * dt / d.dist.abs().max(0.001);
                    moved = true;
                    if d.frac <= d.goal {
                        d.frac = d.goal;
                        if d.goal <= 0.0 {
                            d.state = DoorState::Closed;
                            fire = Some("OnFullyClosed");
                            also = Some("solid");
                        } else {
                            d.state = DoorState::Open;
                            d.wait = -1.0;
                            stopped = true;
                        }
                    }
                }
                DoorState::Open => {
                    if d.wait >= 0.0 && now >= d.close_at {
                        d.state = DoorState::Closing;
                        fire = Some("OnClose");
                    }
                }
                DoorState::Closed => {}
            }
        }
        if moved {
            self.push_moved(i);
        }
        // A partial SetPosition move ends with the stop sound (CBaseToggle
        // MoveDone plays m_soundStop) without a fully-open/closed output.
        if stopped {
            self.events.push(EntEvent::DoorSound { ent: i, output: "OnFullyOpen".into() });
        }
        if let Some(o) = fire {
            self.fire_output(i, o, None);
        }
        let _ = also;
    }

    fn base_matrix(&self, i: usize) -> Mat4 {
        crate::mapents::entity_matrix(self.ents[i].me.origin, self.ents[i].me.angles)
    }

    /// Transform from the entity's own motion only (door opening, elevator travel), before parenting.
    fn own_matrix(&self, i: usize) -> Mat4 {
        match &self.ents[i].kind {
            Kind::Door(_) => self.door_matrix_own(i),
            Kind::Elevator { offset, .. } | Kind::Train { offset, .. } => {
                Mat4::from_translation(*offset) * self.base_matrix(i)
            }
            _ => self.base_matrix(i),
        }
    }

    /// Current world transform including every ancestor's motion.
    pub fn world_matrix(&self, i: usize) -> Mat4 {
        let own = self.own_matrix(i);
        match self.ents[i].parent {
            Some(p) if p != i => self.parent_delta(p) * own,
            _ => own,
        }
    }

    fn parent_delta(&self, p: usize) -> Mat4 {
        self.world_matrix(p) * self.parent_bind[p].inverse()
    }

    /// Emit `Moved` for `i` and all of its descendants.
    fn push_moved(&mut self, i: usize) {
        let m = self.world_matrix(i);
        self.events.push(EntEvent::Moved { ent: i, matrix: m });
        let kids: Vec<usize> = (0..self.ents.len())
            .filter(|&c| self.ents[c].parent == Some(i) && c != i)
            .collect();
        for c in kids {
            self.push_moved(c);
        }
    }

    /// The path_track after `node` in the travel direction.
    fn train_next(&self, node: usize, forward: bool) -> Option<usize> {
        let name = if forward {
            self.ents[node].me.kv("target").unwrap_or("").to_string()
        } else {
            self.ents[node].me.targetname.clone()
        };
        if name.is_empty() {
            return None;
        }
        self.ents.iter().position(|e| {
            e.alive
                && e.me.classname == "path_track"
                && if forward {
                    e.me.targetname.eq_ignore_ascii_case(&name)
                } else {
                    e.me.kv("target").is_some_and(|t| t.eq_ignore_ascii_case(&name))
                }
        })
    }

    fn think_train(&mut self, i: usize) {
        let dt = self.frame_dt;
        let origin = self.ents[i].me.origin;
        // The first path_track (`target`) is where the train starts.
        if let Kind::Train { at: None, .. } = self.ents[i].kind {
            let first = self.ents[i].me.kv("target").unwrap_or("").to_string();
            let start = self.ents.iter().position(|e| {
                e.me.classname == "path_track" && e.me.targetname.eq_ignore_ascii_case(&first)
            });
            if let Some(start) = start {
                let to = self.ents[start].me.origin - origin;
                if let Kind::Train { at, offset, .. } = &mut self.ents[i].kind {
                    *at = Some(start);
                    *offset = to;
                }
                self.push_moved(i);
            }
        }
        let Kind::Train { offset, speed, forward, moving: true, at: Some(at), .. } = self.ents[i].kind
        else {
            return;
        };
        let mut step = speed.abs() * dt;
        if step <= 0.0 {
            return;
        }
        let mut offset = offset;
        let mut at = at;
        let mut passed = Vec::new();
        let mut stopped = false;
        while step > 0.0 {
            let Some(next) = self.train_next(at, forward) else {
                stopped = true;
                break;
            };
            let goal = self.ents[next].me.origin - origin;
            let d = goal - offset;
            if d.length() <= step {
                step -= d.length();
                offset = goal;
                at = next;
                passed.push(next);
                if passed.len() > 64 {
                    break;
                }
            } else {
                offset += d.normalize() * step;
                step = 0.0;
            }
        }
        if let Kind::Train { offset: o, at: a, moving, old_speed, speed, .. } = &mut self.ents[i].kind {
            *o = offset;
            *a = Some(at);
            if stopped {
                // Dead end: stop, keeping the speed for the next Start*.
                *moving = false;
                *old_speed = *speed;
            }
        }
        self.push_moved(i);
        for node in passed {
            self.fire_output(node, "OnPass", Some(i));
        }
    }

    fn think_elevator(&mut self, i: usize) {
        let dt = self.frame_dt;
        let mut reached: Option<bool> = None;
        if let Kind::Elevator {
            offset,
            target,
            speed,
            moving: true,
            top,
            bottom,
        } = &mut self.ents[i].kind
        {
            let d = *target - *offset;
            let step = *speed * dt;
            if d.length() <= step {
                *offset = *target;
                // Top stop is offset 0; anything below counts as the bottom.
                reached =
                    Some((*target - Vec3::ZERO).length() < (*target - (*bottom - *top)).length());
            } else {
                *offset += d.normalize() * step;
            }
        }
        if let Kind::Elevator { moving, .. } = &mut self.ents[i].kind {
            if reached.is_some() {
                *moving = false;
            }
        }
        if matches!(&self.ents[i].kind, Kind::Elevator { .. }) {
            self.push_moved(i);
        }
        match reached {
            Some(true) => self.fire_output(i, "OnReachedTop", None),
            Some(false) => self.fire_output(i, "OnReachedBottom", None),
            None => {}
        }
    }

    /// World transform of a door at its current opening fraction (including parents).
    pub fn door_matrix(&self, i: usize) -> Mat4 {
        self.world_matrix(i)
    }

    fn door_matrix_own(&self, i: usize) -> Mat4 {
        let e = &self.ents[i];
        let Kind::Door(d) = &e.kind else {
            return crate::mapents::entity_matrix(e.me.origin, e.me.angles);
        };
        let base = crate::mapents::entity_matrix(e.me.origin, e.me.angles);
        if d.rotating {
            if e.me.classname.starts_with("prop_door_rotating") {
                // Source prop doors change yaw even when the model is pitched.
                let mut angles = e.me.angles;
                angles.y += d.dist * d.frac;
                crate::mapents::entity_matrix(e.me.origin, angles)
            } else {
                base * Mat4::from_axis_angle(d.dir, (d.dist * d.frac).to_radians())
            }
        } else {
            Mat4::from_translation(d.dir * d.dist * d.frac) * base
        }
    }

    /// Set off a car alarm (owned CCarAlarm trigger, server 1019e4e0): once
    /// per car, `OnCarAlarmStart`, a Director panic event, and
    /// `OnCarAlarmEnd` after car_alarm_duration (15 s).
    pub fn trigger_car_alarm(&mut self, ent: usize) -> bool {
        let now = self.time;
        let Kind::CarAlarm { armed, until, triggered, chirp_until, .. } = &mut self.ents[ent].kind else {
            return false;
        };
        if !*armed || *triggered {
            return false;
        }
        *triggered = true;
        *until = now + CAR_ALARM_DURATION;
        // A chirp in progress is cut off by the alarm itself.
        let chirping = *chirp_until > 0.0;
        *chirp_until = 0.0;
        if chirping {
            self.fire_output(ent, "OnCarAlarmChirpEnd", None);
        }
        self.fire_output(ent, "OnCarAlarmStart", None);
        self.cine.director_commands.push(("PanicEvent".into(), String::new()));
        true
    }

    /// A survivor is within car_alarm_chirp_distance (350) of an armed car:
    /// chirp (owned 1019e770) for car_alarm_chirp_duration (1 s), at most
    /// every car_alarm_chirp_interval (3 s).
    pub fn chirp_car_alarm(&mut self, ent: usize) -> bool {
        let now = self.time;
        let Kind::CarAlarm { armed, triggered, chirp_until, next_chirp, .. } = &mut self.ents[ent].kind else {
            return false;
        };
        if !*armed || *triggered || now < *next_chirp {
            return false;
        }
        *chirp_until = now + CAR_ALARM_CHIRP_DURATION;
        *next_chirp = now + CAR_ALARM_CHIRP_INTERVAL;
        self.fire_output(ent, "OnCarAlarmChirpStart", None);
        true
    }

    /// Source damage filters (filter_damage_type, filter_melee_damage,
    /// filter_multi, filter_activator_name). The attacker is a survivor
    /// with no targetname. A name with no filter entity filters nothing.
    fn damage_filter_passes(&self, name: &str, kind: DamageKind, depth: usize) -> bool {
        let Some(&id) = self.find(name).first() else {
            return true;
        };
        if depth > 8 {
            return true;
        }
        let me = &self.ents[id].me;
        let negated = me.kv("Negated").is_some_and(|v| v.trim() == "1");
        // DMG_BULLET 2, DMG_BURN 8, DMG_BLAST 64, DMG_CLUB 128.
        let bits = match kind {
            DamageKind::Bullet => 2,
            DamageKind::Burn => 8,
            DamageKind::Blast => 64,
            DamageKind::Club => 128,
        };
        let pass = match me.classname.as_str() {
            "filter_damage_type" => bits & me.kv_i32("damagetype").unwrap_or(0) != 0,
            "filter_melee_damage" => matches!(kind, DamageKind::Club),
            "filter_multi" => {
                let children: Vec<String> = (1..=10)
                    .filter_map(|i| me.kv(&format!("Filter{i:02}")).map(str::to_owned))
                    .filter(|n| !n.is_empty())
                    .collect();
                let mut results = children.iter().map(|n| self.damage_filter_passes(n, kind, depth + 1));
                if me.kv_i32("filtertype") == Some(1) {
                    results.any(|r| r)
                } else {
                    results.all(|r| r)
                }
            }
            "filter_activator_name" => me.kv("filtername").is_some_and(|n| n.is_empty()),
            // Team/class filters: the attacker is a survivor.
            "filter_activator_team" => me.kv_i32("filterteam").is_none_or(|t| t == 2),
            _ => true,
        };
        pass != negated
    }

    /// Damage a breakable; returns true if it broke.
    pub fn damage_breakable(&mut self, ent: usize, dmg: f32) -> bool {
        self.damage_entity(ent, dmg, DamageKind::Bullet)
    }

    pub fn damage_entity(&mut self, ent: usize, dmg: f32, kind: DamageKind) -> bool {
        if std::env::var_os("L4D2R_BREAK_LOG").is_some() {
            let me = &self.ents[ent].me;
            println!("L4D2RDAMAGE t={:.2} ent={ent} {} {:?} dmg={dmg} kind={kind:?}", self.time, me.classname, me.mdl);
        }
        if !self.ents.get(ent).is_some_and(|e| e.alive) || !dmg.is_finite() || dmg <= 0. {
            return false;
        }
        // The entity's damage filter (`damagefilter`, SetDamageFilter).
        if let Some(filter) = self.ents[ent].me.kv("damagefilter").map(str::to_owned) {
            if !self.damage_filter_passes(&filter, kind, 0) {
                return false;
            }
        }
        if self.ents[ent].me.classname.starts_with("func_breakable")
            && self.ents[ent].me.kv_i32("material") == Some(7) {
            return false; // Native matUnbreakableGlass; explicit Break input remains available.
        }
        if self.ents[ent].me.classname == "func_breakable_surf" {
            self.events.push(EntEvent::SurfaceDamage { ent, damage:dmg, kind });
            return false; // The pane adapter keeps the surface alive after initial fracture.
        }
        let health = if let Some(d) = self.durability.get_mut(&ent) {
            // Owned server 101a37b0 / constant105d3580: a >=40 blast
            // promotes ordinary damageable rotating-door damage to max health.
            let dmg = if matches!(kind, DamageKind::Blast)
                && dmg >= 40. && matches!(self.ents[ent].kind, Kind::Door(_)) {
                d.maximum
            } else { dmg };
            d.health -= dmg
                * match kind {
                    DamageKind::Bullet => d.bullets,
                    DamageKind::Club => d.club,
                    DamageKind::Burn => d.fire,
                    DamageKind::Blast => d.explosive,
                };
            if let Kind::Breakable { health } = &mut self.ents[ent].kind {
                *health = d.health;
            }
            d.health
        } else if let Kind::Breakable { health } = &mut self.ents[ent].kind {
            *health -= dmg;
            *health
        } else {
            return false;
        };
        let broke = health <= 0.0;
        if broke {
            self.break_ent(ent);
        } else {
            self.fire_output_value(ent, "OnHealthChanged", Some(&health.to_string()), None);
        }
        broke
    }

    pub fn break_ent(&mut self, ent: usize) {
        if !self.ents[ent].alive {
            return;
        }
        // L4D2R_BREAK_LOG: debug, every entity break.
        if std::env::var_os("L4D2R_BREAK_LOG").is_some() {
            let me = &self.ents[ent].me;
            println!("L4D2RBREAK t={:.2} ent={ent} {} {:?} at {:?}", self.time, me.classname, me.mdl, me.origin);
        }
        self.fire_output(ent, "OnBreak", None);
        self.events.push(EntEvent::Broken { ent });
        self.ents[ent].alive = false;
        self.events.push(EntEvent::Solid { ent, solid: false });
        self.events.push(EntEvent::Visible {
            ent,
            visible: false,
        });
    }

    /// Deliver one input to an entity.
    pub fn door_open_away_from(&mut self, ent: usize, opener: Option<Vec3>) {
        self.opener = opener.map(|p| (ent, p));
        let e = &self.ents[ent];
        if !e.me.classname.starts_with("prop_door_rotating") {
            return;
        }
        let direction = e.me.kv_i32("opendir").unwrap_or(0);
        let base = crate::mapents::entity_matrix(e.me.origin, e.me.angles);
        let base = match e.parent {
            Some(parent) if parent != ent => self.parent_delta(parent) * base,
            _ => base,
        };
        let behind = opener.is_some_and(|p| {
            base.transform_vector3(Vec3::X)
                .dot(p - base.transform_point3(Vec3::ZERO))
                > 0.0
        });
        if let Kind::Door(d) = &mut self.ents[ent].kind {
            // Preserve the current swing when reversing a partially open door.
            if d.frac <= 0.001 {
                let back = direction == 2 || (direction == 0 && behind);
                d.dist = d.dist.abs() * d.forward_sign * if back { -1.0 } else { 1.0 };
            }
        }
    }

    pub fn input(
        &mut self,
        ent: usize,
        input: &str,
        param: &str,
        activator: Option<usize>,
        caller: Option<usize>,
    ) {
        let mut inp = input.to_ascii_lowercase();
        if inp == "spawnitem" && self.ents[ent].me.classname == "weapon_scavenge_item_spawn" && self.ents[ent].alive {
            if !self.cine.item_spawns.contains(&ent) {
                self.cine.item_spawns.push(ent);
            }
            return;
        }
        if inp == "spawnzombie" && self.ents[ent].me.classname == "info_zombie_spawn" && self.ents[ent].alive {
            self.cine.zombie_spawns.push(ent);
            return;
        }
        if inp == "setdamagefilter" {
            // CBaseEntity::InputSetDamageFilter: an empty name clears it.
            let me = &mut self.ents[ent].me;
            me.ent.0.retain(|(k, _)| !k.eq_ignore_ascii_case("damagefilter"));
            if !param.trim().is_empty() {
                me.ent.0.push(("damagefilter".into(), param.trim().into()));
            }
            return;
        }
        if inp == "alpha" {
            // CBaseEntity::InputAlpha: the render alpha (renderamt). The
            // storm maps ramp func_precipitation "rain" density through it.
            if let Ok(value) = param.trim().parse::<f32>() {
                let me = &mut self.ents[ent].me;
                me.ent.0.retain(|(k, _)| !k.eq_ignore_ascii_case("renderamt"));
                me.ent.0.push(("renderamt".into(), value.clamp(0., 255.).to_string()));
            }
            return;
        }
        if self.fog.input(ent,&mut self.ents[ent],&inp,param,self.time) {return;}
        if self.ents[ent].me.classname == "fog_volume" {
            let disabled = self.ents[ent].me.kv_i32("StartDisabled") == Some(1);
            let next = match inp.as_str() {
                "enable" => Some(false), "disable" => Some(true), "toggle" => Some(!disabled), _ => None,
            };
            if let Some(disabled) = next {
                self.ents[ent].me.ent.0.retain(|(k,_)| !k.eq_ignore_ascii_case("StartDisabled"));
                self.ents[ent].me.ent.0.push(("startdisabled".into(), if disabled {"1"} else {"0"}.into()));
                self.fog.changed(ent,&self.ents[ent]);
                return;
            }
        }
        if self.ents[ent].me.classname == "env_fog_controller" {
            let setting = match inp.as_str() {
                "turnon" => Some(("fogenable", "1".to_string())),
                "turnoff" => Some(("fogenable", "0".to_string())),
                "setcolor" | "setcolorsecondary" => {
                    let values: Vec<_> = param.split_whitespace().filter_map(|s| s.parse::<f32>().ok().filter(|v| v.is_finite())).collect();
                    (values.len() == 3).then(|| (if inp == "setcolor" {"fogcolor"} else {"fogcolor2"}, values.iter().map(|v| v.clamp(0.,255.).to_string()).collect::<Vec<_>>().join(" ")))
                }
                "setstartdist" | "setenddist" | "setmaxdensity" | "setfarz" => {
                    let key = match inp.as_str() {"setstartdist"=>"fogstart", "setenddist"=>"fogend", "setmaxdensity"=>"fogmaxdensity", _=>"farz"};
                    param.trim().parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| (key,v.to_string()))
                }
                _ => None,
            };
            if let Some((key,value)) = setting {
                self.ents[ent].me.ent.0.retain(|(k,_)| !k.eq_ignore_ascii_case(key));
                self.ents[ent].me.ent.0.push((key.into(),value));
                self.fog.changed(ent,&self.ents[ent]);
                return;
            }
        }
        if matches!(
            inp.as_str(),
            "startglowing" | "stopglowing" | "turnglowson" | "turnglowsoff" | "setglowcolor"
        ) {
            if let Some(e) = self.ents.get_mut(ent) {
                let (key, value) = if inp == "setglowcolor" {
                    ("glowcolor", param.to_string())
                } else {
                    (
                        "glowstate",
                        if matches!(inp.as_str(),"startglowing" | "turnglowson") { "3" } else { "0" }.to_string(),
                    )
                };
                e.me.ent.0.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
                e.me.ent.0.push((key.into(), value));
            }
            return;
        }
        if matches!(
            inp.as_str(),
            "runscriptfile" | "runscriptcode" | "callscriptfunction"
        ) {
            self.cine.script_commands.push(ScriptRequest {
                entity: ent,
                input: input.into(),
                param: param.into(),
                activator,
                caller,
            });
            return;
        }
        if matches!(
            self.ents[ent].me.classname.as_str(),
            "env_hudhint" | "env_instructor_hint"
        ) {
            match inp.as_str() {
                "showhudhint" | "showhint" | "show" => {
                    self.events.push(EntEvent::HudHint { ent, show: true })
                }
                "hidehudhint" | "endhint" | "hide" => {
                    self.events.push(EntEvent::HudHint { ent, show: false })
                }
                _ => self.unhandled.push((ent, input.into(), param.into())),
            }
            return;
        }
        if inp == "openawayfrom" {
            let opener = self
                .ents
                .iter()
                .position(|e| e.me.targetname.eq_ignore_ascii_case(param))
                .or(activator)
                .map(|i| self.ents[i].me.origin);
            self.door_open_away_from(ent, opener);
            inp = "open".into();
        } else if matches!(inp.as_str(), "open" | "use" | "toggle") && activator.is_some() {
            self.door_open_away_from(ent, activator.map(|i| self.ents[i].me.origin));
        }
        if std::env::var("L4D2_TRACE_IO").is_ok() {
            eprintln!(
                "[io {:.2}] {} <- {input} {param}",
                self.time, self.ents[ent].me.targetname
            );
        }
        let now = self.time;
        if let Some(out) = input.strip_prefix("__fire:") {
            self.fire_output(ent, out, activator);
            return;
        }
        if let Kind::ScavengeProgress { enabled, total, collected } = &mut self.ents[ent].kind {
            match inp.as_str() {
                "turnon" => *enabled = true,
                "turnoff" => *enabled = false,
                "settotalitems" => if let Ok(value) = param.parse::<u32>() { *total = value; },
                "setcurrentitems" => if let Ok(value) = param.parse::<u32>() { *collected = value.min(*total); },
                _ => self.unhandled.push((ent,input.into(),param.into())),
            }
            return;
        }
        // Universal inputs first.
        match inp.as_str() {
            "kill" | "killhierarchy" => {
                let mut killed = vec![ent];
                if inp == "killhierarchy" {
                    let mut at = 0;
                    while at < killed.len() {
                        let parent = killed[at];
                        for (id, child) in self.ents.iter().enumerate() {
                            if child.parent == Some(parent) && !killed.contains(&id) {
                                killed.push(id);
                            }
                        }
                        at += 1;
                    }
                }
                for id in killed {
                    self.ents[id].alive = false;
                    self.fog.changed(id,&self.ents[id]);
                    self.events.push(EntEvent::Solid {
                        ent: id,
                        solid: false,
                    });
                    self.events.push(EntEvent::Visible {
                        ent: id,
                        visible: false,
                    });
                }
                return;
            }
            "fireuser1" | "fireuser2" | "fireuser3" | "fireuser4" => {
                let n = &inp[8..];
                self.fire_output(ent, &format!("OnUser{n}"), activator);
                return;
            }
            "addoutput" => {
                // "OnTrigger target:input:param:delay:times" or "key value"
                if let Some((out, rest)) = param.split_once(' ') {
                    let parts: Vec<&str> = rest.split([':', ',']).collect();
                    if parts.len() >= 2 && out.to_ascii_lowercase().starts_with("on") {
                        let conn = Connection {
                            output: out.into(),
                            target: parts[0].into(),
                            input: parts[1].into(),
                            param: parts.get(2).copied().unwrap_or("").into(),
                            delay: parts.get(3).and_then(|v| v.parse().ok()).unwrap_or(0.0),
                            times: parts.get(4).and_then(|v| v.parse().ok()).unwrap_or(-1),
                        };
                        self.ents[ent].conns.push(conn);
                    } else {
                        // AddOutput "key value" on any entity (testscripts:
                        // "HammerId 9768024", "classname df_existing").
                        self.set_keyvalue(ent, out, rest);
                    }
                }
                return;
            }
            _ => {}
        }
        let class = self.ents[ent].me.classname.clone();
        if inp == "break" && self.durability.contains_key(&ent) {
            self.break_ent(ent);
            return;
        }
        match (class.as_str(), inp.as_str()) {
            ("prop_dynamic" | "prop_dynamic_override", "setanimation") => {
                self.cine.anims.insert(ent, (param.to_string(), now));
                return;
            }
            ("env_fade", "fade") => {
                let me = &self.ents[ent].me;
                let c = me
                    .kv("rendercolor")
                    .unwrap_or("0 0 0")
                    .split_whitespace()
                    .map(|v| v.parse::<f32>().unwrap_or(0.0) / 255.0)
                    .collect::<Vec<_>>();
                self.cine.fade = Some(Fade {
                    start: now,
                    hold: me.kv_f32("holdtime").unwrap_or(0.0),
                    duration: me.kv_f32("duration").unwrap_or(2.0),
                    color: [
                        c.first().copied().unwrap_or(0.0),
                        c.get(1).copied().unwrap_or(0.0),
                        c.get(2).copied().unwrap_or(0.0),
                    ],
                    alpha: me.kv_f32("renderamt").unwrap_or(255.0),
                    from: me.spawnflags() & 1 != 0,
                    stay: me.spawnflags() & 4 != 0,
                });
                return;
            }
            ("info_survivor_position", "setviewcontrol") => {
                let selected = self.cine.selected_survivor.as_deref().unwrap_or("ellis");
                if self.ents[ent]
                    .me
                    .kv("survivorname")
                    .is_some_and(|name| name.eq_ignore_ascii_case(selected))
                {
                    if let Some(&c) = self.find(param.trim()).first() {
                        self.cine.view_control = Some(c);
                    }
                }
                return;
            }
            (
                "point_viewcontrol_survivor",
                "setparentattachment" | "setparentattachmentmaintainoffset",
            ) => {
                self.cine
                    .parent_attach
                    .insert(ent, param.trim().to_string());
                return;
            }
            ("point_viewcontrol_survivor", "startmovement") => {
                if self.cine.view_control == Some(ent) {
                    let duration = param
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite() && *v > 0.)
                        .unwrap_or(1.);
                    self.cine.view_movement = Some((ent, self.time, duration));
                }
                return;
            }
            ("info_director", "startintro") => {
                self.cine.intro = true;
                return;
            }
            ("info_director", "finishintro") => {
                self.cine.intro = false;
                return;
            }
            ("info_director", "forcesurvivorpositions") => {
                self.cine.force_positions = true;
                return;
            }
            ("func_orator", "speakresponseconcept") => {
                // Response rules match the speaker's targetname ("name").
                let name = self.ents[ent].me.targetname.to_ascii_lowercase();
                let name = if name.is_empty() { "orator".to_string() } else { name };
                // "#n": the I/O actor (0 = player, n = bot n-1) for WhoDidIt.
                let who = match self.io_actor.filter(|a| *a != u64::MAX) {
                    Some(actor) => format!("orator:{name}#{actor}"),
                    None => format!("orator:{name}"),
                };
                self.cine.concepts.push((Some(who), param.trim().into()));
                return;
            }
            ("info_director", i) if i.starts_with("fireconceptto") => {
                let who = &i["fireconceptto".len()..];
                self.cine.concepts.push((
                    if who == "any" || who.is_empty() {
                        None
                    } else {
                        Some(who.to_string())
                    },
                    param.trim().to_string(),
                ));
                return;
            }
            ("logic_choreographed_scene", "start") => {
                self.cine.scene_starts.push(ent);
                return;
            }
            ("info_director", "releasesurvivorpositions") => {
                self.cine.release_positions = true;
                return;
            }
            (
                "info_director",
                "beginscript" | "endscript" | "forcepanicevent" | "panicevent" | "enable"
                | "disable" | "start" | "stop" | "resetmobtimer" | "endcustomscriptedstage",
            ) => {
                self.cine
                    .director_commands
                    .push((inp.clone(), param.into()));
                return;
            }
            _ => {}
        }
        if class == "logic_director_query" && inp == "howangry" {
            let me = &self.ents[ent].me;
            let min = me.kv_i32("minAngerRange").unwrap_or(1);
            let max = me.kv_i32("maxAngerRange").unwrap_or(min);
            let noise = me.kv_i32("noise").unwrap_or(0);
            let v = if self.competitive {
                min
            } else {
                let health = self.survivor_health.unwrap_or(400.);
                let roll = self.rng.random_int(1, 100);
                director_query_anger(health, roll, noise, min, max)
            };
            self.fire_output_value(ent, "OutAnger", Some(&v.to_string()), activator);
            return;
        }
        if class == "logic_branch_listener" && inp == "test" {
            self.evaluate_branch_listener(ent, true, activator);
            return;
        }
        if (class == "point_template" || class == "env_entity_maker") && inp == "forcespawn" {
            self.spawn_template(ent, activator);
            return;
        }
        let pane_surface = self.ents[ent].me.classname == "func_breakable_surf";
        match &mut self.ents[ent].kind {
            Kind::Relay { disabled } => match inp.as_str() {
                "trigger" => {
                    if !*disabled {
                        self.fire_output(ent, "OnTrigger", activator);
                    }
                }
                "enable" => *disabled = false,
                "disable" => *disabled = true,
                "toggle" => *disabled = !*disabled,
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Timer {
                enabled,
                next,
                refire,
                lo,
                hi,
                ..
            } => match inp.as_str() {
                "enable" => {
                    *enabled = true;
                    *next = now + *refire;
                }
                "disable" => *enabled = false,
                "toggle" => *enabled = !*enabled,
                "refiretime" => *refire = param.parse().unwrap_or(*refire),
                "lowerrandombound" => *lo = param.parse().unwrap_or(*lo),
                "upperrandombound" => *hi = param.parse().unwrap_or(*hi),
                "resettimer" => *next = now + *refire,
                "firetimer" => self.fire_output(ent, "OnTimer", activator),
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Case { values } => {
                let values = values.clone();
                match inp.as_str() {
                    "invalue" => {
                        let mut hit = false;
                        for (k, v) in values.iter().enumerate() {
                            if !v.is_empty() && v.eq_ignore_ascii_case(param) {
                                self.fire_output(ent, &format!("OnCase{:02}", k + 1), activator);
                                hit = true;
                            }
                        }
                        if !hit {
                            self.fire_output(ent, "OnDefault", activator);
                        }
                    }
                    "pickrandom" | "pickrandomshuffle" => {
                        // CLogicCase::InputPickRandom: among the cases whose
                        // OnCaseNN output is connected (c2m5's exit side
                        // case has outputs and no CaseNN values).
                        let _ = &values;
                        let filled: Vec<usize> = (0..16)
                            .filter(|k| {
                                let name = format!("oncase{:02}", k + 1);
                                self.ents[ent].conns.iter().any(|c| c.output.eq_ignore_ascii_case(&name))
                            })
                            .collect();
                        if filled.is_empty() {
                            self.fire_output(ent, "OnDefault", activator);
                        } else {
                            let k =
                                filled[self.rng.random_int(0, filled.len() as i32 - 1) as usize];
                            self.fire_output(ent, &format!("OnCase{:02}", k + 1), activator);
                        }
                    }
                    s if s.starts_with("fire") => {
                        if let Ok(n) = s[4..].parse::<usize>() {
                            self.fire_output(ent, &format!("OnCase{n:02}"), activator);
                        }
                    }
                    _ => self.unhandled.push((ent, input.into(), param.into())),
                }
            }
            Kind::Branch { value } => match inp.as_str() {
                "setvalue" => {
                    let old = *value;
                    *value = truthy(param);
                    if old != *value {
                        self.update_branch_listeners(ent, activator);
                    }
                }
                "setvaluetest" => {
                    let old = *value;
                    *value = truthy(param);
                    let new = *value;
                    let o = if new { "OnTrue" } else { "OnFalse" };
                    self.fire_output(ent, o, activator);
                    if old != new {
                        self.update_branch_listeners(ent, activator);
                    }
                }
                "toggle" => {
                    *value = !*value;
                    self.update_branch_listeners(ent, activator);
                }
                "toggletest" => {
                    *value = !*value;
                    let o = if *value { "OnTrue" } else { "OnFalse" };
                    self.fire_output(ent, o, activator);
                    self.update_branch_listeners(ent, activator);
                }
                "test" => {
                    let o = if *value { "OnTrue" } else { "OnFalse" };
                    self.fire_output(ent, o, activator);
                }
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Door(d) => match inp.as_str() {
                "open" | "toggle" | "use" if d.locked => {
                    self.fire_output(ent, "OnLockedUse", activator);
                }
                "setposition" => {
                    let g = param.trim().parse::<f32>().unwrap_or(1.0).clamp(0.0, 1.0);
                    // Source's SetPosition starts a move with its start sound
                    // but fires no OnOpen/OnClose.
                    if (g - d.frac).abs() > 1e-4 {
                        self.events.push(EntEvent::DoorSound { ent, output: "OnMoveStarted".into() });
                    }
                    d.goal = g;
                    d.state = if g > d.frac {
                        DoorState::Opening
                    } else if g < d.frac {
                        DoorState::Closing
                    } else {
                        d.state
                    };
                    self.events.push(EntEvent::Solid {
                        ent,
                        solid: !d.passable,
                    });
                }
                "open" => {
                    if d.state != DoorState::Open {
                        d.goal = 1.0;
                        d.state = DoorState::Opening;
                        self.events.push(EntEvent::Solid {
                            ent,
                            solid: !d.passable,
                        });
                        self.fire_output(ent, "OnOpen", activator);
                    }
                }
                "close" => {
                    if d.state != DoorState::Closed {
                        d.goal = 0.0;
                        d.state = DoorState::Closing;
                        self.fire_output(ent, "OnClose", activator);
                    }
                }
                "toggle" | "use" => {
                    let opening = matches!(d.state, DoorState::Closed | DoorState::Closing);
                    d.goal = if opening { 1.0 } else { 0.0 };
                    d.state = if opening {
                        DoorState::Opening
                    } else {
                        DoorState::Closing
                    };
                    let o = if opening { "OnOpen" } else { "OnClose" };
                    self.fire_output(ent, o, activator);
                }
                "lock" => d.locked = true,
                "unlock" => d.locked = false,
                "setspeed" => d.speed = param.parse().unwrap_or(d.speed),
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Elevator {
                target,
                speed,
                moving,
                top,
                bottom,
                offset,
            } => match inp.as_str() {
                "movetofloor" => {
                    // Floors are `info_elevator_floor` entities named by the parameter; the car goes to the floor's position
                    // relative to the `top` stop.
                    let want = param.trim().to_ascii_lowercase();
                    let (top, bottom) = (*top, *bottom);
                    let floor = self
                        .ents
                        .iter()
                        .find(|e| {
                            e.me.classname == "info_elevator_floor"
                                && e.me.targetname.eq_ignore_ascii_case(&want)
                        })
                        .map(|e| e.me.origin);
                    if let (Some(f), Kind::Elevator { target, moving, .. }) =
                        (floor, &mut self.ents[ent].kind)
                    {
                        *target = f - top;
                        *moving = true;
                        let _ = bottom;
                        self.fire_output(ent, "OnMoveStarted", activator);
                    }
                }
                "setspeed" => *speed = param.parse().unwrap_or(*speed),
                "stop" => *moving = false,
                _ => {
                    let _ = (target, offset);
                    self.unhandled.push((ent, input.into(), param.into()))
                }
            },
            Kind::Train { speed, old_speed, max_speed, forward, moving, .. } => {
                let value: f32 = param.trim().parse().unwrap_or(0.0);
                let max = *max_speed;
                let start = |speed: &mut f32, old_speed: &mut f32, moving: &mut bool| {
                    if *speed <= 0.0 {
                        *speed = if *old_speed > 0.0 { *old_speed } else { max };
                    }
                    *moving = true;
                };
                match inp.as_str() {
                    "setspeed" => {
                        *speed = value.clamp(0.0, 1.0) * max;
                        *moving = *speed > 0.0;
                    }
                    "setspeedreal" => {
                        *speed = value.max(0.0);
                        *moving = *speed > 0.0;
                    }
                    "setspeeddir" => {
                        *forward = value >= 0.0;
                        *speed = value.abs().min(1.0) * max;
                        *moving = *speed > 0.0;
                    }
                    "startforward" => {
                        *forward = true;
                        start(speed, old_speed, moving);
                    }
                    "startbackward" => {
                        *forward = false;
                        start(speed, old_speed, moving);
                    }
                    "resume" => start(speed, old_speed, moving),
                    "toggle" => {
                        if *moving {
                            *old_speed = *speed;
                            *moving = false;
                        } else {
                            start(speed, old_speed, moving);
                        }
                    }
                    "stop" => {
                        if *speed > 0.0 {
                            *old_speed = *speed;
                        }
                        *moving = false;
                    }
                    "reverse" => *forward = !*forward,
                    _ => self.unhandled.push((ent, input.into(), param.into())),
                }
            }
            Kind::Counter { value, min, max } => {
                let (mn, mx) = (*min, *max);
                let p: f32 = param.trim().parse().unwrap_or(0.0);
                let mut fire = true;
                match inp.as_str() {
                    "add" => *value += p,
                    "subtract" => *value -= p,
                    "setvalue" => *value = p,
                    "setvaluenofire" => {
                        *value = p;
                        fire = false;
                    }
                    "setmaxvalue" => *max = p,
                    "setminvalue" => *min = p,
                    "getvalue" => {
                        let v = *value;
                        let _ = v;
                        self.fire_output_value(ent, "OnGetValue", Some(&v.to_string()), activator);
                        return;
                    }
                    _ => {
                        self.unhandled.push((ent, input.into(), param.into()));
                        return;
                    }
                }
                if mx != 0.0 || mn != 0.0 {
                    *value = value.clamp(mn.min(mx), if mx > mn { mx } else { f32::MAX });
                }
                let v = *value;
                if fire {
                    self.fire_output_value(ent, "OutValue", Some(&v.to_string()), activator);
                    if mx > mn && v >= mx {
                        self.fire_output(ent, "OnHitMax", activator);
                    }
                    if v <= mn && (mx > mn || mn != 0.0) {
                        self.fire_output(ent, "OnHitMin", activator);
                    }
                }
            }
            Kind::CarAlarm { armed, .. } => match inp.as_str() {
                "enable" => *armed = true,
                "disable" => *armed = false,
                "triggeralarm" | "alarm" => {
                    self.trigger_car_alarm(ent);
                }
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::NavBlocker { blocked } => {
                match inp.as_str() {
                    "blocknav" => *blocked = true,
                    "unblocknav" => *blocked = false,
                    _ => {}
                }
                self.unhandled.push((ent, input.into(), param.into()));
            }
            Kind::Hurt { damage, .. } if inp == "hurt" => {
                self.events.push(EntEvent::Hurt {
                    ent,
                    damage: *damage * 0.5,
                });
            }
            Kind::Hurt { disabled, .. } | Kind::Push { disabled, .. } => match inp.as_str() {
                "enable" => *disabled = false,
                "disable" => *disabled = true,
                "toggle" => *disabled = !*disabled,
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Breakable { health } => match inp.as_str() {
                "shatter" if pane_surface => {
                    let mut point_radius=Vec3::ZERO;
                    for (i, value) in param.split_whitespace().take(3).enumerate() {
                        point_radius[i]=value.parse::<f32>().ok().filter(|v|v.is_finite()).unwrap_or(0.);
                    }
                    self.events.push(EntEvent::SurfaceShatter {ent,point_radius});
                },
                "break" | "shatter" | "kill" => self.break_ent(ent),
                "sethealth" => {
                    if let Some(value) = param.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        *health = value;
                        if let Some(d) = self.durability.get_mut(&ent) { d.health = value; }
                    }
                },
                "addhealth" => {
                    if let Some(value) = param.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        *health += value;
                        if let Some(d) = self.durability.get_mut(&ent) { d.health = *health; }
                    }
                },
                "removehealth" => {
                    let d = param.parse().unwrap_or(0.0);
                    self.damage_breakable(ent, d);
                }
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Particle { active, started } => match inp.as_str() {
                "start" | "activate" => {
                    *active = true;
                    *started = now;
                }
                "stop" | "stopplayendcap" | "destroyimmediately" => *active = false,
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Fire {
                active,
                extinguish_at,
                ..
            } => match inp.as_str() {
                "startfire" | "ignite" => {
                    *active = true;
                    *extinguish_at = 0.;
                    self.fire_output(ent, "OnIgnited", activator);
                }
                "extinguish" => {
                    let delay = param.parse::<f32>().unwrap_or(0.).max(0.);
                    if delay > 0. {
                        *extinguish_at = now + delay;
                    } else {
                        *active = false;
                        self.fire_output(ent, "OnExtinguished", activator);
                    }
                }
                "extinguishtemporary" => {
                    *active = false;
                    self.fire_output(ent, "OnExtinguished", activator);
                }
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Button {
                locked,
                pressed,
                wait,
                ready_at,
            } => match inp.as_str() {
                "press" | "use" => {
                    if *locked {
                        self.fire_output(ent, "OnUseLocked", activator);
                    } else if now >= *ready_at {
                        *pressed = true;
                        *ready_at = now + wait.max(0.0);
                        self.fire_output(ent, "OnPressed", activator);
                        self.fire_output(ent, "OnIn", activator);
                    }
                }
                "lock" => *locked = true,
                "unlock" => *locked = false,
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Trigger { disabled, .. } => match inp.as_str() {
                "enable" => *disabled = false,
                "disable" => *disabled = true,
                "toggle" => *disabled = !*disabled,
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Brush { enabled, solid } => match inp.as_str() {
                "enable" | "show" => {
                    *enabled = true;
                    self.events.push(EntEvent::Visible { ent, visible: true });
                    self.events.push(EntEvent::Solid { ent, solid: true });
                }
                "disable" | "hide" => {
                    *enabled = false;
                    self.events.push(EntEvent::Visible {
                        ent,
                        visible: false,
                    });
                    self.events.push(EntEvent::Solid { ent, solid: false });
                }
                "toggle" => {
                    *enabled = !*enabled;
                    let e = *enabled;
                    self.events.push(EntEvent::Visible { ent, visible: e });
                    self.events.push(EntEvent::Solid { ent, solid: e });
                }
                "disablecollision" => {
                    *solid = false;
                    self.events.push(EntEvent::Solid { ent, solid: false });
                }
                "enablecollision" => {
                    *solid = true;
                    self.events.push(EntEvent::Solid { ent, solid: true });
                }
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::TimedButton { disabled } => match inp.as_str() {
                "enable" | "unlock" => *disabled = false,
                "disable" | "lock" => *disabled = true,
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Finale {
                started,
                next_at,
                disabled,
                used,
                ..
            } => match inp.as_str() {
                "use" if *disabled || *started => {}
                "use" | "forcefinalestart" | "usefinale" | "finalestart" => {
                    let first = !*used;
                    *used = true;
                    if !*started {
                        *started = true;
                        *next_at = now;
                        if inp == "use" {
                            if first {
                                self.fire_output(ent, "FirstUseStart", activator);
                            }
                            self.fire_output(ent, "UseStart", activator);
                        }
                        self.fire_output(ent, "FinaleStart", activator);
                        // The Director loads the finale script after the
                        // map's FinaleStart outputs run: c3m4 ends its
                        // no-threat script there (director,EndScript), which
                        // would otherwise erase the finale's stage list.
                        self.queue_input_context("!caller", "__beginfinale", "", 0.0, activator, Some(ent));
                    }
                }
                "__beginfinale" => {
                    self.cine.director_commands.push(("BeginFinale".into(), String::new()));
                }
                "enable" => *disabled = false,
                "disable" => *disabled = true,
                // Escape sequence driven by the map's rescue-vehicle relays.
                "finaleescapevehiclereadyforsurvivors" => {
                    self.cine.finale_vehicle_ready = true;
                }
                "finaleescapeforcesurvivorpositions" => {
                    self.cine.finale_force_positions = true;
                }
                "finaleescapefinished" => {
                    if !self.cine.finale_won {
                        self.cine.finale_won = true;
                        self.fire_output(ent, "FinaleWon", activator);
                    }
                }
                "advancefinalestate" => {
                    self.cine.director_commands.push(("AdvanceFinaleState".into(), String::new()));
                }
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Ambient { playing } => match inp.as_str() {
                "playsound" => {
                    *playing = true;
                    self.events.push(EntEvent::Sound { ent, play: true });
                }
                "stopsound" | "fadeout" => {
                    *playing = false;
                    self.events.push(EntEvent::Sound { ent, play: false });
                }
                "togglesound" => {
                    *playing = !*playing;
                    let p = *playing;
                    self.events.push(EntEvent::Sound { ent, play: p });
                }
                _ => self.unhandled.push((ent, input.into(), param.into())),
            },
            Kind::Tonemap => {
                let v = param.trim().parse::<f32>().ok();
                match (inp.as_str(), v) {
                    ("setautoexposuremin", Some(v)) => self.tonemap.auto_min = Some(v),
                    ("setautoexposuremax", Some(v)) => self.tonemap.auto_max = Some(v),
                    ("settonemappercenttarget", Some(v)) => self.tonemap.percent_target = v,
                    ("settonemappercentbrightpixels", Some(v)) => {
                        self.tonemap.percent_bright_pixels = v
                    }
                    ("settonemapminavglum", Some(v)) => self.tonemap.min_avg_lum = v,
                    ("settonemaprate", Some(v)) => self.tonemap.rate = v,
                    ("setbloomscale", Some(v)) => self.tonemap.bloom_scale = Some(v),
                    ("usedefaultautoexposure", _) => {
                        self.tonemap.auto_min = None;
                        self.tonemap.auto_max = None;
                    }
                    ("usedefaultbloomscale", _) => self.tonemap.bloom_scale = None,
                    _ => self.unhandled.push((ent, input.into(), param.into())),
                }
            }
            Kind::Auto | Kind::None | Kind::ScavengeProgress { .. } => self.unhandled.push((ent, input.into(), param.into())),
        }
    }

    /// Activate the entities a `point_template` (or the template an `env_entity_maker` names) refers to.
    fn spawn_template(&mut self, ent: usize, activator: Option<usize>) {
        let tmpl = if self.ents[ent].me.classname == "env_entity_maker" {
            let name = self.ents[ent]
                .me
                .kv("EntityTemplate")
                .unwrap_or("")
                .to_string();
            self.find(&name)
                .into_iter()
                .find(|&i| self.ents[i].me.classname == "point_template")
        } else {
            Some(ent)
        };
        let Some(t) = tmpl else { return };
        let names: Vec<String> = (1..=16)
            .filter_map(|k| {
                self.ents[t]
                    .me
                    .kv(&format!("Template{k:02}"))
                    .map(|s| s.to_ascii_lowercase())
            })
            .collect();
        let mut spawned = Vec::new();
        for (i, e) in self.ents.iter_mut().enumerate() {
            if e.templated && names.iter().any(|n| target_matches(n, &e.me.targetname)) {
                e.alive = true;
                e.templated = false;
                spawned.push(i);
            }
        }
        for i in spawned {
            self.fog.changed(i,&self.ents[i]);
            self.events.push(EntEvent::Spawned { ent: i });
            self.events.push(EntEvent::Visible {
                ent: i,
                visible: true,
            });
            self.events.push(EntEvent::Solid {
                ent: i,
                solid: true,
            });
        }
        self.fire_output(t, "OnEntitySpawned", activator);
    }

    /// `activator` (index of an entity, or None for the player) started touching trigger `ent`.
    pub fn touch(&mut self, ent: usize, activator: Option<usize>) {
        self.touch_actor(ent, activator.map_or(u64::MAX, |id| id as u64), activator);
    }

    pub fn touch_actor(&mut self, ent: usize, actor: u64, activator: Option<usize>) {
        if !self.ents[ent].alive {
            return;
        }
        let class = self.ents[ent].me.classname.clone();
        let wait = self.ents[ent].me.kv_f32("wait").unwrap_or(0.2);
        let Kind::Trigger {
            disabled,
            fired,
            touching,
            ready_at,
        } = &mut self.ents[ent].kind
        else {
            return;
        };
        if *disabled {
            return;
        }
        let occupants = self.touchers.entry(ent).or_default();
        let first_all = occupants.is_empty();
        let first = occupants.insert(actor);
        *touching = true;
        let once = class == "trigger_once";
        if once && *fired {
            return;
        }
        let trigger = class != "trigger_look" && self.time >= *ready_at && (first || wait >= 0.);
        if trigger {
            *fired = true;
            *ready_at = if wait < 0. {
                f32::INFINITY
            } else {
                self.time + wait.max(0.01)
            };
        }
        if first {
            let previous_actor = self.io_actor;
            self.io_actor = Some(actor);
            self.fire_output(ent, "OnStartTouch", activator);
            if first_all {
                self.fire_output(ent, "OnStartTouchAll", activator);
            }
            self.io_actor = previous_actor;
        }
        if trigger {
            let previous_actor = self.io_actor;
            self.io_actor = Some(actor);
            self.fire_output(ent, "OnTrigger", activator);
            self.io_actor = previous_actor;
            if once {
                self.ents[ent].alive = false;
            }
        }
    }

    pub fn untouch(&mut self, ent: usize, activator: Option<usize>) {
        self.untouch_actor(ent, activator.map_or(u64::MAX, |id| id as u64), activator);
    }

    pub fn untouch_actor(&mut self, ent: usize, actor: u64, activator: Option<usize>) {
        if let Kind::Trigger { touching, .. } = &mut self.ents[ent].kind {
            let occupants = self.touchers.entry(ent).or_default();
            if occupants.remove(&actor) {
                *touching = !occupants.is_empty();
                let last = !*touching;
                let previous_actor = self.io_actor;
                self.io_actor = Some(actor);
                self.fire_output(ent, "OnEndTouch", activator);
                if last {
                    self.fire_output(ent, "OnEndTouchAll", activator);
                }
                self.io_actor = previous_actor;
            }
        }
    }

    /// `+use` on an entity: doors/buttons react.
    pub fn scavenge_progress(&self) -> Option<(u32,u32)> {
        self.ents.iter().filter(|e|e.alive).find_map(|e| match e.kind {
            Kind::ScavengeProgress{enabled:true,total,collected} => Some((collected,total)), _=>None,
        })
    }
    pub fn score_scavenge_item(&mut self) {
        for e in &mut self.ents {
            if e.alive {
                if let Kind::ScavengeProgress{total,collected,..}=&mut e.kind { *collected=collected.saturating_add(1).min(*total); }
            }
        }
        let directors:Vec<_>=self.ents.iter().enumerate().filter(|(_,e)|e.alive && e.me.classname=="info_director").map(|(id,_)|id).collect();
        for id in directors { self.fire_output_pub(id,"OnTeamScored"); }
    }
    pub fn use_ent(&mut self, ent: usize, activator: Option<usize>) {
        let spawnflags = self.ents[ent].me.spawnflags();
        let class = self.ents[ent].me.classname.clone();
        let ok = match class.as_str() {
            "func_button" | "func_rot_button" | "momentary_rot_button" => true,
            // Radios, stage buttons and other finale props.
            "trigger_finale" => matches!(self.ents[ent].kind, Kind::Finale { disabled: false, started: false, .. }),
            "func_door" | "func_door_rotating" => spawnflags & 256 != 0,
            "prop_door_rotating" | "prop_door_rotating_checkpoint" => spawnflags & 32768 == 0,
            _ => false,
        };
        if !ok {
            return;
        }
        let door = class.starts_with("prop_door_rotating");
        let was_open = matches!(
            &self.ents[ent].kind,
            Kind::Door(d) if matches!(d.state, DoorState::Open | DoorState::Opening)
        );
        // Source prop doors only close on +use with "Use closes" (8192).
        // Without it an opened door stays open (c1m2's alarmed store doors).
        if door && was_open && spawnflags & 8192 == 0 {
            return;
        }
        let opener = self.opener.take().filter(|(e, _)| *e == ent).map(|(_, p)| p);
        self.uses.push((ent, opener));
        if self.uses.len() > 64 {
            self.uses.remove(0);
        }
        self.input(ent, "Use", "", activator, None);
        // Double doors: every door named by `slavename` moves with this one
        // and fires its own outputs.
        let Some(slave) = self.ents[ent]
            .me
            .kv("slavename")
            .filter(|n| door && !n.is_empty())
            .map(|n| n.to_ascii_lowercase())
        else {
            return;
        };
        let opening = matches!(
            &self.ents[ent].kind,
            Kind::Door(d) if matches!(d.state, DoorState::Open | DoorState::Opening)
        );
        let slaves: Vec<usize> = (0..self.ents.len())
            .filter(|&i| {
                i != ent
                    && self.ents[i].alive
                    && self.ents[i].me.classname.starts_with("prop_door_rotating")
                    && self.ents[i].me.targetname.eq_ignore_ascii_case(&slave)
            })
            .collect();
        for i in slaves {
            self.input(i, if opening { "Open" } else { "Close" }, "", activator, None);
        }
    }

    /// World AABB of a trigger/brush entity (model bounds + origin).
    pub fn brush_bounds(&self, ent: usize) -> Option<(Vec3, Vec3)> {
        let e = &self.ents[ent];
        let m = self.bsp.models.get(e.me.brush?)?;
        let moved = self.world_matrix(ent).w_axis.truncate() - e.me.origin;
        let moved = if matches!(e.kind, Kind::Door(Door { rotating: true, .. })) {
            Vec3::ZERO
        } else {
            moved
        };
        Some((m.mins + e.me.origin + moved, m.maxs + e.me.origin + moved))
    }
}

fn k_is_output(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    (n.starts_with("on") && n.len() > 2) || matches!(n.as_str(), "outanger" | "outvalue")
}

fn initial_kind(me: &MapEnt, bsp: &Bsp) -> Kind {
    let f = |k: &str, d: f32| me.kv_f32(k).unwrap_or(d);
    // UNVERIFIED: window panes are `prop_physics` whose `prop_data` base is `Glass.*`; the model name stands in and health 1 is a guess.
    if matches!(
        me.classname.as_str(),
        "prop_physics" | "prop_physics_override" | "prop_physics_multiplayer"
    ) {
        if let Some(m) = me.mdl.as_deref() {
            let m = m.to_ascii_lowercase();
            if m.contains("window") && m.contains("glass") {
                return Kind::Breakable { health: 1.0 };
            }
        }
    }
    match me.classname.as_str() {
        "game_scavenge_progress_display" => Kind::ScavengeProgress {
            enabled: false, total: me.kv_i32("max").unwrap_or(1).max(0) as u32, collected: 0,
        },
        "logic_auto" => Kind::Auto,
        "logic_relay" => Kind::Relay {
            disabled: me.kv_i32("StartDisabled") == Some(1),
        },
        "logic_timer" => {
            let enabled = me.kv_i32("StartDisabled") != Some(1);
            let refire = f("RefireTime", 1.0);
            Kind::Timer {
                enabled,
                next: refire,
                refire,
                lo: f("LowerRandomBound", 0.0),
                hi: f("UpperRandomBound", 0.0),
                random: me.kv_i32("UseRandomTime") == Some(1),
            }
        }
        "logic_case" => Kind::Case {
            values: (1..=16)
                .map(|i| me.kv(&format!("Case{i:02}")).unwrap_or("").to_string())
                .collect(),
        },
        "logic_branch" => Kind::Branch {
            value: me.kv_i32("InitialValue") == Some(1),
        },
        "func_button_timed" => Kind::TimedButton {
            disabled: me.kv_i32("StartDisabled") == Some(1),
        },
        "func_button" | "func_rot_button" => Kind::Button {
            locked: me.spawnflags() & 2048 != 0,
            pressed: false,
            wait: f("wait", 3.0),
            ready_at: 0.0,
        },
        "trigger_once"
        | "trigger_multiple"
        | "trigger_playermovement"
        | "trigger_look"
        | "info_changelevel"
        | "trigger_changelevel" => Kind::Trigger {
            disabled: me.kv_i32("StartDisabled") == Some(1),
            fired: false,
            touching: false,
            ready_at: 0.,
        },
        "env_tonemap_controller" => Kind::Tonemap,
        "ambient_generic" => Kind::Ambient { playing: false },
        "info_particle_system" => Kind::Particle {
            active: me.kv_i32("start_active") == Some(1),
            started: 0.,
        },
        "env_fire" => Kind::Fire {
            active: me.spawnflags() & 4 != 0,
            size: f("firesize", 64.).max(1.),
            damage: f("damagescale", 1.).max(0.),
            extinguish_at: 0.,
        },
        "trigger_finale" => Kind::Finale {
            started: false,
            phase: 0,
            next_at: 0.0,
            waves_left: 0,
            ftype: me.kv_i32("type").unwrap_or(0),
            disabled: me.kv_i32("StartDisabled").unwrap_or(0) != 0,
            used: false,
        },
        "func_brush"
        | "func_wall_toggle"
        | "prop_dynamic"
        | "prop_dynamic_override"
        | "func_playerinfected_clip"
        | "func_playerghostinfected_clip" => Kind::Brush {
            enabled: me.kv_i32("startdisabled") != Some(1),
            solid: me.kv_i32("solid") != Some(0),
        },
        "prop_car_alarm" => Kind::CarAlarm {
            armed: me.kv_i32("startdisabled") != Some(1),
            until: 0.0,
            triggered: false,
            chirp_until: 0.0,
            next_chirp: 0.0,
        },
        "math_counter" => Kind::Counter {
            value: f("startvalue", 0.0),
            min: f("min", 0.0),
            max: f("max", 0.0),
        },
        "func_nav_blocker" => Kind::NavBlocker {
            blocked: me.kv_i32("StartDisabled") != Some(1),
        },
        "trigger_hurt" => Kind::Hurt {
            disabled: me.kv_i32("StartDisabled") == Some(1),
            damage: f("damage", 10.0),
        },
        "trigger_push" => Kind::Push {
            disabled: me.kv_i32("StartDisabled") == Some(1),
            dir: movedir(me.kv("pushdir").unwrap_or("0 0 0"), me.angles),
            speed: f("speed", 40.0),
        },
        "func_breakable" | "func_breakable_surf" | "prop_breakable" if f("health", 0.0) > 0.0 => {
            Kind::Breakable {
                health: f("health", 0.0),
            }
        }
        "func_tracktrain" => Kind::Train {
            offset: Vec3::ZERO,
            // Source: `speed` is the initial speed, `startspeed` the maximum.
            speed: f("speed", 0.0),
            old_speed: 0.0,
            max_speed: f("startspeed", 100.0),
            forward: true,
            moving: f("speed", 0.0) > 0.0,
            at: None,
        },
        "func_elevator" => {
            let v = |k: &str| {
                let n: Vec<f32> = me
                    .kv(k)
                    .unwrap_or("0 0 0")
                    .split_whitespace()
                    .filter_map(|t| t.parse().ok())
                    .collect();
                if n.len() == 3 {
                    Vec3::new(n[0], n[1], n[2])
                } else {
                    Vec3::ZERO
                }
            };
            Kind::Elevator {
                offset: Vec3::ZERO,
                target: Vec3::ZERO,
                speed: f("speed", 100.0),
                moving: false,
                top: v("top"),
                bottom: v("bottom"),
            }
        }
        "func_door"
        | "func_door_rotating"
        | "prop_door_rotating"
        | "prop_door_rotating_checkpoint"
        | "func_movelinear" => {
            let rotating = !matches!(me.classname.as_str(), "func_door" | "func_movelinear");
            let speed = f("speed", if rotating { 90.0 } else { 100.0 });
            let sf = me.spawnflags();
            let (dist, dir) = if me.classname == "func_movelinear" {
                (
                    f("movedistance", 100.0),
                    movedir(me.kv("movedir").unwrap_or("0 0 0"), me.angles),
                )
            } else if me.classname == "func_door" {
                // movedir is an angle triplet; travel = extent along it minus lip.
                let dir = movedir(me.kv("movedir").unwrap_or("0 0 0"), me.angles);
                let size = me
                    .brush
                    .and_then(|b| bsp.models.get(b))
                    .map(|m| (m.maxs - m.mins).abs())
                    .unwrap_or(Vec3::splat(64.0));
                (size.dot(dir.abs()) - f("lip", 0.0), dir)
            } else {
                let axis = if me.classname == "func_door_rotating" {
                    if sf & 64 != 0 {
                        Vec3::X
                    } else if sf & 128 != 0 {
                        Vec3::Y
                    } else {
                        Vec3::Z
                    }
                } else {
                    Vec3::Z
                };
                let reverse = if me.classname == "func_door_rotating" && sf & 2 != 0 {
                    -1.0
                } else {
                    1.0
                };
                let distance = f("distance", 90.0);
                let distance = if distance.abs() < 0.001 {
                    90.0
                } else {
                    distance.abs()
                };
                let reverse = if me.classname.starts_with("prop_door_rotating") {
                    if me.kv_i32("spawnpos") == Some(2)
                        || (me.kv_i32("spawnpos").unwrap_or(0) == 0
                            && me.kv_i32("opendir") == Some(2))
                    {
                        1.0
                    } else {
                        -1.0
                    }
                } else {
                    reverse
                };
                (distance * reverse, axis)
            };
            let starts_open = if me.classname == "func_movelinear" {
                f("startposition", 0.0) > 0.5
            } else if me.classname == "func_door" || me.classname == "func_door_rotating" {
                // CBaseDoor: the obsolete "Starts Open" spawnflag or the
                // "spawnpos" key (1 = open). c1m4's door_elevator_top spawns
                // open with spawnpos 1, letting the team into the elevator.
                sf & 1 != 0 || me.kv_i32("spawnpos") == Some(1)
            } else {
                me.kv_i32("spawnpos").unwrap_or(0) != 0
            };
            Kind::Door(Door {
                rotating,
                state: if starts_open {
                    DoorState::Open
                } else {
                    DoorState::Closed
                },
                frac: if starts_open { 1.0 } else { 0.0 },
                speed,
                dist,
                forward_sign: -1.0,
                dir,
                wait: if me.classname == "func_movelinear" {
                    -1.0
                } else if rotating && me.classname == "prop_door_rotating" {
                    me.kv_f32("returndelay").unwrap_or(-1.0)
                } else {
                    f("wait", 4.0)
                },
                close_at: 0.0,
                locked: sf & 2048 != 0 || me.kv_i32("Locked") == Some(1),
                passable: sf & 8 != 0 && me.classname.starts_with("func_door"),
                goal: if starts_open { 1.0 } else { 0.0 },
            })
        }
        _ => Kind::None,
    }
}

/// Source `movedir`: pitch/yaw/roll angle triplet -> unit direction (`Up` = (-90,0,0) is +Z; `Down` (-2,0,0) is -Z).
fn movedir(s: &str, ent_angles: Vec3) -> Vec3 {
    let v: Vec<f32> = s
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    let a = if v.len() == 3 {
        Vec3::new(v[0], v[1], v[2])
    } else {
        ent_angles
    };
    // Hammer encodes "Up"/"Down" as special angle values.
    if a == Vec3::new(-1.0, 0.0, 0.0) {
        return Vec3::Z;
    }
    if a == Vec3::new(-2.0, 0.0, 0.0) {
        return -Vec3::Z;
    }
    angle_matrix(a).transform_vector3(Vec3::X)
}

/// Owned CLogicDirectorQuery::InputHowAngry (server 10263210). "True anger"
/// is 1 + 49 x (summed standing-survivor health x 0.25 / 100, clamped to 1);
/// with probability `noise`% it is replaced by (roll - 1) / 99 x 50. The
/// integer part maps linearly into min..max, rounding half up below max.
pub fn director_query_anger(health: f32, roll: i32, noise: i32, min: i32, max: i32) -> i32 {
    let truth = (health * 0.25 * 0.01).clamp(0., 1.) * 49. + 1.;
    let processed = if roll <= noise {
        ((roll as f32 - 1.) * (1. / 99.)).clamp(0., 1.) * 50.
    } else {
        truth
    };
    let out = (max - min) as f32 * ((processed as i32) as f32 - 1.) * (1. / 49.) + min as f32;
    let mut v = out as i32;
    if out - v as f32 >= 0.5 && v < max {
        v += 1;
    }
    v
}

/// Retail car alarm cvars (server.dll registration defaults): car_alarm_distance,
/// car_alarm_duration, car_alarm_chirp_distance/interval/duration.
pub const CAR_ALARM_DISTANCE: f32 = 750.;
pub const CAR_ALARM_DURATION: f32 = 15.;
pub const CAR_ALARM_CHIRP_DISTANCE: f32 = 350.;
pub const CAR_ALARM_CHIRP_INTERVAL: f32 = 3.;
pub const CAR_ALARM_CHIRP_DURATION: f32 = 1.;

/// Source entity-name matching: case-insensitive, a trailing `*` matches any
/// suffix.
fn name_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix),
        None => pattern.eq_ignore_ascii_case(name),
    }
}
