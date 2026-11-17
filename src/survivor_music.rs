//! Survivor music events for the local survivor (server Music::On*):
//! a "Hit" sting plus a music cue when a special grabs you, the downed
//! cue when incapacitated, and fades when they end.
//! - OnPounced 102ad940: Event.HunterHit + Event.HunterPounce; OnPounceEnded
//!   102ab440 stops Event.HunterPounce.
//! - OnPulledByTongue 102ad430: Event.SmokerDragHit + Event.SmokerDrag;
//!   OnChoked 102ad4b0: Event.SmokerChokeHit + Event.SmokerChoke;
//!   OnReleasedByTongue 102ab3e0 stops both.
//! - OnJockeyRide 102addd0: Event.JockeyHit + Event.JockeyRide; OnJockeyRideEnded
//!   102ab490 stops it.
//! - OnStruck 102ae260 (Charger carry): Event.ChargerHit + Event.ChargerSmash;
//!   OnPummeled 102ae2e0: Event.ChargerSlamHit + Event.ChargerSlam (the smash
//!   cue fades over 1 s, 102ab4e0); OnPummeledEnd 102ab520 stops the slam.
//! - OnDown 102abf80: Event.DownHit + Event.Down (L4D1 survivors use the
//!   `_L4D1` cues); OnDownEnded 102ab110 fades Event.Down over 2 s.
//! - OnBleedingOut 102ac060: Event.BleedingOutHit + Event.BleedingOut (same
//!   "down" track, so it replaces Event.Down). The script comment says it
//!   starts 10 s before death: incap health at or below 10 s of decay.
//!   OnBleedingOutEnded 102ab170 fades it over 1 s.
//! - OnKilled 102ac3a0: Event.BleedingOutEnd if bleeding out, then
//!   Event.SurvivorDeathHit + Event.SurvivorDeath, only while other survivors
//!   are still alive (not on mission loss).
//! - OnVomitedUpon 102acfa0: Event.BoomerHit + Event.VomitInTheFace;
//!   OnITExpired 102ab390 stops it.
//! Not reproduced: mob/area reveal, special alerts and boss cues.
use bevy::prelude::*;

/// Map-sound tags for the cues this module stops.
const PIN_MUSIC: usize = usize::MAX - 0x5a11;
const SMASH_MUSIC: usize = usize::MAX - 0x5a12;
const DOWN_MUSIC: usize = usize::MAX - 0x5a13;
const VOMIT_MUSIC: usize = usize::MAX - 0x5a14;
/// Incap health decays 3/s (survivor_incap_decay_rate); ten seconds left.
const BLEEDING_OUT_HEALTH: f32 = 3. * 10.;
/// Retail stops these with the default fade (-1); one second here.
const PIN_FADE: f32 = 1.;
const DOWN_FADE: f32 = 2.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pin {
    #[default]
    None,
    Pounce,
    Drag,
    Choke,
    Ride,
    Smash,
    Slam,
}
/// The local survivor's grab state from the match flags.
pub fn pin_of(s: &crate::game_state::Match, pummeled: bool) -> Pin {
    use crate::special_infected::SpecialClass;
    if !s.pinned || s.infected {
        return Pin::None;
    }
    match s.pinned_by {
        Some(SpecialClass::Hunter) => Pin::Pounce,
        // Dragged survivors may still shoot; choking ones may not.
        Some(SpecialClass::Smoker) if s.pin_allows_fire => Pin::Drag,
        Some(SpecialClass::Smoker) => Pin::Choke,
        Some(SpecialClass::Jockey) => Pin::Ride,
        Some(SpecialClass::Charger) if pummeled => Pin::Slam,
        Some(SpecialClass::Charger) => Pin::Smash,
        _ => Pin::None,
    }
}
/// The cues to start and the tags to fade when the grab state changes.
pub fn pin_change(old: Pin, new: Pin) -> (Vec<(&'static str, Option<usize>)>, Vec<(usize, f32)>) {
    let mut play = vec![];
    let mut fade = vec![];
    if old == new {
        return (play, fade);
    }
    match (old, new) {
        (Pin::Drag, Pin::Choke) => {}
        (Pin::Smash, Pin::Slam) => fade.push((SMASH_MUSIC, 1.)),
        (Pin::None, _) => {}
        (_, _) => {
            fade.push((PIN_MUSIC, PIN_FADE));
            fade.push((SMASH_MUSIC, PIN_FADE));
        }
    }
    match new {
        Pin::Pounce => play.extend([("Event.HunterHit", None), ("Event.HunterPounce", Some(PIN_MUSIC))]),
        Pin::Drag => play.extend([("Event.SmokerDragHit", None), ("Event.SmokerDrag", Some(PIN_MUSIC))]),
        Pin::Choke => play.extend([("Event.SmokerChokeHit", None), ("Event.SmokerChoke", Some(PIN_MUSIC))]),
        Pin::Ride => play.extend([("Event.JockeyHit", None), ("Event.JockeyRide", Some(PIN_MUSIC))]),
        Pin::Smash => play.extend([("Event.ChargerHit", None), ("Event.ChargerSmash", Some(SMASH_MUSIC))]),
        Pin::Slam => play.extend([("Event.ChargerSlamHit", None), ("Event.ChargerSlam", Some(PIN_MUSIC))]),
        Pin::None => {}
    }
    (play, fade)
}
#[derive(Default)]
pub struct Tracked {
    pin: Pin,
    down: bool,
    bleeding: bool,
    dead: bool,
    vomited: bool,
}
/// Cues for the down / bleeding-out / death sequence (L4D1 survivors use the
/// `_L4D1` variants).
pub fn life_change(
    was: (bool, bool, bool),
    now: (bool, bool, bool),
    others_alive: bool,
    l4d1: bool,
) -> (Vec<(&'static str, Option<usize>)>, Vec<(usize, f32)>) {
    let (was_down, was_bleeding, was_dead) = was;
    let (down, bleeding, dead) = now;
    let v = |a: &'static str, b: &'static str| if l4d1 { b } else { a };
    let mut play = vec![];
    let mut fade = vec![];
    if dead && !was_dead {
        fade.push((DOWN_MUSIC, 0.));
        if others_alive {
            if was_bleeding {
                play.push((v("Event.BleedingOutEnd", "Event.BleedingOutEnd_L4D1"), None));
            }
            play.push((v("Event.SurvivorDeathHit", "Event.SurvivorDeathHit_L4D1"), None));
            play.push((v("Event.SurvivorDeath", "Event.SurvivorDeath_L4D1"), None));
        }
        return (play, fade);
    }
    if bleeding && !was_bleeding {
        fade.push((DOWN_MUSIC, 0.));
        play.push((v("Event.BleedingOutHit", "Event.BleedingOutHit_L4D1"), None));
        play.push((v("Event.BleedingOut", "Event.BleedingOut_L4D1"), Some(DOWN_MUSIC)));
    } else if down && !was_down {
        play.push((v("Event.DownHit", "Event.DownHit_L4D1"), None));
        play.push((v("Event.Down", "Event.Down_L4D1"), Some(DOWN_MUSIC)));
    } else if !down && was_down && !dead {
        // Revived: OnDownEnded fades over 2 s, OnBleedingOutEnded over 1 s.
        fade.push((DOWN_MUSIC, if was_bleeding { 1. } else { DOWN_FADE }));
    }
    (play, fade)
}
pub fn update(
    mut tracked: Local<Tracked>,
    mut commands: Commands,
    s: Res<crate::game_state::Match>,
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    mut queue: ResMut<crate::game_audio::Queue>,
    playing: Query<(Entity, &crate::game_audio::MapSound)>,
    squad: Option<Res<crate::game_state::Squad>>,
) {
    let pummeled = crate::special_infected::ai::player_pummeled();
    let pin = if s.dead { Pin::None } else { pin_of(&s, pummeled) };
    let survivor = !s.infected;
    let down = survivor && s.incap && s.ledge.is_none() && !s.dead;
    let bleeding = down && s.incap_health <= BLEEDING_OUT_HEALTH;
    let dead = survivor && s.dead;
    let vomited = survivor && !s.dead && s.vomited > 0.;
    let l4d1 = session.as_ref().is_some_and(|x| {
        use crate::campaign_settings::Survivor::*;
        matches!(x.survivor, Bill | Zoey | Francis | Louis)
    });
    let others_alive = squad.as_ref().is_some_and(|q| q.0.iter().any(|m| !m.state.dead));
    let mut play: Vec<(&'static str, Option<usize>)> = vec![];
    let mut fade: Vec<(usize, f32)> = vec![];
    let (p, f) = pin_change(tracked.pin, pin);
    play.extend(p);
    fade.extend(f);
    let (p, f) = life_change(
        (tracked.down, tracked.bleeding, tracked.dead),
        (down, bleeding, dead),
        others_alive,
        l4d1,
    );
    play.extend(p);
    fade.extend(f);
    if vomited && !tracked.vomited {
        play.extend([("Event.BoomerHit", None), ("Event.VomitInTheFace", Some(VOMIT_MUSIC))]);
    } else if !vomited && tracked.vomited {
        fade.push((VOMIT_MUSIC, PIN_FADE));
    }
    tracked.pin = pin;
    tracked.down = down;
    tracked.bleeding = bleeding;
    tracked.dead = dead;
    tracked.vomited = vomited;
    for (tag, seconds) in fade {
        for (entity, sound) in &playing {
            if sound.0 == tag {
                commands.entity(entity).insert(crate::game_audio::FadeOut::new(seconds));
            }
        }
    }
    for (name, tag) in play {
        queue.0.push_back(crate::game_audio::Request {
            name: name.into(),
            position: None,
            looping: false,
            gain: 1.,
            map_entity: tag,
            soundlevel: None,
        });
    }
}
/// Special-infected alert stings (Music::Update 102b1710 -> per-class
/// handlers -> Music::OnAlert 102ac980). Every special within
/// music_dynamic_SI_inrange_distmax (2400) is offered to its class; the class
/// timer gates it, and the sting plays from the special.
const ALERT_IN_RANGE: f32 = 2400.;
/// music_dynamic_SI_close_distmax / music_dynamic_SI_far_distmin.
const ALERT_CLOSE: f32 = 1200.;
const ALERT_FAR: f32 = 1800.;
/// music_dynamic_PZ_interval_randmultmax 5, music_dynamic_specials_interval_beats 5,
/// music_dynamic_PZ_BPM 80.
const ALERT_MULT: i32 = 5;
const ALERT_BEATS: f32 = 5.;
const ALERT_BPM: f32 = 80.;
/// The cue and the class cooldown range (in RandomInt steps) for a distance
/// in units, or None out of range.
pub fn alert(class_name: &str, units: f32) -> Option<(String, i32, i32)> {
    if units > ALERT_IN_RANGE {
        return None;
    }
    let n = ALERT_MULT;
    Some(if units <= ALERT_CLOSE {
        (format!("Event.{class_name}AlertClose"), n, ((n as f64 * 1.4).round() as i32).max(1))
    } else if units <= ALERT_FAR {
        (format!("Event.{class_name}Alert"), n, (n * 2).max(2))
    } else {
        (format!("Event.{class_name}AlertFar"), n * 2, ((n as f64 * 2. * 2.5).round() as i32).max(3))
    })
}
/// Seconds until the class may alert again: RandomInt steps of the beat interval.
pub fn alert_cooldown(steps: i32) -> f32 {
    steps as f32 * ALERT_BEATS * 60. / ALERT_BPM
}
#[derive(Default)]
pub struct AlertTimers {
    ready: [f32; 6],
    seed: u32,
}
pub fn alerts(
    mut timers: Local<AlertTimers>,
    time: Res<Time>,
    s: Res<crate::game_state::Match>,
    bodies: Query<&crate::game::Body>,
    specials: Query<(&Transform, &crate::special_infected::ai::AiSpecial, &crate::game::Enemy)>,
    mut queue: ResMut<crate::game_audio::Queue>,
    director: Option<Res<crate::source_director::Director>>,
) {
    use crate::special_infected::SpecialClass;
    // OnAlert also needs the danger-atmosphere timer running (Music +0x1b4),
    // which starts once survivors leave the safe area.
    let danger = director.as_ref().is_none_or(|d| d.left_safe_area());
    if s.infected || s.dead || !danger {
        return;
    }
    let Some(body) = bodies.iter().next() else { return };
    let now = time.elapsed_secs();
    if timers.seed == 0 {
        timers.seed = 0x9e37_79b9;
    }
    let mut nearest: [Option<(f32, Vec3)>; 6] = [None; 6];
    for (transform, special, enemy) in &specials {
        if enemy.health <= 0. {
            continue;
        }
        let Some(i) = SpecialClass::MINIONS.iter().position(|c| *c == special.class) else { continue };
        let units = transform.translation.distance(body.feet) / crate::collision::UNIT;
        if nearest[i].is_none_or(|(d, _)| units < d) {
            nearest[i] = Some((units, transform.translation));
        }
    }
    for (i, found) in nearest.into_iter().enumerate() {
        let Some((units, position)) = found else { continue };
        if now < timers.ready[i] {
            continue;
        }
        let name = match SpecialClass::MINIONS[i] {
            SpecialClass::Hunter => "Hunter",
            SpecialClass::Smoker => "Smoker",
            SpecialClass::Boomer => "Boomer",
            SpecialClass::Spitter => "Spitter",
            SpecialClass::Jockey => "Jockey",
            _ => "Charger",
        };
        let Some((cue, lo, hi)) = alert(name, units) else { continue };
        let mut x = timers.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        timers.seed = x;
        let steps = if hi > lo { lo + (x % (hi - lo + 1) as u32) as i32 } else { lo };
        timers.ready[i] = now + alert_cooldown(steps);
        queue.0.push_back(crate::game_audio::Request {
            name: cue,
            position: Some(position),
            looping: false,
            gain: 1.,
            map_entity: None,
            soundlevel: None,
        });
    }
}
/// Round-end and safe-room cues:
/// - OnEnterCheckpoint 102ac620: Event.SafeRoom (the exit checkpoint).
/// - OnMissionLost 102afc50: Event.ScenarioLose when every survivor is down.
/// - OnMissionWon 102ac590: Event.ScenarioWin when the finale is won.
/// - OnFinaleFinalBattle 102ac530: Event.FinalBattle once the finale escape
///   starts (Director 10289e40 calls it for every living survivor, once).
/// Each also stops the survivor's other cues. L4D1 survivors use `_L4D1`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub checkpoint: bool,
    pub lost: bool,
    pub won: bool,
    pub escape: bool,
}
/// Event.FinalBattle; its MusicStopTrackList/MusicBlockTrackList hold the
/// horde drums while it plays.
pub(crate) const FINAL_MUSIC: usize = usize::MAX - 0x5a16;
pub fn outcome_cues(old: Outcome, new: Outcome, l4d1: bool) -> Vec<&'static str> {
    let v = |a: &'static str, b: &'static str| if l4d1 { b } else { a };
    let mut play = vec![];
    if new.won && !old.won {
        play.push(v("Event.ScenarioWin", "Event.ScenarioWin_L4D1"));
    } else if new.lost && !old.lost {
        play.push(v("Event.ScenarioLose", "Event.ScenarioLose_L4D1"));
    } else if new.checkpoint && !old.checkpoint && !new.lost {
        play.push(v("Event.SafeRoom", "Event.SafeRoom_L4D1"));
    } else if new.escape && !old.escape && !new.lost {
        play.push(v("Event.FinalBattle", "Event.FinalBattle_L4D1"));
    }
    play
}
pub fn outcomes(
    mut tracked: Local<Outcome>,
    mut commands: Commands,
    s: Res<crate::game_state::Match>,
    squad: Option<Res<crate::game_state::Squad>>,
    checkpoint: Option<Res<crate::campaign_flow::Checkpoint>>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    director: Option<Res<crate::source_director::Director>>,
    mut queue: ResMut<crate::game_audio::Queue>,
    playing: Query<(Entity, &crate::game_audio::MapSound)>,
) {
    if s.infected {
        *tracked = Outcome::default();
        return;
    }
    let down = |dead: bool, incap: bool| dead || incap;
    let now = Outcome {
        checkpoint: checkpoint.as_ref().is_some_and(|c| c.goal.is_some()),
        lost: down(s.dead, s.incap)
            && squad.as_ref().is_none_or(|q| q.0.iter().all(|m| down(m.state.dead, m.state.incap))),
        won: logic.as_ref().is_some_and(|l| l.world.cine.finale_won),
        escape: director.as_ref().and_then(|d| d.finale()).is_some_and(|f| f.escape),
    };
    let l4d1 = session.as_ref().is_some_and(|x| {
        use crate::campaign_settings::Survivor::*;
        matches!(x.survivor, Bill | Zoey | Francis | Louis)
    });
    let play = outcome_cues(*tracked, now, l4d1);
    *tracked = now;
    if play.is_empty() {
        return;
    }
    if now.won || now.lost {
        for (entity, sound) in &playing {
            if [PIN_MUSIC, SMASH_MUSIC, DOWN_MUSIC, VOMIT_MUSIC].contains(&sound.0) {
                commands.entity(entity).insert(crate::game_audio::FadeOut::new(PIN_FADE));
            }
        }
    }
    let final_battle = play.iter().any(|n| n.starts_with("Event.FinalBattle"));
    if final_battle {
        for (entity, sound) in &playing {
            if sound.0 == ZOMBAT_MUSIC {
                commands.entity(entity).insert(crate::game_audio::FadeOut::new(ZOMBAT_FADE));
            }
        }
    }
    for name in play {
        queue.0.push_back(crate::game_audio::Request {
            name: name.into(),
            position: None,
            looping: false,
            gain: 1.,
            map_entity: final_battle.then_some(FINAL_MUSIC),
            soundlevel: None,
        });
    }
}
/// Horde drums ("zombat"). CTerrorPlayer::CheckEnemySightings (1034ccb0,
/// every 0.1 s) reports the attacking commons it sees once more than
/// director_music_dynamic_mob_size (25) are attacking; Music::Update
/// (102b1710) then starts Event.Zombat_Intro_<RandomInt(1,11)>, whose
/// MusicAutoQueue chain is Zombat_N -> A_N -> B_N -> A_N ... It stops (2 s
/// fade) when intensity < 0.01, the survivor is down, a Tank is alive, a Witch
/// is attacking, or the
/// mob is down to director_music_dynamic_mobstop_size (8) attacking and
/// director_music_dynamic_scanmobstop_size (3) seen.
/// The campaign zombat2/zombat3 layers are `zombat_layers`.
const ZOMBAT_MUSIC: usize = usize::MAX - 0x5a15;
const ZOMBAT_START_MOB: usize = 25;
const ZOMBAT_STOP_MOB: usize = 8;
const ZOMBAT_STOP_SEEN: usize = 3;
const ZOMBAT_FADE: f32 = 2.;
const ZOMBAT_SIGHT_RANGE: f32 = 2500.;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Zombat {
    #[default]
    Off,
    Intro(u32),
    Main(u32),
    A(u32),
    B(u32),
}
impl Zombat {
    pub fn cue(self) -> Option<String> {
        Some(match self {
            Zombat::Off => return None,
            Zombat::Intro(n) => format!("Event.Zombat_Intro_{n}"),
            Zombat::Main(n) => format!("Event.Zombat_{n}"),
            Zombat::A(n) => format!("Event.Zombat_A_{n}"),
            Zombat::B(n) => format!("Event.Zombat_B_{n}"),
        })
    }
    /// The script's MusicAutoQueue successor.
    pub fn next(self) -> Self {
        match self {
            Zombat::Off => Zombat::Off,
            Zombat::Intro(n) => Zombat::Main(n),
            Zombat::Main(n) | Zombat::B(n) => Zombat::A(n),
            Zombat::A(n) => Zombat::B(n),
        }
    }
}
/// Start/stop decision for one check: (attacking, seen) counts.
pub fn zombat_should_play(playing: bool, attacking: usize, seen: usize, intensity: f32, blocked: bool) -> bool {
    if blocked {
        return false;
    }
    if !playing {
        return attacking > ZOMBAT_START_MOB && seen > 0;
    }
    !(intensity < 0.01 || attacking <= ZOMBAT_STOP_MOB && seen <= ZOMBAT_STOP_SEEN)
}
#[derive(Default)]
pub struct ZombatState {
    stage: Zombat,
    check: f32,
    /// The current cue was seen playing; when it is gone, queue the next.
    started: bool,
    requested: f32,
    seed: u32,
    /// Music mob intensity (Music +0x98) and when its decay reaches zero.
    mob_intensity: f32,
    decay_end: f32,
}
/// z_mob_music_size: seen attackers for full mob intensity.
const MOB_MUSIC_SIZE: f32 = 2.;
/// music_dynamic_CI_sight_decay: seconds to decay from full intensity.
const CI_SIGHT_DECAY: f32 = 8.;
/// Music::Update's mob intensity: a higher sighting raises it and restarts
/// a linear decay of CI_SIGHT_DECAY seconds per unit; otherwise it decays.
pub fn mob_intensity(current: f32, decay_end: f32, seen: usize, now: f32) -> (f32, f32) {
    let sighted = (seen as f32 / MOB_MUSIC_SIZE).clamp(0., 1.);
    if sighted > current {
        (sighted, now + sighted * CI_SIGHT_DECAY)
    } else {
        (((decay_end - now).max(0.) / CI_SIGHT_DECAY).clamp(0., 1.), decay_end)
    }
}
#[allow(clippy::too_many_arguments)]
pub fn zombat(
    mut z: Local<ZombatState>,
    mut commands: Commands,
    time: Res<Time>,
    s: Res<crate::game_state::Match>,
    bodies: Query<&crate::game::Body>,
    collision: Option<Res<crate::collision::Collision>>,
    enemies: Query<
        (&Transform, &crate::game::Enemy, Option<&crate::game::mind::Mind>),
        (Without<crate::special_infected::ai::AiSpecial>, Without<crate::special_infected::witch::Witch>),
    >,
    tanks: Query<(&crate::special_infected::ai::AiSpecial, &crate::game::Enemy)>,
    witches: Query<&crate::special_infected::witch::Witch>,
    mut queue: ResMut<crate::game_audio::Queue>,
    playing: Query<(Entity, &crate::game_audio::MapSound)>,
    mut status: ResMut<ZombatStatus>,
) {
    let now = time.elapsed_secs();
    let on = z.stage != Zombat::Off;
    status.stage = z.stage;
    status.mob_intensity = z.mob_intensity;
    let alive = playing.iter().any(|(_, sound)| sound.0 == ZOMBAT_MUSIC);
    let mut stop = false;
    if now >= z.check {
        z.check = now + 0.1;
        let tank = tanks.iter().any(|(ai, e)| ai.class == crate::special_infected::SpecialClass::Tank && e.health > 0.);
        // Music +0x186: a Witch is attacking (chasing or shredding).
        let witch = witches.iter().any(|w| {
            use crate::special_infected::witch::Phase;
            matches!(w.phase, Phase::Attack { .. } | Phase::Frenzy { .. })
        });
        let final_battle = playing.iter().any(|(_, sound)| sound.0 == FINAL_MUSIC);
        let blocked = s.infected || s.dead || s.incap || tank || witch || final_battle;
        let eye = bodies.iter().next().map(|b| b.feet + Vec3::Y * crate::collision::HEIGHT * 0.9);
        let mut attacking = 0;
        let mut seen = 0;
        for (transform, enemy, mind) in &enemies {
            if enemy.health <= 0. || mind.is_some_and(|m| m.state != crate::game::mind::State::Attack) {
                continue;
            }
            attacking += 1;
            let (Some(eye), Some(c)) = (eye, collision.as_ref()) else { continue };
            let target = transform.translation + Vec3::Y * 1.2;
            if seen <= ZOMBAT_STOP_SEEN
                && target.distance(eye) / crate::collision::UNIT <= ZOMBAT_SIGHT_RANGE
                && c.trace(eye, target, Vec3::ZERO).fraction >= 0.99
            {
                seen += 1;
            }
        }
        let (intensity, end) = mob_intensity(z.mob_intensity, z.decay_end, seen, now);
        z.mob_intensity = intensity;
        z.decay_end = end;
        let play = zombat_should_play(on, attacking, seen, intensity, blocked);
        if play && !on {
            if z.seed == 0 {
                z.seed = 0x2545_f491;
            }
            let mut x = z.seed;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            z.seed = x;
            z.stage = Zombat::Intro(1 + x % 11);
            z.started = false;
            z.requested = now;
            if let Some(cue) = z.stage.cue() {
                queue.0.push_back(crate::game_audio::Request {
                    name: cue,
                    position: None,
                    looping: false,
                    gain: 1.,
                    map_entity: Some(ZOMBAT_MUSIC),
                    soundlevel: None,
                });
            }
            return;
        }
        stop = on && !play;
        if stop && crate::game_audio::audit_enabled() {
            println!("Zombat stop: attacking={attacking} seen={seen} intensity={intensity:.3} blocked={blocked}");
        }
    }
    if !on {
        return;
    }
    if stop || !z.started && !alive && now - z.requested > 15. {
        // Stopped, or the cue never started (missing/unloadable).
        for (entity, sound) in &playing {
            if sound.0 == ZOMBAT_MUSIC {
                commands.entity(entity).insert(crate::game_audio::FadeOut::new(ZOMBAT_FADE));
            }
        }
        z.stage = Zombat::Off;
        return;
    }
    if alive {
        z.started = true;
    } else if z.started {
        // The cue finished: MusicAutoQueue the next one.
        z.stage = z.stage.next();
        z.started = false;
        z.requested = now;
        if let Some(cue) = z.stage.cue() {
            queue.0.push_back(crate::game_audio::Request {
                name: cue,
                position: None,
                looping: false,
                gain: 1.,
                map_entity: Some(ZOMBAT_MUSIC),
                soundlevel: None,
            });
        }
    }
}
/// The horde drum stage and music mob intensity, for `zombat_layers`.
#[derive(Resource, Default)]
pub struct ZombatStatus {
    pub stage: Zombat,
    pub mob_intensity: f32,
}
const ZOMBAT2_MUSIC: usize = usize::MAX - 0x5a17;
const ZOMBAT3_MUSIC: usize = usize::MAX - 0x5a18;
/// MusicMasterDivTags of the "zombat" master clips (Event.Zombat_N/A/B).
const MASTER_DIV_TAGS: f32 = 8.;
/// music_dynamic_attack_CI_close_distmax / _num, and the client's 0.42
/// full-volume point for nv_m_zombatMusic3 (102b0ce0).
const LAYER3_DISTMAX: f32 = 600.;
const LAYER3_NUM: f32 = 30.;
const LAYER3_FULL: f32 = 0.42;
/// The zombat3 cue chain: Intro (untagged) -> A -> B -> A ...
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layer3 {
    #[default]
    Off,
    Intro,
    A,
    B,
}
impl Layer3 {
    fn cue(self, postfix: &str) -> Option<String> {
        Some(match self {
            Layer3::Off => return None,
            Layer3::Intro => format!("Event.Zombat3_Intro_{postfix}"),
            Layer3::A => format!("Event.Zombat3_A_{postfix}"),
            Layer3::B => format!("Event.Zombat3_B_{postfix}"),
        })
    }
    fn next(self) -> Self {
        match self {
            Layer3::Off => Layer3::Off,
            Layer3::Intro | Layer3::B => Layer3::A,
            Layer3::A => Layer3::B,
        }
    }
    /// The cue's MusicTagDelay (game_sounds_music.txt): "1, 3" for
    /// Plankcountry, BigEasy and Milltown's B cue, else "1, 7".
    pub fn tag_delay(self, postfix: &str) -> (u32, u32) {
        let p = postfix.to_ascii_lowercase();
        if p == "plankcountry" || p == "bigeasy" || p == "milltown" && self == Layer3::B { (1, 3) } else { (1, 7) }
    }
}
/// A tag-synchronised start (client 10250390 / 1024d190 / 1024d370): the
/// master-clip time to start at, and whether that is in the next master
/// clip (the target wrapped past the clip's end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TagStart {
    pub at: f32,
    pub next_clip: bool,
}
/// Master tags are k x length / divisions (1024f360). With a tag index the
/// target is that tag (1024d370); otherwise the nearest tag strictly after
/// position + delay (1024d190). With no such tag the first tag is used, and
/// a target not ahead of the position is in the next pass of the master
/// (+ its length), so a delay past the last tag waits for the next clip.
pub fn tag_start(position: f32, length: f32, divisions: f32, delay: f32, index: Option<usize>) -> TagStart {
    let tags = divisions.max(1.) as usize;
    let step = length / tags as f32;
    let at = match index {
        Some(index) => step * index.min(tags - 1) as f32,
        None => {
            let want = position + delay;
            (0..tags).map(|k| step * k as f32).find(|t| *t > want).unwrap_or(0.)
        }
    };
    TagStart { at, next_clip: at < position }
}
/// nv_m_zombatMusic3: attacking commons near the player, each weighted by
/// closeness, over music_dynamic_attack_CI_num, scaled to full at 0.42.
pub fn layer3_gain(distances: impl Iterator<Item = f32>) -> f32 {
    let close: f32 = distances
        .filter(|d| *d < LAYER3_DISTMAX)
        .map(|d| (1. - d / LAYER3_DISTMAX) / LAYER3_NUM)
        .sum();
    (close / LAYER3_FULL).clamp(0., 1.)
}
/// nv_m_zombatMusic2: the music mob intensity doubled.
pub fn layer2_gain(mob_intensity: f32) -> f32 {
    (mob_intensity * 2.).clamp(0., 1.)
}
/// Seconds of PCM audio in a RIFF wave (0 when unknown).
pub fn wave_seconds(bytes: &[u8]) -> f32 {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return 0.;
    }
    let mut at = 12;
    let mut byte_rate = 0u32;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = at + 8;
        if id == b"fmt " && body + 12 <= bytes.len() {
            byte_rate = u32::from_le_bytes(bytes[body + 8..body + 12].try_into().unwrap());
        } else if id == b"data" && byte_rate > 0 {
            return size as f32 / byte_rate as f32;
        }
        at = body + size + (size & 1);
    }
    0.
}
#[derive(Default)]
pub struct LayerState {
    postfix: Option<String>,
    map: usize,
    /// zombat2: waiting for the next master clip, then playing.
    layer2_waiting: bool,
    layer2_on: bool,
    layer3: Layer3,
    layer3_started: bool,
    layer3_requested: f32,
    /// A pending tagged zombat3 cue: its target and the master clip it was
    /// scheduled against.
    layer3_pending: Option<(TagStart, Option<Entity>)>,
    seed: u32,
    /// The master clip last seen (entity) and its length.
    master: Option<(Entity, f32)>,
}
impl LayerState {
    fn random(&mut self, lo: u32, hi: u32) -> u32 {
        if self.seed == 0 {
            self.seed = 0x9e37_79b9;
        }
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        lo + x % (hi - lo + 1)
    }
}
fn layer_request(name: String, id: usize) -> crate::game_audio::Request {
    crate::game_audio::Request { name, position: None, looping: false, gain: 1., map_entity: Some(id), soundlevel: None }
}
/// Whether a pending tagged cue should start now, on the current master
/// clip (`entity`, `position` seconds in).
fn tag_ready(target: TagStart, scheduled_on: Option<Entity>, entity: Entity, position: f32) -> bool {
    if Some(entity) == scheduled_on {
        !target.next_clip && position >= target.at
    } else if target.next_clip {
        position >= target.at
    } else {
        // Scheduled within a clip that ended early: start on the new one.
        true
    }
}
/// The campaign zombat layers (Event.Zombat2_Intro_<musicpostfix>,
/// Event.Zombat3_<Intro/A/B>_<musicpostfix>). The server starts them with
/// the horde drums (Music::Update 102b1710); the client mixes their volume
/// from nv_m_zombatMusic2/3 and starts tagged cues on the master "zombat"
/// clip's beat tags (MusicMasterDivTags 8). The L4D1-survivor and The
/// Passing variants are not handled.
#[allow(clippy::too_many_arguments)]
pub fn zombat_layers(
    mut l: Local<LayerState>,
    mut commands: Commands,
    time: Res<Time>,
    status: Res<ZombatStatus>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    bodies: Query<&crate::game::Body>,
    enemies: Query<
        (&Transform, &crate::game::Enemy, Option<&crate::game::mind::Mind>),
        (Without<crate::special_infected::ai::AiSpecial>, Without<crate::special_infected::witch::Witch>),
    >,
    mut queue: ResMut<crate::game_audio::Queue>,
    sources: Res<Assets<bevy::audio::AudioSource>>,
    playing: Query<(
        Entity,
        &crate::game_audio::MapSound,
        Option<&crate::game_audio::Voice>,
        Option<&AudioPlayer>,
        Option<&crate::adrenaline::LayerGain>,
    )>,
) {
    let Some(logic) = logic else { return };
    let map = std::sync::Arc::as_ptr(&logic.world.bsp) as usize;
    if l.map != map {
        let seed = l.seed;
        *l = LayerState { map, seed, ..default() };
        l.postfix = logic
            .world
            .ents
            .iter()
            .find(|e| e.me.classname == "worldspawn")
            .and_then(|e| e.me.kv("musicpostfix"))
            .filter(|p| !p.is_empty())
            .map(str::to_string);
    }
    let Some(postfix) = l.postfix.clone() else { return };
    let now = time.elapsed_secs();
    let on = status.stage != Zombat::Off;
    let fade = |commands: &mut Commands, id: usize| {
        for (entity, sound, ..) in &playing {
            if sound.0 == id {
                commands.entity(entity).insert(crate::game_audio::FadeOut::new(ZOMBAT_FADE));
            }
        }
    };
    if !on {
        if l.layer2_on || l.layer2_waiting {
            fade(&mut commands, ZOMBAT2_MUSIC);
        }
        if l.layer3 != Layer3::Off {
            fade(&mut commands, ZOMBAT3_MUSIC);
        }
        l.layer2_on = false;
        l.layer2_waiting = false;
        l.layer3 = Layer3::Off;
        l.layer3_pending = None;
        l.master = None;
        return;
    }
    // Start: zombat3's untagged intro plays now; zombat2 (MusicTagIndex 0)
    // waits for the master clip's first tag.
    if l.layer3 == Layer3::Off && !l.layer2_on && !l.layer2_waiting {
        l.layer2_waiting = true;
        l.layer3 = Layer3::Intro;
        l.layer3_started = false;
        l.layer3_requested = now;
        if let Some(cue) = l.layer3.cue(&postfix) {
            queue.0.push_back(layer_request(cue, ZOMBAT3_MUSIC));
        }
    }
    // The master "zombat" clip (Zombat_N / A / B; the intro is its own track).
    let master_stage = matches!(status.stage, Zombat::Main(_) | Zombat::A(_) | Zombat::B(_));
    let master = playing
        .iter()
        .find(|(_, sound, voice, ..)| sound.0 == ZOMBAT_MUSIC && voice.is_some() && master_stage)
        .map(|(entity, _, voice, player, _)| {
            let length = match l.master {
                Some((e, len)) if e == entity && len > 0. => len,
                _ => player.and_then(|p| sources.get(&p.0)).map_or(0., |a| wave_seconds(&a.bytes)),
            };
            (entity, (now as f64 - voice.map_or(now as f64, |v| v.1)).max(0.) as f32, length)
        });
    let new_clip = match (master, l.master) {
        (Some((e, ..)), Some((old, _))) => e != old,
        (Some(_), None) => true,
        _ => false,
    };
    l.master = master.map(|(e, _, len)| (e, len));
    if l.layer2_waiting && new_clip {
        l.layer2_waiting = false;
        l.layer2_on = true;
        queue.0.push_back(layer_request(format!("Event.Zombat2_Intro_{postfix}"), ZOMBAT2_MUSIC));
    }
    // zombat3 chain (MusicAutoQueue Intro -> A -> B -> A ...).
    let layer3_alive = playing.iter().any(|(_, sound, ..)| sound.0 == ZOMBAT3_MUSIC);
    if l.layer3 != Layer3::Off && l.layer3_pending.is_none() {
        if layer3_alive {
            l.layer3_started = true;
        } else if l.layer3_started {
            // Finished: the next cue waits RandomInt(MusicTagDelay "1, 7")
            // seconds, then the next master tag.
            l.layer3 = l.layer3.next();
            l.layer3_started = false;
            let (lo, hi) = l.layer3.tag_delay(&postfix);
            let delay = l.random(lo, hi) as f32;
            let target = match master {
                Some((_, position, length)) if length > 0. => tag_start(position, length, MASTER_DIV_TAGS, delay, None),
                _ => TagStart { at: 0., next_clip: true },
            };
            l.layer3_pending = Some((target, master.map(|(e, ..)| e)));
        } else if now - l.layer3_requested > 15. {
            // The cue never started (none for this campaign): stop the chain.
            l.layer3 = Layer3::Off;
        }
    }
    if let (Some((target, scheduled_on)), Some((entity, position, _))) = (l.layer3_pending, master) {
        if tag_ready(target, scheduled_on, entity, position) {
            l.layer3_pending = None;
            l.layer3_requested = now;
            if let Some(cue) = l.layer3.cue(&postfix) {
                queue.0.push_back(layer_request(cue, ZOMBAT3_MUSIC));
            }
        }
    }
    // Live volumes.
    let gain2 = layer2_gain(status.mob_intensity);
    let feet = bodies.iter().next().map(|b| b.feet);
    let gain3 = feet.map_or(0., |feet| {
        layer3_gain(
            enemies
                .iter()
                .filter(|(_, e, m)| e.health > 0. && m.is_none_or(|m| m.state == crate::game::mind::State::Attack))
                .map(|(t, ..)| t.translation.distance(feet) / crate::collision::UNIT),
        )
    });
    for (entity, sound, _, _, gain) in &playing {
        let want = match sound.0 {
            ZOMBAT2_MUSIC => gain2,
            ZOMBAT3_MUSIC => gain3,
            _ => continue,
        };
        if gain.is_none_or(|g| (g.0 - want).abs() > 1e-3) {
            commands.entity(entity).insert(crate::adrenaline::LayerGain(want));
        }
    }
}
/// Newly exposed nav area (Source units squared) from the player's last
/// area change, waiting for `reveals`.
static REVEALED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub fn area_revealed(area: f32) {
    if area > 0. {
        REVEALED.store(area.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }
}
/// music_large_area_reveal_threshold (500000 square units, strictly more).
pub const REVEAL_THRESHOLD: f32 = 500000.;
/// music_large_area_reveal_repeat_threshold (60 s between performances).
pub const REVEAL_REPEAT: f32 = 60.;
/// Whether a reveal of `area` plays now, given the next allowed time.
pub fn reveal_plays(area: f32, now: f32, ready: f32) -> bool {
    area > REVEAL_THRESHOLD && ready <= now
}
/// Music::OnLargeAreaRevealed (server 102ae950), called by the AAS update
/// (10335740) when a survivor's area change exposes more than the
/// threshold: Event.LargeAreaRevealed (`_L4D1` for L4D1 survivors), then
/// the repeat timer restarts.
pub fn reveals(
    mut ready: Local<f32>,
    time: Res<Time>,
    s: Res<crate::game_state::Match>,
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    mut queue: ResMut<crate::game_audio::Queue>,
) {
    let area = f32::from_bits(REVEALED.swap(0, std::sync::atomic::Ordering::Relaxed));
    let now = time.elapsed_secs();
    if s.infected || s.dead || !reveal_plays(area, now, *ready) {
        return;
    }
    let l4d1 = session.as_ref().is_some_and(|x| {
        use crate::campaign_settings::Survivor::*;
        matches!(x.survivor, Bill | Zoey | Francis | Louis)
    });
    queue.0.push_back(crate::game_audio::Request {
        name: if l4d1 { "Event.LargeAreaRevealed_L4D1" } else { "Event.LargeAreaRevealed" }.into(),
        position: None,
        looping: false,
        gain: 1.,
        map_entity: None,
        soundlevel: None,
    });
    *ready = now + REVEAL_REPEAT;
}
pub fn install(app: &mut App) {
    app.init_resource::<ZombatStatus>().add_systems(
        Update,
        (update, alerts, outcomes, (zombat, zombat_layers).chain(), reveals).run_if(crate::game_loading::visible),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grab_cues_follow_retail_music_events() {
        let (play, fade) = pin_change(Pin::None, Pin::Pounce);
        assert_eq!(play, vec![("Event.HunterHit", None), ("Event.HunterPounce", Some(PIN_MUSIC))]);
        assert!(fade.is_empty());
        // Drag to choke keeps the drag cue and adds the choke.
        let (play, fade) = pin_change(Pin::Drag, Pin::Choke);
        assert_eq!(play[1].0, "Event.SmokerChoke");
        assert!(fade.is_empty());
        // Carry to pummel fades the smash cue over 1 s.
        let (play, fade) = pin_change(Pin::Smash, Pin::Slam);
        assert_eq!(play[1].0, "Event.ChargerSlam");
        assert_eq!(fade, vec![(SMASH_MUSIC, 1.)]);
        // Release fades the grab cues.
        let (play, fade) = pin_change(Pin::Ride, Pin::None);
        assert!(play.is_empty());
        assert!(fade.contains(&(PIN_MUSIC, PIN_FADE)));
    }
    #[test]
    fn zombat_chain_and_thresholds_follow_retail() {
        let mut z = Zombat::Intro(4);
        let mut names = vec![];
        for _ in 0..5 {
            names.push(z.cue().unwrap());
            z = z.next();
        }
        assert_eq!(names, ["Event.Zombat_Intro_4", "Event.Zombat_4", "Event.Zombat_A_4", "Event.Zombat_B_4", "Event.Zombat_A_4"]);
        // Starts only above 25 attacking with at least one seen.
        assert!(!zombat_should_play(false, 25, 5, 1., false));
        assert!(zombat_should_play(false, 26, 1, 1., false));
        assert!(!zombat_should_play(false, 40, 0, 1., false));
        // Keeps playing until the mob is small and mostly unseen.
        assert!(zombat_should_play(true, 8, 4, 1., false));
        assert!(!zombat_should_play(true, 8, 3, 1., false));
        assert!(!zombat_should_play(true, 30, 10, 0.005, false));
        assert!(!zombat_should_play(true, 30, 10, 1., true));
        // Two seen attackers give full intensity, decaying over 8 s.
        let (v, end) = mob_intensity(0., 0., 2, 10.);
        assert_eq!((v, end), (1., 18.));
        let (v, _) = mob_intensity(v, end, 0, 14.);
        assert!((v - 0.5).abs() < 1e-6);
    }
    #[test]
    fn round_end_cues_play_once() {
        let none = Outcome::default();
        let inside = Outcome { checkpoint: true, ..none };
        assert_eq!(outcome_cues(none, inside, false), ["Event.SafeRoom"]);
        assert!(outcome_cues(inside, inside, false).is_empty());
        let lost = Outcome { lost: true, ..none };
        assert_eq!(outcome_cues(none, lost, true), ["Event.ScenarioLose_L4D1"]);
        let won = Outcome { won: true, ..inside };
        assert_eq!(outcome_cues(inside, won, false), ["Event.ScenarioWin"]);
        let escape = Outcome { escape: true, ..none };
        assert_eq!(outcome_cues(none, escape, false), ["Event.FinalBattle"]);
        assert_eq!(outcome_cues(none, escape, true), ["Event.FinalBattle_L4D1"]);
        assert!(outcome_cues(escape, escape, false).is_empty());
    }
    #[test]
    fn special_alerts_use_retail_ranges_and_cooldowns() {
        assert_eq!(alert("Hunter", 800.), Some(("Event.HunterAlertClose".into(), 5, 7)));
        assert_eq!(alert("Smoker", 1500.), Some(("Event.SmokerAlert".into(), 5, 10)));
        assert_eq!(alert("Jockey", 2000.), Some(("Event.JockeyAlertFar".into(), 10, 25)));
        assert_eq!(alert("Boomer", 2401.), None);
        assert!((alert_cooldown(5) - 18.75).abs() < 1e-4);
    }
    #[test]
    fn down_bleeding_and_death_cues_follow_retail() {
        let off = (false, false, false);
        let (play, _) = life_change(off, (true, false, false), true, false);
        assert_eq!(play[1], ("Event.Down", Some(DOWN_MUSIC)));
        // Bleeding out replaces the down cue on the same track.
        let (play, fade) = life_change((true, false, false), (true, true, false), true, true);
        assert_eq!(play, vec![("Event.BleedingOutHit_L4D1", None), ("Event.BleedingOut_L4D1", Some(DOWN_MUSIC))]);
        assert_eq!(fade, vec![(DOWN_MUSIC, 0.)]);
        // Death while bleeding out with teammates alive.
        let (play, _) = life_change((true, true, false), (false, false, true), true, false);
        let names: Vec<_> = play.iter().map(|p| p.0).collect();
        assert_eq!(names, ["Event.BleedingOutEnd", "Event.SurvivorDeathHit", "Event.SurvivorDeath"]);
        // No death cue when the whole team is gone.
        let (play, _) = life_change((true, false, false), (false, false, true), false, false);
        assert!(play.is_empty());
        // Revive from bleeding out fades over one second.
        let (_, fade) = life_change((true, true, false), off, true, false);
        assert_eq!(fade, vec![(DOWN_MUSIC, 1.)]);
    }
    #[test]
    fn zombat_layers_follow_the_retail_tags_and_mix() {
        // 8 tags on a 16 s master: every 2 s.
        assert_eq!(tag_start(3., 16., 8., 2., None), TagStart { at: 6., next_clip: false });
        // Strictly after: 4 + 2 = 6 lands on a tag, so the next one (8).
        assert_eq!(tag_start(4., 16., 8., 2., None), TagStart { at: 8., next_clip: false });
        // Past the last tag: the first tag of the next pass.
        assert_eq!(tag_start(13., 16., 8., 5., None), TagStart { at: 0., next_clip: true });
        assert_eq!(tag_start(15., 16., 8., 1., None), TagStart { at: 0., next_clip: true });
        assert_eq!(tag_start(5., 16., 8., 0., Some(0)), TagStart { at: 0., next_clip: true });
        // The drum chain: every horde drum clip is at most 5.65 s, under its
        // 7 s MusicTagDelay, so the next cue waits for the next pass, i.e.
        // the end of the current clip, as the chain already plays.
        assert_eq!(tag_start(0.5, 5.65, 8., 7., None), TagStart { at: 0., next_clip: true });
        assert!((layer2_gain(0.25) - 0.5).abs() < 1e-6);
        assert_eq!(layer2_gain(0.8), 1.);
        // Six commons at 300 units: 6 x 0.5 / 30 = 0.1 -> 0.1 / 0.42.
        assert!((layer3_gain([300.; 6].into_iter()) - 0.1 / 0.42).abs() < 1e-5);
        assert_eq!(layer3_gain([700.; 40].into_iter()), 0.);
        assert_eq!(Layer3::A.tag_delay("Mall"), (1, 7));
        assert_eq!(Layer3::A.tag_delay("Milltown"), (1, 7));
        assert_eq!(Layer3::B.tag_delay("Milltown"), (1, 3));
        assert_eq!(Layer3::A.tag_delay("BigEasy"), (1, 3));
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.extend(16u32.to_le_bytes());
        wav.extend([1, 0, 2, 0]);
        wav.extend(44100u32.to_le_bytes());
        wav.extend((44100u32 * 4).to_le_bytes());
        wav.extend([4, 0, 16, 0]);
        wav.extend(b"data");
        wav.extend((44100u32 * 4 * 3).to_le_bytes());
        assert!((wave_seconds(&wav) - 3.).abs() < 1e-6);
    }
    #[test]
    fn large_reveals_play_over_the_threshold_at_most_once_a_minute() {
        assert!(!reveal_plays(500000., 0., 0.));
        assert!(reveal_plays(500001., 0., 0.));
        assert!(!reveal_plays(900000., 30., 60.));
        assert!(reveal_plays(900000., 60., 60.));
    }
}
