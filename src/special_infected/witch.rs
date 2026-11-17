//! Witch: an AI boss the survivor team encounters (not a playable role).
//!
//! The witch is an ordinary `game::Enemy` (so player/bot gunfire, hitboxes,
//! death animations and ragdolls are shared with the common infected) plus a
//! [`Witch`] brain. `game::enemy_ai` skips her; [`tick`] drives her after it.
//! Behaviour is reconstructed from retail server.dll (build 10097) Witch
//! behaviours and the live retail cvars; see
//! `local-research/si-agents/witch/NOTES.md` for the evidence.
use crate::{
    campaign_settings::Difficulty,
    collision::{Collision, HEIGHT, UNIT},
    game::{Body, Enemy},
    game_state::{Match, Rules, Squad},
};
use bevy::prelude::*;
use l4d2r_game_core::random::UniformRandomStream;
pub const MODEL: &str = "models/infected/witch.mdl";
/// The Passing's bride (CWitch::Spawn 1041bac0 uses it when flagged).
pub const BRIDE_MODEL: &str = "models/infected/witch_bride.mdl";
// Retail cvars (netconsole, build 10097).
/// z_witch_health
pub const HEALTH: f32 = 1000.;
/// z_witch_speed / z_witch_speed_inured (set by "Caught on fire!").
pub const SPEED: f32 = 300.;
pub const SPEED_INURED: f32 = 200.;
pub const ANGER_RATE: f32 = 0.2;
pub const RELAX_RATE: f32 = 0.05;
pub const THREAT_NORMAL_RANGE: f32 = 300.;
pub const THREAT_HOSTILE_RANGE: f32 = 600.;
pub const HOSTILE_AT_ME_ANGER: f32 = 2.;
pub const FLASHLIGHT_RANGE: f32 = 400.;
pub const PERSONAL_SPACE: f32 = 100.;
pub const ATTACK_RANGE: f32 = 60.;
pub const DAMAGE_PER_KILL_HIT: f32 = 30.;
pub const BURN_TIME: f32 = 15.;
pub const RETREAT_MIN_DURATION: f32 = 10.;
pub const RETREAT_EXIT_RANGE: f32 = 1000.;
pub const RETREAT_EXIT_HIDDEN: f32 = 10.;
pub const MIN_RETREAT_RANGE: f32 = 750.;
pub const WANDER_PERSONAL_SPACE: f32 = 240.;
pub const WANDER_PERSONAL_TIME: f32 = 10.;
pub const WANDER_HEAR_RADIUS: f32 = 72.;
pub const WANDER_MUSIC_MIN_DIST: f32 = 90000.;
pub const WANDER_MUSIC_MAX_DIST: f32 = 2000000.;
pub const WANDER_MUSIC_MIN_INTERVAL: f32 = 3.;
pub const WANDER_MUSIC_MAX_INTERVAL: f32 = 20.;
/// z_shotgun_bonus_damage_range / _multiplier (the witch "crown").
pub const SHOTGUN_BONUS_RANGE: f32 = 100.;
pub const SHOTGUN_BONUS_MULTIPLIER: f32 = 5.;
/// Hard-coded in the witch claw handler (FUN_1041b300): 100 slash damage.
pub const CLAW_DAMAGE: f32 = 100.;
/// Retail server.dll constants (FUN_1041dd70, FUN_10420fa0, FUN_1041eab0).
const INJURED_RESUME: f32 = 5.;
/// Bullet damage above this on her head is a "high-powered rifle headshot"
/// (FUN_10420970, DAT_105d6268).
pub const RIFLE_HEADSHOT_DAMAGE: f32 = 50.;
static HEAD_SHOTS: std::sync::Mutex<Vec<(Entity, f32)>> = std::sync::Mutex::new(Vec::new());
/// The local player's bullet hit an infected's head for this much damage.
pub(crate) fn head_shot(entity: Entity, damage: f32) {
    if let Ok(mut shots) = HEAD_SHOTS.lock() {
        shots.push((entity, damage));
    }
}
const RANGE_SLACK: f32 = 50.;
const LOOKING_COS: f32 = 0.98;
const FRENZY_RANGE: f32 = 30.;
const FRENZY_LEASH: f32 = 60.;
const WANDER_SPEED: f32 = 35.;
const SHOT_MEMORY: f32 = 0.5;
/// Clip durations in models/infected/anim_witch.mdl (frames / fps).
pub const KILL_DISPLAY: f32 = 100. / 30.;
pub const KILL_LOOP: f32 = 58. / 25.;
pub const PRE_RETREAT: f32 = 180. / 60.;
pub const WANDER_ACQUIRE: f32 = 30. / 30.;
/// Shoved clips by the shooter's side (front shooter pushes her backward):
/// Shoved_Backward_03 69/30, Shoved_Forward_01 78/33, Shoved_Rightward_01
/// 103/33, Shoved_Leftward_01 105/33 (a shooter on her left pushes right).
pub const SHOVED: [(&str, f32); 4] = [
    ("ACT_TERROR_SHOVED_BACKWARD", 69. / 30.),
    ("ACT_TERROR_SHOVED_FORWARD", 78. / 33.),
    ("ACT_TERROR_SHOVED_RIGHTWARD", 103. / 33.),
    ("ACT_TERROR_SHOVED_LEFTWARD", 105. / 33.),
];
/// AE_ATTACK_HIT cycles of killing_blow_loop.
const KILL_HITS: [f32; 8] = [0.02, 0.14, 0.26, 0.40, 0.52, 0.64, 0.78, 0.90];
/// AE_ATTACK_HIT time of Melee_01 (cycle .40 of 2.0s) and Melee_02 (.29 of 1.825s).
const SWING_HITS: [f32; 2] = [0.8, 0.529];
const SOUND_ID: usize = 0x5717_0000;

/// Who the witch is aware of / attacking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    Player,
    Bot(usize),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Sitting and crying, or wandering (day).
    Calm,
    /// Sitting witch building rage (retail "WitchAngry").
    Angry,
    /// Wandering witch acquiring its victim before the chase.
    Acquire {
        victim: Who,
        clock: f32,
    },
    Attack {
        victim: Who,
    },
    /// The infected shoved action (server 1040e280) a high-powered rifle
    /// headshot runs before WitchAttack: the directional shoved clip, pushed
    /// away from the shooter, played to its end.
    Shoved {
        victim: Who,
        clock: f32,
        side: u8,
    },
    /// Kill display then the shred loop on an incapacitated victim.
    Frenzy {
        victim: Who,
        clock: f32,
        hits: u32,
    },
    Retreat {
        clock: f32,
        hidden: f32,
    },
    Dead,
}
/// One living survivor as the witch perceives it this tick.
#[derive(Clone, Copy, Debug)]
pub struct Seen {
    pub who: Who,
    pub feet: Vec3,
    /// Distance in Source units.
    pub distance: f32,
    pub visible: bool,
    /// Survivor aims at her head within the retail 0.98 cosine.
    pub looking: bool,
    pub flashlight: bool,
    /// Fired a weapon within the last 0.5 seconds.
    pub fired: bool,
    /// Fired this tick (a noise event).
    pub fired_now: bool,
    pub incap: bool,
}
#[derive(Clone, Debug)]
pub struct Perception {
    pub dt: f32,
    pub survivors: Vec<Seen>,
    pub injured_by: Option<Who>,
    /// A bile jar burst on her this tick, thrown by this survivor.
    pub vomited_by: Option<Who>,
    /// A bullet headshot over RIFLE_HEADSHOT_DAMAGE this tick.
    pub rifle_headshot_by: Option<Who>,
    /// Side of her the headshot came from: 0 front, 1 back, 2 left, 3 right.
    pub shot_side: u8,
    pub burning: bool,
    pub moving: bool,
    pub difficulty: Difficulty,
}
impl Perception {
    fn get(&self, who: Who) -> Option<&Seen> {
        self.survivors.iter().find(|s| s.who == who)
    }
    /// Retail vision primary known threat: the closest visible survivor.
    fn primary(&self) -> Option<&Seen> {
        self.survivors
            .iter()
            .filter(|s| s.visible)
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Positional one-shot at the witch.
    Sound(&'static str),
    /// Positional voice that is stopped when the witch changes state.
    Voice(&'static str),
    Music(&'static str),
    StopMusic,
    StopVoice,
    /// Claw hit (z_witch claw, 100 damage; Expert kills).
    Strike(Who),
    /// Kill-loop shred hit.
    Shred(Who),
    Swing,
    Kill,
    Despawn,
}
#[derive(Component)]
pub struct Witch {
    pub phase: Phase,
    pub wander: bool,
    /// Retail m_rage (0..1).
    pub rage: f32,
    pub burning: bool,
    burn_clock: f32,
    injured_ago: f32,
    last_attacker: Option<Who>,
    victim_since: f32,
    despair_clock: f32,
    growl_clock: f32,
    growl_band: u8,
    voice_clock: f32,
    swing: Option<(f32, f32, bool)>,
    swing_index: usize,
    walk_clock: f32,
    walking: bool,
    personal_clock: f32,
    music_clock: f32,
    music_since: f32,
    pub(crate) moving: bool,
    pub(crate) last_health: f32,
    pub(crate) id: usize,
    pub(crate) wander_goal: Option<Vec3>,
    rng: UniformRandomStream,
    /// witch_bride population: bride model and Event.WitchRageBride.
    pub bride: bool,
    /// The info_zombie_spawn that made her (its OnStartled output fires).
    pub(crate) map_spawn: Option<usize>,
    startled_fired: bool,
    biled: bool,
}
impl Witch {
    pub fn new(wander: bool, id: usize) -> Self {
        let mut rng = UniformRandomStream::new(0x5717 + id as i32);
        Self {
            phase: Phase::Calm,
            wander,
            rage: 0.,
            burning: false,
            burn_clock: 0.,
            injured_ago: f32::INFINITY,
            last_attacker: None,
            victim_since: 0.,
            despair_clock: 0.,
            growl_clock: 0.,
            growl_band: 0,
            voice_clock: 0.,
            swing: None,
            swing_index: 0,
            walk_clock: rng.random_float(4., 8.),
            walking: wander,
            personal_clock: 0.,
            music_clock: 0.,
            music_since: f32::INFINITY,
            moving: false,
            last_health: HEALTH,
            id,
            wander_goal: None,
            rng,
            bride: false,
            map_spawn: None,
            startled_fired: false,
            biled: false,
        }
    }
    /// Retail rage clamp (FUN_1041c5a0): m_rage stays in 0..1.
    fn add_rage(&mut self, amount: f32) {
        self.rage = (self.rage + amount).clamp(0., 1.);
    }
    /// Survivor bots hold fire and keep their distance while this is true.
    pub fn calm(&self) -> bool {
        matches!(self.phase, Phase::Calm | Phase::Angry)
    }
    pub fn victim(&self) -> Option<Who> {
        match self.phase {
            Phase::Attack { victim }
            | Phase::Frenzy { victim, .. }
            | Phase::Acquire { victim, .. }
            | Phase::Shoved { victim, .. } => Some(victim),
            _ => None,
        }
    }
    fn attack(&mut self, victim: Who, out: &mut Vec<Effect>) {
        self.rage = 1.;
        self.victim_since = 0.;
        self.swing = None;
        self.voice_clock = 0.;
        self.phase = Phase::Attack { victim };
        out.push(Effect::StopVoice);
        out.push(Effect::Music(if self.burning {
            "Event.WitchBurning"
        } else {
            "Event.WitchAttack"
        }));
    }
    fn startle(&mut self, victim: Who, out: &mut Vec<Effect>) {
        if self.wander && self.phase == Phase::Calm {
            self.rage = 1.;
            self.phase = Phase::Acquire { victim, clock: 0. };
            out.push(Effect::StopVoice);
            out.push(Effect::Sound("WitchZombie.Scream"));
        } else {
            self.attack(victim, out);
        }
    }
    fn retreat(&mut self, out: &mut Vec<Effect>) {
        self.phase = Phase::Retreat {
            clock: 0.,
            hidden: 0.,
        };
        self.voice_clock = 0.;
        self.swing = None;
        out.push(Effect::StopMusic);
    }
    /// Sitting-witch growls (FUN_1041d510): low < .3 <= medium < .6 <= high.
    fn growl(&mut self, out: &mut Vec<Effect>) {
        let band = growl_band(self.rage);
        if band > self.growl_band {
            self.growl_clock = 0.;
        }
        self.growl_band = band;
        if self.growl_clock <= 0. {
            out.push(Effect::Sound(growl_sound(band)));
            self.growl_clock = if band == 2 {
                self.rng.random_float(1.5, 2.)
            } else {
                self.rng.random_float(0.75, 1.5)
            };
        }
    }
    /// One retail behaviour update.
    pub fn think(&mut self, p: &Perception) -> Vec<Effect> {
        let mut out = Vec::new();
        let dt = p.dt;
        if self.phase == Phase::Dead {
            return out;
        }
        self.moving = p.moving;
        self.injured_ago += dt;
        self.victim_since += dt;
        self.growl_clock -= dt;
        self.voice_clock -= dt;
        self.despair_clock -= dt;
        self.music_since += dt;
        let injured = p.injured_by.filter(|who| p.get(*who).is_some());
        if let Some(who) = injured {
            self.injured_ago = 0.;
            self.last_attacker = Some(who);
        }
        // Fire: "Caught on fire!" -> chase at z_witch_speed_inured; burns to
        // death after z_witch_burn_time (FUN_1041fe10).
        if p.burning && !self.burning {
            self.burning = true;
            self.burn_clock = 0.;
            out.push(Effect::Sound("Zombie.IgniteScream"));
            out.push(Effect::Music("Event.WitchBurning"));
            let victim = injured
                .or(self.victim())
                .or_else(|| p.primary().map(|s| s.who))
                .or_else(|| p.survivors.first().map(|s| s.who));
            if let Some(victim) = victim.filter(|_| self.victim().is_none()) {
                self.attack(victim, &mut out);
            }
        }
        if self.burning {
            self.burn_clock += dt;
            if self.burn_clock > BURN_TIME {
                out.push(Effect::Kill);
                return out;
            }
            if self.voice_clock <= 0. {
                out.push(Effect::Sound("WitchZombie.PainFire"));
                self.voice_clock = self.rng.random_float(2., 3.);
            }
        }
        // Idle/angry OnHitByVomitJar (FUN_1041d940, "Vomit thrown on me"):
        // full rage and straight to WitchAttack on the thrower.
        if let Some(who) = p.vomited_by.filter(|w| p.get(*w).is_some()) {
            if matches!(self.phase, Phase::Calm | Phase::Angry) {
                self.add_rage(1.);
                self.attack(who, &mut out);
                return out;
            }
        }
        // Idle/angry OnInjured (FUN_10420970, "Reacting to high-powered rifle
        // headshot"): full rage, WitchZombie.HeadShot, a shove reaction, then
        // WitchAttack on the shooter.
        if let Some(who) = p.rifle_headshot_by.filter(|w| p.get(*w).is_some()) {
            if matches!(self.phase, Phase::Calm | Phase::Angry) {
                self.add_rage(1.);
                out.push(Effect::Sound("WitchZombie.HeadShot"));
                out.push(Effect::StopVoice);
                self.phase = Phase::Shoved { victim: who, clock: 0., side: p.shot_side };
                return out;
            }
        }
        match self.phase {
            Phase::Calm => {
                if self.despair_clock <= 0. {
                    out.push(Effect::Voice(if self.wander {
                        "WanderWitchZombie.Despair"
                    } else {
                        "WitchZombie.Despair"
                    }));
                    self.despair_clock = self.rng.random_float(4., 6.);
                }
                if let Some(who) = injured {
                    self.startle(who, &mut out);
                } else if self.wander {
                    self.calm_wander(p, &mut out);
                } else {
                    self.calm_sitting(p, &mut out);
                }
            }
            Phase::Angry => self.angry(p, &mut out),
            Phase::Acquire { victim, clock } => {
                let clock = clock + dt;
                self.phase = if clock >= WANDER_ACQUIRE {
                    self.attack(victim, &mut out);
                    self.phase
                } else {
                    Phase::Acquire { victim, clock }
                };
            }
            Phase::Shoved { victim, clock, side } => {
                let clock = clock + dt;
                if clock >= SHOVED[side.min(3) as usize].1 {
                    self.attack(victim, &mut out);
                } else {
                    self.phase = Phase::Shoved { victim, clock, side };
                }
            }
            Phase::Attack { victim } => self.attacking(victim, p, injured, &mut out),
            Phase::Frenzy {
                victim,
                clock,
                hits,
            } => self.frenzy(victim, clock, hits, p, &mut out),
            Phase::Retreat { clock, hidden } => {
                let clock = clock + dt;
                let seen = p.survivors.iter().any(|s| s.visible);
                let hidden = if seen { 0. } else { hidden + dt };
                if self.voice_clock <= 0. {
                    out.push(Effect::Sound("WitchZombie.RetreatHorror"));
                    self.voice_clock = self.rng.random_float(1.5, 2.5);
                }
                let nearest = p
                    .survivors
                    .iter()
                    .map(|s| s.distance)
                    .fold(f32::INFINITY, f32::min);
                if clock > RETREAT_MIN_DURATION
                    && (nearest > RETREAT_EXIT_RANGE && hidden > 1. || hidden > RETREAT_EXIT_HIDDEN)
                {
                    out.push(Effect::Despawn);
                }
                self.phase = Phase::Retreat { clock, hidden };
            }
            Phase::Dead => {}
        }
        out
    }
    fn calm_sitting(&mut self, p: &Perception, out: &mut Vec<Effect>) {
        // Idle update (FUN_10420250): a survivor in personal space surprises her.
        let mut threat = p.primary().copied();
        if threat.is_none_or(|t| t.distance >= PERSONAL_SPACE) {
            if let Some(close) = p
                .survivors
                .iter()
                .filter(|s| s.visible && s.distance < PERSONAL_SPACE)
                .min_by(|a, b| a.distance.total_cmp(&b.distance))
            {
                out.push(Effect::Sound("WitchZombie.Surprised"));
                self.growl_clock = 1.;
                threat = Some(*close);
            }
        }
        let alerted = threat.is_some_and(|t| t.distance < threat_range(hostile(&t)));
        if alerted || self.rage > 0. {
            self.phase = Phase::Angry;
            self.growl_band = 0;
            out.push(Effect::StopVoice);
            out.push(Effect::Music("Event.WitchRage"));
        }
    }
    fn calm_wander(&mut self, p: &Perception, out: &mut Vec<Effect>) {
        // Wander update (FUN_10423d50): walk at 35 u/s, idle 3-6s, startle
        // when a survivor stays in z_witch_wander_personal_space for
        // z_witch_wander_personal_time or shoots within the hear radius.
        self.walk_clock -= p.dt;
        if self.walk_clock <= 0. {
            self.walking = !self.walking;
            self.walk_clock = if self.walking {
                self.rng.random_float(4., 8.)
            } else {
                self.rng.random_int(3, 6) as f32
            };
            self.wander_goal = None;
        }
        if let Some(noisy) = p
            .survivors
            .iter()
            .find(|s| s.fired_now && s.distance < WANDER_HEAR_RADIUS)
        {
            self.startle(noisy.who, out);
            return;
        }
        let close = p
            .survivors
            .iter()
            .filter(|s| s.distance < WANDER_PERSONAL_SPACE)
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
            .copied();
        if let Some(close) = close {
            self.walking = false;
            self.personal_clock += p.dt;
            self.rage = (self.personal_clock / WANDER_PERSONAL_TIME).min(1.);
            if self.growl_clock <= 0. {
                let remaining = 1. - self.rage;
                out.push(Effect::Sound(if remaining < 0.25 {
                    "WitchZombie.GrowlHigh"
                } else if remaining < 0.5 {
                    "WitchZombie.GrowlMedium"
                } else {
                    "WitchZombie.GrowlLow"
                }));
                self.growl_clock = self.rng.random_float(1., 2.);
            }
            if self.personal_clock >= WANDER_PERSONAL_TIME {
                self.startle(close.who, out);
            }
            return;
        }
        self.personal_clock = 0.;
        self.rage = 0.;
        let nearest = p
            .survivors
            .iter()
            .map(|s| s.distance * s.distance)
            .fold(f32::INFINITY, f32::min);
        if self.music_since > wander_music_interval(nearest) && self.music_clock <= 0. {
            out.push(Effect::Music("Event.WanderingWitch"));
            self.music_since = 0.;
            self.music_clock = self.rng.random_float(0.25, 1.);
        }
        self.music_clock -= p.dt;
    }
    fn angry(&mut self, p: &Perception, out: &mut Vec<Effect>) {
        // WitchAngry update (FUN_1041dd70).
        self.growl(out);
        if self.injured_ago < INJURED_RESUME {
            if let Some(who) = self.last_attacker.filter(|w| p.get(*w).is_some()) {
                self.attack(who, out);
                return;
            }
        }
        // Noise events from living survivors (FUN_1041d7d0).
        for _ in p.survivors.iter().filter(|s| s.fired_now) {
            self.add_rage(ANGER_RATE * HOSTILE_AT_ME_ANGER * p.dt);
        }
        let threat = p
            .primary()
            .filter(|t| t.distance <= threat_range(hostile(t)) + RANGE_SLACK)
            .copied();
        if let Some(t) = threat {
            self.add_rage(ANGER_RATE * rage_rate(&t, p.difficulty) * p.dt);
            if self.rage >= 1. {
                self.attack(t.who, out);
            }
        } else {
            self.add_rage(-RELAX_RATE * p.dt);
            if self.rage <= 0. {
                self.phase = Phase::Calm;
                self.despair_clock = 0.;
                out.push(Effect::StopMusic);
            }
        }
    }
    fn attacking(
        &mut self,
        victim: Who,
        p: &Perception,
        injured: Option<Who>,
        out: &mut Vec<Effect>,
    ) {
        // WitchAttack update (FUN_1041eab0) and victim change on injury
        // (FUN_1041ef90, z_witch_allow_change_victim 1 after 5 seconds).
        let victim = match injured {
            Some(who) if who != victim && self.victim_since > INJURED_RESUME => {
                self.victim_since = 0.;
                self.swing = None;
                who
            }
            _ => victim,
        };
        self.phase = Phase::Attack { victim };
        let Some(target) = p.get(victim).copied() else {
            // "Lost our victim" / "Retreating in horror from killing a Survivor".
            if self.burning {
                if let Some(next) = p.primary() {
                    self.phase = Phase::Attack { victim: next.who };
                    return;
                }
            }
            self.retreat(out);
            return;
        };
        self.rage = 1.;
        if self.voice_clock <= 0. && !self.burning {
            out.push(Effect::Sound("WitchZombie.Rage"));
            self.voice_clock = self.rng.random_float(1., 2.);
        }
        if target.incap && target.distance <= FRENZY_RANGE {
            out.push(Effect::Sound("WitchZombie.RageBeforeKillingFrenzy"));
            self.swing = None;
            self.phase = Phase::Frenzy {
                victim,
                clock: 0.,
                hits: 0,
            };
            return;
        }
        if let Some((clock, hit_at, done)) = self.swing {
            let clock = clock + p.dt;
            let mut done = done;
            if !done && clock >= hit_at {
                done = true;
                if target.distance <= ATTACK_RANGE && target.visible && !target.incap {
                    out.push(Effect::Strike(victim));
                    out.push(Effect::Sound("WitchZombie.ShredVictim"));
                } else {
                    out.push(Effect::Sound("Zombie.AttackMiss"));
                }
            }
            self.swing = (clock < hit_at + 0.3).then_some((clock, hit_at, done));
        } else if target.distance < ATTACK_RANGE * 1.5 && target.visible && !target.incap {
            let hit_at = SWING_HITS[self.swing_index % SWING_HITS.len()];
            self.swing_index += 1;
            self.swing = Some((0., hit_at, false));
            out.push(Effect::Swing);
        }
    }
    fn frenzy(
        &mut self,
        victim: Who,
        clock: f32,
        hits: u32,
        p: &Perception,
        out: &mut Vec<Effect>,
    ) {
        // WitchKillingFrenzy (FUN_10420fa0 / FUN_10420c60).
        let Some(target) = p.get(victim) else {
            self.retreat(out);
            return;
        };
        if target.distance > FRENZY_LEASH {
            // "Someone moved our victim".
            self.phase = Phase::Attack { victim };
            return;
        }
        self.rage = 1.;
        if self.voice_clock <= 0. && !self.burning {
            out.push(Effect::Sound("WitchZombie.KillingFrenzy"));
            self.voice_clock = self.rng.random_float(0.5, 1.);
        }
        let clock = clock + p.dt;
        let due = frenzy_hits(clock);
        for _ in hits..due {
            out.push(Effect::Shred(victim));
            out.push(Effect::Sound("WitchZombie.ShredVictim"));
        }
        self.phase = Phase::Frenzy {
            victim,
            clock,
            hits: due.max(hits),
        };
    }
    /// Activity for the shared infected animation path.
    pub fn activity(&self) -> &'static str {
        match self.phase {
            Phase::Calm if self.wander => {
                if self.walking && self.moving {
                    "ACT_TERROR_WITCH_WANDER_WALK"
                } else {
                    "ACT_TERROR_WITCH_WANDER_IDLE"
                }
            }
            Phase::Calm => "ACT_TERROR_WITCH_IDLE",
            // Retail switches the agitated layer at rage .54 (FUN_1041d510).
            Phase::Angry if self.rage >= 0.54 => "ACT_TERROR_WITCH_ANGRY_HIGH",
            Phase::Angry => "ACT_TERROR_WITCH_ANGRY",
            Phase::Acquire { .. } => "ACT_TERROR_WITCH_WANDER_ACQUIRE",
            Phase::Shoved { side, .. } => SHOVED[side.min(3) as usize].0,
            Phase::Attack { .. } if self.swing.is_some() => "ACT_TERROR_ATTACK_CONTINUOUSLY",
            Phase::Attack { .. } if !self.moving => "ACT_TERROR_WITCH_STAND",
            Phase::Attack { .. } if self.burning => "ACT_TERROR_RUN_ON_FIRE_INTENSE",
            Phase::Attack { .. } => "ACT_TERROR_RUN_INTENSE",
            Phase::Frenzy { clock, .. } if clock < KILL_DISPLAY => "ACT_TERROR_WITCH_KILL_DISPLAY",
            Phase::Frenzy { .. } => "ACT_TERROR_WITCH_KILL_LOOP",
            Phase::Retreat { clock, .. } if clock < PRE_RETREAT => {
                "ACT_TERROR_WITCH_IDLE_PRE_RETREAT"
            }
            Phase::Retreat { .. } => "ACT_TERROR_WITCH_RETREAT",
            Phase::Dead => "ACT_TERROR_DIE_FROM_STAND",
        }
    }
    /// Ground speed in Source units/second for the current phase.
    pub fn speed(&self) -> f32 {
        match self.phase {
            Phase::Attack { .. } if self.burning => SPEED_INURED,
            Phase::Attack { .. } => SPEED,
            Phase::Retreat { clock, .. } if clock >= PRE_RETREAT => SPEED,
            Phase::Calm if self.wander && self.walking => WANDER_SPEED,
            _ => 0.,
        }
    }
}
fn growl_band(rage: f32) -> u8 {
    if rage >= 0.6 {
        2
    } else if rage >= 0.3 {
        1
    } else {
        0
    }
}
fn growl_sound(band: u8) -> &'static str {
    match band {
        2 => "WitchZombie.GrowlHigh",
        1 => "WitchZombie.GrowlMedium",
        _ => "WitchZombie.GrowlLow",
    }
}
/// Retail hostility test (FUN_1041af50): flashlight aimed at her within
/// z_witch_flashlight_range with line of sight, recently fired a weapon, or
/// within z_witch_personal_space.
pub fn hostile(s: &Seen) -> bool {
    s.flashlight && s.looking && s.visible && s.distance < FLASHLIGHT_RANGE
        || s.fired
        || s.distance < PERSONAL_SPACE
}
pub fn threat_range(hostile: bool) -> f32 {
    if hostile {
        THREAT_HOSTILE_RANGE
    } else {
        THREAT_NORMAL_RANGE
    }
}
/// Rage multiplier for a threat (FUN_1041dd70): hostile threats count
/// z_witch_hostile_at_me_anger (halved unless they look at her), anger falls
/// off linearly between the normal and hostile ranges, Easy halves it.
pub fn rage_rate(s: &Seen, difficulty: Difficulty) -> f32 {
    let mut rate = 1.;
    if hostile(s) {
        rate = if s.looking {
            HOSTILE_AT_ME_ANGER
        } else {
            HOSTILE_AT_ME_ANGER * 0.5
        };
    }
    if s.distance > THREAT_NORMAL_RANGE {
        rate *=
            1. - (s.distance - THREAT_NORMAL_RANGE) / (THREAT_HOSTILE_RANGE - THREAT_NORMAL_RANGE);
    }
    if difficulty == Difficulty::Easy {
        rate *= 0.5;
    }
    rate.max(0.)
}
/// Kill-loop hit damage (FUN_10420c60): Easy x0.5, Advanced x2.
pub fn frenzy_damage(difficulty: Difficulty) -> f32 {
    match difficulty {
        Difficulty::Easy => DAMAGE_PER_KILL_HIT * 0.5,
        Difficulty::Normal => DAMAGE_PER_KILL_HIT,
        Difficulty::Advanced | Difficulty::Expert => DAMAGE_PER_KILL_HIT * 2.,
    }
}
/// Number of AE_ATTACK_HIT events reached `clock` seconds into the frenzy.
pub fn frenzy_hits(clock: f32) -> u32 {
    if clock < KILL_DISPLAY {
        return 0;
    }
    let t = clock - KILL_DISPLAY;
    let loops = (t / KILL_LOOP).floor();
    let cycle = (t - loops * KILL_LOOP) / KILL_LOOP;
    loops as u32 * KILL_HITS.len() as u32 + KILL_HITS.iter().filter(|c| **c <= cycle).count() as u32
}
/// Claw hit through the shared survivor damage path. Easy..Advanced incap a
/// standing survivor (100 damage); Expert never incaps (FUN_10332c20).
pub fn strike(state: &mut Match, difficulty: Difficulty) {
    if state.incap {
        state.damage(frenzy_damage(difficulty));
        return;
    }
    state.damage(CLAW_DAMAGE);
    if difficulty == Difficulty::Expert && state.incap {
        state.damage(state.incap_health.max(1.));
    }
}
/// Shotgun pellet bonus against the witch (infected TraceAttack,
/// z_shotgun_bonus_damage_*): 1 + 4 * (1 - d/100)^2 inside 100 units.
pub fn shotgun_bonus(distance: f32) -> f32 {
    if distance > SHOTGUN_BONUS_RANGE {
        return 1.;
    }
    let f = 1. - distance / SHOTGUN_BONUS_RANGE;
    1. + (SHOTGUN_BONUS_MULTIPLIER - 1.) * f * f
}
/// Event.WanderingWitch repeat interval from the squared distance to the
/// nearest survivor (z_witch_wander_music_*).
pub fn wander_music_interval(distance_squared: f32) -> f32 {
    WANDER_MUSIC_MIN_INTERVAL
        + (distance_squared - WANDER_MUSIC_MIN_DIST)
            * (WANDER_MUSIC_MAX_INTERVAL - WANDER_MUSIC_MIN_INTERVAL)
            / (WANDER_MUSIC_MAX_DIST - WANDER_MUSIC_MIN_DIST)
}
/// Wandering (daytime) witches: `witch_force_wander 1`, or a worldspawn
/// `timeofday` of morning/afternoon. The retail daytime rule is not yet
/// located in server.dll; see NOTES.md.
pub fn wanders(force: bool, time_of_day: Option<i32>) -> bool {
    force || matches!(time_of_day, Some(2 | 3))
}

/// Animation activity consumed by `l4d2r_animation::update`.
#[derive(Component)]
pub(crate) struct WitchActivity(pub &'static str);
/// Witch entities survivor bots must not shoot (not startled yet).
#[derive(Resource, Default)]
pub(crate) struct Calm(pub(crate) std::collections::HashSet<Entity>);
/// Console requests (z_spawn witch / witch_force_wander).
#[derive(Resource, Default)]
pub struct Requests {
    spawn: u32,
    /// Director threat spawns: feet position and Source yaw (degrees).
    pub at: Vec<(Vec3, f32)>,
    pub force_wander: bool,
    serial: usize,
    /// Map info_zombie_spawn witches: entity, feet, Source yaw, bride.
    map: Vec<(usize, Vec3, f32, bool)>,
}
/// Handles witch console commands; `None` leaves the command to others.
pub(crate) fn command(text: &str, requests: &mut Requests) -> Option<String> {
    let words = text.split_whitespace().collect::<Vec<_>>();
    match words.first()?.to_ascii_lowercase().as_str() {
        "z_spawn"
            if words
                .get(1)
                .is_some_and(|w| w.eq_ignore_ascii_case("witch")) =>
        {
            requests.spawn += 1;
            Some("Spawning witch".into())
        }
        "witch_force_wander" => {
            if let Some(v) = words.get(1).and_then(|v| v.parse::<i32>().ok()) {
                requests.force_wander = v != 0;
            }
            Some(format!(
                "\"witch_force_wander\" = \"{}\"",
                requests.force_wander as i32
            ))
        }
        _ => None,
    }
}
pub fn install(app: &mut App) {
    // `--capture-witch` spawns one witch at start for captures/playtests.
    let capture = std::env::args().any(|a| a == "--capture-witch") as u32;
    app.insert_resource(Requests {
        spawn: capture,
        ..default()
    })
    .init_resource::<Calm>()
    .add_systems(
        FixedUpdate,
        (map_spawns, spawn, tick)
            .chain()
            .after(super::tick)
            .run_if(crate::game_loading::visible),
    );
}
/// info_zombie_spawn SpawnZombie (population witch / witch_bride) and the
/// spawner's OnStartled output once its witch is startled.
fn map_spawns(
    mut requests: ResMut<Requests>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    collision: Res<Collision>,
    mut witches: Query<&mut Witch>,
) {
    let Some(mut logic) = logic else { return };
    for id in std::mem::take(&mut logic.world.cine.zombie_spawns) {
        let Some(e) = logic.world.ents.get(id) else { continue };
        let population = e.me.kv("population").unwrap_or("").to_ascii_lowercase();
        let bride = population == "witch_bride";
        if !(bride || population == "witch") {
            eprintln!("info_zombie_spawn {}: population {population:?} is not supported", e.me.targetname);
            continue;
        }
        let o = e.me.origin;
        let feet = Vec3::new(o.x, o.z, -o.y) * UNIT;
        let feet = collision.for_common().supported_spawn_near(feet, HEIGHT).unwrap_or(feet);
        requests.map.push((id, feet, e.me.angles.y, bride));
    }
    for mut witch in &mut witches {
        let startled = matches!(witch.phase, Phase::Acquire { .. } | Phase::Shoved { .. } | Phase::Attack { .. } | Phase::Frenzy { .. });
        if let (Some(id), true, false) = (witch.map_spawn, startled, witch.startled_fired) {
            witch.startled_fired = true;
            logic.world.fire_output(id, "OnStartled", None);
        }
    }
}
/// Point on supported ground under the crosshair (retail z_spawn places
/// under the cursor), or in front of the player.
pub(crate) fn placement(c: &Collision, body: &Body) -> Option<Vec3> {
    let eye = body.feet + Vec3::Y * crate::collision::EYE;
    let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    let flat = (forward * Vec3::new(1., 0., 1.)).normalize_or(Vec3::NEG_Z);
    let hit = c.trace(eye, eye + forward * (2000. * UNIT), Vec3::ZERO);
    let aimed = (hit.fraction < 1.).then(|| eye + forward * (2000. * UNIT) * hit.fraction);
    let mut candidates = vec![];
    if let Some(p) = aimed {
        candidates.push(p - flat * (16. * UNIT));
    }
    candidates.push(body.feet + flat * (120. * UNIT));
    candidates.push(body.feet + flat * (60. * UNIT));
    candidates.into_iter().find_map(|p| {
        let p = c.supported_spawn_near(p, HEIGHT).ok()?;
        (p.distance(body.feet) > 40. * UNIT && c.grounded(p, HEIGHT)).then_some(p)
    })
}
fn spawn(
    mut commands: Commands,
    mut requests: ResMut<Requests>,
    collision: Res<Collision>,
    body: Single<&Body>,
    models: Res<crate::viewer::RenderModels>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
) {
    let mapped = requests.map.pop();
    let placed = if mapped.is_some() { None } else { requests.at.pop() };
    if mapped.is_none() && placed.is_none() && requests.spawn == 0 {
        return;
    }
    let c = collision.for_common();
    let (bride, map_spawn) = mapped.map_or((false, None), |m| (m.3, Some(m.0)));
    let (position, facing) = match mapped.map(|m| (m.1, m.2)).or(placed) {
        Some((position, yaw)) => (position, Some(yaw)),
        None => {
            requests.spawn -= 1;
            let Some(position) = placement(&c, &body) else {
                eprintln!("z_spawn witch: no supported ground in front of the player");
                return;
            };
            (position, None)
        }
    };
    let model = if bride && models.0.contains_key(&(BRIDE_MODEL.to_owned(), 0, 0)) { BRIDE_MODEL } else { MODEL };
    let Some(draws) = models.0.get(&(model.to_owned(), 0, 0)) else {
        eprintln!("z_spawn witch: {model} is not loaded");
        return;
    };
    let time_of_day = logic.as_ref().and_then(|l| {
        l.world
            .ents
            .iter()
            .find(|e| e.me.classname == "worldspawn")
            .and_then(|e| e.me.kv_i32("timeofday"))
    });
    let wander = wanders(requests.force_wander, time_of_day);
    requests.serial += 1;
    // Console: face the player (the model's forward axis is +X). Director
    // threat: the random retail yaw.
    let toward = (body.feet - position) * Vec3::new(1., 0., 1.);
    let rotation = match facing {
        Some(yaw) => Quat::from_rotation_y(yaw.to_radians()),
        None => Quat::from_rotation_y((-toward.z).atan2(toward.x)),
    };
    let mut witch = Witch::new(wander, requests.serial);
    witch.bride = bride;
    witch.map_spawn = map_spawn;
    if map_spawn.is_some() {
        println!("Map witch spawned{} at {position:?}", if bride { " (bride)" } else { "" });
    }
    let activity = WitchActivity(witch.activity());
    let actor = commands
        .spawn((
            Transform::from_translation(position).with_rotation(rotation),
            Visibility::default(),
            Enemy {
                model: model.into(),
                death_loot: None,
                loot_dropped: false,
                reaction_serial: 0,
                attack_serial: 0,
                health: HEALTH,
                attack: 0.,
                vertical: 0.,
                reaction: 0,
                corpse_time: 0.,
                death: Default::default(),
                movement_speed: 0.,
                burning: 0.,
            },
            witch,
            activity,
        ))
        .id();
    for (mesh, material) in draws {
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            crate::viewer::DynamicModelLighting::default(),
            Transform::default(),
            bevy::camera::visibility::RenderLayers::layer(1),
            ChildOf(actor),
        ));
    }
}
/// Per-survivor weapon-fire memory for the hostility/noise tests.
#[derive(Default)]
pub(crate) struct Shots {
    serials: Vec<u64>,
    since: Vec<f32>,
    fired_now: Vec<bool>,
}
impl Shots {
    fn update(&mut self, index: usize, serial: u64, dt: f32) {
        while self.serials.len() <= index {
            self.serials.push(serial);
            self.since.push(f32::INFINITY);
            self.fired_now.push(false);
        }
        self.fired_now[index] = self.serials[index] != serial;
        self.since[index] = if self.fired_now[index] {
            0.
        } else {
            self.since[index] + dt
        };
        self.serials[index] = serial;
    }
}
fn sound(queue: &mut crate::game_audio::Queue, name: &str, at: Option<Vec3>, id: usize) {
    if queue.0.len() < 64 {
        queue.0.push_back(crate::game_audio::Request {
            name: name.into(),
            position: at,
            looping: at.is_none(),
            gain: 1.,
            map_entity: Some(id), soundlevel: None,
        });
    }
}
fn stop(
    queue: &mut crate::game_audio::Queue,
    playing: &Query<(Entity, &crate::game_audio::MapSound)>,
    commands: &mut Commands,
    id: usize,
) {
    queue.1.push(id);
    for (entity, sound) in playing {
        if sound.0 == id {
            commands.entity(entity).despawn();
        }
    }
}
/// Ends the current cue under `id` before a new one starts. Unlike `stop`
/// it does not cancel the id in the audio queue: a cancel also drops a
/// request queued in the same frame, so the new cue would never play.
fn replace(
    queue: &mut crate::game_audio::Queue,
    playing: &Query<(Entity, &crate::game_audio::MapSound)>,
    commands: &mut Commands,
    id: usize,
) {
    queue.0.retain(|r| r.map_entity != Some(id));
    for (entity, sound) in playing {
        if sound.0 == id {
            commands.entity(entity).despawn();
        }
    }
}
pub(crate) fn tick(
    time: Res<Time<Fixed>>,
    collision: Res<Collision>,
    rules: Res<Rules>,
    mut state: ResMut<Match>,
    body: Single<&Body>,
    mut squad: ResMut<Squad>,
    mut queue: ResMut<crate::game_audio::Queue>,
    mut calm: ResMut<Calm>,
    mut witches: Query<(
        Entity,
        &mut Transform,
        &mut Enemy,
        &mut Witch,
        &mut WitchActivity,
    )>,
    world: (
        Commands,
        Query<(Entity, &crate::game_audio::MapSound)>,
        Option<Res<crate::game_console::Console>>,
        Option<Res<crate::campaign_settings::SessionSettings>>,
        Option<ResMut<crate::l4d2r_navigation::Navigation>>,
        Option<Res<crate::l4d2r_cinematic::MapLogic>>,
        Option<Res<crate::projectiles::Projectiles>>,
    ),
    mut shots: Local<Shots>,
) {
    let (mut commands, playing, console, session, mut navigation, logic, projectiles) = world;
    calm.0.clear();
    if state.paused || logic.as_ref().is_some_and(|l| l.active) {
        for (entity, _, _, witch, _) in &witches {
            if witch.calm() {
                calm.0.insert(entity);
            }
        }
        return;
    }
    let head_shots = HEAD_SHOTS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default();
    let dt = time.delta_secs();
    let difficulty = session
        .as_ref()
        .map_or(Difficulty::Normal, |s| s.difficulty);
    let c = collision.for_common();
    // Index 0 is the local player, bots follow.
    shots.update(0, state.shot_serial.wrapping_add(state.shove_serial), dt);
    for (i, member) in squad.0.iter().enumerate() {
        shots.update(i + 1, member.state.shot_serial, dt);
    }
    let flashlight = console.as_ref().is_some_and(|c| c.flashlight);
    let player_eye = body.feet + Vec3::Y * body.stance.eye();
    let player_forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    for (entity, mut transform, mut enemy, mut witch, mut activity) in &mut witches {
        let feet = transform.translation;
        if enemy.health <= 0. {
            if witch.phase != Phase::Dead {
                witch.phase = Phase::Dead;
                queue.play("WitchZombie.Die", Some(feet));
                stop(&mut queue, &playing, &mut commands, SOUND_ID + witch.id * 2);
                stop(
                    &mut queue,
                    &playing,
                    &mut commands,
                    SOUND_ID + witch.id * 2 + 1,
                );
                if enemy.corpse_time <= 0. {
                    enemy.corpse_time = 12.;
                }
            }
            // Corpse settles and is removed like a common infected corpse.
            let start = feet + Vec3::Y * 0.1;
            enemy.vertical -= 800. * UNIT * dt;
            let travel = Vec3::Y * enemy.vertical * dt;
            let hit = c.physics_trace(start, start + travel, Vec3::splat(0.09));
            if !hit.start_solid {
                transform.translation += travel * hit.fraction;
            }
            if hit.fraction < 1. && hit.normal.y > 0.6 {
                enemy.vertical = 0.;
            }
            enemy.corpse_time -= dt;
            if enemy.corpse_time <= 0. {
                commands.entity(entity).despawn();
            }
            continue;
        }
        let head = feet
            + Vec3::Y
                * if witch.wander || !witch.calm() {
                    1.5
                } else {
                    0.9
                };
        // Perception of every living survivor.
        let mut survivors = Vec::new();
        let mut push =
            |who: Who, index: usize, at: Vec3, eye: Vec3, aim: Vec3, light: bool, incap: bool| {
                let distance = feet.distance(at) / UNIT;
                if distance > 3000. {
                    return;
                }
                let visible = c.trace(head, eye, Vec3::ZERO).fraction >= 0.99;
                survivors.push(Seen {
                    who,
                    feet: at,
                    distance,
                    visible,
                    looking: aim
                        .normalize_or_zero()
                        .dot((head - eye).normalize_or_zero())
                        >= LOOKING_COS,
                    flashlight: light,
                    fired: shots.since.get(index).is_some_and(|s| *s < SHOT_MEMORY),
                    fired_now: shots.fired_now.get(index).copied().unwrap_or(false),
                    incap,
                });
            };
        if !state.infected && !state.dead {
            push(
                Who::Player,
                0,
                body.feet,
                player_eye,
                player_forward,
                flashlight,
                state.incap,
            );
        }
        for (i, member) in squad.0.iter().enumerate() {
            if !member.state.dead {
                let eye = member.position
                    + Vec3::Y
                        * if member.state.incap {
                            0.5
                        } else {
                            crate::collision::EYE
                        };
                push(
                    Who::Bot(i),
                    i + 1,
                    member.position,
                    eye,
                    member.aim,
                    false,
                    member.state.incap,
                );
            }
        }
        // Damage since the last tick startles her; attribute it to the
        // survivor who just attacked while aiming at her.
        let mut injured_by = None;
        if enemy.health < witch.last_health - 0.01 {
            let drop = witch.last_health - enemy.health;
            let attacker = survivors
                .iter()
                .filter(|s| s.fired_now)
                .max_by(|a, b| {
                    let aim = |s: &Seen| match s.who {
                        Who::Player => player_forward.dot((head - player_eye).normalize_or_zero()),
                        Who::Bot(i) => squad.0[i]
                            .aim
                            .dot((head - s.feet - Vec3::Y).normalize_or_zero()),
                    };
                    aim(a).total_cmp(&aim(b))
                })
                .or_else(|| {
                    survivors
                        .iter()
                        .min_by(|a, b| a.distance.total_cmp(&b.distance))
                })
                .copied();
            if let Some(attacker) = attacker {
                injured_by = Some(attacker.who);
                let weapon = |s: &Match| {
                    rules
                        .weapons
                        .get(&s.gun().class)
                        .unwrap_or(&rules.pistol)
                        .pellets
                        > 1
                };
                let shotgun = attacker.fired_now
                    && match attacker.who {
                        Who::Player => weapon(&state),
                        Who::Bot(i) => weapon(&squad.0[i].state),
                    };
                if shotgun {
                    let bonus = drop * (shotgun_bonus(attacker.distance) - 1.);
                    if bonus > 0. && enemy.health > 0. {
                        enemy.health -= bonus;
                        if enemy.health <= 0. {
                            let incoming = (feet - attacker.feet).normalize_or_zero();
                            enemy.record_death(
                                crate::infected_death::Cause::Shotgun,
                                &transform,
                                incoming,
                                1.5,
                            );
                            enemy.corpse_time = 12.;
                            match attacker.who {
                                Who::Player => state.kills += 1,
                                Who::Bot(i) => squad.0[i].state.kills += 1,
                            }
                        }
                    }
                }
            }
            if witch.voice_clock <= 0. && witch.calm() {
                queue.play("WitchZombie.Pain", Some(feet));
            }
        }
        witch.last_health = enemy.health;
        if enemy.health <= 0. {
            continue;
        }
        // Only the local player throws bile jars.
        let biled = projectiles.as_ref().is_some_and(|d| d.biled.iter().any(|b| b.0 == entity));
        let vomited_by = (biled && !witch.biled).then_some(Who::Player);
        let rifle_headshot_by = head_shots
            .iter()
            .any(|(e, damage)| *e == entity && *damage > RIFLE_HEADSHOT_DAMAGE)
            .then_some(Who::Player);
        witch.biled = biled;
        // Which side of her (model +X forward) the shooter stands on.
        let shot_side = rifle_headshot_by
            .and_then(|who| survivors.iter().find(|s| s.who == who))
            .map_or(0, |s| {
                let local = transform.rotation.inverse() * ((s.feet - feet) * Vec3::new(1., 0., 1.));
                if local.x.abs() >= local.z.abs() {
                    if local.x >= 0. { 0 } else { 1 }
                } else if local.z < 0. {
                    2
                } else {
                    3
                }
            });
        let perception = Perception {
            shot_side,
            dt,
            survivors,
            injured_by,
            vomited_by,
            rifle_headshot_by,
            burning: enemy.burning > 0.,
            moving: enemy.movement_speed > 0.08,
            difficulty,
        };
        let voice = SOUND_ID + witch.id * 2;
        let music = voice + 1;
        for effect in witch.think(&perception) {
            match effect {
                Effect::Sound(name) => queue.play(name, Some(feet)),
                Effect::Voice(name) => sound(&mut queue, name, Some(feet), voice),
                Effect::Music(name) => {
                    // Music::Update plays the bride's own rage cue.
                    let name = if witch.bride && name == "Event.WitchRage" { "Event.WitchRageBride" } else { name };
                    replace(&mut queue, &playing, &mut commands, music);
                    sound(&mut queue, name, None, music);
                }
                Effect::StopMusic => stop(&mut queue, &playing, &mut commands, music),
                Effect::StopVoice => stop(&mut queue, &playing, &mut commands, voice),
                Effect::Strike(who) => match who {
                    Who::Player => strike(&mut state, difficulty),
                    Who::Bot(i) => strike(&mut squad.0[i].state, difficulty),
                },
                Effect::Shred(who) => match who {
                    Who::Player => state.damage(frenzy_damage(difficulty)),
                    Who::Bot(i) => {
                        squad.0[i].state.damage(frenzy_damage(difficulty));
                        squad.0[i].revive_time = 0.;
                    }
                },
                Effect::Swing => enemy.attack_serial = enemy.attack_serial.wrapping_add(1),
                Effect::Kill => {
                    enemy.health = 0.;
                    enemy.record_death(
                        crate::infected_death::Cause::Fire,
                        &transform,
                        Vec3::NEG_Y,
                        1.,
                    );
                    enemy.corpse_time = 12.;
                }
                Effect::Despawn => {
                    stop(&mut queue, &playing, &mut commands, voice);
                    stop(&mut queue, &playing, &mut commands, music);
                    commands.entity(entity).despawn();
                }
            }
        }
        // Shared hit reactions belong to commons; the witch keeps her own pose.
        enemy.reaction = 0;
        enemy.attack = 0.;
        enemy.burning = (enemy.burning - dt).max(0.);
        // Locomotion.
        let victim_feet = witch.victim().and_then(|who| match who {
            Who::Player => Some(body.feet),
            Who::Bot(i) => squad.0.get(i).map(|m| m.position),
        });
        let goal = match witch.phase {
            Phase::Attack { .. } => victim_feet.filter(|p| p.distance(feet) > 25. * UNIT),
            Phase::Retreat { .. } => {
                let threat = perception.survivors.iter().fold(Vec3::ZERO, |sum, s| {
                    sum + (feet - s.feet).normalize_or_zero()
                });
                let away =
                    (threat * Vec3::new(1., 0., 1.)).normalize_or(transform.rotation * Vec3::X);
                Some(feet + away * (MIN_RETREAT_RANGE * UNIT))
            }
            Phase::Calm if witch.wander => {
                if witch.wander_goal.is_none_or(|g| g.distance(feet) < 0.5) {
                    let mut rng =
                        UniformRandomStream::new(witch.id as i32 ^ (state.elapsed * 10.) as i32);
                    let angle = rng.random_float(0., std::f32::consts::TAU);
                    witch.wander_goal = Some(feet + Vec3::new(angle.cos(), 0., angle.sin()) * 6.);
                }
                witch.wander_goal
            }
            _ => None,
        };
        let speed = witch.speed() * UNIT;
        let grounded = c.grounded(feet, HEIGHT);
        enemy.vertical = if grounded {
            0.
        } else {
            enemy.vertical - 800. * UNIT * dt
        };
        let fall = Vec3::Y * enemy.vertical * dt;
        let mut wish = Vec3::ZERO;
        if let (Some(goal), true) = (goal, speed > 0.) {
            let toward = navigation
                .as_mut()
                .map(|nav| nav.toward(0x4000_0000 + witch.id, feet, goal, dt))
                .filter(|t| t.length_squared() > 1e-6)
                .unwrap_or(goal - feet);
            wish = c.steer(feet, toward, HEIGHT);
            if wish == Vec3::ZERO {
                wish = (toward * Vec3::new(1., 0., 1.)).normalize_or_zero();
            }
        }
        transform.translation = c.walk(feet, wish * speed * dt + fall, HEIGHT, grounded);
        let moved = (transform.translation - feet) * Vec3::new(1., 0., 1.);
        enemy.movement_speed = moved.length() / dt.max(0.001);
        let face = match witch.phase {
            Phase::Frenzy { .. } | Phase::Attack { .. } if moved.length() < 0.01 => {
                victim_feet.map(|p| p - feet)
            }
            _ => (moved.length_squared() > 1e-6).then_some(moved),
        };
        if let Some(face) = face
            .map(|f| f * Vec3::new(1., 0., 1.))
            .filter(|f| f.length_squared() > 1e-6)
        {
            transform.rotation = Quat::from_rotation_y((-face.z).atan2(face.x));
        }
        if activity.0 != witch.activity() {
            activity.0 = witch.activity();
        }
        if witch.calm() {
            calm.0.insert(entity);
            // Survivor bots give a calm witch room (retail bots avoid her).
            for member in squad
                .0
                .iter_mut()
                .filter(|_| !crate::game_state::bots_stopped())
            {
                let offset = (member.position - feet) * Vec3::new(1., 0., 1.);
                let room = (PERSONAL_SPACE + RANGE_SLACK) * UNIT;
                if !member.state.dead && !member.state.incap && !member.state.pinned && offset.length() < room {
                    let out = offset.normalize_or(Vec3::X) * (room - offset.length()).min(4. * dt);
                    member.position = c.walk(member.position, out, HEIGHT, true);
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn seen(who: Who, distance: f32) -> Seen {
        Seen {
            who,
            feet: Vec3::X * distance * UNIT,
            distance,
            visible: true,
            looking: false,
            flashlight: false,
            fired: false,
            fired_now: false,
            incap: false,
        }
    }
    fn perceive(survivors: Vec<Seen>) -> Perception {
        Perception {
            shot_side: 0,
            dt: 1. / 30.,
            survivors,
            injured_by: None,
            vomited_by: None,
            rifle_headshot_by: None,
            burning: false,
            moving: false,
            difficulty: Difficulty::Normal,
        }
    }
    fn run(witch: &mut Witch, p: &Perception, seconds: f32) -> Vec<Effect> {
        let mut all = vec![];
        for _ in 0..(seconds * 30.).round() as usize {
            all.extend(witch.think(p));
        }
        all
    }
    #[test]
    fn rage_rate_follows_retail_hostility_range_falloff_and_easy_scale() {
        let calm = seen(Who::Player, 200.);
        assert_eq!(rage_rate(&calm, Difficulty::Normal), 1.);
        assert_eq!(rage_rate(&calm, Difficulty::Easy), 0.5);
        let mut lit = calm;
        lit.flashlight = true;
        lit.looking = true;
        assert!(hostile(&lit));
        assert_eq!(rage_rate(&lit, Difficulty::Normal), 2.);
        lit.distance = 450.;
        assert!(!hostile(&lit), "flashlight range is 400");
        let mut shooter = seen(Who::Bot(0), 450.);
        shooter.fired = true;
        // Hostile but not looking: 2 * .5, then half way to the hostile range.
        assert!((rage_rate(&shooter, Difficulty::Normal) - 0.5).abs() < 1e-5);
        assert!(hostile(&seen(Who::Player, 99.)));
        assert_eq!(threat_range(false), 300.);
        assert_eq!(threat_range(true), 600.);
    }
    #[test]
    fn sitting_witch_rages_in_five_seconds_at_normal_range_and_relaxes_when_left() {
        let mut witch = Witch::new(false, 1);
        let p = perceive(vec![seen(Who::Player, 200.)]);
        let effects = run(&mut witch, &p, 0.1);
        assert_eq!(witch.phase, Phase::Angry);
        assert!(effects.contains(&Effect::Music("Event.WitchRage")));
        // 0.2/s anger rate: about five seconds of exposure to a full rage.
        run(&mut witch, &p, 4.7);
        assert_eq!(witch.phase, Phase::Angry);
        assert!(witch.rage > 0.9);
        let effects = run(&mut witch, &p, 0.4);
        assert_eq!(
            witch.phase,
            Phase::Attack {
                victim: Who::Player
            }
        );
        assert!(effects.contains(&Effect::Music("Event.WitchAttack")));
        // A second witch: survivors back off and the rage relaxes at 0.05/s.
        let mut witch = Witch::new(false, 2);
        run(&mut witch, &p, 2.);
        let rage = witch.rage;
        let away = perceive(vec![seen(Who::Player, 900.)]);
        run(&mut witch, &away, 1.);
        assert!((rage - witch.rage - 0.05).abs() < 0.01);
        run(&mut witch, &away, rage / 0.05 + 0.5);
        assert_eq!(witch.phase, Phase::Calm);
        assert!(witch.calm());
    }
    #[test]
    fn growls_escalate_with_rage_bands() {
        assert_eq!(growl_sound(growl_band(0.1)), "WitchZombie.GrowlLow");
        assert_eq!(growl_sound(growl_band(0.3)), "WitchZombie.GrowlMedium");
        assert_eq!(growl_sound(growl_band(0.6)), "WitchZombie.GrowlHigh");
        let mut witch = Witch::new(false, 3);
        let mut lit = seen(Who::Player, 150.);
        lit.flashlight = true;
        lit.looking = true;
        let effects = run(&mut witch, &perceive(vec![lit]), 2.4);
        for growl in [
            "WitchZombie.GrowlLow",
            "WitchZombie.GrowlMedium",
            "WitchZombie.GrowlHigh",
        ] {
            assert!(effects.contains(&Effect::Sound(growl)), "{growl}");
        }
        assert_eq!(witch.activity(), "ACT_TERROR_WITCH_ANGRY_HIGH");
    }
    #[test]
    fn damage_startles_instantly_and_far_survivors_are_ignored() {
        let mut witch = Witch::new(false, 4);
        let far = perceive(vec![seen(Who::Bot(1), 800.)]);
        let effects = run(&mut witch, &far, 3.);
        assert_eq!(witch.phase, Phase::Calm);
        assert!(effects.contains(&Effect::Voice("WitchZombie.Despair")));
        let mut shot = far.clone();
        shot.injured_by = Some(Who::Bot(1));
        witch.think(&shot);
        assert_eq!(
            witch.phase,
            Phase::Attack {
                victim: Who::Bot(1)
            }
        );
        assert!(!witch.calm());
        assert_eq!(witch.speed(), SPEED);
    }
    #[test]
    fn bile_jar_sends_a_calm_or_wandering_witch_at_the_thrower() {
        for wander in [false, true] {
            let mut witch = Witch::new(wander, 6);
            let mut p = perceive(vec![seen(Who::Player, 900.)]);
            p.vomited_by = Some(Who::Player);
            let effects = witch.think(&p);
            assert_eq!(witch.phase, Phase::Attack { victim: Who::Player });
            assert_eq!(witch.rage, 1.);
            assert!(effects.contains(&Effect::Music("Event.WitchAttack")));
        }
    }
    #[test]
    fn rifle_headshot_cries_out_staggers_then_attacks_the_shooter() {
        let mut witch = Witch::new(false, 7);
        let mut p = perceive(vec![seen(Who::Player, 900.)]);
        p.rifle_headshot_by = Some(Who::Player);
        p.shot_side = 2;
        let effects = witch.think(&p);
        assert!(effects.contains(&Effect::Sound("WitchZombie.HeadShot")));
        assert_eq!(witch.activity(), "ACT_TERROR_SHOVED_RIGHTWARD");
        assert_eq!(witch.victim(), Some(Who::Player));
        // The stagger plays to the end of Shoved_Rightward_01, then WitchAttack.
        let p = perceive(vec![seen(Who::Player, 900.)]);
        let mut attacked = None;
        for tick in 1..200 {
            let effects = witch.think(&p);
            if effects.contains(&Effect::Music("Event.WitchAttack")) {
                attacked = Some(tick as f32 * p.dt);
                break;
            }
        }
        let at = attacked.expect("attack after the stagger");
        assert!((at - SHOVED[2].1).abs() <= p.dt + 1e-4, "{at}");
        assert_eq!(witch.phase, Phase::Attack { victim: Who::Player });
    }
    #[test]
    fn claw_incaps_through_shared_damage_and_expert_kills() {
        for difficulty in [Difficulty::Easy, Difficulty::Normal, Difficulty::Advanced] {
            let mut state = Match::new(&crate::game_state::tests_weapon());
            state.health = 100.;
            strike(&mut state, difficulty);
            assert!(state.incap && !state.dead, "{difficulty:?}");
        }
        let mut state = Match::new(&crate::game_state::tests_weapon());
        strike(&mut state, Difficulty::Expert);
        assert!(state.dead);
        assert_eq!(frenzy_damage(Difficulty::Easy), 15.);
        assert_eq!(frenzy_damage(Difficulty::Normal), 30.);
        assert_eq!(frenzy_damage(Difficulty::Advanced), 60.);
    }
    #[test]
    fn attack_swings_incap_then_shreds_until_the_victim_dies_and_she_retreats() {
        let mut witch = Witch::new(false, 5);
        let mut victim = seen(Who::Player, 25.);
        let mut state = Match::new(&crate::game_state::tests_weapon());
        witch.attack(Who::Player, &mut vec![]);
        let mut shreds = 0;
        for _ in 0..30 * 40 {
            victim.incap = state.incap;
            let alive = !state.dead;
            let p = perceive(if alive { vec![victim] } else { vec![] });
            for effect in witch.think(&p) {
                match effect {
                    Effect::Strike(Who::Player) => strike(&mut state, Difficulty::Normal),
                    Effect::Shred(Who::Player) => {
                        shreds += 1;
                        state.damage(frenzy_damage(Difficulty::Normal));
                    }
                    _ => {}
                }
            }
            if matches!(witch.phase, Phase::Retreat { .. }) {
                break;
            }
        }
        assert!(state.dead);
        // 300 incap health at 30 per kill-loop hit.
        assert_eq!(shreds, 10);
        assert!(matches!(witch.phase, Phase::Retreat { .. }));
        assert_eq!(witch.activity(), "ACT_TERROR_WITCH_IDLE_PRE_RETREAT");
    }
    #[test]
    fn frenzy_hits_follow_native_kill_loop_events() {
        assert_eq!(frenzy_hits(KILL_DISPLAY - 0.01), 0);
        assert_eq!(frenzy_hits(KILL_DISPLAY + 0.05), 1);
        assert_eq!(frenzy_hits(KILL_DISPLAY + KILL_LOOP * 0.95), 8);
        assert_eq!(frenzy_hits(KILL_DISPLAY + KILL_LOOP * 1.03), 9);
    }
    #[test]
    fn retreating_witch_leaves_once_hidden_or_far() {
        let mut witch = Witch::new(false, 6);
        witch.retreat(&mut vec![]);
        let mut seen_far = seen(Who::Player, 1500.);
        seen_far.visible = false;
        let p = perceive(vec![seen_far]);
        let effects = run(&mut witch, &p, 9.);
        assert!(!effects.contains(&Effect::Despawn));
        assert!(effects.contains(&Effect::Sound("WitchZombie.RetreatHorror")));
        let effects = run(&mut witch, &p, 1.5);
        assert!(effects.contains(&Effect::Despawn));
    }
    #[test]
    fn burning_witch_uses_inured_speed_and_burns_out() {
        let mut witch = Witch::new(false, 7);
        let mut p = perceive(vec![seen(Who::Player, 300.)]);
        p.burning = true;
        let effects = witch.think(&p);
        assert!(effects.contains(&Effect::Sound("Zombie.IgniteScream")));
        assert_eq!(witch.speed(), SPEED_INURED);
        p.burning = false;
        let effects = run(&mut witch, &p, BURN_TIME + 0.1);
        assert!(effects.contains(&Effect::Kill));
    }
    #[test]
    fn wandering_witch_startles_after_personal_time_or_close_gunfire() {
        let mut witch = Witch::new(true, 8);
        let near = perceive(vec![seen(Who::Bot(0), 200.)]);
        run(&mut witch, &near, WANDER_PERSONAL_TIME - 0.2);
        assert_eq!(witch.phase, Phase::Calm);
        let effects = run(&mut witch, &near, 0.3);
        assert!(effects.contains(&Effect::Sound("WitchZombie.Scream")));
        assert!(matches!(
            witch.phase,
            Phase::Acquire {
                victim: Who::Bot(0),
                ..
            }
        ));
        assert_eq!(witch.activity(), "ACT_TERROR_WITCH_WANDER_ACQUIRE");
        run(&mut witch, &near, WANDER_ACQUIRE + 0.05);
        assert_eq!(
            witch.phase,
            Phase::Attack {
                victim: Who::Bot(0)
            }
        );
        let mut witch = Witch::new(true, 9);
        let mut shot = seen(Who::Player, 60.);
        shot.fired_now = true;
        witch.think(&perceive(vec![shot]));
        assert!(matches!(
            witch.phase,
            Phase::Acquire {
                victim: Who::Player,
                ..
            }
        ));
        assert!((wander_music_interval(WANDER_MUSIC_MIN_DIST) - 3.).abs() < 1e-3);
        assert!((wander_music_interval(WANDER_MUSIC_MAX_DIST) - 20.).abs() < 1e-3);
    }
    #[test]
    fn victim_changes_to_a_new_attacker_only_after_five_seconds() {
        let mut witch = Witch::new(false, 10);
        witch.attack(Who::Player, &mut vec![]);
        let mut p = perceive(vec![seen(Who::Player, 500.), seen(Who::Bot(2), 400.)]);
        p.injured_by = Some(Who::Bot(2));
        witch.think(&p);
        assert_eq!(witch.victim(), Some(Who::Player));
        p.injured_by = None;
        run(&mut witch, &p, 5.);
        p.injured_by = Some(Who::Bot(2));
        witch.think(&p);
        assert_eq!(witch.victim(), Some(Who::Bot(2)));
    }
    #[test]
    fn point_blank_shotgun_bonus_crowns_with_a_pump_shotgun() {
        assert_eq!(shotgun_bonus(0.), 5.);
        assert_eq!(shotgun_bonus(100.), 1.);
        assert_eq!(shotgun_bonus(150.), 1.);
        // Pump shotgun: 10 pellets x 25 damage at point blank.
        assert!(250. * shotgun_bonus(10.) >= HEALTH);
        assert!(250. * shotgun_bonus(60.) < HEALTH);
    }
    #[test]
    fn console_spawns_and_wander_rule() {
        let mut requests = Requests::default();
        assert_eq!(
            command("z_spawn witch", &mut requests).as_deref(),
            Some("Spawning witch")
        );
        assert_eq!(requests.spawn, 1);
        assert!(command("z_spawn tank", &mut requests).is_none());
        assert!(command("witch_force_wander 1", &mut requests).is_some());
        assert!(requests.force_wander);
        assert!(wanders(true, None));
        assert!(!wanders(false, Some(0)));
        assert!(wanders(false, Some(3)));
    }
    #[test]
    fn spawn_placement_is_supported_ground_ahead_of_the_player() {
        let c = Collision::test_solid_floor();
        let body = Body::at(Vec3::Y * 0.004);
        let p = placement(&c, &body).expect("supported ground");
        assert!(p.z < -0.9, "{p:?}");
        assert!(c.grounded(p, HEIGHT));
    }
    #[test]
    fn owned_witch_assets_have_activities_and_sounds() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        assert!(!assets.read(MODEL).unwrap().is_empty());
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        for name in [
            "witchzombie.despair",
            "wanderwitchzombie.despair",
            "witchzombie.surprised",
            "witchzombie.growllow",
            "witchzombie.growlmedium",
            "witchzombie.growlhigh",
            "witchzombie.rage",
            "witchzombie.scream",
            "witchzombie.ragebeforekillingfrenzy",
            "witchzombie.killingfrenzy",
            "witchzombie.shredvictim",
            "witchzombie.retreathorror",
            "witchzombie.pain",
            "witchzombie.headshot",
            "witchzombie.painfire",
            "witchzombie.die",
            "zombie.ignitescream",
            "zombie.attackmiss",
            "event.witchattack",
            "event.witchrage",
            "event.witchburning",
            "event.wanderingwitch",
        ] {
            let sound = registry.definitions.get(name).expect(name);
            assert!(!assets.read(&sound.waves[0]).unwrap().is_empty(), "{name}");
        }
        let assets = std::cell::RefCell::new(assets);
        let sk =
            l4d2_formats::anim::Skeleton::load_with(MODEL, |p| assets.borrow_mut().read(p).ok())
                .unwrap();
        let mut witch = Witch::new(false, 0);
        let mut activities = vec![];
        for (phase, moving, swing, burning) in [
            (Phase::Calm, false, false, false),
            (Phase::Angry, false, false, false),
            (
                Phase::Attack {
                    victim: Who::Player,
                },
                true,
                false,
                false,
            ),
            (
                Phase::Attack {
                    victim: Who::Player,
                },
                true,
                false,
                true,
            ),
            (
                Phase::Attack {
                    victim: Who::Player,
                },
                false,
                true,
                false,
            ),
            (
                Phase::Attack {
                    victim: Who::Player,
                },
                false,
                false,
                false,
            ),
            (
                Phase::Frenzy {
                    victim: Who::Player,
                    clock: 0.,
                    hits: 0,
                },
                false,
                false,
                false,
            ),
            (
                Phase::Frenzy {
                    victim: Who::Player,
                    clock: 9.,
                    hits: 0,
                },
                false,
                false,
                false,
            ),
            (
                Phase::Retreat {
                    clock: 0.,
                    hidden: 0.,
                },
                false,
                false,
                false,
            ),
            (
                Phase::Retreat {
                    clock: 9.,
                    hidden: 0.,
                },
                true,
                false,
                false,
            ),
            (
                Phase::Acquire {
                    victim: Who::Player,
                    clock: 0.,
                },
                false,
                false,
                false,
            ),
        ] {
            witch.phase = phase;
            witch.moving = moving;
            witch.burning = burning;
            witch.swing = swing.then_some((0., 1., false));
            activities.push(witch.activity());
        }
        witch.phase = Phase::Angry;
        witch.rage = 0.9;
        activities.push(witch.activity());
        witch.phase = Phase::Calm;
        witch.wander = true;
        witch.walking = true;
        witch.moving = true;
        activities.push(witch.activity());
        witch.moving = false;
        activities.push(witch.activity());
        for activity in activities {
            assert!(
                sk.sequences().iter().any(|s| s.activity_name == activity),
                "{activity}"
            );
        }
        for (label, duration) in [
            ("killing_blow_display", KILL_DISPLAY),
            ("killing_blow_loop", KILL_LOOP),
            ("Idle_Pre_Retreat", PRE_RETREAT),
            ("Wander_acquire", WANDER_ACQUIRE),
        ] {
            let seq = sk.find_sequence(label).expect(label);
            let anim = sk.sequence_anim(seq).expect(label);
            assert!(
                (sk.anims()[anim].duration() - duration).abs() < 0.02,
                "{label}"
            );
        }
    }
}
